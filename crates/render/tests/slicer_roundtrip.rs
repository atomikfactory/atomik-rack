//! A loop sliced by the grid, written as a note clip and bounced: the loop comes back **sample-accurately** at its own tempo, the bounce
//! does not depend on the host block or the worker count, and at another tempo the slices start
//! at the re-timed positions without being stretched.
//!
//! The loop is synthesised here (no libm: parabola waves and IEEE `+ - * /` only): 4 beats at
//! 120 BPM (96 000 frames at 48 kHz) of sixteen 1/16 steps, each a click followed by a
//! decaying tone of its own pitch, so every slice has a sharp, identifiable start.
//!
//! # Tolerance
//!
//! The slicer plays a slice at ratio 1 bit-exactly (its own tests), but the bounce is not the
//! loop's samples bit for bit: the voice gain (zone gain, the velocity law), the constant-power
//! pan (-3 dB at the centre) and the console channel and master multiply every sample by
//! constants. The test therefore fits one gain `g` over the whole bounce (least squares against
//! the loop) and requires every compared sample to match `g x loop` within **5e-8 absolute**
//! (the loop peaks at 0.55; the fit gives `g` = 0.5, two -3 dB centre pans in the chain; the
//! deviation measured is 5.5e-9, a few f32 roundings).
//!
//! The compared region of a slice starts 400 frames in (past its 0.5 ms attack ramp of 24
//! frames and, at a faster tempo, the 5 ms decay tail of the previous slice) and ends before
//! the slice's 128-frame end fade, or at the next slice's start when that comes first (the
//! next note-off begins the 5 ms decay then). The fit has to be the same constant at every
//! tempo, which proves the slice bodies are not scaled or stretched. Start positions are
//! exact: the region only matches at the frame the model's `Song::frame_at` gives for the
//! slice's tick (a one-frame shift breaks the fit by orders of magnitude, a test checks).

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, DeviceKey, Document, RackDoc, SongCommand,
    ZoneDoc, empty_doc, grid_markers, slices_from_markers, to_track_notes,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const RATE: u32 = 48_000;
const BEATS: u32 = 4;
const FRAMES: usize = 96_000;
/// 1/16 steps, 6 000 frames each.
const STEP_FRAMES: usize = 6_000;
/// Frames skipped at the start of a slice: its 24-frame attack ramp, and the tail of the
/// previous slice's voice (the 5 ms decay after its note-off is -80 dB after 320 frames; at a
/// tempo faster than the loop it overlaps the next slice).
const SETTLE_FRAMES: usize = 400;
const TOLERANCE: f32 = 5e-8;

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

/// The synthesised loop: sixteen steps, each a 16-frame click (0.3) on top of a decaying tone
/// of its own pitch (peak 0.25), so no slice is silent anywhere.
fn synth_loop() -> Vec<f32> {
    let mut out = vec![0.0f32; FRAMES];
    for step in 0..16usize {
        let hz = 110.0 + 37.0 * step as f32;
        let (mut phase, mut amp) = (0.0f32, 0.25f32);
        for i in 0..STEP_FRAMES {
            let click = if i < 16 {
                0.3 * (1.0 - i as f32 / 16.0)
            } else {
                0.0
            };
            out[step * STEP_FRAMES + i] = click + wave(phase) * amp;
            phase += hz / RATE as f32;
            phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
            amp *= 0.9995;
        }
    }
    out
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-render-slicer-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

struct Fixture {
    dir: Tmp,
    registry: Registry,
    hash: String,
    meta: AssetMeta,
    loop_data: Vec<f32>,
}

fn fixture() -> Fixture {
    let dir = Tmp::new();
    let loop_data = synth_loop();
    let mut bundle = Bundle::create(&dir.0).expect("bundle");
    let data = AudioData::new(RATE, vec![loop_data.clone()]).unwrap();
    let info = bundle.add_recording(&data, "synth-loop.wav").unwrap();
    Fixture {
        dir,
        registry: default_registry(),
        hash: info.hash,
        meta: AssetMeta {
            channels: 1,
            sample_rate: RATE,
            frames: FRAMES as u64,
            original_name: "synth-loop.wav".into(),
        },
        loop_data,
    }
}

/// The slicer project: the loop cut by the 1/16 grid, written with `to_track_notes` into one
/// clip at tick 0, played at `tempo` BPM. Gate mode, so a slice holds its level until the
/// next slice's note-off; a 5 ms decay; master 0 dB; velocity does not scale the level.
fn project(f: &Fixture, tempo: f64) -> (RackDoc, Vec<ZoneDoc>) {
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &f.registry).expect("valid command");
    };
    apply(Command::RegisterAsset {
        hash: f.hash.clone(),
        meta: f.meta.clone(),
    });
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.loop-slicer".into(),
        key: Some(key("slc")),
    });
    for (param, value) in [
        ("env.mode", 0.0),
        ("env.attack", 0.5),
        ("env.decay", 5.0),
        ("master.level", 0.0),
        ("velocity.amp", 0.0),
        ("loop.beats", BEATS as f32),
    ] {
        apply(Command::SetParam {
            key: key("slc"),
            param: param.into(),
            value,
            merge: None,
        });
    }
    let markers = grid_markers(FRAMES as u64, BEATS, 4);
    let zones = slices_from_markers(&f.hash, FRAMES as u64, &markers, &[], 48);
    assert_eq!(zones.len(), 16);
    apply(Command::SetZones {
        device: key("slc"),
        zones: zones.clone(),
        merge: None,
    });
    let (notes, length) = to_track_notes(&zones, FRAMES as u64, BEATS);
    assert_eq!(length, (BEATS as u64) * PPQ as u64);
    assert_eq!(notes.len(), 16);
    apply(Command::Song(SongCommand::AddTrack { device: key("slc") }));
    apply(Command::Song(SongCommand::AddClip {
        device: key("slc"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: length as i64,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: None,
            content: ClipContent::Notes(notes.into()),
        },
    }));
    apply(Command::SetTempo(tempo));
    // The song is exactly the clip: the bounce ends at its end plus the tail rules.
    apply(Command::Song(SongCommand::SetSongEnd(BEATS as i64 * PPQ)));
    apply(Command::Song(SongCommand::SetLoop {
        range: (0, BEATS as i64 * PPQ),
        enabled: false,
    }));
    (d.doc().clone(), zones)
}

