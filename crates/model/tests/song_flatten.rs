//! Song helpers for the UI, app and engine compile: lookup, bars.beats.ticks, flattening of
//! clips (offset, loop, mute, clip-end cuts overlaps) and mute/solo.

mod common;

use atomik_model::song::{
    audible_spans, bars_beats_ticks, effective_tracks, flatten_automation, flatten_notes,
    format_bbt, ticks_from_bars_beats_ticks,
};
use atomik_model::{Clip, ClipId, Curve, Lane, Song, Track, empty_song};
use common::key;
use common::song::*;
use proptest::prelude::*;

fn with_id(mut c: Clip, id: u64) -> Clip {
    c.id = ClipId(id);
    c
}

fn track(note_clips: Vec<Clip>, auto: Vec<Clip>) -> Track {
    Track {
        device: key("synth"),
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        frozen: None,
        lanes: vec![
            Lane::notes().with_clips(note_clips),
            Lane::automation("filter.cutoff").with_clips(auto),
        ],
    }
}

fn notes_track(clips: Vec<Clip>) -> Track {
    track(clips, vec![])
}

// ---- bars.beats.ticks -----------------------------------------------------------------------

#[test]
fn bbt_is_one_based_and_follows_the_meter() {
    assert_eq!(bars_beats_ticks(0, (4, 4)), (1, 1, 0));
    assert_eq!(bars_beats_ticks(BAR + PPQ + 5, (4, 4)), (2, 2, 5));
    // 6/8: a beat is an eighth, a bar three quarters.
    assert_eq!(bars_beats_ticks(3 * PPQ, (6, 8)), (2, 1, 0));
    assert_eq!(bars_beats_ticks(PPQ / 2 + 1, (6, 8)), (1, 2, 1));
    // 3/2: a beat is a half note.
    assert_eq!(bars_beats_ticks(6 * PPQ + 2 * PPQ, (3, 2)), (2, 2, 0));
    assert_eq!(
        bars_beats_ticks(-10, (4, 4)),
        (1, 1, 0),
        "negative ticks clamp"
    );
    assert_eq!(
        ticks_from_bars_beats_ticks((2, 2, 5), (4, 4)),
        BAR + PPQ + 5
    );
    assert_eq!(ticks_from_bars_beats_ticks((0, 0, 0), (4, 4)), 0);
    assert_eq!(format_bbt(BAR + PPQ + 5, (4, 4)), "2.2.5");
}

proptest! {
    #[test]
    fn bbt_round_trips(tick in 0i64..1_000_000_000, b in 1u8..=32, u in prop::sample::select(vec![2u8, 4, 8, 16])) {
        let bbt = bars_beats_ticks(tick, (b, u));
        prop_assert_eq!(ticks_from_bars_beats_ticks(bbt, (b, u)), tick);
        prop_assert!(bbt.1 >= 1 && bbt.1 <= u32::from(b));
    }
}

// ---- lookup ---------------------------------------------------------------------------------

#[test]
fn song_lookup_helpers() {
    let mut song = empty_song(120.0);
    song.tracks.push(track(
        vec![with_id(notes_clip(0, BAR, vec![]), 3)],
        vec![with_id(auto_clip(0, BAR, vec![]), 4)],
    ));
    assert_eq!(song.track(&key("synth")).unwrap().device, key("synth"));
    assert!(song.track(&key("x")).is_none());
    let (t, lane, c) = song.clip(ClipId(4)).unwrap();
    assert_eq!((t.device.as_str(), lane, c.id), ("synth", 1, ClipId(4)));
    assert!(song.clip(ClipId(5)).is_none());
    assert_eq!(song.meter(), (4, 4));
    assert_eq!(song.tracks[0].automation_lane("filter.cutoff"), Some(1));
    assert_eq!(song.tracks[0].automation_lane("osc.mix"), None);
}

// ---- notes ----------------------------------------------------------------------------------

