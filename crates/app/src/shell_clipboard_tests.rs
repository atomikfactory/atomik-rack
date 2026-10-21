//! Rack multi-selection, the clipboard keys and the context menus end to end: key scripts through the router and a headless `Shell`, the OS a
//! parameter. Cut, copy, paste, duplicate, select all and delete work in the rack, the
//! arrangement and the piano roll; a right click opens a menu built from the registry with the
//! derived shortcut text; every operation is one undo step.

use super::*;
use crate::clipboard::ClipboardKind;
use crate::session::NullBackend;
use atomik_model::{Cable, ClipId, Command, DeviceKey, SocketRef, SongCommand};
use atomik_ui::PointerButton;
use atomik_ui::keymap::script::event_for;

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const BAR: i64 = 4 * 15_360;

fn shell_for(os: Os) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 2400.0)));
    // These tests are about the instruments and effects: the rack as it looked before
    // With the console devices hidden.
    s.core_mut().view_mut().set_show_console_devices(false);
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(s: &mut Shell<NullBackend>, type_id: &str, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.to_string(),
            key: Some(key(k)),
        })
        .unwrap();
}

fn cable(s: &mut Shell<NullBackend>, from: (&str, &str), to: (&str, &str)) {
    let c = Cable {
        from: SocketRef {
            device: key(from.0),
            socket: from.1.into(),
        },
        to: SocketRef {
            device: key(to.0),
            socket: to.1.into(),
        },
    };
    s.core_mut().apply_command(Command::Connect(c)).unwrap();
}

/// Three instruments `a`, `b`, `c` (a feeds nothing; b and c are effects) in the rack.
fn rack3() -> Shell<NullBackend> {
    let mut s = shell_for(Os::Windows);
    add(&mut s, SYNTH, "a");
    add(&mut s, DELAY, "b");
    add(&mut s, DELAY, "c");
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    s
}

fn devices(s: &Shell<NullBackend>) -> Vec<String> {
    s.core()
        .doc()
        .devices
        .iter()
        .filter(|d| !atomik_model::console::is_console_type(&d.type_id))
        .map(|d| d.key.0.clone())
        .collect()
}

fn plate(s: &Shell<NullBackend>, k: &str) -> Point {
    let p = s.core().view().plate_center(&key(k)).unwrap();
    Point::new(p.x, p.y + s.layout().rack.y0)
}

fn press(
    s: &mut Shell<NullBackend>,
    p: Point,
    button: PointerButton,
    modifiers: Modifiers,
) -> Vec<ShellRequest> {
    let mut r = s.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers,
        clicks: 1,
    });
    r.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button,
        modifiers,
    }));
    r
}

fn click(s: &mut Shell<NullBackend>, p: Point, modifiers: Modifiers) {
    press(s, p, PointerButton::Primary, modifiers);
}

fn ctrl() -> Modifiers {
    Modifiers::CTRL
}

fn tap(s: &mut Shell<NullBackend>, os: Os, chord: &str) {
    let (k, m) = event_for(os, chord).unwrap();
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: k.clone(),
            modifiers: m,
            pressed,
        });
    }
}

