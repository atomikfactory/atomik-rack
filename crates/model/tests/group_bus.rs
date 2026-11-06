//! A mixer group routed to an output bus: its members play into the bus, a strip that joins or
//! leaves the group follows in the same undo step, and the optional `bus` and `routed` fields of
//! the project format. Plus the mute and solo lanes of a group.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, Command, ConsoleCommand, Curve, DeviceKey, Document, OutputDest, SongCommand,
    VcaLaneKind, load_project, output_dest, save_project, vca_lane_key_of, vca_lane_parts,
};
use common::console_ops::assert_console_invariants;
use common::song::*;
use common::*;

/// Four synths `a`..`d` with tracks (channels `channel`, `channel-2`, `channel-3`, `channel-4`)
/// and a group "Drums" (id 1) over the first two channels.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = legacy_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
        d.apply(add(i, SYNTH, Some(k)), &r).unwrap();
    }
    for k in ["a", "b", "c", "d"] {
        d.apply(add_track(k), &r).unwrap();
    }
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel"), key("channel-2")],
        }),
        &r,
    )
    .unwrap();
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn dest(d: &Document, channel: &str) -> OutputDest {
    output_dest(d.doc(), &key(channel))
}

fn route_new(d: &mut Document, r: &Registry, vca: u32, name: &str) {
    d.apply(
        con(ConsoleCommand::RouteVcaToNewBus {
            vca,
            name: name.into(),
        }),
        r,
    )
    .unwrap();
}

fn bus_of(d: &Document, name: &str) -> DeviceKey {
    d.doc().console.bus_by_name(name).unwrap().bus.clone()
}

fn assign(d: &mut Document, r: &Registry, vca: Option<u32>, strips: &[&str]) {
    d.apply(
        con(ConsoleCommand::AssignToVca {
            vca,
            strips: strips.iter().map(|k| key(k)).collect(),
        }),
        r,
    )
    .unwrap();
}

fn routed(d: &Document, vca: u32) -> Vec<String> {
    d.doc()
        .console
        .vca(vca)
        .unwrap()
        .routed
        .iter()
        .map(|m| m.0.clone())
        .collect()
}

#[test]
fn routing_a_group_to_a_new_bus_is_one_undo_step() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    route_new(&mut d, &r, 1, "Drum bus");
    let bus = bus_of(&d, "Drum bus");
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(bus.clone()));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Bus(bus.clone()));
    assert_eq!(dest(&d, "channel-3"), OutputDest::Master);
    assert_eq!(d.doc().console.vca(1).unwrap().bus, Some(bus));
    assert_eq!(routed(&d, 1), ["channel", "channel-2"]);
    assert_console_invariants(d.doc());
    d.doc().validate().unwrap();
    let after = d.doc().clone();
    d.undo().unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "the bus, the cables and the route go in one step"
    );
    d.redo().unwrap();
    assert_eq!(d.doc(), &after);
}

#[test]
fn a_group_can_play_into_a_bus_that_already_exists_and_moves_to_another() {
    let (mut d, r) = project();
    for name in ["Mix", "Other"] {
        d.apply(
            con(ConsoleCommand::CreateBus {
                name: name.into(),
                dest: OutputDest::Master,
            }),
            &r,
        )
        .unwrap();
    }
    let (mix, other) = (bus_of(&d, "Mix"), bus_of(&d, "Other"));
    let route = |bus: &DeviceKey| {
        con(ConsoleCommand::RouteVcaToBus {
            vca: 1,
            bus: bus.clone(),
        })
    };
    d.apply(route(&mix), &r).unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(mix.clone()));
    let e = d.apply(route(&mix), &r).unwrap_err().to_string();
    assert!(e.contains("already plays"), "{e}");
    d.apply(route(&other), &r).unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(other.clone()));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Bus(other.clone()));
    assert_eq!(d.doc().console.vca(1).unwrap().bus, Some(other));
    // Refused: a key that is no bus, a group that does not exist, an empty group.
    for command in [
        ConsoleCommand::RouteVcaToBus {
            vca: 1,
            bus: key("a"),
        },
        ConsoleCommand::RouteVcaToBus { vca: 9, bus: mix },
        ConsoleCommand::RouteVcaToNewBus {
            vca: 9,
            name: "Nothing".into(),
        },
        ConsoleCommand::UnrouteVca { vca: 9 },
    ] {
        assert!(d.apply(con(command), &r).is_err());
    }
    assert_console_invariants(d.doc());
}

