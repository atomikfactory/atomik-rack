//! Model commands: asset pool, audio tracks, audio clips, input channels, pool
//! cleanup.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, AudioClipContent, ClipContent, Command, Document, FadeCurve, LaneKind, ModelError,
    SongCommand, SyncOp, TempoEvent, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::*;

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let last = d.last_clip_id();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo, "a rejected command records no step");
    assert_eq!(d.last_clip_id(), last);
    e
}

// ---- empty doc / pool -----------------------------------------------------------------------

#[test]
fn new_documents_have_two_inputs_and_an_empty_pool() {
    let doc = empty_doc();
    assert_eq!(doc.input_channels, 2);
    assert!(doc.assets.is_empty());
}

#[test]
fn register_asset_adds_to_the_pool_without_an_undo_step() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    let ops = d.apply(register(7, 1000), &reg).unwrap();
    assert!(ops.is_empty());
    assert_eq!(d.doc().assets[&hash(7)], meta(1000));
    assert!(!d.can_undo(), "registering is not an undoable edit");
}

#[test]
fn register_asset_is_idempotent_and_first_name_wins() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    let before = d.doc().clone();
    d.apply(register(1, 480_000), &reg).unwrap();
    let renamed = AssetMeta {
        original_name: "other.wav".into(),
        ..meta(480_000)
    };
    d.apply(
        Command::RegisterAsset {
            hash: hash(1),
            meta: renamed,
        },
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn register_asset_rejections() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    let bad = |h: String, m: AssetMeta| Command::RegisterAsset { hash: h, meta: m };
    for h in [
        String::new(),
        "abc".to_owned(),
        hash(0xab).to_uppercase(),
        format!("{}g", "a".repeat(63)),
        "a".repeat(65),
    ] {
        assert!(matches!(
            rejects(&mut d, bad(h, meta(10)), &reg),
            ModelError::BadSong(_)
        ));
    }
    for m in [
        AssetMeta {
            channels: 0,
            ..meta(10)
        },
        AssetMeta {
            sample_rate: 0,
            ..meta(10)
        },
        AssetMeta {
            frames: 0,
            ..meta(10)
        },
        AssetMeta {
            frames: u64::MAX,
            ..meta(10)
        },
    ] {
        assert!(matches!(
            rejects(&mut d, bad(hash(9), m), &reg),
            ModelError::OutOfRange(_)
        ));
    }
    // Same id, different audio.
    assert!(matches!(
        rejects(&mut d, bad(hash(1), meta(5)), &reg),
        ModelError::BadSong(_)
    ));
}

#[test]
fn register_asset_keeps_redo() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    d.undo(&reg).unwrap();
    assert!(d.can_redo());
    d.apply(register(3, 10), &reg).unwrap();
    assert!(d.can_redo(), "registering does not clear redo");
    d.redo(&reg).unwrap();
}

// ---- audio tracks ---------------------------------------------------------------------------

#[test]
fn add_audio_track_is_one_step_with_device_channel_track_and_cables() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    let before = d.doc().clone();
    let ops = d.apply(add_audio_track(), &reg).unwrap();
    let doc = d.doc();
    let dev = doc.device(&key("audio-track")).expect("device");
    assert_eq!(dev.type_id, AUDIO_TRACK);
    assert_eq!(dev.name, "Audio Track");
    let t = doc.song.track(&key("audio-track")).expect("track");
    assert_eq!(t.lanes.len(), 1);
    assert_eq!(t.lanes[0].kind, LaneKind::Audio);
    assert!(t.is_audio());
    assert!(doc.is_audio_track(&key("audio-track"), &reg));
    // Console channel and master came with it.
    let ch = doc
        .console
        .channel_of(&key("audio-track"))
        .expect("channel");
    assert!(doc.console.master.is_some());
    assert_eq!(doc.console.channels.len(), 1);
    assert!(doc.device(&ch.channel).is_some());
    // Input cables.
    assert!(
        doc.cables
            .contains(&cable("interface:in_1", "audio-track:in_l"))
    );
    assert!(
        doc.cables
            .contains(&cable("interface:in_2", "audio-track:in_r"))
    );
    assert!(ops.contains(&SyncOp::SongChanged));
    assert!(ops.contains(&SyncOp::ConsoleChanged));
    assert!(matches!(ops[0], SyncOp::DeviceAdded(_)));
    // One undo step.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn add_audio_track_undo_redo_roundtrip() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    let before = d.doc().clone();
    d.apply(add_audio_track(), &reg).unwrap();
    let after = d.doc().clone();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc(), &after);
}

