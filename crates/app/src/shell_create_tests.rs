//! The auto-routing creation chain end to end: key
//! scripts through the router and a headless `Shell`, the OS a parameter. Ctrl+I, a name and
//! Enter make a device, its channel and an armed track in one undo step; Ctrl+E with a channel
//! selected fills its next insert; the Workflow setting and Shift and Alt change what is made.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey, insert_chain};
use atomik_ui::PointerButton;
use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::script::event_for;

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";

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
    s.set_focus(Pane::Rack);
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
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

fn type_name(s: &mut Shell<NullBackend>, os: Os, name: &str) {
    for c in name.chars() {
        tap(s, os, &c.to_ascii_uppercase().to_string());
    }
}

/// Create key, a filter, and the confirming chord.
fn create(s: &mut Shell<NullBackend>, os: Os, open: &str, name: &str, confirm: &str) {
    tap(s, os, open);
    type_name(s, os, name);
    tap(s, os, confirm);
}

fn saved(s: &Shell<NullBackend>) -> String {
    atomik_model::save_project(s.core().doc())
}

fn armed(s: &Shell<NullBackend>) -> Vec<String> {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.armed)
        .map(|t| t.device.0.clone())
        .collect()
}

fn plate(s: &Shell<NullBackend>, k: &str) -> Point {
    let p = s.core().view().plate_center(&key(k)).unwrap();
    Point::new(p.x, p.y + s.layout().rack.y0)
}

fn click(s: &mut Shell<NullBackend>, p: Point, button: PointerButton) {
    for pressed in [true, false] {
        let e = if pressed {
            UiEvent::PointerDown {
                pos: p,
                button,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos: p,
                button,
                modifiers: Modifiers::NONE,
            }
        };
        s.handle(&e);
    }
}

#[test]
fn e13_3_one_key_makes_the_device_its_channel_and_an_armed_selected_track() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os);
        let start = saved(&s);
        create(&mut s, os, "Cmd+I", "poly", "Return");
        let doc = s.core().doc();
        let synth = doc
            .devices
            .iter()
            .find(|d| d.type_id == SYNTH)
            .unwrap_or_else(|| panic!("{os:?}: no synth"))
            .key
            .clone();
        assert!(doc.console.channel_of(&synth).is_some(), "{os:?}");
        assert!(doc.console.master.is_some(), "{os:?}");
        assert_eq!(armed(&s), vec![synth.0.clone()], "{os:?}");
        assert_eq!(s.core().view().selected(), Some(&synth), "{os:?}");
        assert_eq!(
            s.core().notes_target_device().map(|d| d.key.clone()),
            Some(synth.clone()),
            "{os:?}: the keyboard plays it at once"
        );
        let created = saved(&s);
        tap(&mut s, os, "Cmd+Z");
        assert_eq!(saved(&s), start, "{os:?}: one undo removes everything");
        tap(&mut s, os, "Cmd+Y");
        assert_eq!(saved(&s), created, "{os:?}: redo brings it back identical");
    }
}

#[test]
fn ctrl_e_with_a_channel_selected_fills_the_next_free_insert_slot() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    create(&mut s, os, "Cmd+I", "poly", "Return");
    let synth = s.core().view().selected().unwrap().clone();
    let channel = s
        .core()
        .doc()
        .console
        .channel_of(&synth)
        .unwrap()
        .channel
        .clone();
    s.core_mut().select(Some(channel.clone()));
    let chain =
        |s: &Shell<NullBackend>| insert_chain(s.core().doc(), s.core().registry(), &channel);
    create(&mut s, os, "Cmd+E", "delay", "Return");
    assert_eq!(chain(&s).len(), 1);
    assert_eq!(s.core().view().selected(), chain(&s).first());
    // The effect is selected now; with the channel selected again the next slot is used.
    s.core_mut().select(Some(channel.clone()));
    create(&mut s, os, "Cmd+E", "reverb", "Return");
    assert_eq!(chain(&s).len(), 2);
    let before = saved(&s);
    tap(&mut s, os, "Cmd+Z");
    assert_eq!(chain(&s).len(), 1, "one undo step per effect");
    tap(&mut s, os, "Cmd+Y");
    assert_eq!(saved(&s), before);
}

