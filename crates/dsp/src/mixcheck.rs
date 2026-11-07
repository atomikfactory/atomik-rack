//! Mix check: the measurements and the thresholds behind the findings of the mix check panel.
//!
//! It runs on a worker from rendered audio, never on the audio thread. Sums run in `f64` in a
//! fixed order and the dB come from [`log2_det`], so the same audio gives the same numbers on
//! every platform.
//!
//! # Measurements
//!
//! [`measure`] reads one stereo signal (a strip's stem, the master, or the signal that enters a
//! device) in one pass:
//!
//! * **Level.** The RMS of the blocks of [`RMS_BLOCK`] frames that are not silent (above
//!   [`RMS_GATE_DB`]), as the power mean of those blocks: a track that plays one bar in eight is
//!   measured while it plays. The power of a stereo block is `(L^2 + R^2) / 2`.
//! * **Peak envelope.** The highest absolute sample of every window of [`PEAK_WINDOW`] frames, so
//!   the loudest spots of the master can be matched with what every track did there.
//! * **Low end.** Both channels are low-passed at [`LOW_HZ`] (a fourth-order Linkwitz-Riley
//!   filter); the mean square of that signal is kept for the stereo pair and for its mono sum
//!   `(L + R) / 2`. [`Lows::lost_db`] is what a mono playback loses.
//!
//! # Judgements
//!
//! Every threshold is a constant below with its reason; the findings are built from them in one
//! place (the render crate's `mixcheck` module) so tuning a limit means changing one line here.

use crate::Crossover;
use crate::exp2_det_f64;
use crate::masking::{ACTIVE_DB, BANDS, SILENT_DB, band_centre, band_of, db_to_power, power_to_db};

/// The level non-linear devices (compressors, distortion, saturation, clippers, amps, the
/// mastering device) are meant to be fed at, dBFS RMS. A level of this order leaves 18 dB of
/// headroom, which is what such a device's thresholds and drive ranges are voiced for.
pub const REFERENCE_RMS_DB: f32 = -18.0;
/// How far from [`REFERENCE_RMS_DB`] the feed of a non-linear device may be before the check says
/// so, dB. Within 9 dB the device's own input and output controls can compensate; beyond it the
/// device works in a range it was not voiced for.
pub const OFF_REFERENCE_DB: f32 = 9.0;
/// The largest trim the one-click fix applies, dB.
pub const MAX_TRIM_DB: f32 = 24.0;
/// The device types the level check looks at.
pub const NONLINEAR_DEVICES: [&str; 7] = [
    "atomik.compressor",
    "atomik.dynamics",
    "atomik.distortion",
    "atomik.character",
    "atomik.clipper",
    "atomik.guitar-amp",
    "atomik.mastering",
];

/// The range a bass root should sit in, Hz. Under about 40 Hz most speakers and headphones give
/// little, and over about 60 Hz the root no longer leaves the room below it that a kick needs.
pub const BASS_ROOT_HZ: (f32, f32) = (40.0, 60.0);
/// How far outside [`BASS_ROOT_HZ`] a root may be and still pass: one semitone, so the notes E1 to
/// B1 pass and D1 and C2 do not.
pub const BASS_ROOT_SEMITONES: f32 = 1.0;

/// A fundamental this much under its strongest harmonic (the 2nd to the 4th) is weak, dB. The
/// ear takes the pitch and the weight of a bass from the fundamental; 6 dB is the gap at which a
/// small speaker that drops it leaves the harmonics alone playing a different, thinner note.
pub const WEAK_FUNDAMENTAL_DB: f32 = 6.0;
/// The highest harmonic of the fundamental that is compared.
pub const HIGHEST_HARMONIC: u32 = 4;
/// The range a kick's fundamental is searched in, Hz.
pub const KICK_HZ: (f32, f32) = (40.0, 80.0);
/// The range a bass without notes is searched in for its fundamental, Hz.
pub const BASS_SEARCH_HZ: (f32, f32) = (40.0, 100.0);

