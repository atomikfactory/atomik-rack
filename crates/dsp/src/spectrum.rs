//! Spectrum analysis for the display side of the analyser device: windowed FFT power per bin,
//! averaging, a log-frequency or 1/3-octave trace in dB, and a peak hold with decay.
//!
//! It runs on the UI side from the frames the device's `ScopeTap` holds, never on the audio
//! thread, so nothing here has to be real-time safe (it still allocates only when the size, the
//! window or the number of columns changes). The FFT is the in-house radix-2 transform of the
//! time-stretch (`stretch::fft`), with twiddles and windows from the deterministic sine of
//! `det_trig` and dB from `log2_det`, so the same frames give the same trace on every platform.
//!
//! * **Calibration.** [`Analyser::analyse`] returns the power per bin normalised so that a sine
//!   of amplitude `A` centred on a bin reads `A^2` (0 dB is a full-scale sine). A sine between two
//!   bins reads up to 1.4 dB (Hann) or 0.8 dB (Blackman-Harris) lower: the scalloping loss of the
//!   window, not an error. White noise reads its power per bin; the 1/3-octave bands divide by
//!   the window's equivalent noise bandwidth ([`Analyser::enbw`], in bins), so a band holds the
//!   power of the signal inside it: a sine inside a band reads its own `A^2`.
//! * **Channels** ([`ChannelMode`]): left, right, mid `(L+R)/2`, side `(L-R)/2`, or the sum, the
//!   mean of the left and right power spectra.
//! * **Averaging** is a one-pole average of the bin power with a time constant in ms; the time
//!   that passed is the number of frames since the previous update, so the result does not depend
//!   on how often the display redraws.
//! * **Trace.** [`Settings::bands`] off: `columns` columns spaced evenly in log frequency from
//!   [`DISPLAY_LO_HZ`] to [`DISPLAY_HI_HZ`]; a column narrower than a bin interpolates between
//!   the two nearest bins (in dB), a column wider than a bin shows its loudest bin, so a narrow
//!   peak never disappears at the top of the axis. Bands on: the 31 one-third-octave bands from
//!   20 Hz to 20 kHz ([`BAND_COUNT`], [`band_centres`]), each the sum of the bin power inside its
//!   edges (a bin that straddles an edge counts in proportion). A band narrower than a bin (the
//!   lowest ones at 1024 points) reads the power density times its width, so it under-reads a
//!   pure tone; use a larger size for the bass.
//! * **Tilt** adds `tilt * log2(f / 1 kHz)` dB to the trace and the peaks (4.5 dB/octave flattens
//!   typical music), never to the peak readout.
//! * **Peak hold.** Each column keeps its highest value for [`Settings::hold_ms`] and then falls
//!   by [`Settings::decay_db_per_s`] until it meets the trace; a hold of 0 turns it off (the
//!   peaks equal the trace).
//! * **Peak readout** ([`Spectrum::peak_hz`], [`Spectrum::peak_db`]): the loudest bin between 20
//!   Hz and 20 kHz of the averaged spectrum, refined by a parabola through the dB values of its
//!   neighbours, so the frequency is good to a small fraction of a bin and the level to the
//!   scalloping of the window.

use crate::det_trig::cos_cycles;
use crate::stretch::fft::Fft;
use crate::{exp2_det_f64, log2_det};

/// Lowest displayed frequency in Hz.
pub const DISPLAY_LO_HZ: f32 = 20.0;
/// Highest displayed frequency in Hz.
pub const DISPLAY_HI_HZ: f32 = 20_000.0;
/// Smallest analysis size.
pub const MIN_SIZE: usize = 256;
/// Largest analysis size (the scope tap holds twice as many frames).
pub const MAX_SIZE: usize = 8192;
/// Number of one-third-octave bands from 20 Hz to 20 kHz.
pub const BAND_COUNT: usize = 31;
/// A power below this reads as silence (dB).
pub const FLOOR_DB: f32 = -200.0;

