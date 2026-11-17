//! A tab clip's techniques, end to end: the contract between the model's technique renderer and
//! the Plucked Strings device (the constants agree, the lanes agree), and a score imported with
//! a palm mute, a harmonic, a dead note and a bend, bounced through the device, sounding
//! different from the plain note.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_devices::plucked::{EXPR_DEAD, EXPR_HARMONIC, EXPR_HARMONIC_BELOW, EXPR_NORMAL_BELOW};
use atomik_model::empty_doc;
use atomik_model::tab_data::PlainNote;
use atomik_model::tab_render::{
    self, DEAD_LENGTH, PRESSURE_DEAD, PRESSURE_HARMONIC, Spot, TIMBRE_PALM_MUTE,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_tab::instrument::find_tuning;
use atomik_tab::score::{
    Bend, Harmonic, HarmonicKind, TabNote, TabScore, TabTrack, TechFlags, Techniques,
};

fn guitar_note(fret: u8, tech: Techniques) -> TabNote {
    let mut n = TabNote::new(0, 8 * PPQ, 3, fret);
    n.tech = tech;
    n
}

fn one_note_score(note: TabNote) -> TabScore {
    let mut score = TabScore::new();
    let tuning = find_tuning("guitar6", "standard").unwrap().tuning.clone();
    let mut t = TabTrack::new("Guitar", "guitar6", tuning);
    t.notes = vec![note];
    score.tracks = vec![t];
    score.tempo = vec![(0, 120.0)];
    score
}

/// The note imported on Steel Acoustic and bounced: the left channel and the sample rate.
fn bounce(note: TabNote) -> (Vec<f32>, f64) {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.set_factory_root(Some(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../factory"),
    ));
    c.apply_tab(&one_note_score(note), Vec::new(), false, "x.gp5")
        .unwrap();
    let out = render_doc(c.doc(), &default_registry(), &ProjectOptions::default()).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    (out.channels[0].clone(), f64::from(out.sample_rate))
}

/// The samples between `from` and `to` seconds.
fn window(x: &[f32], rate: f64, from: f64, to: f64) -> &[f32] {
    &x[(from * rate) as usize..(to * rate) as usize]
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// The energy of the first difference over the energy: how much of the sound is high.
fn brightness(x: &[f32]) -> f64 {
    let d: f64 = x.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2)).sum();
    let e: f64 = x.iter().map(|s| f64::from(*s).powi(2)).sum();
    d / e
}

// ------------------------------------------------------------------ the contract

fn rendered(tech: &Techniques) -> tab_render::Rendered {
    tab_render::render_note(
        Spot {
            tick: 0,
            key: 57,
            fret: 7,
        },
        PlainNote {
            velocity: 0.7,
            length: PPQ,
        },
        tech,
        None,
    )
}

#[test]
fn the_model_codes_are_the_ones_the_device_reads() {
    assert_eq!(PRESSURE_HARMONIC, EXPR_HARMONIC);
    assert_eq!(PRESSURE_DEAD, EXPR_DEAD);
    // The device's thresholds put each code in its own class.
    const { assert!(0.0 < EXPR_NORMAL_BELOW) };
    assert!((EXPR_NORMAL_BELOW..EXPR_HARMONIC_BELOW).contains(&PRESSURE_HARMONIC));
    const { assert!(PRESSURE_DEAD >= EXPR_HARMONIC_BELOW) };
    // The palm mute amount is a valid amount of the device's Slide expression (0..1) and strong
    // enough to hear.
    assert!((0.3..=1.0).contains(&TIMBRE_PALM_MUTE));
}

