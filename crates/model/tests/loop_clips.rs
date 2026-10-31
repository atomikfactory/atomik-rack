//! Looped clips and clip names and colours: the loop conversions play what the loop played, a
//! split of a looped clip leaves two looped clips, and names and colours set, clear, undo, travel
//! with copies and splits and survive the project and clip files.

mod common;

use atomik_model::song::{
    flatten_audio, flatten_automation, flatten_controllers, flatten_notes_with,
};
use atomik_model::{
    Clip, ClipContent, ClipId, ClipSnapshot, Command, ControllerKind, ControllerLane, Curve,
    Document, LoopConvert, MetaChange, ModelError, SongCommand, StepGrid, StepResolution,
    WorkflowCommand, empty_doc, load_clip_file, load_project, save_project,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

fn synth_doc(r: &atomik_device_sdk::Registry, clips: Vec<Clip>) -> Document {
    synth_doc_with(r, clips, false)
}

fn synth_doc_with(r: &atomik_device_sdk::Registry, clips: Vec<Clip>, tails: bool) -> Document {
    let mut doc = empty_doc();
    doc.song_mut().tails = tails;
    let mut d = Document::new(doc);
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
        ],
    );
    for c in clips {
        let lane = usize::from(matches!(c.content, ClipContent::Automation(_)));
        run(&mut d, r, [add_clip("synth", lane, c)]);
    }
    d
}

fn clips_of(d: &Document, lane: usize) -> Vec<Clip> {
    d.doc().song.track(&key("synth")).unwrap().lanes[lane]
        .clips
        .clone()
}

fn meta(ids: Vec<ClipId>, name: MetaChange<String>, color: MetaChange<[u8; 3]>) -> Command {
    sc(SongCommand::SetClipMeta { ids, name, color })
}

/// Everything the synth track plays from its note and automation lanes.
type Played = (
    Vec<(i64, i64, u8, f32)>,
    Vec<(i64, f32, Curve)>,
    Vec<(ControllerKind, Vec<(i64, f32, Curve)>)>,
);

fn played(d: &Document) -> Played {
    let song = &d.doc().song;
    let t = song.track(&key("synth")).unwrap();
    (
        flatten_notes_with(t, song.tails),
        flatten_automation(t, 1),
        flatten_controllers(t),
    )
}

// ------------------------------------------------------------------------ names and colours

fn two_clips(r: &atomik_device_sdk::Registry) -> (Document, ClipId, ClipId) {
    let d = synth_doc(
        r,
        vec![
            notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)]),
            notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 64, 0.8)]),
        ],
    );
    let ids = clip_ids(d.doc());
    (d, ids[0], ids[1])
}

#[test]
fn a_name_and_a_colour_are_set_cleared_and_undone() {
    let r = registry();
    let (mut d, a, b) = two_clips(&r);
    let before = d.doc().clone();
    one_step(
        &mut d,
        &r,
        meta(
            vec![a, b],
            MetaChange::Set("  Verse ".into()),
            MetaChange::Set([200, 30, 90]),
        ),
    );
    for c in clips_of(&d, 0) {
        assert_eq!(c.name.as_deref(), Some("Verse"), "the name is trimmed");
        assert_eq!(c.color, Some([200, 30, 90]));
    }
    assert_eq!(d.undo_label(), Some("Rename Clip"));
    d.undo(&r).unwrap();
    assert_eq!(*d.doc().song, *before.song);
    d.redo(&r).unwrap();
    // Colour only: the names stay.
    one_step(
        &mut d,
        &r,
        meta(vec![a], MetaChange::Keep, MetaChange::Set([1, 2, 3])),
    );
    assert_eq!(d.undo_label(), Some("Colour Clips"));
    let c = &clips_of(&d, 0)[0];
    assert_eq!(
        (c.name.as_deref(), c.color),
        (Some("Verse"), Some([1, 2, 3]))
    );
    // A blank name clears it, Clear clears the colour; the other clip is untouched.
    one_step(
        &mut d,
        &r,
        meta(vec![a], MetaChange::Set("   ".into()), MetaChange::Clear),
    );
    let c = &clips_of(&d, 0);
    assert_eq!((c[0].name.clone(), c[0].color), (None, None));
    assert_eq!(c[1].name.as_deref(), Some("Verse"));
    d.undo(&r).unwrap();
    assert_eq!(clips_of(&d, 0)[0].name.as_deref(), Some("Verse"));
}

