//! The granular sampler's blocks: windows, a grain pool, a grain clock and the sample read that
//! keeps a high transposition from aliasing. An original design from the textbook of granular
//! synthesis (Gabor, Xenakis, Roads); the voice, the controls and the device are in
//! `atomik-devices`.
//!
//! * **Grains.** A grain is a short windowed read of a source: it starts at a position in the
//!   source, advances by `step` source frames per output sample (the pitch ratio, negative for a
//!   reversed grain), lasts `len` output samples and is scaled by a [`Window`] and a stereo
//!   gain pair. A [`GrainPool`] holds [`MAX_GRAINS`] of them in a fixed array: nothing is
//!   allocated after construction. When the pool is full the grain **closest to its end**
//!   (largest `age / len`, ties to the lowest slot) is replaced, so the cut falls where the
//!   window is nearly silent and the choice is a pure function of the pool.
//! * **Windows.** [`Window`] is `Hann`, `Tukey` (a cosine taper of a quarter of the grain on each
//!   side, flat between) or `Percussive` (a 3 % attack, then an exponential decay of about
//!   52 dB, then a 3 % taper to zero). [`WindowBank`] holds them as 512-point tables read with a
//!   linear interpolation. Hann grains at half-length spacing, and Tukey grains at three quarters,
//!   sum to exactly 1: a steady stream has no ripple.
//! * **Clock.** [`GrainClock`] turns a grain rate into spawn instants: a phase accumulator that
//!   fires when it passes a threshold. A regular stream has the exact rate whatever the rate
//!   modulation does (a spawn is never skipped or doubled), and `timing` randomises each
//!   interval around the mean, so the rate holds on average.
//! * **Position.** A position is a frame count from the start of the source window; reads wrap
//!   round the window, so a grain that is longer than the window, or that runs past its end,
//!   loops it. [`fit_start`] moves a start so that a grain that fits the window does not
//!   straddle its edge (no splice inside a grain).
//! * **Interpolation and aliasing.** At a step of 1 or less a read is a four-point Hermite.
//!   Above 1 the read would skip frames, and what lies above `fs / (2 step)` would fold back as an
//!   alias; the read then averages `2n + 1` linearly interpolated taps (`n = ceil(step)`) spread
//!   over `±step` frames with triangular weights, a filter with the response `sinc^2(f step)`
//!   (nulls at `fs / step`, 8 dB down at the new Nyquist rate, 21 dB down at three quarters of the
//!   source rate when the step is 4), blended in over steps 1 to 1.25 so a pitch glide through 1
//!   is continuous. The step is limited to [`MAX_STEP`] (three octaves up).
//!
//! Everything is deterministic: only IEEE `+ - * /`, `floor` and the polynomial helpers of this
//! crate, so every platform rounds identically. [`default_source`] builds the built-in source of
//! the device (a morphing vowel pad) the same way.

mod source;

pub use source::{DEFAULT_SOURCE_ROOT, DEFAULT_SOURCE_SECONDS, default_source};

use crate::det_math::exp2_det_f64;
use crate::rng::Rng;
use crate::sample_read::{hermite4, sin_quarter};

/// The most grains a pool holds.
pub const MAX_GRAINS: usize = 64;
/// Points of a window table (it has one more, for the last interpolation).
pub const WINDOW_POINTS: usize = 512;
/// The largest source step per output sample (three octaves up).
pub const MAX_STEP: f64 = 8.0;
/// The most taps on each side of the centre of the anti-alias read.
pub const MAX_AA_TAPS: usize = 8;
/// The number of windows.
pub const WINDOW_COUNT: usize = 3;
/// The longest grain, in output samples (a read position stays within f64 precision).
pub const MAX_GRAIN_SAMPLES: u32 = 1 << 20;

/// Fraction of a grain taken by the attack and by the closing taper of the percussive window.
const PERC_EDGE: f64 = 0.03;
/// Octaves of decay over the whole percussive grain (about 52 dB).
const PERC_DECAY_OCTAVES: f64 = 8.656;
/// Fraction of a grain taken by each taper of the Tukey window.
const TUKEY_TAPER: f64 = 0.25;

/// The shape of a grain's amplitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Window {
    /// `sin^2(pi p)`: smooth, no flat part.
    #[default]
    Hann,
    /// Cosine tapers over the first and last quarter, flat between.
    Tukey,
    /// A fast attack and an exponential decay: a plucked, percussive grain.
    Percussive,
}

