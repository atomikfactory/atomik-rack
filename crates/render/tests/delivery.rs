//! The master of a song measured for the delivery targets: a louder master reads louder by the same
//! amount, the report has a true peak at or above the sample peak, the numbers are the same at
//! every host block size, a stopped render returns nothing and a document without a master
//! section is refused.

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::delivery::measure_master;
use atomik_render::loudness::LoudnessReport;
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
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(keys.iter().map(|&k| Note::new(0, length, k, 0.9)).collect()),
    }
}

/// One sustained chord, `level` dB on the synth, four bars long.
fn song(level: f32) -> RackDoc {
    let mut d = Document::new(new_project_doc());
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
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
                key: key("lead"),
                param: p.into(),
                value: v,
                merge: None,
            },
        );
    }
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: sustained(0, 4 * BAR, &[60, 64, 67, 72]),
        }),
    );
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

fn report(doc: &RackDoc, block: usize) -> LoudnessReport {
    measure_master(
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
fn a_louder_master_reads_louder_by_the_same_amount() {
    let quiet = report(&song(-24.0), 512);
    let loud = report(&song(-12.0), 512);
    let (q, l) = (
        quiet.integrated_lufs.unwrap(),
        loud.integrated_lufs.unwrap(),
    );
    assert!((l - q - 12.0).abs() < 0.6, "{q} then {l}");
    let (qp, lp) = (quiet.true_peak_dbtp.unwrap(), loud.true_peak_dbtp.unwrap());
    assert!((lp - qp - 12.0).abs() < 0.6, "{qp} then {lp}");
}

#[test]
fn the_true_peak_is_never_below_the_sample_peak() {
    let r = report(&song(-12.0), 512);
    let (tp, sp) = (r.true_peak_dbtp.unwrap(), r.sample_peak_dbfs.unwrap());
    assert!(tp >= sp - 0.01, "{tp} dBTP under {sp} dBFS");
    assert!(sp < 0.0 && !r.is_silent(), "{r:?}");
}

#[test]
fn the_numbers_are_the_same_at_every_host_block_size() {
    let doc = song(-12.0);
    assert_eq!(report(&doc, 512), report(&doc, 64));
}

#[test]
fn a_stopped_render_returns_nothing() {
    let doc = song(-12.0);
    let stop = AtomicBool::new(true);
    let result = measure_master(
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
    let mut doc = song(-12.0);
    doc.console.master = None;
    let err = measure_master(
        &doc,
        &default_registry(),
        (&options(512), None),
        &mut PluginBounce::default(),
        RenderControl::default(),
    )
    .expect_err("no master");
    assert!(err.to_string().contains("master"), "{err}");
}
