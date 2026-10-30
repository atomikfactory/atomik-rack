//! The `atomik-render` command line. `main.rs` only forwards to [`main_with`].

use crate::build::build;
use crate::patch::Patch;
use crate::plugins::{
    MissingPlugins, PluginHost, PluginIsolation, bounce_settings, determinism_note,
    host_program_name, load_cache, locate_host_program,
};
use crate::project::{PluginBounce, ProjectOptions, is_project, render_doc_with};
use crate::wav::{SampleFormat, write_wav_file};
use crate::{RenderError, default_registry, render_built};
use atomik_device_sdk::RunMode;
use atomik_plugin_sandbox::HostCommand;
use clap::{Parser, ValueEnum};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Exit code for any failure (bad arguments, unreadable or invalid patch, write error).
pub const EXIT_FAILURE: i32 = 2;

/// The plugin run mode of `--plugin-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PluginModeArg {
    /// Plugins are told they render offline (CLAP `render` extension, the default).
    Offline,
    /// Plugins run in their live mode, for a bounce that must match what was heard.
    Realtime,
}

impl PluginModeArg {
    /// The run mode the plugins are told.
    pub fn run_mode(self) -> RunMode {
        match self {
            Self::Offline => RunMode::Offline,
            Self::Realtime => RunMode::Realtime,
        }
    }
}

/// The `--plugin-sandbox` switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxArg {
    /// Each plugin runs in its own `atomik-plugin-host` process, which must sit beside this
    /// program (the default).
    On,
    /// Plugins are loaded into this process.
    Off,
}

/// Where plugins run for `--plugin-sandbox`: in process when it is off, else in child processes
/// started from the `atomik-plugin-host` program beside the executable `exe`.
///
/// # Errors
/// A message that says how to get the program, when the sandbox is on and it is not there.
pub fn plugin_isolation(arg: SandboxArg, exe: Option<&Path>) -> Result<PluginIsolation, String> {
    match arg {
        SandboxArg::Off => Ok(PluginIsolation::InProcess),
        SandboxArg::On => exe
            .and_then(locate_host_program)
            .map(|program| PluginIsolation::Sandbox {
                command: HostCommand::new(program),
                // The bounce waits for every batch.
                bounce: true,
            })
            .ok_or_else(|| {
                format!(
                    "the plugin host program {} is not next to atomik-render: build it (`cargo build -p atomik-plugin-hostd`) or pass --plugin-sandbox off to load plugins into this process",
                    host_program_name()
                )
            }),
    }
}

/// What `--missing-plugins` does with a plugin that is not installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MissingArg {
    /// Fail, naming the plugin (the default).
    Error,
    /// Bounce with a stand-in: silent for an instrument, the dry input for an effect.
    #[value(name = "stand-in")]
    StandIn,
}

/// Renders an Atomik Rack patch or project (JSON) offline to a WAV file.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "atomik-render",
    version,
    about,
    after_help = crate::plugins::VST_TRADEMARK_NOTICE
)]
pub struct Args {
    /// Patch file (JSON, format "atomik-patch" version 1 or 2) or project file (`.atomik`, or a
    /// project bundle folder; a project with audio loads `Audio/` from its bundle,
    /// format "atomik-project"). A project bounces its song: `song.end` plus 2 beats.
    pub patch: PathBuf,
    /// Output WAV file.
    #[arg(short = 'o', long = "output", value_name = "OUT.wav")]
    pub output: PathBuf,
    /// Output sample format (`pcm16` is dithered with seeded TPDF).
    #[arg(long, value_enum, default_value_t = SampleFormat::F32)]
    pub format: SampleFormat,
    /// Host block size in frames; the output does not depend on it.
    #[arg(long, default_value_t = 512, value_parser = clap::value_parser!(u32).range(1..=1_048_576))]
    pub block: u32,
    /// Engine worker threads (projects only); the output does not depend on it.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=64))]
    pub workers: u16,
    /// Projects only: play the song loop N times (N >= 1), then the tail. Without it the
    /// loop is off for the bounce.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=10_000))]
    pub loop_count: Option<u32>,
    /// Projects only: allow third-party plugins (native code made by other companies; it runs in
    /// this process with its access, or in a child process with --plugin-sandbox on, the default). A
    /// project that uses plugins is refused without this flag.
    /// The host gives every plugin the same inputs for any --block and --workers, but a plugin's
    /// output is bit-identical only if the plugin itself is deterministic: many real synths and
    /// reverbs are not (the report says "determinism not guaranteed").
    #[arg(long)]
    pub allow_plugins: bool,
    /// Projects with --allow-plugins: the mode plugins render in.
    #[arg(long, value_enum, default_value_t = PluginModeArg::Offline)]
    pub plugin_mode: PluginModeArg,
    /// Projects with --allow-plugins: run every plugin in its own `atomik-plugin-host` process
    /// (it must sit beside atomik-render), so a plugin that crashes cannot take the bounce
    /// down. `off` loads plugins into this process.
    #[arg(long, value_enum, default_value_t = SandboxArg::On)]
    pub plugin_sandbox: SandboxArg,
    /// Projects with --allow-plugins: what to do with a plugin that is not installed.
    #[arg(long, value_enum, default_value_t = MissingArg::Error)]
    pub missing_plugins: MissingArg,
    /// Projects with --allow-plugins: the plugin scan cache to read (default: the app's
    /// `plugins.json` in its config folder; scan with the app first).
    #[arg(long, value_name = "plugins.json")]
    pub plugin_cache: Option<PathBuf>,
    /// Print frames, duration, wall time, real-time factor and peak levels.
    #[arg(long)]
    pub stats: bool,
    /// Print what is being built and rendered to stderr.
    #[arg(long, short)]
    pub verbose: bool,
}

