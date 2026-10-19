//! The status bar text: host, device, buffer, DSP load, xruns and MIDI.

/// What the status bar shows about the live session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioStatus {
    /// Host display name (e.g. `WASAPI`).
    pub host: String,
    /// Output device name; empty when audio is off.
    pub device: String,
    /// Stream sample rate in Hz.
    pub sample_rate: u32,
    /// Actual buffer size, once the host reported it.
    pub buffer_frames: Option<u32>,
    /// Average DSP load (1.0 = 100 %) over the last reading.
    pub dsp_load: f32,
    /// Peak DSP load over the last reading.
    pub dsp_peak: f32,
    /// Xruns since the stream started.
    pub xruns: u64,
    /// Whether a stream is running.
    pub running: bool,
    /// Why audio is off (open failure), if it is.
    pub error: Option<String>,
    /// MIDI input port name, when one is open.
    pub midi_port: Option<String>,
    /// Why MIDI is off (open failure), if it is.
    pub midi_error: Option<String>,
    /// Name of the device live notes go to.
    pub notes_target: Option<String>,
    /// Input channels the document asks for (`RackDoc::input_channels`).
    pub input_channels_wanted: u16,
    /// Input channels the host opened (0 when the input is off).
    pub input_channels_open: u16,
    /// The capture device, when input is open.
    pub input_device: Option<String>,
    /// Why the input is off although the document wants it (the session fell back to
    /// output-only), if it is.
    pub input_error: Option<String>,
    /// Smoothed input latency in frames (device plus the input ring), 0 until measured.
    pub input_latency_frames: u32,
    /// Smoothed output latency in frames, 0 until measured.
    pub output_latency_frames: u32,
    /// Streamed audio the engine could not deliver in time (frames of silence events) since
    /// the stream started.
    pub stream_underruns: u64,
    /// Whether the project has streamed (disk) assets: shows the underrun counter.
    pub streaming: bool,
}

/// One line of status text, e.g.
/// `WASAPI · Virtual Cable Input · 48 kHz · buffer 480 · DSP 3% (peak 5%) · xruns 0 · MIDI Example Keyboard → Bass`.
pub fn status_text(s: &AudioStatus) -> String {
    let mut parts = vec![s.host.clone()];
    if s.running {
        parts.push(s.device.clone());
        let khz = f64::from(s.sample_rate) / 1000.0;
        parts.push(if s.sample_rate.is_multiple_of(1000) {
            format!("{khz:.0} kHz")
        } else {
            format!("{khz:.1} kHz")
        });
        parts.push(match s.buffer_frames {
            Some(n) => format!("buffer {n}"),
            None => "buffer \u{2026}".to_string(),
        });
        parts.push(format!(
            "DSP {:.0}% (peak {:.0}%)",
            s.dsp_load * 100.0,
            s.dsp_peak * 100.0
        ));
        parts.push(format!("xruns {}", s.xruns));
        if s.streaming || s.stream_underruns > 0 {
            parts.push(format!("underruns {}", s.stream_underruns));
        }
        if s.input_channels_wanted > 0 {
            parts.push(input_text(s));
        }
    } else {
        parts.push(match &s.error {
            Some(e) => format!("audio off: {e}"),
            None => "audio off".to_string(),
        });
    }
    parts.push(match (&s.midi_port, &s.midi_error) {
        (Some(port), _) => format!(
            "MIDI {port} \u{2192} {}",
            s.notes_target.as_deref().unwrap_or("(no notes input)")
        ),
        (None, Some(e)) => format!("MIDI error: {e}"),
        (None, None) => "MIDI off".to_string(),
    });
    parts.join(" \u{b7} ")
}

/// The settings dialog's note about the input the running stream opened.
pub fn input_note(s: &AudioStatus) -> Option<String> {
    if !s.running {
        return None;
    }
    let (open, wanted) = (s.input_channels_open, s.input_channels_wanted);
    Some(match (&s.input_error, open, wanted) {
        (_, _, 0) => {
            "Input: the document has no input channels (see Rack > Input channels).".into()
        }
        (Some(e), _, _) => format!("Input off ({e}); recording reads silence."),
        (None, 0, _) => "Input off; recording reads silence.".into(),
        (None, n, w) if n >= w => format!(
            "Input: {n} channels open ({}).",
            s.input_device.as_deref().unwrap_or("same as output")
        ),
        (None, n, w) => format!(
            "Input: {n} of {w} channels open ({}); the rest read silence.",
            s.input_device.as_deref().unwrap_or("same as output")
        ),
    })
}

