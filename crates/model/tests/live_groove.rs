//! A track's live groove: a groove name and strength saved on the track, applied when the song is
//! flattened. The stored notes never change; editing, renaming or deleting the groove follows on
//! every track that uses it; the field is in project format 43.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::groove::{GrooveSlot, groove_notes};
use atomik_model::song::{flatten_notes, flatten_track_notes};
use atomik_model::{
    ClipContent, ClipId, Command, DeviceKey, Document, Groove, ModelError, PROJECT_VERSION,
    SongCommand, StepResolution, SyncOp, TrackGroove, load_project, save_project,
};
use common::audio::{audio_registry, with_audio_track};
use common::song::*;
use common::*;
use serde_json::Value;

const S: i64 = PPQ / 4;

/// A groove whose second 1/16 slot plays a quarter step late and at half velocity.
fn late() -> Groove {
    Groove {
        name: "Late".into(),
        step: StepResolution::Sixteenth,
        slots: vec![
            GrooveSlot::STRAIGHT,
            GrooveSlot {
                offset: 0.25,
                velocity: 0.5,
            },
        ],
    }
}

fn live(name: &str, strength: u8) -> TrackGroove {
    TrackGroove {
        name: name.into(),
        strength,
    }
}

fn set(device: &str, groove: Option<TrackGroove>) -> Command {
    sc(SongCommand::SetTrackGroove {
        device: DeviceKey(device.into()),
        groove,
    })
}

/// Tracks `lead` (notes on the first two steps and the third) and `synth` (one note on step 1),
/// and the groove `Late` in the song.
fn song(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(sc(SongCommand::SetGroove(late())), reg).unwrap();
    for (key, notes) in [
        (
            "lead",
            vec![
                note(0, S, 60, 0.8),
                note(S, S, 62, 0.8),
                note(2 * S, S, 64, 0.8),
            ],
        ),
        ("synth", vec![note(S, S, 36, 1.0)]),
    ] {
        d.apply(add_clip(key, 0, notes_clip(0, BAR, notes)), reg)
            .unwrap();
    }
    d
}

fn played(d: &Document, key: &str) -> Vec<(i64, i64, u8, f32)> {
    let song = &d.doc().song;
    flatten_track_notes(song, song.track(&DeviceKey(key.into())).unwrap())
}

fn stored(d: &Document, id: u64) -> Vec<atomik_model::Note> {
    let (_, _, c) = d.doc().song.clip(ClipId(id)).unwrap();
    match &c.content {
        ClipContent::Notes(n) => n.to_vec(),
        _ => panic!("not notes"),
    }
}

#[test]
fn a_track_without_a_groove_plays_exactly_what_flatten_notes_gives() {
    let reg = registry();
    let d = song(&reg);
    let song = &d.doc().song;
    for t in &song.tracks {
        assert_eq!(flatten_track_notes(song, t), flatten_notes(t));
    }
}

#[test]
fn the_groove_moves_the_notes_of_its_slot_and_leaves_the_stored_notes() {
    let reg = registry();
    let mut d = song(&reg);
    let before = stored(&d, 1);
    d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    let notes = played(&d, "lead");
    // Step 0 straight, step 1 a quarter step late and softer, step 2 straight again.
    assert_eq!(notes[0], (0, S, 60, 0.8));
    assert_eq!(notes[1].0, S + S / 4);
    assert_eq!((notes[1].1, notes[1].2), (S, 62));
    assert!((notes[1].3 - 0.4).abs() < 1e-6);
    assert_eq!(notes[2], (2 * S, S, 64, 0.8));
    assert_eq!(stored(&d, 1), before, "the notes are not edited");
    // The other track is not touched.
    assert_eq!(played(&d, "synth"), [(S, S, 36, 1.0)]);
}

#[test]
fn the_strength_scales_the_move_and_zero_plays_the_notes_as_they_are() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 50))), &reg).unwrap();
    let half = played(&d, "lead");
    assert_eq!(half[1].0, S + S / 8);
    assert!((half[1].3 - 0.6).abs() < 1e-6);
    d.apply(set("lead", Some(live("Late", 0))), &reg).unwrap();
    let song = &d.doc().song;
    assert_eq!(
        played(&d, "lead"),
        flatten_notes(song.track(&DeviceKey("lead".into())).unwrap())
    );
}

