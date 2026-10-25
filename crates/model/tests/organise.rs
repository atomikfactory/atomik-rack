//! Track organisation: folders (`GroupTracks`, `Ungroup`, `RenameGroup`, `SetGroupCollapsed`,
//! `SetGroupColor`, `MoveTrack`, `MoveGroup`), track colours and comments, folded lanes and open
//! take rows: the commands, their undo, the outline the arrangement reads, and project format 22
//! (older files load unchanged, bad files are refused with their JSON path).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, DeviceKey, Document, MAX_GROUP_DEPTH, MAX_TRACK_COMMENT, ModelError, OutlineKind,
    PROJECT_VERSION, Song, SongCommand, SyncOp, load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::{Value, json};

const CUTOFF: &str = "filter.cutoff";

/// Six tracks in order: `synth`, `lead`, `a`, `b`, `c`, `d`.
fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
        d.apply(add(4 + i, SYNTH, Some(k)), reg).unwrap();
    }
    for k in ["synth", "lead", "a", "b", "c", "d"] {
        d.apply(add_track(k), reg).unwrap();
    }
    d
}

fn run(d: &mut Document, reg: &Registry, c: SongCommand) -> Vec<SyncOp> {
    d.apply(sc(c), reg).unwrap()
}

fn group(d: &mut Document, reg: &Registry, devices: &[&str], name: &str) -> u32 {
    run(
        d,
        reg,
        SongCommand::GroupTracks {
            devices: devices.iter().map(|k| key(k)).collect(),
            name: name.into(),
        },
    );
    d.doc().song.org.groups.iter().map(|g| g.id).max().unwrap()
}

fn order(d: &Document) -> Vec<String> {
    d.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

/// The outline as text: `[Name` for a folder header, the key for a track, two spaces per level.
fn outline(d: &Document) -> Vec<String> {
    let song = &d.doc().song;
    song.outline()
        .into_iter()
        .map(|e| {
            let pad = "  ".repeat(e.depth);
            match e.kind {
                OutlineKind::Folder(id) => {
                    format!("{pad}[{}", song.org.group(id).unwrap().name)
                }
                OutlineKind::Track(i) => format!("{pad}{}", song.tracks[i].device.0),
            }
        })
        .collect()
}

fn rejects(d: &mut Document, reg: &Registry, cmd: SongCommand) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(sc(cmd), reg).expect_err("must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo);
    e
}

fn mv(d: &mut Document, reg: &Registry, k: &str, down: bool) {
    run(
        d,
        reg,
        SongCommand::MoveTrack {
            device: key(k),
            down,
        },
    );
}

fn mv_group(d: &mut Document, reg: &Registry, group: u32, down: bool) {
    run(d, reg, SongCommand::MoveGroup { group, down });
}

// ---- folders -------------------------------------------------------------------------------

#[test]
fn a_new_song_has_no_organisation() {
    let reg = registry();
    let d = doc(&reg);
    assert_eq!(d.doc().song.org, Default::default());
    assert_eq!(
        outline(&d),
        ["synth", "lead", "a", "b", "c", "d"],
        "no folder rows"
    );
}

#[test]
fn grouping_gathers_the_tracks_at_the_first_one_and_undoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let ops = run(
        &mut d,
        &reg,
        SongCommand::GroupTracks {
            devices: vec![key("c"), key("lead"), key("synth")],
            name: "Pads".into(),
        },
    );
    assert_eq!(ops, vec![SyncOp::SongChanged], "tracks moved");
    assert_eq!(order(&d), ["synth", "lead", "c", "a", "b", "d"]);
    assert_eq!(
        outline(&d),
        ["[Pads", "  synth", "  lead", "  c", "a", "b", "d"]
    );
    let org = &d.doc().song.org;
    assert_eq!(org.groups.len(), 1);
    let g = &org.groups[0];
    assert_eq!(
        (g.id, g.name.as_str(), g.parent, g.collapsed),
        (1, "Pads", None, false)
    );
    assert_eq!(d.doc().song.group_tracks(1), [0, 1, 2]);
    assert_eq!(d.doc().song.track_group(&key("lead")), Some(1));
    assert_eq!(d.doc().song.track_group(&key("a")), None);
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "one undo step restores the order and the folders"
    );
    d.redo(&reg).unwrap();
    assert_eq!(order(&d), ["synth", "lead", "c", "a", "b", "d"]);
}

