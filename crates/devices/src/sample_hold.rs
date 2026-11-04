//! `atomik.sample-hold`: samples a control voltage (or noise) at each clock and holds it.
//!
//! # Signal flow
//!
//! ```text
//! clock tick  -> held = the level of cv_in at that sample      (noise when nothing is cabled)
//! cv_out      = held * level                                   (every sample)
//! clock_out   = 1 ms pulse at every tick
//! ```
//!
//! * **Clock.** `clock` picks the source of the ticks. `trigger` ticks at every rising edge of
//!   `trig_in` (high above 0.5, low below 0.3). `free` ticks at `rate` Hz. `tempo` ticks at a
//!   note value (`division`) of the transport tempo; while the transport plays it takes its
//!   position from the song, so a locate and a bounce tick at the same places, and it runs freely
//!   at the tempo while stopped. The two internal clocks tick at the first sample after the
//!   device starts, so a held value exists at once.
//! * **Source.** With a cable on `cv_in` the held value is its level (limited to +-1, a
//!   non-finite sample counts as 0). Without one the held value is noise: uniform in -1..+1 from
//!   a generator seeded by the project seed, so a bounce repeats exactly.
//! * **`level`** (-1..+1) scales the output; a negative level inverts it. It acts on the output,
//!   so a change is heard at once and not at the next tick.
//! * Before the first tick the output is 0. No latency; allocation-free.

use crate::tool_support::{
    DIVISIONS, GateIn, choice, cv_in, cv_out, expo, finite, linear, pulse_samples,
};
use atomik_core::{BATCH, PPQ, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::Rng;

const P_CLOCK: u32 = 0;
const P_RATE: u32 = 1;
const P_DIVISION: u32 = 2;
const P_LEVEL: u32 = 3;

const CV_IN: SocketId = SocketId(0);
const TRIG_IN: SocketId = SocketId(1);
const CV_OUT: SocketId = SocketId(2);
const CLOCK_OUT: SocketId = SocketId(3);

/// A tick comes when the phase is within this of a whole cycle (the sum of many small steps
/// can fall just short of it).
const PHASE_SLACK: f64 = 1e-9;

const CLOCK_TRIGGER: u8 = 0;
const CLOCK_FREE: u8 = 1;
const CLOCK_TEMPO: u8 = 2;

static CLOCKS: [&str; 3] = ["trigger", "free", "tempo"];

static PARAMS: [ParamDescriptor; 4] = [
    choice(P_CLOCK, "clock", "Clock", &CLOCKS, 0.0),
    expo(P_RATE, "rate", "Rate", 0.1, 200.0, 4.0, Unit::Hz),
    choice(P_DIVISION, "division", "Division", &DIVISIONS, 5.0),
    linear(P_LEVEL, "level", "Level", -1.0, 1.0, 1.0, Unit::Percent),
];

static SOCKETS: [SocketDescriptor; 4] = [
    cv_in(0, "cv_in", "CV in"),
    cv_in(1, "trig_in", "Trigger in"),
    cv_out(2, "cv_out", "CV out"),
    cv_out(3, "clock_out", "Clock out"),
];

/// Static description of Sample and Hold.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.sample-hold",
    name: "Sample and Hold",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`SampleHold`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(SampleHold::new()),
    }
}

/// The sample-and-hold device.
#[derive(Debug, Clone)]
pub struct SampleHold {
    clock: u8,
    rate: f32,
    beats: f64,
    level: f32,
    sample_rate: f32,
    seed: u64,
    rng: Rng,
    trig: GateIn,
    held: f32,
    /// Fractional clock position of the free-running clocks.
    phase: f64,
    /// The tempo cycle of the last tick while the clock is locked to the song.
    cycle: Option<i64>,
    /// No tick has happened since the start: the internal clocks tick at once.
    first: bool,
    pulse_len: u32,
    pulse_left: u32,
}

impl SampleHold {
    fn new() -> Self {
        Self {
            clock: CLOCK_TRIGGER,
            rate: 4.0,
            beats: crate::stereo_delay::SYNC_BEATS[5],
            level: 1.0,
            sample_rate: 48_000.0,
            seed: 0,
            rng: Rng::new(0),
            trig: GateIn::default(),
            held: 0.0,
            phase: 0.0,
            cycle: None,
            first: true,
            pulse_len: pulse_samples(48_000.0),
            pulse_left: 0,
        }
    }
}

