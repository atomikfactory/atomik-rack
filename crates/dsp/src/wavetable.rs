//! Band-limited wavetables: a factory bank generated in code, played without aliasing.
//!
//! * **Tables.** [`TABLE_COUNT`] tables of [`FRAMES`] frames each. Every frame is one cycle of a
//!   waveform, defined by its harmonics (amplitude and phase of each sine, up to
//!   [`MAX_HARMONICS`]): the additive families (saw to square, pulse width, sine to saw, hollow,
//!   triangle to saw, vowels, a resonant sweep, a drawbar organ, seeded random spectra) are written
//!   down directly, the others (hard-sync sweep, two phase-modulation families) are drawn in the
//!   time domain and read back with a forward FFT. Nothing is read from a file: the bank is plain
//!   arithmetic, so it is bit-identical on every platform (the twiddles use [`sin_cycles`], the
//!   formant curves [`exp2_det_f64`], the random spectra the seeded [`Rng`]).
//! * **Frame position.** A position `0..=1` morphs from the first frame to the last; between two
//!   frames the output is their linear blend, so it is continuous in the position.
//! * **Mip levels.** Each frame is stored once per octave of bandwidth: level `m` keeps the first
//!   [`LEVEL_HARMONICS`]`[m]` harmonics (1023, 512, 256, ... 1), made by an inverse FFT of the
//!   truncated spectrum (a textbook "FFT truncation"), at a length of eight samples per top
//!   harmonic (at most 2048). [`level_for`] picks the richest level whose top harmonic stays below
//!   Nyquist for a fundamental, so a read never aliases; [`Wavetable::read`] interpolates the
//!   cycle with a four-point Hermite and the two frames linearly.
//! * **Peak level.** Each frame is scaled so its full-bandwidth waveform peaks at 1. A waveform
//!   with a jump rings at the top (Gibbs: a jump of 2 overshoots by 18 %), so a square or a saw
//!   sits a little under 1 between its jumps.
//!
//! The bank is built once, on first use, off the audio thread ([`factory_bank`]); reading is
//! allocation-free and lock-free. [`display_frame`] draws one interpolated frame for a panel
//! without building the bank.

use std::sync::OnceLock;

use crate::Rng;
use crate::det_math::exp2_det_f64;
use crate::det_trig::{cos_cycles, sin_cycles};
use crate::sample_read::hermite4;

/// Number of frames in every table.
pub const FRAMES: usize = 16;
/// Number of factory tables.
pub const TABLE_COUNT: usize = 12;
/// Highest harmonic a frame defines (a 2048-sample cycle has room for 1023).
pub const MAX_HARMONICS: usize = 1023;
/// Number of mip levels.
pub const LEVELS: usize = 11;
/// Highest harmonic kept by each mip level: level 0 is full bandwidth, each next level halves it.
pub const LEVEL_HARMONICS: [usize; LEVELS] = [1023, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1];
/// Samples per cycle of each mip level (eight per top harmonic, at most 2048): powers of two.
pub const LEVEL_LEN: [usize; LEVELS] = [2048, 2048, 2048, 1024, 512, 256, 128, 64, 32, 16, 8];
/// Longest stored cycle.
pub const CYCLE_LEN: usize = 2048;
/// The factory table names, in table order.
pub const TABLE_NAMES: [&str; TABLE_COUNT] = [
    "saw>square",
    "pulse width",
    "sine>saw",
    "sine>hollow",
    "triangle>saw",
    "vowels",
    "harmonic sweep",
    "sync sweep",
    "fm 1:2",
    "fm 1:3",
    "digital",
    "organ",
];

/// Floats of one frame over all its mip levels.
const FRAME_FLOATS: usize = 8184;
/// Where each level starts inside a frame's floats.
const LEVEL_OFFSET: [usize; LEVELS] = [
    0, 2048, 4096, 6144, 7168, 7680, 7936, 8064, 8128, 8160, 8176,
];
/// Size of the time-domain drawing of the hard-sync sweep (four times the cycle, to keep the
/// jumps from folding into the harmonics we keep).
const SYNC_DRAW_LEN: usize = 4096;
/// Seed of the "digital" table's random spectra.
const DIGITAL_SEED: u64 = 0x5741_5645_5441_424C;