#[test]
fn grouping_neighbours_moves_nothing_and_reports_a_view_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = run(
        &mut d,
        &reg,
        SongCommand::GroupTracks {
            devices: vec![key("lead"), key("a")],
            name: "Keys".into(),
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(order(&d), ["synth", "lead", "a", "b", "c", "d"]);
}

#[test]
fn folders_nest_and_the_depth_is_limited() {
    let reg = registry();
    let mut d = doc(&reg);
    let outer = group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    let inner = group(&mut d, &reg, &["lead", "a"], "Rhythm");
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, Some(outer));
    assert_eq!(
        outline(&d),
        [
            "[Band",
            "  synth",
            "  [Rhythm",
            "    lead",
            "    a",
            "b",
            "c",
            "d"
        ]
    );
    assert_eq!(d.doc().song.group_tracks(outer), [0, 1, 2]);
    assert_eq!(d.doc().song.group_tracks(inner), [1, 2]);
    // Tracks of different folders cannot be grouped, and neither can a folder's track with
    // a loose one.
    rejects(
        &mut d,
        &reg,
        SongCommand::GroupTracks {
            devices: vec![key("synth"), key("lead")],
            name: "X".into(),
        },
    );
    rejects(
        &mut d,
        &reg,
        SongCommand::GroupTracks {
            devices: vec![key("a"), key("b")],
            name: "X".into(),
        },
    );
    // Wrapping the same track again nests one level deeper, up to the limit.
    let mut depth = 2;
    while depth < MAX_GROUP_DEPTH {
        group(&mut d, &reg, &["a"], &format!("Level {depth}"));
        depth += 1;
    }
    let e = rejects(
        &mut d,
        &reg,
        SongCommand::GroupTracks {
            devices: vec![key("a")],
            name: "Too deep".into(),
        },
    );
    assert!(e.to_string().contains("levels"), "{e}");
}

#[test]
fn bad_group_commands_are_rejected_and_change_nothing() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["a", "b"], "Two");
    let names = ["", " x", "x ", "a\nb", &"n".repeat(65)];
    for name in names {
        rejects(
            &mut d,
            &reg,
            SongCommand::GroupTracks {
                devices: vec![key("c")],
                name: name.into(),
            },
        );
        rejects(
            &mut d,
            &reg,
            SongCommand::RenameGroup {
                group: g,
                name: name.into(),
            },
        );
    }
    for devices in [
        vec![],
        vec![key("nobody")],
        vec![key("c"), key("c")],
        vec![key("delay")],
    ] {
        rejects(
            &mut d,
            &reg,
            SongCommand::GroupTracks {
                devices,
                name: "X".into(),
            },
        );
    }
    for cmd in [
        SongCommand::Ungroup { group: 99 },
        SongCommand::RenameGroup {
            group: 99,
            name: "x".into(),
        },
        SongCommand::SetGroupCollapsed {
            group: 99,
            collapsed: true,
        },
        SongCommand::SetGroupColor {
            group: 99,
            color: None,
        },
        SongCommand::MoveGroup {
            group: 99,
            down: true,
        },
    ] {
        rejects(&mut d, &reg, cmd);
    }
}

#[test]
fn ungrouping_leaves_the_tracks_in_the_parent() {
    let reg = registry();
    let mut d = doc(&reg);
    let outer = group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    let inner = group(&mut d, &reg, &["lead", "a"], "Rhythm");
    let before = d.doc().clone();
    run(&mut d, &reg, SongCommand::Ungroup { group: inner });
    assert_eq!(
        outline(&d),
        ["[Band", "  synth", "  lead", "  a", "b", "c", "d"]
    );
    assert_eq!(d.doc().song.track_group(&key("lead")), Some(outer));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    run(&mut d, &reg, SongCommand::Ungroup { group: outer });
    assert_eq!(
        outline(&d),
        ["synth", "[Rhythm", "  lead", "  a", "b", "c", "d"],
        "the inner folder is at the top level now"
    );
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, None);
    assert_eq!(order(&d).len(), 6, "no track is deleted");
}