#[test]
fn add_audio_track_keys_and_names_are_unique_and_the_name_is_used() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    d.apply(add_audio_track(), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddAudioTrack {
            name: Some("Vocals".into()),
        }),
        &reg,
    )
    .unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    let doc = d.doc();
    let tracks: Vec<_> = doc.song.tracks.iter().map(|t| t.device.0.clone()).collect();
    assert_eq!(tracks, ["audio-track", "audio-track-2", "audio-track-3"]);
    assert_eq!(doc.device(&key("audio-track-2")).unwrap().name, "Vocals");
    assert_eq!(
        doc.device(&key("audio-track-3")).unwrap().name,
        "Audio Track 2"
    );
    // Every track's input sockets are cabled from the same two interface inputs.
    assert_eq!(
        doc.cables
            .iter()
            .filter(|c| c.from.device.0 == "interface")
            .count(),
        6
    );
}

#[test]
fn add_audio_track_with_one_input_feeds_both_sockets_from_in_1() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    d.apply(Command::SetInputChannels(1), &reg).unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    let c = &d.doc().cables;
    assert!(c.contains(&cable("interface:in_1", "audio-track:in_l")));
    assert!(c.contains(&cable("interface:in_1", "audio-track:in_r")));
    assert!(!c.iter().any(|c| c.from.socket == "in_2"));
}

#[test]
fn add_audio_track_without_inputs_has_no_input_cables() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    d.apply(Command::SetInputChannels(0), &reg).unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| c.from.device.0 == "interface")
    );
}

#[test]
fn add_audio_track_without_console_devices_still_works() {
    let reg = audio_registry_no_console();
    let mut d = Document::new(empty_doc());
    d.apply(add_audio_track(), &reg).unwrap();
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(d.doc().song.tracks.len(), 1);
    assert_eq!(d.doc().cables.len(), 2);
}

#[test]
fn add_audio_track_rejections() {
    let reg = registry(); // no audio track type
    let mut d = Document::new(empty_doc());
    assert!(matches!(
        rejects(&mut d, add_audio_track(), &reg),
        ModelError::UnknownType(_)
    ));
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    assert!(matches!(
        rejects(
            &mut d,
            sc(SongCommand::AddAudioTrack {
                name: Some("  ".into())
            }),
            &reg
        ),
        ModelError::BadSong(_)
    ));
}

#[test]
fn add_track_on_an_audio_track_device_makes_an_audio_lane() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, AUDIO_TRACK, Some("tape")), &reg).unwrap();
    assert!(d.doc().console.channel_of(&key("tape")).is_some());
    d.apply(add_track("tape"), &reg).unwrap();
    let t = d.doc().song.track(&key("tape")).unwrap();
    assert_eq!(t.lanes.len(), 1);
    assert_eq!(t.lanes[0].kind, LaneKind::Audio);
    assert!(matches!(
        rejects(&mut d, add_track("tape"), &reg),
        ModelError::BadSong(_)
    ));
}

#[test]
fn audio_tracks_take_no_automation_lanes_and_notes_tracks_stay_notes() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let e = rejects(
        &mut d,
        sc(SongCommand::AddAutomationLane {
            device: key("audio-track"),
            param: "monitor".into(),
        }),
        &reg,
    );
    assert!(matches!(e, ModelError::BadSong(_)));
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    let t = d.doc().song.track(&key("synth")).unwrap();
    assert!(!t.is_audio());
    assert!(!d.doc().is_audio_track(&key("synth"), &reg));
}

#[test]
fn removing_an_audio_track_device_keeps_the_pool_and_undo_restores_everything() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let before = d.doc().clone();
    d.apply(
        Command::RemoveDevice {
            key: key("audio-track"),
        },
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.tracks.is_empty());
    assert!(
        d.doc().assets.contains_key(&hash(1)),
        "pool is kept for undo"
    );
    assert!(d.doc().referenced_assets().is_empty());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---- audio clips ----------------------------------------------------------------------------

