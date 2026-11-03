//! Device groups and the rack sorting modes: what a group holds, one column per track in the
//! per-track mode, groups that stay together in the auto-group mode, one undo step per edit,
//! and the optional `"rack_sort"` field of project format 42.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    RackSort, SongCommand, WorkflowCommand, load_project, new_project_doc, save_project,
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

fn synth(d: &mut Document, r: &Registry, k: &str) {
    run(
        d,
        r,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typ(SYNTH),
            key: Some(key(k)),
            options: CreateOptions::default(),
        })],
    );
}

fn effect_on(d: &mut Document, r: &Registry, t: &str, selected: &str) {
    run(
        d,
        r,
        [wf(WorkflowCommand::CreateEffect {
            source: typ(t),
            target: CreateTarget::Selected(key(selected)),
            key: None,
            options: CreateOptions::default(),
        })],
    );
}

fn player_on(d: &mut Document, r: &Registry, selected: &str) {
    run(
        d,
        r,
        [wf(WorkflowCommand::CreatePlayer {
            type_id: ARP.into(),
            target: CreateTarget::Selected(key(selected)),
            options: CreateOptions::default(),
        })],
    );
}

fn start() -> (Document, Registry) {
    (Document::new(new_project_doc()), reg())
}

fn names(keys: &[DeviceKey]) -> Vec<String> {
    keys.iter().map(|k| k.0.clone()).collect()
}

fn columns_used(d: &Document) -> Vec<u16> {
    let mut c: Vec<u16> = d.doc().devices.iter().map(|x| x.column).collect();
    c.dedup();
    c
}

#[test]
fn a_group_holds_the_instrument_its_player_its_insert_its_channel_and_the_track() {
    let (mut d, r) = start();
    synth(&mut d, &r, "s1");
    player_on(&mut d, &r, "s1");
    effect_on(&mut d, &r, DELAY, "s1");
    let doc = d.doc();
    let channel = doc.console.channel_of(&key("s1")).unwrap().channel.clone();
    let group = doc.device_group(&r, &key("s1")).unwrap();
    assert_eq!(group.anchor, key("s1"));
    assert_eq!(group.track, Some(key("s1")));
    assert!(group.members.contains(&channel));
    assert_eq!(group.members.len(), 4, "{:?}", names(&group.members));
    // Asking by any member gives the same group; the master is in none.
    assert_eq!(doc.device_group(&r, &channel).unwrap(), group);
    let master = doc.console.master.clone().unwrap();
    assert!(doc.device_group(&r, &master).is_none());
}

#[test]
fn an_audio_track_is_a_group_of_its_own() {
    let mut d = Document::new(new_project_doc());
    let r = common::audio::audio_registry();
    synth(&mut d, &r, "s1");
    run(
        &mut d,
        &r,
        [Command::Song(SongCommand::AddAudioTrack { name: None })],
    );
    let groups = d.doc().device_groups(&r);
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|g| g.track.is_some()));
    assert_ne!(groups[0].anchor, groups[1].anchor);
}

#[test]
fn two_instruments_sharing_an_effect_return_stay_separate_groups() {
    let (mut d, r) = start();
    synth(&mut d, &r, "s1");
    synth(&mut d, &r, "s2");
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateEffect {
            source: typ(DELAY),
            target: CreateTarget::Nothing,
            key: Some(key("fx")),
            options: CreateOptions::default(),
        })],
    );
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("fx")),
        })],
    );
    let groups = d.doc().device_groups(&r);
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|g| !g.members.contains(&key("fx"))));
    for g in &groups {
        assert!(
            g.members
                .iter()
                .all(|m| groups.iter().filter(|o| o.members.contains(m)).count() == 1)
        );
    }
}

