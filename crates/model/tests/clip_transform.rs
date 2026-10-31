//! Reverse, scale tempo and chop clips: each is one undo step, two reverses give the clip back,
//! a chop plays what the clip played, and a scale and its inverse agree within a tick.

mod common;

use atomik_model::song::flatten_notes;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ControllerKind, ControllerLane, Curve,
    Document, ModelError, Note, SongCommand, WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

fn reverse(ids: Vec<ClipId>, graphical: bool) -> Command {
    wf(WorkflowCommand::ReverseClips { ids, graphical })
}

fn scale(ids: Vec<ClipId>, percent: f64) -> Command {
    wf(WorkflowCommand::ScaleTempo { ids, percent })
}

fn chop(ids: Vec<ClipId>, length: i64) -> Command {
    wf(WorkflowCommand::ChopClips { ids, length })
}

fn synth_doc(r: &atomik_device_sdk::Registry, clip: Clip) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, clip),
        ],
    );
    d
}

fn the_clip(d: &Document, i: usize) -> Clip {
    d.doc().song.track(&key("synth")).unwrap().lanes[0].clips[i].clone()
}

fn notes_of(c: &Clip) -> Vec<(i64, i64, u8)> {
    let ClipContent::Notes(n) = &c.content else {
        panic!("notes")
    };
    n.iter().map(|n| (n.tick, n.length, n.key)).collect()
}

#[test]
fn reversing_notes_mirrors_them_as_bars_or_by_their_starts() {
    let r = registry();
    let q = PPQ / 4;
    let clip = notes_clip(
        BAR,
        BAR,
        vec![note(0, q, 60, 0.8), note(PPQ, 2 * q, 62, 0.8)],
    );
    let mut d = synth_doc(&r, clip);
    let id = the_clip(&d, 0).id;
    one_step(&mut d, &r, reverse(vec![id], true));
    // Bars: [0, q) lands at the end, [PPQ, PPQ + 2q) ends where the first began.
    assert_eq!(
        notes_of(&the_clip(&d, 0)),
        [(BAR - PPQ - 2 * q, 2 * q, 62), (BAR - q, q, 60)]
    );
    d.undo(&r).unwrap();
    one_step(&mut d, &r, reverse(vec![id], false));
    // Starts only: the first note stays on the downbeat, the other mirrors, lengths stay.
    assert_eq!(
        notes_of(&the_clip(&d, 0)),
        [(0, q, 60), (BAR - PPQ, 2 * q, 62)]
    );
}

#[test]
fn reversing_automation_moves_the_curves_with_their_segments() {
    let r = registry();
    let mut d = synth_doc(&r, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)]));
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(
                    0,
                    BAR,
                    vec![
                        point(0, 100.0, Curve::Linear),
                        point(PPQ, 200.0, Curve::Step),
                        point(2 * PPQ, 300.0, Curve::Linear),
                    ],
                ),
            ),
        ],
    );
    let id = d.doc().song.track(&key("synth")).unwrap().lanes[1].clips[0].id;
    one_step(&mut d, &r, reverse(vec![id], false));
    let ClipContent::Automation(p) =
        &d.doc().song.track(&key("synth")).unwrap().lanes[1].clips[0].content
    else {
        panic!("automation")
    };
    let got: Vec<(i64, f32, Curve)> = p.iter().map(|x| (x.tick, x.value, x.curve)).collect();
    assert_eq!(
        got,
        [
            (BAR - 1 - 2 * PPQ, 300.0, Curve::Step),
            (BAR - 1 - PPQ, 200.0, Curve::Linear),
            (BAR - 1, 100.0, Curve::Linear)
        ]
    );
    // Twice is the identity.
    one_step(&mut d, &r, reverse(vec![id], false));
    let ClipContent::Automation(p) =
        &d.doc().song.track(&key("synth")).unwrap().lanes[1].clips[0].content
    else {
        panic!("automation")
    };
    assert_eq!(p[0], point(0, 100.0, Curve::Linear));
    assert_eq!(p[2], point(2 * PPQ, 300.0, Curve::Linear));
}

