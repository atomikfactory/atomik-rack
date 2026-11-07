//! The decision zone: seven attributes of a finished mix or of a reference file, and where a mix
//! stands among the references.
//!
//! [`measure`] reads the first two channels of an audio file (a render of the song, or a
//! reference) and returns a [`ZoneMetrics`]. Every number is a pure function of the samples, so
//! the same audio reports the same numbers on every platform (the filters and the loudness meter
//! run in `f64`; a difference between platforms stays far under the resolution shown).
//!
//! | attribute | meaning | level independent |
//! |---|---|---|
//! | loudness | gated integrated loudness, LUFS | no |
//! | loudest | the loudest short-term (3 s) loudness, LUFS | no |
//! | punch | sample peak minus integrated loudness (peak-to-loudness ratio), dB | yes |
//! | brightness | slope of the octave-band energy from 125 Hz to 8 kHz, dB per octave (0 reads as a pink-noise balance, a darker mix is lower) | yes |
//! | low end | share of the energy below 120 Hz (a 4th-order low-pass, 6 dB down at 120 Hz), percent | yes |
//! | width | energy of the side (L-R)/2 against the mid (L+R)/2, dB | yes |
//! | correlation | correlation of left and right, -1 to +1 | yes |
//!
//! The level-independent ones read the same when a file is scaled, so a mix can be set against a
//! reference without matching their levels first ([`Attribute::normalised`]). The two loudness
//! rows are the ones the level match moves.
//!
//! [`zone_row`] sets a mix against a list of references: the range is the lowest and the highest
//! reference reading, and the [`Verdict`] says whether the mix is inside, above or below it, and
//! by how much. An analysis never changes anything: it only measures.

use atomik_audio_file::AudioData;
use atomik_devices::mastering::loudness::LoudnessMeter;

/// The short-term windows a reading needs: 3 s in steps of 100 ms.
const SHORT_WINDOWS: usize = 30;
/// The centres of the octave bands the brightness is read from, in Hz.
const TILT_BANDS: [f64; 7] = [125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0];
/// The low end is the energy below this, in Hz.
const LOW_END_HZ: f64 = 120.0;
/// The side-to-mid energy ratio is held in this range, in dB.
const WIDTH_RANGE_DB: (f64, f64) = (-60.0, 20.0);
/// Readings at or below this are silence, in LUFS.
const ABSOLUTE_GATE: f32 = -70.0;
/// A band is measured only when its centre is under this share of the sample rate.
const BAND_LIMIT: f64 = 0.45;

/// The measurements of one file. A value that does not exist (silence, a file too short for the
/// loudest section) is `None`. See the module docs for the meaning of each.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZoneMetrics {
    /// Gated integrated loudness in LUFS.
    pub integrated_lufs: Option<f32>,
    /// The loudest short-term loudness in LUFS.
    pub loudest_lufs: Option<f32>,
    /// Peak-to-loudness ratio in dB.
    pub punch_db: Option<f32>,
    /// Octave-band energy slope in dB per octave.
    pub tilt_db_per_octave: Option<f32>,
    /// Share of the energy below 120 Hz in percent.
    pub low_end_percent: Option<f32>,
    /// Side-to-mid energy in dB.
    pub width_db: Option<f32>,
    /// Left-right correlation.
    pub correlation: Option<f32>,
}

/// One row of the decision zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute {
    /// Integrated loudness.
    Loudness,
    /// Loudest short-term loudness.
    Loudest,
    /// Peak-to-loudness ratio.
    Punch,
    /// Spectral tilt.
    Brightness,
    /// Share of the low end.
    LowEnd,
    /// Side against mid.
    Width,
    /// Left-right correlation.
    Correlation,
}

impl Attribute {
    /// Every attribute, in the order the panel lists them.
    pub const ALL: [Attribute; 7] = [
        Attribute::Loudness,
        Attribute::Loudest,
        Attribute::Punch,
        Attribute::Brightness,
        Attribute::LowEnd,
        Attribute::Width,
        Attribute::Correlation,
    ];

