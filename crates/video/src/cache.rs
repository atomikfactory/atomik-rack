//! The per-user cache of proxy files.
//!
//! **Identity.** A film is identified cheaply, without reading it all: its absolute path, its
//! size, its modification time and a hash of its first and last mebibyte ([`SourceIdentity`]).
//! The key of a proxy is the hash of those and of [`PROXY_VERSION`], written as 32 hexadecimal
//! digits; the file is `<key>.atproxy`. Change the film (re-render it, move it, replace it by a
//! file of the same name) and the key changes, so the old proxy is never shown for the new film.
//! The version is also stored in the proxy's header, so a proxy of an older format is refused by
//! the reader and rebuilt.
//!
//! **Size policy.** The cache holds at most [`DEFAULT_MAX_BYTES`] (2 GiB) of proxies. After each
//! build [`ProxyCache::prune`] removes the least recently used proxies (a proxy that is used is
//! touched, so its modification time is its last use) until the total fits, never the proxy that
//! was just built or used, and removes `.part` files that no job is writing. Nothing else in the
//! folder is touched.

use crate::VideoError;
use crate::proxy::{PROXY_VERSION, ProxyReader};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The size the cache is pruned to, in bytes.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The extension of a finished proxy.
pub const PROXY_EXTENSION: &str = "atproxy";

/// How much of the start and of the end of a film goes into its identity.
const EDGE_BYTES: u64 = 1024 * 1024;

/// What identifies a film without reading it all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    /// The absolute path, as text.
    pub path: String,
    /// The size in bytes.
    pub size: u64,
    /// The modification time in nanoseconds since 1970 (0 when the system does not say).
    pub modified_ns: u128,
    /// The SHA-256 of the first and the last mebibyte.
    pub edges: [u8; 32],
}

impl SourceIdentity {
    /// Reads the identity of `path`: its metadata and its first and last mebibyte.
    ///
    /// # Errors
    /// [`VideoError::Io`] when the file cannot be read.
    pub fn read(path: &Path) -> Result<Self, VideoError> {
        let absolute = std::path::absolute(path)?;
        let mut file = File::open(&absolute)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(VideoError::Io(io::Error::other("it is not a file")));
        }
        let size = meta.len();
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let mut hash = Sha256::new();
        let head = EDGE_BYTES.min(size);
        let mut buf = vec![0u8; head as usize];
        file.read_exact(&mut buf)?;
        hash.update(&buf);
        let tail = EDGE_BYTES.min(size);
        file.seek(SeekFrom::Start(size - tail))?;
        buf.resize(tail as usize, 0);
        file.read_exact(&mut buf)?;
        hash.update(&buf);
        Ok(Self {
            path: absolute.to_string_lossy().into_owned(),
            size,
            modified_ns,
            edges: hash.finalize().into(),
        })
    }

    /// The cache key: 32 hexadecimal digits of the hash of the identity and the format version.
    pub fn key(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"atomik-video-proxy");
        hash.update(PROXY_VERSION.to_le_bytes());
        hash.update((self.path.len() as u64).to_le_bytes());
        hash.update(self.path.as_bytes());
        hash.update(self.size.to_le_bytes());
        hash.update(self.modified_ns.to_le_bytes());
        hash.update(self.edges);
        hash.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// What a prune did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pruned {
    /// Proxies removed to fit the size.
    pub proxies: usize,
    /// Unfinished files removed.
    pub partials: usize,
    /// The bytes freed.
    pub bytes: u64,
}

