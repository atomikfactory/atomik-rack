//! `atomik.gain-tool`: a channel-strip style utility for a stereo signal: polarity, mono, dual
//! pan, width, gain (trim or VCA fader), balance and mute (with an optional long fade), plus an
//! optional second stereo input to add or cross-fade in, and the meters of its panel.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in -> polarity (invert_l / invert_r) -> mono ((L+R)/2 on both sides)
//!    -> aux:  gain mode   s = x
//!             mix mode    s = x + aux * blend
//!             xfade mode  s = x * (1 - blend) + aux * blend
//!    -> dual pan (pan_l places the left input, pan_r the right one, with the pan law)
//!    -> width (mid/side, skipped at exactly 1)
//!    -> * gain * balance gains * mute fade gain -> out
//! ```
//!
//! Every stage that is neutral (polarity off, mono off, pan hard left / hard right, width 1,
//! fade 0) is skipped or multiplies by exactly 1, so a project saved before these stages existed
//! sounds bit for bit the same (`existing_projects_sound_bit_identical_after_the_upgrade`).
//!
//! * **Polarity** `invert_l` / `invert_r` flip the sign of that input channel before anything
//!   else. **Mono** (`mono`) replaces both channels by `(L + R) / 2`. Both are cross-faded
//!   (10 ms smoothing, ramped per sample) so switching never clicks.
//! * **Dual pan.** The left input is placed at `pan_l` (default -1, hard left) and the right
//!   input at `pan_r` (default +1, hard right), each with the `pan_law`:
//!   `out_l = l * pl_l + r * pr_l`, `out_r = l * pl_r + r * pr_r`. With position `x = (pan+1)/2`
//!   (0 hard left, 1 hard right) the gains `(to left, to right)` are
//!   `0 dB`: `(min(1, 2(1-x)), min(1, 2x))` (unity at the centre, as the balance control);
//!   `-3 dB`: `(cos(x pi/2), sin(x pi/2))` (constant power, 0.707 at the centre, the default);
//!   `-4.5 dB`: the geometric mean of the `-3 dB` and `-6 dB` gains (0.595 at the centre);
//!   `-6 dB`: `(1-x, x)` (linear, 0.5 at the centre). At the extremes every law gives exactly
//!   `(1, 0)` / `(0, 1)`, and the default placement (identity matrix) skips the stage.
//! * **Width** is the mid/side width of `atomik_dsp::stereo_width`: 0 is mono, 1 leaves the
//!   signal alone (the path is skipped), 2 doubles the side signal.
//! * **Gain** is `gain` (dB) plus `gain_cv_trim * gain_cv_in * 24 dB` (the console's level
//!   convention), converted with the deterministic `exp2_det`. `fader_mode` chooses how the
//!   fader value is read: **Trim** (0, default) uses it as is, -60..+24 dB. **VCA** (1) uses the
//!   same stored `gain`, but only its -inf..0 dB half: a gain at the bottom of the range
//!   (`gain <= -60`) is silence (-inf, CV ignored), anything above is `min(gain + CV dB, 0)`, so
//!   the fader never boosts. Switching mode ramps like any other gain change.
//! * **Balance** (`pan`, -1..+1, plus `pan_cv_trim * pan_cv_in`) is unity on both sides at the
//!   centre; moving it away from the centre lowers the far side linearly to silence at the end.
//! * **Mute** is the `mute` param OR a gate on `mute_cv_in` (above 0.5 mutes, below 0.3 releases).
//!   With `fade` = 0 (default) the gain ramps to 0 with the usual 10 ms smoothing. With
//!   `fade` > 0 (ms, up to 60 s) a fade position `p` (1 open, 0 muted) moves to its target in
//!   exactly `fade` (a fade reversed half way takes half as long) and the gain is
//!   `10^((p-1) * 90 / 20)`: equal steps in dB down to -90 dB, then exactly 0 at `p = 0`
//!   (and exactly 1 at `p = 1`). The gain and balance smoothing stays on top of it.
//! * **Smoothing.** Gain, balance gains, width, blend, polarity, mono and the pan matrix are
//!   smoothed per batch (10 ms, `BlockSmoother`) and ramped linearly across the batch; CV is its
//!   mean over the batch.
//! * **Meter values** ([`METER_COUNT`] = 8, indices are the `METER_*` consts): input peak L/R and
//!   output peak L/R (linear, the peak of the last 16 batches), the held output peak in dB (max
//!   since the last reset, -120 when silent), signal present (input above -60 dBFS), CV active
//!   (a connected CV input moved off 0 during the last 16 batches) and fading (1 while a mute
//!   fade is in progress). `peak_reset` is a momentary toggle: a rising edge clears the hold
//!   (leave it at 0 afterwards). `peak_auto` also clears the hold every 3 s of audio.
//! * No latency. Non-finite input samples count as 0. Allocation- and lock-free after
//!   construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::recent_peak::RecentPeak;
use crate::tool_support::{
    GateIn, audio_in, audio_out, balance_gains, choice, cv_in, finite, linear, toggle, trim,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, exp2_det, sin_cycles};

