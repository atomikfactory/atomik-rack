//! Folders that mute and solo by themselves (`SetGroupFlags`), the colour a track inherits from
//! its folder, and moving a folder with its tracks to a place (`MoveGroupTo`): the commands,
//! their undo, what plays, and how the flags are saved.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::effective_tracks;
use atomik_model::{
    Document, MAX_GROUP_DEPTH, ModelError, OutlineKind, SongCommand, SyncOp, TrackPlace,
    load_project, save_project,
};
use common::song::*;
use common::*;

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

fn flags(d: &mut Document, reg: &Registry, group: u32, muted: bool, solo: bool) {
    run(d, reg, SongCommand::SetGroupFlags { group, muted, solo });
}

fn playing(d: &Document) -> Vec<String> {
    effective_tracks(&d.doc().song)
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

fn outline(d: &Document) -> Vec<String> {
    let song = &d.doc().song;
    song.outline()
        .into_iter()
        .map(|e| {
            let pad = "  ".repeat(e.depth);
            match e.kind {
                OutlineKind::Folder(id) => format!("{pad}[{}", song.org.group(id).unwrap().name),
                OutlineKind::Track(i) => format!("{pad}{}", song.tracks[i].device.0),
            }
        })
        .collect()
}

fn move_group(d: &mut Document, reg: &Registry, group: u32, to_index: usize, into: Option<u32>) {
    run(
        d,
        reg,
        SongCommand::MoveGroupTo {
            group,
            place: TrackPlace { to_index, into },
        },
    );
}

fn rejects(d: &mut Document, reg: &Registry, cmd: SongCommand) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(sc(cmd), reg).expect_err("must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo);
    e
}

// ---- mute and solo ----------------------------------------------------------------------------

#[test]
fn a_muted_folder_silences_its_tracks_without_touching_their_flags() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    let before = d.doc().clone();
    let ops = run(
        &mut d,
        &reg,
        SongCommand::SetGroupFlags {
            group: band,
            muted: true,
            solo: false,
        },
    );
    assert_eq!(ops, [SyncOp::SongChanged], "what plays changed");
    assert_eq!(playing(&d), ["synth", "b", "c", "d"]);
    assert!(d.doc().song.tracks.iter().all(|t| !t.muted && !t.solo));
    assert!(d.doc().song.muted_by_folder(&key("lead")));
    assert!(!d.doc().song.muted_by_folder(&key("synth")));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    assert_eq!(playing(&d).len(), 6);
    d.redo().unwrap();
    assert_eq!(playing(&d), ["synth", "b", "c", "d"]);
}

#[test]
fn a_soloed_folder_plays_alone_and_a_track_solo_joins_it() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    flags(&mut d, &reg, band, false, true);
    assert_eq!(playing(&d), ["lead", "a"]);
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackFlags {
            device: key("c"),
            muted: false,
            solo: true,
            armed: false,
        },
    );
    assert_eq!(playing(&d), ["lead", "a", "c"]);
    // A mute still beats a solo, on the track or on the folder.
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackFlags {
            device: key("a"),
            muted: true,
            solo: false,
            armed: false,
        },
    );
    assert_eq!(playing(&d), ["lead", "c"]);
    flags(&mut d, &reg, band, true, true);
    assert_eq!(playing(&d), ["c"]);
}

#[test]
fn the_flags_of_folders_nest() {
    let reg = registry();
    let mut d = doc(&reg);
    let outer = group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    let inner = group(&mut d, &reg, &["lead", "a"], "Rhythm");
    flags(&mut d, &reg, outer, true, false);
    assert_eq!(playing(&d), ["b", "c", "d"]);
    flags(&mut d, &reg, outer, false, false);
    flags(&mut d, &reg, inner, true, false);
    assert_eq!(playing(&d), ["synth", "b", "c", "d"]);
    flags(&mut d, &reg, inner, false, true);
    assert_eq!(playing(&d), ["lead", "a"]);
    // Taking a track out of a muted folder frees it.
    flags(&mut d, &reg, inner, true, false);
    run(
        &mut d,
        &reg,
        SongCommand::MoveTracks {
            devices: vec![key("a")],
            place: TrackPlace {
                to_index: 5,
                into: None,
            },
        },
    );
    assert_eq!(playing(&d), ["synth", "b", "c", "d", "a"]);
}

