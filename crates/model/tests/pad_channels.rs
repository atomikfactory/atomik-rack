//! `ConsoleCommand::AddChannelFor` (fixes): a console channel fed by chosen
//! outputs of a source (a drum pad's individual outputs), several channels per source, and
//! `Command::Batch` with `AddTrack` / `AddClip`.

mod common;

use atomik_model::{
    ClipContent, Command, ConsoleChannel, ConsoleCommand, Document, ModelError, SongCommand,
    SyncOp, empty_doc, load_project, save_project,
};
use common::song::{add_clip, add_track, notes_clip, sc};
use common::*;

const DRUMS: &str = "atomik.drum-machine";

fn pad_channel(source: &str, pad: u8, name: Option<&str>) -> Command {
    Command::Console(ConsoleCommand::AddChannelFor {
        source: key(source),
        out_l: format!("pad{pad}_out_l"),
        out_r: format!("pad{pad}_out_r"),
        name: name.map(str::to_owned),
    })
}

fn drums_doc() -> (Document, atomik_device_sdk::Registry) {
    let reg = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, DRUMS, Some("drums")), &reg).unwrap();
    (d, reg)
}

#[test]
fn add_channel_for_cables_names_and_is_one_undo_step() {
    let (mut d, reg) = drums_doc();
    let before = d.doc().clone();
    let main = d.doc().console.channels[0].clone();
    d.apply(pad_channel("drums", 2, None), &reg).unwrap();
    let doc = d.doc();
    assert_eq!(doc.console.channels.len(), 2);
    assert_eq!(
        doc.console.channels[0], main,
        "the main channel stays first"
    );
    let ConsoleChannel { channel, source } = doc.console.channels[1].clone();
    assert_eq!(source, Some(key("drums")));
    assert_eq!(doc.device(&channel).unwrap().name, "Drum machine Pad 2");
    assert!(
        doc.cables
            .contains(&cable("drums:pad2_out_l", &format!("{channel}:in_l")))
    );
    assert!(
        doc.cables
            .contains(&cable("drums:pad2_out_r", &format!("{channel}:in_r")))
    );
    let master = doc.console.master.clone().unwrap();
    assert!(doc.cables.contains(&cable(
        &format!("{channel}:out_l"),
        &format!("{master}:bus_l")
    )));
    assert!(doc.cables.contains(&cable(
        &format!("{channel}:send1_l"),
        &format!("{master}:send_bus1_l")
    )));
    // One undo step, exact; redo restores the same document.
    let after = doc.clone();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc(), &after);
}

#[test]
fn add_channel_for_uses_the_given_name() {
    let (mut d, reg) = drums_doc();
    d.apply(pad_channel("drums", 1, Some("Kick")), &reg)
        .unwrap();
    let ch = d.doc().console.channels[1].channel.clone();
    assert_eq!(d.doc().device(&ch).unwrap().name, "Kick");
}

#[test]
fn add_channel_for_rejects_bad_sockets_and_cabled_outputs() {
    let (mut d, reg) = drums_doc();
    let bad = |out_l: &str, out_r: &str, source: &str| {
        Command::Console(ConsoleCommand::AddChannelFor {
            source: key(source),
            out_l: out_l.into(),
            out_r: out_r.into(),
            name: None,
        })
    };
    for cmd in [
        bad("nope", "pad1_out_r", "drums"),
        bad("pad1_out_l", "nope", "drums"),
        bad("pad1_out_l", "pad1_out_r", "ghost"),
        bad("pad1_out_l", "pad1_out_r", "master"),
        // An input is not an output.
        bad("pad1_out_l", "midi_in", "drums"),
    ] {
        let before = d.doc().clone();
        assert!(d.apply(cmd.clone(), &reg).is_err(), "{cmd:?}");
        assert_eq!(d.doc(), &before);
    }
    // A socket that is not an audio output is refused by the command itself (BadConsole), not
    // by a later cable check.
    for (l, r) in [
        ("nope", "pad1_out_r"),
        ("pad1_out_l", "nope"),
        ("pad1_out_l", "midi_in"),
    ] {
        let e = d.apply(bad(l, r, "drums"), &reg).unwrap_err();
        assert!(matches!(e, ModelError::BadConsole(_)), "{l} {r}: {e:?}");
    }
    d.apply(pad_channel("drums", 1, None), &reg).unwrap();
    let before = d.doc().clone();
    let e = d.apply(pad_channel("drums", 1, None), &reg).unwrap_err();
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    assert_eq!(d.doc(), &before);
    // The main outputs are cabled by the automatic channel.
    let e = d
        .apply(
            Command::Console(ConsoleCommand::AddChannelFor {
                source: key("drums"),
                out_l: "out_l".into(),
                out_r: "out_r".into(),
                name: None,
            }),
            &reg,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
}

#[test]
fn solo_with_two_channels_of_one_source() {
    let (mut d, reg) = drums_doc();
    d.apply(pad_channel("drums", 1, None), &reg).unwrap();
    let main = d.doc().console.channels[0].channel.clone();
    let pad = d.doc().console.channels[1].channel.clone();
    d.apply(
        Command::SetParam {
            key: pad.clone(),
            param: "solo".into(),
            value: 1.0,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    assert_eq!(
        d.doc().solo_silence(&reg),
        [(main, true), (pad, false)],
        "the main channel of the same source is silenced"
    );
}

#[test]
fn removing_the_source_removes_every_channel_and_undo_restores_them() {
    let (mut d, reg) = drums_doc();
    d.apply(pad_channel("drums", 1, None), &reg).unwrap();
    d.apply(pad_channel("drums", 3, None), &reg).unwrap();
    let before = d.doc().clone();
    assert_eq!(before.console.channels.len(), 3);
    d.apply(Command::RemoveDevice { key: key("drums") }, &reg)
        .unwrap();
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(keys(&d), ["master"]);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn channels_of_one_source_survive_a_save_load_round_trip() {
    let (mut d, reg) = drums_doc();
    d.apply(pad_channel("drums", 2, None), &reg).unwrap();
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
}

#[test]
fn batch_accepts_add_track_and_add_clip_as_one_step() {
    let (mut d, reg) = drums_doc();
    let before = d.doc().clone();
    let clip = notes_clip(0, 1920, Vec::new());
    let ops = d
        .apply(
            Command::Batch(vec![
                add_track("drums"),
                add_clip("drums", 0, clip),
                sc(SongCommand::SetClipGrid {
                    clip: atomik_model::ClipId(1),
                    grid: None,
                    merge: None,
                }),
            ]),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(d.doc().song.tracks.len(), 1);
    assert_eq!(d.last_clip_id(), Some(atomik_model::ClipId(1)));
    assert!(matches!(
        d.doc().song.tracks[0].lanes[0].clips[0].content,
        ClipContent::Notes(_)
    ));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn batch_with_a_failing_song_command_changes_nothing() {
    let (mut d, reg) = drums_doc();
    let before = d.doc().clone();
    let r = d.apply(
        Command::Batch(vec![add_track("drums"), add_track("drums")]),
        &reg,
    );
    assert!(r.is_err());
    assert_eq!(d.doc(), &before);
}
