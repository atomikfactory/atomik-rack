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
//!   "midi_input": "Example Keyboard",
//!   "workers": 2,
//!   "worker_linger_spins": 500000,
//!   "metronome_gain": 0.5,
//!   "input_device": null,
//!   "record_offset_frames": 0,
//!   "pre_count_bars": 0,
//!   "surface": { "driver": "mcu", "input": "Example Surface", "output": "Example Surface" }
//! }
//! ```
//!
//! Every field is optional (missing fields take their defaults; unknown fields are ignored).
//! `output_device: null` is the host's default output, `midi_input: null` is no MIDI input,
//! `worker_linger_spins: 0` is the engine default. `input_device: null` is the device named like
//! the output when the host lists it as an input, else no input (pick one). A positive
//! `record_offset_frames` places recordings earlier (see [`crate::audio_record`]).
//! `pre_count_bars` (0 to 4, default 0 = off) is the count-in a recording starts with: the click
//! counts that many bars before the song starts, and nothing is recorded during them.
//! `surface` is the control surface (the Surfaces tab of the settings): a driver id (`"mcu"`, the
//! eight-fader motor surface; `null` is none) and the MIDI input and output ports it talks through.
//! It is an app setting, not part of the project. `stream_threshold_bytes` (hidden; for tests and the stress run) overrides the size above
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
    /// MIDI input port name (exact or unique substring); `None` = no MIDI input.
    pub midi_input: Option<String>,
    /// Extra engine worker threads (0 = single-threaded).
    pub workers: u16,
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
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            host: HostChoice::platform_default(),
            output_device: None,
            buffer_frames: 256,
            midi_input: None,
            workers: 2,
            worker_linger_spins: DEFAULT_LINGER_SPINS,
            metronome_gain: DEFAULT_METRONOME_GAIN,
            input_device: None,
            record_offset_frames: 0,
            pre_count_bars: 0,
            stream_threshold_bytes: None,
            plugins: atomik_plugin_host::PluginSettings::default(),
            surface: SurfaceSettings::default(),
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

impl AudioSettings {
    /// Clamps the buffer size to `MIN_BUFFER..=MAX_BUFFER` and workers to `MAX_WORKERS`, and
    /// turns empty device or port names into `None`.
    pub fn sanitized(self) -> Self {
        let name = |n: Option<String>| n.filter(|s| !s.trim().is_empty());
        Self {
            output_device: name(self.output_device),
            midi_input: name(self.midi_input),
            input_device: name(self.input_device),
            surface: SurfaceSettings {
                driver: name(self.surface.driver),
                input: name(self.surface.input),
                output: name(self.surface.output),
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

    /// Pretty JSON for the settings file.
    pub fn to_json(&self) -> String {
        // Plain data with string keys: serialization cannot fail.
        let mut json = serde_json::to_string_pretty(self).expect("settings serialize");
        json.push('\n');
        json
    }

    /// Parses settings JSON (missing fields take their defaults), then sanitizes.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(json)
            .map(Self::sanitized)
            .map_err(|e| e.to_string())
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
        assert_eq!(s.midi_input, None);
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
    fn json_round_trips() {
        let s = AudioSettings {
            host: HostChoice::Asio,
            output_device: Some("Example USB ASIO".into()),
            buffer_frames: 128,
            midi_input: Some("Example Keyboard".into()),
            workers: 4,
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
                sandbox: false,
            },
            surface: SurfaceSettings {
                driver: Some("mcu".into()),
                input: Some("Example Surface".into()),
                output: Some("Example Surface Out".into()),
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
        assert_eq!(s.midi_input, None);
        let big = AudioSettings {
            buffer_frames: 1 << 20,
            ..AudioSettings::default()
        };
        assert_eq!(big.sanitized().buffer_frames, MAX_BUFFER);
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
            midi_input: Some("Example Keyboard".into()),
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
