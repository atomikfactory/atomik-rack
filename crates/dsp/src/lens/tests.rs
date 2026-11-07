use super::*;

const RATE: f64 = 48_000.0;

fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / RATE).sin() as f32)
        .collect()
}

/// The peak of the second half of `lens` run on a sine in both channels.
fn gain_db(settings: LensSettings, hz: f64) -> f64 {
    let mut lens = Lens::new(settings, RATE);
    let x = sine(hz, 48_000);
    let y: Vec<f32> = x.iter().map(|&s| lens.process(s, s).0).collect();
    let peak = y[24_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    20.0 * f64::from(peak).log10()
}

fn small() -> LensSettings {
    LensSettings {
        voicing: Voicing::SmallSpeaker,
        mono: false,
    }
}

fn club() -> LensSettings {
    LensSettings {
        voicing: Voicing::ClubSub,
        mono: false,
    }
}

#[test]
fn a_neutral_lens_is_the_identity_bit_for_bit() {
    let mut lens = Lens::new(LensSettings::OFF, RATE);
    for (i, s) in sine(97.0, 4_000).into_iter().enumerate() {
        let r = s * 0.37 + i as f32 * 1e-7;
        let (a, b) = lens.process(s, r);
        assert_eq!(a.to_bits(), s.to_bits());
        assert_eq!(b.to_bits(), r.to_bits());
    }
    assert!(LensSettings::default().is_neutral());
}

#[test]
fn the_small_speaker_loses_the_low_end_and_keeps_the_mids() {
    assert!(gain_db(small(), 50.0) < -17.0);
    assert!(gain_db(small(), 100.0) < -6.0);
    // The corner is -3 dB; the bell is small there.
    let corner = response_db(Voicing::SmallSpeaker, SMALL_HIGHPASS_HZ, RATE);
    assert!((-4.0..=-2.0).contains(&corner), "{corner}");
    let mid = gain_db(small(), 1_000.0);
    assert!(mid.abs() < 1.5, "{mid}");
}

#[test]
fn the_presence_curve_is_a_gentle_bell() {
    let bell = response_db(Voicing::SmallSpeaker, SMALL_PRESENCE_HZ, RATE);
    assert!((2.5..=3.5).contains(&bell), "{bell}");
    // Gentle: the highs far above and the mids far below stay near 0 dB.
    assert!(response_db(Voicing::SmallSpeaker, 12_000.0, RATE) < 1.5);
    assert!(response_db(Voicing::SmallSpeaker, 600.0, RATE).abs() < 1.0);
}

#[test]
fn the_club_system_cuts_only_under_its_sub() {
    assert!(gain_db(club(), 16.0) < -10.0);
    assert!(gain_db(club(), 100.0).abs() < 0.3);
    let corner = response_db(Voicing::ClubSub, CLUB_HIGHPASS_HZ, RATE);
    assert!((-3.5..=-2.5).contains(&corner), "{corner}");
    assert!(response_db(Voicing::ClubSub, 40.0, RATE) > -3.0);
    assert!((30.0..=35.0).contains(&CLUB_HIGHPASS_HZ));
    assert!((120.0..=150.0).contains(&SMALL_HIGHPASS_HZ));
}

#[test]
fn the_response_is_what_the_filters_do() {
    for voicing in [Voicing::SmallSpeaker, Voicing::ClubSub] {
        for hz in [40.0, 90.0, 140.0, 500.0, 3_000.0] {
            let measured = gain_db(
                LensSettings {
                    voicing,
                    mono: false,
                },
                hz,
            );
            let said = response_db(voicing, hz, RATE);
            assert!(
                (measured - said).abs() < 0.2,
                "{voicing:?} {hz}: {measured} {said}"
            );
        }
    }
    assert_eq!(response_db(Voicing::Headphones, 100.0, RATE), 0.0);
}

#[test]
fn mono_sums_the_channels_and_cancels_opposite_ones() {
    let mut lens = Lens::new(
        LensSettings {
            voicing: Voicing::Headphones,
            mono: true,
        },
        RATE,
    );
    assert_eq!(lens.process(0.5, 0.5), (0.5, 0.5));
    assert_eq!(lens.process(1.0, -1.0), (0.0, 0.0));
    assert_eq!(lens.process(1.0, 0.0), (0.5, 0.5));
}

#[test]
fn mono_and_a_voicing_together_filter_the_sum() {
    let settings = LensSettings {
        voicing: Voicing::SmallSpeaker,
        mono: true,
    };
    let mut lens = Lens::new(settings, RATE);
    let x = sine(50.0, 48_000);
    let y: Vec<(f32, f32)> = x.iter().map(|&s| lens.process(s, s)).collect();
    assert!(y.iter().all(|(l, r)| l.to_bits() == r.to_bits()));
    let peak = y[24_000..].iter().fold(0.0f32, |m, v| m.max(v.0.abs()));
    assert!(peak < 0.15, "{peak}");
}

#[test]
fn the_output_stays_finite_for_loud_input() {
    let mut lens = Lens::new(small(), RATE);
    for i in 0..10_000 {
        let s = if i % 2 == 0 { 8.0 } else { -8.0 };
        let (l, r) = lens.process(s, -s);
        assert!(l.is_finite() && r.is_finite());
    }
}