#[test]
fn a_mixed_selection_reverses_notes_automation_and_audio_together() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(
        &mut d,
        &r,
        [
            add(1, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(0, BAR, vec![note(0, PPQ / 4, 60, 0.8)]),
            ),
            add_audio_clip("audio-track", 0, BAR, 1),
        ],
    );
    let note_id = the_clip(&d, 0).id;
    let audio_id = d.doc().song.track(&key("audio-track")).unwrap().lanes[0].clips[0].id;
    let was = text(&d);
    one_step(&mut d, &r, reverse(vec![note_id, audio_id], false));
    let ClipContent::Audio(a) = &doc_clip(d.doc(), audio_id).content else {
        panic!("audio")
    };
    assert!(a.reverse);
    one_step(&mut d, &r, reverse(vec![note_id, audio_id], false));
    assert_eq!(text(&d), was);
    // Audio alone in a selection of clips that are not all reversed flips them all.
    let e = rejected(&mut d, &r, reverse(Vec::new(), false));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

proptest! {
    /// Two reverses give the clip back exactly: notes, controllers and automation.
    #[test]
    fn reverse_twice_restores_the_clip(
        slots in proptest::collection::btree_set((0i64..60, 60u8..64), 1..12),
        graphical in any::<bool>(),
        offset in 0i64..3,
        looped in any::<bool>(),
    ) {
        let r = registry();
        let q = PPQ / 4;
        let notes: Vec<Note> = slots
            .iter()
            .map(|&(s, k)| note(s * q, q, k, 0.7))
            .collect();
        let mut clip = notes_clip(BAR, 4 * BAR, notes);
        clip.offset = offset * q;
        if looped {
            clip.loop_length = Some(BAR);
        }
        clip.controllers = vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![
                AutomationPoint { tick: q, value: 0.2, curve: Curve::Linear },
                AutomationPoint { tick: PPQ, value: 0.9, curve: Curve::Step },
                AutomationPoint { tick: 3 * PPQ, value: 0.4, curve: Curve::Linear },
            ],
        )];
        let mut d = synth_doc(&r, clip);
        let id = the_clip(&d, 0).id;
        let before = text(&d);
        let first = d.apply(reverse(vec![id], graphical), &r);
        if first.is_ok() {
            d.apply(reverse(vec![id], graphical), &r).unwrap();
            prop_assert_eq!(text(&d), before);
        }
    }

    /// A chop plays exactly what the clip played, and makes pieces of the asked length.
    #[test]
    fn chop_keeps_what_plays(
        slots in proptest::collection::btree_set((0i64..16, 60u8..63), 1..10),
        piece in 1i64..5,
        start in 0i64..3,
    ) {
        let r = registry();
        let notes: Vec<Note> = slots
            .iter()
            .map(|&(s, k)| note(s * (PPQ / 4), PPQ / 8, k, 0.7))
            .collect();
        let mut d = synth_doc(&r, notes_clip(start * BAR, BAR, notes));
        let id = the_clip(&d, 0).id;
        let t = |d: &Document| flatten_notes(d.doc().song.track(&key("synth")).unwrap());
        let before = t(&d);
        let length = piece * PPQ / 2;
        let res = d.apply(chop(vec![id], length), &r);
        if res.is_ok() {
            let clips = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
            let total: i64 = clips.iter().map(|c| c.length).sum();
            prop_assert_eq!(total, BAR);
            for c in &clips[..clips.len() - 1] {
                prop_assert_eq!(c.length, length);
            }
            // Notes a piece cuts short stay as they were here (the notes are shorter than a piece
            // boundary gap), so the flatten output is identical.
            prop_assert_eq!(t(&d), before);
            d.undo(&r).unwrap();
            prop_assert_eq!(d.doc().song.track(&key("synth")).unwrap().lanes[0].clips.len(), 1);
        }
    }

    /// Scale then the inverse agree within a tick.
    #[test]
    fn scale_round_trip_is_within_a_tick(
        slots in proptest::collection::btree_set((0i64..40, 60u8..64), 1..10),
        pct in prop_oneof![Just(200.0f64), Just(50.0), Just(150.0), Just(75.0)],
    ) {
        let r = registry();
        let notes: Vec<Note> = slots
            .iter()
            .map(|&(s, k)| note(s * 777, 500, k, 0.7))
            .collect();
        let mut d = synth_doc(&r, notes_clip(BAR, 3 * BAR, notes));
        let id = the_clip(&d, 0).id;
        let before = notes_of(&the_clip(&d, 0));
        d.apply(scale(vec![id], pct), &r).unwrap();
        d.apply(scale(vec![id], 10_000.0 / pct), &r).unwrap();
        let after = notes_of(&the_clip(&d, 0));
        prop_assert_eq!(before.len(), after.len());
        for (b, a) in before.iter().zip(&after) {
            prop_assert!((b.0 - a.0).abs() <= 2 && (b.1 - a.1).abs() <= 3 && b.2 == a.2);
        }
    }
}