fn selection(s: &Shell<NullBackend>) -> Vec<String> {
    s.core()
        .view()
        .selection()
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn saved(s: &Shell<NullBackend>) -> String {
    atomik_model::save_project(s.core().doc())
}

// ------------------------------------------------------------------ rack selection and keys

#[test]
fn ctrl_click_toggles_shift_click_ranges_and_ctrl_a_selects_everything() {
    let mut s = rack3();
    let (a, b, c) = (plate(&s, "a"), plate(&s, "b"), plate(&s, "c"));
    click(&mut s, a, Modifiers::NONE);
    click(&mut s, c, ctrl());
    assert_eq!(selection(&s), ["a", "c"]);
    click(&mut s, a, Modifiers::SHIFT);
    assert_eq!(selection(&s), ["a", "b", "c"], "range from the anchor");
    click(&mut s, b, Modifiers::NONE);
    assert_eq!(selection(&s), ["b"]);
    tap(&mut s, Os::Windows, "Cmd+A");
    assert_eq!(selection(&s), ["a", "b", "c"]);
}

#[test]
fn on_macos_command_click_toggles_and_the_keys_use_command() {
    let mut s = shell_for(Os::Mac);
    add(&mut s, SYNTH, "a");
    add(&mut s, DELAY, "b");
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    let (a, b) = (plate(&s, "a"), plate(&s, "b"));
    click(&mut s, a, Modifiers::NONE);
    let cmd = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    click(&mut s, b, cmd);
    assert_eq!(selection(&s), ["a", "b"]);
    tap(&mut s, Os::Mac, "Cmd+C");
    assert_eq!(s.core().clipboard().kind(), Some(ClipboardKind::Devices));
    // Ctrl+C on macOS is not the copy key.
    s.core_mut().clipboard_mut().clear();
    let ctrl_c = Modifiers::CTRL;
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: Key::Char('c'),
            modifiers: ctrl_c,
            pressed,
        });
    }
    assert!(s.core().clipboard().is_empty());
}

#[test]
fn copy_paste_cut_duplicate_and_delete_by_key_in_the_rack() {
    let mut s = rack3();
    cable(&mut s, ("a", "out_l"), ("b", "in_l"));
    let a = plate(&s, "a");
    let b = plate(&s, "b");
    click(&mut s, a, Modifiers::NONE);
    click(&mut s, b, ctrl());
    tap(&mut s, Os::Windows, "Cmd+C");
    assert_eq!(s.core().clipboard().kind(), Some(ClipboardKind::Devices));
    let before = saved(&s);
    tap(&mut s, Os::Windows, "Cmd+V");
    assert_eq!(devices(&s).len(), 5);
    assert_eq!(selection(&s).len(), 2, "the copies are selected");
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(saved(&s), before, "one undo step");
    click(&mut s, a, Modifiers::NONE);
    click(&mut s, b, ctrl());
    // Duplicate.
    tap(&mut s, Os::Windows, "Cmd+D");
    assert_eq!(devices(&s).len(), 5);
    tap(&mut s, Os::Windows, "Cmd+Z");
    // Delete the selection with the key, in one undo step.
    click(&mut s, a, Modifiers::NONE);
    click(&mut s, b, ctrl());
    tap(&mut s, Os::Windows, "Delete");
    assert_eq!(devices(&s), ["c"]);
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(saved(&s), before);
    // Cut and paste moves a device.
    let c = plate(&s, "c");
    click(&mut s, c, Modifiers::NONE);
    tap(&mut s, Os::Windows, "Cmd+X");
    assert!(!devices(&s).contains(&"c".to_string()));
    tap(&mut s, Os::Windows, "Cmd+V");
    assert_eq!(devices(&s).len(), 3);
}

#[test]
fn up_and_down_move_the_rack_selection_and_shift_extends_it() {
    let mut s = rack3();
    tap(&mut s, Os::Windows, "Down");
    assert_eq!(selection(&s), ["a"]);
    tap(&mut s, Os::Windows, "Down");
    assert_eq!(selection(&s), ["b"]);
    tap(&mut s, Os::Windows, "Shift+Down");
    assert_eq!(selection(&s), ["b", "c"]);
    tap(&mut s, Os::Windows, "Up");
    assert_eq!(selection(&s), ["b"]);
}

#[test]
fn rack_disconnect_runs_on_a_device_and_the_status_names_a_refusal() {
    let mut s = rack3();
    cable(&mut s, ("a", "out_l"), ("b", "in_l"));
    remap_disconnect(&mut s);
    let b = plate(&s, "b");
    click(&mut s, b, Modifiers::NONE);
    tap(&mut s, Os::Windows, "Alt+U");
    assert!(
        s.core()
            .doc()
            .cables
            .iter()
            .all(|c| c.from.device != key("b") && c.to.device != key("b"))
    );
    // Nothing left to remove: a message, no change.
    let before = saved(&s);
    tap(&mut s, Os::Windows, "Alt+U");
    assert_eq!(saved(&s), before);
    assert!(s.core().last_rejection().unwrap().contains("no cables"));
}