#[test]
fn rename_colour_and_collapse_are_view_changes_and_undo() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    let ops = run(
        &mut d,
        &reg,
        SongCommand::RenameGroup {
            group: g,
            name: "Piano".into(),
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    run(
        &mut d,
        &reg,
        SongCommand::SetGroupColor {
            group: g,
            color: Some([200, 40, 40]),
        },
    );
    let ops = run(
        &mut d,
        &reg,
        SongCommand::SetGroupCollapsed {
            group: g,
            collapsed: true,
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    let grp = d.doc().song.org.group(g).unwrap().clone();
    assert_eq!(
        (grp.name.as_str(), grp.color, grp.collapsed),
        ("Piano", Some([200, 40, 40]), true)
    );
    let hidden: Vec<bool> = d.doc().song.outline().iter().map(|e| e.hidden).collect();
    assert_eq!(
        hidden,
        [false, false, true, true, false, false, false],
        "the header stays, its tracks hide"
    );
    for _ in 0..3 {
        d.undo(&reg).unwrap();
    }
    assert_eq!(d.doc().song.org.group(g).unwrap().name, "Keys");
    assert_eq!(d.doc().song.org.group(g).unwrap().color, None);
}

// ---- moving --------------------------------------------------------------------------------

#[test]
fn a_track_passes_its_neighbour_and_stops_at_the_ends() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = d.apply(
        sc(SongCommand::MoveTrack {
            device: key("a"),
            down: false,
        }),
        &reg,
    );
    assert_eq!(ops.unwrap(), vec![SyncOp::SongChanged]);
    assert_eq!(order(&d), ["synth", "a", "lead", "b", "c", "d"]);
    mv(&mut d, &reg, "a", true);
    mv(&mut d, &reg, "a", true);
    assert_eq!(order(&d), ["synth", "lead", "b", "a", "c", "d"]);
    d.undo(&reg).unwrap();
    assert_eq!(order(&d), ["synth", "lead", "a", "b", "c", "d"]);
    let e = rejects(
        &mut d,
        &reg,
        SongCommand::MoveTrack {
            device: key("synth"),
            down: false,
        },
    );
    assert!(e.to_string().contains("first"), "{e}");
    let e = rejects(
        &mut d,
        &reg,
        SongCommand::MoveTrack {
            device: key("d"),
            down: true,
        },
    );
    assert!(e.to_string().contains("last"), "{e}");
}

#[test]
fn a_track_leaves_a_folder_at_its_edge_and_enters_one_from_outside() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    // Up from the first track of the folder: out, above its header. Nothing is reordered.
    mv(&mut d, &reg, "lead", false);
    assert_eq!(
        outline(&d),
        ["synth", "lead", "[Keys", "  a", "b", "c", "d"]
    );
    assert_eq!(order(&d), ["synth", "lead", "a", "b", "c", "d"]);
    // Down from the track above the folder: in, as its first track.
    mv(&mut d, &reg, "lead", true);
    assert_eq!(
        outline(&d),
        ["synth", "[Keys", "  lead", "  a", "b", "c", "d"]
    );
    // Down from the last: out below.
    mv(&mut d, &reg, "a", true);
    assert_eq!(
        outline(&d),
        ["synth", "[Keys", "  lead", "a", "b", "c", "d"]
    );
    // Up from the track below the folder: in, as its last.
    mv(&mut d, &reg, "a", false);
    assert_eq!(
        outline(&d),
        ["synth", "[Keys", "  lead", "  a", "b", "c", "d"]
    );
    // A single track leaving empties the folder, which goes away.
    run(&mut d, &reg, SongCommand::Ungroup { group: g });
    let solo = group(&mut d, &reg, &["a"], "Solo");
    mv(&mut d, &reg, "a", false);
    assert_eq!(outline(&d), ["synth", "lead", "a", "b", "c", "d"]);
    assert!(d.doc().song.org.group(solo).is_none());
    assert!(d.doc().song.normalised_org().groups.is_empty());
    // And undo brings the folder back.
    d.undo(&reg).unwrap();
    assert_eq!(
        outline(&d),
        ["synth", "lead", "[Solo", "  a", "b", "c", "d"]
    );
}

#[test]
fn a_track_enters_the_innermost_folder_one_level_at_a_time() {
    let reg = registry();
    let mut d = doc(&reg);
    group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    group(&mut d, &reg, &["lead", "a"], "Rhythm");
    // `b` is below Band; up enters Band (its last place), up again swaps with `a`'s folder
    // end: it enters Rhythm.
    mv(&mut d, &reg, "b", false);
    assert_eq!(
        outline(&d),
        [
            "[Band",
            "  synth",
            "  [Rhythm",
            "    lead",
            "    a",
            "  b",
            "c",
            "d"
        ]
    );
    mv(&mut d, &reg, "b", false);
    assert_eq!(
        outline(&d),
        [
            "[Band",
            "  synth",
            "  [Rhythm",
            "    lead",
            "    a",
            "    b",
            "c",
            "d"
        ]
    );
    // Out of Rhythm and out of Band again, downwards.
    mv(&mut d, &reg, "b", true);
    mv(&mut d, &reg, "b", true);
    assert_eq!(
        outline(&d),
        [
            "[Band",
            "  synth",
            "  [Rhythm",
            "    lead",
            "    a",
            "b",
            "c",
            "d"
        ]
    );
}

#[test]
fn a_folder_moves_as_a_block_and_leaves_its_parent_at_the_edge() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    mv_group(&mut d, &reg, g, true);
    assert_eq!(
        outline(&d),
        ["synth", "b", "[Keys", "  lead", "  a", "c", "d"]
    );
    assert_eq!(order(&d), ["synth", "b", "lead", "a", "c", "d"]);
    mv_group(&mut d, &reg, g, false);
    mv_group(&mut d, &reg, g, false);
    assert_eq!(
        outline(&d),
        ["[Keys", "  lead", "  a", "synth", "b", "c", "d"]
    );
    let e = rejects(
        &mut d,
        &reg,
        SongCommand::MoveGroup {
            group: g,
            down: false,
        },
    );
    assert!(e.to_string().contains("first"), "{e}");
    // A folder passes a whole sibling folder.
    let other = group(&mut d, &reg, &["b", "c"], "Perc");
    mv_group(&mut d, &reg, g, true);
    mv_group(&mut d, &reg, g, true);
    assert_eq!(
        outline(&d),
        [
            "synth", "[Perc", "  b", "  c", "[Keys", "  lead", "  a", "d"
        ]
    );
    mv_group(&mut d, &reg, other, true);
    assert_eq!(
        outline(&d),
        [
            "synth", "[Keys", "  lead", "  a", "[Perc", "  b", "  c", "d"
        ]
    );
    // One undo step each.
    d.undo(&reg).unwrap();
    assert_eq!(order(&d)[1..3], ["b", "c"]);
}