#[test]
fn add_audio_clip_creates_a_clip_and_reports_its_id() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let before = d.doc().clone();
    let a = AudioClipContent {
        source_offset: 4800,
        gain_db: -6.0,
        fade_in: 100,
        fade_out: 200,
        fade_curve: FadeCurve::EqualPower,
        ..audio(1)
    };
    let ops = d
        .apply(
            add_audio_clip_with("audio-track", BAR, 2 * BAR, a.clone()),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    let id = last(&d);
    let c = doc_clip(d.doc(), id);
    assert_eq!(
        (c.start, c.length, c.offset, c.loop_length, c.muted),
        (BAR, 2 * BAR, 0, None, false)
    );
    assert_eq!(c.content, ClipContent::Audio(a));
    assert!(c.content.is_audio() && !c.content.is_notes());
    assert_eq!(d.doc().song.next_clip_id, id.0 + 1);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn add_audio_clip_rejections() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(add(0, AUDIO_TRACK, Some("bare")), &reg).unwrap();
    let ok = audio(1);
    // Unknown asset.
    assert!(matches!(
        rejects(&mut d, add_audio_clip("audio-track", 0, BAR, 9), &reg),
        ModelError::MissingAsset(_)
    ));
    // Unknown device, device without a track, notes track.
    assert!(matches!(
        rejects(&mut d, add_audio_clip("nope", 0, BAR, 1), &reg),
        ModelError::UnknownDevice(_)
    ));
    assert!(matches!(
        rejects(&mut d, add_audio_clip("bare", 0, BAR, 1), &reg),
        ModelError::BadSong(_)
    ));
    assert!(matches!(
        rejects(&mut d, add_audio_clip("synth", 0, BAR, 1), &reg),
        ModelError::BadSong(_)
    ));
    // Geometry.
    for (start, length) in [
        (-1, BAR),
        (0, 0),
        (0, -5),
        (atomik_model::song::MAX_TICK + 1, 1),
    ] {
        assert!(matches!(
            rejects(
                &mut d,
                add_audio_clip_with("audio-track", start, length, ok.clone()),
                &reg
            ),
            ModelError::OutOfRange(_)
        ));
    }
    // Content.
    let bads = [
        AudioClipContent {
            source_offset: 480_001,
            ..ok.clone()
        },
        AudioClipContent {
            gain_db: f32::NAN,
            ..ok.clone()
        },
        AudioClipContent {
            gain_db: f32::INFINITY,
            ..ok.clone()
        },
        AudioClipContent {
            gain_db: 24.5,
            ..ok.clone()
        },
        AudioClipContent {
            gain_db: -200.0,
            ..ok.clone()
        },
        AudioClipContent {
            fade_in: u64::MAX,
            ..ok.clone()
        },
        AudioClipContent {
            fade_out: u64::MAX,
            ..ok.clone()
        },
    ];
    for a in bads {
        assert!(matches!(
            rejects(&mut d, add_audio_clip_with("audio-track", 0, BAR, a), &reg),
            ModelError::OutOfRange(_)
        ));
    }
    // The generic AddClip checks lane and content kinds as well.
    let notes = add_clip("audio-track", 0, notes_clip(0, BAR, vec![]));
    assert!(matches!(
        rejects(&mut d, notes, &reg),
        ModelError::BadSong(_)
    ));
    let audio_on_notes = add_clip("synth", 0, clip(0, BAR, ClipContent::Audio(ok.clone())));
    assert!(matches!(
        rejects(&mut d, audio_on_notes, &reg),
        ModelError::BadSong(_)
    ));
    // Audio clips have no content offset, and a reversed one cannot loop.
    let mut c = clip(0, BAR, ClipContent::Audio(ok.clone()));
    c.offset = 5;
    assert!(matches!(
        rejects(&mut d, add_clip("audio-track", 0, c), &reg),
        ModelError::OutOfRange(_)
    ));
    let mut c = clip(
        0,
        BAR,
        ClipContent::Audio(AudioClipContent {
            reverse: true,
            ..ok
        }),
    );
    c.loop_length = Some(PPQ);
    assert!(matches!(
        rejects(&mut d, add_clip("audio-track", 0, c), &reg),
        ModelError::OutOfRange(_)
    ));
}

#[test]
fn generic_add_clip_accepts_audio_content_on_the_audio_lane() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(
        add_clip("audio-track", 0, clip(0, BAR, ClipContent::Audio(audio(2)))),
        &reg,
    )
    .unwrap();
    assert_eq!(
        doc_clip(d.doc(), last(&d)).content,
        ClipContent::Audio(audio(2))
    );
}

