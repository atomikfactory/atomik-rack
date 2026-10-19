//! The master lock: the console master cannot be deleted, copied or
//! moved, and nothing goes above it while it is the first device. The lock lives in the
//! commands only: old and foreign documents still load, and a rejected command changes nothing.

mod common;

use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceSnapshot, Document,
    ModelError, WorkflowCommand, empty_doc, load_project, new_project_doc, save_project,
};
use common::plugin::*;
use common::workflow::*;
use common::*;

fn reg() -> atomik_device_sdk::Registry {
    plugin_console_registry()
}

/// The starter with a synth (and its channel), a delay and a mixer after the master.
fn project(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        r,
        [
            add(usize::MAX, SYNTH, Some("synth")),
            add(usize::MAX, DELAY, Some("delay")),
            add(usize::MAX, MIXER, Some("mixer")),
        ],
    );
    d
}

/// A document without a master: the master is created with the first channel, at the end.
fn old_style(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [add(0, DELAY, Some("delay")), add(1, SYNTH, Some("synth"))],
    );
    assert_eq!(keys(&d).last().unwrap(), "master", "the master is last");
    d
}

fn is_fixed(e: &ModelError) -> bool {
    matches!(e, ModelError::Fixed { what: "master", .. })
}

fn order(d: &Document) -> Vec<String> {
    keys(d)
}

// ------------------------------------------------------------------------------ delete

#[test]
fn remove_device_of_the_master_is_rejected_with_a_message() {
    let r = reg();
    let mut d = project(&r);
    let e = rejected(&mut d, &r, Command::RemoveDevice { key: key("master") });
    assert!(is_fixed(&e), "{e:?}");
    assert_eq!(e.to_string(), "The Master Section cannot be deleted");
}

#[test]
fn every_removal_path_rejects_the_master() {
    let r = reg();
    let mut d = project(&r);
    let paths = [
        Command::RemoveDevice { key: key("master") },
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["master"]),
        }),
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["delay", "master"]),
        }),
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&["master"]),
        }),
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&["synth", "master", "mixer"]),
        }),
        Command::Console(ConsoleCommand::RemoveChannel {
            channel: key("master"),
        }),
    ];
    for c in paths {
        let e = rejected(&mut d, &r, c.clone());
        assert!(is_fixed(&e), "{c:?}: {e:?}");
        assert_eq!(e.to_string(), "The Master Section cannot be deleted");
    }
}

#[test]
fn removing_the_other_devices_still_works_and_keeps_the_master() {
    let r = reg();
    let mut d = project(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["delay", "mixer"]),
        }),
    );
    one_step(&mut d, &r, Command::RemoveDevice { key: key("synth") });
    assert_eq!(order(&d), ["master"]);
    assert_eq!(d.doc().console.master, Some(key("master")));
}

#[test]
fn a_cut_is_a_copy_and_a_removal_so_a_cut_master_is_rejected() {
    let r = reg();
    let mut d = project(&r);
    // Copy alone says the master cannot be copied.
    let e = DeviceSnapshot::capture(d.doc(), &keys_of(&["master"]), false).unwrap_err();
    assert!(is_fixed(&e), "{e:?}");
    assert_eq!(e.to_string(), "The Master Section cannot be copied");
    // The removal half rejects too, with a mixed selection as well.
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["master", "delay"]),
        }),
    );
    assert!(is_fixed(&e));
}

#[test]
fn an_old_style_document_loads_and_its_master_cannot_be_deleted() {
    let r = reg();
    let mut d = old_style(&r);
    let back = load_project(&save_project(d.doc()), &r).unwrap();
    assert_eq!(&back, d.doc(), "the lock is not a load error");
    let e = rejected(&mut d, &r, Command::RemoveDevice { key: key("master") });
    assert!(is_fixed(&e));
}

#[test]
fn a_document_without_a_master_gets_one_with_the_first_channel() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("synth"))]);
    assert_eq!(d.doc().console.master, Some(key("master")));
    assert_eq!(d.doc().console.channels.len(), 1);
}

#[test]
fn a_master_device_outside_the_console_is_an_ordinary_device() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, MASTER, Some("stray"))]);
    assert_eq!(d.doc().console.master, None);
    one_step(&mut d, &r, Command::RemoveDevice { key: key("stray") });
    assert!(d.doc().devices.is_empty());
}

// ------------------------------------------------------------------------------ copy and paste

#[test]
fn copy_skips_the_master_in_a_mixed_selection() {
    let r = reg();
    let d = project(&r);
    let s = DeviceSnapshot::capture(d.doc(), &keys_of(&["master", "delay"]), false).unwrap();
    let copied: Vec<&str> = s.devices.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(copied, ["delay"]);
}

#[test]
fn duplicating_the_master_is_rejected() {
    let r = reg();
    let mut d = project(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateDevices {
            keys: keys_of(&["master"]),
        }),
    );
    assert!(is_fixed(&e), "{e:?}");
}

#[test]
fn a_paste_that_would_duplicate_the_master_is_rejected() {
    let r = reg();
    let mut d = project(&r);
    let mut snapshot = DeviceSnapshot::capture(d.doc(), &keys_of(&["delay"]), false).unwrap();
    snapshot.devices.insert(0, d.doc().devices[0].clone());
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot,
            container: None,
            at: 3,
        }),
    );
    assert!(is_fixed(&e), "{e:?}");
    assert_eq!(e.to_string(), "The Master Section cannot be copied");
}

// ------------------------------------------------------------------------------ moves

