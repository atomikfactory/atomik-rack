//! The space reverb, the vocoder and the convolution reverb bounced through the real build paths.
//!
//! * an inline patch (a poly synth into the space reverb with a gated tail and a high cut, into the
//!   vocoder on its internal carrier) sounds, stays finite and bounded, is bit-identical at every host
//!   block size under the allocation guard, and is identical when bounced twice;
//! * the new params at their defaults sound like a patch that never names them;
//! * a project with a convolution reverb on a send, its impulse response a generated file in a
//!   temporary bundle, plays the impulse response (the bounce differs from the same project without
//!   it) and is bit-identical across host blocks and worker counts.
atomik_testkit::install_alloc_guard!();

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_engine::{OfflineJob, render_offline_with};
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, CreateTarget,
    DeviceKey, Document, Note, RackDoc, SongCommand, WorkflowCommand, ZoneDoc, new_project_doc,
};
use atomik_render::build::build;
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FRAMES: usize = 3 * 48_000;

const TEMPLATE: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 110, "seed": 5,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "synth", "type": "atomik.poly-synth", "params": { "filter.cutoff": 2400, "spread": 0.6 } },
    { "id": "space", "type": "atomik.space-reverb", "params": { {SPACE} } },
    { "id": "voc", "type": "atomik.vocoder", "params": { {VOC} } }
  ],
  "cables": [
    { "from": "synth:out_l", "to": "space:in_l" }, { "from": "synth:out_r", "to": "space:in_r" },
    { "from": "space:out_l", "to": "voc:in_l" }, { "from": "space:out_r", "to": "voc:in_r" },
    { "from": "voc:out_l", "to": "interface:out_1" },
    { "from": "voc:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 55, "velocity": 0.9 },
    { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 62, "velocity": 0.9 },
    { "device": "synth", "socket": "notes_in", "at_beats": 1.5, "length_beats": 1, "key": 60, "velocity": 0.8 },
    { "device": "synth", "socket": "notes_in", "at_beats": 2.5, "length_beats": 1.5, "key": 67, "velocity": 0.8 }
  ]
}"#;

const SPACE_ON: &str = r#""algorithm": 2, "mix": 0.5, "decay": 2.5, "gate": 120, "gate_level": -45, "highcut": 6000, "size": 0.6"#;
const VOC_ON: &str =
    r#""bands": 24, "low": 120, "high": 9000, "bandwidth": 1.3, "hiss": 0.5, "mix": 0.7"#;
const SPACE_PLAIN: &str = r#""mix": 0.5"#;
const VOC_PLAIN: &str = r#""mix": 0.7"#;
const SPACE_DEFAULTS: &str =
    r#""algorithm": 0, "mix": 0.5, "decay": 3, "highcut": 20000, "gate": 0, "gate_level": -40"#;
const VOC_DEFAULTS: &str = r#""mix": 0.7, "bands": 16, "low": 100, "high": 8000, "bandwidth": 1, "attack": 2, "release": 40, "hiss": 0.3, "output": 0"#;

fn patch(space: &str, voc: &str) -> String {
    TEMPLATE.replace("{SPACE}", space).replace("{VOC}", voc)
}

fn bounce(text: &str, host_block: usize) -> Vec<Vec<u32>> {
    let patch = Patch::from_json(text).unwrap();
    let built = build(&patch, &default_registry()).unwrap();
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    let job = OfflineJob {
        frames: FRAMES,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    render_offline_with(built.plan, built.config, &job, |f| assert_no_alloc(f))
        .iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

fn floats(out: &[Vec<u32>], c: usize) -> Vec<f32> {
    out[c].iter().map(|b| f32::from_bits(*b)).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn the_patch_sounds_in_stereo_and_stays_bounded() {
    let out = bounce(&patch(SPACE_ON, VOC_ON), 64);
    let (l, r) = (floats(&out, 0), floats(&out, 1));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() <= 1.5));
    assert!(
        rms(&l) > 0.001 && rms(&r) > 0.001,
        "{} {}",
        rms(&l),
        rms(&r)
    );
    assert_ne!(l, r);
}

#[test]
fn the_patch_is_bit_identical_for_every_host_block() {
    let text = patch(SPACE_ON, VOC_ON);
    let reference = bounce(&text, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(&text, block) == reference,
            "host block {block} differs from host block 1"
        );
    }
    assert!(bounce(&text, 64) == bounce(&text, 64));
}