#[test]
fn changing_the_template_re_times_every_track_that_uses_it() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    d.apply(set("synth", Some(live("Late", 100))), &reg)
        .unwrap();
    let mut later = late();
    later.slots[1].offset = 0.5;
    let ops = d.apply(sc(SongCommand::SetGroove(later)), &reg).unwrap();
    assert!(ops.contains(&SyncOp::SongChanged), "{ops:?}");
    assert_eq!(played(&d, "lead")[1].0, S + S / 2);
    assert_eq!(played(&d, "synth")[0].0, S + S / 2);
    // Undo gives the old template back on both.
    d.undo().unwrap();
    assert_eq!(played(&d, "lead")[1].0, S + S / 4);
    assert_eq!(played(&d, "synth")[0].0, S + S / 4);
}

#[test]
fn a_built_in_groove_can_be_used_and_the_start_never_goes_below_zero() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("synth", Some(live("Swing 66%", 100))), &reg)
        .unwrap();
    assert!(played(&d, "synth")[0].0 > S);
    let early = vec![(0, S, 60, 0.5)];
    let g = Groove {
        name: "Early".into(),
        step: StepResolution::Sixteenth,
        slots: vec![GrooveSlot {
            offset: -0.5,
            velocity: 1.0,
        }],
    };
    assert_eq!(groove_notes(early, &g, 100)[0].0, 0);
}

#[test]
fn renaming_a_groove_renames_it_on_the_tracks_and_undo_restores_both() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 70))), &reg).unwrap();
    let played_before = played(&d, "lead");
    d.apply(
        sc(SongCommand::RenameGroove {
            from: "Late".into(),
            to: "Behind".into(),
        }),
        &reg,
    )
    .unwrap();
    let song_now = &d.doc().song;
    assert_eq!(song_now.grooves[0].name, "Behind");
    assert_eq!(
        song_now.tracks[0].groove.as_ref().unwrap(),
        &live("Behind", 70)
    );
    assert_eq!(played(&d, "lead"), played_before);
    d.undo().unwrap();
    assert_eq!(d.doc().song.grooves[0].name, "Late");
    assert_eq!(d.doc().song.tracks[0].groove, Some(live("Late", 70)));
}

#[test]
fn renaming_is_refused_for_a_taken_built_in_blank_or_missing_name() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(
        sc(SongCommand::SetGroove(Groove {
            name: "Other".into(),
            ..late()
        })),
        &reg,
    )
    .unwrap();
    for (from, to) in [
        ("Late", "Other"),
        ("Late", "Straight"),
        ("Late", " "),
        ("Late", "Late"),
        ("Missing", "Fresh"),
        ("Straight", "Fresh"),
    ] {
        let before = save_project(d.doc());
        let r = d.apply(
            sc(SongCommand::RenameGroove {
                from: from.into(),
                to: to.into(),
            }),
            &reg,
        );
        assert!(r.is_err(), "{from} -> {to:?}");
        assert_eq!(save_project(d.doc()), before);
    }
}

#[test]
fn deleting_a_groove_takes_it_off_the_tracks_in_one_undo_step() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    d.apply(
        sc(SongCommand::DeleteGroove {
            name: "Late".into(),
        }),
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.tracks[0].groove.is_none());
    assert!(d.doc().song.grooves.is_empty());
    d.doc().validate().unwrap();
    d.undo().unwrap();
    assert_eq!(d.doc().song.tracks[0].groove, Some(live("Late", 100)));
    assert_eq!(d.doc().song.grooves.len(), 1);
}

#[test]
fn a_groove_that_does_not_exist_is_refused() {
    let reg = registry();
    let mut d = song(&reg);
    let unknown = d
        .apply(set("lead", Some(live("Nope", 100))), &reg)
        .unwrap_err();
    assert!(matches!(unknown, ModelError::UnknownKey(_)), "{unknown}");
    let too_much = d
        .apply(set("lead", Some(live("Late", 101))), &reg)
        .unwrap_err();
    assert!(matches!(too_much, ModelError::OutOfRange(_)), "{too_much}");
    assert!(d.doc().song.tracks.iter().all(|t| t.groove.is_none()));
}

#[test]
fn an_audio_track_plays_no_notes_for_a_groove_to_move() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(sc(SongCommand::SetGroove(late())), &reg).unwrap();
    let audio = d
        .apply(set("audio-track", Some(live("Late", 100))), &reg)
        .unwrap_err();
    assert!(matches!(audio, ModelError::BadSong(_)), "{audio}");
    assert!(d.doc().song.tracks[0].groove.is_none());
}

