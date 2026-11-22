//! The status bar text: host, device, buffer, DSP load, xruns and MIDI.

use atomik_audio_io::StreamState;

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
    /// The buffer size the stream was opened with, when the host accepted one (the actual size
    /// of each callback is `buffer_frames`).
    pub buffer_requested: Option<u32>,
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
    /// True once the engine has faulted: a device panicked while rendering and is silent, or
    /// the render callback caught a panic. Sticky until the engine is rebuilt.
    pub faulted: bool,
    /// The engine's instance id of the device that panicked first, when a device did.
    pub faulted_device: Option<u32>,
    /// The faulted device's name, resolved by the shell for the status line (empty when the
    /// fault is outside any device, or the name is unknown).
    pub faulted_name: Option<String>,
    /// Streamed audio the engine could not deliver in time (frames of silence events) since
    /// the stream started.
    pub stream_underruns: u64,
    /// Whether the project has streamed (disk) assets: shows the underrun counter.
    pub streaming: bool,
    /// Work dropped or late on the real-time side; the non-zero counters are shown.
    pub overload: OverloadStats,
    /// Whether the stream still calls back (see [`atomik_audio_io::Liveness`]).
    pub stream_state: StreamState,
    /// Stream error callbacks since the stream started (they are part of `xruns` too).
    pub stream_errors: u64,
    /// How long the callbacks have been silent while the stream is stalled, in milliseconds.
    pub stream_silent_ms: u64,
    /// MIDI clock in and out.
    pub sync: SyncStatus,
    /// Where plugins run (`sandbox` or `in-process`); set while plugins are on.
    pub plugin_mode: Option<&'static str>,
    /// How the render callback trimmed the captured input (see [`InputTrim`]).
    pub input_trim: InputTrim,
    /// The render workers: how many, the linger, and what they have done.
    pub workers: atomik_engine::WorkerStats,
}

/// What the render callback did to the captured input ring, for the diagnostics: the count of
/// trims that discarded frames, the largest backlog before one, and the range of callback sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputTrim {
    /// Trims that discarded captured frames since the stream started.
    pub trims: u64,
    /// The most frames the ring held just before a trim.
    pub backlog_max_frames: u32,
    /// The smallest callback seen, in frames (0 before the first).
    pub callback_min: u32,
    /// The largest callback seen, in frames (0 before the first).
    pub callback_max: u32,
    /// The most input frames the ring may hold beyond one callback's read now (0 without input):
    /// the bound on the latency the ring adds to the input.
    pub allowed_backlog_frames: u32,
    /// The part of that above the base of two buffers (0: nothing was needed).
    pub extra_backlog_frames: u32,
}

/// The one debug line written when the trim count rose, e.g. `input trimmed: 3 trims, backlog max
/// 1440 frames, allowed 1536 frames, callback 96..480 frames, buffer 480`.
pub fn input_trim_line(s: &AudioStatus) -> String {
    let t = &s.input_trim;
    format!(
        "input trimmed: {} trims, backlog max {} frames, allowed {} frames, callback {}..{} frames, buffer {}",
        t.trims,
        t.backlog_max_frames,
        t.allowed_backlog_frames,
        t.callback_min,
        t.callback_max,
        s.buffer_requested
            .map_or("unknown".to_string(), |n| n.to_string())
    )
}

/// What MIDI clock is doing (see [`crate::settings::SyncSettings`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncStatus {
    /// The clock is being sent.
    pub clock_out: bool,
    /// The input port the clock is followed from, while it is open.
    pub clock_in_port: Option<String>,
    /// Why the clock in is off although it is switched on, if it is.
    pub clock_in_error: Option<String>,
    /// The transport follows the clock: its buttons do nothing.
    pub following: bool,
    /// The tempo measured from the clock, in BPM.
    pub external_tempo: Option<f64>,
    /// The sender runs (Start or Continue came, Stop did not).
    pub external_running: bool,
    /// Clocks have come since the last look.
    pub receiving: bool,
}

impl SyncStatus {
    /// The pieces of the status line: nothing when clock is off.
    pub fn parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.clock_out {
            parts.push("clock out".to_string());
        }
        if let Some(e) = &self.clock_in_error {
            parts.push(format!("clock in error: {e}"));
        } else if self.following {
            let run = if self.external_running {
                "running"
            } else {
                "stopped"
            };
            parts.push(match (self.receiving, self.external_tempo) {
                (true, Some(bpm)) => format!("following clock {bpm:.1} BPM ({run})"),
                (false, Some(bpm)) => format!("no clock, last {bpm:.1} BPM"),
                _ => "waiting for clock".to_string(),
            });
        }
        parts
    }
}

