//! The versions panel: it lists the versions it was given, the name takes typing, SAVE VERSION and
//! Enter in the name emit the save, the list chooses with the pointer and the arrows, RESTORE,
//! a double-click and Enter in the list emit the restore, PROTECT toggles an automatic version
//! only, CLOSE closes, COMPARE and C ask for a comparison (with a Shift-marked second version or the
//! song now) and the comparison is a scrollable text that BACK, Enter and Backspace leave.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, Key, Modifiers, Theme, UiAction, UiEvent, VersionItem, VersionsAction,
    VersionsView,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn item(id: u32, name: &str, automatic: bool, protected: bool) -> VersionItem {
    VersionItem {
        id,
        when: format!("2026-11-06 12:{id:02}"),
        name: name.to_owned(),
        automatic,
        protected,
    }
}

/// Newest first.
fn items() -> Vec<VersionItem> {
    vec![
        item(5, "", true, false),
        item(4, "Before the mix", false, true),
        item(3, "", true, true),
        item(2, "", true, false),
        item(1, "", true, false),
    ]
}

fn view(items: Vec<VersionItem>) -> VersionsView {
    let mut v = VersionsView::new(Theme::dark(), items);
    v.set_bounds(BOUNDS);
    v
}

fn actions(out: Vec<UiAction>) -> Vec<VersionsAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Versions(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn press(v: &mut VersionsView, key: Key) -> Vec<VersionsAction> {
    actions(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn type_text(v: &mut VersionsView, text: &str) {
    for c in text.chars() {
        press(v, Key::Char(c));
    }
}

fn click(v: &mut VersionsView, p: Point) -> Vec<VersionsAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    actions(out)
}

fn click_on(v: &mut VersionsView, rect: impl Fn(&VersionsView) -> Rect) -> Vec<VersionsAction> {
    let p = rect(v).center();
    click(v, p)
}

fn texts(v: &VersionsView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_shows_its_labels_and_the_versions() {
    let t = texts(&view(items()));
    for want in [
        "VERSIONS",
        "Name",
        "SAVE VERSION",
        "CLOSE",
        "RESTORE",
        "2026-11-06 12:05",
        "Before the mix",
        "NAMED",
        "PROTECTED",
        "Saved automatically",
    ] {
        assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
    }
    let t = texts(&view(Vec::new()));
    assert!(t.iter().any(|s| s.starts_with("No versions yet")), "{t:?}");
}

#[test]
fn typing_edits_the_name_and_save_version_sends_it() {
    let mut v = view(items());
    assert!(v.name_has_keys());
    type_text(&mut v, "Idea A");
    assert_eq!(v.name(), "Idea A");
    assert_eq!(
        click_on(&mut v, |v| v.save_rect()),
        vec![VersionsAction::Save("Idea A".to_owned())]
    );
    assert_eq!(
        press(&mut v, Key::Enter),
        vec![VersionsAction::Save("Idea A".to_owned())]
    );
    // The name stops at its limit.
    type_text(&mut v, &"x".repeat(200));
    assert_eq!(v.name().chars().count(), 80);
}

#[test]
fn the_app_clears_the_name_after_a_save_and_the_choice_stays() {
    let mut v = view(items());
    type_text(&mut v, "Idea");
    click_on(&mut v, |v| v.row_rect(2));
    assert_eq!(v.selected_id(), Some(3));
    let mut more = items();
    more.insert(0, item(6, "Idea", false, true));
    v.set_items(more, true);
    assert_eq!(v.name(), "");
    assert_eq!(v.selected_id(), Some(3), "the chosen version stays chosen");
}

#[test]
fn rows_choose_with_the_pointer_and_the_arrows_and_restore_with_enter_or_a_double_click() {
    let mut v = view(items());
    assert_eq!(v.selected_id(), Some(5), "the newest is chosen first");
    assert_eq!(click_on(&mut v, |v| v.row_rect(3)), vec![]);
    assert_eq!(v.selected_id(), Some(2));
    assert!(
        !v.name_has_keys(),
        "a click on a row gives the list the keys"
    );
    press(&mut v, Key::Down);
    assert_eq!(v.selected_id(), Some(1));
    press(&mut v, Key::Down);
    assert_eq!(v.selected_id(), Some(1), "the last row stays");
    press(&mut v, Key::Up);
    assert_eq!(v.selected_id(), Some(2));
    assert_eq!(press(&mut v, Key::Enter), vec![VersionsAction::Restore(2)]);
    let p = v.row_rect(0).center();
    let out = actions(v.handle(&down_with(p, atomik_ui::PointerButton::Primary, 2)));
    assert_eq!(out, vec![VersionsAction::Restore(5)]);
    assert_eq!(
        click_on(&mut v, |v| v.restore_rect()),
        vec![VersionsAction::Restore(5)]
    );
}

#[test]
fn up_from_the_first_row_and_tab_move_the_keys_between_the_name_and_the_list() {
    let mut v = view(items());
    press(&mut v, Key::Tab);
    assert!(!v.name_has_keys());
    press(&mut v, Key::Up);
    assert!(v.name_has_keys());
    press(&mut v, Key::Down);
    assert!(!v.name_has_keys());
    press(&mut v, Key::Tab);
    assert!(v.name_has_keys());
}

#[test]
fn protect_toggles_an_automatic_version_and_is_off_for_a_named_one() {
    let mut v = view(items());
    assert!(v.can_protect());
    assert_eq!(
        click_on(&mut v, |v| v.protect_rect()),
        vec![VersionsAction::Protect {
            id: 5,
            protected: true
        }]
    );
    click_on(&mut v, |v| v.row_rect(2));
    assert_eq!(
        click_on(&mut v, |v| v.protect_rect()),
        vec![VersionsAction::Protect {
            id: 3,
            protected: false
        }],
        "a protected version is unprotected"
    );
    assert!(texts(&v).iter().any(|s| s == "UNPROTECT"));
    click_on(&mut v, |v| v.row_rect(1));
    assert!(!v.can_protect());
    assert_eq!(click_on(&mut v, |v| v.protect_rect()), vec![]);
}

#[test]
fn close_closes_and_an_empty_list_restores_nothing() {
    let mut v = view(items());
    assert_eq!(
        click_on(&mut v, |v| v.close_rect()),
        vec![VersionsAction::Close]
    );
    let mut empty = view(Vec::new());
    assert_eq!(empty.selected_id(), None);
    assert_eq!(click_on(&mut empty, |v| v.restore_rect()), vec![]);
    press(&mut empty, Key::Tab);
    assert_eq!(press(&mut empty, Key::Enter), vec![]);
    assert!(!empty.can_protect());
}

#[test]
fn a_long_list_scrolls_with_the_wheel_and_follows_the_choice() {
    let many: Vec<VersionItem> = (1..=30u32)
        .rev()
        .map(|id| item(id, "", true, false))
        .collect();
    let mut v = view(many);
    let first_shown = |v: &VersionsView| {
        let t = texts(v);
        (1..=30u32)
            .rev()
            .find(|id| t.iter().any(|s| s == &format!("2026-11-06 12:{id:02}")))
            .unwrap()
    };
    assert_eq!(first_shown(&v), 30);
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -3.0),
        pos: v.row_rect(2).center(),
        modifiers: Modifiers::NONE,
    });
    assert!(first_shown(&v) < 30);
    // End chooses the oldest and scrolls it into view.
    press(&mut v, Key::Tab);
    press(&mut v, Key::End);
    assert_eq!(v.selected_id(), Some(1));
    assert_eq!(first_shown(&v), 10, "the last ten rows show");
    press(&mut v, Key::Home);
    assert_eq!(v.selected_id(), Some(30));
    assert_eq!(first_shown(&v), 30);
}

