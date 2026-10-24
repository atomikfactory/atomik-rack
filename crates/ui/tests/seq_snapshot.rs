//! Display-list snapshots of the sequencer views, compared with checked-in goldens:
//! the arrangement (3 tracks, one with an automation lane, clips with loop repeats, the loop
//! range and the playhead), the piano roll on the 4-bar bass clip with its velocity lane, and
//! the transport bar. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test seq_snapshot`.

mod support;

use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::theory::{ChordKind, Scale, ScaleKind};
use atomik_model::{ClipId, Curve};
use atomik_ui::chord_edit::Stamp;
use atomik_ui::key_rows::Fold;
use atomik_ui::timeline::Snap;
use atomik_ui::{DisplayList, TransportState};
use std::path::PathBuf;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn record_arrangement() -> DisplayList {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    v.set_playhead(2 * BAR + PPQ + PPQ / 2);
    v.set_selected_clips(&[ClipId(2)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl
}

fn record_piano_roll() -> DisplayList {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_playhead(2 * BAR + PPQ);
    v.set_selected_notes(&[4, 5, 6]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl
}

/// The piano roll on the bass clip with a scale (C minor pentatonic... the bass line is in
/// C and G with the odd F, E flat and B flat), the fold and the chord stamp shown.
fn record_piano_roll_scale(fold: Fold) -> DisplayList {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.tracks[0].lanes[0].clips[0].scale = Some(Scale::new(0, ScaleKind::NaturalMinor));
    let mut v = piano_roll(&doc, &reg);
    v.set_fold(fold);
    v.set_snap_to_scale(true);
    v.set_stamp(Stamp::Chord(ChordKind::Minor));
    v.set_selected_notes(&[4]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl
}

/// The piano roll with the controller area open on a pitch bend lane (a ramp, a hold and a
/// return to the centre) and a mod wheel lane, the second one shown.
fn record_piano_roll_controllers() -> DisplayList {
    let (mut doc, reg) = (song_doc(), registry());
    let pt = |tick, value, curve| atomik_model::AutomationPoint { tick, value, curve };
    doc.song.tracks[0].lanes[0].clips[0].controllers = vec![
        ControllerLane::new(
            ControllerKind::PitchBend,
            vec![
                pt(0, 0.0, Curve::Linear),
                pt(PPQ, 0.5, Curve::Step),
                pt(2 * PPQ, 0.0, Curve::Step),
            ],
        ),
        ControllerLane::new(
            ControllerKind::Cc(1),
            vec![pt(PPQ, 0.25, Curve::Linear), pt(3 * PPQ, 1.0, Curve::Step)],
        ),
    ];
    let mut v = piano_roll(&doc, &reg);
    v.set_controllers_open(true);
    v.set_controller_kind(ControllerKind::Cc(1));
    v.set_playhead(PPQ);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl
}

fn record_transport() -> DisplayList {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    v.set_state(TransportState {
        playing: true,
        recording: false,
        metronome: true,
        position: 2 * BAR + PPQ + PPQ / 2,
    });
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl
}

#[test]
fn arrangement_matches_golden() {
    check("arrangement_1x.json", &record_arrangement().to_json());
}

#[test]
fn piano_roll_matches_golden() {
    check("piano_roll_1x.json", &record_piano_roll().to_json());
}

#[test]
fn piano_roll_with_the_controller_area_matches_golden() {
    check(
        "piano_roll_controllers_1x.json",
        &record_piano_roll_controllers().to_json(),
    );
}

#[test]
fn piano_roll_with_a_scale_matches_golden() {
    check(
        "piano_roll_scale_1x.json",
        &record_piano_roll_scale(Fold::Off).to_json(),
    );
}

#[test]
fn piano_roll_folded_to_the_scale_matches_golden() {
    check(
        "piano_roll_scale_fold_1x.json",
        &record_piano_roll_scale(Fold::Scale).to_json(),
    );
}

#[test]
fn piano_roll_folded_to_the_used_notes_matches_golden() {
    check(
        "piano_roll_used_fold_1x.json",
        &record_piano_roll_scale(Fold::Used).to_json(),
    );
}

#[test]
fn transport_bar_matches_golden() {
    check("transport_1x.json", &record_transport().to_json());
}

#[test]
fn recordings_are_deterministic_and_complete() {
    let a = record_arrangement();
    assert_eq!(a.to_json(), record_arrangement().to_json());
    let texts = a.texts();
    for t in [
        "Bass",
        "Lead",
        "Keys",
        "Cutoff",
        "+  Track for Pad",
        "SNAP",
        "Beat",
        "1",
        "5",
        "M",
        "S",
        "R",
    ] {
        assert!(
            texts.contains(&t),
            "arrangement is missing {t:?}: {texts:?}"
        );
    }
    let p = record_piano_roll();
    assert_eq!(p.to_json(), record_piano_roll().to_json());
    let texts = p.texts();
    for t in ["PIANO ROLL", "VEL", "C2", "C3", "1/16", "QUANT 100%"] {
        assert!(texts.contains(&t), "piano roll is missing {t:?}: {texts:?}");
    }
    let t = record_transport();
    let texts = t.texts();
    for s in ["3.2.07680", "00:04.750", "120.00", "4/4"] {
        assert!(texts.contains(&s), "transport is missing {s:?}: {texts:?}");
    }
}