/// The window applied to the frames before the transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowKind {
    /// Periodic Hann: narrow main lobe, side lobes fall quickly. The default.
    #[default]
    Hann,
    /// Four-term Blackman-Harris (minimum side lobe): wide main lobe, over 90 dB of range.
    BlackmanHarris,
}

/// What the analyser listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMode {
    /// The left channel.
    Left,
    /// The right channel.
    Right,
    /// `(L + R) / 2`.
    Mid,
    /// `(L - R) / 2`.
    Side,
    /// The mean of the left and right power spectra. The default.
    #[default]
    Sum,
}

/// The coefficient of a periodic window at index `i` of `n`.
fn window_at(kind: WindowKind, i: usize, n: usize) -> f64 {
    let x = i as f64 / n as f64;
    match kind {
        WindowKind::Hann => 0.5 - 0.5 * cos_cycles(x),
        WindowKind::BlackmanHarris => {
            0.358_75 - 0.488_29 * cos_cycles(x) + 0.141_28 * cos_cycles(2.0 * x)
                - 0.011_68 * cos_cycles(3.0 * x)
        }
    }
}

/// `10 log10(p)`, floored at [`FLOOR_DB`] (3.0103 dB per octave of power).
fn db_of_power(p: f32) -> f32 {
    if p <= 0.0 {
        FLOOR_DB
    } else {
        (3.010_3 * log2_det(p)).max(FLOOR_DB)
    }
}

/// A windowed FFT that turns the newest `size` frames into power per bin (module docs).
#[derive(Debug, Clone)]
pub struct Analyser {
    size: usize,
    kind: WindowKind,
    fft: Fft,
    window: Vec<f64>,
    sum_w: f64,
    enbw: f64,
    re: Vec<f64>,
    im: Vec<f64>,
    /// Power per bin of the last analysis.
    power: Vec<f32>,
    /// The second channel of the sum.
    other: Vec<f32>,
}

impl Analyser {
    /// An analyser of `size` points (a power of two within [`MIN_SIZE`]..=[`MAX_SIZE`]; other
    /// values are rounded up and clamped).
    pub fn new(size: usize, kind: WindowKind) -> Self {
        let size = size.next_power_of_two().clamp(MIN_SIZE, MAX_SIZE);
        let window: Vec<f64> = (0..size).map(|i| window_at(kind, i, size)).collect();
        let sum_w: f64 = window.iter().sum();
        let sum_w2: f64 = window.iter().map(|w| w * w).sum();
        let bins = size / 2 + 1;
        Self {
            size,
            kind,
            fft: Fft::new(size),
            window,
            sum_w,
            enbw: size as f64 * sum_w2 / (sum_w * sum_w),
            re: vec![0.0; size],
            im: vec![0.0; size],
            power: vec![0.0; bins],
            other: vec![0.0; bins],
        }
    }

    /// The transform size.
    pub fn size(&self) -> usize {
        self.size
    }

    /// The window.
    pub fn window(&self) -> WindowKind {
        self.kind
    }

    /// Bins per analysis: `size / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }

    /// The equivalent noise bandwidth of the window in bins (1.5 for Hann, about 2.0 for
    /// Blackman-Harris).
    pub fn enbw(&self) -> f32 {
        self.enbw as f32
    }

    /// Rebuilds the analyser for another size or window. Returns whether anything changed.
    pub fn configure(&mut self, size: usize, kind: WindowKind) -> bool {
        let size = size.next_power_of_two().clamp(MIN_SIZE, MAX_SIZE);
        if size == self.size && kind == self.kind {
            return false;
        }
        *self = Self::new(size, kind);
        true
    }

