//! Hardware checks (round trip, stress), gated and ignored by default.
//!
//! Safety (as in `atomik-audio-io`'s `hw_loopback`): the ONLY devices touched are the
//! virtual audio cable endpoints, `CABLE Input` for playback and `CABLE Output` for capture
//! (WASAPI). Every test asserts the opened device names contain "CABLE" before it plays
//! anything; the default device and the main audio interface are never opened here.
//!
//! The numbers are informative: virtual cable endpoints may run at mismatched rates and xrun, so nothing asserts zero xruns.
//!
//! Run (each prints a result line):
//!
//! ```text
//! ATOMIK_HW_TESTS=1 cargo test -p atomik-app --test hw_e5 -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `ATOMIK_HW_SECONDS` sets the play time (default 20).

use atomik_app::AppCore;
use atomik_app::EngineSink;
use atomik_app::app_core::default_registry;
use atomik_app::session::{LiveSession, SystemBackend};
use atomik_app::settings::{AudioSettings, HostChoice};
use atomik_app::stress::{
    STRESS_AUDIO_THRESHOLD_BYTES, stress_audio_dir, stress_song_doc, write_stress_audio,
};
use atomik_audio_file::{AudioData, Bundle, write_wav_f32};
use atomik_model::{ClipContent, Command, SongCommand, empty_doc};
use atomik_ui::{ImportTarget, TransportAction};
use std::time::{Duration, Instant};

const PLAYBACK: &str = "CABLE Input";
const CAPTURE: &str = "CABLE Output";

fn enabled() -> bool {
    std::env::var("ATOMIK_HW_TESTS").as_deref() == Ok("1")
}

#[allow(clippy::disallowed_methods)] // Justification: hardware test timing.
fn now() -> Instant {
    Instant::now()
}

fn cable_settings(buffer: u32, _input: bool) -> AudioSettings {
    assert!(PLAYBACK.contains("CABLE") && CAPTURE.contains("CABLE"));
    AudioSettings {
        host: HostChoice::Wasapi,
        output_device: Some(PLAYBACK.into()),
        input_device: Some(CAPTURE.into()),
        buffer_frames: buffer,
        midi_input: None,
        workers: 2,
        stream_threshold_bytes: Some(STRESS_AUDIO_THRESHOLD_BYTES),
        ..AudioSettings::default()
    }
}

fn pump(core: &mut AppCore<LiveSession<SystemBackend>>, seconds: f64) -> (u64, f32, u64) {
    let end = now() + Duration::from_secs_f64(seconds);
    let (mut peak, mut xruns, mut underruns) = (0f32, 0, 0);
    while now() < end {
        core.frame();
        let status = core.sink_mut().poll().clone();
        assert!(status.device.contains("CABLE"), "{}", status.device);
        peak = peak.max(status.dsp_peak);
        xruns = status.xruns;
        underruns = status.stream_underruns;
        std::thread::sleep(Duration::from_millis(16));
    }
    (xruns, peak, underruns)
}

