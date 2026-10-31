//! Insert and remove time: every structure that holds a song position moves together, in one
//! undo step; insert then remove gives the song back.

mod common;

use atomik_model::song::{flatten_audio, flatten_notes};
use atomik_model::{
    ClipContent, ClipId, Command, Document, Frozen, Marker, MeterEvent, ModelError, RackDoc,
    RangeMarker, SongCommand, TempoEvent, WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

fn insert(at: i64, length: i64) -> Command {
    wf(WorkflowCommand::InsertTime { at, length })
}

fn remove(at: i64, length: i64) -> Command {
    wf(WorkflowCommand::RemoveTime { at, length })
}

/// A synth track with note clips at bars 1..2, 3..5 and 6..7 (zero-based starts 1, 3 and 6).
fn song_doc(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(
                    BAR,
                    BAR,
                    vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.6)],
                ),
            ),
            add_clip(
                "synth",
                0,
                notes_clip(
                    3 * BAR,
                    2 * BAR,
                    vec![note(0, PPQ, 67, 0.5), note(BAR, PPQ, 69, 0.5)],
                ),
            ),
            add_clip(
                "synth",
                0,
                notes_clip(6 * BAR, BAR, vec![note(0, PPQ, 72, 0.5)]),
            ),
            sc(SongCommand::SetSongEnd(8 * BAR)),
        ],
    );
    d
}

fn ticks(d: &Document) -> Vec<(i64, u8)> {
    flatten_notes(d.doc().song.track(&key("synth")).unwrap())
        .iter()
        .map(|n| (n.0, n.2))
        .collect()
}

fn starts(d: &Document) -> Vec<(i64, i64)> {
    d.doc().song.track(&key("synth")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| (c.start, c.length))
        .collect()
}

#[test]
fn insert_moves_clips_splits_the_straddling_one_and_moves_the_end() {
    let r = registry();
    let mut d = song_doc(&r);
    let before = ticks(&d);
    // One bar at the start of bar 4 (inside the 3..5 clip).
    one_step(&mut d, &r, insert(4 * BAR, BAR));
    assert_eq!(
        starts(&d),
        [(BAR, BAR), (3 * BAR, BAR), (5 * BAR, BAR), (7 * BAR, BAR)]
    );
    assert_eq!(d.doc().song.end_tick(), 9 * BAR);
    // The notes after the point moved, the ones before did not.
    let after = ticks(&d);
    assert_eq!(after.len(), before.len());
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(a.1, b.1);
        assert_eq!(a.0, if b.0 >= 4 * BAR { b.0 + BAR } else { b.0 });
    }
    // One undo step gives it back.
    d.undo(&r).unwrap();
    assert_eq!(ticks(&d), before);
    assert_eq!(d.doc().song.end_tick(), 8 * BAR);
}

#[test]
fn insert_then_remove_gives_the_song_back() {
    let r = registry();
    let mut d = song_doc(&r);
    let start = d.doc().clone();
    // In a gap: exact.
    one_step(&mut d, &r, insert(2 * BAR, 2 * BAR));
    assert_ne!(*d.doc().song, *start.song);
    one_step(&mut d, &r, remove(2 * BAR, 2 * BAR));
    assert_eq!(*d.doc().song, *start.song);
    // Through a clip: the clip comes back as two halves that play the same.
    let before = ticks(&d);
    one_step(&mut d, &r, insert(4 * BAR, BAR));
    one_step(&mut d, &r, remove(4 * BAR, BAR));
    assert_eq!(ticks(&d), before);
    assert_eq!(d.doc().song.end_tick(), start.song.end_tick());
}

