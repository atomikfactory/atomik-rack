//! Track management: moving several tracks to a place (`MoveTracks`), duplicating several tracks
//! (`DuplicateTracks`, with and without a place), and deleting a track together with its device.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Clip, ClipId, Command, Document, Frozen, ModelError, Note, OutlineKind, SongCommand,
    TrackPlace, WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::workflow::{keys_of, one_step, rejected, text, wf};
use common::*;

/// Six tracks in order: `synth`, `lead`, `a`, `b`, `c`, `d`.
fn six(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
        d.apply(add(4 + i, SYNTH, Some(k)), reg).unwrap();
    }
    for k in ["synth", "lead", "a", "b", "c", "d"] {
        d.apply(add_track(k), reg).unwrap();
    }
    d
}

fn order(d: &Document) -> Vec<String> {
    d.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

/// The outline as text: `[id` for a folder header, the key for a track, two spaces per level.
fn outline(d: &Document) -> Vec<String> {
    let song = &d.doc().song;
    song.outline()
        .into_iter()
        .map(|e| {
            let pad = "  ".repeat(e.depth);
            match e.kind {
                OutlineKind::Folder(id) => format!("{pad}[{id}"),
                OutlineKind::Track(i) => format!("{pad}{}", song.tracks[i].device.0),
            }
        })
        .collect()
}

fn move_cmd(devices: &[&str], to_index: usize, into: Option<u32>) -> Command {
    sc(SongCommand::MoveTracks {
        devices: keys_of(devices),
        place: TrackPlace { to_index, into },
    })
}

fn group(d: &mut Document, reg: &Registry, devices: &[&str]) -> u32 {
    d.apply(
        sc(SongCommand::GroupTracks {
            devices: keys_of(devices),
            name: "Folder".into(),
        }),
        reg,
    )
    .unwrap();
    d.doc().song.org.groups.iter().map(|g| g.id).max().unwrap()
}

#[test]
fn several_tracks_move_side_by_side_in_one_undo_step() {
    let reg = registry();
    let mut d = six(&reg);
    one_step(&mut d, &reg, move_cmd(&["c", "a"], 0, None));
    // They keep their own order (a before c), whatever order the command lists them in.
    assert_eq!(order(&d), ["a", "c", "synth", "lead", "b", "d"]);
    d.undo(&reg).unwrap();
    assert_eq!(order(&d), ["synth", "lead", "a", "b", "c", "d"]);
    d.apply(move_cmd(&["synth", "lead"], 4, None), &reg)
        .unwrap();
    assert_eq!(order(&d), ["a", "b", "c", "d", "synth", "lead"]);
}

#[test]
fn a_move_counts_places_among_the_tracks_that_stay() {
    let reg = registry();
    let mut d = six(&reg);
    // `lead` leaves, so place 2 is between a and b (synth, a | b, c, d).
    d.apply(move_cmd(&["lead"], 2, None), &reg).unwrap();
    assert_eq!(order(&d), ["synth", "a", "lead", "b", "c", "d"]);
    let e = rejected(&mut d, &reg, move_cmd(&["lead"], 6, None));
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e}");
    let e = rejected(&mut d, &reg, move_cmd(&[], 0, None));
    assert!(matches!(e, ModelError::BadSong(_)), "{e}");
    let e = rejected(&mut d, &reg, move_cmd(&["nope"], 0, None));
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
    let e = rejected(&mut d, &reg, move_cmd(&["a", "a"], 0, None));
    assert!(e.to_string().contains("twice"), "{e}");
    // Back where it is: nothing to do.
    let e = rejected(&mut d, &reg, move_cmd(&["lead"], 2, None));
    assert!(e.to_string().contains("already"), "{e}");
}

#[test]
fn tracks_join_a_folder_and_leave_it() {
    let reg = registry();
    let mut d = six(&reg);
    let f = group(&mut d, &reg, &["a", "b"]);
    assert_eq!(outline(&d), ["synth", "lead", "[1", "  a", "  b", "c", "d"]);
    // `d` joins the folder after b.
    one_step(&mut d, &reg, move_cmd(&["d"], 4, Some(f)));
    assert_eq!(
        outline(&d),
        ["synth", "lead", "[1", "  a", "  b", "  d", "c"]
    );
    // `synth` and `c` join it at the top: two tracks from two places, side by side.
    d.apply(move_cmd(&["synth", "c"], 1, Some(f)), &reg)
        .unwrap();
    assert_eq!(
        outline(&d),
        ["lead", "[1", "  synth", "  c", "  a", "  b", "  d"]
    );
    // Out again, to the top level at the end of the list.
    one_step(&mut d, &reg, move_cmd(&["a", "d"], 4, None));
    assert_eq!(
        outline(&d),
        ["lead", "[1", "  synth", "  c", "  b", "a", "d"]
    );
    // The folder goes with its last track.
    d.apply(move_cmd(&["synth", "c", "b"], 1, None), &reg)
        .unwrap();
    assert!(d.doc().song.org.groups.is_empty());
}

