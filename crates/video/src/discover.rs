//! Finding the user's FFmpeg.
//!
//! Nothing is bundled: video uses the `ffmpeg` and `ffprobe` programs the user installed. The
//! search order is
//!
//! 1. the folder set in Settings (its `bin` folder too, and a folder that is the `ffmpeg`
//!    program itself): when a folder is set and does not hold both programs the answer is
//!    "missing" and the system path is **not** tried, so a wrong setting is seen, not hidden;
//! 2. the folders of the `PATH` variable, in order;
//! 3. on macOS the folders the usual package managers install into, because a windowed app
//!    starts with a minimal `PATH`.
//!
//! The first folder with both programs wins; `ffmpeg -version` is then run (through the
//! [`VideoTool`], so a test needs no FFmpeg) and must state a release of at least 4. Every
//! outcome is a [`FfmpegStatus`] with a message fit for the Settings tab; nothing here can fail
//! hard, the video features are simply off.

use crate::runner::{VideoTool, capture};
use crate::version::{FfmpegVersion, MIN_MAJOR, parse_version};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// The system the search runs for (it decides the program file names and the extra folders).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Programs end in `.exe`.
    Windows,
    /// A windowed app has a minimal `PATH`: the package-manager folders are searched too.
    MacOs,
    /// Anything else.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    /// The file name of the program `base` (`ffmpeg`, `ffmpeg.exe`).
    pub fn exe_name(self, base: &str) -> String {
        match self {
            Self::Windows => format!("{base}.exe"),
            _ => base.to_owned(),
        }
    }

    /// The folders searched after `PATH`.
    fn extra_folders(self) -> &'static [&'static str] {
        match self {
            Self::MacOs => &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"],
            _ => &[],
        }
    }
}

/// The two programs that were found and run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    /// The `ffmpeg` program.
    pub ffmpeg: PathBuf,
    /// The `ffprobe` program.
    pub ffprobe: PathBuf,
    /// The version `ffmpeg` stated.
    pub version: FfmpegVersion,
}

/// What a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfmpegStatus {
    /// Not looked for yet.
    Unchecked,
    /// Usable.
    Found(Ffmpeg),
    /// No folder holds both programs.
    Missing {
        /// The folder set in Settings, when one was set (and then it is the only one searched).
        folder: Option<PathBuf>,
        /// Which program was not found.
        detail: String,
    },
    /// Found, but older than [`MIN_MAJOR`].
    TooOld {
        /// The `ffmpeg` program.
        path: PathBuf,
        /// Its version.
        version: FfmpegVersion,
    },
    /// Found, but it does not run or does not say it is FFmpeg.
    Broken {
        /// The `ffmpeg` program.
        path: PathBuf,
        /// What went wrong.
        why: String,
    },
}

impl FfmpegStatus {
    /// The programs, when they are usable.
    pub fn tools(&self) -> Option<&Ffmpeg> {
        match self {
            Self::Found(f) => Some(f),
            _ => None,
        }
    }

    /// Whether video can be used.
    pub fn is_usable(&self) -> bool {
        self.tools().is_some()
    }

    /// The line the Settings tab and the toast show.
    pub fn message(&self) -> String {
        match self {
            Self::Unchecked => "FFmpeg has not been looked for yet.".to_owned(),
            Self::Found(f) => format!(
                "{} found at {}",
                f.version,
                f.ffmpeg.parent().unwrap_or(&f.ffmpeg).display()
            ),
            Self::Missing { folder: None, .. } => {
                "FFmpeg is not installed: video needs it. Install it and set its folder here."
                    .to_owned()
            }
            Self::Missing {
                folder: Some(folder),
                detail,
            } => format!(
                "FFmpeg was not found in {} ({detail}): video needs it. Install it and set \
                 its folder here.",
                folder.display()
            ),
            Self::TooOld { path, version } => format!(
                "{} at {} is too old: video needs FFmpeg {MIN_MAJOR} or later. Install a newer \
                 one and set its folder here.",
                version,
                path.parent().unwrap_or(path).display()
            ),
            Self::Broken { path, why } => format!(
                "The program at {} does not work as FFmpeg ({why}). Install FFmpeg and set \
                 its folder here.",
                path.display()
            ),
        }
    }
}

/// Where to look.
#[derive(Debug, Clone, Copy)]
pub struct Search<'a> {
    /// The folder set in Settings (`None`: search the system path).
    pub folder: Option<&'a Path>,
    /// The `PATH` variable.
    pub path_var: Option<&'a OsStr>,
    /// The system.
    pub platform: Platform,
}

impl Search<'_> {
    /// The search of this process: the given Settings folder, the real `PATH`, this platform.
    pub fn of_process<'a>(folder: Option<&'a Path>, path_var: Option<&'a OsStr>) -> Search<'a> {
        Search {
            folder,
            path_var,
            platform: Platform::current(),
        }
    }
}

