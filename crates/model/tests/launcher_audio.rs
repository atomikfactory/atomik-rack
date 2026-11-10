//! Audio slots of the clip launcher: an audio clip in the slot of an audio track, kinds that do not
//! fit refused, the file bookkeeping (usage, relink, undo history), the saved shape and a slot
//! clip put in the arrangement on another track.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, Command, Document, LauncherClip, LauncherCommand, SongCommand, load_project,
    save_project,
};
use common::audio::*;
use common::song::*;
use common::*;

fn lc(c: LauncherCommand) -> Command {
    sc(SongCommand::Launcher(c))
}

/// An audio track `audio-track` with assets 1 and 2, and a note track `synth`.
fn doc(reg: &Registry) -> (Document, String) {
    let mut d = with_audio_track(reg);
    d.apply(add(0, SYNTH, Some("synth")), reg).unwrap();
    d.apply(add_track("synth"), reg).unwrap();
    let audio = d
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| t.is_audio())
        .unwrap()
        .device
        .0
        .clone();
    (d, audio)
}

fn audio_clip(n: u8) -> LauncherClip {
    LauncherClip {
        clip: clip(0, 15_360 * 2, ClipContent::Audio(audio(n))),
        quantize: None,
    }
}

fn put(d: &mut Document, reg: &Registry, track: &str, row: usize, n: u8) {
    d.apply(
        lc(LauncherCommand::SetSlot {
            device: key(track),
            row,
            clip: audio_clip(n),
        }),
        reg,
    )
    .unwrap();
}

#[test]
fn an_audio_clip_goes_in_the_slot_of_an_audio_track_and_nowhere_else() {
    let reg = audio_registry();
    let (mut d, audio_track) = doc(&reg);
    put(&mut d, &reg, &audio_track, 0, 1);
    let s = d.doc().song.launcher.slot(&key(&audio_track), 0).unwrap();
    assert!(s.clip.content.is_audio() && s.clip.start == 0);
    // A note track takes no audio clip, an audio track no note clip, and neither is made empty.
    d.apply(
        lc(LauncherCommand::SetSlot {
            device: key("synth"),
            row: 0,
            clip: audio_clip(1),
        }),
        &reg,
    )
    .expect_err("audio on a note track");
    d.apply(
        lc(LauncherCommand::SetSlot {
            device: key(&audio_track),
            row: 1,
            clip: LauncherClip {
                clip: notes_clip(0, 15_360, vec![note(0, 100, 60, 0.5)]),
                quantize: None,
            },
        }),
        &reg,
    )
    .expect_err("notes on an audio track");
    d.apply(
        lc(LauncherCommand::NewClip {
            device: key(&audio_track),
            row: 1,
            length: 15_360 * 4,
        }),
        &reg,
    )
    .expect_err("an audio slot needs a file");
    d.apply(
        lc(LauncherCommand::MoveSlot {
            from: (key(&audio_track), 0),
            to: (key("synth"), 0),
            copy: false,
        }),
        &reg,
    )
    .expect_err("an audio slot does not move to a note track");
    d.doc().validate().unwrap();
}

#[test]
fn a_slot_keeps_its_file_in_use_through_save_prune_undo_and_relink() {
    let reg = audio_registry();
    let (mut d, audio_track) = doc(&reg);
    put(&mut d, &reg, &audio_track, 0, 1);
    // The file is used by the slot alone.
    let usage = d.doc().asset_usage();
    assert_eq!(usage[&hash(1)].clips, 1);
    assert!(d.doc().referenced_assets().contains(&hash(1)));
    let mut pruned = d.doc().clone();
    pruned.prune_unreferenced_assets();
    assert!(pruned.assets.contains_key(&hash(1)) && !pruned.assets.contains_key(&hash(2)));
    // The saved project keeps it and loads back the same.
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.launcher, d.doc().song.launcher);
    assert_eq!(save_project(&loaded), text);
    // An undone slot clip is still needed by the history: the asset cannot go.
    d.undo().unwrap();
    assert!(d.referenced_assets().contains(&hash(1)));
    d.redo().unwrap();
    // Relinking the file moves the slot to the replacement in one step.
    let depth = d.undo_depth();
    d.apply(
        Command::RelinkAsset {
            from: hash(1),
            to: hash(2),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    let s = d.doc().song.launcher.slot(&key(&audio_track), 0).unwrap();
    assert!(matches!(&s.clip.content, ClipContent::Audio(a) if a.asset == hash(2)));
    d.undo().unwrap();
    let s = d.doc().song.launcher.slot(&key(&audio_track), 0).unwrap();
    assert!(matches!(&s.clip.content, ClipContent::Audio(a) if a.asset == hash(1)));
}

#[test]
fn a_slot_clip_goes_to_the_arrangement_on_its_own_track_or_another_of_its_kind() {
    let reg = audio_registry();
    let (mut d, audio_track) = doc(&reg);
    d.apply(add_audio_track(), &reg).unwrap();
    let other = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.is_audio())
        .map(|t| t.device.0.clone())
        .find(|k| *k != audio_track)
        .unwrap();
    put(&mut d, &reg, &audio_track, 0, 1);
    d.apply(
        lc(LauncherCommand::ToArrangement {
            device: key(&audio_track),
            row: 0,
            start: 15_360 * 4,
            onto: Some(key(&other)),
        }),
        &reg,
    )
    .unwrap();
    let lane = &d.doc().song.track(&key(&other)).unwrap().lanes[0];
    assert_eq!(lane.clips.len(), 1);
    assert_eq!(lane.clips[0].start, 15_360 * 4);
    d.apply(
        lc(LauncherCommand::ToArrangement {
            device: key(&audio_track),
            row: 0,
            start: 0,
            onto: Some(key("synth")),
        }),
        &reg,
    )
    .expect_err("audio on a note track");
    d.undo();
    d.doc().validate().unwrap();
}
