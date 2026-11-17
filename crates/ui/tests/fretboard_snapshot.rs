//! Display-list goldens of the fretboard: a 6-string guitar with a capo, a banjo with its drone,
//! a 4-string bass, an 8-string guitar, a left-handed neck, the playing state with the next
//! notes ghosted and a chord voicing with its fingers, a partial capo, and the strip under the
//! Tab Editor staff.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test fretboard_snapshot`.

mod support;

use atomik_model::{Note, TabData, TabDiagram, TabNoteRef};
use atomik_tab::Capo;
use atomik_tab::chord::ChordSymbol;
use atomik_tab::voicing::{VoicingOptions, generate};
use atomik_ui::fretboard::{FretChord, FretNote, FretboardData};
use atomik_ui::kurbo::Rect;
use atomik_ui::{
    DisplayList, FRETBOARD_HEIGHT, FretLabels, FretboardFeed, FretboardOptions, FretboardView,
    Theme,
};
use std::path::PathBuf;
use std::sync::Arc;
use support::tab::*;
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

fn note(tick: i64, length: i64, string: u8, fret: u8) -> FretNote {
    FretNote {
        tick,
        length,
        string,
        fret,
    }
}

fn data(instrument: &str, capo: Capo, notes: Vec<FretNote>) -> FretboardData {
    let tab = TabData::for_instrument(instrument).unwrap();
    FretboardData {
        tuning: tab.tuning,
        capo,
        notes,
        chords: Vec::new(),
    }
}

fn with_chord(mut d: FretboardData, name: &str) -> FretboardData {
    let v = generate(
        &d.tuning,
        d.capo,
        &ChordSymbol::parse(name).unwrap(),
        &VoicingOptions::default(),
    )
    .remove(0);
    d.chords.push(FretChord {
        tick: 0,
        name: name.into(),
        voicing: v,
    });
    d
}

fn draw(d: FretboardData, tick: i64, playing: bool, opts: FretboardOptions) -> String {
    let mut v = FretboardView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 900.0, FRETBOARD_HEIGHT));
    v.set_options(opts);
    v.set_feed(&FretboardFeed {
        data: Some(Arc::new(d)),
        tick: Some(tick),
        playing,
        ..FretboardFeed::default()
    });
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.to_json()
}

#[test]
fn a_guitar_with_a_capo_matches_golden() {
    // Frets are relative to the capo: fret 0 sounds at the capo.
    let d = data(
        "guitar6",
        Capo::at(2),
        vec![note(0, 100, 5, 0), note(0, 100, 4, 2), note(0, 100, 3, 2)],
    );
    check(
        "fretboard_guitar_capo_1x.json",
        &draw(d, 0, false, FretboardOptions::default()),
    );
}

#[test]
fn a_banjo_with_the_drone_matches_golden() {
    // The drone (string 4) starts at its fifth fret with a peg there; an open drone note sits at
    // the peg, a fretted one in its fret.
    let d = data(
        "banjo5",
        Capo::NONE,
        vec![
            note(0, 100, 4, 0),
            note(0, 100, 2, 0),
            note(100, 100, 4, 7),
            note(100, 100, 1, 1),
        ],
    );
    check(
        "fretboard_banjo_1x.json",
        &draw(d, 0, false, FretboardOptions::default()),
    );
}

#[test]
fn a_four_string_bass_matches_golden() {
    let d = data(
        "bass4",
        Capo::NONE,
        vec![note(0, 100, 3, 3), note(100, 100, 2, 5)],
    );
    check(
        "fretboard_bass_1x.json",
        &draw(
            d,
            0,
            false,
            FretboardOptions {
                labels: FretLabels::Frets,
                ..FretboardOptions::default()
            },
        ),
    );
}

