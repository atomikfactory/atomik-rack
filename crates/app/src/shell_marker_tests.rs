//! Markers and ranges in the shell: the View toggles and their saved state, the keys (add a
//! marker, previous and next marker), the Transport menu, typing a name in place and the
//! context-menu items of the strip and the markers.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Marker, RangeMarker, SongCommand};
use atomik_ui::ContextTarget;
use atomik_ui::MarkSel;
use atomik_ui::context_menu::Spec;
use atomik_ui::keymap::Os;
use atomik_ui::keymap::script::event_for;

const BAR: i64 = 4 * 15_360;

fn shell_for(os: Os, profile: Option<Profile>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_for(Os::Windows, None)
}

fn tap(s: &mut Shell<NullBackend>, os: Os, chord: &str) {
    for pressed in [true, false] {
        let (key, modifiers) = event_for(os, chord).unwrap();
        s.handle(&UiEvent::Key {
            key,
            modifiers,
            pressed,
        });
    }
}

fn marker(id: u32, tick: i64, name: &str) -> Marker {
    Marker {
        id,
        tick,
        name: name.into(),
        color: None,
    }
}

fn locate(s: &mut Shell<NullBackend>, tick: i64) {
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(tick));
}

fn markers(s: &Shell<NullBackend>) -> Vec<(i64, String)> {
    s.core()
        .doc()
        .song
        .markers
        .iter()
        .map(|m| (m.tick, m.name.clone()))
        .collect()
}

#[test]
fn the_strip_is_on_and_the_list_off_by_default_and_the_commands_toggle_and_save_them() {
    let mut s = shell();
    assert!(s.core().arrangement().marker_strip_visible());
    assert!(!s.core().arrangement().marker_list_visible());
    assert!(s.ui_state().show_marker_strip && !s.ui_state().show_marker_list);
    for (id, command) in [
        ("view.toggle_marker_strip", AppCommand::ToggleMarkerStrip),
        ("view.toggle_marker_list", AppCommand::ToggleMarkerList),
        ("transport.add_marker", AppCommand::AddMarker),
        (
            "transport.add_range_from_loop",
            AppCommand::AddRangeFromLoop,
        ),
        ("transport.prev_marker", AppCommand::PreviousMarker),
        ("transport.next_marker", AppCommand::NextMarker),
        ("transport.set_end_marker", AppCommand::SetEndMarker),
    ] {
        assert_eq!(AppCommand::from_command(id, None), Some(command), "{id}");
        assert_eq!(command.command_id(), Some((id, None)));
    }
    s.execute(AppCommand::ToggleMarkerList);
    assert!(s.core().arrangement().marker_list_visible());
    assert!(s.ui_state().show_marker_list);
    assert!(s.menu_state().marker_list && s.menu_state().marker_strip);
    s.execute(AppCommand::ToggleMarkerStrip);
    assert!(!s.core().arrangement().marker_strip_visible());
    assert!(!s.ui_state().show_marker_strip);
    s.execute(AppCommand::ToggleMarkerList);
    s.execute(AppCommand::ToggleMarkerStrip);
    assert!(!s.ui_state().show_marker_list && s.ui_state().show_marker_strip);
}

#[test]
fn the_saved_view_state_is_restored_at_start() {
    let dir = std::env::temp_dir().join(format!("atomik-marker-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-state.json");
    std::fs::write(
        &path,
        r#"{"show_marker_strip": false, "show_marker_list": true}"#,
    )
    .unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: Some(path),
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    assert!(!s.core().arrangement().marker_strip_visible());
    assert!(s.core().arrangement().marker_list_visible());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shift_m_adds_a_marker_at_the_playhead_on_every_os_and_undo_removes_it() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os, None);
        locate(&mut s, 2 * BAR);
        tap(&mut s, os, "Shift+M");
        assert_eq!(
            markers(&s),
            vec![(2 * BAR, "Marker 1".to_string())],
            "{os:?}"
        );
        // Again on the same spot: nothing new.
        tap(&mut s, os, "Shift+M");
        assert_eq!(markers(&s).len(), 1);
        locate(&mut s, 3 * BAR);
        tap(&mut s, os, "Shift+M");
        assert_eq!(markers(&s)[1], (3 * BAR, "Marker 2".to_string()));
        s.core_mut().undo();
        assert_eq!(markers(&s).len(), 1, "one undo step per marker");
        s.core_mut().undo();
        assert!(markers(&s).is_empty());
    }
}