/// The folder among `candidates` that holds both programs; else the name of the missing one.
fn first_with_both(
    candidates: &[PathBuf],
    platform: Platform,
    is_file: &dyn Fn(&Path) -> bool,
) -> Result<(PathBuf, PathBuf), String> {
    let (ffmpeg_name, ffprobe_name) = (platform.exe_name("ffmpeg"), platform.exe_name("ffprobe"));
    let mut saw_ffmpeg = false;
    for dir in candidates {
        let (ffmpeg, ffprobe) = (dir.join(&ffmpeg_name), dir.join(&ffprobe_name));
        match (is_file(&ffmpeg), is_file(&ffprobe)) {
            (true, true) => return Ok((ffmpeg, ffprobe)),
            (true, false) => saw_ffmpeg = true,
            _ => {}
        }
    }
    Err(if saw_ffmpeg {
        format!("{ffprobe_name} is missing next to {ffmpeg_name}")
    } else {
        format!("no {ffmpeg_name}")
    })
}

/// The folders the Settings folder stands for: itself, its `bin` folder, and the folder holding
/// it when it names the `ffmpeg` program.
fn settings_candidates(folder: &Path, platform: Platform) -> Vec<PathBuf> {
    let program = platform.exe_name("ffmpeg");
    if folder
        .file_name()
        .is_some_and(|n| n == OsStr::new(&program))
        && let Some(parent) = folder.parent()
    {
        return vec![parent.to_path_buf()];
    }
    vec![folder.to_path_buf(), folder.join("bin")]
}