const P_MODE: u32 = 0;
const P_GAIN: u32 = 1;
const P_PAN: u32 = 2;
const P_WIDTH: u32 = 3;
const P_BLEND: u32 = 4;
const P_MUTE: u32 = 5;
const P_GAIN_TRIM: u32 = 6;
const P_PAN_TRIM: u32 = 7;
// 8 is `bypass`.
const P_FADE: u32 = 9;
const P_INVERT_L: u32 = 10;
const P_INVERT_R: u32 = 11;
const P_MONO: u32 = 12;
const P_PAN_L: u32 = 13;
const P_PAN_R: u32 = 14;
const P_PAN_LAW: u32 = 15;
const P_FADER_MODE: u32 = 16;
const P_PEAK_RESET: u32 = 17;
const P_PEAK_AUTO: u32 = 18;

/// The CV range of `gain_cv_in` at full trim, in dB.
const CV_GAIN_DB: f32 = 24.0;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The mute fade runs in equal dB steps from 0 dB down to this level, then to exact silence.
const FADE_RANGE_DB: f32 = 90.0;
/// Longest mute fade, in milliseconds.
const FADE_MAX_MS: f32 = 60_000.0;
/// A VCA fader at or below this gain (dB) is silence.
const VCA_BOTTOM_DB: f32 = -60.0;
/// Input level (linear) above which the signal-present flag is set: -60 dBFS.
const SIGNAL_THRESHOLD: f32 = 0.001;
/// Seconds between the automatic resets of the held peak.
const PEAK_AUTO_SECONDS: f32 = 3.0;
/// The held peak reads this (dB) while nothing has passed.
const HOLD_FLOOR_DB: f32 = -120.0;
/// Batches the CV-activity flag stays up after the last movement.
const CV_HOLD_BATCHES: u32 = 16;

/// Number of values `meter_values` writes.
pub const METER_COUNT: usize = 8;
/// Meter value: input peak, left (linear, last 16 batches).
pub const METER_IN_PEAK_L: usize = 0;
/// Meter value: input peak, right (linear).
pub const METER_IN_PEAK_R: usize = 1;
/// Meter value: output peak, left (linear).
pub const METER_OUT_PEAK_L: usize = 2;
/// Meter value: output peak, right (linear).
pub const METER_OUT_PEAK_R: usize = 3;
/// Meter value: held output peak in dB (max of both sides since the last reset; -120 if none).
pub const METER_HOLD_DB: usize = 4;
/// Meter value: 1 while the input is above -60 dBFS, else 0.
pub const METER_SIGNAL: usize = 5;
/// Meter value: 1 while a CV input is connected and moving / non-zero, else 0.
pub const METER_CV_ACTIVE: usize = 6;
/// Meter value: 1 while a mute fade is in progress, else 0.
pub const METER_FADING: usize = 7;

static MODES: [&str; 3] = ["gain", "mix", "xfade"];
static PAN_LAWS: [&str; 4] = ["0 dB", "-3 dB", "-4.5 dB", "-6 dB"];
static FADER_MODES: [&str; 2] = ["Trim", "VCA"];