/// The mip level to read at `freq_hz` and `sample_rate`: the richest whose top harmonic is still
/// below Nyquist (level 0 for fundamentals under about 23 Hz, the sine level above Nyquist).
#[inline]
pub fn level_for(freq_hz: f32, sample_rate: f32) -> usize {
    let room = 0.5 * sample_rate / freq_hz.max(1.0e-3);
    // Harmonics that fit below Nyquist; saturate before the cast.
    let room = if room >= 1_024.0 {
        1_024
    } else {
        room as usize
    };
    LEVEL_HARMONICS
        .iter()
        .position(|&h| h <= room)
        .unwrap_or(LEVELS - 1)
}

/// One table: [`FRAMES`] frames of [`LEVELS`] mip levels, all in one allocation.
#[derive(Debug, Clone, PartialEq)]
pub struct Wavetable {
    data: Box<[f32]>,
}

impl Wavetable {
    /// The cycle of `frame` at mip `level` (length [`LEVEL_LEN`]`[level]`).
    pub fn cycle(&self, frame: usize, level: usize) -> &[f32] {
        let at = frame.min(FRAMES - 1) * FRAME_FLOATS + LEVEL_OFFSET[level.min(LEVELS - 1)];
        &self.data[at..at + LEVEL_LEN[level.min(LEVELS - 1)]]
    }

    /// Reads the table at frame `position` (`0..=1`, clamped) and `phase` (cycles, any value:
    /// only the fractional part counts) from mip `level`. Allocation-free.
    #[inline]
    pub fn read(&self, level: usize, position: f32, phase: f64) -> f32 {
        let level = level.min(LEVELS - 1);
        let len = LEVEL_LEN[level];
        let x = (phase - phase.floor()) * len as f64;
        let i = (x as usize).min(len - 1);
        let frac = (x - i as f64) as f32;
        let at = position.clamp(0.0, 1.0) * (FRAMES - 1) as f32;
        let a = (at as usize).min(FRAMES - 2);
        let blend = at - a as f32;
        let first = self.read_frame(level, a, i, frac);
        if blend == 0.0 {
            return first;
        }
        let second = self.read_frame(level, a + 1, i, frac);
        first + (second - first) * blend
    }

    #[inline]
    fn read_frame(&self, level: usize, frame: usize, i: usize, frac: f32) -> f32 {
        let len = LEVEL_LEN[level];
        let mask = len - 1;
        let base = frame * FRAME_FLOATS + LEVEL_OFFSET[level];
        let cycle = &self.data[base..base + len];
        hermite4(
            cycle[(i + len - 1) & mask],
            cycle[i],
            cycle[(i + 1) & mask],
            cycle[(i + 2) & mask],
            frac,
        )
    }
}

/// The factory tables.
#[derive(Debug)]
pub struct WavetableBank {
    tables: Vec<Wavetable>,
}

impl WavetableBank {
    /// Builds every table. Takes a moment (a few hundred inverse FFTs): call it off the audio
    /// thread, or use [`factory_bank`], which does it once.
    pub fn build() -> Self {
        let mut synth = Synth::new();
        Self {
            tables: (0..TABLE_COUNT).map(|t| synth.table(t)).collect(),
        }
    }

    /// Table `index` (clamped to the last table).
    pub fn table(&self, index: usize) -> &Wavetable {
        &self.tables[index.min(TABLE_COUNT - 1)]
    }

    /// Number of tables ([`TABLE_COUNT`]).
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Whether the bank holds no table (never, for the factory bank).
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

/// The factory bank, built on first use and shared by every device and thread.
pub fn factory_bank() -> &'static WavetableBank {
    static BANK: OnceLock<WavetableBank> = OnceLock::new();
    BANK.get_or_init(WavetableBank::build)
}

