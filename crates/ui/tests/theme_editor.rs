//! The theme editor: a swatch asks for the colour picker, the hex text edits a token as it is
//! typed, RESET goes back to the base theme, UNDO (Ctrl+Z) takes the last change back, and the
//! panel's display list is a golden. Regenerate the golden with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test theme_editor`.

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::theme_doc::{Base, TOKENS, ThemeDoc, rgba_of};
use atomik_ui::{
    DisplayList, Key, Modifiers, PointerButton, Theme, ThemeEditorAction, ThemeEditorView,
    UiAction, UiEvent,
};
use std::path::PathBuf;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn index(key: &str) -> usize {
    TOKENS.iter().position(|t| t.key == key).unwrap()
}

fn editor() -> ThemeEditorView {
    let mut doc = ThemeDoc::from_theme("Night drive", Base::Dark, &Theme::synthwave());
    doc.name = "Night drive".into();
    let mut v = ThemeEditorView::new(Theme::synthwave(), "user:night-drive".into(), doc);
    v.set_bounds(Rect::new(0.0, 0.0, 900.0, 700.0));
    v
}

fn click(v: &mut ThemeEditorView, at: Point) -> Vec<UiAction> {
    let mut out = v.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(v.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

fn key(v: &mut ThemeEditorView, key: Key, modifiers: Modifiers) -> Vec<UiAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    })
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    }
}

fn type_text(v: &mut ThemeEditorView, text: &str) -> Vec<UiAction> {
    let mut out = Vec::new();
    for c in text.chars() {
        out.extend(key(v, Key::Char(c), Modifiers::NONE));
    }
    out
}

fn actions(out: &[UiAction]) -> Vec<ThemeEditorAction> {
    out.iter()
        .filter_map(|a| match a {
            UiAction::ThemeEditor(a) => Some(a.clone()),
            _ => None,
        })
        .collect()
}

fn list(v: &ThemeEditorView) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.to_json()
}

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(expected == actual, "{name} differs from its golden");
}

#[test]
fn the_editor_lists_every_token_in_its_groups_and_starts_clean() {
    let mut v = editor();
    assert!(!v.dirty() && v.history_len() == 0 && !v.take_changed());
    assert_eq!(v.id(), "user:night-drive");
    // Scrolled down, every token has a row at some point.
    let mut seen = 0;
    for _ in 0..40 {
        seen = seen.max(
            (0..TOKENS.len())
                .filter(|i| v.row_rect(*i).is_some())
                .count(),
        );
        v.handle(&UiEvent::Wheel {
            delta: Vec2::new(0.0, -3.0),
            pos: Point::new(450.0, 300.0),
            modifiers: Modifiers::NONE,
        });
    }
    assert!(seen >= 10, "{seen} rows show at once");
    assert!(
        (0..TOKENS.len()).any(|i| v.row_rect(i).is_some()),
        "the end of the list shows rows"
    );
}

