//! The `sampler-demo` example project and its bundle, and the bounce of sampler
//! projects.
//!
//! `examples/projects/sampler-demo/` is a bundle folder: `sampler-demo.atomik` and `Audio/`
//! with five short **procedurally generated** mono samples ([`demo_samples`]: three plucks, a
//! noise snap and a steady pad; no third-party audio). Two samplers play a four-bar song:
//!
//! * `keys`: the three plucks auto-mapped by their note names (C3, G3, C4), plus a layered
//!   one-shot snap zone that only sounds on hard hits (velocity 96 and above).
//! * `pad`: one looped zone (forward loop with a 4 000-frame crossfade) held for a chord.
//!
//! Regenerate the bundle and the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test sampler_demo regenerate
//! ```
//!
//! The generator uses only IEEE `+ - * /` and comparisons (no libm calls), so it gives the same
//! samples on every platform; the committed WAVs are checked against it bit for bit.

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_device_sdk::{LoopMode, TriggerMode};
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand,
    ZoneDoc, auto_map, empty_doc, load_project, save_project,
};
use atomik_render::bundle::{existing_bundle_root, missing_assets};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const RATE: f32 = 48_000.0;
/// The whole bundle must stay small (the brief's budget).
const MAX_BUNDLE_BYTES: u64 = 2 * 1024 * 1024;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/sampler-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("sampler-demo.atomik")
}

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

fn advance(phase: &mut f32, hz: f32) {
    *phase += hz / RATE;
    *phase -= if *phase >= 1.0 { 1.0 } else { 0.0 };
}

/// A plucked tone: two harmonics with a decay and a 40-frame attack, `frames` long, ending silent.
fn pluck(hz: f32, frames: usize) -> AudioData {
    let (mut p1, mut p2, mut amp) = (0.0f32, 0.0f32, 0.8f32);
    let out: Vec<f32> = (0..frames)
        .map(|i| {
            let attack = if i < 40 { i as f32 / 40.0 } else { 1.0 };
            let left = (frames - i) as f32;
            let tail = if left < 200.0 {
                (left - 1.0) / 200.0
            } else {
                1.0
            };
            let s = (wave(p1) * 0.8 + wave(p2) * 0.25) * amp * attack * tail;
            advance(&mut p1, hz);
            advance(&mut p2, hz * 2.0);
            amp *= 0.99985;
            s
        })
        .collect();
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

/// A short burst of differentiated noise (a snap).
fn snap(frames: usize) -> AudioData {
    let mut x = 0x2545_f491u32;
    let (mut amp, mut last) = (0.7f32, 0.0f32);
    let out: Vec<f32> = (0..frames)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let n = (x >> 8) as f32 / 8_388_608.0 - 1.0;
            let left = (frames - i) as f32;
            let tail = if left < 64.0 {
                (left - 1.0) / 64.0
            } else {
                1.0
            };
            let s = (n - last) * 0.5 * amp * tail;
            last = n;
            amp *= 0.9992;
            s
        })
        .collect();
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

/// A steady pad: two slightly detuned waves with a slow swell, one second long.
fn pad(frames: usize) -> AudioData {
    let (mut p1, mut p2) = (0.0f32, 0.0f32);
    let out: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / frames as f32;
            let swell = 0.75 + 0.25 * (if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 });
            let left = (frames - i) as f32;
            let tail = if left < 64.0 {
                (left - 1.0) / 64.0
            } else {
                1.0
            };
            let s = (wave(p1) + wave(p2)) * 0.3 * swell * tail;
            advance(&mut p1, 130.81);
            advance(&mut p2, 131.6);
            s
        })
        .collect();
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

