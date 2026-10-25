//! The file browser's worker threads and every disk access it makes besides the library file.
//!
//! Three threads, all message-passing: `atomik-browse` lists folders, sub-folders, favourite
//! and tag places, the drives, and walks folders for the recursive search; `atomik-probe` reads
//! audio headers; `atomik-preview` decodes the file being previewed into the engine's ring.
//! They answer over one channel of [`BrowseMsg`]s which [`crate::AppCore`] drains with
//! `try_recv` every frame. Each submitted job ends with exactly one [`BrowseMsg::Done`] (also
//! when it panics or the result is stale), so the core can count jobs in flight.
//!
//! Nothing here writes, moves or deletes a user file: the functions only read directories,
//! metadata and audio files.

use atomik_audio_file::{StreamDecoder, probe_file};
use atomik_engine::PreviewFeed;
use atomik_library::{AudioInfo, FileKind, FileRow, classify, natural_cmp};
use atomik_ui::TreeChild;
use std::collections::{BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

/// Rows per message while walking sub-folders.
pub(crate) const WALK_BATCH: usize = 2_000;
/// The most rows a recursive walk delivers.
pub(crate) const WALK_CAP: usize = 100_000;
/// Probe results per message.
pub(crate) const PROBE_BATCH: usize = 256;
/// Frames of the first decoded chunk (starts the sound quickly).
const FIRST_CHUNK: usize = 2_048;
/// Frames of every later chunk.
const CHUNK: usize = 8_192;
/// Paths probed between two looks at the request channel (newer requests go first).
const PROBE_SLICE: usize = 16;

/// A request for the `atomik-browse` thread.
#[derive(Debug)]
pub(crate) enum BrowseJob {
    /// Folders and audio files of `path`.
    List { generation: u64, path: PathBuf },
    /// Sub-folders of `path` for the tree.
    Children(PathBuf),
    /// Audio files below `root` at depth 2 and deeper, streamed in batches.
    Walk { generation: u64, root: PathBuf },
    /// Rows for these files (favourites and tag places); missing files are left out.
    Stat {
        generation: u64,
        paths: Vec<PathBuf>,
    },
    /// Drives (or `/`) and the home folder.
    Roots,
}

/// An answer from a worker.
#[derive(Debug)]
pub(crate) enum BrowseMsg {
    /// A folder listing, or why it failed.
    Listed {
        generation: u64,
        path: PathBuf,
        result: Result<Vec<FileRow>, String>,
    },
    /// Sub-folders of a tree node.
    Children {
        path: PathBuf,
        children: Vec<TreeChild>,
    },
    /// A batch of walked rows.
    Walked { generation: u64, rows: Vec<FileRow> },
    /// Rows for a favourites or tag place.
    Stats { generation: u64, rows: Vec<FileRow> },
    /// Tree roots.
    Roots(Vec<PathBuf>),
    /// Probed headers.
    Info(Vec<(PathBuf, AudioInfo)>),
    /// The decoder of this preview opened; its length when stated.
    PreviewTotal { path: PathBuf, frames: Option<u64> },
    /// A preview could not be decoded (or broke off).
    PreviewFailed { path: PathBuf, error: String },
    /// One submitted job (or probe request) ended.
    Done,
}

/// A request for the `atomik-preview` thread.
pub(crate) struct PreviewJob {
    /// The file to decode.
    pub(crate) path: PathBuf,
    /// The decoder side of the ring whose stream the engine plays.
    pub(crate) feed: PreviewFeed,
}

/// The sending side of the three workers plus the shared generation counter.
pub(crate) struct Workers {
    browse: Sender<BrowseJob>,
    probe: Sender<Vec<PathBuf>>,
    preview: Sender<PreviewJob>,
    /// Answers of all three workers.
    pub(crate) rx: Receiver<BrowseMsg>,
    /// Bumped by the core whenever a listing is replaced; walks stop when theirs is stale.
    pub(crate) generation: Arc<AtomicU64>,
}

impl Workers {
    /// Starts the three threads.
    pub(crate) fn spawn() -> Self {
        let (tx, rx) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let (browse, browse_rx) = channel();
        let (probe, probe_rx) = channel();
        let (preview, preview_rx) = channel();
        let t = tx.clone();
        let g = Arc::clone(&generation);
        spawn_named("atomik-browse", move || browse_thread(&browse_rx, &t, &g));
        let t = tx.clone();
        spawn_named("atomik-probe", move || probe_thread(&probe_rx, &t));
        spawn_named("atomik-preview", move || preview_thread(&preview_rx, &tx));
        Self {
            browse,
            probe,
            preview,
            rx,
            generation,
        }
    }

    /// Sends a job to the browse thread; false when the thread is gone.
    pub(crate) fn browse(&self, job: BrowseJob) -> bool {
        self.browse.send(job).is_ok()
    }

    /// Queues paths for probing (visible rows first); false when the thread is gone.
    pub(crate) fn probe(&self, paths: Vec<PathBuf>) -> bool {
        self.probe.send(paths).is_ok()
    }

    /// Hands a preview to the decoder thread; false when the thread is gone.
    pub(crate) fn preview(&self, job: PreviewJob) -> bool {
        self.preview.send(job).is_ok()
    }

    /// The current generation.
    pub(crate) fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Starts a new generation (older listings and walks become stale) and returns it.
    pub(crate) fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    // A failed spawn leaves the senders without a receiver: sends fail and the core reports
    // nothing is in flight.
    let _ = std::thread::Builder::new().name(name.into()).spawn(f);
}

// ---------------------------------------------------------------------------------- browse

struct BrowseCtx<'a> {
    tx: &'a Sender<BrowseMsg>,
    generation: &'a AtomicU64,
    /// The most rows a walk delivers.
    cap: usize,
}