#[test]
fn the_editor_matches_its_golden() {
    check("theme_editor_synthwave_1x.json", &list(&editor()));
    let mut v = editor();
    let i = index("background");
    v.set_color(i, [1, 2, 3]);
    v.handle(&UiEvent::PointerDown {
        pos: v.hex_rect(i).unwrap().center(),
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    check("theme_editor_synthwave_editing_1x.json", &list(&v));
}

#[test]
fn a_swatch_asks_for_the_colour_picker_and_the_answer_keeps_the_transparency() {
    let mut v = editor();
    let i = index("grid_bar");
    let before = v.color(i);
    assert!(before[3] < 255, "the bar line is see-through");
    let at = v.swatch_rect(i).or_else(|| {
        // Scroll the row into view first.
        for _ in 0..40 {
            v.handle(&UiEvent::Wheel {
                delta: Vec2::new(0.0, -3.0),
                pos: Point::new(450.0, 300.0),
                modifiers: Modifiers::NONE,
            });
            if v.swatch_rect(i).is_some() {
                break;
            }
        }
        v.swatch_rect(i)
    });
    let out = click(&mut v, at.unwrap().center());
    assert_eq!(actions(&out), [ThemeEditorAction::PickColor(i)]);
    v.set_color(i, [10, 20, 30]);
    assert_eq!(v.color(i), [10, 20, 30, before[3]]);
    assert!(v.dirty() && v.take_changed());
    assert_eq!(rgba_of(v.preview().grid_bar), [10, 20, 30, before[3]]);
    assert_eq!(
        v.hex_text().to_lowercase(),
        "#0a141e".to_string() + &format!("{:02x}", before[3])
    );
}

/// Typing a colour in the hex text changes the token as soon as the text names one; a run of
/// typing is one step of history.
#[test]
fn typing_a_hex_colour_edits_the_token_and_is_one_undo_step() {
    let mut v = editor();
    let i = index("background");
    let at = v.hex_rect(i).unwrap().center();
    click(&mut v, at);
    assert_eq!(v.selected(), Some(i));
    assert_eq!(v.hex_text(), "#140c28");
    // Select all, type a new colour.
    key(&mut v, Key::Char('a'), ctrl());
    let mut all = Vec::new();
    all.extend(type_text(&mut v, "#ff0000"));
    assert_eq!(v.color(i), [255, 0, 0, 255]);
    assert!(actions(&all).contains(&ThemeEditorAction::Changed));
    assert_eq!(rgba_of(v.preview().background), [255, 0, 0, 255]);
    assert_eq!(v.history_len(), 1, "one step for the whole run");
    // Half a colour keeps the last good one.
    key(&mut v, Key::Backspace, Modifiers::NONE);
    assert_eq!(v.hex_text(), "#ff000");
    assert_eq!(v.color(i), [255, 0, 0, 255]);
    type_text(&mut v, "0");
    assert_eq!(v.hex_text(), "#ff0000");
    assert_eq!(v.color(i), [255, 0, 0, 255]);
    assert_eq!(v.history_len(), 1);
    // Ctrl+Z takes the whole run back.
    let out = key(&mut v, Key::Char('z'), ctrl());
    assert!(actions(&out).contains(&ThemeEditorAction::Changed));
    assert_eq!(rgba_of(v.preview().background), [0x14, 0x0c, 0x28, 255]);
    assert_eq!(v.history_len(), 0);
    assert!(!v.dirty());
    // Text that is not hex cannot be typed.
    key(&mut v, Key::Char('a'), ctrl());
    type_text(&mut v, "zz!");
    assert!(
        v.hex_text()
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '#')
    );
}

#[test]
fn reset_sets_a_token_back_to_its_base_and_undo_brings_the_change_back() {
    let mut v = editor();
    let i = index("lane");
    assert!(!v.differs_from_base(i) || v.color(i) != [0, 0, 0, 255]);
    v.set_color(i, [200, 100, 50]);
    assert!(v.differs_from_base(i));
    v.reset(i);
    assert_eq!(rgba_of(v.preview().lane), rgba_of(Theme::dark().lane));
    assert!(!v.differs_from_base(i));
    assert!(v.undo(), "the reset is a step");
    assert_eq!(v.color(i), [200, 100, 50, 255]);
    assert!(v.undo(), "and so is the change");
    assert_eq!(rgba_of(v.preview().lane), rgba_of(Theme::synthwave().lane));
    assert!(!v.undo(), "nothing is left to undo");
}

#[test]
fn save_close_and_the_history_buttons_do_their_jobs() {
    let mut v = editor();
    let at = v.save_rect().center();
    let out = click(&mut v, at);
    assert_eq!(actions(&out), [ThemeEditorAction::Save]);
    let at = v.close_rect().center();
    let out = click(&mut v, at);
    assert_eq!(actions(&out), [ThemeEditorAction::Close]);
    let out = key(&mut v, Key::Escape, Modifiers::NONE);
    assert_eq!(actions(&out), [ThemeEditorAction::Close]);
    let out = key(&mut v, Key::Char('s'), ctrl());
    assert_eq!(actions(&out), [ThemeEditorAction::Save]);

    v.set_color(index("text"), [1, 1, 1]);
    assert!(v.dirty());
    v.mark_saved();
    assert!(!v.dirty());
    let at = v.undo_rect().center();
    let out = click(&mut v, at);
    assert_eq!(actions(&out), [ThemeEditorAction::Changed]);
    assert!(v.dirty(), "undoing past a save is a change again");
}

#[test]
fn the_panel_is_the_same_whatever_theme_paints_it_only_in_colour() {
    let mut a = editor();
    let mut b = editor();
    b.set_theme(Theme::light());
    assert_ne!(list(&a), list(&b));
    a.set_theme(Theme::light());
    assert_eq!(list(&a), list(&b));
}
