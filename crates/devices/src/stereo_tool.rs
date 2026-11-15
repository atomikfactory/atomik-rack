//! `atomik.stereo-tool`: a dual-band stereo imager with polarity, balance, mono and mid/side
//! conversion.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in -> polarity (invert L / R) -> rotation -> split at `low_bypass` (the crossover)
//!    -> lo band: lo_width        -> lo_out_l / lo_out_r
//!    -> hi band: width (hi)      -> hi_out_l / hi_out_r
//!    -> sum -> solo -> balance -> M/S mode -> mono -> out
//! ```
//!
//! * **Polarity.** `invert_l` / `invert_r` multiply a channel by -1 (a smoothed -1..+1 ramp, so
//!   the switch does not click).
//! * **Rotation.** `rotate` (0 to 180 degrees) turns the phase of both channels by that angle
//!   at `rotate_hz` with a first-order all-pass ([`atomik_dsp::PhaseRotator`]): the level is
//!   untouched at every frequency, the turn is less below `rotate_hz` and more above it, and 180
//!   is a polarity inversion. It reshapes the waveform of a bass or a kick (see the kick and
//!   bass check). At 0 the stage is skipped, so the input passes bit for bit. A change of the
//!   angle or the frequency takes effect at once, like the mode switch.
//! * **Two bands.** The crossover is `low_bypass` (key kept from the first release; shown as
//!   "X-over", 20 Hz to 6 kHz). Above 20.5 Hz the signal is split into a low and a high band by
//!   a fourth-order Linkwitz-Riley crossover at that frequency per channel
//!   ([`atomik_dsp::Crossover`]). At 20.5 Hz or below there is **no split**: the whole signal is
//!   the high band and the low band is silent.
//! * **Width.** The mid/side width of `atomik_dsp::stereo_width`: `M = (L + R) / 2`,
//!   `S = (L - R) / 2 * width`. 0 is mono, 1 leaves the band alone, 2 doubles its side signal.
//!   `width` (the key of the first release) is the **high** band's width, `lo_width` the low
//!   band's. With both at 1 the whole stage is skipped, so **both widths 1 (with everything else
//!   at rest) is the input bit for bit**. A project from before the second band has `lo_width`
//!   1, which adds the low band back as it was: it plays the same bits.
//! * **Solo** (`solo`: normal, lo, hi) picks what the sum carries: the low band only, the high
//!   band only (each after its width), or both. It sits after the sum and before balance. With no
//!   split, solo lo is silence and solo hi is everything. A switch is a 10 ms ramp of the band
//!   gains, and out of the skipped stage (both widths 1, a split) it blends from the input to the
//!   band sum, so it does not click.
//! * **Band outputs.** `lo_out_l/r` and `hi_out_l/r` carry each band after its own width, before
//!   solo, balance, mode and mono. With no split the low outputs are silent and the high ones
//!   carry the widened signal. With both widths 1 the two bands add up to the stage input (an
//!   all-pass of it: the crossover is phase-shifted but flat in level).
//! * **Balance** (-1..+1): unity at the centre, the far side falls linearly to silence at the end.
//! * **M/S mode.** `stereo` leaves the signal as it is. `encode` writes the mid `(L + R) / 2` to
//!   `out_l` and the side `(L - R) / 2` to `out_r`; `decode` reads `in_l` as the mid and `in_r`
//!   as the side and writes `L = M + S`, `R = M - S`. The mode switches at once (a hard cut).
//! * **Mono** (switch, smoothed over 10 ms), the last stage: both output channels become
//!   `(L + R) / 2` of what the mode produced, so the output is mono in every mode.
//! * **CV.** `lo_width_cv`, `hi_width_cv` and `xover_cv` take the mean of the batch, clamped to
//!   -1..+1, times their trim (`lo_width_cv_trim`, `hi_width_cv_trim`, `xover_cv_trim`: -1..+1,
//!   **default 0**, so an unpatched or untrimmed CV does nothing). A width becomes
//!   `width + trim * cv * 2` (full scale sweeps the whole range, clamped to 0..2). The crossover
//!   is multiplied by `2^(trim * cv * 4)` (four octaves at full scale), smoothed (20 ms) and
//!   clamped to 20.6 Hz..6 kHz. The CV only moves a crossover that is on: with `low_bypass` at
//!   20 Hz (no split) it does nothing, and it never turns the split off.
//! * **Smoothing.** Widths, solo, balance, polarity and mono are smoothed per batch (10 ms,
//!   `BlockSmoother`) and ramped linearly across it. A `low_bypass` param change takes effect at
//!   once (as it always did); only the CV part of the crossover is smoothed, and the crossover
//!   coefficients are updated once per batch at most, so a moving crossover cannot blow up.
//! * **Crossfade.** The switch between the dry input (both widths 1) and the band sum (an
//!   all-pass of it) stays a hard one for a project that never touches the low band or solo
//!   (bit for bit as before). As soon as `lo_width`, its CV or solo is away from rest the output
//!   crossfades between the two over 10 ms, and stays in that mode until the widths and the
//!   crossfade have settled. A reset puts the crossover back on `low_bypass` (a CV had moved it)
//!   and entering the split starts the crossover filters from silence.
//! * **Meter values** ([`METER_COUNT`] = 5, indices are the `METER_*` consts): the correlation of
//!   the low band and of the high band (-1..+1, each after its width, an exponentially smoothed
//!   estimate over about 300 ms, sample-rate aware, 0 when the band is silent or one channel
//!   is), a flag per band that is 1 while the band is **active** (it carries signal, above about
//!   -60 dBFS, and its width differs from 1 by more than 0.001, CV included; the low band is
//!   never active with no split), and 1 while the crossover splits.
//! * No latency. Non-finite input samples count as 0. Allocation-free after construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::batch_mean;
use crate::tool_support::{
    audio_in, audio_out, balance_gains, choice, cv_in, expo, finite, linear, toggle,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, Crossover, PhaseRotator, exp2_det, exp2_det_f64};

