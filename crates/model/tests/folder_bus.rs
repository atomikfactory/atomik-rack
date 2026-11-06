//! A track folder routed to an output bus: the channels of its tracks play into the bus, a track
//! that joins or leaves the folder follows in the same undo step, and the optional
//! `folder_routes` field of the project format.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, OutputDest, SongCommand, TrackPlace, load_project,
    output_dest, save_project,
};
use common::console_ops::assert_console_invariants;
use common::song::*;
use common::*;

/// Four synths `a`..`d`, each with a track (channels `channel`, `channel-2`, `channel-3`,
/// `channel-4`).
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
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn folder(d: &mut Document, r: &Registry, tracks: &[&str], name: &str) -> u32 {
    d.apply(
        sc(SongCommand::GroupTracks {
            devices: tracks.iter().map(|k| key(k)).collect(),
            name: name.into(),
        }),
        r,
    )
    .unwrap();
    d.doc().song.org.groups.iter().map(|g| g.id).max().unwrap()
}

fn route_new(d: &mut Document, r: &Registry, folder: u32, name: &str) {
    d.apply(
        con(ConsoleCommand::RouteFolderToNewBus {
            folder,
            name: name.into(),
        }),
        r,
    )
    .unwrap();
}

fn dest(d: &Document, channel: &str) -> OutputDest {
    output_dest(d.doc(), &key(channel))
}

fn bus_of(d: &Document, name: &str) -> atomik_model::DeviceKey {
    d.doc().console.bus_by_name(name).unwrap().bus.clone()
}

fn move_tracks(d: &mut Document, r: &Registry, tracks: &[&str], to: usize, into: Option<u32>) {
    d.apply(
        sc(SongCommand::MoveTracks {
            devices: tracks.iter().map(|k| key(k)).collect(),
            place: TrackPlace { to_index: to, into },
        }),
        r,
    )
    .unwrap();
}

fn members(d: &Document, folder: u32) -> Vec<String> {
    d.doc()
        .console
        .folder_route(folder)
        .unwrap()
        .members
        .iter()
        .map(|m| m.0.clone())
        .collect()
}

#[test]
fn routing_a_folder_to_a_new_bus_is_one_undo_step() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let before = d.doc().clone();
    route_new(&mut d, &r, f, "Strings bus");
    let bus = bus_of(&d, "Strings bus");
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(bus.clone()));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Bus(bus.clone()));
    assert_eq!(dest(&d, "channel-3"), OutputDest::Master);
    assert_eq!(d.doc().console.folder_route(f).unwrap().bus, bus);
    assert_eq!(members(&d, f), ["channel", "channel-2"]);
    assert_console_invariants(d.doc());
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
fn a_folder_can_play_into_a_bus_that_already_exists() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::CreateBus {
            name: "Mix".into(),
            dest: OutputDest::Master,
        }),
        &r,
    )
    .unwrap();
    let f = folder(&mut d, &r, &["c", "d"], "Keys");
    let bus = bus_of(&d, "Mix");
    d.apply(
        con(ConsoleCommand::RouteFolderToBus {
            folder: f,
            bus: bus.clone(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(dest(&d, "channel-3"), OutputDest::Bus(bus.clone()));
    assert_eq!(dest(&d, "channel-4"), OutputDest::Bus(bus.clone()));
    // The same again changes nothing and is refused.
    let e = d
        .apply(
            con(ConsoleCommand::RouteFolderToBus {
                folder: f,
                bus: bus.clone(),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("already plays"), "{e}");
    // A key that is no bus, and a folder that does not exist, are refused.
    for command in [
        ConsoleCommand::RouteFolderToBus {
            folder: f,
            bus: key("a"),
        },
        ConsoleCommand::RouteFolderToBus {
            folder: f + 9,
            bus: bus.clone(),
        },
        ConsoleCommand::RouteFolderToNewBus {
            folder: f + 9,
            name: "Nothing".into(),
        },
        ConsoleCommand::UnrouteFolder { folder: f + 9 },
    ] {
        assert!(d.apply(con(command), &r).is_err());
    }
    assert_console_invariants(d.doc());
}

#[test]
fn a_track_that_joins_or_leaves_the_folder_follows_in_the_same_undo_step() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    let bus = bus_of(&d, "Strings bus");

    let before = d.doc().clone();
    move_tracks(&mut d, &r, &["c"], 2, Some(f));
    assert_eq!(dest(&d, "channel-3"), OutputDest::Bus(bus.clone()));
    assert_eq!(members(&d, f), ["channel", "channel-2", "channel-3"]);
    let after = d.doc().clone();
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the move and the routing are one step");
    d.redo().unwrap();
    assert_eq!(d.doc(), &after);

    move_tracks(&mut d, &r, &["a"], 3, None);
    assert_eq!(
        dest(&d, "channel"),
        OutputDest::Master,
        "a track that leaves goes back to the master"
    );
    assert_eq!(members(&d, f), ["channel-2", "channel-3"]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_new_track_made_inside_the_folder_plays_into_the_bus() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    let bus = bus_of(&d, "Strings bus");
    d.apply(add(4, SYNTH, Some("e")), &r).unwrap();
    d.apply(add_track("e"), &r).unwrap();
    assert_eq!(dest(&d, "channel-5"), OutputDest::Master);
    move_tracks(&mut d, &r, &["e"], 2, Some(f));
    assert_eq!(dest(&d, "channel-5"), OutputDest::Bus(bus));
    assert_console_invariants(d.doc());
}

#[test]
fn an_output_chosen_by_hand_for_one_member_stays() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    d.apply(
        con(ConsoleCommand::SetOutput {
            strip: key("channel-2"),
            dest: OutputDest::AudioIo(2),
        }),
        &r,
    )
    .unwrap();
    // Later commands (a track joining) do not pull it back.
    move_tracks(&mut d, &r, &["c"], 2, Some(f));
    assert_eq!(dest(&d, "channel-2"), OutputDest::AudioIo(2));
    assert_console_invariants(d.doc());
}

#[test]
fn stopping_the_route_sends_the_channels_back_to_the_master() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    let routed = d.doc().clone();
    d.apply(con(ConsoleCommand::UnrouteFolder { folder: f }), &r)
        .unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::Master);
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
    assert!(d.doc().console.folder_routes.is_empty());
    assert!(
        d.doc().console.bus_by_name("Strings bus").is_some(),
        "the bus stays"
    );
    // A member the user had moved elsewhere is left where it is.
    d.undo().unwrap();
    assert_eq!(d.doc(), &routed);
    d.apply(
        con(ConsoleCommand::SetOutput {
            strip: key("channel"),
            dest: OutputDest::AudioIo(2),
        }),
        &r,
    )
    .unwrap();
    d.apply(con(ConsoleCommand::UnrouteFolder { folder: f }), &r)
        .unwrap();
    assert_eq!(dest(&d, "channel"), OutputDest::AudioIo(2));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Master);
}

#[test]
fn routing_a_folder_again_moves_it_to_the_other_bus() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "First");
    route_new(&mut d, &r, f, "Second");
    let second = bus_of(&d, "Second");
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(second.clone()));
    assert_eq!(d.doc().console.folder_routes.len(), 1);
    assert_eq!(d.doc().console.folder_route(f).unwrap().bus, second);
    assert_console_invariants(d.doc());
}

