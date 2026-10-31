//! `atomik.compressor`: stereo feed-forward compressor with sidechain.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! key = sidechain (when `sc_enable` and `sc_in_l/r` connected) else main input
//! key -> HP (`sc_hp`) -> LP (`sc_lp`) -> level (peak or RMS) in dB -> stereo link
//!     -> gain computer (soft knee) -> ballistics (dB domain) -> gain reduction GR
//! main -> lookahead delay -> x 10^(-GR/20) x makeup -> mix with the (equally delayed) dry
//! ```
//!
//! * **Detector.** *Peak*: `|x|` per channel. *RMS*: one-pole mean square with a 10 ms time
//!   constant, sine-calibrated (`sqrt(2 ms)`, a sine of peak `A` reads `A` in both modes). Levels
//!   are converted to dB with the deterministic [`atomik_dsp::log2_det`].
//! * **Stereo link.** Each channel has a level `l_c` in dB; the detector level of channel `c` is
//!   `l_c + link * (max(l_L, l_R) - l_c)`: `stereo_link = 1` reduces both channels by the same
//!   amount (the louder one decides), `0` compresses the channels independently.
//! * **Gain computer** (dB, threshold `T`, ratio `R`, knee width `W` centred on `T`):
//!   `x < T - W/2`: 0; inside the knee `(1 - 1/R)(x - T + W/2)^2 / (2W)`; above
//!   `(1 - 1/R)(x - T)`. This is [`atomik_dsp::static_gain_reduction_knee_db`] (the same formulas
//!   as the console compressor). `limit` on makes `R` infinite (a limiter, slope 1); `ratio`
//!   itself runs 1 to 20.
//! * **Ballistics** (smoothing of the GR `c` in dB; the tests measure the constants):
//!   `y1 = max(c, a_r*y1 + (1 - a_r)*c)` (release with peak hold), then
//!   `gr = a_a*gr + (1 - a_a)*y1` while `y1 > gr` (attack), else `gr = y1`. With
//!   `a = exp(-1/(tau*sr))`: **attack and release are the 63.2 % / 36.8 % one-pole time
//!   constants** of the GR in dB. The attack time is the time for a GR step to reach 63.2 % of
//!   its final value; the release time is the time for the GR to fall to 36.8 % of its starting
//!   value (to 0 dB GR, with the input well below the threshold).
//! * **Auto release** (program dependent, dual release): the `release` knob sets the *slow*
//!   constant; the fast one is `max(release / 8, 2 ms)`. The GR runs through a fast peak-hold
//!   release stage and, in parallel, through a slow stage that is fed by an integrated GR (the
//!   fast stage's output followed with a `release / 2` time constant when rising, at once when
//!   falling), so only *sustained* compression
//!   engages the slow release: after a short transient the gain recovers fast, after long heavy
//!   compression slowly. The two stages are combined with `max`.
//! * **Lookahead.** `lookahead` 0 to 5 ms delays the audio path (and the dry path) by
//!   `round(lookahead * sr / 1000)` samples while the detector sees the undelayed key, so the gain
//!   reduction arrives before the peak. The delay is the reported latency.
//! * **Latency decision (dynamic).** `latency_frames()` returns the *current* lookahead in
//!   samples. The engine publishes device latencies after every batch boundary's parameter
//!   changes and bumps `latency_epoch` when one changes (arena publish, `fill_latencies`), so a
//!   host that recompiles on `latency_epoch` re-aligns PDC after the user moves the knob. A
//!   lookahead change moves the read position of the delay at once (a click on programme material,
//!   as with any delay-time change); automating it is not recommended. The ring buffer is
//!   allocated for the 5 ms maximum in `prepare`.
//! * **Makeup.** `makeup` (0 to 24 dB, total capped at 36 dB) plus, with `auto_makeup`, the gain that would bring a 0 dBFS
//!   input back to 0 dBFS: `+GR_static(0 dB)` for the current threshold, ratio and knee. Makeup
//!   and mix are smoothed per batch (BlockSmoother, linear ramps).
//! * **Mix.** `out = dry * (1 - mix) + wet * mix`, where dry is the input **delayed by the same
//!   lookahead** (aligned), so `mix = 0` is the delayed input bit for bit (the input itself at
//!   lookahead 0).
//! * **`gr_cv_out`** is `1 - g`, with `g` the smaller of the two channel gains (linear, before
//!   makeup and mix): 0 at rest, towards 1 with heavy reduction (the console's convention).
//! * **Meter values** (`has_meter_values`): `[input peak (linear, max of L/R over the batch),
//!   output peak (same), gain reduction in dB (the largest GR of either channel in the batch)]`.
//! * **Safety.** Non-finite input samples are treated as 0; all state is `f64` with denormal
//!   flushes; the per-sample path uses only `+ - * /` and `sqrt` plus the deterministic
//!   `log2_det`/`exp2_det`.