#[test]
fn chopping_a_bar_into_quarters_gives_four_clips() {
    let r = registry();
    let mut d = synth_doc(
        &r,
        notes_clip(
            BAR,
            BAR,
            vec![
                note(0, PPQ, 60, 0.8),
                note(PPQ, PPQ, 62, 0.8),
                note(2 * PPQ, PPQ, 64, 0.8),
                note(3 * PPQ, PPQ, 65, 0.8),
            ],
        ),
    );
    let id = the_clip(&d, 0).id;
    let t = |d: &Document| flatten_notes(d.doc().song.track(&key("synth")).unwrap());
    let before = t(&d);
    one_step(&mut d, &r, chop(vec![id], PPQ));
    let clips = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    assert_eq!(
        clips
            .iter()
            .map(|c| (c.start, c.length))
            .collect::<Vec<_>>(),
        [
            (BAR, PPQ),
            (BAR + PPQ, PPQ),
            (BAR + 2 * PPQ, PPQ),
            (BAR + 3 * PPQ, PPQ)
        ]
    );
    assert_eq!(t(&d), before);
    // Nothing to chop at or beyond the clip length.
    d.undo(&r).unwrap();
    let e = rejected(&mut d, &r, chop(vec![id], BAR));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(&mut d, &r, chop(vec![id], 0));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn scaling_moves_clips_relative_to_the_earliest_start_and_scales_the_loop() {
    let r = registry();
    let mut d = synth_doc(&r, notes_clip(BAR, 2 * BAR, vec![note(PPQ, PPQ, 60, 0.8)]));
    let mut looped = notes_clip(5 * BAR, 2 * BAR, vec![note(0, PPQ, 62, 0.8)]);
    looped.loop_length = Some(BAR);
    run(&mut d, &r, [add_clip("synth", 0, looped)]);
    let ids: Vec<ClipId> = d.doc().song.track(&key("synth")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    let was = text(&d);
    one_step(&mut d, &r, scale(ids.clone(), 200.0));
    let a = the_clip(&d, 0);
    let b = the_clip(&d, 1);
    // Twice as fast: half as long, the second clip 4 bars after the first becomes 2.
    assert_eq!((a.start, a.length), (BAR, BAR));
    assert_eq!(
        (b.start, b.length, b.loop_length),
        (3 * BAR, BAR, Some(BAR / 2))
    );
    assert_eq!(notes_of(&a), [(PPQ / 2, PPQ / 2, 60)]);
    one_step(&mut d, &r, scale(ids.clone(), 50.0));
    assert_eq!(text(&d), was);
    // Out of range and no-op percentages are refused.
    for p in [100.0, 5.0, 5000.0] {
        let e = rejected(&mut d, &r, scale(ids.clone(), p));
        assert!(
            matches!(e, ModelError::BadSong(_) | ModelError::OutOfRange(_)),
            "{e:?}"
        );
    }
}

#[test]
fn scaling_audio_warps_it_and_a_reversed_clip_is_refused() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(&mut d, &r, [add_audio_clip("audio-track", BAR, 2 * BAR, 1)]);
    let id = d.doc().song.track(&key("audio-track")).unwrap().lanes[0].clips[0].id;
    one_step(&mut d, &r, scale(vec![id], 200.0));
    let c = doc_clip(d.doc(), id);
    assert_eq!((c.start, c.length), (BAR, BAR));
    let ClipContent::Audio(a) = &c.content else {
        panic!("audio")
    };
    // The asset plays at the song's 120 BPM before; twice as fast needs a source at 60.
    assert!(a.warp.enabled);
    assert!(
        (a.warp.source_bpm - 60.0).abs() < 1e-9,
        "{}",
        a.warp.source_bpm
    );
    d.undo(&r).unwrap();
    one_step(&mut d, &r, reverse(vec![id], false));
    let e = rejected(&mut d, &r, scale(vec![id], 200.0));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn chopping_audio_keeps_the_place_in_the_asset() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(&mut d, &r, [add_audio_clip("audio-track", 0, 2 * BAR, 1)]);
    let id = d.doc().song.track(&key("audio-track")).unwrap().lanes[0].clips[0].id;
    one_step(&mut d, &r, chop(vec![id], BAR / 2));
    let clips = &d.doc().song.track(&key("audio-track")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 4);
    let offsets: Vec<u64> = clips
        .iter()
        .map(|c| match &c.content {
            ClipContent::Audio(a) => a.source_offset,
            _ => unreachable!(),
        })
        .collect();
    assert!(offsets.windows(2).all(|w| w[0] < w[1]), "{offsets:?}");
}
