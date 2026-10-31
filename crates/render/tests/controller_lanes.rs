//! Controller lanes in a bounce: a pitch bend lane of a note clip bends the poly synth at the
//! tick it says (and not before), the lane-free project does not bend,
//! the bounce does not depend on the host block size or the workers, and a saved and loaded
//! project bounces the same.

use atomik_core::PPQ;
use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Document, Note, RackDoc,
    SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_testkit::signal::dominant_frequency;
use std::num::NonZeroUsize;

const SR: f64 = 48_000.0;

/// A4 held for four beats at 120 BPM (two seconds), with `lanes` on the clip.
fn song(lanes: Vec<ControllerLane>) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let synth = DeviceKey("synth".into());
    let length = 4 * PPQ;
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
        Command::Song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
        Command::Song(SongCommand::AddClip {
            device: synth.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                name: None,
                color: None,
                scale: None,
                controllers: lanes,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, length, 69, 1.0)].into()),
            },
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    let mut doc = d.doc().clone();
    // A pure, wide-open sine so the pitch is easy to read.
    let dev = doc.devices.iter_mut().find(|d| d.key == synth).unwrap();
    for (k, v) in [
        ("osc1.wave", 3.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.resonance", 0.0),
    ] {
        dev.params.insert(k.to_owned(), v);
    }
    doc
}

fn bend_up_at_beat_two() -> Vec<ControllerLane> {
    vec![ControllerLane::new(
        ControllerKind::PitchBend,
        vec![AutomationPoint {
            tick: 2 * PPQ,
            value: 1.0,
            curve: Curve::Step,
        }],
    )]
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
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

#[test]
fn a_pitch_bend_lane_bends_the_note_from_its_tick_on() {
    let out = bounce(&song(bend_up_at_beat_two()), 512, 0);
    // Beat 2 is one second in: A4 (440 Hz) before, two semitones up (493.9 Hz) after.
    let before = dominant_frequency(&out[9_600..43_200], SR);
    let after = dominant_frequency(&out[52_800..91_200], SR);
    assert!((before - 440.0).abs() < 2.0, "{before}");
    assert!((after - 493.88).abs() < 3.0, "{after}");
    let plain = bounce(&song(Vec::new()), 512, 0);
    assert!((dominant_frequency(&plain[52_800..91_200], SR) - 440.0).abs() < 2.0);
    assert_ne!(out, plain);
}

#[test]
fn the_bounce_is_the_same_at_any_block_size_and_worker_count() {
    let doc = song(bend_up_at_beat_two());
    let base = bounce(&doc, 64, 0);
    for (block, workers) in [(333, 0), (512, 0), (512, 2)] {
        assert_eq!(bounce(&doc, block, workers), base, "{block} / {workers}");
    }
}

#[test]
fn a_saved_and_loaded_project_bounces_the_same() {
    let doc = song(bend_up_at_beat_two());
    let text = save_project(&doc);
    assert!(text.contains("\"pitch_bend\""));
    let back = load_project(&text, &default_registry()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(bounce(&back, 512, 0), bounce(&doc, 512, 0));
}
