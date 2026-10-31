//! Player stacks (project format 31): `Track.notes_to`, `WorkflowCommand::CreatePlayer`, delete
//! bridging of note cables, and the file format of the entry.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document, PROJECT_VERSION,
    SongCommand, WorkflowCommand, empty_doc, load_project, save_project,
};
use common::audio::{audio_registry, with_audio_track};
use common::song::add_track;
use common::workflow::*;
use common::*;
use serde_json::Value;

const FILTER: &str = "atomik.note-filter";

fn doc_with_synth(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        reg,
        [add(0, SYNTH, Some("synth")), add_track("synth")],
    );
    d
}

fn player(type_id: &str, selected: &str) -> Command {
    wf(WorkflowCommand::CreatePlayer {
        type_id: type_id.into(),
        target: CreateTarget::Selected(key(selected)),
        options: CreateOptions::default(),
    })
}

/// The keys of the devices of `type_id`, in rack order.
fn of_type(d: &Document, type_id: &str) -> Vec<String> {
    d.doc()
        .devices
        .iter()
        .filter(|x| x.type_id == type_id)
        .map(|x| x.key.0.clone())
        .collect()
}

fn index(d: &Document, k: &str) -> usize {
    d.doc().devices.iter().position(|x| x.key.0 == k).unwrap()
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

/// `synth` with a track and three players stacked on it, the last created on top:
/// `top -> middle -> bottom -> synth`. Returns the keys (top, middle, bottom).
fn stacked(reg: &Registry) -> (Document, [String; 3]) {
    let mut d = doc_with_synth(reg);
    run(
        &mut d,
        reg,
        [
            player(FILTER, "synth"),
            player(ARP, "synth"),
            player("atomik.key-chord", "synth"),
        ],
    );
    let top = of_type(&d, "atomik.key-chord").remove(0);
    let middle = of_type(&d, ARP).remove(0);
    let bottom = of_type(&d, FILTER).remove(0);
    (d, [top, middle, bottom])
}

#[test]
fn create_player_on_an_instrument_cables_it_and_moves_the_track_entry() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    // A player already cabled in is the entry; the new one goes above it: `arp -> seq -> synth`.
    run(&mut d, &reg, [add(0, SEQ, Some("seq"))]);
    run(&mut d, &reg, [connect("seq:notes_out", "synth:notes_in")]);
    one_step(&mut d, &reg, player(ARP, "synth"));
    let arp = of_type(&d, ARP).remove(0);
    assert_eq!(index(&d, &arp) + 1, index(&d, "seq"));
    assert!(has_cable(&d, &format!("{arp}:notes_out"), "seq:notes_in"));
    assert!(has_cable(&d, "seq:notes_out", "synth:notes_in"));
    // The track stays the instrument's and enters at the new player.
    assert_eq!(entry(&d, "synth"), Some(arp));
    assert_eq!(d.doc().song.tracks.len(), 1);
}

#[test]
fn create_player_with_route_off_is_a_plain_add_after_the_selection() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    let command = wf(WorkflowCommand::CreatePlayer {
        type_id: ARP.into(),
        target: CreateTarget::Selected(key("synth")),
        options: CreateOptions {
            route: false,
            ..CreateOptions::default()
        },
    });
    one_step(&mut d, &reg, command);
    let arp = of_type(&d, ARP).remove(0);
    assert_eq!(index(&d, &arp), index(&d, "synth") + 1);
    assert!(d.doc().cables.is_empty());
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn create_player_refuses_a_device_that_is_not_a_player() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    rejected(&mut d, &reg, player(DELAY, "synth"));
}

