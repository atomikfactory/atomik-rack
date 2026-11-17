//! The tab score <-> song conversion: what an import makes of a score, what an export makes of
//! clips, and the round trip of both.

use super::*;
use atomik_tab::instrument::find_tuning;
use atomik_tab::score::{Bend, SlideKind, TechFlags, Techniques};

pub(crate) fn guitar() -> atomik_tab::Tuning {
    find_tuning("guitar6", "standard").unwrap().tuning.clone()
}

/// A guitar track with a bend, a hammer-on, a slide, a tie, a harmonic-free plain note and
/// lyrics, a bass track, a drum track, a tempo change, a meter change and a marker.
pub(crate) fn fixture_score() -> TabScore {
    let mut score = TabScore::new();
    score.title = "Made up".to_owned();
    let mut g = TabTrack::new("Guitar", "guitar6", guitar());
    g.capo = atomik_tab::Capo::at(2);
    let mut n0 = TabNote::new(0, PPQ, 1, 3);
    n0.tech = Techniques {
        bend: Some(Bend::up(200)),
        ..Techniques::default()
    };
    let mut n1 = TabNote::new(PPQ, PPQ, 1, 5);
    n1.tech = Techniques::default().with(TechFlags::HAMMER_ON);
    let mut n2 = TabNote::new(2 * PPQ, PPQ, 2, 2);
    n2.tech = Techniques {
        slide: Some(SlideKind::Legato),
        ..Techniques::default()
    };
    let mut tie_a = TabNote::new(3 * PPQ, PPQ, 4, 0);
    tie_a.velocity = 60;
    let mut tie_b = TabNote::new(4 * PPQ, PPQ, 4, 0);
    tie_b.tie = true;
    g.notes = vec![n0, n1, n2, tie_a, tie_b, TabNote::new(5 * PPQ, PPQ, 5, 3)];
    g.lyrics = vec![(0, "la".to_owned()), (PPQ, "di".to_owned())];
    g.chords = vec![
        (0, atomik_tab::ChordSymbol::parse("Am").unwrap()),
        (4 * PPQ, atomik_tab::ChordSymbol::parse("G").unwrap()),
    ];
    let mut b = TabTrack::new(
        "Bass",
        "bass4",
        find_tuning("bass4", "standard").unwrap().tuning.clone(),
    );
    b.notes = vec![
        TabNote::new(0, 2 * PPQ, 3, 3),
        TabNote::new(2 * PPQ, 2 * PPQ, 2, 5),
    ];
    let mut d = TabTrack::new("Drums", "", atomik_tab::Tuning::from_opens(&[0], 0));
    d.is_percussion = true;
    d.notes = vec![
        TabNote::new(0, PPQ / 4, 0, 36),
        TabNote::new(PPQ, PPQ / 4, 0, 38),
    ];
    score.tracks = vec![g, b, d];
    score.tempo = vec![(0, 100.0), (8 * PPQ, 140.0)];
    score.meter = vec![(0, 4, 4), (2, 3, 4)];
    score.key = vec![(
        0,
        atomik_tab::score::KeySignature {
            fifths: 1,
            minor: false,
        },
    )];
    score.markers = vec![(0, "Intro".to_owned()), (4 * PPQ, "Verse".to_owned())];
    score
}

#[test]
fn the_instrument_is_one_decision_and_names_pick_catalogue_instruments() {
    assert_eq!(instrument_for("guitar6"), "atomik.plucked");
    assert_eq!(instrument_for(""), instrument_for("banjo5"));
    assert_eq!(patch_for("banjo5"), "Banjo");
    assert_eq!(patch_for("bass4"), "Electric Bass Finger");
    assert_eq!(
        patch_for(""),
        "Steel Acoustic",
        "a custom tuning gets the default"
    );
    for (name, id) in [
        ("Lead Guitar", "guitar6"),
        ("Fingered BASS", "bass4"),
        ("Banjo", "banjo5"),
        ("Uke", "ukulele_soprano"),
        ("Mandolin 2", "mandolin"),
        ("12 string rhythm", "guitar12"),
        ("", "guitar6"),
    ] {
        assert_eq!(instrument_for_name(name), id, "{name}");
    }
}