/// The low-mid region, Hz: where the body of the instruments piles up and the mud builds. The
/// bands of the analysis whose centre lies inside it count.
pub const LOW_MID_HZ: (f32, f32) = (150.0, 500.0);
/// The share of the master's power in the low-mid region above which it is flagged. A balanced mix
/// puts a quarter to a third of its power there; at 40% the region outweighs the low end and the
/// presence together.
pub const LOW_MID_SHARE: f32 = 0.40;
/// The tracks named under a low-mid finding.
pub const LOW_MID_TRACKS: usize = 3;
/// The cut the one-click fix makes, dB, and its width (a Q).
pub const LOW_MID_CUT_DB: f32 = -3.0;
/// The Q of the cut: about two thirds of an octave, wide enough to take the hump and not a note.
pub const LOW_MID_CUT_Q: f32 = 1.5;

/// The corner of the low end the mono check looks at, Hz. Under it the two speakers of a stereo
/// playback add, so what cancels in mono is what a club system's single bass bin loses.
pub const LOW_HZ: f32 = 120.0;
/// A low end that loses this much when summed to mono is flagged, dB. 3 dB is half the power.
pub const MONO_LOSS_DB: f32 = 3.0;
/// A track is named in a mono finding when it loses at least this much itself, dB.
pub const MONO_TRACK_LOSS_DB: f32 = 1.5;
/// The low end must be at least this loud to be judged, dBFS (mean square); under it there is
/// nothing to lose.
pub const MONO_MIN_DB: f32 = -50.0;

/// The master's peaks are reported when the highest is above this, dBFS. Under it there is room
/// and nothing needs a limiter's attention.
pub const PEAK_REPORT_DB: f32 = -6.0;
/// A track is named for a peak when its own peak there is within this many dB of the loudest
/// contributor's.
pub const PEAK_SPREAD_DB: f32 = 9.0;
/// The peak spots of the master that are reported, and the least time between two of them, windows.
pub const PEAK_SPOTS: usize = 3;
/// The least distance between two reported spots: about a second at 48 kHz.
pub const PEAK_APART: usize = 48;
/// The tracks named at a peak spot.
pub const PEAK_TRACKS: usize = 3;

/// Frames per block of the level measurement.
pub const RMS_BLOCK: usize = 4096;
/// Blocks quieter than this do not count in the level, dBFS (mean square).
pub const RMS_GATE_DB: f32 = -50.0;
/// Frames per window of the peak envelope.
pub const PEAK_WINDOW: usize = 1024;

/// A level in dB for a linear amplitude (`SILENT_DB` for silence).
pub fn amp_db(amp: f32) -> f32 {
    if amp <= 0.0 || !amp.is_finite() {
        SILENT_DB
    } else {
        power_to_db(f64::from(amp) * f64::from(amp))
    }
}

/// The energy of the low end of a stereo signal: its mean square for the pair and for its mono sum.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lows {
    /// Mean of `(L^2 + R^2) / 2` of the low-passed signal.
    pub stereo: f64,
    /// Mean of `((L + R) / 2)^2` of the same.
    pub mono: f64,
}

impl Lows {
    /// The level of the stereo low end, dBFS.
    pub fn stereo_db(&self) -> f32 {
        power_to_db(self.stereo)
    }

    /// What the mono sum loses, dB (zero or positive; 0 for a signal with no low end).
    pub fn lost_db(&self) -> f32 {
        if self.stereo <= 0.0 {
            return 0.0;
        }
        (power_to_db(self.stereo) - power_to_db(self.mono.max(1e-12))).max(0.0)
    }

    /// The power the mono sum loses (what is not left of the stereo power).
    pub fn lost_power(&self) -> f64 {
        (self.stereo - self.mono).max(0.0)
    }
}

/// What [`measure`] reads from one signal.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalStats {
    /// Frames measured.
    pub frames: usize,
    /// The highest absolute sample of either channel (linear).
    pub peak: f32,
    /// The level over the blocks that are not silent, dBFS RMS ([`SILENT_DB`] when all are).
    pub rms_db: f32,
    /// The highest absolute sample of each window of [`PEAK_WINDOW`] frames.
    pub peaks: Vec<f32>,
    /// The low end.
    pub lows: Lows,
}