fn remap_disconnect(s: &mut Shell<NullBackend>) {
    use atomik_ui::keymap::{BindingDef, UserKeymap};
    let user = UserKeymap {
        bindings: vec![BindingDef {
            command: "rack.disconnect".into(),
            arg: None,
            context: Context::Rack,
            chords: vec!["Alt+U".into()],
            os: None,
        }],
        ..UserKeymap::default()
    };
    s.set_user_keymap(user, false);
}

#[test]
fn the_clipboard_commands_also_work_with_a_control_focused_and_do_nothing_in_the_console() {
    let mut s = rack3();
    let a = plate(&s, "a");
    click(&mut s, a, Modifiers::NONE);
    s.set_focus(Pane::Console);
    let before = saved(&s);
    tap(&mut s, Os::Windows, "Cmd+V");
    tap(&mut s, Os::Windows, "Cmd+C");
    assert_eq!(saved(&s), before);
    assert!(
        s.core().clipboard().is_empty(),
        "the console has no clipboard"
    );
}

// ------------------------------------------------------------------ context menus

fn open_menu_on(s: &mut Shell<NullBackend>, device: &str) -> Vec<(String, Option<String>, bool)> {
    let at = plate(s, device);
    open_menu(s, at)
}
fn open_menu(s: &mut Shell<NullBackend>, at: Point) -> Vec<(String, Option<String>, bool)> {
    press(s, at, PointerButton::Secondary, Modifiers::NONE);
    s.context_menu()
        .map(|m| {
            m.items()
                .map(|i| (i.command.clone(), i.shortcut.clone(), i.enabled))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn right_click_on_a_device_opens_the_menu_with_derived_shortcut_text() {
    let mut s = rack3();
    let items = open_menu_on(&mut s, "a");
    let ids: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(
        ids,
        [
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.duplicate",
            "edit.delete",
            "create.instrument",
            "create.effect",
            "rack.auto_route",
            "rack.assign_send",
            "rack.disconnect",
            "rack.go_to_track",
        ]
    );
    let text = |id: &str| {
        items
            .iter()
            .find(|(c, _, _)| c == id)
            .and_then(|(_, t, _)| t.clone())
    };
    assert_eq!(text("edit.copy").as_deref(), Some("Ctrl+C"));
    assert_eq!(text("edit.delete").as_deref(), Some("Delete"));
    assert_eq!(text("create.instrument").as_deref(), Some("Ctrl+I"));
    let enabled = |id: &str| items.iter().find(|(c, _, _)| c == id).unwrap().2;
    assert!(enabled("edit.cut") && enabled("edit.copy"));
    assert!(!enabled("edit.paste"), "the clipboard is empty");
    assert!(!enabled("rack.go_to_track"), "no song track yet");
    assert_eq!(
        selection(&s),
        ["a"],
        "a right click selects what it points at"
    );
}

#[test]
fn the_menu_shows_the_command_chords_of_macos() {
    let mut s = shell_for(Os::Mac);
    add(&mut s, SYNTH, "a");
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    let items = open_menu_on(&mut s, "a");
    let text = |id: &str| {
        items
            .iter()
            .find(|(c, _, _)| c == id)
            .and_then(|(_, t, _)| t.clone())
    };
    assert_eq!(text("edit.copy").as_deref(), Some("Cmd+C"));
    assert_eq!(text("create.effect").as_deref(), Some("Cmd+E"));
}

#[test]
fn choosing_an_item_runs_the_command_and_closes_the_menu() {
    let mut s = rack3();
    open_menu_on(&mut s, "a");
    let at = s.context_menu().unwrap().item_center("edit.copy").unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert!(s.context_menu().is_none());
    assert_eq!(s.core().clipboard().kind(), Some(ClipboardKind::Devices));
    // Now Paste is enabled in the menu of another device and pastes after it.
    let items = open_menu_on(&mut s, "c");
    assert!(items.iter().find(|(c, _, _)| c == "edit.paste").unwrap().2);
    let at = s.context_menu().unwrap().item_center("edit.paste").unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert_eq!(devices(&s).len(), 4);
    assert!(s.core().document().can_undo());
}

#[test]
fn delete_from_the_menu_removes_the_whole_selection_in_one_step() {
    let mut s = rack3();
    let (a, c) = (plate(&s, "a"), plate(&s, "c"));
    click(&mut s, a, Modifiers::NONE);
    click(&mut s, c, ctrl());
    let before = saved(&s);
    open_menu(&mut s, c);
    assert_eq!(
        selection(&s),
        ["a", "c"],
        "a right click on a member keeps the group"
    );
    let at = s
        .context_menu()
        .unwrap()
        .item_center("edit.delete")
        .unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert_eq!(devices(&s), ["b"]);
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(saved(&s), before);
}

#[test]
fn escape_and_a_click_elsewhere_close_the_menu_without_running_anything() {
    let mut s = rack3();
    let before = saved(&s);
    open_menu_on(&mut s, "a");
    tap(&mut s, Os::Windows, "Escape");
    assert!(s.context_menu().is_none());
    open_menu_on(&mut s, "a");
    click(&mut s, Point::new(1090.0, 760.0), Modifiers::NONE);
    assert!(s.context_menu().is_none());
    assert_eq!(saved(&s), before);
}

#[test]
fn the_menu_swallows_the_keys_while_it_is_open() {
    let mut s = rack3();
    open_menu_on(&mut s, "a");
    tap(&mut s, Os::Windows, "Cmd+A");
    assert!(
        s.context_menu().is_some(),
        "keys do not run behind the menu"
    );
    assert_eq!(selection(&s), ["a"]);
}

#[test]
fn right_click_on_the_empty_rack_offers_paste_create_and_select_all() {
    let mut s = rack3();
    let l = s.layout();
    let empty = Point::new(500.0, l.rack.y1 - 4.0);
    let items = open_menu(&mut s, empty);
    let ids: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(
        ids,
        [
            "edit.paste",
            "create.instrument",
            "create.effect",
            "edit.select_all"
        ]
    );
    let at = s
        .context_menu()
        .unwrap()
        .item_center("edit.select_all")
        .unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert_eq!(selection(&s).len(), 3);
}

#[test]
fn go_to_track_from_the_device_menu_selects_the_track_in_the_arrangement() {
    let mut s = rack3();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    s.core_mut().after_view_change();
    open_menu_on(&mut s, "a");
    let at = s
        .context_menu()
        .unwrap()
        .item_center("rack.go_to_track")
        .unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert_eq!(s.core().arrangement().selected_track(), Some(&key("a")));
    assert!(s.core().arrangement().selected_clips().is_empty());
}

// ------------------------------------------------------------------ arrangement and roll

fn with_clips() -> Shell<NullBackend> {
    let mut s = shell_for(Os::Windows);
    add(&mut s, SYNTH, "a");
    for c in [
        SongCommand::AddTrack { device: key("a") },
        SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: atomik_model::Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                grid: None,
                content: atomik_model::ClipContent::Notes(vec![atomik_model::Note {
                    tick: 0,
                    length: 15_360,
                    key: 60,
                    velocity: 0.8,
                }]),
            },
        },
    ] {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    }
    s.core_mut().after_view_change();
    s.set_focus(Pane::Arrangement);
    s
}

fn clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .map(|l| l.clips.len())
        .sum()
}