#[test]
fn a_second_instrument_takes_the_keyboard_and_disarms_the_first() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    create(&mut s, os, "Cmd+I", "poly", "Return");
    let first = s.core().view().selected().unwrap().clone();
    create(&mut s, os, "Cmd+I", "poly", "Return");
    let second = s.core().view().selected().unwrap().clone();
    assert_ne!(first, second);
    assert_eq!(armed(&s), vec![second.0.clone()]);
    assert_eq!(
        s.core().notes_target_device().map(|d| d.key.clone()),
        Some(second)
    );
}

#[test]
fn the_workflow_setting_turns_auto_routing_off_and_shift_and_alt_change_one_device() {
    let os = Os::Windows;
    // Shift inverts the setting for one device: with auto-route on it adds the device only.
    let mut s = shell_for(os);
    create(&mut s, os, "Cmd+I", "poly", "Shift+Return");
    assert!(s.core().doc().song.tracks.is_empty(), "Shift: no routing");
    assert_eq!(
        s.core().doc().devices.len(),
        3,
        "the console rule still adds the channel (and the master)"
    );
    // Alt skips the track only.
    let mut s = shell_for(os);
    create(&mut s, os, "Cmd+I", "poly", "Alt+Return");
    assert!(s.core().doc().song.tracks.is_empty());
    assert_eq!(s.core().doc().console.channels.len(), 1);
    // The setting off: the plain action adds only, Shift routes.
    let mut s = shell_for(os);
    s.workflow.auto_route = false;
    s.apply_workflow_to_views();
    create(&mut s, os, "Cmd+I", "poly", "Return");
    assert!(s.core().doc().song.tracks.is_empty(), "the setting is off");
    assert_eq!(s.core().view().selected(), None);
    create(&mut s, os, "Cmd+I", "poly", "Shift+Return");
    assert_eq!(
        s.core().doc().song.tracks.len(),
        1,
        "Shift routes with the setting off"
    );
}

#[test]
fn the_menu_item_auto_route_routes_the_selected_instrument() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    // An instrument added the old way: a channel, no track.
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: Some(key("lead")),
        })
        .unwrap();
    s.core_mut().after_view_change();
    let at = plate(&s, "lead");
    click(&mut s, at, PointerButton::Secondary);
    let enabled = s
        .context_menu()
        .unwrap()
        .items()
        .find(|i| i.command == "rack.auto_route")
        .unwrap()
        .enabled;
    assert!(enabled, "a selected device can be auto-routed");
    let item = s
        .context_menu()
        .unwrap()
        .item_center("rack.auto_route")
        .unwrap();
    let before = saved(&s);
    click(&mut s, item, PointerButton::Primary);
    assert_eq!(armed(&s), vec!["lead".to_string()]);
    tap(&mut s, os, "Cmd+Z");
    assert_eq!(saved(&s), before, "one undo step");
}

#[test]
fn deleting_in_the_rack_bridges_unless_auto_routing_is_off() {
    let os = Os::Windows;
    for auto in [true, false] {
        let mut s = shell_for(os);
        for (i, k) in ["d1", "d2", "d3"].into_iter().enumerate() {
            s.core_mut()
                .apply_command(Command::AddDevice {
                    index: i,
                    type_id: DELAY.into(),
                    key: Some(key(k)),
                })
                .unwrap();
        }
        for (a, b) in [("d1", "d2"), ("d2", "d3")] {
            for side in ["l", "r"] {
                s.core_mut()
                    .apply_command(Command::Connect(atomik_model::Cable {
                        from: atomik_model::SocketRef {
                            device: key(a),
                            socket: format!("out_{side}"),
                        },
                        to: atomik_model::SocketRef {
                            device: key(b),
                            socket: format!("in_{side}"),
                        },
                    }))
                    .unwrap();
            }
        }
        s.workflow.auto_route = auto;
        s.apply_workflow_to_views();
        s.core_mut().after_view_change();
        let at = plate(&s, "d2");
        click(&mut s, at, PointerButton::Primary);
        tap(&mut s, os, "Delete");
        let bridged = s
            .core()
            .doc()
            .cables
            .iter()
            .any(|c| c.from.device == key("d1") && c.to.device == key("d3"));
        assert_eq!(bridged, auto, "auto-route {auto}");
    }
}

#[test]
fn ctrl_e_in_the_shell_opens_the_effect_browser_and_enter_creates() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    tap(&mut s, os, "Cmd+E");
    assert_eq!(s.core().view().browser_create(), Some(CreateKind::Effect));
    type_name(&mut s, os, "delay");
    let before = saved(&s);
    tap(&mut s, os, "Return");
    assert_ne!(saved(&s), before);
}