/// The five demo samples as `(file name, audio)`.
pub fn demo_samples() -> Vec<(&'static str, AudioData)> {
    vec![
        ("pluck_C3.wav", pluck(130.81, 24_000)),
        ("pluck_G3.wav", pluck(196.0, 19_200)),
        ("pluck_C4.wav", pluck(261.63, 14_400)),
        ("snap.wav", snap(4_800)),
        ("pad_C3.wav", pad(48_000)),
    ]
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// `(beat, length in beats, key, velocity)`.
type Step = (f64, f64, u8, f32);

fn notes(steps: &[Step]) -> Vec<Note> {
    steps
        .iter()
        .map(|&(beat, length, key, velocity)| {
            Note::new(
                (beat * PPQ as f64) as i64,
                (length * PPQ as f64) as i64,
                key,
                velocity,
            )
        })
        .collect()
}

/// The project for samples whose hashes and formats are `pool` (in [`demo_samples`] order).
pub fn demo_doc(pool: &[(String, AssetMeta)]) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).expect("the demo commands are valid");
    };
    for (hash, meta) in pool {
        apply(Command::RegisterAsset {
            hash: hash.clone(),
            meta: meta.clone(),
        });
    }
    for (k, name, color) in [
        ("keys", "Keys", [0xd9, 0x82, 0x2b]),
        ("pad", "Pad", [0x3f, 0xa7, 0xd6]),
    ] {
        apply(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.sampler".into(),
            key: Some(key(k)),
        });
        apply(Command::Rename {
            key: key(k),
            name: name.into(),
        });
        apply(Command::SetColor {
            key: key(k),
            color: Some(color),
        });
    }
    let param = |k: &str, param: &str, value: f32| Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    };
    apply(param("keys", "env.amp.release", 350.0));
    apply(param("keys", "master.level", -9.0));
    apply(param("pad", "env.amp.attack", 120.0));
    apply(param("pad", "env.amp.release", 700.0));
    apply(param("pad", "master.level", -12.0));

    // keys: the plucks by their note names, plus the layered snap on hard hits.
    let files: Vec<(String, String, u64)> = pool[..3]
        .iter()
        .zip(demo_samples())
        .map(|((hash, meta), (name, _))| (hash.clone(), name.to_string(), meta.frames))
        .collect();
    let mut keys_zones = auto_map(&files, None);
    let (snap_hash, snap_meta) = &pool[3];
    let mut layer = ZoneDoc::new(snap_hash.clone(), snap_meta.frames);
    layer.vel_lo = 96;
    layer.gain_db = -9.0;
    layer.trigger = TriggerMode::OneShot;
    keys_zones.push(layer);
    apply(Command::SetZones {
        device: key("keys"),
        zones: keys_zones,
        merge: None,
    });
    // pad: one forward loop with a crossfade, root C3.
    let (pad_hash, pad_meta) = &pool[4];
    let mut zone = ZoneDoc::new(pad_hash.clone(), pad_meta.frames);
    zone.root_key = 48;
    zone.loop_mode = LoopMode::Forward;
    (zone.loop_start, zone.loop_end, zone.crossfade) = (12_000, 40_000, 4_000);
    apply(Command::SetZones {
        device: key("pad"),
        zones: vec![zone],
        merge: None,
    });

    let bars = 4;
    let song_end = 4 * bars * PPQ;
    let clip = |content| Clip {
        id: ClipId(0),
        start: 0,
        length: song_end,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(content),
    };
    let keys_steps: Vec<Step> = vec![
        (0.0, 0.5, 48, 0.5),
        (0.5, 0.5, 55, 0.95),
        (1.0, 0.5, 60, 0.5),
        (1.5, 0.5, 64, 0.95),
        (2.0, 1.5, 48, 0.7),
        (2.0, 1.5, 55, 0.7),
        (2.0, 1.5, 60, 0.7),
        (4.0, 0.5, 50, 0.45),
        (4.5, 0.5, 53, 0.6),
        (5.0, 0.5, 57, 0.75),
        (5.5, 0.5, 62, 0.9),
        (6.0, 1.5, 65, 0.98),
        (8.0, 1.0, 48, 1.0),
        (8.0, 1.0, 55, 1.0),
        (8.0, 1.0, 64, 1.0),
        (9.0, 0.5, 52, 0.4),
        (9.5, 0.5, 59, 0.55),
        (10.0, 0.5, 64, 0.7),
        (10.5, 0.5, 67, 0.85),
        (11.0, 2.0, 72, 0.9),
        (12.0, 3.0, 36, 0.8),
        (12.0, 3.0, 43, 0.8),
        (12.0, 3.0, 48, 0.8),
    ];
    let pad_steps: Vec<Step> = vec![
        (0.0, 7.5, 48, 0.8),
        (0.0, 7.5, 52, 0.8),
        (0.0, 7.5, 55, 0.8),
        (8.0, 7.5, 53, 0.8),
        (8.0, 7.5, 57, 0.8),
        (8.0, 7.5, 60, 0.8),
    ];
    for (k, steps) in [("keys", keys_steps), ("pad", pad_steps)] {
        apply(Command::Song(SongCommand::AddTrack { device: key(k) }));
        apply(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: clip(notes(&steps).into()),
        }));
    }
    apply(Command::Song(SongCommand::SetSongEnd(song_end)));
    apply(Command::Song(SongCommand::SetLoop {
        range: (0, song_end),
        enabled: false,
    }));
    d.doc().clone()
}