/// Writes one cycle of table `table` at frame `position` (`0..=1`) into `out` (full bandwidth,
/// resampled linearly to `out.len()` points, the first point at phase 0), for a panel display.
/// Does not build the bank.
pub fn display_frame(table: usize, position: f32, out: &mut [f32]) {
    if out.is_empty() {
        return;
    }
    let at = position.clamp(0.0, 1.0) * (FRAMES - 1) as f32;
    let a = (at as usize).min(FRAMES - 2);
    let blend = at - a as f32;
    let mut synth = Synth::new();
    let first = synth.cycle(table, a);
    let second = if blend == 0.0 {
        first.clone()
    } else {
        synth.cycle(table, a + 1)
    };
    let points = out.len();
    for (n, o) in out.iter_mut().enumerate() {
        let x = n as f64 / points as f64 * CYCLE_LEN as f64;
        let i = (x as usize).min(CYCLE_LEN - 1);
        let f = (x - i as f64) as f32;
        let read = |c: &[f32]| c[i] + (c[(i + 1) % CYCLE_LEN] - c[i]) * f;
        let (u, v) = (read(&first), read(&second));
        *o = u + (v - u) * blend;
    }
}

// ------------------------------------------------------------------------------------- spectra

/// One harmonic: `re` is the amplitude of its sine, `im` the amplitude of its cosine.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Harmonic {
    re: f64,
    im: f64,
}

/// A frame: harmonics `1..=MAX_HARMONICS` (index 0, the DC, stays zero).
type Spectrum = Vec<Harmonic>;

fn silent() -> Spectrum {
    vec![Harmonic::default(); MAX_HARMONICS + 1]
}

/// A spectrum whose harmonic `h` is a sine of amplitude `amp(h)`.
fn sines(amp: impl Fn(usize) -> f64) -> Spectrum {
    let mut s = silent();
    for (h, x) in s.iter_mut().enumerate().skip(1) {
        x.re = amp(h);
    }
    s
}

fn odd(h: usize) -> bool {
    h % 2 == 1
}

/// `exp(-x^2)` from the deterministic `2^x`.
fn gauss(x: f64) -> f64 {
    exp2_det_f64(-x * x * std::f64::consts::LOG2_E)
}

/// The five vowels' first three formants in Hz (public textbook averages for a male voice).
const VOWELS: [[f64; 3]; 5] = [
    [800.0, 1150.0, 2900.0],
    [400.0, 1600.0, 2700.0],
    [270.0, 2140.0, 2950.0],
    [450.0, 800.0, 2830.0],
    [325.0, 700.0, 2530.0],
];

