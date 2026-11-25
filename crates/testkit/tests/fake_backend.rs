//! The scriptable fake audio backend: what each knob does, with simulated time only.

use atomik_audio_io::{HostKind, IoError, StreamSpec, Telemetry};
use atomik_core::{SampleRate, Tempo};
use atomik_engine::{Engine, EngineConfig, new_engine};
use atomik_testkit::backend::{AcceptedBuffer, FakeAudio};
use std::time::Duration;

fn engine() -> Engine {
    new_engine(EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 0,
    })
    .0
}

fn spec(device: Option<&str>, rate: u32, buffer: u32) -> StreamSpec {
    StreamSpec {
        host: HostKind::Wasapi,
        device: device.map(str::to_owned),
        sample_rate: rate,
        buffer_frames: buffer,
        input_channels: 0,
        output_channels: 2,
    }
}

#[test]
fn start_stop_and_list_calls_are_counted() {
    let fake = FakeAudio::new();
    fake.plug("Interface");
    assert_eq!((fake.lists(), fake.starts().len(), fake.stops()), (0, 0, 0));
    assert_eq!(fake.list_audio().unwrap()[0].devices.len(), 1);
    fake.set_midi_ports(&["Keys"]);
    assert_eq!(fake.list_midi(), vec!["Keys".to_string()]);
    let a = fake
        .start(&spec(None, 48_000, 256), None, engine())
        .unwrap();
    let b = fake.start(&spec(Some("Interface"), 48_000, 128), None, engine());
    assert!(b.is_ok());
    assert_eq!(fake.open_streams(), 2);
    drop(b);
    assert_eq!(fake.open_streams(), 1, "a dropped stream is not open");
    a.stop().unwrap();
    assert_eq!(
        (
            fake.lists(),
            fake.midi_lists(),
            fake.stops(),
            fake.open_streams()
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(fake.start_calls().len(), 2);
    assert_eq!(fake.starts()[1].buffer_frames, 128);
}

#[test]
fn callbacks_count_only_when_the_test_runs_them() {
    let fake = FakeAudio::new();
    let stream = fake
        .start(&spec(None, 48_000, 480), None, engine())
        .unwrap();
    assert_eq!(stream.telemetry().callbacks, 0);
    fake.run_callbacks(5);
    let t = stream.telemetry();
    assert_eq!((t.callbacks, t.buffer_frames, t.xruns), (5, 480, 0));
    // 480 frames at 48 kHz is 10 ms: a quarter of a second is 25 callbacks.
    fake.advance(Duration::from_millis(250));
    assert_eq!(stream.telemetry().callbacks, 30);
}

#[test]
fn a_stall_swallows_callbacks_for_a_while_and_then_they_come_back() {
    let fake = FakeAudio::new();
    let stream = fake
        .start(&spec(None, 48_000, 480), None, engine())
        .unwrap();
    fake.run_callbacks(3);
    fake.stall();
    fake.run_callbacks(100);
    assert_eq!(stream.telemetry().callbacks, 3);
    fake.resume();
    fake.run_callbacks(2);
    assert_eq!(stream.telemetry().callbacks, 5);
    fake.stall_callbacks(10);
    fake.run_callbacks(15);
    assert_eq!(
        stream.telemetry().callbacks,
        10,
        "ten swallowed, five arrive"
    );
    fake.stall_for(Duration::from_millis(100));
    fake.advance(Duration::from_millis(300));
    assert_eq!(stream.telemetry().callbacks, 30, "100 ms of 300 swallowed");
    assert!(!stream.telemetry().device_lost, "a stall is not a loss");
}

#[test]
fn the_callback_cost_sets_the_load_and_the_overruns() {
    let fake = FakeAudio::new();
    let stream = fake
        .start(&spec(None, 48_000, 480), None, engine())
        .unwrap();
    fake.callback_cost_ns(5_000_000);
    fake.run_callbacks(4);
    let t = stream.telemetry();
    assert!((t.dsp_load_avg - 0.5).abs() < 1e-6, "{}", t.dsp_load_avg);
    assert_eq!((t.xruns, t.render_overruns), (0, 0));
    fake.callback_cost_ns(12_000_000);
    fake.run_callbacks(3);
    let t = stream.telemetry();
    assert!(t.dsp_load_peak > 1.0);
    assert_eq!((t.xruns, t.render_overruns), (3, 3));
    fake.run_callbacks(1);
    assert_eq!(stream.telemetry().xruns, 4, "counters are totals");
    assert!(
        stream.telemetry().dsp_load_avg.abs() < 1e-6,
        "loads reset on read"
    );
}

#[test]
fn unplugging_loses_the_stream_and_the_device_until_it_is_plugged_back() {
    let fake = FakeAudio::new();
    fake.plug("Interface");
    let stream = fake
        .start(&spec(Some("Interface"), 48_000, 480), None, engine())
        .unwrap();
    fake.run_callbacks(2);
    fake.unplug();
    fake.run_callbacks(2);
    let t = stream.telemetry();
    assert!(t.device_lost && t.stream_errors == 1 && t.callbacks == 2);
    assert!(fake.list_audio().unwrap()[0].devices.is_empty());
    let refused = fake.start(&spec(Some("Interface"), 48_000, 480), None, engine());
    assert!(matches!(refused, Err(IoError::DeviceNotFound(_))));
    fake.replug();
    assert_eq!(fake.list_audio().unwrap()[0].devices.len(), 1);
    let again = fake
        .start(&spec(Some("Interface"), 48_000, 480), None, engine())
        .unwrap();
    fake.run_callbacks(1);
    assert!(!again.telemetry().device_lost);
    assert!(stream.telemetry().device_lost, "the old stream stays lost");
}

#[test]
fn a_rate_change_invalidates_the_stream_and_the_old_rate_is_refused() {
    let fake = FakeAudio::new();
    fake.plug("Interface");
    let stream = fake
        .start(&spec(Some("Interface"), 48_000, 480), None, engine())
        .unwrap();
    fake.set_rate(44_100);
    assert!(stream.telemetry().device_lost);
    assert_eq!(
        fake.list_audio().unwrap()[0].devices[0].sample_rates,
        [44_100]
    );
    let refused = fake.start(&spec(Some("Interface"), 48_000, 480), None, engine());
    assert!(matches!(refused, Err(IoError::UnsupportedConfig(_))));
    assert!(
        fake.start(&spec(Some("Interface"), 44_100, 480), None, engine())
            .is_ok()
    );
}

#[test]
fn a_buffer_change_is_reported_from_the_next_callback() {
    let fake = FakeAudio::new();
    fake.accept_buffer(AcceptedBuffer::AsRequested);
    let stream = fake
        .start(&spec(None, 48_000, 256), None, engine())
        .unwrap();
    assert_eq!(stream.info().requested_buffer_frames, Some(256));
    fake.run_callbacks(1);
    assert_eq!(stream.telemetry().buffer_frames, 256);
    fake.set_buffer(64);
    assert_eq!(
        stream.telemetry().buffer_frames,
        256,
        "not before a callback"
    );
    fake.run_callbacks(1);
    assert_eq!(stream.telemetry().buffer_frames, 64);
}

#[test]
fn pinned_telemetry_is_reported_as_it_is() {
    let fake = FakeAudio::new();
    let stream = fake
        .start(&spec(None, 48_000, 256), None, engine())
        .unwrap();
    fake.set_telemetry(Telemetry {
        callbacks: 7,
        ..Telemetry::default()
    });
    fake.run_callbacks(3);
    assert_eq!(stream.telemetry().callbacks, 7);
    assert_eq!(stream.telemetry().callbacks, 7);
}

#[test]
fn scripted_failures_are_counted_as_calls_and_do_not_open() {
    let fake = FakeAudio::new();
    fake.fail_next_start(IoError::Backend("busy".into()));
    assert!(
        fake.start(&spec(None, 48_000, 256), None, engine())
            .is_err()
    );
    assert!(fake.start(&spec(None, 48_000, 256), None, engine()).is_ok());
    assert_eq!((fake.start_calls().len(), fake.starts().len()), (2, 1));
    fake.break_device("Dead");
    assert!(
        fake.start(&spec(Some("Dead"), 48_000, 256), None, engine())
            .is_err()
    );
    assert!(fake.start(&spec(None, 48_000, 256), None, engine()).is_ok());
}
