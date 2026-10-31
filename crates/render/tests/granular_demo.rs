//! The `granular-demo` example patch (`examples/patches/granular-demo.json`): a frozen pad, a
//! shimmer and a stutter, all `atomik.granular` on the built-in source and loaded with factory
//! sounds, bounced through the real build path; and a granular sampler on a sample of its own
//! (generated here, in a temporary bundle) bounced as a project.
//!
//! * the patch sounds, stays finite and stereo;
//! * it is bit-identical at every host block size, under the allocation guard (HI-1 and HI-2);
//! * the same patch bounced twice is identical;
//! * the three devices carry exactly the params of the factory sounds, so the patch and the
//!   sounds do not drift apart;
//! * every factory sound bounces finite and within the output bound;
//! * a sample project is bit-identical across host blocks and worker counts, plays the sample
//!   (not the built-in pad) and follows the position knob through it.
atomik_testkit::install_alloc_guard!();

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_devices::granular::sounds;
use atomik_engine::{OfflineJob, render_offline_with};
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand,
    ZoneDoc, empty_doc,
};
use atomik_render::build::build;
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FRAMES: usize = 4 * 48_000;

fn patch_text() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/patches/granular-demo.json");
    std::fs::read_to_string(path).unwrap()
}

/// Renders the first `FRAMES` frames of `text` at `host_block`, under the guard.
fn bounce(text: &str, host_block: usize) -> Vec<Vec<u32>> {
    let patch = Patch::from_json(text).unwrap();
    let built = build(&patch, &atomik_render::default_registry()).unwrap();
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

fn channels(out: &[Vec<u32>]) -> (Vec<f32>, Vec<f32>) {
    let f = |c: usize| -> Vec<f32> { out[c].iter().map(|b| f32::from_bits(*b)).collect() };
    (f(0), f(1))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn the_demo_sounds_in_stereo_and_stays_bounded() {
    let (l, r) = channels(&bounce(&patch_text(), 64));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
    assert!(rms(&l) > 0.02 && rms(&r) > 0.02, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r, "the grain spread makes the channels differ");
    // The pad swells in: the first tenth of a second is quieter than the middle of the chord.
    assert!(rms(&l[..4_800]) < 0.5 * rms(&l[48_000..96_000]));
    // The pad's release carries into the end.
    assert!(rms(&l[FRAMES - 24_000..]) > 1e-3);
}

#[test]
fn the_demo_is_bit_identical_for_every_host_block() {
    let text = patch_text();
    let reference = bounce(&text, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(&text, block) == reference,
            "HI-2: host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_patch_twice_is_identical_and_the_seed_matters() {
    let text = patch_text();
    assert!(bounce(&text, 64) == bounce(&text, 64));
    assert!(bounce(&text.replace("\"seed\": 5,", "\"seed\": 6,"), 64) != bounce(&text, 64));
}

#[test]
fn the_demo_devices_carry_the_factory_sounds() {
    let patch: serde_json::Value = serde_json::from_str(&patch_text()).unwrap();
    for (id, sound) in [("pad", 1), ("shimmer", 2), ("beat", 3)] {
        let dev = patch["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .unwrap();
        let params = dev["params"].as_object().unwrap();
        for (key, value) in sounds::values(sound) {
            // The patch lists what differs from the defaults; the rest is the default.
            let default = atomik_devices::granular::DESCRIPTOR
                .param_by_key(key)
                .unwrap()
                .default;
            let got = params
                .get(key)
                .map_or(default, |v| v.as_f64().unwrap() as f32);
            assert_eq!(got, value, "{id}: {key}");
        }
        for key in params.keys() {
            assert!(
                sounds::SOUNDS[sound].changes.iter().any(|(k, _)| k == key),
                "{id}: {key} is not part of the sound"
            );
        }
    }
}

#[test]
fn every_factory_sound_bounces_finite_and_bounded() {
    let text = patch_text();
    // The pad device with each sound in turn: replace its params block.
    let mut patch: serde_json::Value = serde_json::from_str(&text).unwrap();
    for sound in 0..sounds::SOUNDS.len() {
        let params: serde_json::Map<String, serde_json::Value> = sounds::values(sound)
            .into_iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        for dev in patch["devices"].as_array_mut().unwrap() {
            if dev["type"] == "atomik.granular" {
                dev["params"] = serde_json::Value::Object(params.clone());
            }
        }
        let (l, r) = channels(&bounce(&patch.to_string(), 64));
        assert!(
            l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0),
            "{}",
            sounds::SOUND_NAMES[sound]
        );
        assert!(
            rms(&l) > 0.005,
            "{} is audible: {}",
            sounds::SOUND_NAMES[sound],
            rms(&l)
        );
    }
}

// ------------------------------------------------------------------ a sample project

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

/// Two seconds that change along their length: a low tone (130 Hz) in the first half, a high
/// one (520 Hz) in the second, a quarter of a second of blend between, faded at both ends.
fn sample() -> AudioData {
    let rate = 48_000.0f32;
    let frames = 96_000usize;
    let (mut p1, mut p2) = (0.0f32, 0.0f32);
    let out: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / frames as f32;
            let blend = ((t - 0.4) / 0.2).clamp(0.0, 1.0);
            let edge = (i.min(frames - 1 - i) as f32 / 480.0).min(1.0);
            p1 += 130.0 / rate;
            p1 -= if p1 >= 1.0 { 1.0 } else { 0.0 };
            p2 += 520.0 / rate;
            p2 -= if p2 >= 1.0 { 1.0 } else { 0.0 };
            (wave(p1) * (1.0 - blend) + wave(p2) * blend) * 0.8 * edge
        })
        .collect();
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-granular-demo-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A project: one granular sampler with the sample as its one zone, playing a chord for two
/// bars; `position` is the knob (0 the low tone, 1 the high one).
fn project(root: &Path, position: f32, with_sample: bool) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).expect("the commands are valid");
    };
    let data = sample();
    let mut bundle = Bundle::create(root).expect("bundle");
    bundle.add_recording(&data, "tones.wav").expect("asset");
    let hash = content_hash(&data);
    apply(Command::RegisterAsset {
        hash: hash.clone(),
        meta: AssetMeta {
            channels: 1,
            sample_rate: 48_000,
            frames: data.frames() as u64,
            original_name: "tones.wav".into(),
        },
    });
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.granular".into(),
        key: Some(key("g")),
    });
    for (p, v) in [
        ("pos.position", position),
        ("pos.spray", 0.02),
        ("grain.density", 40.0),
        ("grain.size", 80.0),
        ("pitch.key_track", 0.0),
        ("env.amp.attack", 20.0),
        ("master.level", -6.0),
    ] {
        apply(Command::SetParam {
            key: key("g"),
            param: p.into(),
            value: v,
            merge: None,
        });
    }
    if with_sample {
        apply(Command::SetZones {
            device: key("g"),
            zones: vec![ZoneDoc::new(hash, data.frames() as u64)],
            merge: None,
        });
    }
    let end = 8 * PPQ;
    apply(Command::Song(SongCommand::AddTrack { device: key("g") }));
    let notes: Vec<Note> = [48u8, 55, 60]
        .iter()
        .map(|k| Note::new(0, 7 * PPQ, *k, 0.8))
        .collect();
    apply(Command::Song(SongCommand::AddClip {
        device: key("g"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: end,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(notes.into()),
        },
    }));
    apply(Command::Song(SongCommand::SetSongEnd(end)));
    apply(Command::Song(SongCommand::SetLoop {
        range: (0, end),
        enabled: false,
    }));
    d.doc().clone()
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn render(doc: &RackDoc, root: &Path, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(
        doc,
        &default_registry(),
        &options(block, workers),
        Some(root),
    )
    .unwrap()
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

/// The frequency of the strongest of `candidates` in `x` (single-bin DFT).
fn strongest(x: &[f32], candidates: &[f64]) -> f64 {
    let level = |hz: f64| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, s) in x.iter().enumerate() {
            let w = std::f64::consts::TAU * hz * i as f64 / 48_000.0;
            re += f64::from(*s) * w.cos();
            im += f64::from(*s) * w.sin();
        }
        re.hypot(im)
    };
    *candidates
        .iter()
        .max_by(|a, b| level(**a).total_cmp(&level(**b)))
        .unwrap()
}

