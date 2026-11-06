//! Mixer snapshots, the third round of follow-ups: the output routing and the insert chain
//! members a snapshot keeps (recall, one undo step, drift, morph, compare, the project file) and
//! the snapshot lane (an automation lane whose points recall a snapshot).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Curve, DeviceKey, Document, InsertEffect, OutputDest,
    SNAPSHOT_LANE_KEY, SnapshotScope, SongCommand, insert_chain, load_project, output_dest,
    save_project,
};
use common::console_ops::assert_console_invariants;
use common::song::*;
use common::*;
use serde_json::Value;

/// The starter with four synths and an effect `comp` on the insert of `channel`.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = legacy_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        ["a", "b", "c", "d"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, COMP, Some("comp")),
            con(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot: 0,
                effect: Some(InsertEffect::Existing(key("comp"))),
            }),
        ],
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn store(name: &str) -> Command {
    con(ConsoleCommand::StoreSnapshot { name: name.into() })
}

fn recall(id: u32) -> Command {
    con(ConsoleCommand::RecallSnapshot {
        id,
        scope: SnapshotScope::ALL,
    })
}

fn recall_scoped(id: u32, scope: SnapshotScope) -> Command {
    con(ConsoleCommand::RecallSnapshot { id, scope })
}

fn dest(d: &Document, strip: &str) -> OutputDest {
    output_dest(d.doc(), &key(strip))
}

fn chain(d: &Document, r: &Registry, strip: &str) -> Vec<String> {
    insert_chain(d.doc(), r, &key(strip))
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn bus(name: &str) -> Command {
    con(ConsoleCommand::CreateBus {
        name: name.into(),
        dest: OutputDest::Master,
    })
}

fn bus_key(d: &Document, name: &str) -> DeviceKey {
    d.doc().console.bus_by_name(name).unwrap().bus.clone()
}

fn route(strip: &str, to: OutputDest) -> Command {
    con(ConsoleCommand::SetOutput {
        strip: key(strip),
        dest: to,
    })
}

fn set_insert(strip: &str, slot: usize, effect: &str) -> Command {
    con(ConsoleCommand::SetInsert {
        channel: key(strip),
        slot,
        effect: Some(InsertEffect::Existing(key(effect))),
    })
}

fn one_step(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    assert_console_invariants(&after);
    after.validate().expect("valid after");
    d.undo().expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo().expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

#[test]
fn a_snapshot_holds_the_output_and_the_insert_chain_of_each_strip() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let wiring = d.doc().console.snapshots[0]
        .state
        .wiring
        .clone()
        .expect("a new snapshot holds the wiring");
    assert_eq!(wiring[&key("channel")].inserts, vec![key("comp")]);
    assert_eq!(wiring[&key("channel")].output, Some(OutputDest::Master));
    assert!(wiring[&key("channel-2")].inserts.is_empty());
    assert!(wiring.contains_key(&key("master")));
    assert_eq!(wiring[&key("master")].output, None);
    assert!(wiring.contains_key(&bus_key(&d, "Drums")));
}

#[test]
fn recall_routes_the_strips_back_in_one_undo_step() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [
            route("channel", OutputDest::Bus(drums.clone())),
            route("channel-2", OutputDest::AudioIo(2)),
        ],
    );
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(drums));
    assert!(!d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    one_step(&mut d, &r, recall(1));
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn recall_restores_the_members_and_the_order_of_the_insert_chain() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, COMP, Some("comp2")),
            set_insert("channel", 1, "comp2"),
            store("A"),
        ],
    );
    assert_eq!(chain(&d, &r, "channel"), ["comp", "comp2"]);
    // Reordered, then one member taken off.
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::MoveInsert {
            channel: key("channel"),
            from: 0,
            to: 1,
        })],
    );
    assert_eq!(chain(&d, &r, "channel"), ["comp2", "comp"]);
    one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp", "comp2"]);
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::ClearInserts {
            strip: key("channel"),
        })],
    );
    assert!(chain(&d, &r, "channel").is_empty());
    one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp", "comp2"]);
}

#[test]
fn a_member_can_move_from_one_strip_to_another() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    // The effect leaves `channel` for `channel-2`.
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
            set_insert("channel-2", 0, "comp"),
            store("B"),
        ],
    );
    one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp"]);
    assert!(chain(&d, &r, "channel-2").is_empty());
    one_step(&mut d, &r, recall(2));
    assert!(chain(&d, &r, "channel").is_empty());
    assert_eq!(chain(&d, &r, "channel-2"), ["comp"]);
}