static PARAMS: [ParamDescriptor; 19] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    linear(P_GAIN, "gain", "Gain", -60.0, 24.0, 0.0, Unit::Db),
    linear(P_PAN, "pan", "Balance", -1.0, 1.0, 0.0, Unit::None),
    linear(P_WIDTH, "width", "Width", 0.0, 2.0, 1.0, Unit::None),
    linear(P_BLEND, "blend", "Blend", 0.0, 1.0, 0.5, Unit::Percent),
    toggle(P_MUTE, "mute", "Mute", 0.0),
    trim(P_GAIN_TRIM, "gain_cv_trim", "Gain CV trim"),
    trim(P_PAN_TRIM, "pan_cv_trim", "Balance CV trim"),
    bypass_param(8),
    linear(P_FADE, "fade", "Mute fade", 0.0, FADE_MAX_MS, 0.0, Unit::Ms),
    toggle(P_INVERT_L, "invert_l", "Invert L", 0.0),
    toggle(P_INVERT_R, "invert_r", "Invert R", 0.0),
    toggle(P_MONO, "mono", "Mono", 0.0),
    linear(P_PAN_L, "pan_l", "Pan L", -1.0, 1.0, -1.0, Unit::None),
    linear(P_PAN_R, "pan_r", "Pan R", -1.0, 1.0, 1.0, Unit::None),
    choice(P_PAN_LAW, "pan_law", "Pan law", &PAN_LAWS, 1.0),
    choice(P_FADER_MODE, "fader_mode", "Fader mode", &FADER_MODES, 0.0),
    toggle(P_PEAK_RESET, "peak_reset", "Reset peak", 0.0),
    toggle(P_PEAK_AUTO, "peak_auto", "Auto reset peak", 0.0),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const AUX_L: SocketId = SocketId(2);
const AUX_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const GAIN_CV: SocketId = SocketId(6);
const PAN_CV: SocketId = SocketId(7);
const MUTE_CV: SocketId = SocketId(8);

static SOCKETS: [SocketDescriptor; 9] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "aux_l", "Aux L"),
    audio_in(3, "aux_r", "Aux R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_in(6, "gain_cv_in", "Gain CV"),
    cv_in(7, "pan_cv_in", "Balance CV"),
    cv_in(8, "mute_cv_in", "Mute CV"),
];

/// Static description of the gain tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.gain-tool",
    name: "Gain Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`GainTool`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(GainTool::new())),
    }
}

/// Gains `(to left, to right)` of a source placed at `pan` (-1 hard left .. +1 hard right) with
/// the pan law `law` (0: 0 dB, 1: -3 dB, 2: -4.5 dB, 3: -6 dB). The extremes are exactly
/// `(1, 0)` and `(0, 1)` for every law.
fn law_gains(law: u8, pan: f32) -> (f32, f32) {
    let x = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
    if x <= 0.0 {
        return (1.0, 0.0);
    }
    if x >= 1.0 {
        return (0.0, 1.0);
    }
    let sincos = || {
        (
            sin_cycles(0.25 * f64::from(1.0 - x)) as f32,
            sin_cycles(0.25 * f64::from(x)) as f32,
        )
    };
    match law {
        0 => ((2.0 * (1.0 - x)).min(1.0), (2.0 * x).min(1.0)),
        1 => sincos(),
        2 => {
            let (c, s) = sincos();
            (((1.0 - x) * c).sqrt(), (x * s).sqrt())
        }
        _ => (1.0 - x, x),
    }
}

/// The mute fade gain at fade position `p` (1 open, 0 muted): equal dB steps down to
/// `-FADE_RANGE_DB`, exactly 1 at the top and exactly 0 at the bottom.
fn fade_curve(p: f64) -> f32 {
    if p >= 1.0 {
        1.0
    } else if p <= 0.0 {
        0.0
    } else {
        exp2_det((p as f32 - 1.0) * FADE_RANGE_DB * LOG2_10_20)
    }
}

/// Whether a smoother that went from `before` to `after` was ever away from `neutral`.
#[allow(clippy::float_cmp)]
fn moving(before: f32, after: f32, neutral: f32) -> bool {
    before != neutral || after != neutral
}

/// The gain tool device.
#[derive(Debug, Clone)]
pub struct GainTool {
    mode: u8,
    gain_db: f32,
    pan: f32,
    width: f32,
    blend: f32,
    mute: bool,
    gain_trim: f32,
    pan_trim: f32,
    fade_ms: f32,
    invert: [bool; 2],
    mono: bool,
    pan_pos: [f32; 2],
    pan_law: u8,
    vca: bool,
    peak_auto: bool,
    reset_held: bool,
    reset_pending: bool,
    fresh: bool,
    sample_rate: f32,
    gain: BlockSmoother,
    left: BlockSmoother,
    right: BlockSmoother,
    width_sm: BlockSmoother,
    blend_sm: BlockSmoother,
    pol: [BlockSmoother; 2],
    mono_sm: BlockSmoother,
    /// Pan matrix `[l->l, l->r, r->l, r->r]`.
    mat: [BlockSmoother; 4],
    mute_gate: GateIn,
    fade_pos: f64,
    fade_on: bool,
    in_peak: [RecentPeak; 2],
    out_peak: [RecentPeak; 2],
    hold: f32,
    hold_batches: u32,
    cv_hold: u32,
    meters: [f32; METER_COUNT],
}

