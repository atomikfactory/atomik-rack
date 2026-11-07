//! The repetition finder's suggestions, planned against a real document: each simple edit is one
//! undo step and changes what the track plays by exactly what it says; the rest is advice.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::repetition::{Idea, VariationKind, find, variations};
use atomik_model::song::flatten_track_notes;
use atomik_model::{Clip, ClipContent, Document, Note};
use common::song::*;
use common::*;

const LAST_BAR: i64 = 3 * BAR;

fn pattern() -> Vec<Note> {
    vec![
        note(0, PPQ / 2, 36, 0.9),
        note(PPQ, PPQ / 2, 38, 0.8),
        note(2 * PPQ, PPQ / 2, 36, 0.9),
        note(3 * PPQ, PPQ / 2, 38, 0.7),
    ]
}

/// A synth `a` with a track and a channel, and the pattern in four clips of one bar each.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    d.apply(add(0, SYNTH, Some("a")), &r).unwrap();
    d.apply(add_track("a"), &r).unwrap();
    for bar in 0..4 {
        d.apply(add_clip("a", 0, notes_clip(bar * BAR, BAR, pattern())), &r)
            .unwrap();
    }
    (d, r)
}

fn played(d: &Document) -> Vec<(i64, i64, u8, f32)> {
    let doc = d.doc();
    flatten_track_notes(&doc.song, doc.song.track(&key("a")).unwrap())
}

fn idea(d: &Document) -> Idea {
    let mut found = find(&d.doc().song);
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

fn variation(
    d: &Document,
    r: &Registry,
    kind: VariationKind,
) -> atomik_model::repetition::Variation {
    let i = idea(d);
    variations(d.doc(), r, &i)
        .into_iter()
        .find(|v| v.kind == kind)
        .unwrap()
}

#[test]
fn every_idea_gets_the_three_suggestions() {
    let (d, r) = project();
    let kinds: Vec<_> = variations(d.doc(), &r, &idea(&d))
        .iter()
        .map(|v| v.kind)
        .collect();
    assert_eq!(kinds, VariationKind::ALL);
    assert_eq!(VariationKind::DropHit.label(), "Drop a hit");
}

#[test]
fn dropping_a_hit_mutes_the_last_hit_of_the_last_bar_in_one_step() {
    let (mut d, r) = project();
    let before = played(&d);
    assert_eq!(before.len(), 16);
    let v = variation(&d, &r, VariationKind::DropHit);
    let command = v.command.expect("a plain clip can be edited");
    let depth = d.undo_depth();
    d.apply(command, &r).unwrap();
    assert_eq!(d.undo_depth(), depth + 1, "one undo step");
    let after = played(&d);
    assert_eq!(after.len(), 15);
    assert!(
        !after.iter().any(|n| n.0 == LAST_BAR + 3 * PPQ),
        "the last hit of the last bar is gone"
    );
    // Everything else plays as before.
    let kept: Vec<_> = before
        .iter()
        .filter(|n| n.0 != LAST_BAR + 3 * PPQ)
        .copied()
        .collect();
    assert_eq!(after, kept);
    // The idea is gone too: the run is now three repeats plus a changed bar.
    let found = find(&d.doc().song);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].repeats, 3);
    d.undo().unwrap();
    assert_eq!(played(&d), before);
}

#[test]
fn cutting_a_beat_mutes_the_notes_of_the_last_beat() {
    let (mut d, r) = project();
    let v = variation(&d, &r, VariationKind::CutBeat);
    d.apply(v.command.unwrap(), &r).unwrap();
    let after = played(&d);
    assert_eq!(after.len(), 15);
    assert!(
        after
            .iter()
            .all(|n| n.0 < LAST_BAR + 3 * PPQ || n.0 >= 4 * BAR)
    );
}

#[test]
fn a_looped_clip_is_advice_only_because_an_edit_would_change_every_repeat() {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    d.apply(add(0, SYNTH, Some("a")), &r).unwrap();
    d.apply(add_track("a"), &r).unwrap();
    let mut looped = notes_clip(0, 4 * BAR, pattern());
    looped.loop_length = Some(BAR);
    d.apply(add_clip("a", 0, looped), &r).unwrap();
    let drop = variation(&d, &r, VariationKind::DropHit);
    assert!(drop.command.is_none());
    assert!(!drop.text.is_empty());
    assert!(variation(&d, &r, VariationKind::CutBeat).command.is_none());
}

#[test]
fn a_bar_with_one_hit_has_nothing_to_drop() {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    d.apply(add(0, SYNTH, Some("a")), &r).unwrap();
    d.apply(add_track("a"), &r).unwrap();
    for bar in 0..3 {
        d.apply(
            add_clip(
                "a",
                0,
                notes_clip(bar * BAR, BAR, vec![note(0, PPQ, 36, 0.9)]),
            ),
            &r,
        )
        .unwrap();
    }
    let v = variation(&d, &r, VariationKind::DropHit);
    assert!(v.command.is_none());
}

#[test]
fn the_filter_variation_adds_a_sweep_on_the_channel_in_one_step() {
    let (mut d, r) = project();
    let v = variation(&d, &r, VariationKind::FilterLastBar);
    let command = v.command.expect("a track with a channel can be filtered");
    let depth = d.undo_depth();
    d.apply(command, &r).unwrap();
    assert_eq!(d.undo_depth(), depth + 1, "one undo step");
    let song = &d.doc().song;
    let channel = key("channel");
    let track = song
        .track(&channel)
        .expect("an automation track for the strip");
    let on = &track.lanes[track.automation_lane("lpf.on").unwrap()];
    let freq = &track.lanes[track.automation_lane("lpf.freq").unwrap()];
    assert_eq!(on.static_value, Some(0.0), "off outside the bar");
    assert_eq!(freq.clips.len(), 1);
    let clip: &Clip = &freq.clips[0];
    assert_eq!((clip.start, clip.length), (LAST_BAR, BAR));
    let ClipContent::Automation(points) = &clip.content else {
        panic!("not automation");
    };
    assert_eq!(points.len(), 2);
    assert!(points[0].value > points[1].value, "it sweeps down");
    assert_eq!(points[1].tick, BAR);
    d.undo().unwrap();
    assert!(d.doc().song.track(&channel).is_none());
}

#[test]
fn a_strip_that_is_already_filtered_or_automated_is_left_alone() {
    let (mut d, r) = project();
    d.apply(set_param("channel", "lpf.on", 1.0), &r).unwrap();
    let v = variation(&d, &r, VariationKind::FilterLastBar);
    assert!(v.command.is_none(), "{}", v.text);
}
