//! Display-list goldens of the Chord Trainer pane: the setup, a timed drill on a guitar, a banjo
//! in open G, a ukulele with a capo, a listen drill, the scores table, the list editor and the
//! instrument list. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test chord_trainer_snapshot`.

mod support;

use atomik_tab::grade;
use atomik_tab::trainer::{PairResult, Scores};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Rect;
use atomik_ui::{ChordTrainerView, DisplayList, Key, Lang, Modifiers, Theme, UiEvent};
use std::path::PathBuf;
use support::sheet::SHEET_BOUNDS;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn golden_path(dir: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(dir)
        .join(name)
}

fn check(name: &str, actual: &str) {
    let path = if name.starts_with("fr/") {
        golden_path("fr", name.trim_start_matches("fr/"))
    } else {
        golden_path("", name)
    };
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

fn ok(v: &mut ChordTrainerView, id: &str, arg: Option<&str>) {
    match v.command(id, arg, &reference_doc(), &registry()) {
        CommandOutcome::Handled(_) => {}
        other => panic!("{id} {arg:?}: {other:?}"),
    }
}

fn chroma_of(v: &ChordTrainerView, i: usize) -> [f32; 12] {
    let mask = grade::pc_mask(&v.chords()[i].pitch_classes());
    let mut c = [0.03f32; 12];
    for (pc, x) in c.iter_mut().enumerate() {
        if mask >> pc & 1 == 1 {
            *x = 1.0;
        }
    }
    c
}

fn scene(
    bounds: Rect,
    setup: impl FnOnce(&mut ChordTrainerView),
) -> (ChordTrainerView, DisplayList) {
    let doc = reference_doc();
    let reg = registry();
    let mut v = ChordTrainerView::new(Theme::dark());
    v.set_bounds(bounds);
    v.set_input(Some("In 1".to_owned()));
    v.set_seed(5);
    v.sync(&doc, &reg);
    setup(&mut v);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    (v, dl)
}

fn scores() -> Scores {
    let mut s = Scores::new();
    for (key, attempts, hits, cpm) in [
        ("Am|C", 12, 11, 48.0),
        ("C|G", 20, 16, 36.0),
        ("D|G", 8, 5, 24.0),
        ("Em|G", 10, 10, 54.0),
    ] {
        s.record(&PairResult {
            key: key.to_owned(),
            attempts,
            hits,
            cpm,
        });
    }
    s
}

/// A drill running at 90 BPM, two beats a chord, ticked to a time inside its third window.
fn running(v: &mut ChordTrainerView, mode: &str) {
    ok(v, "editor.trainer_mode", Some(mode));
    for _ in 0..6 {
        ok(v, "editor.trainer_bpm", Some("up"));
    }
    ok(v, "editor.trainer_beats", Some("down"));
    ok(v, "editor.trainer_beats", Some("down"));
    ok(v, "editor.trainer_start", None);
    v.tick(100.0);
    v.tick(105.0);
}

#[test]
fn the_setup_matches_golden() {
    let (v, dl) = scene(SHEET_BOUNDS, |_| {});
    assert_eq!(v.chord_names().len(), 7);
    let json = dl.to_json();
    assert!(json.contains("Chord names") || json.contains("Timed"));
    check("chord_trainer_setup_1x.json", &json);
}

#[test]
fn a_timed_drill_on_a_guitar_matches_golden() {
    let (v, dl) = scene(SHEET_BOUNDS, |v| {
        v.set_scores(scores());
        running(v, "timed");
    });
    assert!(v.running());
    let json = dl.to_json();
    assert!(json.contains("Changes: "));
    check("chord_trainer_timed_guitar_1x.json", &json);
}

#[test]
fn a_banjo_in_open_g_matches_golden() {
    let (_, dl) = scene(SHEET_BOUNDS, |v| {
        v.set_instrument("banjo5");
        for _ in 0..7 {
            ok(v, "editor.trainer_key", Some("up"));
        }
        running(v, "timed");
    });
    check("chord_trainer_banjo_1x.json", &dl.to_json());
}

#[test]
fn a_ukulele_with_a_capo_and_a_pair_matches_golden() {
    let (_, dl) = scene(SHEET_BOUNDS, |v| {
        v.set_instrument("ukulele_concert");
        ok(v, "editor.trainer_capo", Some("up"));
        ok(v, "editor.trainer_capo", Some("up"));
        ok(v, "editor.trainer_order", Some("pair"));
        ok(v, "editor.trainer_pair", Some("b"));
        running(v, "timed");
    });
    check("chord_trainer_ukulele_1x.json", &dl.to_json());
}

#[test]
fn a_listen_drill_with_a_heard_chord_matches_golden() {
    let (v, dl) = scene(SHEET_BOUNDS, |v| {
        v.set_scores(scores());
        running(v, "listen");
        let first = chroma_of(v, 0);
        v.hear(101.0, first);
        let second = chroma_of(v, 1);
        v.hear(103.0, second);
        // The next chord is being heard.
        let third = chroma_of(v, 2);
        v.hear(104.0, third);
    });
    assert!(v.drill().unwrap().changes() >= 2);
    check("chord_trainer_listen_1x.json", &dl.to_json());
}

#[test]
fn the_scores_table_matches_golden() {
    let (_, dl) = scene(SHEET_BOUNDS, |v| v.set_scores(scores()));
    let json = dl.to_json();
    assert!(json.contains("Best scores") && json.contains("Em - G"));
    check("chord_trainer_scores_1x.json", &json);
}

#[test]
fn the_list_editor_and_the_instrument_list_match_golden() {
    let doc = reference_doc();
    let (v, dl) = scene(SHEET_BOUNDS, |v| {
        ok(v, "editor.trainer_source", Some("list"));
        ok(v, "editor.trainer_edit_list", None);
        for ch in "Am F C xyz G7".chars() {
            v.handle(
                &UiEvent::Key {
                    key: Key::Char(ch),
                    modifiers: Modifiers::NONE,
                    pressed: true,
                },
                &doc,
                &registry(),
            );
        }
    });
    assert!(v.is_text_owner());
    check("chord_trainer_list_editor_1x.json", &dl.to_json());
    let (v, dl) = scene(SHEET_BOUNDS, |v| v.open_instrument_list());
    assert!(v.is_text_owner());
    check("chord_trainer_instruments_1x.json", &dl.to_json());
}

#[test]
fn a_narrow_pane_keeps_the_drill_in_view_matches_golden() {
    let (_, dl) = scene(Rect::new(0.0, 0.0, 480.0, 420.0), |v| running(v, "timed"));
    check("chord_trainer_narrow_1x.json", &dl.to_json());
}

#[test]
fn the_drill_in_french_matches_golden() {
    let (list, _) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            scene(SHEET_BOUNDS, |v| {
                v.set_scores(scores());
                running(v, "verified");
            })
            .1
        })
    });
    assert!(list.to_json().contains("Meilleurs scores"));
    check("fr/chord_trainer_drill_1x.json", &list.to_json());
}