    /// The name shown.
    pub fn label(self) -> &'static str {
        match self {
            Attribute::Loudness => "Loudness",
            Attribute::Loudest => "Loudest part",
            Attribute::Punch => "Punch",
            Attribute::Brightness => "Brightness",
            Attribute::LowEnd => "Low end",
            Attribute::Width => "Width",
            Attribute::Correlation => "Correlation",
        }
    }

    /// The unit written after a value.
    pub fn unit(self) -> &'static str {
        match self {
            Attribute::Loudness | Attribute::Loudest => "LUFS",
            Attribute::Punch | Attribute::Width => "dB",
            Attribute::Brightness => "dB/oct",
            Attribute::LowEnd => "%",
            Attribute::Correlation => "",
        }
    }

    /// True when the reading does not change with the level of the file (see the module docs).
    pub fn normalised(self) -> bool {
        !matches!(self, Attribute::Loudness | Attribute::Loudest)
    }

    /// How far outside the range still reads as inside: the resolution of the reading.
    pub fn tolerance(self) -> f32 {
        match self {
            Attribute::Loudness | Attribute::Loudest => 0.1,
            Attribute::Punch | Attribute::Brightness => 0.1,
            Attribute::LowEnd | Attribute::Width => 0.5,
            Attribute::Correlation => 0.02,
        }
    }

    /// The reading of `metrics`.
    pub fn of(self, metrics: &ZoneMetrics) -> Option<f32> {
        match self {
            Attribute::Loudness => metrics.integrated_lufs,
            Attribute::Loudest => metrics.loudest_lufs,
            Attribute::Punch => metrics.punch_db,
            Attribute::Brightness => metrics.tilt_db_per_octave,
            Attribute::LowEnd => metrics.low_end_percent,
            Attribute::Width => metrics.width_db,
            Attribute::Correlation => metrics.correlation,
        }
    }
}

/// Where a mix stands against the range of the references.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Within the range (or within the resolution of its edge).
    Inside,
    /// Above the highest reference by this much, in the attribute's unit.
    Above(f32),
    /// Below the lowest reference by this much, in the attribute's unit.
    Below(f32),
}

impl Verdict {
    /// The word for the status text: `inside`, `above 1.2 dB`, `below 0.4`.
    pub fn text(self, attribute: Attribute) -> String {
        let (word, distance) = match self {
            Verdict::Inside => return "inside".to_owned(),
            Verdict::Above(d) => ("above", d),
            Verdict::Below(d) => ("below", d),
        };
        let digits = if attribute == Attribute::Correlation {
            2
        } else {
            1
        };
        match attribute.unit() {
            "" => format!("{word} by {distance:.digits$}"),
            "LUFS" => format!("{word} by {distance:.digits$} LU"),
            unit => format!("{word} by {distance:.digits$} {unit}"),
        }
    }
}

/// One attribute across the references and the mix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneRow {
    /// The attribute.
    pub attribute: Attribute,
    /// The lowest and highest reading over the references (`None` with no reading).
    pub range: Option<(f32, f32)>,
    /// The mix's reading.
    pub mix: Option<f32>,
    /// The verdict (`None` when the mix or every reference has no reading).
    pub verdict: Option<Verdict>,
}

/// Sets the mix against the references for one attribute.
pub fn zone_row(attribute: Attribute, references: &[ZoneMetrics], mix: &ZoneMetrics) -> ZoneRow {
    let readings = references.iter().filter_map(|r| attribute.of(r));
    let range = readings.fold(None, |range: Option<(f32, f32)>, x| {
        Some(range.map_or((x, x), |(lo, hi)| (lo.min(x), hi.max(x))))
    });
    let mix_reading = attribute.of(mix);
    let verdict = range.zip(mix_reading).map(|((lo, hi), m)| {
        let tolerance = attribute.tolerance();
        if m < lo - tolerance {
            Verdict::Below(lo - m)
        } else if m > hi + tolerance {
            Verdict::Above(m - hi)
        } else {
            Verdict::Inside
        }
    });
    ZoneRow {
        attribute,
        range,
        mix: mix_reading,
        verdict,
    }
}

/// Every row of the decision zone.
pub fn zone(references: &[ZoneMetrics], mix: &ZoneMetrics) -> Vec<ZoneRow> {
    Attribute::ALL
        .iter()
        .map(|a| zone_row(*a, references, mix))
        .collect()
}

