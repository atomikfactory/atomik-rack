//! The mixer group chip on the device headers of the rack: a channel that is in a group and its
//! instrument wear the group's name, the chip follows renames, moves and deletes of the group,
//! and a rack without groups draws exactly as before.

mod support;

use atomik_model::{Command, ConsoleCommand, Document};
use atomik_ui::{DisplayList, RackView};
use support::starter::*;
use support::*;

fn unfolded() -> (Document, atomik_device_sdk::Registry) {
    let reg = registry();
    let mut d = two_instruments(&reg);
    for k in ["synth", "bass", "channel"] {
        run(
            &mut d,
            &reg,
            [Command::SetFolded {
                key: key(k),
                folded: false,
            }],
        );
    }
    (d, reg)
}

fn texts(v: &mut RackView, d: &Document, reg: &atomik_device_sdk::Registry) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl.texts().into_iter().map(str::to_owned).collect()
}

fn group(d: &mut Document, reg: &atomik_device_sdk::Registry, name: &str, members: &[&str]) {
    run(
        d,
        reg,
        [Command::Console(ConsoleCommand::CreateVca {
            name: name.into(),
            members: members.iter().map(|m| key(m)).collect(),
        })],
    );
}

#[test]
fn a_member_channel_and_its_instrument_wear_the_group_name() {
    let (mut d, reg) = unfolded();
    let mut v: RackView = view(d.doc(), &reg);
    let plain = paint_json(&mut v, d.doc(), &reg);
    assert!(!texts(&mut v, &d, &reg).iter().any(|t| t == "Rhythm"));
    group(&mut d, &reg, "Rhythm", &["channel"]);
    v.sync(d.doc(), &reg);
    let named = texts(&mut v, &d, &reg);
    assert_eq!(
        named.iter().filter(|t| *t == "Rhythm").count(),
        2,
        "the channel and its synth: {named:?}"
    );
    assert_ne!(paint_json(&mut v, d.doc(), &reg), plain);
}

#[test]
fn the_chip_follows_a_rename_and_goes_with_the_group() {
    let (mut d, reg) = unfolded();
    group(&mut d, &reg, "Rhythm", &["channel"]);
    let mut v: RackView = view(d.doc(), &reg);
    run(
        &mut d,
        &reg,
        [Command::Console(ConsoleCommand::RenameVca {
            vca: 1,
            name: "Drums".into(),
        })],
    );
    v.sync(d.doc(), &reg);
    let named = texts(&mut v, &d, &reg);
    assert!(named.iter().any(|t| t == "Drums") && !named.iter().any(|t| t == "Rhythm"));
    run(
        &mut d,
        &reg,
        [Command::Console(ConsoleCommand::DeleteVca { vca: 1 })],
    );
    v.sync(d.doc(), &reg);
    assert!(!texts(&mut v, &d, &reg).iter().any(|t| t == "Drums"));
}

#[test]
fn a_device_outside_every_group_has_no_chip() {
    let (mut d, reg) = unfolded();
    group(&mut d, &reg, "Rhythm", &["channel"]);
    let mut v: RackView = view(d.doc(), &reg);
    // The bass has its own channel, which is not in the group: one synth and one channel only.
    let named = texts(&mut v, &d, &reg);
    assert_eq!(named.iter().filter(|t| *t == "Rhythm").count(), 2);
}
