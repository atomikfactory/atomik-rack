//! Autosave and crash recovery plumbing that has no document in it: the writer thread of the
//! recovery snapshots, the session marker that tells the next start whether this one ended
//! cleanly, atomic file writes, and the scan for recordings a dead session left behind.
//!
//! # Design
//!
//! * **One writer.** [`RecoveryWriter`] is a single thread fed by a channel. The UI thread hands
//!   it a clone of the document; the thread serialises, writes to a temporary file, renames it
//!   and prunes the folder. One thread means two requests never prune each other's file, and
//!   the serialisation never stalls a frame.
//! * **Session marker.** [`SessionMarker`] is a file `session-<pid>-<n>.lock` in the recovery
//!   folder that the process keeps locked for as long as it lives. The operating system releases
//!   the lock when the process ends in any way, so a marker that can be locked belongs to a
//!   process that is gone: the next start learns of an unclean exit without asking whether a pid
//!   is alive (and a reused pid cannot fool it). A clean quit removes the file.
//! * **Orphan recordings.** A recording streams to `rec-<pid>-<take>-<n>.wav`
//!   (see `audio_writer`). [`scan_orphans`] lists, within bounds, such files whose process is gone.

use atomik_model::{RackDoc, save_project};
use atomik_plugin_host::RecoveryDir;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

/// Most files looked at in one folder of a scan (a folder with more is not an Atomik folder).
const SCAN_LIMIT: usize = 512;
/// Prefix of the session marker files.
const MARKER_PREFIX: &str = "session-";
/// Suffix of the session marker files.
const MARKER_SUFFIX: &str = ".lock";
/// Prefix of the folders of a session's scratch bundles (inside the OS temp folder).
pub const SCRATCH_PREFIX: &str = "atomik-rack-session-";

/// Writes `bytes` to `path` atomically: a temporary file next to it (flushed to disk), then a
/// rename, so a crash leaves the old file or the new one, never half of one.
///
/// # Errors
/// The I/O error; the temporary file is removed when the rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Moves an unusable file out of the way as `<name>.bad`; the new path, or `None` when the
/// rename failed.
pub fn quarantine(path: &Path) -> Option<PathBuf> {
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    let bad = PathBuf::from(bad);
    std::fs::rename(path, &bad).ok().map(|()| bad)
}

/// One snapshot to write.
struct Snapshot {
    doc: RackDoc,
    dir: RecoveryDir,
    name: String,
    sequence: u64,
    keep: usize,
}

enum Job {
    Snapshot(Box<Snapshot>),
    /// Answered once every earlier job is done.
    Barrier(Sender<()>),
}

/// The thread that writes the recovery snapshots. See the module docs.
#[derive(Debug)]
pub struct RecoveryWriter {
    tx: Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl RecoveryWriter {
    /// Starts the thread. `None` when the operating system refuses a thread.
    pub fn new() -> Option<Self> {
        let (tx, rx) = channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("atomik-recovery".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Snapshot(s) => {
                            let _ = write_snapshot(*s);
                        }
                        Job::Barrier(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .ok()?;
        Some(Self {
            tx: Some(tx),
            handle: Some(handle),
        })
    }

    /// Queues a snapshot of `doc` named `<name>-<sequence>` in `dir`, keeping the newest `keep`.
    /// The thread prunes unreferenced assets and serialises; the caller only cloned `doc`.
    pub fn submit(&self, doc: RackDoc, dir: RecoveryDir, name: String, sequence: u64, keep: usize) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job::Snapshot(Box::new(Snapshot {
                doc,
                dir,
                name,
                sequence,
                keep,
            })));
        }
    }

