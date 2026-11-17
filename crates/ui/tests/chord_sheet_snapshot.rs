//! Display-list goldens of the Chord Sheet pane: a guitar sheet, a ukulele sheet with its diagram
//! strip, a banjo popup, a transposed sheet with a capo, the text editor, a narrow pane that
//! wraps, and the empty state.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test chord_sheet_snapshot`.

mod support;

use atomik_model::RackDoc;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    ChordSheetView, DisplayList, Key, Modifiers, PointerButton, SheetButton, Theme, UiEvent,
};
use std::path::PathBuf;
use support::sheet::*;
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

fn click(v: &mut ChordSheetView, doc: &RackDoc, p: Point) {
    v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        doc,
        &registry(),
    );
}

fn press(v: &mut ChordSheetView, doc: &RackDoc, b: SheetButton) {
    let p = v.button_center(b).expect("a button");
    click(v, doc, p);
}

/// A view with `setup` run on it, painted.
fn scene(
    doc: &RackDoc,
    bounds: Rect,
    setup: impl FnOnce(&mut ChordSheetView, &RackDoc),
) -> (ChordSheetView, DisplayList) {
    let reg = registry();
    let mut v = ChordSheetView::new(Theme::dark());
    v.set_bounds(bounds);
    v.sync(doc, &reg);
    setup(&mut v, doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    (v, dl)
}

#[test]
fn a_guitar_sheet_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (_, dl) = scene(&doc, SHEET_BOUNDS, |_, _| {});
    let json = dl.to_json();
    assert!(json.contains("Lantern Road") && json.contains("Verse 1"));
    check("chord_sheet_guitar_1x.json", &json);
}

#[test]
fn a_ukulele_sheet_with_its_diagram_strip_matches_golden() {
    let doc = sheet_doc(vec![sheet(HARBOUR)]);
    let (_, dl) = scene(&doc, SHEET_BOUNDS, |v, _| {
        v.set_instrument("ukulele_concert")
    });
    check("chord_sheet_ukulele_1x.json", &dl.to_json());
}

#[test]
fn a_banjo_popup_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (v, dl) = scene(&doc, SHEET_BOUNDS, |v, d| {
        v.set_instrument("banjo5");
        let at = v.chord_position("Em").expect("Em");
        click(v, d, at);
    });
    assert_eq!(v.popup_chord().as_deref(), Some("Em"));
    check("chord_sheet_banjo_popup_1x.json", &dl.to_json());
}

#[test]
fn a_transposed_sheet_with_a_capo_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (v, dl) = scene(&doc, SHEET_BOUNDS, |v, d| {
        press(v, d, SheetButton::TransposeUp);
        press(v, d, SheetButton::TransposeUp);
        press(v, d, SheetButton::CapoUp);
        press(v, d, SheetButton::CapoUp);
    });
    assert_eq!(v.chord_names(), vec!["G", "D", "Em", "C"]);
    let json = dl.to_json();
    assert!(json.contains("Capo 2"), "the capo is in the header line");
    check("chord_sheet_transposed_capo_1x.json", &json);
}

#[test]
fn the_text_editor_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (v, dl) = scene(&doc, SHEET_BOUNDS, |v, d| press(v, d, SheetButton::Edit));
    assert!(v.editing());
    check("chord_sheet_edit_1x.json", &dl.to_json());
}

#[test]
fn a_narrow_pane_wraps_lines_and_keeps_the_chords_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (_, dl) = scene(&doc, Rect::new(0.0, 0.0, 260.0, 520.0), |_, _| {});
    check("chord_sheet_narrow_1x.json", &dl.to_json());
}

#[test]
fn the_empty_state_and_the_instrument_list_match_golden() {
    let doc = sheet_doc(Vec::new());
    let (_, dl) = scene(&doc, SHEET_BOUNDS, |_, _| {});
    check("chord_sheet_empty_1x.json", &dl.to_json());
    let doc = sheet_doc(vec![sheet(HARBOUR)]);
    let (v, dl) = scene(&doc, SHEET_BOUNDS, |v, d| {
        press(v, d, SheetButton::Instrument)
    });
    assert!(v.picking_instrument());
    check("chord_sheet_instruments_1x.json", &dl.to_json());
}

#[test]
fn the_rename_box_matches_golden() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (v, dl) = scene(&doc, SHEET_BOUNDS, |v, d| press(v, d, SheetButton::Rename));
    assert!(v.renaming());
    let _ = Key::Enter;
    check("chord_sheet_rename_1x.json", &dl.to_json());
}

#[test]
fn the_practice_bar_under_the_toolbar_matches_golden() {
    use atomik_ui::{PracticeState, TrainerState};
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (_, dl) = scene(&doc, SHEET_BOUNDS, |v, _| {
        v.set_practice(PracticeState {
            speed: 75,
            count_in: 2,
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
    assert!(json.contains("75%") && json.contains("Count-in 2"));
    check("chord_sheet_practice_1x.json", &json);
}

#[test]
fn a_narrow_pane_drops_the_controls_that_do_not_fit() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (_, dl) = scene(&doc, Rect::new(0.0, 0.0, 420.0, 400.0), |_, _| {});
    check("chord_sheet_practice_narrow_1x.json", &dl.to_json());
}

/// Writes PNG previews of the scenarios to `target/preview` (run with `--ignored`).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn write_previews() {
    use atomik_ui::TextEngine;
    use atomik_ui::cpu_canvas::CpuCanvas;
    let reg = registry();
    type Setup = fn(&mut ChordSheetView, &RackDoc);
    let lantern = || sheet_doc(vec![sheet(LANTERN)]);
    let harbour = || sheet_doc(vec![sheet(HARBOUR)]);
    let scenes: Vec<(&str, RackDoc, Setup)> = vec![
        ("sheet_guitar.png", lantern(), |_, _| {}),
        ("sheet_ukulele.png", harbour(), |v, _| {
            v.set_instrument("ukulele_concert")
        }),
        ("sheet_banjo_popup.png", lantern(), |v, d| {
            v.set_instrument("banjo5");
            let at = v.chord_position("Em").unwrap();
            click(v, d, at);
        }),
        ("sheet_capo.png", lantern(), |v, d| {
            press(v, d, SheetButton::TransposeUp);
            press(v, d, SheetButton::CapoUp);
        }),
        ("sheet_edit.png", lantern(), |v, d| {
            press(v, d, SheetButton::Edit)
        }),
        ("sheet_instruments.png", harbour(), |v, d| {
            press(v, d, SheetButton::Instrument)
        }),
        ("sheet_rename.png", lantern(), |v, d| {
            press(v, d, SheetButton::Rename)
        }),
        ("sheet_empty.png", sheet_doc(Vec::new()), |_, _| {}),
    ];
    for (name, doc, setup) in scenes {
        let mut v = ChordSheetView::new(Theme::dark());
        v.set_bounds(SHEET_BOUNDS);
        v.sync(&doc, &reg);
        setup(&mut v, &doc);
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