#[test]
fn clip_keys_copy_paste_duplicate_join_and_delete() {
    let mut s = with_clips();
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1)]);
    tap(&mut s, Os::Windows, "Cmd+C");
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(2 * BAR));
    tap(&mut s, Os::Windows, "Cmd+V");
    assert_eq!(clip_count(&s), 2);
    // Both clips selected: join them into one (they are on one lane, not looped).
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1), ClipId(2)]);
    tap(&mut s, Os::Windows, "Cmd+J");
    assert_eq!(clip_count(&s), 1);
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(clip_count(&s), 2);
    // Join with one clip selected says so.
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1)]);
    tap(&mut s, Os::Windows, "Cmd+J");
    assert!(s.core().last_rejection().unwrap().contains("two or more"));
    // Duplicate and delete.
    tap(&mut s, Os::Windows, "Cmd+D");
    assert_eq!(clip_count(&s), 3);
    tap(&mut s, Os::Windows, "Delete");
    assert_eq!(clip_count(&s), 2, "the copy was the selection");
}

#[test]
fn up_and_down_select_tracks_and_ctrl_d_duplicates_the_selected_track() {
    let mut s = with_clips();
    tap(&mut s, Os::Windows, "Down");
    assert_eq!(s.core().arrangement().selected_track(), Some(&key("a")));
    tap(&mut s, Os::Windows, "Cmd+D");
    assert_eq!(s.core().doc().song.tracks.len(), 2);
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(s.core().doc().song.tracks.len(), 1);
}