#[test]
fn clip_meta_rejects_what_it_cannot_apply() {
    let r = registry();
    let (mut d, a, _) = two_clips(&r);
    let set = |name: &str| meta(vec![a], MetaChange::Set(name.into()), MetaChange::Keep);
    assert!(matches!(
        rejected(&mut d, &r, set(&"x".repeat(65))),
        ModelError::OutOfRange(_)
    ));
    assert!(matches!(
        rejected(&mut d, &r, set("two\nlines")),
        ModelError::BadSong(_)
    ));
    assert!(matches!(
        rejected(
            &mut d,
            &r,
            meta(vec![ClipId(99)], MetaChange::Clear, MetaChange::Keep)
        ),
        ModelError::UnknownKey(_)
    ));
    assert!(matches!(
        rejected(
            &mut d,
            &r,
            meta(vec![], MetaChange::Clear, MetaChange::Clear)
        ),
        ModelError::BadSong(_)
    ));
    // Nothing to change.
    assert!(matches!(
        rejected(
            &mut d,
            &r,
            meta(vec![a], MetaChange::Keep, MetaChange::Keep)
        ),
        ModelError::BadSong(_)
    ));
    assert!(matches!(
        rejected(
            &mut d,
            &r,
            meta(vec![a], MetaChange::Clear, MetaChange::Clear)
        ),
        ModelError::BadSong(_)
    ));
    // 64 characters are fine.
    one_step(&mut d, &r, set(&"x".repeat(64)));
}

#[test]
fn names_and_colours_follow_copies_splits_and_joins() {
    let r = registry();
    let (mut d, a, b) = two_clips(&r);
    one_step(
        &mut d,
        &r,
        meta(
            vec![a],
            MetaChange::Set("Hook".into()),
            MetaChange::Set([9, 8, 7]),
        ),
    );
    // Split: both halves keep them.
    one_step(
        &mut d,
        &r,
        sc(SongCommand::SplitClip { id: a, at: BAR / 2 }),
    );
    let halves = clips_of(&d, 0);
    let hooks: Vec<_> = halves.iter().filter(|c| c.name.is_some()).collect();
    assert_eq!(hooks.len(), 2);
    assert!(hooks.iter().all(|c| c.color == Some([9, 8, 7])));
    // Duplicate and the clipboard keep them.
    one_step(
        &mut d,
        &r,
        sc(SongCommand::DuplicateClip {
            id: a,
            start: 8 * BAR,
        }),
    );
    let snap = ClipSnapshot::capture(d.doc(), &[a]).unwrap();
    assert_eq!(snap.entries[0].clip.name.as_deref(), Some("Hook"));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteClips {
            track: None,
            lane: None,
            at_tick: 12 * BAR,
            clips: snap,
        }),
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateClips { ids: vec![b] }),
    );
    let named = clips_of(&d, 0).iter().filter(|c| c.name.is_some()).count();
    assert_eq!(named, 4, "two halves, the duplicate and the paste");
    // The duplicate of an unnamed clip stays unnamed.
    assert_eq!(
        clips_of(&d, 0).iter().filter(|c| c.name.is_none()).count(),
        2
    );
}

