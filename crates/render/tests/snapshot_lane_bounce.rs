//! A bounce recalls the mixer snapshots at the points of the snapshot lane, as playback does: the
//! mix changes at the point (params the engine smooths, an effect added to or removed from a
//! chain, the routing), what plays after it is what a project saved in that state plays, a
//! project without the lane bounces as it always did, and the output does not depend on the host
//! block size or the worker count.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    Document, InsertEffect, Note, RackDoc, SNAPSHOT_LANE_KEY, SnapshotScope, SongCommand,
    empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn set(k: &str, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    }
}

fn console(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn clip(content: ClipContent, length: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content,
    }
}

/// Beats the song lasts.
const BEATS: i64 = 8;

/// One synth track `lead` (its channel is `channel`) holding a note for the whole song.
fn song() -> Document {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let length = BEATS * PPQ;
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("lead")),
        },
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: clip(
                ClipContent::Notes(vec![Note::new(0, length, 60, 0.8)].into()),
                length,
            ),
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    d
}

/// Adds the snapshot lane of the master with `points` (tick, snapshot id) in one clip.
fn with_lane(d: &mut Document, points: &[(i64, u32)]) {
    let master = d.doc().console.master.clone().expect("a master");
    apply(
        d,
        Command::Song(SongCommand::AddAutomationLane {
            device: master.clone(),
            param: SNAPSHOT_LANE_KEY.into(),
        }),
    );
    let lane = d.doc().snapshot_lane().expect("the lane").1;
    let points = points
        .iter()
        .map(|&(tick, id)| AutomationPoint {
            tick,
            value: id as f32,
            curve: Curve::Step,
        })
        .collect();
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: master,
            lane,
            clip: clip(ClipContent::Automation(points), BEATS * PPQ),
        }),
    );
}

fn store(d: &mut Document, name: &str) {
    apply(
        d,
        console(ConsoleCommand::StoreSnapshot { name: name.into() }),
    );
}

fn recall(d: &mut Document, id: u32) {
    apply(
        d,
        console(ConsoleCommand::RecallSnapshot {
            id,
            scope: SnapshotScope::ALL,
        }),
    );
}

fn bounce_with(doc: &RackDoc, block: usize, workers: u16, loops: Option<u32>) -> Vec<Vec<f32>> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: loops,
    };
    common::render_guarded(doc, &default_registry(), &options)
        .unwrap()
        .channels
}

fn bounce(doc: &RackDoc) -> Vec<f32> {
    bounce_with(doc, 512, 0, None).remove(0)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn peak(a: &[f32]) -> f32 {
    a.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// The frame of beat `beat` at the project's tempo and rate.
fn frame_of(doc: &RackDoc, beat: f64) -> usize {
    (beat * 60.0 / doc.tempo_bpm * f64::from(doc.sample_rate)) as usize
}

/// The largest difference between `a` and `b` over frames `from..to`.
fn max_diff(a: &[f32], b: &[f32], from: usize, to: usize) -> f32 {
    a[from..to]
        .iter()
        .zip(&b[from..to])
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// Snapshot 1 is the mix as saved (the fader up), snapshot 2 the same with the fader down; the
/// document is left in the state of snapshot 1.
fn fader_song() -> Document {
    let mut d = song();
    store(&mut d, "Up");
    apply(&mut d, set("channel", "fader", 0.4));
    store(&mut d, "Down");
    recall(&mut d, 1);
    d
}

#[test]
fn a_recall_at_a_lane_point_changes_the_mix_there_and_nowhere_before() {
    let mut d = fader_song();
    let up = bounce(d.doc());
    let mut down_doc = fader_song();
    recall(&mut down_doc, 2);
    let down = bounce(down_doc.doc());
    assert!(peak(&up) > 0.05, "the song is not silent");
    assert!(
        max_diff(&up, &down, 0, up.len()) > 0.02,
        "the faders differ"
    );

    with_lane(&mut d, &[(0, 1), (4 * PPQ, 2)]);
    let played = bounce(d.doc());
    assert_eq!(played.len(), up.len());
    let at = frame_of(d.doc(), 4.0);
    // Before the point, the mix as saved, bit for bit.
    assert_eq!(bits(&played[..at]), bits(&up[..at]), "before the point");
    // After the engine's 5 ms smoothing has settled, the mix of the other snapshot.
    let settled = at + frame_of(d.doc(), 1.0) / 2;
    assert!(
        max_diff(&played, &down, settled, down.len()) < 1e-3 * peak(&up),
        "after the point"
    );
    assert!(
        max_diff(&played, &up, settled, up.len()) > 0.02,
        "and not the old one"
    );
}

#[test]
fn the_points_of_the_lane_recall_in_order_and_a_repeated_snapshot_recalls_once() {
    let mut d = fader_song();
    with_lane(&mut d, &[(0, 1), (2 * PPQ, 2), (4 * PPQ, 2), (6 * PPQ, 1)]);
    let played = bounce(d.doc());
    let up = bounce(fader_song().doc());
    let mut down_doc = fader_song();
    recall(&mut down_doc, 2);
    let down = bounce(down_doc.doc());
    let beat = |b: f64| frame_of(d.doc(), b);
    let settle = beat(0.5);
    assert!(max_diff(&played, &up, 0, beat(2.0)) < 1e-6);
    assert!(max_diff(&played, &down, beat(2.0) + settle, beat(6.0)) < 1e-3 * peak(&up));
    assert!(max_diff(&played, &up, beat(6.0) + settle, up.len()) < 1e-3 * peak(&up));
}

#[test]
fn a_recall_adds_an_effect_to_a_chain_and_the_bounce_plays_it() {
    // Snapshot 1 has no insert, snapshot 2 a gain tool at -12 dB on the channel.
    let mut d = song();
    store(&mut d, "Dry");
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.gain-tool".into(),
            key: Some(key("trim")),
        },
        console(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("trim"))),
        }),
        set("trim", "gain", -12.0),
    ] {
        apply(&mut d, c);
    }
    store(&mut d, "Wet");
    recall(&mut d, 1);
    assert!(
        d.doc().device(&key("trim")).is_none(),
        "the recall removed it"
    );
    let dry = bounce(d.doc());
    let mut wet_doc = Document::new(d.doc().clone());
    recall(&mut wet_doc, 2);
    assert!(
        wet_doc.doc().device(&key("trim")).is_some(),
        "and brings it back"
    );
    let wet = bounce(wet_doc.doc());
    assert!(
        max_diff(&dry, &wet, 0, dry.len()) > 0.02,
        "the effect is heard"
    );

    with_lane(&mut d, &[(0, 1), (4 * PPQ, 2)]);
    let played = bounce(d.doc());
    let at = frame_of(d.doc(), 4.0);
    assert_eq!(
        bits(&played[..at]),
        bits(&dry[..at]),
        "dry before the point"
    );
    let settled = at + frame_of(d.doc(), 1.0) / 2;
    assert!(
        max_diff(&played, &wet, settled, wet.len()) < 2e-3 * peak(&dry),
        "the effect after it"
    );
}