#[test]
fn right_click_on_a_clip_opens_the_clip_menu() {
    let mut s = with_clips();
    let r = s.core().arrangement().clip_rect(ClipId(1)).unwrap();
    let items = open_menu(&mut s, r.center());
    let ids: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(
        ids,
        [
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.duplicate",
            "edit.delete",
            "sequencer.open_clip",
            "sequencer.split_at_playhead",
            "sequencer.join",
        ]
    );
    let enabled = |id: &str| items.iter().find(|(c, _, _)| c == id).unwrap().2;
    assert!(enabled("sequencer.open_clip"), "a note clip");
    assert!(!enabled("sequencer.join"), "one clip");
    assert_eq!(s.core().arrangement().selected_clips(), vec![ClipId(1)]);
    // Duplicate from the menu.
    let at = s
        .context_menu()
        .unwrap()
        .item_center("edit.duplicate")
        .unwrap();
    click(&mut s, at, Modifiers::NONE);
    assert_eq!(clip_count(&s), 2);
}

#[test]
fn right_click_on_a_track_header_opens_the_track_menu() {
    let mut s = with_clips();
    let at = s
        .core()
        .arrangement()
        .track_header_center(&key("a"))
        .unwrap();
    let items = open_menu(&mut s, at);
    assert!(items.iter().any(|(c, _, _)| c == "edit.duplicate"));
    assert_eq!(s.core().arrangement().selected_track(), Some(&key("a")));
    let m = s.context_menu().unwrap();
    let dup = m.items().find(|i| i.command == "edit.duplicate").unwrap();
    assert_eq!(dup.label, "Duplicate Track");
}

#[test]
fn piano_roll_keys_copy_paste_and_duplicate_notes() {
    let mut s = with_clips();
    s.core_mut().open_piano_roll(ClipId(1));
    s.set_focus(Pane::PianoRoll);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    tap(&mut s, Os::Windows, "Cmd+C");
    assert_eq!(s.core().clipboard().kind(), Some(ClipboardKind::Notes));
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(15_360));
    tap(&mut s, Os::Windows, "Cmd+V");
    let notes =
        |s: &Shell<NullBackend>| match &s.core().doc().song.clip(ClipId(1)).unwrap().2.content {
            atomik_model::ClipContent::Notes(n) => n.len(),
            _ => 0,
        };
    assert_eq!(notes(&s), 2);
    tap(&mut s, Os::Windows, "Cmd+D");
    assert_eq!(notes(&s), 3);
    tap(&mut s, Os::Windows, "Cmd+Z");
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert_eq!(notes(&s), 1);
}