#[test]
fn a_folder_at_the_edge_of_its_parent_leaves_it() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["synth", "lead"], "Band");
    let inner = group(&mut d, &reg, &["lead"], "Solo");
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, Some(band));
    mv_group(&mut d, &reg, inner, true);
    assert_eq!(d.doc().song.org.group(inner).unwrap().parent, None);
    assert_eq!(outline(&d)[..4], ["[Band", "  synth", "[Solo", "  lead"]);
}

// ---- colours and comments ------------------------------------------------------------------

#[test]
fn track_colour_and_comment_set_clear_and_undo() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = run(
        &mut d,
        &reg,
        SongCommand::SetTrackColor {
            device: key("lead"),
            color: Some([10, 20, 30]),
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackComment {
            device: key("lead"),
            comment: "Record the take\nwith the mod wheel".into(),
        },
    );
    let song = &d.doc().song;
    assert_eq!(song.track_color(&key("lead")), Some([10, 20, 30]));
    assert_eq!(
        song.track_comment(&key("lead")),
        "Record the take\nwith the mod wheel"
    );
    assert_eq!(song.track_comment(&key("synth")), "");
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackColor {
            device: key("lead"),
            color: None,
        },
    );
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackComment {
            device: key("lead"),
            comment: "  ".into(),
        },
    );
    assert!(d.doc().song.org.colors.is_empty() && d.doc().song.org.comments.is_empty());
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.track_color(&key("lead")), Some([10, 20, 30]));
    assert!(!d.doc().song.track_comment(&key("lead")).is_empty());
}