#[test]
fn an_eight_string_guitar_matches_golden() {
    let d = data(
        "guitar8",
        Capo::partial(3, 0b0011_1100),
        vec![note(0, 100, 7, 0), note(0, 100, 2, 4), note(0, 100, 0, 12)],
    );
    check(
        "fretboard_eight_string_1x.json",
        &draw(d, 0, false, FretboardOptions::default()),
    );
}

#[test]
fn a_left_handed_neck_matches_golden() {
    let d = data(
        "guitar6",
        Capo::NONE,
        vec![note(0, 100, 5, 3), note(0, 100, 4, 2), note(0, 100, 3, 0)],
    );
    check(
        "fretboard_left_handed_1x.json",
        &draw(
            d,
            0,
            false,
            FretboardOptions {
                left_handed: true,
                ..FretboardOptions::default()
            },
        ),
    );
}

#[test]
fn the_playing_state_with_ghosts_and_a_voicing_matches_golden() {
    // At tick 150 the low C is sounding; the next group (the G on the fifth string and the high
    // E) is ghosted; the C chord's rings and fingers show under the notes, intervals to C.
    let d = with_chord(
        data(
            "guitar6",
            Capo::NONE,
            vec![
                note(0, 200, 4, 3),
                note(200, 100, 3, 2),
                note(200, 100, 0, 0),
            ],
        ),
        "C",
    );
    check(
        "fretboard_playing_ghosts_1x.json",
        &draw(
            d,
            150,
            true,
            FretboardOptions {
                chord_voicing: true,
                labels: FretLabels::Intervals,
                ..FretboardOptions::default()
            },
        ),
    );
}

#[test]
fn the_empty_state_matches_golden() {
    let mut v = FretboardView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 900.0, FRETBOARD_HEIGHT));
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("fretboard_empty_1x.json", &dl.to_json());
}