#[test]
fn a_snapshot_without_wiring_leaves_the_routing_and_the_chains_alone() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    // The file of a build that did not know the wiring has no such field.
    let text = save_project(d.doc());
    let mut json: Value = serde_json::from_str(&text).unwrap();
    for s in json["console"]["snapshots"].as_array_mut().unwrap() {
        s.as_object_mut().unwrap().remove("wiring").unwrap();
    }
    let mut old = Document::new(load_project(&json.to_string(), &r).unwrap());
    assert!(old.doc().console.snapshots[0].state.wiring.is_none());
    let drums = bus_key(&old, "Drums");
    run(
        &mut old,
        &r,
        [
            set_param("channel", "fader", 0.3),
            route("channel", OutputDest::Bus(drums.clone())),
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
        ],
    );
    run(&mut old, &r, [recall(1)]);
    assert_eq!(dest(&old, "channel"), OutputDest::Bus(drums));
    assert!(chain(&old, &r, "channel").is_empty());
    assert_ne!(
        old.doc().param_value(&key("channel"), "fader", &r).unwrap(),
        0.3,
        "the params still recall"
    );
    assert!(old.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn the_routing_scope_recalls_only_the_wiring() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.31),
            route("channel", OutputDest::Bus(drums)),
        ],
    );
    run(&mut d, &r, [recall_scoped(1, SnapshotScope::LEVELS)]);
    assert!(
        matches!(dest(&d, "channel"), OutputDest::Bus(_)),
        "levels only"
    );
    run(&mut d, &r, [set_param("channel", "fader", 0.31)]);
    run(&mut d, &r, [recall_scoped(1, SnapshotScope::ROUTING)]);
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    assert_eq!(
        d.doc().param_value(&key("channel"), "fader", &r).unwrap(),
        0.31,
        "the fader stays"
    );
    assert_eq!(
        SnapshotScope::from_name("routing"),
        Some(SnapshotScope::ROUTING)
    );
}

#[test]
fn what_is_gone_is_skipped_and_named_and_the_rest_recalls() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            bus("Drums"),
            set_param("channel-3", "fader", 0.7),
            store("A"),
        ],
    );
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [
            route("channel", OutputDest::Bus(drums.clone())),
            set_param("channel-3", "fader", 0.1),
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
            con(ConsoleCommand::DeleteBus { bus: drums }),
            Command::RemoveDevice { key: key("comp") },
        ],
    );
    let notes = d.doc().snapshot_notes(1);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("comp") && n.contains("insert")),
        "{notes:?}"
    );
    run(&mut d, &r, [recall(1)]);
    assert_eq!(
        dest(&d, "channel"),
        OutputDest::Master,
        "the bus took it back"
    );
    assert_eq!(
        d.doc().param_value(&key("channel-3"), "fader", &r).unwrap(),
        0.7
    );
    assert!(chain(&d, &r, "channel").is_empty());
}

#[test]
fn a_member_on_another_strips_chain_is_not_taken() {
    let (mut d, r) = project();
    // The snapshot holds `channel` and `channel-2` only (channel-3 does not exist yet).
    run(&mut d, &r, [store("A")]);
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
            add(usize::MAX, COMP, Some("other")),
            set_insert("channel-2", 0, "other"),
        ],
    );
    run(&mut d, &r, [recall(1)]);
    assert_eq!(chain(&d, &r, "channel"), ["comp"]);
    assert!(
        chain(&d, &r, "channel-2").is_empty(),
        "channel-2 held an empty chain"
    );
    d.doc().validate().unwrap();
}

#[test]
fn a_free_effect_with_a_cable_on_it_is_not_taken_for_a_chain() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, COMP, Some("loose")),
            set_insert("channel-2", 0, "loose"),
            store("A"),
            con(ConsoleCommand::ClearInserts {
                strip: key("channel-2"),
            }),
            // A hand cable on the effect's input: it is no longer free.
            connect("a:out_l", "loose:in_l"),
        ],
    );
    let before = d.doc().cables.clone();
    // Nothing is left to recall: the effect cannot join the chain.
    let e = d.apply(recall(1), &r).expect_err("nothing to recall");
    assert!(e.to_string().contains("already matches"), "{e}");
    assert!(chain(&d, &r, "channel-2").is_empty());
    assert_eq!(d.doc().cables, before, "its cables stay as they were");
}

