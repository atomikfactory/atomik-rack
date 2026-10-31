//! The stress rack: many devices and cables, all valid.

pub use atomik_model::stress::{
    STRESS_CABLES, STRESS_DEVICES, STRESS_SEND_DB, STRESS_SONG_BARS, STRESS_SONG_RETURNS,
    STRESS_SONG_TRACKS, stress_doc, stress_song_doc,
};

use crate::app_core::AppCore;
use crate::sink::EngineSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_device_sdk::LoopMode;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    Note, SocketRef, SongCommand,
};
use std::path::{Path, PathBuf};

/// Audio tracks of the stress project (next to the 8 instrument tracks of the song).
pub const STRESS_AUDIO_TRACKS: usize = 16;
/// Streaming threshold the stress run uses (bytes of decoded f32): 8 MiB, so the longer
/// generated files stream from disk and the shorter stay in RAM.
pub const STRESS_AUDIO_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;

/// Frames of the generated stress file `i` (0-based) at `rate`: 12 s plus 2 s per file, so the
/// 16 files run from 12 s to 42 s and, with [`STRESS_AUDIO_THRESHOLD_BYTES`], files 5..16 stream.
pub fn stress_audio_frames(i: usize, rate: u32) -> u64 {
    u64::from(rate) * (12 + 2 * i as u64)
}

/// Writes the stress audio files `dir/stress-01.wav` ... (stereo, 32-bit float, a tone of a
/// different pitch per file with a slow tremolo, so playback is audible and unique per track),
/// reusing files that already exist with the right length. `frames(i)` gives each file's length.
///
/// # Errors
/// The first file that cannot be written.
pub fn write_stress_audio_with(
    dir: &Path,
    rate: u32,
    frames: &dyn Fn(usize) -> u64,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths = Vec::new();
    for i in 0..STRESS_AUDIO_TRACKS {
        let path = dir.join(format!("stress-{:02}.wav", i + 1));
        let n = frames(i);
        let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // 44-byte header plus 8 bytes per stereo frame (a float WAV header may be longer:
        // a wrong guess only regenerates the file).
        if have < 8 * n {
            let hz = 110.0 * 2f64.powf(i as f64 / 12.0);
            let tone = |phase: f64| -> Vec<f32> {
                (0..n)
                    .map(|k| {
                        let t = k as f64 / f64::from(rate);
                        let trem = 0.6 + 0.4 * (std::f64::consts::TAU * 0.5 * t + phase).sin();
                        (0.1 * trem * (std::f64::consts::TAU * hz * t).sin()) as f32
                    })
                    .collect()
            };
            let data = AudioData::new(rate, vec![tone(0.0), tone(1.0)])
                .map_err(|e| format!("{}: {e}", path.display()))?;
            write_wav_f32(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        paths.push(path);
    }
    Ok(paths)
}

/// [`write_stress_audio_with`] with the lengths ([`stress_audio_frames`]).
///
/// # Errors
/// The first file that cannot be written.
pub fn write_stress_audio(dir: &Path, rate: u32) -> Result<Vec<PathBuf>, String> {
    write_stress_audio_with(dir, rate, &|i| stress_audio_frames(i, rate))
}

/// Where the generated files live (reused between runs).
pub fn stress_audio_dir() -> PathBuf {
    std::env::temp_dir().join("atomik-stress-audio")
}

/// Samplers of the stress project (next to the 8 instrument tracks of the song).
pub const STRESS_SAMPLERS: usize = 4;
/// Notes of each sampler's chord: with the two layered zones every note starts two voices, so
/// one chord fills the sampler's 32 voices.
pub const STRESS_CHORD_NOTES: usize = 16;

/// Frames of each generated stress sample (2 s).
fn stress_sample_frames(rate: u32) -> usize {
    2 * rate as usize
}

/// Writes the two stress samples into `dir` (stereo, 32-bit float, 2 s of a harmonic tone with a
/// slow tremolo; both are named for C4, so auto-mapping layers them over the whole keyboard),
/// reusing files that already exist with the right size.
///
/// # Errors
/// The first file that cannot be written.
pub fn write_stress_samples(dir: &Path, rate: u32) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let n = stress_sample_frames(rate);
    let mut paths = Vec::new();
    for (name, hz) in [
        ("stress-sample-a_C4.wav", 261.63),
        ("stress-sample-b_C4.wav", 392.0),
    ] {
        let path = dir.join(name);
        let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if have < 8 * n as u64 {
            let tone = |phase: f64| -> Vec<f32> {
                (0..n)
                    .map(|k| {
                        let t = k as f64 / f64::from(rate);
                        let tau = std::f64::consts::TAU;
                        let trem = 0.7 + 0.3 * (tau * 0.5 * t + phase).sin();
                        let body = (tau * hz * t).sin() + 0.4 * (tau * 2.0 * hz * t).sin();
                        (0.06 * trem * body) as f32
                    })
                    .collect()
            };
            let data = AudioData::new(rate, vec![tone(0.0), tone(1.0)])
                .map_err(|e| format!("{}: {e}", path.display()))?;
            write_wav_f32(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        paths.push(path);
    }
    Ok(paths)
}

/// Where the generated samples live (reused between runs).
pub fn stress_sampler_dir() -> PathBuf {
    std::env::temp_dir().join("atomik-stress-sampler")
}

fn apply<S: EngineSink>(core: &mut AppCore<S>, key: &DeviceKey, c: Command) -> Result<(), String> {
    core.apply_command(c).map_err(|e| format!("{key}: {e}"))
}

/// Adds the load to `core`: [`STRESS_SAMPLERS`] samplers (`smp1`…`smp4`, each with its
/// console channel and a track), the generated samples dropped onto each through the normal
/// import path, both zones looped forward over the whole keyboard (so a held note never ends),
/// the filter on and a 1.5 s release, and a 64-bar clip of a [`STRESS_CHORD_NOTES`]-note chord a
/// bar, each note held for the bar. Two layered zones make 32 voices a chord, and the long
/// release of the previous chord keeps all 32 voices of every sampler busy and stealing:
/// 4 × 32 = 128 voices.
///
/// # Errors
/// A message when a sample cannot be written, imported or mapped.
pub fn add_stress_samplers<S: EngineSink>(core: &mut AppCore<S>, dir: &Path) -> Result<(), String> {
    let paths = write_stress_samples(dir, core.doc().sample_rate)?;
    let bar = 4 * PPQ;
    for i in 0..STRESS_SAMPLERS {
        let key = DeviceKey(format!("smp{}", i + 1));
        apply(
            core,
            &key,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.sampler".into(),
                key: Some(key.clone()),
            },
        )?;
        apply(
            core,
            &key,
            Command::Rename {
                key: key.clone(),
                name: format!("Stress Sampler {}", i + 1),
            },
        )?;
        apply(
            core,
            &key,
            Command::SetParam {
                key: key.clone(),
                param: "filter.enabled".into(),
                value: 1.0,
                merge: None,
            },
        )?;
        apply(
            core,
            &key,
            Command::SetParam {
                key: key.clone(),
                param: "env.amp.release".into(),
                value: 1500.0,
                merge: None,
            },
        )?;
        apply(
            core,
            &key,
            Command::Song(SongCommand::AddTrack {
                device: key.clone(),
            }),
        )?;
        core.import_samples(paths.clone(), key.clone(), None);
        core.wait_background();
        let mut zones = core
            .doc()
            .device(&key)
            .map(|d| d.zones.clone())
            .unwrap_or_default();
        if zones.len() != paths.len() {
            let why = core.last_rejection().unwrap_or("no zones were added");
            return Err(format!("{key}: {why}"));
        }
        for z in &mut zones {
            z.loop_mode = LoopMode::Forward;
            (z.loop_start, z.loop_end) = (z.end / 4, z.end - z.end / 4);
            z.crossfade = 480;
        }
        core.apply_command(Command::SetZones {
            device: key.clone(),
            zones,
            merge: None,
        })
        .map_err(|e| format!("{key}: {e}"))?;
        // The chord: a stack of fourths and thirds from a root that differs per sampler.
        let root = 36 + 2 * i as u8;
        let chord = (0..STRESS_CHORD_NOTES).map(|n| {
            Note::new(
                0,
                bar,
                root + 3 * n as u8 / 2 + n as u8,
                0.6 + 0.02 * (n % 8) as f32,
            )
        });
        let length = STRESS_SONG_BARS * bar;
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: Some(bar),
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(chord.collect()),
            },
        }))
        .map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

/// Drum machines of the stress project (`--stress-drums`, next to the 8 instrument tracks).
pub const STRESS_DRUM_MACHINES: usize = 4;
/// Steps of a stress drum pattern: 32 x 1/32 is one bar, looped for the whole song.
pub const STRESS_DRUM_STEPS: u16 = 32;

/// Writes the 16 generated drum sounds into `dir` (mono, 32-bit float, 0.2 to 0.6 s, a decaying
/// tone plus a little noise, named `stress-drum-01.wav` to `stress-drum-16.wav` so a background
/// drop fills the pads in order), reusing files that already exist with the right size.
///
/// # Errors
/// The first file that cannot be written.
pub fn write_stress_drums(dir: &Path, rate: u32) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths = Vec::new();
    for pad in 1..=16usize {
        let path = dir.join(format!("stress-drum-{pad:02}.wav"));
        let n = (f64::from(rate) * (0.175 + 0.025 * pad as f64)) as usize;
        let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if have < 4 * n as u64 {
            let hz = 55.0 + 38.0 * pad as f64;
            let rate_hz = f64::from(rate);
            let mut noise = 0x9e37_79b9u32.wrapping_mul(pad as u32);
            let data: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f64 / rate_hz;
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    let hiss = f64::from(noise >> 8) / 8_388_608.0 - 1.0;
                    let env = (-t * (5.0 + pad as f64 * 0.6)).exp();
                    let fade = ((n - k) as f64 / 64.0).min(1.0);
                    let tone = (std::f64::consts::TAU * hz * (1.0 + env) * t).sin();
                    (0.35 * env * fade * (0.8 * tone + 0.2 * hiss)) as f32
                })
                .collect();
            let audio =
                AudioData::new(rate, vec![data]).map_err(|e| format!("{}: {e}", path.display()))?;
            write_wav_f32(&path, &audio).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        paths.push(path);
    }
    Ok(paths)
}

/// Where the generated drum sounds live (reused between runs).
pub fn stress_drums_dir() -> PathBuf {
    std::env::temp_dir().join("atomik-stress-drums")
}

/// The dense stress pattern of one bar: pad `pad` (1..=16) hits every 1/32 step of its density
/// (pads 1, 4, 7, ... every step, pads 2, 5, ... every second step, pads 3, 6, ... every third),
/// with velocities that vary from step to step, one step long.
pub fn stress_drum_notes(pad: u8) -> Vec<Note> {
    let every = 1 + (u16::from(pad) - 1) % 3;
    let step = PPQ / 8;
    (0..STRESS_DRUM_STEPS)
        .filter(|s| s % every == 0)
        .map(|s| {
            Note::new(
                i64::from(s) * step,
                step,
                atomik_model::pad_key(pad),
                0.5 + 0.1 * f32::from((s + u16::from(pad)) % 5),
            )
        })
        .collect()
}

