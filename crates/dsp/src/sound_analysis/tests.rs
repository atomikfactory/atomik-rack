use super::*;
use std::f64::consts::TAU;

const RATE: u32 = 44_100;

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(RATE)) as usize
}

/// A tone of the first `harmonics` multiples of `hz` (all of them, or only the odd ones) at
/// `1/k`, with the amplitude `env(t)` and the harmonic `k` scaled by `tilt(k, t)`.
fn tone(
    hz: f64,
    (harmonics, odd_only): (usize, bool),
    seconds: f64,
    env: impl Fn(f64) -> f64,
    tilt: impl Fn(usize, f64) -> f64,
) -> Vec<f32> {
    (0..frames(seconds))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let sum: f64 = (1..=harmonics)
                .filter(|k| !odd_only || k % 2 == 1)
                .map(|k| (TAU * hz * k as f64 * t).sin() * tilt(k, t) / k as f64)
                .sum();
            (0.4 * env(t) * sum) as f32
        })
        .collect()
}

/// A saw pluck: a 2 ms attack, then a fall to a tenth in 0.3 s, getting duller as it goes.
fn saw_pluck() -> Vec<f32> {
    let tau = 0.3 / 10f64.ln();
    tone(
        220.0,
        (40, false),
        2.0,
        |t| (t / 0.002).min(1.0) * (-t / tau).exp(),
        |k, t| (-((k - 1) as f64) * t / 0.25).exp(),
    )
}

/// A square pad: a linear attack of 0.3 s, held to 1.5 s, then a release that takes a tenth in 0.23 s.
fn square_pad() -> Vec<f32> {
    let tau = 0.1;
    tone(
        220.0,
        (39, true),
        2.6,
        |t| {
            if t < 0.3 {
                t / 0.3
            } else if t < 1.5 {
                1.0
            } else {
                (-(t - 1.5) / tau).exp()
            }
        },
        |_, _| 1.0,
    )
}

/// White noise through a first difference (a high-pass) that falls to a tenth in 60 ms.
fn noise_hat() -> Vec<f32> {
    let mut state = 0x1234_5678_u32;
    let mut last = 0.0f32;
    let tau = 0.06 / 10f64.ln();
    (0..frames(0.5))
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            let y = x - last;
            last = x;
            let t = i as f64 / f64::from(RATE);
            (0.4 * f64::from(y) * (-t / tau).exp()) as f32
        })
        .collect()
}

fn sine(hz: f64, seconds: f64) -> Vec<f32> {
    tone(hz, (1, false), seconds, |_| 1.0, |_, _| 1.0)
}

fn profile(x: &[f32]) -> SoundProfile {
    analyse(&[x], RATE).expect("analyses")
}

#[test]
fn a_saw_pluck_is_pitched_with_all_harmonics_and_dies_by_itself() {
    let p = profile(&saw_pluck());
    let pitch = p.pitch.expect("pitched");
    assert!((pitch.fundamental_hz - 220.0).abs() < 1.5, "{pitch:?}");
    assert_eq!(p.harmonics.kind, HarmonicKind::All, "{:?}", p.harmonics);
    assert!(p.noise_share < 0.15, "{}", p.noise_share);
    let e = p.envelope;
    assert!(e.sustain == 0.0 && e.release_s == 0.0, "{e:?}");
    assert!(e.attack_s < 0.025, "{e:?}");
    // A tenth after 0.3 s (the window blurs the start a little).
    assert!((e.decay_s - 0.3).abs() < 0.06, "{e:?}");
    assert!((e.hold_s - 0.3).abs() < 0.08, "{e:?}");
    assert!(pitch.start_semitones == 0.0, "{pitch:?}");
}

#[test]
fn a_pluck_that_loses_its_harmonics_gets_darker() {
    let p = profile(&saw_pluck());
    let b = p.brightness;
    assert!(b.start_hz > 2.0 * b.end_hz, "{b:?}");
    assert!(b.fall_s > 0.0, "{b:?}");
    // The curve itself falls from the peak.
    let top = p.level.iter().position(|l| *l >= 0.95).unwrap();
    let a = p.centroid_hz[top];
    let z = p.centroid_hz[top + 40];
    assert!(a > 0.0 && z > 0.0 && a > 1.5 * z, "{a} {z}");
}

