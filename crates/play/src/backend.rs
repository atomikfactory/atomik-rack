//! The device boundary. [`SystemBackend`] uses `atomik-audio-io` and `atomik-midi-io`; tests
//! inject fakes so the whole lifecycle runs without audio or MIDI hardware.

use atomik_audio_io::{
    HostInfo, IoError, RunningStream, StreamInfo, StreamSpec, Telemetry, XrunEvent,
};
use atomik_device_sdk::SocketId;
use atomik_engine::{DeviceInstanceId, Engine, FrameClock, LiveNoteSender};
use atomik_midi_io::{MidiError, MidiInput};
use std::sync::Arc;

/// A running audio stream that drives the engine.
pub trait LiveStream {
    /// Telemetry since the previous call (loads reset on read; counters are totals).
    fn telemetry(&self) -> Telemetry;
    /// What was opened.
    fn info(&self) -> &StreamInfo;
    /// Stops the stream and hands the engine back (to be dropped on the caller's thread).
    fn stop(self) -> Result<Engine, IoError>;
    /// Xrun events queued since the previous call, oldest first. The default has none.
    fn drain_events(&self) -> Vec<XrunEvent> {
        Vec::new()
    }
}

/// An open MIDI input.
pub trait MidiPort {
    /// Full port name.
    fn port_name(&self) -> &str;
    /// Notes dropped because the live note queue was full.
    fn dropped(&self) -> u64;
}

/// Everything the player needs from the outside world.
pub trait Backend {
    /// Stream type.
    type Stream: LiveStream;
    /// MIDI input type.
    type Midi: MidiPort;
    /// Lists audio hosts and devices.
    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError>;
    /// Lists MIDI input ports.
    fn list_midi(&self) -> Result<Vec<String>, MidiError>;
    /// Opens and starts a stream driving `engine`.
    fn start(
        &self,
        spec: &StreamSpec,
        engine: Engine,
        clock: Arc<FrameClock>,
    ) -> Result<Self::Stream, IoError>;
    /// Opens a MIDI input forwarding notes to `target`.
    fn open_midi(
        &self,
        name: &str,
        target: (DeviceInstanceId, SocketId),
        sender: LiveNoteSender,
        clock: Arc<FrameClock>,
        sample_rate: f64,
        latency_frames: u64,
    ) -> Result<Self::Midi, MidiError>;
}

/// The real devices.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemBackend;

impl LiveStream for RunningStream {
    fn telemetry(&self) -> Telemetry {
        RunningStream::telemetry(self)
    }

    fn info(&self) -> &StreamInfo {
        RunningStream::info(self)
    }

    fn stop(self) -> Result<Engine, IoError> {
        RunningStream::stop(self)
    }

    fn drain_events(&self) -> Vec<XrunEvent> {
        RunningStream::drain_events(self)
    }
}

impl MidiPort for MidiInput {
    fn port_name(&self) -> &str {
        MidiInput::port_name(self)
    }

    fn dropped(&self) -> u64 {
        MidiInput::dropped(self)
    }
}

impl Backend for SystemBackend {
    type Stream = RunningStream;
    type Midi = MidiInput;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        atomik_audio_io::list()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        atomik_midi_io::list_inputs()
    }

    fn start(
        &self,
        spec: &StreamSpec,
        engine: Engine,
        clock: Arc<FrameClock>,
    ) -> Result<RunningStream, IoError> {
        atomik_audio_io::start(spec, engine, clock)
    }

    fn open_midi(
        &self,
        name: &str,
        target: (DeviceInstanceId, SocketId),
        sender: LiveNoteSender,
        clock: Arc<FrameClock>,
        sample_rate: f64,
        latency_frames: u64,
    ) -> Result<MidiInput, MidiError> {
        atomik_midi_io::open_input(name, target, sender, clock, sample_rate, latency_frames)
    }
}