/// Adds the `--stress-drums` load to `core`: [`STRESS_DRUM_MACHINES`] drum machines (`drm1`...
/// `drm4`, each with its console channel and a track), the 16 generated sounds dropped onto each
/// through the normal import path (a background drop fills all 16 pads), pads 7 and 8 in one
/// choke group, and a 64-bar clip that loops one bar of the dense 1/32 pattern of
/// [`stress_drum_notes`] on every pad with a 25 % swing.
///
/// # Errors
/// A message when a sound cannot be written, imported or mapped.
pub fn add_stress_drums<S: EngineSink>(core: &mut AppCore<S>, dir: &Path) -> Result<(), String> {
    let paths = write_stress_drums(dir, core.doc().sample_rate)?;
    let bar = 4 * PPQ;
    for i in 0..STRESS_DRUM_MACHINES {
        let key = DeviceKey(format!("drm{}", i + 1));
        apply(
            core,
            &key,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.drum-machine".into(),
                key: Some(key.clone()),
            },
        )?;
        apply(
            core,
            &key,
            Command::Rename {
                key: key.clone(),
                name: format!("Stress Drums {}", i + 1),
            },
        )?;
        for pad in [7, 8] {
            apply(
                core,
                &key,
                Command::SetParam {
                    key: key.clone(),
                    param: format!("pad{pad}.choke"),
                    value: 1.0,
                    merge: None,
                },
            )?;
        }
        apply(
            core,
            &key,
            Command::Song(SongCommand::AddTrack {
                device: key.clone(),
            }),
        )?;
        core.import_samples(paths.clone(), key.clone(), None);
        core.wait_background();
        let loaded = core.doc().device(&key).map_or(0, |d| d.zones.len());
        if loaded != paths.len() {
            let why = core.last_rejection().unwrap_or("not every pad was loaded");
            return Err(format!("{key}: {why}"));
        }
        let mut notes: Vec<Note> = (1..=16u8).flat_map(stress_drum_notes).collect();
        notes.sort_by_key(|n| (n.tick, n.key));
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: STRESS_SONG_BARS * bar,
                offset: 0,
                loop_length: Some(bar),
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: Some(atomik_model::StepGrid {
                    resolution: atomik_model::StepResolution::ThirtySecond,
                    steps: STRESS_DRUM_STEPS,
                    swing: 25,
                }),
                content: ClipContent::Notes(notes.into()),
            },
        }))
        .map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

/// Loop slicers of the stress project (`--stress-slicer`, next to the 8 instrument tracks).
pub const STRESS_SLICERS: usize = 4;

/// Writes the generated 2-bar drum loop into `dir` as `stress-loop.wav` (mono, 32-bit float,
/// 8 beats at 120 BPM: a kick on every second beat, a snare on the others and a closed hat on
/// every 1/8 note; synthesised, deterministic) and returns its path, reusing a file that
/// already has the right size.
///
/// # Errors
/// The file cannot be written.
pub fn write_stress_loop(dir: &Path, rate: u32) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("stress-loop.wav");
    let beat = (u64::from(rate) / 2) as usize; // 120 BPM
    let n = beat * 8;
    let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if have >= 4 * n as u64 {
        return Ok(path);
    }
    let rate_hz = f64::from(rate);
    let mut noise = 0x2545_f491u32;
    let mut data = vec![0.0f32; n];
    for (k, out) in data.iter_mut().enumerate() {
        let pos = k % beat; // frames into the beat
        let b = k / beat; // beat 0..8
        let t = pos as f64 / rate_hz;
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let hiss = f64::from(noise >> 8) / 8_388_608.0 - 1.0;
        let mut x = 0.0;
        if b.is_multiple_of(2) {
            // Kick: a falling tone.
            let hz = 55.0 * (1.0 + 2.0 * (-t * 30.0).exp());
            x += 0.8 * (-t * 14.0).exp() * (std::f64::consts::TAU * hz * t).sin();
        } else {
            // Snare: tone and noise.
            x += 0.6
                * (-t * 18.0).exp()
                * (0.6 * hiss + 0.4 * (std::f64::consts::TAU * 190.0 * t).sin());
        }
        // Hats on every 1/8 note (the beat and its off-beat).
        for half in [0usize, beat / 2] {
            if pos >= half {
                let th = (pos - half) as f64 / rate_hz;
                x += 0.25 * (-th * 90.0).exp() * hiss;
            }
        }
        *out = x.clamp(-1.0, 1.0) as f32;
    }
    let audio = AudioData::new(rate, vec![data]).map_err(|e| format!("{}: {e}", path.display()))?;
    write_wav_f32(&path, &audio).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Where the generated loop lives (reused between runs).
pub fn stress_slicer_dir() -> PathBuf {
    std::env::temp_dir().join("atomik-stress-slicer")
}

/// Adds the `--stress-slicer` load to `core`: [`STRESS_SLICERS`] loop slicers (`slc1`...`slc4`,
/// each with its console channel and a track), the generated 2-bar loop dropped onto each
/// through the normal import path (transient detection on the worker, beats and slices as for
/// a real drop), and a 64-bar clip that loops one bar of a dense pattern: slice *i* (0-based,
/// key order) retriggers every 1 + *i* mod 3 steps of 1/32, one step long, with varying
/// velocities (far more retriggers a second than the 16 voices of a slicer).
///
/// # Errors
/// A message when the loop cannot be written, imported or sliced.
pub fn add_stress_slicers<S: EngineSink>(core: &mut AppCore<S>, dir: &Path) -> Result<(), String> {
    let path = write_stress_loop(dir, core.doc().sample_rate)?;
    let bar = 4 * PPQ;
    let step = PPQ / 8;
    for i in 0..STRESS_SLICERS {
        let key = DeviceKey(format!("slc{}", i + 1));
        apply(
            core,
            &key,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.loop-slicer".into(),
                key: Some(key.clone()),
            },
        )?;
        apply(
            core,
            &key,
            Command::Rename {
                key: key.clone(),
                name: format!("Stress Slicer {}", i + 1),
            },
        )?;
        apply(
            core,
            &key,
            Command::Song(SongCommand::AddTrack {
                device: key.clone(),
            }),
        )?;
        core.import_samples(vec![path.clone()], key.clone(), None);
        core.wait_background();
        let mut slices: Vec<u8> = core
            .doc()
            .device(&key)
            .map(|d| d.zones.iter().map(|z| z.key_lo).collect())
            .unwrap_or_default();
        slices.sort_unstable();
        if slices.len() < 2 {
            let why = core.last_rejection().unwrap_or("the loop was not sliced");
            return Err(format!("{key}: {why}"));
        }
        let mut notes: Vec<Note> = Vec::new();
        for (n, slice_key) in slices.iter().enumerate() {
            let every = 1 + n % 3;
            for s in (0..STRESS_DRUM_STEPS).filter(|s| usize::from(*s) % every == 0) {
                notes.push(Note::new(
                    i64::from(s) * step,
                    step,
                    *slice_key,
                    0.5 + 0.1 * ((usize::from(s) + n) % 5) as f32,
                ));
            }
        }
        notes.sort_by_key(|n| (n.tick, n.key));
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: STRESS_SONG_BARS * bar,
                offset: 0,
                loop_length: Some(bar),
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes.into()),
            },
        }))
        .map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

/// Effect inserts of the `--stress-fx` project, on the first channels in console order.
pub const STRESS_FX_INSERTS: usize = 8;
/// Reverbs of the `--stress-fx` project, on FX returns `STRESS_SONG_RETURNS + 1` and the next.
pub const STRESS_FX_REVERBS: usize = 2;

/// Adds the load (`--stress-fx`) to `core`: [`STRESS_FX_INSERTS`] effect inserts of mixed
/// types (`fxi1`...`fxi8`: compressor, EQ, distortion with 4x oversampling, modulation, then the
/// same four again) on the first eight channels' insert loops, [`STRESS_FX_REVERBS`] reverbs
/// (`fxr1`, `fxr2`, a hall and a plate) on FX returns 5 and 6 with the channels sending to them
/// (even channels to 5, odd to 6, at the stress send level), and the mastering suite
/// (`fx-mastering`) on the master insert. Everything goes through ordinary model commands (the
/// insert loop is cabled, the returns use `SetReturn`), so it combines with the other stress
/// loads; it needs the console (the default registry).
///
/// # Errors
/// A message when the document has no master or a command is rejected.
pub fn add_stress_fx<S: EngineSink>(core: &mut AppCore<S>) -> Result<(), String> {
    const INSERTS: [&str; 4] = [
        "atomik.compressor",
        "atomik.eq",
        "atomik.distortion",
        "atomik.modulation",
    ];
    let master = core
        .doc()
        .console
        .master
        .clone()
        .ok_or("fx stress: the document has no master")?;
    let channels: Vec<DeviceKey> = core
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .take(STRESS_FX_INSERTS)
        .collect();
    let add = |core: &mut AppCore<S>, type_id: &str, key: &DeviceKey| {
        apply(
            core,
            key,
            Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.to_string(),
                key: Some(key.clone()),
            },
        )
    };
    let wire = |core: &mut AppCore<S>, host: &DeviceKey, fx: &DeviceKey| -> Result<(), String> {
        for side in ["l", "r"] {
            for (from, to) in [
                (
                    (host, format!("insert_send_{side}")),
                    (fx, format!("in_{side}")),
                ),
                (
                    (fx, format!("out_{side}")),
                    (host, format!("insert_return_{side}")),
                ),
            ] {
                let sock = |(d, s): (&DeviceKey, String)| SocketRef {
                    device: d.clone(),
                    socket: s,
                };
                apply(
                    core,
                    fx,
                    Command::Connect(Cable {
                        from: sock(from),
                        to: sock(to),
                    }),
                )?;
            }
        }
        Ok(())
    };
    let set = |core: &mut AppCore<S>, key: &DeviceKey, param: &str, value: f32| {
        apply(
            core,
            key,
            Command::SetParam {
                key: key.clone(),
                param: param.to_string(),
                value,
                merge: None,
            },
        )
    };
    for (i, channel) in channels.iter().enumerate() {
        let type_id = INSERTS[i % INSERTS.len()];
        let fx = DeviceKey(format!("fxi{}", i + 1));
        add(core, type_id, &fx)?;
        wire(core, channel, &fx)?;
        match type_id {
            // Lookahead on: the dynamic-latency path (PDC) is part of the load.
            "atomik.compressor" => set(core, &fx, "lookahead", 5.0)?,
            "atomik.distortion" => set(core, &fx, "drive", 24.0)?,
            _ => {}
        }
    }
    for n in 0..STRESS_FX_REVERBS {
        let fx = DeviceKey(format!("fxr{}", n + 1));
        add(core, "atomik.reverb", &fx)?;
        set(core, &fx, "mix", 1.0)?;
        set(core, &fx, "algorithm", if n == 0 { 1.0 } else { 2.0 })?;
        set(core, &fx, "decay", 3.0)?;
        apply(
            core,
            &fx,
            Command::Console(ConsoleCommand::SetReturn {
                slot: STRESS_SONG_RETURNS + n,
                effect: Some(fx.clone()),
            }),
        )?;
    }
    for (i, channel) in channels.iter().enumerate() {
        let k = STRESS_SONG_RETURNS + 1 + i % STRESS_FX_REVERBS;
        set(core, channel, &format!("send{k}.on"), 1.0)?;
        set(core, channel, &format!("send{k}.level"), STRESS_SEND_DB)?;
    }
    let mastering = DeviceKey("fx-mastering".to_string());
    add(core, "atomik.mastering", &mastering)?;
    wire(core, &master, &mastering)
}