#[test]
fn per_track_gives_every_track_a_column_and_creating_and_deleting_leave_none_empty() {
    let (mut d, r) = start();
    synth(&mut d, &r, "s1");
    synth(&mut d, &r, "s2");
    let steps = d.undo_depth();
    run(&mut d, &r, [Command::SetRackSort(RackSort::PerTrack)]);
    assert_eq!(d.undo_depth(), steps + 1, "sorting is one step");
    assert_eq!(d.undo_label(), Some("Sort Rack"));
    // The master is loose (column 0), then one column per track.
    assert_eq!(columns_used(&d), vec![0, 1, 2]);
    assert_eq!(d.doc().columns, 3);
    d.doc().check_columns().unwrap();
    // Creating a third track adds a column in the same undo step as the creation.
    let steps = d.undo_depth();
    synth(&mut d, &r, "s3");
    assert_eq!(d.undo_depth(), steps + 1);
    assert_eq!(columns_used(&d), vec![0, 1, 2, 3]);
    assert_eq!(d.doc().columns, 4);
    // Deleting a track closes its column.
    let steps = d.undo_depth();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["s2"]),
        })],
    );
    assert_eq!(d.undo_depth(), steps + 1);
    assert_eq!(columns_used(&d), vec![0, 1, 2]);
    assert_eq!(d.doc().columns, 3);
    d.doc().check_columns().unwrap();
    d.undo().unwrap();
    assert_eq!(d.doc().columns, 4);
    d.undo().unwrap();
    assert_eq!(d.doc().columns, 3);
}

#[test]
fn auto_group_keeps_a_group_together_and_a_group_move_keeps_the_relative_order() {
    let (mut d, r) = start();
    synth(&mut d, &r, "s1");
    synth(&mut d, &r, "s2");
    effect_on(&mut d, &r, DELAY, "s1");
    run(&mut d, &r, [Command::SetRackSort(RackSort::AutoGroup)]);
    let order =
        |d: &Document| -> Vec<String> { d.doc().devices.iter().map(|x| x.key.0.clone()).collect() };
    let group_runs = |d: &Document| {
        d.doc().device_groups(&r).iter().all(|g| {
            let at: Vec<usize> = g
                .members
                .iter()
                .map(|m| d.doc().devices.iter().position(|x| &x.key == m).unwrap())
                .collect();
            at.windows(2).all(|w| w[1] == w[0] + 1)
        })
    };
    assert!(group_runs(&d), "{:?}", order(&d));
    // Moving one device of the first group moves its whole group, in its own order.
    let before = d.doc().device_group(&r, &key("s1")).unwrap().members;
    let end = d.doc().devices.len();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["s1"]),
            to: end,
            route: false,
        })],
    );
    assert!(group_runs(&d), "{:?}", order(&d));
    let after = d.doc().device_group(&r, &key("s1")).unwrap().members;
    assert_eq!(before, after, "relative order inside the group");
    let first = d
        .doc()
        .devices
        .iter()
        .position(|x| x.key == after[0])
        .unwrap();
    let second_group = d.doc().device_group(&r, &key("s2")).unwrap();
    let s2 = d
        .doc()
        .devices
        .iter()
        .position(|x| x.key == second_group.members[0])
        .unwrap();
    assert!(first > s2, "the moved group is last");
}

#[test]
fn the_sort_mode_is_saved_only_when_set_and_an_old_project_loads_as_free() {
    let (mut d, r) = start();
    synth(&mut d, &r, "s1");
    let free = text(&d);
    assert!(!free.contains("rack_sort"), "free writes nothing");
    assert_eq!(load_project(&free, &r).unwrap().sort_mode, RackSort::Free);
    run(&mut d, &r, [Command::SetRackSort(RackSort::AutoGroup)]);
    let saved = text(&d);
    assert!(saved.contains("\"rack_sort\": \"auto_group\""), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.sort_mode, RackSort::AutoGroup);
    assert_eq!(save_project(&back), saved, "save, load, save");
    run(&mut d, &r, [Command::SetRackSort(RackSort::Free)]);
    d.undo().unwrap();
    assert_eq!(d.doc().sort_mode, RackSort::AutoGroup);
}

#[test]
fn sorting_to_the_current_mode_is_rejected_and_changes_nothing() {
    let (mut d, r) = start();
    rejected(&mut d, &r, Command::SetRackSort(RackSort::Free));
}

#[test]
fn every_sorted_edit_keeps_the_columns_valid() {
    let (mut d, r) = start();
    run(&mut d, &r, [Command::SetRackSort(RackSort::PerTrack)]);
    for k in ["a", "b", "c"] {
        synth(&mut d, &r, k);
        player_on(&mut d, &r, k);
        d.doc().check_columns().unwrap();
        d.doc().validate().unwrap();
    }
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["a"]),
            to: 100,
            route: true,
        })],
    );
    d.doc().check_columns().unwrap();
    run(&mut d, &r, [Command::SetRackSort(RackSort::AutoGroup)]);
    d.doc().check_columns().unwrap();
    for _ in 0..d.undo_depth() {
        d.undo().unwrap();
        d.doc().check_columns().unwrap();
    }
}