#[test]
fn a_recall_removes_an_effect_from_a_chain_and_the_bounce_stops_playing_it() {
    let mut d = song();
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.gain-tool".into(),
            key: Some(key("trim")),
        },
        console(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("trim"))),
        }),
        set("trim", "gain", -12.0),
    ] {
        apply(&mut d, c);
    }
    store(&mut d, "Wet");
    apply(
        &mut d,
        console(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: None,
        }),
    );
    apply(&mut d, Command::RemoveDevice { key: key("trim") });
    store(&mut d, "Dry");
    recall(&mut d, 1);
    let wet = bounce(d.doc());
    let mut dry_doc = Document::new(d.doc().clone());
    recall(&mut dry_doc, 2);
    let dry = bounce(dry_doc.doc());
    assert!(max_diff(&dry, &wet, 0, dry.len()) > 0.02);

    with_lane(&mut d, &[(0, 1), (4 * PPQ, 2)]);
    let played = bounce(d.doc());
    let at = frame_of(d.doc(), 4.0);
    assert_eq!(bits(&played[..at]), bits(&wet[..at]));
    let settled = at + frame_of(d.doc(), 1.0) / 2;
    assert!(max_diff(&played, &dry, settled, dry.len()) < 2e-3 * peak(&dry));
}

#[test]
fn a_project_without_the_lane_bounces_as_it_did_with_or_without_snapshots() {
    let with = fader_song();
    let without = song();
    assert_eq!(bits(&bounce(with.doc())), bits(&bounce(without.doc())));
}

#[test]
fn a_bounce_with_recalls_does_not_depend_on_the_block_size_or_the_workers() {
    let mut d = fader_song();
    with_lane(&mut d, &[(0, 1), (3 * PPQ, 2), (5 * PPQ, 1)]);
    let a = bounce_with(d.doc(), 512, 0, None);
    let b = bounce_with(d.doc(), 64, 2, None);
    let c = bounce_with(d.doc(), 1000, 1, None);
    assert_eq!(a.len(), 2);
    for ch in 0..2 {
        assert_eq!(
            bits(&a[ch]),
            bits(&b[ch]),
            "channel {ch}, block 64 and workers"
        );
        assert_eq!(bits(&a[ch]), bits(&c[ch]), "channel {ch}, block 1000");
    }
}

#[test]
fn a_point_past_the_end_of_the_song_is_never_reached() {
    let mut d = fader_song();
    with_lane(&mut d, &[(0, 1), (60 * PPQ, 2)]);
    let played = bounce(d.doc());
    let up = bounce(fader_song().doc());
    assert_eq!(bits(&played), bits(&up));
}

#[test]
fn a_looped_bounce_recalls_the_points_inside_the_loop_on_every_lap() {
    // A two-beat loop at the start of the song: the lane recalls Up at 0 and Down at beat 1, and
    // every lap does the same.
    let looped = |d: &mut Document| {
        apply(
            d,
            Command::Song(SongCommand::SetLoop {
                range: (0, 2 * PPQ),
                enabled: true,
            }),
        );
    };
    let mut d = fader_song();
    looped(&mut d);
    with_lane(&mut d, &[(0, 1), (PPQ, 2)]);
    let played = bounce_with(d.doc(), 512, 0, Some(3)).remove(0);
    let mut up_doc = fader_song();
    looped(&mut up_doc);
    let up = bounce_with(up_doc.doc(), 512, 0, Some(3)).remove(0);
    let mut down_doc = fader_song();
    looped(&mut down_doc);
    recall(&mut down_doc, 2);
    let down = bounce_with(down_doc.doc(), 512, 0, Some(3)).remove(0);
    let beat = |b: f64| frame_of(d.doc(), b);
    let settle = beat(0.5);
    let limit = 1e-3 * peak(&up);
    // The first lap, then the second and the third from their first point on: Up on the first
    // beat of a lap, Down on the second.
    for lap in 0..3 {
        let start = f64::from(lap) * 2.0;
        assert!(
            max_diff(
                &played,
                &down,
                beat(start + 1.0) + settle,
                beat(start + 2.0)
            ) < limit,
            "lap {lap}: the second beat plays the other snapshot"
        );
    }
    for lap in 1..3 {
        let start = f64::from(lap) * 2.0;
        assert!(
            max_diff(&played, &up, beat(start) + settle, beat(start + 1.0)) < limit,
            "lap {lap}: the first beat is the first snapshot again"
        );
    }
}