const P_MODE: u32 = 0;
const P_INVERT_L: u32 = 1;
const P_INVERT_R: u32 = 2;
const P_WIDTH: u32 = 3;
const P_LOW_BYPASS: u32 = 4;
const P_BALANCE: u32 = 5;
const P_MONO: u32 = 6;
const P_ROTATE: u32 = 7;
const P_ROTATE_HZ: u32 = 8;
const P_LO_WIDTH: u32 = 10;
const P_SOLO: u32 = 11;
const P_LO_TRIM: u32 = 12;
const P_HI_TRIM: u32 = 13;
const P_XOVER_TRIM: u32 = 14;

/// Corner at or below which there is no split.
const LOW_BYPASS_OFF_HZ: f32 = 20.5;
/// Lowest corner a CV can drive a split crossover to.
const XOVER_CV_MIN_HZ: f32 = 20.6;
/// Highest crossover corner.
const XOVER_MAX_HZ: f32 = 6_000.0;
/// Octaves of crossover a full-scale CV moves.
const XOVER_CV_OCT: f32 = 4.0;
/// Width a full-scale CV adds at full trim.
const WIDTH_CV_RANGE: f32 = 2.0;
/// Correlation integration time (seconds).
const CORR_SECONDS: f64 = 0.3;
/// Mean band power (per channel) below which a band reads as silent (-60 dBFS).
const ACTIVE_POWER: f64 = 1e-6;
/// Geometric-mean channel power below which the correlation reads 0.
const CORR_FLOOR: f64 = 1e-12;

/// Number of values `meter_values` writes.
pub const METER_COUNT: usize = 5;
/// Meter value: correlation of the low band after its width (-1..+1, 0 when silent).
pub const METER_LO_CORR: usize = 0;
/// Meter value: correlation of the high band after its width (-1..+1, 0 when silent).
pub const METER_HI_CORR: usize = 1;
/// Meter value: 1 while the low band carries signal and its width is not 1.
pub const METER_LO_ACTIVE: usize = 2;
/// Meter value: 1 while the high band carries signal and its width is not 1.
pub const METER_HI_ACTIVE: usize = 3;
/// Meter value: 1 while the crossover splits (`low_bypass` above 20.5 Hz).
pub const METER_SPLIT: usize = 4;

