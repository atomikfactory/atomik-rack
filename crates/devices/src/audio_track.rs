//! `atomik.audio-track`: the audio track player device.
//!
//! The engine renders the track's audio clips (sample-accurate starts, gain, fades) into
//! per-batch *playback* batches and hands them to the device through
//! [`ProcessContext::playback`]; the device itself never reads files. Its output is
//!
//! `out = playback + input × monitor_gain`
//!
//! where `input` is the cabled `in_l`/`in_r` (from `interface:in_k`, for input monitoring) and
//! `monitor_gain` is the input gain (`rec_gain`, 1 at its default of 0 dB) when `monitor` is
//! **on**, or when it is **auto** and the app has set `internal.monitor_live` to 1 (the track is
//! armed), and 0 otherwise (**off**). The gain change is a 5 ms linear crossfade, snapped on
//! the first batch after `prepare`/`reset` like the other devices, so turning the input gain
//! needs no zipper and what a performer hears is what the take gets (the app applies the same
//! gain to the recorded signal, see `atomik_engine::TapControls`).
//!
//! `rec_gain` (−60..+24 dB, the minimum is off) and `rec_clip_safe` are the track's recording
//! input settings. They are ordinary stored params (undoable, merged while dragged), so a
//! project written before they existed loads at 0 dB with clip-safe off. The device only uses
//! the gain; the clip-safe limiter works on the recorded take, never on the monitor.
//!
//! **Tuner.** The device keeps a [`ScopeTap`] of its input after the gain (what the performer hears
//! and what is recorded, whether or not the track is armed or monitored) and pushes it every
//! batch; the app runs the pitch analysis on the UI side from it ([`atomik_dsp::tuner`]), so the
//! audio thread only stores 2 x 64 floats. `internal.tuner_live` is set by the app while the
//! track shows its tuner; with `tuner_mute` on (the default) the track output (playback and
//! monitor) is then faded out over 5 ms, so a guitar can be tuned in silence. `tuner_ref` (A4,
//! 415..466 Hz) is the reference pitch the display uses; the device does not read it. The three
//! are additive params: a project written before them loads with 440 Hz and mute on, which does
//! nothing while no tuner shows.
//!
//! Without playback (`None`: transport stopped, or no engine playback) only the monitored
//! input is heard. Unconnected inputs read as zeros.
//!
//! The device is `DeviceKind::Instrument` (so it gets a console channel), has a single stage
//! and no latency. `process` never allocates, locks or blocks and is a pure function
//! of its inputs and params.

use atomik_core::{
    BATCH, ParamDescriptor, ParamId, REC_CLIP_SAFE_PARAM, REC_GAIN_MAX_DB, REC_GAIN_MIN_DB,
    REC_GAIN_PARAM, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::Smoother;
use std::sync::Arc;

/// `monitor` choices: off, auto, on.
pub const MONITOR_MODES: &[&str] = &["off", "auto", "on"];
/// Gain smoothing time of the monitor crossfade.
const SMOOTH_MS: f32 = 5.0;

const P_MONITOR: u32 = 0;
const P_MONITOR_LIVE: u32 = 1;
const P_REC_GAIN: u32 = 2;
const P_REC_CLIP_SAFE: u32 = 3;
const P_TUNER_LIVE: u32 = 4;
const P_TUNER_MUTE: u32 = 5;
const P_TUNER_REF: u32 = 6;

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

const fn socket(
    id: u16,
    key: &'static str,
    name: &'static str,
    dir: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Audio,
        direction: dir,
    }
}

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor {
        id: ParamId(P_MONITOR),
        key: "monitor",
        name: "Monitor",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Choice(MONITOR_MODES),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_MONITOR_LIVE),
        key: "internal.monitor_live",
        name: "Monitor live",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_REC_GAIN),
        key: REC_GAIN_PARAM,
        name: "Input gain",
        min: REC_GAIN_MIN_DB,
        max: REC_GAIN_MAX_DB,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_REC_CLIP_SAFE),
        key: REC_CLIP_SAFE_PARAM,
        name: "Clip safe",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_TUNER_LIVE),
        key: "internal.tuner_live",
        name: "Tuner live",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_TUNER_MUTE),
        key: "tuner_mute",
        name: "Mute while tuning",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_TUNER_REF),
        key: "tuner_ref",
        name: "Reference A4",
        min: 415.0,
        max: 466.0,
        default: 440.0,
        taper: Taper::Linear,
        unit: Unit::Hz,
        discrete: true,
    },
];