#[test]
fn recording_takes_stack_with_later_wins() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 4 * BAR, 1), &reg)
        .unwrap();
    let first = last(&d);
    d.apply(add_audio_clip("audio-track", BAR, 2 * BAR, 2), &reg)
        .unwrap();
    let second = last(&d);
    assert_ne!(first, second);
    let lane = &d.doc().song.tracks[0].lanes[0];
    let spans = atomik_model::song::audible_spans(lane);
    assert_eq!(
        spans,
        vec![
            (first, 0, BAR),
            (second, BAR, 3 * BAR),
            (first, 3 * BAR, 4 * BAR)
        ]
    );
}

#[test]
fn set_clip_geometry_moves_and_trims_audio_clips() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 2 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let geo = |start, length, offset, loop_length, muted| {
        sc(SongCommand::SetClipGeometry {
            id,
            start,
            length,
            offset,
            loop_length,
            muted,
            merge: None,
        })
    };
    d.apply(geo(BAR, BAR, 0, None, true), &reg).unwrap();
    let c = doc_clip(d.doc(), id);
    assert_eq!((c.start, c.length, c.muted), (BAR, BAR, true));
    assert!(matches!(
        rejects(&mut d, geo(0, BAR, 3, None, false), &reg),
        ModelError::OutOfRange(_)
    ));
    // An audio clip may loop, but not backwards, and not into more repetitions than allowed.
    d.apply(geo(0, BAR, 0, Some(PPQ), false), &reg).unwrap();
    assert_eq!(doc_clip(d.doc(), id).loop_length, Some(PPQ));
    assert!(matches!(
        rejects(&mut d, geo(0, BAR * 4096, 0, Some(PPQ), false), &reg),
        ModelError::OutOfRange(_)
    ));
    let reversed = AudioClipContent {
        reverse: true,
        ..audio(1)
    };
    assert!(matches!(
        rejects(
            &mut d,
            sc(SongCommand::SetClipContent {
                id,
                content: ClipContent::Audio(reversed),
                merge: None,
            }),
            &reg
        ),
        ModelError::OutOfRange(_)
    ));
}

#[test]
fn set_clip_content_replaces_audio_content_and_validates() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let set = |content| {
        sc(SongCommand::SetClipContent {
            id,
            content,
            merge: Some(1),
        })
    };
    let a = AudioClipContent {
        gain_db: 3.0,
        fade_in: 480,
        ..audio(2)
    };
    d.apply(set(ClipContent::Audio(a.clone())), &reg).unwrap();
    assert_eq!(doc_clip(d.doc(), id).content, ClipContent::Audio(a));
    // Drag merge: a second SetClipContent with the same group is one undo step.
    let b = AudioClipContent {
        gain_db: 4.0,
        ..audio(2)
    };
    d.apply(set(ClipContent::Audio(b)), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(doc_clip(d.doc(), id).content, ClipContent::Audio(audio(1)));
    assert!(matches!(
        rejects(&mut d, set(ClipContent::Audio(audio(9))), &reg),
        ModelError::MissingAsset(_)
    ));
    assert!(matches!(
        rejects(&mut d, set(ClipContent::Notes(vec![].into())), &reg),
        ModelError::BadSong(_)
    ));
}

#[test]
fn set_audio_clip_changes_everything_in_one_step_and_merges_drags() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 2 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let before = d.doc().clone();
    let cmd = |start, length, so, merge| {
        sc(SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted: false,
            content: AudioClipContent {
                source_offset: so,
                ..audio(1)
            },
            merge,
        })
    };
    // A left trim: new start, shorter, later source offset.
    d.apply(cmd(BAR / 2, BAR, 100, Some(5)), &reg).unwrap();
    d.apply(cmd(BAR, BAR / 2, 200, Some(5)), &reg).unwrap();
    let c = doc_clip(d.doc(), id);
    assert_eq!((c.start, c.length), (BAR, BAR / 2));
    assert_eq!(
        c.content,
        ClipContent::Audio(AudioClipContent {
            source_offset: 200,
            ..audio(1)
        })
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "a drag is one undo step");
    assert!(matches!(
        rejects(&mut d, cmd(0, 0, 0, None), &reg),
        ModelError::OutOfRange(_)
    ));
    assert!(matches!(
        rejects(&mut d, cmd(0, BAR, 480_001, None), &reg),
        ModelError::OutOfRange(_)
    ));
}