#[test]
fn ungrouping_a_muted_folder_unmutes_its_tracks() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    flags(&mut d, &reg, band, true, false);
    run(&mut d, &reg, SongCommand::Ungroup { group: band });
    assert_eq!(playing(&d).len(), 6);
}

#[test]
fn setting_the_flags_a_folder_has_is_refused_and_so_is_an_unknown_folder() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    rejects(
        &mut d,
        &reg,
        SongCommand::SetGroupFlags {
            group: band,
            muted: false,
            solo: false,
        },
    );
    rejects(
        &mut d,
        &reg,
        SongCommand::SetGroupFlags {
            group: 99,
            muted: true,
            solo: false,
        },
    );
}

#[test]
fn the_flags_are_saved_only_when_set_and_load_back() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"muted\": true"));
    flags(&mut d, &reg, band, true, true);
    let text = save_project(d.doc());
    assert!(text.contains("\"muted\": true, \"solo\": true"), "{text}");
    let loaded = load_project(&text, &reg).unwrap();
    let g = loaded.song.org.group(band).unwrap();
    assert!(g.muted && g.solo);
    assert_eq!(save_project(&loaded), text, "canonical");
    // A file without the fields loads with the folder neither muted nor soloed.
    let back = load_project(&plain, &reg).unwrap();
    let g = back.song.org.group(band).unwrap();
    assert!(!g.muted && !g.solo);
}

// ---- colour -----------------------------------------------------------------------------------

#[test]
fn a_track_without_a_colour_shows_the_nearest_folder_colour() {
    let reg = registry();
    let mut d = doc(&reg);
    let outer = group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    let inner = group(&mut d, &reg, &["lead", "a"], "Rhythm");
    let song = |d: &Document| d.doc().song.clone();
    assert_eq!(song(&d).shown_color(&key("lead")), None);
    run(
        &mut d,
        &reg,
        SongCommand::SetGroupColor {
            group: outer,
            color: Some([1, 2, 3]),
        },
    );
    assert_eq!(song(&d).shown_color(&key("synth")), Some([1, 2, 3]));
    assert_eq!(song(&d).shown_color(&key("lead")), Some([1, 2, 3]));
    assert_eq!(song(&d).shown_color(&key("b")), None, "outside the folder");
    run(
        &mut d,
        &reg,
        SongCommand::SetGroupColor {
            group: inner,
            color: Some([9, 8, 7]),
        },
    );
    assert_eq!(song(&d).shown_color(&key("lead")), Some([9, 8, 7]));
    assert_eq!(song(&d).shown_color(&key("synth")), Some([1, 2, 3]));
    run(
        &mut d,
        &reg,
        SongCommand::SetTrackColor {
            device: key("lead"),
            color: Some([5, 5, 5]),
        },
    );
    assert_eq!(song(&d).shown_color(&key("lead")), Some([5, 5, 5]));
    assert_eq!(song(&d).track_color(&key("a")), None, "its own stays unset");
    assert_eq!(song(&d).shown_color(&key("a")), Some([9, 8, 7]));
}

// ---- moving a folder --------------------------------------------------------------------------

#[test]
fn a_folder_moves_with_its_tracks_to_the_end_and_back() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    let before = d.doc().clone();
    // Four tracks stay: synth, b, c, d.
    move_group(&mut d, &reg, band, 4, None);
    assert_eq!(
        outline(&d),
        ["synth", "b", "c", "d", "[Band", "  lead", "  a"]
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    move_group(&mut d, &reg, band, 0, None);
    assert_eq!(
        outline(&d),
        ["[Band", "  lead", "  a", "synth", "b", "c", "d"]
    );
}

