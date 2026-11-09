//! The command palette view: the field, the ranked list, the keys and the pointer, the rows it
//! paints (name, path, shortcut, the matched letters, a dimmed row for a command that cannot run)
//! and a golden of the palette with a query.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test command_palette`.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    CommandPaletteView, DisplayList, Key, Modifiers, PaletteAction, PaletteEntry, Theme, UiAction,
    UiEvent,
};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);
const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    meta: false,
};

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
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn entry(id: &str, name: &str, path: &str, shortcut: Option<&str>, enabled: bool) -> PaletteEntry {
    PaletteEntry {
        key: id.to_owned(),
        id: id.to_owned(),
        name: name.to_owned(),
        path: path.to_owned(),
        shortcut: shortcut.map(str::to_owned),
        enabled,
    }
}

fn entries() -> Vec<PaletteEntry> {
    let mut list = vec![
        entry("editor.quantize", "Quantize", "Edit", Some("Ctrl+K"), true),
        entry(
            "edit.quantize_options",
            "Quantize Options",
            "Edit",
            None,
            true,
        ),
        entry(
            "sequencer.audio_quantize",
            "Quantize Audio",
            "Edit > Audio",
            None,
            false,
        ),
        entry(
            "transport.input_quantize",
            "Input Quantize",
            "Transport",
            None,
            true,
        ),
        entry("edit.undo", "Undo", "Edit", Some("Ctrl+Z"), false),
        entry("edit.redo", "Redo", "Edit", Some("Ctrl+Y"), false),
        entry("file.save", "Save", "File", Some("Ctrl+S"), true),
        entry("help.guided_tour", "Guided Tour", "Help", None, true),
    ];
    for n in 0..30 {
        list.push(entry(
            &format!("rack.extra_{n:02}"),
            &format!("Extra Command {n:02}"),
            "Rack",
            None,
            true,
        ));
    }
    list
}

fn view() -> CommandPaletteView {
    let mut v = CommandPaletteView::new(Theme::dark(), entries(), vec!["file.save".into()]);
    v.set_bounds(BOUNDS);
    v
}

fn key(v: &mut CommandPaletteView, key: Key, modifiers: Modifiers) -> Vec<PaletteAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::CommandPalette(a) => Some(a),
        _ => None,
    })
    .collect()
}

fn type_text(v: &mut CommandPaletteView, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            key(v, Key::Space, Modifiers::NONE);
        } else {
            key(v, Key::Char(c), Modifiers::NONE);
        }
    }
}

