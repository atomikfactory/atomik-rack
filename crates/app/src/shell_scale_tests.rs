//! Scales, fold and chords in the shell: the piano roll's context menus with their submenus (the
//! check marks, choosing an item runs its command), the scale bar menus, the keys, the saved
//! window state, and one undo step per edit.

use super::*;
use crate::session::NullBackend;
use atomik_model::ClipId;
use atomik_model::theory::{ChordKind, Scale, ScaleKind};
use atomik_ui::PointerButton;
use atomik_ui::chord_edit::Stamp;
use atomik_ui::key_rows::Fold;
use atomik_ui::keymap::script::event_for;
use atomik_ui::piano_roll::ScaleBarField;

fn make(path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: path,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s
}

/// A shell with the demo project and its first note clip open in the piano roll.
fn roll_shell(path: Option<PathBuf>) -> (Shell<NullBackend>, ClipId) {
    let mut s = make(path);
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    s.execute(AppCommand::TogglePianoRoll);
    let id = s
        .core()
        .piano_roll_clip()
        .expect("the piano roll opened on a note clip");
    s.set_focus(Pane::PianoRoll);
    (s, id)
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    for pressed in [true, false] {
        let (key, modifiers) = event_for(Os::Windows, chord).unwrap();
        s.handle(&UiEvent::Key {
            key,
            modifiers,
            pressed,
        });
    }
}

fn mouse(s: &mut Shell<NullBackend>, at: Point, button: PointerButton) {
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: at,
        button,
        modifiers: Modifiers::NONE,
    });
}

fn hover(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: at,
        modifiers: Modifiers::NONE,
    });
}

/// An empty spot of the grid, far to the right of every note.
fn empty_spot(s: &Shell<NullBackend>) -> Point {
    let roll = s.core().piano_roll();
    let b = roll.bounds();
    Point::new(b.x1 - 40.0, roll.ruler_y() + 60.0)
}

fn scale_of(s: &Shell<NullBackend>, id: ClipId) -> Option<Scale> {
    s.core().doc().song.clip(id).unwrap().2.scale
}

fn labels(s: &Shell<NullBackend>) -> Vec<String> {
    s.context_menu()
        .unwrap()
        .entries()
        .iter()
        .filter_map(|e| match e {
            atomik_ui::context_menu::Entry::Item(i) => Some(i.label.clone()),
            atomik_ui::context_menu::Entry::Submenu(m) => Some(format!("{}>", m.label)),
            atomik_ui::context_menu::Entry::Separator => None,
        })
        .collect()
}

/// Opens the notes menu and hovers the submenu path, then clicks the item.
fn choose(s: &mut Shell<NullBackend>, path: &[&str], command: &str, arg: &str) {
    pick(s, path, command, (!arg.is_empty()).then_some(arg));
}

fn pick(s: &mut Shell<NullBackend>, path: &[&str], command: &str, arg: Option<&str>) {
    let spot = empty_spot(s);
    mouse(s, spot, PointerButton::Secondary);
    for label in path {
        let at = s.context_menu().unwrap().submenu_center(label).unwrap();
        hover(s, at);
    }
    let m = s.context_menu().unwrap();
    let at = match arg {
        Some(a) => m.item_center_with(command, a),
        None => m.item_center(command),
    }
    .unwrap_or_else(|| panic!("{command} {arg:?}"));
    mouse(s, at, PointerButton::Primary);
    assert!(s.context_menu().is_none(), "choosing closes the menu");
}

#[test]
fn the_notes_menu_has_the_scale_chord_stamp_and_chords_submenus() {
    let (mut s, _) = roll_shell(None);
    let spot = empty_spot(&s);
    mouse(&mut s, spot, PointerButton::Secondary);
    let l = labels(&s);
    for want in ["Scale>", "Chord Stamp>", "Chords>", "Quantize"] {
        assert!(l.iter().any(|x| x == want), "{want} in {l:?}");
    }
}

#[test]
fn choosing_a_scale_from_the_submenus_sets_it_on_the_clip_and_the_checks_follow() {
    let (mut s, id) = roll_shell(None);
    choose(
        &mut s,
        &["Scale", "Scale Type"],
        "editor.set_scale",
        "dorian",
    );
    assert_eq!(scale_of(&s, id), Some(Scale::new(0, ScaleKind::Dorian)));
    // The next menu checks Dorian and the root C.
    let spot = empty_spot(&s);
    mouse(&mut s, spot, PointerButton::Secondary);
    let at = s.context_menu().unwrap().submenu_center("Scale").unwrap();
    hover(&mut s, at);
    let at = s
        .context_menu()
        .unwrap()
        .submenu_center("Scale Type")
        .unwrap();
    hover(&mut s, at);
    let checked: Vec<String> = s
        .context_menu()
        .unwrap()
        .all_items()
        .iter()
        .filter(|i| i.checked == Some(true) && i.command == "editor.set_scale")
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(checked, ["Dorian"]);
    s.close_context_menu();
    // The root from its submenu.
    choose(&mut s, &["Scale", "Root"], "editor.set_scale_root", "d");
    assert_eq!(scale_of(&s, id), Some(Scale::new(2, ScaleKind::Dorian)));
    // Chromatic clears it.
    choose(
        &mut s,
        &["Scale", "Scale Type"],
        "editor.set_scale",
        "chromatic",
    );
    assert_eq!(scale_of(&s, id), None);
}

