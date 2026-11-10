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
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1100.0)));
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
            "Rename Track\u{2026}",
            "Track Comment\u{2026}",
            "Delete Track and Device",
            "Add Note Lane",
            "Rename Note Lane\u{2026}",
            "Delete Note Lane",
            "Mute Lane",
            "Merge Note Lanes",
            "Explode Notes to Lanes",
            "Freeze Track",
            "Freeze Track up to Selected Device",
            "Freeze with Tail",
            "Freeze with Strip",
            "Re-render Frozen Track",
            "Unfreeze Track",
            "Unfreeze Track and Keep Audio",
            "Cancel Freeze",
            "Bounce in Place",
            "Bounce in Place and Mute Originals",
            "Bounce Clips to New Track (Dry)",
            "Bounce Range in Place",
            "Send Players to Track",
            "Direct Record",
            "Fold All Players",
            "Bypass All Players",
            "Add Pattern Lane",
            "Pattern Lane to Notes",
            "Edit Track Controllers",
            "Add Reverb Send\u{2026}",
            "Duck from\u{2026}",
            "Duck from (Pre-Fader)\u{2026}",
            "Gentle Pump",
            "Remove Ducking",
            "Tail Repeat",
            "Reverb Tail",
            "0.5 s",
            "1 s",
            "2 s",
            "4 s",
            "8 s",
            "Riser, 1 Bar",
            "Riser, 2 Bars",
            "Riser, 4 Bars",
            "Impact",
            "Filter Down, 1 Bar",
            "Filter Down, 2 Bars",
            "Filter Down, 4 Bars",
            "Group Filter Down, 1 Bar",
            "Group Filter Down, 2 Bars",
            "Group Filter Down, 4 Bars",
            "Silence on Beat One",
            "Fill",
            "Fold Automation Lanes",
            "Unfold Automation Lanes",
            "Track Height: Small",
            "Track Height: Medium",
            "Track Height: Large",
            "Track Height: Fit",
            "Track Height: Minimise / Restore",
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
        Some("Ctrl+Shift+T")
    );
    assert_eq!(
        shortcut(&s, "Move Track Up").as_deref(),
        Some("Ctrl+Alt+Shift+Up")
    );
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
    tap(&mut s, "Ctrl+Shift+T");
    assert_eq!(s.core().doc().song.org.groups.len(), 1);
    // Without a selection the key says what is missing.
    s.core_mut().arrangement_mut().set_selected_track(None);
    s.core_mut().arrangement_mut().set_selected_group(None);
    tap(&mut s, "Ctrl+Shift+T");
    assert_eq!(s.core().doc().song.org.groups.len(), 1, "nothing new");
}

#[test]
fn the_move_keys_and_menu_items_move_the_track_and_the_folder() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("c")));
    tap(&mut s, "Ctrl+Shift+Alt+Up");
    assert_eq!(order(&s), ["a", "c", "b", "d"]);
    s.execute(AppCommand::MoveTrackDown);
    assert_eq!(order(&s), ["a", "b", "c", "d"]);
    // A folder moves with its tracks.
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    s.core_mut().arrangement_mut().set_selected_group(Some(1));
    tap(&mut s, "Ctrl+Shift+Alt+Down");
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
    // The mute is the folder's own: its tracks keep their flags and play as muted with it.
    let song = &s.core().doc().song;
    assert!(song.org.group(1).unwrap().muted);
    assert!(song.muted_by_folder(&key("a")) && song.muted_by_folder(&key("b")));
    assert!(!song.muted_by_folder(&key("c")));
    assert!(!song.track(&key("a")).unwrap().muted);
    assert!(s.core_mut().undo(), "the folder mute is one undo step");
    assert!(!s.core().doc().song.org.group(1).unwrap().muted);
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
            "mixer.create_vca",
            "mixer.create_vca_for_folder",
            "mixer.route_folder",
            "mixer.unroute_folder",
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

#[test]
fn rename_track_from_the_menu_types_the_name_in_one_undo_step() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("b")));
    s.set_focus(Pane::Rack);
    s.execute(AppCommand::RenameTrack);
    assert_eq!(s.focus, Pane::Arrangement, "the focus follows the typing");
    assert!(s.core().arrangement().is_text_owner());
    for _ in 0..40 {
        tap(&mut s, "Backspace");
    }
    for chord in ["l", "o", "w"] {
        tap(&mut s, chord);
    }
    tap(&mut s, "Enter");
    let name = |s: &Shell<NullBackend>| s.core().doc().device(&key("b")).unwrap().name.clone();
    assert_eq!(name(&s), "low");
    assert!(s.core_mut().undo());
    assert_ne!(name(&s), "low");
}

