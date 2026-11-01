//! Audio and MIDI settings: what to open, persisted as JSON in the OS config
//! directory.
//!
//! # File
//!
//! `settings.json` in:
//!
//! | OS | Directory |
//! |---|---|
//! | Windows | `%APPDATA%\Atomik Rack` |
//! | macOS | `$HOME/Library/Application Support/Atomik Rack` |
//! | Linux and others | `$XDG_CONFIG_HOME/atomik-rack`, else `$HOME/.config/atomik-rack` |
//!
//! ```json
//! {
//!   "host": "wasapi",
//!   "output_device": "Virtual Cable Input",
//!   "buffer_frames": 256,
//!   "midi_inputs": [
//!     { "name": "Example Keyboard", "enabled": true, "role": "keyboard" },
//!     { "name": "Example Pads", "enabled": true, "role": "mapping" }
//!   ],
//!   "workers": 2,
//!   "workers_auto": true,
//!   "worker_linger_spins": 500000,
//!   "metronome_gain": 0.5,
//!   "input_device": null,
//!   "record_offset_frames": 0,
//!   "pre_count_bars": 0,
//!   "surface": { "driver": "mcu", "input": "Example Surface", "output": "Example Surface" },
//!   "midi_out_port": "Example Synth",
//!   "sync": {
//!     "clock_out": true,
//!     "out_ports": [{ "name": "Example Drum Machine", "offset_ms": -5 }],
//!     "send_while_stopped": false, "send_position": true, "restart_on_locate": true,
//!     "clock_in_port": "Example Sequencer", "follow_clock": false
//!   },
//!   "device_loss": {
//!     "reset_to_default": false,
//!     "last_good": { "host": "wasapi", "output_device": null, "input_device": null,
//!                    "buffer_frames": 256 }
//!   }
//! }
//! ```
//!
//! Every field is optional (missing fields take their defaults; unknown fields are ignored).
//! `output_device: null` is the host's default output, `midi_inputs: []` is no MIDI input (every
//! enabled port is open at once; an older file with one `midi_input` name loads as that input,
//! enabled, and is saved in the new form),
//! `workers_auto: true` starts one worker per physical core but the audio thread's own (at most 8)
//! and ignores `workers`; a file with `workers` and no `workers_auto` keeps its count,
//! `worker_linger_spins: 0` is the engine default. `input_device: null` is the device named like
//! the output when the host lists it as an input, else no input (pick one). A positive
//! `record_offset_frames` places recordings earlier (see [`crate::audio_record`]).
//! `pre_count_bars` (0 to 4, default 0 = off) is the count-in a recording starts with: the click
//! counts that many bars before the song starts, and nothing is recorded during them.
//! `surface` is the control surface (the Surfaces tab of the settings): a driver id (`"mcu"`, the
//! eight-fader motor surface; `null` is none) and the MIDI input and output ports it talks through.
//! `midi_out_port` is the output port a MIDI Out device sends to while it has no port of its own
//! (the Surfaces tab lists it under the surface ports); `null` is none.
//! `sync` is MIDI clock (the Sync tab and Options > Sync): `clock_out` sends 24 clocks per quarter
//! note, Start, Stop and Continue to every port of `out_ports` (each with an `offset_ms` that shifts
//! what leaves that port, `-100..=100`), with `send_while_stopped` keeping the clock running while
//! the transport is stopped, `send_position` adding Song Position Pointer and `restart_on_locate`
//! sending Stop, the position and Continue for a locate while playing. `follow_clock` makes the
//! transport follow the clock of `clock_in_port` (its tempo, Start, Stop, Continue and position)
//! and the transport buttons stop working while it does. Clock settings are app settings, not part
//! of the project, and a bounce never follows a clock.
//! It is an app setting, not part of the project. `device_loss.reset_to_default` makes the app
//! switch to the host's default devices on its own when the stream is lost (default off: it says
//! so and offers Audio > Restart Audio); `device_loss.last_good` is the device configuration that
//! last opened a stream, written when settings are applied: a configuration that fails to open
//! is replaced by it, and is never saved. `stream_threshold_bytes` (hidden; for tests and the stress run) overrides the size above
//! which audio assets stream from disk instead of loading into RAM. The file is written only when the user
//! applies the settings dialog; command-line overrides are never saved. `keymap.json` and
//! `workflow.json` (the Keyboard and Workflow tabs, see [`crate::keymap_store`]) live next to it.

use atomik_audio_io::{HostKind, StreamSpec};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Buffer sizes offered by the settings dialog (frames).
pub const BUFFER_CHOICES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
/// Worker thread counts offered by the settings dialog.
pub const WORKER_CHOICES: [u16; 7] = [0, 1, 2, 3, 4, 6, 8];
/// Worker linger values offered by the settings dialog (spins; 0 = engine default).
pub const LINGER_CHOICES: [u64; 4] = [0, 20_000, 100_000, 500_000];
/// Default worker linger (spins), as `atomik-play --worker-linger`: workers keep spinning
/// between callbacks instead of being woken by each one (OI-1 mitigation).
pub const DEFAULT_LINGER_SPINS: u64 = 500_000;
/// Smallest accepted buffer size.
pub const MIN_BUFFER: u32 = 16;
/// Largest accepted buffer size.
pub const MAX_BUFFER: u32 = 8192;
/// Most worker threads accepted.
pub const MAX_WORKERS: u16 = 16;
/// Largest accepted user record offset, in frames (either sign): one second at 48 kHz.
pub const MAX_RECORD_OFFSET: i32 = 48_000;
/// User record offsets offered by the settings dialog (frames).
pub const RECORD_OFFSET_CHOICES: [i32; 7] = [-480, -240, -96, 0, 96, 240, 480];
/// Default metronome level (linear gain).
pub const DEFAULT_METRONOME_GAIN: f32 = 0.5;
/// Most bars of pre-count.
pub const MAX_PRE_COUNT_BARS: u8 = 4;
/// Pre-count lengths offered by the settings dialog (bars; 0 = off).
pub const PRE_COUNT_CHOICES: [u8; 5] = [0, 1, 2, 3, 4];
/// Settings file name.
pub const SETTINGS_FILE: &str = "settings.json";