#[test]
fn the_master_cannot_be_moved_while_it_is_first() {
    let r = reg();
    let mut d = project(&r);
    for to in [0, 1, 2, usize::MAX] {
        let e = rejected(
            &mut d,
            &r,
            Command::MoveDevice {
                key: key("master"),
                to,
            },
        );
        assert!(is_fixed(&e), "to {to}: {e:?}");
    }
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["master"]),
            to: 2,
        }),
    );
    assert!(is_fixed(&e));
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["master", "delay"]),
            to: 2,
        }),
    );
    assert!(is_fixed(&e));
    // A group that holds the master is refused for the master, whatever else it holds.
    assert_eq!(e.to_string(), "The Master Section cannot be moved");
    for to in [1, 2, 3] {
        let e = rejected(
            &mut d,
            &r,
            wf(WorkflowCommand::MoveDevices {
                keys: keys_of(&["delay", "master"]),
                to,
            }),
        );
        assert_eq!(
            e.to_string(),
            "The Master Section cannot be moved",
            "to {to}"
        );
    }
}

#[test]
fn nothing_can_be_moved_above_the_master() {
    let r = reg();
    let mut d = project(&r);
    let e = rejected(
        &mut d,
        &r,
        Command::MoveDevice {
            key: key("delay"),
            to: 0,
        },
    );
    assert!(is_fixed(&e));
    assert_eq!(
        e.to_string(),
        "Nothing can be placed above the Master Section"
    );
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["delay", "mixer"]),
            to: 0,
        }),
    );
    assert!(is_fixed(&e), "{e:?}");
}

#[test]
fn moves_below_the_master_work_as_before() {
    let r = reg();
    let mut d = project(&r);
    let before = order(&d);
    one_step(
        &mut d,
        &r,
        Command::MoveDevice {
            key: key("mixer"),
            to: 1,
        },
    );
    assert_eq!(order(&d)[0], "master");
    assert_eq!(order(&d)[1], "mixer");
    assert_ne!(order(&d), before);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["delay", "synth"]),
            to: 1,
        }),
    );
    assert_eq!(order(&d)[0], "master");
}

#[test]
fn an_old_style_master_may_move_and_things_may_go_to_the_top() {
    let r = reg();
    let mut d = old_style(&r);
    // The master is last: not the first device, so there is no position lock.
    one_step(
        &mut d,
        &r,
        Command::MoveDevice {
            key: key("synth"),
            to: 0,
        },
    );
    assert_eq!(order(&d)[0], "synth");
    one_step(
        &mut d,
        &r,
        Command::MoveDevice {
            key: key("master"),
            to: 0,
        },
    );
    assert_eq!(order(&d)[0], "master");
    // Now it is first, and the lock holds.
    let e = rejected(
        &mut d,
        &r,
        Command::MoveDevice {
            key: key("delay"),
            to: 0,
        },
    );
    assert!(is_fixed(&e));
}

// ------------------------------------------------------------------------------ adds clamp

#[test]
fn adding_at_the_top_puts_the_device_below_the_master() {
    let r = reg();
    let mut d = project(&r);
    one_step(&mut d, &r, add(0, DELAY, Some("top")));
    assert_eq!(order(&d)[0], "master");
    assert_eq!(order(&d)[1], "top");
}

#[test]
fn creating_at_the_top_puts_the_device_below_the_master() {
    let r = reg();
    let mut d = project(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateInstrument {
            index: 0,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    assert_eq!(order(&d)[0], "master");
    assert_eq!(order(&d)[1], "lead");
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Type(DELAY.into()),
            target: CreateTarget::Nothing,
            key: Some(key("echo")),
            options: CreateOptions {
                route: false,
                ..CreateOptions::default()
            },
        }),
    );
    assert_eq!(order(&d)[0], "master");
}

#[test]
fn a_plugin_added_at_the_top_goes_below_the_master() {
    let r = reg();
    let mut d = project(&r);
    one_step(
        &mut d,
        &r,
        add_plugin(0, &fx_ref(), &fx_layout(), Some("plug")),
    );
    assert_eq!(order(&d)[0], "master");
    assert_eq!(order(&d)[1], "plug");
}

#[test]
fn a_paste_at_the_top_goes_below_the_master() {
    let r = reg();
    let mut d = project(&r);
    let snapshot = DeviceSnapshot::capture(d.doc(), &keys_of(&["delay"]), false).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot,
            container: None,
            at: 0,
        }),
    );
    assert_eq!(order(&d)[0], "master");
    assert_eq!(order(&d)[1], "stereo-delay");
}

#[test]
fn adds_at_the_top_are_not_clamped_when_the_master_is_not_first() {
    let r = reg();
    let mut d = old_style(&r);
    one_step(&mut d, &r, add(0, MIXER, Some("top")));
    assert_eq!(order(&d)[0], "top");
}

#[test]
fn a_container_keeps_its_own_order() {
    // The lock is about the top level; inside a container index 0 is a valid place.
    let r = reg();
    let mut d = project(&r);
    run(&mut d, &r, [add(usize::MAX, CONTAINER, Some("box"))]);
    one_step(&mut d, &r, add_inner("box", 0, DELAY, "first"));
}

// ------------------------------------------------------------------------------ load

#[test]
fn foreign_documents_still_open() {
    let r = reg();
    // The master in the middle, the console without a master, and a master that is not first.
    let mut d = old_style(&r);
    run(&mut d, &r, [add(1, MIXER, Some("mid"))]);
    let text = save_project(d.doc());
    assert_eq!(load_project(&text, &r).unwrap(), *d.doc());
}