#[test]
fn a_place_in_the_middle_of_another_folder_is_refused() {
    let reg = registry();
    let mut d = six(&reg);
    group(&mut d, &reg, &["a", "b", "c"]);
    // Between a and b at the top level would cut the folder in two.
    let e = rejected(&mut d, &reg, move_cmd(&["d"], 3, None));
    assert!(e.to_string().contains("folder"), "{e}");
    // So would putting a member of the folder there, out of it.
    let e = rejected(&mut d, &reg, move_cmd(&["a"], 3, None));
    assert!(e.to_string().contains("folder"), "{e}");
    // A folder that does not exist.
    let e = rejected(&mut d, &reg, move_cmd(&["d"], 3, Some(9)));
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
    // The edges of the block are fine, from either side.
    d.apply(move_cmd(&["d"], 2, None), &reg).unwrap();
    assert_eq!(
        outline(&d),
        ["synth", "lead", "d", "[1", "  a", "  b", "  c"]
    );
    d.apply(move_cmd(&["synth"], 5, None), &reg).unwrap();
    assert_eq!(
        outline(&d),
        ["lead", "d", "[1", "  a", "  b", "  c", "synth"]
    );
}

#[test]
fn a_nested_folder_keeps_its_place_when_a_track_joins_the_outer_one() {
    let reg = registry();
    let mut d = six(&reg);
    let outer = group(&mut d, &reg, &["a", "b", "c"]);
    group(&mut d, &reg, &["b"]);
    assert_eq!(
        outline(&d),
        ["synth", "lead", "[1", "  a", "  [2", "    b", "  c", "d"]
    );
    // Into the outer folder after the inner one's track: the track lands outside the inner
    // folder's block, which the outline shows as a member of the outer one only.
    d.apply(move_cmd(&["d"], 5, Some(outer)), &reg).unwrap();
    assert_eq!(
        outline(&d),
        ["synth", "lead", "[1", "  a", "  [2", "    b", "  c", "  d"]
    );
    // Into the inner folder.
    d.apply(move_cmd(&["synth"], 3, Some(2)), &reg).unwrap();
    assert_eq!(
        outline(&d),
        [
            "lead",
            "[1",
            "  a",
            "  [2",
            "    b",
            "    synth",
            "  c",
            "  d"
        ]
    );
}

#[test]
fn a_frozen_track_is_not_moved() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    d.apply(register(1, 480_000), &reg).unwrap();
    for k in ["one", "two", "three"] {
        d.apply(add(0, SYNTH, Some(k)), &reg).unwrap();
        d.apply(add_track(k), &reg).unwrap();
    }
    d.apply(
        sc(SongCommand::FreezeTrack {
            device: key("two"),
            frozen: Frozen {
                start: 0,
                length: 2 * BAR,
                audio: audio(1),
                effects: Vec::new(),
            },
        }),
        &reg,
    )
    .unwrap();
    let e = rejected(&mut d, &reg, move_cmd(&["one", "two"], 1, None));
    assert!(matches!(e, ModelError::Frozen(_)), "{e}");
    let e = rejected(
        &mut d,
        &reg,
        wf(WorkflowCommand::DuplicateTracks {
            devices: keys_of(&["two"]),
            place: None,
        }),
    );
    assert!(matches!(e, ModelError::Frozen(_)), "{e}");
    // The ones that are not frozen move as before.
    d.apply(move_cmd(&["three"], 0, None), &reg).unwrap();
    assert_eq!(order(&d), ["three", "one", "two"]);
}

// ---------------------------------------------------------------- duplicate

/// A song with `n` synth tracks (each with a console channel and a note clip), the first one
/// armed.
fn tracks(reg: &Registry, n: usize) -> Document {
    let mut d = Document::new(empty_doc());
    for i in 0..n {
        let k = format!("t{i}");
        d.apply(add(usize::MAX, SYNTH, Some(&k)), reg).unwrap();
        d.apply(add_track(&k), reg).unwrap();
        d.apply(
            add_clip(
                &k,
                0,
                Clip {
                    id: ClipId(0),
                    ..notes_clip(0, 2 * BAR, vec![Note::new(0, 480, 60 + i as u8, 0.8)])
                },
            ),
            reg,
        )
        .unwrap();
    }
    d
}

fn duplicate(devices: &[&str], place: Option<TrackPlace>) -> Command {
    wf(WorkflowCommand::DuplicateTracks {
        devices: keys_of(devices),
        place,
    })
}

