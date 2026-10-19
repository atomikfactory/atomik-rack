//! The out-of-process plugin scanner, as a library so the app can run the very same
//! code as `atomik --plugin-scan <format> <path>` when no `atomik-plugin-scan` program sits
//! beside it (the child-process isolation is the same: the app starts itself as the scanner).
//!
//! Loads one plugin file, creates every plugin in it, activates each once, and prints one line
//! of JSON (a `ScanResult`) to stdout. Exit code 0 means a clean result, even one whose `error`
//! field says the file is not a plugin; anything else (a crash, a signal, exit code 2 for bad
//! arguments) means the file or the call failed. The scanner never runs plugin audio and never
//! writes anything but the JSON line.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::{PluginBackend, ScanResult, mark_main_thread};
use atomik_plugin_vst3::Vst3Backend;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The hidden first argument with which `atomik` runs as the scanner.
pub const SELF_SCAN_FLAG: &str = "--plugin-scan";

/// The usage text printed on bad arguments.
pub const USAGE: &str = "usage: atomik-plugin-scan clap|vst3 <path>";

/// Parses `<format> <path>`; `None` for anything else.
pub fn parse_args(args: &[String]) -> Option<(PluginFormat, PathBuf)> {
    match args {
        [f, p] if f == "clap" => Some((PluginFormat::Clap, PathBuf::from(p))),
        [f, p] if f == "vst3" => Some((PluginFormat::Vst3, PathBuf::from(p))),
        _ => None,
    }
}

/// Scans one file in this process (the caller must have called [`mark_main_thread`]).
pub fn scan_file(format: PluginFormat, path: &Path) -> ScanResult {
    let outcome = match format {
        PluginFormat::Clap => ClapBackend::new().scan_file(path),
        PluginFormat::Vst3 => Vst3Backend::new().scan_file(path),
    };
    match outcome {
        Ok(plugins) => ScanResult {
            path: path.to_path_buf(),
            format,
            plugins,
            error: None,
        },
        Err(e) => ScanResult {
            path: path.to_path_buf(),
            format,
            plugins: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

/// The scanner program: `args` are `<format> <path>`. Prints the JSON line and returns the exit
/// code (0 for a clean result, 2 for bad arguments).
pub fn run(args: &[String]) -> ExitCode {
    let Some((format, path)) = parse_args(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    // Every backend call asserts it runs on the plugin main thread; in the scanner that is this one.
    mark_main_thread();
    let result = scan_file(format, &path);
    // A plugin may have printed to stdout while loading; start a fresh line so ours is last.
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\n{}", result.to_json());
    let _ = out.flush();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn arguments_are_a_format_and_a_path() {
        assert_eq!(
            parse_args(&args(&["clap", "a.clap"])),
            Some((PluginFormat::Clap, PathBuf::from("a.clap")))
        );
        assert_eq!(
            parse_args(&args(&["vst3", "a.vst3"])),
            Some((PluginFormat::Vst3, PathBuf::from("a.vst3")))
        );
        assert_eq!(parse_args(&args(&["vst2", "a.dll"])), None);
        assert_eq!(parse_args(&args(&["clap"])), None);
        assert_eq!(parse_args(&args(&[])), None);
        assert_eq!(parse_args(&args(&["clap", "a", "b"])), None);
    }

    #[test]
    fn a_missing_file_is_a_clean_result_with_an_error() {
        mark_main_thread();
        let r = scan_file(PluginFormat::Clap, Path::new("definitely-not-here.clap"));
        assert!(r.plugins.is_empty());
        assert!(r.error.is_some());
        assert_eq!(r.format, PluginFormat::Clap);
    }
}