/// Support: 16 audio tracks (11 streamed) plus the 8 instrument tracks of the song on
/// the cable at 128 frames.
#[test]
#[ignore = "hardware: plays on the virtual audio cable"]
fn e5_3_stress_on_the_cable() {
    if !enabled() {
        return;
    }
    let seconds: f64 = std::env::var("ATOMIK_HW_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20.0);
    let reg = default_registry();
    let session = LiveSession::new(SystemBackend, cable_settings(128, false), true);
    let mut core = AppCore::with_doc(session, stress_song_doc(&reg), None);
    let paths = write_stress_audio(&stress_audio_dir(), core.doc().sample_rate).unwrap();
    core.import_audio(paths, ImportTarget::NewTracks, 0);
    core.wait_background();
    let streamed = core
        .song_assets()
        .values()
        .filter(|a| a.is_stream())
        .count();
    let status = core.sink_mut().poll().clone();
    println!("opened: {status:?}");
    assert!(status.running, "the cable did not open: {:?}", status.error);
    assert!(status.device.contains("CABLE"));
    core.transport_action(TransportAction::Play);
    let (xruns, peak, underruns) = pump(&mut core, seconds);
    let status = core.sink_mut().poll().clone();
    println!(
        "E5-3 cable: {} tracks ({} audio, {streamed} streamed), buffer {:?}, {seconds} s, xruns {xruns}, DSP peak {:.1} %, stream underruns {underruns}",
        core.doc().song.tracks.len(),
        core.song_assets().len(),
        status.buffer_frames,
        peak * 100.0,
    );
    core.transport_action(TransportAction::Stop);
}

/// Support: a click track plays out of `CABLE Input`; an armed audio track records it back
/// from `CABLE Output`. Reports where the recorded click lands against where it was played (0 =
/// the compensation is exact).
#[test]
#[ignore = "hardware: plays on the virtual audio cable"]
fn e5_2_loopback_recording_on_the_cable() {
    if !enabled() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("atomik-hw-e5-2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A 1 kHz burst (10 ms, 0.5) every half second from 0.5 s, 4 s long, stereo.
    // CABLE Output only runs at 44.1 kHz: run the whole session at that rate.
    let rate = 44_100usize;
    let mut left = vec![0.0f32; 4 * rate];
    for k in 1..8 {
        for i in 0..rate / 100 {
            left[k * rate / 2 + i] =
                0.5 * (std::f32::consts::TAU * 1_000.0 * i as f32 / rate as f32).sin();
        }
    }
    let click = dir.join("click.wav");
    write_wav_f32(
        &click,
        &AudioData::new(rate as u32, vec![left.clone(), left]).unwrap(),
    )
    .unwrap();

    let mut doc = empty_doc();
    doc.input_channels = 2;
    doc.sample_rate = rate as u32;
    let session = LiveSession::new(SystemBackend, cable_settings(256, true), true);
    let mut core = AppCore::with_doc(session, doc, None);
    core.import_audio(vec![click], ImportTarget::NewTracks, 0);
    core.wait_background();
    let status = core.sink_mut().poll().clone();
    println!("opened: {status:?}");
    assert!(status.running, "the cable did not open: {:?}", status.error);
    assert!(status.device.contains("CABLE"));
    if status.input_channels_open == 0 {
        println!(
            "E5-2 cable: the capture side did not open ({:?}); nothing to record",
            status.input_error
        );
        return;
    }
    assert!(
        status
            .input_device
            .as_deref()
            .is_some_and(|d| d.contains("CABLE"))
    );
    core.apply_command(Command::Song(SongCommand::AddAudioTrack {
        name: Some("rec".into()),
    }))
    .unwrap();
    let rec = core.doc().song.tracks.last().unwrap().device.clone();
    // No input monitoring: the capture is the cable's own output, monitoring would feed back.
    core.apply_command(Command::SetParam {
        key: rec.clone(),
        param: "monitor".into(),
        value: 0.0,
        merge: None,
    })
    .unwrap();
    core.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: rec.clone(),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 5.0);
    let status = core.sink_mut().poll().clone();
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 0.6);
    core.wait_background();
    let clips = &core.doc().song.track(&rec).unwrap().lanes[0].clips;
    println!(
        "latency in {} out {} frames, compensation {} frames, message {:?}",
        status.input_latency_frames,
        status.output_latency_frames,
        core.sink().record_offset_frames(),
        core.last_rejection()
    );
    let Some(clip) = clips.first() else {
        println!("E5-2 cable: no clip was recorded");
        return;
    };
    let ClipContent::Audio(a) = &clip.content else {
        panic!("audio clip");
    };
    let bundle = Bundle::open(core.bundle_root().unwrap()).unwrap();
    let data = bundle.load(&a.asset).unwrap();
    let onset = data.channels[0]
        .iter()
        .position(|s| s.abs() > 0.1)
        .expect("a click was recorded");
    let song_frame =
        core.doc().song.frame_at(clip.start, rate as u32) + onset as u64 - a.source_offset;
    let expected = (rate / 2) as i64;
    let error = song_frame as i64 - expected;
    println!(
        "E5-2 cable: first click at song frame {song_frame}, played at {expected}: error {error} frames ({:.2} ms)",
        error as f64 * 1000.0 / rate as f64
    );
    let _ = std::fs::remove_dir_all(&dir);
}