#[test]
fn a_dissolved_folder_or_a_deleted_bus_ends_the_route_and_keeps_the_cables() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    let bus = bus_of(&d, "Strings bus");
    let routed = d.doc().clone();
    d.apply(sc(SongCommand::Ungroup { group: f }), &r).unwrap();
    assert!(d.doc().console.folder_routes.is_empty());
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(bus.clone()));
    d.undo().unwrap();
    assert_eq!(d.doc(), &routed);

    d.apply(con(ConsoleCommand::DeleteBus { bus }), &r).unwrap();
    assert!(d.doc().console.folder_routes.is_empty());
    assert_console_invariants(d.doc());
    d.undo().unwrap();
    assert_eq!(d.doc(), &routed);
}

#[test]
fn a_routed_folder_inside_another_keeps_its_own_bus() {
    let (mut d, r) = project();
    let outer = folder(&mut d, &r, &["a", "b", "c"], "Outer");
    route_new(&mut d, &r, outer, "Outer bus");
    let outer_bus = bus_of(&d, "Outer bus");
    let inner = folder(&mut d, &r, &["a", "b"], "Inner");
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, Some(outer));
    route_new(&mut d, &r, inner, "Inner bus");
    let inner_bus = bus_of(&d, "Inner bus");
    assert_eq!(dest(&d, "channel"), OutputDest::Bus(inner_bus.clone()));
    assert_eq!(dest(&d, "channel-2"), OutputDest::Bus(inner_bus));
    assert_eq!(dest(&d, "channel-3"), OutputDest::Bus(outer_bus));
    assert_eq!(members(&d, outer), ["channel-3"]);
    assert_console_invariants(d.doc());
}

#[test]
fn folder_routes_are_saved_additively_and_read_back() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let plain = save_project(d.doc());
    assert!(!plain.contains("folder_routes"), "written only when set");
    route_new(&mut d, &r, f, "Strings bus");
    let text = save_project(d.doc());
    assert!(text.contains("\"folder_routes\""));
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console.folder_routes, d.doc().console.folder_routes);
    assert_eq!(save_project(&loaded), text, "the text is canonical");
    assert_console_invariants(&loaded);
}

#[test]
fn a_route_to_a_folder_the_song_lacks_is_dropped_and_bad_routes_are_refused() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    route_new(&mut d, &r, f, "Strings bus");
    let text = save_project(d.doc());
    let stale = text.replace(
        &format!("\"folder\": {f}, \"bus\""),
        &format!("\"folder\": {}, \"bus\"", f + 7),
    );
    assert_ne!(stale, text);
    let loaded = load_project(&stale, &r).unwrap();
    assert!(loaded.console.folder_routes.is_empty());
    let zero = text.replace(
        &format!("\"folder\": {f}, \"bus\""),
        "\"folder\": 0, \"bus\"",
    );
    assert!(load_project(&zero, &r).is_err());
    let not_a_bus = text.replace("\"bus\": \"bus\"", "\"bus\": \"channel\"");
    assert_ne!(not_a_bus, text);
    assert!(load_project(&not_a_bus, &r).is_err());
    let not_a_channel = text.replace("\"members\": [\"channel\"", "\"members\": [\"bus\"");
    assert_ne!(not_a_channel, text);
    assert!(load_project(&not_a_channel, &r).is_err());
}