#[test]
fn bad_colours_and_comments_are_rejected() {
    let reg = registry();
    let mut d = doc(&reg);
    for comment in [
        " padded".to_owned(),
        "tab\there".to_owned(),
        "x".repeat(MAX_TRACK_COMMENT + 1),
    ] {
        rejects(
            &mut d,
            &reg,
            SongCommand::SetTrackComment {
                device: key("lead"),
                comment,
            },
        );
    }
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackComment {
            device: key("lead"),
            comment: "x".repeat(MAX_TRACK_COMMENT),
        },
    );
    rejects(
        &mut d,
        &reg,
        SongCommand::SetTrackColor {
            device: key("nobody"),
            color: None,
        },
    );
    rejects(
        &mut d,
        &reg,
        SongCommand::SetTrackComment {
            device: key("nobody"),
            comment: "x".into(),
        },
    );
}

// ---- folded lanes and open takes -----------------------------------------------------------

fn with_lane(reg: &Registry) -> Document {
    let mut d = doc(reg);
    run(
        &mut d,
        reg,
        SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        },
    );
    d
}

#[test]
fn a_lane_folds_and_unfolds_and_the_lane_must_exist() {
    let reg = registry();
    let mut d = with_lane(&reg);
    assert!(!d.doc().song.lane_folded(&key("lead"), CUTOFF));
    let ops = run(
        &mut d,
        &reg,
        SongCommand::SetLaneFolded {
            device: key("lead"),
            param: CUTOFF.into(),
            folded: true,
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert!(d.doc().song.lane_folded(&key("lead"), CUTOFF));
    d.undo(&reg).unwrap();
    assert!(!d.doc().song.lane_folded(&key("lead"), CUTOFF));
    d.redo(&reg).unwrap();
    run(
        &mut d,
        &reg,
        SongCommand::SetLaneFolded {
            device: key("lead"),
            param: CUTOFF.into(),
            folded: false,
        },
    );
    assert!(d.doc().song.org.folded_lanes.is_empty());
    for (device, param) in [("lead", "nope"), ("synth", CUTOFF), ("nobody", CUTOFF)] {
        rejects(
            &mut d,
            &reg,
            SongCommand::SetLaneFolded {
                device: key(device),
                param: param.into(),
                folded: true,
            },
        );
    }
}

fn with_takes(reg: &Registry) -> Document {
    let mut d = doc(reg);
    run(
        &mut d,
        reg,
        SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: vec![
                notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)]),
                notes_clip(0, BAR, vec![note(0, PPQ, 62, 0.8)]),
            ],
        },
    );
    d
}

#[test]
fn take_rows_open_and_close_and_the_group_must_exist() {
    let reg = registry();
    let mut d = with_takes(&reg);
    let id = d.doc().song.take_groups().next().unwrap().2.id;
    assert!(d.doc().song.org.open_takes.is_empty(), "closed by default");
    let ops = run(
        &mut d,
        &reg,
        SongCommand::SetTakesOpen {
            take_group: id,
            open: true,
        },
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert!(d.doc().song.org.open_takes.contains(&id));
    d.undo(&reg).unwrap();
    assert!(d.doc().song.org.open_takes.is_empty());
    rejects(
        &mut d,
        &reg,
        SongCommand::SetTakesOpen {
            take_group: 77,
            open: true,
        },
    );
}

// ---- removing tracks -----------------------------------------------------------------------

#[test]
fn removing_a_track_keeps_the_folder_and_undo_brings_everything_back() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackColor {
            device: key("a"),
            color: Some([1, 2, 3]),
        },
    );
    run(&mut d, &reg, SongCommand::RemoveTrack { device: key("a") });
    assert_eq!(outline(&d), ["synth", "[Keys", "  lead", "b", "c", "d"]);
    // The folder with no track left disappears from the outline and from the file.
    run(
        &mut d,
        &reg,
        SongCommand::RemoveTrack {
            device: key("lead"),
        },
    );
    assert_eq!(outline(&d), ["synth", "b", "c", "d"]);
    let text = save_project(d.doc());
    assert!(
        !text.contains("\"groups\"") && !text.contains("#010203"),
        "{text}"
    );
    assert!(load_project(&text, &reg).is_ok());
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(
        outline(&d),
        ["synth", "[Keys", "  lead", "  a", "b", "c", "d"]
    );
    assert_eq!(d.doc().song.track_color(&key("a")), Some([1, 2, 3]));
    assert!(d.doc().song.org.group(g).is_some());
}