static SOCKETS: [SocketDescriptor; 4] = [
    socket(0, "in_l", "Input L", Direction::Input),
    socket(1, "in_r", "Input R", Direction::Input),
    socket(2, "out_l", "Out L", Direction::Output),
    socket(3, "out_r", "Out R", Direction::Output),
];

/// Static description of the audio track device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.audio-track",
    name: "Audio Track",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`AudioTrack`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(AudioTrack::new()),
    }
}

/// The audio track device.
#[derive(Debug, Clone)]
pub struct AudioTrack {
    monitor: u8,
    live: bool,
    /// The input gain, linear (`rec_gain`).
    input_gain: f32,
    gain: Smoother,
    /// The tuner is shown and `tuner_mute` is on: the output is faded out.
    tuner_live: bool,
    tuner_mute: bool,
    /// 1 when the track sounds, 0 while the tuner mutes it.
    mute: Smoother,
    /// The input after the gain, for the tuner (allocated here: it outlives re-prepares).
    tap: Arc<ScopeTap>,
    fresh: bool,
}

impl Default for AudioTrack {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioTrack {
    /// A device with default params (call `prepare` before processing).
    pub fn new() -> Self {
        Self {
            monitor: 1,
            live: false,
            input_gain: 1.0,
            gain: Smoother::new(0.0, SMOOTH_MS, 48_000.0),
            tuner_live: false,
            tuner_mute: true,
            mute: Smoother::new(1.0, SMOOTH_MS, 48_000.0),
            tap: Arc::new(ScopeTap::new()),
            fresh: true,
        }
    }

    fn target(&self) -> f32 {
        let on = self.monitor == 2 || (self.monitor == 1 && self.live);
        if on { self.input_gain } else { 0.0 }
    }

