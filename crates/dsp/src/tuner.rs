//! The tuner: the pitch of a monophonic signal (a guitar or a bass), its nearest note and the
//! deviation in cents, with the smoothing and the hold a display needs.
//!
//! It runs on the UI side from the newest frames of a scope tap, never on the audio thread.
//!
//! **Detection** ([`detect`]), in two stages so that it is cheap and accurate at once:
//! 1. *Coarse:* the signal is averaged down to about 12 kHz and a YIN tracker (de Cheveigne and
//!    Kawahara, 2002: the squared difference function, its cumulative mean normalisation, the
//!    first dip below a threshold walked down to its minimum, parabolic interpolation) finds the
//!    period. A window of one longest period keeps the work in the hundreds of thousands of
//!    operations.
//! 2. *Fine:* at the full rate, the normalised square difference (McLeod and Wyvill, 2005) is
//!    evaluated for the few lags around the coarse period over a window of several periods; its
//!    peak, interpolated with a parabola, gives the period to a small fraction of a sample.
//!
//! A frame whose RMS is below [`GATE`] is silent and gives no pitch, and so is one with no clear
//! period (noise, a chord).
//!
//! **Display** ([`Tuner`]): readings are smoothed in the pitch domain (a one-pole filter over the
//! elapsed frames, so it does not depend on how often the display asks), a jump of more than half
//! a semitone replaces the smoothed pitch at once (a new note), and the last note is held for
//! [`HOLD_SECONDS`] after the signal decays, flagged as held.

/// Lowest pitch looked for, Hz (a little under the open low B of a five-string bass, 30.9 Hz).
pub const MIN_HZ: f32 = 28.0;
/// Highest pitch looked for, Hz (above the E6 of a guitar's 24th fret, 1318.5 Hz).
pub const MAX_HZ: f32 = 1600.0;
/// RMS below which a frame is silent (about -56 dBFS).
pub const GATE: f32 = 0.0016;
/// The reference pitch of A4, Hz, and the range the setting offers.
pub const REF_DEFAULT_HZ: f32 = 440.0;
/// Lowest reference pitch, Hz.
pub const REF_MIN_HZ: f32 = 415.0;
/// Highest reference pitch, Hz.
pub const REF_MAX_HZ: f32 = 466.0;
/// A note this close to in tune (cents, either way) counts as in tune.
pub const IN_TUNE_CENTS: f32 = 3.0;
/// How long the last note stays on the display after the signal is gone.
pub const HOLD_SECONDS: f32 = 0.8;
/// Time constant of the smoothing of the pitch.
pub const SMOOTH_SECONDS: f32 = 0.07;

/// Rate the coarse stage works at (the signal is averaged down to about this).
const COARSE_HZ: f32 = 12_000.0;
/// The YIN threshold on the normalised difference.
const YIN_THRESHOLD: f32 = 0.15;
/// A frame with no dip below the threshold is still voiced when its best dip is below this.
const YIN_FALLBACK: f32 = 0.30;
/// Frames the gate looks at.
const GATE_FRAMES: usize = 2048;
/// The fine stage needs at least this much normalised correlation.
const MIN_CLARITY: f32 = 0.5;

/// Frames of the newest signal [`detect`] wants at `sample_rate` (8192 at 48 kHz): enough for
/// the longest period plus the windows of both stages.
pub fn frames_wanted(sample_rate: f32) -> usize {
    (8192.0 * sample_rate / 48_000.0).round().max(2048.0) as usize
}

/// A detected pitch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    /// Frequency in Hz.
    pub hz: f32,
    /// How periodic the frame is, 0 to 1 (the normalised correlation at the period).
    pub clarity: f32,
}

/// The nearest note of a frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// MIDI key (A4 = 69).
    pub key: i32,
    /// Deviation of the frequency from the note, cents, `-50..=50`.
    pub cents: f32,
}

/// The nearest note of `hz` with A4 at `ref_hz`, and how far off it is.
pub fn note_of(hz: f32, ref_hz: f32) -> Note {
    let key = 69.0 + 12.0 * (f64::from(hz) / f64::from(ref_hz)).log2();
    let nearest = key.round();
    Note {
        key: nearest as i32,
        cents: ((key - nearest) * 100.0) as f32,
    }
}

