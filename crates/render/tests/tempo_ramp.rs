//! Bounces of projects with tempo ramps and meter changes: the clips of an audio track start
//! and end at the frames of the closed form (the log formula written out here, independent of
//! `atomik_core`), the bounce is the same at any host block, and it is the same sound the live
//! engine plays for the same project (the engine song of `song_input`, rendered by hand).

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Command, DeviceKey, Document, FadeCurve, MeterEvent, RackDoc, SongCommand,
    TempoEvent, empty_doc, load_project, save_project,
};
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::PathBuf;

const BAR: i64 = 4 * PPQ;
const SR: f64 = 48_000.0;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-render-ramp-{tag}-{}", std::process::id()));
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

/// A tempo map as `(tick, bpm, ramp)`.
type Map = [(i64, f64, bool)];

const RAMPS: [(i64, f64, bool); 4] = [
    (0, 60.0, true),
    (2 * BAR, 180.0, false),
    (3 * BAR, 100.0, true),
    (4 * BAR, 40.0, false),
];

/// Seconds at `tick`: a segment holds its BPM or ramps linearly to the next one,
/// `60 / PPQ * L / (b1 - b0) * ln(b(x) / b0)` over `L` ticks.
fn seconds(map: &Map, tick: i64) -> f64 {
    let k = 60.0 / PPQ as f64;
    let mut total = 0.0;
    for (i, &(t0, b0, ramp)) in map.iter().enumerate() {
        let end = map.get(i + 1).map_or(i64::MAX, |n| n.0);
        let upto = tick.min(end);
        if upto <= t0 {
            break;
        }
        let x = (upto - t0) as f64;
        total += match map.get(i + 1) {
            Some(&(_, b1, _)) if ramp && (b1 - b0).abs() > 0.0 => {
                let len = (end - t0) as f64;
                k * len / (b1 - b0) * ((b0 + (b1 - b0) * x / len) / b0).ln()
            }
            _ => k * x / b0,
        };
    }
    total
}

fn frame(map: &Map, tick: i64) -> usize {
    (seconds(map, tick) * SR).round() as usize
}

/// A project with a constant 0.25 asset (a long one, so no clip runs out) as audio clips at
/// `starts`, each `length` ticks long, under the tempo map `map` and the meter map `meter`.
fn project(
    dir: &std::path::Path,
    starts: &[i64],
    length: i64,
    map: &Map,
    meter: &[MeterEvent],
) -> (RackDoc, Bundle) {
    let asset = AudioData::new(48_000, vec![vec![0.25; 600_000]; 2]).unwrap();
    let mut bundle = Bundle::create(dir).unwrap();
    let info = bundle.add_recording(&asset, "fixture.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).unwrap();
    };
    apply(Command::RegisterAsset {
        hash: info.hash.clone(),
        meta: AssetMeta {
            channels: info.channels,
            sample_rate: info.sample_rate,
            frames: info.frames,
            original_name: info.original_name.clone(),
        },
    });
    apply(Command::Song(SongCommand::AddAudioTrack { name: None }));
    apply(Command::Song(SongCommand::SetTempoMap(
        map.iter()
            .map(|&(tick, bpm, ramp)| TempoEvent { tick, bpm, ramp })
            .collect(),
    )));
    apply(Command::Song(SongCommand::SetMeterMap(meter.to_vec())));
    apply(Command::Song(SongCommand::SetSongEnd(
        starts.last().copied().unwrap_or(0) + 2 * length,
    )));
    for &start in starts {
        apply(Command::Song(SongCommand::AddAudioClip {
            device: DeviceKey("audio-track".into()),
            start,
            length,
            asset: info.hash.clone(),
            source_offset: 0,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: FadeCurve::Linear,
        }));
    }
    (d.doc().clone(), bundle)
}

fn render(doc: &RackDoc, bundle: &Bundle, block: usize) -> Rendered {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    };
    render_doc_in(doc, &default_registry(), &options, Some(bundle.root())).unwrap()
}

/// The `[start, end)` frame runs where the channel is not silent.
fn runs(x: &[f32]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (n, s) in x.iter().enumerate() {
        if s.abs() > 0.0 {
            match out.last_mut() {
                Some(r) if r.1 == n => r.1 = n + 1,
                _ => out.push((n, n + 1)),
            }
        }
    }
    out
}

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn four_four() -> Vec<MeterEvent> {
    vec![MeterEvent::new(1, 4, 4)]
}