#[test]
fn names_and_colours_survive_the_project_file() {
    let r = registry();
    let (mut d, a, b) = two_clips(&r);
    one_step(
        &mut d,
        &r,
        meta(
            vec![a],
            MetaChange::Set("Café \"A\"".into()),
            MetaChange::Set([255, 0, 16]),
        ),
    );
    one_step(
        &mut d,
        &r,
        meta(vec![b], MetaChange::Keep, MetaChange::Set([1, 2, 3])),
    );
    let text = save_project(d.doc());
    assert!(
        text.contains("\"muted\": false, \"name\": \"Café \\\"A\\\"\", \"color\": \"#ff0010\"")
    );
    assert!(text.contains("\"muted\": false, \"color\": \"#010203\""));
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(*loaded.song, *d.doc().song);
    assert_eq!(save_project(&loaded), text);
    // A project that names nothing writes neither field.
    let plain = synth_doc(&r, vec![notes_clip(0, BAR, vec![])]);
    let text = save_project(plain.doc());
    assert!(!text.contains("\"muted\": false, \"name\""));
    assert!(!text.contains("\"muted\": false, \"color\""));
    // A bad name or colour names its path.
    let at = "\"loop_length\": null, \"muted\": false,";
    let bad_name = text.replacen(at, &format!("{at} \"name\": \" x\","), 1);
    let e = load_project(&bad_name, &r).unwrap_err().to_string();
    assert!(e.contains("clips[0].name"), "{e}");
    let bad_color = text.replacen(at, &format!("{at} \"color\": \"red\","), 1);
    let e = load_project(&bad_color, &r).unwrap_err().to_string();
    assert!(e.contains("clips[0].color"), "{e}");
}

#[test]
fn names_and_colours_survive_the_clip_file() {
    let mut c = notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)]);
    c.name = Some("Riff".into());
    c.color = Some([10, 20, 30]);
    let file = atomik_model::ClipFile {
        tempo_bpm: 120.0,
        meter: (4, 4),
        clip: c,
    };
    let text = atomik_model::save_clip_file(&file).unwrap();
    assert!(text.contains("\"name\": \"Riff\", \"color\": \"#0a141e\""));
    let back = load_clip_file(&text).unwrap();
    assert_eq!(back, file);
    assert_eq!(atomik_model::save_clip_file(&back).unwrap(), text);
}

// --------------------------------------------------------------------------------- loops

fn loop_spec() -> impl Strategy<Value = (i64, i64, i64, i64, bool)> {
    // start, length, loop length, offset, with swing (the offset and loop in sixteenth steps).
    let s = PPQ / 4;
    (0i64..8, 1i64..40, 1i64..9, 0i64..24, any::<bool>())
        .prop_map(move |(start, len, lp, off, swing)| (start * s, len * s, lp * s, off * s, swing))
}

fn notes_for(loop_length: i64) -> Vec<atomik_model::Note> {
    let s = PPQ / 4;
    let mut v = Vec::new();
    for i in 0..(loop_length / s + 4) {
        v.push(note(i * s, s + (i % 3) * s, 48 + (i % 5) as u8, 0.5));
    }
    v
}

fn looped_notes(
    (start, len, lp, off, swing): (i64, i64, i64, i64, bool),
    with_controllers: bool,
) -> Clip {
    let mut c = notes_clip(start, len, notes_for(lp));
    c.offset = off;
    c.loop_length = Some(lp);
    if with_controllers {
        c.controllers = vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![
                point(0, 0.1, Curve::Linear),
                point(lp / 2, 0.9, Curve::Linear),
                point(lp + PPQ, 0.2, Curve::Step),
            ],
        )];
    }
    if swing {
        c.grid = Some(StepGrid {
            resolution: StepResolution::Sixteenth,
            steps: 16,
            swing: 30,
        });
    }
    c
}