    /// Blocks until every queued snapshot is on disk (tests, the exit path).
    pub fn wait(&self) {
        let (done, wait) = channel();
        if let Some(tx) = &self.tx
            && tx.send(Job::Barrier(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }
}

impl Drop for RecoveryWriter {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn write_snapshot(mut s: Snapshot) -> std::io::Result<()> {
    s.doc.prune_unreferenced_assets();
    let text = save_project(&s.doc);
    let path = s.dir.next_path(&s.name, s.sequence);
    atomic_write(&path, text.as_bytes())?;
    s.dir.prune(s.keep)
}

/// The session marker of this process. See the module docs.
#[derive(Debug)]
pub struct SessionMarker {
    file: Option<File>,
    path: PathBuf,
}

fn marker_pid(name: &str) -> Option<u32> {
    let body = name
        .strip_prefix(MARKER_PREFIX)?
        .strip_suffix(MARKER_SUFFIX)?;
    body.split('-').next()?.parse().ok()
}

/// Whether the marker file at `path` is held by a live process: `Some(true)` locked elsewhere,
/// `Some(false)` free (its process is gone), `None` unreadable.
fn held(path: &Path) -> Option<bool> {
    let file = OpenOptions::new().write(true).open(path).ok()?;
    match file.try_lock() {
        Ok(()) => Some(false),
        Err(TryLockError::WouldBlock) => Some(true),
        Err(TryLockError::Error(_)) => None,
    }
}

impl SessionMarker {
    /// Looks in `dir` for markers of sessions that are gone, removes them, and creates this
    /// process's marker. The flag is true when a dead session was found: the previous run did
    /// not quit cleanly. `None` for the marker when the folder is not writable (recovery then
    /// still works, a crash is just not detected).
    pub fn start(dir: &Path) -> (Option<Self>, bool) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        if std::fs::create_dir_all(dir).is_err() {
            return (None, false);
        }
        let mut unclean = false;
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten().take(SCAN_LIMIT) {
                let path = entry.path();
                let is_marker = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| marker_pid(n).is_some());
                if is_marker && held(&path) == Some(false) {
                    unclean = true;
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{MARKER_PREFIX}{}-{n}{MARKER_SUFFIX}",
            std::process::id()
        ));
        let marker = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .ok()
            .and_then(|mut file| {
                let _ = writeln!(file, "pid {}", std::process::id());
                file.try_lock().ok().map(|()| file)
            })
            .map(|file| Self {
                file: Some(file),
                path,
            });
        (marker, unclean)
    }

    /// A clean quit: releases the lock and removes the file.
    pub fn clean_exit(&mut self) {
        self.file = None;
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whether process `pid` has a live session marker in `dir`. This process counts as alive.
pub fn session_alive(dir: &Path, pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    read.flatten().take(SCAN_LIMIT).any(|e| {
        let path = e.path();
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| marker_pid(n) == Some(pid))
            && held(&path) == Some(true)
    })
}

/// A recording a dead session left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanRecording {
    /// The `rec-*.wav` file.
    pub path: PathBuf,
    /// The process that was writing it.
    pub pid: u32,
    /// Its take number.
    pub take: u64,
    /// The track within the take.
    pub track: u64,
}

/// Parses `rec-<pid>-<take>-<n>.wav`.
pub fn parse_recording_name(name: &str) -> Option<(u32, u64, u64)> {
    let body = name.strip_prefix("rec-")?.strip_suffix(".wav")?;
    let mut parts = body.split('-');
    let pid = parts.next()?.parse().ok()?;
    let take = parts.next()?.parse().ok()?;
    let track = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((pid, take, track))
}

fn scan_audio_dir(audio: &Path, alive: &dyn Fn(u32) -> bool, out: &mut Vec<OrphanRecording>) {
    let Ok(read) = std::fs::read_dir(audio) else {
        return;
    };
    for entry in read.flatten().take(SCAN_LIMIT) {
        let path = entry.path();
        let Some((pid, take, track)) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(parse_recording_name)
        else {
            continue;
        };
        if !alive(pid) && path.is_file() {
            out.push(OrphanRecording {
                path,
                pid,
                take,
                track,
            });
        }
    }
}

/// The unfinished recordings of dead sessions in the `Audio/` folders of the bundle roots
/// `bundles` and in the scratch bundles under `scratch` (the OS temp folder, with how long a
/// file must have been left alone before it counts: the scratch folder is shared by every
/// Atomik of the user, whose markers may live in another configuration folder). Only the
/// folders of [`SCRATCH_PREFIX`] whose session is gone are read. `alive` says whether a process
/// is still running. The scan is bounded: a folder is read up to a fixed number of entries.
pub fn scan_orphans(
    bundles: &[PathBuf],
    scratch: Option<(&Path, Duration)>,
    alive: &dyn Fn(u32) -> bool,
) -> Vec<OrphanRecording> {
    let mut out = Vec::new();
    for root in bundles {
        scan_audio_dir(&root.join("Audio"), alive, &mut out);
    }
    if let Some((temp, quiet)) = scratch
        && let Ok(read) = std::fs::read_dir(temp)
    {
        for session in read.flatten().take(SCAN_LIMIT) {
            let session = session.path();
            let Some(pid) = session
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(SCRATCH_PREFIX))
                .and_then(|n| n.split('-').next())
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if alive(pid) {
                continue;
            }
            let Ok(bundles) = std::fs::read_dir(&session) else {
                continue;
            };
            for bundle in bundles.flatten().take(SCAN_LIMIT) {
                let mut found = Vec::new();
                scan_audio_dir(&bundle.path().join("Audio"), alive, &mut found);
                out.extend(found.into_iter().filter(|o| {
                    std::fs::metadata(&o.path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age >= quiet)
                }));
            }
        }
    }
    out.sort_by(|a, b| (a.pid, a.take, a.track, &a.path).cmp(&(b.pid, b.take, b.track, &b.path)));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
