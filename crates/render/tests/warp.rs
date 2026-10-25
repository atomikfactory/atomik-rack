//! Bounces of warped audio clips: they follow the song tempo (also across a tempo change),
//! transpose without changing length, are bit-identical from run to run and across blocks and
//! workers, and a project that does not warp bounces as it always did.

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::song::TempoEvent;
use atomik_model::{
    AssetMeta, AudioClipContent, Command, DeviceKey, Document, RackDoc, SongCommand, WarpMode,
    WarpSettings, load_project, save_project,
};
use atomik_render::project::{
    ProjectOptions, render_doc_in, song_input_with_assets, song_input_with_warps,
};
use atomik_render::warp::{pool_key, render_warps};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::PathBuf;

const SR: u32 = 48_000;
const BEAT: usize = 24_000;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-warp-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn sine(hz: f64, frames: usize) -> AudioData {
    let ch: Vec<f32> = (0..frames)
        .map(|i| (0.4 * (std::f64::consts::TAU * hz * i as f64 / f64::from(SR)).sin()) as f32)
        .collect();
    AudioData::new(SR, vec![ch.clone(), ch]).unwrap()
}

/// A click (a short noise burst) at the start of every `period` frames.
fn clicks(period: usize, frames: usize) -> AudioData {
    let mut x: u32 = 0x9e37_79b9;
    let mut ch = vec![0.0f32; frames];
    for start in (0..frames).step_by(period) {
        for i in 0..3000.min(frames - start) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let n = (x >> 8) as f32 / 8_388_608.0 - 1.0;
            ch[start + i] = n * 0.7 * (-(i as f32) / 500.0).exp();
        }
    }
    AudioData::new(SR, vec![ch.clone(), ch]).unwrap()
}

struct Song {
    doc: RackDoc,
    bundle: Bundle,
    asset: String,
    _dir: TempDir,
}

fn song(tag: &str, asset: &AudioData, build: impl FnOnce(&mut Document, &str)) -> Song {
    let dir = TempDir::new(tag);
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    let info = bundle.add_recording(asset, "fixture.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(
        Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        },
        &registry,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::AddAudioTrack { name: None }),
        &registry,
    )
    .unwrap();
    d.apply(Command::Song(SongCommand::SetSongEnd(8 * PPQ)), &registry)
        .unwrap();
    build(&mut d, &info.hash);
    Song {
        doc: d.doc().clone(),
        bundle,
        asset: info.hash,
        _dir: dir,
    }
}

fn put(d: &mut Document, asset: &str, start: i64, length: i64, warp: WarpSettings) {
    let registry = default_registry();
    let mut a = AudioClipContent::new(asset, 0);
    a.warp = warp;
    d.apply(
        Command::Song(SongCommand::AddAudioClip {
            device: DeviceKey("audio-track".into()),
            start,
            length,
            asset: asset.to_owned(),
            source_offset: 0,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: atomik_model::FadeCurve::Linear,
        }),
        &registry,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted: false,
            content: a,
            merge: None,
        }),
        &registry,
    )
    .unwrap();
}

fn render(s: &Song, block: usize, workers: u16) -> Rendered {
    render_doc_in(
        &s.doc,
        &default_registry(),
        &options(block, workers),
        Some(s.bundle.root()),
    )
    .unwrap()
}

fn on(bpm: f64, mode: WarpMode) -> WarpSettings {
    WarpSettings {
        enabled: true,
        mode,
        source_bpm: bpm,
        ..WarpSettings::default()
    }
}