#[test]
fn the_legacy_profile_keeps_its_keys_and_the_marker_commands_stay_in_the_menus() {
    let mut s = shell_for(Os::Windows, Some(Profile::Legacy));
    locate(&mut s, BAR);
    tap(&mut s, Os::Windows, "Shift+M");
    tap(&mut s, Os::Windows, ",");
    assert!(
        markers(&s).is_empty(),
        "the legacy profile binds no marker key"
    );
    s.execute(AppCommand::AddMarker);
    assert_eq!(markers(&s).len(), 1);
}

#[test]
fn comma_and_period_go_to_the_previous_and_next_marker() {
    let mut s = shell();
    for (i, t) in [BAR, 3 * BAR, 6 * BAR].into_iter().enumerate() {
        locate(&mut s, t);
        tap(&mut s, Os::Windows, "Shift+M");
        assert_eq!(markers(&s).len(), i + 1);
    }
    locate(&mut s, 0);
    tap(&mut s, Os::Windows, ".");
    assert_eq!(s.core().position(), BAR);
    tap(&mut s, Os::Windows, ".");
    assert_eq!(s.core().position(), 3 * BAR);
    tap(&mut s, Os::Windows, ",");
    assert_eq!(s.core().position(), BAR);
    tap(&mut s, Os::Windows, ",");
    assert_eq!(
        s.core().position(),
        BAR,
        "no marker before the first: it stays"
    );
    locate(&mut s, 7 * BAR);
    tap(&mut s, Os::Windows, ".");
    assert_eq!(
        s.core().position(),
        7 * BAR,
        "no marker after the last: it stays"
    );
}

#[test]
fn the_menu_commands_work_from_any_focus() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetLoop {
            range: (BAR, 5 * BAR),
            enabled: true,
        }))
        .unwrap();
    s.execute(AppCommand::AddRangeFromLoop);
    let r = &s.core().doc().song.ranges;
    assert_eq!(
        (r[0].start, r[0].end, r[0].name.as_str()),
        (BAR, 5 * BAR, "Range 1")
    );
    locate(&mut s, 2 * BAR);
    s.execute(AppCommand::AddMarker);
    assert_eq!(markers(&s), vec![(2 * BAR, "Marker 1".to_string())]);
    locate(&mut s, 0);
    s.execute(AppCommand::NextMarker);
    assert_eq!(s.core().position(), 2 * BAR);
    s.execute(AppCommand::PreviousMarker);
    assert_eq!(s.core().position(), 2 * BAR, "none before: it stays");
    // A marker edit is a document change, not an engine restart.
    assert!(s.core().is_dirty());
}

#[test]
fn the_end_marker_menu_command_sets_the_end_at_the_playhead_and_undoes() {
    let mut s = shell();
    let before = s.core().doc().song.end_tick();
    locate(&mut s, 3 * BAR + 7);
    s.execute(AppCommand::SetEndMarker);
    assert_eq!(s.core().doc().song.end_tick(), 3 * BAR + 7);
    s.core_mut().undo();
    assert_eq!(s.core().doc().song.end_tick(), before);
}

#[test]
fn the_menus_list_the_marker_items() {
    let s = shell();
    let menus = menus(&s.menu_state(), s.keymap());
    let labels = |title: &str| -> Vec<(&'static str, Option<String>)> {
        menus
            .iter()
            .find(|m| m.title == title)
            .unwrap()
            .items()
            .map(|i| (i.label, i.shortcut.clone()))
            .collect()
    };
    let transport = labels("Transport");
    for (label, shortcut) in [
        ("Add Marker", Some("Shift+M")),
        ("Add Range Marker from Loop", None),
        ("Set End Marker at Playhead", None),
        ("Previous Marker", Some(",")),
        ("Next Marker", Some(".")),
    ] {
        let found = transport.iter().find(|(l, _)| *l == label);
        assert_eq!(
            found.map(|f| f.1.as_deref()),
            Some(shortcut),
            "{label} in {transport:?}"
        );
    }
    let view = labels("View");
    assert!(view.iter().any(|(l, _)| *l == "Marker Strip"));
    assert!(view.iter().any(|(l, _)| *l == "Marker List"));
}

#[test]
fn typing_a_name_in_place_owns_the_keys_and_commits_with_enter() {
    let mut s = shell();
    locate(&mut s, BAR);
    s.execute(AppCommand::AddMarker);
    s.set_focus(Pane::Arrangement);
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Sequencer]));
    assert!(
        s.core_mut()
            .arrangement_mut()
            .begin_marker_rename(MarkSel::Marker(1))
    );
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Text, Context::Sequencer])
    );
    // Keys that are commands outside a text field are typed: Shift+M, M, the comma and Space.
    for _ in 0..8 {
        tap(&mut s, Os::Windows, "Backspace");
    }
    for chord in ["Shift+M", "i", "x", ",", "Space", "2"] {
        tap(&mut s, Os::Windows, chord);
    }
    assert_eq!(s.core().arrangement().marker_rename_text(), Some("mix, 2"));
    assert_eq!(markers(&s).len(), 1, "Shift+M did not add a marker");
    tap(&mut s, Os::Windows, "Enter");
    assert_eq!(markers(&s), vec![(BAR, "mix, 2".to_string())]);
    assert!(!s.core().arrangement().is_text_owner());
    s.core_mut().undo();
    assert_eq!(markers(&s)[0].1, "Marker 1", "the rename was one undo step");
}

