//! Sound analysis ("recreate this sound"): offline measurements of a short sound that say how to
//! rebuild it on a synth: how loud it is over time, how bright it is over time, how its pitch
//! moves, what its harmonics look like and how much of it is noise.
//!
//! It runs on a worker from decoded audio, never on the audio thread. [`analyse`] mixes the input
//! to mono, keeps the first [`MAX_SECONDS`] and measures, on a grid of [`HOP_S`] (5 ms) frames
//! that starts where the sound starts:
//!
//! 1. **Level.** The RMS of a [`WINDOW_S`] window per frame, divided by the loudest frame
//!    (`0..=1`). The sound starts at the first frame that reaches [`ONSET_LEVEL`] of the loudest and
//!    ends at the last that still reaches [`END_LEVEL`] (-40 dB); everything below is cut. A
//!    take whose peak is under [`SILENT_PEAK`] is silent, and one shorter than [`MIN_SECONDS`]
//!    after the cut is too short.
//! 2. **Envelope** ([`Envelope`]), from the level curve, with crossings found between frames:
//!    the *attack* is the time the level takes from [`ATTACK_LOW`] to [`ATTACK_HIGH`] of the peak.
//!    A *plateau* is the longest run (at least [`MIN_PLATEAU_S`]) after the peak in which the
//!    level stays within [`PLATEAU_DB`] of its first value and above [`SUSTAIN_FLOOR`]. With a
//!    plateau the *sustain* is its mean level, the *decay* the time from the peak to within
//!    [`GAP_FRACTION`] of the way down to it, the note is *held* until the plateau ends, and the
//!    *release* is the time from there to a tenth of the sustain. Without one the sustain is 0,
//!    the decay is the time to fall to [`GAP_FRACTION`] of the peak, the note is held until then,
//!    and the release is 0 (the sound dies by itself). The window blurs the start of a sound: an
//!    attack shorter than about 0.8 [`WINDOW_S`] reads as that long.
//! 3. **Brightness** ([`Brightness`]): the spectral centroid (magnitude weighted mean frequency
//!    of a Hann window of about 40 ms) of every frame whose level reaches [`BRIGHTNESS_GATE`];
//!    the rest are 0. `start_hz` is the geometric mean of the first few frames from the peak,
//!    `end_hz` that of the last part of the held note, and `fall_s` the time the centroid takes
//!    to travel 90 % of the way (in octaves) from one to the other; a movement under
//!    [`STATIC_OCTAVES`] counts as none.
//! 4. **Pitch** ([`PitchMotion`]): the tracker of the audio-to-notes extraction
//!    ([`crate::notes::track_pitch`], keys [`KEY_LO`] to [`KEY_HI`]) gives a pitch per frame. The
//!    sound is *pitched* when at least half of its frames above [`PITCH_GATE`] are voiced; the
//!    fundamental is the median of those frames. `start_semitones` is the offset of the first
//!    voiced frames from it and `settle_s` the time they take to come within [`SETTLED_SEMITONES`];
//!    an offset under that counts as none.
//! 5. **Harmonics and noise** ([`Harmonics`], `noise_share`): the mean power spectrum of three
//!    [`SPECTRUM_FFT`]-frame Hann windows from the peak. A bin is a *peak* when it is a local
//!    maximum, at least [`PEAK_PROMINENCE`] times the median of the 65 bins around it and
//!    within -50 dB of the loudest bin. `noise_share` is the power that is not within two bins
//!    of a peak, over the total power (0 for a clean tone, about 1 for noise). With a
//!    fundamental, the strongest peak near each of its first [`HARMONICS`] multiples
//!    ([`HARMONIC_TOLERANCE`]) is that harmonic; the profile is then [`HarmonicKind::Inharmonic`]
//!    when less than [`ALIGNED_MIN`] of the power of the peaks within -40 dB of the loudest is on
//!    harmonics, [`HarmonicKind::Pure`] when harmonics 2 to 12 hold under [`PURE_BELOW`] of the
//!    power of the harmonics, [`HarmonicKind::OddOnly`] when the even harmonics 2, 4 and 6 hold
//!    [`ODD_ONLY_DB`] less than the odd ones 3, 5 and 7 and [`HarmonicKind::All`] otherwise. A
//!    `noise_share` of [`NOISE_FROM`] or more is [`HarmonicKind::Noise`] whatever the peaks, and
//!    so is an unpitched sound without clear peaks; an unpitched one with peaks is
//!    [`HarmonicKind::Inharmonic`] (a bell); a single partial is [`HarmonicKind::Pure`]. An
//!    inharmonic sound has no pitch motion (the tracker's pitch means little for it) and its
//!    partials are measured against the lowest strong one.
//!
//! [`compare`] measures how far a second profile (a render of a proposed patch) is from a first
//! on the three curves: [`Distances`].
//!
//! The decision path uses only IEEE `+ - * /`, `sqrt`, sorting and the deterministic `log2`,
//! `exp2` and cosine of this crate, so the same samples give the same profile on every platform
//! (the pitch tracker is deterministic by the same rules). Silence and non-finite samples give an
//! error or a profile, never `NaN`.