#[test]
fn create_player_without_a_usable_selection_only_adds_it() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(&mut d, &reg, [add(1, DELAY, Some("delay"))]);
    for target in [CreateTarget::Nothing, CreateTarget::Selected(key("delay"))] {
        let command = wf(WorkflowCommand::CreatePlayer {
            type_id: ARP.into(),
            target,
            options: CreateOptions::default(),
        });
        d.apply(command, &reg).unwrap();
    }
    assert!(d.doc().cables.is_empty(), "nothing to route to");
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn second_player_goes_above_the_first() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(&mut d, &reg, [player(ARP, "synth")]);
    let first = of_type(&d, ARP).remove(0);
    one_step(&mut d, &reg, player(FILTER, "synth"));
    let second = of_type(&d, FILTER).remove(0);
    assert_eq!(index(&d, &second) + 1, index(&d, &first));
    assert_eq!(index(&d, &first) + 1, index(&d, "synth"));
    assert!(has_cable(
        &d,
        &format!("{second}:notes_out"),
        &format!("{first}:notes_in")
    ));
    assert!(has_cable(
        &d,
        &format!("{first}:notes_out"),
        "synth:notes_in"
    ));
    assert_eq!(entry(&d, "synth"), Some(second.clone()));
    // Selecting a player of the chain stacks on the same chain, above its head.
    one_step(&mut d, &reg, player("atomik.key-chord", &first));
    let third = of_type(&d, "atomik.key-chord").remove(0);
    assert_eq!(index(&d, &third) + 1, index(&d, &second));
    assert!(has_cable(
        &d,
        &format!("{third}:notes_out"),
        &format!("{second}:notes_in")
    ));
    assert_eq!(entry(&d, "synth"), Some(third));
}

#[test]
fn second_player_on_an_instrument_without_a_track_still_goes_above_the_first() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &reg, [add(0, SYNTH, Some("synth"))]);
    run(
        &mut d,
        &reg,
        [player(ARP, "synth"), player(FILTER, "synth")],
    );
    let (arp, filter) = (of_type(&d, ARP).remove(0), of_type(&d, FILTER).remove(0));
    assert!(has_cable(
        &d,
        &format!("{filter}:notes_out"),
        &format!("{arp}:notes_in")
    ));
    assert!(has_cable(&d, &format!("{arp}:notes_out"), "synth:notes_in"));
}

#[test]
fn a_player_is_created_in_the_column_of_the_chain_head() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &reg,
        [
            add(0, DELAY, Some("delay")),
            add(1, SYNTH, Some("synth")),
            add_track("synth"),
            wf(WorkflowCommand::MoveDevicesToSlot {
                keys: keys_of(&["synth"]),
                slot: atomik_model::RackSlot {
                    column: 1,
                    index: 0,
                },
            }),
        ],
    );
    assert_eq!(d.doc().devices[1].column, 1);
    run(&mut d, &reg, [player(ARP, "synth")]);
    let arp = of_type(&d, ARP).remove(0);
    let (a, s) = (index(&d, &arp), index(&d, "synth"));
    assert_eq!(a + 1, s);
    assert_eq!(d.doc().devices[a].column, 1, "the column of the instrument");
    assert_eq!(d.doc().devices[0].column, 0);
}

#[test]
fn deleting_a_middle_player_bridges_the_chain() {
    let reg = registry();
    let (mut d, [top, middle, bottom]) = stacked(&reg);
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&[&middle]),
        }),
    );
    assert!(has_cable(
        &d,
        &format!("{top}:notes_out"),
        &format!("{bottom}:notes_in")
    ));
    assert!(has_cable(
        &d,
        &format!("{bottom}:notes_out"),
        "synth:notes_in"
    ));
    assert_eq!(entry(&d, "synth"), Some(top.clone()));
    // Deleting the head moves the entry down to the next device.
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&[&top]),
        }),
    );
    assert_eq!(entry(&d, "synth"), Some(bottom.clone()));
    // Deleting the last player hands the entry back to the instrument.
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&[&bottom]),
        }),
    );
    assert_eq!(entry(&d, "synth"), None);
    assert!(d.doc().cables.is_empty());
}

#[test]
fn deleting_a_player_bridges_the_cable_it_took_over() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(&mut d, &reg, [add(0, SEQ, Some("seq"))]);
    run(&mut d, &reg, [connect("seq:notes_out", "synth:notes_in")]);
    run(&mut d, &reg, [player(ARP, "synth")]);
    let arp = of_type(&d, ARP).remove(0);
    d.apply(
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&[&arp]),
        }),
        &reg,
    )
    .unwrap();
    assert!(has_cable(&d, "seq:notes_out", "synth:notes_in"));
    assert_eq!(d.doc().cables.len(), 1);
}

