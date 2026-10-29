//! (hardware, gated): virtual audio cable loopback.
//!
//! Every test returns early (passes) unless `ATOMIK_HW_TESTS=1`. Safety rules:
//! - the ONLY devices touched are the virtual audio cable endpoints: playback on "CABLE Input"
//!   through the WASAPI host, capture on "CABLE Output" with a plain cpal input stream;
//! - the device-name filters are hard-coded and asserted to contain "CABLE" before anything
//!   is opened; the default device and the main audio interface are never opened;
//! - the test signal is an impulse of 0.1 (−20 dBFS): `ImpulseOnNote` scaled by
//!   `Passthrough` gain 0.1, triggered by scheduled notes.
//!
//! Run: `ATOMIK_HW_TESTS=1 cargo test -p atomik-audio-io --test hw_loopback -- --nocapture`

use atomik_audio_io::{HostKind, StreamSpec, list, start};
use atomik_core::{SampleRate, Tempo};
use atomik_engine::{Endpoint, EngineConfig, FrameClock, Graph, ScheduledNote, new_engine};
use atomik_testkit::fakes::{ImpulseOnNote, Passthrough};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Playback endpoint of the virtual cable (WASAPI).
const PLAYBACK: &str = "CABLE Input";
/// Capture endpoint of the virtual cable (WASAPI).
const CAPTURE: &str = "CABLE Output";
/// Test level: −20 dBFS.
const LEVEL: f32 = 0.1;
/// Impulses played, 0.5 s apart, starting 0.5 s in.
const IMPULSES: u64 = 3;

fn hw_enabled() -> bool {
    std::env::var("ATOMIK_HW_TESTS").as_deref() == Ok("1")
}

#[allow(clippy::disallowed_methods)] // Justification: hardware test timing (capture timestamps).
fn now() -> Instant {
    Instant::now()
}

fn assert_cable(name: &str) {
    assert!(
        name.contains("CABLE"),
        "refusing to touch a non-virtual cable device: {name:?}"
    );
}

#[test]
fn hw_list_shows_hosts_devices_and_the_cable() {
    if !hw_enabled() {
        return;
    }
    let hosts = list().expect("list");
    for host in &hosts {
        println!("host {:?}", host.kind);
        for d in &host.devices {
            println!(
                "  {:?} in={} out={} rates={:?} buffer={:?} default_out={}",
                d.name,
                d.max_inputs,
                d.max_outputs,
                d.sample_rates,
                d.buffer_range,
                d.is_default_output
            );
        }
    }
    assert!(
        hosts
            .iter()
            .flat_map(|h| &h.devices)
            .any(|d| d.name.contains(PLAYBACK)),
        "virtual audio cable not listed"
    );
}

/// One capture callback: when it ran, and its interleaved samples.
type Captured = Vec<(Instant, Vec<f32>)>;