    /// One real channel through window and FFT into `out` (power per bin).
    fn channel_power(
        &mut self,
        frames: usize,
        mut sample: impl FnMut(usize) -> f32,
        out: PowerOut,
    ) {
        let pad = self.size - frames;
        for i in 0..self.size {
            let x = if i < pad { 0.0 } else { sample(i - pad) };
            let x = if x.is_finite() { f64::from(x) } else { 0.0 };
            self.re[i] = x * self.window[i];
            self.im[i] = 0.0;
        }
        self.fft.run(&mut self.re, &mut self.im, false);
        let scale = 2.0 / self.sum_w;
        let last = self.size / 2;
        let power = match out {
            PowerOut::Main => &mut self.power,
            PowerOut::Other => &mut self.other,
        };
        for (k, p) in power.iter_mut().enumerate() {
            let m = self.re[k].hypot(self.im[k]) * scale;
            // DC and Nyquist have no mirror image to add.
            let m = if k == 0 || k == last { m * 0.5 } else { m };
            *p = (m * m) as f32;
        }
    }

    /// Analyses the newest frames (`left` and `right`, oldest first, equal length). The last
    /// `size` are used; fewer are zero-padded in front. Returns the power per bin
    /// ([`Self::bins`] values, bin `k` at `k * sample_rate / size` Hz).
    pub fn analyse(&mut self, left: &[f32], right: &[f32], mode: ChannelMode) -> &[f32] {
        let frames = left.len().min(right.len()).min(self.size);
        let (l, r) = (&left[left.len() - frames..], &right[right.len() - frames..]);
        match mode {
            ChannelMode::Left => self.channel_power(frames, |i| l[i], PowerOut::Main),
            ChannelMode::Right => self.channel_power(frames, |i| r[i], PowerOut::Main),
            ChannelMode::Mid => {
                self.channel_power(frames, |i| 0.5 * (l[i] + r[i]), PowerOut::Main);
            }
            ChannelMode::Side => {
                self.channel_power(frames, |i| 0.5 * (l[i] - r[i]), PowerOut::Main);
            }
            ChannelMode::Sum => {
                self.channel_power(frames, |i| l[i], PowerOut::Main);
                self.channel_power(frames, |i| r[i], PowerOut::Other);
                for (a, b) in self.power.iter_mut().zip(&self.other) {
                    *a = 0.5 * (*a + *b);
                }
            }
        }
        &self.power
    }
}

#[derive(Clone, Copy)]
enum PowerOut {
    Main,
    Other,
}

/// The centre frequencies of the one-third-octave bands, exact: `1 kHz * 10^((n - 30) / 10)`
/// for `n` = 13 (19.95 Hz) to 43 (19.95 kHz).
pub fn band_centres() -> [f32; BAND_COUNT] {
    std::array::from_fn(|i| band_centre(i) as f32)
}

fn band_centre(i: usize) -> f64 {
    1000.0 * exp2_det_f64((i as f64 - 17.0) * 0.1 * std::f64::consts::LOG2_10)
}

/// The nominal labels of the bands (Hz), as printed on an analyser.
pub const BAND_LABELS: [f32; BAND_COUNT] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10_000.0, 12_500.0, 16_000.0, 20_000.0,
];

/// The log-frequency of column `c` of `columns`, edges included (`c` in `0..=columns`): the
/// frequency in Hz at `c / columns` of the way from [`DISPLAY_LO_HZ`] to [`DISPLAY_HI_HZ`].
pub fn column_edge_hz(c: f64, columns: usize) -> f64 {
    let t = c / columns.max(1) as f64;
    f64::from(DISPLAY_LO_HZ) * exp2_det_f64(t * f64::from(log2_det(DISPLAY_HI_HZ / DISPLAY_LO_HZ)))
}

/// What to analyse and how to show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The transform size (a power of two, [`MIN_SIZE`]..=[`MAX_SIZE`]).
    pub size: usize,
    /// The window.
    pub window: WindowKind,
    /// The channel (or combination) to analyse.
    pub channel: ChannelMode,
    /// The one-third-octave bands instead of the fine trace.
    pub bands: bool,
    /// Averaging time constant in ms (0 = none).
    pub averaging_ms: f32,
    /// How long a peak is held in ms (0 = no peak hold).
    pub hold_ms: f32,
    /// How fast a released peak falls, dB per second.
    pub decay_db_per_s: f32,
    /// Slope compensation in dB per octave about 1 kHz.
    pub tilt_db_per_oct: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: 4096,
            window: WindowKind::Hann,
            channel: ChannelMode::Sum,
            bands: false,
            averaging_ms: 150.0,
            hold_ms: 1000.0,
            decay_db_per_s: 15.0,
            tilt_db_per_oct: 0.0,
        }
    }
}

