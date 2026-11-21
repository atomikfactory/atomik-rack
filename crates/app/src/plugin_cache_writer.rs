//! The thread that writes the plugin scan cache (`plugins.json`).
//!
//! The file can be tens of megabytes on a machine with many plugins, so the interface thread
//! never serialises or writes it. It hands a copy of the cache to [`CacheWriter`], whose one
//! thread serialises it, writes it through a temporary file and renames it over the old one.
//! Only the newest copy matters: a copy that was handed over while an earlier one was being
//! written replaces any copy still waiting (latest wins), so a burst of learned layouts is
//! written once or twice, not once each. A failed write is kept for
//! [`CacheWriter::take_error`], which the frame reports. When the writer is dropped (the exit
//! path) it waits a bounded time for the write in progress and the copy behind it, and a write
//! that takes longer is left to finish on its own thread.

use atomik_plugin_host::PluginCache;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long dropping the writer waits for the writes still to be done.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);

/// What the interface thread and the writer thread share.
#[derive(Default)]
struct Slot {
    /// The newest copy waiting to be written, and where.
    next: Option<(Box<PluginCache>, PathBuf)>,
    /// A write is in progress.
    busy: bool,
    /// The writer is being dropped: it ends when nothing is waiting.
    closing: bool,
    /// Why the last write failed, until [`CacheWriter::take_error`] takes it.
    error: Option<String>,
    /// How many writes have finished (for tests and the exit wait).
    written: u64,
    /// The thread that did the last write.
    #[cfg(test)]
    last_thread: Option<std::thread::ThreadId>,
}

struct Shared {
    slot: Mutex<Slot>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Writes scan caches on a thread of its own. See the module docs.
pub struct CacheWriter {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl CacheWriter {
    /// Starts the thread. `None` when the operating system refuses a thread.
    pub fn new() -> Option<Self> {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::default()),
            cv: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("atomik-plugin-cache".into())
            .spawn(move || run(&thread_shared))
            .ok()?;
        Some(Self {
            shared,
            handle: Some(handle),
        })
    }

    /// Hands over `cache` to be written to `path`, replacing a copy that is still waiting.
    /// Returns at once.
    pub fn submit(&self, cache: PluginCache, path: PathBuf) {
        let mut slot = self.shared.lock();
        slot.next = Some((Box::new(cache), path));
        self.shared.cv.notify_all();
    }

    /// Why the last write failed (the path and the error), once.
    pub fn take_error(&self) -> Option<String> {
        self.shared.lock().error.take()
    }

    /// True while a copy waits or is being written.
    pub fn is_busy(&self) -> bool {
        let slot = self.shared.lock();
        slot.next.is_some() || slot.busy
    }

    /// How many writes have finished.
    pub fn written(&self) -> u64 {
        self.shared.lock().written
    }

    /// Waits up to `limit` for the writes to be done; true when they are.
    pub fn wait(&self, limit: Duration) -> bool {
        let slot = self.shared.lock();
        let (slot, _) = self
            .shared
            .cv
            .wait_timeout_while(slot, limit, |s| s.next.is_some() || s.busy)
            .unwrap_or_else(PoisonError::into_inner);
        slot.next.is_none() && !slot.busy
    }
}

fn run(shared: &Shared) {
    let mut slot = shared.lock();
    loop {
        let Some((cache, path)) = slot.next.take() else {
            if slot.closing {
                return;
            }
            slot = shared.cv.wait(slot).unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        slot.busy = true;
        drop(slot);
        let result = cache.save(&path);
        drop(cache);
        slot = shared.lock();
        slot.busy = false;
        slot.written += 1;
        #[cfg(test)]
        {
            slot.last_thread = Some(std::thread::current().id());
        }
        slot.error = result
            .err()
            .map(|e| format!("{}: {e}", path.display()))
            .or(slot.error.take());
        shared.cv.notify_all();
    }
}

impl Drop for CacheWriter {
    fn drop(&mut self) {
        {
            let mut slot = self.shared.lock();
            slot.closing = true;
            self.shared.cv.notify_all();
        }
        // A write that takes longer than the wait is left to finish by itself: the exit must not
        // hang on a slow disk, and the file is replaced by a rename, so it is never half written.
        if self.wait(EXIT_WAIT)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_plugin_host::PluginCache;

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("atomik-cache-writer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("plugins.json")
    }

    #[test]
    fn a_submitted_cache_is_written_complete_on_another_thread() {
        let path = temp_file("complete");
        let writer = CacheWriter::new().expect("a thread");
        writer.submit(PluginCache::default(), path.clone());
        assert!(writer.wait(Duration::from_secs(30)), "the write finished");
        assert_eq!(writer.written(), 1);
        let wrote_on = writer.shared.lock().last_thread;
        assert!(wrote_on.is_some());
        assert_ne!(
            wrote_on,
            Some(std::thread::current().id()),
            "the file is written by the writer's own thread, not by the caller"
        );
        let (loaded, status) = PluginCache::load(&path);
        assert_eq!(status, atomik_plugin_host::LoadStatus::Loaded);
        assert_eq!(loaded, PluginCache::default());
        assert!(writer.take_error().is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_write_that_fails_is_reported_once() {
        // A folder where the file should be makes the rename fail.
        let path = temp_file("fails");
        std::fs::create_dir_all(&path).unwrap();
        let writer = CacheWriter::new().expect("a thread");
        writer.submit(PluginCache::default(), path.clone());
        assert!(writer.wait(Duration::from_secs(30)));
        let error = writer.take_error().expect("the failure is kept");
        assert!(error.contains("plugins.json"), "{error}");
        assert!(writer.take_error().is_none(), "told once");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_newest_copy_wins_over_one_that_is_still_waiting() {
        let first = temp_file("first");
        let second = temp_file("second");
        let writer = CacheWriter::new().expect("a thread");
        {
            let mut slot = writer.shared.lock();
            slot.next = Some((Box::default(), first.clone()));
            slot.next = Some((Box::default(), second.clone()));
            writer.shared.cv.notify_all();
        }
        assert!(writer.wait(Duration::from_secs(30)));
        assert!(second.exists(), "the newest copy was written");
        assert!(!first.exists(), "the replaced copy never was");
        let _ = std::fs::remove_dir_all(second.parent().unwrap());
    }

    #[test]
    fn dropping_the_writer_waits_for_the_write_that_is_queued() {
        let path = temp_file("drop");
        {
            let writer = CacheWriter::new().expect("a thread");
            writer.submit(PluginCache::default(), path.clone());
        }
        assert!(path.exists(), "the exit wrote what was queued");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