#[test]
fn each_technique_is_on_the_lane_the_device_reads_it_from() {
    let none = Techniques::default();
    let plain = rendered(&none);
    assert!(plain.expression.pitch.is_empty());
    assert!(plain.expression.timbre.is_empty());
    assert!(plain.expression.pressure.is_empty());

    // Palm mute: the timbre lane (the device's Slide) holds the amount, nothing on pressure.
    let pm = rendered(&none.clone().with(TechFlags::PALM_MUTE)).expression;
    assert!(!pm.timbre.is_empty());
    assert!(pm.timbre.iter().all(|p| p.value == TIMBRE_PALM_MUTE));
    assert!(pm.pressure.is_empty());

    // Harmonic: the pressure code at the note start, and the pitch lane carries the node.
    let harmonic = Techniques {
        harmonic: Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: None,
        }),
        ..Techniques::default()
    };
    let h = rendered(&harmonic).expression;
    assert_eq!(
        h.pressure.first().map(|p| (p.tick, p.value)),
        Some((0, EXPR_HARMONIC))
    );
    assert_eq!(
        h.pitch.first().map(|p| p.value),
        Some(12.0),
        "fret 7 sounds 19 above the open string"
    );

    // Dead note: its code, at the start, and a short note.
    let dead = rendered(&none.clone().with(TechFlags::DEAD));
    assert_eq!(
        dead.expression.pressure.first().map(|p| (p.tick, p.value)),
        Some((0, EXPR_DEAD))
    );
    assert!(dead.length <= DEAD_LENGTH);

    // Bend: pitch only.
    let bent = rendered(&Techniques {
        bend: Some(Bend::up(200)),
        ..Techniques::default()
    })
    .expression;
    assert!(bent.pitch.iter().any(|p| (p.value - 2.0).abs() < 0.01));
    assert!(bent.pressure.is_empty() && bent.timbre.is_empty());

    // Ghost and accent notes only change the velocity.
    let ghost = rendered(&none.clone().with(TechFlags::GHOST));
    let accent = rendered(&none.with(TechFlags::ACCENT));
    assert!(ghost.velocity < 0.7 && accent.velocity > 0.7);
    assert!(ghost.expression.pitch.is_empty() && accent.expression.pressure.is_empty());
}

// ------------------------------------------------------------------ what is heard

#[test]
fn a_palm_muted_note_dies_away_faster_than_a_plain_one() {
    let (plain, rate) = bounce(guitar_note(7, Techniques::default()));
    let muted_tech = Techniques::default().with(TechFlags::PALM_MUTE);
    let (muted, _) = bounce(guitar_note(7, muted_tech));
    let decay = |x: &[f32]| rms(window(x, rate, 0.5, 0.7)) / rms(window(x, rate, 0.05, 0.25));
    let (p, m) = (decay(&plain), decay(&muted));
    assert!(p > 0.25, "the plain note rings on: {p}");
    assert!(m < p / 3.0, "the muted note falls away: {m} against {p}");
    assert!(
        brightness(window(&muted, rate, 0.05, 0.4)) < brightness(window(&plain, rate, 0.05, 0.4)),
        "and is darker"
    );
}

#[test]
fn a_dead_note_is_a_short_click() {
    let (plain, rate) = bounce(guitar_note(3, Techniques::default()));
    let (dead, _) = bounce(guitar_note(3, Techniques::default().with(TechFlags::DEAD)));
    assert!(rms(window(&dead, rate, 0.0, 0.1)) > 0.001, "it is heard");
    let late = |x: &[f32]| rms(window(x, rate, 0.3, 0.6));
    assert!(
        late(&dead) < late(&plain) / 20.0,
        "{} {}",
        late(&dead),
        late(&plain)
    );
}

#[test]
fn a_harmonic_rings_at_the_node_pitch_and_is_purer_than_the_fretted_note() {
    let harmonic = Techniques {
        harmonic: Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: None,
        }),
        ..Techniques::default()
    };
    // Fret 7 of the D string, harmonic: A4. The plain comparison is the fretted A4 (fret 19).
    let (h, rate) = bounce(guitar_note(7, harmonic));
    let (plain, _) = bounce(guitar_note(19, Techniques::default()));
    let f = pitch(window(&h, rate, 0.1, 0.5), rate);
    assert!((f / 440.0 - 1.0).abs() < 0.02, "{f} Hz");
    assert!(
        brightness(window(&h, rate, 0.1, 0.5)) < brightness(window(&plain, rate, 0.1, 0.5)),
        "fewer overtones"
    );
}

#[test]
fn a_bent_note_rises_in_pitch() {
    let bend = Techniques {
        bend: Some(Bend::up(200)),
        ..Techniques::default()
    };
    let (bent, rate) = bounce(guitar_note(7, bend));
    let (plain, _) = bounce(guitar_note(7, Techniques::default()));
    let rise = |x: &[f32]| {
        pitch(window(x, rate, 1.0, 1.4), rate) / pitch(window(x, rate, 0.05, 0.25), rate)
    };
    // The note starts on pitch (A3 is 220 Hz) and ends two semitones up.
    assert!((rise(&plain) - 1.0).abs() < 0.015, "{}", rise(&plain));
    let two_semitones = 2.0_f64.powf(2.0 / 12.0);
    assert!(
        (rise(&bent) / two_semitones - 1.0).abs() < 0.03,
        "{}",
        rise(&bent)
    );
}

/// The pitch of the newest 8192 samples of `x`, by the tuner's detector.
fn pitch(x: &[f32], rate: f64) -> f64 {
    f64::from(
        atomik_dsp::tuner::detect(x, rate as f32)
            .expect("the note is pitched")
            .hz,
    )
}
