//! The video of a song: the five `VideoCommand`s as undo steps (drags merge), the optional
//! `"video"` field of the project format (written only when set, byte-identical save, load and
//! save, an old file opens without it), the lock to seconds across a tempo change, and its limits.

mod common;

use atomik_model::{
    Command, Document, ModelError, SongCommand, SyncOp, TempoEvent, VideoClip, VideoCommand,
    VideoFacts, load_project, load_project_with_warnings, new_project_doc, save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

fn facts() -> VideoFacts {
    VideoFacts {
        duration_us: 60_000_000,
        fps_num: 25,
        fps_den: 1,
        width: 1280,
        height: 720,
        timecode: Some("10:00:00:00".into()),
        has_audio: true,
    }
}

fn clip() -> VideoClip {
    VideoClip::new("films\\scene one.mov", facts())
}

fn with_video() -> (Document, atomik_device_sdk::Registry) {
    let (mut d, r) = doc();
    d.apply(Command::Video(VideoCommand::Set(clip())), &r)
        .unwrap();
    (d, r)
}

fn video(d: &Document) -> VideoClip {
    d.doc().video.clone().expect("the song has a video")
}

#[test]
fn a_new_project_has_none_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().video.is_none());
    assert!(!text(&d).contains("\"video\""));
}

#[test]
fn adding_replacing_and_removing_are_one_undo_step_each() {
    let (mut d, r) = doc();
    let ops = one_step(&mut d, &r, Command::Video(VideoCommand::Set(clip())));
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.undo_label(), Some("Set Video"));
    d.apply(Command::Video(VideoCommand::Set(clip())), &r)
        .unwrap_err();
    d.apply(Command::Video(VideoCommand::Set(clip())), &r).ok();
    let mut other = VideoClip::new("other.mp4", facts());
    other.start_us = 1_000_000;
    one_step(&mut d, &r, Command::Video(VideoCommand::Set(other.clone())));
    assert_eq!(video(&d), other);
    one_step(&mut d, &r, Command::Video(VideoCommand::Remove));
    assert_eq!(d.undo_label(), Some("Remove Video"));
    assert!(d.doc().video.is_none());
    d.undo().unwrap();
    assert_eq!(video(&d), other);
    d.undo().unwrap();
    assert_eq!(video(&d), clip());
    d.undo().unwrap();
    assert!(d.doc().video.is_none());
}

#[test]
fn nothing_to_edit_without_a_video() {
    let (mut d, r) = doc();
    for c in [
        VideoCommand::Remove,
        VideoCommand::Move {
            start_us: 5,
            merge: None,
        },
        VideoCommand::Trim {
            in_us: 0,
            out_us: 5_000,
            merge: None,
        },
        VideoCommand::Relink {
            path: "x.mov".into(),
            facts: facts(),
        },
    ] {
        let e = rejected(&mut d, &r, Command::Video(c));
        assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    }
}

#[test]
fn moving_is_one_undo_step_and_a_drag_merges() {
    let (mut d, r) = with_video();
    let mv = |us, merge| {
        Command::Video(VideoCommand::Move {
            start_us: us,
            merge,
        })
    };
    one_step(&mut d, &r, mv(2_500_000, None));
    assert_eq!(d.undo_label(), Some("Move Video"));
    rejected(&mut d, &r, mv(2_500_000, None));
    // A drag: three moves with one id are one step.
    let before = text(&d);
    let steps = |d: &Document| {
        let mut d = d.clone();
        let mut n = 0;
        while d.undo().is_some() {
            n += 1;
        }
        n
    };
    let n = steps(&d);
    for us in [3_000_000, 3_500_000, -4_000_000] {
        d.apply(mv(us, Some(7)), &r).unwrap();
    }
    assert_eq!(steps(&d), n + 1);
    assert_eq!(video(&d).start_us, -4_000_000);
    d.undo().unwrap();
    assert_eq!(text(&d), before);
    d.redo().unwrap();
    assert_eq!(video(&d).start_us, -4_000_000);
    // Another drag is another step.
    d.apply(mv(1, Some(8)), &r).unwrap();
    assert_eq!(steps(&d), n + 2);
}

#[test]
fn trimming_moves_the_start_with_the_in_point_and_a_drag_merges() {
    let (mut d, r) = with_video();
    let trim = |i, o, merge| {
        Command::Video(VideoCommand::Trim {
            in_us: i,
            out_us: o,
            merge,
        })
    };
    one_step(&mut d, &r, trim(10_000_000, 50_000_000, None));
    let v = video(&d);
    // The picture that stays keeps its place: the start moved by the in point.
    assert_eq!(
        (v.start_us, v.in_us, v.out_us),
        (10_000_000, 10_000_000, 50_000_000)
    );
    assert_eq!(v.length_us(), 40_000_000);
    assert_eq!(v.source_at(10_000_000), Some(10_000_000));
    assert_eq!(d.undo_label(), Some("Trim Video"));
    // Trimming the out point alone leaves the start.
    one_step(&mut d, &r, trim(10_000_000, 40_000_000, None));
    assert_eq!(video(&d).start_us, 10_000_000);
    // A drag of the in point merges.
    let before = text(&d);
    for i in [11_000_000, 12_000_000, 13_000_000] {
        d.apply(trim(i, 40_000_000, Some(3)), &r).unwrap();
    }
    assert_eq!(video(&d).start_us, 13_000_000);
    d.undo().unwrap();
    assert_eq!(text(&d), before);
}

