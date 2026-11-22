//! The out-of-process plugin scanner, as a library so the app can run the very same
//! code as `atomik --plugin-scan <format> <path>` when no `atomik-plugin-scan` program sits
//! beside it (the child-process isolation is the same: the app starts itself as the scanner).
//!
//! Loads one plugin file, creates every plugin in it, activates each once. It prints one line of
//! JSON as it learns the classes of the file (`classes`) and one as it finishes each class
//! (`class`), so a parent that has to kill it half way keeps what was done, then one line (a
//! `ScanResult`) to stdout. `--skip <class id>` leaves a class out (the parent restarts the scan
//! without a class that hung). The scanner turns the system error dialogs off, so a crash is a
//! crash at once. Exit code 0 means a clean result, even one whose `error`
//! field says the file is not a plugin; anything else (a crash, a signal, exit code 2 for bad
//! arguments) means the file or the call failed. The scanner never runs plugin audio and never
//! writes anything but the JSON lines.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::{
    PluginBackend, SKIP_FLAG, ScanEvent, ScanResult, mark_main_thread, suppress_error_dialogs,
};
use atomik_plugin_vst3::Vst3Backend;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The hidden first argument with which `atomik` runs as the scanner.
pub const SELF_SCAN_FLAG: &str = "--plugin-scan";

/// The usage text printed on bad arguments.
pub const USAGE: &str = "usage: atomik-plugin-scan clap|vst3 <path> [--skip <class id>]...";

/// What the scanner was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanArgs {
    /// The plugin format of the file.
    pub format: PluginFormat,
    /// The file to scan.
    pub path: PathBuf,
    /// The ids of the classes to leave out (the parent already knows them).
    pub skip: Vec<String>,
}

/// Parses `<format> <path> [--skip <class id>]...`; `None` for anything else.
pub fn parse_args(args: &[String]) -> Option<ScanArgs> {
    let [f, p, rest @ ..] = args else {
        return None;
    };
    let format = match f.as_str() {
        "clap" => PluginFormat::Clap,
        "vst3" => PluginFormat::Vst3,
        _ => return None,
    };
    let mut skip = Vec::new();
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        if flag != SKIP_FLAG {
            return None;
        }
        skip.push(rest.next()?.clone());
    }
    Some(ScanArgs {
        format,
        path: PathBuf::from(p),
        skip,
    })
}

/// Scans one file in this process (the caller must have called [`mark_main_thread`]).
pub fn scan_file(format: PluginFormat, path: &Path) -> ScanResult {
    scan_file_streaming(format, path, &[], &mut |_| {})
}

/// As [`scan_file`], leaving out the classes in `skip` and telling `emit` about each class as
/// it is found (see [`ScanEvent`]). The result holds the classes that were scanned.
pub fn scan_file_streaming(
    format: PluginFormat,
    path: &Path,
    skip: &[String],
    emit: &mut dyn FnMut(&ScanEvent),
) -> ScanResult {
    let mut plugins = Vec::new();
    let mut each = |event: ScanEvent| {
        emit(&event);
        if let ScanEvent::Class(p) = event {
            plugins.push(p);
        }
    };
    let outcome = match format {
        PluginFormat::Clap => ClapBackend::new().scan_file_each(path, skip, &mut each),
        PluginFormat::Vst3 => Vst3Backend::new().scan_file_each(path, skip, &mut each),
    };
    ScanResult {
        path: path.to_path_buf(),
        format,
        plugins,
        error: outcome.err().map(|e| e.to_string()),
    }
}

/// Prints one line on a fresh line (a plugin may have printed to stdout without ending its own).
fn print_line(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\n{text}");
    let _ = out.flush();
}

/// The scanner program: `args` are `<format> <path> [--skip <class id>]...`. Prints one line per
/// class as it is found, then the result line, and returns the exit code (0 for a clean result,
/// 2 for bad arguments).
pub fn run(args: &[String]) -> ExitCode {
    let Some(ScanArgs { format, path, skip }) = parse_args(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    // A plugin that crashes while it loads ends this process at once, with no system dialog
    // that would hold it until the parent's timeout.
    suppress_error_dialogs();
    // Every backend call asserts it runs on the plugin main thread; in the scanner that is this one.
    mark_main_thread();
    let result = scan_file_streaming(format, &path, &skip, &mut |event| {
        print_line(&event.to_json());
    });
    print_line(&result.to_json());
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
        let plain = |format, path: &str, skip: &[&str]| {
            Some(ScanArgs {
                format,
                path: PathBuf::from(path),
                skip: skip.iter().map(|s| (*s).to_string()).collect(),
            })
        };
        assert_eq!(
            parse_args(&args(&["clap", "a.clap"])),
            plain(PluginFormat::Clap, "a.clap", &[])
        );
        assert_eq!(
            parse_args(&args(&["vst3", "a.vst3", "--skip", "x", "--skip", "y"])),
            plain(PluginFormat::Vst3, "a.vst3", &["x", "y"])
        );
        assert_eq!(parse_args(&args(&["clap", "a", "--skip"])), None);
        assert_eq!(parse_args(&args(&["clap", "a", "--other", "x"])), None);
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
