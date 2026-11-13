//! Per-note expression in a bounce: the pitch curve of a note bends the poly synth voice of that
//! note (and not the note beside it), a project without expression bounces as it always did, the
//! bounce does not depend on the host block size or the workers, and a saved and loaded project
//! bounces the same.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, ExpressionPoint, MidiData, Note,
    NoteExpression, RackDoc, SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use atomik_testkit::signal::dominant_frequency;
use std::num::NonZeroUsize;

const SR: f64 = 48_000.0;

/// Notes held for four beats at 120 BPM (two seconds) with `expression` in the clip.
fn song(keys: &[u8], expression: Vec<NoteExpression>) -> RackDoc {
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
                param_lanes: Vec::new(),
                midi: MidiData {
                    expression,
                    ..Default::default()
                }
                .into(),
                groove: None,
                follow_song_key: true,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(
                    keys.iter().map(|k| Note::new(0, length, *k, 1.0)).collect(),
                ),
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

/// The tuning of the note on `key`: unchanged for a beat, then up an octave.
fn octave_up_at_beat_two(key: u8) -> NoteExpression {
    let mut e = NoteExpression::new(0, key);
    e.pitch = vec![
        ExpressionPoint::new(0, 0.0),
        ExpressionPoint::new(2 * PPQ - 1, 0.0),
        ExpressionPoint::new(2 * PPQ, 12.0),
    ];
    e
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

#[test]
fn a_pitch_curve_bends_its_note_from_its_tick_on() {
    let out = bounce(&song(&[69], vec![octave_up_at_beat_two(69)]), 512, 0);
    // Beat 2 is one second in: A4 (440 Hz) before, an octave up (880 Hz) after.
    let before = dominant_frequency(&out[9_600..43_200], SR);
    let after = dominant_frequency(&out[52_800..91_200], SR);
    assert!((before - 440.0).abs() < 2.0, "{before}");
    assert!((after - 880.0).abs() < 4.0, "{after}");
    let plain = bounce(&song(&[69], Vec::new()), 512, 0);
    assert!((dominant_frequency(&plain[52_800..91_200], SR) - 440.0).abs() < 2.0);
    assert_ne!(out, plain);
}

#[test]
fn a_curve_belongs_to_its_note_alone() {
    // Two notes, an octave apart in the output once the first is tuned up: the other voice
    // keeps its pitch, so the spectrum has both A5 and the second note's own pitch.
    let doc = song(&[69, 76], vec![octave_up_at_beat_two(69)]);
    let out = bounce(&doc, 512, 0);
    let spectrum = atomik_testkit::signal::magnitude_spectrum(&out[52_800..52_800 + 32_768]);
    let bin = |hz: f64| (hz * 32_768.0 / SR).round() as usize;
    let near = |hz: f64| {
        let b = bin(hz);
        spectrum[b - 3..=b + 3]
            .iter()
            .cloned()
            .fold(0.0f64, f64::max)
    };
    let (a5, e5, a4) = (near(880.0), near(659.26), near(440.0));
    assert!(a5 > 10.0 * a4, "the first note left A4: {a5} vs {a4}");
    assert!(
        e5 > 10.0 * a4,
        "the second note stayed where it was: {e5} vs {a4}"
    );
}

#[test]
fn the_bounce_is_the_same_at_any_block_size_and_worker_count() {
    let doc = song(&[69, 76], vec![octave_up_at_beat_two(69)]);
    let base = bounce(&doc, 64, 0);
    for (block, workers) in [(333, 0), (512, 0), (512, 2)] {
        assert_eq!(bounce(&doc, block, workers), base, "{block} / {workers}");
    }
}

#[test]
fn a_saved_and_loaded_project_bounces_the_same() {
    let doc = song(&[69], vec![octave_up_at_beat_two(69)]);
    let text = save_project(&doc);
    assert!(text.contains("\"expression\""));
    let back = load_project(&text, &default_registry()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(bounce(&back, 512, 0), bounce(&doc, 512, 0));
}
