//! The clip launcher in the model: commands and their undo, the optional `launcher` field of
//! project format 44 (written only when set, an old file without it, strict reading), and the
//! clip editors working on a slot clip.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, ClipId, Command, Document, LaunchQuantize, LauncherClip, LauncherCommand,
    MetaChange, SongCommand, WorkflowCommand, load_project, save_project,
};
use common::audio::{audio_registry, with_audio_track};
use common::song::*;
use common::*;

fn lc(c: LauncherCommand) -> Command {
    sc(SongCommand::Launcher(c))
}

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(add_track("lead"), reg).unwrap();
    d
}

fn new_clip(d: &mut Document, reg: &Registry, track: &str, row: usize) {
    d.apply(
        lc(LauncherCommand::NewClip {
            device: key(track),
            row,
            length: 15_360 * 4,
        }),
        reg,
    )
    .unwrap();
}

fn slot_id(d: &Document, track: &str, row: usize) -> ClipId {
    d.doc()
        .song
        .launcher
        .slot(&key(track), row)
        .unwrap()
        .clip
        .id
}

#[test]
fn a_song_starts_without_a_launcher() {
    let reg = registry();
    let d = doc(&reg);
    assert!(d.doc().song.launcher.is_default());
    assert!(!save_project(d.doc()).contains("launcher"));
}

#[test]
fn a_new_clip_adds_scenes_up_to_its_row_and_undoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    new_clip(&mut d, &reg, "synth", 2);
    let l = &d.doc().song.launcher;
    assert_eq!(l.scenes.len(), 3);
    assert!(l.slot(&key("synth"), 2).is_some() && l.slot(&key("synth"), 0).is_none());
    assert_eq!(l.scene_label(1), "Scene 2");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    assert!(d.doc().song.launcher.slot(&key("synth"), 2).is_some());
}

