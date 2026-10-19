//! `start` argument validation. None of these tests opens an audio device: they fail
//! before any stream is built (host selection, channel validation, device-name matching).

use atomik_audio_io::{HostKind, IoError, StreamSpec, start};
use atomik_core::{SampleRate, Tempo};
use atomik_engine::{EngineConfig, FrameClock, new_engine};
use std::sync::Arc;

atomik_testkit::install_alloc_guard!();

fn engine() -> atomik_engine::Engine {
    new_engine(EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    })
    .0
}

fn spec(host: HostKind, device: &str) -> StreamSpec {
    StreamSpec {
        host,
        device: Some(device.to_string()),
        sample_rate: 48_000,
        buffer_frames: 256,
        input_channels: 0,
        output_channels: 2,
    }
}

const NO_SUCH_DEVICE: &str = "atomik-no-such-device-7f3a9c";

#[cfg(not(feature = "asio"))]
#[test]
fn start_asio_without_feature_returns_host_unavailable() {
    let err = start(
        &spec(HostKind::Asio, NO_SUCH_DEVICE),
        engine(),
        Arc::new(FrameClock::new()),
    )
    .unwrap_err();
    assert!(matches!(err, IoError::HostUnavailable(_)), "{err:?}");
}

#[cfg(windows)]
#[test]
fn start_host_of_another_platform_returns_host_unavailable() {
    let err = start(
        &spec(HostKind::CoreAudio, NO_SUCH_DEVICE),
        engine(),
        Arc::new(FrameClock::new()),
    )
    .unwrap_err();
    assert!(matches!(err, IoError::HostUnavailable(_)), "{err:?}");
}

#[test]
fn start_zero_output_channels_returns_unsupported_config() {
    let mut s = spec(HostKind::Default, NO_SUCH_DEVICE);
    s.output_channels = 0;
    let err = start(&s, engine(), Arc::new(FrameClock::new())).unwrap_err();
    assert!(matches!(err, IoError::UnsupportedConfig(_)), "{err:?}");
}

#[test]
fn start_unknown_device_returns_device_not_found() {
    let err = start(
        &spec(HostKind::Default, NO_SUCH_DEVICE),
        engine(),
        Arc::new(FrameClock::new()),
    )
    .unwrap_err();
    assert!(
        matches!(err, IoError::DeviceNotFound(_) | IoError::Backend(_)),
        "{err:?}"
    );
}
