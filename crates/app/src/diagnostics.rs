//! Help > Export Diagnostics: one text report a user can attach to an issue.
//!
//! The report holds the version and platform, the status line and every telemetry and overload
//! counter, the settings file, a summary of the plugin cache and the plugin journal, the last
//! crash report and the tail of the log. Paths inside the user's home folder are written as `~`
//! ([`redact`]), so the report names no one's account.

use crate::logging::{self, LOG_FILE};
use crate::status::AudioStatus;
use atomik_plugin_host::PluginCache;
use std::path::{Path, PathBuf};

/// How much of the log the report carries (the end of the newest file).
pub const LOG_TAIL_BYTES: usize = 256 * 1024;

/// What the report is made from.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// The folder of the logs and the crash reports (`<config folder>/logs`).
    pub log_dir: PathBuf,
    /// The settings file, when the run has one.
    pub settings_path: Option<PathBuf>,
    /// The plugin scan cache file.
    pub plugin_cache: Option<PathBuf>,
    /// The plugin crash journal file.
    pub plugin_journal: Option<PathBuf>,
    /// The status bar text.
    pub status_line: String,
    /// The audio telemetry, the overload counters included.
    pub audio: AudioStatus,
    /// The home folders to hide (see [`home_dirs`]).
    pub home_dirs: Vec<String>,
}

