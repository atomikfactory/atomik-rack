//! QA gap fill for / HI-2: a bounce of a project with several audio tracks
//! is bit-identical across host blocks and worker counts.
//!
//! `audio_project.rs` bounces one audio track, and the console chain behind a single track is
//! linear, so no dependency level holds more than one device and the workers never receive any
//! work. Three audio tracks put three track players on one level, which the pool distributes
//! over its workers; a worker-dependent result would show up here.

use atomik_audio_file::{AssetInfo, AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Command, DeviceKey, Document, FadeCurve, RackDoc, SongCommand, TempoEvent, empty_doc,
};
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-qa-workers-{tag}-{}",
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

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn render(doc: &RackDoc, bundle: &Bundle, block: usize, workers: u16) -> Rendered {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc_in(doc, &default_registry(), &options, Some(bundle.root())).unwrap()
}

/// `(track, asset, start tick, length ticks, source offset, gain dB, fade in, fade out, curve)`.
type ClipRow<'a> = (
    &'a str,
    &'a AssetInfo,
    i64,
    i64,
    u64,
    f32,
    u64,
    u64,
    FadeCurve,
);

/// Three audio tracks over two assets (stereo and mono), mixed fades of both curves, offsets, a
/// tempo change and overlapping clips on one track.
fn project(dir: &std::path::Path) -> (RackDoc, Bundle) {
    let mut bundle = Bundle::create(dir).unwrap();
    let stereo = AudioData::new(
        48_000,
        vec![
            (0..60_000)
                .map(|i| (i as f32 * 0.031).sin() * 0.5)
                .collect(),
            (0..60_000)
                .map(|i| (i as f32 * 0.047).cos() * 0.4)
                .collect(),
        ],
    )
    .unwrap();
    let mono = AudioData::new(
        48_000,
        vec![
            (0..40_000)
                .map(|i| (i as f32 * 0.013).sin() * 0.6)
                .collect(),
        ],
    )
    .unwrap();
    let a = bundle.add_recording(&stereo, "stereo.wav").unwrap();
    let b = bundle.add_recording(&mono, "mono.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).unwrap();
    };
    for info in [&a, &b] {
        apply(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        });
    }
    for _ in 0..3 {
        apply(Command::Song(SongCommand::AddAudioTrack { name: None }));
    }
    apply(Command::Song(SongCommand::SetSongEnd(6 * PPQ)));
    apply(Command::Song(SongCommand::SetTempoMap(vec![
        TempoEvent {
            tick: 0,
            bpm: 120.0,
        },
        TempoEvent {
            tick: 3 * PPQ,
            bpm: 90.0,
        },
    ])));
    let clips: [ClipRow; 5] = [
        (
            "audio-track",
            &a,
            0,
            2 * PPQ,
            0,
            -3.0,
            2_400,
            4_800,
            FadeCurve::EqualPower,
        ),
        (
            "audio-track",
            &a,
            PPQ,
            2 * PPQ,
            1_000,
            0.0,
            500,
            500,
            FadeCurve::Linear,
        ),
        (
            "audio-track-2",
            &b,
            PPQ / 2,
            3 * PPQ,
            300,
            -6.0,
            4_000,
            6_000,
            FadeCurve::EqualPower,
        ),
        (
            "audio-track-3",
            &a,
            2 * PPQ,
            2 * PPQ,
            9_000,
            1.5,
            0,
            0,
            FadeCurve::Linear,
        ),
        (
            "audio-track-3",
            &b,
            4 * PPQ,
            PPQ,
            0,
            0.0,
            1_200,
            0,
            FadeCurve::Linear,
        ),
    ];
    for (device, info, start, length, source_offset, gain_db, fade_in, fade_out, fade_curve) in
        clips
    {
        apply(Command::Song(SongCommand::AddAudioClip {
            device: DeviceKey(device.into()),
            start,
            length,
            asset: info.hash.clone(),
            source_offset,
            gain_db,
            fade_in,
            fade_out,
            fade_curve,
        }));
    }
    (d.doc().clone(), bundle)
}

#[test]
fn a_bounce_of_several_audio_tracks_is_identical_across_blocks_and_workers() {
    let t = TempDir::new("blocks");
    let (doc, bundle) = project(&t.0.join("song"));
    let reference = render(&doc, &bundle, 64, 0);
    let ref_bits = bits(&reference);
    assert!(
        reference
            .channels
            .iter()
            .all(|c| c.iter().any(|s| s.abs() > 1e-3)),
        "the bounce is audible on both channels"
    );
    for block in [1, 64, 333, 4096] {
        for workers in [0, 3, 4] {
            assert!(
                bits(&render(&doc, &bundle, block, workers)) == ref_bits,
                "block {block}, workers {workers}"
            );
        }
    }
}
