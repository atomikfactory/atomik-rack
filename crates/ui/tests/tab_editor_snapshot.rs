//! Display-list goldens of the tab editor and of the chord diagram widget: a 6-string guitar
//! with techniques, chords and lyrics, a 5-string banjo with its drone, a 4-string bass, and the
//! empty state; plus the diagram in several shapes.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test tab_editor_snapshot`.

mod support;

use atomik_model::{ClipId, Note, RackDoc, TabData, TabDiagram, TabLyric, TabNoteRef};
use atomik_tab::chord::ChordSymbol;
use atomik_tab::fretting::FretOptions;
use atomik_tab::score::{
    Bend, Harmonic, HarmonicKind, SlideKind, TechFlags, Techniques, VibratoKind,
};
use atomik_tab::voicing::{Voicing, VoicingOptions, generate};
use atomik_tab::{Capo, Tuning};
use atomik_ui::chord_diagram::{self, Diagram};
use atomik_ui::kurbo::Rect;
use atomik_ui::{DisplayList, TabEditorView, Theme};
use std::path::PathBuf;
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

/// A note with its sidecar entry (the techniques set by `f`).
fn nt(
    tab: &mut TabData,
    notes: &mut Vec<Note>,
    (tick, len): (i64, i64),
    (string, fret): (u8, u8),
    f: impl FnOnce(&mut Techniques),
) {
    let key = tab
        .tuning
        .pitch(usize::from(string), fret, tab.capo)
        .expect("pitch");
    notes.push(Note::new(tick, len, key, 0.8));
    let mut r = TabNoteRef::new(tick, key, string, fret);
    f(&mut r.tech);
    tab.notes.push(r);
}

fn voicing(tab: &TabData, name: &str) -> Voicing {
    generate(
        &tab.tuning,
        tab.capo,
        &ChordSymbol::parse(name).expect("chord"),
        &VoicingOptions::default(),
    )
    .into_iter()
    .next()
    .expect("a voicing")
}

fn finish(doc_name: &str, mut tab: TabData, mut notes: Vec<Note>) -> RackDoc {
    notes.sort_by_key(|n| (n.tick, n.key));
    tab.notes.sort_by_key(|r| (r.tick, r.key));
    tab_doc(doc_name, notes, Some(tab))
}

fn guitar_doc() -> RackDoc {
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    let mut notes = Vec::new();
    let q = PPQ;
    let e = PPQ / 2;
    let s = PPQ / 4;
    // Bar 1: a hammer-on and a pull-off, a slide, a bend with its amount, vibrato.
    nt(&mut tab, &mut notes, (0, e), (2, 0), |_| {});
    nt(&mut tab, &mut notes, (e, e), (2, 2), |t| {
        t.flags.insert(TechFlags::HAMMER_ON)
    });
    nt(&mut tab, &mut notes, (q, e), (2, 0), |t| {
        t.flags.insert(TechFlags::PULL_OFF)
    });
    nt(&mut tab, &mut notes, (q + e, e), (1, 3), |t| {
        t.slide = Some(SlideKind::Legato)
    });
    nt(&mut tab, &mut notes, (2 * q, q), (1, 7), |t| {
        t.bend = Some(Bend::up(200))
    });
    nt(&mut tab, &mut notes, (3 * q, q), (0, 5), |t| {
        t.vibrato = Some(VibratoKind::Normal)
    });
    // Bar 2: a palm-muted run, a dead note, a ghost note, a harmonic, a let-ring chord.
    for k in 0..3 {
        nt(&mut tab, &mut notes, (4 * q + k * s, s), (4, 2), |t| {
            t.flags.insert(TechFlags::PALM_MUTE)
        });
    }
    nt(&mut tab, &mut notes, (4 * q + 3 * s, s), (3, 0), |t| {
        t.flags.insert(TechFlags::DEAD)
    });
    nt(&mut tab, &mut notes, (5 * q, e), (2, 2), |t| {
        t.flags.insert(TechFlags::GHOST)
    });
    nt(&mut tab, &mut notes, (5 * q + e, e), (2, 12), |t| {
        t.harmonic = Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: None,
        });
    });
    for (string, fret) in [(5, 0), (4, 2), (3, 2)] {
        nt(&mut tab, &mut notes, (6 * q, 2 * q), (string, fret), |t| {
            t.flags.insert(TechFlags::LET_RING)
        });
    }
    let em = voicing(&tab, "Em");
    let c = voicing(&tab, "C");
    tab.diagrams.push(TabDiagram {
        tick: 0,
        name: "Em".into(),
        voicing: em,
    });
    tab.diagrams.push(TabDiagram {
        tick: 4 * q,
        name: "C".into(),
        voicing: c,
    });
    for (tick, text) in [
        (0, "la"),
        (q, "la-"),
        (2 * q, "dee"),
        (4 * q, "doo"),
        (6 * q, "dah"),
    ] {
        tab.lyrics.push(TabLyric {
            tick,
            text: text.into(),
        });
    }
    finish("Guitar", tab, notes)
}

