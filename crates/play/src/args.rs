//! Command-line arguments and their validation beyond what clap checks.

use atomik_audio_io::HostKind;
use atomik_render::patch::SAMPLE_RATES;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Default requested buffer size in frames.
pub const DEFAULT_BUFFER_FRAMES: u32 = 128;

/// Default `--worker-linger`: idle spins before a render worker parks. About 8 ms on the
/// development machine (one spin ≈ 16 ns), i.e. three 128-frame periods at 48 kHz, so workers
/// stay awake from one callback to the next instead of being woken by every callback.
pub const DEFAULT_WORKER_LINGER_SPINS: u64 = 500_000;

/// Audio host API as named on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HostArg {
    /// The platform default host (WASAPI on Windows).
    Default,
    /// Windows WASAPI (shared mode).
    Wasapi,
    /// Windows ASIO (needs a build with `--features asio`).
    Asio,
    /// macOS CoreAudio.
    #[value(name = "coreaudio")]
    CoreAudio,
    /// JACK.
    Jack,
    /// ALSA.
    Alsa,
}

impl HostArg {
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
}

/// Plays an Atomik Rack patch live on an audio device, with live MIDI input.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "atomik-play", version, about)]
pub struct Args {
    /// Patch file (JSON, format "atomik-patch" version 1). Its `notes` are ignored.
    #[arg(required_unless_present = "list", value_name = "PATCH")]
    pub patch: Option<PathBuf>,
    /// Audio host API.
    #[arg(long, value_enum, default_value_t = HostArg::Default)]
    pub host: HostArg,
    /// Output device (exact name or unique case-insensitive substring); default: the host's.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
    /// Sample rate in Hz (44100, 48000 or 96000); default: the patch's.
    #[arg(long, value_name = "HZ")]
    pub rate: Option<u32>,
    /// Requested buffer size in frames (best effort; the host may use its default period).
    #[arg(long, value_name = "FRAMES", default_value_t = DEFAULT_BUFFER_FRAMES,
          value_parser = clap::value_parser!(u32).range(16..=8192))]
    pub buffer: u32,
    /// MIDI input port (exact name or unique case-insensitive substring).
    #[arg(long, value_name = "NAME")]
    pub midi: Option<String>,
    /// Notes input receiving the MIDI notes; default: the first device with a notes input.
    #[arg(long, value_name = "DEVICE_ID:SOCKET")]
    pub to: Option<String>,
    /// Engine worker threads besides the audio thread (0 = single-threaded).
    #[arg(long, value_name = "N", default_value_t = 0,
          value_parser = clap::value_parser!(u16).range(0..=64))]
    pub workers: u16,
    /// Idle spins before a render worker parks. While audio runs, workers spin between
    /// callbacks (each one keeps a core busy) instead of being woken by every callback;
    /// 0 = the engine's short spin (20 000), which parks between callbacks.
    #[arg(long, value_name = "SPINS", default_value_t = DEFAULT_WORKER_LINGER_SPINS)]
    pub worker_linger: u64,
    /// Stop after this many seconds (exit code 1 if any xrun occurred); default: until Ctrl-C.
    #[arg(long, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..))]
    pub duration: Option<u64>,
    /// Print callbacks, xruns, DSP load and dropped events once per second, and one `xrun`
    /// line per xrun as soon as it is seen.
    #[arg(long)]
    pub stats: bool,
    /// Also write the banner, stats lines, xrun lines and summary to FILE (with or without
    /// --stats), to share a run.
    #[arg(long, value_name = "FILE")]
    pub log: Option<PathBuf>,
    /// List audio hosts, devices and MIDI inputs, then exit.
    #[arg(long)]
    pub list: bool,
}

/// Whether this build includes ASIO support.
pub const ASIO_COMPILED: bool = cfg!(feature = "asio");