// ------------------------------------------------------------------ the Edit menu

#[test]
fn the_edit_menu_is_live_and_shows_the_derived_chords() {
    let mut s = rack3();
    let find = |s: &Shell<NullBackend>, label: &str| {
        s.menu_model()
            .iter()
            .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
            .find(|i| i.label == label)
            .unwrap()
    };
    let copy = find(&s, "Copy");
    assert_eq!(copy.shortcut.as_deref(), Some("Ctrl+C"));
    assert!(!copy.enabled, "nothing is selected");
    assert!(!find(&s, "Paste").enabled);
    let a = plate(&s, "a");
    click(&mut s, a, Modifiers::NONE);
    assert!(find(&s, "Copy").enabled && find(&s, "Delete").enabled);
    // Choosing Copy from the menu works on the focused pane.
    s.execute(AppCommand::Copy);
    assert_eq!(s.core().clipboard().kind(), Some(ClipboardKind::Devices));
    assert!(find(&s, "Paste").enabled);
}

// ------------------------------------------------------------------ goldens

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

/// The menu of a selected device with something to paste: every item enabled but Go to Track
/// (no song track).
fn device_menu(os: Os) -> Shell<NullBackend> {
    let mut s = shell_for(os);
    add(&mut s, SYNTH, "a");
    add(&mut s, DELAY, "b");
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    let a = plate(&s, "a");
    click(&mut s, a, Modifiers::NONE);
    tap(&mut s, os, "Cmd+C");
    open_menu_on(&mut s, "b");
    s
}

#[test]
fn rack_context_menu_device_matches_golden() {
    let s = device_menu(Os::Windows);
    golden("rack_context_menu_device", s.context_menu().unwrap());
}

#[test]
fn rack_context_menu_device_on_macos_matches_golden() {
    let s = device_menu(Os::Mac);
    golden("rack_context_menu_device_mac", s.context_menu().unwrap());
}

#[test]
fn rack_context_menu_background_matches_golden() {
    let mut s = rack3();
    let empty = Point::new(1090.0, s.layout().rack.y1 - 4.0);
    open_menu(&mut s, empty);
    golden("rack_context_menu_rack", s.context_menu().unwrap());
}

#[test]
fn clip_and_track_context_menus_match_golden() {
    let mut s = with_clips();
    let r = s.core().arrangement().clip_rect(ClipId(1)).unwrap();
    open_menu(&mut s, r.center());
    golden("arrangement_context_menu_clip", s.context_menu().unwrap());
    s.close_context_menu();
    let at = s
        .core()
        .arrangement()
        .track_header_center(&key("a"))
        .unwrap();
    open_menu(&mut s, at);
    golden("arrangement_context_menu_track", s.context_menu().unwrap());
}

// ------------------------------------------------------------------ the other menus

#[test]
fn right_click_on_an_empty_lane_opens_the_lane_menu_and_paste_lands_at_the_playhead() {
    let mut s = with_clips();
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1)]);
    tap(&mut s, Os::Windows, "Cmd+C");
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(3 * BAR));
    let lane = s.core().arrangement().lane_rect(&key("a"), 0).unwrap();
    let at = Point::new(s.core().arrangement().tick_to_x(6 * BAR), lane.center().y);
    let items = open_menu(&mut s, at);
    let ids: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(ids, ["edit.paste", "edit.select_all"]);
    assert!(items[0].2, "the clipboard holds a clip");
    let item = s.context_menu().unwrap().item_center("edit.paste").unwrap();
    click(&mut s, item, Modifiers::NONE);
    let starts: Vec<i64> = s
        .core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes[0].clips.iter().map(|c| c.start))
        .collect();
    assert_eq!(starts, [0, 3 * BAR]);
}