impl SignalStats {
    /// The peak in dBFS.
    pub fn peak_db(&self) -> f32 {
        amp_db(self.peak)
    }
}

fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Measures a stereo signal (`right` may be empty for a mono one): see the module docs.
pub fn measure(left: &[f32], right: &[f32], rate: u32) -> SignalStats {
    let len = if right.is_empty() {
        left.len()
    } else {
        left.len().min(right.len())
    };
    let r_at = |i: usize| clean(if right.is_empty() { left[i] } else { right[i] });
    let gate = db_to_power(RMS_GATE_DB);
    let mut peaks = Vec::with_capacity(len.div_ceil(PEAK_WINDOW));
    let (mut block, mut block_n) = (0.0f64, 0usize);
    let (mut active, mut active_n) = (0.0f64, 0usize);
    let mut peak = 0.0f32;
    let mut window = 0.0f32;
    let mut lows = (
        Crossover::new(rate.max(1) as f32),
        Crossover::new(rate.max(1) as f32),
    );
    lows.0.set_freq(LOW_HZ);
    lows.1.set_freq(LOW_HZ);
    let (mut low_stereo, mut low_mono) = (0.0f64, 0.0f64);
    for (i, &sample) in left.iter().enumerate().take(len) {
        let (l, r) = (clean(sample), r_at(i));
        let top = l.abs().max(r.abs());
        window = window.max(top);
        peak = peak.max(top);
        let (l64, r64) = (f64::from(l), f64::from(r));
        block += (l64 * l64 + r64 * r64) * 0.5;
        block_n += 1;
        let (ll, _) = lows.0.process(l);
        let (lr, _) = lows.1.process(r);
        let (ll, lr) = (f64::from(ll), f64::from(lr));
        low_stereo += (ll * ll + lr * lr) * 0.5;
        let m = (ll + lr) * 0.5;
        low_mono += m * m;
        if (i + 1) % PEAK_WINDOW == 0 || i + 1 == len {
            peaks.push(window);
            window = 0.0;
        }
        if block_n == RMS_BLOCK || i + 1 == len {
            let mean = block / block_n as f64;
            if mean > gate {
                active += mean;
                active_n += 1;
            }
            block = 0.0;
            block_n = 0;
        }
    }
    let n = len.max(1) as f64;
    SignalStats {
        frames: len,
        peak,
        rms_db: if active_n == 0 {
            SILENT_DB
        } else {
            power_to_db(active / active_n as f64)
        },
        peaks,
        lows: Lows {
            stereo: low_stereo / n,
            mono: low_mono / n,
        },
    }
}

/// The note frequency of a MIDI key, Hz.
pub fn key_hz(key: u8) -> f32 {
    (440.0 * exp2_det_f64((f64::from(key) - 69.0) / 12.0)) as f32
}

/// The name of a MIDI key: `E1`, `F#2` (middle C is C4).
pub fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key) % 12],
        i32::from(key) / 12 - 1
    )
}

/// The root of a bass part from its notes `(length, key)`: the key at the middle of the held time
/// (the median of the keys weighted by length). `None` for no notes.
pub fn bass_root(notes: &[(i64, u8)]) -> Option<u8> {
    let mut sorted: Vec<(u8, i64)> = notes
        .iter()
        .filter(|(len, _)| *len > 0)
        .map(|&(len, key)| (key, len))
        .collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable();
    let total: i64 = sorted.iter().map(|(_, l)| l).sum();
    let mut seen = 0i64;
    for (key, len) in &sorted {
        seen += len;
        if seen * 2 >= total {
            return Some(*key);
        }
    }
    sorted.last().map(|(k, _)| *k)
}

/// Whether a bass root of `hz` sits in [`BASS_ROOT_HZ`] (with its tolerance).
pub fn root_in_range(hz: f32) -> bool {
    let tolerance = exp2_det_f64(f64::from(BASS_ROOT_SEMITONES) / 12.0) as f32;
    hz >= BASS_ROOT_HZ.0 / tolerance && hz <= BASS_ROOT_HZ.1 * tolerance
}

