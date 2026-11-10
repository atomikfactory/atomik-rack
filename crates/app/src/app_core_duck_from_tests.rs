//! Quick sidechain in the app core: one named undo step with the ducker selected and a toast that
//! names the knob, the repeat that only selects, the loop refusal in plain words, and the menu
//! data and gates.

use super::*;
use crate::commands::DuckGate;
use crate::sink::RecordingSink;
use atomik_model::duck_from::{duckers_of, pump_of};
use atomik_model::{ConsoleCommand, CreateOptions, CreateSource, OutputDest, WorkflowCommand};

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// A project with synth tracks `Kick`, `Bass` and `Pad`.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::new_project_doc(), None);
    for (k, name) in [("kick", "Kick"), ("bass", "Bass"), ("pad", "Pad")] {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        c.apply_command(Command::Rename {
            key: key(k),
            name: name.into(),
        })
        .unwrap();
    }
    c
}

fn select_track(c: &mut AppCore<RecordingSink>, k: &str) {
    c.view_mut().set_selection(&[], None);
    c.arrangement_mut().set_selected_track(Some(key(k)));
}

#[test]
fn duck_from_is_one_named_undo_step_that_selects_the_ducker_and_names_the_knob() {
    let mut c = core();
    select_track(&mut c, "bass");
    let before = c.doc().clone();
    assert!(c.duck_from(None, &key("kick"), false));
    assert_eq!(c.document().undo_label(), Some("Duck from Track"));
    let ducker = duckers_of(c.doc(), c.registry(), &key("bass"));
    assert_eq!(ducker.len(), 1);
    assert_eq!(ducker[0].source, Some(key("kick")));
    assert_eq!(c.view().selection(), vec![ducker[0].device.clone()]);
    assert_eq!(
        c.last_rejection(),
        Some("Bass ducks under Kick. Adjust: Threshold on the Compressor.")
    );
    assert_eq!(
        c.doc().device(&ducker[0].device).unwrap().name,
        "Duck from Kick"
    );

    c.undo();
    assert_eq!(c.doc().devices, before.devices);
    assert_eq!(c.doc().cables, before.cables);
    assert_eq!(c.doc().console, before.console);
}

#[test]
fn ducking_again_from_the_same_track_only_selects_the_device() {
    let mut c = core();
    select_track(&mut c, "bass");
    assert!(c.duck_from(None, &key("kick"), false));
    let ducker = duckers_of(c.doc(), c.registry(), &key("bass"))[0]
        .device
        .clone();
    c.view_mut().set_selection(&[], None);
    let steps = c.document().history_len();
    assert!(c.duck_from(None, &key("kick"), true));
    assert_eq!(
        c.document().history_len(),
        steps,
        "no second device, no new step"
    );
    assert_eq!(duckers_of(c.doc(), c.registry(), &key("bass")).len(), 1);
    assert_eq!(c.view().selection(), vec![ducker]);
    assert_eq!(
        c.last_rejection(),
        Some("Already ducking from Kick. Adjust: Threshold on the Compressor.")
    );
}

#[test]
fn a_loop_is_refused_with_a_plain_message_and_nothing_changes() {
    let mut c = core();
    c.apply_command(Command::Console(ConsoleCommand::CreateBus {
        name: "Drums".into(),
        dest: OutputDest::Master,
    }))
    .unwrap();
    let bus = c.doc().console.buses[0].bus.clone();
    let channel = c
        .doc()
        .console
        .channel_of(&key("bass"))
        .unwrap()
        .channel
        .clone();
    c.apply_command(Command::Console(ConsoleCommand::SetOutput {
        strip: channel,
        dest: OutputDest::Bus(bus.clone()),
    }))
    .unwrap();
    select_track(&mut c, "bass");
    let before = c.doc().clone();
    assert!(!c.duck_from(None, &bus, false));
    let said = c.last_rejection().unwrap();
    assert!(said.contains("loop") && !said.contains("->"), "{said}");
    assert_eq!(c.doc(), &before);
}

#[test]
fn no_track_selected_says_so() {
    let mut c = core();
    c.view_mut().set_selection(&[], None);
    assert_eq!(c.duck_target(), None);
    assert_eq!(c.duck_gate(), DuckGate::default());
    assert!(!c.duck_from(None, &key("kick"), false));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Select a track"))
    );
    assert!(!c.gentle_pump(None));
    assert!(!c.remove_ducking(None));
}

#[test]
fn a_menu_names_its_own_target_and_lists_the_other_tracks() {
    let mut c = core();
    // The rack's primary device is the target of a device menu, the selected track of a track menu.
    c.view_mut().set_selection(&[key("pad")], Some(&key("pad")));
    c.arrangement_mut().set_selected_track(Some(key("bass")));
    assert_eq!(c.duck_rack_target(), Some(key("pad")));
    assert_eq!(c.duck_arranged_target(), Some(key("bass")));
    assert_eq!(c.duck_target(), Some(key("pad")), "the rack wins");

    let menu = c.duck_menu(&key("bass"));
    assert_eq!(
        menu.sources
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Kick", "Pad"]
    );
    assert!(!menu.has_ducking && menu.can_pump);
    assert!(c.duck_from(Some(key("bass")), &key("pad"), false));
    let menu = c.duck_menu(&key("bass"));
    assert!(menu.has_ducking);
    assert_eq!(
        menu.sources
            .iter()
            .map(|s| (s.name.as_str(), s.current))
            .collect::<Vec<_>>(),
        [("Kick", false), ("Pad", true)]
    );
    // The track the menu was opened on, not the rack's, was ducked.
    assert_eq!(duckers_of(c.doc(), c.registry(), &key("pad")).len(), 0);
}

#[test]
fn a_device_that_is_not_a_track_has_no_menu() {
    let mut c = core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.compressor".into(),
        key: Some(key("comp")),
    })
    .unwrap();
    assert_eq!(c.duck_menu(&key("comp")), DuckMenu::default());
    c.view_mut()
        .set_selection(&[key("comp")], Some(&key("comp")));
    assert_eq!(c.duck_rack_target(), None);
}

#[test]
fn the_gate_follows_the_target_and_the_pump_and_remove_work() {
    let mut c = core();
    select_track(&mut c, "bass");
    assert_eq!(
        c.duck_gate(),
        DuckGate {
            can_duck: true,
            can_pump: true,
            has_ducking: false
        }
    );
    assert!(c.gentle_pump(None));
    assert_eq!(c.document().undo_label(), Some("Add Gentle Pump"));
    assert!(pump_of(c.doc(), c.registry(), &key("bass")).is_some());
    assert_eq!(
        c.last_rejection(),
        Some("Bass pumps gently on every beat. Adjust: Depth on the Sidechain Tool.")
    );
    assert_eq!(
        c.duck_gate(),
        DuckGate {
            can_duck: true,
            can_pump: false,
            has_ducking: true
        }
    );
    assert!(!c.gentle_pump(None), "only one");
    assert!(c.remove_ducking(None));
    assert_eq!(c.document().undo_label(), Some("Remove Ducking"));
    assert_eq!(c.last_rejection(), Some("Ducking removed from Bass."));
    assert!(duckers_of(c.doc(), c.registry(), &key("bass")).is_empty());
    assert!(!c.remove_ducking(None), "nothing left to remove");
}

#[test]
fn a_song_of_one_track_has_nothing_to_duck_from() {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("only")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    select_track(&mut c, "only");
    let gate = c.duck_gate();
    assert!(!gate.can_duck && gate.can_pump);
}
