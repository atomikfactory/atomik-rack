//! Track organisation in the shell: the Track menu and its state, the group and move keys, typing
//! a folder name or a track comment in place, the track header and folder row context menus,
//! the height presets and a saved project that keeps it all.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey, SongCommand};
use atomik_ui::PointerButton;
use atomik_ui::arrangement::{FolderButton, HEIGHT_LARGE, HeightPreset};
use atomik_ui::keymap::Os;
use atomik_ui::keymap::script::event_for;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A headless shell with four synth tracks, `a` to `d`, the arrangement focused.
fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    for k in ["a", "b", "c", "d"] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.to_string(),
                key: Some(key(k)),
            })
            .unwrap();
        s.core_mut()
            .apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
            .unwrap();
    }
    s.core_mut().after_view_change();
    s.set_focus(Pane::Arrangement);
    s
}

fn press(s: &mut Shell<NullBackend>, p: Point, button: PointerButton, modifiers: Modifiers) {
    for down in [true, false] {
        s.handle(&if down {
            UiEvent::PointerDown {
                pos: p,
                button,
                modifiers,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos: p,
                button,
                modifiers,
            }
        });
    }
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    let (k, m) = event_for(Os::Windows, chord).unwrap();
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: k.clone(),
            modifiers: m,
            pressed,
        });
    }
}

fn header(s: &Shell<NullBackend>, k: &str) -> Point {
    s.core().arrangement().track_header_center(&key(k)).unwrap()
}

fn order(s: &Shell<NullBackend>) -> Vec<String> {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

fn track_menu(s: &Shell<NullBackend>) -> Vec<(String, bool, bool)> {
    s.menu_model()
        .iter()
        .find(|m| m.title == "Track")
        .expect("a Track menu")
        .items()
        .map(|i| (i.label.to_string(), i.enabled, i.shortcut.is_some()))
        .collect()
}

fn enabled(s: &Shell<NullBackend>, label: &str) -> bool {
    track_menu(s)
        .into_iter()
        .find(|(l, _, _)| l == label)
        .unwrap_or_else(|| panic!("{label}"))
        .1
}

/// Selects tracks `a` and `b` in the arrangement.
fn select_ab(s: &mut Shell<NullBackend>) {
    let arr = s.core_mut().arrangement_mut();
    arr.set_selected_track(Some(key("a")));
    arr.toggle_selected_track(&key("b"));
}

#[test]
fn the_track_menu_follows_the_selection() {
    let mut s = shell();
    let labels: Vec<String> = track_menu(&s).into_iter().map(|(l, _, _)| l).collect();
    assert_eq!(
        labels,
        [
            "Group Tracks",
            "Ungroup",
            "Collapse Folder",
            "Rename Folder\u{2026}",
            "Move Track Up",
            "Move Track Down",
            "Track Comment\u{2026}",
            "Freeze Track",
            "Freeze Track up to Selected Device",
            "Freeze with Tail",
            "Unfreeze Track",
            "Unfreeze Track and Keep Audio",
            "Cancel Freeze",
            "Bounce in Place",
            "Bounce in Place and Mute Originals",
            "Fold Automation Lanes",
            "Unfold Automation Lanes",
            "Track Height: Small",
            "Track Height: Medium",
            "Track Height: Large",
            "Track Height: Fit",
        ]
    );
    for label in [
        "Group Tracks",
        "Ungroup",
        "Rename Folder\u{2026}",
        "Move Track Up",
        "Track Comment\u{2026}",
    ] {
        assert!(!enabled(&s, label), "{label}: nothing is selected");
    }
    assert!(
        enabled(&s, "Track Height: Large"),
        "heights need no selection"
    );
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("b")));
    assert!(enabled(&s, "Group Tracks") && enabled(&s, "Track Comment\u{2026}"));
    assert!(enabled(&s, "Move Track Up") && enabled(&s, "Move Track Down"));
    assert!(!enabled(&s, "Ungroup") && !enabled(&s, "Rename Folder\u{2026}"));
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("a")));
    assert!(!enabled(&s, "Move Track Up"), "the first track");
    s.execute(AppCommand::GroupTracks);
    assert!(
        enabled(&s, "Ungroup"),
        "the selected track is in a folder now"
    );
    s.core_mut().arrangement_mut().set_selected_group(Some(1));
    assert!(enabled(&s, "Rename Folder\u{2026}"));
    s.execute(AppCommand::ToggleFolder);
    assert!(
        track_menu(&s)
            .iter()
            .any(|(l, e, _)| l == "Expand Folder" && *e)
    );
}

