//! A VCA group that follows a track folder: the group's members are the console channels of the
//! folder's tracks, kept so after every command in the same undo step, and the optional `follows`
//! field of the project format.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::VcaGroup;
use atomik_model::{
    Command, ConsoleCommand, Document, SongCommand, TrackPlace, load_project, save_project,
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

fn members(d: &Document, vca: u32) -> Vec<String> {
    d.doc()
        .console
        .vca(vca)
        .unwrap()
        .members
        .iter()
        .map(|m| m.0.clone())
        .collect()
}

fn follow(d: &mut Document, r: &Registry, folder: u32) -> u32 {
    d.apply(
        con(ConsoleCommand::CreateVcaForFolder {
            folder,
            name: "Strings".into(),
        }),
        r,
    )
    .unwrap();
    d.doc().console.vcas.last().unwrap().id
}

/// Moves `tracks` to the place `to_index` among the tracks that stay.
fn move_tracks(
    d: &mut Document,
    r: &Registry,
    tracks: &[&str],
    to_index: usize,
    into: Option<u32>,
) {
    d.apply(
        sc(SongCommand::MoveTracks {
            devices: tracks.iter().map(|k| key(k)).collect(),
            place: TrackPlace { to_index, into },
        }),
        r,
    )
    .unwrap();
}

#[test]
fn a_group_made_for_a_folder_holds_the_channels_of_its_tracks() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let before = d.doc().clone();
    let id = follow(&mut d, &r, f);
    assert_eq!(members(&d, id), ["channel", "channel-2"]);
    assert_eq!(d.doc().console.vca(id).unwrap().follows, Some(f));
    assert_console_invariants(d.doc());
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the creation is one undo step");
}

#[test]
fn the_group_follows_tracks_moving_in_and_out_in_the_same_undo_step() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let id = follow(&mut d, &r, f);

    let before = d.doc().clone();
    move_tracks(&mut d, &r, &["c"], 2, Some(f));
    assert_eq!(members(&d, id), ["channel", "channel-2", "channel-3"]);
    let after = d.doc().clone();
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the move and the group are one step");
    d.redo().unwrap();
    assert_eq!(d.doc(), &after);

    move_tracks(&mut d, &r, &["a"], 3, None);
    assert_eq!(members(&d, id), ["channel-2", "channel-3"]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_track_added_to_a_nested_folder_joins_the_group_of_the_outer_one() {
    let (mut d, r) = project();
    let outer = folder(&mut d, &r, &["a", "b"], "Outer");
    let id = follow(&mut d, &r, outer);
    let inner = folder(&mut d, &r, &["b"], "Inner");
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, Some(outer));
    move_tracks(&mut d, &r, &["d"], 1, Some(inner));
    assert_eq!(members(&d, id), ["channel", "channel-2", "channel-4"]);
}

#[test]
fn a_strip_of_the_folder_leaves_the_hand_made_group_it_was_in() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Mine".into(),
            members: vec![key("channel-3"), key("channel-4")],
        }),
        &r,
    )
    .unwrap();
    let f = folder(&mut d, &r, &["a"], "Strings");
    let id = follow(&mut d, &r, f);
    move_tracks(&mut d, &r, &["c"], 1, Some(f));
    assert_eq!(members(&d, id), ["channel", "channel-3"]);
    assert_eq!(members(&d, 1), ["channel-4"]);
}

#[test]
fn dissolving_the_folder_keeps_the_members_and_stops_following() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let id = follow(&mut d, &r, f);
    let before = d.doc().clone();
    d.apply(sc(SongCommand::Ungroup { group: f }), &r).unwrap();
    let g = d.doc().console.vca(id).unwrap();
    assert_eq!(g.follows, None);
    assert_eq!(members(&d, id), ["channel", "channel-2"]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn deleting_a_track_with_its_device_takes_the_channel_out() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let id = follow(&mut d, &r, f);
    d.apply(Command::RemoveDevice { key: key("a") }, &r)
        .unwrap();
    assert_eq!(members(&d, id), ["channel-2"]);
    assert_console_invariants(d.doc());
    d.undo().unwrap();
    assert_eq!(members(&d, id), ["channel", "channel-2"]);
}