/// Spectrum of `table` at frame `frame` before the level scaling.
fn raw_spectrum(table: usize, frame: usize) -> Spectrum {
    use std::f64::consts::PI;
    let t = frame.min(FRAMES - 1) as f64 / (FRAMES - 1) as f64;
    let pi_h = |h: usize| PI * h as f64;
    match table {
        // A falling saw whose even harmonics fade out: a square at the end.
        0 => sines(|h| {
            let even = if odd(h) { 2.0 * t } else { 0.0 };
            2.0 / pi_h(h) * ((1.0 - t) + even)
        }),
        // A pulse narrowing from 50 % to 5 %.
        1 => {
            let d = 0.5 - 0.45 * t;
            let mut s = silent();
            for (h, x) in s.iter_mut().enumerate().skip(1) {
                let turn = h as f64 * d;
                x.re = 2.0 * (1.0 - cos_cycles(turn)) / pi_h(h);
                x.im = 2.0 * sin_cycles(turn) / pi_h(h);
            }
            s
        }
        // A sine that gains harmonics one by one, the newest fading in: a saw at the end.
        2 => {
            let count = 1.0 + 62.0 * t;
            sines(|h| 2.0 / pi_h(h) * (count - h as f64 + 1.0).clamp(0.0, 1.0))
        }
        // The same with odd harmonics only: ends on a square.
        3 => {
            let count = 1.0 + 62.0 * t;
            sines(|h| {
                if odd(h) {
                    4.0 / pi_h(h) * (count - h as f64 + 1.0).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
        }
        // A triangle blended into a falling saw.
        4 => sines(|h| {
            let tri = if odd(h) {
                let sign = if (h / 2) % 2 == 0 { 1.0 } else { -1.0 };
                8.0 / (PI * PI) * sign / (h * h) as f64
            } else {
                0.0
            };
            (1.0 - t) * tri + t * 2.0 / pi_h(h)
        }),
        // a, e, i, o, u: three formant peaks over a falling source, sliding between vowels.
        5 => {
            let v = t * (VOWELS.len() - 1) as f64;
            let i = (v as usize).min(VOWELS.len() - 2);
            let f = v - i as f64;
            let formant = |k: usize| VOWELS[i][k] + (VOWELS[i + 1][k] - VOWELS[i][k]) * f;
            let (gain, width) = ([1.0, 0.7, 0.4], [110.0, 160.0, 260.0]);
            // The table is written for a 110 Hz cycle: harmonic `h` sits at 110 h Hz.
            sines(|h| {
                let hz = 110.0 * h as f64;
                let peaks: f64 = (0..3)
                    .map(|k| gain[k] * gauss((hz - formant(k)) / width[k]))
                    .sum();
                (0.03 + peaks) / h as f64
            })
        }
        // A resonant peak climbing from the fundamental to the 32nd harmonic.
        6 => {
            let centre = 1.0 + 31.0 * t;
            let width = 1.0 + 0.15 * centre;
            sines(|h| (0.5 + 8.0 * gauss((h as f64 - centre) / width)) / h as f64)
        }
        // A saw synced to a master cycle: the slave runs one to eight times per cycle.
        7 => {
            let ratio = 1.0 + 7.0 * t;
            let x: Vec<f64> = (0..SYNC_DRAW_LEN)
                .map(|n| {
                    let p = ratio * n as f64 / SYNC_DRAW_LEN as f64;
                    1.0 - 2.0 * (p - p.floor())
                })
                .collect();
            from_time(&x)
        }
        // Phase modulation, modulator at twice the carrier, the index opening from 0 to 6.
        8 => phase_modulated(2.0, 6.0 * t),
        // The same with the modulator at three times the carrier, index 0 to 5.
        9 => phase_modulated(3.0, 5.0 * t),
        // Four seeded random spectra with a falling slope, crossfaded.
        10 => {
            let keys = digital_keys();
            let v = t * (keys.len() - 1) as f64;
            let i = (v as usize).min(keys.len() - 2);
            let f = v - i as f64;
            let mut s = silent();
            for (h, x) in s.iter_mut().enumerate() {
                x.re = keys[i][h].re + (keys[i + 1][h].re - keys[i][h].re) * f;
                x.im = keys[i][h].im + (keys[i + 1][h].im - keys[i][h].im) * f;
            }
            s
        }
        // Drawbar partials 1, 2, 3, 4, 5, 6 and 8 whose levels swell in turn.
        _ => {
            const PARTIALS: [(usize, f64); 7] = [
                (1, 1.0),
                (2, 0.8),
                (3, 0.8),
                (4, 0.6),
                (5, 0.4),
                (6, 0.3),
                (8, 0.2),
            ];
            let mut s = silent();
            for (k, (h, level)) in PARTIALS.iter().enumerate() {
                let swell = 0.5 + 0.5 * cos_cycles(1.5 * t - k as f64 / PARTIALS.len() as f64);
                s[*h].re = level * (0.35 + 0.65 * swell);
            }
            s
        }
    }
}

/// `sin(2 pi p + index sin(2 pi ratio p))`, drawn over one cycle and analysed.
fn phase_modulated(ratio: f64, index: f64) -> Spectrum {
    let m = CYCLE_LEN;
    let x: Vec<f64> = (0..m)
        .map(|n| {
            let p = n as f64 / m as f64;
            sin_cycles(p + index / std::f64::consts::TAU * sin_cycles(ratio * p))
        })
        .collect();
    from_time(&x)
}

/// The "digital" table's four keyframes: random amplitude and phase for harmonics 1 to 48.
fn digital_keys() -> [Spectrum; 4] {
    let mut rng = Rng::new(DIGITAL_SEED);
    std::array::from_fn(|_| {
        let mut s = silent();
        for (h, x) in s.iter_mut().enumerate().take(49).skip(1) {
            let amp = f64::from(rng.next_f32());
            let amp = amp * amp / (h as f64).sqrt();
            let turn = f64::from(rng.next_f32());
            x.re = amp * cos_cycles(turn);
            x.im = amp * sin_cycles(turn);
        }
        s
    })
}

/// The harmonics of one cycle drawn in the time domain (`x.len()` a power of two, at least 2048).
fn from_time(x: &[f64]) -> Spectrum {
    let m = x.len();
    let fft = Fft::new(m);
    let mut re = x.to_vec();
    let mut im = vec![0.0; m];
    fft.run(&mut re, &mut im, -1.0);
    // x = sum s sin + c cos gives X[h] = m/2 (c - i s).
    let scale = 2.0 / m as f64;
    let mut s = silent();
    for (h, out) in s.iter_mut().enumerate().take(m / 2).skip(1) {
        out.re = -im[h] * scale;
        out.im = re[h] * scale;
    }
    s
}

// ---------------------------------------------------------------------------------------- FFT

/// A radix-2 complex FFT with twiddles from the deterministic sine.
struct Fft {
    n: usize,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    fn new(n: usize) -> Self {
        debug_assert!(n.is_power_of_two() && n >= 2);
        let turn = |k: usize| k as f64 / n as f64;
        Self {
            n,
            cos: (0..n / 2).map(|k| cos_cycles(turn(k))).collect(),
            sin: (0..n / 2).map(|k| sin_cycles(turn(k))).collect(),
        }
    }

    /// In place, unnormalised: `sign` -1 is the forward transform, +1 the inverse.
    fn run(&self, re: &mut [f64], im: &mut [f64], sign: f64) {
        let n = self.n;
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut half = 1;
        while half < n {
            let step = n / (2 * half);
            for start in (0..n).step_by(2 * half) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], sign * self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            half *= 2;
        }
    }
}

// --------------------------------------------------------------------------------- the builder

/// Builds frames: holds the inverse FFTs of every cycle length.
struct Synth {
    ffts: Vec<Fft>,
    re: Vec<f64>,
    im: Vec<f64>,
}

impl Synth {
    fn new() -> Self {
        Self {
            ffts: Vec::new(),
            re: Vec::new(),
            im: Vec::new(),
        }
    }

    /// Writes the first `harmonics` harmonics of `spec` as one cycle of `out.len()` samples,
    /// scaled by `gain`.
    fn render(&mut self, spec: &[Harmonic], harmonics: usize, gain: f64, out: &mut [f32]) {
        let n = out.len();
        let at = match self.ffts.iter().position(|f| f.n == n) {
            Some(at) => at,
            None => {
                self.ffts.push(Fft::new(n));
                self.ffts.len() - 1
            }
        };
        self.re.clear();
        self.re.resize(n, 0.0);
        self.im.clear();
        self.im.resize(n, 0.0);
        for (h, x) in spec.iter().enumerate().take(harmonics + 1).skip(1) {
            self.re[h] = x.re;
            self.im[h] = x.im;
        }
        self.ffts[at].run(&mut self.re, &mut self.im, 1.0);
        // sum (re + i im) e^{i w n}: the imaginary part is sum re sin + im cos.
        for (o, v) in out.iter_mut().zip(&self.im) {
            *o = (v * gain) as f32;
        }
    }

    /// The gain that makes the full-bandwidth cycle of `spec` peak at 1.
    fn peak_gain(&mut self, spec: &[Harmonic]) -> f64 {
        let mut full = vec![0.0f32; CYCLE_LEN];
        self.render(spec, MAX_HARMONICS, 1.0, &mut full);
        let peak = full.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1.0e-9 {
            1.0 / f64::from(peak)
        } else {
            1.0
        }
    }

    /// One full-bandwidth, peak-normalised cycle.
    fn cycle(&mut self, table: usize, frame: usize) -> Vec<f32> {
        let spec = raw_spectrum(table, frame);
        let gain = self.peak_gain(&spec);
        let mut out = vec![0.0f32; CYCLE_LEN];
        self.render(&spec, MAX_HARMONICS, gain, &mut out);
        out
    }

    fn table(&mut self, table: usize) -> Wavetable {
        let mut data = vec![0.0f32; FRAMES * FRAME_FLOATS];
        for frame in 0..FRAMES {
            let spec = raw_spectrum(table, frame);
            let gain = self.peak_gain(&spec);
            for level in 0..LEVELS {
                let at = frame * FRAME_FLOATS + LEVEL_OFFSET[level];
                let out = &mut data[at..at + LEVEL_LEN[level]];
                self.render(&spec, LEVEL_HARMONICS[level], gain, out);
            }
        }
        Wavetable {
            data: data.into_boxed_slice(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank shared by the tests of this module (built once).
    fn bank() -> &'static WavetableBank {
        factory_bank()
    }

    #[test]
    fn layout_constants_agree() {
        let mut at = 0;
        for l in 0..LEVELS {
            assert_eq!(LEVEL_OFFSET[l], at);
            assert!(LEVEL_LEN[l].is_power_of_two());
            // Eight samples per top harmonic, capped at the cycle length.
            assert_eq!(
                LEVEL_LEN[l],
                (8 * LEVEL_HARMONICS[l]).next_power_of_two().min(CYCLE_LEN)
            );
            at += LEVEL_LEN[l];
        }
        assert_eq!(at, FRAME_FLOATS);
        assert_eq!(TABLE_NAMES.len(), TABLE_COUNT);
    }

    #[test]
    fn fft_round_trips_a_known_cycle() {
        let n = 256;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                0.5 * sin_cycles(3.0 * i as f64 / n as f64)
                    + 0.25 * cos_cycles(7.0 * i as f64 / n as f64)
            })
            .collect();
        let s = from_time(&x[..]);
        // Sizes below the cycle length still analyse (only the first harmonics are filled).
        assert!(
            (s[3].re - 0.5).abs() < 1e-9 && s[3].im.abs() < 1e-9,
            "{:?}",
            s[3]
        );
        assert!(
            (s[7].im - 0.25).abs() < 1e-9 && s[7].re.abs() < 1e-9,
            "{:?}",
            s[7]
        );
        assert!(s[4].re.abs() < 1e-9 && s[4].im.abs() < 1e-9);
    }

    #[test]
    fn saw_frame_is_a_falling_saw_peaking_at_one() {
        let b = bank();
        let c = b.table(0).cycle(0, 0);
        let peak = c.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 1e-3, "{peak}");
        // A falling saw: positive just after the start, negative just before the end.
        assert!(c[40] > 0.5 && c[CYCLE_LEN - 40] < -0.5);
        // Zero mean (no DC).
        let mean: f32 = c.iter().sum::<f32>() / CYCLE_LEN as f32;
        assert!(mean.abs() < 1e-4, "{mean}");
    }

    #[test]
    fn last_frame_of_the_first_table_is_a_square() {
        let c = bank().table(0).cycle(FRAMES - 1, 0);
        // The plateau sits under the Gibbs overshoot the peak scaling took out (a jump of 2 rings to 1.18: about 0.85).
        let plateau = |from: usize| c[from..from + 200].iter().sum::<f32>() / 200.0;
        assert!(plateau(200) > 0.83 && plateau(CYCLE_LEN - 400) < -0.83);
        // Odd symmetry: x(t + 1/2) = -x(t).
        for n in [10, 300, 900] {
            assert!((c[n] + c[n + CYCLE_LEN / 2]).abs() < 1e-3);
        }
    }

    #[test]
    fn every_frame_is_finite_and_audible() {
        let b = bank();
        for t in 0..TABLE_COUNT {
            for f in 0..FRAMES {
                for (l, len) in LEVEL_LEN.iter().enumerate() {
                    let c = b.table(t).cycle(f, l);
                    assert_eq!(c.len(), *len);
                    assert!(
                        c.iter().all(|s| s.is_finite() && s.abs() < 2.0),
                        "{t} {f} {l}"
                    );
                    let rms = (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt();
                    // The thin top levels of a sync sweep can be nearly silent: audible down to
                    // level 4 (64 harmonics).
                    if l <= 4 {
                        assert!(rms > 0.05, "table {t} frame {f} level {l}: rms {rms}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_sine_level_is_a_sine() {
        let c = bank().table(0).cycle(0, LEVELS - 1);
        for (n, s) in c.iter().enumerate() {
            let want = sin_cycles(n as f64 / c.len() as f64) as f32;
            // Peak normalisation scales a lone fundamental; compare the shape.
            let peak = c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((s / peak - want).abs() < 1e-4);
        }
    }

    #[test]
    fn level_for_keeps_the_top_harmonic_below_nyquist() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            for f in [
                10.0f32, 23.0, 55.0, 110.0, 440.0, 1_000.0, 4_186.0, 12_000.0, 30_000.0,
            ] {
                let l = level_for(f, sr);
                let top = LEVEL_HARMONICS[l] as f32 * f;
                assert!(
                    top <= 0.5 * sr || l == LEVELS - 1,
                    "{f} Hz at {sr}: level {l} reaches {top} Hz"
                );
                // And it is the richest such level.
                if l > 0 {
                    assert!(
                        LEVEL_HARMONICS[l - 1] as f32 * f > 0.5 * sr,
                        "{f} Hz at {sr}"
                    );
                }
            }
        }
        assert_eq!(level_for(5.0, 48_000.0), 0);
        assert_eq!(level_for(40_000.0, 48_000.0), LEVELS - 1);
        assert_eq!(level_for(0.0, 48_000.0), 0);
    }

    #[test]
    fn read_matches_the_stored_cycle_at_its_samples() {
        let t = bank().table(2);
        let c = t.cycle(5, 3);
        for n in [0, 1, 77, 1023] {
            let got = t.read(3, 5.0 / (FRAMES - 1) as f32, n as f64 / c.len() as f64);
            assert!((got - c[n]).abs() < 1e-5, "{n}: {got} vs {}", c[n]);
        }
    }

    #[test]
    fn read_is_periodic_and_blends_frames_linearly() {
        let t = bank().table(1);
        let p = 0.3f64;
        assert!((t.read(2, 0.4, p) - t.read(2, 0.4, p + 3.0)).abs() < 1e-6);
        assert!((t.read(2, 0.4, p) - t.read(2, 0.4, p - 1.0)).abs() < 1e-6);
        // Halfway between frames 4 and 5 is their mean.
        let mid = 4.5 / (FRAMES - 1) as f32;
        let (a, b) = (4.0 / (FRAMES - 1) as f32, 5.0 / (FRAMES - 1) as f32);
        let want = 0.5 * (t.read(2, a, p) + t.read(2, b, p));
        assert!((t.read(2, mid, p) - want).abs() < 1e-5);
        // Out-of-range positions clamp to the first and last frame.
        assert_eq!(t.read(2, -3.0, p).to_bits(), t.read(2, 0.0, p).to_bits());
        assert_eq!(t.read(2, 9.0, p).to_bits(), t.read(2, 1.0, p).to_bits());
    }

    #[test]
    fn position_morph_is_continuous() {
        let b = bank();
        for t in 0..TABLE_COUNT {
            let table = b.table(t);
            // Between two frames the sweep is a straight line, so a step of the position moves
            // the output by at most the largest frame-to-frame difference per step.
            for phase in [0.05f64, 0.31, 0.77] {
                let at = |f: usize| table.read(2, f as f32 / (FRAMES - 1) as f32, phase);
                let gap = (1..FRAMES)
                    .map(|f| (at(f) - at(f - 1)).abs())
                    .fold(0.0f32, f32::max);
                let steps = 150 * (FRAMES - 1);
                let allowed = gap * (FRAMES - 1) as f32 / steps as f32 * 1.01 + 1.0e-6;
                let mut prev = table.read(2, 0.0, phase);
                for i in 1..=steps {
                    let now = table.read(2, i as f32 / steps as f32, phase);
                    assert!(
                        (now - prev).abs() <= allowed,
                        "table {t} phase {phase}: step {} > {allowed}",
                        (now - prev).abs()
                    );
                    prev = now;
                }
            }
        }
    }

    #[test]
    fn display_frame_matches_the_bank_cycle() {
        let mut out = [0.0f32; 128];
        display_frame(5, 6.0 / (FRAMES - 1) as f32, &mut out);
        let c = bank().table(5).cycle(6, 0);
        for (n, v) in out.iter().enumerate() {
            let want = c[n * CYCLE_LEN / 128];
            assert!((v - want).abs() < 1e-5, "{n}");
        }
        display_frame(0, 0.0, &mut []);
    }

    #[test]
    fn the_bank_is_deterministic() {
        let again = WavetableBank::build();
        let b = bank();
        for t in 0..TABLE_COUNT {
            assert_eq!(b.table(t), again.table(t), "table {t}");
        }
    }

    #[test]
    fn the_tables_differ_from_each_other() {
        let b = bank();
        for t in 1..TABLE_COUNT {
            assert_ne!(b.table(t - 1).cycle(8, 0), b.table(t).cycle(8, 0));
        }
        // And a table's frames differ along its position.
        for t in 0..TABLE_COUNT {
            assert_ne!(
                b.table(t).cycle(0, 0),
                b.table(t).cycle(FRAMES - 1, 0),
                "table {t}"
            );
        }
    }
}