#[test]
fn set_audio_clip_rejects_notes_clips() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    d.apply(register(1, 100), &reg).unwrap();
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    let id = last(&d);
    let cmd = sc(SongCommand::SetAudioClip {
        id,
        start: 0,
        length: BAR,
        muted: false,
        content: audio(1),
        merge: None,
    });
    assert!(matches!(rejects(&mut d, cmd, &reg), ModelError::BadSong(_)));
}

#[test]
fn split_audio_clip_advances_the_source_offset_by_the_frames_to_the_split() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    // 120 BPM at 48 kHz: one beat (PPQ ticks) is 24000 frames.
    let a = AudioClipContent {
        source_offset: 1000,
        fade_in: 50,
        fade_out: 60,
        fade_curve: FadeCurve::EqualPower,
        gain_db: -3.0,
        ..audio(1)
    };
    d.apply(
        add_audio_clip_with("audio-track", BAR, 8 * PPQ, a.clone()),
        &reg,
    )
    .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::SplitClip { id, at: BAR + PPQ }), &reg)
        .unwrap();
    let right = last(&d);
    let l = doc_clip(d.doc(), id);
    let r = doc_clip(d.doc(), right);
    assert_eq!((l.start, l.length), (BAR, PPQ));
    assert_eq!((r.start, r.length), (BAR + PPQ, 7 * PPQ));
    let (ClipContent::Audio(la), ClipContent::Audio(ra)) = (&l.content, &r.content) else {
        panic!()
    };
    assert_eq!(la.source_offset, 1000);
    assert_eq!(ra.source_offset, 1000 + 24_000);
    // No fades are invented at the cut: the left half keeps its fade in, the right its fade out.
    assert_eq!((la.fade_in, la.fade_out), (50, 0));
    assert_eq!((ra.fade_in, ra.fade_out), (0, 60));
    assert_eq!((la.gain_db, ra.gain_db), (-3.0, -3.0));
    assert_eq!(ra.fade_curve, FadeCurve::EqualPower);
    assert_eq!(ra.asset, hash(1));
    // Undo restores the original clip.
    d.undo(&reg).unwrap();
    assert_eq!(doc_clip(d.doc(), id).content, ClipContent::Audio(a));
}

#[test]
fn split_uses_the_tempo_map_and_the_document_sample_rate() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    // 60 BPM for the first beat, then 120 BPM: beat 1 = 48000 frames, beat 2 = 24000.
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 60.0),
            TempoEvent::step(PPQ, 120.0),
        ])),
        &reg,
    )
    .unwrap();
    d.apply(add_audio_clip("audio-track", 0, 4 * PPQ, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::SplitClip { id, at: 2 * PPQ }), &reg)
        .unwrap();
    let ClipContent::Audio(ra) = &doc_clip(d.doc(), last(&d)).content else {
        panic!()
    };
    assert_eq!(ra.source_offset, 48_000 + 24_000);
}

#[test]
fn split_past_the_asset_end_clamps_the_offset_to_the_asset_length() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    // Asset 2 has 96000 frames (2 s); the clip is 4 beats (2 s) starting at source frame 90000.
    let a = AudioClipContent {
        source_offset: 90_000,
        ..audio(2)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 4 * PPQ, a), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::SplitClip { id, at: 2 * PPQ }), &reg)
        .unwrap();
    let ClipContent::Audio(ra) = &doc_clip(d.doc(), last(&d)).content else {
        panic!()
    };
    assert_eq!(ra.source_offset, 96_000);
}

#[test]
fn split_rejections_still_apply_to_audio_clips() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    for at in [0, BAR, -1, 2 * BAR] {
        assert!(matches!(
            rejects(&mut d, sc(SongCommand::SplitClip { id, at }), &reg),
            ModelError::OutOfRange(_)
        ));
    }
}

#[test]
fn duplicate_and_remove_audio_clips() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::DuplicateClip { id, start: 3 * BAR }), &reg)
        .unwrap();
    let copy = last(&d);
    assert_ne!(copy, id);
    assert_eq!(doc_clip(d.doc(), copy).start, 3 * BAR);
    assert_eq!(
        doc_clip(d.doc(), copy).content,
        doc_clip(d.doc(), id).content
    );
    d.apply(sc(SongCommand::RemoveClip { id }), &reg).unwrap();
    assert!(d.doc().song.clip(id).is_none());
    assert!(
        d.doc().assets.contains_key(&hash(1)),
        "pool is kept for undo"
    );
    d.undo(&reg).unwrap();
    assert!(d.doc().song.clip(id).is_some());
}

