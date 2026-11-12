//! The hand models are not part of the installer: the user downloads them once, after a dialog
//! that says what is fetched, from where and how big. This module is the store of those files and
//! the download.
//!
//! * The addresses, sizes and SHA-256 hashes are constants in this file ([`MODEL_FILES`]) and
//!   nowhere else.
//! * A download goes through the [`Fetcher`] trait. [`UreqFetcher`] is the real one (HTTPS only,
//!   no redirect, timeouts, no cookie, a generic User-Agent, nothing about the user or the
//!   machine); [`FakeFetcher`] serves bytes from memory, so no test reaches the network.
//! * Each file is written under a temporary name, its size and SHA-256 are checked against the
//!   constants, and only when every file is good are they renamed into place. A mismatch, a
//!   failed request or a cancel deletes what was written: nothing is left behind.
//! * [`ModelStore::verified_paths`] checks the files again before they are loaded, once per run
//!   of the program.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// One model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// The name in the model folder.
    pub name: &'static str,
    /// The one fixed HTTPS address it comes from.
    pub url: &'static str,
    /// The exact size in bytes.
    pub size: u64,
    /// The SHA-256 of the file, lower-case hex.
    pub sha256: &'static str,
}

/// The palm detector: finds hands in a whole frame.
pub const PALM_DETECTOR: ModelFile = ModelFile {
    name: "palm_detection_full.tflite",
    url: "https://storage.googleapis.com/mediapipe-assets/palm_detection_full.tflite",
    size: 2_339_846,
    sha256: "1b14e9422c6ad006cde6581a46c8b90dd573c07ab7f3934b5589e7cea3f89a54",
};

/// The hand landmark model: the 21 points of one hand in a crop.
pub const HAND_LANDMARKS: ModelFile = ModelFile {
    name: "hand_landmark_full.tflite",
    url: "https://storage.googleapis.com/mediapipe-assets/hand_landmark_full.tflite",
    size: 5_478_917,
    sha256: "11c272b891e1a99ab034208e23937a8008388cf11ed2a9d776ed3d01d0ba00e3",
};

/// Every file the hand tracker needs, in the order they are downloaded.
pub const MODEL_FILES: &[ModelFile] = &[PALM_DETECTOR, HAND_LANDMARKS];

/// The model files are published by Google under the Apache License 2.0 (see the model cards).
pub const MODEL_LICENCE: &str = "Apache-2.0";

/// The state of the model folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not downloaded (a file is not there).
    Missing,
    /// Every file is there and matches its size and SHA-256.
    Ready,
    /// A file is there and does not match.
    Corrupt,
}

/// How far a download is, over all the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// The file being fetched (0 based).
    pub file: usize,
    /// How many files there are.
    pub files: usize,
    /// Bytes received so far, over all files.
    pub received: u64,
    /// Bytes expected, over all files.
    pub total: u64,
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// The request could not be made or finished.
    #[error("request failed: {0}")]
    Failed(String),
    /// The server answered something else than 200.
    #[error("the server answered {0}")]
    Status(u16),
    /// The sink asked to stop.
    #[error("stopped")]
    Stopped,
}

/// Why a download did not finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DownloadError {
    /// The request failed.
    #[error("download failed: {0}")]
    Fetch(#[from] FetchError),
    /// A file is not the size it should be.
    #[error("{name} has the wrong size ({got} bytes, expected {expected})")]
    WrongSize {
        /// The file.
        name: String,
        /// The bytes received.
        got: u64,
        /// The bytes expected.
        expected: u64,
    },
    /// A file does not match its SHA-256.
    #[error("{name} does not match its checksum")]
    HashMismatch {
        /// The file.
        name: String,
    },
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
    /// A file could not be written or renamed.
    #[error("cannot write the model files: {0}")]
    Io(String),
}

/// A client that fetches one address and hands the body over in chunks.
pub trait Fetcher: Send + Sync {
    /// Streams the body of `url` to `sink`; the sink returns `false` to stop (the fetch then
    /// returns [`FetchError::Stopped`]).
    fn fetch(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), FetchError>;
}

/// The real client: HTTPS only, rustls with the bundled roots, no redirect, no cookie, timeouts,
/// a User-Agent that names only the program and its version.
#[derive(Debug, Clone, Copy, Default)]
pub struct UreqFetcher;