fn click(v: &mut CommandPaletteView, p: Point) -> Vec<PaletteAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::CommandPalette(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &CommandPaletteView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn name_at(v: &CommandPaletteView, row: usize) -> String {
    v.entry_at(row).expect("a row").name.clone()
}

#[test]
fn with_nothing_typed_the_recent_command_is_first_and_a_hint_shows() {
    let v = view();
    assert_eq!(name_at(&v, 0), "Save");
    assert_eq!(v.query(), "");
    let t = texts(&v);
    assert!(t.iter().any(|s| s.starts_with("Type a command")), "{t:?}");
    assert!(t.contains(&"Save".to_owned()));
    assert!(t.contains(&"Ctrl+S".to_owned()), "the shortcut shows");
    assert!(t.contains(&"File".to_owned()), "the menu path shows");
    assert!(t.contains(&"38".to_owned()), "the number of commands");
    assert!(v.is_text_owner());
}

#[test]
fn typing_ranks_the_commands_and_shows_the_count() {
    let mut v = view();
    type_text(&mut v, "quan");
    assert_eq!(v.query(), "quan");
    assert_eq!(name_at(&v, 0), "Quantize");
    assert_eq!(v.ranked().len(), 4);
    assert_eq!(v.ranked()[0].name_hits, [0, 1, 2, 3]);
    let t = texts(&v);
    assert!(t.contains(&"4 of 38".to_owned()), "{t:?}");
    // The matched letters are drawn on their own, bold: the name is cut into runs.
    assert!(t.contains(&"Quan".to_owned()), "{t:?}");
    assert!(t.contains(&"tize".to_owned()), "{t:?}");
    type_text(&mut v, "zzz");
    assert!(v.ranked().is_empty());
    assert!(texts(&v).contains(&"No command matches.".to_owned()));
    assert!(key(&mut v, Key::Enter, Modifiers::NONE).is_empty());
}

#[test]
fn the_field_is_a_text_field_with_select_all_and_backspace() {
    let mut v = view();
    type_text(&mut v, "undo");
    assert_eq!(name_at(&v, 0), "Undo");
    key(&mut v, Key::Char('a'), CTRL);
    type_text(&mut v, "red");
    assert_eq!(v.query(), "red");
    assert_eq!(name_at(&v, 0), "Redo");
    key(&mut v, Key::Backspace, Modifiers::NONE);
    assert_eq!(v.query(), "re");
    key(&mut v, Key::Left, Modifiers::NONE);
    type_text(&mut v, "x");
    assert_eq!(v.query(), "rxe");
    // The query stops growing at its limit.
    key(&mut v, Key::Char('a'), CTRL);
    type_text(&mut v, &"a".repeat(200));
    assert_eq!(v.query().chars().count(), 80);
}

#[test]
fn space_is_text_and_does_not_leave_the_palette() {
    let mut v = view();
    type_text(&mut v, "guided tour");
    assert_eq!(v.query(), "guided tour");
    assert_eq!(name_at(&v, 0), "Guided Tour");
}

#[test]
fn enter_runs_the_highlighted_entry_and_escape_closes() {
    let mut v = view();
    type_text(&mut v, "undo");
    let at = v.ranked()[0].entry;
    assert_eq!(
        key(&mut v, Key::Enter, Modifiers::NONE),
        [PaletteAction::Run(at)]
    );
    assert_eq!(
        key(&mut v, Key::Escape, Modifiers::NONE),
        [PaletteAction::Close]
    );
}

#[test]
fn up_down_and_the_pages_move_the_highlight_and_keep_it_in_view() {
    let mut v = view();
    assert_eq!(v.selected(), Some(0));
    key(&mut v, Key::Up, Modifiers::NONE);
    assert_eq!(v.selected(), Some(0), "no wrapping above the first");
    for _ in 0..3 {
        key(&mut v, Key::Down, Modifiers::NONE);
    }
    assert_eq!(v.selected(), Some(3));
    key(&mut v, Key::PageDown, Modifiers::NONE);
    assert_eq!(v.selected(), Some(13), "a page is the ten rows in view");
    assert!(v.row_rect(13).is_some(), "the row is scrolled into view");
    assert!(v.row_rect(0).is_none());
    for _ in 0..40 {
        key(&mut v, Key::PageDown, Modifiers::NONE);
    }
    assert_eq!(v.selected(), Some(37));
    assert!(v.row_rect(37).is_some());
    key(&mut v, Key::PageUp, Modifiers::NONE);
    assert_eq!(v.selected(), Some(27));
    let want = v.ranked()[27].entry;
    assert_eq!(
        key(&mut v, Key::Enter, Modifiers::NONE),
        [PaletteAction::Run(want)]
    );
    // The highlight starts again at the best match when the query changes.
    type_text(&mut v, "q");
    assert_eq!(v.selected(), Some(0));
}

#[test]
fn a_click_on_a_row_runs_it_and_one_outside_the_panel_closes() {
    let mut v = view();
    type_text(&mut v, "quan");
    let at = v.row_center(1).unwrap();
    let want = v.ranked()[1].entry;
    assert_eq!(click(&mut v, at), [PaletteAction::Run(want)]);
    let outside = Point::new(3.0, 3.0);
    assert!(!v.panel().contains(outside));
    assert_eq!(click(&mut v, outside), [PaletteAction::Close]);
    // A click in the field places the caret and runs nothing.
    let field = v.field_rect().center();
    assert!(click(&mut v, field).is_empty());
}

#[test]
fn the_wheel_scrolls_the_list() {
    let mut v = view();
    assert!(v.row_rect(0).is_some());
    let over = v.row_center(2).unwrap();
    v.handle(&UiEvent::Wheel {
        delta: Point::new(0.0, -3.0).to_vec2(),
        pos: over,
        modifiers: Modifiers::NONE,
    });
    assert!(v.row_rect(0).is_none());
    assert!(v.row_rect(5).is_some());
}

#[test]
fn the_panel_is_centred_and_inside_the_window() {
    let v = view();
    let p = v.panel();
    assert!(BOUNDS.contains_rect(p));
    assert!((p.center().x - BOUNDS.center().x).abs() < 1.0);
    // A narrow window still holds it.
    let mut narrow = view();
    narrow.set_bounds(Rect::new(0.0, 0.0, 300.0, 400.0));
    assert!(Rect::new(0.0, 0.0, 300.0, 400.0).contains_rect(narrow.panel()));
}

#[test]
fn a_command_that_cannot_run_is_listed_and_painted_apart() {
    let mut v = view();
    type_text(&mut v, "undo");
    assert!(!v.entry_at(0).unwrap().enabled);
    let mut dim = DisplayList::new();
    v.paint(&mut dim);
    let mut lit_entries = entries();
    for e in &mut lit_entries {
        e.enabled = true;
    }
    let mut lit = CommandPaletteView::new(Theme::dark(), lit_entries, vec!["file.save".into()]);
    lit.set_bounds(BOUNDS);
    type_text(&mut lit, "undo");
    let mut bright = DisplayList::new();
    lit.paint(&mut bright);
    assert_eq!(dim.texts(), bright.texts(), "the same words");
    assert_ne!(dim.to_json(), bright.to_json(), "in another ink");
    // The tooltip says why the row is apart.
    let help = v.help_at(v.row_center(0).unwrap()).expect("a hint");
    assert!(help.text.len() <= 90);
}

#[test]
fn the_palette_with_a_query_matches_its_golden() {
    let mut v = view();
    type_text(&mut v, "quan");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("command_palette_query_1x.json", &dl.to_json());
}

#[test]
fn the_theme_and_scale_follow() {
    let mut v = view();
    let mut before = DisplayList::new();
    v.paint(&mut before);
    v.set_theme(Theme::light());
    v.set_scale(2.0);
    let mut after = DisplayList::new();
    v.paint(&mut after);
    assert_ne!(before.to_json(), after.to_json());
    assert!(v.needs_repaint());
}