/// The folder of proxy files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl ProxyCache {
    /// A cache in `dir` (made when the first proxy is written) of at most
    /// [`DEFAULT_MAX_BYTES`].
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// The same cache with another size limit.
    pub fn with_limit(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file a finished proxy of `key` has.
    pub fn proxy_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.{PROXY_EXTENSION}"))
    }

    /// The file a proxy of `key` is written to until it is finished.
    pub fn part_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.{PROXY_EXTENSION}.part"))
    }

    /// The finished proxy of `key`, when there is one this build can read (the header and the
    /// index are checked, not the frames). A hit counts as a use. A proxy that cannot be read
    /// is removed so it is rebuilt.
    pub fn lookup(&self, key: &str) -> Option<PathBuf> {
        let path = self.proxy_path(key);
        let file = File::open(&path).ok()?;
        match ProxyReader::open(file) {
            Ok(_) => {
                touch(&path);
                Some(path)
            }
            Err(e) => {
                log::info!("video proxy {} is rebuilt: {e}", path.display());
                let _ = fs::remove_file(&path);
                None
            }
        }
    }

    /// Prunes the folder as described in the module docs. `keep` is a proxy that stays whatever
    /// the size. Nothing is removed from a folder that does not exist.
    ///
    /// # Errors
    /// [`VideoError::Io`] when the folder cannot be listed; a file that cannot be removed (a
    /// viewer holds it open) is skipped.
    pub fn prune(&self, keep: Option<&Path>) -> Result<Pruned, VideoError> {
        let mut done = Pruned::default();
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(done),
            Err(e) => return Err(e.into()),
        };
        let mut proxies: Vec<(std::time::SystemTime, String, PathBuf, u64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(&format!(".{PROXY_EXTENSION}.part")) {
                if fs::remove_file(&path).is_ok() {
                    done.partials += 1;
                    done.bytes += meta.len();
                }
            } else if name.ends_with(&format!(".{PROXY_EXTENSION}")) {
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                proxies.push((modified, name, path, meta.len()));
            }
        }
        // Oldest first; the name breaks a tie so the order does not depend on the listing.
        proxies.sort();
        let mut total: u64 = proxies.iter().map(|p| p.3).sum();
        for (_, _, path, len) in proxies {
            if total <= self.max_bytes {
                break;
            }
            if keep.is_some_and(|k| k == path) {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                done.proxies += 1;
                done.bytes += len;
            }
        }
        Ok(done)
    }
}

