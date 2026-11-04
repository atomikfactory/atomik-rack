//! Device-parameter lanes inside note clips and bent automation segments, end to end: a cutoff lane
//! in a note clip changes the render, travels with the clip, loops with it, yields to a track
//! lane of the same param, and the bounce does not depend on the block size or the worker count;
//! the value a control shows is the value the engine applies; a bent segment follows the power
//! curve it names; a saved project plays the same.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, Ticks};
use atomik_engine::{DeviceInstanceId, SongCurve, SongPoint, lane_value};
use atomik_model::song::flatten_param_lanes;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Document, Note,
    ParamLane, RackDoc, SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, song_input};
use std::num::NonZeroUsize;

const CUTOFF: &str = "filter.cutoff";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn clip(start: i64, length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content,
    }
}

/// A cutoff sweep, bent, then held.
fn sweep() -> ParamLane {
    ParamLane::new(
        key("synth"),
        CUTOFF,
        vec![
            point(0, 200.0, Curve::Bend(40)),
            point(2 * PPQ, 6000.0, Curve::Linear),
            point(3 * PPQ, 900.0, Curve::Step),
        ],
    )
}

/// A synth with one three-beat note in a clip at `start`; the clip carries `lanes`.
fn song(start: i64, lanes: Vec<ParamLane>) -> Document {
    let mut d = Document::new(empty_doc());
    let length = 4 * PPQ;
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("synth")),
        },
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddTrack {
            device: key("synth"),
        }),
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
            lane: 0,
            clip: Clip {
                param_lanes: lanes,
                ..clip(
                    start,
                    length,
                    ClipContent::Notes(vec![Note::new(0, 3 * PPQ, 48, 0.9)].into()),
                )
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(6 * PPQ)));
    d
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    common::render_guarded(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

#[test]
fn a_lane_in_a_note_clip_changes_the_render_at_any_block_size() {
    let plain = song(0, Vec::new());
    let laned = song(0, vec![sweep()]);
    let still = bounce(plain.doc(), 64, 0);
    let moving = bounce(laned.doc(), 64, 0);
    assert!(still.iter().any(|s| s.abs() > 0.01), "the song makes sound");
    assert_ne!(bits(&still), bits(&moving), "the lane changes the render");
    for (block, workers) in [(128, 2), (333, 3), (1024, 1)] {
        assert_eq!(
            bits(&bounce(laned.doc(), block, workers)),
            bits(&moving),
            "block {block} workers {workers}"
        );
    }
}

#[test]
fn a_track_lane_of_the_same_param_wins_over_the_clip_lane() {
    let plain = song(0, Vec::new());
    let mut both = song(0, vec![sweep()]);
    apply(
        &mut both,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        }),
    );
    let lane = both
        .doc()
        .song
        .track(&key("synth"))
        .unwrap()
        .automation_lane(CUTOFF)
        .unwrap();
    apply(
        &mut both,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
            lane,
            clip: clip(
                0,
                6 * PPQ,
                ClipContent::Automation(vec![point(0, 1500.0, Curve::Step)]),
            ),
        }),
    );
    // The same track lane without the clip lane.
    let mut only_track = song(0, Vec::new());
    apply(
        &mut only_track,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        }),
    );
    apply(
        &mut only_track,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
            lane,
            clip: clip(
                0,
                6 * PPQ,
                ClipContent::Automation(vec![point(0, 1500.0, Curve::Step)]),
            ),
        }),
    );
    let a = bounce(both.doc(), 256, 0);
    assert_eq!(bits(&a), bits(&bounce(only_track.doc(), 256, 0)));
    assert_ne!(bits(&a), bits(&bounce(plain.doc(), 256, 0)));
}

#[test]
fn a_lane_moves_duplicates_and_loops_with_its_clip() {
    let mut d = song(0, vec![sweep()]);
    let id = d.doc().song.tracks[0].lanes[0].clips[0].id;
    // Moved one bar later: every point moves with it.
    apply(
        &mut d,
        Command::Song(SongCommand::SetClipGeometry {
            id,
            start: 4 * PPQ,
            length: 4 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }),
    );
    let flat = flatten_param_lanes(&d.doc().song.tracks[0]);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].0, key("synth"));
    assert_eq!(flat[0].2[0].0, 4 * PPQ);
    assert_eq!(flat[0].2[0].2, Curve::Bend(40));
    // Duplicated: the copy carries the lane.
    apply(
        &mut d,
        Command::Song(SongCommand::DuplicateClip { id, start: 8 * PPQ }),
    );
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 2);
    assert!(lane.clips.iter().all(|c| c.param_lanes.len() == 1));
    // Looped: the sweep restarts at each repetition.
    let mut looped = song(0, vec![sweep()]);
    let id = looped.doc().song.tracks[0].lanes[0].clips[0].id;
    apply(
        &mut looped,
        Command::Song(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 8 * PPQ,
            offset: 0,
            loop_length: Some(4 * PPQ),
            muted: false,
            merge: None,
        }),
    );
    let flat = &flatten_param_lanes(&looped.doc().song.tracks[0])[0].2;
    assert!(
        flat.iter()
            .any(|p| p.0 == 4 * PPQ && p.2 == Curve::Bend(40))
    );
}

