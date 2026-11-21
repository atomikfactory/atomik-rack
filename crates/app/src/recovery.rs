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
//! * **Bounded exit.** A stalled disk or network folder must not hang the app at exit: the wait
//!   for the writer ([`RecoveryWriter::wait_for`], [`RecoveryWriter`]'s `Drop`) and the removal of
//!   the scratch folder ([`remove_dir_bounded`]) give up after [`EXIT_WAIT`]. A snapshot the
//!   writer was still writing is left to finish (or not) by itself: it is a rename of a complete
//!   temporary file, so the folder never holds half of one.
//! * **Orphan recordings.** A recording streams to `rec-<pid>-<take>-<n>.wav`
//!   (see `audio_writer`). [`scan_orphans`] lists, within bounds, such files whose process is gone.

use atomik_model::{RackDoc, save_project};
use atomik_plugin_host::RecoveryDir;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
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
/// The longest the exit waits for the recovery writer, and for the scratch folder to be removed,
/// before it goes on without them.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);

/// Runs `work` on a helper thread and waits for it up to `limit`: true when it finished. A
/// helper that is still running at the limit is left alone (it ends with the process), and so is
/// the work when no thread can be made: then `work` runs here, as it would have anyway.
pub fn run_bounded(limit: Duration, work: impl FnOnce() + Send + 'static) -> bool {
    let (done, finished) = channel::<()>();
    let work = std::sync::Arc::new(std::sync::Mutex::new(Some(work)));
    let helper = std::sync::Arc::clone(&work);
    let spawned = std::thread::Builder::new()
        .name("atomik-exit-helper".into())
        .spawn(move || {
            let job = helper.lock().ok().and_then(|mut w| w.take());
            if let Some(job) = job {
                job();
            }
            let _ = done.send(());
        });
    if spawned.is_err() {
        if let Some(job) = work.lock().ok().and_then(|mut w| w.take()) {
            job();
        }
        return true;
    }
    finished.recv_timeout(limit).is_ok()
}

/// Removes `dir` and what is in it (best effort) without waiting longer than `limit` for a
/// stalled disk: true when it is done. Whatever is left is found by the next start's scan of
/// dead sessions' folders.
pub fn remove_dir_bounded(dir: &Path, limit: Duration) -> bool {
    let dir = dir.to_path_buf();
    let done = run_bounded(limit, move || {
        let _ = std::fs::remove_dir_all(&dir);
    });
    if !done {
        log::warn!("the scratch folder is still being removed after {limit:?}; not waiting");
    }
    done
}

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
    /// The bundle folder the document's audio lives in.
    bundle: Option<PathBuf>,
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
    /// Disconnects when the thread ends (its end of the channel is dropped with it).
    ended: Option<Receiver<()>>,
    /// How long `Drop` waits for the thread.
    bound: Duration,
}

impl RecoveryWriter {
    /// Starts the thread. `None` when the operating system refuses a thread.
    pub fn new() -> Option<Self> {
        Self::spawn(EXIT_WAIT, |s| {
            let _ = write_snapshot(s);
        })
    }

    /// Starts the thread with `write` as what it does for a snapshot (the tests give it one that
    /// blocks) and `bound` as the longest `Drop` waits for it.
    fn spawn(bound: Duration, write: impl Fn(Snapshot) + Send + 'static) -> Option<Self> {
        let (tx, rx) = channel::<Job>();
        let (alive, ended) = channel::<()>();
        let handle = std::thread::Builder::new()
            .name("atomik-recovery".into())
            .spawn(move || {
                let _alive = alive;
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Snapshot(s) => write(*s),
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
            ended: Some(ended),
            bound,
        })
    }