/// Writes PNG previews of the scenarios to `target/preview` (run with `--ignored`).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn write_previews() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let doc = reference_doc();
    let reg = registry();
    type Setup = fn(&mut ChordTrainerView);
    let scenes: Vec<(&str, Setup)> = vec![
        ("trainer_setup.png", |_| {}),
        ("trainer_guitar.png", |v| {
            v.set_scores(scores());
            running(v, "timed")
        }),
        ("trainer_banjo.png", |v| {
            v.set_instrument("banjo5");
            for _ in 0..7 {
                ok(v, "editor.trainer_key", Some("up"));
            }
            running(v, "timed")
        }),
        ("trainer_ukulele.png", |v| {
            v.set_instrument("ukulele_concert");
            ok(v, "editor.trainer_capo", Some("up"));
            ok(v, "editor.trainer_order", Some("pair"));
            running(v, "verified")
        }),
        ("trainer_listen.png", |v| {
            running(v, "listen");
            let c = chroma_of(v, 0);
            v.hear(101.0, c);
        }),
        ("trainer_list.png", |v| {
            ok(v, "editor.trainer_source", Some("list"));
            ok(v, "editor.trainer_edit_list", None);
            v.set_edit_text("Am F C xyz");
        }),
    ];
    for (name, setup) in scenes {
        let mut v = ChordTrainerView::new(Theme::dark());
        v.set_bounds(SHEET_BOUNDS);
        v.set_input(Some("In 1".to_owned()));
        v.sync(&doc, &reg);
        setup(&mut v);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(900, 520, &mut text);
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