impl Window {
    /// The window of choice index `i` (0 Hann, 1 Tukey, 2 Percussive; others clamp).
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Hann,
            1 => Self::Tukey,
            _ => Self::Percussive,
        }
    }

    /// The index of this window in a [`WindowBank`].
    pub fn index(self) -> usize {
        match self {
            Self::Hann => 0,
            Self::Tukey => 1,
            Self::Percussive => 2,
        }
    }
}

/// The value of window `w` at phase `p` (clamped to `0..=1`): 0 at both ends, at most 1.
pub fn window_value(w: Window, p: f64) -> f64 {
    let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
    match w {
        Window::Hann => {
            let u = if p <= 0.5 { 2.0 * p } else { 2.0 * (1.0 - p) };
            let s = sin_quarter(u);
            s * s
        }
        Window::Tukey => {
            let edge = p.min(1.0 - p);
            if edge >= TUKEY_TAPER {
                1.0
            } else {
                let s = sin_quarter(edge / TUKEY_TAPER);
                s * s
            }
        }
        Window::Percussive => {
            let up = sin_quarter(p / PERC_EDGE);
            let down = sin_quarter((1.0 - p) / PERC_EDGE);
            let decay = exp2_det_f64(-PERC_DECAY_OCTAVES * (p - PERC_EDGE).max(0.0));
            up * up * decay * down * down
        }
    }
}

/// The three windows as tables, read with a linear interpolation (no libm call per sample).
#[derive(Debug, Clone)]
pub struct WindowBank {
    tables: [[f32; WINDOW_POINTS + 1]; WINDOW_COUNT],
    mean_square: [f32; WINDOW_COUNT],
}

impl Default for WindowBank {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowBank {
    /// Builds the tables (non-real-time: it is a few thousand evaluations).
    pub fn new() -> Self {
        let mut tables = [[0.0f32; WINDOW_POINTS + 1]; WINDOW_COUNT];
        let mut mean_square = [0.0f32; WINDOW_COUNT];
        for w in 0..WINDOW_COUNT {
            let window = Window::from_index(w);
            let mut sum = 0.0f64;
            for (i, v) in tables[w].iter_mut().enumerate() {
                let x = window_value(window, i as f64 / WINDOW_POINTS as f64);
                *v = x as f32;
                sum += x * x;
            }
            mean_square[w] = (sum / (WINDOW_POINTS + 1) as f64) as f32;
        }
        Self {
            tables,
            mean_square,
        }
    }

    /// The window at phase `phase` (clamped to `0..=1`).
    #[inline]
    pub fn gain(&self, w: Window, phase: f32) -> f32 {
        let x = if phase.is_nan() {
            0.0
        } else {
            phase.clamp(0.0, 1.0) * WINDOW_POINTS as f32
        };
        let i = (x as usize).min(WINDOW_POINTS);
        let f = x - i as f32;
        let t = &self.tables[w.index()];
        let a = t[i];
        let b = t[(i + 1).min(WINDOW_POINTS)];
        a + (b - a) * f
    }

    /// The mean of the square of window `w`: how much power a grain of it carries (Hann 0.375).
    pub fn mean_square(&self, w: Window) -> f32 {
        self.mean_square[w.index()]
    }
}

/// A window of frames of the audio a grain plays: one channel twice for a mono source, two
/// channels for a stereo one. Positions are frames from the start of the window and wrap round
/// it.
#[derive(Debug, Clone, Copy)]
pub struct GrainSource<'a> {
    left: &'a [f32],
    right: &'a [f32],
    start: usize,
    len: usize,
    mono: bool,
}

impl<'a> GrainSource<'a> {
    /// A stereo window over frames `start..end` of `left` and `right`. Any input is accepted:
    /// `end` is limited to the shorter channel, and a window with `start >= end` is empty
    /// (it reads silence).
    pub fn new(left: &'a [f32], right: &'a [f32], start: u64, end: u64) -> Self {
        Self::build(left, right, start, end, false)
    }

    /// A mono window over frames `start..end` of `channel` (both outputs read it).
    pub fn mono(channel: &'a [f32], start: u64, end: u64) -> Self {
        Self::build(channel, channel, start, end, true)
    }

    fn build(left: &'a [f32], right: &'a [f32], start: u64, end: u64, mono: bool) -> Self {
        let frames = left.len().min(right.len()) as u64;
        let end = end.min(frames);
        let (start, len) = if start < end {
            (start as usize, (end - start) as usize)
        } else {
            (0, 0)
        };
        Self {
            left,
            right,
            start,
            len,
            mono,
        }
    }

    /// Frames in the window.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the window has no frames.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Frame `j` of the window (wrapping round it).
    #[inline]
    fn frame(&self, j: i64) -> (f32, f32) {
        let n = self.len as i64;
        let k = if (0..n).contains(&j) {
            j as usize
        } else {
            j.rem_euclid(n) as usize
        };
        let i = self.start + k;
        (self.left[i], self.right[i])
    }

