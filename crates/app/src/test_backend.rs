//! The app's side of the shared fake audio backend (`atomik_testkit::backend`): the traits of
//! [`crate::session`] implemented for it, once, for every test module that fakes the audio host.
//!
//! A test that only needs audio (no MIDI port to play into) passes a [`FakeAudio`] to
//! `LiveSession::new` as it is; one that needs MIDI wraps it in its own backend and answers
//! `list_audio`, `list_midi` and `start` with the calls of the same names.

use crate::midi::{MidiConnection, MidiLink};
use crate::session::{AudioBackend, LiveStream};
use atomik_audio_io::{HostInfo, IoError, StreamInfo, StreamSpec, Telemetry};
use atomik_engine::{Engine, FrameClock};
use atomik_midi_io::MidiError;
use std::sync::Arc;

pub(crate) use atomik_testkit::backend::{AcceptedBuffer, FakeAudio, FakeStream};

impl LiveStream for FakeStream {
    fn telemetry(&self) -> Telemetry {
        FakeStream::telemetry(self)
    }
    fn info(&self) -> &StreamInfo {
        FakeStream::info(self)
    }
    fn stop(self) -> Result<Engine, IoError> {
        FakeStream::stop(self)
    }
}

/// Audio only: no MIDI port exists (opening one fails as a port that is not there).
impl AudioBackend for FakeAudio {
    type Stream = FakeStream;
    type Midi = MidiConnection;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        FakeAudio::list_audio(self)
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(FakeAudio::list_midi(self))
    }

    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<FakeStream, IoError> {
        FakeAudio::start(self, spec, input_device, engine)
    }

    fn open_midi(&self, name: &str, _link: MidiLink) -> Result<MidiConnection, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
}