use crate::det_math::{exp2_det, log2_det};
use crate::det_trig::cos_cycles;
use crate::notes::track_pitch;
use crate::spectral::Fft;

/// Seconds between two frames of the curves.
pub const HOP_S: f32 = 0.005;
/// Seconds of one RMS window of the level curve.
pub const WINDOW_S: f32 = 0.02;
/// The longest part of the input that is analysed, in seconds.
pub const MAX_SECONDS: f32 = 10.0;
/// The shortest sound that is analysed, in seconds (after the silence at both ends is cut).
pub const MIN_SECONDS: f32 = 0.05;
/// A take whose peak sample is below this is silent (about -80 dBFS).
pub const SILENT_PEAK: f32 = 1.0e-4;
/// The sound starts at the first frame at this share of the loudest frame.
pub const ONSET_LEVEL: f32 = 0.02;
/// The sound ends at the last frame at this share of the loudest frame (-40 dB).
pub const END_LEVEL: f32 = 0.01;
/// The share of the peak where the attack starts.
pub const ATTACK_LOW: f32 = 0.1;
/// The share of the peak where the attack ends.
pub const ATTACK_HIGH: f32 = 0.9;
/// A run of frames is a plateau while the level stays this close to its first value, in dB.
pub const PLATEAU_DB: f32 = 1.0;
/// The shortest plateau, in seconds.
pub const MIN_PLATEAU_S: f32 = 0.15;
/// A plateau must be above this share of the peak (below it the level is a tail, not a sustain).
pub const SUSTAIN_FLOOR: f32 = 0.2;
/// The decay ends when the level is this share of the way down from the peak to the sustain.
pub const GAP_FRACTION: f32 = 0.1;
/// A frame below this share of the loudest has no centroid (-50 dB).
pub const BRIGHTNESS_GATE: f32 = 0.003;
/// A centroid movement under this many octaves counts as none.
pub const STATIC_OCTAVES: f32 = 0.25;
/// Frames from the peak that give the start brightness.
const START_FRAMES: usize = 4;
/// The share of the held note, from its end, that gives the end brightness.
const END_SHARE: f32 = 0.4;
/// The lowest key the pitch tracker looks for (about 41 Hz).
pub const KEY_LO: u8 = 28;
/// The highest key the pitch tracker looks for (about 2.6 kHz).
pub const KEY_HI: u8 = 100;
/// The tracker's sensitivity.
const PITCH_SENSITIVITY: f32 = 0.5;
/// A frame must reach this share of the loudest to count for the pitch.
pub const PITCH_GATE: f32 = 0.05;
/// A pitch offset under this many semitones counts as none.
pub const SETTLED_SEMITONES: f32 = 0.3;
/// The most a pitch offset can be, in semitones.
pub const MAX_OFFSET_SEMITONES: f32 = 24.0;
/// Frames of the spectrum windows.
pub const SPECTRUM_FFT: usize = 8192;
/// Windows averaged for the spectrum.
const SPECTRUM_WINDOWS: usize = 3;
/// A peak must be this many times the median power of the bins around it (12 dB).
pub const PEAK_PROMINENCE: f64 = 16.0;
/// Bins either side of a bin whose median is taken.
const MEDIAN_SPAN: usize = 32;
/// Bins either side of a peak that belong to it.
const LOBE: usize = 2;
/// The weakest peak that counts, as a share of the loudest bin's power (-50 dB).
const PEAK_FLOOR: f64 = 1.0e-5;
/// How many harmonics are looked for.
pub const HARMONICS: usize = 12;
/// How far a peak may be from a multiple of the fundamental to be that harmonic, as a ratio.
pub const HARMONIC_TOLERANCE: f64 = 0.03;
/// The share of the power of the strong peaks that must be on harmonics to be harmonic.
pub const ALIGNED_MIN: f64 = 0.75;
/// Peaks within this share of the loudest bin's power count as strong (-40 dB).
const STRONG_FLOOR: f64 = 1.0e-4;
/// The share of the harmonics' power in harmonics 2 to 12 under which a tone is pure.
pub const PURE_BELOW: f64 = 0.001;
/// How much weaker the even harmonics must be than the odd ones to be odd only, in dB.
pub const ODD_ONLY_DB: f64 = 15.0;
/// The share of noise from which a sound is noise.
pub const NOISE_FROM: f32 = 0.75;
/// The reference of an inharmonic sound is its lowest peak within this share of the loudest
/// bin's power (-13 dB).
const REFERENCE_FLOOR: f64 = 0.05;
/// How many partials a profile lists.
pub const MAX_PARTIALS: usize = 8;

/// Why a sound cannot be analysed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisError {
    /// There is no sound: the take is silent.
    Silent,
    /// The sound is shorter than [`MIN_SECONDS`].
    TooShort,
    /// The sample rate is under 8 kHz.
    Rate,
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Silent => "the sound is silent",
            Self::TooShort => "the sound is too short to analyse",
            Self::Rate => "the sample rate is too low to analyse",
        })
    }
}

impl std::error::Error for AnalysisError {}

