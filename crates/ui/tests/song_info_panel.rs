//! The song information panel: it shows the texts it was given, typing edits the focused text up
//! to its limit, Tab moves between the texts, APPLY and Enter emit the apply action, CLOSE closes.

mod support;

use atomik_model::{MAX_INFO_TITLE_CHARS, SongInfo};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, InfoField, Key, Modifiers, SongInfoAction, SongInfoView, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn view(info: SongInfo) -> SongInfoView {
    let mut v = SongInfoView::new(Theme::dark(), info);
    v.set_bounds(BOUNDS);
    v
}

fn press(v: &mut SongInfoView, key: Key) -> Vec<SongInfoAction> {
    press_with(v, key, Modifiers::NONE)
}

fn press_with(v: &mut SongInfoView, key: Key, modifiers: Modifiers) -> Vec<SongInfoAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::SongInfo(a) => Some(a),
        _ => None,
    })
    .collect()
}

fn type_text(v: &mut SongInfoView, text: &str) {
    for c in text.chars() {
        press(v, Key::Char(c));
    }
}

fn click(v: &mut SongInfoView, p: Point) -> Vec<SongInfoAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::SongInfo(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &SongInfoView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn filled() -> SongInfo {
    SongInfo {
        title: "Example Song".into(),
        author: "Example Author".into(),
        notes: "one\ntwo".into(),
        url: "https://example.org".into(),
        email: "author@example.org".into(),
    }
}

#[test]
fn the_panel_shows_its_labels_and_the_texts_it_was_given() {
    let v = view(filled());
    let t = texts(&v);
    for want in [
        "SONG INFORMATION",
        "Title",
        "Author",
        "E-mail",
        "Web",
        "Notes",
        "APPLY",
        "CLOSE",
        "Example Song",
        "Example Author",
        "author@example.org",
        "https://example.org",
        "one",
        "two",
    ] {
        assert!(t.iter().any(|x| x.contains(want)), "{want} in {t:?}");
    }
    assert_eq!(v.info(), &filled());
}

#[test]
fn a_new_panel_starts_empty_with_the_keys_on_the_title() {
    let v = view(SongInfo::default());
    assert!(v.info().is_empty());
    assert_eq!(v.focus(), InfoField::Title);
}

#[test]
fn typing_edits_the_focused_text_and_backspace_removes_the_last_character() {
    let mut v = view(SongInfo::default());
    type_text(&mut v, "My song");
    assert_eq!(v.info().title, "My song");
    press(&mut v, Key::Backspace);
    assert_eq!(v.info().title, "My son");
    press(&mut v, Key::Tab);
    assert_eq!(v.focus(), InfoField::Author);
    type_text(&mut v, "Me");
    press(&mut v, Key::Space);
    type_text(&mut v, "Too");
    assert_eq!(v.info().author, "Me Too");
    assert_eq!(v.info().title, "My son", "the title kept its text");
    // An empty text does not underflow.
    let mut e = view(SongInfo::default());
    press(&mut e, Key::Backspace);
    assert!(e.info().is_empty());
}

#[test]
fn tab_and_shift_tab_cycle_through_the_texts() {
    let mut v = view(SongInfo::default());
    let mut seen = vec![v.focus()];
    for _ in 0..4 {
        press(&mut v, Key::Tab);
        seen.push(v.focus());
    }
    assert_eq!(seen, InfoField::ALL);
    press(&mut v, Key::Tab);
    assert_eq!(v.focus(), InfoField::Title, "wraps");
    press_with(
        &mut v,
        Key::Tab,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(v.focus(), InfoField::Notes, "Shift+Tab goes back");
}

#[test]
fn a_click_on_a_text_gives_it_the_keys() {
    let mut v = view(SongInfo::default());
    for field in [
        InfoField::Email,
        InfoField::Notes,
        InfoField::Url,
        InfoField::Title,
    ] {
        let at = v.field_rect(field).center();
        assert!(click(&mut v, at).is_empty());
        assert_eq!(v.focus(), field);
    }
    for pair in InfoField::ALL.windows(2) {
        assert!(
            v.field_rect(pair[0]).y1 <= v.field_rect(pair[1]).y0,
            "{pair:?} do not overlap"
        );
    }
}

#[test]
fn a_text_stops_at_its_limit_and_takes_no_control_characters() {
    let mut v = view(SongInfo::default());
    type_text(&mut v, &"x".repeat(MAX_INFO_TITLE_CHARS + 20));
    assert_eq!(v.info().title.chars().count(), MAX_INFO_TITLE_CHARS);
    assert_eq!(v.info().title.chars().count(), InfoField::Title.limit());
    press(&mut v, Key::Char('\u{7}'));
    press(&mut v, Key::Char('\n'));
    assert_eq!(v.info().title.chars().count(), MAX_INFO_TITLE_CHARS);
    assert!(v.info().check().is_ok());
    // The same holds for what a user types in a multi-byte alphabet.
    let mut w = view(SongInfo::default());
    type_text(&mut w, &"\u{e9}".repeat(MAX_INFO_TITLE_CHARS + 1));
    assert_eq!(w.info().title.chars().count(), MAX_INFO_TITLE_CHARS);
}

#[test]
fn enter_applies_in_a_one_line_text_and_is_a_new_line_in_the_notes() {
    let mut v = view(SongInfo::default());
    assert_eq!(press(&mut v, Key::Enter), [SongInfoAction::Apply]);
    for _ in 0..4 {
        press(&mut v, Key::Tab);
    }
    assert_eq!(v.focus(), InfoField::Notes);
    assert!(press(&mut v, Key::Enter).is_empty());
    type_text(&mut v, "second");
    press(&mut v, Key::Char('\t'));
    assert_eq!(v.info().notes, "\nsecond\t");
    assert!(v.info().check().is_ok());
}

#[test]
fn a_chord_is_not_typed() {
    let mut v = view(SongInfo::default());
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    press_with(&mut v, Key::Char('s'), ctrl);
    press_with(
        &mut v,
        Key::Char('s'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    assert!(v.info().is_empty());
}

#[test]
fn apply_and_close_emit_their_actions_and_the_panel_changes_nothing_by_itself() {
    let mut v = view(filled());
    let at = v.apply_rect().center();
    assert_eq!(click(&mut v, at), [SongInfoAction::Apply]);
    let at = v.close_rect().center();
    assert_eq!(click(&mut v, at), [SongInfoAction::Close]);
    assert_eq!(v.info(), &filled(), "the panel only reports");
    // A click elsewhere, or a secondary click, does nothing.
    assert!(click(&mut v, Point::new(2.0, 2.0)).is_empty());
    let out = v.handle(&down_with(
        v.apply_rect().center(),
        atomik_ui::PointerButton::Secondary,
        1,
    ));
    assert!(out.is_empty());
}

#[test]
fn the_notes_show_their_last_lines() {
    let notes: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    let v = view(SongInfo {
        notes,
        ..SongInfo::default()
    });
    let t = texts(&v);
    assert!(t.iter().any(|x| x.contains("line 12")), "{t:?}");
    assert!(
        !t.iter().any(|x| x == "line 1"),
        "the first lines scrolled out"
    );
}

#[test]
fn the_panel_stays_inside_a_small_window_and_has_tooltips() {
    let mut v = view(SongInfo::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 200.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 300.0 && p.y1 <= 200.0, "{p:?}");
    let v = view(SongInfo::default());
    assert!(v.help_at(v.apply_rect().center()).is_some());
    assert!(v.help_at(v.close_rect().center()).is_some());
    assert!(v.help_at(Point::new(2.0, 2.0)).is_none());
}
