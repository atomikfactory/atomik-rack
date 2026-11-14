//! Quick sidechain in the audio: a steady bass under a kick on the beats. With the bass ducked
//! from the kick its level dips on every kick hit by about what the compressor's maths says, comes
//! back before the next beat, and the stretch before the first hit is the plain bass. The kick is
//! panned hard left and the bass hard right, so the right channel of the bounce is the bass alone.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::duck_from::{RATIO, THRESHOLD_DB};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand,
    WorkflowCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

const RATE: usize = 48_000;
/// One beat at 120 BPM, in frames.
const BEAT: usize = RATE / 2;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn clip(length: i64, notes: Vec<Note>) -> Clip {
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
        content: ClipContent::Notes(notes.into()),
    }
}

fn channel_of(doc: &RackDoc, track: &str) -> DeviceKey {
    doc.console.channel_of(&key(track)).unwrap().channel.clone()
}

fn set(k: &DeviceKey, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: k.clone(),
        param: param.into(),
        value,
        merge: None,
    }
}

/// A kick on beats 1 to 4 (the first beat is left quiet) and a bass held for the whole song,
/// the kick hard left and the bass hard right.
fn song() -> Document {
    let mut d = Document::new(empty_doc());
    let length = 6 * PPQ;
    let kicks: Vec<Note> = (1..5)
        .map(|b| Note::new(b * PPQ, PPQ / 4, 36, 1.0))
        .collect();
    for (k, notes) in [
        ("kick", kicks),
        ("bass", vec![Note::new(0, length, 43, 0.6)]),
    ] {
        for c in [
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            },
            Command::Song(SongCommand::AddTrack { device: key(k) }),
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: clip(length, notes),
            }),
        ] {
            apply(&mut d, c);
        }
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(length)));
    let (kick, bass) = (channel_of(d.doc(), "kick"), channel_of(d.doc(), "bass"));
    apply(&mut d, set(&kick, "pan", -1.0));
    apply(&mut d, set(&bass, "pan", 1.0));
    d
}

fn bounce(doc: &RackDoc) -> Vec<Vec<f32>> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    };
    common::render_guarded(doc, &default_registry(), &options)
        .unwrap()
        .channels
}

fn rms_db(x: &[f32]) -> f64 {
    let sum: f64 = x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    20.0 * (sum / x.len().max(1) as f64).sqrt().max(1e-9).log10()
}

fn peak_db(x: &[f32]) -> f64 {
    20.0 * f64::from(x.iter().fold(0.0_f32, |m, s| m.max(s.abs())))
        .max(1e-9)
        .log10()
}

/// The level of `window` frames from `at` of the right channel (the bass), in dB.
fn bass_level(out: &[Vec<f32>], at: usize, window: usize) -> f64 {
    rms_db(&out[1][at..at + window])
}

#[test]
fn the_bass_dips_on_every_kick_hit_and_comes_back_before_the_next_one() {
    let plain = song();
    let mut ducked = song();
    // A threshold well under the kick's level, so the dip is the compressor's static maths.
    apply(
        &mut ducked,
        Command::Workflow(WorkflowCommand::DuckFrom {
            target: key("bass"),
            source: key("kick"),
            pre_fader: false,
        }),
    );
    let (plain_out, ducked_out) = (bounce(plain.doc()), bounce(ducked.doc()));
    assert_eq!(plain_out[1].len(), ducked_out[1].len());

    // Before the first kick the bass is the plain bass.
    let before = bass_level(&plain_out, BEAT / 3, BEAT / 4);
    assert!(before > -40.0, "the bass is audible: {before} dB");
    let before_ducked = bass_level(&ducked_out, BEAT / 3, BEAT / 4);
    assert!(
        (before - before_ducked).abs() < 0.2,
        "no kick, no duck: {before} against {before_ducked} dB"
    );

    // The key: the kick on its own, from the left channel.
    let key_peak = peak_db(&plain_out[0][BEAT..2 * BEAT]);
    assert!(
        key_peak > THRESHOLD_DB as f64 + 6.0,
        "the kick is above the threshold: {key_peak}"
    );
    let expected = (1.0 - 1.0 / f64::from(RATIO)) * (key_peak - f64::from(THRESHOLD_DB));

    for beat in 1..=4 {
        // A window from 15 ms after the hit, past the attack and while the kick still rings.
        let at = beat * BEAT + RATE * 15 / 1000;
        let window = RATE * 40 / 1000;
        let dip = bass_level(&plain_out, at, window) - bass_level(&ducked_out, at, window);
        assert!(
            dip > 0.35 * expected,
            "beat {beat}: dips {dip:.1} dB, the maths says up to {expected:.1}"
        );
        assert!(
            dip < 1.2 * expected,
            "beat {beat}: dips {dip:.1} dB, the maths says up to {expected:.1}"
        );
        // Most of a beat later (the release is 120 ms) the bass is back.
        let late = (beat + 1) * BEAT - RATE * 100 / 1000;
        if beat < 4 {
            let gap = bass_level(&plain_out, late, window) - bass_level(&ducked_out, late, window);
            assert!(
                gap.abs() < 1.5,
                "beat {beat}: back within 1.5 dB, off by {gap:.1}"
            );
        }
    }
}

#[test]
fn a_pre_fader_key_ducks_even_when_the_source_fader_is_down() {
    let down = |pre: bool| {
        let mut d = song();
        let kick = channel_of(d.doc(), "kick");
        // The kick fader nearly closed: the post-fader key has little level left.
        apply(&mut d, set(&kick, "fader", 0.1));
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::DuckFrom {
                target: key("bass"),
                source: key("kick"),
                pre_fader: pre,
            }),
        );
        bounce(d.doc())
    };
    let plain = bounce(song().doc());
    let at = BEAT + RATE * 15 / 1000;
    let window = RATE * 40 / 1000;
    let post = bass_level(&plain, at, window) - bass_level(&down(false), at, window);
    let pre = bass_level(&plain, at, window) - bass_level(&down(true), at, window);
    assert!(
        pre > post + 3.0,
        "pre-fader ducks deeper ({pre:.1} dB) than post-fader ({post:.1} dB)"
    );
}

#[test]
fn removing_the_ducking_gives_the_plain_bounce_back() {
    let plain = bounce(song().doc());
    let mut d = song();
    for c in [
        WorkflowCommand::DuckFrom {
            target: key("bass"),
            source: key("kick"),
            pre_fader: false,
        },
        WorkflowCommand::RemoveDucking {
            target: key("bass"),
        },
    ] {
        apply(&mut d, Command::Workflow(c));
    }
    let after = bounce(d.doc());
    assert_eq!(
        plain[1].iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        after[1].iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );
}