/// How long the connection and the answer may take, and the whole download.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The most time a whole file may take.
pub const FILE_TIMEOUT: Duration = Duration::from_secs(600);

impl Fetcher for UreqFetcher {
    fn fetch(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), FetchError> {
        let failed = |e: ureq::Error| FetchError::Failed(e.to_string());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(FILE_TIMEOUT))
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(concat!("atomik-rack/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let mut response = agent.get(url).call().map_err(failed)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(FetchError::Status(status));
        }
        let mut reader = response.body_mut().with_config().limit(64 << 20).reader();
        let mut chunk = vec![0u8; 64 << 10];
        loop {
            let n = reader
                .read(&mut chunk)
                .map_err(|e| FetchError::Failed(e.to_string()))?;
            if n == 0 {
                return Ok(());
            }
            if !sink(&chunk[..n]) {
                return Err(FetchError::Stopped);
            }
        }
    }
}

/// A fetcher that serves bytes from memory and counts the requests.
#[derive(Debug, Default)]
pub struct FakeFetcher {
    bodies: BTreeMap<String, Result<Vec<u8>, String>>,
    requests: AtomicUsize,
    chunk: usize,
}

impl FakeFetcher {
    /// A fetcher that serves nothing (every request fails) in chunks of 1000 bytes.
    pub fn new() -> Self {
        Self {
            bodies: BTreeMap::new(),
            requests: AtomicUsize::new(0),
            chunk: 1000,
        }
    }

    /// Serves `body` at `url`.
    pub fn serve(&mut self, url: &str, body: Vec<u8>) {
        self.bodies.insert(url.to_string(), Ok(body));
    }

    /// Makes the request for `url` fail.
    pub fn fail(&mut self, url: &str, message: &str) {
        self.bodies
            .insert(url.to_string(), Err(message.to_string()));
    }

    /// How many requests were made.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Fetcher for FakeFetcher {
    fn fetch(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), FetchError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        match self.bodies.get(url) {
            None => Err(FetchError::Status(404)),
            Some(Err(m)) => Err(FetchError::Failed(m.clone())),
            Some(Ok(body)) => {
                for part in body.chunks(self.chunk.max(1)) {
                    if !sink(part) {
                        return Err(FetchError::Stopped);
                    }
                }
                Ok(())
            }
        }
    }
}

/// The SHA-256 of `bytes` as lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The SHA-256 of a file, read in pieces.
fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

/// The folders whose files were already checked in this run of the program.
static VERIFIED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// The model folder.
#[derive(Debug, Clone)]
pub struct ModelStore {
    dir: PathBuf,
    files: Vec<ModelFile>,
}

enum FileState {
    Missing,
    Good,
    Bad,
}