use crate::bypass::{Bypassable, bypass_param};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, exp2_det, exp2_det_f64, log2_det,
    static_gain_reduction_knee_db,
};

#[cfg(test)]
mod tests;

/// Longest lookahead in ms.
pub const MAX_LOOKAHEAD_MS: f32 = 5.0;
/// RMS detector time constant in ms.
pub const RMS_WINDOW_MS: f32 = 10.0;
/// Largest total makeup (manual + automatic) in dB.
const MAX_MAKEUP_DB: f32 = 36.0;
/// Fast release of the auto-release mode relative to `release`.
const AUTO_FAST_DIV: f32 = 8.0;
/// Shortest fast release in auto mode, ms.
const AUTO_FAST_MIN_MS: f32 = 2.0;

const P_THRESHOLD: u32 = 0;
const P_RATIO: u32 = 1;
const P_LIMIT: u32 = 2;
const P_KNEE: u32 = 3;
const P_ATTACK: u32 = 4;
const P_RELEASE: u32 = 5;
const P_AUTO_RELEASE: u32 = 6;
const P_LOOKAHEAD: u32 = 7;
const P_MAKEUP: u32 = 8;
const P_AUTO_MAKEUP: u32 = 9;
const P_MIX: u32 = 10;
const P_SC_ENABLE: u32 = 11;
const P_SC_HP: u32 = 12;
const P_SC_LP: u32 = 13;
const P_STEREO_LINK: u32 = 14;
const P_DETECTOR: u32 = 15;

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

static PARAMS: [ParamDescriptor; 17] = [
    ParamDescriptor {
        id: ParamId(P_THRESHOLD),
        key: "threshold",
        name: "Threshold",
        min: -60.0,
        max: 0.0,
        default: -18.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_RATIO),
        key: "ratio",
        name: "Ratio",
        min: 1.0,
        max: 20.0,
        default: 4.0,
        taper: Taper::Exponential,
        unit: Unit::None,
        discrete: false,
    },
    toggle(P_LIMIT, "limit", "Limit (infinite ratio)"),
    ParamDescriptor {
        id: ParamId(P_KNEE),
        key: "knee",
        name: "Knee",
        min: 0.0,
        max: 24.0,
        default: 6.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_ATTACK),
        key: "attack",
        name: "Attack",
        min: 0.1,
        max: 200.0,
        default: 10.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_RELEASE),
        key: "release",
        name: "Release",
        min: 5.0,
        max: 2_000.0,
        default: 150.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    toggle(P_AUTO_RELEASE, "auto_release", "Auto release"),
    ParamDescriptor {
        id: ParamId(P_LOOKAHEAD),
        key: "lookahead",
        name: "Lookahead",
        min: 0.0,
        max: 5.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MAKEUP),
        key: "makeup",
        name: "Makeup",
        min: 0.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    toggle(P_AUTO_MAKEUP, "auto_makeup", "Auto makeup"),
    ParamDescriptor {
        id: ParamId(P_MIX),
        key: "mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    toggle(P_SC_ENABLE, "sc_enable", "Sidechain"),
    ParamDescriptor {
        id: ParamId(P_SC_HP),
        key: "sc_hp",
        name: "SC high-pass",
        min: 20.0,
        max: 1_000.0,
        default: 20.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_SC_LP),
        key: "sc_lp",
        name: "SC low-pass",
        min: 1_000.0,
        max: 20_000.0,
        default: 20_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_STEREO_LINK),
        key: "stereo_link",
        name: "Stereo link",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_DETECTOR),
        key: "detector",
        name: "Detector",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["peak", "rms"]),
        discrete: true,
    },
    bypass_param(16),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SC_L: SocketId = SocketId(2);
const SC_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const GR_CV: SocketId = SocketId(6);