#[test]
fn escape_cancels_the_edit_and_the_name_stays() {
    let mut s = shell();
    s.execute(AppCommand::AddMarker);
    s.set_focus(Pane::Arrangement);
    s.core_mut()
        .arrangement_mut()
        .begin_marker_rename(MarkSel::Marker(1));
    tap(&mut s, Os::Windows, "z");
    tap(&mut s, Os::Windows, "Escape");
    assert_eq!(markers(&s)[0].1, "Marker 1");
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Sequencer]));
}

#[test]
fn the_marker_context_menus_list_their_commands() {
    let items =
        |t: ContextTarget, selection: bool| -> Vec<(&'static str, Option<&'static str>, bool)> {
            let e = context_menus::MenuEnable {
                marker_selection: selection,
                ..context_menus::MenuEnable::default()
            };
            context_menus::specs(t, &e)
                .into_iter()
                .filter_map(|s| match s {
                    Spec::Item {
                        command,
                        arg,
                        enabled,
                        ..
                    } => Some((command, arg, enabled)),
                    Spec::Separator | Spec::Submenu { .. } => None,
                })
                .collect()
        };
    let marker = items(ContextTarget::Marker, false);
    assert_eq!(marker[0], ("sequencer.marker_rename", None, true));
    assert_eq!(
        marker.last(),
        Some(&("sequencer.marker_delete", None, true))
    );
    let colours: Vec<_> = marker
        .iter()
        .filter(|i| i.0 == "sequencer.marker_color")
        .map(|i| i.1.unwrap())
        .collect();
    assert_eq!(
        colours,
        ["red", "orange", "yellow", "green", "blue", "purple", "none"]
    );
    let range = items(ContextTarget::MarkerRange, false);
    assert_eq!(range[0].0, "sequencer.marker_loop_range");
    assert_eq!(range[1].0, "sequencer.marker_select_clips");
    assert!(range.iter().any(|i| i.0 == "sequencer.marker_rename"));
    assert_eq!(range.last().unwrap().0, "sequencer.marker_delete");
    assert_eq!(
        items(ContextTarget::MarkerStrip, false),
        vec![
            ("sequencer.marker_add_here", None, true),
            ("transport.add_range_from_loop", None, true),
            ("sequencer.marker_range_from_selection", None, false),
        ]
    );
    assert!(items(ContextTarget::MarkerStrip, true)[2].2);
    // Every item is a registry command with a valid argument.
    let registry = atomik_ui::keymap::Registry::bundled();
    for t in [
        ContextTarget::Marker,
        ContextTarget::MarkerRange,
        ContextTarget::MarkerStrip,
    ] {
        for (command, arg, _) in items(t, true) {
            assert!(registry.validate(command, arg), "{command} {arg:?}");
        }
    }
}

#[test]
fn a_right_click_on_a_marker_opens_its_menu_and_the_colour_item_recolours_it() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetMarkers(vec![marker(
            1, BAR, "Drop",
        )])))
        .unwrap();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetRanges(vec![RangeMarker {
            id: 1,
            start: 0,
            end: BAR,
            name: "Intro".into(),
            color: None,
        }])))
        .unwrap();
    s.set_focus(Pane::Arrangement);
    // The menu items run through the same path as the keys.
    s.core_mut()
        .arrangement_mut()
        .set_selected_marker(Some(MarkSel::Marker(1)));
    assert!(
        s.run_command("sequencer.marker_color", Some("blue"))
            .is_some()
    );
    assert_eq!(
        s.core().doc().song.markers[0].color,
        Some(atomik_ui::marker_edit::PALETTE[4].1)
    );
    s.core_mut()
        .arrangement_mut()
        .set_selected_marker(Some(MarkSel::Range(1)));
    assert!(s.run_command("sequencer.marker_loop_range", None).is_some());
    assert_eq!(s.core().doc().song.loop_range, (0, BAR));
    assert!(s.core().doc().song.loop_enabled);
    // Delete removes the selected range and is undone in one step.
    assert!(s.run_command("edit.delete", None).is_some());
    assert!(s.core().doc().song.ranges.is_empty());
    s.core_mut().undo();
    assert_eq!(s.core().doc().song.ranges.len(), 1);
}
