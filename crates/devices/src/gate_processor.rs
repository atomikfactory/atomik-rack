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
//!   length together. A change takes effect at once (a gate that is high while the delay moves
//!   can come out shorter or longer). The delay is a deliberate effect: the device reports no
//!   latency.
//! * **Length.** `length` 0 leaves the gate as long as it was. Above 0 it replaces the gate by
//!   one of exactly that many ms (at least one sample) from each rising edge, so a trigger becomes
//!   a gate of a set length; an edge inside a running gate restarts it.
//! * **Outputs.** `gate_out` is the result; `trig_out` is a 1 ms pulse at every rising edge of
//!   `gate_out` (a gate to trigger converter).
//! * The delay line is allocated when the device is prepared (two seconds at the sample rate).
//!   Non-finite input counts as low; allocation-free while running.

use crate::tool_support::{GateIn, choice, count, cv_in, cv_out, finite, linear, pulse_samples};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};

const P_LOGIC: u32 = 0;
const P_DIVIDE: u32 = 1;
const P_DELAY: u32 = 2;
const P_LENGTH: u32 = 3;

const GATE_IN: SocketId = SocketId(0);
const GATE_B_IN: SocketId = SocketId(1);
const GATE_OUT: SocketId = SocketId(2);
const TRIG_OUT: SocketId = SocketId(3);

/// The longest delay and length, ms.
const MAX_MS: f32 = 2000.0;

static LOGICS: [&str; 6] = ["a", "and", "or", "xor", "nand", "nor"];

static PARAMS: [ParamDescriptor; 4] = [
    choice(P_LOGIC, "logic", "Logic", &LOGICS, 0.0),
    count(P_DIVIDE, "divide", "Divide", 1.0, 16.0, 1.0),
    linear(P_DELAY, "delay", "Delay", 0.0, MAX_MS, 0.0, Unit::Ms),
    linear(P_LENGTH, "length", "Length", 0.0, MAX_MS, 0.0, Unit::Ms),
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
    sample_rate: f32,
    a: GateIn,
    b: GateIn,
    /// The combined gate of the last sample (`None` before the first one).
    combined: Option<bool>,
    /// Rising edges counted by the divider.
    edges: u32,
    /// Whether the current gate passes the divider.
    passing: bool,
    /// The divided gate, a ring of the last `ring.len()` samples.
    ring: Vec<bool>,
    write: usize,
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
            sample_rate: 48_000.0,
            a: GateIn::default(),
            b: GateIn::default(),
            combined: None,
            edges: 0,
            passing: true,
            ring: Vec::new(),
            write: 0,
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

    fn derive(&mut self) {
        let samples = |ms: f32| (f64::from(ms) * 0.001 * f64::from(self.sample_rate)).round();
        let max_delay = self.ring.len().saturating_sub(1);
        self.delay_samples = (samples(self.delay_ms) as usize).min(max_delay);
        self.length_samples = if self.length_ms > 0.0 {
            (samples(self.length_ms) as u32).max(1)
        } else {
            0
        };
    }
}

impl Device for GateProcessor {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.pulse_len = pulse_samples(self.sample_rate);
        let len = (f64::from(MAX_MS) * 0.001 * info.sample_rate.hz()).ceil() as usize + 1;
        self.ring.clear();
        self.ring.resize(len, false);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
        self.combined = None;
        self.edges = 0;
        self.passing = true;
        self.ring.fill(false);
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
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let ga = *ctx.input(GATE_IN);
        let gb = *ctx.input(GATE_B_IN);
        let b_cabled = ctx.is_connected(GATE_B_IN);
        let mut gate = [0.0f32; BATCH];
        let mut trig = [0.0f32; BATCH];
        let ring_len = self.ring.len();
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
                self.ring.fill(c);
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
                self.ring[self.write] = divided;
                let read = (self.write + ring_len - self.delay_samples) % ring_len;
                let d = self.ring[read];
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
            &["logic", "divide", "delay", "length"],
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
}