#[test]
fn track_flags_apply_to_audio_tracks() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("audio-track"),
            muted: true,
            solo: false,
            armed: true,
        }),
        &reg,
    )
    .unwrap();
    let t = &d.doc().song.tracks[0];
    assert!(t.muted && t.armed && !t.solo);
}

// ---- input channels -------------------------------------------------------------------------

#[test]
fn set_input_channels_validates_and_undoes() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    let ops = d.apply(Command::SetInputChannels(4), &reg).unwrap();
    assert!(ops.contains(&SyncOp::RoutingChanged));
    assert_eq!(d.doc().input_channels, 4);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().input_channels, 2);
    assert!(matches!(
        rejects(&mut d, Command::SetInputChannels(33), &reg),
        ModelError::OutOfRange(_)
    ));
}

#[test]
fn input_sockets_follow_input_channels() {
    let reg = audio_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, AUDIO_TRACK, Some("tape")), &reg).unwrap();
    d.apply(connect("interface:in_2", "tape:in_l"), &reg)
        .unwrap();
    assert!(matches!(
        rejects(&mut d, connect("interface:in_3", "tape:in_r"), &reg),
        ModelError::UnknownKey(_)
    ));
    assert!(matches!(
        rejects(&mut d, connect("interface:in_0", "tape:in_r"), &reg),
        ModelError::UnknownKey(_)
    ));
    // An interface input is an output: it cannot take a cable.
    assert!(matches!(
        rejects(&mut d, connect("tape:out_l", "interface:in_1"), &reg),
        ModelError::BadCable(_)
    ));
}

#[test]
fn shrinking_inputs_removes_the_cables_from_the_removed_inputs_in_the_same_step() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let before = d.doc().clone();
    d.apply(Command::SetInputChannels(1), &reg).unwrap();
    assert!(
        d.doc()
            .cables
            .contains(&cable("interface:in_1", "audio-track:in_l"))
    );
    assert!(
        !d.doc()
            .cables
            .contains(&cable("interface:in_2", "audio-track:in_r"))
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    // Setting the same value is a valid step that changes nothing.
    d.apply(Command::SetInputChannels(2), &reg).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---- pool cleanup ---------------------------------------------------------------------------

#[test]
fn referenced_assets_and_pruning() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(register(3, 10), &reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    d.apply(add_audio_clip("audio-track", BAR, BAR, 1), &reg)
        .unwrap();
    let want: std::collections::BTreeSet<String> = [hash(1)].into();
    assert_eq!(d.doc().referenced_assets(), want);
    assert_eq!(d.doc().assets.len(), 3);
    let mut doc = d.doc().clone();
    doc.prune_unreferenced_assets();
    assert_eq!(doc.assets.keys().cloned().collect::<Vec<_>>(), [hash(1)]);
    // The rack doc is untouched otherwise.
    assert_eq!(doc.song, d.doc().song);
}

#[test]
fn document_pruning_keeps_what_undo_and_redo_still_need() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::RemoveClip { id }), &reg).unwrap();
    // Asset 1 is unreferenced by the song, but undo restores a clip using it; asset 2 was
    // never used by any step.
    assert!(d.doc().referenced_assets().is_empty());
    assert_eq!(d.referenced_assets(), [hash(1)].into());
    d.prune_unreferenced_assets();
    assert_eq!(
        d.doc().assets.keys().cloned().collect::<Vec<_>>(),
        [hash(1)]
    );
    d.undo(&reg).unwrap();
    assert!(d.doc().song.clip(id).is_some());
    // Undone steps (the redo history) keep their assets as well.
    d.undo(&reg).unwrap();
    assert!(d.doc().song.clip(id).is_none());
    d.prune_unreferenced_assets();
    assert!(d.doc().assets.contains_key(&hash(1)));
    d.redo(&reg).unwrap();
    d.redo(&reg).unwrap();
    assert!(d.doc().song.clip(id).is_none());
}

// ---- cable batches (input selector) -----------------------------------------------------------

fn input_cables(d: &Document) -> Vec<String> {
    let mut v: Vec<String> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.to.device.0 == "audio-track")
        .map(|c| format!("{}>{}", c.from.socket, c.to.socket))
        .collect();
    v.sort();
    v
}