#[test]
fn the_trim_stays_inside_the_film() {
    let (mut d, r) = with_video();
    let trim = |i, o| {
        Command::Video(VideoCommand::Trim {
            in_us: i,
            out_us: o,
            merge: None,
        })
    };
    for (i, o) in [
        (0, 60_000_001),
        (5_000, 5_000),
        (9_000_000, 8_000_000),
        (59_999_500, 60_000_000),
    ] {
        let e = rejected(&mut d, &r, trim(i, o));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{i}..{o}: {e:?}");
    }
    one_step(&mut d, &r, trim(59_990_000, 60_000_000));
}

#[test]
fn the_limits_are_enforced() {
    let (mut d, r) = doc();
    let mut bad = clip();
    bad.path.clear();
    let mut far = clip();
    far.start_us = i64::MAX;
    let mut wide = clip();
    wide.facts.width = 0;
    let mut still = clip();
    still.facts.fps_den = 0;
    let mut long = clip();
    long.out_us = 61_000_000;
    for c in [bad, far, wide, still, long] {
        let e = rejected(&mut d, &r, Command::Video(VideoCommand::Set(c)));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    }
    let (mut d, r) = with_video();
    let e = rejected(
        &mut d,
        &r,
        Command::Video(VideoCommand::Move {
            start_us: i64::MIN,
            merge: None,
        }),
    );
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
}

#[test]
fn the_command_is_not_accepted_in_a_batch() {
    let (mut d, r) = with_video();
    let e = d
        .apply(
            Command::Batch(vec![Command::Video(VideoCommand::Remove)]),
            &r,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::BadBatch(_)), "{e:?}");
    assert!(d.doc().video.is_some());
}

#[test]
fn a_relink_swaps_the_path_and_the_facts_and_keeps_start_and_trim() {
    let (mut d, r) = with_video();
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 3_000_000,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    d.apply(
        Command::Video(VideoCommand::Trim {
            in_us: 0,
            out_us: 55_000_000,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let mut shorter = facts();
    shorter.duration_us = 40_000_000;
    shorter.fps_num = 24;
    let relink = Command::Video(VideoCommand::Relink {
        path: "moved\\scene one.mov".into(),
        facts: shorter.clone(),
    });
    one_step(&mut d, &r, relink);
    let v = video(&d);
    assert_eq!(v.path, "moved\\scene one.mov");
    assert_eq!(v.facts, shorter);
    assert_eq!(v.start_us, 3_000_000);
    assert_eq!(
        v.out_us, 40_000_000,
        "the out point is limited to the new length"
    );
    assert_eq!(d.undo_label(), Some("Relink Video"));
    // A replacement that ends before the in point is refused.
    d.apply(
        Command::Video(VideoCommand::Trim {
            in_us: 30_000_000,
            out_us: 40_000_000,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let mut tiny = facts();
    tiny.duration_us = 20_000_000;
    rejected(
        &mut d,
        &r,
        Command::Video(VideoCommand::Relink {
            path: "tiny.mov".into(),
            facts: tiny,
        }),
    );
    // The same path and facts change nothing.
    let same = video(&d);
    rejected(
        &mut d,
        &r,
        Command::Video(VideoCommand::Relink {
            path: same.path,
            facts: same.facts,
        }),
    );
}

#[test]
fn a_tempo_change_keeps_the_video_locked_to_time() {
    let (mut d, r) = with_video();
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 8_000_000,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let at = |d: &Document| {
        let v = video(d);
        (v.start_us, v.end_us(), v.start_tick(&d.doc().song))
    };
    let (start, end, tick) = at(&d);
    // 120 BPM: 8 s is 16 beats.
    assert_eq!(tick, 16 * atomik_core::PPQ);
    d.apply(Command::SetTempo(60.0), &r).unwrap();
    let (start2, end2, tick2) = at(&d);
    assert_eq!(
        (start2, end2),
        (start, end),
        "the film does not move in time"
    );
    assert_eq!(tick2, 8 * atomik_core::PPQ, "its tick follows the tempo");
    // A tempo map with a step later keeps the lock too.
    d.apply(
        Command::Song(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 120.0),
            TempoEvent::step(8 * atomik_core::PPQ, 240.0),
        ])),
        &r,
    )
    .unwrap();
    let (start3, end3, tick3) = at(&d);
    assert_eq!((start3, end3), (start, end));
    // 4 s at 120 BPM make 8 beats; the other 4 s at 240 BPM make 16 more.
    assert_eq!(tick3, 24 * atomik_core::PPQ);
    // And the undo of the tempo changes does not touch the film either.
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(at(&d).0, start);
}

#[test]
fn the_save_is_canonical_and_loads_back_byte_for_byte() {
    let (mut d, r) = with_video();
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: -1_250_000,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let saved = text(&d);
    assert!(saved.contains("\"video\": {"), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.video, d.doc().video);
    assert_eq!(save_project(&back), saved, "save, load, save");
}

#[test]
fn an_old_project_opens_without_a_video() {
    let (d, r) = doc();
    let saved = text(&d);
    let (back, warnings) = load_project_with_warnings(&saved, &r).unwrap();
    assert!(back.video.is_none() && warnings.is_empty());
    // Every demo fixture of an earlier format opens with none.
    for name in ["demo-v1.atomik", "demo-v20.atomik", "demo-v43.atomik"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        if let Ok(old) = std::fs::read_to_string(path) {
            let loaded = load_project(&old, &r).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(loaded.video.is_none(), "{name}");
        }
    }
}

#[test]
fn the_loader_names_the_field_of_a_bad_video() {
    let (d, r) = with_video();
    let saved = text(&d);
    let bad = saved.replacen("\"fps_num\": 25", "\"fps_num\": \"fast\"", 1);
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(e.contains("video.fps_num"), "{e}");
    let past = saved.replacen("\"out_us\": 60000000", "\"out_us\": 70000000", 1);
    let e = load_project(&past, &r).unwrap_err().to_string();
    assert!(e.contains("video"), "{e}");
}
