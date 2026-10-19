//! Test support for the plugin tests.
//!
//! The test plugins are cdylibs built with `cargo build -p ...` (through the `CARGO`
//! environment variable) under a lock, so concurrent tests and test binaries build each one
//! once, then located by the platform's library file name in the target directory. A missing or
//! unbuildable artifact fails loudly instead of skipping the test.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, PoisonError};

/// The file name Cargo gives a cdylib crate on this platform: `<name>.dll`, `lib<name>.dylib`
/// or `lib<name>.so` (`crate_name` with underscores).
pub fn cdylib_file_name(crate_name: &str) -> String {
    if cfg!(windows) {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

/// The name a CLAP bundle file gets when a cdylib is installed for scanning.
pub fn clap_file_name(crate_name: &str) -> String {
    format!("{crate_name}.clap")
}

/// A test plugin artifact that is not built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TestingError {
    /// The artifact is not where the build should have put it.
    #[error("test plugin `{0}` is not built (expected at {1})")]
    NotBuilt(String, PathBuf),
    /// `cargo build -p <package>` failed; the second field is the tail of its output.
    #[error("building test plugin `{0}` failed: {1}")]
    BuildFailed(String, String),
    /// A file operation of the test support failed.
    #[error("test support I/O for `{0}`: {1}")]
    Io(String, String),
}

/// The target directory of this test run: `CARGO_TARGET_DIR` (a relative one is relative to the
/// workspace root, where Cargo was started) or `<root>/target`.
pub fn target_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            }
        }
        None => root.join("target"),
    }
}

/// The directory Cargo builds into for this test run, with the profile directory name (`debug`
/// or `release`).
pub fn profile_dir() -> PathBuf {
    target_dir().join(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    })
}

/// Where the built cdylib of `crate_name` should be, failing loudly if it is not there.
///
/// # Errors
/// [`TestingError::NotBuilt`] when the file does not exist.
pub fn locate_cdylib(crate_name: &str) -> Result<PathBuf, TestingError> {
    let path = profile_dir().join(cdylib_file_name(crate_name));
    if path.is_file() {
        Ok(path)
    } else {
        Err(TestingError::NotBuilt(crate_name.to_string(), path))
    }
}

/// Serialises builds inside one process (the file lock does it across processes).
static BUILD_LOCK: Mutex<()> = Mutex::new(());

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds the cdylib package `package` (for example `atomik-test-clap`) and returns the path of
/// its library. Safe to call from several tests and test binaries at once: builds are
/// serialised by an in-process mutex and a lock file in the target directory, and building an
/// up-to-date package is a no-op for Cargo.
///
/// # Errors
/// [`TestingError::BuildFailed`] when Cargo fails (an unknown package, a compile error) and
/// [`TestingError::NotBuilt`] when it succeeds but the library is not where it should be.
pub fn build_cdylib(package: &str) -> Result<PathBuf, TestingError> {
    let _in_process = BUILD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = profile_dir();
    let io = |e: std::io::Error| TestingError::Io(package.to_string(), e.to_string());
    std::fs::create_dir_all(&dir).map_err(io)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".atomik-test-plugin-build.lock"))
        .map_err(io)?;
    lock.lock().map_err(io)?;
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", target_dir())
        .args(["build", "-p", package, "--quiet"]);
    if !cfg!(debug_assertions) {
        cargo.arg("--release");
    }
    let output = cargo.output().map_err(io)?;
    // Dropping `lock` releases it; unlock explicitly so a failure still releases promptly.
    let _ = lock.unlock();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        return Err(TestingError::BuildFailed(package.to_string(), tail));
    }
    locate_cdylib(&package.replace('-', "_"))
}

/// Builds `package` and installs a copy named `<crate>.clap` in a `clap-plugins` folder of the
/// target directory (what a scanner finds), returning the copy's path. The copy is rewritten
/// only when the build is newer, so a library a test process has loaded is not replaced under it.
///
/// # Errors
/// As [`build_cdylib`], plus [`TestingError::Io`] when the copy fails.
pub fn clap_test_plugin(package: &str) -> Result<PathBuf, TestingError> {
    let built = build_cdylib(package)?;
    let folder = profile_dir().join("clap-plugins");
    let io = |e: std::io::Error| TestingError::Io(package.to_string(), e.to_string());
    std::fs::create_dir_all(&folder).map_err(io)?;
    let target = folder.join(clap_file_name(&package.replace('-', "_")));
    let _in_process = BUILD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let stale = match (std::fs::metadata(&built), std::fs::metadata(&target)) {
        (Ok(a), Ok(b)) => a.modified().map_err(io)? > b.modified().map_err(io)?,
        _ => true,
    };
    if stale {
        // A copy that is loaded elsewhere cannot be overwritten on Windows; keeping it is fine
        // because it is a copy of an earlier build of the same sources.
        let _ = std::fs::copy(&built, &target);
        if !target.is_file() {
            return Err(TestingError::NotBuilt(package.to_string(), target));
        }
    }
    Ok(target)
}