#[test]
fn notes_move_with_clip_start_and_sort_by_tick_then_key() {
    let t = notes_track(vec![with_id(
        notes_clip(
            1000,
            BAR,
            vec![
                note(0, 50, 64, 0.5),
                note(0, 60, 60, 0.25),
                note(100, 10, 1, 1.0),
            ],
        ),
        1,
    )]);
    assert_eq!(
        flatten_notes(&t),
        [
            (1000, 60, 60, 0.25),
            (1000, 50, 64, 0.5),
            (1100, 10, 1, 1.0)
        ]
    );
}

#[test]
fn offset_skips_notes_before_the_content_start() {
    let mut c = notes_clip(
        1000,
        BAR,
        vec![
            note(0, 500, 60, 0.5),
            note(100, 10, 61, 0.5),
            note(150, 10, 62, 0.5),
        ],
    );
    c.offset = 100;
    let t = notes_track(vec![with_id(c, 1)]);
    // The note at content 0 starts before the clip: it does not play.
    assert_eq!(
        flatten_notes(&t),
        [(1000, 10, 61, 0.5), (1050, 10, 62, 0.5)]
    );
}

#[test]
fn notes_are_cut_at_the_clip_end() {
    let t = notes_track(vec![with_id(
        notes_clip(
            0,
            1000,
            vec![
                note(900, 500, 60, 0.5),
                note(999, 5, 61, 0.5),
                note(1000, 5, 62, 0.5),
            ],
        ),
        1,
    )]);
    assert_eq!(flatten_notes(&t), [(900, 100, 60, 0.5), (999, 1, 61, 0.5)]);
}

#[test]
fn loops_repeat_the_window_and_cut_at_each_repetition() {
    let mut c = notes_clip(
        2000,
        1000,
        vec![
            note(0, 100, 60, 0.5),
            note(300, 200, 61, 0.5), // crosses the loop end: cut to 100
            note(500, 10, 62, 0.5),  // outside the loop window: never plays
        ],
    );
    c.loop_length = Some(400);
    let t = notes_track(vec![with_id(c, 1)]);
    assert_eq!(
        flatten_notes(&t),
        [
            (2000, 100, 60, 0.5),
            (2300, 100, 61, 0.5),
            (2400, 100, 60, 0.5),
            (2700, 100, 61, 0.5),
            (2800, 100, 60, 0.5),
        ]
    );
}

#[test]
fn loop_with_offset_starts_inside_the_window() {
    let mut c = notes_clip(0, 800, vec![note(0, 10, 60, 0.5), note(300, 10, 61, 0.5)]);
    c.loop_length = Some(400);
    c.offset = 300;
    let t = notes_track(vec![with_id(c, 1)]);
    assert_eq!(
        flatten_notes(&t),
        [
            (0, 10, 61, 0.5),
            (100, 10, 60, 0.5),
            (400, 10, 61, 0.5),
            (500, 10, 60, 0.5)
        ]
    );
    // An offset beyond the window wraps.
    let mut c2 = notes_clip(0, 800, vec![note(0, 10, 60, 0.5), note(300, 10, 61, 0.5)]);
    c2.loop_length = Some(400);
    c2.offset = 700;
    assert_eq!(
        flatten_notes(&notes_track(vec![with_id(c2, 1)])),
        flatten_notes(&t)
    );
}

#[test]
fn muted_clips_neither_play_nor_hide_earlier_clips() {
    let a = with_id(notes_clip(0, 1000, vec![note(500, 10, 60, 0.5)]), 1);
    let mut b = with_id(notes_clip(400, 200, vec![note(0, 10, 70, 0.5)]), 2);
    b.muted = true;
    let t = notes_track(vec![a, b]);
    assert_eq!(flatten_notes(&t), [(500, 10, 60, 0.5)]);
}