#[test]
fn clips_start_and_end_at_the_closed_form_frames_across_ramps() {
    let t = TempDir::new("frames");
    let starts = [PPQ + 123, 5 * BAR / 2, 3 * BAR + 17, 4 * BAR + PPQ];
    let length = PPQ / 2;
    let (doc, bundle) = project(&t.0.join("song"), &starts, length, &RAMPS, &four_four());
    let r = render(&doc, &bundle, 64);
    let found = runs(&r.channels[0]);
    assert_eq!(found.len(), starts.len(), "{found:?}");
    for (&start, &(a, b)) in starts.iter().zip(&found) {
        assert_eq!(a, frame(&RAMPS, start), "start of the clip at {start}");
        assert_eq!(
            b,
            frame(&RAMPS, start + length),
            "end of the clip at {start}"
        );
    }
}

#[test]
fn a_ramped_bounce_is_the_same_at_any_host_block() {
    let t = TempDir::new("blocks");
    let starts = [PPQ, 2 * BAR + 5, 3 * BAR + PPQ / 3];
    let (doc, bundle) = project(&t.0.join("song"), &starts, PPQ, &RAMPS, &four_four());
    let reference = bits(&render(&doc, &bundle, 64));
    assert!(reference.iter().any(|b| f32::from_bits(*b) != 0.0));
    for block in [1, 333, 4096] {
        assert!(
            bits(&render(&doc, &bundle, block)) == reference,
            "block {block}"
        );
    }
}

#[test]
fn the_bounce_length_follows_the_ramps() {
    // The bounce is the song end plus two beats, in frames of the tempo map.
    let t = TempDir::new("length");
    let (doc, bundle) = project(&t.0.join("song"), &[BAR], PPQ, &RAMPS, &four_four());
    let r = render(&doc, &bundle, 512);
    let end = doc.song.end;
    let want = frame(&RAMPS, end + 2 * PPQ);
    assert_eq!(r.channels[0].len(), want);
}

#[test]
fn a_project_with_ramps_and_meter_changes_saves_loads_and_bounces_the_same() {
    let t = TempDir::new("file");
    let meter = [
        MeterEvent::new(1, 4, 4),
        MeterEvent::new(3, 7, 8),
        MeterEvent::new(6, 3, 4),
    ];
    let (doc, bundle) = project(&t.0.join("song"), &[PPQ, 3 * BAR], PPQ, &RAMPS, &meter);
    let loaded = load_project(&save_project(&doc), &default_registry()).unwrap();
    assert_eq!(loaded, doc);
    assert!(bits(&render(&loaded, &bundle, 512)) == bits(&render(&doc, &bundle, 512)));
}

#[test]
fn the_live_engine_plays_the_frames_of_the_bounce() {
    use atomik_core::{SampleRate, Tempo};
    use atomik_engine::{EngineConfig, TransportCommand, compile_song, new_engine};
    let t = TempDir::new("live");
    let starts = [PPQ + 123, 2 * BAR + 1, 3 * BAR + 17];
    let (doc, bundle) = project(&t.0.join("song"), &starts, PPQ / 2, &RAMPS, &four_four());
    let bounced = render(&doc, &bundle, 64);
    // The engine song the app builds from this document: its snapshot answers the same frames
    // as the bounce's plan and as the model.
    let input = atomik_render::project::song_input(&doc, &default_registry(), |_| None);
    let snapshot = compile_song(&input).unwrap();
    for &start in &starts {
        let from_engine = snapshot.song_frame(atomik_core::Ticks(start), SR) as usize;
        assert_eq!(from_engine, frame(&RAMPS, start));
        assert_eq!(from_engine as u64, doc.song.frame_at(start, 48_000));
        assert!(
            runs(&bounced.channels[0])
                .iter()
                .any(|r| r.0 == from_engine)
        );
    }
    // And a live engine playing the song reports the same tempo and tick the closed form says.
    let config = EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let (mut engine, mut handle) = new_engine(config);
    handle.submit_song(snapshot).unwrap();
    handle.transport(TransportCommand::Play).unwrap();
    let mut out = [vec![0.0f32; 64], vec![0.0f32; 64]];
    let frames = frame(&RAMPS, 3 * BAR) / 64 * 64;
    for _ in 0..frames / 64 {
        let mut refs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
        engine.render(&[], &mut refs);
    }
    // The state is that of the last batch: its song frame is the played frames minus one batch
    // and its tick (rounded) is where the closed form puts that frame.
    let state = handle.transport_state();
    assert_eq!(state.song_frame as usize, frames - 64);
    let at = seconds(&RAMPS, state.tick) * SR;
    assert!(
        (at - state.song_frame as f64).abs() < 3.2,
        "{at} vs {}",
        state.song_frame
    );
}