#[test]
fn a_cable_batch_is_one_undo_step_and_reports_routing_once() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    d.apply(add_audio_track(), &reg).unwrap();
    d.apply(Command::SetInputChannels(4), &reg).unwrap();
    let before = input_cables(&d);
    assert_eq!(before, ["in_1>in_l", "in_2>in_r"]);
    let ops = d
        .apply(
            Command::Batch(vec![
                Command::Disconnect {
                    to: sock("audio-track:in_l"),
                },
                Command::Disconnect {
                    to: sock("audio-track:in_r"),
                },
                connect("interface:in_3", "audio-track:in_l"),
                connect("interface:in_4", "audio-track:in_r"),
            ]),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [routing()]);
    assert_eq!(input_cables(&d), ["in_3>in_l", "in_4>in_r"]);
    d.undo(&reg).unwrap();
    assert_eq!(input_cables(&d), before, "one undo restores the old inputs");
    d.redo(&reg).unwrap();
    assert_eq!(input_cables(&d), ["in_3>in_l", "in_4>in_r"]);
}

#[test]
fn a_cable_batch_is_validated_in_order_and_rejected_as_a_whole() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    d.apply(add_audio_track(), &reg).unwrap();
    // Connecting an occupied input fails: the earlier disconnect must not stay applied.
    let e = rejects(
        &mut d,
        Command::Batch(vec![
            Command::Disconnect {
                to: sock("audio-track:in_l"),
            },
            connect("interface:in_1", "audio-track:in_r"),
        ]),
        &reg,
    );
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // Only cable commands may be batched.
    let e = rejects(&mut d, Command::Batch(vec![Command::SetTempo(100.0)]), &reg);
    assert!(matches!(e, ModelError::BadBatch(_)), "{e:?}");
    // An empty batch is rejected (it would record an empty step).
    rejects(&mut d, Command::Batch(Vec::new()), &reg);
}

// ---- the clip inspector's edits (fades, transpose on several clips) -----------------------------

fn set_audio(
    d: &Document,
    id: atomik_model::ClipId,
    f: impl Fn(&mut AudioClipContent),
    merge: Option<u64>,
) -> Command {
    let c = doc_clip(d.doc(), id);
    let ClipContent::Audio(a) = &c.content else {
        panic!("not audio")
    };
    let mut a = a.clone();
    f(&mut a);
    sc(SongCommand::SetAudioClip {
        id,
        start: c.start,
        length: c.length,
        muted: c.muted,
        content: a,
        merge,
    })
}

fn two_clips(reg: &Registry) -> (Document, atomik_model::ClipId, atomik_model::ClipId) {
    let mut d = with_audio_track(reg);
    d.apply(add_audio_clip("audio-track", 0, 15_360, 1), reg)
        .unwrap();
    let a = last(&d);
    d.apply(add_audio_clip("audio-track", 30_720, 15_360, 2), reg)
        .unwrap();
    (d.clone(), a, last(&d))
}

#[test]
fn a_fade_drag_over_two_clips_is_one_merge_group_and_one_undo_step() {
    let reg = audio_registry();
    let (mut d, a, b) = two_clips(&reg);
    let before = d.doc().clone();
    // Three moves of a drag, each sent to both clips under one merge id.
    for fade in [100u64, 200, 300] {
        for id in [a, b] {
            let cmd = set_audio(&d, id, |c| c.fade_in = fade, Some(9));
            d.apply(cmd, &reg).unwrap();
        }
    }
    for id in [a, b] {
        let ClipContent::Audio(c) = &doc_clip(d.doc(), id).content else {
            panic!()
        };
        assert_eq!(c.fade_in, 300);
    }
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "one undo reverts the whole drag on both clips"
    );
}

