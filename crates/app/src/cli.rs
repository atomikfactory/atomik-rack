//! Command-line options of `atomik`.

use crate::settings::{AudioSettings, HostChoice};
use atomik_ui::keymap::{Os, Profile, UserKeymap, sheet};
use clap::Parser;
use std::path::{Path, PathBuf};

/// `atomik`: the Atomik Rack app.
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(
    name = "atomik",
    version,
    about = "Atomik Rack: a rack of devices you patch and play.",
    after_help = atomik_render::plugins::VST_TRADEMARK_NOTICE
)]
pub struct Cli {
    /// Project (`.atomik`) or patch file (format v1 or v2) to open.
    pub patch: Option<PathBuf>,
    /// Audio host for this run: default, wasapi, asio, coreaudio, jack, alsa.
    #[arg(long, value_parser = parse_host)]
    pub host: Option<HostChoice>,
    /// Output device for this run (exact name or unique substring), e.g. "Virtual Cable Input".
    #[arg(long = "audio-device", value_name = "NAME")]
    pub audio_device: Option<String>,
    /// Requested buffer size in frames for this run.
    #[arg(long, value_name = "FRAMES")]
    pub buffer: Option<u32>,
    /// MIDI input port for this run (exact name or unique substring).
    #[arg(long, value_name = "PORT", conflicts_with = "no_midi")]
    pub midi: Option<String>,
    /// No MIDI input for this run.
    #[arg(long)]
    pub no_midi: bool,
    /// Engine worker threads for this run.
    #[arg(long, value_name = "N")]
    pub workers: Option<u16>,
    /// Worker linger in spins for this run (0 = engine default).
    #[arg(long, value_name = "SPINS")]
    pub linger: Option<u64>,
    /// Start without audio (no stream is opened until Audio ▸ Restart Audio).
    #[arg(long)]
    pub no_audio: bool,
    /// Settings file to use instead of the one in the OS config directory.
    #[arg(long, value_name = "PATH")]
    pub settings: Option<PathBuf>,
    /// Print the keyboard cheat sheet (markdown: every command and its chords per OS and
    /// profile, the output `docs/KEYBOARD.md` is checked against) and exit. Narrow it with
    /// `--os` and `--profile`; `--keymap` adds a user keymap on top.
    #[arg(long = "print-keymap", conflicts_with = "patch")]
    pub print_keymap: bool,
    /// With `--print-keymap`: only this OS (win, mac, linux).
    #[arg(long, value_name = "OS", value_parser = parse_os, requires = "print_keymap")]
    pub os: Option<Os>,
    /// With `--print-keymap`: only this profile (standard, legacy).
    #[arg(long, value_name = "PROFILE", value_parser = parse_profile, requires = "print_keymap")]
    pub profile: Option<Profile>,
    /// User keymap file to use instead of `keymap.json` in the OS config directory (also saved
    /// to when the settings dialog changes the keys).
    #[arg(long, value_name = "FILE")]
    pub keymap: Option<PathBuf>,
    /// Open the stress rack (100 devices, 300 cables) instead of a patch.
    #[arg(long, conflicts_with_all = ["patch", "stress_song", "stress_audio", "stress_sampler", "stress_drums", "stress_slicer", "stress_fx", "stress_players", "stress_containers", "stress_plugins"])]
    pub stress: bool,
    /// Open the stress song (8 tracks × 64 bars with automation) instead of a file.
    #[arg(long, conflicts_with = "patch")]
    pub stress_song: bool,
    /// Open the stress project: the song's 8 instrument tracks plus 16 generated audio
    /// tracks (12 to 42 s each; the longer ones stream from disk through the 8 MiB test
    /// threshold unless the settings override it).
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_audio: bool,
    /// Open the stress project: the song's 8 instrument tracks plus 4 samplers with
    /// generated samples and dense chords (4 x 32 = 128 voices). Combine with `--stress-audio`
    /// to add the 16 audio tracks too.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_sampler: bool,
    /// Open the drum stress project: the song's 8 instrument tracks plus 4 drum
    /// machines with all 16 pads loaded (generated short sounds) and dense 1/32 patterns on
    /// every pad. Combine with `--stress-sampler` and `--stress-audio` to add those loads too.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_drums: bool,
    /// Open the slicer stress project: the song's 8 instrument tracks plus 4 loop slicers
    /// with a generated 2-bar drum loop sliced at its transients and dense retriggered 1/32
    /// patterns on every slice. Combine with `--stress-drums`, `--stress-sampler` and
    /// `--stress-audio` to add those loads too.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_slicer: bool,
    /// Open the effects stress project: the song's 8 instrument tracks plus 8 effect
    /// inserts of mixed types (compressor with lookahead, EQ, 4x distortion, modulation) on the
    /// first channels, 2 reverbs on FX returns 5 and 6 and the mastering suite on the master.
    /// Combine with `--stress-audio`, `--stress-sampler`, `--stress-drums` and `--stress-slicer`.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_fx: bool,
    /// Open the players stress project: the song's 8 instrument tracks plus 4 chains
    /// of a step sequencer (dense 1/32 patterns) into an arpeggiator (1/32, three octaves) into
    /// a poly synth with its own channel. Combine with `--stress-fx` and the other loads.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_players: bool,
    /// Open the containers stress project: the song's 8 instrument tracks plus 8 containers
    /// (a poly synth, a compressor with look-ahead, a delay and a nested effect container with a
    /// reverb inside, macros moving by automation), each with a console channel and a song track.
    /// Combine with `--stress-players`, `--stress-fx` and the other loads.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_containers: bool,
    /// Open the Character stress project: the song's 8 instrument tracks plus 8
    /// containers, each a poly synth into a Character effect whose preset steps every bar
    /// through all 16 presets (the long latency class `lossy` included, so the delay
    /// compensation is recompiled under load) and whose Transform is automated, each with its
    /// own console channel. Combine with `--stress-fx`, `--stress-containers` and the other
    /// loads.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_character: bool,
    /// Open the plugins stress project: the song's 8 instrument tracks plus 8 CLAP test
    /// synths playing dense chords and 8 CLAP test effects (inserts, FX returns, latencies from
    /// 0 to 256 frames). Needs the in-repo test plugins (`cargo build -p atomik-test-clap`, next
    /// to the app) and the scanner program beside the app; plugins are on for this run only.
    /// Combine with `--stress-containers`, `--stress-players`, `--stress-fx` and the other loads.
    #[arg(long, conflicts_with_all = ["patch", "stress", "stress_song"])]
    pub stress_plugins: bool,
    /// With `--stress-plugins`: also load up to N installed plugins that are not the test ones
    /// (the ones the plugin scan found; default 0).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        requires = "stress_plugins"
    )]
    pub stress_plugins_real: usize,
    /// Debug: start playback as soon as the window is up.
    #[arg(long)]
    pub play: bool,
    /// Debug: open the piano roll on the first note clip (of the selected device's track).
    #[arg(long)]
    pub piano_roll: bool,
    /// Debug: show the console in the lower pane at start (as F5; not maximised).
    #[arg(long)]
    pub console: bool,
    /// Debug: show the console maximised at start (as Shift+F5).
    #[arg(long)]
    pub console_max: bool,
    /// Debug: select this device at start (e.g. `bass`; also picks the piano-roll clip).
    #[arg(long, value_name = "KEY")]
    pub select: Option<String>,
    /// Debug: take the `--screenshot` after this many seconds instead of at the first frame.
    #[arg(long, value_name = "SECONDS", requires = "screenshot")]
    pub screenshot_after: Option<f64>,
    /// Debug: render the first frame offscreen (GPU) and write it to this PNG.
    #[arg(long, value_name = "PNG")]
    pub screenshot: Option<PathBuf>,
    /// Debug: show the back of the rack at start.
    #[arg(long)]
    pub back: bool,
    /// Debug: quit after this many seconds.
    #[arg(long, value_name = "SECONDS")]
    pub quit_after: Option<f64>,
}