#[test]
fn the_buttons_have_tooltips() {
    let v = view(items());
    for r in [
        v.close_rect(),
        v.save_rect(),
        v.restore_rect(),
        v.protect_rect(),
        v.compare_rect(),
    ] {
        assert!(v.help_at(r.center()).is_some());
    }
    assert!(v.help_at(Point::new(1.0, 1.0)).is_none());
}

fn press_with(v: &mut VersionsView, key: Key, modifiers: Modifiers) -> Vec<VersionsAction> {
    actions(v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }))
}

const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
    meta: false,
};

fn shift_click(v: &mut VersionsView, p: Point) {
    v.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: SHIFT,
        clicks: 1,
    });
}

#[test]
fn compare_asks_for_the_chosen_version_against_the_song_now() {
    let mut v = view(items());
    assert_eq!(
        click_on(&mut v, |v| v.compare_rect()),
        vec![VersionsAction::Compare { id: 5, with: None }]
    );
    press(&mut v, Key::Tab);
    press(&mut v, Key::Down);
    assert_eq!(
        press(&mut v, Key::Char('c')),
        vec![VersionsAction::Compare { id: 4, with: None }]
    );
    assert_eq!(
        press(&mut v, Key::Char('C')),
        vec![VersionsAction::Compare { id: 4, with: None }]
    );
    // C in the name is typed text.
    press(&mut v, Key::Up);
    press(&mut v, Key::Up);
    assert!(v.name_has_keys());
    assert_eq!(press(&mut v, Key::Char('c')), vec![]);
    assert_eq!(v.name(), "c");
}

