//! `atomik.line-mixer`: six stereo channels into a stereo master (03 §4.2).
//!
//! Per channel: level (dB, 5 ms linear smoothing), constant-power pan (−3 dB centre), mute,
//! solo and a level CV input. If only one side of a channel is connected the channel is mono and
//! feeds both sides before panning (03 §4.2 specifies the `_l`-only case; an `_r`-only channel is
//! treated the same way). Stereo channels are panned with the same law: `L·cos θ`, `R·sin θ`.

use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{Smoother, pan_constant_power};

/// Number of stereo channels.
pub const CHANNELS: usize = 6;
/// Params per channel (level, pan, mute, solo).
const PARAMS_PER_CHANNEL: usize = 4;
/// Sockets per channel (in_l, in_r, level_cv_in).
const SOCKETS_PER_CHANNEL: usize = 3;
/// Id of `master.level`.
const MASTER_LEVEL: usize = CHANNELS * PARAMS_PER_CHANNEL;
/// Id of `master_l` (`master_r` follows).
const MASTER_L: u16 = (CHANNELS * SOCKETS_PER_CHANNEL) as u16;
/// Gain smoothing time.
const SMOOTH_MS: f32 = 5.0;

const fn level(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: -60.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        discrete: false,
    }
}

const fn pan(id: u32, key: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name: "Pan",
        min: -1.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    }
}

const fn toggle(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    }
}

const fn socket(id: u16, key: &'static str, kind: SocketKind, dir: Direction) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind,
        direction: dir,
    }
}

macro_rules! channel_params {
    ($c:literal, $base:expr) => {
        [
            level($base, concat!("ch", $c, ".level"), "Level"),
            pan($base + 1, concat!("ch", $c, ".pan")),
            toggle($base + 2, concat!("ch", $c, ".mute"), "Mute"),
            toggle($base + 3, concat!("ch", $c, ".solo"), "Solo"),
        ]
    };
}

macro_rules! channel_sockets {
    ($c:literal, $base:expr) => {
        [
            socket(
                $base,
                concat!("ch", $c, "_in_l"),
                SocketKind::Audio,
                Direction::Input,
            ),
            socket(
                $base + 1,
                concat!("ch", $c, "_in_r"),
                SocketKind::Audio,
                Direction::Input,
            ),
            socket(
                $base + 2,
                concat!("ch", $c, "_level_cv_in"),
                SocketKind::Cv,
                Direction::Input,
            ),
        ]
    };
}

static PARAMS: [ParamDescriptor; MASTER_LEVEL + 1] = {
    let ch = [
        channel_params!("1", 0),
        channel_params!("2", 4),
        channel_params!("3", 8),
        channel_params!("4", 12),
        channel_params!("5", 16),
        channel_params!("6", 20),
    ];
    let mut out = [level(MASTER_LEVEL as u32, "master.level", "Master level"); MASTER_LEVEL + 1];
    let mut i = 0;
    while i < MASTER_LEVEL {
        out[i] = ch[i / PARAMS_PER_CHANNEL][i % PARAMS_PER_CHANNEL];
        i += 1;
    }
    out
};

static SOCKETS: [SocketDescriptor; MASTER_L as usize + 2] = {
    let ch = [
        channel_sockets!("1", 0),
        channel_sockets!("2", 3),
        channel_sockets!("3", 6),
        channel_sockets!("4", 9),
        channel_sockets!("5", 12),
        channel_sockets!("6", 15),
    ];
    let master_r = socket(
        MASTER_L + 1,
        "master_r",
        SocketKind::Audio,
        Direction::Output,
    );
    let mut out = [master_r; MASTER_L as usize + 2];
    let mut i = 0;
    while i < MASTER_L as usize {
        out[i] = ch[i / SOCKETS_PER_CHANNEL][i % SOCKETS_PER_CHANNEL];
        i += 1;
    }
    out[MASTER_L as usize] = socket(MASTER_L, "master_l", SocketKind::Audio, Direction::Output);
    out
};

/// Static description of the line mixer.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.line-mixer",
    name: "Line Mixer",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`LineMixer`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(LineMixer::new()),
    }
}

