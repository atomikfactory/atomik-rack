//! Where a channel's panel goes in the rack: right under its source instrument, whichever way the
//! channel is made (the create chain, `AddChannel`), so the mix channel panel sits under the
//! instrument it mixes. A channel without a source (`AddMixChannel`) follows the last channel.

mod common;

use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, DeviceKey, Document, WorkflowCommand,
    new_project_doc,
};
use common::workflow::*;
use common::*;

fn create(index: usize, k: &str) -> Command {
    wf(WorkflowCommand::CreateInstrument {
        index,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key(k)),
        options: CreateOptions::default(),
    })
}

fn position(d: &Document, k: &DeviceKey) -> usize {
    d.doc()
        .devices
        .iter()
        .position(|x| &x.key == k)
        .unwrap_or_else(|| panic!("{k} is in the rack"))
}

fn channel_of(d: &Document, source: &str) -> DeviceKey {
    d.doc()
        .console
        .channel_of(&key(source))
        .expect("a channel")
        .channel
        .clone()
}

#[test]
fn a_created_instruments_channel_sits_right_under_it_wherever_it_is_created() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [create(usize::MAX, "first"), create(usize::MAX, "last")],
    );
    // An instrument created between the two.
    let at = position(&d, &key("last"));
    run(&mut d, &r, [create(at, "middle")]);
    for source in ["first", "middle", "last"] {
        assert_eq!(
            position(&d, &channel_of(&d, source)),
            position(&d, &key(source)) + 1,
            "{source}: its channel follows it"
        );
    }
    assert!(
        position(&d, &key("first")) < position(&d, &key("middle"))
            && position(&d, &key("middle")) < position(&d, &key("last"))
    );
}

#[test]
fn add_channel_puts_the_channel_right_under_its_source_even_with_other_channels_after_it() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    // Every instrument comes with a channel; drop two of them and make them again.
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, SYNTH, Some("b")),
            add(usize::MAX, SYNTH, Some("c")),
        ],
    );
    let (ca, cc) = (channel_of(&d, "a"), channel_of(&d, "c"));
    run(
        &mut d,
        &r,
        [
            Command::Console(ConsoleCommand::RemoveChannel { channel: cc }),
            Command::Console(ConsoleCommand::RemoveChannel { channel: ca }),
            Command::Console(ConsoleCommand::AddChannel { source: key("c") }),
            Command::Console(ConsoleCommand::AddChannel { source: key("a") }),
        ],
    );
    for source in ["a", "c"] {
        assert_eq!(
            position(&d, &channel_of(&d, source)),
            position(&d, &key(source)) + 1,
            "{source}"
        );
    }
}

#[test]
fn an_insert_effect_sits_right_under_its_channel_and_a_mix_channel_after_the_last_channel() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [create(usize::MAX, "synth")]);
    let channel = channel_of(&d, "synth");
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot: 0,
            effect: Some(atomik_model::InsertEffect::New {
                type_id: "atomik.reverb".into(),
            }),
        })],
    );
    let fx = atomik_model::insert_chain(d.doc(), &r, &channel)[0].clone();
    assert_eq!(position(&d, &fx), position(&d, &channel) + 1);
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::AddMixChannel {
            name: "Group".into(),
        })],
    );
    let mix = d.doc().console.channels.last().unwrap().channel.clone();
    assert!(
        position(&d, &mix) > position(&d, &channel),
        "a source-less channel comes after the last channel"
    );
}
