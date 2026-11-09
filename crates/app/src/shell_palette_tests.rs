//! The command palette in the shell: how it opens, what it lists (the live menus and the other
//! commands of the registry), how typing, Enter, a click and Escape work, that it takes the
//! keyboard, that a command runs after it has closed and acts on the pane that had the focus,
//! the recent commands it keeps, and that a command that cannot run is still listed and runs
//! without harm. Headless (null audio backend).

use super::*;
use crate::keymap_store::KEYMAP_FILE;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

/// A temporary config folder that is removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "atomik-shell-palette-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }

    fn recent_file(&self) -> PathBuf {
        self.0.join(crate::palette::FILE)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell_in(d: &Dir) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.set_recent_path(d.0.join("recent.json"));
    let mut s = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(d.0.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(d.0.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    meta: false,
};
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: true,
    alt: false,
    meta: false,
};

fn press(s: &mut Shell<NullBackend>, key: Key, modifiers: Modifiers) {
    s.handle(&UiEvent::Key {
        key: key.clone(),
        modifiers,
        pressed: true,
    });
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: false,
    });
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            press(s, Key::Space, Modifiers::NONE);
        } else {
            press(s, Key::Char(c), Modifiers::NONE);
        }
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.into())
}

fn names(s: &Shell<NullBackend>) -> Vec<String> {
    let v = s.palette_view().expect("the palette is open");
    v.ranked()
        .iter()
        .map(|r| v.entries()[r.entry].name.clone())
        .collect()
}

// ---- opening ---------------------------------------------------------------------------------

#[test]
fn the_command_is_in_the_help_menu_with_its_key() {
    let d = Dir::new("menus");
    let s = shell_in(&d);
    let item = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Help")
        .and_then(|m| {
            m.items()
                .find(|i| i.command == AppCommand::CommandPalette)
                .cloned()
        })
        .expect("Command Palette in the Help menu");
    assert_eq!(item.label, "Command Palette\u{2026}");
    assert_eq!(item.shortcut.as_deref(), Some("Ctrl+Shift+K"));
    // One menu holds it (a test keeps each command in one menu).
    let count = s
        .menu_model()
        .iter()
        .flat_map(|m| m.items())
        .filter(|i| i.command == AppCommand::CommandPalette)
        .count();
    assert_eq!(count, 1);
    assert_eq!(
        AppCommand::CommandPalette.command_id(),
        Some(("view.command_palette", None))
    );
}

#[test]
fn the_key_opens_the_palette_and_closes_it_again() {
    let d = Dir::new("key");
    let mut s = shell_in(&d);
    assert!(!s.palette_open());
    press(&mut s, Key::Char('K'), CTRL_SHIFT);
    assert!(s.palette_open());
    assert!(
        s.modal_open(),
        "the other windows give way while it is open"
    );
    assert!(s.wants_text_input());
    press(&mut s, Key::Char('K'), CTRL_SHIFT);
    assert!(!s.palette_open());
    s.execute(AppCommand::CommandPalette);
    assert!(s.palette_open());
    press(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.palette_open());
}

#[test]
fn it_lists_the_menu_items_and_the_other_commands_but_not_the_ones_that_need_a_pick() {
    let d = Dir::new("list");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    let v = s.palette_view().unwrap();
    assert!(v.entries().len() > 400, "{} entries", v.entries().len());
    let named = |name: &str| v.entries().iter().find(|e| e.name == name);
    // A menu item has its menu path; a command only the keys reach has its group.
    let guided = named("Guided Tour").expect("a Help menu item");
    assert_eq!(guided.path, "Help");
    assert_eq!(guided.id, "help.guided_tour");
    let pane = v
        .entries()
        .iter()
        .find(|e| e.id == "rack.mod_matrix" || e.id == "rack.reset_device")
        .expect("a rack command");
    assert!(!pane.path.contains("Help"));
    // The menus' own submenus show in the path.
    assert!(
        v.entries().iter().any(|e| e.path.contains(" > ")),
        "a submenu path"
    );
    for e in v.entries() {
        assert!(!e.id.ends_with("_pick"), "{}", e.id);
        assert!(!e.id.starts_with("keys."), "{}", e.id);
        assert!(!e.name.ends_with('\u{2026}'), "{}", e.name);
    }
    // Each command is listed once.
    let mut keys: Vec<&str> = v.entries().iter().map(|e| e.key.as_str()).collect();
    keys.sort_unstable();
    let all = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), all);
}