#[test]
fn ctrl_r_merges_the_note_lanes_of_the_selected_track_in_one_undo_step() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddNoteLane {
            device: key("a"),
            name: None,
        }))
        .unwrap();
    s.core_mut().after_view_change();
    let at = header(&s, "a");
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    s.set_focus(Pane::Arrangement);
    tap(&mut s, "Cmd+R");
    assert_eq!(
        s.core()
            .doc()
            .song
            .track(&key("a"))
            .unwrap()
            .note_lane_count(),
        1
    );
    tap(&mut s, "Cmd+Z");
    assert_eq!(
        s.core()
            .doc()
            .song
            .track(&key("a"))
            .unwrap()
            .note_lane_count(),
        2
    );
}

// ------------------------------------------------------------------ moving tracks in folders

/// Puts the tracks `names` (consecutive, in order) into one folder with the Group Tracks command.
fn folder_of(s: &mut Shell<NullBackend>, names: &[&str]) {
    let arr = s.core_mut().arrangement_mut();
    arr.set_selected_track(Some(key(names[0])));
    for n in &names[1..] {
        arr.toggle_selected_track(&key(n));
    }
    s.execute(AppCommand::GroupTracks);
}

/// The rows of the arrangement in display order: track names, `[folder]` for a header.
fn rows(s: &Shell<NullBackend>) -> Vec<String> {
    use atomik_model::OutlineKind;
    let song = &s.core().doc().song;
    song.outline()
        .iter()
        .map(|e| match e.kind {
            OutlineKind::Folder(_) => "[folder]".to_string(),
            OutlineKind::Track(i) => song.tracks[i].device.0.clone(),
        })
        .collect()
}

fn pick(s: &mut Shell<NullBackend>, k: &str) {
    let at = header(s, k);
    press(s, at, PointerButton::Primary, Modifiers::NONE);
    s.set_focus(Pane::Arrangement);
}

#[test]
fn a_track_in_the_middle_of_a_folder_moves_up_and_down_inside_it() {
    let mut s = shell();
    folder_of(&mut s, &["a", "b", "c"]);
    assert_eq!(rows(&s), ["[folder]", "a", "b", "c", "d"]);
    pick(&mut s, "b");
    tap(&mut s, "Ctrl+Shift+Alt+Up");
    assert_eq!(rows(&s), ["[folder]", "b", "a", "c", "d"]);
    assert_eq!(s.core().doc().song.track_group(&key("b")), Some(1));
    tap(&mut s, "Ctrl+Shift+Alt+Down");
    assert_eq!(rows(&s), ["[folder]", "a", "b", "c", "d"]);
    assert_eq!(s.core().doc().song.track_group(&key("b")), Some(1));
}

#[test]
fn the_first_track_of_a_folder_leaves_it_and_moves_above_the_header_in_one_press() {
    let mut s = shell();
    folder_of(&mut s, &["b", "c"]);
    assert_eq!(rows(&s), ["a", "[folder]", "b", "c", "d"]);
    pick(&mut s, "b");
    tap(&mut s, "Ctrl+Shift+Alt+Up");
    assert_eq!(rows(&s), ["a", "b", "[folder]", "c", "d"]);
    assert_eq!(s.core().doc().song.track_group(&key("b")), None);
    assert!(s.core_mut().undo(), "one undo step");
    assert_eq!(rows(&s), ["a", "[folder]", "b", "c", "d"]);
}

#[test]
fn the_last_track_of_a_folder_leaves_it_below_the_members_in_one_press() {
    let mut s = shell();
    folder_of(&mut s, &["b", "c"]);
    pick(&mut s, "c");
    tap(&mut s, "Ctrl+Shift+Alt+Down");
    assert_eq!(rows(&s), ["a", "[folder]", "b", "c", "d"]);
    assert_eq!(s.core().doc().song.track_group(&key("c")), None);
    tap(&mut s, "Ctrl+Shift+Alt+Down");
    assert_eq!(rows(&s), ["a", "[folder]", "b", "d", "c"]);
}

#[test]
fn a_track_above_a_folder_moves_down_into_it() {
    let mut s = shell();
    folder_of(&mut s, &["c", "d"]);
    pick(&mut s, "b");
    tap(&mut s, "Ctrl+Shift+Alt+Down");
    assert_eq!(rows(&s), ["a", "[folder]", "b", "c", "d"]);
    assert_eq!(s.core().doc().song.track_group(&key("b")), Some(1));
}

#[test]
fn clicking_a_track_name_after_a_folder_header_moves_the_track_not_the_folder() {
    let mut s = shell();
    folder_of(&mut s, &["b", "c"]);
    // The folder header was picked (the folder was just created and is selected).
    let toggle = s.core().arrangement().folder_toggle_center(1).unwrap();
    press(
        &mut s,
        Point::new(toggle.x + 40.0, toggle.y),
        PointerButton::Primary,
        Modifiers::NONE,
    );
    pick(&mut s, "c");
    tap(&mut s, "Ctrl+Shift+Alt+Up");
    assert_eq!(rows(&s), ["a", "[folder]", "c", "b", "d"]);
}