/// The counters of work the real-time side had to drop or could not do in time. The audio
/// callback only increments them (atomics); the UI thread reads them a few times a second, shows
/// the non-zero ones in the status bar, and writes the changes into the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OverloadStats {
    /// Note events the engine dropped because a socket's buffer was full, or no target took
    /// them.
    pub dropped_events: u64,
    /// Input frames the capture ring lost because the output callback did not read them in time.
    pub input_overruns: u64,
    /// Frames of the audio recording tap lost because its ring was full (recorded as silence).
    pub tap_overflow_frames: u64,
    /// Events a plugin could not take or give, summed over the plugins in the rack.
    pub plugin_drops: u64,
    /// Control messages lost to a full queue: pushes to the engine (parameters, routing,
    /// transport, ...) it had no room for, and MIDI control messages the inbox dropped.
    pub control_stalled: u64,
    /// Sandbox batches whose result came back after the audio thread needed it, summed over the
    /// sandboxed plugins in the rack.
    pub late_sandbox_batches: u64,
    /// Blocks a plugin answered with an error or non-finite output, replaced by the dry input or
    /// silence, summed over the plugins in the rack.
    pub bad_plugin_blocks: u64,
}

impl OverloadStats {
    /// Every counter with its label, in the order the status bar shows them.
    pub fn fields(&self) -> [(&'static str, u64); 7] {
        [
            ("dropped events", self.dropped_events),
            ("input overruns", self.input_overruns),
            ("record overflow", self.tap_overflow_frames),
            ("plugin drops", self.plugin_drops),
            ("control stalled", self.control_stalled),
            ("late sandbox batches", self.late_sandbox_batches),
            ("bad plugin blocks", self.bad_plugin_blocks),
        ]
    }

    /// True when every counter is zero.
    pub fn is_clear(&self) -> bool {
        self.fields().iter().all(|(_, n)| *n == 0)
    }

    /// `label count` for each counter that is not zero: the pieces the status bar shows.
    pub fn parts(&self) -> Vec<String> {
        self.fields()
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(label, n)| format!("{label} {n}"))
            .collect()
    }
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
        if s.stream_errors > 0 {
            parts.push(format!("stream errors {}", s.stream_errors));
        }
        if let Some(alert) = stream_alert(s) {
            parts.push(alert);
        }
        if s.faulted {
            parts.push(faulted_text(s));
        }
        if s.streaming || s.stream_underruns > 0 {
            parts.push(format!("underruns {}", s.stream_underruns));
        }
        parts.extend(s.overload.parts());
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
    parts.extend(s.sync.parts());
    if let Some(mode) = s.plugin_mode {
        parts.push(format!("plugins: {mode}"));
    }
    parts.join(" \u{b7} ")
}

/// `AUDIO DEVICE LOST` or `AUDIO STOPPED` when the stream does not deliver audio although it is
/// open (the device was unplugged, or the driver stopped calling back); `None` while it runs.
pub fn stream_alert(s: &AudioStatus) -> Option<String> {
    match s.stream_state {
        StreamState::Lost => Some("AUDIO DEVICE LOST".to_string()),
        StreamState::Stalled => Some(format!(
            "AUDIO STOPPED ({:.1} s without audio)",
            s.stream_silent_ms as f64 / 1000.0
        )),
        StreamState::Off | StreamState::Starting | StreamState::Running => None,
    }
}

/// The settings dialog's note about the buffer and the latency of the running stream.
pub fn latency_note(s: &AudioStatus) -> Option<String> {
    if !s.running {
        return None;
    }
    let ms = |f: u32| f64::from(f) * 1000.0 / f64::from(s.sample_rate.max(1));
    let mut text = match s.buffer_frames {
        Some(n) => format!("Buffer {n} frames ({:.1} ms)", ms(n)),
        None => "Buffer size not reported yet".to_string(),
    };
    if s.sample_rate > 0 && s.output_latency_frames > 0 {
        text.push_str(&format!(
            "; latency out {:.1} ms",
            ms(s.output_latency_frames)
        ));
        if s.input_channels_open > 0 && s.input_latency_frames > 0 {
            text.push_str(&format!(", in {:.1} ms", ms(s.input_latency_frames)));
        }
    }
    text.push('.');
    Some(text)
}