fn browse_thread(rx: &Receiver<BrowseJob>, tx: &Sender<BrowseMsg>, generation: &AtomicU64) {
    let ctx = BrowseCtx {
        tx,
        generation,
        cap: WALK_CAP,
    };
    let mut backlog: VecDeque<BrowseJob> = VecDeque::new();
    loop {
        let job = match backlog.pop_front() {
            Some(j) => j,
            None => match rx.recv() {
                Ok(j) => j,
                Err(_) => return,
            },
        };
        if !run_guarded(&ctx, job, rx, &mut backlog) {
            return;
        }
    }
}

/// Runs one job, then sends its `Done`, even after a panic. False when the core is gone.
fn run_guarded(
    ctx: &BrowseCtx<'_>,
    job: BrowseJob,
    rx: &Receiver<BrowseJob>,
    backlog: &mut VecDeque<BrowseJob>,
) -> bool {
    let alive = catch_unwind(AssertUnwindSafe(|| run_job(ctx, job, rx, backlog))).unwrap_or(true);
    alive && ctx.tx.send(BrowseMsg::Done).is_ok()
}

/// False when the answer channel is closed.
fn run_job(
    ctx: &BrowseCtx<'_>,
    job: BrowseJob,
    rx: &Receiver<BrowseJob>,
    backlog: &mut VecDeque<BrowseJob>,
) -> bool {
    match job {
        BrowseJob::List { generation, path } => {
            let result = list_folder(&path);
            ctx.tx
                .send(BrowseMsg::Listed {
                    generation,
                    path,
                    result,
                })
                .is_ok()
        }
        BrowseJob::Children(path) => {
            let children = list_children(&path);
            ctx.tx.send(BrowseMsg::Children { path, children }).is_ok()
        }
        BrowseJob::Stat { generation, paths } => {
            let rows = stat_files(&paths);
            ctx.tx.send(BrowseMsg::Stats { generation, rows }).is_ok()
        }
        BrowseJob::Roots => ctx.tx.send(BrowseMsg::Roots(roots())).is_ok(),
        BrowseJob::Walk { generation, root } => walk(ctx, generation, &root, rx, backlog),
    }
}