#[test]
fn remove_cuts_the_range_out_and_closes_the_gap() {
    let r = registry();
    let mut d = song_doc(&r);
    // Bars 3..4 (the first bar of the second clip) go.
    one_step(&mut d, &r, remove(3 * BAR, BAR));
    assert_eq!(
        starts(&d),
        [(BAR, BAR), (3 * BAR, BAR), (5 * BAR, BAR)],
        "{:?}",
        starts(&d)
    );
    assert_eq!(
        ticks(&d),
        [
            (BAR, 60),
            (BAR + PPQ, 64),
            // The note at the start of the removed bar went with it, the one at bar 4 moved.
            (3 * BAR, 69),
            (5 * BAR, 72)
        ]
    );
    assert_eq!(d.doc().song.end_tick(), 7 * BAR);
    // A range in the middle of a clip leaves two clips.
    let mut d = song_doc(&r);
    one_step(&mut d, &r, remove(3 * BAR + BAR / 2, BAR / 2));
    assert_eq!(
        d.doc().song.track(&key("synth")).unwrap().lanes[0]
            .clips
            .len(),
        4
    );
    // Nothing to do is refused.
    let mut d = Document::new(empty_doc());
    let e = rejected(&mut d, &r, insert(0, 0));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn markers_ranges_loop_tempo_and_meter_follow() {
    let r = registry();
    let mut d = song_doc(&r);
    let marker = |id: u32, tick: i64| Marker {
        id,
        tick,
        name: format!("m{id}"),
        color: None,
    };
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::SetMarkers(vec![
                marker(1, BAR),
                marker(2, 4 * BAR),
                marker(3, 6 * BAR),
            ])),
            sc(SongCommand::SetRanges(vec![RangeMarker {
                id: 1,
                start: 2 * BAR,
                end: 6 * BAR,
                name: "verse".into(),
                color: None,
            }])),
            sc(SongCommand::SetLoop {
                range: (2 * BAR, 6 * BAR),
                enabled: true,
            }),
            sc(SongCommand::SetTempoMap(vec![
                TempoEvent::step(0, 120.0),
                TempoEvent::step(5 * BAR, 90.0),
            ])),
            sc(SongCommand::SetMeterMap(vec![
                MeterEvent::new(1, 4, 4),
                MeterEvent::new(7, 3, 4),
            ])),
        ],
    );
    one_step(&mut d, &r, insert(4 * BAR, BAR));
    let s = &d.doc().song;
    assert_eq!(
        s.markers.iter().map(|m| m.tick).collect::<Vec<_>>(),
        [BAR, 5 * BAR, 7 * BAR]
    );
    assert_eq!((s.ranges[0].start, s.ranges[0].end), (2 * BAR, 7 * BAR));
    assert_eq!(s.loop_range, (2 * BAR, 7 * BAR));
    assert_eq!(s.tempo[1].tick, 6 * BAR);
    assert_eq!(s.meter[1].bar, 8);
    d.undo(&r).unwrap();
    assert_eq!(d.doc().song.meter[1].bar, 7);
    // Remove: markers inside go, the rest close up, the tempo in effect after carries on.
    one_step(&mut d, &r, remove(4 * BAR, 2 * BAR));
    let s = &d.doc().song;
    assert_eq!(
        s.markers.iter().map(|m| m.tick).collect::<Vec<_>>(),
        [BAR, 4 * BAR]
    );
    assert_eq!((s.ranges[0].start, s.ranges[0].end), (2 * BAR, 4 * BAR));
    assert_eq!(s.loop_range, (2 * BAR, 4 * BAR));
    assert_eq!(s.tempo.len(), 2);
    assert_eq!((s.tempo[1].tick, s.tempo[1].bpm), (4 * BAR, 90.0));
    assert_eq!(s.meter[1].bar, 5);
    // A range that is not on bar lines cannot move a meter change.
    let e = rejected(&mut d, &r, insert(BAR / 2, BAR));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn removing_across_tempo_changes_keeps_the_tempo_after_the_range() {
    let r = registry();
    let mut d = song_doc(&r);
    run(
        &mut d,
        &r,
        [sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 120.0),
            TempoEvent::step(2 * BAR, 100.0),
            TempoEvent::step(3 * BAR, 80.0),
        ]))],
    );
    // 1..3 bars out: 100 BPM is inside, 80 carries on at the cut.
    one_step(&mut d, &r, remove(BAR, 2 * BAR));
    let s = &d.doc().song;
    let got: Vec<(i64, f64)> = s.tempo.iter().map(|e| (e.tick, e.bpm)).collect();
    assert_eq!(got, [(0, 120.0), (BAR, 80.0)]);
}

#[test]
fn take_groups_move_whole_and_a_straddling_group_is_refused() {
    let r = registry();
    let mut d = song_doc(&r);
    run(
        &mut d,
        &r,
        [sc(SongCommand::NewTakeGroup {
            device: key("synth"),
            lane: 0,
            takes: vec![
                notes_clip(10 * BAR, 2 * BAR, vec![note(0, PPQ, 50, 0.5)]),
                notes_clip(10 * BAR, 2 * BAR, vec![note(PPQ, PPQ, 52, 0.5)]),
            ],
        })],
    );
    let before = ticks(&d);
    one_step(&mut d, &r, insert(9 * BAR, BAR));
    let g = &d.doc().song.track(&key("synth")).unwrap().lanes[0].take_groups[0];
    assert_eq!(g.range(), (11 * BAR, 13 * BAR));
    assert!(
        g.comp
            .iter()
            .all(|s| s.start >= 11 * BAR && s.end <= 13 * BAR)
    );
    one_step(&mut d, &r, remove(9 * BAR, BAR));
    assert_eq!(ticks(&d), before);
    // A group inside the removed range goes with its clips.
    one_step(&mut d, &r, remove(9 * BAR, 4 * BAR));
    assert!(
        d.doc().song.track(&key("synth")).unwrap().lanes[0]
            .take_groups
            .is_empty()
    );
    d.undo(&r).unwrap();
    // A group that straddles an edge is refused, and nothing changes.
    let text_before = text(&d);
    let e = rejected(&mut d, &r, insert(11 * BAR, BAR));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(&mut d, &r, remove(11 * BAR, BAR));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    assert_eq!(text(&d), text_before);
}

