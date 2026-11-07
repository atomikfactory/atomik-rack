//! `atomik.pitch-shifter`: a real-time pitch shifter of up to an octave either way, with a fixed,
//! reported latency.
//!
//! # How it works
//!
//! The input goes into a ring buffer. A read head runs through it at `rate` input samples per
//! output sample (`rate = 2^(shift / 12)`), so its distance behind the write head, the *delay*,
//! changes by `1 - rate` every sample. Left alone it would run into the write head (a shift up) or
//! away from it (a shift down). Before that happens the head *jumps* by about one grain
//! (back for a shift up, forward for a shift down) and the old and the new head are cross-faded
//! over `X` samples with a raised cosine. Where the new head lands is searched, so the two
//! signals are in phase during the fade:
//!
//! 1. the nominal jump is `G = 2 X` samples; the candidates are `G - X ..= G + X`;
//! 2. a coarse pass scores every fourth candidate with the normalised cross-correlation of the
//!    next stretch of the mono sum (`(l + r) / 2`) at the old head against the same stretch at the
//!    candidate, reading every fourth sample; a fine pass tries the 7 candidates around the best at
//!    full rate;
//! 3. both channels jump by the same amount, so the stereo image is kept. A silent stretch takes
//!    the nominal jump.
//!
//! This is the overlap-add family of time-domain shifters: a pitched sound (a voice, a bass, a
//! guitar) shifts cleanly because the fade joins waveforms in phase; a dense mix shifts with a
//! slight roughness at the jumps, more at a larger `quality` setting.
//!
//! # Delay and latency
//!
//! The delay stays between `X + 4` and `X + 4 + 3 X` samples. [`Device::latency_frames`]
//! reports the middle of that range, `D = 5 X / 2 + 4` (with `X` even), which is **exactly** the
//! delay of the signal when the shift is zero: then the head stands still at `D` and the output is
//! the input delayed by `D` samples, bit for bit. With a shift the delay moves around `D`, and
//! the delay compensation lines the device up on average. If the shift returns to zero while the
//! head is away from `D`, one extra cross-fade (without the search) brings it back once the one in
//! progress is over: the output is exact again within `2 X` samples.
//!
//! `quality` sets `X`: `fast` 3 ms (`D` about 7.5 ms), `balanced` 6 ms (15 ms), `smooth` 10 ms
//! (25 ms). The latency follows it, so the device is `variable_latency`.
//!
//! # Params, CV, limits
//!
//! * `semitones` and `cents` add up (clamped to -12..+12 semitones); `pitch_cv_in` adds
//!   `pitch_cv_trim * cv * 12` semitones (the batch mean, same clamp).
//! * `mix` blends the dry input, delayed by `D` so it stays aligned (1 is shifted only).
//! * Deterministic maths (`exp2_det`, `sin_cycles`); non-finite input samples count as 0; no
//!   allocation after `prepare` (the buffers are sized there).

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{audio_in, audio_out, choice, cv_in, finite, linear, trim};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{exp2_det, sin_cycles};

const P_SEMITONES: u32 = 0;
const P_CENTS: u32 = 1;
const P_QUALITY: u32 = 2;
const P_MIX: u32 = 3;
const P_PITCH_TRIM: u32 = 4;

static QUALITIES: [&str; 3] = ["fast", "balanced", "smooth"];
/// The cross-fade length `X` of each quality, ms.
const FADE_MS: [f32; 3] = [3.0, 6.0, 10.0];
/// The largest shift either way, semitones.
const MAX_SHIFT: f32 = 12.0;
/// Margin the heads keep from the write head beyond the cross-fade, samples.
const MARGIN: usize = 4;
/// Largest `X` the buffers are sized for, ms.
const MAX_FADE_MS: f32 = 10.0;
/// How far the CV moves the pitch at full trim, semitones.
const CV_SEMITONES: f32 = 12.0;
/// Candidate spacing of the coarse search.
const COARSE: usize = 4;

static PARAMS: [ParamDescriptor; 6] = [
    linear(
        P_SEMITONES,
        "semitones",
        "Semitones",
        -MAX_SHIFT,
        MAX_SHIFT,
        0.0,
        Unit::Semitones,
    ),
    linear(P_CENTS, "cents", "Cents", -50.0, 50.0, 0.0, Unit::Cents),
    choice(P_QUALITY, "quality", "Quality", &QUALITIES, 1.0),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    trim(P_PITCH_TRIM, "pitch_cv_trim", "Pitch CV trim"),
    bypass_param(5),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const PITCH_CV: SocketId = SocketId(2);
const OUT_L: SocketId = SocketId(3);
const OUT_R: SocketId = SocketId(4);

static SOCKETS: [SocketDescriptor; 5] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    cv_in(2, "pitch_cv_in", "Pitch CV"),
    audio_out(3, "out_l", "Out L"),
    audio_out(4, "out_r", "Out R"),
];