#[test]
fn an_import_makes_a_track_per_tab_track_with_the_sidecar() {
    let song = tab_song(&fixture_score());
    assert!(song.warnings.is_empty(), "{:?}", song.warnings);
    assert_eq!(song.song.tracks.len(), 3);
    let g = &song.song.tracks[0];
    assert_eq!(g.name, "Guitar");
    // The tie joined the two notes into one of two beats; capo 2 raises the pitches.
    assert_eq!(g.notes.len(), 5);
    let tied = g.notes.iter().find(|n| n.tick == 3 * PPQ).unwrap();
    assert_eq!(tied.length, 2 * PPQ);
    assert_eq!(tied.key, 47, "A2 open string with the capo on 2");
    let tab = g.midi.tab.as_ref().expect("a sidecar");
    assert_eq!(tab.instrument_id, "guitar6");
    assert_eq!(tab.capo.fret, 2);
    assert_eq!(tab.notes.len(), 5);
    assert_eq!(tab.lyrics.len(), 2);
    let first = tab.note_of(0, g.notes[0].key).unwrap();
    assert_eq!((first.string, first.fret), (1, 3));
    assert!(first.tech.bend.is_some());
    // The bend is rendered into the note's expression.
    assert!(!g.midi.expression.is_empty());
    // The drums are plain notes with their keys.
    let d = &song.song.tracks[2];
    assert!(d.midi.tab.is_none());
    assert_eq!(d.notes.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 38]);
    // Song facts.
    let tempo = song.song.tempo.as_ref().unwrap();
    assert_eq!((tempo.len(), tempo[1].tick), (2, 8 * PPQ));
    let meter = song.song.meter.as_ref().unwrap();
    assert_eq!((meter[1].bar, meter[1].num, meter[1].den), (3, 3, 4));
    assert_eq!(song.song.markers.len(), 2);
    assert_eq!(song.song.chords.len(), 2);
    assert!(song.song.key.is_some());
    assert_eq!(song.instrument, "atomik.plucked");
    // The sounds follow the tracks: Plucked Strings with the instrument's patch, the drums plain.
    assert_eq!(
        song.sounds,
        [
            TabSound {
                instrument: "atomik.plucked",
                patch: Some("Steel Acoustic")
            },
            TabSound {
                instrument: "atomik.plucked",
                patch: Some("Electric Bass Finger")
            },
            TabSound {
                instrument: crate::app_core::IMPORT_INSTRUMENT,
                patch: None
            },
        ]
    );
}

#[test]
fn notes_beyond_the_neck_are_left_out_and_reported() {
    let mut score = fixture_score();
    score.tracks[0]
        .notes
        .push(TabNote::new(9 * PPQ, PPQ, 0, 40));
    let song = tab_song(&score);
    assert_eq!(song.warnings.len(), 1, "{:?}", song.warnings);
    assert!(song.warnings[0].contains("Guitar"));
}

#[test]
fn an_empty_score_converts_to_nothing_and_a_hostile_one_does_not_panic() {
    assert!(tab_song(&TabScore::new()).song.tracks.is_empty());
    let mut score = TabScore::new();
    let mut t = TabTrack::new("", "", atomik_tab::Tuning::from_opens(&[], 12));
    t.notes = vec![TabNote::new(0, 1, 3, 3)];
    score.tracks.push(t);
    score.meter = vec![(5, 4, 4), (2, 7, 3), (2, 3, 4)];
    score.tempo = vec![(100, 5000.0)];
    let song = tab_song(&score);
    assert_eq!(song.song.tracks.len(), 1);
    assert_eq!(song.song.tracks[0].name, "Track 1");
    assert_eq!(song.song.tempo.as_ref().unwrap()[0].tick, 0);
    assert_eq!(song.song.meter.as_ref().unwrap()[0].bar, 1);
}

#[test]
fn unknown_files_are_refused_with_a_message() {
    assert!(read_tab(b"nothing to see").unwrap_err().contains("not a"));
    assert!(read_tab(&[]).is_err());
    assert_eq!(sniff_tab(b"nothing"), None);
}