impl Cli {
    /// Whether one of the `--stress-*` loads that build a generated song project is on.
    pub fn stress_song_loads(&self) -> bool {
        self.stress_song
            || self.stress_audio
            || self.stress_sampler
            || self.stress_drums
            || self.stress_slicer
            || self.stress_fx
            || self.stress_players
            || self.stress_containers
            || self.stress_character
            || self.stress_plugins
    }

    /// The document the app starts with when no file is opened: the generated stress racks
    /// for the `--stress*` options, otherwise the starter project (Audio
    /// I/O, the locked master and the two `main` cables, the same as File, New).
    pub fn start_document(&self, registry: &atomik_device_sdk::Registry) -> atomik_model::RackDoc {
        if self.stress {
            crate::stress::stress_doc(
                registry,
                crate::stress::STRESS_DEVICES,
                crate::stress::STRESS_CABLES,
            )
        } else if self.stress_song_loads() {
            crate::stress::stress_song_doc(registry)
        } else {
            atomik_model::new_project_doc()
        }
    }
}

fn parse_os(name: &str) -> Result<Os, String> {
    match name.to_ascii_lowercase().as_str() {
        "win" | "windows" => Ok(Os::Windows),
        "mac" | "macos" => Ok(Os::Mac),
        "linux" => Ok(Os::Linux),
        _ => Err(format!("unknown OS {name:?} (expected win, mac, linux)")),
    }
}