#[test]
fn the_real_commands_rank_quantize_first_for_quan() {
    let d = Dir::new("quan");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "quan");
    let list = names(&s);
    assert_eq!(list[0], "Quantize", "{list:?}");
    assert!(list.iter().any(|n| n == "Quantize Options"));
    assert_eq!(s.palette_view().unwrap().query(), "quan");
}

// ---- keys ------------------------------------------------------------------------------------

#[test]
fn typing_guided_and_enter_runs_the_command_and_closes_the_palette() {
    let d = Dir::new("guided");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "guided");
    assert_eq!(names(&s)[0], "Guided Tour");
    assert!(!s.guide_open());
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.palette_open());
    assert!(s.guide_open(), "the guided tour started");
}

#[test]
fn space_is_typed_and_does_not_start_playback() {
    let d = Dir::new("space");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "a b");
    assert_eq!(s.palette_view().unwrap().query(), "a b");
    press(&mut s, Key::Space, Modifiers::NONE);
    assert!(s.palette_open());
    assert!(!s.core().is_playing(), "Space is text here, not Play");
    // Letters that are shortcuts elsewhere did not run them either.
    assert!(!s.core().is_recording());
}

#[test]
fn control_chords_edit_the_field() {
    let d = Dir::new("chords");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "export");
    press(&mut s, Key::Char('a'), CTRL);
    press(&mut s, Key::Char('x'), Modifiers::NONE);
    // Select all, then a letter replaces the whole text.
    assert_eq!(s.palette_view().unwrap().query(), "x");
    press(&mut s, Key::Backspace, Modifiers::NONE);
    assert_eq!(s.palette_view().unwrap().query(), "");
    assert!(s.palette_open());
    // The shortcut of Select All did not select anything in the song.
    assert!(s.core().view().selection().is_empty());
}

#[test]
fn the_arrows_and_pages_move_the_highlight_and_enter_runs_that_row() {
    let d = Dir::new("move");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    assert_eq!(s.palette_view().unwrap().selected(), Some(0));
    press(&mut s, Key::Down, Modifiers::NONE);
    press(&mut s, Key::Down, Modifiers::NONE);
    assert_eq!(s.palette_view().unwrap().selected(), Some(2));
    press(&mut s, Key::Up, Modifiers::NONE);
    assert_eq!(s.palette_view().unwrap().selected(), Some(1));
    press(&mut s, Key::PageDown, Modifiers::NONE);
    let paged = s.palette_view().unwrap().selected().unwrap();
    assert!(paged >= 10, "a page is a screenful of rows: {paged}");
    press(&mut s, Key::PageUp, Modifiers::NONE);
    assert_eq!(s.palette_view().unwrap().selected(), Some(1));
    // A new query starts on its best match again.
    type_text(&mut s, "guided");
    assert_eq!(s.palette_view().unwrap().selected(), Some(0));
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(s.guide_open());
}

// ---- the pointer -----------------------------------------------------------------------------

fn click(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn a_click_on_a_row_runs_it_and_a_click_outside_closes_the_palette() {
    let d = Dir::new("click");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "guided");
    let at = s.palette_view().unwrap().row_center(0).unwrap();
    click(&mut s, at);
    assert!(!s.palette_open());
    assert!(s.guide_open());
    s.execute(AppCommand::CommandPalette);
    click(&mut s, Point::new(2.0, 400.0));
    assert!(!s.palette_open(), "a click outside gives way");
    assert!(s.guide_open());
}

// ---- running ---------------------------------------------------------------------------------

#[test]
fn a_command_acts_on_the_pane_that_had_the_focus() {
    let d = Dir::new("focus");
    let mut s = shell_in(&d);
    for k in ["a", "b"] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            })
            .unwrap();
    }
    s.set_focus(Pane::Rack);
    assert!(s.core().view().selection().len() < 2);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "select all");
    assert_eq!(names(&s)[0], "Select All");
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.palette_open());
    assert_eq!(s.focus(), Pane::Rack, "the focus is where it was");
    assert!(
        s.core().view().selection().len() >= 2,
        "Select All reached the rack"
    );
}

