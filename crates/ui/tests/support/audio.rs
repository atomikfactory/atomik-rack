//! Audio fixtures for the arrangement: a synth track, two audio tracks (the first with two
//! clips and the interface inputs cabled), a stereo and a mono asset with synthetic peaks.

use super::song::*;
use super::*;
use atomik_audio_file::{AudioData, PeakPyramid};
use atomik_model::{AssetMeta, AudioClipContent, Clip, ClipContent, ClipId, FadeCurve, Lane};
use atomik_ui::{ArrangementView, Theme};
use std::sync::Arc;

pub const RATE: u32 = 48_000;
/// 4 s at 120 BPM is two bars.
pub const FRAMES_PER_BAR: u64 = 96_000;
pub const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// Stereo asset: 8 s.
pub const FRAMES_A: u64 = 8 * RATE as u64;
/// Mono asset: 3 s.
pub const FRAMES_B: u64 = 3 * RATE as u64;

/// A deterministic pseudo-waveform: a decaying burst train, different per channel.
pub fn wave(frames: usize, channel: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let beat = (i % 24_000) as f32 / 24_000.0;
            let env = (1.0 - beat).powi(2);
            let phase = (i as f32) * (0.05 + 0.02 * channel as f32);
            let tone = phase.sin() * 0.7 + (phase * 2.7).sin() * 0.3;
            (tone * env * if channel == 0 { 0.9 } else { 0.6 }).clamp(-1.0, 1.0)
        })
        .collect()
}

pub fn pyramid(frames: u64, channels: usize) -> Arc<PeakPyramid> {
    let data = AudioData::new(
        RATE,
        (0..channels).map(|c| wave(frames as usize, c)).collect(),
    )
    .expect("audio data");
    Arc::new(PeakPyramid::build(&data).expect("pyramid"))
}

pub fn audio_clip(
    id: u64,
    start: i64,
    length: i64,
    asset: &str,
    source_offset: u64,
    gain_db: f32,
    (fade_in, fade_out): (u64, u64),
) -> Clip {
    Clip {
        id: ClipId(id),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Audio(AudioClipContent {
            asset: asset.to_string(),
            source_offset,
            gain_db,
            fade_in,
            fade_out,
            fade_curve: FadeCurve::Linear,
            fade_out_curve: None,
            reverse: false,
            envelope: Vec::new(),
            xfade: None,
            warp: Default::default(),
            pitch_edits: Vec::new(),
        }),
    }
}

/// Bass synth track (notes clip 1), audio track `audio-track` ("Guitar", inputs 1+2 cabled)
/// with clips 10 (stereo asset, offset 1 s, -3 dB, fades) and 11 (mono asset, offset 0.5 s)
/// and an empty audio track `audio-track-2` ("Vox"). Four host inputs.
pub fn audio_doc() -> RackDoc {
    let mut doc = reference_doc();
    doc.input_channels = 4;
    doc.devices = vec![
        device("bass", "atomik.poly-synth", "Bass"),
        device("audio-track", "atomik.audio-track", "Guitar"),
        device("audio-track-2", "atomik.audio-track", "Vox"),
    ];
    doc.cables = vec![
        cable(("interface", "in_1"), ("audio-track", "in_l")),
        cable(("interface", "in_2"), ("audio-track", "in_r")),
    ];
    doc.cables.sort();
    doc.assets.insert(
        HASH_A.into(),
        AssetMeta {
            channels: 2,
            sample_rate: RATE,
            frames: FRAMES_A,
            original_name: "drums.wav".into(),
        },
    );
    doc.assets.insert(
        HASH_B.into(),
        AssetMeta {
            channels: 1,
            sample_rate: RATE,
            frames: FRAMES_B,
            original_name: "vox.flac".into(),
        },
    );
    let audio = |clips| vec![Lane::audio().with_clips(clips)];
    doc.song_mut().tracks = vec![
        track(
            "bass",
            vec![Lane::notes().with_clips(vec![notes_clip(1, 0, 2 * BAR, bass_line())])],
        ),
        track(
            "audio-track",
            audio(vec![
                audio_clip(
                    10,
                    BAR,
                    2 * BAR,
                    HASH_A,
                    RATE as u64,
                    -3.0,
                    (24_000, 48_000),
                ),
                audio_clip(11, 4 * BAR, 2 * BAR, HASH_B, 24_000, 0.0, (0, 0)),
            ]),
        ),
        track("audio-track-2", audio(Vec::new())),
    ];
    doc.song_mut().next_clip_id = 12;
    doc
}

/// A synced arrangement of [`audio_doc`] with peaks for the stereo asset only.
pub fn audio_arrangement(doc: &RackDoc, reg: &Registry) -> ArrangementView {
    let mut v = ArrangementView::new(Theme::dark());
    v.set_bounds(ARR_BOUNDS);
    v.sync(doc, reg);
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v
}

/// A note of clip 10 for Vocal mode: asset frames `start..end` at the fractional key `pitch`,
/// with a 5 ms curve that leans `lean` semitones from start to end.
pub fn vocal_note(
    start: u64,
    end: u64,
    pitch: f32,
    lean: f32,
) -> atomik_dsp::stretch::psola::VocalNote {
    use atomik_dsp::stretch::psola::{NotePitch, VocalNote};
    let step = 240;
    let n = ((end - start) / step) as usize;
    let keys: Vec<Option<f32>> = (0..n)
        .map(|k| Some(pitch - lean / 2.0 + lean * k as f32 / n.max(1) as f32))
        .collect();
    VocalNote {
        start,
        end,
        key: pitch.round() as u8,
        level: 0.5,
        pitch: NotePitch {
            step,
            first: start,
            keys,
            centre: Some(pitch),
        },
    }
}

/// Three notes of clip 10 (it plays asset frames 48 000 to 240 000): a flat C3, a sharp E3 and a
/// D3 that leans up.
pub fn vocal_notes() -> Vec<atomik_dsp::stretch::psola::VocalNote> {
    vec![
        vocal_note(60_000, 100_000, 59.8, 0.0),
        vocal_note(110_000, 150_000, 64.3, 0.0),
        vocal_note(160_000, 220_000, 62.1, 0.4),
    ]
}
