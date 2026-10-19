//! Plugin search paths and the recursive file search.
//!
//! The CLAP defaults and the `CLAP_PATH` rule are the ones of the comment in `clap/entry.h`;
//! the VST3 defaults are the ones of the VST3 developer portal's
//! "Plug-in Locations" page (the 32-bit `Program Files (x86)` and
//! the per-application folders are not searched). Everything takes the OS and an environment
//! lookup as arguments, so each platform's answer is testable on any host.

use atomik_device_sdk::PluginFormat;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directories deeper than this below a search root are not visited (symlink loops end here).
pub const MAX_SEARCH_DEPTH: usize = 8;

/// The operating systems with their own folder conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Linux and every other Unix.
    Linux,
}

impl Os {
    /// The OS this build runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// The file extension (without dot) of a format's plugin files or bundles.
pub fn plugin_extension(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
    }
}

/// The platform's default folders for a format, in the order of the specifications. A folder
/// whose environment variable is not set is left out.
pub fn default_search_paths(
    format: PluginFormat,
    os: Os,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let (upper, lower) = match format {
        PluginFormat::Clap => ("CLAP", "clap"),
        PluginFormat::Vst3 => ("VST3", "vst3"),
    };
    let mut out = Vec::new();
    match os {
        Os::Windows => {
            out.extend(var("COMMONPROGRAMFILES").map(|d| d.join(upper)));
            out.extend(var("LOCALAPPDATA").map(|d| d.join("Programs").join("Common").join(upper)));
        }
        Os::MacOs => {
            out.push(PathBuf::from("/Library/Audio/Plug-Ins").join(upper));
            out.extend(var("HOME").map(|d| d.join("Library/Audio/Plug-Ins").join(upper)));
            if format == PluginFormat::Vst3 {
                out.push(PathBuf::from("/Network/Library/Audio/Plug-Ins").join(upper));
            }
        }
        Os::Linux => {
            out.extend(var("HOME").map(|d| d.join(format!(".{lower}"))));
            if format == PluginFormat::Vst3 {
                out.push(PathBuf::from("/usr/lib64/vst3"));
            }
            out.push(PathBuf::from(format!("/usr/lib/{lower}")));
            if format == PluginFormat::Vst3 {
                out.push(PathBuf::from("/usr/local/lib64/vst3"));
                out.push(PathBuf::from("/usr/local/lib/vst3"));
            }
        }
    }
    out
}

/// Every folder to search: for CLAP the `CLAP_PATH` list first, then the defaults, then the
/// user's own folders; duplicates keep their first position.
pub fn search_roots(
    format: PluginFormat,
    os: Os,
    env: &dyn Fn(&str) -> Option<OsString>,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if format == PluginFormat::Clap
        && let Some(list) = env("CLAP_PATH").and_then(|v| v.into_string().ok())
    {
        let sep = if os == Os::Windows { ';' } else { ':' };
        out.extend(list.split(sep).filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    out.extend(default_search_paths(format, os, env));
    out.extend(extra.iter().cloned());
    let mut seen = Vec::new();
    out.retain(|p| {
        if seen.contains(p) {
            false
        } else {
            seen.push(p.clone());
            true
        }
    });
    out
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Finds the plugin files and bundles of `format` below `roots`, recursively. A directory named
/// `*.clap` is a bundle and is not entered. Symlinks are followed; a directory that is its own
/// ancestor is skipped and the depth is cut at [`MAX_SEARCH_DEPTH`]. Missing roots are ignored.
/// The result is sorted and has no duplicates; paths that are not valid UTF-8 are skipped
/// (the cache stores text).
pub fn find_plugin_files(format: PluginFormat, roots: &[PathBuf]) -> Vec<PathBuf> {
    let ext = plugin_extension(format);
    let mut out = Vec::new();
    for root in roots {
        let mut chain = Vec::new();
        walk(root, ext, 0, &mut chain, &mut out);
    }
    out.retain(|p| p.to_str().is_some());
    out.sort();
    out.dedup();
    out
}

/// The folder names VST2 plugins conventionally live in (compared without case).
const VST2_FOLDER_NAMES: [&str; 4] = ["vstplugins", "vst2", "vst2plugins", "vstplugin"];

/// True when `dir` holds, below it, a file that looks like a VST2 plugin (`.dll`, or a `.vst`
/// bundle); bundles of the supported formats are not entered.
fn holds_vst2_files(dir: &Path, depth: usize) -> bool {
    if depth > MAX_SEARCH_DEPTH {
        return false;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        if has_extension(&path, "dll") || has_extension(&path, "vst") {
            return true;
        }
        if has_extension(&path, "vst3") || has_extension(&path, "clap") {
            continue;
        }
        if path.is_dir() && holds_vst2_files(&path, depth + 1) {
            return true;
        }
    }
    false
}

/// The folders among `folders` that exist and hold VST2 plugins only: no `.clap` or `.vst3` below
/// them, and either a `.dll` (or `.vst` bundle) below them or one of the usual VST2 folder names
/// (`VSTPlugins`, `Vstplugins`, ...). VST2 is not supported; the app hints at it after a scan so
/// a user who added such a folder is not left with a silent zero.
pub fn vst2_only_folders(folders: &[PathBuf]) -> Vec<PathBuf> {
    folders
        .iter()
        .filter(|f| f.is_dir())
        .filter(|f| {
            let roots = std::slice::from_ref(*f);
            find_plugin_files(PluginFormat::Clap, roots).is_empty()
                && find_plugin_files(PluginFormat::Vst3, roots).is_empty()
        })
        .filter(|f| {
            let named = f
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| VST2_FOLDER_NAMES.contains(&n.to_ascii_lowercase().as_str()));
            named || holds_vst2_files(f, 0)
        })
        .cloned()
        .collect()
}

fn walk(dir: &Path, ext: &str, depth: usize, chain: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>) {
    if depth > MAX_SEARCH_DEPTH {
        return;
    }
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if chain.contains(&canonical) {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    chain.push(canonical);
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if has_extension(&path, ext) {
            out.push(path);
        } else if meta.is_dir() {
            walk(&path, ext, depth + 1, chain, out);
        }
    }
    chain.pop();
}
