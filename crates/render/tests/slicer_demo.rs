//! The `slicer-demo` example project and its bundle, and the bounce of
//! loop-slicer projects.
//!
//! `examples/projects/slicer-demo/` is a bundle folder: `slicer-demo.atomik` and `Audio/` with
//! one **procedurally generated** 2-bar drum loop ([`demo_loop`]: kick, snare and closed hat
//! synthesised from parabola waves and noise; 8 beats at 140 BPM; no third-party audio). One
//! loop slicer plays it:
//!
//! * the loop is **sliced by transients** (`atomik_dsp::onset::detect_onsets` at 50 %, then
//!   `slices_from_markers`, exactly what a drop in the app does);
//! * `loop.beats` is 8 (the loop's own length), and **"To track"** wrote the slices into a note
//!   clip with `to_track_notes`, placed twice (two passes) on the slicer's track;
//! * the song runs at **100 BPM**, not the loop's 140: the slices start at the re-timed
//!   positions and are not stretched or pitched, with gaps between them (gate mode, a 150 ms
//!   decay after each note);
//! * one slice is **reversed**, one is quieter and one panned.
//!
//! Regenerate the bundle and the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test slicer_demo regenerate
//! ```
//!
//! The generator uses only IEEE `+ - * /` and comparisons (no libm calls), so it gives the same
//! samples on every platform; the committed WAV is checked against it bit for bit. The
//! transient detector is deterministic on every platform (its documentation), so the markers
//! in the committed project are checked against a fresh detection too.

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, DeviceKey, Document, RackDoc, SongCommand,
    ZoneDoc, empty_doc, load_project, markers_of, save_project, slices_from_markers,
    to_track_notes,
};
use atomik_render::bundle::{existing_bundle_root, missing_assets};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const RATE: u32 = 48_000;
/// 8 beats at 140 BPM: `48 000 x 60 x 8 / 140` rounded down.
const FRAMES: usize = 164_571;
const LOOP_BEATS: u32 = 8;
const SONG_BPM: f64 = 100.0;
const SENSITIVITY: f32 = 0.5;
/// The whole bundle must stay small (the brief's budget).
const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/slicer-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("slicer-demo.atomik")
}

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

/// A xorshift noise source in `[-1, 1)`.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// The last 200 frames fade to exactly zero.
fn tail(i: usize, frames: usize) -> f32 {
    let left = (frames - i) as f32;
    if left < 200.0 {
        (left - 1.0) / 200.0
    } else {
        1.0
    }
}

fn kick(frames: usize) -> Vec<f32> {
    let (mut phase, mut sweep, mut amp) = (0.0f32, 1.0f32, 0.9f32);
    (0..frames)
        .map(|i| {
            let hz = 48.0 + 120.0 * sweep;
            let s = wave(phase) * amp * tail(i, frames);
            phase += hz / RATE as f32;
            phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
            sweep *= 0.9992;
            amp *= 0.99988;
            s
        })
        .collect()
}

fn snare(frames: usize) -> Vec<f32> {
    let (mut noise, mut phase) = (Noise(0x2545_f491), 0.0f32);
    let (mut amp_n, mut amp_t) = (0.5f32, 0.45f32);
    (0..frames)
        .map(|i| {
            let s = (noise.next() * amp_n + wave(phase) * amp_t) * tail(i, frames);
            phase += 190.0 / RATE as f32;
            phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
            amp_n *= 0.9993;
            amp_t *= 0.9985;
            s
        })
        .collect()
}

fn hat(frames: usize, seed: u32) -> Vec<f32> {
    let (mut noise, mut last, mut amp) = (Noise(seed), 0.0f32, 0.5f32);
    (0..frames)
        .map(|i| {
            let n = noise.next();
            let s = (n - last) * 0.5 * amp * tail(i, frames);
            last = n;
            amp *= 0.998;
            s
        })
        .collect()
}