impl Default for GainTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GainTool {
    /// A gain tool with default params.
    pub fn new() -> Self {
        Self {
            mode: 0,
            gain_db: 0.0,
            pan: 0.0,
            width: 1.0,
            blend: 0.5,
            mute: false,
            gain_trim: 1.0,
            pan_trim: 1.0,
            fade_ms: 0.0,
            invert: [false; 2],
            mono: false,
            pan_pos: [-1.0, 1.0],
            pan_law: 1,
            vca: false,
            peak_auto: false,
            reset_held: false,
            reset_pending: false,
            fresh: true,
            sample_rate: 48_000.0,
            gain: BlockSmoother::new(1.0),
            left: BlockSmoother::new(1.0),
            right: BlockSmoother::new(1.0),
            width_sm: BlockSmoother::new(1.0),
            blend_sm: BlockSmoother::new(0.5),
            pol: [BlockSmoother::new(1.0); 2],
            mono_sm: BlockSmoother::new(0.0),
            mat: [
                BlockSmoother::new(1.0),
                BlockSmoother::new(0.0),
                BlockSmoother::new(0.0),
                BlockSmoother::new(1.0),
            ],
            mute_gate: GateIn::default(),
            fade_pos: 1.0,
            fade_on: false,
            in_peak: [RecentPeak::default(); 2],
            out_peak: [RecentPeak::default(); 2],
            hold: 0.0,
            hold_batches: 0,
            cv_hold: 0,
            meters: [0.0, 0.0, 0.0, 0.0, HOLD_FLOOR_DB, 0.0, 0.0, 0.0],
        }
    }

    fn smoothers(&mut self) -> [&mut BlockSmoother; 12] {
        let [m0, m1, m2, m3] = &mut self.mat;
        let [p0, p1] = &mut self.pol;
        [
            &mut self.gain,
            &mut self.left,
            &mut self.right,
            &mut self.width_sm,
            &mut self.blend_sm,
            p0,
            p1,
            &mut self.mono_sm,
            m0,
            m1,
            m2,
            m3,
        ]
    }
}