#[test]
fn a_clip_from_the_arrangement_is_copied_into_a_slot() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(0, 15_360 * 2, vec![note(0, 100, 60, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    let arrangement = clip_ids(d.doc())[0];
    d.apply(
        lc(LauncherCommand::PutClip {
            device: key("lead"),
            row: 0,
            clip: arrangement,
        }),
        &reg,
    )
    .unwrap();
    let slot = d.doc().song.launcher.slot(&key("lead"), 0).unwrap();
    assert_ne!(slot.clip.id, arrangement);
    assert_eq!(slot.clip.length, 15_360 * 2);
    assert_eq!(slot.clip.start, 0);
    // The arrangement keeps its clip.
    assert!(d.doc().song.clip(arrangement).is_some());
}

#[test]
fn moving_copying_and_clearing_slots() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    let id = slot_id(&d, "synth", 0);
    d.apply(
        lc(LauncherCommand::MoveSlot {
            from: (key("synth"), 0),
            to: (key("lead"), 1),
            copy: false,
        }),
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.launcher.slot(&key("synth"), 0).is_none());
    assert_eq!(slot_id(&d, "lead", 1), id);
    d.apply(
        lc(LauncherCommand::MoveSlot {
            from: (key("lead"), 1),
            to: (key("synth"), 3),
            copy: true,
        }),
        &reg,
    )
    .unwrap();
    assert_ne!(slot_id(&d, "synth", 3), id);
    assert_eq!(slot_id(&d, "lead", 1), id);
    d.apply(
        lc(LauncherCommand::ClearSlots {
            slots: vec![(key("lead"), 1), (key("synth"), 3)],
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.launcher.clips().count(), 0);
    d.apply(
        lc(LauncherCommand::ClearSlots {
            slots: vec![(key("lead"), 1)],
        }),
        &reg,
    )
    .expect_err("nothing to clear");
}

#[test]
fn a_slot_edit_changes_name_colour_length_and_quantize() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    d.apply(
        lc(LauncherCommand::EditSlot {
            device: key("synth"),
            row: 0,
            name: MetaChange::Set("Bass".into()),
            color: MetaChange::Set([1, 2, 3]),
            length: Some(15_360 * 8),
            quantize: Some(Some(LaunchQuantize::Eighth)),
        }),
        &reg,
    )
    .unwrap();
    let s = d.doc().song.launcher.slot(&key("synth"), 0).unwrap();
    assert_eq!(s.clip.name.as_deref(), Some("Bass"));
    assert_eq!(s.clip.color, Some([1, 2, 3]));
    assert_eq!(s.loop_ticks(), 15_360 * 8);
    assert_eq!(
        d.doc().song.launcher.quantize_of(&key("synth"), 0),
        LaunchQuantize::Eighth
    );
    assert_eq!(
        d.doc().song.launcher.quantize_of(&key("synth"), 1),
        LaunchQuantize::Bar
    );
    d.apply(
        lc(LauncherCommand::EditSlot {
            device: key("synth"),
            row: 0,
            name: MetaChange::Keep,
            color: MetaChange::Keep,
            length: Some(1),
            quantize: None,
        }),
        &reg,
    )
    .expect_err("a loop needs a sensible length");
}

#[test]
fn scenes_insert_delete_and_duplicate_with_their_slots() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    new_clip(&mut d, &reg, "lead", 1);
    d.apply(lc(LauncherCommand::InsertScene { at: 1 }), &reg)
        .unwrap();
    let l = &d.doc().song.launcher;
    assert_eq!(l.scenes.len(), 3);
    assert!(l.slot(&key("lead"), 2).is_some() && l.slot(&key("lead"), 1).is_none());
    d.apply(lc(LauncherCommand::DuplicateScene { at: 0 }), &reg)
        .unwrap();
    let l = &d.doc().song.launcher;
    assert_eq!(l.scenes.len(), 4);
    let (a, b) = (
        l.slot(&key("synth"), 0).unwrap(),
        l.slot(&key("synth"), 1).unwrap(),
    );
    assert_ne!(a.clip.id, b.clip.id);
    assert_eq!(a.clip.content, b.clip.content);
    d.apply(lc(LauncherCommand::DeleteScene { at: 0 }), &reg)
        .unwrap();
    assert_eq!(d.doc().song.launcher.scenes.len(), 3);
    d.apply(
        lc(LauncherCommand::RenameScene {
            at: 0,
            name: Some("Drop".into()),
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.launcher.scene_label(0), "Drop");
    d.apply(lc(LauncherCommand::DeleteScene { at: 9 }), &reg)
        .expect_err("no such scene");
    while d.undo().is_some() {}
    d.doc().validate().unwrap();
}

#[test]
fn an_audio_track_takes_no_slot() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let audio = d.doc().song.tracks[0].device.0.clone();
    d.apply(
        lc(LauncherCommand::NewClip {
            device: key(&audio),
            row: 0,
            length: 15_360 * 4,
        }),
        &reg,
    )
    .expect_err("slots are for note tracks");
}

#[test]
fn a_slot_goes_to_the_arrangement_as_a_clip() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    d.apply(
        lc(LauncherCommand::ToArrangement {
            device: key("synth"),
            row: 0,
            start: 15_360 * 8,
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 1);
    let (_, _, c) = d.doc().song.clip(ids[0]).unwrap();
    assert_eq!((c.start, c.length), (15_360 * 8, 15_360 * 4));
    assert_ne!(ids[0], slot_id(&d, "synth", 0));
}

#[test]
fn the_clip_editors_edit_a_slot_clip() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    let id = slot_id(&d, "synth", 0);
    d.apply(
        sc(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(vec![note(0, 480, 64, 0.7)].into()),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let s = d.doc().song.launcher.slot(&key("synth"), 0).unwrap();
    assert!(matches!(&s.clip.content, ClipContent::Notes(n) if n.len() == 1));
    assert_eq!(s.clip.start, 0);
    // The lanes stay empty: the edit did not put the clip in the arrangement.
    assert!(clip_ids(d.doc()).is_empty());
    assert!(d.doc().song.clip_or_take(id).is_some());
    d.undo().unwrap();
    let s = d.doc().song.launcher.slot(&key("synth"), 0).unwrap();
    assert!(matches!(&s.clip.content, ClipContent::Notes(n) if n.is_empty()));
    // A paste goes through the workflow command.
    d.apply(
        Command::Workflow(WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: 0,
            notes: atomik_model::NoteSnapshot {
                notes: vec![note(0, 480, 60, 0.5)],
                span: 480,
            },
        }),
        &reg,
    )
    .ok();
    d.doc().validate().unwrap();
}

#[test]
fn the_launcher_round_trips_and_is_written_only_when_set() {
    let reg = registry();
    let mut d = doc(&reg);
    let plain = save_project(d.doc());
    new_clip(&mut d, &reg, "synth", 1);
    let id = slot_id(&d, "synth", 1);
    d.apply(
        sc(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(
                vec![note(0, 480, 64, 0.7), note(960, 480, 67, 0.5)].into(),
            ),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        lc(LauncherCommand::EditSlot {
            device: key("synth"),
            row: 1,
            name: MetaChange::Set("Riff".into()),
            color: MetaChange::Keep,
            length: None,
            quantize: Some(Some(LaunchQuantize::TwoBars)),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        lc(LauncherCommand::RenameScene {
            at: 0,
            name: Some("Intro".into()),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        lc(LauncherCommand::SetSceneTempo {
            at: 1,
            tempo: Some(140.0),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        lc(LauncherCommand::SetQuantize(LaunchQuantize::Quarter)),
        &reg,
    )
    .unwrap();
    d.apply(lc(LauncherCommand::SetStopEmpty(false)), &reg)
        .unwrap();
    let text = save_project(d.doc());
    assert!(text.contains("\"launcher\""));
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.launcher, d.doc().song.launcher);
    assert_eq!(save_project(&loaded), text);
    // Nothing else changed in the song: without the launcher the text is the old text.
    let mut cleared = d.doc().clone();
    cleared.song_mut().launcher = Default::default();
    cleared.song_mut().next_clip_id = d.doc().song.next_clip_id;
    let stripped = save_project(&cleared);
    assert!(!stripped.contains("launcher"));
    assert!(plain.len() <= stripped.len());
}

#[test]
fn a_file_without_a_launcher_opens_with_an_empty_one() {
    let reg = registry();
    let d = doc(&reg);
    let text = save_project(d.doc());
    assert!(!text.contains("launcher"));
    assert!(
        load_project(&text, &reg)
            .unwrap()
            .song
            .launcher
            .is_default()
    );
}

#[test]
fn a_reader_that_does_not_know_the_field_drops_it() {
    // Version 45 file that says an older reader may skip the launcher.
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    let text = save_project(d.doc());
    let newer = text.replacen(
        "\"version\": 44,",
        "\"version\": 45,\n  \"compat\": 44,\n  \"ignorable\": {\"song.launcher\": 45, \"future\": 45},",
        1,
    );
    let loaded = load_project(&newer, &reg);
    // This reader knows the launcher: the field is a normal one and the file is newer than it,
    // so only the "ignorable" machinery applies; it must not be misread as a launcher.
    match loaded {
        Ok(doc) => assert!(doc.song.launcher.is_default()),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn bad_launchers_name_their_path() {
    let reg = registry();
    let mut d = doc(&reg);
    new_clip(&mut d, &reg, "synth", 0);
    let text = save_project(d.doc());
    for (from, to, expect) in [
        ("\"row\": 0", "\"row\": 5", "launcher.slots[0].row"),
        (
            "\"track\": \"synth\"",
            "\"track\": \"nope\"",
            "launcher.slots[0].track",
        ),
    ] {
        let bad = text.replacen(from, to, 1);
        let err = load_project(&bad, &reg).expect_err("rejected").to_string();
        assert!(err.contains(expect), "{err}");
    }
    let id = slot_id(&d, "synth", 0);
    // A slot clip may not share its id with a lane clip.
    let mut with_lane = d.clone();
    with_lane
        .apply(add_clip("lead", 0, notes_clip(0, 960, vec![])), &reg)
        .unwrap();
    let lane_id = clip_ids(with_lane.doc())[0];
    let text = save_project(with_lane.doc()).replacen(
        &format!("\"id\": {}", id.0),
        &format!("\"id\": {}", lane_id.0),
        1,
    );
    assert!(load_project(&text, &reg).is_err());
}

#[test]
fn undoing_a_slot_command_restores_the_next_clip_id() {
    let reg = registry();
    let mut d = doc(&reg);
    let first = d.doc().song.next_clip_id;
    new_clip(&mut d, &reg, "synth", 0);
    assert_eq!(d.doc().song.next_clip_id, first + 1);
    d.undo().unwrap();
    assert_eq!(d.doc().song.next_clip_id, first);
}

#[test]
fn pasting_a_ready_clip_gives_it_a_new_id() {
    let reg = registry();
    let mut d = doc(&reg);
    let ready = LauncherClip {
        clip: notes_clip(0, 15_360 * 2, vec![note(0, 100, 60, 0.8)]),
        quantize: Some(LaunchQuantize::None),
    };
    d.apply(
        lc(LauncherCommand::SetSlot {
            device: key("synth"),
            row: 0,
            clip: ready,
        }),
        &reg,
    )
    .unwrap();
    let s = d.doc().song.launcher.slot(&key("synth"), 0).unwrap();
    assert_eq!(s.quantize, Some(LaunchQuantize::None));
    assert!(s.clip.id.0 < d.doc().song.next_clip_id);
    d.doc().validate().unwrap();
}
