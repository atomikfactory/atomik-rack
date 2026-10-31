//! The mapping of the shared block. The parent creates the file and maps it; the child opens
//! the same file and maps it. Atomics are used in place in the mapping.

use crate::layout::{BLOCK_SIZE, Block, Header, MAGIC, PROTOCOL_VERSION, Request, Response};
use memmap2::{MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::ptr::addr_of_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// One mapping, shared (by `Arc`) between the block and the doorbell made for it so the memory
/// outlives both.
struct Mapping {
    // Unmapped by hand in `drop`, before the file is removed (Windows cannot delete a file that
    // is still mapped by this process).
    map: ManuallyDrop<MmapMut>,
    ptr: *mut Block,
    path: PathBuf,
    owner: bool,
    /// The creator already removed the path: do not remove it again (the name may have been
    /// reused).
    unlinked: AtomicBool,
    /// Kept open for the life of the mapping. On Windows the file was created delete-on-close,
    /// so the OS removes it when the last handle (this one, the child's) closes, even if the
    /// process dies.
    _file: File,
}

// SAFETY: the mapping is plain memory. Everything shared with the other process goes through
// atomics or through the request/response protocol of `layout`, which the two ports enforce
// with `&mut self` on every method that writes a non-atomic area.
unsafe impl Send for Mapping {}
// SAFETY: see above; `&self` methods only touch atomics (or read areas the protocol hands over).
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `map` is dropped exactly once, here, and `ptr` is not used afterwards.
        unsafe { ManuallyDrop::drop(&mut self.map) };
        if self.owner && !self.unlinked.load(Ordering::Acquire) {
            // Best effort: on Windows the delete fails while the child still maps the file,
            // and the temp folder is cleaned by the OS anyway.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// One mapped block. Created by the parent (a file in the temp folder or `/dev/shm`), opened by
/// the child. The creator removes the file when it drops the block.
pub struct SharedBlock {
    inner: Arc<Mapping>,
}

impl std::fmt::Debug for SharedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBlock")
            .field("path", &self.inner.path)
            .field("owner", &self.inner.owner)
            .finish()
    }
}

impl SharedBlock {
    /// Parent: creates the file (it must not exist: `O_EXCL`, mode `0600` on Unix), sizes it,
    /// maps it, and writes the magic and the version. The rest of the block is zero.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // GENERIC_READ | GENERIC_WRITE | DELETE (delete-on-close needs the DELETE right);
            // share read, write and delete so the child can open it; delete the file when the
            // last handle closes (FILE_FLAG_DELETE_ON_CLOSE), whatever happens to the process.
            options
                .access_mode(0x8000_0000 | 0x4000_0000 | 0x0001_0000)
                .share_mode(0x7)
                .custom_flags(0x0400_0000);
        }
        let file = options.open(path)?;
        file.set_len(BLOCK_SIZE as u64)?;
        let block = Self::map(file, path, true)?;
        let ptr = block.raw();
        // SAFETY: `ptr` points to a fresh, zeroed, writable mapping of `BLOCK_SIZE` bytes and
        // no other process has opened the file yet (the child starts after this returns).
        unsafe {
            addr_of_mut!((*ptr).header.magic).write(MAGIC);
            addr_of_mut!((*ptr).header.version).write(PROTOCOL_VERSION);
        }
        Ok(block)
    }

    /// Child: opens and maps the parent's file, checking the magic and the version.
    ///
    /// A wrong magic, a different version or a short file is `InvalidData`; the message names
    /// both versions.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Share read, write and delete: the creator's handle is delete-on-close, and
            // Windows refuses a second open that does not share delete.
            options.share_mode(0x7);
        }
        let mut file = options.open(path)?;
        let mut head = [0u8; 8];
        file.read_exact(&mut head).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                invalid("the plugin block file is shorter than its header".to_string())
            } else {
                e
            }
        })?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let version = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        if magic != MAGIC {
            return Err(invalid(format!(
                "not a plugin block: magic {magic:#010x}, expected {MAGIC:#010x}"
            )));
        }
        if version != PROTOCOL_VERSION {
            return Err(invalid(format!(
                "plugin block protocol mismatch: the parent wrote version {version}, \
                 this process speaks version {PROTOCOL_VERSION}"
            )));
        }
        let len = file.metadata()?.len();
        if len < BLOCK_SIZE as u64 {
            return Err(invalid(format!(
                "the plugin block file is {len} bytes, expected {BLOCK_SIZE}"
            )));
        }
        Self::map(file, path, false)
    }

    fn map(file: File, path: &Path, owner: bool) -> io::Result<Self> {
        // SAFETY: the file is the block file, sized to `BLOCK_SIZE`. The other process mutates
        // it only through atomics and the request/response protocol; nobody truncates it while
        // it is mapped (the creator owns it and only deletes it after unmapping).
        let mut map = unsafe { MmapOptions::new().len(BLOCK_SIZE).map_mut(&file)? };
        let ptr = map.as_mut_ptr().cast::<Block>();
        // A mapping is page aligned, which covers the 64 bytes the block needs.
        assert!(
            (ptr as usize).is_multiple_of(std::mem::align_of::<Block>()),
            "the block mapping is not aligned"
        );
        Ok(Self {
            inner: Arc::new(Mapping {
                map: ManuallyDrop::new(map),
                ptr,
                path: path.to_path_buf(),
                owner,
                unlinked: AtomicBool::new(false),
                _file: file,
            }),
        })
    }

    /// Creator only: removes the block file now while the mapping stays valid, so a crash
    /// afterwards leaves nothing behind. Call it once the child has opened and mapped the block
    /// (after its `Hello`): `open` after `unlink` fails with `NotFound`. Idempotent. A block
    /// made by `open` refuses with `PermissionDenied`.
    ///
    /// On Windows this does nothing: the file is delete-on-close and the OS removes it when
    /// the last handle closes, and Windows cannot delete a mapped file by name anyway.
    pub fn unlink(&self) -> io::Result<()> {
        if !self.inner.owner {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "only the creator of a block unlinks it",
            ));
        }
        if cfg!(windows) {
            return Ok(());
        }
        match std::fs::remove_file(&self.inner.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.inner.unlinked.store(true, Ordering::Release);
        Ok(())
    }

    /// The file this block is mapped from.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The header (the atomics live here).
    pub(crate) fn header(&self) -> &Header {
        // SAFETY: the mapping is valid for the life of `self`; the non-atomic header fields are
        // written once by `create` before the file is shared, and the rest are atomics.
        unsafe { &(*self.inner.ptr).header }
    }

    /// The base pointer of the block.
    pub(crate) fn raw(&self) -> *mut Block {
        self.inner.ptr
    }

    /// The request area (the parent writes it, the child reads it).
    pub(crate) fn request(&self) -> *mut Request {
        // SAFETY: only computes the address of a field inside the mapping.
        unsafe { addr_of_mut!((*self.inner.ptr).request) }
    }

    /// The response area (the child writes it, the parent reads it).
    pub(crate) fn response(&self) -> *mut Response {
        // SAFETY: only computes the address of a field inside the mapping.
        unsafe { addr_of_mut!((*self.inner.ptr).response) }
    }

    /// Another handle on the same mapping (doorbell, death handle). The file is removed when the
    /// last handle goes.
    pub(crate) fn share(&self) -> SharedBlock {
        SharedBlock {
            inner: Arc::clone(&self.inner),
        }
    }
}