#[test]
fn a_square_pad_has_odd_harmonics_a_plateau_and_a_release() {
    let p = profile(&square_pad());
    assert_eq!(p.harmonics.kind, HarmonicKind::OddOnly, "{:?}", p.harmonics);
    assert!((p.pitch.expect("pitched").fundamental_hz - 220.0).abs() < 1.5);
    let e = p.envelope;
    // A linear rise from 0 to 1 in 0.3 s takes 0.24 s from a tenth to nine tenths.
    assert!((e.attack_s - 0.24).abs() < 0.05, "{e:?}");
    assert!(e.sustain > 0.9, "{e:?}");
    assert!(e.decay_s == 0.0, "{e:?}");
    assert!((e.hold_s - 1.5).abs() < 0.12, "{e:?}");
    // Down to a tenth in 0.1 * ln 10.
    assert!((e.release_s - 0.23).abs() < 0.06, "{e:?}");
    // The square's third harmonic is a third of the first: about -9.5 dB.
    let third = p.harmonics.third_db.expect("found");
    assert!((third + 9.5).abs() < 3.0, "{third}");
}

#[test]
fn a_noise_hat_is_unpitched_noise_that_falls_fast() {
    let p = profile(&noise_hat());
    assert!(p.pitch.is_none(), "{:?}", p.pitch);
    assert!(p.pitch_hz.iter().all(|h| *h == 0.0));
    assert_eq!(p.harmonics.kind, HarmonicKind::Noise);
    assert!(p.noise_share > 0.9, "{}", p.noise_share);
    let e = p.envelope;
    assert!(e.sustain == 0.0 && e.release_s == 0.0, "{e:?}");
    assert!((e.decay_s - 0.06).abs() < 0.025, "{e:?}");
    // A high-passed noise is bright.
    assert!(p.brightness.mean_hz > 5_000.0, "{:?}", p.brightness);
}

#[test]
fn a_sine_is_pure_and_a_bell_is_inharmonic() {
    let p = profile(&sine(330.0, 1.0));
    assert_eq!(p.harmonics.kind, HarmonicKind::Pure, "{:?}", p.harmonics);
    assert!(p.noise_share < 0.05, "{}", p.noise_share);
    // Two sines at 1 : 2.76 do not line up.
    let bell: Vec<f32> = (0..frames(1.5))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let env = (-t / 0.4).exp();
            (0.3 * env * ((TAU * 300.0 * t).sin() + 0.8 * (TAU * 828.0 * t).sin())) as f32
        })
        .collect();
    let b = profile(&bell);
    assert_eq!(
        b.harmonics.kind,
        HarmonicKind::Inharmonic,
        "{:?}",
        b.harmonics
    );
    assert!(b.noise_share < 0.3, "{}", b.noise_share);
    // The two partials keep their 1 : 2.76 spacing whatever they are measured against.
    let ratios: Vec<f32> = b.harmonics.partials.iter().map(|p| p.ratio).collect();
    assert_eq!(ratios.len(), 2, "{ratios:?}");
    let spacing = ratios[0].max(ratios[1]) / ratios[0].min(ratios[1]);
    assert!((spacing - 2.76).abs() < 0.05, "{ratios:?}");
}

#[test]
fn a_falling_pitch_gives_a_pitch_offset_and_a_settling_time() {
    // 12 semitones above 110 Hz, falling to it in about 80 ms.
    let tau = 0.04;
    let mut phase = 0.0f64;
    let kick: Vec<f32> = (0..frames(0.8))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let semis = 12.0 * (-t / tau).exp();
            phase += TAU * 110.0 * 2f64.powf(semis / 12.0) / f64::from(RATE);
            (0.5 * phase.sin() * (-t / 0.35).exp()) as f32
        })
        .collect();
    let p = profile(&kick);
    let pitch = p.pitch.expect("pitched");
    assert!((pitch.fundamental_hz - 110.0).abs() < 4.0, "{pitch:?}");
    assert!(pitch.start_semitones > 3.0, "{pitch:?}");
    assert!((0.02..0.3).contains(&pitch.settle_s), "{pitch:?}");
}