#[test]
fn a_recall_that_would_make_a_loop_skips_that_route() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("One"), bus("Two")]);
    let (one, two) = (bus_key(&d, "One"), bus_key(&d, "Two"));
    // A: One plays into Two.
    run(
        &mut d,
        &r,
        [route(&one.0, OutputDest::Bus(two.clone())), store("A")],
    );
    // Now Two plays into One instead.
    run(
        &mut d,
        &r,
        [
            route(&one.0, OutputDest::Master),
            route(&two.0, OutputDest::Bus(one.clone())),
        ],
    );
    run(&mut d, &r, [recall(1)]);
    assert_console_invariants(d.doc());
    d.doc().validate().unwrap();
    // One of the two routes holds; no loop exists.
    assert_ne!(
        (dest(&d, &one.0), dest(&d, &two.0)),
        (OutputDest::Bus(two), OutputDest::Bus(one))
    );
}

#[test]
fn a_morph_flips_the_wiring_at_the_half_way_point() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [
            route("channel", OutputDest::Bus(drums.clone())),
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
            store("B"),
        ],
    );
    let morph = |at: f32| con(ConsoleCommand::MorphSnapshots { from: 1, to: 2, at });
    run(&mut d, &r, [morph(0.25)]);
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    assert_eq!(chain(&d, &r, "channel"), ["comp"]);
    run(&mut d, &r, [morph(0.75)]);
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(drums));
    assert!(chain(&d, &r, "channel").is_empty());
    run(&mut d, &r, [morph(0.0)]);
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    run(&mut d, &r, [morph(1.0)]);
    assert!(d.doc().snapshot_matches(2, &r, SnapshotScope::ALL));
}

#[test]
fn a_compare_flips_the_wiring_with_the_rest_of_the_mix() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [route("channel", OutputDest::Bus(drums.clone()))],
    );
    let working = d.doc().capture_mixer(&r);
    run(&mut d, &r, [con(ConsoleCommand::Compare { id: 1 })]);
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    run(&mut d, &r, [con(ConsoleCommand::Compare { id: 1 })]);
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(drums));
    assert_eq!(d.doc().capture_mixer(&r), working);
}

#[test]
fn updating_a_snapshot_takes_the_wiring_as_it_is() {
    let (mut d, r) = project();
    run(&mut d, &r, [bus("Drums"), store("A")]);
    let drums = bus_key(&d, "Drums");
    run(
        &mut d,
        &r,
        [
            route("channel", OutputDest::Bus(drums.clone())),
            con(ConsoleCommand::UpdateSnapshot { id: 1 }),
        ],
    );
    let wiring = d.doc().console.snapshots[0].state.wiring.clone().unwrap();
    assert_eq!(wiring[&key("channel")].output, Some(OutputDest::Bus(drums)));
}

#[test]
fn the_wiring_round_trips_through_the_project_file() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            bus("Drums"),
            route("channel-2", OutputDest::AudioIo(2)),
            store("A"),
        ],
    );
    let text = save_project(d.doc());
    assert!(text.contains("\"wiring\""));
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.console.snapshots, d.doc().console.snapshots);
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_malformed_wiring_is_refused_with_its_path() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    let text = save_project(d.doc());
    for (patch, needle) in [
        (
            r#"{"channel": {"output": "elsewhere", "inserts": []}}"#,
            "master, bus",
        ),
        (
            r#"{"channel": {"output": "io:0", "inserts": []}}"#,
            "Audio I/O",
        ),
        (r#"{"channel": {"output": "master"}}"#, "inserts"),
        (r#"{"channel": {"inserts": [3]}}"#, "wiring"),
        (r#"[1]"#, "expected an object"),
    ] {
        let mut json: Value = serde_json::from_str(&text).unwrap();
        json["console"]["snapshots"][0]["wiring"] = serde_json::from_str(patch).unwrap();
        let e = load_project(&json.to_string(), &r)
            .expect_err(patch)
            .to_string();
        assert!(e.contains("snapshots[0].wiring"), "{e}");
        assert!(e.contains(needle) || needle == "wiring", "{e}");
    }
}

// ----------------------------------------------------------------------------- the lane

fn master(d: &Document) -> DeviceKey {
    d.doc().console.master.clone().unwrap()
}

fn add_lane(d: &mut Document, r: &Registry) {
    let device = master(d);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device,
            param: SNAPSHOT_LANE_KEY.into(),
        }),
        r,
    )
    .unwrap();
}