#[test]
fn e13_3_the_whole_key_sequence_on_every_os() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os);
        let start = saved(&s);
        // Cmd+I, a name, Enter: device, channel, track, armed, selected.
        create(&mut s, os, "Cmd+I", "poly", "Return");
        let synth = s.core().view().selected().unwrap().clone();
        let channel = s
            .core()
            .doc()
            .console
            .channel_of(&synth)
            .unwrap_or_else(|| panic!("{os:?}: channel"))
            .channel
            .clone();
        // Cmd+E and a reverb with the instrument (or its channel) selected: next free insert.
        create(&mut s, os, "Cmd+E", "reverb", "Return");
        let chain =
            |s: &Shell<NullBackend>| insert_chain(s.core().doc(), s.core().registry(), &channel);
        assert_eq!(chain(&s).len(), 1, "{os:?}: the reverb is in the insert");
        // Cmd+T adds an audio track, one undo step, and does not touch the arming.
        let before_track = saved(&s);
        let tracks = s.core().doc().song.tracks.len();
        tap(&mut s, os, "Cmd+T");
        assert_eq!(s.core().doc().song.tracks.len(), tracks + 1, "{os:?}");
        assert_eq!(armed(&s), vec![synth.0.clone()], "{os:?}");
        tap(&mut s, os, "Cmd+Z");
        assert_eq!(saved(&s), before_track, "{os:?}: undo of the track only");
        tap(&mut s, os, "Cmd+Z");
        assert_eq!(chain(&s).len(), 0, "{os:?}: undo of the effect only");
        tap(&mut s, os, "Cmd+Z");
        assert_eq!(
            saved(&s),
            start,
            "{os:?}: three undo steps back to the start"
        );
        // The setting off: the create key now adds the device only.
        s.workflow.auto_route = false;
        s.apply_workflow_to_views();
        create(&mut s, os, "Cmd+I", "poly", "Return");
        assert!(s.core().doc().song.tracks.is_empty(), "{os:?}");
        assert!(armed(&s).is_empty(), "{os:?}");
    }
}

#[test]
fn the_context_menu_item_use_as_send_effect_takes_the_first_free_return() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    for (k, ty) in [("lead", SYNTH), ("verb", "atomik.reverb")] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: ty.into(),
                key: Some(key(k)),
            })
            .unwrap();
    }
    // Keep the loose effect inside the pane (the console strips would push it down).
    s.core_mut().view_mut().set_show_console_devices(false);
    s.core_mut().after_view_change();
    let at = plate(&s, "verb");
    click(&mut s, at, PointerButton::Secondary);
    let enabled = |s: &Shell<crate::session::NullBackend>| {
        s.context_menu()
            .unwrap()
            .items()
            .find(|i| i.command == "rack.assign_send")
            .map(|i| i.enabled)
    };
    assert_eq!(
        enabled(&s),
        Some(true),
        "an unrouted effect can be a send effect"
    );
    let item = s
        .context_menu()
        .unwrap()
        .item_center("rack.assign_send")
        .unwrap();
    let before = saved(&s);
    click(&mut s, item, PointerButton::Primary);
    assert_eq!(s.core().doc().console.returns[0], Some(key("verb")));
    tap(&mut s, os, "Cmd+Z");
    assert_eq!(saved(&s), before, "one undo step");
    tap(&mut s, os, "Cmd+Shift+Z");
    assert_eq!(s.core().doc().console.returns[0], Some(key("verb")));

    // Routed already: the item is listed but disabled.
    let at = plate(&s, "verb");
    click(&mut s, at, PointerButton::Secondary);
    assert_eq!(enabled(&s), Some(false), "already on a return");
}

