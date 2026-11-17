use super::*;
use std::f64::consts::TAU;

const SR: u32 = 48_000;

fn hz_of(key: i32) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

/// Adds a plucked-string stand-in at `key` starting at `start` frames: a 1 ms attack, five
/// partials decaying at different rates.
fn pluck(out: &mut [f32], start: usize, key: i32, amp: f64) {
    pluck_fast(out, start, key, amp, 1.0);
}

/// [`pluck`] with every decay rate multiplied by `decay`.
fn pluck_fast(out: &mut [f32], start: usize, key: i32, amp: f64, decay: f64) {
    let hz = hz_of(key);
    let partials: [(f64, f64, f64); 5] = [
        (1.0, 0.8, 2.5),
        (2.0, 1.0, 3.0),
        (3.0, 0.6, 4.0),
        (4.0, 0.35, 5.0),
        (5.0, 0.2, 6.0),
    ];
    for (i, o) in out.iter_mut().enumerate().skip(start) {
        let t = (i - start) as f64 / f64::from(SR);
        let attack = (t / 0.001).min(1.0);
        let s: f64 = partials
            .iter()
            .map(|(k, a, d)| a * (-d * decay * t).exp() * (TAU * hz * k * t).sin())
            .sum();
        *o += (amp * attack * s * 0.35) as f32;
    }
}

fn run(x: &[f32], block: usize) -> Vec<PlayedNote> {
    let mut l = Listener::new(SR);
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < x.len() {
        let n = block.min(x.len() - pos);
        out.extend(l.feed(pos as u64, &x[pos..pos + n]));
        pos += n;
    }
    // Silence after the signal lets the last window complete.
    let tail = vec![0.0f32; l.latency_frames() + 4096];
    out.extend(l.feed(pos as u64, &tail));
    out
}

fn ms(frames: u64) -> f64 {
    frames as f64 * 1000.0 / f64::from(SR)
}

#[test]
fn a_sequence_of_plucked_notes_gives_their_onsets_and_pitches() {
    let starts = [0.20, 0.60, 1.00, 1.45];
    let keys = [40, 45, 52, 59];
    let mut x = vec![0.0f32; SR as usize * 2];
    for (s, k) in starts.iter().zip(keys) {
        pluck(&mut x, (s * f64::from(SR)) as usize, k, 0.8);
    }
    let notes = run(&x, 512);
    assert_eq!(notes.len(), 4, "{notes:?}");
    for (n, (s, k)) in notes.iter().zip(starts.iter().zip(keys)) {
        assert!(
            (ms(n.frame) - s * 1000.0).abs() < 8.0,
            "onset {} ms for {s}",
            ms(n.frame)
        );
        assert_eq!(n.key, Some(k), "{n:?}");
        assert!(n.clarity > 0.8);
        assert!(n.cents.abs() < 15.0);
    }
}

#[test]
fn the_result_does_not_depend_on_the_block_size() {
    let mut x = vec![0.0f32; SR as usize];
    pluck(&mut x, 9_000, 50, 0.7);
    pluck(&mut x, 30_000, 55, 0.7);
    let a = run(&x, 64);
    let b = run(&x, 4096);
    let c = run(&x, 777);
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert_eq!(a.len(), 2);
}

#[test]
fn a_chord_has_the_chroma_of_its_notes_and_no_single_pitch_needed() {
    // C major: C3 E3 G3 C4 E4 as a near-simultaneous strum.
    let mut x = vec![0.0f32; SR as usize * 2];
    for (i, k) in [48, 52, 55, 60, 64].into_iter().enumerate() {
        pluck(&mut x, 24_000 + i * 240, k, 0.5);
    }
    let notes = run(&x, 1024);
    assert_eq!(notes.len(), 1, "{notes:?}");
    let c = notes[0].chroma;
    assert!((c.iter().fold(0.0f32, |m, v| m.max(*v)) - 1.0).abs() < 1e-6);
    let mut order: Vec<usize> = (0..12).collect();
    order.sort_by(|a, b| c[*b].total_cmp(&c[*a]));
    let top: std::collections::BTreeSet<usize> = order[..3].iter().copied().collect();
    assert_eq!(top, [0usize, 4, 7].into_iter().collect(), "{c:?}");
    assert!(ms(notes[0].frame) > 495.0 && ms(notes[0].frame) < 510.0);
}