impl ModelStore {
    /// The store in the app's data folder: `<data>/models/hands/`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::with_files(data_dir.join("models").join("hands"), MODEL_FILES.to_vec())
    }

    /// A store of other files in another folder (tests).
    pub fn with_files(dir: PathBuf, files: Vec<ModelFile>) -> Self {
        Self { dir, files }
    }

    /// The model folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The files this store holds.
    pub fn files(&self) -> &[ModelFile] {
        &self.files
    }

    /// The bytes a full download fetches.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    fn state(&self, f: &ModelFile) -> FileState {
        let path = self.dir.join(f.name);
        match std::fs::metadata(&path) {
            Err(_) => FileState::Missing,
            Ok(m) if m.len() != f.size => FileState::Bad,
            Ok(_) => match file_sha256(&path) {
                Ok(h) if h == f.sha256 => FileState::Good,
                _ => FileState::Bad,
            },
        }
    }

    /// Looks at the files (size and SHA-256 of each).
    pub fn status(&self) -> Status {
        let mut missing = false;
        for f in &self.files {
            match self.state(f) {
                FileState::Bad => return Status::Corrupt,
                FileState::Missing => missing = true,
                FileState::Good => {}
            }
        }
        if missing {
            Status::Missing
        } else {
            Status::Ready
        }
    }

    /// The paths of the files, checked by size and SHA-256 before they are loaded. The check is
    /// made once per run of the program; later calls trust it.
    pub fn verified_paths(&self) -> Result<Vec<PathBuf>, StoreError> {
        let paths: Vec<PathBuf> = self.files.iter().map(|f| self.dir.join(f.name)).collect();
        let seen = VERIFIED.lock().is_ok_and(|v| v.contains(&self.dir));
        if seen {
            return Ok(paths);
        }
        match self.status() {
            Status::Ready => {
                if let Ok(mut v) = VERIFIED.lock() {
                    v.insert(self.dir.clone());
                }
                Ok(paths)
            }
            Status::Missing => Err(StoreError::Missing),
            Status::Corrupt => Err(StoreError::Corrupt),
        }
    }

    /// Downloads what is missing or corrupt. Nothing is requested when the files are ready.
    /// `progress` is called as bytes arrive; setting `cancel` stops the download. On any error
    /// everything written is deleted.
    pub fn download(
        &self,
        fetcher: &dyn Fetcher,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<(), DownloadError> {
        let wanted: Vec<(usize, &ModelFile)> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !matches!(self.state(f), FileState::Good))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        if let Ok(mut v) = VERIFIED.lock() {
            v.remove(&self.dir);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| DownloadError::Io(e.to_string()))?;
        let total = self.total_bytes();
        let mut temps: Vec<(PathBuf, PathBuf)> = Vec::new();
        let result = self.fetch_all(fetcher, progress, cancel, &wanted, total, &mut temps);
        if result.is_err() {
            for (temp, _) in &temps {
                let _ = std::fs::remove_file(temp);
            }
            // The folder is only removed when it is empty (it may hold nothing else).
            let _ = std::fs::remove_dir(&self.dir);
            return result;
        }
        // Every file is good: put them in place.
        let mut placed: Vec<&Path> = Vec::new();
        for (temp, target) in &temps {
            if let Err(e) = std::fs::rename(temp, target) {
                for p in placed {
                    let _ = std::fs::remove_file(p);
                }
                for (t, _) in &temps {
                    let _ = std::fs::remove_file(t);
                }
                return Err(DownloadError::Io(e.to_string()));
            }
            placed.push(target);
        }
        Ok(())
    }

    fn fetch_all(
        &self,
        fetcher: &dyn Fetcher,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
        wanted: &[(usize, &ModelFile)],
        total: u64,
        temps: &mut Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), DownloadError> {
        // Bytes of the files that are not fetched count as received, so the bar reaches the end.
        let mut received: u64 = self.files.iter().map(|f| f.size).sum::<u64>()
            - wanted.iter().map(|(_, f)| f.size).sum::<u64>();
        for &(index, f) in wanted {
            let temp = self.dir.join(format!("{}.part", f.name));
            let target = self.dir.join(f.name);
            temps.push((temp.clone(), target));
            let mut out =
                std::fs::File::create(&temp).map_err(|e| DownloadError::Io(e.to_string()))?;
            let mut hasher = Sha256::new();
            let mut got: u64 = 0;
            let mut io_error: Option<String> = None;
            let mut cancelled = false;
            let fetched = fetcher.fetch(f.url, &mut |chunk| {
                if cancel.load(Ordering::SeqCst) {
                    cancelled = true;
                    return false;
                }
                got += chunk.len() as u64;
                if got > f.size {
                    return false; // more than the file can be: stop reading
                }
                if let Err(e) = out.write_all(chunk) {
                    io_error = Some(e.to_string());
                    return false;
                }
                hasher.update(chunk);
                received += chunk.len() as u64;
                progress(Progress {
                    file: index,
                    files: self.files.len(),
                    received,
                    total,
                });
                !cancel.load(Ordering::SeqCst)
            });
            drop(out);
            if cancelled || cancel.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            if let Some(e) = io_error {
                return Err(DownloadError::Io(e));
            }
            if got > f.size {
                return Err(DownloadError::WrongSize {
                    name: f.name.to_string(),
                    got,
                    expected: f.size,
                });
            }
            fetched?;
            if got != f.size {
                return Err(DownloadError::WrongSize {
                    name: f.name.to_string(),
                    got,
                    expected: f.size,
                });
            }
            if hex(&hasher.finalize()) != f.sha256 {
                return Err(DownloadError::HashMismatch {
                    name: f.name.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Why the files cannot be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The models are not downloaded.
    #[error("the hand models are not downloaded")]
    Missing,
    /// A model file is damaged.
    #[error("a hand model file is damaged; download it again")]
    Corrupt,
}