/// Folders and audio files of `path`: no dot names, sizes from metadata, no header data.
pub(crate) fn list_folder(path: &Path) -> Result<Vec<FileRow>, String> {
    let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        if let Some(row) = row_of(&entry, true) {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// The row of a directory entry, or `None` for hidden names and non-audio files. Symlinks are
/// resolved when `follow` is set (listing) and treated as files otherwise (walking).
fn row_of(entry: &std::fs::DirEntry, follow: bool) -> Option<FileRow> {
    let name = entry.file_name().to_string_lossy().into_owned();
    if name.starts_with('.') {
        return None;
    }
    let path = entry.path();
    let file_type = entry.file_type().ok()?;
    let (is_dir, size) = if file_type.is_symlink() && follow {
        let meta = std::fs::metadata(&path).ok()?;
        (meta.is_dir(), meta.len())
    } else if file_type.is_dir() {
        (true, 0)
    } else {
        (false, entry.metadata().map_or(0, |m| m.len()))
    };
    let kind = classify(&path, is_dir)?;
    Some(FileRow {
        path,
        name,
        kind,
        size: if is_dir { 0 } else { size },
        info: None,
        item: None,
        favorite: false,
        tags: Vec::new(),
    })
}

/// Sub-folders of `path`, naturally sorted, hidden names left out.
pub(crate) fn list_children(path: &Path) -> Vec<TreeChild> {
    let mut out: Vec<TreeChild> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = if name.starts_with('.') {
                false
            } else if entry.file_type().ok()?.is_symlink() {
                std::fs::metadata(entry.path()).ok()?.is_dir()
            } else {
                entry.file_type().ok()?.is_dir()
            };
            is_dir.then(|| TreeChild {
                name,
                path: entry.path(),
            })
        })
        .collect();
    out.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    out
}

/// `path` with its file name spelled as on disk, when that differs only by case.
fn disk_spelling(path: &Path) -> PathBuf {
    let on_disk = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.file_name().map(ToOwned::to_owned));
    match (on_disk, path.file_name()) {
        (Some(disk), Some(given))
            if disk != given
                && disk.to_string_lossy().to_lowercase()
                    == given.to_string_lossy().to_lowercase() =>
        {
            path.with_file_name(disk)
        }
        _ => path.to_path_buf(),
    }
}

/// Rows for existing audio files; missing or non-audio paths are skipped.
pub(crate) fn stat_files(paths: &[PathBuf]) -> Vec<FileRow> {
    paths
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            if meta.is_dir() {
                return None;
            }
            // Library keys may be case-folded: take the file name's spelling from the disk, but keep the
            // folders as given so the row still finds its library entry behind a symlink or 8.3 name.
            let path = disk_spelling(path);
            let kind = classify(&path, false)?;
            Some(FileRow {
                name: path.file_name()?.to_string_lossy().into_owned(),
                path,
                kind,
                size: meta.len(),
                info: None,
                item: None,
                favorite: false,
                tags: Vec::new(),
            })
        })
        .collect()
}

/// The user's home folder from the environment (no disk access).
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Drives (existing `A:`..`Z:` roots on Windows, `/` elsewhere) and the home folder.
pub(crate) fn roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", char::from(letter)));
            if root.exists() {
                out.push(root);
            }
        }
    } else {
        out.push(PathBuf::from("/"));
    }
    if let Some(home) = home_dir().filter(|h| h.is_dir() && !out.contains(h)) {
        out.push(home);
    }
    out
}