fn bounce(f: &Fixture, doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &f.registry, &options, Some(&f.dir.0)).expect("bounce");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    r.channels.into_iter().next().unwrap()
}

/// The compared region of every slice at this tempo: `(output start frame, loop start frame,
/// length)`. Starts are the model's frames of the slice ticks.
fn regions(doc: &RackDoc, zones: &[ZoneDoc]) -> Vec<(usize, usize, usize)> {
    let clip = &doc.song.track(&key("slc")).unwrap().lanes[0].clips[0];
    let ClipContent::Notes(notes) = &clip.content else {
        panic!("notes");
    };
    let starts: Vec<usize> = notes
        .iter()
        .map(|n| doc.song.frame_at(n.tick, RATE) as usize)
        .collect();
    let end = doc.song.frame_at(clip.length, RATE) as usize;
    zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let gap = starts.get(i + 1).copied().unwrap_or(end) - starts[i];
            let slice_len = (z.end - z.start) as usize;
            // The end fade (128 frames) matters whenever the slice ends before the next note starts.
            let limit = if slice_len <= gap {
                slice_len - 128
            } else {
                gap
            };
            let from = SETTLE_FRAMES;
            (
                starts[i] + from,
                z.start as usize + from,
                limit.saturating_sub(from),
            )
        })
        .collect()
}

/// The least-squares gain of `out` against `reference` over the regions, and the largest
/// deviation from `g x reference`.
fn fit(out: &[f32], reference: &[f32], regions: &[(usize, usize, usize)]) -> (f64, f64) {
    let (mut xy, mut xx) = (0.0f64, 0.0f64);
    for &(o, r, len) in regions {
        for j in 0..len {
            let (y, x) = (f64::from(out[o + j]), f64::from(reference[r + j]));
            xy += x * y;
            xx += x * x;
        }
    }
    let g = xy / xx;
    let mut worst = 0.0f64;
    for &(o, r, len) in regions {
        for j in 0..len {
            let e = (f64::from(out[o + j]) - g * f64::from(reference[r + j])).abs();
            worst = worst.max(e);
        }
    }
    (g, worst)
}

#[test]
fn the_loop_is_synthesised_without_gaps_and_deterministically() {
    let a = synth_loop();
    assert!(a == synth_loop());
    assert!(a.iter().all(|x| x.is_finite() && x.abs() < 0.6));
    for step in 0..16 {
        let s = &a[step * STEP_FRAMES..(step + 1) * STEP_FRAMES];
        assert!(
            s[..16].iter().any(|x| x.abs() > 0.1),
            "a click starts step {step}"
        );
        assert!(
            s[5_000..].iter().any(|x| x.abs() > 0.005),
            "no gap in step {step}"
        );
    }
}