#[test]
fn a_scale_change_is_one_undo_step() {
    let (mut s, id) = roll_shell(None);
    choose(
        &mut s,
        &["Scale", "Scale Type"],
        "editor.set_scale",
        "blues",
    );
    assert!(scale_of(&s, id).is_some());
    tap(&mut s, "Cmd+Z");
    assert_eq!(scale_of(&s, id), None);
}

#[test]
fn the_scale_bar_fields_open_their_menus_and_choosing_runs_the_command() {
    let (mut s, id) = roll_shell(None);
    // The scale list.
    let at = s.core().piano_roll().scale_bar_center(ScaleBarField::Kind);
    mouse(&mut s, at, PointerButton::Primary);
    let m = s.context_menu().expect("the scale menu");
    assert_eq!(m.items().count(), ScaleKind::ALL.len());
    let pick = m
        .item_center_with("editor.set_scale", "natural_minor")
        .unwrap();
    mouse(&mut s, pick, PointerButton::Primary);
    assert_eq!(
        scale_of(&s, id),
        Some(Scale::new(0, ScaleKind::NaturalMinor))
    );
    // The root list: twelve notes, then A.
    let at = s.core().piano_roll().scale_bar_center(ScaleBarField::Root);
    mouse(&mut s, at, PointerButton::Primary);
    let m = s.context_menu().expect("the root menu");
    assert_eq!(m.items().count(), 12);
    let pick = m.item_center_with("editor.set_scale_root", "a").unwrap();
    mouse(&mut s, pick, PointerButton::Primary);
    assert_eq!(
        scale_of(&s, id),
        Some(Scale::new(9, ScaleKind::NaturalMinor))
    );
    // The chord list.
    let at = s.core().piano_roll().scale_bar_center(ScaleBarField::Chord);
    mouse(&mut s, at, PointerButton::Primary);
    let m = s.context_menu().expect("the chord menu");
    assert!(m.items().count() >= 22, "none and every chord");
    let pick = m.item_center_with("editor.set_chord", "sus4").unwrap();
    mouse(&mut s, pick, PointerButton::Primary);
    assert_eq!(s.core().piano_roll().stamp(), Stamp::Chord(ChordKind::Sus4));
    // Detect from the bar.
    let at = s
        .core()
        .piano_roll()
        .scale_bar_center(ScaleBarField::Detect);
    mouse(&mut s, at, PointerButton::Primary);
    assert!(scale_of(&s, id).is_some());
}

#[test]
fn the_chord_stamp_chords_and_options_submenus_set_their_state() {
    let (mut s, _) = roll_shell(None);
    choose(&mut s, &["Chord Stamp"], "editor.set_chord", "maj7");
    assert_eq!(
        s.core().piano_roll().stamp(),
        Stamp::Chord(ChordKind::MajorSeventh)
    );
    choose(
        &mut s,
        &["Chord Stamp", "Inversion"],
        "editor.set_chord_inversion",
        "2",
    );
    assert_eq!(s.core().piano_roll().inversion(), 2);
    choose(
        &mut s,
        &["Chords", "Arpeggio Pattern"],
        "editor.set_arp_pattern",
        "up_down",
    );
    choose(
        &mut s,
        &["Chords", "Arpeggio Gate"],
        "editor.set_arp_gate",
        "75",
    );
    choose(
        &mut s,
        &["Chords", "Arpeggio Octaves"],
        "editor.set_arp_octaves",
        "3",
    );
    choose(
        &mut s,
        &["Chords", "Strum Amount"],
        "editor.set_strum_amount",
        "16",
    );
    choose(
        &mut s,
        &["Chords", "Strum Direction"],
        "editor.set_strum_direction",
        "down",
    );
    let t = s.core().piano_roll().tools();
    assert_eq!(t.arp.gate_percent, 75);
    assert_eq!(t.arp.octaves, 3);
    assert_eq!(t.strum.ticks, 3840);
    assert_eq!(
        t.strum.direction,
        atomik_ui::chord_edit::StrumDirection::Down
    );
}