fn looped_points(spec: (i64, i64, i64, i64, bool)) -> Clip {
    let (start, len, lp, off, _) = spec;
    let mut c = auto_clip(
        start,
        len,
        vec![
            point(0, 0.2, Curve::Linear),
            point(lp / 2 + 1, 0.8, Curve::Step),
            point(lp + PPQ, 0.4, Curve::Linear),
            point(lp + 2 * PPQ, 0.9, Curve::Step),
        ],
    );
    c.offset = off;
    c.loop_length = Some(lp);
    c
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn separate_and_single_play_what_the_loop_played(
        spec in loop_spec(),
        tails in any::<bool>(),
        controllers in any::<bool>(),
    ) {
        let r = registry();
        for mode in [LoopConvert::Separate, LoopConvert::Single] {
            let mut d = synth_doc_with(
                &r,
                vec![looped_notes(spec, controllers), looped_points(spec)],
                tails,
            );
            let ids = clip_ids(d.doc());
            let before = played(&d);
            let swing = spec.4;
            let res = d.apply(wf(WorkflowCommand::ConvertLoop { ids: ids.clone(), mode }), &r);
            if mode == LoopConvert::Single && swing {
                prop_assert!(matches!(res, Err(ModelError::BadSong(_))));
                continue;
            }
            prop_assert!(res.is_ok(), "{res:?}");
            prop_assert_eq!(played(&d), before.clone());
            // No clip loops any more, the song validates, and undo gives the loop back.
            prop_assert!(clips_of(&d, 0).iter().all(|c| c.loop_length.is_none()));
            prop_assert!(clips_of(&d, 1).iter().all(|c| c.loop_length.is_none()));
            d.undo(&r).unwrap();
            prop_assert_eq!(played(&d), before);
            prop_assert!(clips_of(&d, 0)[0].loop_length.is_some());
        }
    }

    #[test]
    fn a_loop_instance_made_independent_changes_no_sound(
        spec in loop_spec(),
        pick in 0usize..64,
    ) {
        let r = registry();
        let mut d = synth_doc(&r, vec![looped_notes(spec, true), looped_points(spec)]);
        let ids = clip_ids(d.doc());
        let before = played(&d);
        for id in ids {
            let c = atomik_model::Clip::clone(d.doc().song.clip(id).unwrap().2);
            let n = (c.length / c.loop_length.unwrap()) as usize + 2;
            let res = d.apply(
                wf(WorkflowCommand::DuplicateLoopInstance { id, index: pick % n }),
                &r,
            );
            if res.is_ok() {
                prop_assert_eq!(played(&d), before.clone());
            } else {
                prop_assert!(matches!(
                    res,
                    Err(ModelError::OutOfRange(_) | ModelError::BadSong(_))
                ));
            }
        }
    }
}

#[test]
fn a_looped_clip_splits_into_two_looped_clips_that_play_the_same() {
    let r = registry();
    let lp = BAR / 2;
    let mut c = notes_clip(BAR, 3 * BAR, notes_for(lp));
    c.offset = PPQ / 2;
    c.loop_length = Some(lp);
    // With tails on, a note keeps its length across the cut; without, the cut ends it.
    let mut d = synth_doc_with(&r, vec![c], true);
    let before = played(&d);
    one_step(
        &mut d,
        &r,
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: BAR + 2 * BAR + PPQ / 3,
        }),
    );
    let clips = clips_of(&d, 0);
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().all(|c| c.loop_length == Some(lp)));
    assert_eq!(played(&d), before);
}

#[test]
fn converting_needs_a_loop_and_room_on_the_lane() {
    let r = registry();
    // Nothing loops.
    let mut d = synth_doc(&r, vec![notes_clip(0, BAR, notes_for(BAR))]);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Separate,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("loops")));
    // Another clip overlapping the loop would change who hides whom.
    let mut looped = notes_clip(0, 4 * BAR, notes_for(BAR));
    looped.loop_length = Some(BAR);
    let mut d = synth_doc(
        &r,
        vec![looped, notes_clip(BAR + BAR / 2, BAR, notes_for(BAR))],
    );
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Separate,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("overlaps")));
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateLoopInstance {
            id: ClipId(1),
            index: 1,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)));
    // Single does not move any clip, so it works there.
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Single,
        }),
    );
    assert!(clips_of(&d, 0)[0].loop_length.is_none());
}

#[test]
fn separate_gives_each_repetition_its_own_clip_and_ids_in_order() {
    let r = registry();
    let mut c = notes_clip(BAR, 3 * BAR + BAR / 2, notes_for(BAR));
    c.offset = BAR / 4;
    c.loop_length = Some(BAR);
    c.name = Some("Loop".into());
    let mut d = synth_doc(&r, vec![c]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Separate,
        }),
    );
    let clips = clips_of(&d, 0);
    // The first repetition is cut short by the offset, then three more.
    let shape: Vec<_> = clips
        .iter()
        .map(|c| (c.id.0, c.start, c.length, c.offset))
        .collect();
    assert_eq!(
        shape,
        [
            (1, BAR, 3 * BAR / 4, BAR / 4),
            (2, 7 * BAR / 4, BAR, 0),
            (3, 11 * BAR / 4, BAR, 0),
            (4, 15 * BAR / 4, 3 * BAR / 4, 0),
        ]
    );
    assert!(clips.iter().all(|c| c.name.as_deref() == Some("Loop")));
    assert_eq!(d.last_clip_id(), Some(ClipId(4)));
}

