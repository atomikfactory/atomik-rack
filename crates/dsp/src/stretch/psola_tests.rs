use super::fft::Fft;
use super::psola::{NoteShift, mono, note_offsets, note_pitch, shift_notes, vocal_notes};
use crate::notes::track_pitch;

const SR: u32 = 48_000;
/// A sung-like note: from 0.25 s to 1.75 s of a 2 s take.
const START: u64 = 12_000;
const END: u64 = 84_000;
const LEN: usize = 96_000;

/// The level of a resonance at `hz` and its harmonics, around `formant` Hz.
fn formant_gain(f: f64, formant: f64) -> f64 {
    let q = 3.0;
    let r = f / formant;
    1.0 / ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt()
}

/// A sawtooth (every harmonic at `1/h`) through one formant at 1 kHz, its pitch `key(t)` (a
/// fractional MIDI key, `t` in seconds), sounding between [`START`] and [`END`].
fn voice(key: impl Fn(f64) -> f64) -> Vec<f32> {
    let mut out = vec![0.0f32; LEN];
    let mut phase = 0.0f64;
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / f64::from(SR);
        let f0 = 440.0 * 2f64.powf((key(t) - 69.0) / 12.0);
        phase += f0 / f64::from(SR);
        if (i as u64) < START || i as u64 >= END {
            continue;
        }
        let ramp = ((i as u64 - START).min(END - 1 - i as u64) as f64 / 480.0).min(1.0);
        let mut s = 0.0;
        let mut h = 1.0;
        while h * f0 < 8000.0 {
            s += formant_gain(h * f0, 1000.0) / h * (std::f64::consts::TAU * h * phase).sin();
            h += 1.0;
        }
        *o = (0.05 * ramp * s) as f32;
    }
    out
}

/// The tracked pitch (fractional keys) of the voiced frames between 80 ms after the note
/// starts and 80 ms before it ends.
fn pitches(x: &[f32]) -> Vec<f32> {
    let t = track_pitch(&[x], SR, (28, 96), 0.5);
    let margin = 3840;
    t.keys
        .iter()
        .enumerate()
        .filter_map(|(k, p)| {
            let at = k as u64 * t.step;
            p.filter(|_| at >= START + margin && at < END - margin)
        })
        .collect()
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[s.len() / 2]
}

