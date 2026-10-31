//! Moving, duplicating and pasting devices with `route`: an effect or a note player lands as if it
//! had been created at its new place (bridged where it was, routed where it goes), in one undo
//! step; without `route` (Shift) the cables stay.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, DeviceKey, DeviceSnapshot, Document,
    RackSlot, WorkflowCommand, empty_doc, insert_chain,
};
use common::plugin::*;
use common::workflow::*;
use common::*;

fn reg() -> Registry {
    plugin_console_registry()
}

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

/// Two synths `s1`, `s2`, each with channel and track.
fn two_synths(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    for k in ["s1", "s2"] {
        run(
            &mut d,
            r,
            [wf(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: typ(SYNTH),
                key: Some(key(k)),
                options: CreateOptions::default(),
            })],
        );
    }
    d
}

fn effect_on(t: &str, selected: &str) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: typ(t),
        target: CreateTarget::Selected(key(selected)),
        key: None,
        options: CreateOptions::default(),
    })
}

fn player_on(t: &str, selected: &str) -> Command {
    wf(WorkflowCommand::CreatePlayer {
        type_id: t.into(),
        target: CreateTarget::Selected(key(selected)),
        options: CreateOptions::default(),
    })
}

fn channel_of(d: &Document, source: &str) -> DeviceKey {
    d.doc()
        .console
        .channel_of(&key(source))
        .unwrap()
        .channel
        .clone()
}

fn chain(d: &Document, r: &Registry, source: &str) -> Vec<String> {
    insert_chain(d.doc(), r, &channel_of(d, source))
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn index(d: &Document, k: &str) -> usize {
    d.doc().devices.iter().position(|x| x.key.0 == k).unwrap()
}

/// The flat index among the other devices of the top level to put `moving` right after `after`.
fn to_after(d: &Document, moving: &str, after: &str) -> usize {
    let rest: Vec<&str> = d
        .doc()
        .devices
        .iter()
        .map(|x| x.key.0.as_str())
        .filter(|k| *k != moving)
        .collect();
    rest.iter().position(|k| *k == after).unwrap() + 1
}

fn move_to(moving: &str, to: usize, route: bool) -> Command {
    wf(WorkflowCommand::MoveDevices {
        keys: keys_of(&[moving]),
        to,
        route,
    })
}

fn entry(d: &Document, track: &str) -> Option<String> {
    d.doc()
        .song
        .track(&key(track))
        .unwrap()
        .notes_to
        .as_ref()
        .map(|k| k.0.clone())
}

fn has_cable(d: &Document, from: &str, to: &str) -> bool {
    d.doc().cables.contains(&cable(from, to))
}

/// The effect created on `synth`'s channel in a document of two synths, keyed `fx` by its type.
fn with_effect_on_s1(r: &Registry) -> (Document, String) {
    let mut d = two_synths(r);
    run(&mut d, r, [effect_on(DELAY, "s1")]);
    let fx = chain(&d, r, "s1").remove(0);
    (d, fx)
}

#[test]
fn an_insert_effect_moved_below_another_strip_joins_that_strips_chain_in_one_undo() {
    let r = reg();
    let (mut d, fx) = with_effect_on_s1(&r);
    let c2 = channel_of(&d, "s2").0;
    let to = to_after(&d, &fx, &c2);
    one_step(&mut d, &r, move_to(&fx, to, true));
    assert!(chain(&d, &r, "s1").is_empty(), "the old chain closed up");
    assert_eq!(chain(&d, &r, "s2"), vec![fx.clone()]);
    // The result is the document a hand-built twin has: the effect created at the new place.
    let mut twin = two_synths(&r);
    run(&mut twin, &r, [effect_on(DELAY, "s2")]);
    assert_eq!(text(&d), text(&twin));
}

#[test]
fn shift_keeps_the_cables_of_a_moved_effect() {
    let r = reg();
    let (mut d, fx) = with_effect_on_s1(&r);
    let c2 = channel_of(&d, "s2").0;
    let to = to_after(&d, &fx, &c2);
    let cables = d.doc().cables.clone();
    one_step(&mut d, &r, move_to(&fx, to, false));
    assert_eq!(d.doc().cables, cables);
    assert_eq!(chain(&d, &r, "s1"), vec![fx]);
}

#[test]
fn an_effect_moved_between_the_effects_of_a_chain_joins_the_chain_and_a_middle_one_is_bridged() {
    let r = reg();
    let mut d = two_synths(&r);
    run(
        &mut d,
        &r,
        [
            effect_on(DELAY, "s1"),
            effect_on("atomik.reverb", "s1"),
            effect_on(COMP, "s1"),
        ],
    );
    let before = chain(&d, &r, "s1");
    assert_eq!(before.len(), 3);
    // Take the middle one out of the chain by moving it to the other strip.
    let c2 = channel_of(&d, "s2").0;
    let to = to_after(&d, &before[1], &c2);
    one_step(&mut d, &r, move_to(&before[1], to, true));
    assert_eq!(
        chain(&d, &r, "s1"),
        vec![before[0].clone(), before[2].clone()]
    );
    assert_eq!(chain(&d, &r, "s2"), vec![before[1].clone()]);
}

#[test]
fn a_moved_effect_frees_its_send_return_slot() {
    let r = reg();
    let mut d = two_synths(&r);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateEffect {
            source: typ("atomik.reverb"),
            target: CreateTarget::Master,
            key: None,
            options: CreateOptions::default(),
        })],
    );
    let verb = d.doc().console.returns[0]
        .clone()
        .expect("on the first return");
    let c1 = channel_of(&d, "s1").0;
    let to = to_after(&d, &verb.0, &c1);
    one_step(&mut d, &r, move_to(&verb.0, to, true));
    assert!(d.doc().console.returns[0].is_none(), "the slot is free");
    assert_eq!(chain(&d, &r, "s1"), vec![verb.0]);
}