/// Looks for FFmpeg as described in the module docs and runs `ffmpeg -version` on what it finds.
/// `is_file` answers whether a path is a file (the real search passes the file system's).
pub fn discover(
    search: &Search<'_>,
    tool: &dyn VideoTool,
    is_file: &dyn Fn(&Path) -> bool,
) -> FfmpegStatus {
    let platform = search.platform;
    let found = match search.folder {
        Some(folder) => first_with_both(&settings_candidates(folder, platform), platform, is_file),
        None => {
            let mut dirs: Vec<PathBuf> = search
                .path_var
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();
            dirs.retain(|d| !d.as_os_str().is_empty());
            dirs.extend(platform.extra_folders().iter().map(PathBuf::from));
            first_with_both(&dirs, platform, is_file)
        }
    };
    let (ffmpeg, ffprobe) = match found {
        Ok(pair) => pair,
        Err(detail) => {
            return FfmpegStatus::Missing {
                folder: search.folder.map(Path::to_path_buf),
                detail,
            };
        }
    };
    let args: [OsString; 2] = ["-hide_banner".into(), "-version".into()];
    let never = AtomicBool::new(false);
    let text = match capture(tool, &ffmpeg, &args, 64 * 1024, &never) {
        Ok(text) => text,
        Err(e) => {
            return FfmpegStatus::Broken {
                path: ffmpeg,
                why: e.to_string(),
            };
        }
    };
    match parse_version(&text) {
        None => FfmpegStatus::Broken {
            path: ffmpeg,
            why: "it did not state a version".to_owned(),
        },
        Some(version) if !version.is_supported() => FfmpegStatus::TooOld {
            path: ffmpeg,
            version,
        },
        Some(version) => FfmpegStatus::Found(Ffmpeg {
            ffmpeg,
            ffprobe,
            version,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeTool;
    use std::collections::BTreeSet;

    fn files(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn run(
        folder: Option<&str>,
        path_var: &str,
        platform: Platform,
        present: &BTreeSet<PathBuf>,
        tool: &FakeTool,
    ) -> FfmpegStatus {
        let path_var = std::env::join_paths(path_var.split(';').filter(|s| !s.is_empty())).unwrap();
        discover(
            &Search {
                folder: folder.map(Path::new),
                path_var: Some(&path_var),
                platform,
            },
            tool,
            &|p| present.contains(p),
        )
    }

    #[test]
    fn the_settings_folder_beats_the_path() {
        let present = files(&[
            "/set/ffmpeg",
            "/set/ffprobe",
            "/usr/bin/ffmpeg",
            "/usr/bin/ffprobe",
        ]);
        let tool = FakeTool::new();
        let status = run(Some("/set"), "/usr/bin", Platform::Other, &present, &tool);
        assert_eq!(status.tools().unwrap().ffmpeg, PathBuf::from("/set/ffmpeg"));
        let status = run(None, "/usr/bin", Platform::Other, &present, &tool);
        assert_eq!(
            status.tools().unwrap().ffmpeg,
            PathBuf::from("/usr/bin/ffmpeg")
        );
    }

    #[test]
    fn the_first_path_folder_with_both_programs_wins() {
        let present = files(&["/a/ffmpeg", "/b/ffmpeg", "/b/ffprobe"]);
        let status = run(None, "/a;/b", Platform::Other, &present, &FakeTool::new());
        let tools = status.tools().unwrap();
        assert_eq!(tools.ffmpeg, PathBuf::from("/b/ffmpeg"));
        assert_eq!(tools.ffprobe, PathBuf::from("/b/ffprobe"));
    }

    #[test]
    fn a_missing_program_turns_video_off_with_a_clear_message() {
        let tool = FakeTool::new();
        let status = run(None, "/a;/b", Platform::Other, &BTreeSet::new(), &tool);
        assert!(matches!(status, FfmpegStatus::Missing { folder: None, .. }));
        assert!(!status.is_usable());
        assert_eq!(
            status.message(),
            "FFmpeg is not installed: video needs it. Install it and set its folder here."
        );
        // The program was never started.
        assert!(tool.calls().is_empty());
    }

    #[test]
    fn a_settings_folder_without_the_programs_does_not_fall_back_to_the_path() {
        let present = files(&["/usr/bin/ffmpeg", "/usr/bin/ffprobe"]);
        let status = run(
            Some("/empty"),
            "/usr/bin",
            Platform::Other,
            &present,
            &FakeTool::new(),
        );
        let FfmpegStatus::Missing { folder, .. } = &status else {
            panic!("{status:?}");
        };
        assert_eq!(folder.as_deref(), Some(Path::new("/empty")));
        assert!(status.message().contains("/empty"), "{}", status.message());
    }

    #[test]
    fn ffprobe_is_required_next_to_ffmpeg() {
        let present = files(&["/a/ffmpeg"]);
        let status = run(None, "/a", Platform::Other, &present, &FakeTool::new());
        let FfmpegStatus::Missing { detail, .. } = status else {
            panic!("expected missing");
        };
        assert!(detail.contains("ffprobe"), "{detail}");
    }

    #[test]
    fn the_settings_folder_may_be_the_bin_folder_parent_or_the_program() {
        let tool = FakeTool::new();
        let present = files(&["/ff/bin/ffmpeg", "/ff/bin/ffprobe"]);
        for folder in ["/ff", "/ff/bin", "/ff/bin/ffmpeg"] {
            let status = run(Some(folder), "", Platform::Other, &present, &tool);
            assert!(status.is_usable(), "{folder}: {status:?}");
        }
    }

    #[test]
    fn programs_end_in_exe_on_windows() {
        let present = files(&["/set/ffmpeg.exe", "/set/ffprobe.exe", "/set/ffmpeg"]);
        let status = run(
            Some("/set"),
            "",
            Platform::Windows,
            &present,
            &FakeTool::new(),
        );
        assert_eq!(
            status.tools().unwrap().ffprobe,
            PathBuf::from("/set/ffprobe.exe")
        );
    }

    #[test]
    fn macos_also_searches_the_package_manager_folders() {
        let present = files(&["/opt/homebrew/bin/ffmpeg", "/opt/homebrew/bin/ffprobe"]);
        let tool = FakeTool::new();
        let status = run(None, "/usr/bin", Platform::MacOs, &present, &tool);
        assert_eq!(
            status.tools().unwrap().ffmpeg,
            PathBuf::from("/opt/homebrew/bin/ffmpeg")
        );
        // Other systems do not look there.
        let status = run(None, "/usr/bin", Platform::Other, &present, &tool);
        assert!(!status.is_usable());
    }

    #[test]
    fn the_version_is_checked_and_shown() {
        let present = files(&["/a/ffmpeg", "/a/ffprobe"]);
        let tool = FakeTool::new().version("ffmpeg version 7.1.1-full Copyright (c)");
        let status = run(None, "/a", Platform::Other, &present, &tool);
        assert_eq!(
            status.message(),
            format!("FFmpeg 7.1 found at {}", Path::new("/a").display())
        );
        let calls = tool.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].exe, PathBuf::from("/a/ffmpeg"));
        assert!(calls[0].args.iter().any(|a| a == "-version"));
    }

    #[test]
    fn an_old_ffmpeg_is_refused_with_its_version() {
        let present = files(&["/a/ffmpeg", "/a/ffprobe"]);
        let tool = FakeTool::new().version("ffmpeg version 3.4.13 Copyright (c)");
        let status = run(None, "/a", Platform::Other, &present, &tool);
        assert!(matches!(status, FfmpegStatus::TooOld { .. }), "{status:?}");
        assert!(
            status.message().contains("FFmpeg 3.4"),
            "{}",
            status.message()
        );
        assert!(
            status.message().contains("4 or later"),
            "{}",
            status.message()
        );
        assert!(!status.is_usable());
    }

    #[test]
    fn a_program_that_does_not_run_or_is_not_ffmpeg_is_broken_not_a_crash() {
        let present = files(&["/a/ffmpeg", "/a/ffprobe"]);
        let status = run(
            None,
            "/a",
            Platform::Other,
            &present,
            &FakeTool::new().version_fails(),
        );
        assert!(matches!(status, FfmpegStatus::Broken { .. }), "{status:?}");
        let status = run(
            None,
            "/a",
            Platform::Other,
            &present,
            &FakeTool::new().version("hello there"),
        );
        assert!(matches!(status, FfmpegStatus::Broken { .. }), "{status:?}");
        let status = run(None, "/a", Platform::Other, &present, &FakeTool::none());
        assert!(matches!(status, FfmpegStatus::Broken { .. }), "{status:?}");
    }
}