#[test]
fn removing_a_player_without_bridging_still_moves_the_entry_down() {
    let reg = registry();
    let (mut d, [top, middle, _]) = stacked(&reg);
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&[&top]),
        }),
    );
    assert_eq!(entry(&d, "synth"), Some(middle.clone()));
    one_step(&mut d, &reg, Command::RemoveDevice { key: key(&middle) });
    assert_eq!(
        entry(&d, "synth").as_deref(),
        Some(of_type(&d, FILTER)[0].as_str())
    );
}

#[test]
fn undo_restores_cables_and_entry() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(&mut d, &reg, [add(0, SEQ, Some("seq"))]);
    run(&mut d, &reg, [connect("seq:notes_out", "synth:notes_in")]);
    let before = text(&d);
    d.apply(player(ARP, "synth"), &reg).unwrap();
    assert_ne!(text(&d), before);
    d.undo(&reg).unwrap();
    assert_eq!(
        text(&d),
        before,
        "one undo restores cables, devices and entry"
    );
    assert!(d.doc().devices.iter().all(|x| x.type_id != ARP));
    assert_eq!(entry(&d, "synth").as_deref(), Some("seq"));
    d.redo(&reg).unwrap();
    assert!(entry(&d, "synth").is_some());
}

#[test]
fn pasting_a_track_keeps_its_entry_only_when_the_player_is_copied_too() {
    let reg = registry();
    let (mut d, [top, middle, bottom]) = stacked(&reg);
    let copy = |d: &mut Document, devices: &[&str]| {
        let snapshot =
            atomik_model::DeviceSnapshot::capture(d.doc(), &keys_of(devices), true).unwrap();
        d.apply(
            wf(WorkflowCommand::PasteDevices {
                snapshot,
                container: None,
                at: 0,
            }),
            &reg,
        )
        .unwrap();
    };
    // The instrument alone: its copy enters at itself.
    copy(&mut d, &["synth"]);
    let copies: Vec<&atomik_model::Track> = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.device.0 != "synth")
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].notes_to, None);
    // With the whole stack: the copy enters at the copy of the head.
    copy(&mut d, &["synth", &top, &middle, &bottom]);
    let new_head = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.device.0 != "synth" && t.notes_to.is_some())
        .map(|t| t.notes_to.clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(new_head.len(), 1);
    assert_ne!(new_head[0].0, top, "the copy of the head, not the original");
    assert!(d.doc().device(&new_head[0]).is_some());
}

#[test]
fn notes_entry_falls_back_to_the_track_device_when_the_target_is_gone() {
    let reg = registry();
    let (d, [top, ..]) = stacked(&reg);
    let doc = d.doc();
    let track = doc.song.track(&key("synth")).unwrap();
    assert_eq!(doc.notes_entry(track).0, top);
    let mut stale = track.clone();
    stale.notes_to = Some(key("no-such-device"));
    assert_eq!(doc.notes_entry(&stale), &key("synth"));
}

// ------------------------------------------------------------------------------- the file

#[test]
fn notes_to_round_trips_and_old_projects_have_none() {
    let reg = registry();
    let (d, [top, ..]) = stacked(&reg);
    let text = save_project(d.doc());
    assert!(text.contains(&format!("\"notes_to\": \"{top}\"")), "{text}");
    assert_eq!(text.matches("\"notes_to\"").count(), 1);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song, d.doc().song);
    assert_eq!(save_project(&loaded), text);
    // A project without a stack saves without the key, apart from the version line.
    let plain = doc_with_synth(&reg);
    let new = save_project(plain.doc());
    assert!(!new.contains("notes_to"));
    let v: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(v["version"], 37);
    assert_eq!(PROJECT_VERSION, 37);
    let old = new.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 30",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert!(loaded.song.tracks.iter().all(|t| t.notes_to.is_none()));
    assert_eq!(save_project(&loaded), new);
}