/// The input part of the status: channels opened, or why the input is off, plus the I/O latency
/// in milliseconds once measured.
fn input_text(s: &AudioStatus) -> String {
    let (open, wanted) = (s.input_channels_open, s.input_channels_wanted);
    let mut text = match (&s.input_error, open) {
        (Some(e), _) => format!("input off: {e}"),
        (None, 0) => "input off".to_string(),
        (None, n) if n >= wanted => format!("in {n} ch"),
        (None, n) => format!("in {n} of {wanted} ch (the rest is silent)"),
    };
    if open > 0 && s.sample_rate > 0 && (s.input_latency_frames > 0 || s.output_latency_frames > 0)
    {
        let ms = |f: u32| f64::from(f) * 1000.0 / f64::from(s.sample_rate);
        text.push_str(&format!(
            " (latency in {:.1} + out {:.1} ms)",
            ms(s.input_latency_frames),
            ms(s.output_latency_frames)
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> AudioStatus {
        AudioStatus {
            host: "WASAPI".into(),
            device: "Virtual Cable Input".into(),
            sample_rate: 48_000,
            buffer_frames: Some(480),
            dsp_load: 0.031,
            dsp_peak: 0.054,
            xruns: 2,
            running: true,
            error: None,
            midi_port: Some("Example Keyboard".into()),
            midi_error: None,
            notes_target: Some("Bass".into()),
            ..AudioStatus::default()
        }
    }

    #[test]
    fn a_running_session_shows_every_field() {
        assert_eq!(
            status_text(&running()),
            "WASAPI \u{b7} Virtual Cable Input \u{b7} 48 kHz \u{b7} buffer 480 \
             \u{b7} DSP 3% (peak 5%) \u{b7} xruns 2 \u{b7} MIDI Example Keyboard \u{2192} Bass"
        );
    }

    #[test]
    fn fractional_rates_and_unknown_buffers() {
        let s = AudioStatus {
            sample_rate: 44_100,
            buffer_frames: None,
            midi_port: None,
            notes_target: None,
            ..running()
        };
        assert_eq!(
            status_text(&s),
            "WASAPI \u{b7} Virtual Cable Input \u{b7} 44.1 kHz \u{b7} buffer \u{2026} \
             \u{b7} DSP 3% (peak 5%) \u{b7} xruns 2 \u{b7} MIDI off"
        );
    }

    #[test]
    fn the_input_note_and_the_streaming_counter() {
        let s = AudioStatus {
            input_channels_wanted: 2,
            input_channels_open: 1,
            input_device: Some("Line".into()),
            ..running()
        };
        let note = input_note(&s).unwrap();
        assert!(note.contains("1 of 2 channels open (Line)"), "{note}");
        assert!(input_note(&AudioStatus::default()).is_none());
        let both = AudioStatus {
            input_channels_wanted: 2,
            input_channels_open: 2,
            input_latency_frames: 480,
            output_latency_frames: 960,
            streaming: true,
            stream_underruns: 3,
            ..running()
        };
        let text = status_text(&both);
        assert!(text.contains("underruns 3"), "{text}");
        assert!(
            text.contains("in 2 ch (latency in 10.0 + out 20.0 ms)"),
            "{text}"
        );
        assert!(!status_text(&running()).contains("underruns"));
    }

    #[test]
    fn midi_errors_and_missing_targets_are_shown() {
        let s = AudioStatus {
            midi_port: None,
            midi_error: Some("no input matches \"P6\"".into()),
            ..running()
        };
        assert!(status_text(&s).ends_with("MIDI error: no input matches \"P6\""));
        let s = AudioStatus {
            notes_target: None,
            ..running()
        };
        assert!(status_text(&s).ends_with("MIDI Example Keyboard \u{2192} (no notes input)"));
    }

    #[test]
    fn audio_off_shows_the_reason() {
        let s = AudioStatus {
            host: "ASIO".into(),
            running: false,
            error: Some("audio device not found or ambiguous: X".into()),
            midi_port: None,
            ..AudioStatus::default()
        };
        assert_eq!(
            status_text(&s),
            "ASIO \u{b7} audio off: audio device not found or ambiguous: X \u{b7} MIDI off"
        );
        let s = AudioStatus {
            host: "WASAPI".into(),
            ..AudioStatus::default()
        };
        assert_eq!(status_text(&s), "WASAPI \u{b7} audio off \u{b7} MIDI off");
    }
}