/// The demo loop: 32 steps of 1/16 over 8 beats, kicks on steps 0, 6, 10, 16, 22 and 26,
/// snares on the backbeat (4, 12, 20 and 28) and a closed hat on every other step.
pub fn demo_loop() -> AudioData {
    let mut out = vec![0.0f32; FRAMES];
    let mut add = |step: usize, sound: &[f32], gain: f32| {
        let start = step * FRAMES / 32;
        for (i, x) in sound.iter().enumerate() {
            if let Some(o) = out.get_mut(start + i) {
                *o += x * gain;
            }
        }
    };
    let (k, s) = (kick(11_000), snare(9_000));
    for step in [0, 6, 10, 16, 22, 26] {
        add(step, &k, 1.0);
    }
    for step in [4, 12, 20, 28] {
        add(step, &s, 0.9);
    }
    for (n, step) in (0..32).step_by(2).enumerate() {
        add(step, &hat(2_400, 0x9e37_79b9 ^ n as u32), 0.45);
    }
    for x in &mut out {
        *x = x.clamp(-0.9, 0.9);
    }
    AudioData::new(RATE, vec![out]).expect("valid mono data")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// The transient markers of the loop, as a drop in the app finds them.
fn demo_markers(data: &AudioData) -> Vec<u64> {
    let channels: Vec<&[f32]> = data.channels.iter().map(Vec::as_slice).collect();
    atomik_dsp::onset::detect_onsets(&channels, RATE, SENSITIVITY)
}

/// The project for the loop asset `hash` with `meta`, sliced at `markers`.
pub fn demo_doc(hash: &str, meta: &AssetMeta, markers: &[u64]) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).expect("the demo commands are valid");
    };
    apply(Command::RegisterAsset {
        hash: hash.to_owned(),
        meta: meta.clone(),
    });
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.loop-slicer".into(),
        key: Some(key("slicer")),
    });
    apply(Command::Rename {
        key: key("slicer"),
        name: "Loop Slicer".into(),
    });
    apply(Command::SetColor {
        key: key("slicer"),
        color: Some([0xc2, 0x4d, 0x8f]),
    });
    for (param, value) in [
        ("env.mode", 0.0), // gate: a slice holds while its note lasts
        ("env.attack", 1.0),
        ("env.decay", 150.0),
        ("master.level", -6.0),
        ("loop.beats", LOOP_BEATS as f32),
    ] {
        apply(Command::SetParam {
            key: key("slicer"),
            param: param.into(),
            value,
            merge: None,
        });
    }
    // What a drop does: the slices of the markers (1 ms minimum). Then three per-slice edits.
    let mut zones: Vec<ZoneDoc> =
        slices_from_markers(hash, meta.frames, markers, &[], u64::from(RATE / 1000));
    let n = zones.len();
    zones[n.min(2) - 1].gain_db = -4.0;
    zones[n.min(4) - 1].pan = 0.4;
    zones[n.min(6) - 1].reverse = true;
    apply(Command::SetZones {
        device: key("slicer"),
        zones: zones.clone(),
        merge: None,
    });
    // "To track": the notes of the slices, played twice.
    let (notes, length) = to_track_notes(&zones, meta.frames, LOOP_BEATS);
    apply(Command::Song(SongCommand::AddTrack {
        device: key("slicer"),
    }));
    for pass in 0..2i64 {
        apply(Command::Song(SongCommand::AddClip {
            device: key("slicer"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: pass * length as i64,
                length: length as i64,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(notes.clone()),
            },
        }));
    }
    apply(Command::SetTempo(SONG_BPM));
    let song_end = 2 * length as i64;
    apply(Command::Song(SongCommand::SetSongEnd(song_end)));
    apply(Command::Song(SongCommand::SetLoop {
        range: (0, song_end),
        enabled: false,
    }));
    d.doc().clone()
}

fn demo_meta(data: &AudioData) -> AssetMeta {
    AssetMeta {
        channels: 1,
        sample_rate: RATE,
        frames: data.frames() as u64,
        original_name: "drum-loop-140.wav".into(),
    }
}

fn generated_doc() -> (AudioData, RackDoc) {
    let data = demo_loop();
    let doc = demo_doc(
        &content_hash(&data),
        &demo_meta(&data),
        &demo_markers(&data),
    );
    (data, doc)
}