/// `(hash, meta)` of every demo sample, in [`demo_samples`] order.
fn demo_pool() -> Vec<(String, AssetMeta)> {
    demo_samples()
        .into_iter()
        .map(|(name, data)| {
            (
                content_hash(&data),
                AssetMeta {
                    channels: 1,
                    sample_rate: 48_000,
                    frames: data.frames() as u64,
                    original_name: name.to_string(),
                },
            )
        })
        .collect()
}

#[test]
fn regenerate() {
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    let root = demo_root();
    let mut bundle = Bundle::create(&root).expect("bundle folder");
    for (name, data) in demo_samples() {
        bundle.add_recording(&data, name).expect("asset written");
    }
    // Only the assets belong in the repository; the peak cache is rebuilt on demand.
    let _ = std::fs::remove_dir_all(bundle.peaks_dir());
    let doc = demo_doc(&demo_pool());
    bundle
        .remove_orphans(&doc.referenced_assets())
        .expect("orphans removed");
    std::fs::write(demo_file(), save_project(&doc)).expect("project written");
}

fn registry_and_doc() -> (atomik_device_sdk::Registry, RackDoc) {
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    (registry, doc)
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        dir_bytes(&p)
                    } else {
                        e.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[test]
fn the_generators_are_deterministic_short_and_finite() {
    let a = demo_samples();
    let b = demo_samples();
    assert_eq!(a.len(), 5);
    for ((n1, d1), (n2, d2)) in a.iter().zip(&b) {
        assert_eq!(n1, n2);
        assert!(d1 == d2, "{n1} is deterministic");
        assert!(d1.channels[0].iter().all(|x| x.is_finite()), "{n1}");
        let peak = d1.channels[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.05..0.95).contains(&peak), "{n1} peak {peak}");
        assert_eq!(*d1.channels[0].last().unwrap(), 0.0, "{n1} ends silent");
        assert!(d1.frames() <= 48_000);
    }
}