/// The audio host API, as stored in the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostChoice {
    /// The platform default host.
    Default,
    /// Windows WASAPI (shared mode).
    Wasapi,
    /// Windows ASIO (builds with the `asio` feature).
    Asio,
    /// macOS CoreAudio.
    #[serde(rename = "coreaudio")]
    CoreAudio,
    /// JACK.
    Jack,
    /// ALSA.
    Alsa,
}

/// The operating system a config path is computed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Linux and other Unix-likes (XDG).
    Other,
}

impl TargetOs {
    /// The OS this binary was built for.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

impl HostChoice {
    /// The platform default: WASAPI on Windows, CoreAudio on macOS, the cpal default elsewhere.
    pub fn platform_default() -> Self {
        match TargetOs::current() {
            TargetOs::Windows => Self::Wasapi,
            TargetOs::MacOs => Self::CoreAudio,
            TargetOs::Other => Self::Default,
        }
    }

    /// The audio-io host kind.
    pub fn kind(self) -> HostKind {
        match self {
            Self::Default => HostKind::Default,
            Self::Wasapi => HostKind::Wasapi,
            Self::Asio => HostKind::Asio,
            Self::CoreAudio => HostKind::CoreAudio,
            Self::Jack => HostKind::Jack,
            Self::Alsa => HostKind::Alsa,
        }
    }

    /// The choice for an audio-io host kind.
    pub fn from_kind(kind: HostKind) -> Self {
        match kind {
            HostKind::Default => Self::Default,
            HostKind::Wasapi => Self::Wasapi,
            HostKind::Asio => Self::Asio,
            HostKind::CoreAudio => Self::CoreAudio,
            HostKind::Jack => Self::Jack,
            HostKind::Alsa => Self::Alsa,
        }
    }

    /// Display name (`WASAPI`, `ASIO`, `CoreAudio`, `JACK`, `ALSA`, `Default`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Wasapi => "WASAPI",
            Self::Asio => "ASIO",
            Self::CoreAudio => "CoreAudio",
            Self::Jack => "JACK",
            Self::Alsa => "ALSA",
        }
    }

    /// Every choice, in menu order.
    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::Wasapi,
        Self::Asio,
        Self::CoreAudio,
        Self::Jack,
        Self::Alsa,
    ];

    /// Parses a command-line name (case-insensitive): `default`, `wasapi`, `asio`,
    /// `coreaudio`, `jack`, `alsa`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|c| c.label().eq_ignore_ascii_case(name.trim()))
    }
}

/// The control surface: a driver and the MIDI ports it talks through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceSettings {
    /// The driver id (see [`atomik_midi_io::surface::DRIVERS`]); `None` = no surface.
    pub driver: Option<String>,
    /// The input port name (exact or unique substring); `None` = none.
    pub input: Option<String>,
    /// The output port name (exact or unique substring), for the motors, LEDs and displays;
    /// `None` = none.
    pub output: Option<String>,
}

/// What a MIDI input is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiInputRole {
    /// Plays the selected device (notes) and feeds the mappings: a keyboard.
    #[default]
    Keyboard,
    /// Feeds the mappings only: its notes never play a device (a pad controller whose pads are
    /// mapped to transport or parameters).
    Mapping,
}

impl MidiInputRole {
    /// The other role.
    pub fn flipped(self) -> Self {
        match self {
            Self::Keyboard => Self::Mapping,
            Self::Mapping => Self::Keyboard,
        }
    }
}

/// One MIDI input port of the settings: its name, whether it is open, and what it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiInputSetting {
    /// The port name (exact or unique substring).
    pub name: String,
    /// The port is opened (a disabled port stays listed so it can be switched on again).
    pub enabled: bool,
    /// What the port is for.
    pub role: MidiInputRole,
}

impl Default for MidiInputSetting {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            role: MidiInputRole::Keyboard,
        }
    }
}

impl MidiInputSetting {
    /// An enabled keyboard input named `name`.
    pub fn enabled(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// The device part of the audio settings: what a stream last opened with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastGood {
    /// Host API.
    pub host: HostChoice,
    /// Output device name; `None` = the host default.
    pub output_device: Option<String>,
    /// Input device name; `None` = the device named like the output.
    pub input_device: Option<String>,
    /// Buffer size in frames.
    pub buffer_frames: u32,
}

impl Default for LastGood {
    fn default() -> Self {
        let d = AudioSettings::default();
        Self::of(&d)
    }
}

impl LastGood {
    /// The device configuration of `settings`.
    pub fn of(settings: &AudioSettings) -> Self {
        Self {
            host: settings.host,
            output_device: settings.output_device.clone(),
            input_device: settings.input_device.clone(),
            buffer_frames: settings.buffer_frames,
        }
    }
}

/// What the app does when the audio device stops working, and the configuration to fall back to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceLossSettings {
    /// Switch to the host's default devices on its own when the stream is lost (the saved
    /// devices stay as they are for when the interface is back). Off: the app says so and offers
    /// Audio > Restart Audio.
    pub reset_to_default: bool,
    /// The device configuration that last opened a stream. A configuration that fails to open
    /// (at start-up, or when applied in the settings dialog) is replaced by this one.
    pub last_good: Option<LastGood>,
}

/// The most the clock leaving an output port may be shifted, in milliseconds (either sign).
pub const MAX_CLOCK_OFFSET_MS: i32 = 100;
/// Clock offsets the settings dialog offers (milliseconds).
pub const CLOCK_OFFSET_CHOICES: [i32; 7] = [-20, -10, -5, 0, 5, 10, 20];

/// An output port the MIDI clock is sent to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockOutPort {
    /// The port name (exact or unique substring).
    pub name: String,
    /// Milliseconds the clock leaves this port later (negative: earlier), for a device that
    /// answers slower or faster than the others, `-100..=100`.
    pub offset_ms: i32,
}