fn parse_profile(name: &str) -> Result<Profile, String> {
    Profile::ALL
        .into_iter()
        .find(|p| p.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("unknown profile {name:?} (expected standard, legacy)"))
}

/// The cheat sheet for `--print-keymap` (markdown): the OS and profile of the options, or all
/// of them, with the `--keymap` file applied when given. The error is for the command line.
pub fn cheat_sheet(cli: &Cli) -> Result<String, String> {
    let user = match &cli.keymap {
        Some(path) => Some(read_keymap(path)?),
        None => None,
    };
    let profiles = cli
        .profile
        .map_or_else(|| Profile::ALL.to_vec(), |p| vec![p]);
    let oses = cli.os.map_or_else(|| Os::ALL.to_vec(), |o| vec![o]);
    Ok(sheet::markdown(&profiles, &oses, user.as_ref()))
}

fn read_keymap(path: &Path) -> Result<UserKeymap, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    UserKeymap::from_json(&text)
        .map(|(user, _)| user)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_host(name: &str) -> Result<HostChoice, String> {
    HostChoice::parse(name).ok_or_else(|| {
        format!("unknown host {name:?} (expected default, wasapi, asio, coreaudio, jack, alsa)")
    })
}

impl Cli {
    /// The settings for this run: the saved `settings` with the command-line overrides
    /// applied (never written back).
    pub fn apply(&self, settings: AudioSettings) -> AudioSettings {
        let mut s = settings;
        if let Some(host) = self.host {
            s.host = host;
        }
        if let Some(device) = &self.audio_device {
            s.output_device = Some(device.clone());
        }
        if let Some(buffer) = self.buffer {
            s.buffer_frames = buffer;
        }
        if let Some(midi) = &self.midi {
            s.midi_input = Some(midi.clone());
        }
        if self.no_midi {
            s.midi_input = None;
        }
        if let Some(workers) = self.workers {
            s.workers = workers;
        }
        if let Some(linger) = self.linger {
            s.worker_linger_spins = linger;
        }
        s.sanitized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("atomik").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn the_app_starts_with_the_starter_project_unless_a_stress_load_is_asked_for() {
        let registry = crate::app_core::default_registry();
        let plain = parse(&[]).start_document(&registry);
        assert_eq!(
            atomik_model::save_project(&plain),
            atomik_model::save_project(&atomik_model::new_project_doc())
        );
        assert_eq!(plain.devices.len(), 1, "the master");
        assert!(plain.console.master.is_some());
        // A file on the command line is opened over it; the start document is still the starter.
        assert_eq!(
            parse(&["x.atomik"]).start_document(&registry).devices.len(),
            1
        );
        // The stress options keep their generated racks.
        assert!(parse(&["--stress"]).start_document(&registry).devices.len() > 10);
        assert!(
            parse(&["--stress-song"])
                .start_document(&registry)
                .song
                .tracks
                .len()
                >= 8
        );
        assert!(parse(&["--stress-fx"]).stress_song_loads());
    }

    #[test]
    fn print_keymap_takes_an_os_a_profile_and_a_keymap_file() {
        let cli = parse(&["--print-keymap"]);
        assert!(cli.print_keymap && cli.os.is_none() && cli.profile.is_none());
        let cli = parse(&["--print-keymap", "--os", "mac", "--profile", "legacy"]);
        assert_eq!(cli.os, Some(Os::Mac));
        assert_eq!(cli.profile, Some(Profile::Legacy));
        for (name, os) in [
            ("win", Os::Windows),
            ("windows", Os::Windows),
            ("linux", Os::Linux),
            ("macos", Os::Mac),
        ] {
            assert_eq!(
                parse(&["--print-keymap", "--os", name]).os,
                Some(os),
                "{name}"
            );
        }
        let cli = parse(&["--keymap", "my.json"]);
        assert_eq!(cli.keymap, Some(PathBuf::from("my.json")));
        let bad = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("atomik").chain(args.iter().copied())).is_err()
        };
        assert!(bad(&["--print-keymap", "--os", "beos"]));
        assert!(bad(&["--print-keymap", "--profile", "vim"]));
        assert!(bad(&["--os", "mac"]), "--os needs --print-keymap");
        assert!(
            bad(&["--profile", "legacy"]),
            "--profile needs --print-keymap"
        );
        assert!(
            bad(&["x.atomik", "--print-keymap"]),
            "a sheet is not a project run"
        );
    }

    #[test]
    fn the_cheat_sheet_follows_the_options() {
        let all = cheat_sheet(&parse(&["--print-keymap"])).unwrap();
        for t in [
            "Windows",
            "macOS",
            "Linux",
            "Standard keys",
            "Atomik legacy keys",
        ] {
            assert!(all.contains(t), "{t}");
        }
        let one = cheat_sheet(&parse(&[
            "--print-keymap",
            "--os",
            "mac",
            "--profile",
            "standard",
        ]))
        .unwrap();
        assert!(one.contains("macOS") && !one.contains("Windows") && !one.contains("legacy keys"));
        assert!(one.contains("`Cmd+Opt+I`"));
    }

    #[test]
    fn a_keymap_file_changes_the_sheet_and_a_bad_one_is_an_error() {
        let dir = std::env::temp_dir().join(format!("atomik-cli-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keymap.json");
        std::fs::write(
            &file,
            r#"{"bindings": [{"command": "edit.duplicate", "context": "global", "chords": ["Cmd+Alt+D"]}]}"#,
        )
        .unwrap();
        let cli = parse(&[
            "--print-keymap",
            "--os",
            "win",
            "--profile",
            "standard",
            "--keymap",
            file.to_str().unwrap(),
        ]);
        assert!(cheat_sheet(&cli).unwrap().contains("`Ctrl+Alt+D`"));
        std::fs::write(&file, "{ nope").unwrap();
        let err = cheat_sheet(&cli).unwrap_err();
        assert!(err.contains("keymap.json"), "{err}");
        let none = dir.join("none.json");
        let missing = parse(&["--print-keymap", "--keymap", none.to_str().unwrap()]);
        assert!(cheat_sheet(&missing).unwrap_err().contains("none.json"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `docs/KEYBOARD.md` is the output of `atomik --print-keymap`.
    /// Regenerate it with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-app --lib cli::`.
    #[test]
    fn the_committed_keyboard_md_is_the_generated_cheat_sheet() {
        let generated = cheat_sheet(&parse(&["--print-keymap"])).unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/KEYBOARD.md");
        if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
            std::fs::write(&path, &generated).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| {
                panic!(
                    "missing {}; run with ATOMIK_UPDATE_GOLDEN=1",
                    path.display()
                )
            })
            .replace("\r\n", "\n");
        assert!(
            committed == generated,
            "docs/KEYBOARD.md differs from `atomik --print-keymap`; regenerate with ATOMIK_UPDATE_GOLDEN=1"
        );
    }

    #[test]
    fn the_help_carries_the_vst_trademark_notice() {
        use clap::CommandFactory;
        let help = Cli::command().render_long_help().to_string();
        assert!(
            help.contains("VST is a trademark of Steinberg Media Technologies GmbH"),
            "{help}"
        );
    }

    #[test]
    fn the_plugin_stress_flag_combines_with_the_other_loads() {
        let cli = parse(&["--stress-plugins"]);
        assert!(cli.stress_plugins);
        assert_eq!(cli.stress_plugins_real, 0);
        let cli = parse(&[
            "--stress-plugins",
            "--stress-plugins-real",
            "2",
            "--stress-containers",
            "--stress-players",
            "--stress-fx",
            "--stress-audio",
            "--stress-sampler",
        ]);
        assert!(cli.stress_plugins && cli.stress_containers && cli.stress_fx);
        assert_eq!(cli.stress_plugins_real, 2);
        assert!(!parse(&[]).stress_plugins);
    }

    #[test]
    fn the_character_stress_flag_is_a_song_load_and_combines_with_the_others() {
        let cli = parse(&["--stress-character"]);
        assert!(cli.stress_character && cli.stress_song_loads());
        assert!(!parse(&[]).stress_character);
        let cli = parse(&[
            "--stress-character",
            "--stress-fx",
            "--stress-containers",
            "--stress-players",
            "--stress-audio",
            "--stress-sampler",
            "--stress-drums",
            "--stress-slicer",
        ]);
        assert!(cli.stress_character && cli.stress_fx && cli.stress_containers);
        // It builds on the generated song, so a file or the plain stress rack is refused.
        let registry = crate::app_core::default_registry();
        let doc = parse(&["--stress-character"]).start_document(&registry);
        assert!(doc.song.tracks.len() >= 8, "the stress song is the base");
        let bad = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("atomik").chain(args.iter().copied())).is_err()
        };
        assert!(bad(&["x.atomik", "--stress-character"]));
        assert!(bad(&["--stress", "--stress-character"]));
        assert!(bad(&["--stress-song", "--stress-character"]));
    }

    #[test]
    fn no_arguments_change_nothing() {
        let cli = parse(&[]);
        assert_eq!(cli.patch, None);
        let saved = AudioSettings {
            midi_input: Some("P6".into()),
            ..AudioSettings::default()
        };
        assert_eq!(cli.apply(saved.clone()), saved);
    }

    #[test]
    fn overrides_replace_saved_values() {
        let cli = parse(&[
            "rack.json",
            "--host",
            "WASAPI",
            "--audio-device",
            "Virtual Cable Input",
            "--buffer",
            "512",
            "--midi",
            "Example",
            "--workers",
            "4",
            "--linger",
            "1000",
        ]);
        assert_eq!(cli.patch, Some(PathBuf::from("rack.json")));
        let s = cli.apply(AudioSettings::default());
        assert_eq!(s.host, HostChoice::Wasapi);
        assert_eq!(s.output_device.as_deref(), Some("Virtual Cable Input"));
        assert_eq!(s.buffer_frames, 512);
        assert_eq!(s.midi_input.as_deref(), Some("Example"));
        assert_eq!(s.workers, 4);
        assert_eq!(s.worker_linger_spins, 1000);
    }

    #[test]
    fn no_midi_clears_the_saved_port() {
        let cli = parse(&["--no-midi"]);
        let saved = AudioSettings {
            midi_input: Some("P6".into()),
            ..AudioSettings::default()
        };
        assert_eq!(cli.apply(saved).midi_input, None);
    }

    #[test]
    fn overrides_are_sanitized() {
        let cli = parse(&["--buffer", "1", "--workers", "500"]);
        let s = cli.apply(AudioSettings::default());
        assert_eq!(s.buffer_frames, crate::settings::MIN_BUFFER);
        assert_eq!(s.workers, crate::settings::MAX_WORKERS);
    }

    #[test]
    fn bad_values_and_conflicts_are_rejected() {
        let bad = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("atomik").chain(args.iter().copied())).is_err()
        };
        assert!(bad(&["--host", "pulse"]));
        assert!(bad(&["--midi", "x", "--no-midi"]));
        assert!(bad(&["x.json", "--stress"]));
        assert!(bad(&["x.atomik", "--stress-song"]));
        assert!(bad(&["--stress", "--stress-song"]));
        assert!(bad(&["--stress-audio", "--stress-song"]));
        assert!(bad(&["--stress-audio", "--stress"]));
        assert!(bad(&["--stress-sampler", "--stress"]));
        assert!(bad(&["--stress-sampler", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-sampler"]));
        assert!(bad(&["--stress-drums", "--stress"]));
        assert!(bad(&["--stress-drums", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-drums"]));
        assert!(bad(&["--stress-slicer", "--stress"]));
        assert!(bad(&["--stress-slicer", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-slicer"]));
        assert!(bad(&["x.atomik", "--stress-audio"]));
        assert!(bad(&["--stress-fx", "--stress"]));
        assert!(bad(&["--stress-fx", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-fx"]));
        assert!(bad(&["--stress-players", "--stress"]));
        assert!(bad(&["--stress-players", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-players"]));
        assert!(bad(&["--stress-containers", "--stress"]));
        assert!(bad(&["--stress-containers", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-containers"]));
        assert!(bad(&["--stress-plugins", "--stress"]));
        assert!(bad(&["--stress-plugins", "--stress-song"]));
        assert!(bad(&["x.atomik", "--stress-plugins"]));
        assert!(
            bad(&["--stress-plugins-real", "2"]),
            "needs --stress-plugins"
        );
        assert!(bad(&["--stress-plugins", "--stress-plugins-real", "x"]));
        assert!(bad(&["--screenshot-after", "2"]), "needs --screenshot");
        assert!(bad(&["--buffer", "-3"]));
    }

    #[test]
    fn debug_flags_parse() {
        let cli = parse(&[
            "--stress",
            "--screenshot",
            "shot.png",
            "--quit-after",
            "2.5",
            "--back",
        ]);
        assert!(cli.stress);
        assert!(cli.back);
        assert_eq!(cli.screenshot, Some(PathBuf::from("shot.png")));
        assert_eq!(cli.quit_after, Some(2.5));
        let cli = parse(&[
            "demo.atomik",
            "--play",
            "--piano-roll",
            "--select",
            "bass",
            "--screenshot",
            "s.png",
            "--screenshot-after",
            "3",
        ]);
        assert_eq!(cli.patch, Some(PathBuf::from("demo.atomik")));
        assert!(cli.play && cli.piano_roll && !cli.stress_song);
        assert_eq!(cli.select.as_deref(), Some("bass"));
        assert_eq!(cli.screenshot_after, Some(3.0));
        assert!(parse(&["--stress-song"]).stress_song);
        assert!(parse(&["--stress-audio"]).stress_audio);
        assert!(parse(&["--stress-sampler"]).stress_sampler);
        let both = parse(&["--stress-audio", "--stress-sampler"]);
        assert!(both.stress_audio && both.stress_sampler, "they combine");
        assert!(parse(&["--stress-drums"]).stress_drums);
        let all = parse(&["--stress-audio", "--stress-sampler", "--stress-drums"]);
        assert!(
            all.stress_audio && all.stress_sampler && all.stress_drums,
            "all three combine"
        );
        assert!(parse(&["--stress-slicer"]).stress_slicer);
        let four = parse(&[
            "--stress-audio",
            "--stress-sampler",
            "--stress-drums",
            "--stress-slicer",
        ]);
        assert!(four.stress_slicer && four.stress_drums, "all four combine");
        assert!(parse(&["--stress-fx"]).stress_fx);
        let five = parse(&[
            "--stress-audio",
            "--stress-sampler",
            "--stress-drums",
            "--stress-slicer",
            "--stress-fx",
        ]);
        assert!(five.stress_fx && five.stress_slicer, "all five combine");
        assert!(parse(&["--stress-players"]).stress_players);
        let six = parse(&["--stress-fx", "--stress-players", "--stress-drums"]);
        assert!(six.stress_players && six.stress_fx && six.stress_drums);
        assert!(parse(&["--stress-containers"]).stress_containers);
        let seven = parse(&[
            "--stress-containers",
            "--stress-players",
            "--stress-fx",
            "--stress-audio",
        ]);
        assert!(seven.stress_containers && seven.stress_players && seven.stress_fx);
        let cli = parse(&["demo.atomik", "--console"]);
        assert!(cli.console && !cli.console_max);
        assert!(parse(&["--console-max"]).console_max);
    }
}