#[test]
fn an_empty_group_cannot_be_routed_and_an_unrouted_one_cannot_be_unrouted() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Empty".into(),
            members: vec![],
        }),
        &r,
    )
    .unwrap();
    let e = d
        .apply(
            con(ConsoleCommand::RouteVcaToNewBus {
                vca: 2,
                name: "Nowhere".into(),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("no member"), "{e}");
    assert!(d.doc().console.bus_by_name("Nowhere").is_none());
    let e = d
        .apply(con(ConsoleCommand::UnrouteVca { vca: 1 }), &r)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not routed"), "{e}");
}

#[test]
fn a_strip_that_joins_or_leaves_the_group_follows_in_the_same_undo_step() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    let bus = bus_of(&d, "Drum bus");

    let before = d.doc().clone();
    assign(&mut d, &r, Some(1), &["channel-3"]);
    assert_eq!(dest(&d, "channel-3"), OutputDest::Bus(bus.clone()));
    assert_eq!(routed(&d, 1), ["channel", "channel-2", "channel-3"]);
    let after = d.doc().clone();
    d.undo().unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "the membership and the routing are one step"
    );
    d.redo().unwrap();
    assert_eq!(d.doc(), &after);

    assign(&mut d, &r, None, &["channel"]);
    assert_eq!(
        dest(&d, "channel"),
        OutputDest::Master,
        "a strip that leaves goes back to the master"
    );
    assert_eq!(routed(&d, 1), ["channel-2", "channel-3"]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_strip_moved_to_another_group_leaves_the_bus_of_the_first() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Keys".into(),
            members: vec![key("channel-3")],
        }),
        &r,
    )
    .unwrap();
    assign(&mut d, &r, Some(2), &["channel-2"]);
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
    assert_eq!(routed(&d, 1), ["channel"]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_strip_removed_from_the_console_leaves_the_route() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    d.apply(
        con(ConsoleCommand::RemoveChannel {
            channel: key("channel-2"),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(routed(&d, 1), ["channel"]);
    assert_console_invariants(d.doc());
    d.doc().validate().unwrap();
}

#[test]
fn an_output_chosen_by_hand_for_one_member_stays() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    d.apply(
        con(ConsoleCommand::SetOutput {
            strip: key("channel-2"),
            dest: OutputDest::AudioIo(2),
        }),
        &r,
    )
    .unwrap();
    // Later commands (a strip joining) do not pull it back.
    assign(&mut d, &r, Some(1), &["channel-3"]);
    assert_eq!(dest(&d, "channel-2"), OutputDest::AudioIo(2));
    assert_console_invariants(d.doc());
}

#[test]
fn stopping_the_route_sends_the_members_back_to_the_master() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    let routed_doc = d.doc().clone();
    d.apply(con(ConsoleCommand::UnrouteVca { vca: 1 }), &r)
        .unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
    let g = d.doc().console.vca(1).unwrap();
    assert!(g.bus.is_none() && g.routed.is_empty());
    assert!(
        d.doc().console.bus_by_name("Drum bus").is_some(),
        "the bus stays"
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &routed_doc);
    // A member the user had moved elsewhere is left where it is.
    d.apply(
        con(ConsoleCommand::SetOutput {
            strip: key("channel"),
            dest: OutputDest::AudioIo(2),
        }),
        &r,
    )
    .unwrap();
    d.apply(con(ConsoleCommand::UnrouteVca { vca: 1 }), &r)
        .unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::AudioIo(2));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
}

#[test]
fn deleting_the_bus_forgets_the_route_and_deleting_the_group_keeps_the_bus() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    let bus = bus_of(&d, "Drum bus");
    d.apply(con(ConsoleCommand::DeleteBus { bus }), &r).unwrap();
    let g = d.doc().console.vca(1).unwrap();
    assert!(g.bus.is_none() && g.routed.is_empty());
    assert_console_invariants(d.doc());
    d.doc().validate().unwrap();

    route_new(&mut d, &r, 1, "Again");
    d.apply(con(ConsoleCommand::DeleteVca { vca: 1 }), &r)
        .unwrap();
    assert!(d.doc().console.bus_by_name("Again").is_some());
    assert_console_invariants(d.doc());
}