#[test]
fn duplicating_three_tracks_is_one_undo_step_and_the_copies_are_not_armed() {
    let reg = console_registry();
    let mut d = tracks(&reg, 4);
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("t0"),
            muted: false,
            solo: false,
            armed: true,
        }),
        &reg,
    )
    .unwrap();
    let before = d.doc().song.tracks.len();
    let channels = d.doc().console.channels.len();
    one_step(&mut d, &reg, duplicate(&["t2", "t0", "t1"], None));
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), before + 3);
    assert_eq!(doc.console.channels.len(), channels + 3);
    let copies: Vec<_> = doc
        .song
        .tracks
        .iter()
        .filter(|t| !["t0", "t1", "t2", "t3"].contains(&t.device.0.as_str()))
        .collect();
    assert_eq!(copies.len(), 3);
    assert!(copies.iter().all(|t| !t.armed), "the copies are not armed");
    assert!(
        copies
            .iter()
            .all(|t| t.lanes.iter().map(|l| l.clips.len()).sum::<usize>() == 1),
        "the clips come with them"
    );
    // The original stays armed; the copies sit together right after the last original (t2).
    assert!(doc.song.track(&key("t0")).unwrap().armed);
    let o = order(&d);
    assert_eq!(&o[..3], ["t0", "t1", "t2"]);
    assert_eq!(o[6], "t3");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks.len(), before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks.len(), before + 3);
}

#[test]
fn duplicating_tracks_with_a_place_puts_the_copies_there_together() {
    let reg = console_registry();
    let mut d = tracks(&reg, 3);
    // A folder around t1 and t2: the copies of t0 and t1 go to the top level, last.
    group(&mut d, &reg, &["t1", "t2"]);
    let place = TrackPlace {
        to_index: 3,
        into: None,
    };
    one_step(&mut d, &reg, duplicate(&["t0", "t1"], Some(place)));
    let o = order(&d);
    assert_eq!(o.len(), 5);
    assert_eq!(&o[..3], ["t0", "t1", "t2"], "the originals stay");
    let o2 = outline(&d);
    // The copies come last, outside the folder.
    assert!(
        o2[o2.len() - 2..].iter().all(|s| !s.starts_with(' ')),
        "{o2:?}"
    );
    // A place that the originals' folder cannot hold is refused as a whole.
    let e = rejected(
        &mut d,
        &reg,
        duplicate(
            &["t0"],
            Some(TrackPlace {
                to_index: 2,
                into: None,
            }),
        ),
    );
    assert!(e.to_string().contains("folder"), "{e}");
    let e = rejected(&mut d, &reg, duplicate(&[], None));
    assert!(matches!(e, ModelError::BadSong(_)), "{e}");
    let e = rejected(&mut d, &reg, duplicate(&["t0", "t0"], None));
    assert!(e.to_string().contains("twice"), "{e}");
}

#[test]
fn deleting_a_track_with_its_device_takes_the_channel_and_undo_restores_everything() {
    let reg = console_registry();
    let mut d = tracks(&reg, 3);
    let before = text(&d);
    let channels = d.doc().console.channels.len();
    d.apply(
        wf(WorkflowCommand::DeleteDevices {
            keys: keys_of(&["t1", "t2"]),
        }),
        &reg,
    )
    .unwrap();
    let doc = d.doc();
    assert_eq!(order(&d), ["t0"]);
    assert_eq!(doc.console.channels.len(), channels - 2);
    assert!(
        doc.console
            .channels
            .iter()
            .all(|c| c.source.as_ref().is_none_or(|s| s.0 == "t0")),
        "no channel plays a deleted track"
    );
    assert!(doc.device(&key("t1")).is_none() && doc.device(&key("t2")).is_none());
    d.undo(&reg).unwrap();
    assert_eq!(text(&d), before, "clips, channels and cables are back");
}

#[test]
fn a_track_colour_and_its_device_colour_change_in_one_undo_step() {
    let reg = registry();
    let mut d = six(&reg);
    let colour = [200, 40, 40];
    let batch = Command::Batch(vec![
        sc(SongCommand::SetTrackColor {
            device: key("a"),
            color: Some(colour),
        }),
        Command::SetColor {
            key: key("a"),
            color: Some(colour),
        },
    ]);
    d.apply(batch, &reg).unwrap();
    assert_eq!(d.doc().song.track_color(&key("a")), Some(colour));
    assert_eq!(d.doc().device(&key("a")).unwrap().color, Some(colour));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.track_color(&key("a")), None);
    assert_eq!(d.doc().device(&key("a")).unwrap().color, None);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().device(&key("a")).unwrap().color, Some(colour));
}

#[test]
fn the_song_says_whether_a_drop_would_be_taken() {
    let reg = registry();
    let mut d = six(&reg);
    group(&mut d, &reg, &["a", "b", "c"]);
    let song = &d.doc().song;
    let at = |to_index, into| TrackPlace { to_index, into };
    // `d` to the front or back of the folder block is fine, into the middle at top level not.
    assert!(song.moves_tracks(&keys_of(&["d"]), at(2, None)));
    assert!(!song.moves_tracks(&keys_of(&["d"]), at(3, None)));
    assert!(song.moves_tracks(&keys_of(&["d"]), at(3, Some(1))));
    // Where it is already: nothing to do.
    assert!(!song.moves_tracks(&keys_of(&["d"]), at(5, None)));
    // Copies count every track of the song: after d, or inside the folder, but not between.
    assert!(song.accepts_copies_at(at(6, None)));
    assert!(song.accepts_copies_at(at(3, Some(1))));
    assert!(!song.accepts_copies_at(at(3, None)));
    assert!(!song.accepts_copies_at(at(7, None)));
}