/// Player chains of the `--stress-players` project: each is a step sequencer into an
/// arpeggiator into a poly synth.
pub const STRESS_PLAYER_CHAINS: usize = 4;

/// The dense pattern of chain `n` (0-based): 64 steps of 1/32 all on, a minor pentatonic walk
/// starting at a different degree per chain, varied velocities, every eighth step at 70 %
/// probability and 20 % swing.
pub fn stress_player_pattern(n: usize) -> atomik_model::PatternDoc {
    const SCALE: [u8; 5] = [0, 3, 5, 7, 10];
    let mut p = atomik_model::PatternDoc::new("Stress");
    p.length = 64;
    p.resolution = atomik_model::SeqResolution::ThirtySecond;
    p.swing = 20;
    for (i, step) in p.steps.iter_mut().enumerate() {
        let degree = (i + n) % SCALE.len();
        let octave = 12 * ((i / 5 + n) % 3) as u8;
        step.on = true;
        step.key = 45 + SCALE[degree] + octave;
        step.velocity = 60 + ((i * 7 + n * 13) % 60) as u8;
        step.gate = 90;
        step.prob = if i % 8 == 7 { 70 } else { 100 };
    }
    p
}

/// Adds the load (`--stress-players`) to `core`: [`STRESS_PLAYER_CHAINS`] chains of a
/// step sequencer (`pseq1`...) with the dense 1/32 pattern of [`stress_player_pattern`], into an
/// arpeggiator (`parp1`...: up-down, three octaves, 1/32, always running) into a poly synth
/// (`psyn1`...) with its own console channel. A 64-bar chord clip on every arpeggiator's track
/// gives it held keys next to the sequencer's notes, and the sequencer's pitch CV drives the
/// synth's cutoff. Everything goes through ordinary model commands, so it combines with the
/// other stress loads.
///
/// # Errors
/// A message when a command is rejected.
pub fn add_stress_players<S: EngineSink>(core: &mut AppCore<S>) -> Result<(), String> {
    let bar = 4 * PPQ;
    for n in 0..STRESS_PLAYER_CHAINS {
        let (seq, arp, syn) = (
            DeviceKey(format!("pseq{}", n + 1)),
            DeviceKey(format!("parp{}", n + 1)),
            DeviceKey(format!("psyn{}", n + 1)),
        );
        for (type_id, key, name) in [
            ("atomik.step-sequencer", &seq, "Stress Seq"),
            ("atomik.arpeggiator", &arp, "Stress Arp"),
            ("atomik.poly-synth", &syn, "Stress Voice"),
        ] {
            apply(
                core,
                key,
                Command::AddDevice {
                    index: usize::MAX,
                    type_id: type_id.to_string(),
                    key: Some(key.clone()),
                },
            )?;
            apply(
                core,
                key,
                Command::Rename {
                    key: key.clone(),
                    name: format!("{name} {}", n + 1),
                },
            )?;
        }
        apply(
            core,
            &seq,
            Command::SetPattern {
                device: seq.clone(),
                index: 0,
                pattern: stress_player_pattern(n),
                merge: None,
            },
        )?;
        let set = |core: &mut AppCore<S>, key: &DeviceKey, param: &str, value: f32| {
            apply(
                core,
                key,
                Command::SetParam {
                    key: key.clone(),
                    param: param.to_string(),
                    value,
                    merge: None,
                },
            )
        };
        // Up-down, 1/32, three octaves, gate 70 %, running with the transport stopped too.
        for (param, value) in [
            ("mode", 2.0),
            ("rate", 5.0),
            ("octaves", 3.0),
            ("gate", 70.0),
            ("run", 1.0),
        ] {
            set(core, &arp, param, value)?;
        }
        set(core, &syn, "master.level", -20.0)?;
        set(core, &syn, "env.amp.release", 80.0)?;
        let sref = |d: &DeviceKey, s: &str| SocketRef {
            device: d.clone(),
            socket: s.to_string(),
        };
        for (from, to) in [
            (sref(&seq, "notes_out"), sref(&arp, "notes_in")),
            (sref(&arp, "notes_out"), sref(&syn, "notes_in")),
            (sref(&seq, "pitch_cv_out"), sref(&syn, "cutoff_cv_in")),
        ] {
            apply(core, &seq, Command::Connect(Cable { from, to }))?;
        }
        apply(
            core,
            &arp,
            Command::Song(SongCommand::AddTrack {
                device: arp.clone(),
            }),
        )?;
        let root = 57 + 2 * n as u8;
        let notes: Vec<Note> = [0u8, 3, 7]
            .iter()
            .map(|k| Note::new(0, STRESS_SONG_BARS * bar, root + k, 0.75))
            .collect();
        apply(
            core,
            &arp,
            Command::Song(SongCommand::AddClip {
                device: arp.clone(),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length: STRESS_SONG_BARS * bar,
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
            }),
        )?;
    }
    Ok(())
}

/// Containers of the `--stress-containers` project.
pub const STRESS_CONTAINERS: usize = 8;

/// Adds the load (`--stress-containers`) to `core`: [`STRESS_CONTAINERS`] containers
/// (`cont1`...) each holding a poly synth (`syn`), a compressor with look-ahead (`comp`), a stereo
/// delay (`dly`) and a nested effect container (`fx`) with a reverb (`rev`), cabled
/// `syn -> comp -> dly -> fx -> output`, one full-range split to the synth, macro 1 (`Bright`)
/// driving the cutoff of the synth, the delay mix and the compressor threshold and macro 2
/// (`Space`) the mix of the nested reverb, a console channel (TO CH) and a song track whose 64-bar
/// clip plays a chord a bar, with `macro1` and `macro2` automated by ramps every 4 bars.
/// Everything goes through ordinary model commands, so it combines with the other stress loads.
///
/// # Errors
/// A message when a command is rejected.
pub fn add_stress_containers<S: EngineSink>(core: &mut AppCore<S>) -> Result<(), String> {
    use atomik_model::{ContainerCommand, MacroTarget};
    let bar = 4 * PPQ;
    let key = |s: String| DeviceKey(s);
    for n in 0..STRESS_CONTAINERS {
        let cont = key(format!("cont{}", n + 1));
        let path = |local: &str| key(format!("{}/{local}", cont.0));
        apply(
            core,
            &cont,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.container".into(),
                key: Some(cont.clone()),
            },
        )?;
        apply(
            core,
            &cont,
            Command::Rename {
                key: cont.clone(),
                name: format!("Stress Container {}", n + 1),
            },
        )?;
        for (type_id, local, auto_split) in [
            ("atomik.poly-synth", "syn", true),
            ("atomik.compressor", "comp", false),
            ("atomik.stereo-delay", "dly", false),
            ("atomik.container", "fx", false),
        ] {
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::AddInner {
                    container: cont.clone(),
                    index: usize::MAX,
                    type_id: type_id.into(),
                    key: Some(key(local.to_string())),
                    auto_split,
                }),
            )?;
        }
        let fx = path("fx");
        apply(
            core,
            &fx,
            Command::Container(ContainerCommand::AddInner {
                container: fx.clone(),
                index: usize::MAX,
                type_id: "atomik.reverb".into(),
                key: Some(key("rev".to_string())),
                auto_split: false,
            }),
        )?;
        let set = |core: &mut AppCore<S>, k: &DeviceKey, param: &str, value: f32| {
            apply(
                core,
                k,
                Command::SetParam {
                    key: k.clone(),
                    param: param.to_string(),
                    value,
                    merge: None,
                },
            )
        };
        for (device, param, value) in [
            ("syn", "master.level", -14.0),
            ("syn", "env.amp.release", 400.0),
            ("comp", "lookahead", 2.0),
            ("dly", "time.ms", 300.0),
            ("dly", "feedback", 0.4),
        ] {
            set(core, &path(device), param, value)?;
        }
        set(core, &path("fx/rev"), "decay", 1.5)?;
        let sref = |d: &DeviceKey, s: &str| SocketRef {
            device: d.clone(),
            socket: s.to_string(),
        };
        let stages = [
            (path("syn"), "out", path("comp"), "in"),
            (path("comp"), "out", path("dly"), "in"),
            (path("dly"), "out", path("fx"), "in"),
            (path("fx"), "out", path("boundary"), "out"),
        ];
        for (from, fs, to, ts) in stages {
            for side in ["l", "r"] {
                apply(
                    core,
                    &cont,
                    Command::Connect(Cable {
                        from: sref(&from, &format!("{fs}_{side}")),
                        to: sref(&to, &format!("{ts}_{side}")),
                    }),
                )?;
            }
        }
        for side in ["l", "r"] {
            apply(
                core,
                &cont,
                Command::Connect(Cable {
                    from: sref(&path("fx/boundary"), &format!("in_{side}")),
                    to: sref(&path("fx/rev"), &format!("in_{side}")),
                }),
            )?;
            apply(
                core,
                &cont,
                Command::Connect(Cable {
                    from: sref(&path("fx/rev"), &format!("out_{side}")),
                    to: sref(&path("fx/boundary"), &format!("out_{side}")),
                }),
            )?;
        }
        let target = |device: &str, param: &str, min: f32, max: f32| MacroTarget {
            device: key(device.to_string()),
            param: param.to_string(),
            min,
            max,
        };
        for (index, t) in [
            (0u8, target("syn", "filter.cutoff", 300.0, 4_000.0)),
            (0, target("dly", "mix", 0.05, 0.4)),
            (0, target("comp", "threshold", -6.0, -24.0)),
            (1, target("fx/rev", "mix", 0.05, 0.4)),
        ] {
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::SetMacro {
                    container: cont.clone(),
                    index,
                    target: t,
                    merge: None,
                }),
            )?;
        }
        for (index, name) in [(0u8, "Bright"), (1, "Space")] {
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::RenameMacro {
                    container: cont.clone(),
                    index,
                    name: name.into(),
                }),
            )?;
        }
        apply(
            core,
            &cont,
            Command::Console(ConsoleCommand::AddChannel {
                source: cont.clone(),
            }),
        )?;
        apply(
            core,
            &cont,
            Command::Song(SongCommand::AddTrack {
                device: cont.clone(),
            }),
        )?;
        let root = 48 + (n as u8 % 5) * 2;
        let mut notes = Vec::new();
        for b in 0..STRESS_SONG_BARS {
            let shift = [0u8, 5, 7, 3][(b % 4) as usize];
            for k in [0u8, 3, 7, 12] {
                notes.push(Note::new(b * bar, bar, root + shift + k, 0.7));
            }
        }
        let clip = |content| Clip {
            id: ClipId(0),
            start: 0,
            length: STRESS_SONG_BARS * bar,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content,
        };
        apply(
            core,
            &cont,
            Command::Song(SongCommand::AddClip {
                device: cont.clone(),
                lane: 0,
                clip: clip(ClipContent::Notes(notes.into())),
            }),
        )?;
        for (lane, param, phase) in [(1usize, "macro1", 0i64), (2, "macro2", 2)] {
            apply(
                core,
                &cont,
                Command::Song(SongCommand::AddAutomationLane {
                    device: cont.clone(),
                    param: param.into(),
                }),
            )?;
            let points: Vec<AutomationPoint> = (0..STRESS_SONG_BARS / 4)
                .map(|i| AutomationPoint {
                    tick: i * 4 * bar,
                    value: if (i + phase) % 2 == 0 { 0.0 } else { 1.0 },
                    curve: Curve::Linear,
                })
                .collect();
            apply(
                core,
                &cont,
                Command::Song(SongCommand::AddClip {
                    device: cont.clone(),
                    lane,
                    clip: clip(ClipContent::Automation(points)),
                }),
            )?;
        }
    }
    Ok(())
}