fn invalid(text: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

/// The folder block files go in: `/dev/shm` on Linux when it is a writable folder, else the
/// temp folder.
pub fn default_block_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let shm = Path::new("/dev/shm");
        if shm.is_dir() && writable(shm) {
            return shm.to_path_buf();
        }
    }
    std::env::temp_dir()
}

/// True when a file can be created in `dir`.
#[cfg(target_os = "linux")]
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".atomik-probe-{}", std::process::id()));
    let ok = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The file name of the block of one instance: the parent's pid keeps two apps apart, the
/// instance number two plugins of one app, and a random suffix makes the name impossible to
/// guess (the file is also created exclusively, so a planted file or link is refused).
pub fn block_file_name(parent_pid: u32, instance: u64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(parent_pid);
    hasher.write_u64(instance);
    format!(
        "atomik-sandbox-{parent_pid}-{instance}-{:016x}.blk",
        hasher.finish()
    )
}

/// The pid in the name of a block file made by [`block_file_name`].
#[cfg_attr(not(unix), allow(dead_code))]
fn block_file_owner(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("atomik-sandbox-")?;
    if !name.ends_with(".blk") {
        return None;
    }
    rest.split('-').next()?.parse().ok()
}

/// Removes the block files of apps that are no longer running (a crash leaves its files in
/// `dir` on Unix; Windows removes them by itself). Returns how many were removed. Files of a
/// live process, and files that are not block files, are left alone.
pub fn sweep_stale_blocks(dir: &Path) -> usize {
    #[cfg(unix)]
    {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(block_file_owner) else {
                continue;
            };
            if !process_alive(pid) && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        0
    }
}

