//! `atomik.gate-processor`: turns gates into triggers and back, divides, delays and lengthens
//! them, and combines two gates with logic.
//!
//! # Signal flow
//!
//! ```text
//! gate_in, gate_b_in -> logic -> divide -> delay -> length -> gate_out
//!                                                                 `-> trig_out (1 ms pulse per rise)
//! ```
//!
//! Every input is a gate with hysteresis: high above 0.5, low below 0.3.
//!
//! * **Logic.** `logic` combines `gate_in` (A) with `gate_b_in` (B): `a` (B ignored), `and`, `or`,
//!   `xor`, `nand`, `nor`. An unconnected B counts as low. A gate that is already high at the
//!   first sample after the device starts is not an edge but the starting state, so an idle
//!   `nand` or `nor` does not fire at the start (the delay line starts out as that state too).
//! * **Divide.** `divide` (1..16) passes the first of every N rising edges and its whole gate,
//!   and drops the others. 1 passes everything.
//! * **Delay.** `delay` (0..2000 ms) shifts the divided gate later by that time, edges and
//!   length together. With `delay.sync` on, the delay is the note value `delay.note` (1/64 to four
//!   bars, triplet and dotted, see below) of the transport tempo instead of `delay`. A change takes
//!   effect at once (a gate that is high while the delay moves can come out shorter or longer; a
//!   tempo ramp moves a synced delay the same way). The delay is a deliberate effect: the device
//!   reports no latency.
//! * **Length.** `length` 0 leaves the gate as long as it was. Above 0 it replaces the gate by
//!   one of exactly that many ms (at least one sample) from each rising edge, so a trigger becomes
//!   a gate of a set length; an edge inside a running gate restarts it. With `length.sync` on, the
//!   length is the note value `length.note` of the transport tempo and always replaces the gate
//!   (a note value is never zero).
//! * **Note values.** 1/64T, 1/64, 1/64D (triplet, plain, dotted) up to 1/1D, then `1 bar`, `2 bars`
//!   and `4 bars` of the time signature at the batch; a whole note is four beats. A synced time
//!   follows the tempo and the meter of each batch, rounded to a whole number of samples, and
//!   is kept inside the delay line.
//! * **Outputs.** `gate_out` is the result; `trig_out` is a 1 ms pulse at every rising edge of
//!   `gate_out` (a gate to trigger converter).
//! * The delay line is a bit per sample, allocated when the device is prepared (a minute at the
//!   sample rate; a synced delay longer than that is cut to it). Non-finite input counts as low;
//!   allocation-free while running.

use crate::tool_support::{
    GateIn, NOTE_1_8, NOTE_VALUES, beats_to_samples, choice, count, cv_in, cv_out, finite, linear,
    note_value_beats, pulse_samples, toggle,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};

const P_LOGIC: u32 = 0;
const P_DIVIDE: u32 = 1;
const P_DELAY: u32 = 2;
const P_LENGTH: u32 = 3;
const P_DELAY_SYNC: u32 = 4;
const P_DELAY_NOTE: u32 = 5;
const P_LENGTH_SYNC: u32 = 6;
const P_LENGTH_NOTE: u32 = 7;

const GATE_IN: SocketId = SocketId(0);
const GATE_B_IN: SocketId = SocketId(1);
const GATE_OUT: SocketId = SocketId(2);
const TRIG_OUT: SocketId = SocketId(3);

/// The longest delay and length in ms.
const MAX_MS: f32 = 2000.0;
/// The delay line holds this many seconds (the longest synced delay).
const RING_SECONDS: f64 = 60.0;

static LOGICS: [&str; 6] = ["a", "and", "or", "xor", "nand", "nor"];