#[test]
fn chordify_from_the_menu_is_one_undo_step() {
    let (mut s, id) = roll_shell(None);
    choose(&mut s, &["Chord Stamp"], "editor.set_chord", "major");
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    let count = |s: &Shell<NullBackend>| match &s.core().doc().song.clip(id).unwrap().2.content {
        atomik_model::ClipContent::Notes(n) => n.len(),
        _ => 0,
    };
    let before = count(&s);
    choose(&mut s, &["Chords"], "editor.chordify", "");
    assert_eq!(count(&s), before + 2, "a triad out of one note");
    tap(&mut s, "Cmd+Z");
    assert_eq!(count(&s), before, "one undo step");
}

#[test]
fn the_keys_toggle_snap_to_scale_and_fold_and_the_window_state_is_saved() {
    let dir = std::env::temp_dir().join(format!("atomik-scale-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-state.json");
    let _ = std::fs::remove_file(&path);
    let (mut s, id) = roll_shell(Some(path.clone()));
    assert!(!s.core().piano_roll().snap_to_scale());
    tap(&mut s, "Shift+S");
    assert!(s.core().piano_roll().snap_to_scale());
    assert!(s.ui_state().piano_roll_snap_scale);
    // No scale yet: the fold key folds to the used notes.
    tap(&mut s, "Shift+F");
    assert_eq!(s.core().piano_roll().fold(), Fold::Used);
    tap(&mut s, "Shift+F");
    assert_eq!(s.core().piano_roll().fold(), Fold::Off);
    choose(
        &mut s,
        &["Scale", "Scale Type"],
        "editor.set_scale",
        "major",
    );
    tap(&mut s, "Shift+F");
    assert_eq!(s.core().piano_roll().fold(), Fold::Scale);
    assert_eq!(s.ui_state().piano_roll_fold, Fold::Scale);
    let saved = crate::ui_state::UiState::load(&path);
    assert!(saved.piano_roll_snap_scale && saved.piano_roll_fold == Fold::Scale);
    // A new shell reads them back.
    let (s2, _) = roll_shell(Some(path.clone()));
    assert!(s2.core().piano_roll().snap_to_scale());
    assert_eq!(s2.core().piano_roll().fold(), Fold::Scale);
    let _ = id;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_chordify_key_builds_the_stamp_chord_on_the_selection() {
    let (mut s, id) = roll_shell(None);
    let count = |s: &Shell<NullBackend>| match &s.core().doc().song.clip(id).unwrap().2.content {
        atomik_model::ClipContent::Notes(n) => n.len(),
        _ => 0,
    };
    let before = count(&s);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    // No stamp: nothing happens.
    tap(&mut s, "Shift+C");
    assert_eq!(count(&s), before);
    choose(&mut s, &["Chord Stamp"], "editor.set_chord", "major");
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    tap(&mut s, "Shift+C");
    assert_eq!(count(&s), before + 2, "a triad out of one note");
    tap(&mut s, "Cmd+Z");
    assert_eq!(count(&s), before, "one undo step");
}

fn golden(name: &str, menu: &atomik_ui::context_menu::ContextMenu) {
    let mut list = atomik_ui::DisplayList::new();
    menu.paint(&mut list, &atomik_ui::Theme::dark());
    let actual = list.to_json();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
        path.display()
    );
}

#[test]
fn the_notes_menu_with_the_scale_submenus_open_matches_golden() {
    let (mut s, _) = roll_shell(None);
    let spot = empty_spot(&s);
    mouse(&mut s, spot, PointerButton::Secondary);
    for label in ["Scale", "Scale Type"] {
        let at = s.context_menu().unwrap().submenu_center(label).unwrap();
        hover(&mut s, at);
    }
    assert_eq!(
        s.context_menu().unwrap().open_path(),
        ["Scale", "Scale Type"]
    );
    golden("piano_roll_context_menu_scale", s.context_menu().unwrap());
}

#[test]
fn the_cursor_follows_the_piano_roll_and_goes_back_to_the_arrow_elsewhere() {
    let (mut s, _) = roll_shell(None);
    assert_eq!(s.cursor(), CursorHint::Default);
    let r = s.core().piano_roll().note_rect(0).expect("a note");
    let edge = atomik_ui::kurbo::Point::new(r.x1 - 2.0, r.center().y);
    s.handle(&UiEvent::PointerMove {
        pos: edge,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.cursor(), CursorHint::ResizeHorizontal);
    let body = r.center();
    s.handle(&UiEvent::PointerMove {
        pos: body,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.cursor(), CursorHint::Move);
    // Over another pane the roll gets a PointerLeave and the arrow comes back.
    s.handle(&UiEvent::PointerMove {
        pos: atomik_ui::kurbo::Point::new(5.0, 5.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.cursor(), CursorHint::Default);
}