#[test]
fn the_curves_have_one_entry_per_frame_and_start_at_the_sound() {
    // Half a second of silence, then the pluck.
    let mut x = vec![0.0f32; frames(0.5)];
    x.extend(saw_pluck());
    let p = profile(&x);
    assert!((p.lead_s - 0.5).abs() < 0.03, "{}", p.lead_s);
    assert_eq!(p.level.len(), p.centroid_hz.len());
    assert_eq!(p.level.len(), p.pitch_hz.len());
    assert!(p.level[0] > 0.0 && p.level.iter().all(|l| (0.0..=1.0).contains(l)));
    assert!((p.duration_s - p.level.len() as f32 * p.hop_s).abs() < 1e-6);
    // The first frame of the curve is the start of the sound.
    assert!(p.level[0] < 0.6, "{}", p.level[0]);
}

#[test]
fn silence_a_blip_and_a_low_rate_are_refused() {
    assert_eq!(
        analyse(&[&vec![0.0; 44_100]], RATE),
        Err(AnalysisError::Silent)
    );
    assert_eq!(
        analyse(&[&vec![1e-6; 44_100]], RATE),
        Err(AnalysisError::Silent)
    );
    let mut blip = vec![0.0f32; 44_100];
    blip[100..220].fill(0.5);
    assert_eq!(analyse(&[&blip], RATE), Err(AnalysisError::TooShort));
    assert_eq!(
        analyse(&[&sine(220.0, 1.0)], 4_000),
        Err(AnalysisError::Rate)
    );
    assert_eq!(analyse(&[], RATE), Err(AnalysisError::Silent));
}

#[test]
fn non_finite_samples_do_not_poison_the_profile() {
    let mut x = saw_pluck();
    x[1000] = f32::NAN;
    x[2000] = f32::INFINITY;
    let p = profile(&x);
    let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
    assert!(finite(&p.level) && finite(&p.centroid_hz) && finite(&p.pitch_hz));
    assert!(p.noise_share.is_finite() && p.brightness.mean_hz.is_finite());
}

#[test]
fn the_profile_does_not_depend_on_the_level_or_on_the_channel_count() {
    let x = saw_pluck();
    let quiet: Vec<f32> = x.iter().map(|v| v * 0.1).collect();
    let a = profile(&x);
    let b = profile(&quiet);
    assert_eq!(a.harmonics.kind, b.harmonics.kind);
    assert!((a.envelope.decay_s - b.envelope.decay_s).abs() < 0.01);
    assert!((a.noise_share - b.noise_share).abs() < 0.05);
    // The same signal on two channels is the same sound.
    let both = analyse(&[&x, &x], RATE).unwrap();
    assert_eq!(both, a);
}

#[test]
fn the_same_audio_gives_the_same_profile() {
    let x = square_pad();
    assert_eq!(profile(&x), profile(&x));
}

#[test]
fn a_profile_matches_itself_exactly_and_a_different_sound_less() {
    let pluck = profile(&saw_pluck());
    let same = compare(&pluck, &pluck);
    assert_eq!(same.envelope_db, 0.0);
    assert_eq!(same.brightness_octaves, 0.0);
    assert_eq!(same.pitch_semitones, Some(0.0));
    assert!((same.overall() - 1.0).abs() < 1e-6);
    let pad = profile(&square_pad());
    let far = compare(&pluck, &pad);
    assert!(far.envelope_db > 5.0, "{far:?}");
    assert!(far.overall() < same.overall());
    // An unpitched target has no pitch distance.
    let hat = profile(&noise_hat());
    assert_eq!(compare(&hat, &pluck).pitch_semitones, None);
}

#[test]
fn a_pitch_that_is_a_semitone_off_is_a_semitone_away() {
    let a = profile(&sine(220.0, 1.0));
    let b = profile(&sine(220.0 * 2f64.powf(1.0 / 12.0), 1.0));
    let d = compare(&a, &b);
    let semis = d.pitch_semitones.unwrap();
    assert!((semis - 1.0).abs() < 0.1, "{d:?}");
    assert!(d.envelope_db < 1.0 && d.brightness_octaves < 0.2, "{d:?}");
}

#[test]
fn the_scores_fall_as_the_distances_grow() {
    let d = |e: f32| Distances {
        envelope_db: e,
        brightness_octaves: e / 6.0,
        pitch_semitones: Some(e / 12.0),
    };
    let (a, b) = (d(1.0), d(12.0));
    assert!(a.overall() > b.overall());
    let half = Distances {
        envelope_db: HALF_MATCH.0,
        brightness_octaves: HALF_MATCH.1,
        pitch_semitones: None,
    };
    let (e, br, p) = half.scores();
    assert!((e - 0.5).abs() < 1e-6 && (br - 0.5).abs() < 1e-6 && p.is_none());
}