/// The user's home folders from the environment (`USERPROFILE`, `HOME`), the paths to redact.
pub fn home_dirs(env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Vec<String> {
    let mut homes: Vec<String> = ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(|k| env(k))
        .map(|v| {
            v.to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string()
        })
        .filter(|v| v.len() > 3)
        .collect();
    homes.sort();
    homes.dedup();
    homes
}

/// `text` with every occurrence of a home folder (any letter case, either slash, and the
/// doubled backslashes of a JSON string) replaced by `~`.
pub fn redact(text: &str, homes: &[String]) -> String {
    let mut variants: Vec<String> = Vec::new();
    for home in homes.iter().filter(|h| h.len() > 3) {
        for v in [
            home.clone(),
            home.replace('\\', "/"),
            home.replace('/', "\\"),
            home.replace('/', "\\").replace('\\', "\\\\"),
        ] {
            if !variants.contains(&v) {
                variants.push(v);
            }
        }
    }
    // The longest first: a folder inside another is replaced whole.
    variants.sort_by_key(|v| std::cmp::Reverse(v.len()));
    let mut out = text.to_string();
    for v in &variants {
        out = replace_ignoring_case(&out, v, "~");
    }
    out
}

/// Replaces `from` (compared ASCII case-insensitively) by `to`.
fn replace_ignoring_case(text: &str, from: &str, to: &str) -> String {
    // ASCII lowercasing keeps every byte offset, so matches map straight back onto `text`.
    let (hay, needle) = (text.to_ascii_lowercase(), from.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = hay[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(to);
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// The end of the file at `path`, at most `max` bytes, starting at a line.
fn tail_of(path: &Path, max: usize) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let cut = bytes.len().saturating_sub(max);
    let mut text = String::from_utf8_lossy(&bytes[cut..]).into_owned();
    if cut > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    Some(text)
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n== {title} ==\n"));
}

fn session_section(out: &mut String, i: &Inputs) {
    let a = &i.audio;
    section(out, "Session");
    out.push_str(&format!("status: {}\n", i.status_line));
    out.push_str(&format!(
        "audio: host {:?}, device {:?}, {} Hz, buffer {}, running {}\n",
        a.host,
        a.device,
        a.sample_rate,
        a.buffer_frames
            .map_or("unknown".to_string(), |n| n.to_string()),
        a.running
    ));
    out.push_str(&format!(
        "load: DSP {:.1}% (peak {:.1}%), xruns {}, stream underruns {}\n",
        a.dsp_load * 100.0,
        a.dsp_peak * 100.0,
        a.xruns,
        a.stream_underruns
    ));
    out.push_str(&format!(
        "input: wanted {} open {} latency in {} out {} frames\n",
        a.input_channels_wanted,
        a.input_channels_open,
        a.input_latency_frames,
        a.output_latency_frames
    ));
    out.push_str(&format!(
        "stream: {:?}, {} stream errors\n",
        a.stream_state, a.stream_errors
    ));
    out.push_str(&format!(
        "engine faulted: {}{}\n",
        a.faulted,
        a.faulted_name
            .as_ref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default()
    ));
    for (what, error) in [("audio error", &a.error), ("input error", &a.input_error)] {
        if let Some(e) = error {
            out.push_str(&format!("{what}: {e}\n"));
        }
    }
    out.push_str(&format!(
        "midi: {}{}\n",
        a.midi_port.as_deref().unwrap_or("off"),
        a.midi_error
            .as_ref()
            .map(|e| format!(" (error: {e})"))
            .unwrap_or_default()
    ));
    out.push_str("overload counters:\n");
    for (label, n) in a.overload.fields() {
        out.push_str(&format!("  {label}: {n}\n"));
    }
}

fn plugins_section(out: &mut String, i: &Inputs) {
    section(out, "Plugins");
    match &i.plugin_cache {
        Some(path) => {
            let (cache, status) = PluginCache::load(path);
            let plugins: usize = cache.files().map(|f| f.plugins.len()).sum();
            out.push_str(&format!(
                "cache: {} files, {plugins} plugins, {} blacklisted ({status:?})\n",
                cache.files().count(),
                cache.blacklist().count()
            ));
            for b in cache.blacklist() {
                let reason = b.reason.lines().next().unwrap_or("");
                out.push_str(&format!(
                    "  blacklisted {:?} {}: {reason}\n",
                    b.format,
                    b.path.display()
                ));
            }
        }
        None => out.push_str("cache: none (no config folder)\n"),
    }
    match i
        .plugin_journal
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
    {
        Some(text) if !text.trim().is_empty() => {
            out.push_str(
                "journal (the plugins loaded now; after an unclean end, the ones that were):\n",
            );
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        _ => out.push_str("journal: empty (the last run ended cleanly)\n"),
    }
}

/// The report text for `inputs`, home folders redacted. `now_secs` is the time to print.
pub fn build(i: &Inputs, now_secs: u64) -> String {
    let mut out = format!(
        "Atomik Rack diagnostics\ngenerated: {}\nversion: {}\nos: {} {}\n",
        logging::utc_stamp(now_secs),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    session_section(&mut out, i);
    section(&mut out, "Settings");
    match i
        .settings_path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
    {
        Some(text) => out.push_str(&text),
        None => out.push_str("(no settings file: the defaults are in use)"),
    }
    out.push('\n');
    plugins_section(&mut out, i);
    section(&mut out, "Last crash report");
    match crate::crash::latest_report(&i.log_dir) {
        Some(path) => {
            out.push_str(&format!(
                "{}\n",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
        None => out.push_str("(none)\n"),
    }
    section(&mut out, "Log");
    match tail_of(&i.log_dir.join(LOG_FILE), LOG_TAIL_BYTES) {
        Some(text) => out.push_str(&text),
        None => out.push_str("(no log file)\n"),
    }
    redact(&out, &i.home_dirs)
}

/// Writes the report to `path`.
///
/// # Errors
/// The file cannot be written.
pub fn export(i: &Inputs, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, build(i, logging::unix_seconds()))
}

/// The file name the save dialog proposes.
pub fn default_file_name() -> String {
    format!(
        "atomik-diagnostics-{}.txt",
        logging::file_stamp(logging::unix_seconds())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::OverloadStats;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atomik-diag-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn home_folders_are_hidden_in_every_spelling() {
        let homes = vec![
            "C:\\Users\\name\\".trim_end_matches('\\').to_string(),
            "/home/name".to_string(),
        ];
        let text = "a C:\\Users\\name\\x b c:/users/name/y \"C:\\\\Users\\\\name\\\\z\" /home/name/.cfg D:\\Plugins";
        assert_eq!(
            redact(text, &homes),
            "a ~\\x b ~/y \"~\\\\z\" ~/.cfg D:\\Plugins"
        );
        // A short or empty value is never a home (it would erase every slash).
        assert_eq!(redact("a/b", &["/".to_string(), String::new()]), "a/b");
        assert_eq!(
            home_dirs(&|k| (k == "HOME").then(|| "/home/name/".into())),
            ["/home/name"]
        );
    }

    #[test]
    fn the_report_carries_the_log_the_counters_and_no_home_paths() {
        let root = scratch("report");
        let home = root.join("home").join("name");
        let home_text = home.to_string_lossy().into_owned();
        let logs = home.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join(LOG_FILE),
            format!("old line\nloaded {home_text}/plugin.clap\nlast line\n"),
        )
        .unwrap();
        std::fs::write(logs.join("crash-20261029-010203.txt"), "message: boom\n").unwrap();
        let settings = home.join("settings.json");
        std::fs::write(
            &settings,
            format!(
                "{{\"library\": {}}}",
                serde_json::to_string(&format!("{home_text}/Library")).unwrap()
            ),
        )
        .unwrap();
        let inputs = Inputs {
            log_dir: logs,
            settings_path: Some(settings),
            status_line: format!("WASAPI \u{b7} {home_text}"),
            audio: AudioStatus {
                host: "WASAPI".into(),
                overload: OverloadStats {
                    dropped_events: 7,
                    ..OverloadStats::default()
                },
                ..AudioStatus::default()
            },
            home_dirs: vec![home_text.clone()],
            ..Inputs::default()
        };
        let text = build(&inputs, 1_793_277_296);
        assert!(!text.contains(&home_text), "{text}");
        assert!(
            !text
                .to_ascii_lowercase()
                .contains(&home_text.replace('\\', "\\\\").to_ascii_lowercase()),
            "{text}"
        );
        assert!(text.contains("generated: 2026-10-29T12:34:56Z"), "{text}");
        assert!(text.contains("loaded ~/plugin.clap") || text.contains("loaded ~\\plugin.clap"));
        assert!(text.contains("last line"), "{text}");
        assert!(text.contains("message: boom"), "{text}");
        assert!(text.contains("dropped events: 7"), "{text}");
        assert!(text.contains("== Plugins =="), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        let out = root.join("report.txt");
        export(&inputs, &out).unwrap();
        assert!(std::fs::read_to_string(&out).unwrap().contains("last line"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_report_without_any_files_still_says_so() {
        let text = build(&Inputs::default(), 0);
        for want in [
            "(no settings file",
            "cache: none",
            "journal: empty",
            "(none)",
            "(no log file)",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
    }

    #[test]
    fn the_log_tail_starts_at_a_line() {
        let dir = scratch("tail");
        let path = dir.join(LOG_FILE);
        std::fs::write(&path, "aaaa\nbbbb\ncccc\n").unwrap();
        assert_eq!(tail_of(&path, 7).unwrap(), "cccc\n");
        assert_eq!(tail_of(&path, 100).unwrap(), "aaaa\nbbbb\ncccc\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