/// Checks what clap cannot: the sample rate and ASIO availability. `asio_compiled` is
/// [`ASIO_COMPILED`] outside tests.
pub fn validate(args: &Args, asio_compiled: bool) -> Result<(), String> {
    if args.host == HostArg::Asio && !asio_compiled {
        return Err(
            "--host asio: this atomik-play was built without ASIO support; rebuild with \
             `cargo build --release -p atomik-play --features asio` after installing the ASIO \
             SDK (see the atomik-audio-io crate docs), or use --host wasapi"
                .into(),
        );
    }
    if let Some(rate) = args.rate
        && !SAMPLE_RATES.contains(&rate)
    {
        let valid: Vec<String> = SAMPLE_RATES.iter().map(u32::to_string).collect();
        return Err(format!(
            "--rate {rate} is not supported; use one of {}",
            valid.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("atomik-play").chain(argv.iter().copied()))
    }

    #[test]
    fn defaults_match_the_spec() {
        let a = parse(&["p.json"]).unwrap();
        assert_eq!(a.patch, Some(PathBuf::from("p.json")));
        assert_eq!(a.host, HostArg::Default);
        assert_eq!(a.buffer, 128);
        assert_eq!(a.workers, 0);
        assert_eq!(
            (a.rate, a.duration, a.stats, a.list),
            (None, None, false, false)
        );
        assert_eq!((a.device, a.midi, a.to), (None, None, None));
        assert_eq!(a.log, None);
        assert_eq!(a.worker_linger, DEFAULT_WORKER_LINGER_SPINS);
    }

    #[test]
    fn every_option_parses() {
        let a = parse(&[
            "p.json",
            "--host",
            "wasapi",
            "--device",
            "CABLE",
            "--rate",
            "44100",
            "--buffer",
            "256",
            "--midi",
            "Example Keyboard",
            "--to",
            "synth:notes_in",
            "--workers",
            "4",
            "--duration",
            "600",
            "--stats",
            "--log",
            "run.log",
            "--worker-linger",
            "0",
        ])
        .unwrap();
        assert_eq!(a.log, Some(PathBuf::from("run.log")));
        assert_eq!(a.worker_linger, 0);
        assert_eq!(a.host, HostArg::Wasapi);
        assert_eq!(a.device.as_deref(), Some("CABLE"));
        assert_eq!(a.rate, Some(44_100));
        assert_eq!(a.buffer, 256);
        assert_eq!(a.midi.as_deref(), Some("Example Keyboard"));
        assert_eq!(a.to.as_deref(), Some("synth:notes_in"));
        assert_eq!((a.workers, a.duration, a.stats), (4, Some(600), true));
    }

    #[test]
    fn host_names_map_to_host_kinds() {
        for (name, kind) in [
            ("default", HostKind::Default),
            ("wasapi", HostKind::Wasapi),
            ("asio", HostKind::Asio),
            ("coreaudio", HostKind::CoreAudio),
            ("jack", HostKind::Jack),
            ("alsa", HostKind::Alsa),
        ] {
            assert_eq!(parse(&["p", "--host", name]).unwrap().host.kind(), kind);
        }
    }

    #[test]
    fn unknown_host_is_rejected_with_the_valid_names() {
        let e = parse(&["p", "--host", "directsound"]).unwrap_err();
        let text = e.to_string();
        assert!(text.contains("directsound"), "{text}");
        assert!(
            text.contains("wasapi") && text.contains("coreaudio"),
            "{text}"
        );
    }

    #[test]
    fn patch_is_required_unless_listing() {
        assert!(parse(&[]).is_err());
        let a = parse(&["--list"]).unwrap();
        assert!(a.list && a.patch.is_none());
    }

    #[test]
    fn out_of_range_numbers_are_rejected() {
        assert!(parse(&["p", "--buffer", "0"]).is_err());
        assert!(parse(&["p", "--buffer", "100000"]).is_err());
        assert!(parse(&["p", "--duration", "0"]).is_err());
        assert!(parse(&["p", "--workers", "65"]).is_err());
        assert!(parse(&["p", "--workers", "-1"]).is_err());
    }

    #[test]
    fn asio_without_the_feature_explains_how_to_get_it() {
        let a = parse(&["p", "--host", "asio"]).unwrap();
        let e = validate(&a, false).unwrap_err();
        assert!(e.contains("--features asio"), "{e}");
        assert!(e.contains("ASIO"), "{e}");
        assert_eq!(validate(&a, true), Ok(()));
    }

    #[test]
    fn unsupported_rate_is_rejected() {
        let e = validate(&parse(&["p", "--rate", "22050"]).unwrap(), false).unwrap_err();
        assert!(e.contains("22050") && e.contains("48000"), "{e}");
        for ok in ["44100", "48000", "96000"] {
            assert_eq!(
                validate(&parse(&["p", "--rate", ok]).unwrap(), false),
                Ok(())
            );
        }
    }

    #[test]
    fn plain_args_validate() {
        assert_eq!(validate(&parse(&["p"]).unwrap(), false), Ok(()));
        assert_eq!(validate(&parse(&["--list"]).unwrap(), false), Ok(()));
    }
}