    fn update_target(&mut self) {
        self.gain.set_target(self.target());
        let muted = self.tuner_live && self.tuner_mute;
        self.mute.set_target(if muted { 0.0 } else { 1.0 });
    }
}

impl Device for AudioTrack {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.gain.set_time(SMOOTH_MS, info.sample_rate.hz() as f32);
        self.mute.set_time(SMOOTH_MS, info.sample_rate.hz() as f32);
        self.update_target();
        self.fresh = true;
    }

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }

    fn reset(&mut self) {
        self.update_target();
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_MONITOR => self.monitor = plain.round().clamp(0.0, 2.0) as u8,
            P_MONITOR_LIVE => self.live = plain >= 0.5,
            P_REC_GAIN => self.input_gain = PARAMS[P_REC_GAIN as usize].db_param_gain(plain),
            P_TUNER_LIVE => self.tuner_live = plain >= 0.5,
            P_TUNER_MUTE => self.tuner_mute = plain >= 0.5,
            _ => return,
        }
        self.update_target();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            let t = self.gain.target();
            self.gain.set_immediate(t);
            let m = self.mute.target();
            self.mute.set_immediate(m);
            self.fresh = false;
        }
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let (pb_l, pb_r) = (ctx.playback(0).copied(), ctx.playback(1).copied());
        let mut gains = [0.0f32; BATCH];
        for g in &mut gains {
            *g = self.gain.next_value();
        }
        let mut mutes = [1.0f32; BATCH];
        for m in &mut mutes {
            *m = self.mute.next_value();
        }
        let out = ctx.output(OUT_L);
        for n in 0..BATCH {
            out[n] = (pb_l.map_or(0.0, |p| p[n]) + in_l[n] * gains[n]) * mutes[n];
        }
        let out = ctx.output(OUT_R);
        for n in 0..BATCH {
            out[n] = (pb_r.map_or(0.0, |p| p[n]) + in_r[n] * gains[n]) * mutes[n];
        }
        // The tuner hears the input after the gain, whatever the monitor does.
        let g = self.input_gain;
        let (mut tap_l, mut tap_r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        for n in 0..BATCH {
            tap_l[n] = in_l[n] * g;
            tap_r[n] = in_r[n] * g;
        }
        self.tap.push(&tap_l, &tap_r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness};
    use atomik_core::Batch;

    fn ramp(offset: f32) -> Batch {
        let mut b = [0.0; BATCH];
        for (i, s) in b.iter_mut().enumerate() {
            *s = offset + i as f32 * 0.01;
        }
        b
    }

    fn rig() -> atomik_testkit::harness::DeviceHarness {
        harness(Box::new(AudioTrack::new()))
    }

    #[test]
    fn descriptor_keys_are_in_contract_order() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "monitor",
                "internal.monitor_live",
                "rec_gain",
                "rec_clip_safe",
                "internal.tuner_live",
                "tuner_mute",
                "tuner_ref",
            ],
            &["in_l", "in_r", "out_l", "out_r"],
        );
        assert_eq!(DESCRIPTOR.type_id, "atomik.audio-track");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
        assert_eq!(DESCRIPTOR.stage_count(), 1);
        let monitor = DESCRIPTOR.param_by_key("monitor").unwrap();
        assert_eq!(monitor.default.to_bits(), 1.0f32.to_bits());
        assert_eq!(monitor.unit, Unit::Choice(&["off", "auto", "on"]));
        assert_eq!(factory().descriptor.type_id, "atomik.audio-track");
    }

    #[test]
    fn the_input_settings_default_to_unity_and_clip_safe_off() {
        let gain = DESCRIPTOR.param_by_key("rec_gain").unwrap();
        assert_eq!((gain.min, gain.max, gain.default), (-60.0, 24.0, 0.0));
        assert_eq!(gain.unit, Unit::Db);
        assert_eq!(gain.taper, Taper::Decibel);
        let safe = DESCRIPTOR.param_by_key("rec_clip_safe").unwrap();
        assert_eq!((safe.default, safe.unit), (0.0, Unit::Toggle));
    }

    #[test]
    fn the_input_gain_scales_the_monitored_input() {
        for db in [-12.0f32, 6.0, 24.0] {
            let mut h = rig();
            h.set_param("monitor", 2.0);
            h.set_param("rec_gain", db);
            *h.input_mut("in_l") = ramp(0.01);
            *h.input_mut("in_r") = [-0.2; BATCH];
            h.process();
            let g = atomik_core::db_to_gain(db);
            for (i, v) in h.output("out_l").iter().enumerate() {
                assert_eq!(v.to_bits(), (ramp(0.01)[i] * g).to_bits(), "{db} dB, {i}");
            }
            assert_eq!(h.output("out_r")[7].to_bits(), (-0.2f32 * g).to_bits());
        }
    }

    #[test]
    fn the_bottom_of_the_gain_range_is_off() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        h.set_param("rec_gain", -60.0);
        *h.input_mut("in_l") = [0.9; BATCH];
        let y = h.render(3, &["out_l"]);
        assert!(y[0].iter().all(|s| s.to_bits() == 0));
    }

    #[test]
    fn a_gain_change_is_a_five_ms_ramp_and_the_default_is_bit_exact() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        *h.input_mut("in_l") = [0.5; BATCH];
        let plain = h.render(1, &["out_l"]).remove(0);
        assert!(plain.iter().all(|s| s.to_bits() == 0.5f32.to_bits()));
        h.set_param("rec_gain", 6.0);
        let y = h.render(5, &["out_l"]).remove(0);
        let to = 0.5 * atomik_core::db_to_gain(6.0);
        let ramp_len = 240;
        for (n, v) in y.iter().enumerate().take(ramp_len) {
            let want = 0.5 + (to - 0.5) * (n + 1) as f32 / ramp_len as f32;
            assert!((v - want).abs() < 1e-5, "sample {n}: {v} vs {want}");
        }
        assert!(y[ramp_len..].iter().all(|s| (s - to).abs() < 1e-6));
    }

    #[test]
    fn clip_safe_does_not_touch_the_monitor() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        h.set_param("rec_gain", 12.0);
        *h.input_mut("in_l") = [0.9; BATCH];
        let before = h.render(2, &["out_l"]).remove(0);
        h.set_param("rec_clip_safe", 1.0);
        let after = h.render(2, &["out_l"]).remove(0);
        assert_eq!(before, after);
        assert!(after[0] > 1.0, "the monitor is not limited");
    }

    #[test]
    fn playback_passes_through_with_monitoring_off_by_default() {
        let mut h = rig();
        h.set_playback(&[ramp(0.1), ramp(-0.4)]);
        *h.input_mut("in_l") = [0.9; BATCH];
        *h.input_mut("in_r") = [0.9; BATCH];
        let out = h.render(3, &["out_l", "out_r"]);
        for k in 0..3 {
            assert_eq!(out[0][k * BATCH..(k + 1) * BATCH], ramp(0.1));
            assert_eq!(out[1][k * BATCH..(k + 1) * BATCH], ramp(-0.4));
        }
    }

    #[test]
    fn monitor_on_adds_the_input_to_the_playback() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        h.set_playback(&[ramp(0.1), ramp(0.2)]);
        *h.input_mut("in_l") = [0.5; BATCH];
        *h.input_mut("in_r") = [-0.25; BATCH];
        h.process();
        let (l, r) = (*h.output("out_l"), *h.output("out_r"));
        for i in 0..BATCH {
            assert_eq!(l[i].to_bits(), (ramp(0.1)[i] + 0.5).to_bits());
            assert_eq!(r[i].to_bits(), (ramp(0.2)[i] - 0.25).to_bits());
        }
    }

    #[test]
    fn monitor_off_never_passes_the_input_even_when_live() {
        let mut h = rig();
        h.set_param("monitor", 0.0);
        h.set_param("internal.monitor_live", 1.0);
        *h.input_mut("in_l") = [0.5; BATCH];
        let out = h.render(4, &["out_l"]);
        assert!(out[0].iter().all(|s| s.to_bits() == 0));
    }

    #[test]
    fn auto_follows_monitor_live() {
        let mut h = rig();
        *h.input_mut("in_l") = [0.5; BATCH];
        *h.input_mut("in_r") = [0.5; BATCH];
        let silent = h.render(2, &["out_l"]);
        assert!(silent[0].iter().all(|s| s.to_bits() == 0));
        h.set_param("internal.monitor_live", 1.0);
        let heard = h.render(8, &["out_l"]);
        assert_eq!(heard[0].last().unwrap().to_bits(), 0.5f32.to_bits());
        h.set_param("internal.monitor_live", 0.0);
        let gone = h.render(8, &["out_l"]);
        assert!(gone[0].last().unwrap().abs() < 1e-7);
    }

    #[test]
    fn monitor_change_is_a_five_ms_linear_crossfade() {
        let mut h = rig();
        *h.input_mut("in_l") = [1.0; BATCH];
        h.render(1, &["out_l"]);
        h.set_param("monitor", 2.0);
        let y = h.render(5, &["out_l"]).remove(0);
        let ramp_len = 240; // 5 ms at 48 kHz
        for (n, v) in y.iter().enumerate().take(ramp_len) {
            let want = (n + 1) as f32 / ramp_len as f32;
            assert!((v - want).abs() < 1e-5, "sample {n}: {v} vs {want}");
        }
        assert!(y[ramp_len..].iter().all(|s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn params_set_before_the_first_batch_snap_without_a_ramp() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        *h.input_mut("in_l") = [1.0; BATCH];
        let y = h.render(1, &["out_l"]).remove(0);
        assert!(y.iter().all(|s| s.to_bits() == 1.0f32.to_bits()));
    }

    #[test]
    fn without_playback_only_the_monitor_is_heard() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        *h.input_mut("in_l") = ramp(0.3);
        *h.input_mut("in_r") = ramp(0.6);
        h.process();
        assert_eq!(*h.output("out_l"), ramp(0.3));
        assert_eq!(*h.output("out_r"), ramp(0.6));
        // One playback channel only: the missing channel adds nothing.
        h.set_playback(&[[0.5; BATCH]]);
        h.process();
        assert_eq!(h.output("out_l")[0].to_bits(), (0.3f32 + 0.5).to_bits());
        assert_eq!(h.output("out_r")[0].to_bits(), 0.6f32.to_bits());
    }

    #[test]
    fn unconnected_inputs_and_no_playback_are_silent_and_allocation_free() {
        let mut h = rig();
        for monitor in [0.0, 1.0, 2.0] {
            h.set_param("monitor", monitor);
            h.set_param("internal.monitor_live", 1.0);
            let y = h.render(4, &["out_l", "out_r"]);
            assert!(y.iter().flatten().all(|s| s.to_bits() == 0));
        }
        h.set_playback(&[ramp(0.0), ramp(0.0)]);
        h.render(50, &["out_l", "out_r"]); // process is guarded by the harness
        atomik_testkit::no_alloc::assert_no_alloc(|| h.device_mut().reset());
        h.process();
    }

    fn tap_frames(h: &atomik_testkit::harness::DeviceHarness, n: usize) -> (Vec<f32>, Vec<f32>) {
        let tap = h.device().scope_tap().expect("a tap");
        let (mut l, mut r) = (vec![9.0; n], vec![9.0; n]);
        tap.read_latest(&mut l, &mut r);
        (l, r)
    }

    #[test]
    fn the_tuner_params_are_additive_with_defaults() {
        let reference = DESCRIPTOR.param_by_key("tuner_ref").unwrap();
        assert_eq!(
            (reference.min, reference.max, reference.default),
            (415.0, 466.0, 440.0)
        );
        assert_eq!(reference.unit, Unit::Hz);
        assert!(reference.discrete);
        let mute = DESCRIPTOR.param_by_key("tuner_mute").unwrap();
        assert_eq!((mute.default, mute.unit), (1.0, Unit::Toggle));
        let live = DESCRIPTOR.param_by_key("internal.tuner_live").unwrap();
        assert_eq!((live.default, live.unit), (0.0, Unit::Toggle));
        // The existing params keep their ids: a project stores params by key, the engine by id.
        for (i, key) in [
            "monitor",
            "internal.monitor_live",
            "rec_gain",
            "rec_clip_safe",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(DESCRIPTOR.param_by_key(key).unwrap().id, ParamId(i as u32));
        }
    }

    #[test]
    fn the_tap_holds_the_gained_input_even_unarmed_and_unmonitored() {
        let mut h = rig();
        let tap = h.device().scope_tap().expect("a tap");
        assert!(Arc::ptr_eq(&tap, &h.device().scope_tap().unwrap()));
        h.set_param("monitor", 0.0);
        h.set_param("rec_gain", 6.0);
        *h.input_mut("in_l") = ramp(0.01);
        *h.input_mut("in_r") = [-0.2; BATCH];
        h.set_playback(&[ramp(0.5), ramp(0.5)]);
        h.render(3, &["out_l"]);
        assert_eq!(tap.head(), (3 * BATCH) as u64);
        let g = atomik_core::db_to_gain(6.0);
        let (l, r) = tap_frames(&h, BATCH);
        for i in 0..BATCH {
            assert_eq!(l[i].to_bits(), (ramp(0.01)[i] * g).to_bits(), "{i}");
            assert_eq!(r[i].to_bits(), (-0.2f32 * g).to_bits());
        }
        // An unconnected input is silence.
        *h.input_mut("in_l") = [0.0; BATCH];
        h.process();
        assert!(tap_frames(&h, 8).0.iter().all(|s| s.to_bits() == 0));
    }

    #[test]
    fn the_tuner_mutes_the_track_over_five_ms_unless_the_option_is_off() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        *h.input_mut("in_l") = [0.5; BATCH];
        h.set_playback(&[[0.25; BATCH], [0.25; BATCH]]);
        let before = h.render(2, &["out_l"]).remove(0);
        assert!(before.iter().all(|s| s.to_bits() == 0.75f32.to_bits()));
        // Showing the tuner fades the whole output (playback and monitor) out.
        h.set_param("internal.tuner_live", 1.0);
        let y = h.render(5, &["out_l", "out_r"]);
        let ramp_len = 240;
        for (n, v) in y[0].iter().enumerate().take(ramp_len) {
            let want = 0.75 * (1.0 - (n + 1) as f32 / ramp_len as f32);
            assert!((v - want).abs() < 1e-5, "sample {n}: {v} vs {want}");
        }
        assert!(y[0][ramp_len..].iter().all(|s| s.abs() < 1e-7));
        assert!(y[1].last().unwrap().abs() < 1e-7);
        // The tap still hears the input: the tuner needs it.
        assert!(tap_frames(&h, 4).0.iter().all(|s| (s - 0.5).abs() < 1e-6));
        // Hiding it brings the sound back.
        h.set_param("internal.tuner_live", 0.0);
        let back = h.render(5, &["out_l"]).remove(0);
        assert!((back.last().unwrap() - 0.75).abs() < 1e-6);
        // With the option off the tuner shows and the track keeps sounding.
        h.set_param("tuner_mute", 0.0);
        h.set_param("internal.tuner_live", 1.0);
        let open = h.render(5, &["out_l"]).remove(0);
        assert!(open.iter().all(|s| (s - 0.75).abs() < 1e-6));
    }

    #[test]
    fn a_tuner_shown_before_the_first_batch_mutes_without_a_ramp() {
        let mut h = rig();
        h.set_param("monitor", 2.0);
        h.set_param("internal.tuner_live", 1.0);
        *h.input_mut("in_l") = [0.5; BATCH];
        let y = h.render(1, &["out_l"]).remove(0);
        assert!(y.iter().all(|s| s.to_bits() == 0));
    }
}