#[test]
fn the_members_of_a_following_group_are_not_edited_by_hand() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let id = follow(&mut d, &r, f);
    let refuse = |d: &mut Document, c: ConsoleCommand| {
        let before = d.doc().clone();
        let e = d.apply(con(c), &r).unwrap_err().to_string();
        assert_eq!(d.doc(), &before);
        assert!(e.contains("follows"), "{e}");
    };
    refuse(
        &mut d,
        ConsoleCommand::AssignToVca {
            vca: Some(id),
            strips: vec![key("channel-3")],
        },
    );
    refuse(
        &mut d,
        ConsoleCommand::AssignToVca {
            vca: None,
            strips: vec![key("channel")],
        },
    );
    refuse(
        &mut d,
        ConsoleCommand::CreateVca {
            name: "Other".into(),
            members: vec![key("channel-2")],
        },
    );
}

#[test]
fn one_group_follows_a_folder_and_the_rules_of_following() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let id = follow(&mut d, &r, f);
    let again = d
        .apply(
            con(ConsoleCommand::CreateVcaForFolder {
                folder: f,
                name: "Twice".into(),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(again.contains("already follows"), "{again}");
    let no_folder = d
        .apply(
            con(ConsoleCommand::CreateVcaForFolder {
                folder: 99,
                name: "Lost".into(),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(no_folder.contains("no track folder"), "{no_folder}");
    let same = d
        .apply(
            con(ConsoleCommand::SetVcaFollow {
                vca: id,
                folder: Some(f),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(same.contains("already follows"), "{same}");
}

#[test]
fn a_group_made_by_hand_can_follow_a_folder_and_stop() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Hand".into(),
            members: vec![key("channel-4")],
        }),
        &r,
    )
    .unwrap();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    let before = d.doc().clone();
    d.apply(
        con(ConsoleCommand::SetVcaFollow {
            vca: 1,
            folder: Some(f),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(members(&d, 1), ["channel", "channel-2"]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();

    d.apply(
        con(ConsoleCommand::SetVcaFollow {
            vca: 1,
            folder: None,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().follows, None);
    assert_eq!(members(&d, 1), ["channel", "channel-2"]);
    // A group that stopped following is an ordinary group again.
    d.apply(
        con(ConsoleCommand::AssignToVca {
            vca: Some(1),
            strips: vec![key("channel-3")],
        }),
        &r,
    )
    .unwrap();
    assert_eq!(members(&d, 1), ["channel", "channel-2", "channel-3"]);
}

#[test]
fn the_follow_is_saved_only_when_set_and_round_trips() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::CreateVca {
            name: "Hand".into(),
            members: vec![key("channel-4")],
        }),
        &r,
    )
    .unwrap();
    assert!(!save_project(d.doc()).contains("follows"));
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    follow(&mut d, &r, f);
    let text = save_project(d.doc());
    assert_eq!(text.matches("\"follows\"").count(), 1, "{text}");
    let loaded = load_project(&text, &r).unwrap();
    let g: &VcaGroup = loaded.console.vca(2).unwrap();
    assert_eq!(g.follows, Some(f));
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_file_that_follows_a_missing_folder_loads_as_an_ordinary_group() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a", "b"], "Strings");
    follow(&mut d, &r, f);
    let text = save_project(d.doc()).replace(
        &format!("\"follows\": {f}"),
        &format!("\"follows\": {}", f + 7),
    );
    let loaded = load_project(&text, &r).unwrap();
    let g = loaded.console.vca(1).unwrap();
    assert_eq!(g.follows, None);
    assert_eq!(g.members.len(), 2);
    let zero = save_project(d.doc()).replace(&format!("\"follows\": {f}"), "\"follows\": 0");
    assert!(load_project(&zero, &r).is_err());
}

#[test]
fn a_folder_that_loses_its_last_track_is_no_longer_followed() {
    let (mut d, r) = project();
    let f = folder(&mut d, &r, &["a"], "Solo");
    let id = follow(&mut d, &r, f);
    d.apply(sc(SongCommand::RemoveTrack { device: key("a") }), &r)
        .unwrap();
    assert_eq!(d.doc().console.vca(id).unwrap().follows, None);
    d.undo().unwrap();
    assert_eq!(d.doc().console.vca(id).unwrap().follows, Some(f));
    // A folder without a track cannot be followed.
    let e = d
        .apply(
            con(ConsoleCommand::CreateVcaForFolder {
                folder: f + 5,
                name: "None".into(),
            }),
            &r,
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("no track folder"), "{e}");
}