/// The fundamental of a part against its harmonics, in the mean spectrum of the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fundamental {
    /// The fundamental, Hz (the centre of its band).
    pub hz: f32,
    /// Its level, dB.
    pub level_db: f32,
    /// The harmonic number (2 to [`HIGHEST_HARMONIC`]) with the highest level.
    pub harmonic: u32,
    /// That harmonic's level, dB.
    pub harmonic_db: f32,
}

impl Fundamental {
    /// How far the strongest harmonic is over the fundamental, dB (negative when it is under).
    pub fn gap_db(&self) -> f32 {
        self.harmonic_db - self.level_db
    }

    /// Whether the fundamental is weak ([`WEAK_FUNDAMENTAL_DB`]).
    pub fn weak(&self) -> bool {
        self.gap_db() > WEAK_FUNDAMENTAL_DB
    }
}

/// The level of the fundamental `hz` (the loudest of its band and its two neighbours, since a
/// note rarely sits on a centre) against the loudest of its 2nd to 4th harmonics, read from the
/// mean spectrum `mean`. `None` when the part has no content there (the fundamental is under
/// [`ACTIVE_DB`]) or the harmonics are out of the map.
pub fn fundamental_of(mean: &[f32; BANDS], hz: f32) -> Option<Fundamental> {
    let base = band_of(hz);
    let level = |b: usize| mean[b.min(BANDS - 1)];
    let fundamental = (base.saturating_sub(1)..=base + 1)
        .map(level)
        .fold(SILENT_DB, f32::max);
    if fundamental < ACTIVE_DB {
        return None;
    }
    let mut best: Option<(u32, f32)> = None;
    for h in 2..=HIGHEST_HARMONIC {
        let band = band_of(hz * h as f32);
        if band >= BANDS - 1 || band <= base + 1 {
            continue;
        }
        let l = level(band);
        if best.is_none_or(|(_, b)| l > b) {
            best = Some((h, l));
        }
    }
    let (harmonic, harmonic_db) = best?;
    Some(Fundamental {
        hz: band_centre(base),
        level_db: fundamental,
        harmonic,
        harmonic_db,
    })
}

/// The frequency of the loudest band of `mean` whose centre lies in `range` (Hz), and `None` when
/// the loudest is under [`ACTIVE_DB`].
pub fn loudest_in(mean: &[f32; BANDS], range: (f32, f32)) -> Option<f32> {
    let (band, level) = (0..BANDS)
        .filter(|&b| {
            let c = band_centre(b);
            c >= range.0 * 0.99 && c <= range.1 * 1.01
        })
        .map(|b| (b, mean[b]))
        .fold(
            (0, SILENT_DB),
            |best, x| if x.1 > best.1 { x } else { best },
        );
    (level >= ACTIVE_DB).then(|| band_centre(band))
}

/// The bands whose centre lies in [`LOW_MID_HZ`].
pub fn low_mid_bands() -> std::ops::RangeInclusive<usize> {
    let inside = |b: &usize| {
        let c = band_centre(*b);
        c >= LOW_MID_HZ.0 * 0.99 && c <= LOW_MID_HZ.1 * 1.01
    };
    let first = (0..BANDS).find(inside).unwrap_or(0);
    let last = (0..BANDS).rev().find(inside).unwrap_or(first);
    first..=last
}

/// The power of the low-mid bands and of all the bands of a mean spectrum.
pub fn low_mid_power(mean: &[f32; BANDS]) -> (f64, f64) {
    let region = low_mid_bands();
    let mut low = 0.0;
    let mut all = 0.0;
    for (b, l) in mean.iter().enumerate() {
        let p = db_to_power(*l);
        all += p;
        if region.contains(&b) {
            low += p;
        }
    }
    (low, all)
}

/// The windows of the master's peak envelope that are its highest spots: the loudest first, each
/// at least [`PEAK_APART`] windows from the ones chosen before, at most `max`.
pub fn peak_spots(master: &[f32], max: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..master.len()).filter(|&w| master[w] > 0.0).collect();
    order.sort_by(|&a, &b| master[b].total_cmp(&master[a]).then(a.cmp(&b)));
    let mut out: Vec<usize> = Vec::new();
    for w in order {
        if out.len() == max {
            break;
        }
        if out.iter().all(|&o| o.abs_diff(w) >= PEAK_APART) {
            out.push(w);
        }
    }
    out
}