/// Static description of the pitch shifter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.pitch-shifter",
    name: "Pitch Shifter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`PitchShifter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(PitchShifter::new())),
    }
}

/// The cross-fade length `X` in samples of `quality` at `sample_rate` (even, at least 16).
pub fn fade_samples(quality: usize, sample_rate: f32) -> usize {
    let ms = FADE_MS[quality.min(FADE_MS.len() - 1)];
    let half = (ms * 0.001 * sample_rate / 2.0).round().max(8.0) as usize;
    half * 2
}

/// The latency in samples of `quality` at `sample_rate`: the delay at a shift of zero.
pub fn latency_samples(quality: usize, sample_rate: f32) -> usize {
    let x = fade_samples(quality, sample_rate);
    x * 5 / 2 + MARGIN
}

/// A cross-fade in progress: the new head, how far it is and how long it lasts.
#[derive(Debug, Clone, Copy)]
struct Fade {
    to: f64,
    done: usize,
    len: usize,
}

/// The pitch shifter device.
#[derive(Debug, Clone)]
pub struct PitchShifter {
    sample_rate: f32,
    semitones: f32,
    cents: f32,
    quality: usize,
    mix: f32,
    pitch_trim: f32,
    // Derived from the quality.
    x: usize,
    latency: usize,
    // The rings (power-of-two size) and the write counter (index of the newest sample).
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    mask: usize,
    w: i64,
    // The read head (an absolute, fractional index) and the cross-fade.
    head: f64,
    fade: Option<Fade>,
}

impl Default for PitchShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl PitchShifter {
    /// A pitch shifter with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut s = Self {
            sample_rate: 48_000.0,
            semitones: 0.0,
            cents: 0.0,
            quality: 1,
            mix: 1.0,
            pitch_trim: 1.0,
            x: 0,
            latency: 0,
            ring_l: Vec::new(),
            ring_r: Vec::new(),
            mask: 0,
            w: 0,
            head: 0.0,
            fade: None,
        };
        s.resize();
        s.reset();
        s
    }

    /// Sizes the rings for the sample rate (the only allocation) and derives `X` and `D`.
    fn resize(&mut self) {
        let x_max = fade_samples(FADE_MS.len() - 1, self.sample_rate)
            .max((MAX_FADE_MS * 0.001 * self.sample_rate) as usize);
        let size = (x_max * 8 + 64).next_power_of_two();
        self.ring_l = vec![0.0; size];
        self.ring_r = vec![0.0; size];
        self.mask = size - 1;
        self.derive();
    }

    fn derive(&mut self) {
        self.x = fade_samples(self.quality, self.sample_rate);
        self.latency = latency_samples(self.quality, self.sample_rate);
    }

    /// Back to the rest state: empty rings, the head `D` behind the write head.
    fn restart(&mut self) {
        self.ring_l.fill(0.0);
        self.ring_r.fill(0.0);
        self.w = 0;
        self.head = 1.0 - self.latency as f64;
        self.fade = None;
    }

    #[inline]
    fn at(&self, i: i64) -> (f32, f32) {
        let k = (i as usize) & self.mask;
        (self.ring_l[k], self.ring_r[k])
    }

    /// The mono sum at an absolute index.
    #[inline]
    fn mono(&self, i: i64) -> f32 {
        let (l, r) = self.at(i);
        (l + r) * 0.5
    }

    /// Four-point cubic read at the fractional index `p`.
    #[inline]
    fn read(&self, p: f64) -> (f32, f32) {
        let i = p.floor();
        let t = (p - i) as f32;
        let i = i as i64;
        let (a, b, c, d) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        let cubic = |y0: f32, y1: f32, y2: f32, y3: f32| {
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            ((c3 * t + c2) * t + c1) * t + y1
        };
        (cubic(a.0, b.0, c.0, d.0), cubic(a.1, b.1, c.1, d.1))
    }

    /// The correlation score of the stretch of `len` samples (every `step`-th) at `a` against the
    /// one at `b`: normalised, so a louder candidate does not win.
    fn score(&self, a: i64, b: i64, len: usize, step: usize) -> f32 {
        let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
        let mut i = 0;
        while i < len {
            let (x, y) = (self.mono(a + i as i64), self.mono(b + i as i64));
            ab += x * y;
            aa += x * x;
            bb += y * y;
            i += step;
        }
        ab / (aa * bb + 1e-12).sqrt()
    }

    /// The jump, in samples, from the head at `base` (an integer index) that lines the signal
    /// after it up best with the signal after the head moved by `sign * jump`; `sign` is -1 for
    /// a head that goes back. `len` is how much of the signal is compared.
    fn best_jump(&self, base: i64, sign: i64, len: usize) -> usize {
        let g = self.x * 2;
        let s = self.x;
        let len = len.max(2 * COARSE);
        let energy: f32 = (0..len)
            .step_by(COARSE)
            .map(|i| {
                let v = self.mono(base + i as i64);
                v * v
            })
            .sum();
        if energy < 1e-10 {
            return g;
        }
        let at = |jump: usize| base + sign * jump as i64;
        let mut best = (g, f32::MIN);
        let mut jump = g - s;
        while jump <= g + s {
            let c = self.score(base, at(jump), len, COARSE);
            if c > best.1 {
                best = (jump, c);
            }
            jump += COARSE;
        }
        let mut fine = (best.0, f32::MIN);
        let lo = best.0.saturating_sub(COARSE - 1).max(g - s);
        let hi = (best.0 + COARSE - 1).min(g + s);
        for jump in lo..=hi {
            let c = self.score(base, at(jump), len, 1);
            if c > fine.1 {
                fine = (jump, c);
            }
        }
        fine.0
    }

    /// Starts the cross-fade for the head at `p`: with the search (`rate` is not 1) or straight
    /// to the delay `D` (`rate` is 1, the head coming home).
    fn start_fade(&mut self, rest: bool, rate: f64, delay: f64) {
        let p = self.head;
        let to = if rest {
            (self.w - self.latency as i64) as f64
        } else {
            // A shift up moves the head back, a shift down forward.
            let sign = if rate > 1.0 { -1 } else { 1 };
            let reach = (delay as usize).saturating_sub(MARGIN + 2);
            let len = ((self.x as f64 * rate) as usize).min(reach);
            let jump = self.best_jump(p.floor() as i64, sign, len);
            p + (sign * jump as i64) as f64
        };
        self.fade = Some(Fade {
            to,
            done: 0,
            len: self.x,
        });
    }
}