#[test]
fn compare_with_nothing_chosen_asks_for_nothing() {
    let mut v = view(Vec::new());
    assert_eq!(click_on(&mut v, |v| v.compare_rect()), vec![]);
}

#[test]
fn shift_marks_a_second_version_to_compare_with() {
    let mut v = view(items());
    // Shift-click on row 3 chooses it and marks the row the choice left (the newest).
    let p = v.row_rect(3).center();
    shift_click(&mut v, p);
    assert_eq!(v.selected_id(), Some(2));
    assert_eq!(v.marked_id(), Some(5));
    assert_eq!(
        click_on(&mut v, |v| v.compare_rect()),
        vec![VersionsAction::Compare {
            id: 2,
            with: Some(5)
        }]
    );
    assert!(texts(&v).iter().any(|s| s == "COMPARE WITH"));
    // A plain click chooses and keeps the mark; shift-clicking back onto the mark clears it.
    let p = v.row_rect(0).center();
    shift_click(&mut v, p);
    assert_eq!(v.selected_id(), Some(5));
    assert_eq!(v.marked_id(), None);
    // Shift with the arrows marks the row left behind.
    press_with(&mut v, Key::Down, SHIFT);
    assert_eq!((v.selected_id(), v.marked_id()), (Some(4), Some(5)));
    press_with(&mut v, Key::Down, SHIFT);
    assert_eq!((v.selected_id(), v.marked_id()), (Some(3), Some(5)));
    assert_eq!(
        press(&mut v, Key::Char('c')),
        vec![VersionsAction::Compare {
            id: 3,
            with: Some(5)
        }]
    );
    // Going back onto the marked row clears the mark: it would compare a version with itself.
    press_with(&mut v, Key::Up, SHIFT);
    press_with(&mut v, Key::Up, SHIFT);
    assert_eq!((v.selected_id(), v.marked_id()), (Some(5), None));
    // The app replacing the rows drops a mark whose version is gone.
    shift_click(&mut v, p);
    let p = v.row_rect(1).center();
    shift_click(&mut v, p);
    assert_eq!(v.marked_id(), Some(5));
    v.set_items(items().into_iter().skip(1).collect(), false);
    assert_eq!(v.marked_id(), None);
}

fn report_lines(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("  ~ Line {i}")).collect()
}