#[test]
fn later_clips_win_where_they_overlap() {
    // A: [0, 1000), B: [400, 600). A is audible on [0, 400) and [600, 1000).
    let a = with_id(
        notes_clip(
            0,
            1000,
            vec![
                note(0, 900, 60, 0.5),   // cut where B starts
                note(500, 10, 61, 0.5),  // starts under B: silent
                note(700, 500, 62, 0.5), // cut at A's end
            ],
        ),
        1,
    );
    let b = with_id(notes_clip(400, 200, vec![note(0, 300, 70, 0.5)]), 2);
    let t = notes_track(vec![a.clone(), b.clone()]);
    assert_eq!(
        flatten_notes(&t),
        [(0, 400, 60, 0.5), (400, 200, 70, 0.5), (700, 300, 62, 0.5)]
    );
    assert_eq!(
        audible_spans(&t.lanes[0]),
        [
            (ClipId(1), 0, 400),
            (ClipId(2), 400, 600),
            (ClipId(1), 600, 1000)
        ]
    );
}

#[test]
fn same_start_overlap_is_won_by_the_higher_id() {
    let a = with_id(notes_clip(0, 100, vec![note(0, 10, 60, 0.5)]), 1);
    let b = with_id(notes_clip(0, 50, vec![note(0, 10, 70, 0.5)]), 2);
    let t = notes_track(vec![a, b]);
    // A is fully hidden on [0, 50) and only plays notes starting in [50, 100).
    assert_eq!(flatten_notes(&t), [(0, 10, 70, 0.5)]);
    assert_eq!(
        audible_spans(&t.lanes[0]),
        [(ClipId(2), 0, 50), (ClipId(1), 50, 100)]
    );
}

#[test]
fn an_earlier_long_clip_does_not_hide_a_later_one() {
    // B starts later, so B wins even though A started first and is longer.
    let a = with_id(notes_clip(0, 1000, vec![]), 5);
    let b = with_id(notes_clip(100, 100, vec![note(0, 10, 70, 0.5)]), 1);
    let t = notes_track(vec![a, b]);
    assert_eq!(flatten_notes(&t), [(100, 10, 70, 0.5)]);
    assert_eq!(
        audible_spans(&t.lanes[0]),
        [
            (ClipId(5), 0, 100),
            (ClipId(1), 100, 200),
            (ClipId(5), 200, 1000)
        ]
    );
}

// ---- automation -----------------------------------------------------------------------------

/// What the engine does with flattened points: hold before the first and after the last,
/// linear or step in between.
fn eval(points: &[(i64, f32, Curve)], tick: i64) -> f32 {
    let i = points.partition_point(|p| p.0 <= tick);
    if i == 0 {
        return points[0].1;
    }
    let (t0, v0, c0) = points[i - 1];
    match (points.get(i), c0) {
        (Some(&(t1, v1, _)), Curve::Linear) => {
            (f64::from(v0)
                + (f64::from(v1) - f64::from(v0)) * (tick - t0) as f64 / (t1 - t0) as f64)
                as f32
        }
        _ => v0,
    }
}

#[test]
fn automation_points_move_with_the_clip() {
    let t = track(
        vec![],
        vec![with_id(
            auto_clip(
                1000,
                1000,
                vec![
                    point(0, 100.0, Curve::Linear),
                    point(500, 600.0, Curve::Step),
                ],
            ),
            1,
        )],
    );
    assert_eq!(
        flatten_automation(&t, 1),
        [(1000, 100.0, Curve::Linear), (1500, 600.0, Curve::Step)]
    );
    assert!(
        flatten_automation(&t, 0).is_empty(),
        "the note lane has no automation"
    );
    assert!(flatten_automation(&t, 7).is_empty());
}