#[test]
fn hw_vb_cable_impulse_round_trip() {
    if !hw_enabled() {
        return;
    }
    assert_cable(PLAYBACK);
    assert_cable(CAPTURE);

    // --- Capture side: a plain cpal input stream on "CABLE Output" (WASAPI). ---
    let host = cpal::host_from_id(
        cpal::available_hosts()
            .into_iter()
            .find(|id| id.name().eq_ignore_ascii_case("WASAPI"))
            .expect("WASAPI host"),
    )
    .expect("WASAPI");
    let capture_dev = host
        .input_devices()
        .expect("input devices")
        .find(|d| d.to_string().contains(CAPTURE))
        .expect("CABLE Output capture device");
    assert_cable(&capture_dev.to_string());
    let capture_cfg = capture_dev.default_input_config().expect("capture config");
    assert_eq!(
        capture_cfg.sample_format(),
        cpal::SampleFormat::F32,
        "cable capture is f32 in shared mode"
    );
    let cap_sr = capture_cfg.sample_rate();
    let cap_ch = usize::from(capture_cfg.channels());
    println!("capture: {capture_dev} @ {cap_sr} Hz, {cap_ch} ch");

    // The engine runs at the playback endpoint's native (mix-format) rate so that WASAPI does
    // not resample our stream. This only queries the format; nothing is opened here.
    let playback_dev = host
        .output_devices()
        .expect("output devices")
        .find(|d| d.to_string().contains(PLAYBACK))
        .expect("CABLE Input playback device");
    assert_cable(&playback_dev.to_string());
    let sr = playback_dev
        .default_output_config()
        .expect("playback config")
        .sample_rate();
    println!("playback endpoint native rate: {sr} Hz");
    if sr != cap_sr {
        println!(
            "note: the cable endpoints run at different rates ({sr} vs {cap_sr} Hz); Windows resamples between them, so the impulse arrives band-limited (spread over a few samples)"
        );
    }

    let captured: Arc<Mutex<Captured>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    let capture = capture_dev
        .build_input_stream(
            capture_cfg.config(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let at = now();
                sink.lock().unwrap().push((at, data.to_vec()));
            },
            |e| eprintln!("capture error: {e}"),
            Some(Duration::from_secs(5)),
        )
        .expect("capture stream");
    capture.play().expect("capture play");

    // --- Playback side: the engine through atomik-audio-io on "CABLE Input". ---
    let cfg = EngineConfig {
        sample_rate: SampleRate::new(f64::from(sr)).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let mut g = Graph::new(&cfg);
    let imp = g.add_device(ImpulseOnNote::boxed());
    let gain = g.add_device(Passthrough::boxed(LEVEL));
    let out_1 = g
        .descriptor(Graph::INTERFACE)
        .unwrap()
        .socket_by_key("out_1")
        .unwrap()
        .id;
    g.connect(
        Endpoint {
            device: imp,
            socket: ImpulseOnNote::AUDIO_OUT,
        },
        Endpoint {
            device: gain,
            socket: Passthrough::AUDIO_IN,
        },
    )
    .unwrap();
    g.connect(
        Endpoint {
            device: gain,
            socket: Passthrough::AUDIO_OUT,
        },
        Endpoint {
            device: Graph::INTERFACE,
            socket: out_1,
        },
    )
    .unwrap();
    let (engine, mut handle) = new_engine(cfg);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let note_frames: Vec<u64> = (1..=IMPULSES).map(|k| u64::from(sr) * k / 2).collect();
    for &frame in &note_frames {
        handle
            .send_note(ScheduledNote {
                frame,
                device: imp,
                socket: ImpulseOnNote::NOTES_IN,
                kind: atomik_core::NoteKind::On {
                    key: 60,
                    velocity: 1.0,
                },
            })
            .unwrap();
    }
    let latency = u64::from(engine.latency_frames());

    let spec = StreamSpec {
        host: HostKind::Wasapi,
        device: Some(PLAYBACK.to_string()),
        sample_rate: sr,
        buffer_frames: 480,
        input_channels: 0,
        output_channels: 2,
    };
    assert_cable(spec.device.as_deref().unwrap());
    let clock = Arc::new(FrameClock::new());
    let stream = start(&spec, engine, Arc::clone(&clock)).expect("start on CABLE Input");
    assert_cable(&stream.info().output_device);
    println!("playback: {:?}", stream.info());

    std::thread::sleep(Duration::from_millis(500 * (IMPULSES + 1) + 300));
    let clock_sample = clock
        .read()
        .expect("the audio callback published the clock");
    let telemetry = stream.telemetry();
    println!(
        "telemetry: {telemetry:?}, realtime priority: {:?}",
        stream.realtime_priority()
    );
    // Every xrun of the snapshot produced an event (or a counted drop); more may have
    // arrived since the snapshot.
    let events = stream.drain_events();
    for e in &events {
        println!("xrun event: {e:?}");
    }
    assert!(events.len() as u64 + telemetry.events_dropped >= telemetry.xruns);
    assert_eq!(
        telemetry.xruns,
        telemetry.late_callbacks + telemetry.render_overruns + telemetry.stream_errors
    );
    let engine = stream.stop().unwrap();
    drop(capture);
    drop(engine);

    // --- Analysis: channel 0 as one signal with a capture instant per sample. ---
    let captured = captured.lock().unwrap();
    let cap_sr_f = f64::from(cap_sr);
    let mut signal: Vec<(Instant, f32)> = Vec::new();
    for (at, data) in captured.iter() {
        let frames = data.len() / cap_ch;
        for (i, x) in data.iter().step_by(cap_ch).enumerate() {
            let back = Duration::from_secs_f64((frames - 1 - i) as f64 / cap_sr_f);
            signal.push((*at - back, *x));
        }
    }
    // An impulse is a cluster above LEVEL/4; its peak is the largest sample of the cluster.
    let window = usize::try_from(cap_sr / 10).unwrap();
    let mut hits: Vec<(Instant, f32, usize)> = Vec::new(); // (peak arrival, peak, sample index)
    let mut i = 0;
    while i < signal.len() {
        if signal[i].1.abs() > LEVEL / 4.0 {
            let end = (i + 64).min(signal.len());
            let (k, peak) = (i..end)
                .map(|k| (k, signal[k].1.abs()))
                .fold((i, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
            hits.push((signal[k].0, peak, k));
            i += window;
        } else {
            i += 1;
        }
    }
    let max_abs = signal.iter().map(|s| s.1.abs()).fold(0.0f32, f32::max);
    println!(
        "impulses captured: {} (peaks {:?}); max |sample| {max_abs}",
        hits.len(),
        hits.iter().map(|h| h.1).collect::<Vec<_>>()
    );
    assert_eq!(
        hits.len() as u64,
        IMPULSES,
        "every impulse arrives exactly once"
    );
    assert!(
        max_abs <= LEVEL * 1.1,
        "never louder than -20 dBFS: {max_abs}"
    );

    let (h, t_h) = clock_sample;
    let sr_f = f64::from(sr);
    for (k, ((arrival, peak, index), &frame)) in hits.iter().zip(&note_frames).enumerate() {
        assert!(*peak > LEVEL * 0.5, "impulse {k} too weak: {peak}");
        // Host frame `frame + latency` was written by the callback that published host frame
        // h at t_h; extrapolate its write time along the published clock.
        let offset_s = (frame as f64 + latency as f64 - h as f64) / sr_f;
        let emitted = if offset_s >= 0.0 {
            t_h + Duration::from_secs_f64(offset_s)
        } else {
            t_h - Duration::from_secs_f64(-offset_s)
        };
        let lat = arrival.saturating_duration_since(emitted).as_secs_f64();
        println!(
            "impulse {k}: peak {peak:.4} ({:.1} dBFS), round-trip latency {:.0} frames @ {sr} Hz = {:.2} ms (callback write -> capture callback)",
            20.0 * f64::from(*peak).log10(),
            lat * sr_f,
            lat * 1000.0
        );
        if k > 0 {
            println!(
                "  spacing to previous: {} capture samples (expected {:.1})",
                index - hits[k - 1].2,
                cap_sr_f / 2.0
            );
        }
    }
    assert!(telemetry.callbacks > 0);
}