/// The line mixer device.
#[derive(Debug, Clone)]
pub struct LineMixer {
    sample_rate: f32,
    level_db: [f32; CHANNELS],
    pan: [f32; CHANNELS],
    mute: [bool; CHANNELS],
    solo: [bool; CHANNELS],
    master_db: f32,
    gain_l: [Smoother; CHANNELS],
    gain_r: [Smoother; CHANNELS],
    master: Smoother,
    /// Set by `prepare`/`reset`: the next `process` snaps every smoother to its target, so
    /// params set before the first batch apply at once.
    fresh: bool,
}

impl Default for LineMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl LineMixer {
    /// A mixer with default params (call `prepare` before processing).
    pub fn new() -> Self {
        let sm = Smoother::new(0.0, SMOOTH_MS, 48_000.0);
        Self {
            sample_rate: 48_000.0,
            level_db: [0.0; CHANNELS],
            pan: [0.0; CHANNELS],
            mute: [false; CHANNELS],
            solo: [false; CHANNELS],
            master_db: 0.0,
            gain_l: [sm; CHANNELS],
            gain_r: [sm; CHANNELS],
            master: sm,
            fresh: true,
        }
    }

    /// Recomputes every smoother target from the current params (solo affects all channels).
    fn update_targets(&mut self) {
        let any_solo = self.solo.iter().any(|s| *s);
        for c in 0..CHANNELS {
            let audible = !self.mute[c] && (!any_solo || self.solo[c]);
            let g = if audible {
                PARAMS[c * PARAMS_PER_CHANNEL].db_param_gain(self.level_db[c])
            } else {
                0.0
            };
            let (pl, pr) = pan_constant_power(self.pan[c]);
            self.gain_l[c].set_target(g * pl);
            self.gain_r[c].set_target(g * pr);
        }
        self.master
            .set_target(PARAMS[MASTER_LEVEL].db_param_gain(self.master_db));
    }

    /// Jumps every smoother to its target.
    fn snap(&mut self) {
        for sm in self
            .gain_l
            .iter_mut()
            .chain(self.gain_r.iter_mut())
            .chain(std::iter::once(&mut self.master))
        {
            let t = sm.target();
            sm.set_immediate(t);
        }
    }
}