#[test]
fn automation_starts_with_the_value_at_the_content_start() {
    // Content starts between the points: the first emitted point carries the interpolated
    // value; before the first content point the first value holds.
    let mut c = auto_clip(
        0,
        1000,
        vec![
            point(100, 100.0, Curve::Linear),
            point(300, 300.0, Curve::Step),
        ],
    );
    c.offset = 200;
    let t = track(vec![], vec![with_id(c, 1)]);
    assert_eq!(
        flatten_automation(&t, 1),
        [(0, 200.0, Curve::Linear), (100, 300.0, Curve::Step)]
    );
    let t2 = track(
        vec![],
        vec![with_id(
            auto_clip(50, 1000, vec![point(100, 100.0, Curve::Step)]),
            1,
        )],
    );
    assert_eq!(
        flatten_automation(&t2, 1),
        [(50, 100.0, Curve::Step), (150, 100.0, Curve::Step)]
    );
}

#[test]
fn automation_ramp_is_cut_at_the_clip_end() {
    let t = track(
        vec![],
        vec![with_id(
            auto_clip(
                0,
                501,
                vec![
                    point(0, 0.0, Curve::Linear),
                    point(1000, 1000.0, Curve::Step),
                ],
            ),
            1,
        )],
    );
    // The ramp ends at the last tick inside the clip, then holds.
    assert_eq!(
        flatten_automation(&t, 1),
        [(0, 0.0, Curve::Linear), (500, 500.0, Curve::Step)]
    );
}

#[test]
fn automation_loops_and_overlaps_match_the_content_at_every_tick() {
    let content = vec![
        point(0, 100.0, Curve::Linear),
        point(300, 400.0, Curve::Step),
        point(350, 1000.0, Curve::Linear),
        point(600, 0.0, Curve::Linear),
    ];
    let mut a = auto_clip(0, 2000, content.clone());
    a.loop_length = Some(500);
    a.offset = 120;
    let b = auto_clip(900, 300, vec![point(0, 5000.0, Curve::Step)]);
    let t = track(vec![], vec![with_id(a.clone(), 1), with_id(b, 2)]);
    let flat = flatten_automation(&t, 1);
    let ticks: Vec<_> = flat.iter().map(|p| p.0).collect();
    let mut sorted = ticks.clone();
    sorted.dedup();
    assert_eq!(ticks, sorted, "strictly increasing ticks");
    // Analytic content value (all points count, the window is [0, 500)).
    let content_value = |c: i64| -> f64 {
        let i = content.partition_point(|p| p.tick <= c);
        if i == 0 {
            return 100.0;
        }
        let p = content[i - 1];
        match (content.get(i), p.curve) {
            (Some(n), Curve::Linear) => {
                f64::from(p.value)
                    + (f64::from(n.value) - f64::from(p.value)) * (c - p.tick) as f64
                        / (n.tick - p.tick) as f64
            }
            _ => f64::from(p.value),
        }
    };
    for tick in 0..2000 {
        let got = f64::from(eval(&flat, tick));
        let want = if (900..1200).contains(&tick) {
            5000.0
        } else {
            content_value((120 + tick).rem_euclid(500))
        };
        assert!((got - want).abs() < 1e-3, "tick {tick}: {got} vs {want}");
    }
}

// ---- mute and solo --------------------------------------------------------------------------

fn flags(k: &str, muted: bool, solo: bool) -> Track {
    Track {
        device: key(k),
        muted,
        solo,
        armed: false,
        chase: true,
        frozen: None,
        lanes: vec![Lane::notes()],
    }
}

fn names(song: &Song) -> Vec<&str> {
    effective_tracks(song)
        .iter()
        .map(|t| t.device.as_str())
        .collect()
}

#[test]
fn effective_tracks_apply_mute_and_solo() {
    let mut song = empty_song(120.0);
    song.tracks = vec![
        flags("a", false, false),
        flags("b", true, false),
        flags("c", false, false),
    ];
    assert_eq!(names(&song), ["a", "c"]);
    song.tracks[2].solo = true;
    assert_eq!(names(&song), ["c"]);
    song.tracks[1].solo = true; // soloed but muted
    assert_eq!(names(&song), ["c"]);
    song.tracks[0].solo = true;
    assert_eq!(names(&song), ["a", "c"]);
    song.tracks[2].muted = true;
    assert_eq!(names(&song), ["a"]);
}