/// Frequency of a MIDI key with A4 at `ref_hz`.
pub fn key_hz(key: i32, ref_hz: f32) -> f32 {
    (f64::from(ref_hz) * (f64::from(key - 69) / 12.0).exp2()) as f32
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The name of a MIDI key without its octave (`"C#"`).
pub fn key_name(key: i32) -> &'static str {
    NAMES[key.rem_euclid(12) as usize]
}

/// The octave of a MIDI key in scientific pitch notation (key 60 is C4, 69 is A4).
pub fn key_octave(key: i32) -> i32 {
    key.div_euclid(12) - 1
}

/// The root-mean-square of `x`.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let sum: f64 = x.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / x.len() as f64).sqrt() as f32
}

/// The pitch of the newest frames of a mono signal `x` (oldest first), `None` for a silent or
/// aperiodic frame. `x` should hold [`frames_wanted`] frames; with fewer, the lowest pitches are
/// not found.
pub fn detect(x: &[f32], sample_rate: f32) -> Option<Pitch> {
    if x.is_empty() || sample_rate <= 0.0 || !x.iter().all(|s| s.is_finite()) {
        return None;
    }
    if rms(&x[x.len().saturating_sub(GATE_FRAMES)..]) < GATE {
        return None;
    }
    let coarse = coarse_period(x, sample_rate)?;
    let (period, clarity) = refine(x, coarse.0, coarse.1)?;
    Some(Pitch {
        hz: (f64::from(sample_rate) / period) as f32,
        clarity,
    })
}

/// Stage 1: `(period in frames at the full rate, decimation)`.
fn coarse_period(x: &[f32], sample_rate: f32) -> Option<(f64, usize)> {
    let d = ((sample_rate / COARSE_HZ).round() as usize).max(1);
    let rate = f64::from(sample_rate) / d as f64;
    let m = x.len() / d;
    let tau_max = (rate / f64::from(MIN_HZ)).ceil() as usize;
    let tau_min = ((rate / f64::from(MAX_HZ)).floor() as usize).max(2);
    let w = tau_max;
    if m < w + tau_max + 1 {
        return None;
    }
    // Average down, newest frames, aligned to the end.
    let start = x.len() - m * d;
    let y: Vec<f64> = (0..m)
        .map(|i| {
            let s = &x[start + i * d..start + (i + 1) * d];
            s.iter().map(|v| f64::from(*v)).sum::<f64>() / d as f64
        })
        .collect();
    let y = &y[m - (w + tau_max + 1)..];
    // The squared difference function and its cumulative mean normalisation.
    let mut cmnd = vec![1.0f64; tau_max + 1];
    let mut running = 0.0;
    for tau in 1..=tau_max {
        let mut dsum = 0.0;
        for j in 0..w {
            let e = y[j] - y[j + tau];
            dsum += e * e;
        }
        running += dsum;
        cmnd[tau] = if running > 0.0 {
            dsum * tau as f64 / running
        } else {
            1.0
        };
    }
    let threshold = f64::from(YIN_THRESHOLD);
    let mut pick = None;
    let mut tau = tau_min;
    while tau < tau_max {
        if cmnd[tau] < threshold {
            while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            pick = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = match pick {
        Some(t) => t,
        None => {
            let best = (tau_min..tau_max).min_by(|a, b| cmnd[*a].total_cmp(&cmnd[*b]))?;
            if cmnd[best] > f64::from(YIN_FALLBACK) {
                return None;
            }
            best
        }
    };
    let delta = if tau > 1 && tau < tau_max {
        parabola(cmnd[tau - 1], cmnd[tau], cmnd[tau + 1], true)
    } else {
        0.0
    };
    Some(((tau as f64 + delta) * d as f64, d))
}

/// Stage 2: the period at the full rate and its clarity.
fn refine(x: &[f32], coarse: f64, d: usize) -> Option<(f64, f32)> {
    let slack = d as f64 + 2.0;
    let lo = ((coarse - slack).floor() as usize).max(2);
    let hi = (coarse + slack).ceil() as usize;
    let window = ((3.0 * coarse) as usize).max(2048);
    if x.len() < hi + 2 {
        return None;
    }
    let window = window.min(x.len() - hi - 1);
    if window < 64 {
        return None;
    }
    let base = x.len() - window - hi - 1;
    let nsdf = |tau: usize| -> f64 {
        let (mut acf, mut energy) = (0.0f64, 0.0f64);
        for j in 0..window {
            let (a, b) = (f64::from(x[base + j]), f64::from(x[base + j + tau]));
            acf += a * b;
            energy += a * a + b * b;
        }
        if energy > 0.0 {
            2.0 * acf / energy
        } else {
            0.0
        }
    };
    let r: Vec<f64> = (lo..=hi).map(nsdf).collect();
    let best = (0..r.len()).max_by(|a, b| r[*a].total_cmp(&r[*b]))?;
    if best == 0 || best + 1 == r.len() {
        return None;
    }
    let clarity = r[best] as f32;
    if clarity < MIN_CLARITY {
        return None;
    }
    let delta = parabola(r[best - 1], r[best], r[best + 1], false);
    Some(((lo + best) as f64 + delta, clarity.min(1.0)))
}

/// The offset of the extremum of the parabola through three equally spaced points (`minimum`
/// says which kind), in `-0.5..=0.5`.
fn parabola(a: f64, b: f64, c: f64, minimum: bool) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-12 {
        return 0.0;
    }
    let d = 0.5 * (a - c) / denom;
    // For a minimum the denominator is positive, for a maximum negative; anything else is not a
    // peak of the expected kind.
    if (minimum && denom < 0.0) || (!minimum && denom > 0.0) {
        return 0.0;
    }
    d.clamp(-0.5, 0.5)
}

/// What the tuner display shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The smoothed frequency, Hz.
    pub hz: f32,
    /// The nearest note of it (`cents` is smoothed with the frequency).
    pub note: Note,
    /// The signal is gone and this is the last note it had.
    pub held: bool,
}