#[test]
fn regenerate() {
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    let root = demo_root();
    let mut bundle = Bundle::create(&root).expect("bundle folder");
    let data = demo_loop();
    bundle
        .add_recording(&data, "drum-loop-140.wav")
        .expect("asset written");
    // Only the asset belongs in the repository; the peak cache is rebuilt on demand.
    let _ = std::fs::remove_dir_all(bundle.peaks_dir());
    let (_, doc) = generated_doc();
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
fn the_generator_is_deterministic_finite_and_not_clipping() {
    let a = demo_loop();
    assert!(a == demo_loop());
    assert_eq!(a.frames(), FRAMES);
    assert!(a.channels[0].iter().all(|x| x.is_finite()));
    let peak = a.channels[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.3..=0.9).contains(&peak), "peak {peak}");
    // The detector is deterministic too, and finds the hits (kicks, snares and hats).
    let markers = demo_markers(&a);
    assert_eq!(markers, demo_markers(&a));
    assert!(markers.len() >= 8, "{} markers", markers.len());
}

#[test]
fn the_committed_bundle_is_intact_the_project_canonical_and_the_whole_small() {
    let (_, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(save_project(&doc), text.replace("\r\n", "\n"), "canonical");
    let (data, generated) = generated_doc();
    assert_eq!(
        save_project(&generated),
        text.replace("\r\n", "\n"),
        "the project is the generator's output"
    );
    let root = existing_bundle_root(&demo_file()).expect("its own folder is the bundle");
    assert_eq!(root, demo_root());
    let bundle = Bundle::open(&root).expect("bundle");
    assert!(missing_assets(&doc, Some(&bundle)).is_empty());
    assert_eq!(doc.assets.len(), 1);
    let hash = content_hash(&data);
    assert!(
        bundle.load(&hash).expect("the asset decodes") == data,
        "the loop is the generator's output"
    );
    assert!(
        dir_bytes(&root) <= MAX_BUNDLE_BYTES,
        "{} bytes",
        dir_bytes(&root)
    );
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Audio", "slicer-demo.atomik"]);
}

#[test]
fn the_demo_is_a_transient_sliced_loop_played_at_another_tempo_with_one_reversed_slice() {
    let (_, doc) = registry_and_doc();
    let slicer = doc.device(&key("slicer")).unwrap();
    // Sliced where the detector finds the transients.
    let (data, _) = generated_doc();
    let expected = slices_from_markers(
        &content_hash(&data),
        FRAMES as u64,
        &demo_markers(&data),
        &[],
        u64::from(RATE / 1000),
    );
    assert_eq!(markers_of(&slicer.zones), markers_of(&expected));
    assert!(slicer.zones.len() >= 8);
    assert_eq!(slicer.zones.iter().filter(|z| z.reverse).count(), 1);
    assert_eq!(slicer.params.get("loop.beats"), Some(&(LOOP_BEATS as f32)));
    // The song tempo is not the loop's: 8 beats at 140 BPM is 164 571 frames, at 100 BPM
    // 8 beats would be 230 400.
    assert_eq!(doc.tempo_bpm, SONG_BPM);
    // Two clips with the notes "To track" writes.
    let clips = &doc.song.track(&key("slicer")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 2);
    let (notes, length) = to_track_notes(&slicer.zones, FRAMES as u64, LOOP_BEATS);
    for (i, clip) in clips.iter().enumerate() {
        assert_eq!(clip.start, i as i64 * length as i64);
        assert_eq!(clip.length, LOOP_BEATS as i64 * PPQ);
        assert_eq!(clip.content, ClipContent::Notes(notes.clone()));
    }
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

#[test]
fn the_demo_sounds_does_not_clip_and_the_reversed_slice_is_audible() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.05..1.0).contains(&peak), "peak {peak}");
    // Without the reverse flag the bounce is different.
    let mut forward = doc.clone();
    for z in &mut forward
        .devices
        .iter_mut()
        .find(|d| d.key == key("slicer"))
        .unwrap()
        .zones
    {
        z.reverse = false;
    }
    assert!(bits(&bounce(&forward, 512, 0)) != bits(&all));
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}