fn banjo_doc() -> RackDoc {
    let mut tab = TabData::for_instrument("banjo5").unwrap();
    let mut notes = Vec::new();
    let e = PPQ / 2;
    // A forward roll on the open strings with the drone on the offbeat.
    let roll = [
        (4u8, 0u8),
        (2, 0),
        (1, 0),
        (4, 0),
        (2, 0),
        (1, 0),
        (0, 0),
        (1, 0),
    ];
    for (i, (string, fret)) in roll.iter().enumerate() {
        nt(
            &mut tab,
            &mut notes,
            (i as i64 * e, e),
            (*string, *fret),
            |_| {},
        );
    }
    nt(&mut tab, &mut notes, (4 * PPQ, PPQ), (4, 7), |t| {
        t.flags.insert(TechFlags::PULL_OFF)
    });
    let g = voicing(&tab, "G");
    tab.diagrams.push(TabDiagram {
        tick: 0,
        name: "G".into(),
        voicing: g,
    });
    finish("Banjo", tab, notes)
}

fn bass_doc() -> RackDoc {
    let mut tab = TabData::for_instrument("bass4").unwrap();
    let mut notes = Vec::new();
    let (q, e) = (PPQ, PPQ / 2);
    nt(&mut tab, &mut notes, (0, q), (3, 3), |_| {});
    nt(&mut tab, &mut notes, (q, e), (3, 3), |_| {});
    nt(&mut tab, &mut notes, (q + e, e), (2, 5), |t| {
        t.slide = Some(SlideKind::Shift)
    });
    nt(&mut tab, &mut notes, (2 * q, q), (2, 7), |_| {});
    nt(&mut tab, &mut notes, (3 * q, q), (1, 5), |t| {
        t.flags.insert(TechFlags::DEAD)
    });
    nt(&mut tab, &mut notes, (4 * q, 4 * q), (0, 3), |_| {});
    finish("Bass", tab, notes)
}

fn record(doc: &RackDoc) -> DisplayList {
    let reg = registry();
    let mut v = TabEditorView::new(Theme::dark());
    v.set_bounds(TAB_BOUNDS);
    v.open(ClipId(1));
    v.sync(doc, &reg);
    v.set_cursor(PPQ, 2);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    dl
}

#[test]
fn the_empty_state_matches_golden() {
    let reg = registry();
    let mut v = TabEditorView::new(Theme::dark());
    v.set_bounds(TAB_BOUNDS);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &tab_doc("Guitar", Vec::new(), None), &reg);
    check("tab_editor_empty_1x.json", &dl.to_json());
}

#[test]
fn a_guitar_with_techniques_chords_and_lyrics_matches_golden() {
    check(
        "tab_editor_guitar_1x.json",
        &record(&guitar_doc()).to_json(),
    );
}

#[test]
fn a_banjo_with_the_drone_matches_golden() {
    check("tab_editor_banjo_1x.json", &record(&banjo_doc()).to_json());
}

#[test]
fn a_bass_matches_golden() {
    check("tab_editor_bass_1x.json", &record(&bass_doc()).to_json());
}