static PARAMS: [ParamDescriptor; 8] = [
    choice(P_LOGIC, "logic", "Logic", &LOGICS, 0.0),
    count(P_DIVIDE, "divide", "Divide", 1.0, 16.0, 1.0),
    linear(P_DELAY, "delay", "Delay", 0.0, MAX_MS, 0.0, Unit::Ms),
    linear(P_LENGTH, "length", "Length", 0.0, MAX_MS, 0.0, Unit::Ms),
    toggle(P_DELAY_SYNC, "delay.sync", "Delay sync", 0.0),
    choice(
        P_DELAY_NOTE,
        "delay.note",
        "Delay note",
        &NOTE_VALUES,
        NOTE_1_8,
    ),
    toggle(P_LENGTH_SYNC, "length.sync", "Length sync", 0.0),
    choice(
        P_LENGTH_NOTE,
        "length.note",
        "Length note",
        &NOTE_VALUES,
        NOTE_1_8,
    ),
];

static SOCKETS: [SocketDescriptor; 4] = [
    cv_in(0, "gate_in", "Gate A in"),
    cv_in(1, "gate_b_in", "Gate B in"),
    cv_out(2, "gate_out", "Gate out"),
    cv_out(3, "trig_out", "Trigger out"),
];

/// Static description of Gate Processor.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.gate-processor",
    name: "Gate Processor",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`GateProcessor`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(GateProcessor::new()),
    }
}

/// The gate and trigger processor.
#[derive(Debug, Clone)]
pub struct GateProcessor {
    logic: u8,
    divide: u32,
    delay_ms: f32,
    length_ms: f32,
    /// The delay and the length follow the tempo (note values) instead of ms.
    delay_sync: bool,
    length_sync: bool,
    delay_note: usize,
    length_note: usize,
    sample_rate: f32,
    a: GateIn,
    b: GateIn,
    /// The combined gate of the last sample (`None` before the first one).
    combined: Option<bool>,
    /// Rising edges counted by the divider.
    edges: u32,
    /// Whether the current gate passes the divider.
    passing: bool,
    /// The divided gate, a ring of the last `ring_len` samples, one bit each.
    ring: Vec<u64>,
    ring_len: usize,
    write: usize,
    /// The delay and length in samples of the ms values.
    delay_ms_samples: usize,
    length_ms_samples: u32,
    /// The delay and length in force (the ms values or the note values of the last batch).
    delay_samples: usize,
    length_samples: u32,
    delayed_prev: Option<bool>,
    length_left: u32,
    out_prev: Option<bool>,
    pulse_len: u32,
    pulse_left: u32,
}

impl GateProcessor {
    fn new() -> Self {
        let mut g = Self {
            logic: 0,
            divide: 1,
            delay_ms: 0.0,
            length_ms: 0.0,
            delay_sync: false,
            length_sync: false,
            delay_note: NOTE_1_8 as usize,
            length_note: NOTE_1_8 as usize,
            sample_rate: 48_000.0,
            a: GateIn::default(),
            b: GateIn::default(),
            combined: None,
            edges: 0,
            passing: true,
            ring: Vec::new(),
            ring_len: 0,
            write: 0,
            delay_ms_samples: 0,
            length_ms_samples: 0,
            delay_samples: 0,
            length_samples: 0,
            delayed_prev: None,
            length_left: 0,
            out_prev: None,
            pulse_len: pulse_samples(48_000.0),
            pulse_left: 0,
        };
        g.derive();
        g
    }

    /// The delay and length of the ms values, in samples; the values in force follow them until a
    /// synced one replaces them for a batch.
    fn derive(&mut self) {
        let samples = |ms: f32| (f64::from(ms) * 0.001 * f64::from(self.sample_rate)).round();
        self.delay_ms_samples = (samples(self.delay_ms) as usize).min(self.max_delay());
        self.length_ms_samples = if self.length_ms > 0.0 {
            (samples(self.length_ms) as u32).max(1)
        } else {
            0
        };
        self.delay_samples = self.delay_ms_samples;
        self.length_samples = self.length_ms_samples;
    }

    /// The longest delay the line holds, in samples.
    fn max_delay(&self) -> usize {
        self.ring_len.saturating_sub(1)
    }