/// The display state of one analyser: averaged bins, the trace, the peaks and the readout.
#[derive(Debug, Clone)]
pub struct Spectrum {
    analyser: Analyser,
    sample_rate: f32,
    columns: usize,
    bands: bool,
    channel: ChannelMode,
    /// Averaged power per bin; `None` before the first update.
    avg: Option<Vec<f32>>,
    trace: Vec<f32>,
    peaks: Vec<f32>,
    /// Seconds since each peak was set or last raised.
    age: Vec<f32>,
    peak_hz: f32,
    peak_db: f32,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    /// An empty display (the first [`Self::update`] fills it).
    pub fn new() -> Self {
        let s = Settings::default();
        Self {
            analyser: Analyser::new(s.size, s.window),
            sample_rate: 48_000.0,
            columns: 0,
            bands: false,
            channel: s.channel,
            avg: None,
            trace: Vec::new(),
            peaks: Vec::new(),
            age: Vec::new(),
            peak_hz: 0.0,
            peak_db: FLOOR_DB,
        }
    }

    /// Forgets the average, the trace and the peaks.
    pub fn reset(&mut self) {
        self.avg = None;
        self.trace.clear();
        self.peaks.clear();
        self.age.clear();
        self.peak_hz = 0.0;
        self.peak_db = FLOOR_DB;
    }

    /// The trace in dB, one value per column (or per band), [`FLOOR_DB`] for silence.
    pub fn trace(&self) -> &[f32] {
        &self.trace
    }

    /// The held peaks in dB, same layout as the trace (equal to it without peak hold).
    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// Frequency of the loudest bin in Hz (0 for silence).
    pub fn peak_hz(&self) -> f32 {
        self.peak_hz
    }

