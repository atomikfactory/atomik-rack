use super::*;
use std::f32::consts::PI;

const SR: u32 = 48_000;
/// Frames between two kick hits (half a second).
const GAP: usize = 24_000;

/// A kick of `hz` that starts at `phase` and decays over about 0.2 s, once per [`GAP`].
fn kick(hz: f32, phase: f32, seconds: usize) -> Vec<f32> {
    let n = SR as usize * seconds;
    (0..n)
        .map(|i| {
            let t = (i % GAP) as f32 / SR as f32;
            if t < 0.3 {
                0.8 * (-t / 0.2).exp() * (2.0 * PI * hz * t + phase).sin()
            } else {
                0.0
            }
        })
        .collect()
}

/// A steady bass of `hz` with `phase` at the start of the signal.
fn bass(hz: f32, phase: f32, amp: f32, seconds: usize) -> Vec<f32> {
    (0..SR as usize * seconds)
        .map(|i| amp * (2.0 * PI * hz * i as f32 / SR as f32 + phase).sin())
        .collect()
}

#[test]
fn a_bass_in_phase_with_the_kick_builds_up_and_one_against_it_cancels() {
    let k = kick(50.0, 0.0, 4);
    let together = check(&k, &bass(50.0, 0.0, 0.6, 4), SR);
    assert!(together.hits.len() >= 6, "{}", together.hits.len());
    assert!(
        together.building() >= 6 && together.cancelling() == 0,
        "{together:?}"
    );
    assert!(together.overall_db > 1.5, "{}", together.overall_db);
    let against = check(&k, &bass(50.0, PI, 0.6, 4), SR);
    assert!(
        against.cancelling() >= 6 && against.building() == 0,
        "{against:?}"
    );
    assert!(against.overall_db < -3.0 && against.worst_db < -3.0);
    assert!(against.score() < together.score());
}

#[test]
fn the_check_is_the_same_every_time() {
    let (k, b) = (kick(50.0, 0.4, 3), bass(50.0, 1.0, 0.5, 3));
    assert_eq!(check(&k, &b, SR), check(&k, &b, SR));
}

#[test]
fn silence_has_no_hits_and_no_warning() {
    let c = check(&vec![0.0; 48_000], &vec![0.0; 48_000], SR);
    assert!(c.hits.is_empty());
    assert_eq!((c.kick_hz, c.bass_hz, c.semitones), (None, None, None));
    assert!(!c.beating);
    assert_eq!((c.overall_db, c.worst_db), (0.0, 0.0));
    let empty = check(&[], &[], SR);
    assert!(empty.hits.is_empty() && empty.frames == 0);
}

#[test]
fn the_fundamentals_are_found_and_close_ones_are_flagged_as_beating() {
    let k = kick(50.0, 0.0, 4);
    let near = check(&k, &bass(52.5, 0.0, 0.4, 4), SR);
    let (kh, bh) = (near.kick_hz.unwrap(), near.bass_hz.unwrap());
    assert!((kh - 50.0).abs() < 0.8, "{kh}");
    assert!((bh - 52.5).abs() < 0.5, "{bh}");
    assert!(near.beating, "{near:?}");
    let beat = near.beat_hz.unwrap();
    assert!((beat - 2.5).abs() < 1.0, "{beat}");
    let far = check(&k, &bass(75.0, 0.0, 0.4, 4), SR);
    assert!(!far.beating && far.beat_hz.is_none());
    assert!(far.semitones.unwrap() > 6.0);
}

#[test]
fn the_same_pitch_is_a_phase_question_not_a_beat() {
    let c = check(&kick(50.0, 0.0, 4), &bass(50.0, 0.0, 0.4, 4), SR);
    assert!(
        c.semitones.unwrap() < SAME_SEMITONES + 0.1,
        "{:?}",
        c.semitones
    );
    assert!(!c.beating);
}

#[test]
fn a_tone_is_measured_from_its_zero_crossings() {
    let x = bass(41.2, 0.7, 0.5, 1);
    let hz = tone_hz(&x, SR).unwrap();
    assert!((hz - 41.2).abs() < 0.05, "{hz}");
    assert_eq!(tone_hz(&[0.0; 4_800], SR), None);
    assert_eq!(tone_hz(&x[..300], SR), None);
}

#[test]
fn the_sub_band_keeps_the_bass_and_drops_the_highs() {
    let low = bass(50.0, 0.0, 1.0, 1);
    let high = bass(2_000.0, 0.0, 1.0, 1);
    let rms = |v: &[f32]| (v[24_000..].iter().map(|s| s * s).sum::<f32>() / 24_000.0).sqrt();
    assert!(rms(&sub_band(&low, SR)) > 0.65);
    assert!(rms(&sub_band(&high, SR)) < 0.01);
}

#[test]
fn a_delay_of_the_bass_can_put_it_back_in_phase() {
    let k = kick(50.0, 0.0, 4);
    let against = bass(50.0, PI, 0.6, 4);
    let delay = best_bass_delay(&k, &against, SR, 960).expect("a delay helps");
    // Half a period of 50 Hz is 480 frames.
    assert!((delay as i64 - 480).abs() <= 40, "{delay}");
    let mut delayed = vec![0.0; delay];
    delayed.extend_from_slice(&against[..against.len() - delay]);
    assert!(check(&k, &delayed, SR).overall_db > 1.0);
    // A bass that already builds up needs no delay.
    assert_eq!(best_bass_delay(&k, &bass(50.0, 0.0, 0.6, 4), SR, 960), None);
}

#[test]
fn the_mono_sum_averages_and_ignores_non_finite_samples() {
    let m = mono(&[1.0, f32::NAN, 0.5], &[0.0, 1.0, 0.5, 9.0]);
    assert_eq!(m, vec![0.5, 0.5, 0.5]);
}

#[test]
fn a_kick_that_stops_early_still_gives_its_pitch() {
    // 120 ms of 50 Hz, then silence until the next hit.
    let k: Vec<f32> = (0..SR as usize * 4)
        .map(|i| {
            let t = (i % GAP) as f32 / SR as f32;
            if t < 0.12 {
                0.8 * (2.0 * PI * 50.0 * t).sin()
            } else {
                0.0
            }
        })
        .collect();
    let c = check(&k, &bass(50.0, 0.0, 0.5, 4), SR);
    let hz = c.kick_hz.expect("a pitch");
    assert!((hz - 50.0).abs() < 2.0, "{hz}");
}