/// The amplitude envelope of a sound. Times are seconds from the start of the sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    /// The rise from 10 % to 90 % of the peak.
    pub attack_s: f32,
    /// From the peak to the end of the decay (see the module docs).
    pub decay_s: f32,
    /// The held level as a share of the peak (0 when the sound dies by itself).
    pub sustain: f32,
    /// From the end of the held note to a tenth of the sustain; 0 without a sustain.
    pub release_s: f32,
    /// How long the note is held: from the start of the sound to where the plateau ends, or
    /// where the decay ends when there is no plateau.
    pub hold_s: f32,
}

/// How the brightness (the spectral centroid) moves. Frequencies in Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brightness {
    /// The centroid at the peak.
    pub start_hz: f32,
    /// The centroid at the end of the held note.
    pub end_hz: f32,
    /// The level-weighted geometric mean over the sound.
    pub mean_hz: f32,
    /// The time to travel 90 % of the way (in octaves) from `start_hz` to `end_hz`; 0 when the
    /// brightness is static.
    pub fall_s: f32,
}

/// How the pitch of a pitched sound is placed and moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchMotion {
    /// The fundamental, the median of the voiced frames, in Hz.
    pub fundamental_hz: f32,
    /// The same as a fractional MIDI key (69 is 440 Hz).
    pub key: f32,
    /// How far the pitch starts from the fundamental, in semitones (0 for none).
    pub start_semitones: f32,
    /// The time the pitch takes to settle; 0 for none.
    pub settle_s: f32,
}

/// What the harmonics of a sound look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarmonicKind {
    /// A single partial: a sine.
    Pure,
    /// Harmonics of every order: a saw.
    All,
    /// The odd harmonics only: a square or a triangle.
    OddOnly,
    /// Partials that are not multiples of the fundamental: a bell, metal, FM.
    Inharmonic,
    /// Broadband noise.
    Noise,
}

impl HarmonicKind {
    /// A short name for the panel.
    pub fn text(self) -> &'static str {
        match self {
            Self::Pure => "pure tone",
            Self::All => "all harmonics",
            Self::OddOnly => "odd harmonics only",
            Self::Inharmonic => "inharmonic",
            Self::Noise => "noise",
        }
    }
}

/// One partial of the spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Partial {
    /// Its frequency over the fundamental (of an unpitched sound: over its loudest partial).
    pub ratio: f32,
    /// Its level against the loudest partial, in dB (0 or less).
    pub level_db: f32,
}

/// The harmonic profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Harmonics {
    /// The kind.
    pub kind: HarmonicKind,
    /// The frequency the ratios of the partials are over: the fundamental, or for an unpitched
    /// sound its loudest partial; `None` when no peak was found.
    pub reference_hz: Option<f32>,
    /// The strongest partials, loudest first (at most [`MAX_PARTIALS`]).
    pub partials: Vec<Partial>,
    /// The level of the third harmonic against the first, in dB, when both were found.
    pub third_db: Option<f32>,
}

/// What [`analyse`] measured.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundProfile {
    /// The sample rate of the analysed audio.
    pub sample_rate: u32,
    /// Seconds between two frames of the curves (close to [`HOP_S`]).
    pub hop_s: f32,
    /// The silence cut from the start, in seconds.
    pub lead_s: f32,
    /// The length of the sound, from its start to its end, in seconds.
    pub duration_s: f32,
    /// The amplitude envelope.
    pub envelope: Envelope,
    /// The level curve, one value per frame, `0..=1` of the peak.
    pub level: Vec<f32>,
    /// The centroid curve in Hz per frame, 0 where the frame is too quiet.
    pub centroid_hz: Vec<f32>,
    /// The pitch curve in Hz per frame, 0 where the frame is unvoiced.
    pub pitch_hz: Vec<f32>,
    /// How the brightness moves.
    pub brightness: Brightness,
    /// The pitch of a pitched sound; `None` for an unpitched one.
    pub pitch: Option<PitchMotion>,
    /// The harmonic profile.
    pub harmonics: Harmonics,
    /// The share of the power that is noise, `0..=1`.
    pub noise_share: f32,
}

impl SoundProfile {
    /// Whether the sound has a pitch.
    pub fn pitched(&self) -> bool {
        self.pitch.is_some()
    }

    /// The time of frame `i`, in seconds from the start of the sound.
    pub fn time_of(&self, i: usize) -> f32 {
        i as f32 * self.hop_s
    }
}

/// The distances between two profiles, one per curve; smaller is closer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distances {
    /// The mean difference of the level curves in dB.
    pub envelope_db: f32,
    /// The mean difference of the centroid curves in octaves.
    pub brightness_octaves: f32,
    /// The mean difference of the pitch curves in semitones; `None` when the target is unpitched.
    pub pitch_semitones: Option<f32>,
}

/// The distance at which a curve counts as half matched, per curve: dB, octaves, semitones.
pub const HALF_MATCH: (f32, f32, f32) = (6.0, 0.75, 0.5);