// ------------------------------------------------------------------------------------ filters

/// A second-order section in `f64` (transposed direct form II).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b: [b[0] / a0, b[1] / a0, b[2] / a0],
            a: [a1 / a0, a2 / a0],
            z: [0.0; 2],
        }
    }

    /// A band-pass of unity gain at `hz` and an octave wide.
    fn band_pass(hz: f64, rate: f64) -> Self {
        let w = std::f64::consts::TAU * hz / rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::SQRT_2);
        Self::new(
            [alpha, 0.0, -alpha],
            1.0 + alpha,
            -2.0 * w.cos(),
            1.0 - alpha,
        )
    }

    /// A Butterworth low-pass at `hz` (two of them in a row make a Linkwitz-Riley 4th order).
    fn low_pass(hz: f64, rate: f64) -> Self {
        let w = std::f64::consts::TAU * hz / rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w.cos();
        Self::new(
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The mean square of `mid` through two band-pass sections at `hz`.
fn band_energy(mid: &[f64], hz: f64, rate: f64) -> f64 {
    let mut a = Biquad::band_pass(hz, rate);
    let mut b = a;
    let mut sum = 0.0;
    for &x in mid {
        let y = b.tick(a.tick(x));
        sum += y * y;
    }
    sum / mid.len().max(1) as f64
}

/// The slope in dB per octave of a least-squares line through `(index, dB)` points.
fn slope(points: &[(f64, f64)]) -> Option<f64> {
    let n = points.len() as f64;
    if points.len() < 4 {
        return None;
    }
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let num: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    (den > 0.0).then(|| num / den)
}

// --------------------------------------------------------------------------------- measuring

/// Measures `audio` (see the module docs): the first two channels, a single channel as a mono
/// file.
pub fn measure(audio: &AudioData) -> ZoneMetrics {
    measure_channels(&audio.channels, audio.sample_rate)
}

/// [`measure`] for planes of samples (the rendered mix).
pub fn measure_channels(channels: &[Vec<f32>], sample_rate: u32) -> ZoneMetrics {
    let Some(left) = channels.first() else {
        return ZoneMetrics::default();
    };
    let right = channels.get(1).unwrap_or(left);
    let frames = left.len().min(right.len());
    let (left, right) = (&left[..frames], &right[..frames]);
    let rate = f64::from(sample_rate.max(1));
    let finite = |x: f32| if x.is_finite() { f64::from(x) } else { 0.0 };

    let peak = left
        .iter()
        .chain(right)
        .fold(0.0f64, |m, &x| m.max(finite(x).abs()));
    if frames == 0 || peak == 0.0 {
        return ZoneMetrics::default();
    }

    // Loudness: integrated and the loudest short-term window (every 100 ms, from the first full
    // 3 s on; a file shorter than that has only its integrated reading).
    let mut meter = LoudnessMeter::new(rate);
    let step = ((rate * 0.1).round() as usize).max(1);
    let (mut windows, mut loudest) = (0usize, None::<f32>);
    for start in (0..frames).step_by(step) {
        let end = (start + step).min(frames);
        meter.process(&left[start..end], &right[start..end]);
        if end - start == step {
            windows += 1;
            if windows >= SHORT_WINDOWS {
                let st = meter.short_term();
                if st > ABSOLUTE_GATE && loudest.is_none_or(|b| st > b) {
                    loudest = Some(st);
                }
            }
        }
    }
    let integrated = meter.integrated();
    let integrated = (integrated > ABSOLUTE_GATE).then_some(integrated);
    let loudest = loudest.or(integrated);
    let punch = integrated.map(|l| (20.0 * peak.log10()) as f32 - l);

    // Energies of the two channels, the mid and the side.
    let (mut ll, mut rr, mut lr, mut mm, mut ss) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut mid = Vec::with_capacity(frames);
    for (&l, &r) in left.iter().zip(right) {
        let (l, r) = (finite(l), finite(r));
        ll += l * l;
        rr += r * r;
        lr += l * r;
        let (m, s) = ((l + r) / 2.0, (l - r) / 2.0);
        mm += m * m;
        ss += s * s;
        mid.push(m);
    }
    let correlation = (ll > 0.0 && rr > 0.0).then(|| (lr / (ll * rr).sqrt()).clamp(-1.0, 1.0));
    let width = (mm > 0.0 || ss > 0.0).then(|| {
        let ratio = 10.0 * (ss.max(1e-30) / mm.max(1e-30)).log10();
        ratio.clamp(WIDTH_RANGE_DB.0, WIDTH_RANGE_DB.1)
    });

    // Low end: the energy of both channels through a Linkwitz-Riley low-pass.
    let mut low = 0.0;
    for plane in [left, right] {
        let mut a = Biquad::low_pass(LOW_END_HZ, rate);
        let mut b = a;
        for &x in plane {
            let y = b.tick(a.tick(finite(x)));
            low += y * y;
        }
    }
    let total = ll + rr;
    let low_end = (total > 0.0).then(|| (100.0 * low / total).clamp(0.0, 100.0));

    // Brightness: the slope of the octave-band energies of the mid.
    let points: Vec<(f64, f64)> = TILT_BANDS
        .iter()
        .enumerate()
        .filter(|(_, hz)| **hz < rate * BAND_LIMIT)
        .filter_map(|(i, hz)| {
            let e = band_energy(&mid, *hz, rate);
            (e > 1e-20).then(|| (i as f64, 10.0 * e.log10()))
        })
        .collect();
    let tilt = slope(&points);

    ZoneMetrics {
        integrated_lufs: integrated,
        loudest_lufs: loudest,
        punch_db: punch,
        tilt_db_per_octave: tilt.map(|t| t as f32),
        low_end_percent: low_end.map(|x| x as f32),
        width_db: width.map(|x| x as f32),
        correlation: correlation.map(|x| x as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    fn sine(hz: f64, amp: f64, seconds: f64) -> Vec<f64> {
        let n = (seconds * f64::from(RATE)) as usize;
        (0..n)
            .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / f64::from(RATE)).sin())
            .collect()
    }

    fn sum(parts: &[Vec<f64>]) -> Vec<f64> {
        let n = parts.iter().map(Vec::len).min().unwrap();
        (0..n).map(|i| parts.iter().map(|p| p[i]).sum()).collect()
    }

    fn audio(l: &[f64], r: &[f64]) -> AudioData {
        let plane = |x: &[f64]| x.iter().map(|&v| v as f32).collect::<Vec<f32>>();
        AudioData::new(RATE, vec![plane(l), plane(r)]).unwrap()
    }

    /// A signal whose octave-band energy falls by `slope` dB per octave (one sine at the centre
    /// of each band), at a sixth of full scale in all.
    fn tilted(slope: f64) -> Vec<f64> {
        let parts: Vec<Vec<f64>> = TILT_BANDS
            .iter()
            .enumerate()
            .map(|(i, hz)| sine(*hz, 0.02 * 10f64.powf(slope * i as f64 / 20.0), 6.0))
            .collect();
        sum(&parts)
    }

    #[test]
    fn brightness_reads_the_slope_of_the_octave_bands() {
        let mut readings = Vec::new();
        for slope in [-4.0, -2.0, 0.0, 2.0] {
            let t = tilted(slope);
            let m = measure(&audio(&t, &t));
            let got = f64::from(m.tilt_db_per_octave.expect("audible"));
            assert!((got - slope).abs() < 0.5, "{slope}: read {got}");
            readings.push(got);
        }
        assert!(readings.windows(2).all(|w| w[0] < w[1]), "{readings:?}");
    }

    #[test]
    fn the_low_end_is_the_share_of_the_energy_under_120_hz() {
        let low = sine(60.0, 0.3, 4.0);
        let high = sine(1_000.0, 0.3, 4.0);
        let both = sum(&[low.clone(), high.clone()]);
        let half = measure(&audio(&both, &both)).low_end_percent.unwrap();
        // The low-pass is 6 dB down at 120 Hz, so a 60 Hz tone counts for about 89 percent.
        assert!((half - 44.3).abs() < 1.5, "{half}");
        let none = measure(&audio(&high, &high)).low_end_percent.unwrap();
        assert!(none < 0.5, "{none}");
        let deep = sine(40.0, 0.3, 4.0);
        let all = measure(&audio(&deep, &deep)).low_end_percent.unwrap();
        assert!(all > 95.0, "{all}");
    }

    #[test]
    fn width_and_correlation_read_the_stereo_image() {
        let tone = sine(440.0, 0.3, 3.0);
        let silent = vec![0.0; tone.len()];
        let negated: Vec<f64> = tone.iter().map(|x| -x).collect();
        // Mono: no side, the channels are the same.
        let mono = measure(&audio(&tone, &tone));
        assert!(mono.width_db.unwrap() <= -59.0, "{:?}", mono.width_db);
        assert!((mono.correlation.unwrap() - 1.0).abs() < 1e-4);
        // One channel only: the side and the mid have the same energy.
        let one = measure(&audio(&tone, &silent));
        assert!(one.width_db.unwrap().abs() < 0.01, "{:?}", one.width_db);
        // Out of phase: all side.
        let wide = measure(&audio(&tone, &negated));
        assert!(wide.width_db.unwrap() >= 19.0, "{:?}", wide.width_db);
        assert!((wide.correlation.unwrap() + 1.0).abs() < 1e-4);
        // Different notes in the two channels: nearly uncorrelated, equal side and mid.
        let other = sine(587.0, 0.3, 3.0);
        let apart = measure(&audio(&tone, &other));
        assert!(apart.correlation.unwrap().abs() < 0.05);
        assert!(apart.width_db.unwrap().abs() < 1.0);
    }

    #[test]
    fn punch_is_the_peak_over_the_loudness() {
        // A stereo sine reads its peak level in LUFS (the 3 dB of its crest factor and the 3 dB
        // of the channel sum cancel near 1 kHz): the ratio is near 0 dB.
        let tone = sine(997.0, 0.1, 6.0);
        let m = measure(&audio(&tone, &tone));
        let punch = m.punch_db.unwrap();
        assert!(punch.abs() < 0.3, "{punch}");
        // The same tone with a spike: the ratio grows by the spike's height over the peak.
        let mut spiky = tone.clone();
        spiky[1_000] = 0.9;
        let m2 = measure(&audio(&spiky, &spiky));
        let punch2 = m2.punch_db.unwrap();
        assert!(punch2 > punch + 15.0, "{punch2}");
    }

    #[test]
    fn scaling_a_file_moves_only_the_loudness_rows() {
        let t = tilted(-3.0);
        let quiet: Vec<f64> = t.iter().map(|x| x * 0.5).collect();
        let other = sine(300.0, 0.05, 6.0);
        let l = sum(&[t.clone(), other.clone()]);
        let r = t.clone();
        let lq: Vec<f64> = l.iter().map(|x| x * 0.5).collect();
        let a = measure(&audio(&l, &r));
        let b = measure(&audio(&lq, &quiet));
        for attribute in Attribute::ALL {
            let (x, y) = (attribute.of(&a).unwrap(), attribute.of(&b).unwrap());
            if attribute.normalised() {
                assert!((x - y).abs() < 0.01, "{attribute:?}: {x} {y}");
            } else {
                assert!((x - y - 6.02).abs() < 0.1, "{attribute:?}: {x} {y}");
            }
        }
    }

    #[test]
    fn the_loudest_part_reads_the_loudest_window() {
        let loud = sine(997.0, 0.5, 5.0);
        let medium = sine(997.0, 0.25, 10.0);
        let song: Vec<f64> = loud.iter().chain(&medium).copied().collect();
        let m = measure(&audio(&song, &song));
        let (integrated, loudest) = (m.integrated_lufs.unwrap(), m.loudest_lufs.unwrap());
        assert!(loudest > integrated + 2.5, "{integrated} {loudest}");
        assert!((loudest - -6.0).abs() < 0.5, "{loudest}");
    }

    #[test]
    fn silence_and_missing_audio_have_nothing_to_report() {
        assert_eq!(measure_channels(&[], RATE), ZoneMetrics::default());
        let silent = AudioData::new(RATE, vec![vec![0.0; 10_000]; 2]).unwrap();
        assert_eq!(measure(&silent), ZoneMetrics::default());
        let mono = AudioData::new(
            RATE,
            vec![sine(500.0, 0.2, 2.0).iter().map(|&x| x as f32).collect()],
        )
        .unwrap();
        let m = measure(&mono);
        assert!(m.correlation.unwrap() > 0.999);
    }

    fn metrics(tilt: f32, width: f32) -> ZoneMetrics {
        ZoneMetrics {
            tilt_db_per_octave: Some(tilt),
            width_db: Some(width),
            ..ZoneMetrics::default()
        }
    }

    #[test]
    fn the_range_is_the_lowest_and_highest_reference_and_the_verdict_names_the_distance() {
        let refs = [
            metrics(-3.0, -6.0),
            metrics(-1.0, -2.0),
            metrics(-2.0, -4.0),
        ];
        let row = zone_row(Attribute::Brightness, &refs, &metrics(-2.5, 0.0));
        assert_eq!(row.range, Some((-3.0, -1.0)));
        assert_eq!(row.verdict, Some(Verdict::Inside));
        let dark = zone_row(Attribute::Brightness, &refs, &metrics(-4.5, 0.0));
        assert_eq!(dark.verdict, Some(Verdict::Below(1.5)));
        let wide = zone_row(Attribute::Width, &refs, &metrics(0.0, 1.0));
        assert_eq!(wide.verdict, Some(Verdict::Above(3.0)));
        // The edge counts as inside within the resolution.
        let edge = zone_row(Attribute::Brightness, &refs, &metrics(-0.95, 0.0));
        assert_eq!(edge.verdict, Some(Verdict::Inside));
        assert_eq!(
            Verdict::Below(1.5).text(Attribute::Brightness),
            "below by 1.5 dB/oct"
        );
        assert_eq!(
            Verdict::Above(3.0).text(Attribute::Width),
            "above by 3.0 dB"
        );
        assert_eq!(
            Verdict::Above(2.0).text(Attribute::Loudness),
            "above by 2.0 LU"
        );
        assert_eq!(Verdict::Inside.text(Attribute::Width), "inside");
    }

    #[test]
    fn a_row_with_nothing_to_compare_has_no_verdict() {
        let none = zone_row(Attribute::Punch, &[], &metrics(0.0, 0.0));
        assert_eq!(none.range, None);
        assert_eq!(none.verdict, None);
        let silent_mix = zone_row(
            Attribute::Brightness,
            &[metrics(-2.0, 0.0)],
            &ZoneMetrics::default(),
        );
        assert_eq!(silent_mix.range, Some((-2.0, -2.0)));
        assert_eq!(silent_mix.verdict, None);
    }

    #[test]
    fn synthetic_references_give_the_expected_ranges_and_the_mix_the_right_verdict() {
        // Two references: a darker one (-4 dB/oct) and a brighter one (-1), both narrow.
        let dark = tilted(-4.0);
        let bright = tilted(-1.0);
        // The right channel is quieter: a measurable side that is the same for every signal.
        let stereo = |t: &[f64]| {
            let r: Vec<f64> = t.iter().map(|x| x * 0.6).collect();
            audio(t, &r)
        };
        let refs = [measure(&stereo(&dark)), measure(&stereo(&bright))];
        let tilt_range = zone_row(Attribute::Brightness, &refs, &ZoneMetrics::default())
            .range
            .unwrap();
        assert!(tilt_range.0 < -3.0 && tilt_range.1 > -1.6, "{tilt_range:?}");
        // A mix between them is inside; a duller one is below; a brighter one above.
        let inside = measure(&stereo(&tilted(-2.5)));
        let duller = measure(&stereo(&tilted(-6.0)));
        let brighter = measure(&stereo(&tilted(1.5)));
        let verdict = |m: &ZoneMetrics| zone_row(Attribute::Brightness, &refs, m).verdict.unwrap();
        assert_eq!(verdict(&inside), Verdict::Inside);
        assert!(
            matches!(verdict(&duller), Verdict::Below(d) if d > 1.0),
            "{:?}",
            verdict(&duller)
        );
        assert!(
            matches!(verdict(&brighter), Verdict::Above(d) if d > 1.0),
            "{:?}",
            verdict(&brighter)
        );
        // Width of all of them is the same, so the width row is inside for the mix.
        let width = zone_row(Attribute::Width, &refs, &inside).verdict.unwrap();
        assert_eq!(width, Verdict::Inside);
    }
}