fn put_points(d: &mut Document, r: &Registry, start: i64, pts: &[(i64, f32)]) {
    let device = master(d);
    let lane = d.doc().snapshot_lane().expect("the lane exists").1;
    d.apply(
        add_clip(
            &device.0,
            lane,
            auto_clip(
                start,
                4 * PPQ * 4,
                pts.iter()
                    .map(|(t, v)| point(*t, *v, Curve::Step))
                    .collect(),
            ),
        ),
        r,
    )
    .unwrap();
}

#[test]
fn the_snapshot_lane_is_a_lane_of_the_master_with_a_snapshot_number_range() {
    let (mut d, r) = project();
    assert!(d.doc().snapshot_lane().is_none());
    add_lane(&mut d, &r);
    let (m, lane) = d.doc().snapshot_lane().unwrap();
    assert_eq!(m, master(&d));
    assert_eq!(
        d.doc().song.track(&m).unwrap().lane_label(lane),
        "Snapshots"
    );
    d.doc().validate().unwrap();
    // One lane only, and only the master has it.
    let again = sc(SongCommand::AddAutomationLane {
        device: master(&d),
        param: SNAPSHOT_LANE_KEY.into(),
    });
    assert!(d.apply(again, &r).is_err());
    let on_synth = sc(SongCommand::AddAutomationLane {
        device: key("a"),
        param: SNAPSHOT_LANE_KEY.into(),
    });
    assert!(d.apply(on_synth, &r).is_err());
    let fixed = sc(SongCommand::SetLaneStatic {
        device: master(&d),
        param: SNAPSHOT_LANE_KEY.into(),
        value: Some(2.0),
        merge: None,
    });
    let e = d.apply(fixed, &r).expect_err("no static value");
    assert!(e.to_string().contains("static"), "{e}");
    // Points clamp to the snapshot numbers.
    put_points(&mut d, &r, 0, &[(0, 99.0), (PPQ, -3.0)]);
    let m = master(&d);
    let lane = d.doc().snapshot_lane().unwrap().1;
    let atomik_model::ClipContent::Automation(points) =
        &d.doc().song.track(&m).unwrap().lanes[lane].clips[0].content
    else {
        panic!("automation")
    };
    let values: Vec<f32> = points.iter().map(|p| p.value).collect();
    assert_eq!(values, vec![64.0, 1.0]);
}

#[test]
fn the_lane_gives_a_recall_for_each_point_whose_snapshot_changes() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [store("A"), set_param("channel", "fader", 0.4), store("B")],
    );
    assert!(
        d.doc().snapshot_lane_events().is_empty(),
        "no lane, no recall"
    );
    add_lane(&mut d, &r);
    assert!(d.doc().snapshot_lane_events().is_empty(), "no points");
    put_points(
        &mut d,
        &r,
        PPQ,
        &[
            (0, 1.0),
            (PPQ, 1.0),
            (2 * PPQ, 2.0),
            (3 * PPQ, 7.0),
            (3 * PPQ + 1, 2.0),
        ],
    );
    // The same snapshot twice recalls once; snapshot 7 does not exist and plays nothing; the
    // point after it names snapshot 2 again, which differs from 7.
    assert_eq!(
        d.doc().snapshot_lane_events(),
        vec![(PPQ, 1), (3 * PPQ, 2), (4 * PPQ + 1, 2)]
    );
}

#[test]
fn the_lane_follows_the_clips_it_is_drawn_in() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A"), store("B")]);
    add_lane(&mut d, &r);
    put_points(&mut d, &r, 0, &[(0, 1.0), (PPQ, 2.0)]);
    let id = clip_ids(d.doc())[0];
    assert_eq!(d.doc().snapshot_lane_events(), vec![(0, 1), (PPQ, 2)]);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 8 * PPQ,
            length: 16 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        d.doc().snapshot_lane_events(),
        vec![(8 * PPQ, 1), (9 * PPQ, 2)],
        "the recalls move with the clip"
    );
    // A muted lane recalls nothing.
    let (m, lane) = d.doc().snapshot_lane().unwrap();
    d.apply(
        sc(SongCommand::SetLaneMuted {
            device: m,
            lane,
            muted: true,
        }),
        &r,
    )
    .unwrap();
    assert!(d.doc().snapshot_lane_events().is_empty());
}

#[test]
fn the_lane_survives_save_and_load() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    add_lane(&mut d, &r);
    put_points(&mut d, &r, 0, &[(0, 1.0)]);
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.snapshot_lane_events(), vec![(0, 1)]);
    assert_eq!(save_project(&back), text);
}
