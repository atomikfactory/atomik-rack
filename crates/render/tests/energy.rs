//! The master of a song measured for the energy map: a section where more plays reads louder than
//! a quiet one, the blocks are the same at every host block size, a stopped render returns
//! nothing and a document without a master section is refused.

use atomik_core::PPQ;
use atomik_dsp::energy::Blocks;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::energy::render_blocks;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::sync::atomic::AtomicBool;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn sustained(start: i64, length: i64, keys: &[u8]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
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
        content: ClipContent::Notes(keys.iter().map(|&k| Note::new(0, length, k, 0.9)).collect()),
    }
}

/// A quiet pad that plays for four bars and a loud lead that joins it for the last two.
fn song() -> RackDoc {
    let mut d = Document::new(new_project_doc());
    let tracks: [(&str, f32, i64, &[u8]); 2] = [
        ("pad", -24.0, 0, &[48, 55]),
        ("lead", -6.0, 2 * BAR, &[72, 79, 84]),
    ];
    for (k, level, start, keys) in tracks {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        for (p, v) in [
            ("osc1.wave", 0.0f32),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
            ("env.amp.release", 100.0),
        ] {
            apply(
                &mut d,
                Command::SetParam {
                    key: key(k),
                    param: p.into(),
                    value: v,
                    merge: None,
                },
            );
        }
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: sustained(start, 4 * BAR - start, keys),
            }),
        );
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(4 * BAR)));
    d.doc().clone()
}

fn options(block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: std::num::NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn blocks(doc: &RackDoc, block: usize) -> Blocks {
    render_blocks(
        doc,
        &default_registry(),
        (&options(block), None),
        &mut PluginBounce::default(),
        RenderControl::default(),
    )
    .expect("renders")
    .expect("not stopped")
}

#[test]
fn the_section_where_more_plays_reads_louder_and_brighter() {
    let doc = song();
    let b = blocks(&doc, 512);
    let at = |bar: i64| doc.song.frame_at(bar * BAR, doc.sample_rate);
    let quiet = b.features(at(0), at(2));
    let loud = b.features(at(2), at(4));
    assert!(
        loud.loudness_db > quiet.loudness_db + 6.0,
        "{quiet:?} then {loud:?}"
    );
    assert!(
        loud.brightness_hz > quiet.brightness_hz,
        "{quiet:?} {loud:?}"
    );
}

#[test]
fn the_blocks_are_the_same_at_every_host_block_size() {
    let doc = song();
    assert_eq!(blocks(&doc, 512), blocks(&doc, 64));
}

#[test]
fn a_stopped_render_returns_nothing() {
    let doc = song();
    let stop = AtomicBool::new(true);
    let result = render_blocks(
        &doc,
        &default_registry(),
        (&options(512), None),
        &mut PluginBounce::default(),
        RenderControl {
            cancel: Some(&stop),
            progress: None,
        },
    )
    .expect("no error");
    assert!(result.is_none());
}

#[test]
fn a_document_without_a_master_is_refused() {
    let mut doc = song();
    doc.console.master = None;
    let err = render_blocks(
        &doc,
        &default_registry(),
        (&options(512), None),
        &mut PluginBounce::default(),
        RenderControl::default(),
    )
    .expect_err("no master");
    assert!(err.to_string().contains("master"), "{err}");
}
