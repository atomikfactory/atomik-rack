//! The master tune in a bounce: +50 cents moves a sine by 2^(50/1200), the bounce is the same bits
//! for every block size and worker count, and undoing the change gives the untuned bits back.
atomik_testkit::install_alloc_guard!();

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_testkit::signal::dominant_frequency;
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const RATE: f64 = 48_000.0;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A poly synth on a pure sine playing key 69 (A4) for two bars.
fn project() -> Document {
    let mut d = Document::new(empty_doc());
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".to_owned(),
            key: Some(key("synth")),
        },
    );
    for (param, value) in [
        ("osc1.wave", 3.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("env.amp.attack", 2.0),
        ("env.amp.sustain", 1.0),
    ] {
        apply(
            &mut d,
            Command::SetParam {
                key: key("synth"),
                param: param.to_owned(),
                value,
                merge: None,
            },
        );
    }
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
                id: ClipId(0),
                start: 0,
                length: 2 * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 2 * BAR, 69, 0.8)].into()),
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    d
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<Vec<f32>> {
    render_doc_in(doc, &default_registry(), &options(block, workers), None)
        .unwrap()
        .channels
}

fn bits(channels: &[Vec<f32>]) -> Vec<u32> {
    channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn pitch(doc: &RackDoc) -> f64 {
    let out = bounce(doc, 512, 0);
    dominant_frequency(&out[0][12_000..12_000 + 65_536], RATE)
}

#[test]
fn fifty_cents_move_a_sine_by_the_expected_ratio() {
    let mut d = project();
    let flat = pitch(d.doc());
    assert!((flat - 440.0).abs() < 1.0, "{flat}");
    for cents in [50.0f32, -100.0] {
        apply(&mut d, Command::SetMasterTune(cents));
        let f = pitch(d.doc());
        let want = 440.0 * 2f64.powf(f64::from(cents) / 1200.0);
        assert!((f - want).abs() < 0.6, "{cents} cents: {f} vs {want}");
        assert!((f / flat - 2f64.powf(f64::from(cents) / 1200.0)).abs() < 0.002);
    }
}

#[test]
fn a_tuned_bounce_is_the_same_bits_for_every_block_size_and_worker_count() {
    let mut d = project();
    apply(&mut d, Command::SetMasterTune(37.5));
    let first = bits(&bounce(d.doc(), 512, 0));
    for (block, workers) in [(64, 0), (480, 2), (1000, 3), (333, 0)] {
        assert!(
            bits(&bounce(d.doc(), block, workers)) == first,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn undoing_the_tune_gives_the_untuned_bits_back() {
    let mut d = project();
    let before = bits(&bounce(d.doc(), 512, 0));
    apply(&mut d, Command::SetMasterTune(50.0));
    assert!(bits(&bounce(d.doc(), 512, 0)) != before);
    d.undo(&default_registry()).unwrap();
    assert!(bits(&bounce(d.doc(), 512, 0)) == before);
}