#[test]
fn a_command_that_cannot_run_is_listed_dimmed_and_runs_without_harm() {
    let d = Dir::new("disabled");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "undo");
    let v = s.palette_view().unwrap();
    let undo = v.entry_at(0).expect("Undo is listed");
    assert_eq!(undo.name, "Undo");
    assert!(!undo.enabled, "nothing to undo in a new song");
    // The row is painted with the muted ink.
    let mut dim = atomik_ui::DisplayList::new();
    v.paint(&mut dim);
    let mut lit_entries = v.entries().to_vec();
    for e in &mut lit_entries {
        e.enabled = true;
    }
    let mut lit = CommandPaletteView::new(atomik_ui::Theme::dark(), lit_entries, Vec::new());
    lit.set_bounds(Rect::new(0.0, 0.0, 1200.0, 800.0));
    lit.set_query("undo");
    let mut bright = atomik_ui::DisplayList::new();
    lit.paint(&mut bright);
    assert_ne!(dim.to_json(), bright.to_json());
    // Enter runs it all the same; the palette closes and the song is untouched.
    assert!(!s.core().is_dirty());
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.palette_open());
    assert!(!s.core().is_dirty(), "the song is untouched");
    assert!(!s.core().is_playing());
}

#[test]
fn a_command_of_another_view_says_it_is_not_available_here() {
    let d = Dir::new("elsewhere");
    let mut s = shell_in(&d);
    s.set_focus(Pane::Transport);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "join notes");
    let first = s.palette_view().unwrap().entry_at(0).unwrap().clone();
    assert_eq!(first.id, "editor.join_notes");
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.palette_open());
    let said = s.core().last_rejection().unwrap_or_default().to_owned();
    assert!(said.contains("not available"), "{said}");
}

// ---- recent commands ---------------------------------------------------------------------------

#[test]
fn the_last_commands_come_first_with_nothing_typed_and_are_kept_in_a_file() {
    let d = Dir::new("recent");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "guided");
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert_eq!(crate::palette::load(&d.recent_file()), ["help.guided_tour"]);
    // Another command, then the palette again: both lead the empty list, newest first.
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "sequencer only");
    press(&mut s, Key::Enter, Modifiers::NONE);
    s.execute(AppCommand::CommandPalette);
    let list = names(&s);
    assert_eq!(
        &list[..2],
        ["Sequencer Only", "Guided Tour"],
        "{:?}",
        &list[..4]
    );
    press(&mut s, Key::Escape, Modifiers::NONE);
    // A second window reads the same file.
    let mut other = shell_in(&d);
    other.execute(AppCommand::CommandPalette);
    assert_eq!(names(&other)[0], "Sequencer Only");
    // Closing without running remembers nothing new.
    press(&mut other, Key::Escape, Modifiers::NONE);
    assert_eq!(
        crate::palette::load(&d.recent_file()),
        ["view.sequencer_only", "help.guided_tour"]
    );
}

#[test]
fn a_damaged_recent_file_is_an_empty_list() {
    let d = Dir::new("damaged");
    std::fs::write(d.recent_file(), "{ not json").unwrap();
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    let v = s.palette_view().unwrap();
    let first = v.entry_at(0).unwrap().name.to_lowercase();
    // Without recents the list is by name.
    let second = v.entry_at(1).unwrap().name.to_lowercase();
    assert!(first <= second);
}

// ---- keys in the rows --------------------------------------------------------------------------

#[test]
fn a_remapped_key_shows_in_the_row() {
    let d = Dir::new("remap");
    // Sequencer Only is F7 by default; the user moves it.
    std::fs::write(
        d.0.join(KEYMAP_FILE),
        r#"{"bindings":[{"command":"view.sequencer_only","context":"global","chords":["Cmd+Alt+J"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "sequencer only");
    let v = s.palette_view().unwrap();
    let row = v.entry_at(0).unwrap();
    assert_eq!(row.name, "Sequencer Only");
    assert_eq!(row.shortcut.as_deref(), Some("Ctrl+Alt+J"));
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().contains(&"Ctrl+Alt+J"));
    assert!(!dl.texts().contains(&"F7"));
}

#[test]
fn a_command_only_the_keys_reach_shows_its_key_too() {
    let d = Dir::new("keyonly");
    let mut s = shell_in(&d);
    s.execute(AppCommand::CommandPalette);
    let v = s.palette_view().unwrap();
    let cycle = v
        .entries()
        .iter()
        .find(|e| e.id == "view.cables_cycle")
        .expect("Cycle Cable Display");
    assert_eq!(cycle.shortcut.as_deref(), Some("K"));
    assert_eq!(cycle.path, "View");
}

// ---- per window ----------------------------------------------------------------------------------

#[test]
fn the_palette_belongs_to_its_own_window() {
    let d = Dir::new("windows");
    let mut a = shell_in(&d);
    let b = shell_in(&d);
    a.execute(AppCommand::CommandPalette);
    assert!(a.palette_open());
    assert!(!b.palette_open());
}