#[test]
fn load_project_notes_to_before_version_31_is_an_unknown_field() {
    let reg = registry();
    let (d, _) = stacked(&reg);
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 30",
        1,
    );
    let e = load_project(&text, &reg).expect_err("refused").to_string();
    assert!(e.contains("notes_to"), "{e}");
}

#[test]
fn load_project_refuses_a_notes_to_of_the_wrong_shape() {
    let reg = registry();
    let (d, _) = stacked(&reg);
    let text = save_project(d.doc());
    let with = |value: Value| {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["song"]["tracks"][0]["notes_to"] = value;
        serde_json::to_string_pretty(&v).unwrap()
    };
    for bad in [Value::from(3), Value::from(""), Value::from("synth")] {
        let e = load_project(&with(bad.clone()), &reg).expect_err("refused");
        assert!(e.to_string().contains("notes_to"), "{bad}: {e}");
    }
}

#[test]
fn load_project_keeps_a_notes_to_that_names_a_missing_device() {
    let reg = registry();
    let (d, [top, ..]) = stacked(&reg);
    let text = save_project(d.doc()).replace(
        &format!("\"notes_to\": \"{top}\""),
        "\"notes_to\": \"gone\"",
    );
    let loaded = load_project(&text, &reg).expect("the reference is not checked");
    let track = loaded.song.track(&key("synth")).unwrap();
    assert_eq!(track.notes_to, Some(DeviceKey("gone".into())));
    assert_eq!(loaded.notes_entry(track), &key("synth"));
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn the_player_shape_is_exactly_the_player_types() {
    // `CreatePlayer` and delete bridging recognise a player by its sockets (the model does not
    // know the devices crate): the shape must hold for every player type and for nothing else.
    let reg = registry();
    for type_id in reg.type_ids() {
        let desc = reg.descriptor(type_id).unwrap();
        assert_eq!(
            atomik_model::is_note_player(desc),
            atomik_devices::is_player(type_id),
            "{type_id}"
        );
    }
}

// ---------------------------------------------------------------- tracks and hand cabling

const CHORD: &str = "atomik.key-chord";

/// `synth` with two players cabled by hand and no track: `chord -> arp -> synth`.
fn hand_stacked(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        reg,
        [
            add(0, SYNTH, Some("synth")),
            add(1, ARP, Some("arp")),
            add(2, CHORD, Some("chord")),
            connect("chord:notes_out", "arp:notes_in"),
            connect("arp:notes_out", "synth:notes_in"),
        ],
    );
    d
}

/// `hand_stacked` with the synth's track.
fn hand_stacked_with_track(reg: &Registry) -> Document {
    let mut d = hand_stacked(reg);
    run(&mut d, reg, [add_track("synth")]);
    d
}

#[test]
fn add_track_on_an_instrument_fed_by_players_enters_at_the_top() {
    let reg = registry();
    let mut d = hand_stacked(&reg);
    one_step(&mut d, &reg, add_track("synth"));
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
}

#[test]
fn add_track_on_an_instrument_without_players_has_no_entry() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &reg,
        [add(0, SYNTH, Some("synth")), add_track("synth")],
    );
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn add_automation_lane_that_creates_the_track_enters_at_the_top() {
    let reg = registry();
    let mut d = hand_stacked(&reg);
    one_step(
        &mut d,
        &reg,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
    );
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
}

#[test]
fn create_instrument_gives_its_new_track_no_entry() {
    let reg = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &reg,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        })],
    );
    assert!(d.doc().song.track(&key("synth")).is_some());
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn auto_route_creating_a_track_for_a_stacked_instrument_enters_at_the_top() {
    let reg = console_registry();
    let mut d = hand_stacked(&reg);
    assert!(d.doc().song.track(&key("synth")).is_none());
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::AutoRoute {
            devices: keys_of(&["synth"]),
            options: CreateOptions::default(),
        }),
    );
    assert!(d.doc().song.track(&key("synth")).unwrap().armed);
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
}