#[test]
fn an_effect_with_other_cables_is_only_moved() {
    let r = reg();
    let (mut d, _) = with_effect_on_s1(&r);
    // A hand-made cable on a socket the creation rules never use keeps the patch as it is.
    run(&mut d, &r, [add(usize::MAX, COMP, Some("comp"))]);
    run(&mut d, &r, [connect("s1:out_l", "comp:sc_in_l")]);
    let before = d.doc().cables.clone();
    let to = to_after(&d, "comp", &channel_of(&d, "s1").0);
    one_step(&mut d, &r, move_to("comp", to, true));
    assert_eq!(d.doc().cables, before);
}

#[test]
fn a_moved_player_is_re_cabled_and_the_track_entry_follows() {
    let r = reg();
    let mut d = two_synths(&r);
    run(&mut d, &r, [player_on(ARP, "s1")]);
    let arp = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == ARP)
        .unwrap()
        .key
        .0
        .clone();
    assert_eq!(entry(&d, "s1"), Some(arp.clone()));
    let rest: Vec<String> = d
        .doc()
        .devices
        .iter()
        .map(|x| x.key.0.clone())
        .filter(|k| *k != arp)
        .collect();
    let to = rest.iter().position(|k| k == "s2").unwrap();
    one_step(&mut d, &r, move_to(&arp, to, true));
    assert_eq!(
        entry(&d, "s1"),
        None,
        "the first track enters at its instrument again"
    );
    assert_eq!(entry(&d, "s2"), Some(arp.clone()));
    assert!(has_cable(&d, &format!("{arp}:notes_out"), "s2:notes_in"));
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| c.to.device.0 == "s1" && c.to.socket == "notes_in")
    );
    let mut twin = two_synths(&r);
    run(&mut twin, &r, [player_on(ARP, "s2")]);
    assert_eq!(text(&d), text(&twin));
}

#[test]
fn a_player_moved_with_shift_keeps_its_cables() {
    let r = reg();
    let mut d = two_synths(&r);
    run(&mut d, &r, [player_on(ARP, "s1")]);
    let arp = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == ARP)
        .unwrap()
        .key
        .0
        .clone();
    let cables = d.doc().cables.clone();
    let to = to_after(&d, &arp, "s2");
    one_step(&mut d, &r, move_to(&arp, to, false));
    assert_eq!(d.doc().cables, cables);
    assert_eq!(entry(&d, "s1"), Some(arp));
}

#[test]
fn pasting_an_effect_below_an_instrument_routes_it_to_the_channel() {
    let r = reg();
    let mut d = two_synths(&r);
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("spare"))]);
    let snapshot = DeviceSnapshot::capture(d.doc(), &keys_of(&["spare"]), false).unwrap();
    let at = index(&d, "s1") + 1;
    let paste = |route| {
        wf(WorkflowCommand::PasteDevices {
            snapshot: snapshot.clone(),
            container: None,
            at,
            route,
        })
    };
    let mut plain = d.clone();
    one_step(&mut plain, &r, paste(false));
    assert!(chain(&plain, &r, "s1").is_empty());
    one_step(&mut d, &r, paste(true));
    assert_eq!(chain(&d, &r, "s1").len(), 1);
}

#[test]
fn pasting_at_a_slot_and_duplicating_route_the_copy() {
    let r = reg();
    let (mut d, fx) = with_effect_on_s1(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateDevices {
            keys: keys_of(&[&fx]),
            route: true,
        }),
    );
    assert_eq!(
        chain(&d, &r, "s1").len(),
        2,
        "the copy follows the original in its chain"
    );
    let snapshot = DeviceSnapshot::capture(d.doc(), &keys_of(&[&fx]), false).unwrap();
    let at = d.doc().slot_of(&key("s2")).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevicesAt {
            snapshot,
            container: None,
            slot: RackSlot {
                column: at.column,
                index: at.index + 1,
            },
            route: true,
        }),
    );
    assert_eq!(chain(&d, &r, "s2").len(), 1);
    assert_eq!(chain(&d, &r, "s1").len(), 2);
}

#[test]
fn a_duplicate_without_route_is_added_only() {
    let r = reg();
    let (mut d, fx) = with_effect_on_s1(&r);
    let cables = d.doc().cables.clone();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateDevices {
            keys: keys_of(&[&fx]),
            route: false,
        }),
    );
    assert_eq!(d.doc().cables, cables);
}