#[test]
fn group_tracks_in_the_menu_and_by_key_makes_a_folder_in_one_undo_step() {
    let mut s = shell();
    select_ab(&mut s);
    let shortcut = |s: &Shell<NullBackend>, label: &str| {
        s.menu_model()
            .iter()
            .find(|m| m.title == "Track")
            .unwrap()
            .items()
            .find(|i| i.label == label)
            .unwrap()
            .shortcut
            .clone()
    };
    assert_eq!(
        shortcut(&s, "Group Tracks").as_deref(),
        Some("Ctrl+Shift+G")
    );
    assert_eq!(shortcut(&s, "Move Track Up").as_deref(), Some("Alt+Up"));
    assert_eq!(shortcut(&s, "Ungroup"), None, "no default key");
    let undo_depth = |s: &Shell<NullBackend>| s.core().document().can_undo();
    assert!(undo_depth(&s));
    s.execute(AppCommand::GroupTracks);
    assert_eq!(s.core().doc().song.org.groups.len(), 1);
    assert_eq!(s.core().doc().song.org.groups[0].name, "Folder 1");
    assert!(s.core_mut().undo());
    assert!(s.core().doc().song.org.groups.is_empty());
    // The key does the same with the arrangement focused.
    select_ab(&mut s);
    tap(&mut s, "Ctrl+Shift+G");
    assert_eq!(s.core().doc().song.org.groups.len(), 1);
    // Without a selection the key says what is missing.
    s.core_mut().arrangement_mut().set_selected_track(None);
    s.core_mut().arrangement_mut().set_selected_group(None);
    tap(&mut s, "Ctrl+Shift+G");
    assert_eq!(s.core().doc().song.org.groups.len(), 1, "nothing new");
}

#[test]
fn the_move_keys_and_menu_items_move_the_track_and_the_folder() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("c")));
    tap(&mut s, "Alt+Up");
    assert_eq!(order(&s), ["a", "c", "b", "d"]);
    s.execute(AppCommand::MoveTrackDown);
    assert_eq!(order(&s), ["a", "b", "c", "d"]);
    // A folder moves with its tracks.
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    s.core_mut().arrangement_mut().set_selected_group(Some(1));
    tap(&mut s, "Alt+Down");
    assert_eq!(order(&s), ["c", "a", "b", "d"]);
}

#[test]
fn a_folder_row_collapses_and_its_menu_item_says_so() {
    let mut s = shell();
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    let toggle = s.core().arrangement().folder_toggle_center(1).unwrap();
    press(&mut s, toggle, PointerButton::Primary, Modifiers::NONE);
    assert!(s.core().doc().song.org.collapsed(1));
    assert!(s.core().arrangement().track_hidden(&key("a")));
    assert!(s.core().arrangement().lane_rect(&key("a"), 0).is_none());
    let mute = s
        .core()
        .arrangement()
        .folder_button_center(1, FolderButton::Mute)
        .unwrap();
    press(&mut s, mute, PointerButton::Primary, Modifiers::NONE);
    let song = &s.core().doc().song;
    assert!(song.track(&key("a")).unwrap().muted && song.track(&key("b")).unwrap().muted);
    assert!(!song.track(&key("c")).unwrap().muted);
    assert!(s.core_mut().undo(), "the folder mute is one undo step");
    assert!(!s.core().doc().song.track(&key("a")).unwrap().muted);
}

#[test]
fn a_comment_is_typed_in_place_from_the_menu_and_keeps_the_keys() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("b")));
    s.set_focus(Pane::Rack);
    s.execute(AppCommand::TrackComment);
    assert_eq!(s.focus, Pane::Arrangement, "the focus follows the typing");
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Text, Context::Sequencer])
    );
    // Keys that are commands outside a text field are typed (G, M, Space, a chord with Shift).
    for chord in ["Shift+L", "o", "g", "Space", "M"] {
        tap(&mut s, chord);
    }
    assert_eq!(s.core().arrangement().org_edit_text(), Some("log m"));
    assert!(s.core().doc().song.org.groups.is_empty(), "no command ran");
    tap(&mut s, "Enter");
    assert_eq!(s.core().doc().song.track_comment(&key("b")), "log m");
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Sequencer]),
        "the keys are commands again"
    );
    // One undo step.
    assert!(s.core_mut().undo());
    assert_eq!(s.core().doc().song.track_comment(&key("b")), "");
}

#[test]
fn rename_folder_from_the_menu_types_the_name() {
    let mut s = shell();
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    s.core_mut().arrangement_mut().set_selected_group(Some(1));
    s.execute(AppCommand::RenameFolder);
    assert_eq!(s.core().arrangement().org_edit_text(), Some("Folder 1"));
    for _ in 0..8 {
        tap(&mut s, "Backspace");
    }
    for chord in ["D", "r", "u", "m", "s"] {
        tap(&mut s, chord);
    }
    tap(&mut s, "Enter");
    assert_eq!(s.core().doc().song.org.group(1).unwrap().name, "drums");
}