/// MIDI clock in and out (the Sync tab of the settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// Send MIDI clock (24 per quarter note, with Start, Stop and Continue) to `out_ports`.
    pub clock_out: bool,
    /// The ports the clock is sent to, each with its own offset.
    pub out_ports: Vec<ClockOutPort>,
    /// Keep sending the clock while the transport is stopped.
    pub send_while_stopped: bool,
    /// Send Song Position Pointer when the position is set.
    pub send_position: bool,
    /// A locate while playing sends Stop, the position and Continue; off sends the position only.
    pub restart_on_locate: bool,
    /// The input port a clock is followed from (exact or unique substring); `None` = none.
    pub clock_in_port: Option<String>,
    /// Follow the clock of `clock_in_port`: its tempo, start, stop and position drive the
    /// transport, which the transport buttons then cannot.
    pub follow_clock: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            clock_out: false,
            out_ports: Vec::new(),
            send_while_stopped: false,
            send_position: true,
            restart_on_locate: true,
            clock_in_port: None,
            follow_clock: false,
        }
    }
}

impl SyncSettings {
    /// Whether the clock goes anywhere: it is on and has a port.
    pub fn sends(&self) -> bool {
        self.clock_out && !self.out_ports.is_empty()
    }

    /// Whether a clock is followed: it is on and has a port.
    pub fn follows(&self) -> bool {
        self.follow_clock && self.clock_in_port.is_some()
    }

    /// Switches the output port `name` on or off for the clock (new ports start at offset 0).
    pub fn set_out_port(&mut self, name: &str, on: bool) {
        let at = self.out_ports.iter().position(|p| p.name == name);
        match (at, on) {
            (None, true) => self.out_ports.push(ClockOutPort {
                name: name.to_owned(),
                offset_ms: 0,
            }),
            (Some(i), false) => {
                self.out_ports.remove(i);
            }
            _ => {}
        }
    }

    /// Without empty names, repeated ports and out-of-range offsets.
    fn sanitized(self) -> Self {
        let mut out_ports: Vec<ClockOutPort> = Vec::with_capacity(self.out_ports.len());
        for port in self.out_ports {
            if !port.name.trim().is_empty() && !out_ports.iter().any(|o| o.name == port.name) {
                out_ports.push(ClockOutPort {
                    offset_ms: port
                        .offset_ms
                        .clamp(-MAX_CLOCK_OFFSET_MS, MAX_CLOCK_OFFSET_MS),
                    ..port
                });
            }
        }
        Self {
            out_ports,
            clock_in_port: self.clock_in_port.filter(|n| !n.trim().is_empty()),
            ..self
        }
    }
}

/// What the app opens for live audio and MIDI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Host API.
    pub host: HostChoice,
    /// Output device name (exact or unique substring); `None` = the host default.
    pub output_device: Option<String>,
    /// Requested buffer size in frames (the host may pick another; the status bar shows the
    /// actual size).
    pub buffer_frames: u32,
    /// The MIDI input ports: every enabled one is open at once; empty = no MIDI input.
    pub midi_inputs: Vec<MidiInputSetting>,
    /// Extra engine worker threads (0 = single-threaded). Used while `workers_auto` is off.
    pub workers: u16,
    /// Pick the worker count from the machine ([`atomik_audio_io::auto_workers`]) instead of
    /// `workers`. On for new files; a file that sets `workers` and not this keeps its count.
    pub workers_auto: bool,
    /// Worker linger in spins (0 = engine default).
    pub worker_linger_spins: u64,
    /// Metronome level, linear gain 0..=1 (default 0.5).
    pub metronome_gain: f32,
    /// Audio input device name (exact or unique substring); `None` = the device named like the
    /// output device if the host has it as an input, else none (never the host default input, which may be an unrelated microphone).
    pub input_device: Option<String>,
    /// The user's part of the recording compensation, in frames, `-48000..=48000` (default 0).
    /// Positive places recordings earlier on the timeline ([`crate::audio_record`]).
    pub record_offset_frames: i32,
    /// Bars of click the app counts in before a recording starts, `0..=4` (default 0 = off).
    pub pre_count_bars: u8,
    /// Hidden override of the streaming threshold in bytes of decoded f32 audio
    /// (`None` = [`crate::stream_source::STREAM_THRESHOLD_BYTES`]).
    pub stream_threshold_bytes: Option<u64>,
    /// Third-party plugins: off by default, the search folders, the scan
    /// timeout and the plugins the user disabled.
    pub plugins: atomik_plugin_host::PluginSettings,
    /// The control surface (the Surfaces tab).
    pub surface: SurfaceSettings,
    /// The MIDI output port a MIDI Out device uses while it has none of its own (exact or unique
    /// substring of a port name); `None` = such a device sends nothing.
    pub midi_out_port: Option<String>,
    /// MIDI clock in and out (the Sync tab).
    pub sync: SyncSettings,
    /// Device loss handling and the last working device configuration.
    pub device_loss: DeviceLossSettings,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            host: HostChoice::platform_default(),
            output_device: None,
            buffer_frames: 256,
            midi_inputs: Vec::new(),
            workers: 2,
            workers_auto: true,
            worker_linger_spins: DEFAULT_LINGER_SPINS,
            metronome_gain: DEFAULT_METRONOME_GAIN,
            input_device: None,
            record_offset_frames: 0,
            pre_count_bars: 0,
            stream_threshold_bytes: None,
            plugins: atomik_plugin_host::PluginSettings::default(),
            surface: SurfaceSettings::default(),
            midi_out_port: None,
            sync: SyncSettings::default(),
            device_loss: DeviceLossSettings::default(),
        }
    }
}

/// Why the settings file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// Reading or writing failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The file is not valid settings JSON.
    #[error("{path}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
}

/// The input list without empty names and without a second entry for a port already listed.
fn clean_inputs(inputs: Vec<MidiInputSetting>) -> Vec<MidiInputSetting> {
    let mut out: Vec<MidiInputSetting> = Vec::with_capacity(inputs.len());
    for input in inputs {
        if !input.name.trim().is_empty() && !out.iter().any(|o| o.name == input.name) {
            out.push(input);
        }
    }
    out
}