impl Device for GainTool {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        for s in self.smoothers() {
            s.set_time(10.0, sr, BATCH);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
        self.mute_gate.reset();
        self.in_peak = [RecentPeak::default(); 2];
        self.out_peak = [RecentPeak::default(); 2];
        self.hold = 0.0;
        self.hold_batches = 0;
        self.cv_hold = 0;
        self.reset_pending = false;
        self.meters = [0.0, 0.0, 0.0, 0.0, HOLD_FLOOR_DB, 0.0, 0.0, 0.0];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 2.0).round() as u8,
            P_GAIN => self.gain_db = v.clamp(-60.0, 24.0),
            P_PAN => self.pan = v.clamp(-1.0, 1.0),
            P_WIDTH => self.width = v.clamp(0.0, 2.0),
            P_BLEND => self.blend = v.clamp(0.0, 1.0),
            P_MUTE => self.mute = v >= 0.5,
            P_GAIN_TRIM => self.gain_trim = v.clamp(-1.0, 1.0),
            P_PAN_TRIM => self.pan_trim = v.clamp(-1.0, 1.0),
            P_FADE => self.fade_ms = v.clamp(0.0, FADE_MAX_MS),
            P_INVERT_L => self.invert[0] = v >= 0.5,
            P_INVERT_R => self.invert[1] = v >= 0.5,
            P_MONO => self.mono = v >= 0.5,
            P_PAN_L => self.pan_pos[0] = v.clamp(-1.0, 1.0),
            P_PAN_R => self.pan_pos[1] = v.clamp(-1.0, 1.0),
            P_PAN_LAW => self.pan_law = v.clamp(0.0, 3.0).round() as u8,
            P_FADER_MODE => self.vca = v >= 0.5,
            P_PEAK_RESET => {
                let on = v >= 0.5;
                if on && !self.reset_held {
                    self.reset_pending = true;
                }
                self.reset_held = on;
            }
            P_PEAK_AUTO => self.peak_auto = v >= 0.5,
            _ => {}
        }
    }

    #[allow(clippy::float_cmp)]
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let (aux_l, aux_r) = (*ctx.input(AUX_L), *ctx.input(AUX_R));
        let cv_gain = batch_mean(ctx.input(GAIN_CV));
        let cv_pan = batch_mean(ctx.input(PAN_CV));
        let mut cv_seen = false;
        for cv in [GAIN_CV, PAN_CV, MUTE_CV] {
            cv_seen |= ctx
                .input(cv)
                .iter()
                .any(|x| x.is_finite() && x.abs() > 1e-4);
        }
        for &x in ctx.input(MUTE_CV) {
            self.mute_gate.update(x);
        }
        let muted = self.mute || self.mute_gate.is_high();
        let fade_on = self.fade_ms > 0.0;

        let db_with_cv = self.gain_db + self.gain_trim * clean(cv_gain) * CV_GAIN_DB;
        let normal = if self.vca {
            if self.gain_db <= VCA_BOTTOM_DB {
                0.0
            } else {
                exp2_det(db_with_cv.min(0.0) * LOG2_10_20)
            }
        } else {
            exp2_det(db_with_cv * LOG2_10_20)
        };

        // Mute fade position and the gain it gives over this batch.
        let fade_s = f64::from(self.fade_ms) * 0.001;
        let target_pos = if muted { 0.0 } else { 1.0 };
        if self.fresh {
            self.fade_pos = if fade_on { target_pos } else { 1.0 };
        } else if fade_on != self.fade_on {
            if fade_on {
                // Fades start here. A muted gain that has not reached silence yet is still
                // coming down on the old smoothing: the fade starts from that level (so the
                // output never swells back), and the smoother goes to the open gain, the
                // fade now doing the lowering.
                self.fade_pos = 1.0;
                if muted {
                    let level = f64::from(self.gain.value());
                    let open = f64::from(normal);
                    self.fade_pos = if open > 1e-9 && level > open * 1e-4 {
                        (1.0 + 20.0 * (level / open).log10() / f64::from(FADE_RANGE_DB))
                            .clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    self.gain.set_immediate(open as f32);
                }
            } else {
                // Fades end: hand the faded level to the gain smoother so nothing jumps.
                let g = self.gain.value() * fade_curve(self.fade_pos);
                self.gain.set_immediate(g);
                self.fade_pos = 1.0;
            }
        }
        self.fade_on = fade_on;
        let fade_start = self.fade_pos;
        let fading = fade_on && fade_start != target_pos;
        if fading {
            let step = BATCH as f64 / (fade_s * f64::from(self.sample_rate));
            self.fade_pos = if target_pos > fade_start {
                (fade_start + step).min(1.0)
            } else {
                (fade_start - step).max(0.0)
            };
        }
        let (mf0, mf1) = if fade_on {
            (fade_curve(fade_start), fade_curve(self.fade_pos))
        } else {
            (1.0, 1.0)
        };
        let mf_inc = (mf1 - mf0) / BATCH as f32;

        let gain_target = if muted && !fade_on { 0.0 } else { normal };
        let (bl, br) = balance_gains(self.pan + self.pan_trim * clean(cv_pan));
        let (ll, lr) = law_gains(self.pan_law, self.pan_pos[0]);
        let (rl, rr) = law_gains(self.pan_law, self.pan_pos[1]);
        let sign = |inv: bool| if inv { -1.0 } else { 1.0 };
        let mono_t = if self.mono { 1.0 } else { 0.0 };
        let mat_t = [ll, lr, rl, rr];
        if self.fresh {
            self.gain.set_immediate(gain_target);
            self.left.set_immediate(bl);
            self.right.set_immediate(br);
            self.width_sm.set_immediate(self.width);
            self.blend_sm.set_immediate(self.blend);
            for (s, inv) in self.pol.iter_mut().zip(self.invert) {
                s.set_immediate(sign(inv));
            }
            self.mono_sm.set_immediate(mono_t);
            for (s, t) in self.mat.iter_mut().zip(mat_t) {
                s.set_immediate(t);
            }
            self.fresh = false;
        }
        let before_pol = [self.pol[0].value(), self.pol[1].value()];
        let before_mono = self.mono_sm.value();
        let before_mat = [
            self.mat[0].value(),
            self.mat[1].value(),
            self.mat[2].value(),
            self.mat[3].value(),
        ];
        let g = self.gain.advance(gain_target, BATCH);
        let gl = self.left.advance(bl, BATCH);
        let gr = self.right.advance(br, BATCH);
        let w = self.width_sm.advance(self.width, BATCH);
        let b = self.blend_sm.advance(self.blend, BATCH);
        let pol = [
            self.pol[0].advance(sign(self.invert[0]), BATCH),
            self.pol[1].advance(sign(self.invert[1]), BATCH),
        ];
        let mn = self.mono_sm.advance(mono_t, BATCH);
        let mat = [
            self.mat[0].advance(mat_t[0], BATCH),
            self.mat[1].advance(mat_t[1], BATCH),
            self.mat[2].advance(mat_t[2], BATCH),
            self.mat[3].advance(mat_t[3], BATCH),
        ];
        let pol_on = [
            moving(before_pol[0], self.pol[0].value(), 1.0),
            moving(before_pol[1], self.pol[1].value(), 1.0),
        ];
        let mono_on = moving(before_mono, self.mono_sm.value(), 0.0);
        let identity = [1.0, 0.0, 0.0, 1.0];
        let mut pan_on = false;
        for k in 0..4 {
            pan_on |= moving(before_mat[k], self.mat[k].value(), identity[k]);
        }
        let mono_full = self.mono_sm.value() >= 1.0 && before_mono >= 1.0;

        let (mut pk_in, mut pk_out) = ([0.0f32; 2], [0.0f32; 2]);
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let (mut xl, mut xr) = (finite(in_l[n]), finite(in_r[n]));
            pk_in[0] = pk_in[0].max(xl.abs());
            pk_in[1] = pk_in[1].max(xr.abs());
            if pol_on[0] {
                xl *= pol[0].at(n);
            }
            if pol_on[1] {
                xr *= pol[1].at(n);
            }
            if mono_on {
                let mid = 0.5 * (xl + xr);
                if mono_full {
                    xl = mid;
                    xr = mid;
                } else {
                    let m = mn.at(n);
                    xl += (mid - xl) * m;
                    xr += (mid - xr) * m;
                }
            }
            let (al, ar) = (finite(aux_l[n]), finite(aux_r[n]));
            let bn = b.at(n);
            let (mut l, mut r) = match self.mode {
                1 => (xl + al * bn, xr + ar * bn),
                2 => (xl * (1.0 - bn) + al * bn, xr * (1.0 - bn) + ar * bn),
                _ => (xl, xr),
            };
            if pan_on {
                let (pl, pr) = (
                    l * mat[0].at(n) + r * mat[2].at(n),
                    l * mat[1].at(n) + r * mat[3].at(n),
                );
                l = pl;
                r = pr;
            }
            let wn = w.at(n);
            if wn != 1.0 {
                let m = 0.5 * (l + r);
                let s = 0.5 * (l - r) * wn;
                l = m + s;
                r = m - s;
            }
            let mut gn = g.at(n);
            if fade_on {
                gn *= mf0 + mf_inc * n as f32;
            }
            let (ol, or) = (l * (gn * gl.at(n)), r * (gn * gr.at(n)));
            out_l[n] = ol;
            out_r[n] = or;
            pk_out[0] = pk_out[0].max(if ol.is_finite() { ol.abs() } else { 0.0 });
            pk_out[1] = pk_out[1].max(if or.is_finite() { or.abs() } else { 0.0 });
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;

        // Meters.
        let auto_batches = (PEAK_AUTO_SECONDS * self.sample_rate / BATCH as f32).round() as u32;
        if self.reset_pending {
            self.reset_pending = false;
            self.hold = 0.0;
            self.hold_batches = 0;
        } else if self.peak_auto {
            self.hold_batches += 1;
            if self.hold_batches >= auto_batches {
                self.hold = 0.0;
                self.hold_batches = 0;
            }
        } else {
            self.hold_batches = 0;
        }
        self.hold = self.hold.max(pk_out[0]).max(pk_out[1]);
        self.cv_hold = if cv_seen {
            CV_HOLD_BATCHES
        } else {
            self.cv_hold.saturating_sub(1)
        };
        let pin = [
            self.in_peak[0].push(pk_in[0]),
            self.in_peak[1].push(pk_in[1]),
        ];
        let pout = [
            self.out_peak[0].push(pk_out[0]),
            self.out_peak[1].push(pk_out[1]),
        ];
        let hold_db = if self.hold > 0.0 {
            (20.0 * self.hold.log10()).max(HOLD_FLOOR_DB)
        } else {
            HOLD_FLOOR_DB
        };
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        self.meters = [
            pin[0],
            pin[1],
            pout[0],
            pout[1],
            hold_db,
            flag(pin[0].max(pin[1]) > SIGNAL_THRESHOLD),
            flag(self.cv_hold > 0),
            flag(fading),
        ];
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