#[test]
fn a_sample_project_is_bit_identical_across_host_blocks_and_worker_counts() {
    let t = Tmp::new();
    let doc = project(&t.0, 0.25, true);
    let reference = bits(&render(&doc, &t.0, 512, 0));
    assert!(reference.len() > 400_000);
    for (block, workers) in [(64, 0), (333, 1), (1024, 2), (4096, 3)] {
        assert!(
            bits(&render(&doc, &t.0, block, workers)) == reference,
            "block {block}, {workers} workers"
        );
    }
}

#[test]
fn the_sample_is_played_and_the_position_picks_the_tone() {
    let t = Tmp::new();
    // Key tracking is off, so the chord plays the recorded pitch: 130 Hz at the start of the
    // sample, 520 Hz at the end.
    let low = render(&project(&t.0, 0.1, true), &t.0, 512, 0);
    let high = render(&project(&t.0, 0.9, true), &t.0, 512, 0);
    let (l, h) = (
        &low.channels[0][24_000..72_000],
        &high.channels[0][24_000..72_000],
    );
    assert!(rms(l) > 0.01 && rms(h) > 0.01, "{} {}", rms(l), rms(h));
    assert!((strongest(l, &[130.0, 520.0]) - 130.0).abs() < 1.0);
    assert!((strongest(h, &[130.0, 520.0]) - 520.0).abs() < 1.0);
    // Without the sample the built-in pad plays: another sound.
    let pad = render(&project(&t.0, 0.1, false), &t.0, 512, 0);
    assert!(bits(&pad) != bits(&low));
    assert!(rms(&pad.channels[0][24_000..72_000]) > 0.01);
}