/// Character containers of the `--stress-character` project.
pub const STRESS_CHARACTERS: usize = 8;

/// The preset (0 to 15) of Character container `n` (0-based) in bar `bar`: container `n` starts
/// on preset `2 n` and steps one preset per bar, so the eight instances are always on eight
/// different presets, every container visits all 16 (the long latency class `lossy`, preset 8,
/// once per 16 bars) and the classes are crossed all the time.
pub fn stress_character_preset(n: usize, bar: usize) -> usize {
    (2 * n + bar) % 16
}

/// Adds the load (`--stress-character`) to `core`: [`STRESS_CHARACTERS`] containers
/// (`chr1`...) each holding a poly synth (`syn`) cabled into a Character (`fx`), one full-range
/// split to the synth, macro 1 (`Preset`) on the Character's preset and macro 2 (`Transform`) on
/// its Transform, a console channel (TO CH) and a song track whose 64-bar clip plays a four-note
/// chord a bar. `macro1` steps through the presets every bar ([`stress_character_preset`], a step
/// curve) and `macro2` ramps every 4 bars, so a preset change that crosses the latency classes
/// (`lossy` against the rest) and the routing recompile that follows it happen under load, the
/// way an automated project does. Everything goes through ordinary model commands, so it
/// combines with the other stress loads.
///
/// # Errors
/// A message when a command is rejected.
pub fn add_stress_character<S: EngineSink>(core: &mut AppCore<S>) -> Result<(), String> {
    use atomik_model::{ContainerCommand, MacroTarget};
    let bar = 4 * PPQ;
    let key = |s: String| DeviceKey(s);
    for n in 0..STRESS_CHARACTERS {
        let cont = key(format!("chr{}", n + 1));
        let path = |local: &str| key(format!("{}/{local}", cont.0));
        apply(
            core,
            &cont,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.container".into(),
                key: Some(cont.clone()),
            },
        )?;
        apply(
            core,
            &cont,
            Command::Rename {
                key: cont.clone(),
                name: format!("Stress Character {}", n + 1),
            },
        )?;
        for (type_id, local, auto_split) in [
            ("atomik.poly-synth", "syn", true),
            ("atomik.character", "fx", false),
        ] {
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::AddInner {
                    container: cont.clone(),
                    index: usize::MAX,
                    type_id: type_id.into(),
                    key: Some(key(local.to_string())),
                    auto_split,
                }),
            )?;
        }
        let set = |core: &mut AppCore<S>, k: &DeviceKey, param: &str, value: f32| {
            apply(
                core,
                k,
                Command::SetParam {
                    key: k.clone(),
                    param: param.to_string(),
                    value,
                    merge: None,
                },
            )
        };
        for (param, value) in [("master.level", -14.0), ("env.amp.release", 400.0)] {
            set(core, &path("syn"), param, value)?;
        }
        // A little Input Gain drive and a wet-leaning mix, so the saturating presets work.
        set(core, &path("fx"), "gain", 0.6)?;
        set(core, &path("fx"), "mix", 0.85)?;
        let sref = |d: &DeviceKey, s: &str| SocketRef {
            device: d.clone(),
            socket: s.to_string(),
        };
        for side in ["l", "r"] {
            for (from, fs, to, ts) in [
                (path("syn"), "out", path("fx"), "in"),
                (path("fx"), "out", path("boundary"), "out"),
            ] {
                apply(
                    core,
                    &cont,
                    Command::Connect(Cable {
                        from: sref(&from, &format!("{fs}_{side}")),
                        to: sref(&to, &format!("{ts}_{side}")),
                    }),
                )?;
            }
        }
        for (index, name, param, max) in [
            (0u8, "Preset", "preset", 15.0f32),
            (1, "Transform", "transform", 1.0),
        ] {
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::SetMacro {
                    container: cont.clone(),
                    index,
                    target: MacroTarget {
                        device: key("fx".to_string()),
                        param: param.to_string(),
                        min: 0.0,
                        max,
                    },
                    merge: None,
                }),
            )?;
            apply(
                core,
                &cont,
                Command::Container(ContainerCommand::RenameMacro {
                    container: cont.clone(),
                    index,
                    name: name.into(),
                }),
            )?;
        }
        apply(
            core,
            &cont,
            Command::Console(ConsoleCommand::AddChannel {
                source: cont.clone(),
            }),
        )?;
        apply(
            core,
            &cont,
            Command::Song(SongCommand::AddTrack {
                device: cont.clone(),
            }),
        )?;
        let root = 46 + (n as u8 % 5) * 3;
        let mut notes = Vec::new();
        for b in 0..STRESS_SONG_BARS {
            let shift = [0u8, 5, 7, 3][(b % 4) as usize];
            for k in [0u8, 3, 7, 12] {
                notes.push(Note::new(b * bar, bar, root + shift + k, 0.7));
            }
        }
        let clip = |content| Clip {
            id: ClipId(0),
            start: 0,
            length: STRESS_SONG_BARS * bar,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content,
        };
        apply(
            core,
            &cont,
            Command::Song(SongCommand::AddClip {
                device: cont.clone(),
                lane: 0,
                clip: clip(ClipContent::Notes(notes.into())),
            }),
        )?;
        let lanes: [(usize, &str, Vec<AutomationPoint>); 2] = [
            (
                1,
                "macro1",
                (0..STRESS_SONG_BARS)
                    .map(|b| AutomationPoint {
                        tick: b * bar,
                        value: stress_character_preset(n, b as usize) as f32 / 15.0,
                        curve: Curve::Step,
                    })
                    .collect(),
            ),
            (
                2,
                "macro2",
                (0..STRESS_SONG_BARS / 4)
                    .map(|i| AutomationPoint {
                        tick: i * 4 * bar,
                        value: if (i + n as i64) % 2 == 0 { 0.1 } else { 0.9 },
                        curve: Curve::Linear,
                    })
                    .collect(),
            ),
        ];
        for (lane, param, points) in lanes {
            apply(
                core,
                &cont,
                Command::Song(SongCommand::AddAutomationLane {
                    device: cont.clone(),
                    param: param.into(),
                }),
            )?;
            apply(
                core,
                &cont,
                Command::Song(SongCommand::AddClip {
                    device: cont.clone(),
                    lane,
                    clip: clip(ClipContent::Automation(points)),
                }),
            )?;
        }
    }
    Ok(())
}

/// Test synths of the `--stress-plugins` project.
pub const STRESS_PLUGIN_SYNTHS: usize = 8;
/// Test effects of the `--stress-plugins` project: inserts on the synths' channels and, in the
/// free slots, FX returns.
pub const STRESS_PLUGIN_EFFECTS: usize = 8;

/// The CLAP ids of the in-repo test plugins the stress project is made of.
const STRESS_SYNTH_ID: &str = "atomik.test.synth";
const STRESS_EFFECT_ID: &str = "atomik.test.gain";

/// The names of the same plugins in the VST3 test library (VST3 ids are class ids), and of the
/// others that the real-plugin count must not pick up.
const VST3_SYNTH_NAME: &str = "Atomik Test Synth";
const VST3_EFFECT_NAME: &str = "Atomik Test Gain";
const TEST_PLUGIN_NAMES: [&str; 4] = [
    "Atomik Test Gain",
    "Atomik Test Synth",
    "Atomik Test Note Shift",
    "Atomik Test Params",
];

/// The VST3 gain spans -64 to 64 dB (normalised host values); the CLAP one takes dB.
fn vst3_gain_value(db: f32) -> f32 {
    (db + 64.0) / 128.0
}

/// Whether a catalog entry is one of the in-repo test plugins (either format).
fn is_test_plugin(e: &atomik_plugin_host::PluginEntry) -> bool {
    e.plugin.id.starts_with("atomik.test.")
        || (e.plugin.format == atomik_device_sdk::PluginFormat::Vst3
            && TEST_PLUGIN_NAMES.contains(&e.name.as_str()))
}

/// `PluginCommand::Add` for the installed plugin `id` (CLAP), named `name`, at the end of the rack.
fn add_plugin<S: EngineSink>(
    core: &mut AppCore<S>,
    plugin: &atomik_device_sdk::PluginRef,
    key: &DeviceKey,
    name: &str,
) -> Result<(), String> {
    let entry = core
        .plugin_catalog()
        .entry(plugin)
        .filter(|e| e.status == atomik_plugin_host::PluginStatus::Ok)
        .ok_or_else(|| {
            format!(
                "plugin stress: {} is not installed (build the test plugins with `cargo build -p atomik-test-clap -p atomik-test-vst3` and let the scanner find them)",
                plugin.id
            )
        })?;
    let (plugin, layout, path_hint) = (
        entry.plugin.clone(),
        entry.layout.clone(),
        entry.path.to_string_lossy().into_owned(),
    );
    apply(
        core,
        key,
        Command::Plugin(atomik_model::PluginCommand::Add {
            index: usize::MAX,
            container: None,
            plugin,
            layout,
            path_hint: Some(path_hint),
            key: Some(key.clone()),
            auto_split: false,
        }),
    )?;
    apply(
        core,
        key,
        Command::Rename {
            key: key.clone(),
            name: name.to_string(),
        },
    )
}

/// A 64-bar track of dense chords (three notes on every eighth) on `device`, root `root`.
fn add_dense_track<S: EngineSink>(
    core: &mut AppCore<S>,
    device: &DeviceKey,
    root: u8,
) -> Result<(), String> {
    let bar = 4 * PPQ;
    let step = PPQ / 2;
    apply(
        core,
        device,
        Command::Song(SongCommand::AddTrack {
            device: device.clone(),
        }),
    )?;
    let notes: Vec<Note> = (0..STRESS_SONG_BARS * 8)
        .flat_map(|i| {
            let degree = [0u8, 3, 5, 7][(i % 4) as usize];
            [0u8, 4, 7].map(move |k| Note::new(i * step, step, root + degree + k, 0.7))
        })
        .collect();
    apply(
        core,
        device,
        Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: STRESS_SONG_BARS * bar,
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
        }),
    )
}

/// Whether test effect `n` (0-based, `tfx<n + 1>`) of the stress load is the VST3 one when the
/// VST3 test library is installed: every third, so the three kinds of latency (0, 256 and 128
/// frames) all occur in both formats.
pub fn stress_effect_is_vst3(n: usize) -> bool {
    n.is_multiple_of(3)
}

