//! QA gap tests for the renderer and for engine criteria that need the real devices.
//!
//! * `qa_cli_note_onset_is_sample_exact`: platform-independent end-to-end timing check (the
//!   byte-exact golden only runs on Windows x86-64, and the RMS fingerprint cannot see a
//!   one-frame shift; found by the "shift note frames by one" mutation check).
//! * `qa_ac_e1_*`: taken literally (an audio output into a CV input) on real devices.
//! * `qa_ac_e6_*`: with the real devices of slice-demo at every host block.
//! * `qa_ac_g3_*`: the automatable half of the originality checks.
atomik_testkit::install_alloc_guard!();

use atomik_core::BATCH;
use atomik_devices::{line_mixer, poly_synth};
use atomik_engine::{EngineConfig, Graph, GraphError, OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_atomik-render");

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// A fresh temp directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("atomik-qa-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Synth params that make the onset of a note immediately visible (saw, open filter).
const ONSET_PARAMS: [(&str, f32); 4] = [
    ("osc1.wave", 0.0),
    ("osc.mix", 0.0),
    ("filter.cutoff", 20_000.0),
    ("filter.env_amount", 0.0),
];

fn first_nonzero(x: &[f32]) -> Option<usize> {
    x.iter().position(|s| s.to_bits() != 0)
}

#[test]
fn qa_cli_note_onset_is_sample_exact() {
    // Device-intrinsic onset delay `d`: a note at offset 0 first sounds at sample d.
    let mut h = DeviceHarness::new(Box::new(poly_synth::PolySynth::new()), 48_000.0);
    for (k, v) in ONSET_PARAMS {
        h.set_param(k, v);
    }
    h.push_note(
        "notes_in",
        atomik_core::NoteEvent {
            offset: 0,
            kind: atomik_core::NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        },
    );
    let d = first_nonzero(&h.render(1, &["out_l"])[0]).expect("note sounds in its batch");
    assert!(d < BATCH);

    // Notes at 1.5 beats (frame 36 000 = batch 562, offset 32) and 0.25 beats (6 000, offset 48)
    // at 120 BPM / 48 kHz must first sound at exactly frame + d in the WAV written by the CLI.
    let dir = TempDir::new("onset");
    for (at_beats, frame) in [(1.5f64, 36_000usize), (0.25, 6_000)] {
        let params: serde_json::Map<String, serde_json::Value> = ONSET_PARAMS
            .iter()
            .map(|(k, v)| ((*k).to_string(), serde_json::json!(v)))
            .collect();
        let patch = serde_json::json!({
            "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
            "length_beats": 2, "output_channels": 2,
            "devices": [{ "id": "synth", "type": "atomik.poly-synth", "params": params }],
            "cables": [
                { "from": "synth:out_l", "to": "interface:out_1" },
                { "from": "synth:out_r", "to": "interface:out_2" }
            ],
            "notes": [{ "device": "synth", "socket": "notes_in", "at_beats": at_beats,
                        "length_beats": 0.25, "key": 60, "velocity": 1.0 }]
        });
        let patch_path = dir.0.join("onset.json");
        std::fs::write(&patch_path, patch.to_string()).unwrap();
        for block in ["1", "100", "512"] {
            let out = dir.0.join(format!("onset-{block}.wav"));
            let output = Command::new(BIN)
                .arg(&patch_path)
                .arg("-o")
                .arg(&out)
                .args(["--block", block])
                .output()
                .expect("run atomik-render");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let wav = atomik_render::wav::read_wav_file(&out).unwrap();
            for ch in &wav.channels {
                assert_eq!(
                    first_nonzero(ch),
                    Some(frame + d),
                    "note at {at_beats} beats, --block {block}"
                );
            }
        }
    }
}

fn graph_with(
    devices: &[atomik_device_sdk::DeviceFactory],
) -> (Graph, Vec<atomik_engine::DeviceInstanceId>) {
    let cfg = EngineConfig {
        sample_rate: atomik_core::SampleRate::new(48_000.0).unwrap(),
        tempo: atomik_core::Tempo::new(120.0).unwrap(),
        input_channels: 2,
        output_channels: 2,
        seed: 1,
    };
    let mut g = Graph::new(&cfg);
    let ids = devices.iter().map(|f| g.add_device((f.create)())).collect();
    (g, ids)
}

fn endpoint(
    g: &Graph,
    device: atomik_engine::DeviceInstanceId,
    key: &str,
) -> atomik_engine::Endpoint {
    let s = g.descriptor(device).unwrap().socket_by_key(key).unwrap();
    atomik_engine::Endpoint {
        device,
        socket: s.id,
    }
}

#[test]
fn qa_ac_e1_audio_output_into_cv_input_is_kind_mismatch_and_graph_unchanged() {
    let (mut g, ids) = graph_with(&[poly_synth::factory(), line_mixer::factory()]);
    let (synth, mixer) = (ids[0], ids[1]);
    // A valid cable first, so "unchanged" is checked against a non-empty graph.
    g.connect(
        endpoint(&g, synth, "out_l"),
        endpoint(&g, mixer, "ch1_in_l"),
    )
    .unwrap();
    let before: Vec<_> = g.cables().copied().collect();
    let cases = [
        // literally: audio output -> CV input.
        ((mixer, "master_l"), (synth, "cutoff_cv_in")),
        ((synth, "out_r"), (mixer, "ch2_level_cv_in")),
        // The reverse direction of kinds: CV output -> audio input.
        ((synth, "lfo_cv_out"), (mixer, "ch3_in_l")),
    ];
    for ((fd, fk), (td, tk)) in cases {
        assert_eq!(
            g.connect(endpoint(&g, fd, fk), endpoint(&g, td, tk)),
            Err(GraphError::KindMismatch),
            "{fk} -> {tk}"
        );
        assert_eq!(
            g.cables().copied().collect::<Vec<_>>(),
            before,
            "{fk} -> {tk}"
        );
    }
    // The graph still compiles to the same valid plan.
    assert!(g.compile().is_ok());
}

/// Renders the first `frames` of slice-demo with real devices at `host_block`, under the guard.
fn slice_demo(frames: usize, host_block: usize) -> Vec<Vec<u32>> {
    let text = std::fs::read_to_string(repo("examples/patches/slice-demo.json")).unwrap();
    let patch = Patch::from_json(&text).unwrap();
    let built = build(&patch, &atomik_render::default_registry()).unwrap();
    let job = OfflineJob {
        frames,
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

#[test]
fn qa_ac_e6_real_devices_bit_identical_for_every_host_block() {
    // 3 beats: several notes, synth voices, delay echoes and mixer smoothing are all active.
    const FRAMES: usize = 3 * 24_000;
    let reference = slice_demo(FRAMES, 1);
    assert!(
        reference
            .iter()
            .flatten()
            .any(|b| f32::from_bits(*b).abs() > 1e-3),
        "not trivially silent"
    );
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            slice_demo(FRAMES, block) == reference,
            "HI-2: host block {block} differs from host block 1"
        );
    }
}

#[test]
fn qa_render_json_matches_the_cli_output_bit_for_bit() {
    // `render_json` is public API with no other test (coverage report, 2026-10-14).
    let patch = repo("examples/patches/minimal.json");
    let text = std::fs::read_to_string(&patch).unwrap();
    let block = std::num::NonZeroUsize::new(100).unwrap();
    let rendered =
        atomik_render::render_json(&text, &atomik_render::default_registry(), block).unwrap();
    assert_eq!(rendered.sample_rate, 48_000);
    assert!(rendered.warnings.is_empty());
    assert_eq!(rendered.channels.len(), 2);
    assert!(rendered.channels.iter().all(|c| c.len() == 4 * 24_000));

    let dir = TempDir::new("render-json");
    let out = dir.0.join("cli.wav");
    let output = Command::new(BIN)
        .arg(&patch)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run atomik-render");
    assert!(output.status.success());
    let wav = atomik_render::wav::read_wav_file(&out).unwrap();
    for (api, cli) in rendered.channels.iter().zip(&wav.channels) {
        assert!(
            api.iter()
                .map(|s| s.to_bits())
                .eq(cli.iter().map(|s| s.to_bits())),
            "render_json and the CLI (f32) agree bit for bit"
        );
    }
    // Errors carry the JSON path, like the CLI.
    let bad = text.replace("\"filter.cutoff\"", "\"filter.cutof\"");
    let err = atomik_render::render_json(&bad, &atomik_render::default_registry(), block)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("devices[0].params.\"filter.cutof\""),
        "{err}"
    );
}