#[test]
fn connect_a_player_into_a_tracks_instrument_moves_the_entry_up() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(&mut d, &reg, [add(1, ARP, Some("arp"))]);
    one_step(&mut d, &reg, connect("arp:notes_out", "synth:notes_in"));
    assert_eq!(entry(&d, "synth").as_deref(), Some("arp"));
    d.undo(&reg).unwrap();
    assert_eq!(entry(&d, "synth"), None, "undo restores the old entry");
}

#[test]
fn connect_a_second_player_above_the_first_moves_the_entry_again() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(
        &mut d,
        &reg,
        [
            add(1, ARP, Some("arp")),
            add(2, CHORD, Some("chord")),
            connect("arp:notes_out", "synth:notes_in"),
        ],
    );
    assert_eq!(entry(&d, "synth").as_deref(), Some("arp"));
    one_step(&mut d, &reg, connect("chord:notes_out", "arp:notes_in"));
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
    d.undo(&reg).unwrap();
    assert_eq!(entry(&d, "synth").as_deref(), Some("arp"));
}

#[test]
fn connect_a_chain_into_a_tracks_instrument_enters_at_the_top_of_the_chain() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(
        &mut d,
        &reg,
        [
            add(1, ARP, Some("arp")),
            add(2, CHORD, Some("chord")),
            connect("chord:notes_out", "arp:notes_in"),
        ],
    );
    assert_eq!(entry(&d, "synth"), None);
    one_step(&mut d, &reg, connect("arp:notes_out", "synth:notes_in"));
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
}

#[test]
fn disconnect_between_the_top_player_and_the_rest_moves_the_entry_back_down() {
    let reg = registry();
    let mut d = hand_stacked_with_track(&reg);
    assert_eq!(entry(&d, "synth").as_deref(), Some("chord"));
    one_step(
        &mut d,
        &reg,
        Command::Disconnect {
            to: sock("arp:notes_in"),
        },
    );
    assert_eq!(
        entry(&d, "synth").as_deref(),
        Some("arp"),
        "the entry is what still feeds the instrument"
    );
}

#[test]
fn disconnect_the_last_cable_into_the_instrument_clears_the_entry() {
    let reg = registry();
    let mut d = hand_stacked_with_track(&reg);
    one_step(
        &mut d,
        &reg,
        Command::Disconnect {
            to: sock("synth:notes_in"),
        },
    );
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn disconnect_devices_that_cuts_the_entry_off_moves_it_down() {
    let reg = registry();
    let mut d = hand_stacked_with_track(&reg);
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::DisconnectDevices {
            keys: keys_of(&["arp"]),
        }),
    );
    assert_eq!(entry(&d, "synth"), None);
}

#[test]
fn connect_a_player_into_a_device_that_is_nobody_s_entry_keeps_the_entries() {
    let reg = registry();
    let mut d = doc_with_synth(&reg);
    run(
        &mut d,
        &reg,
        [add(1, SAMPLER, Some("sampler")), add(2, ARP, Some("arp"))],
    );
    one_step(&mut d, &reg, connect("arp:notes_out", "sampler:notes_in"));
    assert_eq!(entry(&d, "synth"), None);
    assert!(d.doc().song.track(&key("sampler")).is_none());
}

#[test]
fn connect_above_the_top_of_a_stack_moves_the_entry_to_the_new_top() {
    let reg = registry();
    let mut d = hand_stacked_with_track(&reg);
    run(&mut d, &reg, [add(3, FILTER, Some("filter"))]);
    run(
        &mut d,
        &reg,
        [connect("filter:notes_out", "chord:notes_in")],
    );
    assert_eq!(entry(&d, "synth").as_deref(), Some("filter"));
}

#[test]
fn connecting_players_leaves_an_audio_track_alone() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    run(
        &mut d,
        &reg,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add(0, ARP, Some("arp")),
            connect("arp:notes_out", "synth:notes_in"),
        ],
    );
    assert_eq!(entry(&d, "synth").as_deref(), Some("arp"));
    assert_eq!(entry(&d, "audio-track"), None);
}