#[test]
fn silence_and_steady_noise_make_no_notes() {
    assert!(run(&vec![0.0f32; SR as usize], 1024).is_empty());
    let noise = crate::test_util::noise(7, SR as usize * 2, 0.05);
    assert!(run(&noise, 1024).is_empty());
    let hum: Vec<f32> = (0..SR as usize)
        .map(|i| (0.2 * (TAU * 110.0 * i as f64 / f64::from(SR)).sin()) as f32)
        .collect();
    // A steady tone that is there from the start is not an onset after the first hop.
    assert!(run(&hum, 1024).len() <= 1);
}

#[test]
fn notes_closer_than_the_minimum_gap_merge_and_the_level_does_not_matter() {
    // Two plucks 21 ms apart are one note.
    let mut x = vec![0.0f32; SR as usize];
    pluck(&mut x, 10_000, 45, 0.5);
    pluck(&mut x, 10_000 + 1_000, 52, 0.5);
    let merged = run(&x, 512);
    assert_eq!(merged.len(), 1, "{merged:?}");
    // The same note 30 dB apart in level is found alike.
    let mut quiet = vec![0.0f32; SR as usize];
    pluck(&mut quiet, 10_000, 50, 0.03);
    let mut loud = vec![0.0f32; SR as usize];
    pluck(&mut loud, 10_000, 50, 0.9);
    let (q, l) = (run(&quiet, 512), run(&loud, 512));
    assert_eq!((q.len(), l.len()), (1, 1), "{q:?} {l:?}");
    assert_eq!(q[0].key, Some(50));
    assert_eq!(l[0].key, Some(50));
    assert!(q[0].frame.abs_diff(l[0].frame) < 100);
}

#[test]
fn a_fast_run_keeps_every_note_with_the_window_cut_by_the_next() {
    // Sixteenth notes at 140 bpm: 107 ms apart.
    let gap = (0.107 * f64::from(SR)) as usize;
    let keys = [52, 55, 57, 59, 52, 55];
    let mut x = vec![0.0f32; SR as usize * 2];
    for (i, k) in keys.iter().enumerate() {
        pluck_fast(&mut x, 6_000 + i * gap, *k, 0.8, 5.0);
    }
    let notes = run(&x, 256);
    assert_eq!(notes.len(), keys.len(), "{notes:?}");
    let got: Vec<Option<i32>> = notes.iter().map(|n| n.key).collect();
    let right = got
        .iter()
        .zip(keys)
        .filter(|(g, k)| **g == Some(*k))
        .count();
    assert!(right >= 5, "{got:?}");
}

#[test]
fn a_gap_in_the_frame_numbers_starts_over_and_bad_samples_are_silent() {
    let mut l = Listener::new(SR);
    let mut x = vec![0.0f32; 20_000];
    pluck(&mut x, 5_000, 45, 0.8);
    x[100] = f32::NAN;
    l.feed(0, &x);
    assert_eq!(l.next_frame(), 20_000);
    l.feed(1_000_000, &[0.0; 100]);
    assert_eq!(l.next_frame(), 1_000_100);
    assert!(l.feed(1_000_100, &[0.0; 9000]).is_empty());
}

#[test]
fn other_sample_rates_scale_the_windows() {
    let rate = 44_100u32;
    let hz = hz_of(45);
    let mut x = vec![0.0f32; rate as usize];
    for (i, o) in x.iter_mut().enumerate().skip(9_000) {
        let t = (i - 9_000) as f64 / f64::from(rate);
        *o = (0.5 * (-3.0 * t).exp() * (TAU * hz * t).sin()
            + 0.4 * (-3.0 * t).exp() * (TAU * hz * 2.0 * t).sin()) as f32;
    }
    let mut l = Listener::new(rate);
    let mut out = l.feed(0, &x);
    out.extend(l.feed(x.len() as u64, &vec![0.0; l.latency_frames() + 4096]));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].key, Some(45));
}