#[test]
fn right_click_on_the_roll_grid_opens_the_notes_menu_and_a_note_still_deletes() {
    let mut s = with_clips();
    s.core_mut().open_piano_roll(ClipId(1));
    s.set_focus(Pane::PianoRoll);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    tap(&mut s, Os::Windows, "Cmd+C");
    let roll = s.core().piano_roll();
    let top = s.layout().song.unwrap().y0;
    let at = Point::new(roll.tick_to_x(3 * 15_360), top + 160.0);
    let items = open_menu(&mut s, at);
    let ids: Vec<&str> = items.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(
        ids,
        [
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.duplicate",
            "edit.delete",
            "editor.quantize",
            "edit.select_all"
        ]
    );
    assert!(items.iter().find(|(c, _, _)| c == "edit.paste").unwrap().2);
    let item = s
        .context_menu()
        .unwrap()
        .item_center("edit.select_all")
        .unwrap();
    click(&mut s, item, Modifiers::NONE);
    assert_eq!(s.core().piano_roll().selected_notes(), vec![0]);
}

// ------------------------------------------------------------------ random gestures

/// A deterministic generator (no dependency, no clock): the same seed gives the same script.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// Random selections and clipboard keys in the rack never end in a refused command:
/// every gesture the UI offers is valid for the document it was offered on.
#[test]
fn random_rack_gestures_never_produce_a_refused_command() {
    for seed in 0..12 {
        let mut s = rack3();
        let mut rng = Lcg(seed + 1);
        let mut steps = 0;
        for _ in 0..40 {
            let names = devices(&s);
            match rng.next(9) {
                0 | 1 if !names.is_empty() => {
                    let k = names[rng.next(names.len())].clone();
                    let m = if rng.next(2) == 0 {
                        Modifiers::NONE
                    } else {
                        ctrl()
                    };
                    let p = plate(&s, &k);
                    click(&mut s, p, m);
                }
                2 => tap(&mut s, Os::Windows, "Cmd+A"),
                3 => tap(&mut s, Os::Windows, "Cmd+C"),
                4 => tap(&mut s, Os::Windows, "Cmd+V"),
                5 => tap(&mut s, Os::Windows, "Cmd+D"),
                6 => tap(&mut s, Os::Windows, "Cmd+X"),
                7 => tap(&mut s, Os::Windows, "Delete"),
                _ => tap(&mut s, Os::Windows, "Cmd+Z"),
            }
            steps += 1;
            // A device may be selected that a gesture already removed: pruned by the view.
            let rejection = s.core().last_rejection().map(str::to_string);
            if let Some(m) = rejection {
                assert!(
                    m.contains("Nothing is selected") || m.contains("clipboard is empty"),
                    "seed {seed} step {steps}: a refusal that is not a missing selection: {m}"
                );
            }
        }
        // Undo everything, including the three devices the test added: the empty rack is back.
        while s.core().document().can_undo() {
            s.core_mut().undo();
        }
        assert_eq!(
            saved(&s),
            atomik_model::save_project(&atomik_model::empty_doc()),
            "seed {seed}: undoing every step returns the empty rack"
        );
    }
}

#[test]
fn the_disconnect_item_is_enabled_for_a_group_too() {
    let mut s = rack3();
    cable(&mut s, ("a", "out_l"), ("b", "in_l"));
    let enabled = |items: &[(String, Option<String>, bool)]| {
        items
            .iter()
            .find(|(c, _, _)| c == "rack.disconnect")
            .unwrap()
            .2
    };
    let items = open_menu_on(&mut s, "b");
    assert!(enabled(&items), "one selected device");
    // Close the menu, select a group and open it again on a member.
    tap(&mut s, Os::Windows, "Escape");
    let c = plate(&s, "c");
    press(&mut s, c, PointerButton::Primary, Modifiers::CTRL);
    assert_eq!(selection(&s), ["b", "c"]);
    let items = open_menu_on(&mut s, "b");
    assert!(enabled(&items), "Disconnect works on a group");
}