static SOCKETS: [SocketDescriptor; 7] = [
    SocketDescriptor {
        id: IN_L,
        key: "in_l",
        name: "In L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: IN_R,
        key: "in_r",
        name: "In R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: SC_L,
        key: "sc_in_l",
        name: "Sidechain L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: SC_R,
        key: "sc_in_r",
        name: "Sidechain R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: GR_CV,
        key: "gr_cv_out",
        name: "GR CV",
        kind: SocketKind::Cv,
        direction: Direction::Output,
    },
];

/// Static description of the compressor.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.compressor",
    name: "Compressor",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Compressor`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Compressor::new())),
    }
}

/// `20 * log2(10) / 20` -> `log2(10) / 20`: `10^(-x/20) = 2^(-x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`: dB per octave of amplitude.
const DB_PER_LOG2: f32 = 6.020_6;
/// Level floor in dB (silence).
const LEVEL_FLOOR_DB: f32 = -200.0;

#[inline]
fn level_db(x: f32) -> f32 {
    (DB_PER_LOG2 * log2_det(x)).max(LEVEL_FLOOR_DB)
}

/// One-pole coefficient `exp(-1/(tau sr))` (0 for `tau <= 0`).
fn alpha(ms: f32, sr: f32) -> f64 {
    let n = f64::from(ms) * 0.001 * f64::from(sr);
    if n > 0.0 {
        exp2_det_f64(-std::f64::consts::LOG2_E / n)
    } else {
        0.0
    }
}

#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-18 { 0.0 } else { v }
}

/// Per-channel detector and ballistics state.
#[derive(Debug, Clone, Copy, Default)]
struct Chan {
    hp: Biquad,
    lp: Biquad,
    /// Mean square (RMS mode).
    ms: f64,
    /// Fast / only release stage (dB).
    y_fast: f64,
    /// Integrated GR (auto release).
    g_int: f64,
    /// Slow release stage (auto release).
    y_slow: f64,
    /// Smoothed gain reduction (dB).
    gr: f64,
}

/// Ballistic coefficients.
#[derive(Debug, Clone, Copy, Default)]
struct Coefs {
    attack: f64,
    release: f64,
    fast: f64,
    slow: f64,
    integ: f64,
    rms: f64,
}

/// The compressor device.
#[derive(Debug, Clone)]
pub struct Compressor {
    sample_rate: f32,
    threshold: f32,
    ratio: f32,
    limit: bool,
    knee: f32,
    attack_ms: f32,
    release_ms: f32,
    auto_release: bool,
    lookahead_ms: f32,
    makeup_db: f32,
    auto_makeup: bool,
    mix: f32,
    sc_enable: bool,
    sc_hp: f32,
    sc_lp: f32,
    link: f32,
    rms_mode: bool,
    // Derived.
    coefs: Coefs,
    hp_coeffs: BiquadCoeffs,
    lp_coeffs: BiquadCoeffs,
    hp_on: bool,
    lp_on: bool,
    lookahead_samples: usize,
    // State.
    fresh: bool,
    ch: [Chan; 2],
    ring: [Vec<f32>; 2],
    mask: usize,
    wpos: usize,
    makeup_sm: BlockSmoother,
    mix_sm: BlockSmoother,
    meters: [f32; 3],
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    /// A compressor with default params; its delay is sized by `prepare`.
    pub fn new() -> Self {
        let mut c = Self {
            sample_rate: 48_000.0,
            threshold: -18.0,
            ratio: 4.0,
            limit: false,
            knee: 6.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            auto_release: false,
            lookahead_ms: 0.0,
            makeup_db: 0.0,
            auto_makeup: false,
            mix: 1.0,
            sc_enable: false,
            sc_hp: 20.0,
            sc_lp: 20_000.0,
            link: 1.0,
            rms_mode: false,
            coefs: Coefs::default(),
            hp_coeffs: BiquadCoeffs::IDENTITY,
            lp_coeffs: BiquadCoeffs::IDENTITY,
            hp_on: false,
            lp_on: false,
            lookahead_samples: 0,
            fresh: true,
            ch: [Chan::default(); 2],
            ring: [vec![0.0; 1], vec![0.0; 1]],
            mask: 0,
            wpos: 0,
            makeup_sm: BlockSmoother::new(1.0),
            mix_sm: BlockSmoother::new(1.0),
            meters: [0.0; 3],
        };
        c.derive();
        c
    }

    /// Recomputes every derived value from the raw params and the sample rate.
    fn derive(&mut self) {
        let sr = self.sample_rate;
        let (fast_ms, slow_ms) = if self.auto_release {
            (
                (self.release_ms / AUTO_FAST_DIV).max(AUTO_FAST_MIN_MS),
                self.release_ms,
            )
        } else {
            (self.release_ms, self.release_ms)
        };
        self.coefs = Coefs {
            attack: alpha(self.attack_ms, sr),
            release: alpha(self.release_ms, sr),
            fast: alpha(fast_ms, sr),
            slow: alpha(slow_ms, sr),
            integ: alpha(slow_ms * 0.5, sr),
            rms: alpha(RMS_WINDOW_MS, sr),
        };
        self.hp_on = self.sc_hp > 20.5;
        self.lp_on = self.sc_lp < 19_999.0;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let sr64 = f64::from(sr);
        self.hp_coeffs =
            BiquadCoeffs::design(BiquadShape::Highpass, f64::from(self.sc_hp), q, 0.0, sr64);
        self.lp_coeffs =
            BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(self.sc_lp), q, 0.0, sr64);
        let max = self.mask; // ring length - 1; the largest usable delay
        let want = (self.lookahead_ms * 0.001 * sr).round() as usize;
        self.lookahead_samples = want.min(max.saturating_sub(1));
    }

    /// The ratio used by the gain computer (infinite when `limit` is on).
    #[inline]
    fn effective_ratio(&self) -> f32 {
        if self.limit {
            f32::INFINITY
        } else {
            self.ratio
        }
    }

    /// Static GR (dB) at a detector level.
    #[inline]
    fn static_gr(&self, level: f32) -> f32 {
        static_gain_reduction_knee_db(level, self.threshold, self.effective_ratio(), self.knee)
    }

    /// Makeup gain in dB (manual plus the automatic part).
    fn makeup_total_db(&self) -> f32 {
        let auto = if self.auto_makeup {
            self.static_gr(0.0)
        } else {
            0.0
        };
        (self.makeup_db + auto).min(MAX_MAKEUP_DB)
    }

    /// One sample of ballistics for channel `c`; returns the smoothed GR in dB.
    #[inline]
    fn ballistics(&mut self, c_idx: usize, target: f64) -> f64 {
        let k = self.coefs;
        let auto = self.auto_release;
        let ch = &mut self.ch[c_idx];
        let y1 = if auto {
            ch.y_fast = flush(if ch.y_fast > target {
                target + k.fast * (ch.y_fast - target)
            } else {
                target
            });
            // The integrator is fed by the held fast stage (not the rippling rectified target).
            let held = ch.y_fast;
            ch.g_int = flush(if held > ch.g_int {
                held + k.integ * (ch.g_int - held)
            } else {
                held
            });
            ch.y_slow = flush(if ch.y_slow > ch.g_int {
                ch.g_int + k.slow * (ch.y_slow - ch.g_int)
            } else {
                ch.g_int
            });
            ch.y_fast.max(ch.y_slow)
        } else {
            ch.y_fast = flush(if ch.y_fast > target {
                target + k.release * (ch.y_fast - target)
            } else {
                target
            });
            ch.y_fast
        };
        ch.gr = flush(if y1 > ch.gr {
            y1 + k.attack * (ch.gr - y1)
        } else {
            y1
        });
        ch.gr
    }
}

impl Device for Compressor {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        let need = (MAX_LOOKAHEAD_MS * 0.001 * self.sample_rate).ceil() as usize + 2;
        let len = need.next_power_of_two();
        self.ring = [vec![0.0; len], vec![0.0; len]];
        self.mask = len - 1;
        self.makeup_sm.set_time(20.0, self.sample_rate, BATCH);
        self.mix_sm.set_time(20.0, self.sample_rate, BATCH);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.ch = [Chan::default(); 2];
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.wpos = 0;
        self.meters = [0.0; 3];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_THRESHOLD => self.threshold = v.clamp(-60.0, 0.0),
            P_RATIO => self.ratio = v.clamp(1.0, 20.0),
            P_LIMIT => self.limit = v >= 0.5,
            P_KNEE => self.knee = v.clamp(0.0, 24.0),
            P_ATTACK => self.attack_ms = v.clamp(0.1, 200.0),
            P_RELEASE => self.release_ms = v.clamp(5.0, 2_000.0),
            P_AUTO_RELEASE => self.auto_release = v >= 0.5,
            P_LOOKAHEAD => self.lookahead_ms = v.clamp(0.0, MAX_LOOKAHEAD_MS),
            P_MAKEUP => self.makeup_db = v.clamp(0.0, 24.0),
            P_AUTO_MAKEUP => self.auto_makeup = v >= 0.5,
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_SC_ENABLE => self.sc_enable = v >= 0.5,
            P_SC_HP => self.sc_hp = v.clamp(20.0, 1_000.0),
            P_SC_LP => self.sc_lp = v.clamp(1_000.0, 20_000.0),
            P_STEREO_LINK => self.link = v.clamp(0.0, 1.0),
            P_DETECTOR => self.rms_mode = v >= 0.5,
            _ => return,
        }
        self.derive();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let sc_l_conn = ctx.is_connected(SC_L);
        let sc_r_conn = ctx.is_connected(SC_R);
        let use_sc = self.sc_enable && (sc_l_conn || sc_r_conn);
        let sc_l = *ctx.input(SC_L);
        let sc_r = *ctx.input(SC_R);

        let makeup_target = exp2_det(self.makeup_total_db() * LOG2_10_20);
        if self.fresh {
            self.makeup_sm.set_immediate(makeup_target);
            self.mix_sm.set_immediate(self.mix);
            self.fresh = false;
        }
        let makeup = self.makeup_sm.advance(makeup_target, BATCH);
        let mix = self.mix_sm.advance(self.mix, BATCH);

        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        let delay = self.lookahead_samples;
        let mask = self.mask;
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        let (mut pk_in, mut pk_out) = (0.0f32, 0.0f32);
        let mut gr_max = 0.0f64;
        let rms_a = self.coefs.rms;

        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            pk_in = pk_in.max(x[0].abs()).max(x[1].abs());

            // Key signal: sidechain or main; a lone connected sidechain side feeds both.
            let key = if use_sc {
                let (a, b) = (finite(sc_l[n]), finite(sc_r[n]));
                match (sc_l_conn, sc_r_conn) {
                    (true, false) => [a, a],
                    (false, true) => [b, b],
                    _ => [a, b],
                }
            } else {
                x
            };

            // Per-channel levels in dB.
            let mut lvl = [0.0f32; 2];
            for c in 0..2 {
                let mut k = f64::from(key[c]);
                if self.hp_on {
                    k = self.ch[c].hp.process(&self.hp_coeffs, k);
                }
                if self.lp_on {
                    k = self.ch[c].lp.process(&self.lp_coeffs, k);
                }
                let k = k as f32;
                let amp = if self.rms_mode {
                    let ch = &mut self.ch[c];
                    let x2 = f64::from(k) * f64::from(k);
                    ch.ms = flush(x2 + rms_a * (ch.ms - x2));
                    ((2.0 * ch.ms).sqrt()) as f32
                } else {
                    k.abs()
                };
                lvl[c] = level_db(amp);
            }
            let top = lvl[0].max(lvl[1]);
            let mut gain = [1.0f32; 2];
            for c in 0..2 {
                let det = lvl[c] + self.link * (top - lvl[c]);
                let target = f64::from(self.static_gr(det));
                let gr = self.ballistics(c, target);
                gr_max = gr_max.max(gr);
                gain[c] = exp2_det(-(gr as f32) * LOG2_10_20);
            }
            cv[n] = 1.0 - gain[0].min(gain[1]);

            // Audio path through the lookahead delay (dry and wet share it).
            let w = self.wpos;
            let mut y = [0.0f32; 2];
            let m = mix.at(n);
            let g = makeup.at(n);
            for c in 0..2 {
                self.ring[c][w & mask] = x[c];
                let d = self.ring[c][w.wrapping_sub(delay) & mask];
                y[c] = if m == 0.0 {
                    d
                } else {
                    d * (1.0 - m) + d * gain[c] * g * m
                };
            }
            self.wpos = w.wrapping_add(1);
            out_l[n] = y[0];
            out_r[n] = y[1];
            pk_out = pk_out.max(y[0].abs()).max(y[1].abs());
        }
        self.meters = [pk_in, pk_out, gr_max as f32];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(GR_CV) = cv;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        self.lookahead_samples as u32
    }

    fn variable_latency(&self) -> bool {
        true
    }
}
