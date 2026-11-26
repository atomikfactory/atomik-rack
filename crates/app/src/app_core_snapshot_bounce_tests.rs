//! The snapshot lane in playback and in a bounce: the live path (the core recalls when the playhead
//! crosses a point, one batch at a time) and the bounce (`atomik_render`, which schedules the
//! same recalls before it starts) play a project with a snapshot lane to the same samples, also
//! when a recall adds and removes an effect, and a recall that adds or removes effects is one
//! undo step in the core.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::stepped;
use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    InsertEffect, Note, SongCommand, empty_doc,
};

/// The engine's fixed output latency (one batch) that the live render carries over the bounce.
const SKIP: usize = atomik_core::BATCH;
const BEATS: i64 = 8;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply<S: EngineSink>(c: &mut AppCore<S>, command: Command) {
    c.apply_command(command.clone())
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

fn set<S: EngineSink>(c: &mut AppCore<S>, k: &str, p: &str, v: f32) {
    apply(
        c,
        Command::SetParam {
            key: key(k),
            param: p.into(),
            value: v,
            merge: None,
        },
    );
}

fn clip(content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: BEATS * PPQ,
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

fn store<S: EngineSink>(c: &mut AppCore<S>, name: &str) {
    apply(
        c,
        Command::Console(ConsoleCommand::StoreSnapshot { name: name.into() }),
    );
}

/// A synth `lead` with a note for the whole song and a lane of the master that recalls snapshot
/// 1 at the start and snapshot 2 at beat 4; the document is in the state of snapshot 1.
///
/// The snapshots: 1 is the mix as it is, 2 is the fader down; with `effect`, snapshot 1 has no
/// insert and snapshot 2 a gain tool of -12 dB on the channel.
fn project(effect: bool) -> AppCore<OfflineSink> {
    let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    apply(
        &mut c,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("lead")),
        },
    );
    apply(
        &mut c,
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
    );
    apply(
        &mut c,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: clip(ClipContent::Notes(
                vec![Note::new(0, BEATS * PPQ, 60, 0.8)].into(),
            )),
        }),
    );
    apply(&mut c, Command::Song(SongCommand::SetSongEnd(BEATS * PPQ)));
    store(&mut c, "One");
    if effect {
        apply(
            &mut c,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.gain-tool".into(),
                key: Some(key("trim")),
            },
        );
        apply(
            &mut c,
            Command::Console(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot: 0,
                effect: Some(InsertEffect::Existing(key("trim"))),
            }),
        );
        set(&mut c, "trim", "gain", -12.0);
    } else {
        set(&mut c, "channel", "fader", 0.4);
    }
    store(&mut c, "Two");
    apply(
        &mut c,
        Command::Console(ConsoleCommand::RecallSnapshot {
            id: 1,
            scope: atomik_model::SnapshotScope::ALL,
        }),
    );
    let master = c.doc().console.master.clone().expect("a master");
    apply(
        &mut c,
        Command::Song(SongCommand::AddAutomationLane {
            device: master.clone(),
            param: atomik_model::SNAPSHOT_LANE_KEY.into(),
        }),
    );
    let lane = c.doc().snapshot_lane().expect("the lane").1;
    let points = [(0, 1.0), (4 * PPQ, 2.0)]
        .into_iter()
        .map(|(tick, value)| AutomationPoint {
            tick,
            value,
            curve: Curve::Step,
        })
        .collect();
    apply(
        &mut c,
        Command::Song(SongCommand::AddClip {
            device: master,
            lane,
            clip: clip(ClipContent::Automation(points)),
        }),
    );
    c
}

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &atomik_render::default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
    )
    .unwrap()
    .channels
}

fn first_difference(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> Option<usize> {
    live.iter().zip(bounce).find_map(|(a, b)| {
        a[SKIP..]
            .iter()
            .zip(b)
            .position(|(x, y)| x.to_bits() != y.to_bits())
    })
}

/// Plays the project live from its start, one batch per frame, for the length of `reference`.
fn play(c: &mut AppCore<OfflineSink>, frames: usize) -> Vec<Vec<f32>> {
    c.transport_action(atomik_ui::TransportAction::Play);
    stepped(c, frames + SKIP)
}

#[test]
fn the_live_path_and_the_bounce_play_the_snapshot_lane_to_the_same_samples() {
    for effect in [false, true] {
        let mut c = project(effect);
        let start = c.doc().clone();
        let reference = bounce_of(&start);
        let silent_reference = {
            let mut plain = start.clone();
            plain
                .song_mut()
                .tracks
                .retain(|t| Some(&t.device) != start.console.master.as_ref());
            bounce_of(&plain)
        };
        assert_ne!(
            reference[0], silent_reference[0],
            "effect {effect}: the lane changes what is heard"
        );
        let live = play(&mut c, reference[0].len());
        assert_eq!(live.len(), reference.len());
        assert_eq!(
            first_difference(&live, &reference),
            None,
            "effect {effect}: the live render and the bounce differ"
        );
    }
}

#[test]
fn a_recall_that_adds_and_removes_an_effect_is_one_undo_step_in_the_core() {
    let mut c = project(true);
    let before = c.doc().clone();
    assert!(c.recall_snapshot(Some(2)));
    assert!(c.doc().device(&key("trim")).is_some(), "added");
    assert!(c.undo(), "one undo");
    assert_eq!(c.doc(), &before);
    assert!(c.doc().device(&key("trim")).is_none());
    assert!(c.redo());
    assert!(c.doc().device(&key("trim")).is_some());
}