#[test]
fn audio_loops_convert_to_separate_clips_but_not_to_one() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(
        add_audio_clip("audio-track", BAR, 3 * BAR + BAR / 2, 1),
        &reg,
    )
    .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: BAR,
            length: 3 * BAR + BAR / 2,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let song = d.doc().song.clone();
    let before = flatten_audio(song.track(&key("audio-track")).unwrap(), &song, 48_000);
    assert_eq!(before.len(), 4);
    let e = rejected(
        &mut d,
        &reg,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![id],
            mode: LoopConvert::Single,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("audio")));
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![id],
            mode: LoopConvert::Separate,
        }),
    );
    let song = d.doc().song.clone();
    assert_eq!(song.tracks[0].lanes[0].clips.len(), 4);
    assert_eq!(
        flatten_audio(song.track(&key("audio-track")).unwrap(), &song, 48_000),
        before
    );
}

#[test]
fn a_looped_audio_clip_splits_only_on_a_repetition_boundary() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 4 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 4 * BAR,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let song = d.doc().song.clone();
    let before = flatten_audio(song.track(&key("audio-track")).unwrap(), &song, 48_000);
    let e = rejected(&mut d, &reg, sc(SongCommand::SplitClip { id, at: BAR + 5 }));
    assert!(matches!(e, ModelError::OutOfRange(m) if m.contains("repetition")));
    one_step(&mut d, &reg, sc(SongCommand::SplitClip { id, at: 2 * BAR }));
    let song = d.doc().song.clone();
    let clips = &song.tracks[0].lanes[0].clips;
    assert!(
        clips
            .iter()
            .all(|c| c.loop_length == Some(BAR) && c.offset == 0)
    );
    assert_eq!(
        flatten_audio(song.track(&key("audio-track")).unwrap(), &song, 48_000),
        before
    );
}

#[test]
fn takes_refuse_a_looped_audio_clip() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 2 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 2 * BAR,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let e = rejected(
        &mut d,
        &reg,
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![id],
            extra: vec![],
        }),
    );
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("loop")));
    // A reversed clip cannot loop, nor a looped one reverse.
    let e = rejected(
        &mut d,
        &reg,
        wf(WorkflowCommand::ReverseClips {
            ids: vec![id],
            graphical: false,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)));
}

#[test]
fn the_loop_commands_are_listed_in_the_undo_labels() {
    let r = registry();
    let mut c = notes_clip(0, 2 * BAR, notes_for(BAR));
    c.loop_length = Some(BAR);
    let mut d = synth_doc(&r, vec![c]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateLoopInstance {
            id: ClipId(1),
            index: 1,
        }),
    );
    assert_eq!(d.undo_label(), Some("Make Loop Instance Independent"));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Single,
        }),
    );
    assert_eq!(d.undo_label(), Some("Convert Loop"));
}

#[test]
fn a_looped_audio_clip_with_a_name_round_trips_through_the_project_file() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", BAR, 3 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: BAR,
            length: 3 * BAR,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        meta(
            vec![id],
            MetaChange::Set("Break".into()),
            MetaChange::Set([5, 6, 7]),
        ),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    assert!(text.contains(
        "\"offset\": 0, \"loop_length\": 61440, \"muted\": false, \"name\": \"Break\", \"color\": \"#050607\""
    ));
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(*loaded.song, *d.doc().song);
    assert_eq!(save_project(&loaded), text);
    // A reversed clip cannot loop in a file either.
    let bad = text.replacen(
        "\"fade_curve\": \"linear\"",
        "\"fade_curve\": \"linear\", \"reverse\": true",
        1,
    );
    let e = load_project(&bad, &reg).unwrap_err().to_string();
    assert!(e.contains("cannot loop"), "{e}");
}
