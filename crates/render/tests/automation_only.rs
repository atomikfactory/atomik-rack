//! Automation of a console channel and of an effect (their own automation tracks) in an offline
//! render: the lane changes the sound, plays exactly as a param set to the same value would, is
//! stopped by the track's mute and by nothing else, and the render does not depend on the block
//! size or the worker count.

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    Document, InsertEffect, Note, RackDoc, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const FADER: &str = "fader";
const MIX: &str = "mix";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
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
        controllers: Vec::new(),
        grid: None,
        content,
    }
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A synth `lead` holding a two-bar note, on its console channel, with a delay `fx` in the
/// channel's insert chain; the song is two bars long.
fn song() -> (Document, DeviceKey) {
    let mut d = Document::new(empty_doc());
    let length = 8 * PPQ;
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
                0,
                length,
                ClipContent::Notes(vec![
                    Note::new(0, PPQ, 60, 0.8),
                    Note::new(2 * PPQ, PPQ, 64, 0.8),
                    Note::new(4 * PPQ, PPQ, 67, 0.8),
                ]),
            ),
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.stereo-delay".into(),
            key: Some(key("fx")),
        },
    ] {
        apply(&mut d, c);
    }
    let channel = d.doc().console.channels[0].channel.clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("fx"))),
        }),
    );
    (d, channel)
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn ramp() -> Vec<AutomationPoint> {
    vec![
        point(0, 0.9, Curve::Linear),
        point(3 * PPQ, 0.1, Curve::Linear),
        point(5 * PPQ, 0.8, Curve::Step),
    ]
}

fn lane_on(d: &mut Document, device: &DeviceKey, param: &str, points: Vec<AutomationPoint>) {
    apply(
        d,
        Command::Song(SongCommand::AddAutomationLane {
            device: device.clone(),
            param: param.into(),
        }),
    );
    let lane = d
        .doc()
        .song
        .track(device)
        .unwrap()
        .automation_lane(param)
        .unwrap();
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane,
            clip: clip(0, 8 * PPQ, ClipContent::Automation(points)),
        }),
    );
}

fn set(d: &mut Document, device: &DeviceKey, param: &str, value: f32) {
    apply(
        d,
        Command::SetParam {
            key: device.clone(),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

fn bounce_with(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn bounce(doc: &RackDoc) -> Vec<f32> {
    bounce_with(doc, 64, 0)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// Asserts two renders are the same bit for bit, naming the first frame that differs.
fn same(a: &[f32], b: &[f32], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: length");
    if let Some(i) = (0..a.len()).find(|&i| a[i].to_bits() != b[i].to_bits()) {
        panic!(
            "{what}: first difference at frame {i}: {} against {}",
            a[i], b[i]
        );
    }
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn a_channel_fader_lane_changes_the_sound_and_does_not_depend_on_the_block_size() {
    let (plain, channel) = song();
    let mut m = plain.clone();
    lane_on(&mut m, &channel, FADER, ramp());
    let still = bounce(plain.doc());
    let moving = bounce(m.doc());
    assert!(peak(&still) > 0.0, "the song makes sound");
    assert_ne!(bits(&still), bits(&moving), "the lane changes the render");
    for (block, workers) in [(128, 2), (333, 3), (1024, 1)] {
        same(
            &bounce_with(m.doc(), block, workers),
            &moving,
            &format!("block {block} workers {workers}"),
        );
    }
}

#[test]
fn an_effect_lane_changes_the_sound_and_does_not_depend_on_the_block_size() {
    let (plain, _) = song();
    let mut m = plain.clone();
    lane_on(&mut m, &key("fx"), MIX, ramp());
    let still = bounce(plain.doc());
    let moving = bounce(m.doc());
    assert_ne!(bits(&still), bits(&moving), "the lane changes the render");
    for (block, workers) in [(128, 2), (333, 3), (1024, 1)] {
        same(
            &bounce_with(m.doc(), block, workers),
            &moving,
            &format!("block {block} workers {workers}"),
        );
    }
}

#[test]
fn a_flat_lane_renders_like_the_param_set_to_that_value() {
    let (plain, channel) = song();
    for (device, param, value) in [(channel.clone(), FADER, 0.4), (key("fx"), MIX, 0.8)] {
        let mut lane = plain.clone();
        lane_on(
            &mut lane,
            &device,
            param,
            vec![point(0, value, Curve::Step)],
        );
        let mut hand = plain.clone();
        set(&mut hand, &device, param, value);
        same(&bounce(lane.doc()), &bounce(hand.doc()), param);
    }
}

#[test]
fn a_muted_automation_track_plays_nothing_and_a_solo_elsewhere_changes_nothing() {
    let (plain, channel) = song();
    let mut m = plain.clone();
    lane_on(&mut m, &channel, FADER, ramp());
    let playing = bounce(m.doc());
    // Muted: the param keeps its own value, as without the lane.
    let mut muted = m.clone();
    apply(
        &mut muted,
        Command::Song(SongCommand::SetTrackFlags {
            device: channel.clone(),
            muted: true,
            solo: false,
            armed: false,
        }),
    );
    same(&bounce(muted.doc()), &bounce(plain.doc()), "muted");
    // The synth soloed: the fader still moves.
    let mut solo = m.clone();
    apply(
        &mut solo,
        Command::Song(SongCommand::SetTrackFlags {
            device: key("lead"),
            muted: false,
            solo: true,
            armed: false,
        }),
    );
    same(&bounce(solo.doc()), &playing, "a solo elsewhere");
}
