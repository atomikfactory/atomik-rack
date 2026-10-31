//! Track organisation never reaches the audio: folders, colours, comments, folds, moving tracks
//! and a project saved and loaded with them bounce the same bytes as the plain song, and muting
//! the tracks of a folder in one batch (what the arrangement's folder button sends) is muting
//! each of them.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand, empty_doc,
    load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const CUTOFF: &str = "filter.cutoff";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn song_cmd(c: SongCommand) -> Command {
    Command::Song(c)
}

/// Three synth tracks, each with a one-bar note on its own key, and a cutoff lane on `bass`.
fn song() -> Document {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    for (k, midi) in [("lead", 72u8), ("pad", 64), ("bass", 36)] {
        let length = 4 * PPQ;
        for c in [
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            },
            song_cmd(SongCommand::AddTrack { device: key(k) }),
            song_cmd(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    controllers: Vec::new(),
                    name: None,
                    color: None,
                    grid: None,
                    content: ClipContent::Notes(vec![Note::new(0, length, midi, 0.8)].into()),
                },
            }),
        ] {
            d.apply(c.clone(), &registry)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }
    for c in [
        song_cmd(SongCommand::AddAutomationLane {
            device: key("bass"),
            param: CUTOFF.into(),
        }),
        song_cmd(SongCommand::SetSongEnd(4 * PPQ)),
    ] {
        d.apply(c, &registry).unwrap();
    }
    d
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn apply(d: &mut Document, c: SongCommand) {
    d.apply(song_cmd(c), &default_registry()).unwrap();
}

#[test]
fn organising_the_tracks_does_not_change_the_bounce() {
    let mut d = song();
    let plain = bounce(d.doc(), 512, 0);
    assert!(
        plain.iter().any(|s| s.abs() > 0.01),
        "the song is not silent"
    );
    apply(
        &mut d,
        SongCommand::GroupTracks {
            devices: vec![key("lead"), key("bass")],
            name: "Outer".into(),
        },
    );
    apply(
        &mut d,
        SongCommand::GroupTracks {
            devices: vec![key("bass")],
            name: "Inner".into(),
        },
    );
    for c in [
        SongCommand::SetTrackColor {
            device: key("pad"),
            color: Some([200, 20, 20]),
        },
        SongCommand::SetTrackComment {
            device: key("lead"),
            comment: "a comment".into(),
        },
        SongCommand::SetLaneFolded {
            device: key("bass"),
            param: CUTOFF.into(),
            folded: true,
        },
        SongCommand::SetGroupCollapsed {
            group: 1,
            collapsed: true,
        },
        SongCommand::MoveTrack {
            device: key("pad"),
            down: false,
        },
        SongCommand::MoveTrack {
            device: key("pad"),
            down: true,
        },
        SongCommand::MoveGroup {
            group: 1,
            down: true,
        },
    ] {
        apply(&mut d, c);
    }
    for (block, workers) in [(512, 0), (333, 0), (512, 2)] {
        assert_eq!(
            bounce(d.doc(), block, workers),
            plain,
            "{block} / {workers}"
        );
    }
    // And the file that carries it.
    let text = save_project(d.doc());
    assert!(text.contains("\"groups\"") && text.contains("\"comment\""));
    let loaded = load_project(&text, &default_registry()).unwrap();
    assert_eq!(bounce(&loaded, 512, 0), plain);
    // Taking everything apart again is the plain song too.
    // Two groups and seven more commands.
    for _ in 0..9 {
        d.undo(&default_registry()).unwrap();
    }
    assert_eq!(bounce(d.doc(), 512, 0), plain);
}

#[test]
fn muting_every_track_of_a_folder_is_muting_each_track() {
    let mut folder = song();
    apply(
        &mut folder,
        SongCommand::GroupTracks {
            devices: vec![key("lead"), key("pad")],
            name: "Top".into(),
        },
    );
    let flags: Vec<Command> = ["lead", "pad"]
        .into_iter()
        .map(|k| {
            song_cmd(SongCommand::SetTrackFlags {
                device: key(k),
                muted: true,
                solo: false,
                armed: false,
            })
        })
        .collect();
    folder
        .apply(Command::Batch(flags.clone()), &default_registry())
        .unwrap();
    let mut each = song();
    for f in flags {
        each.apply(f, &default_registry()).unwrap();
    }
    let (a, b) = (bounce(folder.doc(), 512, 0), bounce(each.doc(), 512, 0));
    assert_eq!(a, b);
    assert!(a.iter().any(|s| s.abs() > 0.01), "the bass still plays");
    // One undo step brings the folder's tracks back.
    folder.undo(&default_registry()).unwrap();
    assert!(
        folder.doc().song.tracks.iter().all(|t| !t.muted),
        "one batch is one undo step"
    );
}

#[test]
fn moving_tracks_to_a_place_does_not_change_the_bounce() {
    use atomik_model::TrackPlace;
    let mut d = song();
    let plain = bounce(d.doc(), 512, 0);
    apply(
        &mut d,
        SongCommand::GroupTracks {
            devices: vec![key("pad"), key("bass")],
            name: "Low".into(),
        },
    );
    // `lead` into the folder, then two tracks together to the front, then out again.
    for (devices, to_index, into) in [
        (vec![key("lead")], 0, Some(1)),
        (vec![key("bass"), key("lead")], 1, None),
        (vec![key("pad")], 0, None),
    ] {
        apply(
            &mut d,
            SongCommand::MoveTracks {
                devices,
                place: TrackPlace { to_index, into },
            },
        );
        for (block, workers) in [(512, 0), (333, 0), (512, 2)] {
            assert_eq!(
                bounce(d.doc(), block, workers),
                plain,
                "{block} / {workers}"
            );
        }
    }
    let order: Vec<&str> = d
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.as_str())
        .collect();
    assert_ne!(order, ["lead", "pad", "bass"], "the tracks did move");
}