static MODES: [&str; 3] = ["stereo", "encode", "decode"];
static SOLOS: [&str; 3] = ["normal", "lo", "hi"];

static PARAMS: [ParamDescriptor; 15] = [
    choice(P_MODE, "mode", "Mid/side", &MODES, 0.0),
    toggle(P_INVERT_L, "invert_l", "Invert L", 0.0),
    toggle(P_INVERT_R, "invert_r", "Invert R", 0.0),
    linear(P_WIDTH, "width", "Width", 0.0, 2.0, 1.0, Unit::None),
    expo(
        P_LOW_BYPASS,
        "low_bypass",
        "X-over",
        20.0,
        XOVER_MAX_HZ,
        20.0,
        Unit::Hz,
    ),
    linear(P_BALANCE, "balance", "Balance", -1.0, 1.0, 0.0, Unit::None),
    toggle(P_MONO, "mono", "Mono", 0.0),
    linear(P_ROTATE, "rotate", "Rotate", 0.0, 180.0, 0.0, Unit::None),
    expo(
        P_ROTATE_HZ,
        "rotate_hz",
        "Rotate at",
        20.0,
        1000.0,
        100.0,
        Unit::Hz,
    ),
    bypass_param(9),
    linear(
        P_LO_WIDTH,
        "lo_width",
        "Low width",
        0.0,
        2.0,
        1.0,
        Unit::None,
    ),
    choice(P_SOLO, "solo", "Solo", &SOLOS, 0.0),
    cv_trim(P_LO_TRIM, "lo_width_cv_trim", "Low width CV trim"),
    cv_trim(P_HI_TRIM, "hi_width_cv_trim", "High width CV trim"),
    cv_trim(P_XOVER_TRIM, "xover_cv_trim", "X-over CV trim"),
];

/// A `*_cv_trim` row: -1..+1, default 0 (the CV does nothing until it is turned up).
const fn cv_trim(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    linear(id, key, name, -1.0, 1.0, 0.0, Unit::Percent)
}

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const LO_CV: SocketId = SocketId(4);
const HI_CV: SocketId = SocketId(5);
const XOVER_CV: SocketId = SocketId(6);
const LO_OUT_L: SocketId = SocketId(7);
const LO_OUT_R: SocketId = SocketId(8);
const HI_OUT_L: SocketId = SocketId(9);
const HI_OUT_R: SocketId = SocketId(10);

static SOCKETS: [SocketDescriptor; 11] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
    cv_in(4, "lo_width_cv", "Low width CV"),
    cv_in(5, "hi_width_cv", "High width CV"),
    cv_in(6, "xover_cv", "X-over CV"),
    audio_out(7, "lo_out_l", "Low out L"),
    audio_out(8, "lo_out_r", "Low out R"),
    audio_out(9, "hi_out_l", "High out L"),
    audio_out(10, "hi_out_r", "High out R"),
];

/// Static description of the stereo tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.stereo-tool",
    name: "Stereo Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`StereoTool`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(StereoTool::new())),
    }
}

/// Exponentially smoothed correlation of a stereo pair.
#[derive(Debug, Clone, Copy, Default)]
struct Corr {
    ll: f64,
    rr: f64,
    lr: f64,
}

impl Corr {
    #[inline]
    fn push(&mut self, a: f64, l: f32, r: f32) {
        let (l, r) = (f64::from(l), f64::from(r));
        let b = 1.0 - a;
        self.ll = a * self.ll + b * l * l;
        self.rr = a * self.rr + b * r * r;
        self.lr = a * self.lr + b * l * r;
    }

    /// Clears values that decayed to nothing so they never go denormal.
    fn flush(&mut self) {
        for v in [&mut self.ll, &mut self.rr] {
            if *v < 1e-30 {
                *v = 0.0;
            }
        }
        if self.lr.abs() < 1e-30 {
            self.lr = 0.0;
        }
    }

