//! `WorkflowCommand::AddRumble`: the track of a printed rumble layer in one undo step, with a
//! Sidechain Tool keyed by the kick and an EQ; which notes are the kick; the release read from
//! the kick's rhythm; and rejections that change nothing.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::rumble::{
    DEFAULT_DECAY, DEFAULT_RELEASE_MS, DRUM_MACHINE_TYPE, RumbleTrackOptions, kick_hits, kick_key,
    suggested_decay, suggested_release,
};
use atomik_model::{
    Command, DeviceKey, Document, ModelError, RackDoc, WorkflowCommand, insert_chain,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

const DUCK: &str = "atomik.duck";
const FILTER: &str = "atomik.auto-filter";
const EQ: &str = "atomik.eq";

fn reg() -> Registry {
    audio_registry()
}

/// A synth `kick` with a track holding a note on every beat of `beats` beats, and assets 1
/// (10 s) and 2 (2 s) in the pool.
fn project(r: &Registry, beats: i64) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(register(1, 480_000), r).unwrap();
    d.apply(register(2, 96_000), r).unwrap();
    d.apply(add(0, SYNTH, Some("kick")), r).unwrap();
    d.apply(add_track("kick"), r).unwrap();
    let notes = (0..beats)
        .map(|b| note(b * PPQ, PPQ / 2, 36, 0.9))
        .collect();
    d.apply(add_clip("kick", 0, notes_clip(0, beats * PPQ, notes)), r)
        .unwrap();
    d
}

fn rumble(kick: &str, options: RumbleTrackOptions) -> Command {
    wf(WorkflowCommand::AddRumble {
        kick: key(kick),
        start: BAR,
        length: BAR,
        audio: audio(2),
        name: None,
        options,
    })
}

/// The value of `name` on `device`: the stored one, else the descriptor's default.
fn param(doc: &RackDoc, device: &DeviceKey, name: &str) -> f32 {
    let d = doc.device(device).unwrap();
    d.params.get(name).copied().unwrap_or_else(|| {
        reg()
            .get(&d.type_id)
            .unwrap()
            .descriptor
            .param_by_key(name)
            .unwrap()
            .default
    })
}

#[test]
fn the_rumble_track_is_one_undo_step_with_a_keyed_duck_and_an_eq() {
    let r = reg();
    let mut d = project(&r, 8);
    let tracks = d.doc().song.tracks.len();
    one_step(&mut d, &r, rumble("kick", RumbleTrackOptions::default()));
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), tracks + 1);
    // Right after the kick, an audio track named after it with the clip where it was asked.
    let new = &doc.song.tracks[1];
    assert!(new.is_audio());
    assert_eq!(doc.device(&new.device).unwrap().name, "Poly Synth Rumble");
    let clip = &new.lanes[0].clips[0];
    assert_eq!((clip.start, clip.length), (BAR, BAR));
    assert!(d.last_clip_id().is_some());

    // Its channel holds the duck, the low-pass, then the EQ.
    let channel = doc.console.channel_of(&new.device).unwrap().channel.clone();
    let chain = insert_chain(doc, &r, &channel);
    assert_eq!(chain.len(), 3);
    let (duck, filter, eq) = (&chain[0], &chain[1], &chain[2]);
    assert_eq!(doc.device(duck).unwrap().type_id, DUCK);
    assert_eq!(doc.device(filter).unwrap().type_id, FILTER);
    assert_eq!(doc.device(eq).unwrap().type_id, EQ);
    // Trigger mode, keyed by the kick's channel.
    assert_eq!(param(doc, duck, "mode"), 1.0);
    assert_eq!(param(doc, duck, "depth"), 0.9);
    let kick_channel = doc
        .console
        .channel_of(&key("kick"))
        .unwrap()
        .channel
        .0
        .clone();
    for side in ["l", "r"] {
        let cable = common::cable(
            &format!("{kick_channel}:out_{side}"),
            &format!("{}:sc_in_{side}", duck.0),
        );
        assert!(doc.cables.contains(&cable), "the key of side {side}");
    }
    // A fixed low-pass at 24 dB per octave, and the EQ's low cut under the sub band.
    assert_eq!(param(doc, filter, "mode"), 0.0);
    assert_eq!(param(doc, filter, "slope"), 1.0);
    assert_eq!(param(doc, filter, "cutoff"), 160.0);
    assert_eq!(param(doc, filter, "resonance"), 0.0);
    assert_eq!(param(doc, filter, "depth"), 0.0);
    assert_eq!(param(doc, eq, "band1.freq"), 28.0);
    assert_eq!(param(doc, eq, "band1.on"), 1.0);
}

#[test]
fn a_named_track_and_the_options_are_used() {
    let r = reg();
    let mut d = project(&r, 4);
    let options = RumbleTrackOptions {
        low_pass: 90.0,
        high_pass: 40.0,
        depth: 0.5,
        release_ms: 333.0,
    };
    d.apply(
        wf(WorkflowCommand::AddRumble {
            kick: key("kick"),
            start: 0,
            length: BAR,
            audio: audio(2),
            name: Some("Sub".into()),
            options,
        }),
        &r,
    )
    .unwrap();
    let doc = d.doc();
    let new = &doc.song.tracks[1];
    assert_eq!(doc.device(&new.device).unwrap().name, "Sub");
    let channel = doc.console.channel_of(&new.device).unwrap().channel.clone();
    let chain = insert_chain(doc, &r, &channel);
    assert_eq!(param(doc, &chain[0], "depth"), 0.5);
    assert_eq!(param(doc, &chain[0], "time"), 333.0);
    assert_eq!(param(doc, &chain[1], "cutoff"), 90.0);
    assert_eq!(param(doc, &chain[2], "band1.freq"), 40.0);
}