#[test]
fn a_member_that_would_make_a_loop_stays_out_of_the_route() {
    let (mut d, r) = project();
    // The bus plays into a bus; the group holds the first bus and a channel.
    d.apply(
        con(ConsoleCommand::CreateBus {
            name: "Low".into(),
            dest: OutputDest::Master,
        }),
        &r,
    )
    .unwrap();
    d.apply(
        con(ConsoleCommand::CreateBus {
            name: "High".into(),
            dest: OutputDest::Master,
        }),
        &r,
    )
    .unwrap();
    let (low, high) = (bus_of(&d, "Low"), bus_of(&d, "High"));
    d.apply(
        con(ConsoleCommand::SetOutput {
            strip: low.clone(),
            dest: OutputDest::Bus(high.clone()),
        }),
        &r,
    )
    .unwrap();
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Buses".into(),
            members: vec![high.clone(), key("channel-3")],
        }),
        &r,
    )
    .unwrap();
    // High cannot play into Low (Low already plays into High); the channel can.
    d.apply(
        con(ConsoleCommand::RouteVcaToBus {
            vca: 2,
            bus: low.clone(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(dest(&d, "channel-3"), OutputDest::Bus(low));
    assert_eq!(output_dest(d.doc(), &high), OutputDest::Master);
    assert_eq!(routed(&d, 2), ["channel-3"]);
    assert_console_invariants(d.doc());
}

#[test]
fn the_route_is_saved_and_old_projects_load_unrouted() {
    let (mut d, r) = project();
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"routed\"") && !plain.contains("\"bus\": \""));
    route_new(&mut d, &r, 1, "Drum bus");
    let text = save_project(d.doc());
    assert!(text.contains("\"routed\""), "{text}");
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    // Saving the loaded project is byte for byte the same.
    assert_eq!(save_project(&back), text);
    let old = load_project(&plain, &r).unwrap();
    assert!(old.console.vca(1).unwrap().bus.is_none());
}

#[test]
fn a_project_with_a_bad_route_is_refused() {
    let (mut d, r) = project();
    route_new(&mut d, &r, 1, "Drum bus");
    let text = save_project(d.doc());
    let bus = bus_of(&d, "Drum bus");
    let bad_bus = text.replacen(
        &format!("\"bus\": \"{}\"", bus.0),
        "\"bus\": \"channel\"",
        1,
    );
    assert_ne!(bad_bus, text);
    assert!(load_project(&bad_bus, &r).is_err());
    let bad_member = text.replacen("\"routed\": [\"channel\"", "\"routed\": [\"channel-4\"", 1);
    assert_ne!(bad_member, text);
    assert!(load_project(&bad_member, &r).is_err());
}

// ---- Mute and solo lanes ----

fn master(d: &Document) -> DeviceKey {
    d.doc().console.master.clone().unwrap()
}

fn add_lane(
    d: &mut Document,
    r: &Registry,
    id: u32,
    kind: VcaLaneKind,
) -> Result<(), atomik_model::ModelError> {
    let device = master(d);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device,
            param: vca_lane_key_of(id, kind),
        }),
        r,
    )
    .map(|_| ())
}