    /// The delay and length in force for a batch: the ms values, or the note values at the tempo
    /// and meter of the batch.
    fn follow_tempo(&mut self, bpm: f64, meter: (u8, u8)) {
        let sr = self.sample_rate;
        self.delay_samples = if self.delay_sync {
            let beats = note_value_beats(self.delay_note, meter);
            (beats_to_samples(beats, bpm, sr) as usize).min(self.max_delay())
        } else {
            self.delay_ms_samples
        };
        self.length_samples = if self.length_sync {
            let beats = note_value_beats(self.length_note, meter);
            (beats_to_samples(beats, bpm, sr) as u32).max(1)
        } else {
            self.length_ms_samples
        };
    }

    fn ring_bit(&self, i: usize) -> bool {
        self.ring[i >> 6] >> (i & 63) & 1 != 0
    }

    fn set_ring_bit(&mut self, i: usize, on: bool) {
        let word = &mut self.ring[i >> 6];
        let bit = 1u64 << (i & 63);
        if on {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    fn fill_ring(&mut self, on: bool) {
        self.ring.fill(if on { u64::MAX } else { 0 });
    }
}

impl Device for GateProcessor {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.pulse_len = pulse_samples(self.sample_rate);
        let len = (RING_SECONDS * info.sample_rate.hz()).ceil() as usize + 1;
        self.ring_len = len;
        self.ring.clear();
        self.ring.resize(len.div_ceil(64), 0);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
        self.combined = None;
        self.edges = 0;
        self.passing = true;
        self.fill_ring(false);
        self.write = 0;
        self.delayed_prev = None;
        self.length_left = 0;
        self.out_prev = None;
        self.pulse_left = 0;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_LOGIC => self.logic = v.clamp(0.0, (LOGICS.len() - 1) as f32).round() as u8,
            P_DIVIDE => self.divide = v.clamp(1.0, 16.0).round() as u32,
            P_DELAY => {
                self.delay_ms = v.clamp(0.0, MAX_MS);
                self.derive();
            }
            P_LENGTH => {
                self.length_ms = v.clamp(0.0, MAX_MS);
                self.derive();
            }
            P_DELAY_SYNC => {
                self.delay_sync = v >= 0.5;
                self.derive();
            }
            P_DELAY_NOTE => {
                self.delay_note = v.clamp(0.0, (NOTE_VALUES.len() - 1) as f32).round() as usize;
            }
            P_LENGTH_SYNC => {
                self.length_sync = v >= 0.5;
                self.derive();
            }
            P_LENGTH_NOTE => {
                self.length_note = v.clamp(0.0, (NOTE_VALUES.len() - 1) as f32).round() as usize;
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let ga = *ctx.input(GATE_IN);
        let gb = *ctx.input(GATE_B_IN);
        let b_cabled = ctx.is_connected(GATE_B_IN);
        let mut gate = [0.0f32; BATCH];
        let mut trig = [0.0f32; BATCH];
        if self.delay_sync || self.length_sync {
            self.follow_tempo(ctx.time.tempo.bpm(), ctx.time.time_signature);
        }
        let ring_len = self.ring_len;
        for n in 0..BATCH {
            let a = self.a.update(ga[n]);
            let b = self.b.update(gb[n]) && b_cabled;
            let c = match self.logic {
                0 => a,
                1 => a && b,
                2 => a || b,
                3 => a != b,
                4 => !(a && b),
                _ => !(a || b),
            };
            let before = self.combined.unwrap_or(c);
            if self.combined.is_none() {
                // The first sample: a gate that is already high is the state, not an edge.
                self.fill_ring(c);
            }
            self.combined = Some(c);

            // Divider: the first of every `divide` rising edges passes with its whole gate.
            if c && !before {
                let slot = self.edges % self.divide;
                self.passing = slot == 0;
                self.edges = slot + 1;
            }
            let divided = c && self.passing;

            // Delay.
            let delayed = if ring_len == 0 {
                divided
            } else {
                self.set_ring_bit(self.write, divided);
                let read = (self.write + ring_len - self.delay_samples) % ring_len;
                let d = self.ring_bit(read);
                self.write = (self.write + 1) % ring_len;
                d
            };

            // Length.
            let rose = delayed && !self.delayed_prev.unwrap_or(delayed);
            self.delayed_prev = Some(delayed);
            let high = if self.length_samples > 0 {
                if rose {
                    self.length_left = self.length_samples;
                }
                let h = self.length_left > 0;
                self.length_left = self.length_left.saturating_sub(1);
                h
            } else {
                delayed
            };

            if high && !self.out_prev.unwrap_or(high) {
                self.pulse_left = self.pulse_len;
            }
            self.out_prev = Some(high);
            gate[n] = if high { 1.0 } else { 0.0 };
            if self.pulse_left > 0 {
                trig[n] = 1.0;
                self.pulse_left -= 1;
            }
        }
        *ctx.output(GATE_OUT) = gate;
        *ctx.output(TRIG_OUT) = trig;
    }
}

#[cfg(test)]
#[allow(
    clippy::needless_range_loop,
    clippy::float_cmp,
    clippy::type_complexity
)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;