#[test]
fn the_committed_bundle_is_intact_the_project_canonical_and_the_whole_small() {
    let (_, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(save_project(&doc), text.replace("\r\n", "\n"), "canonical");
    assert_eq!(
        save_project(&demo_doc(&demo_pool())),
        text.replace("\r\n", "\n"),
        "the project is the generator's output"
    );
    let root = existing_bundle_root(&demo_file()).expect("its own folder is the bundle");
    assert_eq!(root, demo_root());
    let bundle = Bundle::open(&root).expect("bundle");
    assert!(missing_assets(&doc, Some(&bundle)).is_empty());
    assert_eq!(doc.assets.len(), 5);
    for (name, data) in demo_samples() {
        let hash = content_hash(&data);
        assert!(doc.assets.contains_key(&hash), "{name} is in the pool");
        let stored = bundle.load(&hash).expect("the asset decodes");
        assert!(stored == data, "{name} is the generator's output");
    }
    assert!(
        dir_bytes(&root) <= MAX_BUNDLE_BYTES,
        "{} bytes",
        dir_bytes(&root)
    );
    // Nothing but the assets and the project: no peak cache, no stray files.
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Audio", "sampler-demo.atomik"]);
}

#[test]
fn the_demo_has_two_layered_samplers_one_looped_with_a_crossfade() {
    let (_, doc) = registry_and_doc();
    let zones = |k: &str| doc.device(&key(k)).unwrap().zones.clone();
    let keys = zones("keys");
    assert_eq!(keys.len(), 4);
    assert_eq!(
        keys.iter().map(|z| z.root_key).take(3).collect::<Vec<_>>(),
        [48, 55, 60]
    );
    assert!(keys[3].vel_lo > 90 && keys[3].trigger == TriggerMode::OneShot);
    let pad = zones("pad");
    assert_eq!(pad.len(), 1);
    assert!(pad[0].loop_mode == LoopMode::Forward && pad[0].crossfade > 0);
    assert_eq!(doc.song.tracks.len(), 2);
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(
        doc,
        &default_registry(),
        &options(block, workers),
        Some(&demo_root()),
    )
    .unwrap()
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

/// The bounce does not depend on the host block size or the worker count.
#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    assert!(reference.len() > 400_000);
    for (block, workers) in [(64, 0), (333, 1), (1024, 2), (480, 4), (4096, 3)] {
        assert!(
            bits(&bounce(&doc, block, workers)) == reference,
            "block {block}, {workers} workers"
        );
    }
}

/// Voice stealing is deterministic too: two voices for dense chords steal constantly.
#[test]
fn voice_stealing_and_layering_are_deterministic() {
    let (_, mut doc) = registry_and_doc();
    doc.devices
        .iter_mut()
        .find(|d| d.key == key("keys"))
        .unwrap()
        .params
        .insert("voices".into(), 2.0);
    let reference = bits(&bounce(&doc, 512, 0));
    for (block, workers) in [(64, 2), (777, 1)] {
        assert!(
            bits(&bounce(&doc, block, workers)) == reference,
            "block {block}, {workers} workers"
        );
    }
    let (_, full) = registry_and_doc();
    assert!(
        bits(&bounce(&full, 512, 0)) != reference,
        "stealing is audible"
    );
}

#[test]
fn both_samplers_sound_and_the_demo_is_not_clipping() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.02..1.0).contains(&peak), "peak {peak}");
    for (soloed, from, to) in [("keys", 0usize, 24_000usize), ("pad", 48_000, 96_000)] {
        let mut solo = doc.clone();
        for t in &mut solo.song_mut().tracks {
            t.solo = t.device == key(soloed);
        }
        let r = bounce(&solo, 512, 0);
        assert!(rms(&r.channels[0], from, to) > 1e-3, "{soloed} is silent");
    }
}

#[test]
fn the_layer_zone_and_the_loop_change_the_sound() {
    let (_, doc) = registry_and_doc();
    let base = bounce(&doc, 512, 0);
    // Without the snap layer the hard hits lose it.
    let mut no_layer = doc.clone();
    no_layer
        .devices
        .iter_mut()
        .find(|d| d.key == key("keys"))
        .unwrap()
        .zones
        .truncate(3);
    assert!(bits(&bounce(&no_layer, 512, 0)) != bits(&base));
    // The pad's loop holds the note: at 3 s the looped pad still rings, the unlooped sample
    // ended at one second.
    let solo_pad = |looped: bool| {
        let mut d = doc.clone();
        for t in &mut d.song_mut().tracks {
            t.solo = t.device == key("pad");
        }
        if !looped {
            d.devices
                .iter_mut()
                .find(|x| x.key == key("pad"))
                .unwrap()
                .zones[0]
                .loop_mode = LoopMode::Off;
        }
        bounce(&d, 512, 0)
    };
    let (looped, once) = (solo_pad(true), solo_pad(false));
    assert!(
        rms(&looped.channels[0], 144_000, 160_000) > 1e-3,
        "the loop holds"
    );
    assert!(
        rms(&once.channels[0], 144_000, 160_000) < 1e-6,
        "no loop: ended"
    );
}

#[test]
fn a_sampler_project_without_its_bundle_fails_naming_the_missing_audio() {
    let (registry, doc) = registry_and_doc();
    let e = render_doc_in(&doc, &registry, &ProjectOptions::default(), None).unwrap_err();
    assert!(e.to_string().contains("pluck_C3.wav"), "{e}");
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}