#[test]
fn a_comparison_replaces_the_list_and_back_returns() {
    let mut v = view(items());
    v.show_report(
        "2026-11-06 12:02",
        "The song now",
        vec!["SONG".into(), "  ~ Tempo: 120 -> 90".into()],
    );
    assert!(v.report_open());
    let t = texts(&v);
    for want in [
        "2026-11-06 12:02  ->  The song now",
        "SONG",
        "  ~ Tempo: 120 -> 90",
        "BACK",
        "CLOSE",
    ] {
        assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
    }
    assert!(
        !t.iter().any(|s| s == "RESTORE" || s == "SAVE VERSION"),
        "{t:?}"
    );
    // The list is out of reach: a click on a row, or on the old buttons, does nothing.
    assert_eq!(click_on(&mut v, |v| v.row_rect(1)), vec![]);
    assert_eq!(click_on(&mut v, |v| v.protect_rect()), vec![]);
    assert_eq!(v.selected_id(), Some(5));
    assert!(v.report_open());
    assert_eq!(click_on(&mut v, |v| v.back_rect()), vec![]);
    assert!(!v.report_open());
    // Enter and Backspace leave it too; close_report says whether there was one.
    v.show_report("a", "b", report_lines(2));
    assert_eq!(press(&mut v, Key::Enter), vec![]);
    assert!(!v.report_open());
    v.show_report("a", "b", report_lines(2));
    press(&mut v, Key::Backspace);
    assert!(!v.report_open());
    v.show_report("a", "b", report_lines(2));
    assert!(v.close_report());
    assert!(!v.close_report());
    // CLOSE still closes the panel from a comparison.
    v.show_report("a", "b", report_lines(2));
    assert_eq!(
        click_on(&mut v, |v| v.close_rect()),
        vec![VersionsAction::Close]
    );
}

#[test]
fn a_long_comparison_scrolls_with_the_wheel_and_the_keys() {
    let total = atomik_ui::REPORT_LINES * 2 + 5;
    let mut v = view(items());
    v.show_report("a", "b", report_lines(total));
    let shown = |v: &VersionsView| {
        let t = texts(v);
        (1..=total)
            .filter(|i| t.iter().any(|s| s == &format!("  ~ Line {i}")))
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(&v), (1..=atomik_ui::REPORT_LINES).collect::<Vec<_>>());
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -1.0),
        pos: v.row_rect(2).center(),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.report_scroll(), 3);
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, 1.0),
        pos: v.row_rect(2).center(),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.report_scroll(), 0);
    press(&mut v, Key::Down);
    assert_eq!(v.report_scroll(), 1);
    press(&mut v, Key::PageDown);
    assert_eq!(v.report_scroll(), 1 + atomik_ui::REPORT_LINES);
    press(&mut v, Key::End);
    assert_eq!(v.report_scroll(), total - atomik_ui::REPORT_LINES);
    assert_eq!(*shown(&v).last().unwrap(), total, "the last line shows");
    press(&mut v, Key::Down);
    assert_eq!(
        v.report_scroll(),
        total - atomik_ui::REPORT_LINES,
        "no further"
    );
    press(&mut v, Key::Home);
    assert_eq!(v.report_scroll(), 0);
    press(&mut v, Key::Up);
    assert_eq!(v.report_scroll(), 0);
    // A comparison that fits does not scroll.
    v.show_report("a", "b", report_lines(3));
    press(&mut v, Key::End);
    assert_eq!(v.report_scroll(), 0);
}

#[test]
fn a_comparison_has_a_back_tooltip_and_the_list_buttons_have_none() {
    let mut v = view(items());
    v.show_report("a", "b", report_lines(1));
    assert!(v.help_at(v.back_rect().center()).is_some());
    assert!(v.help_at(v.close_rect().center()).is_some());
    assert!(v.help_at(v.protect_rect().center()).is_none());
}