#[test]
fn a_folder_goes_into_another_folder_and_out_again() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    let other = group(&mut d, &reg, &["c", "d"], "Other");
    // Into the folder Other, before its first track (stay: synth, b, c, d).
    move_group(&mut d, &reg, band, 2, Some(other));
    assert_eq!(
        outline(&d),
        [
            "synth", "b", "[Other", "  [Band", "    lead", "    a", "  c", "  d"
        ]
    );
    assert_eq!(d.doc().song.org.group(band).unwrap().parent, Some(other));
    // And out to the top level at the start.
    move_group(&mut d, &reg, band, 0, None);
    assert_eq!(
        outline(&d),
        [
            "[Band", "  lead", "  a", "synth", "b", "[Other", "  c", "  d"
        ]
    );
    assert_eq!(d.doc().song.org.group(band).unwrap().parent, None);
}

#[test]
fn moving_the_folder_into_a_muted_folder_mutes_it() {
    let reg = registry();
    let mut d = doc(&reg);
    let band = group(&mut d, &reg, &["lead", "a"], "Band");
    let other = group(&mut d, &reg, &["c", "d"], "Other");
    flags(&mut d, &reg, other, true, false);
    let ops = d
        .apply(
            sc(SongCommand::MoveGroupTo {
                group: band,
                place: TrackPlace {
                    to_index: 2,
                    into: Some(other),
                },
            }),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(playing(&d), ["synth", "b"]);
}

#[test]
fn a_folder_cannot_go_into_itself_into_the_middle_of_another_or_stay_put() {
    let reg = registry();
    let mut d = doc(&reg);
    let outer = group(&mut d, &reg, &["synth", "lead", "a"], "Band");
    let inner = group(&mut d, &reg, &["lead", "a"], "Rhythm");
    let other = group(&mut d, &reg, &["c", "d"], "Other");
    let at = |to_index, into| SongCommand::MoveGroupTo {
        group: outer,
        place: TrackPlace { to_index, into },
    };
    rejects(&mut d, &reg, at(0, Some(outer)));
    rejects(&mut d, &reg, at(0, Some(inner)));
    rejects(&mut d, &reg, at(0, Some(99)));
    rejects(&mut d, &reg, at(0, None));
    rejects(&mut d, &reg, at(9, None));
    // Between c and d the place is inside Other, not at the top level.
    rejects(&mut d, &reg, at(2, None));
    rejects(
        &mut d,
        &reg,
        SongCommand::MoveGroupTo {
            group: 99,
            place: TrackPlace {
                to_index: 0,
                into: None,
            },
        },
    );
    assert!(d.doc().song.moves_group(
        outer,
        TrackPlace {
            to_index: 3,
            into: None
        }
    ));
    assert!(!d.doc().song.moves_group(
        outer,
        TrackPlace {
            to_index: 0,
            into: None
        }
    ));
    let _ = other;
}

#[test]
fn a_folder_cannot_go_deeper_than_the_depth_limit() {
    let reg = registry();
    let mut d = doc(&reg);
    // Eight levels around track `a`.
    let mut ids = vec![group(&mut d, &reg, &["a", "b", "c", "d"], "L1")];
    for n in 2..=MAX_GROUP_DEPTH {
        ids.push(group(&mut d, &reg, &["a"], &format!("L{n}")));
    }
    // A folder of two levels: `Top` with `Sub` in it.
    let top = group(&mut d, &reg, &["synth", "lead"], "Top");
    group(&mut d, &reg, &["lead"], "Sub");
    let into = |level: usize| TrackPlace {
        to_index: 0,
        into: Some(ids[level - 1]),
    };
    for level in [MAX_GROUP_DEPTH, MAX_GROUP_DEPTH - 1] {
        rejects(
            &mut d,
            &reg,
            SongCommand::MoveGroupTo {
                group: top,
                place: into(level),
            },
        );
    }
    let ok = into(MAX_GROUP_DEPTH - 2);
    assert!(d.doc().song.moves_group(top, ok));
}