/// At the loop's original tempo the bounce reproduces every slice body sample-accurately.
#[test]
fn at_the_original_tempo_the_bounce_reproduces_the_loop_sample_accurately() {
    let f = fixture();
    let (doc, zones) = project(&f, 120.0);
    let out = bounce(&f, &doc, 512, 0);
    assert!(out.len() >= FRAMES, "{} frames", out.len());
    let regions = regions(&doc, &zones);
    // Every region has a real length, and together they cover nearly the whole loop.
    let covered: usize = regions.iter().map(|r| r.2).sum();
    assert!(covered > FRAMES * 9 / 10, "{covered} frames compared");
    let (g, worst) = fit(&out, &f.loop_data, &regions);
    assert!(
        (0.2..1.0).contains(&g),
        "gain {g}: a plausible pan and channel chain"
    );
    assert!(
        worst <= f64::from(TOLERANCE),
        "deviation {worst:e} (gain {g})"
    );
    // The click of every slice sits at its frame: the loop's own click frames.
    for (step, &(o, _, _)) in regions.iter().enumerate() {
        let click = out[step * STEP_FRAMES..step * STEP_FRAMES + 64]
            .iter()
            .position(|x| x.abs() > 0.01)
            .expect("a click at the slice start");
        assert_eq!(
            o,
            step * STEP_FRAMES + SETTLE_FRAMES,
            "grid starts are exact ticks"
        );
        assert!(
            click < 40,
            "step {step}: the click is audible by frame {click}"
        );
    }
}

/// The compared samples only match at the exact frame: shifting the reference by one frame
/// breaks the fit by orders of magnitude (so the test really is sample-accurate).
#[test]
fn the_comparison_is_sensitive_to_a_one_frame_shift() {
    let f = fixture();
    let (doc, zones) = project(&f, 120.0);
    let out = bounce(&f, &doc, 512, 0);
    let regions = regions(&doc, &zones);
    let shifted: Vec<(usize, usize, usize)> =
        regions.iter().map(|&(o, r, l)| (o + 1, r, l - 1)).collect();
    let (_, worst) = fit(&out, &f.loop_data, &shifted);
    assert!(
        worst > 100.0 * f64::from(TOLERANCE),
        "a one-frame shift gives {worst:e}"
    );
}

/// The bounce does not depend on the host block size or the worker count.
#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let f = fixture();
    let (doc, _) = project(&f, 120.0);
    let reference: Vec<u32> = bounce(&f, &doc, 512, 0)
        .iter()
        .map(|x| x.to_bits())
        .collect();
    for (block, workers) in [(64, 0), (333, 1), (1024, 2), (480, 4), (4096, 3)] {
        let other: Vec<u32> = bounce(&f, &doc, block, workers)
            .iter()
            .map(|x| x.to_bits())
            .collect();
        assert!(other == reference, "block {block}, {workers} workers");
    }
}

/// At another tempo the clip re-times the slices; the slices themselves are not
/// stretched (same samples, same gain), only their starts move.
#[test]
fn at_another_tempo_the_slices_start_at_the_retimed_frames_without_stretching() {
    let f = fixture();
    let (doc_120, zones) = project(&f, 120.0);
    let regions_120 = regions(&doc_120, &zones);
    let (g_120, _) = fit(&bounce(&f, &doc_120, 512, 0), &f.loop_data, &regions_120);
    for tempo in [90.0, 150.0, 97.0] {
        let (doc, _) = project(&f, tempo);
        let out = bounce(&f, &doc, 512, 0);
        let regions = regions(&doc, &zones);
        // The step starts moved by the tempo ratio (steps are 1/16 notes).
        let moved = regions[5].0 as f64 / regions_120[5].0 as f64;
        let expected = 120.0 / tempo;
        assert!(
            (moved - expected).abs() < 0.01,
            "{tempo} BPM: starts scaled by {moved}"
        );
        let covered: usize = regions.iter().map(|r| r.2).sum();
        assert!(
            covered > FRAMES / 2,
            "{tempo} BPM: {covered} frames compared"
        );
        let (g, worst) = fit(&out, &f.loop_data, &regions);
        assert!(
            (g - g_120).abs() < 1e-7,
            "{tempo} BPM: gain {g} differs from {g_120}: the slice was scaled"
        );
        assert!(
            worst <= f64::from(TOLERANCE),
            "{tempo} BPM: deviation {worst:e}"
        );
        // A tempo slower than the loop leaves silence after each slice ended (no stretching,
        // no looping): the last frames of the gap before the next slice are exactly empty.
        if tempo < 120.0 {
            let clip = &doc.song.track(&key("slc")).unwrap().lanes[0].clips[0];
            let ClipContent::Notes(notes) = &clip.content else {
                panic!("notes");
            };
            let gap_start = doc.song.frame_at(notes[0].tick, RATE) as usize + STEP_FRAMES + 256;
            let gap_end = doc.song.frame_at(notes[1].tick, RATE) as usize;
            assert!(gap_end > gap_start + 100, "{tempo} BPM leaves a gap");
            assert!(
                out[gap_start..gap_end].iter().all(|x| x.abs() < 1e-7),
                "{tempo} BPM: the slice must end, not stretch or ring"
            );
        }
    }
}
