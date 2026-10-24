//! The data and the pure maths of the two analyser panels (`atomik.spectrum` and
//! `atomik.phase-meter`): what the app feeds the view, where a frequency or a level lands in a
//! plot, how a pair of samples becomes a point of the goniometer, and the texts of the readouts.
//! Nothing here touches a canvas, a clock or the document (`tests/analyser_maths.rs`).
//!
//! The analysis itself (FFT, averaging, peak hold, decimation of the scope frames) is done by
//! the app from the device's scope tap and handed over as an [`AnalyserData`]; the numbers the
//! phase meter publishes arrive as ordinary meter values (`crate::fx::update_display`).

use vello::kurbo::Rect;

/// Lowest frequency of the spectrum plot, Hz.
pub const PLOT_LO_HZ: f64 = 20.0;
/// Highest frequency of the spectrum plot, Hz.
pub const PLOT_HI_HZ: f64 = 20_000.0;
/// Columns of the fine spectrum trace the app asks of the analysis.
pub const TRACE_COLUMNS: usize = 240;
/// Frames the app reads from the scope tap for the goniometer (it keeps every other one).
pub const SCOPE_READ_FRAMES: usize = 2048;

/// What the app hands the view about one analyser device: the spectrum trace (dB per column or
/// per band), its held peaks, the loudest bin, and the decimated recent frames of the scope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyserData {
    /// The spectrum trace in dB, one value per column (fine) or per band (1/3 octave).
    pub trace: Vec<f32>,
    /// The held peaks in dB, same layout as the trace (empty without peak hold).
    pub peaks: Vec<f32>,
    /// The trace is the 31 one-third-octave bands.
    pub bands: bool,
    /// Frequency of the loudest bin in Hz (0 for silence).
    pub peak_hz: f32,
    /// Level of the loudest bin in dB.
    pub peak_db: f32,
    /// Left channel of the scope frames, oldest first.
    pub scope_l: Vec<f32>,
    /// Right channel of the scope frames, oldest first.
    pub scope_r: Vec<f32>,
}

/// How the scope draws a pair of samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeMode {
    /// Mid up, side across: a mono signal is a vertical line, the left channel alone a line at
    /// 45 degrees up and to the left, the right channel alone one up and to the right.
    Goniometer,
    /// Left across, right up.
    Lissajous,
}

impl ScopeMode {
    /// The mode of the `scope` choice param (0 goniometer, 1 Lissajous).
    pub fn of_choice(choice: f32) -> Self {
        if choice >= 0.5 {
            ScopeMode::Lissajous
        } else {
            ScopeMode::Goniometer
        }
    }
}

/// The point of the scope for the sample pair `(l, r)`, each coordinate `-1..=1` for a signal
/// within full scale at `gain_db` of 0 (x to the right, y up); larger values are clamped to the
/// frame. The goniometer rotates by 45 degrees and keeps the length: `x = (r - l) / sqrt 2`,
/// `y = (l + r) / sqrt 2`, so full-scale mono reaches 1.41 and is clamped to the frame (set
/// `gain_db` to -3 for a full-scale mono signal to touch the top).
pub fn scope_point(l: f32, r: f32, mode: ScopeMode, gain_db: f32) -> (f64, f64) {
    let gain = 10f64.powf(f64::from(gain_db) / 20.0);
    let (l, r) = (f64::from(l), f64::from(r));
    let (x, y) = match mode {
        ScopeMode::Goniometer => (
            (r - l) * std::f64::consts::FRAC_1_SQRT_2,
            (l + r) * std::f64::consts::FRAC_1_SQRT_2,
        ),
        ScopeMode::Lissajous => (l, r),
    };
    let (x, y) = (x * gain, y * gain);
    if x.is_finite() && y.is_finite() {
        (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0))
    } else {
        (0.0, 0.0)
    }
}

/// The position `0..=1` of `hz` on the log axis of the spectrum plot.
pub fn freq_pos(hz: f64) -> f64 {
    ((hz.max(PLOT_LO_HZ) / PLOT_LO_HZ).ln() / (PLOT_HI_HZ / PLOT_LO_HZ).ln()).clamp(0.0, 1.0)
}

/// Plot x of a frequency.
pub fn freq_x(plot: Rect, hz: f64) -> f64 {
    plot.x0 + freq_pos(hz) * plot.width()
}

/// Plot y of a level in dB for a plot that shows `range_db` below 0 dB: 0 dB at the top.
pub fn level_y(plot: Rect, db: f32, range_db: f32) -> f64 {
    let t = f64::from((-db / range_db.max(1.0)).clamp(0.0, 1.0));
    plot.y0 + t * plot.height()
}

/// The edges `(low, high)` in Hz of one-third-octave band `i` (0..31, 19.95 Hz to 19.95 kHz at
/// the centres): `1 kHz * 10^((i - 17) / 10)` divided and multiplied by `10^0.05`. The same bands
/// as `atomik_dsp::spectrum::band_centres` (a test pins them together).
pub fn band_edges(i: usize) -> (f64, f64) {
    let centre = 1000.0 * 10f64.powf((i as f64 - 17.0) / 10.0);
    let half = 10f64.powf(0.05);
    (centre / half, centre * half)
}

/// Spacing of the horizontal grid lines for a range: 5, 10 or 20 dB.
pub fn grid_step_db(range_db: f32) -> f32 {
    if range_db <= 40.0 {
        5.0
    } else if range_db <= 80.0 {
        10.0
    } else {
        20.0
    }
}

/// A frequency as a readout: `62.5 Hz`, `440 Hz`, `1.00 kHz`, `12.3 kHz`.
pub fn hz_text(hz: f32) -> String {
    if hz >= 10_000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else if hz >= 100.0 {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.1} Hz")
    }
}

/// The spectrum's readout of the loudest bin: `PEAK 1.00 kHz  -20.1 dB`, or `PEAK --`.
pub fn peak_text(hz: f32, db: f32) -> String {
    if hz <= 0.0 || db <= -150.0 {
        "PEAK --".to_string()
    } else {
        format!("PEAK {}  {db:.1} dB", hz_text(hz))
    }
}

/// A level readout in dB or LUFS with one decimal; `--` at or below the floor.
pub fn level_text(db: f32) -> String {
    if db <= -119.9 {
        "--".to_string()
    } else {
        format!("{db:.1}")
    }
}

/// The correlation readout: `+1.00`, `-0.37`.
pub fn correlation_text(c: f32) -> String {
    format!("{c:+.2}")
}

/// The balance readout: `L 40%`, `C`, `R 12%` (percent of the way to the side).
pub fn balance_text(b: f32) -> String {
    let pct = (b.abs() * 100.0).round();
    if pct < 1.0 {
        "C".to_string()
    } else if b < 0.0 {
        format!("L {pct:.0}%")
    } else {
        format!("R {pct:.0}%")
    }
}

/// The width readout in percent: `0%` mono, `50%` unrelated channels, `100%` opposite polarity.
pub fn width_text(w: f32) -> String {
    format!("{:.0}%", (w * 100.0).round())
}