impl Distances {
    /// The three curves as a match, `0..=1` each (1 is the same curve): `1 / (1 + d / h)` with
    /// `h` the [`HALF_MATCH`] distance. An unpitched target has no pitch score.
    pub fn scores(&self) -> (f32, f32, Option<f32>) {
        let score = |d: f32, half: f32| 1.0 / (1.0 + d.max(0.0) / half);
        (
            score(self.envelope_db, HALF_MATCH.0),
            score(self.brightness_octaves, HALF_MATCH.1),
            self.pitch_semitones.map(|d| score(d, HALF_MATCH.2)),
        )
    }

    /// The mean of the scores of the curves that exist.
    pub fn overall(&self) -> f32 {
        let (e, b, p) = self.scores();
        match p {
            Some(p) => (e + b + p) / 3.0,
            None => (e + b) / 2.0,
        }
    }
}

fn to_db(x: f32) -> f32 {
    6.020_6 * log2_det(x.max(1.0e-5))
}

fn mono_of(channels: &[&[f32]], frames: usize) -> Vec<f32> {
    let n = channels
        .iter()
        .map(|c| c.len())
        .min()
        .unwrap_or(0)
        .min(frames);
    let inv = 1.0 / channels.len().max(1) as f64;
    (0..n)
        .map(|i| {
            let mut sum = 0.0f64;
            for c in channels {
                if c[i].is_finite() {
                    sum += f64::from(c[i]);
                }
            }
            (sum * inv) as f32
        })
        .collect()
}

/// The RMS of the window around every multiple of `hop`: frame `j` covers
/// `j * hop - window / 2 .. j * hop + window / 2`.
fn rms_frames(mono: &[f32], hop: usize, window: usize) -> Vec<f32> {
    let mut sums = Vec::with_capacity(mono.len() + 1);
    let mut run = 0.0f64;
    sums.push(0.0);
    for v in mono {
        run += f64::from(*v) * f64::from(*v);
        sums.push(run);
    }
    (0..mono.len() / hop + 1)
        .map(|j| {
            let centre = j * hop;
            let from = centre.saturating_sub(window / 2);
            let to = (centre + window / 2).min(mono.len());
            if to <= from {
                return 0.0;
            }
            ((sums[to] - sums[from]) / (to - from) as f64).sqrt() as f32
        })
        .collect()
}

/// The first position (in frames, fractional) at or after `from` where `level` reaches `at`
/// going up.
fn cross_up(level: &[f32], at: f32, from: usize) -> Option<f32> {
    for i in from..level.len() {
        if level[i] >= at {
            if i == from || level[i - 1] >= at {
                return Some(i as f32);
            }
            let (a, b) = (level[i - 1], level[i]);
            return Some(i as f32 - 1.0 + (at - a) / (b - a).max(f32::MIN_POSITIVE));
        }
    }
    None
}

/// The first position at or after `from` where `level` falls to `at` or less.
fn cross_down(level: &[f32], at: f32, from: usize) -> Option<f32> {
    for i in from..level.len() {
        if level[i] <= at {
            if i == from || level[i - 1] <= at {
                return Some(i as f32);
            }
            let (a, b) = (level[i - 1], level[i]);
            return Some(i as f32 - 1.0 + (a - at) / (a - b).max(f32::MIN_POSITIVE));
        }
    }
    None
}

/// The longest plateau of `level` from frame `top` on, as the first and last frame.
fn plateau(level: &[f32], top: usize, hop_s: f32) -> Option<(usize, usize)> {
    let db: Vec<f32> = level.iter().map(|l| to_db(*l)).collect();
    let min_run = (MIN_PLATEAU_S / hop_s).ceil() as usize;
    let mut best: Option<(usize, usize)> = None;
    let mut i = top;
    while i < level.len() {
        if level[i] < SUSTAIN_FLOOR {
            i += 1;
            continue;
        }
        let reference = db[i];
        let mut k = i;
        while k + 1 < level.len()
            && level[k + 1] >= SUSTAIN_FLOOR
            && (db[k + 1] - reference).abs() <= PLATEAU_DB
        {
            k += 1;
        }
        if k - i + 1 >= min_run && best.is_none_or(|(a, b)| k - i > b - a) {
            best = Some((i, k));
        }
        i = k + 1;
    }
    best
}

/// The envelope of the level curve (module docs, step 2) and the frame where the peak is first
/// reached.
fn envelope_of(level: &[f32], hop_s: f32) -> (Envelope, usize) {
    let top = level.iter().position(|l| *l >= 0.95).unwrap_or(0);
    let low = cross_up(level, ATTACK_LOW, 0).unwrap_or(0.0);
    let high = cross_up(level, ATTACK_HIGH, 0).unwrap_or(top as f32);
    let attack_s = (high - low).max(0.0) * hop_s;
    let last = level.len() - 1;
    let envelope = if let Some((from, to)) = plateau(level, top, hop_s) {
        let held = &level[from..=to];
        let sustain = held.iter().sum::<f32>() / held.len() as f32;
        let decay_s = if sustain >= 0.9 {
            0.0
        } else {
            let down = sustain + GAP_FRACTION * (1.0 - sustain);
            let at = cross_down(level, down, top).unwrap_or(from as f32);
            (at - top as f32).max(0.0) * hop_s
        };
        let off = to as f32;
        let end = cross_down(level, sustain * 0.1, to).unwrap_or(last as f32);
        Envelope {
            attack_s,
            decay_s,
            sustain,
            release_s: (end - off).max(0.0) * hop_s,
            hold_s: off * hop_s,
        }
    } else {
        let at = cross_down(level, GAP_FRACTION, top).unwrap_or(last as f32);
        Envelope {
            attack_s,
            decay_s: (at - top as f32).max(0.0) * hop_s,
            sustain: 0.0,
            release_s: 0.0,
            hold_s: at * hop_s,
        }
    };
    (envelope, top)
}

