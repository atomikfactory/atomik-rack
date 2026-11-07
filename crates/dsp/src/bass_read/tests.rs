use super::*;

const RATE: u32 = 48_000;
const FRAMES: usize = RATE as usize * 3;

/// A band-limited saw at `hz`: the first twenty harmonics at 1/k.
fn saw(hz: f64, amp: f64) -> Vec<f32> {
    (0..FRAMES)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let v: f64 = (1..=20)
                .map(|k| (std::f64::consts::TAU * hz * k as f64 * t).sin() / k as f64)
                .sum();
            (amp * v * 2.0 / std::f64::consts::PI) as f32
        })
        .collect()
}

fn sine(hz: f64, amp: f64) -> Vec<f32> {
    (0..FRAMES)
        .map(|i| (amp * (std::f64::consts::TAU * hz * i as f64 / f64::from(RATE)).sin()) as f32)
        .collect()
}

#[test]
fn a_saw_bass_reads_and_a_pure_sine_does_not() {
    let saw = read(&saw(55.0, 0.5), &[], RATE);
    assert_eq!(saw.verdict, Verdict::Reads, "{saw:?}");
    // 1/k harmonics through the small-speaker high-pass: about 16 %.
    assert!((0.12..=0.2).contains(&saw.share), "{saw:?}");
    assert!((saw.note_hz - 55.0).abs() < 3.0, "{saw:?}");
    let sine = read(&sine(55.0, 0.5), &[], RATE);
    assert_eq!(sine.verdict, Verdict::Lost, "{sine:?}");
    assert!(sine.share < 0.005, "{sine:?}");
    assert!(sine.windows > 10);
}

#[test]
fn the_reading_does_not_depend_on_the_level() {
    let loud = read(&saw(55.0, 0.8), &[], RATE);
    let quiet = read(&saw(55.0, 0.05), &[], RATE);
    assert!(
        (loud.share - quiet.share).abs() < 0.005,
        "{loud:?} {quiet:?}"
    );
}

#[test]
fn a_higher_note_keeps_more_of_its_harmonics() {
    let low = read(&saw(40.0, 0.5), &[], RATE);
    let high = read(&saw(70.0, 0.5), &[], RATE);
    assert!(high.share > low.share, "{low:?} {high:?}");
    assert_eq!(low.verdict, Verdict::Reads);
}

#[test]
fn a_sine_with_a_little_second_harmonic_is_in_between() {
    let mix: Vec<f32> = sine(50.0, 0.5)
        .iter()
        .zip(sine(100.0, 0.5 * 0.15))
        .map(|(a, b)| a + b)
        .collect();
    let r = read(&mix, &[], RATE);
    // The 2nd harmonic is 0.0225 of the power and the high-pass lets 0.2 of it through.
    assert!(r.share > 0.001 && r.share < 0.02, "{r:?}");
    assert_eq!(r.verdict, Verdict::Lost);
}

#[test]
fn silence_and_a_signal_without_low_notes_have_no_bass() {
    assert_eq!(read(&vec![0.0; FRAMES], &[], RATE), Reading::NONE);
    assert_eq!(read(&[], &[], RATE), Reading::NONE);
    let high = read(&sine(2_000.0, 0.5), &[], RATE);
    assert_eq!(high.verdict, Verdict::NoBass);
    assert_eq!(high.windows, 0);
}

#[test]
fn stereo_is_read_as_its_mid_signal() {
    let l = saw(55.0, 0.5);
    let r = l.clone();
    let stereo = read(&l, &r, RATE);
    let mono = read(&l, &[], RATE);
    assert!((stereo.share - mono.share).abs() < 1e-6);
    // Opposite channels have no mid: nothing to read.
    let flipped: Vec<f32> = l.iter().map(|v| -v).collect();
    assert_eq!(read(&l, &flipped, RATE).verdict, Verdict::NoBass);
}

#[test]
fn non_finite_samples_read_as_silence() {
    let mut x = saw(55.0, 0.5);
    x[1000] = f32::NAN;
    x[2000] = f32::INFINITY;
    let r = read(&x, &[], RATE);
    assert!(r.share.is_finite());
}

#[test]
fn the_verdicts_follow_the_thresholds() {
    assert_eq!(verdict_of(0.2), Verdict::Reads);
    assert_eq!(verdict_of(READS_FROM), Verdict::Reads);
    assert_eq!(verdict_of(0.05), Verdict::Partly);
    assert_eq!(verdict_of(PARTLY_FROM), Verdict::Partly);
    assert_eq!(verdict_of(0.0), Verdict::Lost);
    for v in [
        Verdict::NoBass,
        Verdict::Reads,
        Verdict::Partly,
        Verdict::Lost,
    ] {
        assert!(!v.text().is_empty() && !v.advice().is_empty());
    }
}