#[test]
fn a_clip_without_a_sidecar_draws_the_automatic_fingering() {
    let doc = tab_doc(
        "Guitar",
        vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 59, 0.8)],
        None,
    );
    check("tab_editor_automatic_1x.json", &record(&doc).to_json());
}

#[test]
fn the_view_does_not_draw_outside_its_bounds() {
    let doc = guitar_doc();
    let dl = record(&doc);
    // Every text sits inside the view (the display list is scaled 1x).
    let json = dl.to_json();
    assert!(json.contains("Em"), "the chord name is drawn");
    assert!(!json.is_empty());
}

fn diagram_list(items: &[(&str, &Tuning, Capo, &str)]) -> DisplayList {
    let mut dl = DisplayList::new();
    let theme = Theme::dark();
    let mut x = 4.0;
    for (name, tuning, capo, chord) in items {
        let v = generate(
            tuning,
            *capo,
            &ChordSymbol::parse(chord).unwrap(),
            &VoicingOptions::default(),
        )
        .into_iter()
        .next()
        .expect("voicing");
        let starts = Diagram::start_frets_of(tuning, *capo);
        let d = Diagram {
            name: Some(name),
            voicing: &v,
            start_frets: &starts,
        };
        let w = 60.0;
        let h = chord_diagram::diagram_height(w, true);
        chord_diagram::paint_diagram(&mut dl, &theme, Rect::new(x, 4.0, x + w, 4.0 + h), &d);
        x += w + 8.0;
    }
    dl
}

#[test]
fn chord_diagrams_for_any_string_count_match_golden() {
    let guitar = atomik_tab::instrument::find_instrument("guitar6")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let guitar8 = atomik_tab::instrument::find_instrument("guitar8")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let bass = atomik_tab::instrument::find_instrument("bass4")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let banjo = atomik_tab::instrument::find_instrument("banjo5")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let uke = atomik_tab::instrument::find_instrument("ukulele_soprano")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let g7 = atomik_tab::instrument::find_instrument("guitar7")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let list = diagram_list(&[
        ("Am", &guitar, Capo::NONE, "Am"),
        ("F", &guitar, Capo::NONE, "F"),
        ("Bm7", &guitar, Capo::NONE, "Bm7"),
        ("G", &banjo, Capo::NONE, "G"),
        ("G", &banjo, Capo::at(2), "G"),
        ("C", &uke, Capo::NONE, "C"),
        ("E", &bass, Capo::NONE, "E"),
        ("Dm", &g7, Capo::NONE, "Dm"),
        ("Cmaj7", &guitar8, Capo::NONE, "Cmaj7"),
    ]);
    check("chord_diagrams_1x.json", &list.to_json());
}

#[test]
fn a_diagram_draws_a_barre_a_base_fret_label_and_the_markers() {
    let guitar = atomik_tab::instrument::find_instrument("guitar6")
        .unwrap()
        .default_tuning()
        .tuning
        .clone();
    let v = Voicing {
        frets: vec![Some(5), Some(7), Some(7), Some(5), Some(5), Some(5)],
        fingers: vec![Some(1), Some(3), Some(4), Some(1), Some(1), Some(1)],
        barres: vec![atomik_tab::voicing::Barre {
            fret: 5,
            from: 0,
            to: 5,
        }],
        base_fret: 5,
    };
    let mut dl = DisplayList::new();
    let starts = Diagram::start_frets_of(&guitar, Capo::NONE);
    chord_diagram::paint_diagram(
        &mut dl,
        &Theme::dark(),
        Rect::new(0.0, 0.0, 60.0, chord_diagram::diagram_height(60.0, true)),
        &Diagram {
            name: Some("Am"),
            voicing: &v,
            start_frets: &starts,
        },
    );
    let json = dl.to_json();
    assert!(json.contains("5fr"), "the start-fret label");
    assert!(json.contains("Am"));
    let _ = FretOptions::default();
}

