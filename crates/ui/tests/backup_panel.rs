//! The restore list of the backup copies: it says it is looking until it has rows, shows date and
//! size, chooses with the pointer and the arrows, RESTORE, a double click and Enter ask to open the
//! chosen copy, CLOSE and Esc close, a long list scrolls, a failure shows its reason, and the text
//! and geometry match the golden.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    BackupAction, BackupItem, BackupView, DisplayList, Key, Modifiers, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn item(n: usize) -> BackupItem {
    BackupItem {
        when: format!("2026-11-10 09:{:02}:12", 59 - n),
        size: format!("{}.{} KB", 12 - n % 5, n % 10),
    }
}

fn items(n: usize) -> Vec<BackupItem> {
    (0..n).map(item).collect()
}

fn view(n: usize) -> BackupView {
    let mut v = BackupView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_rows(items(n));
    v
}

fn actions(out: Vec<UiAction>) -> Vec<BackupAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Backup(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn press(v: &mut BackupView, key: Key) -> Vec<BackupAction> {
    actions(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn click(v: &mut BackupView, p: Point, clicks: u8) -> Vec<BackupAction> {
    let mut out = v.handle(&down_with(p, atomik_ui::PointerButton::Primary, clicks));
    out.extend(v.handle(&up(p)));
    actions(out)
}

fn texts(v: &BackupView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_says_it_is_looking_until_it_has_the_rows() {
    let mut v = BackupView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let t = texts(&v);
    assert!(
        t.iter().any(|s| s.starts_with("Looking for backup")),
        "{t:?}"
    );
    // Nothing to restore yet: Enter and RESTORE do nothing.
    assert!(press(&mut v, Key::Enter).is_empty());
    let p = v.restore_rect().center();
    assert!(click(&mut v, p, 1).is_empty());
    assert_eq!(v.selected(), None);
}

#[test]
fn the_panel_shows_the_date_and_size_of_each_copy_newest_first() {
    let v = view(3);
    let t = texts(&v);
    for want in [
        "RESTORE FROM BACKUP",
        "CLOSE",
        "RESTORE",
        "3 copies, newest first",
        "2026-11-10 09:59:12",
        "2026-11-10 09:58:12",
        "12.0 KB",
    ] {
        assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
    }
    assert_eq!(v.selected(), Some(0));
    let one = view(1);
    assert!(texts(&one).iter().any(|s| s == "1 copy, newest first"));
}

#[test]
fn an_empty_list_and_a_failure_say_so() {
    let v = view(0);
    assert!(
        texts(&v)
            .iter()
            .any(|s| s.starts_with("There are no backup copies"))
    );
    let mut v = view(2);
    v.set_failed("the folder is gone".to_owned());
    assert!(v.rows().is_empty());
    assert!(texts(&v).iter().any(|s| s == "the folder is gone"));
}

#[test]
fn restore_asks_for_the_chosen_copy() {
    let mut v = view(5);
    // The newest is chosen: Enter and the button restore it.
    assert_eq!(press(&mut v, Key::Enter), vec![BackupAction::Restore(0)]);
    let p = v.restore_rect().center();
    assert_eq!(click(&mut v, p, 1), vec![BackupAction::Restore(0)]);
    // The arrows move the choice and stop at the ends.
    press(&mut v, Key::Down);
    press(&mut v, Key::Down);
    assert_eq!(press(&mut v, Key::Enter), vec![BackupAction::Restore(2)]);
    for _ in 0..9 {
        press(&mut v, Key::Down);
    }
    assert_eq!(v.selected(), Some(4));
    press(&mut v, Key::Home);
    assert_eq!(v.selected(), Some(0));
    press(&mut v, Key::End);
    assert_eq!(v.selected(), Some(4));
    press(&mut v, Key::Up);
    assert_eq!(press(&mut v, Key::Enter), vec![BackupAction::Restore(3)]);
}

#[test]
fn a_click_chooses_and_a_double_click_restores() {
    let mut v = view(5);
    let r = v.row_rect(3).unwrap();
    assert!(click(&mut v, r.center(), 1).is_empty());
    assert_eq!(v.selected(), Some(3));
    assert_eq!(click(&mut v, r.center(), 2), vec![BackupAction::Restore(3)]);
}

#[test]
fn close_and_escape_close() {
    let mut v = view(2);
    let p = v.close_rect().center();
    assert_eq!(click(&mut v, p, 1), vec![BackupAction::Close]);
    assert_eq!(press(&mut v, Key::Escape), vec![BackupAction::Close]);
}

#[test]
fn a_long_list_scrolls_with_the_wheel_and_the_arrows() {
    let mut v = view(30);
    assert!(v.row_rect(0).is_some());
    assert!(v.row_rect(20).is_none());
    for _ in 0..25 {
        press(&mut v, Key::Down);
    }
    // The chosen row is always on show.
    assert_eq!(v.selected(), Some(25));
    assert!(v.row_rect(25).is_some());
    assert!(v.row_rect(0).is_none());
    // The wheel moves the window of rows, not the choice.
    let at = Point::new(450.0, 200.0);
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, 1.0),
        pos: at,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.selected(), Some(25));
    for _ in 0..40 {
        v.handle(&UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: at,
            modifiers: Modifiers::NONE,
        });
    }
    assert!(v.row_rect(29).is_some());
}

#[test]
fn the_buttons_have_tooltips_of_one_short_line() {
    let v = view(2);
    for r in [v.close_rect(), v.restore_rect()] {
        let h = v.help_at(r.center()).expect("a tooltip");
        assert!(
            !h.text.is_empty() && h.text.chars().count() <= 90,
            "{}",
            h.text
        );
    }
    assert!(v.help_at(Point::new(1.0, 1.0)).is_none());
}

#[test]
fn the_panel_text_and_geometry_match_the_golden() {
    let mut out = String::new();
    for (name, v) in [
        ("looking", {
            let mut v = BackupView::new(Theme::dark());
            v.set_bounds(BOUNDS);
            v
        }),
        ("three copies", view(3)),
        ("none", view(0)),
        ("long list", view(30)),
    ] {
        out.push_str(&format!("== {name}\n"));
        let p = v.panel();
        out.push_str(&format!(
            "panel {:.1} {:.1} {:.1} {:.1}\n",
            p.x0, p.y0, p.x1, p.y1
        ));
        out.push_str(&texts(&v).join("\n"));
        out.push('\n');
        for i in 0..v.rows().len().min(2) {
            let r = v.row_rect(i).unwrap();
            out.push_str(&format!(
                "row {i}: {:.1} {:.1} {:.1} {:.1}\n",
                r.x0, r.y0, r.x1, r.y1
            ));
        }
        for (label, r) in [("RESTORE", v.restore_rect()), ("CLOSE", v.close_rect())] {
            out.push_str(&format!(
                "{label}: {:.1} {:.1} {:.1} {:.1}\n",
                r.x0, r.y0, r.x1, r.y1
            ));
        }
    }
    starter::check_golden("backup_panel.txt", &out);
}