    /// Level of the loudest bin in dB ([`FLOOR_DB`] for silence).
    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }

    /// The dB of the averaged spectrum at `hz`, interpolated between bins (no tilt).
    fn bin_db_at(avg: &[f32], hz: f64, df: f64) -> f32 {
        let pos = (hz / df).clamp(0.0, (avg.len() - 1) as f64);
        let k = pos.floor() as usize;
        let t = (pos - k as f64) as f32;
        let a = db_of_power(avg[k]);
        let b = db_of_power(avg[(k + 1).min(avg.len() - 1)]);
        a + (b - a) * t
    }

    /// Feeds the newest frames (`left` and `right`, oldest first: the tap's read) and the number
    /// of frames since the previous update, then rebuilds the trace for `columns` columns (the
    /// bands mode always has [`BAND_COUNT`]). A change of size, window, channel, mode, number of
    /// columns or sample rate starts over.
    pub fn update(
        &mut self,
        left: &[f32],
        right: &[f32],
        elapsed_frames: u64,
        sample_rate: f32,
        s: &Settings,
        columns: usize,
    ) {
        let columns = if s.bands { BAND_COUNT } else { columns.max(2) };
        let changed = self.analyser.configure(s.size, s.window)
            || columns != self.columns
            || s.bands != self.bands
            || s.channel != self.channel
            || (sample_rate - self.sample_rate).abs() > 0.5;
        if changed {
            self.reset();
        }
        self.columns = columns;
        self.bands = s.bands;
        self.channel = s.channel;
        self.sample_rate = sample_rate;
        let dt = (elapsed_frames as f64 / f64::from(sample_rate)) as f32;

        let power = self.analyser.analyse(left, right, s.channel);
        match &mut self.avg {
            Some(avg) => {
                let tau = f64::from(s.averaging_ms) * 0.001;
                let alpha = if tau <= 0.0 {
                    1.0
                } else {
                    1.0 - exp2_det_f64(-f64::from(dt) / tau * std::f64::consts::LOG2_E)
                } as f32;
                for (a, p) in avg.iter_mut().zip(power) {
                    *a += alpha * (*p - *a);
                }
            }
            None => self.avg = Some(power.to_vec()),
        }
        let Some(avg) = self.avg.as_deref() else {
            return;
        };
        let df = f64::from(sample_rate) / self.analyser.size() as f64;

        // Readout from the loudest bin of 20 Hz to 20 kHz.
        let lo = ((f64::from(DISPLAY_LO_HZ) / df).ceil() as usize).clamp(1, avg.len() - 2);
        let hi = ((f64::from(DISPLAY_HI_HZ) / df).floor() as usize).clamp(lo, avg.len() - 2);
        let mut best = lo;
        for k in lo..=hi {
            if avg[k] > avg[best] {
                best = k;
            }
        }
        if avg[best] <= 1e-20 {
            self.peak_hz = 0.0;
            self.peak_db = FLOOR_DB;
        } else {
            let (a, b, c) = (
                db_of_power(avg[best - 1]),
                db_of_power(avg[best]),
                db_of_power(avg[best + 1]),
            );
            let denom = a - 2.0 * b + c;
            let delta = if denom < -1e-6 {
                (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            self.peak_hz = ((best as f32 + delta) * df as f32).max(0.0);
            self.peak_db = b - 0.25 * (a - c) * delta;
        }

        // The trace.
        self.trace.clear();
        let tilt = |hz: f64| -> f32 { s.tilt_db_per_oct * log2_det((hz / 1000.0) as f32) };
        if s.bands {
            let half = exp2_det_f64(0.05 * std::f64::consts::LOG2_10);
            let enbw = f64::from(self.analyser.enbw());
            for i in 0..BAND_COUNT {
                let fc = band_centre(i);
                let (f_lo, f_hi) = (fc / half, fc * half);
                let mut sum = 0.0f64;
                let k0 = (f_lo / df - 0.5).floor().max(0.0) as usize;
                let k1 = ((f_hi / df + 0.5).ceil() as usize).min(avg.len() - 1);
                for (k, p) in avg.iter().enumerate().take(k1 + 1).skip(k0) {
                    let (b_lo, b_hi) = ((k as f64 - 0.5) * df, (k as f64 + 0.5) * df);
                    let overlap = (f_hi.min(b_hi) - f_lo.max(b_lo)).max(0.0) / df;
                    sum += overlap * f64::from(*p);
                }
                self.trace.push(db_of_power((sum / enbw) as f32) + tilt(fc));
            }
        } else {
            for c in 0..columns {
                let (f0, f1) = (
                    column_edge_hz(c as f64, columns),
                    column_edge_hz(c as f64 + 1.0, columns),
                );
                let centre = (f0 * f1).sqrt();
                let mut db = Self::bin_db_at(avg, centre, df);
                // A column that spans bins shows the loudest of them.
                let k0 = (f0 / df).ceil() as usize;
                let k1 = ((f1 / df).floor() as usize).min(avg.len() - 1);
                for p in avg.iter().take(k1 + 1).skip(k0.max(1)) {
                    db = db.max(db_of_power(*p));
                }
                self.trace.push(db + tilt(centre));
            }
        }

        // The peaks.
        if self.peaks.len() != self.trace.len() {
            self.peaks = self.trace.clone();
            self.age = vec![0.0; self.trace.len()];
        }
        let hold = s.hold_ms * 0.001;
        for ((peak, age), now) in self.peaks.iter_mut().zip(&mut self.age).zip(&self.trace) {
            if hold <= 0.0 || *now >= *peak {
                *peak = *now;
                *age = 0.0;
            } else {
                *age += dt;
                if *age > hold {
                    // Falls from the end of the hold, not from the first update after it.
                    let fall_time = (*age - hold).min(dt);
                    *peak = (*peak - s.decay_db_per_s.max(0.0) * fall_time).max(*now);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
