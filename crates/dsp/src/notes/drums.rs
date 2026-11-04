//! Drum audio to notes: the hits of a loop, each sorted into kick, snare, closed or open hat
//! (and, for a full kit, three toms and a crash).
//!
//! **Algorithm:**
//! 1. *Hits:* the onsets of [`crate::onset::detect_onsets`] at the given sensitivity.
//! 2. *Spectrum:* for each hit, the power spectrum of the sound that follows it: 30 ms from half
//!    a millisecond before the onset, under a falling half-Hann window (it keeps the attack, which
//!    a symmetric window would cut), cut short at the next hit (at least 6 ms are kept), zero
//!    padded to a power of two, radix-2 FFT of this crate.
//! 3. *Features:* the energy share of three bands, `low` (20 Hz to 250 Hz), `mid` (to 6 kHz) and
//!    `high` (above), and the zero-crossing rate of the first 6 ms.
//! 4. *Classes:* `low >= 0.30` with at least 1% of the energy above 6 kHz and three times as much
//!    there as in the mid band (the low sound carries a hat on top: a kick has little up there
//!    and what it has falls steeply) is a kick and a hat struck together (two notes); `high >= 0.5`,
//!    or `high >= 0.35` with a crossing rate of 0.25 or more, is a hat; `low >= 0.5` is a kick;
//!    anything else is a snare. A hat is **open** when the level stays above a tenth of its peak
//!    (measured on the high end alone, a second difference of the signal, so a kick ringing under
//!    the hat does not count) for 120 ms (or until the next hit, when that is 120 ms or more away)
//!    and `open_hats` is set; otherwise it is closed. With the [`DrumKit::Full`] kit a low hit
//!    is a tom when, 25 to 75 ms after the hit, its low band still centres above 95 Hz (a kick
//!    sweeps down to its fundamental by then and a tom holds its pitch): a low, mid or high tom by
//!    that centre; and a hat that rings for 600 ms is a crash.
//! 5. *Notes:* the key of each class on the [`DrumMap`] (General MIDI by default: [`KICK`],
//!    [`SNARE`], [`CLOSED_HAT`] and [`OPEN_HAT`], with [`LOW_TOM`], [`MID_TOM`], [`HIGH_TOM`]
//!    and [`CRASH`] for the full kit), a fixed length per class (cut where the next note of the
//!    same class starts), and a velocity from the peak of the first 10 ms in dB, relative to the
//!    loudest hit.

use super::{ExtractedNote, Levels, clamp_sensitivity, mono_mix, velocity_for};
use crate::det_trig::cos_cycles;
use crate::onset::detect_onsets;
use crate::stretch::fft::Fft;

/// General MIDI bass drum 1.
pub const KICK: u8 = 36;
/// General MIDI acoustic snare.
pub const SNARE: u8 = 38;
/// General MIDI closed hi-hat.
pub const CLOSED_HAT: u8 = 42;
/// General MIDI open hi-hat.
pub const OPEN_HAT: u8 = 46;
/// General MIDI low tom.
pub const LOW_TOM: u8 = 45;
/// General MIDI low-mid tom.
pub const MID_TOM: u8 = 47;
/// General MIDI high tom.
pub const HIGH_TOM: u8 = 50;
/// General MIDI crash cymbal 1.
pub const CRASH: u8 = 49;

/// Peak level below which a take is silent (about -80 dBFS).
const MIN_PEAK: f32 = 1.0e-4;
/// Length of the analysed sound, in milliseconds.
const WINDOW_MS: f64 = 30.0;
/// The shortest analysed sound, in milliseconds.
const MIN_WINDOW_MS: u64 = 6;
/// Top of the low band, in Hz.
const LOW_HZ: f64 = 250.0;
/// Bottom of the high band, in Hz.
const HIGH_HZ: f64 = 6000.0;
/// How long a hat must ring to be open, in milliseconds.
const OPEN_MS: u64 = 120;
/// How far the open-hat scan looks, in milliseconds.
const OPEN_SCAN_MS: u64 = 400;
/// How long a hat must ring to be a crash, in milliseconds.
const CRASH_MS: u64 = 600;
/// How far the scan looks when crashes are told apart, in milliseconds.
const CRASH_SCAN_MS: u64 = 1500;
/// The centroid of the low band in the tail of a hit, in Hz, from which a low hit is a tom.
const TOM_HZ: f64 = 95.0;
/// The centroids below which a tom is the low one, and the mid one, in Hz.
const TOM_SPLIT_HZ: (f64, f64) = (135.0, 185.0);
/// Where the tail of a hit that is read for toms starts and ends, in milliseconds after it.
const TAIL_MS: (u64, u64) = (25, 75);
/// The shortest tail that can tell a tom, in milliseconds.
const MIN_TAIL_MS: u64 = 20;

