//! Drum audio to notes: the hits of a loop, each sorted into kick, snare, closed or open hat.
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
//!    and what it has falls steeply) is a kick and a hat struck together (two notes); `high >= 0.5`, or `high >= 0.35` with a crossing rate of 0.25 or more,
//!    is a hat; `low >= 0.5` is a kick; anything else is a snare. A hat is **open** when the level
//!    stays above a tenth of its peak (measured on the high end alone, a second difference of
//!    the signal, so a kick ringing under the hat does not count) for 120 ms (or until the next hit, when that is 120 ms or
//!    more away) and `open_hats` is set; otherwise it is closed.
//! 5. *Notes:* General MIDI keys [`KICK`], [`SNARE`], [`CLOSED_HAT`] and [`OPEN_HAT`], a fixed
//!    length per class (cut where the next note of the same key starts), and a velocity from the
//!    peak of the first 10 ms in dB, relative to the loudest hit.

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

/// What to extract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumParams {
    /// `0..=1`: more finds more (and softer) hits (NaN gives 0.5).
    pub sensitivity: f32,
    /// Tell open hats from closed ones (else every hat is closed).
    pub open_hats: bool,
}

impl Default for DrumParams {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            open_hats: true,
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
    // (start, key, peak) of every hit, before lengths.
    let mut hits: Vec<(u64, u8, f32)> = Vec::new();
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
        match classify(&f) {
            Hit::Kick => hits.push((o, KICK, level)),
            Hit::Snare => hits.push((o, SNARE, level)),
            Hit::Hat => hits.push((o, hat_key(o, next, &levels, params.open_hats, sr), level)),
            Hit::KickAndHat => {
                hits.push((o, KICK, level));
                hits.push((o, hat_key(o, next, &levels, params.open_hats, sr), level));
            }
        }
    }
    let reference = hits.iter().fold(0.0f32, |m, h| m.max(h.2));
    hits.sort_by_key(|h| (h.0, h.1));
    let fixed = |key: u8| match key {
        KICK | SNARE => ms(120),
        OPEN_HAT => ms(300),
        _ => ms(60),
    };
    (0..hits.len())
        .map(|i| {
            let (start, key, level) = hits[i];
            let cut = hits[i + 1..]
                .iter()
                .find(|h| h.1 == key && h.0 > start)
                .map_or(u64::MAX, |h| h.0 - start);
            ExtractedNote {
                start,
                length: fixed(key).min(cut).max(1),
                key,
                velocity: velocity_for(level, reference),
            }
        })
        .collect()
}

/// [`OPEN_HAT`] when the hat struck at `onset` rings (see the module docs), else [`CLOSED_HAT`].
fn hat_key(onset: u64, next: Option<u64>, levels: &Levels, open_hats: bool, sr: u64) -> u8 {
    if !open_hats {
        return CLOSED_HAT;
    }
    let ms = |v: u64| (v * sr / 1000).max(1);
    let block = levels.block() as u64;
    let end = onset + ms(OPEN_SCAN_MS);
    let stop = next.map_or(end, |nx| nx.min(end));
    let mut peak = 0.0f32;
    let mut p = onset;
    while p < onset + ms(10) {
        peak = peak.max(levels.rms_at(p));
        p += block;
    }
    if peak <= 0.0 {
        return CLOSED_HAT;
    }
    let mut p = onset + ms(10);
    while p < stop {
        if levels.rms_at(p) < 0.1 * peak {
            return if p - onset >= ms(OPEN_MS) {
                OPEN_HAT
            } else {
                CLOSED_HAT
            };
        }
        p += block;
    }
    // Still ringing at the next hit (or after the scan).
    if stop - onset >= ms(OPEN_MS) {
        OPEN_HAT
    } else {
        CLOSED_HAT
    }
}