fn with_points(d: &mut Document, r: &Registry, id: u32, kind: VcaLaneKind, pts: &[(i64, f32)]) {
    let device = master(d);
    let lane = d
        .doc()
        .song
        .track(&device)
        .unwrap()
        .automation_lane(&vca_lane_key_of(id, kind))
        .unwrap();
    d.apply(
        add_clip(
            &device.0,
            lane,
            auto_clip(
                0,
                15360 * 4,
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
fn the_keys_name_the_group_and_what_it_automates() {
    assert_eq!(vca_lane_key_of(3, VcaLaneKind::Level), "vca.3");
    assert_eq!(vca_lane_key_of(3, VcaLaneKind::Mute), "vca.3.mute");
    assert_eq!(vca_lane_key_of(3, VcaLaneKind::Solo), "vca.3.solo");
    for kind in VcaLaneKind::ALL {
        assert_eq!(vca_lane_parts(&vca_lane_key_of(12, kind)), Some((12, kind)));
    }
    for bad in [
        "vca.3.",
        "vca.3.level",
        "vca.03.mute",
        "vca.0.solo",
        "vca..mute",
        "vca.3.mute.x",
        "vca.3.Mute",
        "vca.mute",
    ] {
        assert_eq!(vca_lane_parts(bad), None, "{bad}");
    }
    assert_eq!(atomik_model::vca_lane_id("vca.3.mute"), None);
}

#[test]
fn a_mute_and_a_solo_lane_are_lanes_of_the_master_with_a_toggle_range() {
    let (mut d, r) = project();
    for kind in VcaLaneKind::ALL {
        add_lane(&mut d, &r, 1, kind).unwrap();
        assert!(add_lane(&mut d, &r, 1, kind).is_err(), "one lane per kind");
        assert!(add_lane(&mut d, &r, 2, kind).is_err(), "no group 2");
    }
    let m = master(&d);
    let track = d.doc().song.track(&m).unwrap();
    let labels: Vec<String> = (0..track.lanes.len())
        .map(|i| track.lane_label(i))
        .collect();
    assert_eq!(labels, ["Group 1 level", "Group 1 mute", "Group 1 solo"]);
    d.doc().validate().unwrap();
    // The values clamp to 0..1 and a synth has no group lane.
    with_points(&mut d, &r, 1, VcaLaneKind::Mute, &[(0, 7.0), (960, -2.0)]);
    let lane = d
        .doc()
        .song
        .track(&m)
        .unwrap()
        .automation_lane("vca.1.mute")
        .unwrap();
    let ClipContent::Automation(points) =
        &d.doc().song.track(&m).unwrap().lanes[lane].clips[0].content
    else {
        panic!("automation");
    };
    let values: Vec<f32> = points.iter().map(|p| p.value).collect();
    assert_eq!(values, vec![1.0, 0.0]);
    assert!(
        d.apply(
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: "vca.1.mute".into(),
            }),
            &r,
        )
        .is_err()
    );
}

#[test]
fn the_lanes_drive_the_members_flags_and_show_on_the_group() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1, VcaLaneKind::Mute).unwrap();
    add_lane(&mut d, &r, 1, VcaLaneKind::Solo).unwrap();
    with_points(&mut d, &r, 1, VcaLaneKind::Mute, &[(0, 0.0), (15360, 1.0)]);
    with_points(&mut d, &r, 1, VcaLaneKind::Solo, &[(0, 1.0), (15360, 0.0)]);
    let m = master(&d);
    let shown = d.doc().automation_values_at(20000.0);
    assert_eq!(shown[&(m.clone(), "vca.1.mute".to_owned())], 1.0);
    assert_eq!(shown[&(m.clone(), "vca.1.solo".to_owned())], 0.0);
    let view = d.doc().view_with_shown(&shown);
    let g = view.console.vca(1).unwrap();
    assert!(g.mute && !g.solo);
    // The stored flags are untouched.
    let g = d.doc().console.vca(1).unwrap();
    assert!(!g.mute && !g.solo);
    let targets = d.doc().vca_kind_targets(1, VcaLaneKind::Mute);
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|(_, p)| *p == "internal.vca_mute"));
    assert!(
        d.doc()
            .vca_kind_targets(1, VcaLaneKind::Solo)
            .iter()
            .all(|(_, p)| *p == "internal.vca_solo")
    );
    assert!(d.doc().vca_kind_targets(9, VcaLaneKind::Mute).is_empty());
}

#[test]
fn deleting_the_group_deletes_all_its_lanes_in_one_undo_step() {
    let (mut d, r) = project();
    for kind in VcaLaneKind::ALL {
        add_lane(&mut d, &r, 1, kind).unwrap();
    }
    with_points(&mut d, &r, 1, VcaLaneKind::Solo, &[(0, 1.0)]);
    let before = d.doc().clone();
    d.apply(con(ConsoleCommand::DeleteVca { vca: 1 }), &r)
        .unwrap();
    assert!(d.doc().song.track(&master(&d)).is_none());
    d.doc().validate().unwrap();
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_lanes_are_saved_and_a_lane_of_a_missing_group_is_refused() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1, VcaLaneKind::Mute).unwrap();
    add_lane(&mut d, &r, 1, VcaLaneKind::Solo).unwrap();
    with_points(&mut d, &r, 1, VcaLaneKind::Mute, &[(0, 1.0), (960, 0.0)]);
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    let broken = text.replace("vca.1.solo", "vca.7.solo");
    assert!(load_project(&broken, &r).is_err());
}