    /// The window read at `pos` frames with a four-point Hermite.
    #[inline]
    pub fn read_hermite(&self, pos: f64) -> (f32, f32) {
        if self.len == 0 {
            return (0.0, 0.0);
        }
        let pos = if pos.is_finite() {
            pos.clamp(-1.0e12, 1.0e12)
        } else {
            0.0
        };
        let base = pos.floor();
        let t = (pos - base) as f32;
        let i = base as i64;
        let (a, b, c, d) = (
            self.frame(i - 1),
            self.frame(i),
            self.frame(i + 1),
            self.frame(i + 2),
        );
        let l = hermite4(a.0, b.0, c.0, d.0, t);
        let r = if self.mono {
            l
        } else {
            hermite4(a.1, b.1, c.1, d.1, t)
        };
        (l, r)
    }

    /// The window read at `pos` frames with a linear interpolation.
    #[inline]
    pub fn read_linear(&self, pos: f64) -> (f32, f32) {
        if self.len == 0 {
            return (0.0, 0.0);
        }
        let pos = if pos.is_finite() {
            pos.clamp(-1.0e12, 1.0e12)
        } else {
            0.0
        };
        let base = pos.floor();
        let t = (pos - base) as f32;
        let i = base as i64;
        let (a, b) = (self.frame(i), self.frame(i + 1));
        let l = a.0 + (b.0 - a.0) * t;
        let r = if self.mono { l } else { a.1 + (b.1 - a.1) * t };
        (l, r)
    }

    /// The read of a grain at `pos` that advances `step` frames per output sample (`step` is
    /// the absolute value): a Hermite read up to 1, and from there the triangular
    /// anti-alias average (module docs), blended in over steps 1 to 1.25.
    #[inline]
    pub fn read(&self, pos: f64, step: f64) -> (f32, f32) {
        let direct = self.read_hermite(pos);
        if step <= 1.0 {
            return direct;
        }
        let step = step.min(MAX_STEP);
        let n = (step.ceil() as usize).clamp(1, MAX_AA_TAPS);
        let d = step / n as f64;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for k in 0..=2 * n {
            let w = (n - k.abs_diff(n)) as f32;
            if w == 0.0 {
                continue;
            }
            let (x, y) = self.read_linear(pos + (k as f64 - n as f64) * d);
            l += w * x;
            r += w * y;
        }
        let norm = 1.0 / (n * n) as f32;
        let blend = (((step - 1.0) * 4.0) as f32).min(1.0);
        (
            direct.0 + (l * norm - direct.0) * blend,
            direct.1 + (r * norm - direct.1) * blend,
        )
    }
}

/// How to start a grain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainParams {
    /// First read position, in frames from the start of the source window.
    pub pos: f64,
    /// Source frames per output sample: the pitch ratio, negative to read backwards.
    pub step: f64,
    /// Length in output samples (limited to `1..=`[`MAX_GRAIN_SAMPLES`]).
    pub len: u32,
    /// The grain's amplitude shape.
    pub window: Window,
    /// The left and right gains.
    pub gain: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
struct Grain {
    age: u32,
    len: u32,
    pos: f64,
    step: f64,
    abs_step: f64,
    inv_len: f32,
    window: Window,
    gain: [f32; 2],
}

const IDLE: Grain = Grain {
    age: 0,
    len: 0,
    pos: 0.0,
    step: 0.0,
    abs_step: 0.0,
    inv_len: 0.0,
    window: Window::Hann,
    gain: [0.0; 2],
};

impl Grain {
    #[inline]
    fn is_active(&self) -> bool {
        self.age < self.len
    }
}

/// A fixed pool of grains reading one source.
#[derive(Debug, Clone)]
pub struct GrainPool {
    grains: [Grain; MAX_GRAINS],
    limit: usize,
    active: usize,
    stolen: u64,
}

impl Default for GrainPool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrainPool {
    /// An empty pool with the full [`MAX_GRAINS`] available.
    pub fn new() -> Self {
        Self {
            grains: [IDLE; MAX_GRAINS],
            limit: MAX_GRAINS,
            active: 0,
            stolen: 0,
        }
    }