/// Opens the Colour submenu of the header menu of track `k` and presses Custom.
fn pick_custom_colour(s: &mut Shell<NullBackend>, k: &str) {
    let at = header(s, k);
    press(s, at, PointerButton::Secondary, Modifiers::NONE);
    let sub = s.context_menu().unwrap().submenu_center("Colour").unwrap();
    s.handle(&UiEvent::PointerMove {
        pos: sub,
        modifiers: Modifiers::NONE,
    });
    let item = s
        .context_menu()
        .unwrap()
        .item_center_with("sequencer.track_color", "custom")
        .unwrap();
    press(s, item, PointerButton::Primary, Modifiers::NONE);
}

fn type_in_picker(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        s.handle(&UiEvent::Key {
            key: atomik_ui::Key::Char(c),
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
}

#[test]
fn the_custom_colour_item_opens_the_picker_and_enter_colours_the_track_and_its_device() {
    let mut s = shell();
    pick_custom_colour(&mut s, "b");
    assert!(s.color_picker_open());
    assert!(s.wants_text_input(), "the hex text takes input methods");
    type_in_picker(&mut s, "#102030");
    s.handle(&UiEvent::Key {
        key: atomik_ui::Key::Enter,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.color_picker_open());
    let doc = s.core().doc();
    assert_eq!(doc.song.track_color(&key("b")), Some([0x10, 0x20, 0x30]));
    assert_eq!(
        doc.device(&key("b")).unwrap().color,
        Some([0x10, 0x20, 0x30])
    );
    assert_eq!(doc.song.track_color(&key("a")), None);
    assert!(s.core_mut().undo(), "one undo step");
    assert_eq!(s.core().doc().song.track_color(&key("b")), None);
    assert_eq!(s.core().doc().device(&key("b")).unwrap().color, None);
}

#[test]
fn escape_closes_the_picker_without_changing_anything() {
    let mut s = shell();
    pick_custom_colour(&mut s, "b");
    type_in_picker(&mut s, "#ffffff");
    s.handle(&UiEvent::Key {
        key: atomik_ui::Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.color_picker_open());
    assert_eq!(s.core().doc().song.track_color(&key("b")), None);
}

#[test]
fn the_picker_colours_a_folder_from_the_folder_menu_item() {
    let mut s = shell();
    select_ab(&mut s);
    s.execute(AppCommand::GroupTracks);
    let at = s.core().arrangement().folder_header_center(1).unwrap();
    press(&mut s, at, PointerButton::Secondary, Modifiers::NONE);
    let sub = s.context_menu().unwrap().submenu_center("Colour").unwrap();
    s.handle(&UiEvent::PointerMove {
        pos: sub,
        modifiers: Modifiers::NONE,
    });
    let item = s
        .context_menu()
        .unwrap()
        .item_center_with("sequencer.track_color", "custom")
        .unwrap();
    press(&mut s, item, PointerButton::Primary, Modifiers::NONE);
    assert!(s.color_picker_open());
    type_in_picker(&mut s, "#abcdef");
    s.handle(&UiEvent::Key {
        key: atomik_ui::Key::Enter,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    let song = &s.core().doc().song;
    assert_eq!(song.org.group(1).unwrap().color, Some([0xab, 0xcd, 0xef]));
    // The tracks without a colour of their own show the folder's.
    assert_eq!(song.shown_color(&key("a")), Some([0xab, 0xcd, 0xef]));
    assert_eq!(song.track_color(&key("a")), None);
}

#[test]
fn a_context_submenu_opens_at_once_and_a_switch_wakes_the_loop_at_its_deadline() {
    use atomik_ui::context_menu::SUBMENU_DELAY;
    let mut s = shell();
    let at = header(&s, "b");
    press(&mut s, at, PointerButton::Secondary, Modifiers::NONE);
    let hover = |s: &mut Shell<NullBackend>, p: Point| {
        s.handle(&UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        });
    };
    let colour = s.context_menu().unwrap().submenu_center("Colour").unwrap();
    hover(&mut s, colour);
    assert_eq!(s.context_menu().unwrap().open_path(), ["Colour"]);
    assert!(!s.context_menu_pending(), "nothing to wait for");
    assert_eq!(s.context_menu_wake(), None);
    // Another submenu row: the loop is asked for a frame, then for one wake-up at the deadline.
    let height = s.context_menu().unwrap().submenu_center("Height").unwrap();
    hover(&mut s, height);
    assert_eq!(s.context_menu().unwrap().open_path(), ["Colour"]);
    assert!(s.context_menu_pending());
    s.frame_at(40.0);
    assert!(!s.context_menu_pending(), "it sleeps until the deadline");
    let due = s.context_menu_wake().unwrap();
    assert!((due - (40.0 + SUBMENU_DELAY)).abs() < 1e-9);
    s.frame_at(due - 0.01);
    assert_eq!(s.context_menu().unwrap().open_path(), ["Colour"]);
    s.frame_at(due);
    assert_eq!(s.context_menu().unwrap().open_path(), ["Height"]);
    assert!(s.needs_repaint());
    assert_eq!(s.context_menu_wake(), None);
    s.close_context_menu();
}