#[test]
fn a_transpose_batch_over_two_clips_is_one_undo_step() {
    let reg = audio_registry();
    let (mut d, a, b) = two_clips(&reg);
    let before = d.doc().clone();
    let cmds = [a, b]
        .map(|id| set_audio(&d, id, |c| c.warp.transpose_cents = 700, None))
        .to_vec();
    d.apply(Command::Batch(cmds), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_reversed_clip_refuses_a_transposition() {
    let reg = audio_registry();
    let (mut d, a, _) = two_clips(&reg);
    let cmd = set_audio(&d, a, |c| c.reverse = true, None);
    d.apply(cmd, &reg).unwrap();
    let cmd = set_audio(&d, a, |c| c.warp.transpose_cents = 100, None);
    rejects(&mut d, cmd, &reg);
}

// ---- warp markers, split at markers, asset names ---------------------------------------------

fn markers(d: &Document, id: atomik_model::ClipId) -> Vec<atomik_model::WarpMarker> {
    match &doc_clip(d.doc(), id).content {
        ClipContent::Audio(a) => a.warp.markers.clone(),
        _ => panic!("not audio"),
    }
}

fn wm(tick: i64, source: u64) -> atomik_model::WarpMarker {
    atomik_model::WarpMarker { tick, source }
}

#[test]
fn set_warp_markers_is_one_undo_step_and_keeps_the_rest_of_the_content() {
    let reg = audio_registry();
    let (mut d, a, _) = two_clips(&reg);
    let before = d.doc().clone();
    let list = vec![wm(0, 0), wm(4000, 9000), wm(15_360, 40_000)];
    let ops = d
        .apply(
            sc(SongCommand::SetWarpMarkers {
                id: a,
                markers: list.clone(),
                merge: None,
            }),
            &reg,
        )
        .unwrap();
    assert!(ops.contains(&SyncOp::SongChanged));
    assert_eq!(markers(&d, a), list);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(markers(&d, a), list);
}

#[test]
fn a_marker_drag_is_one_merge_group() {
    let reg = audio_registry();
    let (mut d, a, _) = two_clips(&reg);
    let before = d.doc().clone();
    for tick in [4000, 4100, 4200] {
        d.apply(
            sc(SongCommand::SetWarpMarkers {
                id: a,
                markers: vec![wm(0, 0), wm(tick, 9000), wm(15_360, 40_000)],
                merge: Some(7),
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(markers(&d, a)[1].tick, 4200);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_warp_markers_rejections() {
    let reg = audio_registry();
    let (mut d, a, _) = two_clips(&reg);
    let set = |markers| {
        sc(SongCommand::SetWarpMarkers {
            id: a,
            markers,
            merge: None,
        })
    };
    // Equal to the current list.
    rejects(&mut d, set(Vec::new()), &reg);
    // Not increasing in source.
    rejects(&mut d, set(vec![wm(0, 100), wm(10, 100)]), &reg);
    // Past the end of the clip.
    rejects(&mut d, set(vec![wm(0, 0), wm(15_361, 100)]), &reg);
}

#[test]
fn split_at_every_marker_is_one_batch_and_one_undo_step() {
    let reg = audio_registry();
    let (mut d, a, _) = two_clips(&reg);
    d.apply(
        sc(SongCommand::SetWarpMarkers {
            id: a,
            markers: vec![
                wm(0, 0),
                wm(3000, 9000),
                wm(9000, 20_000),
                wm(15_360, 40_000),
            ],
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    let first = doc_clip(d.doc(), a);
    let start = first.start;
    let next = d.doc().song.next_clip_id;
    d.apply(
        Command::Batch(vec![
            sc(SongCommand::SplitClip {
                id: a,
                at: start + 3000,
            }),
            sc(SongCommand::SplitClip {
                id: atomik_model::ClipId(next),
                at: start + 9000,
            }),
        ]),
        &reg,
    )
    .unwrap();
    let lens: Vec<i64> = [
        a,
        atomik_model::ClipId(next),
        atomik_model::ClipId(next + 1),
    ]
    .iter()
    .map(|id| doc_clip(d.doc(), *id).length)
    .collect();
    assert_eq!(lens, vec![3000, 6000, 6360]);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn rename_asset_is_an_undoable_edit() {
    let reg = audio_registry();
    let mut d = with_assets(&reg);
    let id = hash(1);
    let old = d.doc().assets[&id].original_name.clone();
    d.apply(
        Command::RenameAsset {
            asset: id.clone(),
            name: "  Take 4  ".into(),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().assets[&id].original_name, "Take 4");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().assets[&id].original_name, old);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().assets[&id].original_name, "Take 4");
    for bad in ["", "   ", "tab\there", &"x".repeat(201)] {
        rejects(
            &mut d,
            Command::RenameAsset {
                asset: id.clone(),
                name: bad.into(),
            },
            &reg,
        );
    }
    // The same name, and an unknown asset.
    rejects(
        &mut d,
        Command::RenameAsset {
            asset: id,
            name: "Take 4".into(),
        },
        &reg,
    );
    rejects(
        &mut d,
        Command::RenameAsset {
            asset: hash(99),
            name: "x".into(),
        },
        &reg,
    );
}