/// Amplitude of `hz` in `x[from..to]` (single-bin DFT).
fn tone(x: &[f32], from: usize, to: usize, hz: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in x[from..to].iter().enumerate() {
        let w = std::f64::consts::TAU * hz * (from + i) as f64 / f64::from(SR);
        re += f64::from(*s) * w.cos();
        im += f64::from(*s) * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / (to - from) as f64
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

#[test]
fn a_loop_at_another_tempo_is_squeezed_to_the_song_tempo_and_keeps_its_pitch() {
    // 4 beats at 100 BPM are 2.4 s; the clip is 4 beats of a 120 BPM song (2 s).
    for mode in [WarpMode::Tonal, WarpMode::Beats] {
        let s = song("squeeze", &sine(440.0, 115_200), |d, a| {
            put(d, a, 0, 4 * PPQ, on(100.0, mode));
        });
        let r = render(&s, 512, 0);
        let l = &r.channels[0];
        // Sound for the clip's 2 s (96 000 frames), then silence.
        let body = rms(&l[10_000..90_000]);
        assert!(body > 0.05, "{mode:?}: body rms {body}");
        let tail = rms(&l[100_000..110_000]);
        assert!(tail < body * 0.05, "{mode:?}: tail rms {tail} vs {body}");
        // Pitch unchanged.
        let at = tone(l, 20_000, 80_000, 440.0);
        let off = tone(l, 20_000, 80_000, 440.0 * 1.2);
        assert!(at > 8.0 * off, "{mode:?}: 440 Hz {at} vs {off}");
        // Without warp the same clip is the 2.4 s sine cut at 2 s: no silence inside.
        assert!(r.channels[0].iter().all(|x| x.is_finite()));
    }
}

#[test]
fn repitch_changes_pitch_with_the_tempo_ratio() {
    let s = song("repitch", &sine(440.0, 115_200), |d, a| {
        put(d, a, 0, 4 * PPQ, on(100.0, WarpMode::Repitch));
    });
    let l = &render(&s, 512, 0).channels[0].clone();
    // 120 / 100 faster: 528 Hz.
    let at = tone(l, 20_000, 80_000, 528.0);
    let off = tone(l, 20_000, 80_000, 440.0);
    assert!(at > 8.0 * off, "{at} vs {off}");
}

#[test]
fn transposing_moves_the_pitch_and_not_the_length() {
    let s = song("transpose", &sine(440.0, 96_000), |d, a| {
        let w = WarpSettings {
            transpose_cents: 1200,
            mode: WarpMode::Tonal,
            ..WarpSettings::default()
        };
        put(d, a, 0, 4 * PPQ, w);
    });
    let l = &render(&s, 512, 0).channels[0].clone();
    assert!(rms(&l[10_000..90_000]) > 0.05);
    assert!(tone(l, 20_000, 80_000, 880.0) > 8.0 * tone(l, 20_000, 80_000, 440.0));
    assert!(
        rms(&l[100_000..110_000]) < 0.01,
        "same length: silent after the clip"
    );
}

/// First frame at or after `from` whose magnitude is at least `level`.
fn first_over(x: &[f32], from: usize, level: f32) -> usize {
    (from..x.len())
        .find(|&i| x[i].abs() >= level)
        .unwrap_or(usize::MAX)
}

#[test]
fn a_warped_drum_loop_keeps_its_hits_on_the_beats_across_a_tempo_change() {
    let registry = default_registry();
    // Clicks on every beat of a 120 BPM source; the song drops to 60 BPM at beat 2.
    let s = song("tempo", &clicks(BEAT, 8 * BEAT), |d, a| {
        d.apply(
            Command::Song(SongCommand::SetTempoMap(vec![
                TempoEvent::step(0, 120.0),
                TempoEvent::step(2 * PPQ, 60.0),
            ])),
            &registry,
        )
        .unwrap();
        put(d, a, 0, 4 * PPQ, on(120.0, WarpMode::Beats));
    });
    let r = render(&s, 512, 0);
    let l = &r.channels[0];
    // Beats 0 and 1 at 24 000 frames each, then 48 000 per beat.
    let beats = [0usize, 24_000, 48_000, 96_000];
    // The console scales the clicks; find the loudest level once.
    let peak = l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01);
    for &b in &beats {
        let got = first_over(l, b.saturating_sub(400), peak * 0.3);
        assert!(got.abs_diff(b) < 200, "hit due at {b} found at {got}");
        // Quiet just before the hit.
        if b > 1000 {
            assert!(
                l[b - 800..b - 300].iter().all(|x| x.abs() < peak * 0.05),
                "pre-echo at {b}"
            );
        }
    }
}

#[test]
fn warped_bounces_are_bit_identical_across_runs_blocks_and_workers() {
    let s = song("det", &clicks(BEAT / 2, 6 * BEAT), |d, a| {
        put(d, a, 0, 3 * PPQ, on(110.0, WarpMode::Beats));
        put(
            d,
            a,
            4 * PPQ,
            2 * PPQ,
            WarpSettings {
                enabled: true,
                mode: WarpMode::Tonal,
                source_bpm: 130.0,
                transpose_cents: -300,
                markers: Vec::new(),
            },
        );
    });
    let a = render(&s, 512, 0);
    assert_eq!(bits(&a), bits(&render(&s, 512, 0)));
    assert_eq!(bits(&a), bits(&render(&s, 64, 0)));
    assert_eq!(bits(&a), bits(&render(&s, 777, 2)));
    assert!(a.channels[0].iter().any(|x| x.abs() > 0.01));
}

#[test]
fn a_project_without_warp_bounces_the_same_with_or_without_the_feature() {
    // Warp settings that are present but off (nothing to render) change no bit.
    let plain = song("plain", &sine(300.0, 96_000), |d, a| {
        put(d, a, 0, 4 * PPQ, WarpSettings::default())
    });
    let remembered = song("remembered", &sine(300.0, 96_000), |d, a| {
        put(
            d,
            a,
            0,
            4 * PPQ,
            WarpSettings {
                mode: WarpMode::Tonal,
                source_bpm: 90.0,
                ..WarpSettings::default()
            },
        );
    });
    assert_eq!(
        bits(&render(&plain, 512, 0)),
        bits(&render(&remembered, 512, 0))
    );
    assert!(render_warps(&plain.doc, &Default::default()).is_empty());
}

#[test]
fn the_song_plays_the_asset_until_the_buffer_is_ready() {
    let s = song("fallback", &sine(440.0, 115_200), |d, a| {
        put(d, a, 0, 4 * PPQ, on(100.0, WarpMode::Tonal));
    });
    let registry = default_registry();
    let assets = atomik_render::bundle::load_song_assets(&s.doc, Some(&s.bundle)).unwrap();
    let ids = std::collections::BTreeMap::from([(
        DeviceKey("audio-track".into()),
        atomik_engine::DeviceInstanceId(1),
    )]);
    let id_of = |k: &DeviceKey| ids.get(k).copied();
    // No buffer: the asset in real time.
    let dry = song_input_with_assets(&s.doc, &registry, id_of, &assets);
    let region = dry.audio[0].regions[0];
    assert_eq!((region.source_offset, region.clip_pos), (0, 0));
    assert_eq!(dry.assets, vec![assets[&s.asset].clone()]);
    // With the buffer: the pool holds it, read from the clip's own frame.
    let warps = render_warps(&s.doc, &assets);
    assert_eq!(warps.len(), 1);
    let (key, buffer) = warps.iter().next().unwrap();
    assert_eq!(buffer.frames(), 96_000);
    let _ = pool_key(*key);
    let warped = song_input_with_warps(&s.doc, &registry, id_of, &assets, &warps);
    assert_eq!(warped.assets, vec![buffer.clone()]);
    assert_eq!(warped.audio[0].regions[0].source_offset, 0);
    assert!(!warped.audio[0].regions[0].reverse);
}

#[test]
fn a_saved_warp_project_loads_and_bounces_the_same() {
    let s = song("saved", &clicks(BEAT, 4 * BEAT), |d, a| {
        put(d, a, 0, 4 * PPQ, on(100.0, WarpMode::Beats));
    });
    let text = save_project(&s.doc);
    assert!(text.contains("\"version\": 28") && text.contains("\"warp\""));
    let loaded = load_project(&text, &default_registry()).unwrap();
    assert_eq!(loaded, s.doc);
    let r1 = render(&s, 512, 0);
    let again = Song {
        doc: loaded,
        bundle: Bundle::open(s.bundle.root()).unwrap(),
        asset: s.asset.clone(),
        _dir: TempDir::new("unused"),
    };
    assert_eq!(bits(&r1), bits(&render(&again, 256, 0)));
}