impl Device for LineMixer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        for sm in self
            .gain_l
            .iter_mut()
            .chain(self.gain_r.iter_mut())
            .chain(std::iter::once(&mut self.master))
        {
            sm.set_time(SMOOTH_MS, self.sample_rate);
        }
        self.update_targets();
        self.snap();
        self.fresh = true;
    }

    fn reset(&mut self) {
        self.update_targets();
        self.snap();
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i == MASTER_LEVEL {
            self.master_db = plain;
        } else if i < MASTER_LEVEL {
            let c = i / PARAMS_PER_CHANNEL;
            match i % PARAMS_PER_CHANNEL {
                0 => self.level_db[c] = plain,
                1 => self.pan[c] = plain,
                2 => self.mute[c] = plain >= 0.5,
                _ => self.solo[c] = plain >= 0.5,
            }
        } else {
            return;
        }
        self.update_targets();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            self.snap();
            self.fresh = false;
        }
        let mut sum_l = [0.0f32; BATCH];
        let mut sum_r = [0.0f32; BATCH];
        for c in 0..CHANNELS {
            let base = (c * SOCKETS_PER_CHANNEL) as u16;
            let (sl, sr, scv) = (SocketId(base), SocketId(base + 1), SocketId(base + 2));
            let (has_l, has_r) = (ctx.is_connected(sl), ctx.is_connected(sr));
            // Mono fallback: a single connected side feeds both sides before panning.
            let in_l = if has_l || !has_r {
                *ctx.input(sl)
            } else {
                *ctx.input(sr)
            };
            let in_r = if has_r && has_l { *ctx.input(sr) } else { in_l };
            let cv_connected = ctx.is_connected(scv);
            let cv = ctx.input(scv);
            let (gl, gr) = (&mut self.gain_l[c], &mut self.gain_r[c]);
            for n in 0..BATCH {
                let k = if cv_connected {
                    cv[n].clamp(0.0, 1.0)
                } else {
                    1.0
                };
                sum_l[n] += in_l[n] * gl.next_value() * k;
                sum_r[n] += in_r[n] * gr.next_value() * k;
            }
        }
        for n in 0..BATCH {
            let m = self.master.next_value();
            sum_l[n] *= m;
            sum_r[n] *= m;
        }
        *ctx.output(SocketId(MASTER_L)) = sum_l;
        *ctx.output(SocketId(MASTER_L + 1)) = sum_r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::signal::max_step;

    const OUT: [&str; 2] = ["master_l", "master_r"];

    fn mixer() -> DeviceHarness {
        harness(Box::new(LineMixer::new()))
    }

    fn fill(h: &mut DeviceHarness, key: &str, v: f32) {
        *h.input_mut(key) = [v; BATCH];
    }

    fn settled(h: &mut DeviceHarness) -> (f32, f32) {
        let o = h.render(10, &OUT); // 640 samples > 5 ms
        (*o[0].last().unwrap(), *o[1].last().unwrap())
    }

    #[test]
    fn descriptor_validates() {
        let mut params = Vec::new();
        let mut sockets = Vec::new();
        for n in 1..=CHANNELS {
            for p in ["level", "pan", "mute", "solo"] {
                params.push(format!("ch{n}.{p}"));
            }
            for s in ["in_l", "in_r", "level_cv_in"] {
                sockets.push(format!("ch{n}_{s}"));
            }
        }
        params.push("master.level".into());
        sockets.push("master_l".into());
        sockets.push("master_r".into());
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        let s: Vec<&str> = sockets.iter().map(String::as_str).collect();
        crate::test_support::assert_keys(&DESCRIPTOR, &p, &s);
        assert_eq!(DESCRIPTOR.type_id, "atomik.line-mixer");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Mixer);
        let lvl = DESCRIPTOR.param_by_key("ch3.level").unwrap();
        assert_eq!((lvl.taper, lvl.unit), (Taper::Decibel, Unit::Db));
        assert!(DESCRIPTOR.param_by_key("ch6.solo").unwrap().discrete);
        assert_eq!(
            DESCRIPTOR.socket_by_key("ch2_level_cv_in").unwrap().kind,
            SocketKind::Cv
        );
    }

    #[test]
    fn ac_d5_mono_input_center_pan_outputs_0_7071_both_sides() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        let (l, r) = settled(&mut h);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4, "L {l}");
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4, "R {r}");
    }

    #[test]
    fn ac_d6_solo_ch2_only_ch2_is_heard() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        fill(&mut h, "ch1_in_r", 1.0);
        fill(&mut h, "ch2_in_l", 0.25);
        fill(&mut h, "ch2_in_r", 0.25);
        h.set_param("ch2.solo", 1.0);
        let (l, r) = settled(&mut h);
        let expect = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (l - expect).abs() < 1e-5 && (r - expect).abs() < 1e-5,
            "{l} {r}"
        );
        // Muting the soloed channel silences everything.
        h.set_param("ch2.mute", 1.0);
        let (l, r) = settled(&mut h);
        assert!(l.abs() < 1e-7 && r.abs() < 1e-7);
        // Unsolo + unmute: both channels again.
        h.set_param("ch2.mute", 0.0);
        h.set_param("ch2.solo", 0.0);
        let (l, _) = settled(&mut h);
        assert!((l - 1.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn process_mute_silences_channel() {
        let mut h = mixer();
        fill(&mut h, "ch4_in_l", 0.5);
        fill(&mut h, "ch5_in_l", 0.5);
        h.set_param("ch4.mute", 1.0);
        let (l, r) = settled(&mut h);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((l - expect).abs() < 1e-5 && (r - expect).abs() < 1e-5);
    }

    #[test]
    fn process_pan_extremes_and_stereo_channel() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        h.set_param("ch1.pan", -1.0);
        let (l, r) = settled(&mut h);
        assert!((l - 1.0).abs() < 1e-5 && r.abs() < 1e-5);
        h.set_param("ch1.pan", 1.0);
        let (l, r) = settled(&mut h);
        assert!(l.abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
        // Stereo channel: L and R pass through their own sides with the pan law.
        let mut h = mixer();
        fill(&mut h, "ch3_in_l", 0.4);
        fill(&mut h, "ch3_in_r", -0.2);
        let (l, r) = settled(&mut h);
        let k = std::f32::consts::FRAC_1_SQRT_2;
        assert!((l - 0.4 * k).abs() < 1e-5 && (r + 0.2 * k).abs() < 1e-5);
    }

    #[test]
    fn process_level_cv_scales_channel_gain() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        h.set_param("ch1.pan", -1.0);
        for (cv, expect) in [(0.5, 0.5), (2.0, 1.0), (-1.0, 0.0), (0.0, 0.0)] {
            fill(&mut h, "ch1_level_cv_in", cv);
            let (l, _) = settled(&mut h);
            assert!((l - expect).abs() < 1e-5, "cv {cv}: {l}");
        }
        h.set_connected("ch1_level_cv_in", false);
        *h.input_mut("ch1_level_cv_in") = [0.0; BATCH];
        h.set_connected("ch1_level_cv_in", false);
        let (l, _) = settled(&mut h);
        assert!((l - 1.0).abs() < 1e-5, "unconnected CV is unity: {l}");
    }

    #[test]
    fn process_levels_and_master_apply_db() {
        let mut h = mixer();
        fill(&mut h, "ch6_in_l", 1.0);
        h.set_param("ch6.pan", -1.0);
        h.set_param("ch6.level", -6.0);
        h.set_param("master.level", 6.0);
        let (l, _) = settled(&mut h);
        assert!((l - 1.0).abs() < 1e-3, "{l}");
        h.set_param("master.level", -60.0);
        let (l, _) = settled(&mut h);
        assert!(l.abs() < 1e-9, "−60 dB is silence: {l}");
    }

    #[test]
    fn process_gain_change_is_smoothed_over_5ms() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        h.set_param("ch1.pan", -1.0);
        settled(&mut h);
        h.set_param("ch1.level", -60.0);
        let o = h.render(6, &OUT);
        let l = &o[0];
        // 5 ms at 48 kHz = 240 samples, linear: about half way at 120.
        assert!((l[119] - 0.5).abs() < 0.01, "mid-ramp {}", l[119]);
        assert!(l[238] > 0.0 && l[240].abs() < 1e-7, "{} {}", l[238], l[240]);
        assert!(max_step(l) < 1.0 / 200.0, "no jump: {}", max_step(l));
    }

    #[test]
    fn set_param_every_param_min_max_does_not_panic() {
        let mut h = mixer();
        for n in 1..=CHANNELS {
            fill(&mut h, &format!("ch{n}_in_l"), 0.5);
            fill(&mut h, &format!("ch{n}_level_cv_in"), 0.5);
        }
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn reset_silences_tails() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        h.render(4, &OUT);
        h.set_param("ch1.level", -60.0);
        h.device_mut().reset();
        // After reset, smoothers are at their targets: no ramp tail.
        let o = h.render(1, &OUT);
        assert!(o[0].iter().chain(&o[1]).all(|s| s.abs() < 1e-9));
    }

    #[test]
    fn process_is_allocation_free() {
        // Every process call in DeviceHarness runs under assert_no_alloc.
        let mut h = mixer();
        for n in 1..=CHANNELS {
            fill(&mut h, &format!("ch{n}_in_l"), 0.1 * n as f32);
            fill(&mut h, &format!("ch{n}_in_r"), -0.1);
            fill(&mut h, &format!("ch{n}_level_cv_in"), 0.7);
        }
        for i in 0..100 {
            h.set_param("ch2.solo", (i % 2) as f32);
            h.set_param("ch3.pan", (i as f32 / 50.0) - 1.0);
            h.process();
        }
        h.device_mut().reset();
        h.process();
    }

    #[test]
    fn params_set_before_the_first_batch_apply_without_ramp() {
        let mut h = mixer();
        fill(&mut h, "ch1_in_l", 1.0);
        fill(&mut h, "ch2_in_l", 0.5);
        h.set_param("ch1.level", -6.0);
        h.set_param("ch1.pan", -0.5);
        h.set_param("ch2.mute", 1.0);
        h.set_param("master.level", -3.0);
        crate::test_support::assert_first_batch_settled(&mut h, &OUT, 20);
        // And a later change still ramps.
        h.set_param("master.level", -60.0);
        h.process();
        assert!(
            h.output("master_l")[0].abs() > 0.1,
            "ramps after the first batch"
        );
    }
}