impl AudioSettings {
    /// Clamps the buffer size to `MIN_BUFFER..=MAX_BUFFER` and workers to `MAX_WORKERS`, and
    /// turns empty device or port names into `None`.
    pub fn sanitized(self) -> Self {
        let name = |n: Option<String>| n.filter(|s| !s.trim().is_empty());
        Self {
            output_device: name(self.output_device),
            midi_inputs: clean_inputs(self.midi_inputs),
            input_device: name(self.input_device),
            midi_out_port: name(self.midi_out_port),
            sync: self.sync.sanitized(),
            surface: SurfaceSettings {
                driver: name(self.surface.driver),
                input: name(self.surface.input),
                output: name(self.surface.output),
            },
            device_loss: DeviceLossSettings {
                last_good: self.device_loss.last_good.map(|g| LastGood {
                    output_device: name(g.output_device),
                    input_device: name(g.input_device),
                    buffer_frames: g.buffer_frames.clamp(MIN_BUFFER, MAX_BUFFER),
                    ..g
                }),
                ..self.device_loss
            },
            record_offset_frames: self
                .record_offset_frames
                .clamp(-MAX_RECORD_OFFSET, MAX_RECORD_OFFSET),
            pre_count_bars: self.pre_count_bars.min(MAX_PRE_COUNT_BARS),
            buffer_frames: self.buffer_frames.clamp(MIN_BUFFER, MAX_BUFFER),
            workers: self.workers.min(MAX_WORKERS),
            metronome_gain: if self.metronome_gain.is_finite() {
                self.metronome_gain.clamp(0.0, 1.0)
            } else {
                DEFAULT_METRONOME_GAIN
            },
            ..self
        }
    }

    /// Replaces the device configuration (host, devices, buffer) by `good`.
    pub fn use_devices(&mut self, good: &LastGood) {
        self.host = good.host;
        self.output_device = good.output_device.clone();
        self.input_device = good.input_device.clone();
        self.buffer_frames = good.buffer_frames;
    }

    /// True when `self` and `other` differ in anything but the remembered last working
    /// configuration.
    pub fn differs_from(&self, other: &Self) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        a.device_loss.last_good = None;
        b.device_loss.last_good = None;
        a != b
    }

    /// Pretty JSON for the settings file.
    pub fn to_json(&self) -> String {
        // Plain data with string keys: serialization cannot fail.
        let mut json = serde_json::to_string_pretty(self).expect("settings serialize");
        json.push('\n');
        json
    }

    /// Parses settings JSON (missing fields take their defaults), then sanitizes. A file from
    /// before several MIDI inputs, with one `midi_input` name and no `midi_inputs`, loads as
    /// that input, enabled.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let legacy = match value.as_object() {
            Some(o) if !o.contains_key("midi_inputs") => o
                .get("midi_input")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            _ => None,
        };
        // A file from before the automatic count that chose a number keeps it.
        let keeps_count = value
            .as_object()
            .is_some_and(|o| o.contains_key("workers") && !o.contains_key("workers_auto"));
        let mut settings = serde_json::from_value::<Self>(value).map_err(|e| e.to_string())?;
        if keeps_count {
            settings.workers_auto = false;
        }
        if let Some(name) = legacy {
            settings.midi_inputs = vec![MidiInputSetting::enabled(name)];
        }
        Ok(settings.sanitized())
    }

    /// The ports to open, in settings order: the enabled ones.
    pub fn enabled_midi_inputs(&self) -> impl Iterator<Item = &MidiInputSetting> {
        self.midi_inputs.iter().filter(|i| i.enabled)
    }

    /// Switches the port `name` on or off, adding it (as a keyboard) when it is not listed yet.
    pub fn set_midi_input_enabled(&mut self, name: &str, enabled: bool) {
        match self.midi_inputs.iter_mut().find(|i| i.name == name) {
            Some(input) => input.enabled = enabled,
            None if enabled => self.midi_inputs.push(MidiInputSetting::enabled(name)),
            None => {}
        }
    }

    /// Sets what the port `name` is for (a port that is not listed is left alone).
    pub fn set_midi_input_role(&mut self, name: &str, role: MidiInputRole) {
        if let Some(input) = self.midi_inputs.iter_mut().find(|i| i.name == name) {
            input.role = role;
        }
    }

    /// The stream to open for an engine at `sample_rate` with `output_channels` and the
    /// document's `input_channels` (0 = output only). The input device is
    /// [`Self::input_device`], passed separately to the backend.
    pub fn stream_spec(
        &self,
        sample_rate: u32,
        output_channels: u16,
        input_channels: u16,
    ) -> StreamSpec {
        StreamSpec {
            host: self.host.kind(),
            device: self.output_device.clone(),
            sample_rate,
            buffer_frames: self.buffer_frames,
            input_channels,
            output_channels,
        }
    }
}

/// The app config directory for `os`, from environment lookups (`APPDATA`, `HOME`,
/// `XDG_CONFIG_HOME`). `None` when the needed variable is missing or empty.
pub fn config_dir(os: TargetOs, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    match os {
        TargetOs::Windows => var("APPDATA").map(|d| d.join("Atomik Rack")),
        TargetOs::MacOs => var("HOME").map(|h| h.join("Library/Application Support/Atomik Rack")),
        TargetOs::Other => var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("atomik-rack")),
    }
}

/// The library folder used when the user has not chosen one: `Atomik Rack/Library` in the
/// documents folder (`USERPROFILE`, `HOME`, `XDG_DOCUMENTS_DIR`). `None` when the needed
/// variable is missing or empty.
pub fn default_library_dir(
    os: TargetOs,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let documents = match os {
        TargetOs::Windows => var("USERPROFILE").map(|d| d.join("Documents")),
        TargetOs::MacOs => var("HOME").map(|h| h.join("Documents")),
        TargetOs::Other => {
            var("XDG_DOCUMENTS_DIR").or_else(|| var("HOME").map(|h| h.join("Documents")))
        }
    };
    documents.map(|d| d.join("Atomik Rack").join("Library"))
}

/// The settings file path for this OS and process environment.
pub fn default_settings_path() -> Option<PathBuf> {
    config_dir(TargetOs::current(), &|k| std::env::var_os(k)).map(|d| d.join(SETTINGS_FILE))
}

/// The folder that holds the plugin cache, the plugin journal, the recovery snapshots and the
/// logs of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFolder {
    /// The folder: the settings file's, or the temporary one when there is no settings file.
    pub dir: PathBuf,
    /// True when there is no settings file (no config folder in the environment), so `dir` is
    /// the temporary fallback.
    pub fallback: bool,
}