/// The power-weighted centre, in Hz, of the spectrum of the note between 500 Hz and 1.8 kHz,
/// smoothed over 400 Hz so the harmonics blur into the envelope.
fn formant_centre(x: &[f32]) -> f64 {
    let n = 32_768;
    let at = (START as usize + END as usize) / 2 - n / 2;
    let fft = Fft::new(n);
    let (mut re, mut im) = (vec![0.0f64; n], vec![0.0f64; n]);
    for i in 0..n {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        re[i] = f64::from(x[at + i]) * w;
    }
    fft.run(&mut re, &mut im, false);
    let bin = f64::from(SR) / n as f64;
    let power: Vec<f64> = (0..n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect();
    let half = (200.0 / bin) as usize;
    let (mut num, mut den) = (0.0, 0.0);
    for k in (500.0 / bin) as usize..(1800.0 / bin) as usize {
        let smooth: f64 = power[k - half..k + half].iter().sum();
        num += k as f64 * bin * smooth;
        den += smooth;
    }
    num / den
}

/// `x` read `ratio` times as fast (linear interpolation): pitch and formants both move.
fn resample(x: &[f32], ratio: f64) -> Vec<f32> {
    // Centred on the note so the note stays in place.
    let mid = (START + END) as f64 / 2.0;
    (0..x.len())
        .map(|i| {
            let p = mid + (i as f64 - mid) * ratio;
            if p < 0.0 || p >= (x.len() - 1) as f64 {
                return 0.0;
            }
            let k = p.floor() as usize;
            let t = (p - k as f64) as f32;
            x[k] + (x[k + 1] - x[k]) * t
        })
        .collect()
}

fn shift(edit: NoteShift, x: &[f32]) -> Vec<f32> {
    shift_notes(&[x], SR, &[edit], &|| false)
        .expect("not cancelled")
        .remove(0)
}

fn note(shift_cents: i32, drift: f32, vibrato_keep: bool) -> NoteShift {
    NoteShift {
        start: START,
        end: END,
        shift_cents,
        drift,
        vibrato_keep,
    }
}

/// A vibrato of `depth` cents at 6 Hz around key 51 (about 155 Hz).
fn vibrato(depth: f64) -> impl Fn(f64) -> f64 {
    move |t| 51.0 + depth / 100.0 * (std::f64::consts::TAU * 6.0 * t).sin()
}

#[test]
fn no_shift_and_no_flattening_is_a_copy() {
    let x = voice(vibrato(30.0));
    let y = shift(note(0, 0.0, true), &x);
    assert_eq!(x, y);
}

#[test]
fn audio_outside_the_note_is_left_alone() {
    let x = voice(vibrato(30.0));
    let edit = NoteShift {
        start: 30_000,
        end: 60_000,
        ..note(300, 0.0, true)
    };
    let y = shift(edit, &x);
    assert_eq!(x[..30_000], y[..30_000]);
    assert_eq!(x[60_000..], y[60_000..]);
    assert_ne!(x[35_000..55_000], y[35_000..55_000]);
}

#[test]
fn three_semitones_up_moves_the_pitch_and_keeps_the_formant() {
    let x = voice(vibrato(30.0));
    let y = shift(note(300, 0.0, true), &x);
    let moved = f64::from(median(&pitches(&y)) - median(&pitches(&x)));
    assert!((moved - 3.0).abs() <= 0.10, "moved {moved} semitones");
    let (before, after) = (formant_centre(&x), formant_centre(&y));
    let resampled = formant_centre(&resample(&x, 2f64.powf(0.25)));
    assert!(
        (after / before - 1.0).abs() < 0.05,
        "formant {before} Hz -> {after} Hz"
    );
    // A plain resample moves it: the test can tell.
    assert!(
        resampled / before - 1.0 > 0.08,
        "resampled {before} Hz -> {resampled} Hz"
    );
}

#[test]
fn full_drift_flattens_a_slow_wander() {
    // A 30-cent wander at 0.7 Hz.
    let x = voice(|t| 51.0 + 0.30 * (std::f64::consts::TAU * 0.7 * t).sin());
    // The centre is the median of the whole note, as the analysis gives it.
    let centre = note_pitch(&mono(&[&x]), SR, (START, END))
        .centre
        .expect("voiced");
    for keep in [true, false] {
        let y = shift(note(0, 1.0, keep), &x);
        let worst = pitches(&y)
            .iter()
            .fold(0.0f32, |m, p| m.max((p - centre).abs()));
        assert!(worst <= 0.05, "{worst} semitones from the centre");
    }
}

/// The fast part of a pitch curve: its RMS after a 167 ms moving average is taken off.
fn fast_rms(p: &[f32]) -> f64 {
    let len = 33usize;
    let mut sum = 0.0;
    let mut n = 0.0;
    for k in len / 2..p.len() - len / 2 {
        let avg: f32 = p[k - len / 2..k - len / 2 + len].iter().sum::<f32>() / len as f32;
        sum += f64::from(p[k] - avg).powi(2);
        n += 1.0;
    }
    (sum / n).sqrt()
}

#[test]
fn flattening_keeps_the_vibrato_when_asked() {
    let wander_and_vibrato = |t: f64| {
        51.0 + 0.30 * (std::f64::consts::TAU * 0.7 * t).sin()
            + 0.40 * (std::f64::consts::TAU * 6.0 * t).sin()
    };
    let x = voice(wander_and_vibrato);
    let y = shift(note(0, 1.0, true), &x);
    let (before, after) = (fast_rms(&pitches(&x)), fast_rms(&pitches(&y)));
    assert!(
        (after / before - 1.0).abs() <= 0.20,
        "vibrato {before} -> {after}"
    );
    // Without it, the vibrato goes too.
    let flat = shift(note(0, 1.0, false), &x);
    assert!(fast_rms(&pitches(&flat)) < 0.5 * before);
}

#[test]
fn the_render_is_deterministic() {
    let x = voice(vibrato(40.0));
    let edit = NoteShift {
        drift: 0.6,
        ..note(-250, 0.0, true)
    };
    assert_eq!(shift(edit, &x), shift(edit, &x));
}

#[test]
fn a_cancel_stops_the_render() {
    let x = voice(vibrato(40.0));
    assert!(shift_notes(&[&x], SR, &[note(100, 0.0, true)], &|| true).is_none());
}

#[test]
fn the_analysis_finds_the_note_and_its_curve_matches_the_render() {
    let x = voice(vibrato(30.0));
    let m = mono(&[&x]);
    let notes = vocal_notes(&m, SR, (0, LEN as u64), 0.5, &mut |_| true).expect("not stopped");
    assert_eq!(notes.len(), 1, "{notes:?}");
    let n = &notes[0];
    assert_eq!(n.key, 51);
    assert!(n.start.abs_diff(START) < 2400 && n.end.abs_diff(END) < 4800);
    let centre = n.pitch.centre.expect("voiced");
    assert!((centre - 51.0).abs() < 0.1, "{centre}");
    // The curve is read from the note's own span: the same from any buffer that holds it.
    let again = note_pitch(&m, SR, (n.start, n.end));
    assert_eq!(again, n.pitch);
    // The offsets of a shift: the whole shift inside, nothing outside, a glide between.
    let edit = NoteShift {
        start: n.start,
        end: n.end,
        ..note(200, 0.0, true)
    };
    let off = note_offsets(&n.pitch, &edit, SR);
    let mid = off.len() / 2;
    assert!((off[mid] - 2.0).abs() < 1e-6);
    assert_eq!(off[0], 0.0);
    assert!(off.iter().all(|o| (0.0..=2.0).contains(o)));
    // A stop answers nothing.
    assert!(vocal_notes(&m, SR, (0, LEN as u64), 0.5, &mut |_| false).is_none());
}