/// The settings dialog's warning when the open input and output are different devices: their
/// clocks drift apart, which clicks in monitored input and recorded takes. `None` for one
/// device, without input, and while audio is off.
pub fn drift_note(s: &AudioStatus) -> Option<String> {
    if !s.running || s.input_channels_open == 0 {
        return None;
    }
    let input = s.input_device.as_deref()?;
    if input.eq_ignore_ascii_case(&s.device) {
        return None;
    }
    let lost = if s.overload.input_overruns > 0 {
        format!(" {} input frames lost so far.", s.overload.input_overruns)
    } else {
        String::new()
    };
    Some(format!(
        "Warning: input ({input}) and output ({}) are different devices; their clocks drift \
         apart and the input clicks. Use one interface for both.{lost}",
        s.device
    ))
}

/// `ENGINE FAULTED: <device>` (the device that panicked; the engine keeps sounding the rest).
pub fn faulted_text(s: &AudioStatus) -> String {
    match &s.faulted_name {
        Some(name) => format!("ENGINE FAULTED: {name}"),
        None => "ENGINE FAULTED".to_string(),
    }
}

/// The settings dialog's note about the input the running stream opened.
pub fn input_note(s: &AudioStatus) -> Option<String> {
    if !s.running {
        return None;
    }
    let (open, wanted) = (s.input_channels_open, s.input_channels_wanted);
    Some(match (&s.input_error, open, wanted) {
        (_, _, 0) => {
            "Input: the document has no input channels (see Audio > Input Channels).".into()
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
    if open > 0 && s.sample_rate > 0 && s.input_trim.extra_backlog_frames > 0 {
        let ms = f64::from(s.input_trim.allowed_backlog_frames) * 1000.0 / f64::from(s.sample_rate);
        text.push_str(&format!(" (input backlog up to {ms:.1} ms)"));
    }
    text
}

/// What a crash report says about the session: the audio host and device, the rate and buffer,
/// and the MIDI port.
pub fn crash_context(s: &AudioStatus) -> String {
    let audio = if s.running {
        format!(
            "{} \"{}\" {} Hz, buffer {}",
            s.host,
            s.device,
            s.sample_rate,
            s.buffer_frames.map_or("?".to_string(), |n| n.to_string())
        )
    } else {
        format!("{} (audio off)", s.host)
    };
    let midi = match &s.midi_port {
        Some(port) => format!("\"{port}\""),
        None => "off".to_string(),
    };
    format!("audio {audio}; MIDI {midi}")
}

/// How long the DSP figures of the status line stay as they are while they move by less than
/// [`DSP_JUMP`].
pub const DSP_HOLD: std::time::Duration = std::time::Duration::from_secs(10);

/// A move of the DSP load or its peak (1.0 = 100 %) that is shown at once: 2 points.
pub const DSP_JUMP: f32 = 0.02;

/// Keeps the DSP figures of the status line steady. An idle engine's load jitters by a fraction of
/// a point, which flips the rounded figure and repaints the whole window a few times a second for
/// nothing; a figure that moved by less than [`DSP_JUMP`] is shown again as it was until
/// [`DSP_HOLD`] has passed, and a real change shows at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct DspHold {
    shown: Option<(f32, f32, std::time::Instant)>,
}

impl DspHold {
    /// Puts the figures to show into `status` (read at `at`).
    pub fn apply(&mut self, status: &mut AudioStatus, at: std::time::Instant) {
        if !status.running {
            self.shown = None;
            return;
        }
        let (load, peak) = (status.dsp_load, status.dsp_peak);
        if let Some((l, p, since)) = self.shown
            && at.saturating_duration_since(since) < DSP_HOLD
            && (load - l).abs() < DSP_JUMP
            && (peak - p).abs() < DSP_JUMP
        {
            status.dsp_load = l;
            status.dsp_peak = p;
            return;
        }
        self.shown = Some((load, peak, at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_trim_line_has_the_shape_of_the_log() {
        let mut s = AudioStatus {
            input_trim: InputTrim {
                trims: 3,
                backlog_max_frames: 1440,
                callback_min: 96,
                callback_max: 480,
                allowed_backlog_frames: 1536,
                extra_backlog_frames: 512,
            },
            ..AudioStatus::default()
        };
        assert_eq!(
            input_trim_line(&s),
            "input trimmed: 3 trims, backlog max 1440 frames, allowed 1536 frames, callback 96..480 frames, buffer unknown"
        );
        s.buffer_requested = Some(480);
        assert!(input_trim_line(&s).ends_with(", buffer 480"));
    }

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
    fn a_faulted_engine_names_the_device() {
        let mut s = running();
        assert!(!status_text(&s).contains("FAULTED"));
        s.faulted = true;
        assert!(status_text(&s).contains("\u{b7} ENGINE FAULTED \u{b7}"));
        s.faulted_name = Some("Bass".into());
        assert!(status_text(&s).contains("ENGINE FAULTED: Bass"));
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
    fn the_status_shows_how_much_input_backlog_is_allowed() {
        let mut s = AudioStatus {
            input_channels_wanted: 2,
            input_channels_open: 2,
            input_latency_frames: 480,
            output_latency_frames: 960,
            ..running()
        };
        assert!(!status_text(&s).contains("backlog"), "not measured yet");
        s.input_trim.allowed_backlog_frames = 1536;
        assert!(
            !status_text(&s).contains("backlog"),
            "at the base the status line says nothing"
        );
        s.input_trim.extra_backlog_frames = 512;
        let text = status_text(&s);
        assert!(
            text.contains("(latency in 10.0 + out 20.0 ms) (input backlog up to 32.0 ms)"),
            "{text}"
        );
    }

    #[test]
    fn overload_counters_show_only_when_they_are_not_zero() {
        let clear = running();
        assert!(clear.overload.is_clear());
        assert!(!status_text(&clear).contains("dropped"));
        let busy = AudioStatus {
            overload: OverloadStats {
                dropped_events: 4,
                input_overruns: 0,
                tap_overflow_frames: 128,
                plugin_drops: 2,
                control_stalled: 1,
                late_sandbox_batches: 9,
                bad_plugin_blocks: 3,
            },
            ..running()
        };
        assert_eq!(
            status_text(&busy),
            "WASAPI \u{b7} Virtual Cable Input \u{b7} 48 kHz \u{b7} buffer 480 \
             \u{b7} DSP 3% (peak 5%) \u{b7} xruns 2 \u{b7} dropped events 4 \
             \u{b7} record overflow 128 \u{b7} plugin drops 2 \u{b7} control stalled 1 \
             \u{b7} late sandbox batches 9 \u{b7} bad plugin blocks 3 \
             \u{b7} MIDI Example Keyboard \u{2192} Bass"
        );
        let one = AudioStatus {
            overload: OverloadStats {
                input_overruns: 3,
                ..OverloadStats::default()
            },
            ..running()
        };
        assert!(status_text(&one).contains("xruns 2 \u{b7} input overruns 3 \u{b7} MIDI"));
        assert!(!one.overload.is_clear());
    }

    #[test]
    fn a_dead_stream_and_stream_errors_show_in_the_status_bar() {
        let mut s = running();
        s.stream_state = StreamState::Running;
        assert!(stream_alert(&s).is_none());
        assert!(!status_text(&s).contains("stream errors"));
        s.stream_errors = 2;
        s.stream_state = StreamState::Stalled;
        s.stream_silent_ms = 1_500;
        assert_eq!(
            status_text(&s),
            "WASAPI \u{b7} Virtual Cable Input \u{b7} 48 kHz \u{b7} buffer 480 \
             \u{b7} DSP 3% (peak 5%) \u{b7} xruns 2 \u{b7} stream errors 2 \
             \u{b7} AUDIO STOPPED (1.5 s without audio) \u{b7} MIDI Example Keyboard \u{2192} Bass"
        );
        s.stream_state = StreamState::Lost;
        assert!(status_text(&s).contains("stream errors 2 \u{b7} AUDIO DEVICE LOST \u{b7} MIDI"));
        assert!(stream_alert(&AudioStatus::default()).is_none());
    }

    #[test]
    fn the_dialog_notes_give_the_buffer_and_latency() {
        assert_eq!(latency_note(&AudioStatus::default()), None);
        let mut s = running();
        assert_eq!(latency_note(&s).unwrap(), "Buffer 480 frames (10.0 ms).");
        s.output_latency_frames = 960;
        s.input_channels_open = 2;
        s.input_latency_frames = 480;
        assert_eq!(
            latency_note(&s).unwrap(),
            "Buffer 480 frames (10.0 ms); latency out 20.0 ms, in 10.0 ms."
        );
        s.buffer_frames = None;
        assert!(
            latency_note(&s)
                .unwrap()
                .starts_with("Buffer size not reported yet;")
        );
    }

    #[test]
    fn distinct_input_and_output_devices_show_the_drift_warning_and_one_device_does_not() {
        let mut s = AudioStatus {
            input_channels_open: 2,
            input_device: Some("Example USB ASIO".into()),
            ..running()
        };
        let warning = drift_note(&s).expect("different devices drift");
        assert!(
            warning.contains("Example USB ASIO") && warning.contains("Virtual Cable Input"),
            "{warning}"
        );
        assert!(!warning.contains("frames lost"));
        s.overload.input_overruns = 480;
        assert!(
            drift_note(&s)
                .unwrap()
                .contains("480 input frames lost so far.")
        );
        // The same device (any case), no input, or audio off: no warning.
        s.input_device = Some("virtual cable input".into());
        assert_eq!(drift_note(&s), None);
        s.input_device = Some("Other".into());
        s.input_channels_open = 0;
        assert_eq!(drift_note(&s), None);
        s.input_channels_open = 2;
        s.running = false;
        assert_eq!(drift_note(&s), None);
    }

    #[test]
    fn the_crash_context_names_the_devices() {
        assert_eq!(
            crash_context(&running()),
            "audio WASAPI \"Virtual Cable Input\" 48000 Hz, buffer 480; MIDI \"Example Keyboard\""
        );
        assert_eq!(
            crash_context(&AudioStatus {
                host: "ASIO".into(),
                ..AudioStatus::default()
            }),
            "audio ASIO (audio off); MIDI off"
        );
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

    #[test]
    fn the_plugin_mode_is_shown_only_while_plugins_are_on() {
        let mut s = running();
        assert!(!status_text(&s).contains("plugins:"));
        s.plugin_mode = Some("sandbox");
        assert!(
            status_text(&s).ends_with("plugins: sandbox"),
            "{}",
            status_text(&s)
        );
        s.plugin_mode = Some("in-process");
        assert!(status_text(&s).ends_with("plugins: in-process"));
    }

    fn at(base: std::time::Instant, ms: u64) -> std::time::Instant {
        base + std::time::Duration::from_millis(ms)
    }

    #[test]
    fn a_small_jitter_of_the_dsp_figures_is_held_and_a_jump_shows_at_once() {
        #[allow(clippy::disallowed_methods)]
        // Justification: instants for the arithmetic of the hold; nothing depends on the time.
        let base = std::time::Instant::now();
        let mut hold = DspHold::default();
        let reading = |load: f32, peak: f32| AudioStatus {
            dsp_load: load,
            dsp_peak: peak,
            ..running()
        };
        let mut first = reading(0.004, 0.010);
        hold.apply(&mut first, base);
        assert!(
            (first.dsp_load - 0.004).abs() < 1e-6,
            "the first reading is shown"
        );
        // A fraction of a point either way: the same text for ten seconds.
        let mut jitter = reading(0.009, 0.014);
        hold.apply(&mut jitter, at(base, 500));
        assert_eq!(status_text(&jitter), status_text(&first));
        // A rise of four points (12 to 16 %) shows at once.
        let mut rise = reading(0.12, 0.12);
        hold.apply(&mut rise, at(base, 550));
        assert!(
            (rise.dsp_load - 0.12).abs() < 1e-6,
            "far above the first figures"
        );
        let mut rise = reading(0.16, 0.16);
        hold.apply(&mut rise, at(base, 580));
        assert!(
            (rise.dsp_load - 0.16).abs() < 1e-6,
            "four points up is not held"
        );
        // A big jump shows at once too.
        let mut spike = reading(0.30, 0.45);
        hold.apply(&mut spike, at(base, 600));
        assert!((spike.dsp_load - 0.30).abs() < 1e-6);
        assert!((spike.dsp_peak - 0.45).abs() < 1e-6);
        // After the hold the small change shows too.
        let mut later = reading(0.31, 0.46);
        hold.apply(&mut later, at(base, 600 + 10_000));
        assert!((later.dsp_load - 0.31).abs() < 1e-6);
    }

    #[test]
    fn a_stream_that_stops_forgets_the_held_figures() {
        #[allow(clippy::disallowed_methods)]
        // Justification: instants for the arithmetic of the hold; nothing depends on the time.
        let base = std::time::Instant::now();
        let mut hold = DspHold::default();
        let mut on = AudioStatus {
            dsp_load: 0.20,
            ..running()
        };
        hold.apply(&mut on, base);
        let mut off = AudioStatus::default();
        hold.apply(&mut off, at(base, 10));
        let mut again = AudioStatus {
            dsp_load: 0.21,
            ..running()
        };
        hold.apply(&mut again, at(base, 20));
        assert!(
            (again.dsp_load - 0.21).abs() < 1e-6,
            "a new stream starts from its own reading"
        );
    }
}