/// Adds the load (`--stress-plugins`) to `core`: [`STRESS_PLUGIN_SYNTHS`] test
/// synths (`tsyn1`..., each with its console channel and a 64-bar track of dense chords) and
/// [`STRESS_PLUGIN_EFFECTS`] test effects (`tfx1`...: the latency parameter stepping through
/// 0, 64, 128 and 256 frames, so delay compensation is part of the load) as inserts on the synths'
/// channels and as FX returns in the free slots, plus up to `real` installed plugins that are not
/// the test ones, in either format (instruments get a channel and a track, effects an insert slot
/// after the test effect). The odd synths and the effects `tfx1`, `tfx4` and `tfx7`
/// ([`stress_effect_is_vst3`]: latencies 0, 256 and 128 frames) are the **VST3** test plugins when
/// the VST3 test library is installed (both formats in one load, delay compensation of
/// both included: the VST3 test effect announces a latency change itself, like the CLAP one),
/// else every one is CLAP. Everything goes through ordinary model commands, so it combines with
/// the other stress loads. The plugins are created one per frame like a project open; call
/// [`AppCore::wait_plugins`] to wait for them.
///
/// # Errors
/// A message when the test plugins are not in the catalog or a command is rejected.
pub fn add_stress_plugins<S: EngineSink>(core: &mut AppCore<S>, real: usize) -> Result<(), String> {
    let plugin = |id: &str| atomik_device_sdk::PluginRef {
        format: atomik_device_sdk::PluginFormat::Clap,
        id: id.to_string(),
    };
    let (synth, effect) = (plugin(STRESS_SYNTH_ID), plugin(STRESS_EFFECT_ID));
    let vst3 = |name: &str| {
        core.plugin_catalog()
            .entries()
            .iter()
            .find(|e| {
                e.plugin.format == atomik_device_sdk::PluginFormat::Vst3
                    && e.name == name
                    && e.status == atomik_plugin_host::PluginStatus::Ok
            })
            .map(|e| e.plugin.clone())
    };
    let (vst3_synth, vst3_effect) = (vst3(VST3_SYNTH_NAME), vst3(VST3_EFFECT_NAME));
    let set = |core: &mut AppCore<S>, key: &DeviceKey, param: &str, value: f32| {
        apply(
            core,
            key,
            Command::SetParam {
                key: key.clone(),
                param: param.to_string(),
                value,
                merge: None,
            },
        )
    };
    let mut channels: Vec<DeviceKey> = Vec::new();
    for n in 0..STRESS_PLUGIN_SYNTHS {
        let key = DeviceKey(format!("tsyn{}", n + 1));
        let this = if n % 2 == 1 {
            vst3_synth.as_ref().unwrap_or(&synth)
        } else {
            &synth
        };
        add_plugin(core, this, &key, &format!("Stress Test Synth {}", n + 1))?;
        // The plugin's own ids (both formats): p1 waveform (stepped), p2 level.
        set(core, &key, "p1", (n % 2) as f32)?;
        set(core, &key, "p2", 0.2)?;
        add_dense_track(core, &key, 36 + 3 * n as u8)?;
        if let Some(c) = core.doc().console.channel_of(&key) {
            channels.push(c.channel.clone());
        }
    }
    let free_returns: Vec<usize> = core
        .doc()
        .console
        .returns
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_none())
        .map(|(i, _)| i)
        .collect();
    let mut returns = free_returns.into_iter();
    for n in 0..STRESS_PLUGIN_EFFECTS {
        let key = DeviceKey(format!("tfx{}", n + 1));
        let (this, vst3_value) = match stress_effect_is_vst3(n)
            .then_some(vst3_effect.as_ref())
            .flatten()
        {
            Some(v) => (v, true),
            None => (&effect, false),
        };
        add_plugin(core, this, &key, &format!("Stress Test Effect {}", n + 1))?;
        set(core, &key, "p3", [0.0, 64.0, 128.0, 256.0][n % 4])?;
        set(
            core,
            &key,
            "p1",
            if vst3_value {
                vst3_gain_value(-3.0)
            } else {
                -3.0
            },
        )?;
        match (n >= STRESS_PLUGIN_EFFECTS / 2)
            .then(|| returns.next())
            .flatten()
        {
            Some(slot) => {
                apply(
                    core,
                    &key,
                    Command::Console(ConsoleCommand::SetReturn {
                        slot,
                        effect: Some(key.clone()),
                    }),
                )?;
                for channel in channels.iter().take(4) {
                    set(core, channel, &format!("send{}.on", slot + 1), 1.0)?;
                    set(
                        core,
                        channel,
                        &format!("send{}.level", slot + 1),
                        STRESS_SEND_DB,
                    )?;
                }
            }
            None => {
                let channel = channels
                    .get(n % channels.len().max(1))
                    .ok_or("plugin stress: no channel for an insert")?
                    .clone();
                apply(
                    core,
                    &key,
                    Command::Console(ConsoleCommand::SetInsert {
                        channel,
                        slot: 0,
                        effect: Some(atomik_model::InsertEffect::Existing(key.clone())),
                    }),
                )?;
            }
        }
    }
    let real_entries: Vec<(
        atomik_device_sdk::PluginRef,
        atomik_device_sdk::PluginKindHint,
        String,
    )> = core
        .plugin_catalog()
        .entries()
        .iter()
        .filter(|e| e.status == atomik_plugin_host::PluginStatus::Ok && !is_test_plugin(e))
        .map(|e| (e.plugin.clone(), e.kind, e.name.clone()))
        .take(real)
        .collect();
    for (n, (plugin, kind, name)) in real_entries.into_iter().enumerate() {
        let key = DeviceKey(format!("treal{}", n + 1));
        add_plugin(core, &plugin, &key, &name)?;
        match kind {
            atomik_device_sdk::PluginKindHint::Instrument => {
                add_dense_track(core, &key, 48 + 2 * n as u8)?;
            }
            atomik_device_sdk::PluginKindHint::Effect => {
                if let Some(channel) = channels.first().cloned() {
                    apply(
                        core,
                        &key,
                        Command::Console(ConsoleCommand::SetInsert {
                            channel,
                            slot: 1,
                            effect: Some(atomik_model::InsertEffect::Existing(key.clone())),
                        }),
                    )?;
                }
            }
            atomik_device_sdk::PluginKindHint::NoteEffect => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_core::default_registry;
    use atomik_model::{empty_doc, load_patch, load_project, save_patch, save_project};

    #[test]
    fn the_e3_2_song_has_8_tracks_of_64_bars_with_automation_and_round_trips() {
        let reg = default_registry();
        let doc = stress_song_doc(&reg);
        let song = &doc.song;
        assert_eq!(song.tracks.len(), STRESS_SONG_TRACKS);
        assert_eq!(song.end, STRESS_SONG_BARS * 4 * PPQ);
        for t in &song.tracks {
            assert_eq!(t.lanes.len(), 3, "notes + 2 automation lanes");
            assert!(t.lanes.iter().all(|l| l.clips.len() == 1));
            assert_eq!(t.lanes[0].clips[0].length, song.end);
        }
        assert!(
            doc.cables
                .iter()
                .any(|c| c.to.device.as_str() == "interface")
        );
        assert_eq!(load_project(&save_project(&doc), &reg).unwrap(), doc);
        assert_eq!(stress_song_doc(&reg), doc, "deterministic");
    }

    /// The stress song plays through 8 console channels and 4 FX returns.
    #[test]
    fn the_e4_3_song_runs_through_8_channels_and_4_returns() {
        let reg = default_registry();
        let doc = stress_song_doc(&reg);
        let console = &doc.console;
        assert_eq!(console.channels.len(), STRESS_SONG_TRACKS);
        assert!(console.master.is_some());
        let returns: Vec<_> = console.returns.iter().flatten().collect();
        assert_eq!(returns.len(), STRESS_SONG_RETURNS);
        for r in &returns {
            assert_eq!(doc.device(r).unwrap().type_id, "atomik.stereo-delay");
        }
        // Every channel sends to one of the returns.
        for (i, c) in console.channels.iter().enumerate() {
            let k = i % STRESS_SONG_RETURNS + 1;
            let d = doc.device(&c.channel).unwrap();
            assert_eq!(d.params.get(&format!("send{k}.on")), Some(&1.0), "{i}");
            assert_eq!(
                c.source.as_ref().map(|s| s.0.clone()),
                Some(format!("s{}", i + 1))
            );
        }
        // 8 synths + 8 channels + master + 4 effects; no line mixers any more.
        assert_eq!(
            doc.devices.len(),
            2 * STRESS_SONG_TRACKS + 1 + STRESS_SONG_RETURNS
        );
    }

    #[test]
    fn the_e2_2_rack_has_100_devices_and_300_cables() {
        let reg = default_registry();
        let doc = stress_doc(&reg, STRESS_DEVICES, STRESS_CABLES);
        assert_eq!(
            doc.devices.len(),
            100,
            "no automatic channels in the rack stress"
        );
        assert!(doc.console.channels.is_empty());
        assert_eq!(doc.cables.len(), 300);
        let types: std::collections::BTreeSet<_> =
            doc.devices.iter().map(|d| d.type_id.as_str()).collect();
        assert_eq!(types.len(), 3, "all device types are used");
    }

    #[test]
    fn the_rack_is_valid_and_reaches_the_interface() {
        let reg = default_registry();
        let doc = stress_doc(&reg, STRESS_DEVICES, STRESS_CABLES);
        let reloaded = load_patch(&save_patch(&doc), &reg).expect("valid patch");
        assert_eq!(reloaded, doc);
        assert!(
            doc.cables
                .iter()
                .any(|c| c.to.device.as_str() == "interface")
        );
    }

    #[test]
    fn it_is_deterministic_and_scales_down() {
        let reg = default_registry();
        assert_eq!(stress_doc(&reg, 10, 20), stress_doc(&reg, 10, 20));
        let small = stress_doc(&reg, 4, 1000);
        assert_eq!(small.devices.len(), 4);
        assert!(small.cables.len() < 1000, "capped by the available inputs");
        assert!(stress_doc(&reg, 0, 10).devices.is_empty());
    }

    #[test]
    fn the_e5_3_files_run_from_12_to_42_seconds_and_the_longer_ones_stream() {
        let frames: Vec<u64> = (0..STRESS_AUDIO_TRACKS)
            .map(|i| stress_audio_frames(i, 48_000))
            .collect();
        assert_eq!(frames[0], 12 * 48_000);
        assert_eq!(frames[15], 42 * 48_000);
        let streamed = frames
            .iter()
            .filter(|f| crate::stream_source::should_stream(**f, 2, STRESS_AUDIO_THRESHOLD_BYTES))
            .count();
        assert_eq!(streamed, 11, "some stream and some stay in RAM");
        assert_eq!(STRESS_AUDIO_TRACKS, 16);
    }

    #[test]
    fn the_generated_files_are_deterministic_stereo_and_reused() {
        let dir = std::env::temp_dir().join(format!("atomik-stress-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = write_stress_audio_with(&dir, 48_000, &|i| 1_000 + i as u64).unwrap();
        assert_eq!(paths.len(), STRESS_AUDIO_TRACKS);
        let a = std::fs::read(&paths[3]).unwrap();
        let d = atomik_audio_file::read_wav_f32(&paths[3]).unwrap();
        assert_eq!((d.channels.len(), d.frames()), (2, 1_003));
        assert!(d.channels[0].iter().any(|s| s.abs() > 0.01));
        assert_ne!(d.channels[0], d.channels[1]);
        let again = write_stress_audio_with(&dir, 48_000, &|i| 1_000 + i as u64).unwrap();
        assert_eq!(again, paths);
        assert_eq!(std::fs::read(&paths[3]).unwrap(), a, "reused untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_e5_3_project_has_16_audio_and_8_instrument_tracks_some_streamed() {
        use crate::app_core::AppCore;
        use crate::offline_sink::OfflineSink;
        let dir = std::env::temp_dir().join(format!("atomik-stress-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Short files and a matching threshold: the longer ones are above 8 000 bytes.
        let paths = write_stress_audio_with(&dir, 48_000, &|i| 500 + 200 * i as u64).unwrap();
        let reg = default_registry();
        let mut sink = OfflineSink::new();
        sink.threshold = Some(8_000);
        let mut core = AppCore::with_doc(sink, stress_song_doc(&reg), None);
        core.import_audio(paths, atomik_ui::ImportTarget::NewTracks, 0);
        core.wait_background();
        let tracks = &core.doc().song.tracks;
        assert_eq!(tracks.len(), STRESS_SONG_TRACKS + STRESS_AUDIO_TRACKS);
        let audio = tracks
            .iter()
            .filter(|t| core.doc().is_audio_track(&t.device, core.registry()))
            .count();
        assert_eq!(audio, STRESS_AUDIO_TRACKS);
        let streamed = core
            .song_assets()
            .values()
            .filter(|a| a.is_stream())
            .count();
        assert!(streamed > 0 && streamed < STRESS_AUDIO_TRACKS, "{streamed}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_e6_4_samples_are_stereo_two_seconds_and_reused() {
        let dir = std::env::temp_dir().join(format!("atomik-stress-smp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = write_stress_samples(&dir, 48_000).unwrap();
        assert_eq!(paths.len(), 2);
        let d = atomik_audio_file::read_wav_f32(&paths[0]).unwrap();
        assert_eq!((d.channels.len(), d.frames()), (2, 96_000));
        assert_ne!(d.channels[0], d.channels[1]);
        let before = std::fs::read(&paths[1]).unwrap();
        assert_eq!(write_stress_samples(&dir, 48_000).unwrap(), paths);
        assert_eq!(
            std::fs::read(&paths[1]).unwrap(),
            before,
            "reused untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Four samplers, two layered looped zones each, a 16-note chord a bar: 32 voices
    /// each, next to the 8 instrument tracks.
    #[test]
    fn the_e6_4_project_has_4_samplers_of_32_voices_next_to_the_8_instrument_tracks() {
        use crate::offline_sink::OfflineSink;
        use crate::sink::{RecordingSink, SinkCall};
        let dir = std::env::temp_dir().join(format!("atomik-stress-e64-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reg = default_registry();
        let mut core = AppCore::with_doc(RecordingSink::new(), stress_song_doc(&reg), None);
        add_stress_samplers(&mut core, &dir).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        let doc = core.doc();
        assert_eq!(doc.song.tracks.len(), STRESS_SONG_TRACKS + STRESS_SAMPLERS);
        let samplers: Vec<_> = doc
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.sampler")
            .collect();
        assert_eq!(samplers.len(), STRESS_SAMPLERS);
        for d in &samplers {
            assert_eq!(d.zones.len(), 2, "two layers, so every note is two voices");
            assert!(d.zones.iter().all(|z| z.loop_mode == LoopMode::Forward));
            assert!(d.zones.iter().all(|z| (z.key_lo, z.key_hi) == (0, 127)));
            assert_eq!(d.params.get("filter.enabled"), Some(&1.0));
            let track = doc.song.track(&d.key).unwrap();
            let ClipContent::Notes(notes) = &track.lanes[0].clips[0].content else {
                panic!("a note clip");
            };
            assert_eq!(notes.len(), STRESS_CHORD_NOTES);
            assert_eq!(notes.len() * d.zones.len(), 32, "{}: 32 voices", d.key);
        }
        // The engine got one two-zone map per sampler (the files are shared by hash).
        let maps: Vec<usize> = core
            .sink_mut()
            .take()
            .into_iter()
            .filter_map(|c| match c {
                SinkCall::Samples { map: Some(m), .. } => Some(m.zones().len()),
                _ => None,
            })
            .collect();
        assert_eq!(maps.last(), Some(&2));
        assert_eq!(
            core.doc().assets.len(),
            2,
            "the same two files for all samplers"
        );
        core.rebuild_engine();
        let sent = core
            .sink_mut()
            .take()
            .into_iter()
            .filter(|c| matches!(c, SinkCall::Samples { map: Some(_), .. }))
            .count();
        assert_eq!(sent, STRESS_SAMPLERS);

        // Played for real (an offline engine), it sounds and stays finite.
        let mut live = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_samplers(&mut live, &dir).unwrap();
        live.transport_action(atomik_ui::TransportAction::Play);
        let out = live.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 0.01), "it sounds");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--stress-drums`: four drum machines, all 16 pads loaded, dense 1/32 patterns on every
    /// pad, looped for the whole song, and the pads' maps reach the engine.
    #[test]
    fn the_stress_drums_project_has_4_drum_machines_with_16_loaded_pads_and_dense_patterns() {
        use crate::offline_sink::OfflineSink;
        use crate::sink::{RecordingSink, SinkCall};
        let dir = std::env::temp_dir().join(format!("atomik-stress-drums-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reg = default_registry();
        let mut core = AppCore::with_doc(RecordingSink::new(), stress_song_doc(&reg), None);
        add_stress_drums(&mut core, &dir).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        let doc = core.doc();
        assert_eq!(
            doc.song.tracks.len(),
            STRESS_SONG_TRACKS + STRESS_DRUM_MACHINES
        );
        let machines: Vec<_> = doc
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.drum-machine")
            .collect();
        assert_eq!(machines.len(), STRESS_DRUM_MACHINES);
        for d in &machines {
            assert_eq!(d.zones.len(), 16, "every pad has a sound");
            for pad in 1..=16u8 {
                let key = atomik_model::pad_key(pad);
                assert!(d.zones.iter().any(|z| (z.key_lo, z.key_hi) == (key, key)));
            }
            let clip = &doc.song.track(&d.key).unwrap().lanes[0].clips[0];
            let grid = clip.grid.unwrap();
            assert_eq!(grid.resolution, atomik_model::StepResolution::ThirtySecond);
            let ClipContent::Notes(notes) = &clip.content else {
                panic!("a note clip");
            };
            for pad in 1..=16u8 {
                let hits = notes
                    .iter()
                    .filter(|n| n.key == atomik_model::pad_key(pad))
                    .count();
                assert!(hits >= 10, "pad {pad} has {hits} hits a bar");
            }
        }
        assert_eq!(core.doc().assets.len(), 16, "the sounds are shared by hash");
        // The engine got a 16-zone map per drum machine.
        let maps: Vec<usize> = core
            .sink_mut()
            .take()
            .into_iter()
            .filter_map(|c| match c {
                SinkCall::Samples { map: Some(m), .. } => Some(m.zones().len()),
                _ => None,
            })
            .collect();
        assert_eq!(maps.last(), Some(&16));
        // Played for real (an offline engine), it sounds and stays finite.
        let mut live = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_drums(&mut live, &dir).unwrap();
        live.transport_action(atomik_ui::TransportAction::Play);
        let out = live.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 0.01), "it sounds");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--stress-slicer`: four loop slicers with a transient-sliced loop and dense patterns.
    #[test]
    fn the_stress_slicer_project_has_4_sliced_loops_and_dense_retriggered_patterns() {
        use crate::offline_sink::OfflineSink;
        use crate::sink::{RecordingSink, SinkCall};
        let dir = std::env::temp_dir().join(format!("atomik-stress-slicer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reg = default_registry();
        let mut core = AppCore::with_doc(RecordingSink::new(), stress_song_doc(&reg), None);
        add_stress_slicers(&mut core, &dir).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        let doc = core.doc();
        assert_eq!(doc.song.tracks.len(), STRESS_SONG_TRACKS + STRESS_SLICERS);
        let slicers: Vec<_> = doc
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.loop-slicer")
            .collect();
        assert_eq!(slicers.len(), STRESS_SLICERS);
        for d in &slicers {
            assert!(d.zones.len() >= 8, "{} slices", d.zones.len());
            assert_eq!(d.params.get("loop.beats"), Some(&8.0));
            let clip = &doc.song.track(&d.key).unwrap().lanes[0].clips[0];
            let ClipContent::Notes(notes) = &clip.content else {
                panic!("a note clip");
            };
            assert!(notes.len() >= 8 * 10, "{} hits a bar", notes.len());
            assert!(
                notes
                    .iter()
                    .all(|n| d.zones.iter().any(|z| z.key_lo == n.key))
            );
        }
        assert_eq!(doc.assets.len(), 1, "the loop is shared by hash");
        let maps = core
            .sink_mut()
            .take()
            .into_iter()
            .filter(|c| matches!(c, SinkCall::Samples { map: Some(_), .. }))
            .count();
        assert!(maps >= STRESS_SLICERS);
        // Played for real (an offline engine), it sounds and stays finite.
        let mut live = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_slicers(&mut live, &dir).unwrap();
        live.transport_action(atomik_ui::TransportAction::Play);
        let out = live.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 0.01), "it sounds");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--stress-fx` adds 8 effect inserts (mixed), 2 reverbs on returns and mastering
    /// on the master over the stress song.
    #[test]
    fn the_stress_fx_load_adds_8_inserts_2_reverbs_and_mastering() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        let before = core.doc().devices.len();
        add_stress_fx(&mut core).unwrap();
        let doc = core.doc();
        assert_eq!(
            doc.devices.len(),
            before + STRESS_FX_INSERTS + STRESS_FX_REVERBS + 1
        );
        let type_of = |k: &str| doc.device(&DeviceKey(k.into())).unwrap().type_id.clone();
        let types: std::collections::BTreeSet<String> = (1..=STRESS_FX_INSERTS)
            .map(|i| type_of(&format!("fxi{i}")))
            .collect();
        assert!(types.len() >= 4, "mixed types: {types:?}");
        // Every insert sits between its channel's insert send and return.
        for (i, ch) in doc.console.channels.iter().enumerate() {
            let fx = DeviceKey(format!("fxi{}", i + 1));
            let has = |from: &DeviceKey, fs: &str, to: &DeviceKey, ts: &str| {
                doc.cables.iter().any(|c| {
                    c.from.device == *from
                        && c.from.socket == fs
                        && c.to.device == *to
                        && c.to.socket == ts
                })
            };
            assert!(has(&ch.channel, "insert_send_l", &fx, "in_l"), "{i}");
            assert!(has(&fx, "out_r", &ch.channel, "insert_return_r"), "{i}");
        }
        let reverbs: Vec<_> = doc.console.returns[STRESS_SONG_RETURNS..]
            .iter()
            .flatten()
            .collect();
        assert_eq!(reverbs.len(), STRESS_FX_REVERBS);
        for r in reverbs {
            assert_eq!(doc.device(r).unwrap().type_id, "atomik.reverb");
        }
        assert_eq!(type_of("fx-mastering"), "atomik.mastering");
        let master = doc.console.master.clone().unwrap();
        assert!(doc.cables.iter().any(|c| c.from.device == master
            && c.from.socket == "insert_send_l"
            && c.to.device.as_str() == "fx-mastering"));
        // It saves and loads, plays for real and stays finite.
        assert_eq!(load_project(&save_project(doc), &reg).unwrap(), *doc);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(48_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 1e-4), "it sounds");
    }

    #[test]
    fn the_stress_fx_load_combines_with_the_drums_load() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let dir = std::env::temp_dir().join("atomik-stress-fx-drums");
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_drums(&mut core, &dir).unwrap();
        add_stress_fx(&mut core).unwrap();
        // The drum machines' channels have no insert effect; the first 8 channels do.
        assert!(core.doc().device(&DeviceKey("fxi8".into())).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--stress-players` adds 4 sequencers into 4 arpeggiators into 4 synths with dense
    /// 1/32 patterns over the stress song.
    #[test]
    fn the_stress_players_load_chains_4_sequencers_through_4_arpeggiators_into_4_synths() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        let before = core.doc().devices.len();
        let channels = core.doc().console.channels.len();
        add_stress_players(&mut core).unwrap();
        let doc = core.doc();
        assert_eq!(
            doc.devices.len(),
            before + 3 * STRESS_PLAYER_CHAINS + STRESS_PLAYER_CHAINS
        );
        assert_eq!(doc.console.channels.len(), channels + STRESS_PLAYER_CHAINS);
        let has = |from: &str, fs: &str, to: &str, ts: &str| {
            doc.cables.iter().any(|c| {
                c.from.device.0 == from
                    && c.from.socket == fs
                    && c.to.device.0 == to
                    && c.to.socket == ts
            })
        };
        for n in 1..=STRESS_PLAYER_CHAINS {
            let type_of = |k: String| doc.device(&DeviceKey(k)).unwrap().type_id.clone();
            assert_eq!(type_of(format!("pseq{n}")), "atomik.step-sequencer");
            assert_eq!(type_of(format!("parp{n}")), "atomik.arpeggiator");
            assert_eq!(type_of(format!("psyn{n}")), "atomik.poly-synth");
            assert!(has(
                &format!("pseq{n}"),
                "notes_out",
                &format!("parp{n}"),
                "notes_in"
            ));
            assert!(has(
                &format!("parp{n}"),
                "notes_out",
                &format!("psyn{n}"),
                "notes_in"
            ));
            assert!(has(
                &format!("pseq{n}"),
                "pitch_cv_out",
                &format!("psyn{n}"),
                "cutoff_cv_in"
            ));
            let steps = &doc
                .device(&DeviceKey(format!("pseq{n}")))
                .unwrap()
                .steps
                .as_ref()
                .unwrap()
                .patterns[0];
            assert_eq!(steps.resolution, atomik_model::SeqResolution::ThirtySecond);
            assert_eq!(steps.length, 64);
            assert!(
                steps.steps.iter().all(|s| s.on),
                "dense: every 1/32 step plays"
            );
        }
        // It saves and loads, plays for real, stays finite and is deterministic.
        assert_eq!(load_project(&save_project(doc), &reg).unwrap(), *doc);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
        let mut again = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_players(&mut again).unwrap();
        again.transport_action(atomik_ui::TransportAction::Play);
        let twice = again.sink_mut().render(96_000);
        assert!(
            out.iter()
                .flatten()
                .map(|x| x.to_bits())
                .eq(twice.iter().flatten().map(|x| x.to_bits())),
            "two runs are bit-identical"
        );
    }

    #[test]
    fn the_stress_players_load_combines_with_the_fx_load() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_players(&mut core).unwrap();
        add_stress_fx(&mut core).unwrap();
        assert!(core.doc().device(&DeviceKey("fxi8".into())).is_some());
        assert!(core.doc().device(&DeviceKey("pseq4".into())).is_some());
    }

    /// Runs some UI frames over the offline engine, as a running app does: the engine hands back
    /// the plans and songs the edits retired (it applies nothing new while that garbage is parked).
    fn settle(core: &mut AppCore<crate::offline_sink::OfflineSink>) {
        for _ in 0..200 {
            core.sink_mut().render(64);
            core.frame();
        }
    }

    /// `--stress-containers` adds 8 containers (synth, compressor, delay, a nested effect
    /// container) with macros moving by automation over the stress song.
    #[test]
    fn the_stress_containers_load_builds_8_containers_with_nested_effects_and_macro_lanes() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        let before = core.doc().devices.len();
        let channels = core.doc().console.channels.len();
        let tracks = core.doc().song.tracks.len();
        add_stress_containers(&mut core).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        let doc = core.doc();
        // 8 containers and their 8 console channels.
        assert_eq!(
            doc.devices.len(),
            before + STRESS_CONTAINERS + STRESS_CONTAINERS
        );
        assert_eq!(doc.console.channels.len(), channels + STRESS_CONTAINERS);
        assert_eq!(doc.song.tracks.len(), tracks + STRESS_CONTAINERS);
        assert_eq!(doc.inner_device_count(), STRESS_CONTAINERS * 5);
        for n in 1..=STRESS_CONTAINERS {
            let c = DeviceKey(format!("cont{n}"));
            let inner = doc.inner(&c).unwrap();
            let types: Vec<&str> = inner.devices.iter().map(|d| d.type_id.as_str()).collect();
            assert_eq!(
                types,
                [
                    "atomik.poly-synth",
                    "atomik.compressor",
                    "atomik.stereo-delay",
                    "atomik.container"
                ]
            );
            assert_eq!(inner.splits.len(), 1);
            assert_eq!(inner.macros[0].name, "Bright");
            assert_eq!(inner.macros[0].targets.len(), 3);
            assert_eq!(inner.macros[1].targets.len(), 1);
            let nested = doc.inner(&DeviceKey(format!("cont{n}/fx"))).unwrap();
            assert_eq!(nested.devices[0].type_id, "atomik.reverb");
            let track = doc.song.track(&c).unwrap();
            assert_eq!(track.lanes.len(), 3, "notes + macro1 + macro2");
            assert!(doc.console.channel_of(&c).is_some());
        }
        // A split per container, nested ones included.
        let splits = doc
            .expand(&reg)
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.note-split")
            .count();
        assert_eq!(splits, STRESS_CONTAINERS * 2);
        // It saves and loads, plays for real, stays finite and is deterministic.
        assert_eq!(load_project(&save_project(doc), &reg).unwrap(), *doc);
        settle(&mut core);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
        let mut again = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_containers(&mut again).unwrap();
        settle(&mut again);
        again.transport_action(atomik_ui::TransportAction::Play);
        let twice = again.sink_mut().render(96_000);
        assert!(
            out.iter()
                .flatten()
                .map(|x| x.to_bits())
                .eq(twice.iter().flatten().map(|x| x.to_bits())),
            "two runs are bit-identical"
        );
    }

    /// `--stress-plugins` builds 8 test synths and 8 test effects over the stress song, plays,
    /// is deterministic, and survives 200 adds and removes with nothing left behind.
    #[test]
    fn the_stress_plugins_load_builds_8_synths_and_8_effects_and_plays() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{PKG, config};
        let reg = default_registry();
        let make = || {
            let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
            core.configure_plugins(config(true, &[PKG]));
            let channels = core.doc().console.channels.len();
            add_stress_plugins(&mut core, 0).unwrap();
            core.wait_plugins();
            assert!(
                core.last_rejection().is_none(),
                "{:?}",
                core.last_rejection()
            );
            settle(&mut core);
            (core, channels)
        };
        let (mut core, channels) = make();
        assert_eq!(core.plugin_instance_count(), 16, "every plugin is live");
        assert_eq!(core.doc().plugin_devices().len(), 16);
        assert_eq!(
            core.doc().console.channels.len(),
            channels + STRESS_PLUGIN_SYNTHS
        );
        let tracks = core.doc().song.tracks.len();
        assert_eq!(tracks, STRESS_SONG_TRACKS + STRESS_PLUGIN_SYNTHS);
        let inserts = (0..STRESS_PLUGIN_EFFECTS / 2)
            .filter(|n| {
                let chan = core
                    .doc()
                    .console
                    .channel_of(&DeviceKey(format!("tsyn{}", n + 1)));
                chan.is_some()
            })
            .count();
        assert_eq!(inserts, STRESS_PLUGIN_EFFECTS / 2);
        let returns: Vec<_> = core.doc().console.returns.iter().flatten().collect();
        assert!(
            returns.len() > STRESS_SONG_RETURNS,
            "effects on returns: {returns:?}"
        );
        // It saves and loads (the states come from the plugins), plays, and is deterministic.
        let text = save_project(core.doc());
        assert_eq!(
            load_project(&text, core.registry())
                .unwrap()
                .plugin_devices()
                .len(),
            16
        );
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(96_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
        let (mut again, _) = make();
        again.transport_action(atomik_ui::TransportAction::Play);
        let twice = again.sink_mut().render(96_000);
        assert!(
            out.iter()
                .flatten()
                .map(|x| x.to_bits())
                .eq(twice.iter().flatten().map(|x| x.to_bits())),
            "two runs are bit-identical"
        );
    }

    /// In the stress project: with the VST3 test library installed the odd synths and the
    /// odd effects are VST3, all 16 are live, it plays, and it sounds exactly like the all-CLAP
    /// load (the same DSP in both formats).
    #[test]
    fn the_stress_plugins_load_mixes_in_the_vst3_test_plugins_when_they_are_installed() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{PKG, PKG_VST3, config};
        let reg = default_registry();
        let run = |packages: &[&str]| {
            let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
            core.configure_plugins(config(true, packages));
            add_stress_plugins(&mut core, 0).unwrap();
            core.wait_plugins();
            assert!(
                core.last_rejection().is_none(),
                "{:?}",
                core.last_rejection()
            );
            settle(&mut core);
            assert_eq!(core.plugin_instance_count(), 16, "every plugin is live");
            let vst3 = core
                .doc()
                .plugin_devices()
                .iter()
                .filter(|(_, d)| {
                    d.plugin.as_ref().unwrap().format == atomik_device_sdk::PluginFormat::Vst3
                })
                .count();
            core.transport_action(atomik_ui::TransportAction::Play);
            let out = core.sink_mut().render(96_000);
            (vst3, out)
        };
        let (clap_only, clap_out) = run(&[PKG]);
        assert_eq!(clap_only, 0, "no VST3 library, no VST3 device");
        let (mixed, mixed_out) = run(&[PKG, PKG_VST3]);
        assert_eq!(
            mixed,
            STRESS_PLUGIN_SYNTHS / 2
                + (0..STRESS_PLUGIN_EFFECTS)
                    .filter(|&n| stress_effect_is_vst3(n))
                    .count(),
            "the odd synths and the effects 1, 4 and 7 are VST3"
        );
        assert_eq!(
            (0..STRESS_PLUGIN_EFFECTS)
                .filter(|&n| stress_effect_is_vst3(n))
                .collect::<Vec<_>>(),
            [0, 3, 6],
            "tfx1, tfx4 and tfx7 are the VST3 effects"
        );
        let latencies: Vec<usize> = (0..STRESS_PLUGIN_EFFECTS)
            .filter(|&n| stress_effect_is_vst3(n))
            .map(|n| [0, 64, 128, 256][n % 4])
            .collect();
        assert!(
            latencies.contains(&0) && latencies.iter().any(|&l| l > 0),
            "the VST3 effects of the load have latency ({latencies:?}), PDC is part of the load"
        );
        assert!(mixed_out[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
        assert!(
            clap_out
                .iter()
                .flatten()
                .map(|x| x.to_bits())
                .eq(mixed_out.iter().flatten().map(|x| x.to_bits())),
            "the two formats give the same bits"
        );
    }

    /// `--stress-plugins-real N` takes installed plugins of either format that are not the test
    /// plugins (the VST3 variants library stands in for real plugins here).
    #[test]
    fn the_real_plugin_count_takes_vst3_plugins_and_skips_the_test_plugins_of_both_formats() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{PKG, PKG_VST3, PKG_VST3_VARIANTS, config};
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        core.configure_plugins(config(true, &[PKG, PKG_VST3, PKG_VST3_VARIANTS]));
        let usable: Vec<_> = core
            .plugin_catalog()
            .entries()
            .iter()
            .filter(|e| {
                e.status == atomik_plugin_host::PluginStatus::Ok
                    && e.plugin.format == atomik_device_sdk::PluginFormat::Vst3
                    && !TEST_PLUGIN_NAMES.contains(&e.name.as_str())
            })
            .map(|e| e.name.clone())
            .collect();
        assert!(
            usable.len() >= 2,
            "the variants library offers some: {usable:?}"
        );
        add_stress_plugins(&mut core, 2).unwrap();
        core.wait_plugins();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        for k in ["treal1", "treal2"] {
            let device = core.doc().device(&DeviceKey(k.into())).expect(k);
            let plugin = device.plugin.as_ref().unwrap();
            assert_eq!(plugin.format, atomik_device_sdk::PluginFormat::Vst3, "{k}");
            assert!(
                !TEST_PLUGIN_NAMES.contains(&plugin.name()),
                "{k} is {}",
                plugin.name()
            );
        }
        assert!(core.doc().device(&DeviceKey("treal3".into())).is_none());
        assert_eq!(core.plugin_instance_count(), 18);
    }

    #[test]
    fn the_stress_plugins_load_needs_the_test_plugins_and_combines_with_the_other_loads() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{PKG, config};
        let reg = default_registry();
        let mut bare = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        let e = add_stress_plugins(&mut bare, 0).unwrap_err();
        assert!(
            e.contains("not installed") && e.contains("atomik-test-clap"),
            "{e}"
        );
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        core.configure_plugins(config(true, &[PKG]));
        add_stress_players(&mut core).unwrap();
        add_stress_containers(&mut core).unwrap();
        add_stress_plugins(&mut core, 0).unwrap();
        add_stress_fx(&mut core).unwrap();
        core.wait_plugins();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        assert_eq!(core.plugin_instance_count(), 16);
        assert!(core.doc().device(&DeviceKey("tfx8".into())).is_some());
        assert!(core.doc().device(&DeviceKey("cont8".into())).is_some());
        assert!(core.doc().device(&DeviceKey("fxi8".into())).is_some());
        settle(&mut core);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(48_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
    }

    /// An engine restart with 16 plugins re-activates them without creating them again,
    /// and a restart keeps the document's params.
    #[test]
    fn an_engine_restart_with_16_plugins_reattaches_every_instance_without_creating_it_again() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{PKG, config};
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        core.configure_plugins(config(true, &[PKG]));
        add_stress_plugins(&mut core, 0).unwrap();
        core.wait_plugins();
        settle(&mut core);
        let keys: Vec<DeviceKey> = core
            .doc()
            .plugin_devices()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let before: Vec<(DeviceKey, atomik_plugin_host::InstanceId, usize)> = keys
            .into_iter()
            .map(|k| {
                let id = core.plugin_instance(&k).expect("live");
                let native = core.plugin_host().manager_ref().native_handle(id).unwrap();
                (k, id, native)
            })
            .collect();
        assert_eq!(before.len(), 16);
        let params_before: Vec<_> = core
            .doc()
            .plugin_devices()
            .into_iter()
            .map(|(k, d)| (k, d.params.clone()))
            .collect();
        core.rebuild_engine();
        assert!(
            core.plugins_ready(),
            "no instance is queued: all were reattached at once"
        );
        settle(&mut core);
        assert_eq!(core.plugin_instance_count(), 16, "none created, none lost");
        for (key, id, native) in before {
            assert_eq!(core.plugin_instance(&key), Some(id), "{key}");
            assert_eq!(
                core.plugin_host().manager_ref().native_handle(id),
                Some(native),
                "{key} is the same plugin instance"
            );
        }
        let params_after: Vec<_> = core
            .doc()
            .plugin_devices()
            .into_iter()
            .map(|(k, d)| (k, d.params.clone()))
            .collect();
        assert_eq!(
            params_before, params_after,
            "ids, params and state are kept"
        );
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(48_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(
            out[0].iter().any(|x| x.abs() > 1e-3),
            "it plays after the restart"
        );
    }

    /// Adding and removing plugins 200 times leaves no instance, descriptor or thread
    /// behind (the manager's table and the interned descriptor are checked).
    #[test]
    fn two_hundred_adds_and_removes_leave_nothing_behind() {
        use crate::offline_sink::OfflineSink;
        use crate::plugin_test_support::{GAIN, PKG, SYNTH, add_command, config};
        let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
        core.configure_plugins(config(true, &[PKG]));
        let base = core.plugin_instance_count();
        let mut descriptor = None;
        for round in 0..200 {
            let key = DeviceKey(format!("r{round}"));
            let type_id = if round % 2 == 0 { GAIN } else { SYNTH };
            let add = add_command(&core, type_id, &key.0);
            core.apply_command(add).unwrap();
            core.wait_plugins();
            assert_eq!(core.plugin_instance_count(), base + 1, "round {round}");
            let d = core.registry().get(type_id).unwrap().descriptor as *const _ as usize;
            if round < 2 {
                descriptor = descriptor.or(Some(d));
            }
            core.apply_command(Command::RemoveDevice { key }).unwrap();
            for _ in 0..3 {
                core.sink_mut().render(128);
                core.frame();
            }
            assert_eq!(core.plugin_instance_count(), base, "round {round}");
        }
        assert_eq!(core.plugin_instance_count(), 0);
        assert!(descriptor.is_some());
    }

    #[test]
    fn the_stress_containers_load_combines_with_the_players_and_fx_loads() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_players(&mut core).unwrap();
        add_stress_containers(&mut core).unwrap();
        add_stress_fx(&mut core).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        assert!(core.doc().device(&DeviceKey("fxi8".into())).is_some());
        assert!(core.doc().device(&DeviceKey("pseq4".into())).is_some());
        assert!(core.doc().device(&DeviceKey("cont8".into())).is_some());
        settle(&mut core);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(48_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
    }

    /// `--stress-character` adds 8 containers, each a poly synth into a
    /// Character whose preset steps every bar through all 16 presets (the long latency class
    /// included), so a preset change that crosses the classes is under load all the time.
    #[test]
    fn the_stress_character_load_builds_8_character_containers_whose_presets_cycle_through_all_16()
    {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        let before = core.doc().devices.len();
        let channels = core.doc().console.channels.len();
        let tracks = core.doc().song.tracks.len();
        add_stress_character(&mut core).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        let doc = core.doc();
        assert_eq!(
            doc.devices.len(),
            before + STRESS_CHARACTERS + STRESS_CHARACTERS,
            "8 containers and their channels"
        );
        assert_eq!(doc.console.channels.len(), channels + STRESS_CHARACTERS);
        assert_eq!(doc.song.tracks.len(), tracks + STRESS_CHARACTERS);
        assert_eq!(doc.inner_device_count(), STRESS_CHARACTERS * 2);
        let mut seen = std::collections::BTreeSet::new();
        for n in 1..=STRESS_CHARACTERS {
            let c = DeviceKey(format!("chr{n}"));
            let inner = doc.inner(&c).unwrap();
            let types: Vec<&str> = inner.devices.iter().map(|d| d.type_id.as_str()).collect();
            assert_eq!(types, ["atomik.poly-synth", "atomik.character"]);
            assert_eq!(inner.macros[0].name, "Preset");
            assert_eq!(inner.macros[0].targets.len(), 1);
            assert_eq!(inner.macros[1].name, "Transform");
            // A little Input Gain drive and a wet-leaning mix, so the saturating presets work.
            let fx = inner
                .devices
                .iter()
                .find(|d| d.type_id == "atomik.character")
                .unwrap();
            assert_eq!(fx.params["gain"], 0.6, "container {n}");
            assert_eq!(fx.params["mix"], 0.85, "container {n}");
            let track = doc.song.track(&c).unwrap();
            assert_eq!(track.lanes.len(), 3, "notes + macro1 + macro2");
            let ClipContent::Automation(points) = &track.lanes[1].clips[0].content else {
                panic!("macro1 is an automation lane");
            };
            assert_eq!(points.len(), STRESS_SONG_BARS as usize, "a step per bar");
            for (i, p) in points.iter().enumerate() {
                let preset = (p.value * 15.0).round() as usize;
                assert_eq!(preset, stress_character_preset(n - 1, i), "{n} bar {i}");
                seen.insert(preset);
            }
        }
        assert_eq!(seen.len(), 16, "every preset, lossy included");
        // At every bar the 8 instances are on 8 different presets.
        for bar in 0..STRESS_SONG_BARS as usize {
            let at: std::collections::BTreeSet<usize> = (0..STRESS_CHARACTERS)
                .map(|n| stress_character_preset(n, bar))
                .collect();
            assert_eq!(at.len(), STRESS_CHARACTERS, "bar {bar}");
        }
        assert_eq!(load_project(&save_project(doc), &reg).unwrap(), *doc);
    }

    #[test]
    fn the_stress_character_load_plays_follows_the_latency_classes_and_is_deterministic() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let run = || {
            let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
            add_stress_character(&mut core).unwrap();
            settle(&mut core);
            let epoch = core.sink().latency_epoch();
            core.transport_action(atomik_ui::TransportAction::Play);
            // Eight bars at 120 BPM: containers reach `lossy` and leave it again.
            let out = crate::plugin_test_support::stepped(&mut core, 8 * 96_000);
            (out, core.sink().latency_epoch() - epoch)
        };
        let (out, epochs) = run();
        assert!(out[0].iter().all(|x| x.is_finite()));
        assert!(out[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
        assert!(
            epochs >= 4,
            "the preset changes cross the latency classes under load: {epochs} epoch changes"
        );
        let (twice, again) = run();
        assert_eq!(epochs, again);
        assert!(
            out.iter()
                .flatten()
                .map(|x| x.to_bits())
                .eq(twice.iter().flatten().map(|x| x.to_bits())),
            "two runs are bit-identical"
        );
    }

    #[test]
    fn the_stress_character_load_combines_with_the_other_stress_loads() {
        use crate::offline_sink::OfflineSink;
        let reg = default_registry();
        let mut core = AppCore::with_doc(OfflineSink::new(), stress_song_doc(&reg), None);
        add_stress_players(&mut core).unwrap();
        add_stress_containers(&mut core).unwrap();
        add_stress_character(&mut core).unwrap();
        add_stress_fx(&mut core).unwrap();
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        for k in ["chr1", "chr8", "cont8", "pseq4", "fxi8"] {
            assert!(core.doc().device(&DeviceKey(k.into())).is_some(), "{k}");
        }
        settle(&mut core);
        core.transport_action(atomik_ui::TransportAction::Play);
        let out = core.sink_mut().render(48_000);
        assert!(out[0].iter().all(|x| x.is_finite()));
    }
}