fn open_menu(s: &mut Shell<NullBackend>, at: Point) -> Vec<String> {
    press(s, at, PointerButton::Secondary, Modifiers::NONE);
    s.context_menu()
        .map(|m| m.items().map(|i| i.command.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn the_track_header_menu_offers_the_organisation_commands() {
    let mut s = shell();
    let at = header(&s, "b");
    let ids = open_menu(&mut s, at);
    for id in [
        "sequencer.group_tracks",
        "sequencer.move_track_up",
        "sequencer.move_track_down",
        "sequencer.track_comment",
        "sequencer.fold_lanes",
    ] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
    }
    let menu = s.context_menu().unwrap();
    assert!(menu.submenu_center("Colour").is_some());
    assert!(menu.submenu_center("Height").is_some());
    let enabled = |id: &str| {
        s.context_menu()
            .unwrap()
            .items()
            .find(|i| i.command == id)
            .unwrap()
            .enabled
    };
    assert!(enabled("sequencer.group_tracks") && enabled("sequencer.track_comment"));
    assert!(!enabled("sequencer.ungroup_tracks"), "b is in no folder");
    s.close_context_menu();
}

#[test]
fn choosing_a_colour_and_a_height_from_the_header_menu_changes_the_track() {
    let mut s = shell();
    let at = header(&s, "b");
    press(&mut s, at, PointerButton::Secondary, Modifiers::NONE);
    let sub = s.context_menu().unwrap().submenu_center("Colour").unwrap();
    s.handle(&UiEvent::PointerMove {
        pos: sub,
        modifiers: Modifiers::NONE,
    });
    let item = s
        .context_menu()
        .unwrap()
        .item_center_with("sequencer.track_color", "green")
        .unwrap();
    press(&mut s, item, PointerButton::Primary, Modifiers::NONE);
    assert!(s.context_menu().is_none());
    assert_eq!(
        s.core().doc().song.track_color(&key("b")),
        Some([0x5a, 0xb8, 0x6a])
    );
    assert_eq!(s.core().doc().song.track_color(&key("a")), None);
    let at = header(&s, "b");
    press(&mut s, at, PointerButton::Secondary, Modifiers::NONE);
    let sub = s.context_menu().unwrap().submenu_center("Height").unwrap();
    s.handle(&UiEvent::PointerMove {
        pos: sub,
        modifiers: Modifiers::NONE,
    });
    let item = s
        .context_menu()
        .unwrap()
        .item_center_with("sequencer.track_height", "large")
        .unwrap();
    press(&mut s, item, PointerButton::Primary, Modifiers::NONE);
    let heights = &s.core().doc().song.track_heights;
    assert_eq!(heights.get(&key("b")), Some(&HEIGHT_LARGE));
    assert_eq!(heights.len(), 1, "only the clicked track");
}

#[test]
fn the_folder_row_menu_has_the_folder_commands_and_acts_on_the_folder() {
    let mut s = shell();
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    let at = s.core().arrangement().folder_header_center(1).unwrap();
    let ids = open_menu(&mut s, at);
    assert_eq!(
        ids,
        [
            "sequencer.rename_folder",
            "sequencer.toggle_folder",
            "sequencer.ungroup_tracks",
            "sequencer.move_track_up",
            "sequencer.move_track_down",
        ]
    );
    let m = s.context_menu().unwrap();
    let label = |id: &str| {
        m.items()
            .find(|i| i.command == id)
            .map(|i| i.label.clone())
            .unwrap()
    };
    assert_eq!(label("sequencer.toggle_folder"), "Collapse Folder");
    assert_eq!(label("sequencer.move_track_down"), "Move Folder Down");
    let ungroup = m.item_center("sequencer.ungroup_tracks").unwrap();
    press(&mut s, ungroup, PointerButton::Primary, Modifiers::NONE);
    assert!(s.core().doc().song.org.groups.is_empty());
    assert_eq!(order(&s).len(), 4);
}

#[test]
fn the_height_presets_run_from_the_menu_and_fit_shows_every_track() {
    let mut s = shell();
    s.execute(AppCommand::TrackHeight(HeightPreset::Large));
    assert_eq!(s.core().doc().song.track_heights.len(), 4);
    s.execute(AppCommand::TrackHeight(HeightPreset::Medium));
    assert!(s.core().doc().song.track_heights.is_empty());
    s.execute(AppCommand::TrackHeight(HeightPreset::Fit));
    let b = s.core().arrangement().lane_rect(&key("d"), 0).unwrap();
    assert!(b.y1 <= s.core().arrangement().bounds().y1, "{b:?}");
}

#[test]
fn folders_colours_comments_and_folds_survive_saving_and_opening() {
    let dir = std::env::temp_dir().join(format!("atomik-organise-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("organised.atomik");
    let mut s = shell();
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    s.core_mut().arrangement_mut().set_selected_group(Some(1));
    s.execute(AppCommand::ToggleFolder);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("c")));
    for c in [
        SongCommand::SetTrackColor {
            device: key("c"),
            color: Some([1, 2, 3]),
        },
        SongCommand::SetTrackComment {
            device: key("c"),
            comment: "keep me".into(),
        },
    ] {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    }
    let before = s.core().doc().song.org.clone();
    s.core_mut().save_as(&path).unwrap();
    let mut t = shell();
    t.core_mut().open(&path).unwrap();
    assert_eq!(t.core().doc().song.org, before);
    assert!(t.core().arrangement().track_hidden(&key("a")));
    let _ = std::fs::remove_dir_all(&dir);
}