    /// Sets how many slots a new grain may use (`1..=`[`MAX_GRAINS`]). Grains in the slots above
    /// finish; nothing is cut.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.clamp(1, MAX_GRAINS);
    }

    /// The slot limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Grains sounding now.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Grains replaced by a newer one since the last [`GrainPool::clear`] because the pool was
    /// full.
    pub fn stolen(&self) -> u64 {
        self.stolen
    }

    /// Ends every grain at once and clears the steal count.
    pub fn clear(&mut self) {
        self.grains = [IDLE; MAX_GRAINS];
        self.active = 0;
        self.stolen = 0;
    }

    /// Starts a grain in a free slot below the limit, or replaces the grain closest to its end
    /// (module docs). Returns the slot, and whether a grain was replaced.
    pub fn spawn(&mut self, p: GrainParams) -> (usize, bool) {
        let limit = self.limit;
        let free = self.grains[..limit].iter().position(|g| !g.is_active());
        let (slot, stole) = match free {
            Some(i) => (i, false),
            None => {
                let mut best = 0;
                for i in 1..limit {
                    let (a, b) = (&self.grains[i], &self.grains[best]);
                    // age / len, compared without a division.
                    if u64::from(a.age) * u64::from(b.len) > u64::from(b.age) * u64::from(a.len) {
                        best = i;
                    }
                }
                (best, true)
            }
        };
        let len = p.len.clamp(1, MAX_GRAIN_SAMPLES);
        let step = if p.step.is_finite() {
            p.step.clamp(-MAX_STEP, MAX_STEP)
        } else {
            1.0
        };
        self.grains[slot] = Grain {
            age: 0,
            len,
            pos: if p.pos.is_finite() { p.pos } else { 0.0 },
            step,
            abs_step: step.abs(),
            inv_len: 1.0 / len as f32,
            window: p.window,
            gain: p.gain,
        };
        if stole {
            self.stolen += 1;
        } else {
            self.active += 1;
        }
        (slot, stole)
    }

    /// Renders one output sample: the sum of every grain, each read from `src` and scaled by its
    /// window and gains; the grains advance by one sample.
    #[inline]
    pub fn process(&mut self, bank: &WindowBank, src: &GrainSource<'_>) -> (f32, f32) {
        if self.active == 0 {
            return (0.0, 0.0);
        }
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for g in &mut self.grains {
            if g.age >= g.len {
                continue;
            }
            let w = bank.gain(g.window, g.age as f32 * g.inv_len);
            let (x, y) = src.read(g.pos, g.abs_step);
            l += x * w * g.gain[0];
            r += y * w * g.gain[1];
            g.pos += g.step;
            g.age += 1;
            if g.age >= g.len {
                self.active -= 1;
            }
        }
        (l, r)
    }
}

/// Spawn instants from a grain rate: a phase that gains `rate` (grains per sample) every tick and
/// fires when it reaches its threshold, which is 1 or, with timing randomness, a value around 1
/// drawn after each firing (mean 1: the long-run rate stays exact).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainClock {
    phase: f64,
    threshold: f64,
}

impl Default for GrainClock {
    fn default() -> Self {
        Self::new()
    }
}

impl GrainClock {
    /// A clock that fires on its first tick.
    pub fn new() -> Self {
        Self {
            phase: 1.0,
            threshold: 1.0,
        }
    }

    /// Back to the first tick firing.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One sample at `rate` grains per sample (`0..=1`; limited), with `timing` randomness
    /// `0..=1` (0: a regular stream, 1: intervals from 5 % to 195 % of the mean). Returns whether
    /// a grain starts at this sample.
    #[inline]
    pub fn tick(&mut self, rate: f64, timing: f32, rng: &mut Rng) -> bool {
        let rate = if rate.is_finite() {
            rate.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.phase += rate;
        if self.phase < self.threshold {
            return false;
        }
        self.phase -= self.threshold;
        let t = f64::from(timing.clamp(0.0, 1.0));
        self.threshold = if t > 0.0 {
            1.0 + 0.95 * t * f64::from(rng.next_bipolar())
        } else {
            1.0
        };
        true
    }
}

/// Moves a grain start so that a grain spanning `span` source frames stays inside a window of
/// `window` frames when it fits (a forward grain covers `start..start + span`, a reversed one
/// `start - span..start`); a grain longer than the window keeps its start, and the read wraps.
pub fn fit_start(start: f64, span: f64, window: f64, reverse: bool) -> f64 {
    if !(start.is_finite() && span.is_finite() && window.is_finite()) || span >= window {
        return start;
    }
    if reverse {
        start.clamp(span, window)
    } else {
        start.clamp(0.0, window - span)
    }
}

/// `x` folded into `0..=1` by reflection at both ends (`-0.1` gives 0.1, `1.2` gives 0.8); a
/// non-finite value gives 0.
pub fn fold01(x: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let m = x.rem_euclid(2.0);
    if m > 1.0 { 2.0 - m } else { m }
}

#[cfg(test)]
mod tests;