#[test]
fn naming_the_params_at_their_defaults_changes_nothing() {
    let plain = patch(SPACE_PLAIN, VOC_PLAIN);
    let named = patch(SPACE_DEFAULTS, VOC_DEFAULTS);
    assert_ne!(plain, named);
    assert!(bounce(&plain, 64) == bounce(&named, 64));
    assert!(bounce(&plain, 64) != bounce(&patch(SPACE_ON, VOC_ON), 64));
}

// ------------------------------------------------------------------ the convolution reverb

const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-effects-reverb-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn apply(doc: &mut Document, c: Command) {
    doc.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A generated impulse response: half a second of decaying noise.
fn impulse_response() -> AudioData {
    let mut rng = atomik_dsp::Rng::new(77);
    let frames = 24_000;
    let ir: Vec<f32> = (0..frames)
        .map(|i| rng.next_bipolar() * (-(i as f32) / 5_000.0).exp())
        .collect();
    AudioData::new(48_000, vec![ir]).expect("valid mono data")
}

/// A synth on its own channel sending 0 dB to a convolution reverb on send 1; `with_ir` loads the
/// impulse response as its zone.
fn project(root: &Path, with_ir: bool) -> RackDoc {
    let mut d = Document::new(new_project_doc());
    let data = impulse_response();
    let mut bundle = Bundle::create(root).expect("bundle");
    bundle.add_recording(&data, "ir.wav").expect("asset");
    let hash = content_hash(&data);
    apply(
        &mut d,
        Command::RegisterAsset {
            hash: hash.clone(),
            meta: AssetMeta {
                channels: 1,
                sample_rate: 48_000,
                frames: data.frames() as u64,
                original_name: "ir.wav".into(),
            },
        },
    );
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type("atomik.convolver".into()),
            target: CreateTarget::Master,
            key: Some(key("ir")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        Command::SetParam {
            key: key("ir"),
            param: "mix".into(),
            value: 1.0,
            merge: None,
        },
    );
    if with_ir {
        apply(
            &mut d,
            Command::SetZones {
                device: key("ir"),
                zones: vec![ZoneDoc::new(hash, data.frames() as u64)],
                merge: None,
            },
        );
    }
    let lead_channel = d
        .doc()
        .console
        .channel_of(&key("lead"))
        .expect("the lead channel")
        .channel
        .clone();
    for (p, v) in [("send1.on", 1.0), ("send1.level", 0.0)] {
        apply(
            &mut d,
            Command::SetParam {
                key: lead_channel.clone(),
                param: p.into(),
                value: v,
                merge: None,
            },
        );
    }
    let notes: Vec<Note> = [(0, 57u8), (PPQ, 64), (2 * PPQ, 60)]
        .iter()
        .map(|(t, k)| Note::new(*t, PPQ, *k, 0.8))
        .collect();
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 2 * BAR,
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
                content: ClipContent::Notes(notes.into()),
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    apply(
        &mut d,
        Command::Song(SongCommand::SetLoop {
            range: (0, 2 * BAR),
            enabled: false,
        }),
    );
    d.doc().clone()
}

fn render(doc: &RackDoc, root: &Path, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc_in(doc, &default_registry(), &options, Some(root))
        .unwrap()
        .channels
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect()
}

#[test]
fn the_convolution_reverb_plays_its_impulse_response_deterministically() {
    let t = Tmp::new();
    let with = project(&t.0, true);
    let without = project(&t.0, false);
    let a = render(&with, &t.0, 64, 1);
    assert!(a.iter().all(|b| f32::from_bits(*b).is_finite()));
    assert!(
        a != render(&without, &t.0, 64, 1),
        "the impulse response is audible"
    );
    for (block, workers) in [(1, 1), (100, 1), (512, 2), (4096, 4)] {
        assert!(
            render(&with, &t.0, block, workers) == a,
            "block {block}, {workers} workers"
        );
    }
}