#[test]
fn setting_a_groove_reports_a_song_change_and_is_one_undo_step() {
    let reg = registry();
    let mut d = song(&reg);
    let ops = d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    assert_eq!(ops, [SyncOp::SongChanged]);
    d.undo().unwrap();
    assert!(d.doc().song.tracks[0].groove.is_none());
}

#[test]
fn the_live_groove_round_trips_in_format_43_and_a_full_strength_is_not_written() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    d.apply(set("synth", Some(live("Swing 55%", 40))), &reg)
        .unwrap();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 44);
    let v: Value = serde_json::from_str(&text).unwrap();
    let tracks = &v["song"]["tracks"];
    assert_eq!(tracks[0]["groove"], "Late");
    assert!(tracks[0].get("groove_strength").is_none());
    assert_eq!(tracks[1]["groove"], "Swing 55%");
    assert_eq!(tracks[1]["groove_strength"], 40);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.tracks[0].groove, Some(live("Late", 100)));
    assert_eq!(loaded.song.tracks[1].groove, Some(live("Swing 55%", 40)));
    assert_eq!(save_project(&loaded), text, "canonical");
    // No groove, no field.
    let plain = save_project(song(&reg).doc());
    assert!(!plain.contains("\"groove\""));
}

#[test]
fn a_file_with_a_missing_groove_a_stray_strength_or_an_old_version_is_refused() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 60))), &reg).unwrap();
    let text = save_project(d.doc());
    let with = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        f(&mut v);
        v.to_string()
    };
    let missing = with(&|v| v["song"]["tracks"][0]["groove"] = "Gone".into());
    let e = load_project(&missing, &reg).unwrap_err().to_string();
    assert!(e.contains("no groove named"), "{e}");
    let stray = with(&|v| {
        v["song"]["tracks"][1]["groove_strength"] = 10.into();
    });
    assert!(load_project(&stray, &reg).is_err());
    let full = with(&|v| v["song"]["tracks"][0]["groove_strength"] = 100.into());
    assert!(
        load_project(&full, &reg).is_err(),
        "100 is written by omission"
    );
    let old = text.replace("\"version\": 44", "\"version\": 42");
    let e = load_project(&old, &reg).unwrap_err().to_string();
    assert!(e.contains("groove"), "{e}");
}

#[test]
fn deleting_the_track_or_duplicating_it_keeps_a_valid_document() {
    let reg = registry();
    let mut d = song(&reg);
    d.apply(set("lead", Some(live("Late", 100))), &reg).unwrap();
    d.apply(
        Command::Workflow(atomik_model::WorkflowCommand::DuplicateTrack {
            device: DeviceKey("lead".into()),
        }),
        &reg,
    )
    .unwrap();
    d.doc().validate().unwrap();
    let copies = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.groove.is_some())
        .count();
    assert_eq!(copies, 2, "a copy keeps the groove");
}

/// `song` plus a second clip on `lead` (one note on its second step, one bar later); the clips are
/// 1 (first bar) and 3 (second bar).
fn two_clips(reg: &Registry) -> Document {
    let mut d = song(reg);
    d.apply(
        add_clip("lead", 0, notes_clip(BAR, BAR, vec![note(S, S, 67, 0.8)])),
        reg,
    )
    .unwrap();
    d
}

fn set_clip(clips: &[u64], groove: Option<TrackGroove>) -> Command {
    sc(SongCommand::SetClipGroove {
        clips: clips.iter().map(|&i| ClipId(i)).collect(),
        groove,
    })
}

#[test]
fn a_clip_groove_overrides_the_track_groove_for_its_own_notes_only() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(set("lead", Some(live("Swing 55%", 100))), &reg)
        .unwrap();
    d.apply(set_clip(&[3], Some(live("Late", 100))), &reg)
        .unwrap();
    let notes = played(&d, "lead");
    // The first clip plays the track's swing: its step 1 note is delayed by the 55% swing.
    let first = notes.iter().find(|n| n.2 == 62).unwrap();
    assert_eq!(first.0, S + (S as f64 * 0.1).round() as i64);
    // The second clip plays "Late", a quarter step late and softer, not the track's swing.
    let second = notes.iter().find(|n| n.2 == 67).unwrap();
    assert_eq!(second.0, BAR + S + S / 4);
    assert!((second.3 - 0.4).abs() < 1e-6);
    assert!(
        notes
            .windows(2)
            .all(|w| (w[0].0, w[0].2) <= (w[1].0, w[1].2))
    );
    // The stored notes are untouched.
    assert_eq!(stored(&d, 3), [note(S, S, 67, 0.8)]);
}