/// Marks `path` as used now (its modification time), for the least-recently-used order.
fn touch(path: &Path) {
    // Cache housekeeping on a worker, not engine or device behaviour: the wall clock is the
    // point here.
    #[allow(clippy::disallowed_methods)]
    let now = std::time::SystemTime::now();
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::tiny_jpeg;
    use crate::probe::Rational;
    use crate::proxy::ProxyWriter;
    use std::time::{Duration, SystemTime};

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "atomik-video-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn identity() -> SourceIdentity {
        SourceIdentity {
            path: "/films/a.mp4".into(),
            size: 1000,
            modified_ns: 5,
            edges: [3; 32],
        }
    }

    #[test]
    fn the_key_is_stable_and_changes_with_every_part_of_the_identity() {
        let base = identity();
        let key = base.key();
        assert_eq!(key.len(), 32);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(key, identity().key());
        let changed = [
            SourceIdentity {
                path: "/films/b.mp4".into(),
                ..identity()
            },
            SourceIdentity {
                size: 1001,
                ..identity()
            },
            SourceIdentity {
                modified_ns: 6,
                ..identity()
            },
            SourceIdentity {
                edges: [4; 32],
                ..identity()
            },
        ];
        for other in changed {
            assert_ne!(other.key(), key, "{other:?}");
        }
    }

    #[test]
    fn the_identity_reads_the_file_and_sees_an_edit_at_either_end() {
        let dir = Dir::new("identity");
        let path = dir.0.join("film.bin");
        let mut data = vec![7u8; 3 * 1024 * 1024];
        fs::write(&path, &data).unwrap();
        let a = SourceIdentity::read(&path).unwrap();
        assert_eq!(a.size, data.len() as u64);
        assert!(std::path::Path::new(&a.path).is_absolute());
        assert_eq!(a, SourceIdentity::read(&path).unwrap());
        // The same size and a byte changed in the last mebibyte (the time is put back).
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        *data.last_mut().unwrap() = 8;
        fs::write(&path, &data).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(before).unwrap();
        drop(file);
        let b = SourceIdentity::read(&path).unwrap();
        assert_eq!((a.size, a.modified_ns), (b.size, b.modified_ns));
        assert_ne!(a.key(), b.key());
        // A byte in the middle is not looked at (the identity is cheap on purpose).
        data[1_500_000] = 9;
        fs::write(&path, &data).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(before).unwrap();
        drop(file);
        assert_eq!(SourceIdentity::read(&path).unwrap().key(), b.key());
        // A small file is read whole; a missing one is an error.
        fs::write(dir.0.join("tiny"), b"abc").unwrap();
        assert!(SourceIdentity::read(&dir.0.join("tiny")).is_ok());
        assert!(SourceIdentity::read(&dir.0.join("nothing")).is_err());
        assert!(SourceIdentity::read(&dir.0).is_err());
    }

    fn store(cache: &ProxyCache, key: &str, frames: u8) -> PathBuf {
        fs::create_dir_all(cache.dir()).unwrap();
        let path = cache.proxy_path(key);
        let file = File::create(&path).unwrap();
        let mut w = ProxyWriter::new(file, Rational { num: 25, den: 1 }).unwrap();
        for i in 0..frames {
            w.push_frame(&tiny_jpeg(16, 16, i)).unwrap();
        }
        w.finish().unwrap();
        path
    }

    fn age(path: &Path, seconds: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + seconds))
            .unwrap();
    }

    #[test]
    fn a_stored_proxy_is_found_and_a_missing_or_old_one_is_not() {
        let dir = Dir::new("lookup");
        let cache = ProxyCache::new(dir.0.join("cache"));
        assert_eq!(cache.lookup("aa"), None);
        let path = store(&cache, "aa", 3);
        assert_eq!(cache.lookup("aa"), Some(path.clone()));
        // A proxy of another version is removed so that it is rebuilt.
        let mut bytes = fs::read(&path).unwrap();
        bytes[8..12].copy_from_slice(&(PROXY_VERSION + 1).to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert_eq!(cache.lookup("aa"), None);
        assert!(!path.exists());
        // So is one that is cut short.
        let path = store(&cache, "bb", 3);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
        assert_eq!(cache.lookup("bb"), None);
    }

    #[test]
    fn a_hit_counts_as_a_use() {
        let dir = Dir::new("touch");
        let cache = ProxyCache::new(dir.0.join("cache"));
        let path = store(&cache, "aa", 2);
        age(&path, 0);
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(cache.lookup("aa").is_some());
        assert!(fs::metadata(&path).unwrap().modified().unwrap() > before);
    }

    #[test]
    fn pruning_removes_the_oldest_first_down_to_the_limit() {
        let dir = Dir::new("prune");
        let cache = ProxyCache::new(dir.0.join("cache"));
        let paths: Vec<PathBuf> = ["a1", "a2", "a3", "a4"]
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let p = store(&cache, key, 4);
                age(&p, i as u64 * 10);
                p
            })
            .collect();
        let each = fs::metadata(&paths[0]).unwrap().len();
        // Room for two: the two oldest go.
        let cache = cache.with_limit(each * 2 + 1);
        let done = cache.prune(None).unwrap();
        assert_eq!((done.proxies, done.partials), (2, 0));
        assert_eq!(done.bytes, each * 2);
        assert!(!paths[0].exists() && !paths[1].exists());
        assert!(paths[2].exists() && paths[3].exists());
        // Nothing to do when it fits.
        assert_eq!(cache.prune(None).unwrap(), Pruned::default());
    }

    #[test]
    fn the_proxy_just_built_stays_even_when_it_is_the_oldest_and_alone_over_the_limit() {
        let dir = Dir::new("keep");
        let cache = ProxyCache::new(dir.0.join("cache")).with_limit(1);
        let old = store(&cache, "old", 4);
        let fresh = store(&cache, "fresh", 4);
        age(&fresh, 0);
        age(&old, 100);
        let done = cache.prune(Some(&fresh)).unwrap();
        assert_eq!(done.proxies, 1);
        assert!(fresh.exists() && !old.exists());
    }

    #[test]
    fn pruning_removes_unfinished_files_and_leaves_other_files_alone() {
        let dir = Dir::new("part");
        let cache = ProxyCache::new(dir.0.join("cache"));
        let kept = store(&cache, "ok", 2);
        fs::write(cache.part_path("half"), b"unfinished").unwrap();
        fs::write(cache.dir().join("notes.txt"), b"mine").unwrap();
        let done = cache.prune(None).unwrap();
        assert_eq!((done.proxies, done.partials), (0, 1));
        assert!(kept.exists());
        assert!(cache.dir().join("notes.txt").exists());
        assert!(!cache.part_path("half").exists());
    }

    #[test]
    fn pruning_a_folder_that_does_not_exist_is_fine() {
        let dir = Dir::new("none");
        let cache = ProxyCache::new(dir.0.join("never-made"));
        assert_eq!(cache.prune(None).unwrap(), Pruned::default());
    }
}