#[test]
fn a_track_that_stands_apart_from_its_folder_is_shown_outside_it() {
    // A hand-built song that breaks the invariant: the outline and the saved file repair it.
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    let mut song: Song = d.doc().song.clone();
    song.org.members.insert(key("c"), g);
    let shown: Vec<String> = song
        .outline()
        .into_iter()
        .map(|e| match e.kind {
            OutlineKind::Folder(_) => format!("[{}", e.depth),
            OutlineKind::Track(i) => format!("{}:{}", song.tracks[i].device.0, e.depth),
        })
        .collect();
    assert_eq!(
        shown,
        ["synth:0", "[0", "lead:1", "a:1", "b:0", "c:0", "d:0"]
    );
    assert_eq!(song.group_tracks(g), [1, 2]);
    assert!(!song.normalised_org().members.contains_key(&key("c")));
}

// ---- project format 22 ---------------------------------------------------------------------

/// A song with two nested folders, colours, a comment, a folded lane and an open take group.
fn organised(reg: &Registry) -> Document {
    let mut d = with_takes(reg);
    run(
        &mut d,
        reg,
        SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        },
    );
    let band = group(&mut d, reg, &["synth", "lead", "a"], "Band");
    let rhythm = group(&mut d, reg, &["lead", "a"], "Rhythm \"main\"");
    run(
        &mut d,
        reg,
        SongCommand::SetGroupCollapsed {
            group: rhythm,
            collapsed: true,
        },
    );
    run(
        &mut d,
        reg,
        SongCommand::SetGroupColor {
            group: band,
            color: Some([0xff, 0x80, 0x00]),
        },
    );
    run(
        &mut d,
        reg,
        SongCommand::SetTrackColor {
            device: key("b"),
            color: Some([0x12, 0xab, 0xef]),
        },
    );
    run(
        &mut d,
        reg,
        SongCommand::SetTrackComment {
            device: key("lead"),
            comment: "Line one\nLine \"two\"".into(),
        },
    );
    run(
        &mut d,
        reg,
        SongCommand::SetLaneFolded {
            device: key("lead"),
            param: CUTOFF.into(),
            folded: true,
        },
    );
    let take = d.doc().song.take_groups().next().unwrap().2.id;
    run(
        &mut d,
        reg,
        SongCommand::SetTakesOpen {
            take_group: take,
            open: true,
        },
    );
    d
}

#[test]
fn the_organisation_round_trips_canonically_and_matches_the_golden_text() {
    let reg = registry();
    let d = organised(&reg);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 28);
    assert_eq!(
        v["song"]["groups"],
        json!([
            {"id": 1, "name": "Band", "color": "#ff8000"},
            {"id": 2, "name": "Rhythm \"main\"", "parent": 1, "collapsed": true}
        ])
    );
    let tracks = &v["song"]["tracks"];
    assert_eq!(tracks[0]["group"], 1);
    assert_eq!(tracks[1]["group"], 2);
    assert_eq!(tracks[1]["comment"], "Line one\nLine \"two\"");
    assert_eq!(tracks[1]["lanes"][0]["take_groups"][0]["open"], true);
    assert_eq!(tracks[1]["lanes"][1]["folded"], true);
    assert_eq!(tracks[3]["color"], "#12abef");
    assert!(tracks[4].get("group").is_none() && tracks[4].get("color").is_none());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
    // The committed golden pins the text of the new fields.
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/organised_project.json");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &text).unwrap();
    }
    let committed = std::fs::read_to_string(&golden).unwrap();
    assert_eq!(
        committed.replace("\r\n", "\n"),
        text,
        "regenerate with ATOMIK_UPDATE_GOLDEN=1"
    );
}

#[test]
fn the_golden_loads_and_saves_to_itself() {
    let reg = registry();
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/organised_project.json");
    let text = std::fs::read_to_string(golden)
        .unwrap()
        .replace("\r\n", "\n");
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.org.groups.len(), 2);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_song_without_organisation_writes_no_new_field() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let song = v["song"].to_string();
    for field in ["groups", "group", "color", "comment", "folded", "open"] {
        assert!(!song.contains(&format!("\"{field}\"")), "{field}");
    }
}