impl Reading {
    /// The note is within [`IN_TUNE_CENTS`] of in tune.
    pub fn in_tune(&self) -> bool {
        self.note.cents.abs() <= IN_TUNE_CENTS
    }
}

/// The tuner's state between display frames (module docs).
#[derive(Debug, Clone, Default)]
pub struct Tuner {
    /// The smoothed pitch as a fractional MIDI key with A4 at the reference of the last update.
    smoothed: Option<f64>,
    /// Frames since the last voiced frame.
    silent_frames: u64,
    /// The reference the smoothed pitch is relative to.
    ref_hz: f32,
}

impl Tuner {
    /// A tuner showing nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the note on the display.
    pub fn clear(&mut self) {
        self.smoothed = None;
        self.silent_frames = 0;
    }

    /// Folds one analysis into the display. `x` is the newest mono signal ([`detect`]),
    /// `elapsed` the frames since the previous call (0 repeats the last reading) and `ref_hz`
    /// the reference pitch of A4. Returns what to show, `None` for nothing.
    pub fn update(
        &mut self,
        x: &[f32],
        sample_rate: f32,
        elapsed: u64,
        ref_hz: f32,
    ) -> Option<Reading> {
        let ref_hz = ref_hz.clamp(REF_MIN_HZ, REF_MAX_HZ);
        if (ref_hz - self.ref_hz).abs() > f32::EPSILON {
            // A new reference moves every note: start again rather than glide.
            self.smoothed = None;
            self.ref_hz = ref_hz;
        }
        match detect(x, sample_rate) {
            Some(p) => {
                self.silent_frames = 0;
                let key = 69.0 + 12.0 * (f64::from(p.hz) / f64::from(ref_hz)).log2();
                self.smoothed = Some(match self.smoothed {
                    Some(s) if (key - s).abs() <= 0.5 => {
                        let tau = f64::from(SMOOTH_SECONDS) * f64::from(sample_rate);
                        let alpha = 1.0 - (-(elapsed as f64) / tau).exp();
                        s + (key - s) * alpha
                    }
                    _ => key,
                });
            }
            None => {
                self.silent_frames = self.silent_frames.saturating_add(elapsed);
            }
        }
        let s = self.smoothed?;
        let hold = (f64::from(HOLD_SECONDS) * f64::from(sample_rate)) as u64;
        if self.silent_frames > hold {
            self.smoothed = None;
            return None;
        }
        let nearest = s.round();
        Some(Reading {
            hz: (f64::from(ref_hz) * ((s - 69.0) / 12.0).exp2()) as f32,
            note: Note {
                key: nearest as i32,
                cents: ((s - nearest) * 100.0) as f32,
            },
            held: self.silent_frames > 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use std::f64::consts::TAU;

    const SR: f32 = 48_000.0;

    fn sine(hz: f64, amp: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (TAU * hz * i as f64 / f64::from(sr)).sin() as f32)
            .collect()
    }

    /// A plucked-string stand-in: partials with a strong second harmonic and a weaker
    /// fundamental, a little inharmonicity, each decaying faster the higher it is.
    fn pluck(hz: f64, sr: f32, frames: usize) -> Vec<f32> {
        let partials: [(f64, f64, f64); 5] = [
            (1.0, 0.55, 1.2),
            (2.0, 1.0, 1.6),
            (3.0, 0.7, 2.4),
            (4.0, 0.4, 3.2),
            (5.0, 0.25, 4.0),
        ];
        (0..frames)
            .map(|i| {
                let t = i as f64 / f64::from(sr);
                partials
                    .iter()
                    .map(|(k, a, decay)| {
                        let f = hz * k * (1.0 + 0.0002 * k * k);
                        a * (-decay * t).exp() * (TAU * f * t + k).sin()
                    })
                    .sum::<f64>() as f32
                    * 0.3
            })
            .collect()
    }

    /// Karplus-Strong with a delay of `len` frames. The two-point average reads one frame ahead of
    /// the one it writes, so the loop is half a frame short: the pitch is `sr / (len - 0.5)`.
    fn karplus(len: usize, frames: usize) -> Vec<f32> {
        let mut rng = Rng::new(7);
        let mut line: Vec<f32> = (0..len).map(|_| rng.next_bipolar() * 0.5).collect();
        let mut out = Vec::with_capacity(frames);
        let mut at = 0;
        for _ in 0..frames {
            let next = (at + 1) % len;
            let v = 0.5 * (line[at] + line[next]) * 0.9995;
            out.push(line[at]);
            line[at] = v;
            at = next;
        }
        out
    }

    fn cents_between(a: f32, b: f32) -> f32 {
        (1200.0 * (f64::from(a) / f64::from(b)).log2()) as f32
    }

    fn newest(x: &[f32], sr: f32) -> &[f32] {
        &x[x.len() - frames_wanted(sr)..]
    }

    #[test]
    fn a_sine_is_found_within_a_cent_from_b0_to_e6() {
        // B0 is 30.87 Hz, E1 41.2, A2 110, E4 329.6, E6 1318.5.
        for hz in [
            30.87, 41.2, 55.0, 82.41, 110.0, 196.0, 329.63, 440.0, 880.0, 1318.51,
        ] {
            for sr in [44_100.0f32, 48_000.0, 96_000.0] {
                let x = sine(hz, 0.4, sr, frames_wanted(sr));
                let p = detect(&x, sr).unwrap_or_else(|| panic!("{hz} Hz at {sr}: no pitch"));
                let off = cents_between(p.hz, hz as f32);
                assert!(off.abs() < 1.0, "{hz} Hz at {sr}: {off} cents ({})", p.hz);
                assert!(p.clarity > 0.99, "{hz} Hz: clarity {}", p.clarity);
            }
        }
    }

    #[test]
    fn a_plucked_string_is_found_at_its_fundamental() {
        for hz in [41.2, 82.41, 110.0, 146.83, 246.94, 329.63, 659.26, 1318.51] {
            let x = pluck(hz, SR, 48_000);
            // Early (bright) and late (decayed) frames.
            for end in [20_000usize, 30_000, 44_000] {
                let frames = &x[end - frames_wanted(SR)..end];
                let p = detect(frames, SR).unwrap_or_else(|| panic!("{hz} Hz at {end}"));
                let off = cents_between(p.hz, hz as f32);
                assert!(off.abs() < 3.0, "{hz} Hz at {end}: {off} cents");
            }
        }
    }

    #[test]
    fn karplus_strong_strings_are_found() {
        for len in [1166usize, 582, 436, 291, 145, 72, 36] {
            let x = karplus(len, 60_000);
            let frames = newest(&x, SR);
            let want = SR / (len as f32 - 0.5);
            let p = detect(frames, SR).unwrap_or_else(|| panic!("delay {len}: no pitch"));
            let off = cents_between(p.hz, want);
            assert!(
                off.abs() < 6.0,
                "delay {len}: {off} cents ({} vs {want})",
                p.hz
            );
        }
    }

    #[test]
    fn silence_and_a_quiet_hiss_give_no_pitch() {
        assert_eq!(detect(&vec![0.0; 8192], SR), None);
        assert_eq!(detect(&sine(110.0, 0.0005, SR, 8192), SR), None);
        let mut rng = Rng::new(3);
        let hiss: Vec<f32> = (0..8192).map(|_| rng.next_bipolar() * 0.3).collect();
        assert_eq!(detect(&hiss, SR), None, "white noise has no pitch");
        assert_eq!(detect(&[], SR), None);
        assert_eq!(detect(&[f32::NAN; 8192], SR), None);
    }

    #[test]
    fn a_short_buffer_does_not_panic() {
        for n in [1usize, 10, 500, 2047, 4000] {
            let _ = detect(&sine(220.0, 0.4, SR, n), SR);
        }
    }

    #[test]
    fn notes_and_cents_follow_the_reference() {
        let n = note_of(440.0, 440.0);
        assert_eq!((n.key, n.cents.round() as i32), (69, 0));
        let n = note_of(82.41, 440.0);
        assert_eq!(n.key, 40);
        assert!(n.cents.abs() < 0.5, "{}", n.cents);
        // 20 cents sharp of A4, and flat.
        let up = 440.0 * (20.0f64 / 1200.0).exp2() as f32;
        assert!((note_of(up, 440.0).cents - 20.0).abs() < 0.01);
        let down = 440.0 * (-20.0f64 / 1200.0).exp2() as f32;
        assert!((note_of(down, 440.0).cents + 20.0).abs() < 0.01);
        // A reference of 432 Hz: 432 Hz is A4 in tune, 440 Hz is about 31.8 cents sharp.
        let n = note_of(432.0, 432.0);
        assert_eq!((n.key, n.cents.round() as i32), (69, 0));
        let n = note_of(440.0, 432.0);
        assert_eq!(n.key, 69);
        assert!((n.cents - 31.77).abs() < 0.05, "{}", n.cents);
        // The halfway point goes to the nearer side and cents stay within 50.
        for hz in (30..1400).map(|h| h as f32 * 1.37) {
            let n = note_of(hz, 440.0);
            assert!(n.cents.abs() <= 50.0 + 1e-3, "{hz}: {}", n.cents);
            assert!((cents_between(hz, key_hz(n.key, 440.0)) - n.cents).abs() < 0.01);
        }
        assert!((key_hz(69, 415.0) - 415.0).abs() < 1e-3);
        assert!((key_hz(57, 466.0) - 233.0).abs() < 1e-2);
    }

    #[test]
    fn names_and_octaves() {
        assert_eq!((key_name(69), key_octave(69)), ("A", 4));
        assert_eq!((key_name(60), key_octave(60)), ("C", 4));
        assert_eq!((key_name(40), key_octave(40)), ("E", 2));
        assert_eq!((key_name(23), key_octave(23)), ("B", 0));
        assert_eq!((key_name(61), key_octave(61)), ("C#", 4));
        assert_eq!((key_name(12), key_octave(12)), ("C", 0));
        assert_eq!((key_name(11), key_octave(11)), ("B", -1));
    }

    #[test]
    fn the_reference_pitch_moves_the_reading() {
        let x = sine(440.0, 0.4, SR, frames_wanted(SR));
        let mut t = Tuner::new();
        let r = t.update(&x, SR, 800, 440.0).unwrap();
        assert_eq!(r.note.key, 69);
        assert!(r.note.cents.abs() < 1.0 && r.in_tune());
        let r = t.update(&x, SR, 800, 432.0).unwrap();
        assert_eq!(r.note.key, 69);
        assert!((r.note.cents - 31.8).abs() < 1.0, "{}", r.note.cents);
        assert!(!r.in_tune());
        // Out of range references are clamped.
        let r = t.update(&x, SR, 800, 380.0).unwrap();
        assert!((r.note.cents - cents_between(440.0, 415.0) % 100.0).abs() < 60.0);
    }

    #[test]
    fn the_needle_is_smoothed_and_a_new_note_replaces_it_at_once() {
        let a = sine(440.0, 0.4, SR, frames_wanted(SR));
        let sharp = sine(
            440.0 * (10.0f64 / 1200.0).exp2(),
            0.4,
            SR,
            frames_wanted(SR),
        );
        let mut t = Tuner::new();
        t.update(&a, SR, 800, 440.0).unwrap();
        let r = t.update(&sharp, SR, 800, 440.0).unwrap();
        assert!(r.note.cents > 0.5 && r.note.cents < 9.0, "{}", r.note.cents);
        let mut last = r.note.cents;
        for _ in 0..40 {
            last = t.update(&sharp, SR, 800, 440.0).unwrap().note.cents;
        }
        assert!((last - 10.0).abs() < 0.5, "{last}");
        // A different string: no glide through the notes between.
        let low = sine(110.0, 0.4, SR, frames_wanted(SR));
        let r = t.update(&low, SR, 800, 440.0).unwrap();
        assert_eq!(r.note.key, 45);
        assert!(r.note.cents.abs() < 1.0, "{}", r.note.cents);
    }

    #[test]
    fn the_last_note_is_held_while_the_signal_decays_then_cleared() {
        let a = sine(110.0, 0.4, SR, frames_wanted(SR));
        let silence = vec![0.0f32; frames_wanted(SR)];
        let mut t = Tuner::new();
        assert!(
            t.update(&silence, SR, 800, 440.0).is_none(),
            "nothing before a note"
        );
        let live = t.update(&a, SR, 800, 440.0).unwrap();
        assert!(!live.held);
        let held = t.update(&silence, SR, 800, 440.0).unwrap();
        assert!(held.held);
        assert_eq!(held.note.key, live.note.key);
        assert_eq!(held.hz.to_bits(), live.hz.to_bits());
        // Just under the hold: still there; past it: gone.
        let frames = (HOLD_SECONDS * SR) as u64;
        assert!(t.update(&silence, SR, frames - 2000, 440.0).is_some());
        assert!(t.update(&silence, SR, 2500, 440.0).is_none());
        assert!(t.update(&silence, SR, 800, 440.0).is_none());
        // A new note after the hold starts fresh.
        assert!(t.update(&a, SR, 800, 440.0).is_some_and(|r| !r.held));
    }

    #[test]
    fn clear_forgets_the_note() {
        let a = sine(110.0, 0.4, SR, frames_wanted(SR));
        let mut t = Tuner::new();
        t.update(&a, SR, 800, 440.0).unwrap();
        t.clear();
        assert!(t.update(&vec![0.0; 8192], SR, 800, 440.0).is_none());
    }

    #[test]
    fn frames_wanted_scales_with_the_rate() {
        assert_eq!(frames_wanted(48_000.0), 8192);
        assert_eq!(frames_wanted(96_000.0), 16_384);
        assert!(frames_wanted(44_100.0) > 7000);
    }

    #[test]
    fn the_cost_is_modest() {
        // A rough bound on the work rather than a timing: the YIN stage is `window x lags`
        // operations on the decimated signal.
        let tau_max = (12_000.0f32 / MIN_HZ).ceil() as usize;
        assert!(tau_max * tau_max < 400_000);
    }
}