    /// Runs the gate waveform `a` (and `b` when given) through the device in whole batches and
    /// returns `(gate_out, trig_out)`. The waveform is padded with zeros to whole batches.
    fn run(
        h: &mut DeviceHarness,
        a: &[f32],
        b: Option<&[f32]>,
        extra: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let total = (a.len() + extra).div_ceil(BATCH) * BATCH;
        let pad = |x: &[f32]| {
            let mut v = x.to_vec();
            v.resize(total, 0.0);
            v
        };
        let (a, b) = (pad(a), b.map(pad));
        let mut gate = Vec::new();
        let mut trig = Vec::new();
        for k in 0..total / BATCH {
            let range = k * BATCH..(k + 1) * BATCH;
            h.input_mut("gate_in").copy_from_slice(&a[range.clone()]);
            if let Some(b) = &b {
                h.input_mut("gate_b_in").copy_from_slice(&b[range]);
            }
            h.process();
            gate.extend_from_slice(h.output("gate_out"));
            trig.extend_from_slice(h.output("trig_out"));
        }
        (gate, trig)
    }

    /// `n` samples high starting at `at`, in a buffer of `len`.
    fn gate_at(len: usize, at: usize, n: usize) -> Vec<f32> {
        let mut v = vec![0.0; len];
        v[at..at + n].fill(1.0);
        v
    }

    fn rises(x: &[f32]) -> Vec<usize> {
        (0..x.len())
            .filter(|i| x[*i] >= 0.5 && (*i == 0 || x[*i - 1] < 0.5))
            .collect()
    }

    fn highs(x: &[f32]) -> usize {
        x.iter().filter(|v| **v >= 0.5).count()
    }

