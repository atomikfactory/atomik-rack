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
//! * **Mode.** `mode` `sample` is the sample-and-hold above. `track` follows `cv_in` while the
//!   clock is on and holds the last value while it is off. With the `trigger` clock it tracks
//!   while `trig_in` is high; with `free` and `tempo` it tracks for the first half of every
//!   cycle. In both modes `clock_out` pulses at each tick (the start of a tracking stretch). With
//!   no cable on `cv_in` the tracked value is the noise sampled at the tick.
//! * **Track length.** `track.length` `half` is the tracking above. `ms` tracks for `track.ms` and
//!   `note` for the note value `track.note` (1/64 to four bars, triplet and dotted, bars of the time
//!   signature) of the transport tempo, from each tick; the next tick starts it again, and a length
//!   longer than the cycle tracks all the time. With the `trigger` clock the stretch starts at the
//!   rising edge and no longer depends on how long the gate stays high. The length is counted in
//!   samples from the tick (at least one), so a locate and a bounce count it the same way.
//! * Before the first tick the output is 0. No latency; allocation-free.

use crate::tool_support::{
    DIVISIONS, GateIn, NOTE_VALUES, beats_to_samples, choice, cv_in, cv_out, expo,
    finite, linear, note_value_beats, pulse_samples,
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
const P_MODE: u32 = 4;
const P_TRACK_LENGTH: u32 = 5;
const P_TRACK_MS: u32 = 6;
const P_TRACK_NOTE: u32 = 7;

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

static MODES: [&str; 2] = ["sample", "track"];

/// The index in `NOTE_VALUES` of 1/16, the default `track.note`.
const DEFAULT_NOTE: f32 = 7.0;

const TRACK_HALF: u8 = 0;
const TRACK_MS: u8 = 1;
const TRACK_NOTE: u8 = 2;

static TRACK_LENGTHS: [&str; 3] = ["half", "ms", "note"];

static PARAMS: [ParamDescriptor; 8] = [
    choice(P_CLOCK, "clock", "Clock", &CLOCKS, 0.0),
    expo(P_RATE, "rate", "Rate", 0.1, 200.0, 4.0, Unit::Hz),
    choice(P_DIVISION, "division", "Division", &DIVISIONS, 5.0),
    linear(P_LEVEL, "level", "Level", -1.0, 1.0, 1.0, Unit::Percent),
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    choice(
        P_TRACK_LENGTH,
        "track.length",
        "Track length",
        &TRACK_LENGTHS,
        0.0,
    ),
    linear(
        P_TRACK_MS,
        "track.ms",
        "Track time",
        1.0,
        2000.0,
        100.0,
        Unit::Ms,
    ),
    choice(
        P_TRACK_NOTE,
        "track.note",
        "Track note",
        &NOTE_VALUES,
        DEFAULT_NOTE,
    ),
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
    /// Track-and-hold instead of sample-and-hold.
    track: bool,
    /// How long track mode follows the input from a tick: `TRACK_HALF` (half a cycle), `TRACK_MS`
    /// or `TRACK_NOTE`.
    track_length: u8,
    track_ms: f32,
    track_note: usize,
    /// Samples left of a timed tracking stretch.
    track_left: u32,
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
            track: false,
            track_length: TRACK_HALF,
            track_ms: 100.0,
            track_note: DEFAULT_NOTE as usize,
            track_left: 0,
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
        self.track_left = 0;
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
            P_MODE => self.track = v >= 0.5,
            P_TRACK_LENGTH => self.track_length = v.clamp(0.0, 2.0).round() as u8,
            P_TRACK_MS => self.track_ms = v.clamp(1.0, 2000.0),
            P_TRACK_NOTE => {
                self.track_note = v.clamp(0.0, (NOTE_VALUES.len() - 1) as f32).round() as usize;
            }
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

        // The samples a timed track stretch lasts from a tick (0: track half a cycle).
        let timed = if !self.track {
            0
        } else {
            match self.track_length {
                TRACK_MS => ((f64::from(self.track_ms) * 0.001 * f64::from(self.sample_rate))
                    .round() as u32)
                    .max(1),
                TRACK_NOTE => {
                    let beats = note_value_beats(self.track_note, ctx.time.time_signature);
                    (beats_to_samples(beats, ctx.time.tempo.bpm(), self.sample_rate) as u32).max(1)
                }
                _ => 0,
            }
        };
        let mut out = [0.0f32; BATCH];
        let mut clock_out = [0.0f32; BATCH];
        for n in 0..BATCH {
            let edge = self.trig.rose(trig[n]);
            // Whether track mode follows the input at this sample.
            let tracking;
            let tick = match self.clock {
                CLOCK_TRIGGER => {
                    tracking = self.trig.is_high();
                    edge
                }
                _ if locked => {
                    let at = (ctx.time.tick + n as f64 * ctx.time.ticks_per_frame) / cycle_ticks;
                    let c = at.floor() as i64;
                    tracking = at - (c as f64) < 0.5;
                    let fire = self.cycle != Some(c);
                    self.cycle = Some(c);
                    fire
                }
                _ => {
                    let fire = if self.first {
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
                    };
                    tracking = self.phase < 0.5;
                    fire
                }
            };
            if self.clock != CLOCK_TRIGGER {
                self.first = false;
            }
            let tracking = if timed > 0 {
                if tick {
                    self.track_left = timed;
                }
                let on = self.track_left > 0;
                self.track_left = self.track_left.saturating_sub(1);
                on
            } else {
                tracking
            };
            if tick {
                self.held = if cabled {
                    finite(cv[n]).clamp(-1.0, 1.0)
                } else {
                    self.rng.next_bipolar()
                };
                self.pulse_left = self.pulse_len;
            }
            if self.track && tracking && cabled && !tick {
                self.held = finite(cv[n]).clamp(-1.0, 1.0);
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
            &[
                "clock",
                "rate",
                "division",
                "level",
                "mode",
                "track.length",
                "track.ms",
                "track.note",
            ],
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
    fn track_mode_follows_the_input_while_the_trigger_is_high_and_holds_when_low() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        let mut trig = [0.0f32; BATCH];
        trig[10..30].fill(1.0);
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = ramp_batch(0);
        h.process();
        let out = *h.output("cv_out");
        assert!(
            out[..10].iter().all(|v| *v == 0.0),
            "nothing before the gate"
        );
        for n in 10..30 {
            assert_eq!(out[n], n as f32 / 1000.0, "follows at {n}");
        }
        for n in 30..BATCH {
            assert_eq!(out[n], 29.0 / 1000.0, "holds at {n}");
        }
        // The next batch keeps holding while the input moves on.
        *h.input_mut("trig_in") = [0.0; BATCH];
        *h.input_mut("cv_in") = ramp_batch(BATCH);
        h.process();
        assert!(h.output("cv_out").iter().all(|v| *v == 29.0 / 1000.0));
        // A gate that stays high follows through several batches.
        *h.input_mut("trig_in") = [1.0; BATCH];
        h.process();
        assert_eq!(
            h.output("cv_out")[BATCH - 1],
            (2 * BATCH - 1) as f32 / 1000.0
        );
        // The clock output still pulses at the rise.
        assert_eq!(h.output("clock_out")[0], 1.0);
    }

    #[test]
    fn track_mode_with_a_free_clock_tracks_the_first_half_of_each_cycle() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0); // 480 samples a cycle at 48 kHz
        let mut seen = Vec::new();
        for b in 0..16 {
            *h.input_mut("cv_in") = ramp_batch(b * BATCH);
            h.process();
            seen.extend_from_slice(h.output("cv_out"));
        }
        let held: Vec<f32> = seen[240..480].to_vec();
        assert!(
            held.iter().all(|v| *v == held[0]),
            "held over the second half"
        );
        assert!(seen[1] != seen[2], "followed over the first half");
        assert!(seen[500] != seen[510], "follows again in the next cycle");
    }

    #[test]
    fn sample_mode_ignores_the_input_between_ticks_as_before() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("mode", 0.0);
        let mut trig = [0.0f32; BATCH];
        trig[5] = 1.0;
        trig[6..30].fill(1.0);
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = ramp_batch(0);
        h.process();
        assert_eq!(h.output("cv_out")[BATCH - 1], 5.0 / 1000.0);
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

    /// A slow ramp that does not wrap within a few thousand samples.
    fn slope_batch(base: usize) -> [f32; BATCH] {
        let mut x = [0.0f32; BATCH];
        for (i, v) in x.iter_mut().enumerate() {
            *v = (base + i) as f32 * 1e-4;
        }
        x
    }

    /// The output of `samples` samples (whole batches) of the slope.
    fn run_slope(h: &mut DeviceHarness, samples: usize) -> Vec<f32> {
        let mut seen = Vec::new();
        for b in 0..samples.div_ceil(BATCH) {
            *h.input_mut("cv_in") = slope_batch(b * BATCH);
            h.process();
            seen.extend_from_slice(h.output("cv_out"));
        }
        seen
    }

    #[test]
    fn a_timed_track_follows_for_the_set_ms_from_each_tick() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0); // 480 samples a cycle
        h.set_param("track.length", 1.0);
        h.set_param("track.ms", 2.0); // 96 samples
        let seen = run_slope(&mut h, 1024);
        for n in 0..96 {
            assert_eq!(seen[n], n as f32 * 1e-4, "follows at {n}");
        }
        for n in 96..480 {
            assert_eq!(seen[n], 95.0 * 1e-4, "holds at {n}");
        }
        assert_eq!(seen[480], 480.0 * 1e-4, "the next tick tracks again");
        assert_eq!(seen[480 + 95], 575.0 * 1e-4);
        assert_eq!(seen[480 + 96], 575.0 * 1e-4);
    }

    #[test]
    fn a_timed_track_can_be_longer_than_the_cycle() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0);
        h.set_param("track.length", 1.0);
        h.set_param("track.ms", 20.0); // two cycles
        let seen = run_slope(&mut h, 1024);
        for n in 0..1024 {
            assert_eq!(seen[n], n as f32 * 1e-4, "follows at {n}");
        }
    }

    #[test]
    fn a_timed_track_follows_a_note_value_of_the_tempo() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 10.0); // 4800 samples a cycle
        h.set_param("track.length", 2.0);
        h.set_param("track.note", 1.0); // 1/64 at 120 BPM: 1500 samples
        let seen = run_slope(&mut h, 3072);
        assert_eq!(seen[1499], 1499.0 * 1e-4);
        assert_eq!(seen[1500], 1499.0 * 1e-4, "holds from the end of the note");
        assert_eq!(seen[3000], 1499.0 * 1e-4);
        // Half the tempo doubles it.
        let mut h = harness(Box::new(SampleHold::new()));
        h.time_mut().tempo = atomik_core::Tempo::new(60.0).unwrap();
        h.set_param("mode", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 10.0);
        h.set_param("track.length", 2.0);
        h.set_param("track.note", 1.0);
        let seen = run_slope(&mut h, 3072);
        assert_eq!(seen[2999], 2999.0 * 1e-4);
        assert_eq!(seen[3000], 2999.0 * 1e-4);
    }

    #[test]
    fn a_timed_track_with_a_trigger_counts_from_the_edge_not_the_gate() {
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("mode", 1.0);
        h.set_param("track.length", 1.0);
        h.set_param("track.ms", 1.0); // 48 samples
        let mut trig = [0.0f32; BATCH];
        trig[10..].fill(1.0); // the gate stays high to the end of the batch
        *h.input_mut("trig_in") = trig;
        *h.input_mut("cv_in") = slope_batch(0);
        h.process();
        let out = *h.output("cv_out");
        assert_eq!(out[10], 10.0 * 1e-4);
        assert!((out[BATCH - 1] - 57.0 * 1e-4).abs() < 1e-9);
    }

    #[test]
    fn the_track_length_defaults_to_half_a_cycle_so_old_projects_play_as_before() {
        for (key, want) in [
            ("track.length", 0.0),
            ("track.ms", 100.0),
            ("track.note", 7.0),
        ] {
            assert_eq!(DESCRIPTOR.param_by_key(key).unwrap().default, want, "{key}");
        }
        // A timed length has no effect in sample mode.
        let mut h = harness(Box::new(SampleHold::new()));
        h.set_param("track.length", 1.0);
        h.set_param("clock", 1.0);
        h.set_param("rate", 100.0);
        let seen = run_slope(&mut h, 512);
        assert!(seen[..480].iter().all(|v| *v == seen[0]));
    }
}