#[test]
fn the_value_a_control_shows_is_the_value_the_engine_applies() {
    let registry = default_registry();
    let mut d = song(PPQ, vec![sweep()]);
    let id = d.doc().song.tracks[0].lanes[0].clips[0].id;
    // An offset and a loop cut the bent segment at both ends.
    apply(
        &mut d,
        Command::Song(SongCommand::SetClipGeometry {
            id,
            start: PPQ,
            length: 6 * PPQ,
            offset: PPQ / 3,
            loop_length: Some(PPQ + 700),
            muted: false,
            merge: None,
        }),
    );
    let doc = d.doc();
    let synth = key("synth");
    let input = song_input(doc, &registry, |k| {
        (k == &synth).then_some(DeviceInstanceId(1))
    });
    let lane = input
        .tracks
        .iter()
        .flat_map(|t| &t.automation)
        .next()
        .expect("the clip lane reaches the engine");
    assert!(!lane.points.is_empty());
    let shown = |tick: f64| doc.automation_values_at(tick)[&(synth.clone(), CUTOFF.to_string())];
    let mut tick = 0.0;
    while tick < (9 * PPQ) as f64 {
        let engine = lane_value(&lane.points, tick);
        assert_eq!(shown(tick).to_bits(), engine.to_bits(), "tick {tick}");
        tick += 41.5;
    }
}

#[test]
fn a_bent_segment_follows_its_power_curve() {
    let sp = |tick: i64, value: f32, curve| SongPoint {
        tick: Ticks(tick),
        value,
        curve,
    };
    for bend in [-64_i8, -40, -1, 0, 1, 17, 64] {
        let k = (f64::from(bend) / 32.0).exp2();
        let points = [
            sp(100, 0.25, SongCurve::Bend(bend)),
            sp(100 + 3840, 0.75, SongCurve::Step),
        ];
        for i in 0..=3840 {
            let tick = 100.0 + f64::from(i);
            let x = f64::from(i) / 3840.0;
            let want = 0.25 + 0.5 * x.powf(k);
            let got = f64::from(lane_value(&points, tick));
            assert!(
                (got - want).abs() < 1e-6,
                "bend {bend} at {i}: {got} vs {want}"
            );
        }
        // Held at the end.
        assert_eq!(lane_value(&points, 100.0 + 3840.0 + 5.0), 0.75);
    }
}

#[test]
fn a_bent_automation_clip_bounces_the_same_at_any_block_size() {
    let mut d = song(0, Vec::new());
    apply(
        &mut d,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        }),
    );
    let lane = d
        .doc()
        .song
        .track(&key("synth"))
        .unwrap()
        .automation_lane(CUTOFF)
        .unwrap();
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
            lane,
            clip: clip(
                0,
                6 * PPQ,
                ClipContent::Automation(vec![
                    point(0, 150.0, Curve::Bend(-50)),
                    point(3 * PPQ, 9000.0, Curve::Bend(60)),
                    point(5 * PPQ, 300.0, Curve::Step),
                ]),
            ),
        }),
    );
    let base = bounce(d.doc(), 64, 0);
    for (block, workers) in [(100, 0), (512, 2), (777, 3)] {
        assert_eq!(bits(&bounce(d.doc(), block, workers)), bits(&base));
    }
}

#[test]
fn a_saved_project_with_lanes_and_bends_plays_the_same() {
    let mut d = song(0, vec![sweep()]);
    let id = d.doc().song.tracks[0].lanes[0].clips[0].id;
    apply(
        &mut d,
        Command::Song(SongCommand::SetClipParamLanes {
            clip: id,
            lanes: vec![sweep()],
            merge: None,
        }),
    );
    let doc = d.doc().clone();
    let text = save_project(&doc);
    assert!(text.contains("\"param_lanes\""));
    assert!(text.contains("\"bend\", 40"));
    let back = load_project(&text, &default_registry()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), text);
    assert_eq!(bits(&bounce(&back, 512, 0)), bits(&bounce(&doc, 512, 0)));
}