impl Device for SampleHold {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.pulse_len = pulse_samples(self.sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.rng = Rng::new(self.seed);
        self.trig.reset();
        self.held = 0.0;
        self.phase = 0.0;
        self.cycle = None;
        self.first = true;
        self.pulse_left = 0;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_CLOCK => self.clock = v.clamp(0.0, 2.0).round() as u8,
            P_RATE => self.rate = v.clamp(0.1, 200.0),
            P_DIVISION => {
                let i = v.clamp(0.0, 11.0).round() as usize;
                self.beats = crate::stereo_delay::SYNC_BEATS[i];
            }
            P_LEVEL => self.level = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let cv = *ctx.input(CV_IN);
        let trig = *ctx.input(TRIG_IN);
        let cabled = ctx.is_connected(CV_IN);

        let locked = self.clock == CLOCK_TEMPO && ctx.time.playing;
        let inc = match self.clock {
            CLOCK_FREE => f64::from(self.rate) / f64::from(self.sample_rate),
            CLOCK_TEMPO => ctx.time.tempo.bpm() / 60.0 / self.beats / f64::from(self.sample_rate),
            _ => 0.0,
        };
        let cycle_ticks = PPQ as f64 * self.beats;
        if !locked {
            self.cycle = None;
        }

        let mut out = [0.0f32; BATCH];
        let mut clock_out = [0.0f32; BATCH];
        for n in 0..BATCH {
            let edge = self.trig.rose(trig[n]);
            let tick = match self.clock {
                CLOCK_TRIGGER => edge,
                _ if locked => {
                    let c = ((ctx.time.tick + n as f64 * ctx.time.ticks_per_frame) / cycle_ticks)
                        .floor() as i64;
                    let fire = self.cycle != Some(c);
                    self.cycle = Some(c);
                    fire
                }
                _ => {
                    if self.first {
                        self.phase = 0.0;
                        true
                    } else {
                        self.phase += inc;
                        if self.phase >= 1.0 - PHASE_SLACK {
                            self.phase -= 1.0;
                            true
                        } else {
                            false
                        }
                    }
                }
            };
            if self.clock != CLOCK_TRIGGER {
                self.first = false;
            }
            if tick {
                self.held = if cabled {
                    finite(cv[n]).clamp(-1.0, 1.0)
                } else {
                    self.rng.next_bipolar()
                };
                self.pulse_left = self.pulse_len;
            }
            if self.pulse_left > 0 {
                clock_out[n] = 1.0;
                self.pulse_left -= 1;
            }
            out[n] = self.held * self.level;
        }
        *ctx.output(CV_OUT) = out;
        *ctx.output(CLOCK_OUT) = clock_out;
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

    fn ramp_batch(base: usize) -> [f32; BATCH] {
        let mut x = [0.0f32; BATCH];
        for (i, v) in x.iter_mut().enumerate() {
            *v = ((base + i) % 1000) as f32 / 1000.0;
        }
        x
    }

    /// The indexes (over `batches` batches) at which `clock_out` rises.
    fn clock_edges(h: &mut DeviceHarness, batches: usize) -> Vec<usize> {
        let r = h.render(batches, &["clock_out"]);
        let c = &r[0];
        (0..c.len())
            .filter(|i| c[*i] == 1.0 && (*i == 0 || c[*i - 1] == 0.0))
            .collect()
    }

    #[test]
    fn descriptor_lists_the_keys_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &["clock", "rate", "division", "level"],
            &["cv_in", "trig_in", "cv_out", "clock_out"],
        );
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    }

    #[test]
    fn a_trigger_samples_the_input_and_holds_it_between_triggers() {
        let mut h = harness(Box::new(SampleHold::new()));
        let mut trig = [0.0f32; BATCH];
        trig[5] = 1.0;
        trig[6] = 1.0;
        trig[40..].fill(1.0);
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = ramp_batch(0);
        h.process();
        let out = *h.output("cv_out");
        assert!(
            out[..5].iter().all(|v| *v == 0.0),
            "nothing held before the first tick"
        );
        assert_eq!(out[5], 5.0 / 1000.0);
        assert_eq!(out[39], 5.0 / 1000.0, "held while the input moves on");
        assert_eq!(out[40], 40.0 / 1000.0);
        assert_eq!(out[BATCH - 1], 40.0 / 1000.0);
        // The clock output pulses for 1 ms (48 samples at 48 kHz) from each tick.
        let c = h.output("clock_out");
        assert_eq!(c[4], 0.0);
        assert_eq!(c[5], 1.0);
        assert_eq!(c[BATCH - 1], 1.0);
        // Still holding next batch, with no new edge (the gate stays high).
        *h.input_mut("trig_in") = [1.0; BATCH];
        *h.input_mut("cv_in") = ramp_batch(BATCH);
        h.process();
        assert_eq!(h.output("cv_out")[0], 40.0 / 1000.0);
        assert_eq!(h.output("cv_out")[BATCH - 1], 40.0 / 1000.0);
    }

    #[test]
    fn level_scales_and_inverts_the_held_value_live() {
        let mut h = harness(Box::new(SampleHold::new()));
        let mut trig = [0.0f32; BATCH];
        trig[0] = 1.0;
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = [0.8; BATCH];
        h.process();
        assert!((h.output("cv_out")[3] - 0.8).abs() < 1e-6);
        *h.input_mut("trig_in") = [0.0; BATCH];
        h.set_param("level", -0.5);
        h.process();
        assert!((h.output("cv_out")[3] + 0.4).abs() < 1e-6);
    }

    #[test]
    fn the_input_is_limited_to_one_and_non_finite_counts_as_zero() {
        let mut h = harness(Box::new(SampleHold::new()));
        let mut trig = [0.0f32; BATCH];
        trig[0] = 1.0;
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = [5.0; BATCH];
        h.process();
        assert_eq!(h.output("cv_out")[1], 1.0);
        *h.input_mut("trig_in") = [0.0; BATCH];
        h.process();
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = [f32::NAN; BATCH];
        h.process();
        assert_eq!(h.output("cv_out")[1], 0.0);
    }

    #[test]
    fn without_a_cable_it_holds_seeded_noise_that_repeats() {
        let run = |seed: u64| {
            let mut h =
                DeviceHarness::with_options(Box::new(SampleHold::new()), 48_000.0, 120.0, seed);
            let mut trig = [0.0f32; BATCH];
            for i in (0..BATCH).step_by(16) {
                trig[i] = 1.0;
            }
            *h.input_mut("trig_in") = trig;
            h.render(2, &["cv_out"]).remove(0)
        };
        let a = run(3);
        assert_eq!(a, run(3));
        assert_ne!(a, run(4));
        assert!(a.iter().all(|v| (-1.0..1.0).contains(v)));
        // A new value at each trigger, constant in between.
        for k in 0..8 {
            let seg = &a[k * 16..k * 16 + 16];
            assert!(seg.iter().all(|v| *v == seg[0]));
        }
        assert_ne!(a[0], a[16]);
    }

    #[test]
    fn the_free_clock_ticks_at_its_rate_starting_at_once() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0); // 480 samples
        let batches = 4800 / BATCH;
        let edges = clock_edges(&mut h, batches);
        assert_eq!(edges, (0..10).map(|k| k * 480).collect::<Vec<_>>());
    }

    #[test]
    fn a_free_clock_samples_a_ramp_at_each_tick() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0);
        let mut got = Vec::new();
        for b in 0..30 {
            *h.input_mut("cv_in") = ramp_batch(b * BATCH);
            h.process();
            got.extend_from_slice(h.output("cv_out"));
        }
        // Held values step every 480 samples, to the ramp's value at the tick.
        assert_eq!(got[0], 0.0);
        assert_eq!(got[479], 0.0);
        assert_eq!(got[480], 480.0 / 1000.0);
        assert_eq!(got[959], 480.0 / 1000.0);
        assert_eq!(got[960], 960.0 / 1000.0);
    }

    #[test]
    fn the_tempo_clock_runs_free_while_stopped_and_on_the_grid_while_playing() {
        // 120 BPM, quarter notes: every 24000 samples while stopped.
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("clock", 2.0);
        h.set_param("division", 8.0);
        h.time_mut().playing = false;
        let edges = clock_edges(&mut h, 2 * 24_000 / BATCH + 1);
        assert_eq!(edges, vec![0, 24_000, 48_000]);

        // Playing, from tick 700 (a quarter note is 15360 ticks): the cycle changes at 15360.
        let mut p = harness(Box::new(SampleHold::new()));
        p.set_param("clock", 2.0);
        p.set_param("division", 8.0);
        p.time_mut().tick = 700.0;
        let tpf = p.time_mut().ticks_per_frame;
        let edges = clock_edges(&mut p, 24_000 / BATCH);
        let expect = ((15_360.0 - 700.0) / tpf).ceil() as usize;
        assert_eq!(edges, vec![0, expect]);
    }

    #[test]
    fn a_locked_clock_ticks_on_the_same_song_positions_from_any_start() {
        let cycles_at = |start: f64| {
            let mut h = harness(Box::new(SampleHold::new()));
            h.set_param("clock", 2.0);
            h.set_param("division", 2.0); // 1/16 = 3840 ticks
            h.time_mut().tick = start;
            let tpf = h.time_mut().ticks_per_frame;
            clock_edges(&mut h, 400)
                .into_iter()
                .skip(1)
                .map(|e| ((start + e as f64 * tpf) / 3840.0).round() as i64)
                .collect::<Vec<_>>()
        };
        let a = cycles_at(0.0);
        let b = cycles_at(3840.0 * 4.0);
        assert_eq!(a[0], 1);
        assert_eq!(b[0], 5);
        assert!(a.windows(2).all(|w| w[1] == w[0] + 1));
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn reset_restarts_the_noise_and_the_clock() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("clock", 1.0);
        let first = h.render(3, &["cv_out"]).remove(0);
        h.device_mut().reset();
        let again = h.render(3, &["cv_out"]).remove(0);
        assert_eq!(first, again);
    }

    #[test]
    fn sweeps_stay_finite() {
        let mut h = harness(Box::new(SampleHold::new()));
        *h.input_mut("trig_in") = [0.7; BATCH];
        *h.input_mut("cv_in") = [0.25; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}