    #[test]
    fn descriptor_lists_the_keys_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "logic",
                "divide",
                "delay",
                "length",
                "delay.sync",
                "delay.note",
                "length.sync",
                "length.note",
            ],
            &["gate_in", "gate_b_in", "gate_out", "trig_out"],
        );
    }

    #[test]
    fn defaults_pass_the_gate_through_and_make_a_trigger_at_each_rise() {
        let mut h = harness(Box::new(GateProcessor::new()));
        let mut a = gate_at(512, 100, 120);
        a[300..400].fill(1.0);
        let (gate, trig) = run(&mut h, &a, None, 0);
        assert_eq!(gate, a);
        assert_eq!(rises(&trig), vec![100, 300]);
        // The pulse is 1 ms: 48 samples at 48 kHz, whatever the gate length.
        assert_eq!(highs(&trig), 96);
        assert_eq!(trig[100 + 47], 1.0);
        assert_eq!(trig[100 + 48], 0.0);
    }

    #[test]
    fn a_gate_stays_high_through_hysteresis_and_ignores_small_ripples() {
        let mut h = harness(Box::new(GateProcessor::new()));
        let mut a = vec![0.0f32; 128];
        a[10..40].fill(1.0);
        a[20..25].fill(0.4); // between the thresholds: still high
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(highs(&gate[10..40]), 30);
        assert_eq!(gate[40], 0.0);
    }

    #[test]
    fn length_turns_a_trigger_into_a_gate_of_a_set_length() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("length", 2.0); // 96 samples
        let a = gate_at(512, 50, 3);
        let (gate, trig) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![50]);
        assert_eq!(highs(&gate), 96);
        assert_eq!(gate[50 + 95], 1.0);
        assert_eq!(gate[50 + 96], 0.0);
        assert_eq!(rises(&trig), vec![50]);
    }

    #[test]
    fn a_new_edge_restarts_a_running_length() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("length", 2.0);
        let mut a = gate_at(512, 50, 3);
        a[100..103].fill(1.0);
        let (gate, trig) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![50]);
        assert_eq!(highs(&gate), 50 + 96, "one gate from 50 to 196");
        assert_eq!(gate[195], 1.0);
        assert_eq!(gate[196], 0.0);
        assert_eq!(rises(&trig), vec![50], "no second trigger inside the gate");
    }

    #[test]
    fn the_shortest_length_is_one_sample() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("length", 0.01);
        let a = gate_at(128, 20, 50);
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(highs(&gate), 1);
        assert_eq!(gate[20], 1.0);
    }

    #[test]
    fn delay_shifts_edges_and_length_together() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("delay", 5.0); // 240 samples
        let a = gate_at(1024, 100, 60);
        let (gate, trig) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![340]);
        assert_eq!(highs(&gate), 60);
        assert_eq!(rises(&trig), vec![340]);
        // Combined with a set length.
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("delay", 5.0);
        h.set_param("length", 1.0); // 48 samples
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![340]);
        assert_eq!(highs(&gate), 48);
    }

    #[test]
    fn the_longest_delay_is_two_seconds() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("delay", 2000.0);
        let a = gate_at(BATCH, 4, 10);
        let (gate, _) = run(&mut h, &a, None, 96_000 + BATCH);
        assert_eq!(rises(&gate), vec![96_004]);
        assert_eq!(highs(&gate), 10);
    }

    #[test]
    fn the_divider_passes_the_first_of_every_n_edges_with_its_gate() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("divide", 3.0);
        let mut a = vec![0.0f32; 1024];
        for k in 0..7 {
            a[k * 100 + 10..k * 100 + 40].fill(1.0);
        }
        let (gate, trig) = run(&mut h, &a, None, 0);
        // Edges 0, 3 and 6 pass.
        assert_eq!(rises(&gate), vec![10, 310, 610]);
        assert_eq!(highs(&gate), 90);
        assert_eq!(rises(&trig), vec![10, 310, 610]);
        // Dividing by 1 passes all.
        let mut h = harness(Box::new(GateProcessor::new()));
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate).len(), 7);
    }

    #[test]
    fn logic_combines_two_gates() {
        let a = gate_at(256, 20, 100); // 20..120
        let b = gate_at(256, 70, 100); // 70..170
        let expect = |f: fn(bool, bool) -> bool| -> Vec<f32> {
            (0..256)
                .map(|i| {
                    let (x, y) = ((20..120).contains(&i), (70..170).contains(&i));
                    if f(x, y) { 1.0 } else { 0.0 }
                })
                .collect()
        };
        let cases: [(f32, fn(bool, bool) -> bool); 6] = [
            (0.0, |x, _| x),
            (1.0, |x, y| x && y),
            (2.0, |x, y| x || y),
            (3.0, |x, y| x != y),
            (4.0, |x, y| !(x && y)),
            (5.0, |x, y| !(x || y)),
        ];
        for (logic, f) in cases {
            let mut h = harness(Box::new(GateProcessor::new()));
            h.set_param("logic", logic);
            let (gate, _) = run(&mut h, &a, Some(&b), 0);
            assert_eq!(gate, expect(f), "logic {logic}");
        }
    }

    #[test]
    fn an_unconnected_b_counts_as_low() {
        let a = gate_at(256, 20, 100);
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("logic", 2.0); // or
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(gate, a);
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("logic", 1.0); // and
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(highs(&gate), 0);
    }

    #[test]
    fn an_idle_nor_does_not_fire_at_the_start() {
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("logic", 5.0);
        let (gate, trig) = run(&mut h, &[0.0; 128], None, 0);
        assert!(
            gate.iter().all(|v| *v == 1.0),
            "the nor of two lows is high"
        );
        assert_eq!(highs(&trig), 0);
    }

    #[test]
    fn the_order_is_logic_divide_delay_length() {
        // Two gates ORed, every second edge kept, delayed by 2 ms and made 1 ms long.
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("logic", 2.0);
        h.set_param("divide", 2.0);
        h.set_param("delay", 2.0); // 96 samples
        h.set_param("length", 1.0); // 48 samples
        let mut a = vec![0.0f32; 1024];
        let mut b = vec![0.0f32; 1024];
        a[10..40].fill(1.0); // edge 0: kept
        b[200..230].fill(1.0); // edge 1: dropped
        a[400..430].fill(1.0); // edge 2: kept
        b[420..470].fill(1.0); // inside the same OR gate
        let (gate, _) = run(&mut h, &a, Some(&b), 0);
        assert_eq!(rises(&gate), vec![106, 496]);
        assert_eq!(highs(&gate), 96);
    }

    #[test]
    fn non_finite_input_counts_as_low_reset_clears_and_sweeps_stay_finite() {
        let mut h = harness(Box::new(GateProcessor::new()));
        let (gate, _) = run(&mut h, &[f32::NAN; 128], None, 0);
        assert_eq!(highs(&gate), 0);
        h.set_param("delay", 20.0);
        let (gate, _) = run(&mut h, &gate_at(128, 0, 100), None, 0);
        assert!(highs(&gate) < 100);
        h.device_mut().reset();
        let (gate, trig) = run(&mut h, &[0.0; 128], None, 2000);
        assert_eq!(
            highs(&gate) + highs(&trig),
            0,
            "the delay line is empty after a reset"
        );
        *h.input_mut("gate_in") = [0.9; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    /// Switches a harness to a synced delay of the note value `note` (an index of `NOTE_VALUES`).
    fn synced_delay(h: &mut DeviceHarness, note: f32) {
        h.set_param("delay.sync", 1.0);
        h.set_param("delay.note", note);
    }

    #[test]
    fn the_note_values_are_fractions_of_a_whole_note_and_bars_follow_the_meter() {
        let b = |i: usize| note_value_beats(i, (4, 4));
        assert_eq!(b(10), 0.5, "1/8");
        assert_eq!(b(11), 0.75, "1/8 dotted");
        assert!((b(9) - 1.0 / 3.0).abs() < 1e-12, "1/8 triplet");
        assert!(
            (b(0) - 4.0 / 64.0 * 2.0 / 3.0).abs() < 1e-12,
            "1/64 triplet"
        );
        assert_eq!(b(1), 4.0 / 64.0, "1/64");
        assert_eq!(b(19), 4.0, "1/1");
        assert_eq!(b(20), 6.0, "1/1 dotted");
        assert_eq!(b(21), 4.0, "a bar of 4/4");
        assert_eq!(b(23), 16.0, "four bars of 4/4");
        assert_eq!(note_value_beats(21, (3, 4)), 3.0);
        assert_eq!(note_value_beats(22, (6, 8)), 6.0);
        assert_eq!(
            note_value_beats(99, (4, 4)),
            16.0,
            "past the table: the last one"
        );
        assert_eq!(NOTE_VALUES.len(), 24);
    }

    #[test]
    fn a_synced_delay_is_a_note_value_of_the_tempo() {
        // 120 BPM: a beat is 24000 samples at 48 kHz; 1/16 is a quarter of it.
        let a = gate_at(8192, 100, 60);
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 7.0);
        let (gate, trig) = run(&mut h, &a, None, 6000);
        assert_eq!(rises(&gate), vec![6100]);
        assert_eq!(highs(&gate), 60);
        assert_eq!(rises(&trig), vec![6100]);
        // Dotted: three halves of that.
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 8.0);
        let (gate, _) = run(&mut h, &a, None, 9000);
        assert_eq!(rises(&gate), vec![9100]);
        // Triplet: two thirds of it.
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 6.0);
        let (gate, _) = run(&mut h, &a, None, 4000);
        assert_eq!(rises(&gate), vec![4100]);
        // Half the tempo doubles it.
        let mut h = harness(Box::new(GateProcessor::new()));
        h.time_mut().tempo = atomik_core::Tempo::new(60.0).unwrap();
        synced_delay(&mut h, 7.0);
        let (gate, _) = run(&mut h, &a, None, 12_000);
        assert_eq!(rises(&gate), vec![12_100]);
    }

    #[test]
    fn a_synced_delay_reaches_four_bars_and_follows_the_meter() {
        let a = gate_at(BATCH, 4, 10);
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 23.0);
        let (gate, _) = run(&mut h, &a, None, 384_000);
        assert_eq!(
            rises(&gate),
            vec![384_004],
            "four bars of 4/4 at 120 BPM are 8 s"
        );
        assert_eq!(highs(&gate), 10);
        // A bar of 3/4 is three beats.
        let mut h = harness(Box::new(GateProcessor::new()));
        h.time_mut().time_signature = (3, 4);
        synced_delay(&mut h, 21.0);
        let (gate, _) = run(&mut h, &a, None, 72_000);
        assert_eq!(rises(&gate), vec![72_004]);
    }

    #[test]
    fn a_synced_delay_longer_than_the_line_is_cut_to_it() {
        // 32 whole notes' worth of bars at the slowest tempo is far over a minute.
        let a = gate_at(BATCH, 4, 10);
        let mut h = harness(Box::new(GateProcessor::new()));
        h.time_mut().tempo = atomik_core::Tempo::new(20.0).unwrap();
        h.time_mut().time_signature = (32, 4);
        synced_delay(&mut h, 23.0);
        let (gate, _) = run(&mut h, &a, None, 60 * 48_000 + BATCH);
        assert_eq!(rises(&gate), vec![60 * 48_000 + 4]);
    }

    #[test]
    fn a_synced_length_is_a_note_value_and_replaces_the_gate() {
        let a = gate_at(8192, 50, 3);
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("length.sync", 1.0);
        h.set_param("length.note", 4.0); // 1/32: an eighth of a beat, 3000 samples
        let (gate, trig) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![50]);
        assert_eq!(highs(&gate), 3000);
        assert_eq!(gate[50 + 2999], 1.0);
        assert_eq!(gate[50 + 3000], 0.0);
        assert_eq!(rises(&trig), vec![50]);
        // Delay and length synced together.
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 7.0);
        h.set_param("length.sync", 1.0);
        h.set_param("length.note", 1.0); // 1/64: 1500 samples
        let (gate, _) = run(&mut h, &a, None, 6000);
        assert_eq!(rises(&gate), vec![6050]);
        assert_eq!(highs(&gate), 1500);
    }

    #[test]
    fn a_synced_time_follows_a_tempo_change_between_batches() {
        let mut h = harness(Box::new(GateProcessor::new()));
        synced_delay(&mut h, 7.0); // 1/16: 6000 samples at 120 BPM, 3000 at 240
        h.time_mut().tempo = atomik_core::Tempo::new(240.0).unwrap();
        let a = gate_at(4096, 100, 60);
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![3100]);
    }

    #[test]
    fn switching_the_sync_off_returns_to_the_ms_values() {
        let a = gate_at(2048, 100, 60);
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("delay", 5.0); // 240 samples
        h.set_param("length", 1.0); // 48 samples
        synced_delay(&mut h, 7.0);
        h.set_param("length.sync", 1.0);
        h.set_param("delay.sync", 0.0);
        h.set_param("length.sync", 0.0);
        let (gate, _) = run(&mut h, &a, None, 0);
        assert_eq!(rises(&gate), vec![340]);
        assert_eq!(highs(&gate), 48);
    }

    #[test]
    fn the_sync_switches_default_off_so_a_project_without_them_plays_as_before() {
        for key in ["delay.sync", "length.sync"] {
            let p = DESCRIPTOR.param_by_key(key).unwrap();
            assert_eq!(p.default, 0.0, "{key}");
        }
        // The ms path with the new line: a delay of two seconds is still exact.
        let mut h = harness(Box::new(GateProcessor::new()));
        h.set_param("delay", 1234.5);
        let a = gate_at(BATCH, 4, 10);
        let (gate, _) = run(&mut h, &a, None, 60_000);
        assert_eq!(rises(&gate), vec![4 + 59_256]);
    }

    /// The gate processor as it was before the note values: a ring of one `bool` per sample two
    /// seconds long, the same stages in the same order.
    fn old_model(
        a: &[f32],
        b: &[f32],
        (logic, divide, delay_ms, length_ms): (u8, u32, f32, f32),
    ) -> (Vec<f32>, Vec<f32>) {
        let sr = 48_000.0f64;
        let ring_len = (f64::from(MAX_MS) * 0.001 * sr).ceil() as usize + 1;
        let samples = |ms: f32| (f64::from(ms) * 0.001 * sr).round();
        let delay = (samples(delay_ms) as usize).min(ring_len - 1);
        let length = if length_ms > 0.0 {
            (samples(length_ms) as u32).max(1)
        } else {
            0
        };
        let (mut ga, mut gb) = (GateIn::default(), GateIn::default());
        let mut ring = vec![false; ring_len];
        let (mut write, mut edges, mut passing) = (0usize, 0u32, true);
        let (mut combined, mut delayed_prev, mut out_prev): (
            Option<bool>,
            Option<bool>,
            Option<bool>,
        ) = (None, None, None);
        let (mut length_left, mut pulse_left) = (0u32, 0u32);
        let pulse = pulse_samples(48_000.0);
        let (mut gate, mut trig) = (Vec::new(), Vec::new());
        for n in 0..a.len() {
            let x = ga.update(a[n]);
            let y = gb.update(b[n]);
            let c = match logic {
                0 => x,
                1 => x && y,
                2 => x || y,
                3 => x != y,
                4 => !(x && y),
                _ => !(x || y),
            };
            let before = combined.unwrap_or(c);
            if combined.is_none() {
                ring.fill(c);
            }
            combined = Some(c);
            if c && !before {
                let slot = edges % divide;
                passing = slot == 0;
                edges = slot + 1;
            }
            let divided = c && passing;
            ring[write] = divided;
            let delayed = ring[(write + ring_len - delay) % ring_len];
            write = (write + 1) % ring_len;
            let rose = delayed && !delayed_prev.unwrap_or(delayed);
            delayed_prev = Some(delayed);
            let high = if length > 0 {
                if rose {
                    length_left = length;
                }
                let h = length_left > 0;
                length_left = length_left.saturating_sub(1);
                h
            } else {
                delayed
            };
            if high && !out_prev.unwrap_or(high) {
                pulse_left = pulse;
            }
            out_prev = Some(high);
            gate.push(if high { 1.0 } else { 0.0 });
            trig.push(if pulse_left > 0 { 1.0 } else { 0.0 });
            pulse_left = pulse_left.saturating_sub(1);
        }
        (gate, trig)
    }

    #[test]
    fn a_project_without_the_sync_params_renders_exactly_as_the_old_device_did() {
        let mut rng = atomik_dsp::Rng::new(9);
        // Random gates: both inputs flip now and then.
        let total = 40_000;
        let mut a = vec![0.0f32; total];
        let mut b = vec![0.0f32; total];
        let (mut x, mut y) = (false, false);
        for n in 0..total {
            if rng.next_f32() < 0.002 {
                x = !x;
            }
            if rng.next_f32() < 0.001 {
                y = !y;
            }
            a[n] = if x { 1.0 } else { 0.0 };
            b[n] = if y { 1.0 } else { 0.0 };
        }
        for params in [
            (0u8, 1u32, 0.0f32, 0.0f32),
            (2, 3, 7.3, 0.0),
            (3, 2, 120.0, 4.0),
            (4, 5, 0.0, 12.5),
            (5, 1, 1500.0, 0.01),
        ] {
            let mut h = harness(Box::new(GateProcessor::new()));
            h.set_param("logic", f32::from(params.0));
            h.set_param("divide", params.1 as f32);
            h.set_param("delay", params.2);
            h.set_param("length", params.3);
            let (gate, trig) = run(&mut h, &a, Some(&b), 0);
            let (want_gate, want_trig) = old_model(&a, &b, params);
            assert_eq!(gate, want_gate, "gate for {params:?}");
            assert_eq!(trig, want_trig, "trigger for {params:?}");
        }
    }
}