/// Which drums are told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrumKit {
    /// Kick, snare and hats.
    #[default]
    Basic,
    /// Also three toms and a crash.
    Full,
}

/// A kind of drum sound the extraction tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrumClass {
    /// Bass drum.
    Kick,
    /// Snare drum.
    Snare,
    /// Closed hi-hat.
    ClosedHat,
    /// Open hi-hat.
    OpenHat,
    /// Low tom (full kit).
    LowTom,
    /// Middle tom (full kit).
    MidTom,
    /// High tom (full kit).
    HighTom,
    /// Crash cymbal (full kit).
    Crash,
}

/// Which key each [`DrumClass`] is written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrumMap {
    /// The General MIDI drum keys: [`KICK`], [`SNARE`], [`CLOSED_HAT`], [`OPEN_HAT`] and the toms
    /// and crash above. On a 16-pad drum machine they are pads 1, 3, 7, 11, 10, 12, 15 and 14.
    #[default]
    GeneralMidi,
    /// The classes in a row from the key of the first pad of a drum machine (36): kick, snare,
    /// closed hat, open hat, low tom, mid tom, high tom, crash.
    Pads,
}

impl DrumMap {
    /// The key of `class`.
    pub fn key(self, class: DrumClass) -> u8 {
        match self {
            DrumMap::GeneralMidi => match class {
                DrumClass::Kick => KICK,
                DrumClass::Snare => SNARE,
                DrumClass::ClosedHat => CLOSED_HAT,
                DrumClass::OpenHat => OPEN_HAT,
                DrumClass::LowTom => LOW_TOM,
                DrumClass::MidTom => MID_TOM,
                DrumClass::HighTom => HIGH_TOM,
                DrumClass::Crash => CRASH,
            },
            DrumMap::Pads => KICK + class as u8,
        }
    }
}

/// What to extract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumParams {
    /// `0..=1`: more finds more (and softer) hits (NaN gives 0.5).
    pub sensitivity: f32,
    /// Tell open hats from closed ones (else every hat is closed).
    pub open_hats: bool,
    /// Which drums are told apart.
    pub kit: DrumKit,
    /// The keys the classes are written on.
    pub map: DrumMap,
}

impl Default for DrumParams {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            open_hats: true,
            kit: DrumKit::Basic,
            map: DrumMap::GeneralMidi,
        }
    }
}

/// The class of a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Kick,
    Snare,
    Hat,
    KickAndHat,
}

/// What a hit sounds like.
#[derive(Debug, Clone, Copy)]
struct Features {
    low: f64,
    mid: f64,
    high: f64,
    zcr: f64,
}

fn classify(f: &Features) -> Hit {
    if f.low >= 0.30 && f.high >= 0.01 && f.high >= 3.0 * f.mid {
        Hit::KickAndHat
    } else if f.high >= 0.5 || (f.high >= 0.35 && f.zcr >= 0.25) {
        Hit::Hat
    } else if f.low >= 0.5 {
        Hit::Kick
    } else {
        Hit::Snare
    }
}