#[test]
fn a_version_21_project_loads_unchanged_and_resaves_as_22() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let old = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 21",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert_eq!(loaded.song.org, Default::default());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn the_new_fields_are_unknown_before_version_22() {
    let reg = registry();
    let good = save_project(organised(&reg).doc());
    // The file as version 21 with only one of the new fields left in.
    let only = |keep: &str| {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        if keep != "groups" {
            v["song"].as_object_mut().unwrap().remove("groups");
        }
        for t in v["song"]["tracks"].as_array_mut().unwrap() {
            let t = t.as_object_mut().unwrap();
            for f in ["group", "color", "comment"] {
                if f != keep {
                    t.remove(f);
                }
            }
            for lane in t["lanes"].as_array_mut().unwrap() {
                let lane = lane.as_object_mut().unwrap();
                if keep != "folded" {
                    lane.remove("folded");
                }
                for g in lane
                    .get_mut("take_groups")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if keep != "open" {
                        g.as_object_mut().unwrap().remove("open");
                    }
                }
            }
        }
        v["version"] = json!(21);
        v
    };
    for field in ["groups", "group", "color", "comment", "folded", "open"] {
        let e = load_project(&only(field).to_string(), &reg).expect_err("unknown in version 21");
        assert!(
            e.to_string().contains(field) && e.to_string().contains("unknown field"),
            "{field}: {e}"
        );
    }
}

#[test]
fn bad_organisation_is_refused_with_its_json_path() {
    let reg = registry();
    let d = organised(&reg);
    let good = save_project(d.doc());
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["song"]["groups"][0]["id"] = json!(0)),
            "song.groups[0].id",
            "start at 1",
        ),
        (
            Box::new(|v| v["song"]["groups"][1]["id"] = json!(1)),
            "song.groups[1].id",
            "duplicate",
        ),
        (
            Box::new(|v| v["song"]["groups"][0]["name"] = json!(" Band")),
            "song.groups[0].name",
            "trimmed",
        ),
        (
            Box::new(|v| v["song"]["groups"][0]["name"] = json!("")),
            "song.groups[0].name",
            "non-empty",
        ),
        (
            Box::new(|v| v["song"]["groups"][0]["color"] = json!("red")),
            "song.groups[0].color",
            "",
        ),
        (
            Box::new(|v| v["song"]["groups"][1]["parent"] = json!(9)),
            "song.groups[1].parent",
            "no folder has id 9",
        ),
        (
            Box::new(|v| v["song"]["groups"][0]["parent"] = json!(2)),
            "song.groups[0].parent",
            "cannot contain themselves",
        ),
        (
            Box::new(|v| v["song"]["groups"][1]["parent"] = json!(2)),
            "song.groups[1].parent",
            "cannot contain themselves",
        ),
        (
            Box::new(|v| v["song"]["tracks"][4]["group"] = json!(7)),
            "song.tracks[4].group",
            "no folder has id 7",
        ),
        (
            // `b` and `c` are outside the folder: a folder's tracks follow each other.
            Box::new(|v| v["song"]["tracks"][4]["group"] = json!(1)),
            "song.tracks[4].group",
            "not one block",
        ),
        (
            Box::new(|v| {
                v["song"]["groups"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id": 5, "name": "Empty"}))
            }),
            "song.groups[2]",
            "has no tracks",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["comment"] = json!("")),
            "song.tracks[1].comment",
            "non-empty",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["comment"] = json!(" x")),
            "song.tracks[1].comment",
            "trimmed",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["comment"] = json!("a\tb")),
            "song.tracks[1].comment",
            "control",
        ),
        (
            Box::new(|v| v["song"]["tracks"][3]["color"] = json!(5)),
            "song.tracks[3].color",
            "",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["lanes"][1]["folded"] = json!("yes")),
            "song.tracks[1].lanes[1].folded",
            "",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["lanes"][0]["take_groups"][0]["open"] = json!(1)),
            "song.tracks[1].lanes[0].take_groups[0].open",
            "",
        ),
    ];
    for (edit, path, needle) in cases {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        edit(&mut v);
        let e = load_project(&v.to_string(), &reg).expect_err("must be rejected");
        let ModelError::Patch(msg) = &e else {
            panic!("expected a patch error, got {e:?}")
        };
        assert!(
            msg.starts_with(&format!("{path}: ")),
            "{msg} (wanted {path})"
        );
        assert!(msg.contains(needle), "{msg} lacks {needle:?}");
    }
}