    fn value(&self) -> f32 {
        let denom = (self.ll * self.rr).sqrt();
        if denom < CORR_FLOOR {
            0.0
        } else {
            (self.lr / denom).clamp(-1.0, 1.0) as f32
        }
    }

    fn has_signal(&self) -> bool {
        0.5 * (self.ll + self.rr) > ACTIVE_POWER
    }
}

/// The stereo tool device.
#[derive(Debug, Clone)]
pub struct StereoTool {
    mode: u8,
    invert_l: bool,
    invert_r: bool,
    width: f32,
    lo_width: f32,
    low_bypass: f32,
    solo: u8,
    lo_trim: f32,
    hi_trim: f32,
    xover_trim: f32,
    balance: f32,
    mono: bool,
    rotate: f32,
    rotate_hz: f32,
    sample_rate: f32,
    rotor: [PhaseRotator; 2],
    fresh: bool,
    /// Whether the last batch split (the crossover filters only run while it does).
    was_split: bool,
    /// The band-sum crossfade is in charge: the low band or solo is involved (or still settling).
    xf: bool,
    split: [Crossover; 2],
    pol_l: BlockSmoother,
    pol_r: BlockSmoother,
    width_sm: BlockSmoother,
    lo_sm: BlockSmoother,
    solo_lo_sm: BlockSmoother,
    solo_hi_sm: BlockSmoother,
    xover_sm: BlockSmoother,
    /// How much of the band sum (against the dry input) the output carries, while `xf`.
    bs_sm: BlockSmoother,
    bal_l: BlockSmoother,
    bal_r: BlockSmoother,
    mono_sm: BlockSmoother,
    corr_coeff: f64,
    corr: [Corr; 2],
    meters: [f32; METER_COUNT],
}

impl Default for StereoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoTool {
    /// A stereo tool with default params.
    pub fn new() -> Self {
        let mut s = Self {
            mode: 0,
            invert_l: false,
            invert_r: false,
            width: 1.0,
            lo_width: 1.0,
            low_bypass: 20.0,
            solo: 0,
            lo_trim: 0.0,
            hi_trim: 0.0,
            xover_trim: 0.0,
            balance: 0.0,
            mono: false,
            rotate: 0.0,
            rotate_hz: 100.0,
            sample_rate: 48_000.0,
            rotor: [PhaseRotator::new(); 2],
            fresh: true,
            was_split: false,
            xf: false,
            split: [Crossover::new(48_000.0), Crossover::new(48_000.0)],
            pol_l: BlockSmoother::new(1.0),
            pol_r: BlockSmoother::new(1.0),
            width_sm: BlockSmoother::new(1.0),
            lo_sm: BlockSmoother::new(1.0),
            solo_lo_sm: BlockSmoother::new(1.0),
            solo_hi_sm: BlockSmoother::new(1.0),
            xover_sm: BlockSmoother::new(0.0),
            bs_sm: BlockSmoother::new(0.0),
            bal_l: BlockSmoother::new(1.0),
            bal_r: BlockSmoother::new(1.0),
            mono_sm: BlockSmoother::new(0.0),
            corr_coeff: 0.0,
            corr: [Corr::default(); 2],
            meters: [0.0; METER_COUNT],
        };
        s.set_corr_time();
        s
    }

    fn low_split(&self) -> bool {
        self.low_bypass > LOW_BYPASS_OFF_HZ
    }

    fn apply_rotation(&mut self) {
        for r in &mut self.rotor {
            r.set(self.rotate, self.rotate_hz, self.sample_rate);
        }
    }

    fn apply_freq(&mut self, hz: f32) {
        for c in &mut self.split {
            c.set_freq(hz);
        }
    }

    fn set_corr_time(&mut self) {
        let samples = CORR_SECONDS * f64::from(self.sample_rate);
        self.corr_coeff = exp2_det_f64(-std::f64::consts::LOG2_E / samples);
    }