/// Parses `args` (including the program name), runs, and returns the process exit code.
/// Errors go to `stderr` as `error: <json path or file>: <message>`, warnings as
/// `warning: <json path>: <message>`, statistics to `stdout`.
pub fn main_with<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => {
            // --help and --version are "errors" for clap but succeed for the user.
            return if e.use_stderr() {
                let _ = write!(stderr, "{}", e.render());
                EXIT_FAILURE
            } else {
                let _ = write!(stdout, "{}", e.render());
                0
            };
        }
    };
    match run(&args, stdout, stderr) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            EXIT_FAILURE
        }
    }
}

/// Runs the renderer for parsed arguments.
pub fn run(args: &Args, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<(), RenderError> {
    let input = crate::bundle::resolve_project_path(&args.patch);
    if input.is_dir() {
        return Err(RenderError::Read {
            path: input,
            source: std::io::Error::other("no project file (.atomik) in this folder or beside it"),
        });
    }
    let text = std::fs::read_to_string(&input).map_err(|source| RenderError::Read {
        path: input.clone(),
        source,
    })?;
    let block = usize::try_from(args.block)
        .ok()
        .and_then(NonZeroUsize::new)
        .unwrap_or(NonZeroUsize::MIN);
    let mut registry = default_registry();
    #[allow(clippy::disallowed_methods)]
    // Justification: wall-clock time is only measured for the --stats report; it never
    // influences the rendered samples.
    let clock = std::time::Instant::now;
    let mut plugins_used = 0;
    let (channels, sample_rate, wall) = if is_project(&text) {
        // With --allow-plugins the installed plugins are device types before the project loads, so
        // their real descriptors win over the project's saved layouts (no plugin is loaded yet).
        let isolation = if args.allow_plugins {
            let exe = std::env::current_exe().ok();
            plugin_isolation(args.plugin_sandbox, exe.as_deref()).map_err(RenderError::Project)?
        } else {
            PluginIsolation::InProcess
        };
        let mut host = args.allow_plugins.then(|| {
            let mut host = PluginHost::with_isolation(isolation);
            let (cache, status) = load_cache(args.plugin_cache.as_deref());
            match (&status, &args.plugin_cache) {
                (atomik_plugin_host::LoadStatus::Rebuilt(reason), _) => {
                    let _ = writeln!(stderr, "warning: plugin cache ignored: {reason}");
                }
                // A named cache that is not there is almost always a typo or a path the shell
                // changed; without this line the bounce fails with "missing plugins" only.
                (atomik_plugin_host::LoadStatus::Missing, Some(path)) => {
                    let _ = writeln!(
                        stderr,
                        "warning: plugin cache not found: {} (scan with the app first)",
                        path.display()
                    );
                }
                _ => {}
            }
            host.set_catalog(&cache, &bounce_settings(), &mut registry);
            host
        });
        let doc = atomik_model::load_project(&text, &registry)
            .map_err(|e| RenderError::Project(e.to_string()))?;
        let used = doc.plugin_devices();
        if !used.is_empty() && host.is_none() {
            let names: Vec<String> = used
                .iter()
                .map(|(_, d)| format!("{} ({})", d.name, d.type_id))
                .collect();
            return Err(RenderError::Project(format!(
                "this project uses {} third-party plugin{}: {}. Plugins are native code made by other companies and run inside this process: pass --allow-plugins to bounce it",
                used.len(),
                if used.len() == 1 { "" } else { "s" },
                names.join(", ")
            )));
        }
        atomik_model::register_stand_ins(&doc, &mut registry)
            .map_err(|e| RenderError::Project(e.to_string()))?;
        if args.verbose {
            let _ = writeln!(
                stderr,
                "atomik-render: project, {} devices, {} cables, {} tracks; {} Hz, block {},                  workers {}, loop {}",
                doc.devices.len(),
                doc.cables.len(),
                doc.song.tracks.len(),
                doc.sample_rate,
                args.block,
                args.workers,
                args.loop_count
                    .map_or_else(|| "off".to_string(), |n| format!("x{n}")),
            );
        }
        let options = ProjectOptions {
            host_block: block,
            workers: args.workers,
            loop_count: args.loop_count,
        };
        let started = clock();
        let root = crate::bundle::existing_bundle_root(&input);
        let mut bounce = PluginBounce {
            host: host.as_mut(),
            mode: args.plugin_mode.run_mode(),
            missing: match args.missing_plugins {
                MissingArg::Error => MissingPlugins::Error,
                MissingArg::StandIn => MissingPlugins::StandIn,
            },
        };
        let mut rendered =
            render_doc_with(&doc, &registry, &options, root.as_deref(), &mut bounce)?;
        plugins_used = rendered.plugins;
        let wall = started.elapsed();
        for w in std::mem::take(&mut rendered.warnings) {
            let _ = writeln!(stderr, "warning: {w}");
        }
        (rendered.channels, rendered.sample_rate, wall)
    } else {
        if args.workers != 0 || args.loop_count.is_some() {
            let _ = writeln!(
                stderr,
                "warning: --workers and --loop-count apply to project files; a patch renders                  single-threaded without a loop"
            );
        }
        let patch = Patch::from_json(&text)?;
        let mut built = build(&patch, &registry)?;
        for w in std::mem::take(&mut built.warnings) {
            let _ = writeln!(stderr, "warning: {w}");
        }
        if args.verbose {
            let _ = writeln!(
                stderr,
                "atomik-render: {} devices, {} cables, {} note events; {} frames at {} Hz,                  block {}",
                patch.devices.len(),
                patch.cables.len(),
                built.notes.len(),
                built.frames,
                patch.sample_rate,
                args.block
            );
        }
        let started = clock();
        let channels = render_built(built, block)?;
        (channels, patch.sample_rate, started.elapsed())
    };
    write_wav_file(&args.output, &channels, sample_rate, args.format).map_err(|source| {
        RenderError::Write {
            path: args.output.clone(),
            source,
        }
    })?;
    if args.verbose {
        let _ = writeln!(stderr, "atomik-render: wrote {}", args.output.display());
    }
    if plugins_used > 0 {
        let _ = writeln!(stderr, "atomik-render: {}", determinism_note(plugins_used));
    }
    if args.stats {
        let stats = Stats::measure(&channels, sample_rate, wall);
        let _ = write!(stdout, "{stats}");
        if plugins_used > 0 {
            let _ = writeln!(stdout, "{}", determinism_note(plugins_used));
        }
    }
    Ok(())
}

/// Figures reported by `--stats`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// Frames per channel.
    pub frames: usize,
    /// Rendered duration in seconds.
    pub duration_s: f64,
    /// Wall-clock time spent rendering (not parsing or writing), in seconds.
    pub wall_s: f64,
    /// `duration_s / wall_s`.
    pub realtime_factor: f64,
    /// Peak level of each channel in dBFS (−inf for silence).
    pub peak_dbfs: Vec<f64>,
    /// Samples with |x| > 1 over all channels (they clip in 24-bit PCM output).
    pub over_full_scale: usize,
}