#[test]
fn too_many_folders_are_refused() {
    let reg = registry();
    let good = save_project(organised(&reg).doc());
    let mut v: Value = serde_json::from_str(&good).unwrap();
    let groups = v["song"]["groups"].as_array_mut().unwrap();
    for id in 3..=300 {
        groups.push(json!({"id": id, "name": format!("G{id}")}));
    }
    let e = load_project(&v.to_string(), &reg).expect_err("too many");
    assert!(e.to_string().contains("at most 256"), "{e}");
}

#[test]
fn loaded_organisation_matches_the_model_after_each_step() {
    // Whatever a sequence of commands leaves, saving and loading gives the same document.
    let reg = registry();
    let mut d = doc(&reg);
    type Step = Box<dyn Fn(&mut Document, &Registry)>;
    let steps: Vec<Step> = vec![
        Box::new(|d, r| {
            group(d, r, &["a", "c"], "Odd");
        }),
        Box::new(|d, r| mv(d, r, "b", false)),
        Box::new(|d, r| mv(d, r, "lead", true)),
        Box::new(|d, r| {
            group(d, r, &["c"], "Inner");
        }),
        Box::new(|d, r| mv_group(d, r, 1, false)),
        Box::new(|d, r| {
            run(d, r, SongCommand::Ungroup { group: 2 })
                .iter()
                .for_each(drop)
        }),
    ];
    for step in steps {
        step(&mut d, &reg);
        let text = save_project(d.doc());
        let loaded = load_project(&text, &reg).unwrap();
        assert_eq!(&loaded, d.doc(), "{:?}", outline(&d));
        assert_eq!(save_project(&loaded), text);
    }
}

/// Keeps `DeviceKey` in the imports for the helpers above.
#[test]
fn device_keys_compare_by_name() {
    assert_eq!(key("a"), DeviceKey("a".into()));
}

#[test]
fn colours_and_folds_batch_into_one_undo_step() {
    let reg = registry();
    let mut d = with_lane(&reg);
    run(
        &mut d,
        &reg,
        SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        },
    );
    let batch = Command::Batch(vec![
        sc(SongCommand::SetTrackColor {
            device: key("synth"),
            color: Some([1, 2, 3]),
        }),
        sc(SongCommand::SetTrackColor {
            device: key("lead"),
            color: Some([4, 5, 6]),
        }),
        sc(SongCommand::SetLaneFolded {
            device: key("synth"),
            param: CUTOFF.into(),
            folded: true,
        }),
        sc(SongCommand::SetLaneFolded {
            device: key("lead"),
            param: CUTOFF.into(),
            folded: true,
        }),
    ]);
    let ops = d.apply(batch, &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    let org = &d.doc().song.org;
    assert_eq!((org.colors.len(), org.folded_lanes.len()), (2, 2));
    d.undo(&reg).unwrap();
    let org = &d.doc().song.org;
    assert_eq!((org.colors.len(), org.folded_lanes.len()), (0, 0));
}

#[test]
fn can_move_agrees_with_the_commands() {
    let reg = registry();
    let mut d = doc(&reg);
    let g = group(&mut d, &reg, &["lead", "a"], "Keys");
    for k in ["synth", "lead", "a", "b", "c", "d"] {
        for down in [false, true] {
            let song = &d.doc().song;
            let can = song.can_move_track(&key(k), down);
            let mut probe = d.clone();
            let ok = probe
                .apply(
                    sc(SongCommand::MoveTrack {
                        device: key(k),
                        down,
                    }),
                    &reg,
                )
                .is_ok();
            assert_eq!(can, ok, "{k} down={down}");
        }
    }
    for down in [false, true] {
        let can = d.doc().song.can_move_group(g, down);
        let mut probe = d.clone();
        let ok = probe
            .apply(sc(SongCommand::MoveGroup { group: g, down }), &reg)
            .is_ok();
        assert_eq!(can, ok, "group down={down}");
    }
    assert!(!d.doc().song.can_move_track(&key("nobody"), true));
    assert!(!d.doc().song.can_move_group(99, true));
    // At the top of a folder that is at the top of the list: the track may still leave.
    run(
        &mut d,
        &reg,
        SongCommand::MoveGroup {
            group: g,
            down: false,
        },
    );
    assert!(d.doc().song.can_move_track(&key("lead"), false));
}
