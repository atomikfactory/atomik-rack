//! The library file (`library.json`): load with recovery, and a saver thread.
//!
//! This is the only file the file browser writes (plus its `.tmp` and `.bad` companions).
//! Loading happens once, when the path is set; saving never blocks the UI thread: the core
//! hands a JSON snapshot to [`LibrarySaver`], whose thread writes it atomically (temporary file
//! in the same folder, then a rename).

use crate::keymap_store::write_atomic;
use atomik_library::{LibraryDb, LibraryError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What [`load`] found.
#[derive(Debug)]
pub enum Loaded {
    /// A valid library.
    Db(LibraryDb),
    /// No file yet: start empty (it is created by the first save).
    Missing,
    /// Not valid: the file was moved to `library.json.bad` (`moved_to`, when the rename
    /// worked); start empty.
    Corrupt {
        /// Where the damaged file went.
        moved_to: Option<PathBuf>,
        /// What is wrong with it.
        detail: String,
    },
    /// Written by a newer app version: start empty in memory and never overwrite the file.
    Newer(u32),
    /// The file exists but cannot be read: start empty and never overwrite it.
    Unreadable(String),
}

/// Reads the library at `path`; a damaged file is renamed `<path>.bad` (replacing an older
/// one).
pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Unreadable(format!("{}: {e}", path.display())),
    };
    match LibraryDb::from_json(&text) {
        Ok(db) => Loaded::Db(db),
        Err(LibraryError::NewerVersion(v)) => Loaded::Newer(v),
        Err(LibraryError::Corrupt(detail)) => Loaded::Corrupt {
            moved_to: quarantine(path),
            detail,
        },
    }
}

/// Moves a damaged file out of the way as `<name>.bad`, replacing an older `.bad`.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    let bad = PathBuf::from(bad);
    // A rename onto an existing file fails on Windows.
    let _ = std::fs::remove_file(&bad);
    std::fs::rename(path, &bad).ok().map(|()| bad)
}

/// Writes snapshots on its own thread: one write at a time, and when several snapshots wait
/// only the newest is written.
pub struct LibrarySaver {
    tx: Option<Sender<String>>,
    /// Snapshots handed over.
    sent: u64,
    /// Snapshots written or superseded (shared with the thread).
    done: Arc<AtomicU64>,
    /// Files actually written.
    writes: Arc<AtomicU64>,
    /// The last write error, until taken.
    error: Arc<Mutex<Option<String>>>,
}

impl LibrarySaver {
    /// Starts the saver thread for `path`.
    pub fn spawn(path: PathBuf) -> Self {
        let (tx, rx) = channel::<String>();
        let done = Arc::new(AtomicU64::new(0));
        let writes = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let (d, w, e) = (Arc::clone(&done), Arc::clone(&writes), Arc::clone(&error));
        let spawned = std::thread::Builder::new()
            .name("atomik-library-save".into())
            .spawn(move || {
                while let Ok(mut json) = rx.recv() {
                    let mut taken = 1;
                    while let Ok(newer) = rx.try_recv() {
                        json = newer;
                        taken += 1;
                    }
                    let result = write_atomic(&path, &json);
                    w.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut slot) = e.lock() {
                        *slot = result.err();
                    }
                    d.fetch_add(taken, Ordering::Release);
                }
            });
        Self {
            tx: spawned.ok().map(|_| tx),
            sent: 0,
            done,
            writes,
            error,
        }
    }

    /// Hands a snapshot to the thread.
    pub fn save(&mut self, json: String) {
        if let Some(tx) = &self.tx
            && tx.send(json).is_ok()
        {
            self.sent += 1;
        }
    }

    /// True while a handed-over snapshot is not written yet.
    pub fn in_flight(&self) -> bool {
        self.done.load(Ordering::Acquire) < self.sent
    }

    /// Blocks until every handed-over snapshot is written (exit flush, tests).
    pub fn wait_idle(&self) {
        // About five seconds at most: a stuck disk must not hang the exit for ever.
        for _ in 0..5_000 {
            if !self.in_flight() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Files written so far.
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    /// The error of the last write, once.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("atomik-library-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_loads_as_missing() {
        let d = dir("missing");
        assert!(matches!(load(&d.join("library.json")), Loaded::Missing));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn corrupt_file_moves_to_bad_replacing_an_older_one() {
        let d = dir("corrupt");
        let file = d.join("library.json");
        std::fs::write(d.join("library.json.bad"), "older").unwrap();
        std::fs::write(&file, "{ not json").unwrap();
        let Loaded::Corrupt { moved_to, .. } = load(&file) else {
            panic!("expected Corrupt");
        };
        assert_eq!(moved_to, Some(d.join("library.json.bad")));
        assert!(!file.exists());
        assert_eq!(
            std::fs::read_to_string(d.join("library.json.bad")).unwrap(),
            "{ not json"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn newer_version_is_reported_and_left_alone() {
        let d = dir("newer");
        let file = d.join("library.json");
        std::fs::write(&file, r#"{"version":99}"#).unwrap();
        assert!(matches!(load(&file), Loaded::Newer(99)));
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn saver_writes_a_snapshot_that_loads_back() {
        let d = dir("saver");
        let file = d.join("library.json");
        let mut saver = LibrarySaver::spawn(file.clone());
        let mut db = LibraryDb::new();
        db.add_place(Path::new("/music"));
        saver.save(db.to_json());
        saver.wait_idle();
        assert!(!saver.in_flight());
        assert_eq!(saver.writes(), 1);
        let Loaded::Db(loaded) = load(&file) else {
            panic!("expected a db");
        };
        assert_eq!(loaded.places(), db.places());
        assert!(!d.join("library.json.tmp").exists());
        let _ = std::fs::remove_dir_all(d);
    }
}