    /// Queues a snapshot of `doc` named `<name>-<sequence>` in `dir`, keeping the newest `keep`.
    /// The thread prunes unreferenced assets and serialises; the caller only cloned `doc`.
    /// `bundle` is the folder the document's audio lives in: a note next to the snapshot keeps
    /// it, so the recovered song can find its audio again.
    pub fn submit(
        &self,
        doc: RackDoc,
        dir: RecoveryDir,
        name: String,
        sequence: u64,
        keep: usize,
        bundle: Option<PathBuf>,
    ) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job::Snapshot(Box::new(Snapshot {
                doc,
                dir,
                name,
                sequence,
                keep,
                bundle,
            })));
        }
    }

    /// Blocks until every queued snapshot is on disk (tests and offline paths, whose waits stay
    /// exact; the exit path uses [`Self::wait_for`]).
    pub fn wait(&self) {
        let (done, wait) = channel();
        if let Some(tx) = &self.tx
            && tx.send(Job::Barrier(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }

    /// Waits for every queued snapshot up to `limit`: true when they are on disk, false when the
    /// writer is still busy at the limit (a stalled disk): the caller goes on without it.
    pub fn wait_for(&self, limit: Duration) -> bool {
        let (done, wait) = channel();
        match &self.tx {
            Some(tx) if tx.send(Job::Barrier(done)).is_ok() => match wait.recv_timeout(limit) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => true,
                Err(RecvTimeoutError::Timeout) => false,
            },
            _ => true,
        }
    }
}

impl Drop for RecoveryWriter {
    fn drop(&mut self) {
        self.tx = None;
        // Joining a thread that is stuck in a write would hang the exit: past the bound the
        // thread is left to end with the process.
        let ended = self.ended.take();
        let finished = ended.is_none_or(|rx| {
            matches!(
                rx.recv_timeout(self.bound),
                Ok(()) | Err(RecvTimeoutError::Disconnected)
            )
        });
        if let Some(handle) = self.handle.take() {
            if finished {
                let _ = handle.join();
            } else {
                log::warn!(
                    "the recovery writer is still busy after {:?}; not waiting",
                    self.bound
                );
            }
        }
    }
}

fn write_snapshot(mut s: Snapshot) -> std::io::Result<()> {
    s.doc.prune_unreferenced_assets();
    let text = save_project(&s.doc);
    let path = s.dir.next_path(&s.name, s.sequence);
    atomic_write(&path, text.as_bytes())?;
    // The note is best effort: without it the snapshot still opens, its audio just shows missing.
    if let Some(bundle) = s.bundle.filter(|_| !s.doc.assets.is_empty()) {
        let _ = atomic_write(
            &RecoveryDir::bundle_note_path(&path),
            bundle.to_string_lossy().as_bytes(),
        );
    }
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

/// The folder of the recovery snapshots inside the config folder.
pub const RECOVERY_FOLDER: &str = "recovery";

/// The most songs next to the first one a start offers to recover.
pub const MAX_RECOVERED_SONGS: usize = 8;

/// The newest recovery snapshot of each song that was open next to the first one when a run
/// ended without a clean exit, oldest song folder first. The songs after the first keep their
/// snapshots in `<root>/song-<n>/`; one of those folders counts when it holds a session marker
/// whose process is gone and a snapshot. The dead markers are removed, so a later start does
/// not offer the same copies again (and a song opened in this run, which takes such a folder
/// over, does not mistake them for its own crash).
pub fn crashed_song_snapshots(root: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut folders: Vec<(u32, PathBuf)> = read
        .flatten()
        .take(SCAN_LIMIT)
        .filter_map(|entry| {
            let path = entry.path();
            let number = path
                .file_name()?
                .to_str()?
                .strip_prefix("song-")?
                .parse::<u32>()
                .ok()?;
            path.is_dir().then_some((number, path))
        })
        .collect();
    folders.sort();
    let mut found = Vec::new();
    for (_, folder) in folders {
        let mut crashed = false;
        if let Ok(read) = std::fs::read_dir(&folder) {
            for entry in read.flatten().take(SCAN_LIMIT) {
                let path = entry.path();
                let is_marker = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| marker_pid(n).is_some());
                if is_marker && held(&path) == Some(false) {
                    crashed = true;
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        if crashed
            && found.len() < MAX_RECOVERED_SONGS
            && let Some(snapshot) = RecoveryDir::new(&folder).newest()
        {
            found.push(snapshot);
        }
    }
    found
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