/// The geometric mean of the defined (non-zero) values of `v`.
fn geometric_mean(v: &[f32]) -> Option<f32> {
    let defined: Vec<f32> = v.iter().copied().filter(|x| *x > 0.0).collect();
    if defined.is_empty() {
        return None;
    }
    let sum: f32 = defined.iter().map(|x| log2_det(*x)).sum();
    Some(exp2_det(sum / defined.len() as f32))
}

/// A Hann window of `n` frames.
fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * cos_cycles(i as f64 / n as f64)) as f32)
        .collect()
}

/// The FFT size of the centroid frames: a power of two of about 40 ms.
fn centroid_size(rate: u32) -> usize {
    ((rate as f32 * 0.04) as usize).next_power_of_two().max(256)
}

/// The spectral centroid of every frame of `from..from + level.len()` whose level is above the
/// gate; 0 for the others.
fn centroid_curve(mono: &[f32], rate: u32, hop: usize, from: usize, level: &[f32]) -> Vec<f32> {
    let n = centroid_size(rate);
    let fft = Fft::new(n);
    let window = hann(n);
    let per_bin = rate as f32 / n as f32;
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    level
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if *l < BRIGHTNESS_GATE {
                return 0.0;
            }
            let start = ((from + i) * hop) as isize - (n / 2) as isize;
            for (k, (r, w)) in re.iter_mut().zip(&window).enumerate() {
                let at = start + k as isize;
                *r = if at >= 0 && (at as usize) < mono.len() {
                    mono[at as usize] * w
                } else {
                    0.0
                };
            }
            im.fill(0.0);
            fft.forward(&mut re, &mut im);
            let (mut weighted, mut total) = (0.0f64, 0.0f64);
            for k in 1..n / 2 {
                let m = f64::from(re[k]).hypot(f64::from(im[k]));
                weighted += m * (k as f64 * f64::from(per_bin));
                total += m;
            }
            if total > 0.0 {
                (weighted / total) as f32
            } else {
                0.0
            }
        })
        .collect()
}

