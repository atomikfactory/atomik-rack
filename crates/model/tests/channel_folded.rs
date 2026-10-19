//! A channel made by the create chain is folded: the rack grows by
//! one slim strip per instrument. The flag is the existing `folded` of the device, written in
//! the same undo step as the instrument and its channel; the master is created unfolded. The
//! channel of a plain `AddDevice` (patch import, examples, scripts) is left as it was.

mod common;

use atomik_model::{
    Command, CreateOptions, CreateSource, Document, WorkflowCommand, new_project_doc, save_project,
};
use common::workflow::*;
use common::*;

fn create_synth(k: &str) -> Command {
    wf(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key(k)),
        options: CreateOptions::default(),
    })
}

fn folded(d: &Document, k: &atomik_model::DeviceKey) -> bool {
    d.doc().devices.iter().find(|d| &d.key == k).unwrap().folded
}

#[test]
fn a_created_instrument_gets_a_folded_channel_and_the_master_is_not_folded() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [create_synth("synth")]);
    let channel = d.doc().console.channels[0].channel.clone();
    assert!(folded(&d, &channel), "the channel strip starts folded");
    assert!(
        !folded(&d, d.doc().console.master.as_ref().unwrap()),
        "the master stays unfolded"
    );
    assert!(!folded(&d, &key("synth")), "the instrument is not folded");
    run(&mut d, &r, [create_synth("bass")]);
    let second = d.doc().console.channels[1].channel.clone();
    assert!(folded(&d, &second), "every created strip is folded");
}

#[test]
fn the_fold_is_part_of_the_creation_step() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    let start = save_project(d.doc());
    run(&mut d, &r, [create_synth("synth")]);
    let after = save_project(d.doc());
    assert!(d.undo(&r).is_some());
    assert_eq!(
        save_project(d.doc()),
        start,
        "one undo removes the instrument, its channel and the fold"
    );
    assert!(d.redo(&r).is_some());
    assert_eq!(save_project(d.doc()), after);
}

#[test]
fn a_plain_add_device_keeps_its_channel_unfolded() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [add(usize::MAX, SYNTH, Some("synth"))]);
    let channel = d.doc().console.channels[0].channel.clone();
    assert!(
        !folded(&d, &channel),
        "the console-rule channel is as before"
    );
}

#[test]
fn with_route_off_the_create_is_exactly_add_device_and_the_channel_is_unfolded() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    let no_route = wf(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key("synth")),
        options: CreateOptions {
            route: false,
            ..CreateOptions::default()
        },
    });
    run(&mut d, &r, [no_route]);
    let channels = &d.doc().console.channels;
    assert_eq!(
        channels.len(),
        1,
        "the console-rule channel is made either way"
    );
    assert!(!folded(&d, &channels[0].channel.clone()));
}
