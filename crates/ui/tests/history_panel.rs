//! The Undo History panel: rows, the current row, the dimmed redo rows, clicks and keys that go
//! to a row, and a long history that scrolls and keeps the current row in view.

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{HistoryRow, HistoryView, Key, Modifiers, PointerButton, Theme, UiAction, UiEvent};

fn rows(n: usize) -> Vec<HistoryRow> {
    (0..n)
        .map(|i| HistoryRow {
            label: format!("Step {}", i + 1),
            age_secs: Some(((n - i) as u64) * 70),
        })
        .collect()
}

fn panel(n: usize, position: usize) -> HistoryView {
    let mut v = HistoryView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 236.0, 220.0));
    v.sync("Opened", Some(900), rows(n), position);
    v
}

fn click(v: &mut HistoryView, row: usize) -> Vec<UiAction> {
    let at = v.row_rect(row).expect("the row shows").center();
    v.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::default(),
        clicks: 1,
    })
}

fn key(v: &mut HistoryView, key: Key) -> Vec<UiAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::default(),
        pressed: true,
    })
}

#[test]
fn the_starting_row_comes_first_and_each_step_has_a_row() {
    let v = panel(3, 3);
    assert_eq!(v.start_label(), "Opened");
    assert_eq!(v.rows().len(), 3);
    assert_eq!(v.position(), 3);
    assert!(v.row_rect(0).is_some() && v.row_rect(3).is_some());
    assert!(v.row_rect(4).is_none(), "one row per step plus the start");
}

#[test]
fn a_click_on_a_row_asks_to_go_to_it_and_a_click_on_the_current_row_does_nothing() {
    let mut v = panel(3, 2);
    assert_eq!(click(&mut v, 1), vec![UiAction::HistoryGoto(1)]);
    assert_eq!(click(&mut v, 3), vec![UiAction::HistoryGoto(3)]);
    assert_eq!(click(&mut v, 0), vec![UiAction::HistoryGoto(0)]);
    assert!(click(&mut v, 2).is_empty(), "already there");
}

#[test]
fn up_and_down_step_one_row_and_home_and_end_go_to_the_ends() {
    let mut v = panel(3, 2);
    assert_eq!(key(&mut v, Key::Up), vec![UiAction::HistoryGoto(1)]);
    assert_eq!(key(&mut v, Key::Down), vec![UiAction::HistoryGoto(3)]);
    assert_eq!(key(&mut v, Key::Home), vec![UiAction::HistoryGoto(0)]);
    assert_eq!(key(&mut v, Key::End), vec![UiAction::HistoryGoto(3)]);
    let mut at_end = panel(3, 3);
    assert!(key(&mut at_end, Key::Down).is_empty());
    let mut at_start = panel(3, 0);
    assert!(key(&mut at_start, Key::Up).is_empty());
}

#[test]
fn a_long_history_scrolls_and_keeps_the_current_row_in_view() {
    let mut v = panel(1000, 1000);
    let shown = (0..=1000).filter(|i| v.row_rect(*i).is_some()).count();
    assert!(shown > 3 && shown < 20, "{shown} rows show at once");
    assert!(
        v.row_rect(1000).is_some(),
        "the newest step shows at the bottom"
    );
    // Going back far enough scrolls the list up to the current row.
    v.sync("Opened", Some(900), rows(1000), 10);
    assert!(v.row_rect(10).is_some());
    assert!(v.top() <= 10);
    // The wheel scrolls without moving the position.
    let top = v.top();
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -3.0),
        pos: Point::new(100.0, 100.0),
        modifiers: Modifiers::default(),
    });
    assert!(v.top() > top);
    assert_eq!(v.position(), 10);
    // A new position in view leaves the scroll alone; one out of view scrolls to it.
    v.sync("Opened", Some(900), rows(1000), 500);
    assert!(v.row_rect(500).is_some());
}

#[test]
fn a_shorter_history_clamps_the_scroll() {
    let mut v = panel(1000, 1000);
    v.sync("Opened", None, rows(2), 2);
    assert_eq!(v.top(), 0);
    assert!(v.row_rect(2).is_some());
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
fn the_panel_with_dimmed_redo_rows_matches_its_golden() {
    let mut v = HistoryView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 236.0, 220.0));
    let steps = ["Add Device", "Change Parameter", "Move Device", "Add Clip"]
        .into_iter()
        .zip([300, 125, 40, 5])
        .map(|(label, age)| HistoryRow {
            label: label.to_owned(),
            age_secs: Some(age),
        })
        .collect();
    // Two steps are undone: they are the dimmed rows under the current one.
    v.sync("New song", Some(600), steps, 2);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl);
    check("history_panel_1x.json", &dl.to_json());
}