impl Stats {
    /// Computes the statistics of a render that took `wall`.
    pub fn measure(channels: &[Vec<f32>], sample_rate: u32, wall: std::time::Duration) -> Self {
        let frames = channels.first().map_or(0, Vec::len);
        let duration_s = frames as f64 / f64::from(sample_rate);
        let wall_s = wall.as_secs_f64();
        let peak_dbfs = channels
            .iter()
            .map(|c| {
                let peak = c.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                20.0 * f64::from(peak).log10()
            })
            .collect();
        let over_full_scale = channels.iter().flatten().filter(|x| x.abs() > 1.0).count();
        Self {
            frames,
            duration_s,
            wall_s,
            realtime_factor: duration_s / wall_s.max(1e-9),
            peak_dbfs,
            over_full_scale,
        }
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "frames: {}", self.frames)?;
        writeln!(f, "duration: {:.3} s", self.duration_s)?;
        writeln!(f, "wall time: {:.1} ms", self.wall_s * 1e3)?;
        writeln!(f, "real-time factor: {:.1}x", self.realtime_factor)?;
        let peaks: Vec<String> = self
            .peak_dbfs
            .iter()
            .enumerate()
            .map(|(i, db)| format!("out_{} {db:.2} dBFS", i + 1))
            .collect();
        writeln!(f, "peak: {}", peaks.join(", "))?;
        writeln!(f, "samples over full scale: {}", self.over_full_scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_help_carries_the_vst_trademark_notice() {
        use clap::CommandFactory;
        let help = Args::command().render_long_help().to_string();
        assert!(
            help.contains("VST is a trademark of Steinberg Media Technologies GmbH"),
            "{help}"
        );
    }

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("atomik-render-cli-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(args: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = main_with(
            std::iter::once("atomik-render").chain(args.iter().copied()),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn write_patch(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn short_patch() -> String {
        crate::patch::tests::edited(|v| {
            v["length_beats"] = serde_json::json!(1);
            v["devices"][0]["params"]["filter.cutoff"] = serde_json::json!(99_999);
        })
    }

    #[test]
    fn the_plugin_mode_argument_maps_to_the_run_mode_the_plugins_are_told() {
        // `--plugin-mode realtime` is what makes a bounce match what was heard.
        assert_eq!(PluginModeArg::Offline.run_mode(), RunMode::Offline);
        assert_eq!(PluginModeArg::Realtime.run_mode(), RunMode::Realtime);
        let a = Args::try_parse_from(["atomik-render", "p.atomik", "-o", "o.wav"]).unwrap();
        assert_eq!(
            a.plugin_mode.run_mode(),
            RunMode::Offline,
            "offline is the default"
        );
    }

    #[test]
    fn the_plugin_sandbox_argument_is_on_by_default_and_takes_on_or_off() {
        let base = ["atomik-render", "p.atomik", "-o", "o.wav"];
        assert_eq!(
            Args::try_parse_from(base).unwrap().plugin_sandbox,
            SandboxArg::On
        );
        for (text, arg) in [("on", SandboxArg::On), ("off", SandboxArg::Off)] {
            let a =
                Args::try_parse_from(base.into_iter().chain(["--plugin-sandbox", text])).unwrap();
            assert_eq!(a.plugin_sandbox, arg);
        }
        assert!(
            Args::try_parse_from(base.into_iter().chain(["--plugin-sandbox", "maybe"])).is_err()
        );
        let (code, help, _) = run(&["--help"]);
        assert_eq!(code, 0);
        assert!(help.contains("--plugin-sandbox"), "{help}");
    }

    #[test]
    fn the_sandbox_needs_the_child_program_beside_the_executable_unless_it_is_off() {
        let dir = std::env::temp_dir().join(format!("atomik-render-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("atomik-render");
        assert_eq!(
            plugin_isolation(SandboxArg::Off, Some(&exe)),
            Ok(PluginIsolation::InProcess)
        );
        let err = plugin_isolation(SandboxArg::On, Some(&exe)).unwrap_err();
        assert!(err.contains(crate::plugins::host_program_name()), "{err}");
        assert!(err.contains("--plugin-sandbox off"), "{err}");
        assert!(plugin_isolation(SandboxArg::On, None).is_err());
        let program = dir.join(crate::plugins::host_program_name());
        std::fs::write(&program, b"x").unwrap();
        assert_eq!(
            plugin_isolation(SandboxArg::On, Some(&exe)),
            Ok(PluginIsolation::Sandbox {
                command: HostCommand::new(program),
                bounce: true
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_args_defaults() {
        let a = Args::try_parse_from(["atomik-render", "p.json", "-o", "o.wav"]).unwrap();
        assert_eq!(a.patch, PathBuf::from("p.json"));
        assert_eq!(a.output, PathBuf::from("o.wav"));
        assert_eq!((a.format, a.block), (SampleFormat::F32, 512));
        assert_eq!((a.workers, a.loop_count), (0, None));
        assert!(!a.stats && !a.verbose);
        assert!(!a.allow_plugins, "plugins are off by default");
        assert_eq!(a.plugin_mode, PluginModeArg::Offline);
        assert_eq!(a.missing_plugins, MissingArg::Error);
        assert_eq!(a.plugin_cache, None);
        let a = Args::try_parse_from([
            "atomik-render",
            "p.atomik",
            "-o",
            "o.wav",
            "--allow-plugins",
            "--plugin-mode",
            "realtime",
            "--missing-plugins",
            "stand-in",
            "--plugin-cache",
            "c.json",
        ])
        .unwrap();
        assert!(a.allow_plugins);
        assert_eq!(a.plugin_mode, PluginModeArg::Realtime);
        assert_eq!(a.missing_plugins, MissingArg::StandIn);
        assert_eq!(a.plugin_cache, Some(PathBuf::from("c.json")));
        let a = Args::try_parse_from([
            "atomik-render",
            "p.json",
            "-o",
            "o.wav",
            "--format",
            "pcm24",
            "--block",
            "1",
            "--stats",
            "--verbose",
        ])
        .unwrap();
        assert_eq!((a.format, a.block), (SampleFormat::Pcm24, 1));
        assert!(a.stats && a.verbose);
    }

    #[test]
    fn cli_folder_without_a_project_says_so() {
        let dir = TempDir::new("noproj");
        let folder = dir.0.to_string_lossy().into_owned();
        let out = dir.0.join("o.wav").to_string_lossy().into_owned();
        let (code, _, err) = run(&[&folder, "-o", &out]);
        assert_eq!(code, EXIT_FAILURE);
        assert!(err.contains("no project file"), "{err}");
    }

    #[test]
    fn cli_bad_arguments_exit_2() {
        for args in [
            &["p.json"][..],
            &["p.json", "-o", "o.wav", "--block", "0"],
            &["p.json", "-o", "o.wav", "--format", "mp3"],
        ] {
            let (code, _, err) = run(args);
            assert_eq!(code, EXIT_FAILURE, "{args:?}");
            assert!(err.contains("error"), "{err}");
        }
        let (code, out, _) = run(&["--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("--block"));
    }

    #[test]
    fn cli_missing_patch_file_exits_2_naming_the_file() {
        let dir = TempDir::new("missing");
        let patch = dir.0.join("nope.json");
        let out = dir.0.join("o.wav");
        let (code, _, err) = run(&[patch.to_str().unwrap(), "-o", out.to_str().unwrap()]);
        assert_eq!(code, EXIT_FAILURE);
        assert!(err.starts_with("error: "), "{err}");
        assert!(err.contains("nope.json"), "{err}");
        assert!(!out.exists());
    }

    #[test]
    fn cli_invalid_patch_prints_json_path_and_writes_nothing() {
        let dir = TempDir::new("invalid");
        let text = crate::patch::tests::edited(|v| {
            v["devices"][0]["params"]["filter.cutof"] = serde_json::json!(1)
        });
        let patch = write_patch(&dir.0, "bad.json", &text);
        let out = dir.0.join("o.wav");
        let (code, _, err) = run(&[&patch, "-o", out.to_str().unwrap()]);
        assert_eq!(code, EXIT_FAILURE);
        assert!(
            err.starts_with("error: devices[0].params.\"filter.cutof\": "),
            "{err}"
        );
        assert!(!out.exists());
    }

    #[test]
    fn cli_renders_with_warnings_and_stats() {
        let dir = TempDir::new("ok");
        let patch = write_patch(&dir.0, "p.json", &short_patch());
        let out = dir.0.join("o.wav");
        let (code, stdout, err) = run(&[&patch, "-o", out.to_str().unwrap(), "--stats", "-v"]);
        assert_eq!(code, 0, "{err}");
        assert!(
            err.lines()
                .any(|l| l
                    .starts_with("warning: devices[0].params.\"filter.cutoff\": 99999 is outside")),
            "{err}"
        );
        let wav = crate::wav::read_wav_file(&out).unwrap();
        assert_eq!(wav.channels.len(), 2);
        assert_eq!(wav.channels[0].len(), 3 * 24_000);
        assert_eq!(wav.sample_rate, 48_000);
        for key in [
            "frames: 72000",
            "duration: 1.500 s",
            "wall time:",
            "real-time factor:",
            "peak:",
        ] {
            assert!(stdout.contains(key), "{key} missing in {stdout}");
        }
    }

    #[test]
    fn cli_unwritable_output_exits_2_naming_the_file() {
        let dir = TempDir::new("unwritable");
        let patch = write_patch(&dir.0, "p.json", &short_patch());
        let out = dir.0.join("missing-dir").join("o.wav");
        let (code, _, err) = run(&[&patch, "-o", out.to_str().unwrap()]);
        assert_eq!(code, EXIT_FAILURE);
        let error = err
            .lines()
            .find(|l| l.starts_with("error: "))
            .unwrap_or_default();
        assert!(error.contains("o.wav"), "{err}");
    }
}