/// Streams the audio files below `root` (depth 2 and deeper) in batches until done, capped or
/// stale. Jobs that arrive meanwhile run at once (the tree must not wait for a long walk);
/// another walk waits in `backlog` (it is newer, so this one is stale anyway).
fn walk(
    ctx: &BrowseCtx<'_>,
    generation: u64,
    root: &Path,
    rx: &Receiver<BrowseJob>,
    backlog: &mut VecDeque<BrowseJob>,
) -> bool {
    let stale = || ctx.generation.load(Ordering::Acquire) != generation;
    let mut dirs: Vec<PathBuf> = sub_dirs(root);
    let mut batch: Vec<FileRow> = Vec::new();
    let mut total = 0usize;
    while let Some(dir) = dirs.pop() {
        loop {
            match rx.try_recv() {
                Ok(job @ BrowseJob::Walk { .. }) => backlog.push_back(job),
                Ok(job) => {
                    let alive = catch_unwind(AssertUnwindSafe(|| {
                        let mut none = VecDeque::new();
                        run_job(ctx, job, rx, &mut none)
                    }))
                    .unwrap_or(true);
                    if !alive || ctx.tx.send(BrowseMsg::Done).is_err() {
                        return false;
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if stale() {
            return true;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut sub: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Some(row) = row_of(&entry, false) else {
                continue;
            };
            if row.kind == FileKind::Folder {
                sub.push(row.path);
            } else {
                batch.push(row);
                total += 1;
                if batch.len() >= WALK_BATCH {
                    let rows = std::mem::take(&mut batch);
                    if ctx.tx.send(BrowseMsg::Walked { generation, rows }).is_err() {
                        return false;
                    }
                }
                if total >= ctx.cap {
                    break;
                }
            }
        }
        if total >= ctx.cap {
            break;
        }
        // Reverse natural order, so popping visits folders in natural order.
        sub.sort_by(|a, b| natural_cmp(&name_of(b), &name_of(a)));
        dirs.extend(sub);
    }
    if !batch.is_empty() && !stale() {
        return ctx
            .tx
            .send(BrowseMsg::Walked {
                generation,
                rows: batch,
            })
            .is_ok();
    }
    true
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The direct sub-folders of `root`, in reverse natural order (a stack to pop from).
fn sub_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| row_of(&e, false))
        .filter(|r| r.kind == FileKind::Folder)
        .map(|r| r.path)
        .collect();
    dirs.sort_by(|a, b| natural_cmp(&name_of(b), &name_of(a)));
    dirs
}

// ----------------------------------------------------------------------------------- probe

fn probe_thread(rx: &Receiver<Vec<PathBuf>>, tx: &Sender<BrowseMsg>) {
    // Newest requests first: the rows on screen are the latest ones asked for.
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    let mut queued: BTreeSet<PathBuf> = BTreeSet::new();
    let mut requests = 0usize;
    loop {
        if queue.is_empty() {
            if requests > 0 && !finish_requests(tx, &mut requests) {
                return;
            }
            match rx.recv() {
                Ok(paths) => {
                    requests += 1;
                    enqueue(&mut queue, &mut queued, paths);
                }
                Err(_) => return,
            }
        }
        while let Ok(paths) = rx.try_recv() {
            requests += 1;
            enqueue(&mut queue, &mut queued, paths);
        }
        let mut results: Vec<(PathBuf, AudioInfo)> = Vec::new();
        for _ in 0..PROBE_SLICE {
            let Some(path) = queue.pop_front() else {
                break;
            };
            queued.remove(&path);
            if let Ok(Ok(info)) = catch_unwind(AssertUnwindSafe(|| probe_file(&path))) {
                results.push((path, info));
            }
            if results.len() >= PROBE_BATCH {
                break;
            }
        }
        if !results.is_empty() && tx.send(BrowseMsg::Info(results)).is_err() {
            return;
        }
    }
}

/// Newer paths go to the front (in their own order); already queued ones move up.
fn enqueue(queue: &mut VecDeque<PathBuf>, queued: &mut BTreeSet<PathBuf>, paths: Vec<PathBuf>) {
    let fresh: BTreeSet<&PathBuf> = paths.iter().collect();
    queue.retain(|p| !fresh.contains(p));
    for path in paths.into_iter().rev() {
        queued.insert(path.clone());
        queue.push_front(path);
    }
}

fn finish_requests(tx: &Sender<BrowseMsg>, requests: &mut usize) -> bool {
    for _ in 0..std::mem::take(requests) {
        if tx.send(BrowseMsg::Done).is_err() {
            return false;
        }
    }
    true
}

// --------------------------------------------------------------------------------- preview

fn preview_thread(rx: &Receiver<PreviewJob>, tx: &Sender<BrowseMsg>) {
    let mut next: Option<PreviewJob> = None;
    loop {
        let job = match next.take() {
            Some(j) => j,
            None => match rx.recv() {
                Ok(j) => j,
                Err(_) => return,
            },
        };
        let ended = catch_unwind(AssertUnwindSafe(|| decode_into(job, rx, tx, &mut next)));
        if ended.is_err()
            && tx
                .send(BrowseMsg::PreviewFailed {
                    path: PathBuf::new(),
                    error: "the decoder stopped unexpectedly".into(),
                })
                .is_err()
        {
            return;
        }
        if tx.send(BrowseMsg::Done).is_err() {
            return;
        }
    }
}

/// Decodes `job` into its feed until the end, an abandoned stream or a newer job (kept in
/// `next`; only the newest of several is kept, the others end at once).
fn decode_into(
    job: PreviewJob,
    rx: &Receiver<PreviewJob>,
    tx: &Sender<BrowseMsg>,
    next: &mut Option<PreviewJob>,
) {
    let PreviewJob { path, mut feed } = job;
    let mut decoder = match StreamDecoder::open(&path) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(BrowseMsg::PreviewFailed {
                path,
                error: e.to_string(),
            });
            return;
        }
    };
    let info = decoder.info();
    feed.set_source_rate(info.sample_rate);
    let _ = tx.send(BrowseMsg::PreviewTotal {
        path: path.clone(),
        frames: info.frames,
    });
    let mut want = FIRST_CHUNK;
    // A decoded chunk the ring did not take completely yet, and how much of it it took.
    let mut held: Option<(Vec<Vec<f32>>, usize)> = None;
    loop {
        match rx.try_recv() {
            Ok(newer) => {
                *next = Some(newer);
                // Several queued: the newest wins, the older ones end without a sound.
                while let Ok(newest) = rx.try_recv() {
                    *next = Some(newest);
                    let _ = tx.send(BrowseMsg::Done);
                }
                return;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return,
        }
        if feed.is_abandoned() {
            return;
        }
        if let Some((chunk, from)) = held.as_mut() {
            *from += feed.push(chunk, *from);
            if chunk.first().is_none_or(|c| *from >= c.len()) {
                held = None;
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
            continue;
        }
        if feed.free_frames() < want {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        match decoder.next_chunk(want) {
            Ok(Some(chunk)) => {
                held = Some((chunk, 0));
                want = CHUNK;
            }
            Ok(None) => {
                feed.finish();
                return;
            }
            Err(e) => {
                // Forgiving: what was decoded plays out.
                let _ = tx.send(BrowseMsg::PreviewFailed {
                    path,
                    error: e.to_string(),
                });
                feed.finish();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "atomik-browser-workers-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    /// Runs a walk with a fresh channel and returns the batches it sent.
    fn walk_batches(root: &Path, generation: u64, current: u64) -> Vec<Vec<FileRow>> {
        walk_capped(root, generation, current, WALK_CAP)
    }

    fn walk_capped(root: &Path, generation: u64, current: u64, cap: usize) -> Vec<Vec<FileRow>> {
        let (tx, rx) = channel();
        let (_jobs_tx, jobs_rx) = channel();
        let shared = AtomicU64::new(current);
        let ctx = BrowseCtx {
            tx: &tx,
            generation: &shared,
            cap,
        };
        assert!(walk(&ctx, generation, root, &jobs_rx, &mut VecDeque::new()));
        drop(tx);
        rx.try_iter()
            .map(|m| match m {
                BrowseMsg::Walked { rows, .. } => rows,
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn listing_skips_hidden_and_non_audio_and_reads_sizes() {
        let dir = tmp("list");
        touch(&dir, "a.wav");
        touch(&dir, "b.txt");
        touch(&dir, ".hidden.wav");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let mut rows = list_folder(&dir).unwrap();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a.wav", "sub"]);
        assert_eq!(rows[0].size, 1);
        assert_eq!(rows[1].size, 0);
        assert!(rows.iter().all(|r| r.info.is_none()));
        assert!(list_folder(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn children_are_folders_only_in_natural_order() {
        let dir = tmp("children");
        for name in ["pad 10", "pad 2", ".git", "Alpha"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        touch(&dir, "file.wav");
        let names: Vec<String> = list_children(&dir).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Alpha", "pad 2", "pad 10"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn stat_recovers_the_on_disk_spelling_from_a_lowercase_path() {
        let dir = tmp("spelling");
        touch(&dir, "Kick 01.WAV");
        let lower = PathBuf::from(dir.join("Kick 01.WAV").to_string_lossy().to_lowercase());
        let rows = stat_files(&[lower]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Kick 01.WAV");
        assert_eq!(rows[0].path.file_name().unwrap(), "Kick 01.WAV");
        assert!(!rows[0].path.to_string_lossy().starts_with(r"\\?\"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn stat_keeps_the_given_folders_behind_a_symlink() {
        let dir = tmp("link");
        touch(&dir.join("real"), "a.wav");
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let given = dir.join("link").join("a.wav");
        let rows = stat_files(std::slice::from_ref(&given));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, given);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stat_skips_missing_folders_and_non_audio() {
        let dir = tmp("stat");
        touch(&dir, "a.wav");
        touch(&dir, "b.txt");
        std::fs::create_dir_all(dir.join("d.wav")).unwrap();
        let rows = stat_files(&[
            dir.join("a.wav"),
            dir.join("gone.wav"),
            dir.join("b.txt"),
            dir.join("d.wav"),
        ]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "a.wav");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn walk_finds_depth_two_and_deeper_only_and_skips_hidden() {
        let dir = tmp("walk");
        touch(&dir, "top.wav");
        touch(&dir.join("a"), "one.wav");
        touch(&dir.join("a").join("b"), "two.flac");
        touch(&dir.join(".hid"), "no.wav");
        touch(&dir.join("a"), "note.txt");
        let batches = walk_batches(&dir, 1, 1);
        let mut names: Vec<String> = batches.into_iter().flatten().map(|r| r.name).collect();
        names.sort();
        assert_eq!(names, ["one.wav", "two.flac"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn walk_streams_batches_of_at_most_the_batch_size() {
        let dir = tmp("batches");
        for sub in ["x", "y"] {
            for i in 0..1_100 {
                touch(&dir.join(sub), &format!("f{i}.wav"));
            }
        }
        let batches = walk_batches(&dir, 1, 1);
        assert!(batches.len() >= 2);
        assert!(batches.iter().all(|b| b.len() <= WALK_BATCH));
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 2_200);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn walk_stops_at_the_cap() {
        let dir = tmp("cap");
        for sub in ["x", "y", "z"] {
            for i in 0..100 {
                touch(&dir.join(sub), &format!("f{i}.wav"));
            }
        }
        let total: usize = walk_capped(&dir, 1, 1, 150).iter().map(Vec::len).sum();
        assert_eq!(total, 150);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_walk_stops_without_sending() {
        let dir = tmp("stale");
        touch(&dir.join("a"), "one.wav");
        assert!(walk_batches(&dir, 1, 2).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn roots_are_not_empty_and_exist() {
        let roots = roots();
        assert!(!roots.is_empty());
        assert!(roots.iter().all(|r| r.exists()));
    }

    #[test]
    fn enqueue_puts_newer_paths_first_without_duplicates() {
        let mut queue = VecDeque::new();
        let mut queued = BTreeSet::new();
        let p = |s: &str| PathBuf::from(s);
        enqueue(&mut queue, &mut queued, vec![p("a"), p("b")]);
        enqueue(&mut queue, &mut queued, vec![p("c"), p("b")]);
        assert_eq!(queue, [p("c"), p("b"), p("a")]);
    }
}