/// What the app tells the user when it runs on the temporary fallback folder.
pub const NO_CONFIG_NOTE: &str = "No config folder was found: settings are not saved, and the plugin cache, journal, recovery snapshots and logs use a temporary folder";

/// The folder next to `settings_path` (the default settings file, or `--settings`), or a folder
/// in the OS temporary directory when the run has no settings file: recovery and the plugin
/// journal then still exist, for this run.
pub fn config_folder(settings_path: Option<&Path>) -> ConfigFolder {
    match settings_path.and_then(Path::parent) {
        Some(dir) => ConfigFolder {
            dir: dir.to_path_buf(),
            fallback: false,
        },
        None => ConfigFolder {
            dir: std::env::temp_dir().join("atomik-rack"),
            fallback: true,
        },
    }
}

/// Loads settings from `path`. A missing file gives the defaults and no error; an unreadable
/// or invalid file gives the defaults and the error (to show to the user).
pub fn load(path: &Path) -> (AudioSettings, Option<SettingsError>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match AudioSettings::from_json(&text) {
            Ok(s) => (s, None),
            Err(message) => {
                // Kept as `settings.json.bad` so the next save does not overwrite what the
                // user may want to repair by hand.
                let message = match crate::recovery::quarantine(path) {
                    Some(bad) => format!(
                        "{message}; the file was kept as {} and the defaults are used",
                        bad.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ),
                    None => format!("{message}; the defaults are used"),
                };
                (
                    AudioSettings::default(),
                    Some(SettingsError::Parse {
                        path: path.to_path_buf(),
                        message,
                    }),
                )
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (AudioSettings::default(), None),
        Err(source) => (
            AudioSettings::default(),
            Some(SettingsError::Io {
                path: path.to_path_buf(),
                source,
            }),
        ),
    }
}

/// Writes settings to `path`, creating its directory. The write is atomic (a temporary file,
/// then a rename), so a crash never leaves a truncated file.
pub fn save(path: &Path, settings: &AudioSettings) -> Result<(), SettingsError> {
    crate::recovery::atomic_write(path, settings.to_json().as_bytes()).map_err(|source| {
        SettingsError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("atomik-app-settings-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn a_run_without_a_settings_file_uses_a_temporary_config_folder() {
        let none = config_folder(None);
        assert!(none.fallback);
        assert!(none.dir.starts_with(std::env::temp_dir()));
        let beside = config_folder(Some(Path::new("some/dir/foo.json")));
        assert_eq!(beside.dir, Path::new("some/dir"));
        assert!(!beside.fallback);
        // A bare file name is the working directory, as the plugin cache always read it.
        assert!(!config_folder(Some(Path::new("foo.json"))).fallback);
    }

    #[test]
    fn defaults_use_the_platform_host_and_a_small_buffer() {
        let s = AudioSettings::default();
        assert_eq!(s.host, HostChoice::platform_default());
        assert_eq!(s.output_device, None);
        assert_eq!(s.buffer_frames, 256);
        assert!(s.midi_inputs.is_empty());
        assert_eq!(s.workers, 2);
        assert_eq!(s.worker_linger_spins, DEFAULT_LINGER_SPINS);
        assert_eq!(
            DEFAULT_LINGER_SPINS, 500_000,
            "matches atomik-play --worker-linger"
        );
    }

    #[test]
    fn the_platform_default_host_matches_the_os() {
        let expected = match TargetOs::current() {
            TargetOs::Windows => HostChoice::Wasapi,
            TargetOs::MacOs => HostChoice::CoreAudio,
            TargetOs::Other => HostChoice::Default,
        };
        assert_eq!(HostChoice::platform_default(), expected);
        if cfg!(windows) {
            assert_eq!(TargetOs::current(), TargetOs::Windows);
        }
    }

    #[test]
    fn host_choices_map_to_audio_io_kinds_and_back() {
        for (choice, kind, label) in [
            (HostChoice::Default, HostKind::Default, "Default"),
            (HostChoice::Wasapi, HostKind::Wasapi, "WASAPI"),
            (HostChoice::Asio, HostKind::Asio, "ASIO"),
            (HostChoice::CoreAudio, HostKind::CoreAudio, "CoreAudio"),
            (HostChoice::Jack, HostKind::Jack, "JACK"),
            (HostChoice::Alsa, HostKind::Alsa, "ALSA"),
        ] {
            assert_eq!(choice.kind(), kind);
            assert_eq!(HostChoice::from_kind(kind), choice);
            assert_eq!(choice.label(), label);
            assert_eq!(HostChoice::parse(&label.to_lowercase()), Some(choice));
        }
        assert_eq!(HostChoice::parse("WASAPI"), Some(HostChoice::Wasapi));
        assert_eq!(HostChoice::parse("pulse"), None);
    }

    #[test]
    fn third_party_plugins_are_off_by_default_and_an_old_file_keeps_them_off() {
        // Consent is explicit: nothing is enabled until the user says so.
        let d = AudioSettings::default();
        assert!(!d.plugins.enabled);
        assert!(d.plugins.extra_paths.is_empty() && d.plugins.disabled.is_empty());
        // A settings file from before plugin hosting has no `plugins` key: they stay off.
        let old = AudioSettings::from_json(r#"{"host": "asio", "buffer_frames": 128}"#).unwrap();
        assert!(!old.plugins.enabled);
        assert_eq!(old.buffer_frames, 128);
        // A partial section takes the defaults for the rest.
        let partial =
            AudioSettings::from_json(r#"{"plugins": {"enabled": true, "timeout_secs": 5}}"#)
                .unwrap();
        assert!(partial.plugins.enabled);
        assert_eq!(partial.plugins.timeout_secs, 5);
        assert!(partial.plugins.extra_paths.is_empty());
    }

    #[test]
    fn plugins_run_in_a_separate_process_by_default_and_old_files_keep_that() {
        assert!(AudioSettings::default().plugins.sandbox);
        let old = AudioSettings::from_json(r#"{"plugins": {"enabled": true}}"#).unwrap();
        assert!(old.plugins.sandbox, "a file from before the sandbox: on");
        let off = AudioSettings::from_json(r#"{"plugins": {"sandbox": false}}"#).unwrap();
        assert!(!off.plugins.sandbox);
        let saved = AudioSettings::from_json(&off.to_json()).unwrap();
        assert!(!saved.plugins.sandbox, "the switch is saved");
    }

    #[test]
    fn the_surface_is_optional_and_sanitised() {
        let none = AudioSettings::from_json("{}").unwrap();
        assert_eq!(none.surface, SurfaceSettings::default());
        assert_eq!(none.surface.driver, None);
        let s = AudioSettings::from_json(
            r#"{ "surface": { "driver": "mcu", "input": "  ", "output": "Example Desk" } }"#,
        )
        .unwrap();
        assert_eq!(s.surface.driver.as_deref(), Some("mcu"));
        assert_eq!(s.surface.input, None, "an empty port is none");
        assert_eq!(s.surface.output.as_deref(), Some("Example Desk"));
        // Partial objects and unknown fields are fine.
        let s =
            AudioSettings::from_json(r#"{ "surface": { "driver": "mcu", "extra": 1 } }"#).unwrap();
        assert_eq!(s.surface.output, None);
    }

    #[test]
    fn device_loss_is_optional_and_the_last_good_devices_are_sanitised() {
        let none = AudioSettings::from_json("{}").unwrap();
        assert!(!none.device_loss.reset_to_default, "off by default");
        assert_eq!(none.device_loss.last_good, None);
        let s = AudioSettings::from_json(
            r#"{ "device_loss": { "last_good": { "output_device": " ", "buffer_frames": 1 } } }"#,
        )
        .unwrap();
        let good = s.device_loss.last_good.unwrap();
        assert_eq!(good.output_device, None, "an empty name is the default");
        assert_eq!(good.buffer_frames, MIN_BUFFER);
    }

    #[test]
    fn using_the_last_good_devices_leaves_everything_else_and_the_diff_ignores_it() {
        let mut s = AudioSettings {
            output_device: Some("Broken".into()),
            metronome_gain: 0.25,
            ..AudioSettings::default()
        };
        let before = s.clone();
        s.use_devices(&LastGood {
            host: HostChoice::Alsa,
            output_device: Some("Working".into()),
            input_device: Some("Mic".into()),
            buffer_frames: 512,
        });
        assert_eq!(s.output_device.as_deref(), Some("Working"));
        assert_eq!((s.host, s.buffer_frames), (HostChoice::Alsa, 512));
        assert_eq!(s.metronome_gain, 0.25);
        assert!(s.differs_from(&before));
        let mut remembered = before.clone();
        remembered.device_loss.last_good = Some(LastGood::of(&before));
        assert!(!remembered.differs_from(&before), "only last_good differs");
    }

    #[test]
    fn the_worker_count_is_automatic_unless_an_old_file_chose_one() {
        assert!(AudioSettings::default().workers_auto);
        assert!(AudioSettings::from_json("{}").unwrap().workers_auto);
        let old = AudioSettings::from_json(r#"{ "workers": 3 }"#).unwrap();
        assert_eq!((old.workers, old.workers_auto), (3, false));
        let new = AudioSettings::from_json(r#"{ "workers": 3, "workers_auto": true }"#).unwrap();
        assert!(new.workers_auto);
        let json = AudioSettings::default().to_json();
        assert!(AudioSettings::from_json(&json).unwrap().workers_auto);
    }

    #[test]
    fn json_round_trips() {
        let s = AudioSettings {
            host: HostChoice::Asio,
            output_device: Some("Example USB ASIO".into()),
            buffer_frames: 128,
            midi_inputs: vec![MidiInputSetting::enabled("Example Keyboard")],
            workers: 4,
            workers_auto: false,
            worker_linger_spins: 100_000,
            metronome_gain: 0.25,
            input_device: Some("Example USB ASIO".into()),
            record_offset_frames: -96,
            pre_count_bars: 2,
            stream_threshold_bytes: Some(1 << 20),
            plugins: atomik_plugin_host::PluginSettings {
                enabled: true,
                extra_paths: vec![PathBuf::from("D:/Plugins/CLAP")],
                timeout_secs: 60,
                disabled: ["clap.vendor.broken".to_string()].into(),
                crashed: [(
                    "clap.vendor.flaky".to_string(),
                    "exited with code 3".to_string(),
                )]
                .into(),
                sandbox: false,
            },
            surface: SurfaceSettings {
                driver: Some("mcu".into()),
                input: Some("Example Surface".into()),
                output: Some("Example Surface Out".into()),
            },
            midi_out_port: Some("Example Synth".into()),
            sync: SyncSettings {
                clock_out: true,
                out_ports: vec![ClockOutPort {
                    name: "Example Drum Machine".into(),
                    offset_ms: -5,
                }],
                send_while_stopped: true,
                send_position: false,
                restart_on_locate: false,
                clock_in_port: Some("Example Sequencer".into()),
                follow_clock: true,
            },
            device_loss: DeviceLossSettings {
                reset_to_default: true,
                last_good: Some(LastGood {
                    host: HostChoice::Asio,
                    output_device: Some("Example USB ASIO".into()),
                    input_device: None,
                    buffer_frames: 256,
                }),
            },
        };
        let json = s.to_json();
        assert!(json.contains("\"host\": \"asio\""), "{json}");
        assert_eq!(AudioSettings::from_json(&json).unwrap(), s);
        let core_audio = AudioSettings {
            host: HostChoice::CoreAudio,
            ..AudioSettings::default()
        };
        assert!(core_audio.to_json().contains("\"coreaudio\""));
    }

    #[test]
    fn missing_fields_default_and_unknown_fields_are_ignored() {
        let s = AudioSettings::from_json(r#"{ "buffer_frames": 512, "future": true }"#).unwrap();
        assert_eq!(
            s,
            AudioSettings {
                buffer_frames: 512,
                ..AudioSettings::default()
            }
        );
        assert_eq!(
            AudioSettings::from_json("{}").unwrap(),
            AudioSettings::default()
        );
    }

    #[test]
    fn the_sync_settings_are_sanitized_and_read_back() {
        let s = AudioSettings::from_json(
            r#"{ "sync": { "clock_out": true,
                 "out_ports": [ { "name": "A", "offset_ms": 900 }, { "name": "  " },
                                { "name": "A", "offset_ms": 1 }, { "name": "B", "offset_ms": -900 } ],
                 "clock_in_port": " ", "follow_clock": true } }"#,
        )
        .unwrap();
        let ports: Vec<_> = s
            .sync
            .out_ports
            .iter()
            .map(|p| (p.name.as_str(), p.offset_ms))
            .collect();
        assert_eq!(
            ports,
            vec![("A", MAX_CLOCK_OFFSET_MS), ("B", -MAX_CLOCK_OFFSET_MS)]
        );
        assert_eq!(s.sync.clock_in_port, None);
        // A clock to follow needs a port, a clock to send needs one too.
        assert!(!s.sync.follows());
        assert!(s.sync.sends());
        // Defaults: off, position and restart on.
        let d = SyncSettings::default();
        assert!(!d.sends() && !d.follows() && d.send_position && d.restart_on_locate);
        assert_eq!(AudioSettings::from_json("{}").unwrap().sync, d);
    }

    #[test]
    fn output_ports_are_switched_on_and_off_by_name() {
        let mut s = SyncSettings::default();
        s.set_out_port("A", true);
        s.set_out_port("A", true);
        s.set_out_port("B", true);
        assert_eq!(s.out_ports.len(), 2);
        s.out_ports[0].offset_ms = 7;
        s.set_out_port("A", false);
        s.set_out_port("C", false);
        assert_eq!(s.out_ports.len(), 1);
        assert_eq!(s.out_ports[0].name, "B");
    }
    #[test]
    fn invalid_json_is_an_error() {
        assert!(AudioSettings::from_json("{ nope").is_err());
        assert!(AudioSettings::from_json(r#"{ "host": "pulse" }"#).is_err());
        assert!(AudioSettings::from_json(r#"{ "buffer_frames": -1 }"#).is_err());
    }

    #[test]
    fn values_are_sanitized() {
        let s = AudioSettings::from_json(
            r#"{ "buffer_frames": 1, "workers": 99, "output_device": "", "midi_input": "  " }"#,
        )
        .unwrap();
        assert_eq!(s.buffer_frames, MIN_BUFFER);
        assert_eq!(s.workers, MAX_WORKERS);
        assert_eq!(s.output_device, None);
        assert!(s.midi_inputs.is_empty());
        let big = AudioSettings {
            buffer_frames: 1 << 20,
            ..AudioSettings::default()
        };
        assert_eq!(big.sanitized().buffer_frames, MAX_BUFFER);
    }

    /// A settings file from before several inputs, with one `midi_input`, loads as that input,
    /// enabled, and is saved in the new form.
    #[test]
    fn an_old_file_with_one_midi_input_loads_as_that_input_enabled() {
        let old = r#"{ "host": "wasapi", "buffer_frames": 128, "midi_input": "Example Keyboard" }"#;
        let s = AudioSettings::from_json(old).unwrap();
        assert_eq!(
            s.midi_inputs,
            vec![MidiInputSetting {
                name: "Example Keyboard".into(),
                enabled: true,
                role: MidiInputRole::Keyboard,
            }]
        );
        assert_eq!(s.buffer_frames, 128);
        let saved = s.to_json();
        assert!(!saved.contains("\"midi_input\""), "{saved}");
        assert!(saved.contains("\"midi_inputs\""), "{saved}");
        assert_eq!(AudioSettings::from_json(&saved).unwrap(), s);
        // No input before: none now. A null input stays none.
        assert!(
            AudioSettings::from_json("{}")
                .unwrap()
                .midi_inputs
                .is_empty()
        );
        let none = AudioSettings::from_json(r#"{ "midi_input": null }"#).unwrap();
        assert!(none.midi_inputs.is_empty());
    }

    /// The new list wins over a stale single name, and a file written by this build keeps
    /// disabled and mapping-only entries.
    #[test]
    fn the_midi_input_list_is_read_as_written() {
        let json = r#"{
            "midi_input": "Old Keys",
            "midi_inputs": [
                { "name": "Keys", "enabled": true },
                { "name": "Pads", "enabled": false, "role": "mapping" },
                { "name": "Keys" },
                { "name": " " }
            ]
        }"#;
        let s = AudioSettings::from_json(json).unwrap();
        assert_eq!(
            s.midi_inputs,
            vec![
                MidiInputSetting::enabled("Keys"),
                MidiInputSetting {
                    name: "Pads".into(),
                    enabled: false,
                    role: MidiInputRole::Mapping,
                },
            ],
            "a repeated or empty name is dropped, the stale single name is ignored"
        );
        let names: Vec<&str> = s.enabled_midi_inputs().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Keys"]);
        assert_eq!(AudioSettings::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn switching_a_port_on_lists_it_and_off_keeps_it() {
        let mut s = AudioSettings::default();
        s.set_midi_input_enabled("Keys", true);
        s.set_midi_input_enabled("Pads", true);
        s.set_midi_input_role("Pads", MidiInputRole::Mapping);
        s.set_midi_input_enabled("Keys", false);
        s.set_midi_input_enabled("Unknown", false);
        assert_eq!(s.midi_inputs.len(), 2);
        assert!(!s.midi_inputs[0].enabled && s.midi_inputs[1].enabled);
        assert_eq!(s.midi_inputs[1].role, MidiInputRole::Mapping);
        assert_eq!(MidiInputRole::Keyboard.flipped(), MidiInputRole::Mapping);
    }
    #[test]
    fn the_stream_spec_carries_the_document_rate_and_channel_counts() {
        let s = AudioSettings {
            host: HostChoice::Wasapi,
            output_device: Some("Virtual Cable Input".into()),
            buffer_frames: 512,
            ..AudioSettings::default()
        };
        assert_eq!(
            s.stream_spec(44_100, 2, 0),
            StreamSpec {
                host: HostKind::Wasapi,
                device: Some("Virtual Cable Input".into()),
                sample_rate: 44_100,
                buffer_frames: 512,
                input_channels: 0,
                output_channels: 2,
            }
        );
    }

    #[test]
    fn the_stream_spec_requests_the_documents_input_channels() {
        let s = AudioSettings::default();
        assert_eq!(s.stream_spec(48_000, 2, 2).input_channels, 2);
        assert_eq!(s.stream_spec(48_000, 2, 0).input_channels, 0);
    }

    #[test]
    fn old_settings_files_load_with_the_audio_input_defaults() {
        // An older file: none of the audio fields.
        let old = r#"{ "host": "asio", "output_device": "Example USB ASIO",
                       "buffer_frames": 128, "workers": 4, "metronome_gain": 0.3 }"#;
        let s = AudioSettings::from_json(old).unwrap();
        assert_eq!(s.input_device, None, "null = same device as the output");
        assert_eq!(s.record_offset_frames, 0);
        assert_eq!(s.stream_threshold_bytes, None);
        assert_eq!(s.output_device.as_deref(), Some("Example USB ASIO"));
        assert_eq!(s.buffer_frames, 128);
    }

    #[test]
    fn the_record_offset_is_clamped_to_one_second_and_empty_input_names_mean_default() {
        let s =
            AudioSettings::from_json(r#"{ "record_offset_frames": 999999, "input_device": " " }"#)
                .unwrap();
        assert_eq!(s.record_offset_frames, MAX_RECORD_OFFSET);
        assert_eq!(s.input_device, None);
        let s = AudioSettings::from_json(r#"{ "record_offset_frames": -999999 }"#).unwrap();
        assert_eq!(s.record_offset_frames, -MAX_RECORD_OFFSET);
        assert_eq!(MAX_RECORD_OFFSET, 48_000);
    }

    #[test]
    fn config_dirs_follow_platform_conventions() {
        let win = env(&[("APPDATA", r"C:\Users\example\AppData\Roaming")]);
        assert_eq!(
            config_dir(TargetOs::Windows, &win),
            Some(PathBuf::from(r"C:\Users\example\AppData\Roaming").join("Atomik Rack"))
        );
        let mac = env(&[("HOME", "/Users/example")]);
        assert_eq!(
            config_dir(TargetOs::MacOs, &mac),
            Some(PathBuf::from(
                "/Users/example/Library/Application Support/Atomik Rack"
            ))
        );
        let xdg = env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(
            config_dir(TargetOs::Other, &xdg),
            Some(PathBuf::from("/cfg/atomik-rack"))
        );
        let home = env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(
            config_dir(TargetOs::Other, &home),
            Some(PathBuf::from("/home/me/.config/atomik-rack"))
        );
        assert_eq!(config_dir(TargetOs::Windows, &env(&[])), None);
        assert_eq!(config_dir(TargetOs::MacOs, &env(&[("HOME", "")])), None);
        assert_eq!(config_dir(TargetOs::Other, &env(&[])), None);
    }

    #[test]
    fn default_library_dir_follows_platform_conventions() {
        let win = env(&[("USERPROFILE", "profile/example")]);
        assert_eq!(
            default_library_dir(TargetOs::Windows, &win),
            Some(PathBuf::from("profile/example").join("Documents/Atomik Rack/Library"))
        );
        let mac = env(&[("HOME", "/Users/example")]);
        assert_eq!(
            default_library_dir(TargetOs::MacOs, &mac),
            Some(PathBuf::from(
                "/Users/example/Documents/Atomik Rack/Library"
            ))
        );
        let xdg = env(&[("HOME", "/home/me"), ("XDG_DOCUMENTS_DIR", "/docs")]);
        assert_eq!(
            default_library_dir(TargetOs::Other, &xdg),
            Some(PathBuf::from("/docs/Atomik Rack/Library"))
        );
        let home = env(&[("HOME", "/home/me"), ("XDG_DOCUMENTS_DIR", "")]);
        assert_eq!(
            default_library_dir(TargetOs::Other, &home),
            Some(PathBuf::from("/home/me/Documents/Atomik Rack/Library"))
        );
        assert_eq!(default_library_dir(TargetOs::Windows, &env(&[])), None);
        assert_eq!(
            default_library_dir(TargetOs::MacOs, &env(&[("HOME", "")])),
            None
        );
        assert_eq!(default_library_dir(TargetOs::Other, &env(&[])), None);
    }

    #[test]
    fn save_then_load_round_trips_and_creates_the_directory() {
        let path = temp("nested/dir/settings.json");
        let _ = std::fs::remove_dir_all(temp("nested"));
        let s = AudioSettings {
            midi_inputs: vec![MidiInputSetting::enabled("Example Keyboard")],
            ..AudioSettings::default()
        };
        save(&path, &s).unwrap();
        let (loaded, err) = load(&path);
        assert!(err.is_none());
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(temp("nested"));
    }

    #[test]
    fn a_missing_file_gives_defaults_without_error() {
        let (s, err) = load(&temp("missing/settings.json"));
        assert_eq!(s, AudioSettings::default());
        assert!(err.is_none());
    }

    #[test]
    fn a_corrupt_file_gives_defaults_and_an_error() {
        let path = temp("corrupt.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ corrupt").unwrap();
        let (s, err) = load(&path);
        assert_eq!(s, AudioSettings::default());
        assert!(matches!(err, Some(SettingsError::Parse { .. })));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_metronome_gain_defaults_to_one_half_and_is_clamped() {
        let s = AudioSettings::from_json("{}").unwrap();
        assert!((s.metronome_gain - 0.5).abs() < 1e-6);
        let s = AudioSettings::from_json(r#"{"metronome_gain": 3}"#).unwrap();
        assert!((s.metronome_gain - 1.0).abs() < 1e-6);
        let s = AudioSettings::from_json(r#"{"metronome_gain": -1}"#).unwrap();
        assert!(s.metronome_gain.abs() < 1e-6);
    }

    #[test]
    fn the_pre_count_is_off_by_default_and_at_most_four_bars() {
        assert_eq!(AudioSettings::from_json("{}").unwrap().pre_count_bars, 0);
        let s = AudioSettings::from_json(r#"{"pre_count_bars": 2}"#).unwrap();
        assert_eq!(s.pre_count_bars, 2);
        let s = AudioSettings::from_json(r#"{"pre_count_bars": 9}"#).unwrap();
        assert_eq!(s.pre_count_bars, MAX_PRE_COUNT_BARS);
    }
}