/// The features of the sound at `onset`, `len` frames of it; `None` when it is silent.
fn features(
    mono: &[f32],
    onset: u64,
    len: usize,
    sr: u32,
    fft: &Fft,
    n: usize,
) -> Option<Features> {
    let from = (onset as usize).saturating_sub((sr as usize / 2000).max(1));
    let end = (from + len).min(mono.len());
    if end <= from + 1 {
        return None;
    }
    let seg = &mono[from..end];
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    let span = seg.len() as f64;
    for (i, v) in seg.iter().enumerate() {
        // Falling half-Hann: 1 at the onset, 0 at the end of the sound.
        let w = 0.5 + 0.5 * cos_cycles(0.5 * i as f64 / span);
        re[i] = f64::from(*v) * w;
    }
    fft.run(&mut re, &mut im, false);
    let hz = f64::from(sr) / n as f64;
    let (mut low, mut mid, mut high) = (0.0f64, 0.0f64, 0.0f64);
    for k in 1..n / 2 {
        let f = k as f64 * hz;
        if f < 20.0 {
            continue;
        }
        let p = re[k] * re[k] + im[k] * im[k];
        if f < LOW_HZ {
            low += p;
        } else if f < HIGH_HZ {
            mid += p;
        } else {
            high += p;
        }
    }
    let total = low + mid + high;
    if total <= 1e-18 {
        return None;
    }
    let short = &mono[from..(from + (sr as usize * 6 / 1000).max(2)).min(mono.len())];
    let crossings = short
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    Some(Features {
        low: low / total,
        mid: mid / total,
        high: high / total,
        zcr: crossings as f64 / short.len().max(2).saturating_sub(1) as f64,
    })
}