impl Device for PitchShifter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.resize();
        self.reset();
    }

    fn reset(&mut self) {
        self.restart();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_SEMITONES => self.semitones = v.clamp(-MAX_SHIFT, MAX_SHIFT),
            P_CENTS => self.cents = v.clamp(-50.0, 50.0),
            P_QUALITY => {
                let q = (v.clamp(0.0, 2.0).round() as usize).min(FADE_MS.len() - 1);
                if q != self.quality {
                    self.quality = q;
                    self.derive();
                    self.restart();
                }
            }
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_PITCH_TRIM => self.pitch_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let cv = clean(batch_mean(ctx.input(PITCH_CV)));
        let shift = (self.semitones + self.cents * 0.01 + self.pitch_trim * cv * CV_SEMITONES)
            .clamp(-MAX_SHIFT, MAX_SHIFT);
        // Exactly zero is the rest state: the head stands still at `D`.
        let rest = shift.abs() < 1e-6;
        let rate = if rest {
            1.0
        } else {
            f64::from(exp2_det(shift / 12.0))
        };
        let (d_lo, g) = ((self.x + MARGIN) as f64, (self.x * 2) as f64);
        let d_hi = d_lo + g + self.x as f64;
        let latency = self.latency as i64;

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            // The newest sample.
            self.w += 1;
            let k = (self.w as usize) & self.mask;
            self.ring_l[k] = finite(in_l[n]);
            self.ring_r[k] = finite(in_r[n]);
            let w = self.w as f64;

            // A head must stay behind the write head by the cubic's reach.
            let floor = w - 2.0;
            if self.head > floor {
                self.head = floor;
            }
            if let Some(f) = self.fade.as_mut()
                && f.to > floor
            {
                f.to = floor;
            }

            // Decide on a jump.
            if self.fade.is_none() {
                let d = w - self.head;
                let jump = if rest {
                    (d - self.latency as f64).abs() > 0.5
                } else if rate > 1.0 {
                    d <= d_lo
                } else {
                    d >= d_hi
                };
                if jump {
                    self.start_fade(rest, rate, d);
                }
            }

            // The shifted sample.
            let wet = if rest && self.fade.is_none() {
                // Exact at the rest state: an integer index.
                self.at(self.w - latency)
            } else {
                let a = self.read(self.head);
                match self.fade {
                    None => a,
                    Some(f) => {
                        let b = self.read(f.to);
                        let t = (f.done + 1) as f32 / f.len as f32;
                        let s = sin_cycles(f64::from(t) * 0.25) as f32;
                        let w2 = s * s;
                        let w1 = 1.0 - w2;
                        (a.0 * w1 + b.0 * w2, a.1 * w1 + b.1 * w2)
                    }
                }
            };
            // Advance the heads.
            self.head += rate;
            if let Some(f) = self.fade.as_mut() {
                f.to += rate;
                f.done += 1;
                if f.done >= f.len {
                    self.head = f.to;
                    self.fade = None;
                }
            }

            if self.mix >= 1.0 {
                out_l[n] = wet.0;
                out_r[n] = wet.1;
            } else {
                let dry = self.at(self.w - latency);
                out_l[n] = dry.0 * (1.0 - self.mix) + wet.0 * self.mix;
                out_r[n] = dry.1 * (1.0 - self.mix) + wet.1 * self.mix;
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }

    fn latency_frames(&self) -> u32 {
        self.latency as u32
    }

    fn variable_latency(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