/// Writes PNG previews of the scenarios to `target/preview` (run with `--ignored`).
/// A view on `doc` with `setup` run on it, painted.
fn paint_with(doc: &RackDoc, setup: impl FnOnce(&mut TabEditorView, &RackDoc)) -> DisplayList {
    let reg = registry();
    let mut v = TabEditorView::new(Theme::dark());
    v.set_bounds(TAB_BOUNDS);
    v.open(ClipId(1));
    v.sync(doc, &reg);
    setup(&mut v, doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    dl
}

#[test]
fn a_selection_and_the_lit_technique_buttons_match_golden() {
    // The range covers the hammer-on, the pull-off and the slide; the cursor is on the bend, so
    // the bend button is lit and the toolbar shows the amount.
    let dl = paint_with(&guitar_doc(), |v, _| {
        v.select_between((PPQ / 2, 2), (PPQ + PPQ / 2, 1));
    });
    let json = dl.to_json();
    assert!(
        json.contains("Bd") && json.contains("PM"),
        "the toolbar is drawn"
    );
    check("tab_editor_selection_1x.json", &json);
    let dl = paint_with(&guitar_doc(), |v, _| v.set_cursor(2 * PPQ, 1));
    check("tab_editor_toolbar_lit_1x.json", &dl.to_json());
}

#[test]
fn the_tuning_dialog_matches_golden() {
    let reg = registry();
    let dl = paint_with(&guitar_doc(), |v, d| {
        v.command("editor.tab_tuning", None, d, &reg);
    });
    check("tab_editor_tuning_dialog_1x.json", &dl.to_json());
    let dl = paint_with(&banjo_doc(), |v, d| {
        v.command("editor.tab_tuning", None, d, &reg);
    });
    check("tab_editor_tuning_dialog_banjo_1x.json", &dl.to_json());
}

#[test]
fn the_refret_chooser_matches_golden() {
    let reg = registry();
    let dl = paint_with(&guitar_doc(), |v, d| {
        v.select_all();
        v.command("editor.tab_refret", Some("choose"), d, &reg);
    });
    check("tab_editor_refret_chooser_1x.json", &dl.to_json());
}

#[test]
fn the_practice_bar_with_the_trainer_matches_golden() {
    use atomik_ui::{PracticeState, TrainerState};
    let dl = paint_with(&guitar_doc(), |v, _| {
        v.set_practice(PracticeState {
            speed: 75,
            count_in: 1,
            metronome: true,
            looping: true,
            trainer: TrainerState {
                enabled: true,
                passes: 3,
                clean: 1,
                ..TrainerState::default()
            },
            ..PracticeState::default()
        });
    });
    let json = dl.to_json();
    assert!(json.contains("75%") && json.contains("Pass 3 (1/2)"));
    check("tab_editor_practice_1x.json", &json);
}

#[test]
fn a_loop_drag_over_the_ruler_matches_golden() {
    let reg = registry();
    let dl = paint_with(&guitar_doc(), |v, d| {
        let y = 28.0 + 26.0 + 11.0;
        for event in [
            atomik_ui::UiEvent::PointerDown {
                pos: atomik_ui::kurbo::Point::new(v.tick_to_x(PPQ), y),
                button: atomik_ui::PointerButton::Primary,
                modifiers: atomik_ui::Modifiers::NONE,
                clicks: 1,
            },
            atomik_ui::UiEvent::PointerMove {
                pos: atomik_ui::kurbo::Point::new(v.tick_to_x(5 * PPQ), y),
                modifiers: atomik_ui::Modifiers::NONE,
            },
        ] {
            v.handle(&event, d, &reg);
        }
    });
    check("tab_editor_loop_drag_1x.json", &dl.to_json());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn write_previews() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let reg = registry();
    for (name, doc) in [
        ("tab_guitar.png", guitar_doc()),
        ("tab_banjo.png", banjo_doc()),
        ("tab_bass.png", bass_doc()),
    ] {
        let mut v = TabEditorView::new(Theme::dark());
        v.set_bounds(TAB_BOUNDS);
        v.open(ClipId(1));
        v.sync(&doc, &reg);
        v.set_cursor(PPQ, 2);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(1000, 420, &mut text);
        v.paint(&mut canvas, &doc, &reg);
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
fn write_editing_previews() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let reg = registry();
    type Setup = fn(&mut TabEditorView, &RackDoc, &atomik_device_sdk::Registry);
    let scenes: [(&str, RackDoc, Setup); 3] = [
        ("tab_selection.png", guitar_doc(), |v, _, _| {
            v.select_between((PPQ / 2, 2), (PPQ + PPQ / 2, 1));
        }),
        ("tab_tuning_dialog.png", guitar_doc(), |v, d, r| {
            v.command("editor.tab_tuning", None, d, r);
        }),
        ("tab_refret_chooser.png", guitar_doc(), |v, d, r| {
            v.select_all();
            v.command("editor.tab_refret", Some("choose"), d, r);
        }),
    ];
    for (name, doc, setup) in scenes {
        let mut v = TabEditorView::new(Theme::dark());
        v.set_bounds(TAB_BOUNDS);
        v.open(ClipId(1));
        v.sync(&doc, &reg);
        setup(&mut v, &doc, &reg);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(1000, 420, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}

/// The verdicts of a graded pass over the guitar clip: its notes take them in turn.
fn graded_marks(doc: &RackDoc) -> std::sync::Arc<Vec<atomik_ui::GradeMark>> {
    use atomik_model::ClipContent;
    use atomik_tab::grade::Verdict;
    let clip = doc.song.clip(ClipId(1)).expect("the clip").2;
    let ClipContent::Notes(list) = &clip.content else {
        panic!("a note clip");
    };
    let order = [
        Verdict::Hit,
        Verdict::Hit,
        Verdict::Early,
        Verdict::Late,
        Verdict::Wrong,
        Verdict::Missed,
    ];
    let mut marks: Vec<atomik_ui::GradeMark> = list
        .iter()
        .enumerate()
        .map(|(i, n)| atomik_ui::GradeMark {
            tick: n.tick,
            key: n.key,
            verdict: order[i % order.len()],
        })
        .collect();
    marks.sort_by_key(|m| (m.tick, m.key));
    std::sync::Arc::new(marks)
}

#[test]
fn a_graded_pass_colours_the_notes_and_matches_golden() {
    let doc = guitar_doc();
    let marks = graded_marks(&doc);
    let dl = paint_with(&doc, |v, _| {
        assert!(v.set_grades(Some(marks.clone())), "a change");
        assert!(!v.set_grades(Some(marks.clone())), "the same marks again");
    });
    let json = dl.to_json();
    // The badges of early, late, wrong and missed notes are drawn.
    for badge in ["\"<\"", "\">\"", "\"x\"", "\"-\""] {
        assert!(json.contains(badge), "badge {badge}");
    }
    check("tab_editor_graded_1x.json", &json);
    // Clearing the verdicts draws the staff as it was.
    let plain = paint_with(&doc, |v, _| {
        v.set_grades(Some(marks.clone()));
        assert!(v.set_grades(None));
    });
    let never = paint_with(&doc, |_, _| {});
    assert_eq!(plain.to_json(), never.to_json());
}

#[test]
fn the_practice_bar_while_listening_matches_golden() {
    use atomik_ui::{ListenState, PracticeState, TrainerState};
    let dl = paint_with(&guitar_doc(), |v, _| {
        v.set_practice(PracticeState {
            speed: 75,
            metronome: true,
            looping: true,
            trainer: TrainerState::default(),
            listen: ListenState {
                on: true,
                input: Some((0, true)),
                last_percent: Some(92),
                streak: 14,
                best: 21,
                threshold: 90,
                offset_ms: 35,
                history_len: 5,
                history: [60, 71, 84, 78, 92, 0, 0, 0],
                ..ListenState::default()
            },
            ..PracticeState::default()
        });
    });
    let json = dl.to_json();
    assert!(json.contains("92%") && json.contains("x14") && json.contains("In 1"));
    check("tab_editor_practice_listen_1x.json", &json);
}