/// The drum notes of the loop `channels` (planar) at `sample_rate`, sorted by start then key. See
/// the module docs for the method; silence and noise without hits give none.
pub fn extract_drum_notes(
    channels: &[&[f32]],
    sample_rate: u32,
    params: &DrumParams,
) -> Vec<ExtractedNote> {
    if sample_rate < 8000 {
        return Vec::new();
    }
    let mono = mono_mix(channels);
    let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak < MIN_PEAK {
        return Vec::new();
    }
    let sr = u64::from(sample_rate);
    let ms = |v: u64| (v * sr / 1000).max(1);
    let onsets = detect_onsets(&[&mono], sample_rate, clamp_sensitivity(params.sensitivity));
    if onsets.is_empty() {
        return Vec::new();
    }
    // The level of the high end alone (a second difference), so that a low sound that rings
    // under a hat does not make it open.
    let high_pass: Vec<f32> = (0..mono.len())
        .map(|i| {
            let (a, b) = (mono[i.saturating_sub(1)], mono[i.saturating_sub(2)]);
            mono[i] - 2.0 * a + b
        })
        .collect();
    let levels = Levels::new(&high_pass, sample_rate);
    let n = ((WINDOW_MS * f64::from(sample_rate) / 1000.0).round() as usize)
        .next_power_of_two()
        .clamp(256, 4096);
    let fft = Fft::new(n);
    let peak_of = |o: u64| {
        let from = o as usize;
        let to = (from + ms(10) as usize).min(mono.len());
        mono[from.min(to)..to]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    // (start, class, peak) of every hit, before lengths.
    let mut hits: Vec<(u64, DrumClass, f32)> = Vec::new();
    for (i, &o) in onsets.iter().enumerate() {
        let next = onsets.get(i + 1).copied();
        let room = next.map_or(u64::MAX, |nx| nx - o);
        let len = (n as u64)
            .min(room.saturating_sub(ms(1)).max(ms(MIN_WINDOW_MS)))
            .max(2) as usize;
        let Some(f) = features(&mono, o, len, sample_rate, &fft, n) else {
            continue;
        };
        let level = peak_of(o);
        if level <= 0.0 {
            continue;
        }
        let hat = || hat_class(o, next, &levels, params, sr);
        match classify(&f) {
            Hit::Kick => {
                let class = if params.kit == DrumKit::Full {
                    low_class(&mono, o, room, sample_rate, level)
                } else {
                    DrumClass::Kick
                };
                hits.push((o, class, level));
            }
            Hit::Snare => hits.push((o, DrumClass::Snare, level)),
            Hit::Hat => hits.push((o, hat(), level)),
            Hit::KickAndHat => {
                hits.push((o, DrumClass::Kick, level));
                hits.push((o, hat(), level));
            }
        }
    }
    let reference = hits.iter().fold(0.0f32, |m, h| m.max(h.2));
    hits.sort_by_key(|h| (h.0, h.1));
    let fixed = |class: DrumClass| match class {
        DrumClass::Kick | DrumClass::Snare => ms(120),
        DrumClass::LowTom | DrumClass::MidTom | DrumClass::HighTom => ms(200),
        DrumClass::OpenHat => ms(300),
        DrumClass::Crash => ms(800),
        DrumClass::ClosedHat => ms(60),
    };
    (0..hits.len())
        .map(|i| {
            let (start, class, level) = hits[i];
            let cut = hits[i + 1..]
                .iter()
                .find(|h| h.1 == class && h.0 > start)
                .map_or(u64::MAX, |h| h.0 - start);
            ExtractedNote {
                start,
                length: fixed(class).min(cut).max(1),
                key: params.map.key(class),
                velocity: velocity_for(level, reference),
                level,
                bend: Vec::new(),
            }
        })
        .collect()
}

/// What a hat struck at `onset` is: a crash when it rings for [`CRASH_MS`] and the kit has one,
/// else open when it rings for [`OPEN_MS`] and `open_hats` is set, else closed (module docs).
fn hat_class(
    onset: u64,
    next: Option<u64>,
    levels: &Levels,
    params: &DrumParams,
    sr: u64,
) -> DrumClass {
    let crash = params.kit == DrumKit::Full;
    if !params.open_hats && !crash {
        return DrumClass::ClosedHat;
    }
    let ms = |v: u64| (v * sr / 1000).max(1);
    let block = levels.block() as u64;
    let scan = if crash { CRASH_SCAN_MS } else { OPEN_SCAN_MS };
    let end = onset + ms(scan);
    let stop = next.map_or(end, |nx| nx.min(end));
    let mut peak = 0.0f32;
    let mut p = onset;
    while p < onset + ms(10) {
        peak = peak.max(levels.rms_at(p));
        p += block;
    }
    if peak <= 0.0 {
        return DrumClass::ClosedHat;
    }
    // How long it rings: until the level falls to a tenth of the peak, else to the end of the
    // scan or the next hit.
    let mut p = onset + ms(10);
    let mut rang = stop.saturating_sub(onset);
    while p < stop {
        if levels.rms_at(p) < 0.1 * peak {
            rang = p - onset;
            break;
        }
        p += block;
    }
    if crash && rang >= ms(CRASH_MS) {
        DrumClass::Crash
    } else if params.open_hats && rang >= ms(OPEN_MS) {
        DrumClass::OpenHat
    } else {
        DrumClass::ClosedHat
    }
}

/// A low hit at `onset` with `room` frames before the next hit: a tom when the tail of the hit
/// still centres above [`TOM_HZ`] (module docs), else a kick. A hit too short to read, or whose
/// tail has died away, is a kick.
fn low_class(mono: &[f32], onset: u64, room: u64, sr: u32, level: f32) -> DrumClass {
    let ms = |v: u64| (v * u64::from(sr) / 1000).max(1);
    let from = (onset + ms(TAIL_MS.0)) as usize;
    let end = (onset + ms(TAIL_MS.1).min(room.saturating_sub(ms(1)))).min(mono.len() as u64);
    if (end as usize) < from + ms(MIN_TAIL_MS) as usize {
        return DrumClass::Kick;
    }
    let tail = &mono[from..end as usize];
    let rms = (tail
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        / tail.len() as f64)
        .sqrt() as f32;
    if rms < 0.02 * level {
        return DrumClass::Kick;
    }
    let n = tail.len().next_power_of_two();
    let span = tail.len() as f64;
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    for (i, v) in tail.iter().enumerate() {
        // Hann: the tail starts and ends in the middle of the sound.
        let w = 0.5 - 0.5 * cos_cycles(i as f64 / span);
        re[i] = f64::from(*v) * w;
    }
    Fft::new(n).run(&mut re, &mut im, false);
    let hz = f64::from(sr) / n as f64;
    let (mut power, mut moment) = (0.0f64, 0.0f64);
    for k in 1..n / 2 {
        let f = k as f64 * hz;
        if f < 20.0 {
            continue;
        }
        if f >= LOW_HZ {
            break;
        }
        let p = re[k] * re[k] + im[k] * im[k];
        power += p;
        moment += p * f;
    }
    if power <= 1e-18 {
        return DrumClass::Kick;
    }
    let centre = moment / power;
    if centre < TOM_HZ {
        DrumClass::Kick
    } else if centre < TOM_SPLIT_HZ.0 {
        DrumClass::LowTom
    } else if centre < TOM_SPLIT_HZ.1 {
        DrumClass::MidTom
    } else {
        DrumClass::HighTom
    }
}
