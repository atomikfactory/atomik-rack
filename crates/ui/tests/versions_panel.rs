//! The versions panel: it lists the versions it was given, the name takes typing, SAVE VERSION and
//! Enter in the name emit the save, the list chooses with the pointer and the arrows, RESTORE,
//! a double-click and Enter in the list emit the restore, PROTECT toggles an automatic version
//! only, CLOSE closes.

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
    ] {
        assert!(v.help_at(r.center()).is_some());
    }
    assert!(v.help_at(Point::new(1.0, 1.0)).is_none());
}
