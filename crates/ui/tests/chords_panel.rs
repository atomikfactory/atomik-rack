//! The Detect Chords panel: its rows step and ask for a new score, the buttons need chords to put,
//! the key check box shows only with a key to offer, and the picture matches its golden.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    ChordChanges, ChordsAction, ChordsPreview, ChordsRow, ChordsValues, ChordsView, DisplayList,
    Modifiers, NotesProgress, PreviewChord, Theme, UiAction, UiEvent,
};
use support::down;

const BAR: i64 = 3840;

fn click(v: &mut ChordsView, pos: Point) -> Vec<UiAction> {
    v.handle(&down(pos))
}

fn view() -> ChordsView {
    let mut v = ChordsView::new(Theme::dark(), ChordsValues::default());
    v.set_bounds(Rect::new(0.0, 0.0, 900.0, 600.0));
    v
}

fn preview(key: Option<&str>) -> ChordsPreview {
    let chords = [("C", 1.0), ("G/B", 0.9), ("Am7", 0.3), ("F", 0.8)]
        .iter()
        .enumerate()
        .map(|(i, (symbol, confidence))| PreviewChord {
            tick: i as i64 * BAR,
            length: BAR,
            symbol: (*symbol).to_owned(),
            confidence: *confidence,
        })
        .collect();
    ChordsPreview {
        span: (0, 4 * BAR),
        chords,
        key: key.map(str::to_owned),
    }
}

fn rescore() -> Vec<UiAction> {
    vec![UiAction::DetectChords(ChordsAction::Rescore)]
}

#[test]
fn the_rows_step_and_ask_for_a_new_score() {
    let mut v = view();
    assert_eq!(v.value_text(ChordsRow::Changes), "Every half bar");
    assert_eq!(v.value_text(ChordsRow::Chords), "Full (sevenths)");
    assert_eq!(v.value_text(ChordsRow::Slash), "On");
    let plus = v.plus_rect(ChordsRow::Changes).center();
    assert_eq!(click(&mut v, plus), rescore());
    assert_eq!(v.values().changes, ChordChanges::Bar);
    click(&mut v, plus);
    assert_eq!(v.values().changes, ChordChanges::Bar, "stops at a bar");
    let minus = v.minus_rect(ChordsRow::Changes).center();
    click(&mut v, minus);
    click(&mut v, minus);
    assert_eq!(v.values().changes, ChordChanges::Beat);
    let value = v.value_rect(ChordsRow::Chords).center();
    assert_eq!(click(&mut v, value), rescore());
    assert_eq!(v.value_text(ChordsRow::Chords), "Simple (triads)");
    let row = v.row_rect(ChordsRow::Slash).center();
    let wheel = v.handle(&UiEvent::Wheel {
        pos: row,
        delta: Vec2::new(0.0, 1.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(wheel, rescore());
    assert!(!v.values().slash);
}

#[test]
fn putting_the_chords_needs_chords_and_cancel_stops_a_run() {
    let mut v = view();
    let (replace, merge, cancel) = (
        v.replace_rect().center(),
        v.merge_rect().center(),
        v.cancel_rect().center(),
    );
    v.set_progress(Some(NotesProgress {
        text: "Listening for chords: 40 %".into(),
        fraction: 0.4,
    }));
    assert!(click(&mut v, replace).is_empty());
    assert_eq!(
        click(&mut v, cancel),
        vec![UiAction::DetectChords(ChordsAction::Cancel)]
    );
    v.set_progress(None);
    v.set_preview(Some(ChordsPreview::default()));
    assert!(click(&mut v, merge).is_empty(), "no chords found");
    v.set_preview(Some(preview(None)));
    assert_eq!(
        click(&mut v, replace),
        vec![UiAction::DetectChords(ChordsAction::Replace)]
    );
    assert_eq!(
        click(&mut v, merge),
        vec![UiAction::DetectChords(ChordsAction::Merge)]
    );
    assert_eq!(
        click(&mut v, cancel),
        vec![UiAction::DetectChords(ChordsAction::Close)]
    );
    let close = v.close_rect().center();
    assert_eq!(
        click(&mut v, close),
        vec![UiAction::DetectChords(ChordsAction::Close)]
    );
}

#[test]
fn the_key_box_shows_only_with_a_key_and_toggles() {
    let paint = |v: &ChordsView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl.to_json()
    };
    let mut v = view();
    v.set_preview(Some(preview(None)));
    assert!(!paint(&v).contains("Set song key"));
    let key = v.key_rect().center();
    click(&mut v, key);
    assert!(v.values().set_key, "no box, no toggle");
    v.set_preview(Some(preview(Some("A minor"))));
    assert!(paint(&v).contains("Set song key to A minor"));
    assert!(click(&mut v, key).is_empty());
    assert!(!v.values().set_key);
    // Every part sits inside the panel.
    let panel = v.panel();
    for r in [
        v.strip_rect(),
        v.key_rect(),
        v.cancel_rect(),
        v.close_rect(),
    ] {
        assert!(panel.contains(r.origin()) && r.x1 <= panel.x1 && r.y1 <= panel.y1);
    }
    for t in [
        v.help_at(v.replace_rect().center()),
        v.help_at(v.merge_rect().center()),
        v.help_at(v.strip_rect().center()),
        v.help_at(key),
    ] {
        assert!(t.is_some_and(|h| h.text.chars().count() <= 90));
    }
}

#[test]
fn detect_chords_and_key_are_menu_commands_without_a_key() {
    let r = atomik_ui::keymap::Registry::bundled();
    for id in [
        "sequencer.detect_chords_from_audio",
        "sequencer.detect_key_from_audio",
    ] {
        assert!(r.validate(id, None), "{id}");
        assert!(r.get(id).unwrap().menu, "{id}");
    }
}

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

#[test]
fn the_panel_with_a_preview_matches_its_golden() {
    let mut v = view();
    v.set_target("Piano.wav, bars 1 to 4".into());
    v.set_preview(Some(preview(Some("C major"))));
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("chords_panel_1x.json", &dl.to_json());
}