#[test]
fn automation_clips_are_cut_and_moved_like_note_clips() {
    let r = registry();
    let mut d = song_doc(&r);
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
                    4 * BAR,
                    vec![
                        point(0, 500.0, atomik_model::Curve::Linear),
                        point(2 * BAR, 900.0, atomik_model::Curve::Linear),
                    ],
                ),
            ),
        ],
    );
    one_step(&mut d, &r, insert(BAR, BAR));
    let lane = &d.doc().song.track(&key("synth")).unwrap().lanes[1];
    let got: Vec<(i64, i64)> = lane.clips.iter().map(|c| (c.start, c.length)).collect();
    assert_eq!(got, [(0, BAR), (2 * BAR, 3 * BAR)]);
    let ClipContent::Automation(p) = &lane.clips[1].content else {
        panic!("automation")
    };
    // The point at 2 bars of the content is still at 2 bars of the content.
    assert_eq!(p.iter().map(|x| x.tick).collect::<Vec<_>>(), [0, 2 * BAR]);
    assert_eq!(lane.clips[1].offset, BAR);
}

#[test]
fn audio_clips_are_split_and_moved_and_keep_their_place_in_the_asset() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(&mut d, &r, [add_audio_clip("audio-track", BAR, 4 * BAR, 1)]);
    let t = |d: &Document| d.doc().song.track(&key("audio-track")).unwrap().clone();
    one_step(&mut d, &r, insert(3 * BAR, BAR));
    let lane = &t(&d).lanes[0];
    assert_eq!(
        lane.clips
            .iter()
            .map(|c| (c.start, c.length))
            .collect::<Vec<_>>(),
        [(BAR, 2 * BAR), (4 * BAR, 2 * BAR)]
    );
    // The right half plays the second half of the asset: the audio is the same either side of
    // the gap, in frames of song time.
    let regions = flatten_audio(&t(&d), &d.doc().song, d.doc().sample_rate);
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions[1].source_offset,
        regions[0].source_offset + regions[0].frames
    );
    one_step(&mut d, &r, remove(3 * BAR, BAR));
    let lane = &t(&d).lanes[0];
    assert_eq!(
        lane.clips
            .iter()
            .map(|c| (c.start, c.length))
            .collect::<Vec<_>>(),
        [(BAR, 2 * BAR), (3 * BAR, 2 * BAR)]
    );
    let regions = flatten_audio(&t(&d), &d.doc().song, d.doc().sample_rate);
    assert_eq!(
        regions[1].source_offset,
        regions[0].source_offset + regions[0].frames
    );
}

#[test]
fn a_frozen_track_refuses_the_command() {
    let r = registry();
    let d = song_doc(&r);
    let mut doc: RackDoc = d.doc().clone();
    doc.song_mut().tracks[0].frozen = Some(Frozen {
        start: 0,
        length: BAR,
        audio: audio(1),
        effects: Vec::new(),
    });
    let mut d = Document::new(doc);
    let e = rejected(&mut d, &r, insert(0, BAR));
    assert!(matches!(e, ModelError::Frozen(_)), "{e:?}");
    let e = rejected(&mut d, &r, remove(0, BAR));
    assert!(matches!(e, ModelError::Frozen(_)), "{e:?}");
}

#[test]
fn the_end_grows_when_the_shift_puts_a_clip_past_it() {
    let r = registry();
    let mut d = song_doc(&r);
    // The End sits right at the last clip's end (7 bars).
    run(&mut d, &r, [sc(SongCommand::SetSongEnd(7 * BAR))]);
    one_step(&mut d, &r, insert(0, 2 * BAR));
    assert_eq!(d.doc().song.end_tick(), 9 * BAR);
    let last = d.doc().song.track(&key("synth")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.end())
        .max()
        .unwrap();
    assert!(d.doc().song.end_tick() >= last);
    let _: ClipId = d.last_clip_id().unwrap_or(ClipId(0));
}