#[test]
fn a_clip_groove_works_on_a_track_without_one_and_none_hands_the_clip_back() {
    let reg = registry();
    let mut d = two_clips(&reg);
    let plain = played(&d, "lead");
    d.apply(set_clip(&[3], Some(live("Late", 100))), &reg)
        .unwrap();
    let grooved = played(&d, "lead");
    assert_ne!(grooved, plain);
    assert_eq!(grooved[..3], plain[..3], "the first clip plays as stored");
    d.apply(set_clip(&[3], None), &reg).unwrap();
    assert_eq!(played(&d, "lead"), plain);
    d.undo().unwrap();
    assert_eq!(played(&d, "lead"), grooved);
}

#[test]
fn a_clip_groove_is_refused_for_a_missing_groove_a_bad_strength_or_a_non_note_clip() {
    let reg = registry();
    let mut d = two_clips(&reg);
    assert!(matches!(
        d.apply(set_clip(&[3], Some(live("Gone", 100))), &reg),
        Err(ModelError::UnknownKey(_))
    ));
    assert!(matches!(
        d.apply(set_clip(&[3], Some(live("Late", 101))), &reg),
        Err(ModelError::OutOfRange(_))
    ));
    assert!(d.apply(set_clip(&[], None), &reg).is_err());
    assert!(
        d.apply(set_clip(&[3], None), &reg).is_err(),
        "nothing changes"
    );
    assert!(
        d.apply(set_clip(&[99], Some(live("Late", 50))), &reg)
            .is_err()
    );
}

#[test]
fn renaming_or_deleting_a_groove_follows_on_the_clips_in_one_undo_step() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(set_clip(&[1, 3], Some(live("Late", 70))), &reg)
        .unwrap();
    d.apply(
        sc(SongCommand::RenameGroove {
            from: "Late".into(),
            to: "Behind".into(),
        }),
        &reg,
    )
    .unwrap();
    let g = |d: &Document, id| {
        d.doc()
            .song
            .clip(ClipId(id))
            .unwrap()
            .2
            .groove
            .as_deref()
            .cloned()
    };
    assert_eq!(g(&d, 1), Some(live("Behind", 70)));
    assert_eq!(g(&d, 3), Some(live("Behind", 70)));
    d.undo().unwrap();
    assert_eq!(g(&d, 3), Some(live("Late", 70)));
    d.apply(
        sc(SongCommand::DeleteGroove {
            name: "Late".into(),
        }),
        &reg,
    )
    .unwrap();
    assert_eq!((g(&d, 1), g(&d, 3)), (None, None));
    d.undo().unwrap();
    assert_eq!(g(&d, 1), Some(live("Late", 70)));
}

#[test]
fn editing_the_template_reports_a_song_change_when_only_a_clip_uses_it() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(set_clip(&[3], Some(live("Late", 100))), &reg)
        .unwrap();
    let ops = d.apply(sc(SongCommand::SetGroove(late())), &reg).unwrap();
    assert!(ops.contains(&SyncOp::SongChanged), "{ops:?}");
}

#[test]
fn the_clip_groove_round_trips_and_a_clip_naming_a_missing_groove_loads_without_it() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(set_clip(&[1], Some(live("Late", 100))), &reg)
        .unwrap();
    d.apply(set_clip(&[3], Some(live("Swing 55%", 35))), &reg)
        .unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let clips = &v["song"]["tracks"][0]["lanes"][0]["clips"];
    assert_eq!(clips[0]["groove"], "Late");
    assert!(clips[0].get("groove_strength").is_none());
    assert_eq!(clips[1]["groove"], "Swing 55%");
    assert_eq!(clips[1]["groove_strength"], 35);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&loaded), text, "canonical");
    let gone = text.replace("\"groove\": \"Late\"", "\"groove\": \"Gone\"");
    let loaded = load_project(&gone, &reg).unwrap();
    assert!(loaded.song.clip(ClipId(1)).unwrap().2.groove.is_none());
    assert!(!save_project(d.doc()).is_empty());
    // A project without a clip groove writes nothing for it.
    assert!(!save_project(two_clips(&reg).doc()).contains("\"groove\""));
}