/// True when the process `pid` exists.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::sync::atomic::Ordering;

    fn temp_path(tag: &str) -> PathBuf {
        default_block_dir().join(block_file_name(std::process::id(), unique(tag)))
    }

    fn unique(tag: &str) -> u64 {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        n * 1000 + tag.len() as u64
    }

    #[test]
    fn create_writes_the_magic_and_the_version_and_zeroes_the_rest() {
        let path = temp_path("create");
        let block = SharedBlock::create(&path).unwrap();
        let header = block.header();
        assert_eq!(header.magic, MAGIC);
        assert_eq!(header.version, PROTOCOL_VERSION);
        assert_eq!(header.req_seq.load(Ordering::Relaxed), 0);
        assert_eq!(header.done_seq.load(Ordering::Relaxed), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), BLOCK_SIZE as u64);
        assert_eq!(block.path(), path.as_path());
    }

    #[test]
    fn the_base_of_the_mapping_is_aligned_for_the_block() {
        let path = temp_path("align");
        let block = SharedBlock::create(&path).unwrap();
        assert_eq!(block.raw() as usize % std::mem::align_of::<Block>(), 0);
    }

    #[test]
    fn open_maps_the_same_memory_as_create() {
        let path = temp_path("open");
        let parent = SharedBlock::create(&path).unwrap();
        let child = SharedBlock::open(&path).unwrap();
        parent.header().req_seq.store(41, Ordering::Release);
        assert_eq!(child.header().req_seq.load(Ordering::Acquire), 41);
        child.header().done_seq.store(7, Ordering::Release);
        assert_eq!(parent.header().done_seq.load(Ordering::Acquire), 7);
    }

    fn write_header(path: &Path, magic: u32, version: u32) {
        let mut bytes = vec![0u8; BLOCK_SIZE];
        bytes[0..4].copy_from_slice(&magic.to_le_bytes());
        bytes[4..8].copy_from_slice(&version.to_le_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn open_refuses_a_block_of_another_version_and_names_both_versions() {
        let path = temp_path("version");
        write_header(&path, MAGIC, PROTOCOL_VERSION + 1);
        let err = SharedBlock::open(&path).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        let text = err.to_string();
        assert!(text.contains(&(PROTOCOL_VERSION + 1).to_string()), "{text}");
        assert!(
            text.contains(&format!("version {PROTOCOL_VERSION}")),
            "{text}"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn open_refuses_a_file_that_is_not_a_block() {
        let path = temp_path("magic");
        write_header(&path, 0x1234_5678, PROTOCOL_VERSION);
        let err = SharedBlock::open(&path).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn open_refuses_a_file_that_is_too_short() {
        let path = temp_path("short");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let err = SharedBlock::open(&path).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn open_of_a_missing_file_fails_with_not_found() {
        let path = temp_path("missing");
        let err = SharedBlock::open(&path).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn the_creator_removes_the_file_when_it_is_dropped() {
        let path = temp_path("drop");
        let block = SharedBlock::create(&path).unwrap();
        assert!(path.exists());
        drop(block);
        assert!(!path.exists());
    }

    #[test]
    fn the_opener_leaves_the_file_alone() {
        let path = temp_path("opener");
        let parent = SharedBlock::create(&path).unwrap();
        drop(SharedBlock::open(&path).unwrap());
        assert!(path.exists());
        drop(parent);
    }

    #[test]
    fn the_file_name_carries_the_pid_and_the_instance() {
        let name = block_file_name(1234, 56);
        assert!(name.contains("1234") && name.contains("56"), "{name}");
        assert_ne!(block_file_name(1234, 56), block_file_name(1234, 57));
        assert_ne!(block_file_name(1234, 56), block_file_name(1235, 56));
    }

    #[test]
    fn the_default_block_dir_exists() {
        assert!(default_block_dir().is_dir());
    }

    #[test]
    fn the_file_name_cannot_be_guessed_from_the_pid_and_the_instance() {
        let a = block_file_name(1234, 56);
        let b = block_file_name(1234, 56);
        assert_ne!(a, b, "two names for the same instance differ");
        assert_eq!(block_file_owner(&a), Some(1234));
        assert_eq!(block_file_owner("atomik-sandbox-x-1.blk"), None);
        assert_eq!(block_file_owner("atomik-sandbox-12-1.txt"), None);
        assert_eq!(block_file_owner("other-12-1.blk"), None);
    }

    #[test]
    fn a_path_that_exists_is_refused_and_left_untouched() {
        let path = temp_path("exclusive");
        std::fs::write(&path, b"planted").unwrap();
        let err = SharedBlock::create(&path).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"planted");
        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_block_file_is_private_to_its_user() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("mode");
        let block = SharedBlock::create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        drop(block);
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_removes_the_files_of_dead_processes_and_only_those() {
        let dir = std::env::temp_dir().join(format!("atomik-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A pid that cannot be alive: above the kernel's limit.
        let dead = dir.join("atomik-sandbox-2147483646-1-00ff.blk");
        let live = dir.join(block_file_name(std::process::id(), 1));
        let other = dir.join("notes.txt");
        for p in [&dead, &live, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        assert_eq!(sweep_stale_blocks(&dir), 1);
        assert!(!dead.exists() && live.exists() && other.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