#[test]
fn e15_3_ctrl_e_with_the_master_selected_patches_the_effect_into_the_first_free_send() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os);
        create(&mut s, os, "Cmd+I", "poly", "Return");
        let master = s.core().doc().console.master.clone().expect("master");
        let returns = |s: &Shell<NullBackend>| s.core().doc().console.returns.clone();
        // Reverb: slot 1; the send entry of the dialog names it.
        s.core_mut().select(Some(master.clone()));
        tap(&mut s, os, "Cmd+E");
        let doc = s.core().doc().clone();
        assert_eq!(
            s.core().view().browser_send_entry(&doc),
            Some(atomik_ui::browser::SendEntry::Slot(0)),
            "{os:?}"
        );
        type_name(&mut s, os, "reverb");
        let before = saved(&s);
        tap(&mut s, os, "Return");
        let first = returns(&s)[0]
            .clone()
            .unwrap_or_else(|| panic!("{os:?}: slot 1"));
        assert_ne!(saved(&s), before, "{os:?}");
        // Delay: slot 2.
        s.core_mut().select(Some(master.clone()));
        create(&mut s, os, "Cmd+E", "delay", "Return");
        let r = returns(&s);
        assert_eq!(r[0].as_ref(), Some(&first), "{os:?}: slot 1 kept");
        assert!(r[1].is_some(), "{os:?}: slot 2");
        // One undo step removes the last effect and its cables.
        let two = saved(&s);
        tap(&mut s, os, "Cmd+Z");
        assert!(returns(&s)[1].is_none(), "{os:?}");
        assert!(returns(&s)[0].is_some(), "{os:?}");
        tap(&mut s, os, "Cmd+Y");
        assert_eq!(saved(&s), two, "{os:?}: redo is byte-identical");
    }
}

#[test]
fn the_ninth_send_effect_is_added_unrouted_and_the_dialog_greys_the_entry() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    create(&mut s, os, "Cmd+I", "poly", "Return");
    let master = s.core().doc().console.master.clone().expect("master");
    for _ in 0..8 {
        s.core_mut().select(Some(master.clone()));
        create(&mut s, os, "Cmd+E", "reverb", "Return");
    }
    assert!(s.core().doc().console.returns.iter().all(Option::is_some));
    s.core_mut().select(Some(master.clone()));
    tap(&mut s, os, "Cmd+E");
    let doc = s.core().doc().clone();
    assert_eq!(
        s.core().view().browser_send_entry(&doc),
        Some(atomik_ui::browser::SendEntry::Full)
    );
    type_name(&mut s, os, "reverb");
    let devices = s.core().doc().devices.len();
    tap(&mut s, os, "Return");
    assert_eq!(s.core().doc().devices.len(), devices + 1, "added");
    assert_eq!(
        s.core().doc().console.returns.iter().flatten().count(),
        8,
        "but not routed"
    );
}

// ------------------------------------------------------------------ starter project and status text

#[test]
fn e15_3_on_a_new_project_the_master_is_there_from_the_start_and_the_status_names_the_slot() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os);
        s.confirmed(crate::commands::AppCommand::New);
        let master = s
            .core()
            .doc()
            .console
            .master
            .clone()
            .expect("the starter master");
        s.core_mut().select(Some(master.clone()));
        create(&mut s, os, "Cmd+E", "reverb", "Return");
        assert_eq!(
            s.core().last_rejection(),
            Some("Effect routed to send/return 1"),
            "{os:?}"
        );
        s.core_mut().select(Some(master.clone()));
        create(&mut s, os, "Cmd+E", "delay", "Return");
        assert_eq!(
            s.core().last_rejection(),
            Some("Effect routed to send/return 2"),
            "{os:?}"
        );
        // Ctrl+Z takes the delay and its cables away again in one step.
        tap(&mut s, os, "Cmd+Z");
        assert!(s.core().doc().console.returns[1].is_none(), "{os:?}");
        // With a channel selected the effect still goes to its insert slot.
        create(&mut s, os, "Cmd+I", "poly", "Return");
        create(&mut s, os, "Cmd+E", "delay", "Return");
        assert!(
            s.core()
                .last_rejection()
                .is_some_and(|m| m.contains("insert 1")),
            "{os:?}: {:?}",
            s.core().last_rejection()
        );
    }
}

#[test]
fn the_ninth_effect_says_all_eight_slots_are_in_use() {
    let os = Os::Windows;
    let mut s = shell_for(os);
    s.confirmed(crate::commands::AppCommand::New);
    let master = s.core().doc().console.master.clone().expect("master");
    for _ in 0..8 {
        s.core_mut().select(Some(master.clone()));
        create(&mut s, os, "Cmd+E", "reverb", "Return");
    }
    s.core_mut().select(Some(master.clone()));
    create(&mut s, os, "Cmd+E", "reverb", "Return");
    assert_eq!(
        s.core().last_rejection(),
        Some("All 8 send and return slots are in use")
    );
}
