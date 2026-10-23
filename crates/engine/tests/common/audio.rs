//! Helpers for audio-clip tests: assets, regions, and a one-track bounce.

use super::hot::Rig;
use super::song::tempo;
use super::{cfg, engine_with};
use atomik_devices::audio_track;
use atomik_engine::{
    AssetId, DeviceInstanceId, HotEdit, OfflineJob, SongAsset, SongAudioRegion, SongAudioTrack,
    SongFadeCurve, SongInput, TimedEdit, TransportCommand, compile_song, render_offline_song,
};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::sync::Arc;

/// Deterministic, all-distinct test samples: `value(c, i)`.
pub fn value(channel: usize, i: usize) -> f32 {
    let sign = if channel == 0 { 1.0 } else { -1.0 };
    sign * ((i % 20_011) as f32 + 1.0) * 4.0e-5
}

/// A planar asset of `frames` frames with `channels` channels of [`value`].
pub fn planes(channels: usize, frames: usize) -> Arc<[Vec<f32>]> {
    (0..channels)
        .map(|c| (0..frames).map(|i| value(c, i)).collect())
        .collect::<Vec<_>>()
        .into()
}

/// A memory asset of [`value`] samples.
pub fn asset(channels: usize, frames: usize) -> SongAsset {
    SongAsset::memory(planes(channels, frames))
}

/// A plain region of asset 0: no gain change, no fades, a clip equal to the region.
pub fn region(start: u64, offset: u64, frames: u64) -> SongAudioRegion {
    SongAudioRegion {
        asset: AssetId(0),
        song_start_frame: start,
        source_offset: offset,
        frames,
        gain: 1.0,
        fade_in: 0,
        fade_out: 0,
        fade_curve: SongFadeCurve::Linear,
        clip_pos: 0,
        clip_frames: frames,
    }
}

/// Song input in 4/4 at 120 BPM (1 tick = 1.5625 frames at 48 kHz) with audio tracks.
pub fn audio_input(audio: Vec<SongAudioTrack>, assets: Vec<SongAsset>) -> SongInput {
    SongInput {
        tracks: Vec::new(),
        tempo: tempo(&[(0, 120.0)]),
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio,
        assets,
    }
}

/// The tick of a song frame at 120 BPM / 48 kHz (frame must be a multiple of 25).
pub fn tick_of(frame: i64) -> i64 {
    assert_eq!(frame % 25, 0);
    frame * 16 / 25
}

/// What to bounce: the song, optional loop (in song frames), extra scripted edits.
pub struct Bounce {
    pub input: SongInput,
    pub frames: usize,
    pub block: usize,
    pub threads: u16,
    pub loop_frames: Option<(i64, i64)>,
    pub edits: Vec<TimedEdit>,
}

impl Bounce {
    pub fn new(input: SongInput, frames: usize) -> Self {
        Self {
            input,
            frames,
            block: 64,
            threads: 0,
            loop_frames: None,
            edits: Vec::new(),
        }
    }

    /// Renders with audio-track devices `1..=n` (n = number of audio tracks in the input), each
    /// wired to interface outs `(2k-1, 2k)`, and returns the interface outputs.
    pub fn run(self) -> Vec<Vec<f32>> {
        let n = self.input.audio.len();
        let c = cfg(0, (2 * n).max(2) as u16);
        let mut rig = Rig::new(&c);
        for k in 0..n {
            let id = rig.insert(0, k as u32 + 1, audio_track::factory(), &[]);
            rig.wire_outputs(
                id,
                &format!("out_{}", 2 * k + 1),
                &format!("out_{}", 2 * k + 2),
            );
        }
        rig.route(0);
        let mut edits = rig.edits;
        edits.extend(self.edits);
        let (mut engine, mut handle) = engine_with(c, self.threads, None);
        if let Some((a, b)) = self.loop_frames {
            handle
                .transport(TransportCommand::SetLoop {
                    start: atomik_core::Ticks(tick_of(a)),
                    end: atomik_core::Ticks(tick_of(b)),
                    enabled: true,
                })
                .unwrap();
        }
        let job = OfflineJob {
            frames: self.frames,
            host_block: self.block,
            notes: &[],
            params: &[],
            inputs: &[],
        };
        let song = compile_song(&self.input).unwrap();
        let out = render_offline_song(&mut engine, &mut handle, &job, song, edits, |f| {
            assert_no_alloc(f)
        });
        drop(engine);
        out
    }
}

/// One audio track on device `id`.
pub fn audio_track_of(id: u32, regions: Vec<SongAudioRegion>) -> SongAudioTrack {
    SongAudioTrack {
        device: DeviceInstanceId(id),
        regions,
    }
}

/// A scripted edit.
pub fn edit(frame: u64, e: HotEdit) -> TimedEdit {
    TimedEdit { frame, edit: e }
}