fn brightness_of(
    centroid: &[f32],
    level: &[f32],
    top: usize,
    env: &Envelope,
    hop_s: f32,
) -> Brightness {
    let held = ((env.hold_s / hop_s) as usize).clamp(top, centroid.len() - 1);
    // Weighted by level: loud frames count more.
    let (mut sum, mut weight) = (0.0f32, 0.0f32);
    for (c, l) in centroid.iter().zip(level) {
        if *c > 0.0 {
            sum += l * log2_det(*c);
            weight += l;
        }
    }
    let mean_hz = if weight > 0.0 {
        exp2_det(sum / weight)
    } else {
        0.0
    };
    let start = geometric_mean(&centroid[top..(top + START_FRAMES).min(held + 1)]);
    let tail_from = held - ((held - top) as f32 * END_SHARE) as usize;
    let end = geometric_mean(&centroid[tail_from..=held]);
    let start_hz = start.or(end).unwrap_or(mean_hz);
    let end_hz = end.unwrap_or(start_hz);
    let travel = log2_det(start_hz.max(1.0)) - log2_det(end_hz.max(1.0));
    let fall_s = if start_hz <= 0.0 || end_hz <= 0.0 || travel.abs() < STATIC_OCTAVES {
        0.0
    } else {
        let close = 0.1 * travel.abs();
        let target = log2_det(end_hz);
        let found = (top..=held)
            .find(|&i| centroid[i] > 0.0 && (log2_det(centroid[i]) - target).abs() <= close);
        found.map_or(held - top, |i| i - top) as f32 * hop_s
    };
    Brightness {
        start_hz,
        end_hz,
        mean_hz,
        fall_s,
    }
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// The pitch curve and the motion of a pitched sound.
fn pitch_of(
    channels: &[&[f32]],
    rate: u32,
    (hop, from): (usize, usize),
    level: &[f32],
    hop_s: f32,
) -> (Vec<f32>, Option<PitchMotion>) {
    let track = track_pitch(channels, rate, (KEY_LO, KEY_HI), PITCH_SENSITIVITY);
    let keys: Vec<Option<f32>> = (0..level.len())
        .map(|i| {
            let frame = ((from + i) * hop) as f64 / track.step.max(1) as f64;
            track.keys.get(frame.round() as usize).copied().flatten()
        })
        .collect();
    let curve: Vec<f32> = keys
        .iter()
        .map(|k| k.map_or(0.0, |k| 440.0 * exp2_det((k - 69.0) / 12.0)))
        .collect();
    let gated: Vec<usize> = (0..level.len())
        .filter(|i| level[*i] >= PITCH_GATE)
        .collect();
    let voiced: Vec<usize> = gated
        .iter()
        .copied()
        .filter(|i| keys[*i].is_some())
        .collect();
    if voiced.len() < 3 || voiced.len() * 2 < gated.len() {
        return (vec![0.0; level.len()], None);
    }
    let mut all: Vec<f32> = voiced.iter().filter_map(|i| keys[*i]).collect();
    let key = median(&mut all);
    let early: Vec<f32> = voiced
        .iter()
        .take(3)
        .filter_map(|i| keys[*i])
        .map(|k| k - key)
        .collect();
    let mut start = early.iter().sum::<f32>() / early.len() as f32;
    let mut settle_s = 0.0;
    if start.abs() < SETTLED_SEMITONES {
        start = 0.0;
    } else {
        let first = voiced[0];
        // The first frame after which the pitch stays near the fundamental for a while.
        let settled = voiced.iter().position(|i| {
            voiced
                .iter()
                .filter(|j| **j >= *i && **j < *i + 4)
                .all(|j| keys[*j].is_some_and(|k| (k - key).abs() <= SETTLED_SEMITONES))
        });
        settle_s = settled.map_or(0.0, |p| (voiced[p] - first) as f32 * hop_s);
        start = start.clamp(-MAX_OFFSET_SEMITONES, MAX_OFFSET_SEMITONES);
    }
    (
        curve,
        Some(PitchMotion {
            fundamental_hz: 440.0 * exp2_det((key - 69.0) / 12.0),
            key,
            start_semitones: start,
            settle_s,
        }),
    )
}

/// One spectral peak.
#[derive(Debug, Clone, Copy)]
struct Peak {
    hz: f64,
    power: f64,
}

/// The mean power spectrum of three windows of [`SPECTRUM_FFT`] frames from `start`, scaled
/// so that a full-scale sine has the power 1.
fn power_spectrum(mono: &[f32], start: usize) -> Vec<f64> {
    let n = SPECTRUM_FFT;
    let fft = Fft::new(n);
    let window = hann(n);
    let energy: f64 = window.iter().map(|w| f64::from(*w) * f64::from(*w)).sum();
    let scale = 4.0 / (n as f64 * energy);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut power = vec![0.0f64; n / 2];
    for w in 0..SPECTRUM_WINDOWS {
        let from = start + w * n / 4;
        for (k, (r, win)) in re.iter_mut().zip(&window).enumerate() {
            *r = mono.get(from + k).map_or(0.0, |v| *v * win);
        }
        im.fill(0.0);
        fft.forward(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p += (f64::from(re[k]) * f64::from(re[k]) + f64::from(im[k]) * f64::from(im[k]))
                * scale
                / SPECTRUM_WINDOWS as f64;
        }
    }
    power
}

/// The peaks of `power` (module docs, step 5) and the share of the power that is not near one.
fn peaks_of(power: &[f64], rate: u32) -> (Vec<Peak>, f32) {
    let per_bin = f64::from(rate) / SPECTRUM_FFT as f64;
    let loudest = power.iter().skip(1).copied().fold(0.0f64, f64::max);
    let total: f64 = power.iter().skip(1).sum();
    if loudest <= 0.0 || total <= 0.0 {
        return (Vec::new(), 1.0);
    }
    let mut peaks = Vec::new();
    let mut used = vec![false; power.len()];
    let mut tonal = 0.0f64;
    let mut near = Vec::with_capacity(2 * MEDIAN_SPAN + 1);
    for k in 2..power.len() - 1 {
        let p = power[k];
        if p < loudest * PEAK_FLOOR || p < power[k - 1] || p <= power[k + 1] {
            continue;
        }
        near.clear();
        near.extend_from_slice(
            &power[k.saturating_sub(MEDIAN_SPAN)..(k + MEDIAN_SPAN + 1).min(power.len())],
        );
        near.sort_by(f64::total_cmp);
        if p < PEAK_PROMINENCE * near[near.len() / 2] {
            continue;
        }
        // Parabolic interpolation of the log power for the frequency.
        let (l, c, r) = (
            f64::from(log2_det(power[k - 1].max(1e-30) as f32)),
            f64::from(log2_det(p as f32)),
            f64::from(log2_det(power[k + 1].max(1e-30) as f32)),
        );
        let den = l - 2.0 * c + r;
        let shift = if den < -1e-9 {
            (0.5 * (l - r) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        peaks.push(Peak {
            hz: (k as f64 + shift) * per_bin,
            power: p,
        });
        for j in k.saturating_sub(LOBE)..=(k + LOBE).min(power.len() - 1) {
            if !used[j] {
                used[j] = true;
                tonal += power[j];
            }
        }
    }
    peaks.sort_by(|a, b| b.power.total_cmp(&a.power));
    (peaks, (1.0 - tonal / total).clamp(0.0, 1.0) as f32)
}

/// The harmonic profile (module docs, step 5).
fn harmonics_of(peaks: &[Peak], f0: Option<f32>, noise_share: f32, rate: u32) -> Harmonics {
    let Some(strongest) = peaks.first().copied() else {
        return Harmonics {
            kind: HarmonicKind::Noise,
            reference_hz: None,
            partials: Vec::new(),
            third_db: None,
        };
    };
    let base = f0.map_or(strongest.hz, f64::from);
    let level = |p: f64| {
        (10.0 * f64::from(log2_det((p / strongest.power) as f32)) * std::f64::consts::LOG10_2)
            as f32
    };
    let partials: Vec<Partial> = peaks
        .iter()
        .take(MAX_PARTIALS)
        .filter(|p| p.power >= strongest.power * STRONG_FLOOR)
        .map(|p| Partial {
            ratio: (p.hz / base) as f32,
            level_db: level(p.power),
        })
        .collect();
    let noisy = noise_share >= NOISE_FROM;
    let Some(f0) = f0.map(f64::from) else {
        return Harmonics {
            kind: if noisy {
                HarmonicKind::Noise
            } else {
                HarmonicKind::Inharmonic
            },
            reference_hz: Some(base as f32),
            partials,
            third_db: None,
        };
    };
    // The strongest peak near each multiple.
    let ceiling = 0.45 * f64::from(rate);
    let mut harmonic = [0.0f64; HARMONICS + 1];
    for p in peaks {
        let ratio = p.hz / f0;
        let h = ratio.round();
        if h >= 1.0 && (h as usize) <= HARMONICS && (ratio / h - 1.0).abs() <= HARMONIC_TOLERANCE {
            let h = h as usize;
            if h as f64 * f0 <= ceiling {
                harmonic[h] = harmonic[h].max(p.power);
            }
        }
    }
    let strong: Vec<&Peak> = peaks
        .iter()
        .filter(|p| p.power >= strongest.power * STRONG_FLOOR)
        .collect();
    let on_harmonics: f64 = strong
        .iter()
        .filter(|p| {
            let ratio = p.hz / f0;
            let h = ratio.round();
            h >= 1.0 && (ratio / h - 1.0).abs() <= HARMONIC_TOLERANCE
        })
        .map(|p| p.power)
        .sum();
    let all: f64 = strong.iter().map(|p| p.power).sum();
    let rest: f64 = harmonic[2..].iter().sum();
    let even = harmonic[2] + harmonic[4] + harmonic[6];
    let odd = harmonic[3] + harmonic[5] + harmonic[7];
    let kind = if noisy {
        HarmonicKind::Noise
    } else if strong.len() == 1 {
        // A single partial is a tone, even when a sweeping pitch put it off its multiple.
        HarmonicKind::Pure
    } else if all > 0.0 && on_harmonics / all < ALIGNED_MIN {
        HarmonicKind::Inharmonic
    } else if rest < PURE_BELOW * (harmonic[1] + rest) {
        HarmonicKind::Pure
    } else if odd > 0.0 && even * 10f64.powf(ODD_ONLY_DB / 10.0) <= odd {
        HarmonicKind::OddOnly
    } else {
        HarmonicKind::All
    };
    let third_db = (harmonic[1] > 0.0 && harmonic[3] > 0.0).then(|| {
        (10.0 * f64::from(log2_det((harmonic[3] / harmonic[1]) as f32)) * std::f64::consts::LOG10_2)
            as f32
    });
    if kind == HarmonicKind::Inharmonic {
        // The tracker's pitch means little for such a sound: the partials are measured against
        // the lowest strong one instead.
        let lowest = strong
            .iter()
            .filter(|p| p.power >= strongest.power * REFERENCE_FLOOR)
            .map(|p| p.hz)
            .fold(f64::INFINITY, f64::min);
        return Harmonics {
            kind,
            reference_hz: Some(lowest as f32),
            partials: partials
                .iter()
                .map(|p| Partial {
                    ratio: (f64::from(p.ratio) * base / lowest) as f32,
                    level_db: p.level_db,
                })
                .collect(),
            third_db: None,
        };
    }
    Harmonics {
        kind,
        reference_hz: Some(base as f32),
        partials,
        third_db,
    }
}

/// Analyses the audio `channels` (planar) at `sample_rate` (module docs).
///
/// # Errors
/// The take is silent, the sound is shorter than [`MIN_SECONDS`] or the rate is under 8 kHz.
pub fn analyse(channels: &[&[f32]], sample_rate: u32) -> Result<SoundProfile, AnalysisError> {
    if sample_rate < 8000 {
        return Err(AnalysisError::Rate);
    }
    let rate = sample_rate;
    let mono = mono_of(channels, (MAX_SECONDS * rate as f32) as usize);
    let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak < SILENT_PEAK {
        return Err(AnalysisError::Silent);
    }
    let hop = ((HOP_S * rate as f32).round() as usize).max(1);
    let hop_s = hop as f32 / rate as f32;
    let window = ((WINDOW_S * rate as f32).round() as usize).max(2);
    let rms = rms_frames(&mono, hop, window);
    let loudest = rms.iter().fold(0.0f32, |m, v| m.max(*v));
    if loudest <= 0.0 {
        return Err(AnalysisError::Silent);
    }
    let first = rms
        .iter()
        .position(|r| *r >= ONSET_LEVEL * loudest)
        .ok_or(AnalysisError::Silent)?;
    let last = rms
        .iter()
        .rposition(|r| *r >= END_LEVEL * loudest)
        .ok_or(AnalysisError::Silent)?;
    if last <= first || (last - first + 1) as f32 * hop_s < MIN_SECONDS {
        return Err(AnalysisError::TooShort);
    }
    let level: Vec<f32> = rms[first..=last].iter().map(|r| r / loudest).collect();
    let (envelope, top) = envelope_of(&level, hop_s);
    let centroid_hz = centroid_curve(&mono, rate, hop, first, &level);
    let brightness = brightness_of(&centroid_hz, &level, top, &envelope, hop_s);
    let (pitch_hz, pitch) = pitch_of(channels, rate, (hop, first), &level, hop_s);
    let spectrum_at = (first + top) * hop;
    let power = power_spectrum(&mono, spectrum_at.saturating_sub(hop));
    let (peaks, noise_share) = peaks_of(&power, rate);
    let harmonics = harmonics_of(&peaks, pitch.map(|p| p.fundamental_hz), noise_share, rate);
    // An inharmonic sound has no single fundamental to follow.
    let (pitch, pitch_hz) = if harmonics.kind == HarmonicKind::Inharmonic {
        (None, vec![0.0; level.len()])
    } else {
        (pitch, pitch_hz)
    };
    Ok(SoundProfile {
        sample_rate: rate,
        hop_s,
        lead_s: first as f32 * hop_s,
        duration_s: level.len() as f32 * hop_s,
        envelope,
        level,
        centroid_hz,
        pitch_hz,
        brightness,
        pitch,
        harmonics,
        noise_share,
    })
}

/// The value of `curve` (one entry per `hop_s`) at `seconds`, linearly interpolated; `None`
/// past its end.
fn value_at(curve: &[f32], hop_s: f32, seconds: f32) -> Option<f32> {
    let at = seconds / hop_s;
    let i = at.floor() as usize;
    let next = curve.get(i + 1)?;
    let here = curve.get(i)?;
    let frac = at - i as f32;
    Some(here + (next - here) * frac)
}

/// How far `candidate` is from `target` on the three curves (module docs). The curves are read
/// at the frames of the target, over the longer of the two sounds; a curve that has ended counts
/// as -40 dB.
pub fn compare(target: &SoundProfile, candidate: &SoundProfile) -> Distances {
    let seconds = target.duration_s.max(candidate.duration_s);
    let frames = (seconds / target.hop_s).ceil() as usize + 1;
    let floor_db = to_db(END_LEVEL);
    let db_at = |p: &SoundProfile, t: f32| {
        value_at(&p.level, p.hop_s, t).map_or(floor_db, |l| to_db(l).max(floor_db))
    };
    let (mut env_sum, mut bright_sum, mut pitch_sum) = (0.0f32, 0.0f32, 0.0f32);
    let (mut bright_n, mut pitch_n) = (0usize, 0usize);
    for i in 0..frames {
        let t = i as f32 * target.hop_s;
        env_sum += (db_at(target, t) - db_at(candidate, t)).abs();
        let (la, lb) = (
            value_at(&target.level, target.hop_s, t),
            value_at(&candidate.level, candidate.hop_s, t),
        );
        if la.zip(lb).is_some_and(|(a, b)| a.min(b) >= PITCH_GATE) {
            if let (Some(a), Some(b)) = (
                nearest(&target.centroid_hz, target.hop_s, t),
                nearest(&candidate.centroid_hz, candidate.hop_s, t),
            ) {
                bright_sum += (log2_det(a) - log2_det(b)).abs();
                bright_n += 1;
            }
            if let (Some(a), Some(b)) = (
                nearest(&target.pitch_hz, target.hop_s, t),
                nearest(&candidate.pitch_hz, candidate.hop_s, t),
            ) {
                pitch_sum += 12.0 * (log2_det(a) - log2_det(b)).abs();
                pitch_n += 1;
            }
        }
    }
    Distances {
        envelope_db: env_sum / frames.max(1) as f32,
        brightness_octaves: if bright_n > 0 {
            bright_sum / bright_n as f32
        } else {
            // Nothing to compare: as far as a sound can be.
            3.0
        },
        pitch_semitones: target.pitched().then(|| {
            if pitch_n > 0 {
                pitch_sum / pitch_n as f32
            } else {
                12.0
            }
        }),
    }
}

/// The defined (non-zero) value of `curve` nearest to `seconds`.
fn nearest(curve: &[f32], hop_s: f32, seconds: f32) -> Option<f32> {
    let v = *curve.get((seconds / hop_s).round() as usize)?;
    (v > 0.0).then_some(v)
}

#[cfg(test)]
mod tests;