/// The trim that brings a feed of `rms_db` to [`REFERENCE_RMS_DB`], rounded to half a dB and
/// limited to [`MAX_TRIM_DB`].
pub fn trim_for(rms_db: f32) -> f32 {
    let raw = (REFERENCE_RMS_DB - rms_db).clamp(-MAX_TRIM_DB, MAX_TRIM_DB);
    (raw * 2.0).round() / 2.0
}

/// Whether a feed of `rms_db` is far from the reference level ([`OFF_REFERENCE_DB`]).
pub fn off_reference(rms_db: f32) -> bool {
    rms_db > SILENT_DB && (rms_db - REFERENCE_RMS_DB).abs() > OFF_REFERENCE_DB
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::masking::analyse;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn a_stereo_sine_reads_its_level_and_its_peak() {
        let s = sine(1000.0, 0.5, 48_000);
        let stats = measure(&s, &s, RATE);
        // A sine of amplitude 0.5 has an RMS of 0.5 / sqrt(2): -9.03 dBFS.
        assert!((stats.rms_db + 9.03).abs() < 0.1, "{}", stats.rms_db);
        assert!((stats.peak - 0.5).abs() < 1e-3);
        assert_eq!(stats.peaks.len(), 48_000usize.div_ceil(PEAK_WINDOW));
        assert!(stats.peaks.iter().all(|p| *p <= 0.5 + 1e-6));
    }

    #[test]
    fn silent_blocks_do_not_count_in_the_level() {
        let mut s = sine(1000.0, 0.5, 8192);
        s.extend(vec![0.0; 40_000]);
        let stats = measure(&s, &s, RATE);
        assert!((stats.rms_db + 9.03).abs() < 0.2, "{}", stats.rms_db);
        let silence = measure(&[0.0; 9000], &[0.0; 9000], RATE);
        assert_eq!(silence.rms_db, SILENT_DB);
    }

    #[test]
    fn opposite_low_channels_lose_everything_in_mono_and_equal_ones_nothing() {
        let a = sine(60.0, 0.5, 48_000);
        let b: Vec<f32> = a.iter().map(|x| -x).collect();
        let out_of_phase = measure(&a, &b, RATE);
        assert!(
            out_of_phase.lows.lost_db() > 30.0,
            "{}",
            out_of_phase.lows.lost_db()
        );
        let centred = measure(&a, &a, RATE);
        assert!(centred.lows.lost_db() < 0.01);
        // Content above the corner is not part of the low end.
        let high = sine(4000.0, 0.5, 48_000);
        let inverted: Vec<f32> = high.iter().map(|x| -x).collect();
        let m = measure(&high, &inverted, RATE);
        assert!(m.lows.stereo_db() < -60.0, "{}", m.lows.stereo_db());
    }

    #[test]
    fn a_quarter_period_offset_loses_three_db() {
        // 60 Hz in the right channel a quarter turn late: the mono sum has half the power.
        let n = 48_000;
        let l = sine(60.0, 0.5, n);
        let r: Vec<f32> = (0..n)
            .map(|i| {
                0.5 * (2.0 * std::f32::consts::PI * 60.0 * i as f32 / RATE as f32
                    + std::f32::consts::FRAC_PI_2)
                    .sin()
            })
            .collect();
        let lost = measure(&l, &r, RATE).lows.lost_db();
        assert!((lost - 3.0).abs() < 0.3, "{lost}");
    }

    #[test]
    fn measuring_is_repeatable() {
        let s = sine(80.0, 0.3, 20_000);
        assert_eq!(measure(&s, &s, RATE), measure(&s, &s, RATE));
    }

    #[test]
    fn the_bass_root_is_the_middle_of_the_held_time() {
        // E1 held three times longer than G1: E1 is the root.
        assert_eq!(bass_root(&[(300, 28), (100, 31)]), Some(28));
        assert_eq!(bass_root(&[]), None);
        assert_eq!(bass_root(&[(0, 40)]), None);
        // Equal lengths: the lower half decides at the middle.
        assert_eq!(bass_root(&[(100, 28), (100, 40)]), Some(28));
    }

    #[test]
    fn keys_have_their_frequency_and_name() {
        assert!((key_hz(69) - 440.0).abs() < 0.01);
        assert!((key_hz(28) - 41.2).abs() < 0.05);
        assert_eq!(key_name(28), "E1");
        assert_eq!(key_name(60), "C4");
        assert_eq!(key_name(37), "C#2");
    }

    #[test]
    fn the_root_range_has_a_semitone_of_slack() {
        assert!(root_in_range(key_hz(28))); // E1
        assert!(root_in_range(key_hz(35))); // B1
        assert!(root_in_range(key_hz(27))); // D#1
        assert!(!root_in_range(key_hz(26))); // D1
        assert!(!root_in_range(key_hz(36))); // C2
    }

    fn map_of(tone: &[(f32, f32)]) -> [f32; BANDS] {
        // A tone of each (hz, amplitude), measured through the real analysis.
        let n = 32_768;
        let mut s = vec![0.0f32; n];
        for &(hz, amp) in tone {
            for (a, b) in s.iter_mut().zip(sine(hz, amp, n)) {
                *a += b;
            }
        }
        analyse(&s, &s, RATE).mean(&[])
    }

    #[test]
    fn a_bass_that_is_mostly_harmonics_has_a_weak_fundamental() {
        let weak = map_of(&[(50.0, 0.02), (100.0, 0.2), (150.0, 0.2)]);
        let f = fundamental_of(&weak, 50.0).expect("content");
        assert!(f.weak(), "gap {}", f.gap_db());
        assert!(f.harmonic == 2 || f.harmonic == 3);
        let strong = map_of(&[(50.0, 0.4), (100.0, 0.1), (150.0, 0.05)]);
        let f = fundamental_of(&strong, 50.0).expect("content");
        assert!(!f.weak(), "gap {}", f.gap_db());
        // No content at the fundamental: nothing to judge.
        let empty = map_of(&[(1000.0, 0.3)]);
        assert!(fundamental_of(&empty, 50.0).is_none());
    }

    #[test]
    fn the_kick_fundamental_is_the_loudest_low_band() {
        let m = map_of(&[(55.0, 0.3), (200.0, 0.1)]);
        let hz = loudest_in(&m, KICK_HZ).expect("a kick");
        assert!((hz - 50.4).abs() < 15.0, "{hz}");
        assert!(loudest_in(&map_of(&[(2000.0, 0.3)]), KICK_HZ).is_none());
    }

    #[test]
    fn the_low_mid_region_is_five_bands_from_160_hz() {
        let bands = low_mid_bands();
        assert_eq!(bands.clone().count(), 5);
        assert!((band_centre(*bands.start()) - 160.0).abs() < 1.0);
        let m = map_of(&[(250.0, 0.3)]);
        let (low, all) = low_mid_power(&m);
        assert!(low / all > 0.9, "{}", low / all);
    }

    #[test]
    fn peak_spots_are_the_highest_windows_that_are_apart() {
        let mut env = vec![0.1f32; 400];
        env[10] = 0.9;
        env[12] = 0.95; // near the first: the louder of the two wins
        env[200] = 0.8;
        env[390] = 0.7;
        let spots = peak_spots(&env, 3);
        assert_eq!(spots, vec![12, 200, 390]);
        assert_eq!(peak_spots(&env, 1), vec![12]);
        assert!(peak_spots(&[0.0; 10], 3).is_empty());
    }

    #[test]
    fn the_trim_brings_a_feed_to_the_reference_in_half_db_steps() {
        assert_eq!(trim_for(-31.2), 13.0);
        assert_eq!(trim_for(-6.0), -12.0);
        assert_eq!(trim_for(-90.0), MAX_TRIM_DB);
        assert!(off_reference(-31.0));
        assert!(off_reference(-6.0));
        assert!(!off_reference(-20.0));
        assert!(!off_reference(SILENT_DB));
    }
}