    fn solo_gains(&self) -> (f32, f32) {
        match self.solo {
            1 => (1.0, 0.0),
            2 => (0.0, 1.0),
            _ => (1.0, 1.0),
        }
    }
}

impl Device for StereoTool {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        self.apply_rotation();
        self.split = [Crossover::new(sr), Crossover::new(sr)];
        self.apply_freq(self.low_bypass);
        self.set_corr_time();
        for s in [
            &mut self.pol_l,
            &mut self.pol_r,
            &mut self.width_sm,
            &mut self.lo_sm,
            &mut self.solo_lo_sm,
            &mut self.solo_hi_sm,
            &mut self.bal_l,
            &mut self.bal_r,
            &mut self.mono_sm,
            &mut self.bs_sm,
        ] {
            s.set_time(10.0, sr, BATCH);
        }
        self.xover_sm.set_time(20.0, sr, BATCH);
        self.reset();
    }

    fn reset(&mut self) {
        for c in &mut self.split {
            c.reset();
        }
        for r in &mut self.rotor {
            r.reset();
        }
        self.corr = [Corr::default(); 2];
        self.meters = [0.0; METER_COUNT];
        self.fresh = true;
        self.xf = false;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 2.0).round() as u8,
            P_INVERT_L => self.invert_l = v >= 0.5,
            P_INVERT_R => self.invert_r = v >= 0.5,
            P_WIDTH => self.width = v.clamp(0.0, 2.0),
            P_LO_WIDTH => self.lo_width = v.clamp(0.0, 2.0),
            P_LOW_BYPASS => {
                self.low_bypass = v.clamp(20.0, XOVER_MAX_HZ);
                self.apply_freq(self.low_bypass);
            }
            P_SOLO => self.solo = v.clamp(0.0, 2.0).round() as u8,
            P_LO_TRIM => self.lo_trim = v.clamp(-1.0, 1.0),
            P_HI_TRIM => self.hi_trim = v.clamp(-1.0, 1.0),
            P_XOVER_TRIM => self.xover_trim = v.clamp(-1.0, 1.0),
            P_BALANCE => self.balance = v.clamp(-1.0, 1.0),
            P_MONO => self.mono = v >= 0.5,
            P_ROTATE => {
                self.rotate = v.clamp(0.0, 180.0);
                self.apply_rotation();
            }
            P_ROTATE_HZ => {
                self.rotate_hz = v.clamp(20.0, 1000.0);
                self.apply_rotation();
            }
            _ => {}
        }
    }

    #[allow(clippy::float_cmp, clippy::too_many_lines)]
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let cv_lo = batch_mean(ctx.input(LO_CV));
        let cv_hi = batch_mean(ctx.input(HI_CV));
        let cv_x = batch_mean(ctx.input(XOVER_CV));
        let sign = |on: bool| if on { -1.0 } else { 1.0 };
        let (bl, br) = balance_gains(self.balance);
        let mono = if self.mono { 1.0 } else { 0.0 };
        let lo_target = (self.lo_width + self.lo_trim * cv_lo * WIDTH_CV_RANGE).clamp(0.0, 2.0);
        let hi_target = (self.width + self.hi_trim * cv_hi * WIDTH_CV_RANGE).clamp(0.0, 2.0);
        let (so_lo, so_hi) = self.solo_gains();
        // Where the output sits between the dry input and the band sum.
        let bs_target = if hi_target == 1.0 && lo_target == 1.0 {
            0.0
        } else {
            1.0
        };
        if self.fresh {
            // A reset leaves the crossover wherever a CV had taken it: back to the param.
            if self.low_split() {
                self.apply_freq(self.low_bypass);
            }
            self.bs_sm.set_immediate(bs_target);
            self.pol_l.set_immediate(sign(self.invert_l));
            self.pol_r.set_immediate(sign(self.invert_r));
            self.width_sm.set_immediate(hi_target);
            self.lo_sm.set_immediate(lo_target);
            self.solo_lo_sm.set_immediate(so_lo);
            self.solo_hi_sm.set_immediate(so_hi);
            self.xover_sm.set_immediate(0.0);
            self.bal_l.set_immediate(bl);
            self.bal_r.set_immediate(br);
            self.mono_sm.set_immediate(mono);
            self.fresh = false;
        }
        let (lo_start, hi_start) = (self.lo_sm.value(), self.width_sm.value());
        let (sl_start, sh_start) = (self.solo_lo_sm.value(), self.solo_hi_sm.value());
        let pl = self.pol_l.advance(sign(self.invert_l), BATCH);
        let pr = self.pol_r.advance(sign(self.invert_r), BATCH);
        let w = self.width_sm.advance(hi_target, BATCH);
        let wl = self.lo_sm.advance(lo_target, BATCH);
        let sl = self.solo_lo_sm.advance(so_lo, BATCH);
        let sh = self.solo_hi_sm.advance(so_hi, BATCH);
        let gl = self.bal_l.advance(bl, BATCH);
        let gr = self.bal_r.advance(br, BATCH);
        let mo = self.mono_sm.advance(mono, BATCH);
        let split = self.low_split();
        // The old hard switch between the dry input (both widths 1) and the band sum (an all-pass
        // of it) stays for projects that never touch the low band or solo; as soon as either is
        // involved the output crossfades between the two (10 ms) so it does not click.
        let involved = lo_target != 1.0
            || lo_start != 1.0
            || so_lo != 1.0
            || so_hi != 1.0
            || sl_start != 1.0
            || sh_start != 1.0;
        if involved {
            self.xf = true;
        } else if self.xf && self.bs_sm.value() == bs_target && hi_start == hi_target {
            self.xf = false;
        }
        let bs_ramp = if self.xf {
            Some(self.bs_sm.advance(bs_target, BATCH))
        } else {
            None
        };
        // Entering the split: the filters hold what they last saw, which is stale.
        if split && !self.was_split {
            for c in &mut self.split {
                c.reset();
            }
            self.apply_freq(self.low_bypass);
        }
        self.was_split = split;

        // Crossover CV: a smoothed octave offset, applied once per batch at the batch end. With
        // no CV it never touches the corner `set_param` set.
        let xo_target = self.xover_trim * cv_x * XOVER_CV_OCT;
        let xo_moving = xo_target != 0.0 || self.xover_sm.value() != 0.0;
        let xo = self.xover_sm.advance(xo_target, BATCH).at(BATCH);
        if split && xo_moving {
            let hz = if xo == 0.0 {
                self.low_bypass
            } else {
                (self.low_bypass * exp2_det(xo)).clamp(XOVER_CV_MIN_HZ, XOVER_MAX_HZ)
            };
            self.apply_freq(hz);
        }

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut lo_out_l = [0.0f32; BATCH];
        let mut lo_out_r = [0.0f32; BATCH];
        let mut hi_out_l = [0.0f32; BATCH];
        let mut hi_out_r = [0.0f32; BATCH];
        let ca = self.corr_coeff;
        let mut last_skipped = false;
        for n in 0..BATCH {
            let mut l = finite(in_l[n]) * pl.at(n);
            let mut r = finite(in_r[n]) * pr.at(n);
            if !self.rotor[0].is_identity() {
                l = self.rotor[0].process(l);
                r = self.rotor[1].process(r);
            }
            let (whi, wlo) = (w.at(n), wl.at(n));
            let (xl, xr) = (l, r);
            // The bands after their widths.
            let (b_ll, b_lr, b_hl, b_hr);
            if split {
                let (ll, lh) = self.split[0].process(xl);
                let (rl, rh) = self.split[1].process(xr);
                if whi != 1.0 {
                    let m = 0.5 * (lh + rh);
                    let s = 0.5 * (lh - rh) * whi;
                    b_hl = m + s;
                    b_hr = m - s;
                } else {
                    b_hl = lh;
                    b_hr = rh;
                }
                if wlo != 1.0 {
                    let m = 0.5 * (ll + rl);
                    let s = 0.5 * (ll - rl) * wlo;
                    b_ll = m + s;
                    b_lr = m - s;
                } else {
                    b_ll = ll;
                    b_lr = rl;
                }
            } else {
                if whi != 1.0 {
                    let m = 0.5 * (xl + xr);
                    let s = 0.5 * (xl - xr) * whi;
                    b_hl = m + s;
                    b_hr = m - s;
                } else {
                    b_hl = xl;
                    b_hr = xr;
                }
                b_ll = 0.0;
                b_lr = 0.0;
            }
            lo_out_l[n] = b_ll;
            lo_out_r[n] = b_lr;
            hi_out_l[n] = b_hl;
            hi_out_r[n] = b_hr;
            self.corr[0].push(ca, b_ll, b_lr);
            self.corr[1].push(ca, b_hl, b_hr);

            // The sum, then solo.
            let skipped = whi == 1.0 && (!split || wlo == 1.0);
            let (gs_l, gs_h) = (sl.at(n), sh.at(n));
            last_skipped = skipped;
            if let (Some(bsr), true) = (bs_ramp, split) {
                let amt = bsr.at(n).max(1.0 - gs_l.min(gs_h));
                let sum_l = gs_l * b_ll + gs_h * b_hl;
                let sum_r = gs_l * b_lr + gs_h * b_hr;
                if amt >= 1.0 {
                    l = sum_l;
                    r = sum_r;
                } else {
                    l = xl + amt * (sum_l - xl);
                    r = xr + amt * (sum_r - xr);
                }
            } else if gs_l == 1.0 && gs_h == 1.0 {
                if !skipped {
                    if split {
                        l = b_ll + b_hl;
                        r = b_lr + b_hr;
                    } else {
                        l = b_hl;
                        r = b_hr;
                    }
                }
            } else {
                let sum_l = gs_l * b_ll + gs_h * b_hl;
                let sum_r = gs_l * b_lr + gs_h * b_hr;
                if skipped {
                    // Blend from the input (the skipped stage) to the soloed band sum.
                    let a = 1.0 - gs_l.min(gs_h);
                    l = xl + a * (sum_l - xl);
                    r = xr + a * (sum_r - xr);
                } else {
                    l = sum_l;
                    r = sum_r;
                }
            }
            let (g_l, g_r) = (gl.at(n), gr.at(n));
            if g_l != 1.0 || g_r != 1.0 {
                l *= g_l;
                r *= g_r;
            }
            match self.mode {
                1 => {
                    let (m, s) = (0.5 * (l + r), 0.5 * (l - r));
                    l = m;
                    r = s;
                }
                2 => {
                    let (m, s) = (l, r);
                    l = m + s;
                    r = m - s;
                }
                _ => {}
            }
            // Mono is the last stage, so the output is mono whatever the mode.
            let mn = mo.at(n);
            if mn >= 1.0 {
                // Exactly equal channels once the ramp has settled.
                let m = 0.5 * (l + r);
                l = m;
                r = m;
            } else if mn != 0.0 {
                let m = 0.5 * (l + r);
                l += (m - l) * mn;
                r += (m - r) * mn;
            }
            out_l[n] = l;
            out_r[n] = r;
        }
        if !self.xf {
            self.bs_sm
                .set_immediate(if last_skipped { 0.0 } else { 1.0 });
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(LO_OUT_L) = lo_out_l;
        *ctx.output(LO_OUT_R) = lo_out_r;
        *ctx.output(HI_OUT_L) = hi_out_l;
        *ctx.output(HI_OUT_R) = hi_out_r;

        // Meters.
        for c in &mut self.corr {
            c.flush();
        }
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        self.meters = [
            self.corr[0].value(),
            self.corr[1].value(),
            flag(split && self.corr[0].has_signal() && (lo_target - 1.0).abs() > 1e-3),
            flag(self.corr[1].has_signal() && (hi_target - 1.0).abs() > 1e-3),
            flag(split),
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
mod legacy;
#[cfg(test)]
mod tests;