#[test]
fn what_cannot_be_made_is_rejected_and_changes_nothing() {
    let r = reg();
    let mut d = project(&r, 4);
    let defaults = RumbleTrackOptions::default();
    // A kick that is not a track, one inside a container, an asset that is not in the pool.
    rejected(&mut d, &r, rumble("nothing", defaults.clone()));
    rejected(&mut d, &r, rumble("box/kick", defaults.clone()));
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::AddRumble {
            kick: key("kick"),
            start: 0,
            length: BAR,
            audio: audio(9),
            name: None,
            options: defaults.clone(),
        }),
    );
    assert!(matches!(e, ModelError::MissingAsset(_)), "{e:?}");
    // Options outside their range.
    for options in [
        RumbleTrackOptions {
            low_pass: 10.0,
            ..defaults.clone()
        },
        RumbleTrackOptions {
            high_pass: 100.0,
            ..defaults.clone()
        },
        RumbleTrackOptions {
            depth: 0.0,
            ..defaults.clone()
        },
        RumbleTrackOptions {
            release_ms: f32::NAN,
            ..defaults.clone()
        },
    ] {
        let e = rejected(&mut d, &r, rumble("kick", options));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    }
    // A clip that is not plain audio.
    let mut reversed = audio(2);
    reversed.reverse = true;
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::AddRumble {
            kick: key("kick"),
            start: 0,
            length: BAR,
            audio: reversed,
            name: None,
            options: defaults,
        }),
    );
}

#[test]
fn the_kick_is_every_note_or_the_kick_pad_of_a_drum_machine() {
    let r = reg();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    d.apply(add(1, DRUM_MACHINE_TYPE, Some("drums")), &r)
        .unwrap();
    assert_eq!(kick_key(d.doc(), &key("synth")), None);
    assert_eq!(kick_key(d.doc(), &key("nothing")), None);
    // No pad plays a synthesised kick: the first pad is the kick.
    assert_eq!(kick_key(d.doc(), &key("drums")), Some(36));
    // The third pad does: it is the kick.
    d.apply(
        Command::SetParam {
            key: key("drums"),
            param: "pad3.module".into(),
            value: 1.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(kick_key(d.doc(), &key("drums")), Some(38));
}

#[test]
fn the_hits_are_the_distinct_start_ticks_of_the_kick_notes() {
    let r = reg();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(add(0, SYNTH, Some("kick")), &r).unwrap();
    d.apply(add_track("kick"), &r).unwrap();
    let notes = vec![
        note(0, 100, 36, 0.9),
        note(0, 100, 40, 0.9),
        note(PPQ, 100, 36, 0.9),
        note(2 * PPQ, 100, 38, 0.9),
    ];
    d.apply(add_clip("kick", 0, notes_clip(0, BAR, notes)), &r)
        .unwrap();
    let doc = d.doc();
    let track = doc.song.track(&key("kick")).unwrap();
    assert_eq!(kick_hits(&doc.song, track, None), vec![0, PPQ, 2 * PPQ]);
    assert_eq!(kick_hits(&doc.song, track, Some(36)), vec![0, PPQ]);
    assert_eq!(kick_hits(&doc.song, track, Some(60)), Vec::<i64>::new());
}

#[test]
fn the_release_is_a_third_of_the_gap_between_the_hits() {
    let r = reg();
    let d = project(&r, 8);
    let song = &d.doc().song;
    // A beat at 120 BPM is 500 ms: a third of it, 175 ms.
    let ticks: Vec<i64> = (0..8).map(|b| b * PPQ).collect();
    let release = suggested_release(song, &ticks);
    assert!((release - 175.0).abs() < 0.5, "{release}");
    // No rhythm to read: the default. A very close pair and a far apart one are clamped.
    assert_eq!(suggested_release(song, &[0]), DEFAULT_RELEASE_MS);
    assert_eq!(suggested_release(song, &[]), DEFAULT_RELEASE_MS);
    assert_eq!(suggested_release(song, &[0, 10]), 20.0);
    assert_eq!(suggested_release(song, &[0, 64 * PPQ]), 2000.0);
    // Hits at the same tick have no gap.
    assert_eq!(suggested_release(song, &[5, 5]), DEFAULT_RELEASE_MS);
}

#[test]
fn the_reverb_decay_is_twice_the_gap_within_its_range() {
    let r = reg();
    let d = project(&r, 8);
    let song = &d.doc().song;
    // A beat is 0.5 s: twice that is 1 s, the least; two beats apart is 2 s, a bar apart 4 s.
    let at = |gap: i64| [0, gap, 2 * gap, 3 * gap];
    assert!((suggested_decay(song, &at(PPQ)) - 1.0).abs() < 1.0e-3);
    assert!((suggested_decay(song, &at(2 * PPQ)) - 2.0).abs() < 1.0e-3);
    assert!((suggested_decay(song, &at(BAR)) - 4.0).abs() < 1.0e-3);
    assert_eq!(suggested_decay(song, &at(8 * BAR)), 6.0);
    assert_eq!(suggested_decay(song, &[0]), DEFAULT_DECAY);
    assert_eq!(suggested_decay(song, &[7, 7]), DEFAULT_DECAY);
}