/// The binary of a `.vst3` bundle, relative to the bundle folder, for `os` and CPU `arch`
/// (`std::env::consts::ARCH` names), from the VST3 "Plug-in Format" layout: Windows
/// `Contents/<arch>-win/<stem>.vst3`, macOS `Contents/MacOS/<stem>`, Linux
/// `Contents/<arch>-linux/<stem>.so`. `None` for a CPU with no layout.
pub fn vst3_bundle_binary(
    os: crate::paths::Os,
    arch: &str,
    stem: &str,
) -> Option<std::path::PathBuf> {
    use crate::paths::Os;
    use std::path::PathBuf;
    match os {
        Os::Windows => {
            let folder = match arch {
                "x86_64" => "x86_64-win",
                "aarch64" => "arm64-win",
                "x86" => "x86-win",
                _ => return None,
            };
            Some(PathBuf::from(format!("Contents/{folder}/{stem}.vst3")))
        }
        Os::MacOs => Some(PathBuf::from(format!("Contents/MacOS/{stem}"))),
        Os::Linux => {
            let folder = match arch {
                "x86_64" => "x86_64-linux",
                "aarch64" => "aarch64-linux",
                "x86" => "i386-linux",
                _ => return None,
            };
            Some(PathBuf::from(format!("Contents/{folder}/{stem}.so")))
        }
    }
}

/// Builds `package` and installs it as a `.vst3` bundle (`<crate>.vst3`, with the layout of the
/// running operating system) in a `vst3-plugins` folder of the target directory, returning the
/// bundle's path (what a scanner finds). The binary is rewritten only when the build is newer,
/// so a library a test process has loaded is not replaced under it.
///
/// # Errors
/// As [`build_cdylib`], plus [`TestingError::Io`] when the copy fails and
/// [`TestingError::NotBuilt`] when this system has no bundle layout.
pub fn vst3_test_plugin(package: &str) -> Result<PathBuf, TestingError> {
    let built = build_cdylib(package)?;
    let stem = package.replace('-', "_");
    let io = |e: std::io::Error| TestingError::Io(package.to_string(), e.to_string());
    let bundle = profile_dir()
        .join("vst3-plugins")
        .join(format!("{stem}.vst3"));
    let relative = vst3_bundle_binary(crate::paths::Os::current(), std::env::consts::ARCH, &stem)
        .ok_or_else(|| TestingError::NotBuilt(package.to_string(), bundle.clone()))?;
    let binary = bundle.join(relative);
    let _in_process = BUILD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(parent) = binary.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    if cfg!(target_os = "macos") {
        let plist = bundle.join("Contents/Info.plist");
        if !plist.is_file() {
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n<key>CFBundleExecutable</key><string>{stem}</string>\n<key>CFBundleIdentifier</key><string>invalid.atomik.test.{stem}</string>\n<key>CFBundlePackageType</key><string>BNDL</string>\n</dict></plist>\n"
            );
            std::fs::write(&plist, text).map_err(io)?;
        }
    }
    let stale = match (std::fs::metadata(&built), std::fs::metadata(&binary)) {
        (Ok(a), Ok(b)) => a.modified().map_err(io)? > b.modified().map_err(io)?,
        _ => true,
    };
    if stale {
        // A copy that is loaded elsewhere cannot be overwritten on Windows; keeping it is fine
        // because it is a copy of an earlier build of the same sources.
        let _ = std::fs::copy(&built, &binary);
        if !binary.is_file() {
            return Err(TestingError::NotBuilt(package.to_string(), binary));
        }
    }
    Ok(bundle)
}

/// As [`vst3_test_plugin`], but installs the library as one plain `<crate>.vst3` file (the
/// single-file format that Windows hosts still find), in a `vst3-plugins-flat` folder.
///
/// # Errors
/// As [`vst3_test_plugin`].
pub fn vst3_test_plugin_file(package: &str) -> Result<PathBuf, TestingError> {
    let built = build_cdylib(package)?;
    let folder = profile_dir().join("vst3-plugins-flat");
    let io = |e: std::io::Error| TestingError::Io(package.to_string(), e.to_string());
    std::fs::create_dir_all(&folder).map_err(io)?;
    let target = folder.join(format!("{}.vst3", package.replace('-', "_")));
    let _in_process = BUILD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let stale = match (std::fs::metadata(&built), std::fs::metadata(&target)) {
        (Ok(a), Ok(b)) => a.modified().map_err(io)? > b.modified().map_err(io)?,
        _ => true,
    };
    if stale {
        let _ = std::fs::copy(&built, &target);
        if !target.is_file() {
            return Err(TestingError::NotBuilt(package.to_string(), target));
        }
    }
    Ok(target)
}