#[test]
fn the_strip_under_the_tab_staff_matches_golden() {
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    let mut notes = Vec::new();
    for (i, (string, fret)) in [(5u8, 3u8), (4, 2), (3, 0)].into_iter().enumerate() {
        let tick = i as i64 * PPQ;
        let key = tab
            .tuning
            .pitch(usize::from(string), fret, tab.capo)
            .unwrap();
        notes.push(Note::new(tick, PPQ, key, 0.8));
        tab.notes.push(TabNoteRef::new(tick, key, string, fret));
    }
    let v = generate(
        &tab.tuning,
        tab.capo,
        &ChordSymbol::parse("C").unwrap(),
        &VoicingOptions::default(),
    )
    .remove(0);
    tab.diagrams.push(TabDiagram {
        tick: 0,
        name: "C".into(),
        voicing: v,
    });
    let doc = tab_doc("Guitar", notes, Some(tab));
    let reg = registry();
    let mut ed = tab_editor(&doc, &reg);
    ed.set_bounds(Rect::new(0.0, 0.0, 1000.0, 560.0));
    ed.set_fretboard_shown(true);
    ed.set_fretboard_options(FretboardOptions {
        chord_voicing: true,
        ..FretboardOptions::default()
    });
    ed.set_cursor(PPQ, 4);
    let mut dl = DisplayList::new();
    ed.paint(&mut dl, &doc, &reg);
    check("tab_editor_fretboard_strip_1x.json", &dl.to_json());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn write_previews() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let opts = FretboardOptions::default();
    let chord_opts = FretboardOptions {
        chord_voicing: true,
        labels: FretLabels::Intervals,
        ..opts
    };
    let scenes = [
        (
            "fb_guitar_capo.png",
            data(
                "guitar6",
                Capo::at(2),
                vec![note(0, 100, 5, 0), note(0, 100, 4, 2), note(0, 100, 3, 2)],
            ),
            0,
            opts,
        ),
        (
            "fb_banjo.png",
            data(
                "banjo5",
                Capo::NONE,
                vec![note(0, 100, 4, 0), note(0, 100, 2, 0), note(100, 100, 4, 7)],
            ),
            0,
            opts,
        ),
        (
            "fb_eight.png",
            data(
                "guitar8",
                Capo::partial(3, 0b0011_1100),
                vec![note(0, 100, 7, 0), note(0, 100, 2, 4), note(0, 100, 0, 12)],
            ),
            0,
            opts,
        ),
        (
            "fb_left.png",
            data(
                "guitar6",
                Capo::NONE,
                vec![note(0, 100, 5, 3), note(0, 100, 4, 2)],
            ),
            0,
            FretboardOptions {
                left_handed: true,
                ..opts
            },
        ),
        (
            "fb_playing.png",
            with_chord(
                data(
                    "guitar6",
                    Capo::NONE,
                    vec![
                        note(0, 200, 4, 3),
                        note(200, 100, 3, 2),
                        note(200, 100, 0, 0),
                    ],
                ),
                "C",
            ),
            150,
            chord_opts,
        ),
    ];
    for (name, d, tick, o) in scenes {
        let mut v = FretboardView::new(Theme::dark());
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, FRETBOARD_HEIGHT));
        v.set_options(o);
        v.set_feed(&FretboardFeed {
            data: Some(Arc::new(d)),
            tick: Some(tick),
            playing: true,
            ..FretboardFeed::default()
        });
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(900, FRETBOARD_HEIGHT as u16, &mut text);
        v.paint(&mut canvas);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn write_strip_preview() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    let mut notes = Vec::new();
    for (i, (string, fret)) in [(5u8, 3u8), (4, 2), (3, 0)].into_iter().enumerate() {
        let tick = i as i64 * PPQ;
        let key = tab
            .tuning
            .pitch(usize::from(string), fret, tab.capo)
            .unwrap();
        notes.push(Note::new(tick, PPQ, key, 0.8));
        tab.notes.push(TabNoteRef::new(tick, key, string, fret));
    }
    let doc = tab_doc("Guitar", notes, Some(tab));
    let reg = registry();
    let mut ed = tab_editor(&doc, &reg);
    ed.set_bounds(Rect::new(0.0, 0.0, 1000.0, 460.0));
    ed.set_fretboard_shown(true);
    ed.set_cursor(PPQ, 4);
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(1000, 460, &mut text);
    ed.paint(&mut canvas, &doc, &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join("fb_strip.png");
    std::fs::write(&path, png).unwrap();
}

#[test]
fn sounding_notes_take_the_colour_of_their_verdict_and_match_golden() {
    use atomik_tab::grade::Verdict;
    use atomik_ui::FretGrade;
    // A C chord sounding: the low C right on time, the G late, the high E wrong.
    let notes = vec![
        note(0, 200, 4, 3),
        note(0, 200, 3, 2),
        note(0, 200, 0, 0),
        note(0, 200, 1, 1),
    ];
    let grade = |string, fret, verdict| FretGrade {
        tick: 0,
        string,
        fret,
        verdict,
    };
    let mut v = FretboardView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 900.0, FRETBOARD_HEIGHT));
    let feed = FretboardFeed {
        data: Some(Arc::new(data("guitar6", Capo::NONE, notes))),
        tick: Some(100),
        playing: false,
        grades: Some(Arc::new(vec![
            grade(4, 3, Verdict::Hit),
            grade(3, 2, Verdict::Late),
            grade(0, 0, Verdict::Wrong),
        ])),
    };
    assert!(v.set_feed(&feed));
    assert!(!v.set_feed(&feed), "nothing changed");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let json = dl.to_json();
    check("fretboard_graded_1x.json", &json);
    // The same notes without verdicts are drawn differently.
    let mut plain = FretboardView::new(Theme::dark());
    plain.set_bounds(Rect::new(0.0, 0.0, 900.0, FRETBOARD_HEIGHT));
    plain.set_feed(&FretboardFeed {
        grades: None,
        ..feed
    });
    let mut dl2 = DisplayList::new();
    plain.paint(&mut dl2);
    assert_ne!(json, dl2.to_json());
}
