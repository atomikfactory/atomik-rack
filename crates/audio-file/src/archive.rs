//! The project archive: a project and its audio as one `.atomikz` file.
//!
//! The file is a standard ZIP archive whose entries are stored, not compressed (the audio is
//! 32-bit float or PCM and hardly compresses; stored entries stream at disk speed and the
//! reader never expands anything). Its layout is the layout of a project bundle, so unpacking it
//! with any ZIP tool gives a project that opens:
//!
//! ```text
//! <name>/<name>.atomik          the project text, byte for byte
//! <name>/Audio/<hash>.wav       one entry per audio asset the project uses
//! ```
//!
//! Entries over 4 GiB, more than 65 534 entries and archives over 4 GiB use the ZIP64 fields, so
//! a multi-gigabyte project archives like a small one: both directions stream through a fixed
//! buffer and never hold a file in memory.
//!
//! # Writing
//!
//! [`write_archive`] writes to `<archive>.part` next to the target and renames it when it is
//! complete, so the target is never half written. A cancelled or failed write removes the part
//! file. Each file is read twice (a pass for its CRC, then the copy), and a file that changed in
//! between is reported instead of archived.
//!
//! # Reading
//!
//! [`extract_archive`] never trusts the archive. It reads the central directory, and accepts
//! only the two entry shapes above (an entry with any other name, a path component that is `..`,
//! empty, absolute, a drive, or holds a backslash, a colon or a control character, a duplicate, a
//! second project, a compressed or encrypted entry) and refuses the archive before it writes a
//! byte. [`ArchiveLimits`] bound the entry count, each entry, the total and the project text; an
//! entry whose data is not exactly its declared size, or whose CRC differs, fails the extraction.
//! The files go to a fresh folder `<destination>.part`, which becomes `<destination>` when
//! everything is in place and is removed on any failure or cancel; an existing destination is
//! never touched.

use crate::is_valid_hash;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The file extension of a project archive.
pub const ARCHIVE_EXTENSION: &str = "atomikz";

/// The extension of the project file inside an archive.
const PROJECT_EXTENSION: &str = "atomik";
const AUDIO_DIR: &str = "Audio";
const PART_SUFFIX: &str = ".part";
const BUFFER: usize = 1 << 20;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_END: u32 = 0x0605_4b50;
const SIG_END64: u32 = 0x0606_4b50;
const SIG_LOCATOR64: u32 = 0x0706_4b50;
const ZIP64_EXTRA: u16 = 0x0001;
const VERSION_ZIP64: u16 = 45;
/// General purpose flags: bit 11, the names are UTF-8.
const FLAG_UTF8: u16 = 0x0800;
const FLAG_ENCRYPTED: u16 = 0x0001;
/// A fixed time stamp (1980-01-01 00:00) so an archive of the same content is the same bytes.
const DOS_DATE: u16 = 0x0021;
const MAX_NAME: usize = 100;

/// Bounds on what [`extract_archive`] accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Most entries (the project, its audio files and the folder entries).
    pub max_entries: usize,
    /// Most bytes of one audio file.
    pub max_entry_bytes: u64,
    /// Most bytes of all entries together.
    pub max_total_bytes: u64,
    /// Most bytes of the project text.
    pub max_project_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_entry_bytes: 16 << 30,
            max_total_bytes: 1 << 40,
            max_project_bytes: 256 << 20,
        }
    }
}

/// Why an archive could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: io::Error,
    },
    /// The work was cancelled; nothing is left behind.
    #[error("cancelled")]
    Cancelled,
    /// An entry that must not be extracted: a path that leaves the folder, an unknown kind of
    /// file, a duplicate.
    #[error("the archive is refused: {0}")]
    Unsafe(String),
    /// An entry or the whole archive is over a limit.
    #[error("the archive is too large: {0}")]
    TooLarge(String),
    /// The file is not an intact archive of this program.
    #[error("not a project archive: {0}")]
    Corrupt(String),
    /// The destination exists already.
    #[error("{} exists already", .0.display())]
    Exists(PathBuf),
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> ArchiveError + '_ {
    move |source| ArchiveError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// What a finished archive holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveReport {
    /// The project file's number of bytes.
    pub project_bytes: u64,
    /// Audio files.
    pub audio_files: usize,
    /// Bytes of all files.
    pub bytes: u64,
}

/// What an extraction made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The new folder (the project's bundle folder).
    pub root: PathBuf,
    /// The project file inside it.
    pub project: PathBuf,
    /// What it holds.
    pub report: ArchiveReport,
}

// --------------------------------------------------------------------------------- crc and bytes

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// The running CRC-32 of the ZIP format.
#[derive(Clone, Copy)]
struct Crc(u32);

impl Crc {
    fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = CRC_TABLE[((self.0 ^ u32::from(b)) & 0xFF) as usize] ^ (self.0 >> 8);
        }
    }

    fn value(self) -> u32 {
        !self.0
    }
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A little-endian cursor over a byte slice; a read past the end is `None`.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (head, tail) = self.0.split_at_checked(n)?;
        self.0 = tail;
        Some(head)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

// --------------------------------------------------------------------------------------- names

/// True for a name that is safe as one path component of the archive root: 1 to 100 characters
/// without a separator, a colon, a control character or a trailing dot or space, and not `.` or
/// `..`.
pub fn is_safe_component(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

/// What an entry of an archive is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// A folder entry (`root/` or `root/Audio/`): nothing to extract.
    Folder,
    Project,
    Audio(String),
}

/// Classifies the entry `name` of an archive whose root is `root` (`None` while the first
/// entry decides it); the refusal says why.
fn classify(name: &str, root: &mut Option<String>) -> Result<Kind, String> {
    if name.contains('\\')
        || name.contains("//")
        || name.starts_with('/')
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "the entry name {name:?} is not a plain relative path"
        ));
    }
    let parts: Vec<&str> = name.trim_end_matches('/').split('/').collect();
    let folder = name.ends_with('/');
    let first = parts[0];
    if !is_safe_component(first) {
        return Err(format!("the entry name {name:?} has an unsafe first part"));
    }
    match root {
        Some(r) if r != first => {
            return Err(format!("the entry {name:?} is outside the folder {r:?}"));
        }
        Some(_) => {}
        None => *root = Some(first.to_owned()),
    }
    match (parts.as_slice(), folder) {
        ([_], true) => Ok(Kind::Folder),
        ([_, AUDIO_DIR], true) => Ok(Kind::Folder),
        ([r, file], false) if *file == format!("{r}.{PROJECT_EXTENSION}") => Ok(Kind::Project),
        ([_, AUDIO_DIR, file], false) => match file.strip_suffix(".wav") {
            Some(hash) if is_valid_hash(hash) => Ok(Kind::Audio(hash.to_owned())),
            _ => Err(format!(
                "the entry {name:?} is not an audio file of a project"
            )),
        },
        _ => Err(format!("the entry {name:?} is not part of a project")),
    }
}

// ------------------------------------------------------------------------------------- writing

/// What the writer remembers of an entry for the central directory.
struct Written {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
}

/// A stored-entries ZIP writer.
struct ZipOut<W: Write> {
    out: W,
    at: u64,
    entries: Vec<Written>,
}

impl<W: Write> ZipOut<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            at: 0,
            entries: Vec::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }

    /// Writes the local header of an entry of `size` bytes with the CRC `crc`.
    fn begin(&mut self, name: &str, crc: u32, size: u64) -> io::Result<()> {
        let big = size >= u64::from(u32::MAX);
        let mut h = Vec::with_capacity(30 + name.len() + 20);
        put32(&mut h, SIG_LOCAL);
        put16(&mut h, if big { VERSION_ZIP64 } else { 20 });
        put16(&mut h, FLAG_UTF8);
        put16(&mut h, 0); // stored
        put16(&mut h, 0); // time
        put16(&mut h, DOS_DATE);
        put32(&mut h, crc);
        let small = if big { u32::MAX } else { size as u32 };
        put32(&mut h, small);
        put32(&mut h, small);
        put16(&mut h, name.len() as u16);
        put16(&mut h, if big { 20 } else { 0 });
        h.extend_from_slice(name.as_bytes());
        if big {
            put16(&mut h, ZIP64_EXTRA);
            put16(&mut h, 16);
            put64(&mut h, size);
            put64(&mut h, size);
        }
        let offset = self.at;
        self.write(&h)?;
        self.entries.push(Written {
            name: name.to_owned(),
            crc,
            size,
            offset,
        });
        Ok(())
    }

    /// Writes the central directory and the end records, and returns the output.
    fn finish(mut self) -> io::Result<W> {
        let start = self.at;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let (big_size, big_offset) = (
                e.size >= u64::from(u32::MAX),
                e.offset >= u64::from(u32::MAX),
            );
            let mut extra = Vec::new();
            if big_size {
                put64(&mut extra, e.size);
                put64(&mut extra, e.size);
            }
            if big_offset {
                put64(&mut extra, e.offset);
            }
            let mut c = Vec::with_capacity(46 + e.name.len() + extra.len() + 4);
            put32(&mut c, SIG_CENTRAL);
            put16(&mut c, VERSION_ZIP64);
            put16(
                &mut c,
                if big_size || big_offset {
                    VERSION_ZIP64
                } else {
                    20
                },
            );
            put16(&mut c, FLAG_UTF8);
            put16(&mut c, 0);
            put16(&mut c, 0);
            put16(&mut c, DOS_DATE);
            put32(&mut c, e.crc);
            let small = if big_size { u32::MAX } else { e.size as u32 };
            put32(&mut c, small);
            put32(&mut c, small);
            put16(&mut c, e.name.len() as u16);
            put16(
                &mut c,
                if extra.is_empty() {
                    0
                } else {
                    extra.len() as u16 + 4
                },
            );
            put16(&mut c, 0); // comment
            put16(&mut c, 0); // disk
            put16(&mut c, 0); // internal attributes
            put32(&mut c, 0); // external attributes
            put32(
                &mut c,
                if big_offset {
                    u32::MAX
                } else {
                    e.offset as u32
                },
            );
            c.extend_from_slice(e.name.as_bytes());
            if !extra.is_empty() {
                put16(&mut c, ZIP64_EXTRA);
                put16(&mut c, extra.len() as u16);
                c.extend_from_slice(&extra);
            }
            self.write(&c)?;
        }
        let size = self.at - start;
        let count = entries.len() as u64;
        let needs64 =
            count >= 0xFFFF || start >= u64::from(u32::MAX) || size >= u64::from(u32::MAX);
        let mut end = Vec::new();
        if needs64 {
            let record_at = self.at;
            put32(&mut end, SIG_END64);
            put64(&mut end, 44);
            put16(&mut end, VERSION_ZIP64);
            put16(&mut end, VERSION_ZIP64);
            put32(&mut end, 0);
            put32(&mut end, 0);
            put64(&mut end, count);
            put64(&mut end, count);
            put64(&mut end, size);
            put64(&mut end, start);
            put32(&mut end, SIG_LOCATOR64);
            put32(&mut end, 0);
            put64(&mut end, record_at);
            put32(&mut end, 1);
        }
        put32(&mut end, SIG_END);
        put16(&mut end, 0);
        put16(&mut end, 0);
        let small_count = if needs64 { 0xFFFF } else { count as u16 };
        put16(&mut end, small_count);
        put16(&mut end, small_count);
        put32(&mut end, if needs64 { u32::MAX } else { size as u32 });
        put32(&mut end, if needs64 { u32::MAX } else { start as u32 });
        put16(&mut end, 0);
        self.write(&end)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ArchiveError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ArchiveError::Cancelled)
    } else {
        Ok(())
    }
}

/// The CRC-32 and the length of `path`, read through a fixed buffer.
fn crc_of(
    path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<(u32, u64), ArchiveError> {
    let mut file = File::open(path).map_err(io_at(path))?;
    let mut buf = vec![0u8; BUFFER];
    let mut crc = Crc::new();
    let mut size = 0u64;
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buf).map_err(io_at(path))?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        size += n as u64;
        progress.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok((crc.value(), size))
}

/// The total of [`write_archive`]'s progress counter for these files: every byte is counted
/// twice (the CRC pass and the copy).
pub fn archive_work(project_bytes: u64, audio: &[PathBuf]) -> u64 {
    let audio: u64 = audio
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    2 * (project_bytes + audio)
}

/// Writes the archive `dest` of the project `name` (its text `project`, which is stored byte for
/// byte) with the audio files `audio`, each `(asset hash, path of its WAV file)`. See the module
/// docs. `progress` counts bytes up to [`archive_work`]; `cancel` stops the work.
///
/// # Errors
/// [`ArchiveError::Cancelled`] (nothing is left behind), [`ArchiveError::Io`] for an unreadable
/// file or an unwritable destination, [`ArchiveError::Unsafe`] for a name or an asset id that is
/// not valid, [`ArchiveError::Corrupt`] for a file that changed while it was archived.
pub fn write_archive(
    dest: &Path,
    name: &str,
    project: &[u8],
    audio: &[(String, PathBuf)],
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<ArchiveReport, ArchiveError> {
    if !is_safe_component(name) {
        return Err(ArchiveError::Unsafe(format!(
            "{name:?} is not a usable project name"
        )));
    }
    if let Some((hash, _)) = audio.iter().find(|(h, _)| !is_valid_hash(h)) {
        return Err(ArchiveError::Unsafe(format!("{hash:?} is not an asset id")));
    }
    let part = part_path(dest);
    let result = write_part(&part, name, project, audio, cancel, progress);
    match result {
        Ok(report) => {
            if let Err(e) = fs::rename(&part, dest) {
                let _ = fs::remove_file(&part);
                return Err(io_at(dest)(e));
            }
            Ok(report)
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

fn write_part(
    part: &Path,
    name: &str,
    project: &[u8],
    audio: &[(String, PathBuf)],
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<ArchiveReport, ArchiveError> {
    let file = File::create(part).map_err(io_at(part))?;
    let mut zip = ZipOut::new(BufWriter::with_capacity(BUFFER, file));
    let mut report = ArchiveReport {
        project_bytes: project.len() as u64,
        audio_files: 0,
        bytes: project.len() as u64,
    };
    let mut crc = Crc::new();
    crc.update(project);
    zip.begin(
        &format!("{name}/{name}.{PROJECT_EXTENSION}"),
        crc.value(),
        project.len() as u64,
    )
    .and_then(|()| zip.write(project))
    .map_err(io_at(part))?;
    progress.fetch_add(2 * project.len() as u64, Ordering::Relaxed);
    let mut buf = vec![0u8; BUFFER];
    for (hash, path) in audio {
        check_cancel(cancel)?;
        let (crc, size) = crc_of(path, cancel, progress)?;
        zip.begin(&format!("{name}/{AUDIO_DIR}/{hash}.wav"), crc, size)
            .map_err(io_at(part))?;
        let mut source = File::open(path).map_err(io_at(path))?;
        let mut copied = 0u64;
        let mut check = Crc::new();
        loop {
            check_cancel(cancel)?;
            let n = source.read(&mut buf).map_err(io_at(path))?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            check.update(&buf[..n]);
            zip.write(&buf[..n]).map_err(io_at(part))?;
            progress.fetch_add(n as u64, Ordering::Relaxed);
        }
        if copied != size || check.value() != crc {
            return Err(ArchiveError::Corrupt(format!(
                "{} changed while it was archived",
                path.display()
            )));
        }
        report.audio_files += 1;
        report.bytes += size;
    }
    let writer = zip.finish().map_err(io_at(part))?;
    let file = writer
        .into_inner()
        .map_err(|e| io_at(part)(e.into_error()))?;
    file.sync_all().map_err(io_at(part))?;
    Ok(report)
}

// ------------------------------------------------------------------------------------- reading

/// One entry of the central directory.
#[derive(Debug)]
struct Entry {
    name: String,
    kind: Kind,
    crc: u32,
    size: u64,
    offset: u64,
}

fn corrupt(why: impl Into<String>) -> ArchiveError {
    ArchiveError::Corrupt(why.into())
}

/// The end-of-central-directory fields: entry count, directory size and offset.
fn find_directory<R: Read + Seek>(
    file: &mut R,
    path: &Path,
) -> Result<(u64, u64, u64), ArchiveError> {
    let len = file.seek(SeekFrom::End(0)).map_err(io_at(path))?;
    let tail_len = len.min(22 + 0xFFFF);
    file.seek(SeekFrom::Start(len - tail_len))
        .map_err(io_at(path))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail).map_err(io_at(path))?;
    let at = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == SIG_END.to_le_bytes())
        .ok_or_else(|| corrupt("there is no end of archive record"))?;
    let mut c = Cursor(&tail[at + 4..]);
    let bad = || corrupt("the end of archive record is cut short");
    let (_disk, _start, _here) = (
        c.u16().ok_or_else(bad)?,
        c.u16().ok_or_else(bad)?,
        c.u16().ok_or_else(bad)?,
    );
    let count = u64::from(c.u16().ok_or_else(bad)?);
    let size = u64::from(c.u32().ok_or_else(bad)?);
    let offset = u64::from(c.u32().ok_or_else(bad)?);
    if count != 0xFFFF && size != u64::from(u32::MAX) && offset != u64::from(u32::MAX) {
        return Ok((count, size, offset));
    }
    // ZIP64: the locator sits right before the end record.
    let end_at = len - tail_len + at as u64;
    let locator_at = end_at
        .checked_sub(20)
        .ok_or_else(|| corrupt("the ZIP64 locator is missing"))?;
    file.seek(SeekFrom::Start(locator_at))
        .map_err(io_at(path))?;
    let mut locator = [0u8; 20];
    file.read_exact(&mut locator).map_err(io_at(path))?;
    let mut c = Cursor(&locator);
    if c.u32() != Some(SIG_LOCATOR64) {
        return Err(corrupt("the ZIP64 locator is missing"));
    }
    let (_disk, record_at) = (c.u32(), c.u64().ok_or_else(bad)?);
    file.seek(SeekFrom::Start(record_at)).map_err(io_at(path))?;
    let mut record = [0u8; 56];
    file.read_exact(&mut record).map_err(io_at(path))?;
    let mut c = Cursor(&record);
    if c.u32() != Some(SIG_END64) {
        return Err(corrupt("the ZIP64 end record is missing"));
    }
    let _ = (c.u64(), c.u16(), c.u16(), c.u32(), c.u32());
    let count = c.u64().ok_or_else(bad)?;
    let _same = c.u64();
    let size = c.u64().ok_or_else(bad)?;
    let offset = c.u64().ok_or_else(bad)?;
    Ok((count, size, offset))
}

/// Reads and checks the central directory of `file`: every entry is classified and the limits
/// are applied before anything is extracted.
fn read_directory<R: Read + Seek>(
    file: &mut R,
    path: &Path,
    limits: &ArchiveLimits,
) -> Result<Vec<Entry>, ArchiveError> {
    let (count, size, offset) = find_directory(file, path)?;
    if count > limits.max_entries as u64 {
        return Err(ArchiveError::TooLarge(format!(
            "{count} entries, at most {} are allowed",
            limits.max_entries
        )));
    }
    // A directory entry is 46 bytes at least plus a name, and one name is short.
    if size > count.saturating_mul(46 + MAX_NAME as u64 * 3 + 64) + 1024 {
        return Err(corrupt(
            "the central directory is larger than its entries can be",
        ));
    }
    file.seek(SeekFrom::Start(offset)).map_err(io_at(path))?;
    let mut directory = vec![0u8; size as usize];
    file.read_exact(&mut directory)
        .map_err(|_| corrupt("the central directory is cut short"))?;
    let mut c = Cursor(&directory);
    let mut root = None;
    let mut entries = Vec::new();
    let mut projects = 0;
    let mut total = 0u64;
    for _ in 0..count {
        let short = || corrupt("a directory entry is cut short");
        if c.u32() != Some(SIG_CENTRAL) {
            return Err(corrupt("a directory entry has no signature"));
        }
        let _made = c.u16().ok_or_else(short)?;
        let _needed = c.u16().ok_or_else(short)?;
        let flags = c.u16().ok_or_else(short)?;
        let method = c.u16().ok_or_else(short)?;
        let _time = (c.u16().ok_or_else(short)?, c.u16().ok_or_else(short)?);
        let crc = c.u32().ok_or_else(short)?;
        let mut compressed = u64::from(c.u32().ok_or_else(short)?);
        let mut size = u64::from(c.u32().ok_or_else(short)?);
        let name_len = usize::from(c.u16().ok_or_else(short)?);
        let extra_len = usize::from(c.u16().ok_or_else(short)?);
        let comment_len = usize::from(c.u16().ok_or_else(short)?);
        let _ = (c.u16(), c.u16(), c.u32());
        let mut offset = u64::from(c.u32().ok_or_else(short)?);
        let name = std::str::from_utf8(c.take(name_len).ok_or_else(short)?)
            .map_err(|_| ArchiveError::Unsafe("an entry name is not UTF-8".into()))?
            .to_owned();
        let extra = c.take(extra_len).ok_or_else(short)?.to_vec();
        c.take(comment_len).ok_or_else(short)?;
        if flags & FLAG_ENCRYPTED != 0 {
            return Err(ArchiveError::Unsafe(format!("{name:?} is encrypted")));
        }
        if method != 0 {
            return Err(ArchiveError::Unsafe(format!(
                "{name:?} is compressed; project archives hold stored entries"
            )));
        }
        // The ZIP64 extra holds, in order, the fields that were 0xFFFFFFFF.
        let (wants_size, wants_offset) =
            (size == u64::from(u32::MAX), offset == u64::from(u32::MAX));
        if wants_size || wants_offset || compressed == u64::from(u32::MAX) {
            let mut e = Cursor(&extra);
            let mut found = false;
            while let (Some(id), Some(len)) = (e.u16(), e.u16()) {
                let Some(body) = e.take(usize::from(len)) else {
                    break;
                };
                if id == ZIP64_EXTRA {
                    let mut b = Cursor(body);
                    if wants_size {
                        size = b.u64().ok_or_else(short)?;
                    }
                    if compressed == u64::from(u32::MAX) {
                        compressed = b.u64().ok_or_else(short)?;
                    }
                    if wants_offset {
                        offset = b.u64().ok_or_else(short)?;
                    }
                    found = true;
                }
            }
            if !found {
                return Err(corrupt(format!("{name:?} lacks its ZIP64 sizes")));
            }
        }
        if compressed != size {
            return Err(corrupt(format!("{name:?} is not stored as it is")));
        }
        let kind = classify(&name, &mut root).map_err(ArchiveError::Unsafe)?;
        if entries.iter().any(|e: &Entry| e.name == name) {
            return Err(ArchiveError::Unsafe(format!("{name:?} is there twice")));
        }
        match &kind {
            Kind::Folder => {
                if size != 0 {
                    return Err(corrupt(format!("the folder {name:?} has data")));
                }
            }
            Kind::Project => {
                projects += 1;
                if size > limits.max_project_bytes {
                    return Err(ArchiveError::TooLarge(format!(
                        "the project text is {size} bytes, at most {} are allowed",
                        limits.max_project_bytes
                    )));
                }
            }
            Kind::Audio(_) => {
                if size > limits.max_entry_bytes {
                    return Err(ArchiveError::TooLarge(format!(
                        "{name:?} is {size} bytes, at most {} are allowed",
                        limits.max_entry_bytes
                    )));
                }
            }
        }
        total = total.saturating_add(size);
        if total > limits.max_total_bytes {
            return Err(ArchiveError::TooLarge(format!(
                "the files add up to more than {} bytes",
                limits.max_total_bytes
            )));
        }
        entries.push(Entry {
            name,
            kind,
            crc,
            size,
            offset,
        });
    }
    if projects != 1 {
        return Err(ArchiveError::Unsafe(format!(
            "an archive holds one project, this one has {projects}"
        )));
    }
    Ok(entries)
}

/// Where the data of `entry` starts, after checking its local header against the directory.
fn data_start<R: Read + Seek>(
    file: &mut R,
    path: &Path,
    entry: &Entry,
) -> Result<u64, ArchiveError> {
    file.seek(SeekFrom::Start(entry.offset))
        .map_err(io_at(path))?;
    let mut header = [0u8; 30];
    file.read_exact(&mut header)
        .map_err(|_| corrupt(format!("{:?} is cut short", entry.name)))?;
    let mut c = Cursor(&header);
    if c.u32() != Some(SIG_LOCAL) {
        return Err(corrupt(format!("{:?} has no local header", entry.name)));
    }
    let _ = (
        c.u16(),
        c.u16(),
        c.u16(),
        c.u16(),
        c.u16(),
        c.u32(),
        c.u32(),
        c.u32(),
    );
    let name_len = u64::from(c.u16().unwrap_or(0));
    let extra_len = u64::from(c.u16().unwrap_or(0));
    let mut name = vec![0u8; name_len as usize];
    file.read_exact(&mut name)
        .map_err(|_| corrupt(format!("{:?} is cut short", entry.name)))?;
    if name != entry.name.as_bytes() {
        return Err(corrupt(format!(
            "the local header of {:?} names another file",
            entry.name
        )));
    }
    Ok(entry.offset + 30 + name_len + extra_len)
}

/// Reads the project text of the archive `path` (nothing is written anywhere).
///
/// # Errors
/// As [`extract_archive`].
pub fn read_archive_project(path: &Path, limits: &ArchiveLimits) -> Result<Vec<u8>, ArchiveError> {
    let mut file = BufReader::new(File::open(path).map_err(io_at(path))?);
    let entries = read_directory(&mut file, path, limits)?;
    let entry = entries
        .iter()
        .find(|e| e.kind == Kind::Project)
        .ok_or_else(|| corrupt("there is no project"))?;
    let start = data_start(&mut file, path, entry)?;
    file.seek(SeekFrom::Start(start)).map_err(io_at(path))?;
    let mut text = vec![0u8; entry.size as usize];
    file.read_exact(&mut text)
        .map_err(|_| corrupt("the project text is cut short"))?;
    let mut crc = Crc::new();
    crc.update(&text);
    if crc.value() != entry.crc {
        return Err(corrupt("the project text is damaged"));
    }
    Ok(text)
}

/// Extracts the archive `path` into the new folder `dest` (see the module docs for what is
/// accepted and how). `progress` counts the bytes written up to the archive's total size;
/// `cancel` stops the work and removes what was written.
///
/// # Errors
/// [`ArchiveError::Exists`] when `dest` exists, [`ArchiveError::Unsafe`] and
/// [`ArchiveError::TooLarge`] for an archive that is refused, [`ArchiveError::Corrupt`] for one
/// that is damaged, [`ArchiveError::Cancelled`], and [`ArchiveError::Io`].
pub fn extract_archive(
    path: &Path,
    dest: &Path,
    limits: &ArchiveLimits,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<Extracted, ArchiveError> {
    if dest.exists() {
        return Err(ArchiveError::Exists(dest.to_path_buf()));
    }
    let mut file = BufReader::new(File::open(path).map_err(io_at(path))?);
    let entries = read_directory(&mut file, path, limits)?;
    let part = part_path(dest);
    if part.exists() {
        return Err(ArchiveError::Exists(part));
    }
    // The project file is named after its folder, whatever the archive called it.
    let project_name = format!(
        "{}.{PROJECT_EXTENSION}",
        dest.file_name().map_or_else(
            || "project".to_owned(),
            |n| n.to_string_lossy().into_owned()
        )
    );
    fs::create_dir_all(part.join(AUDIO_DIR)).map_err(io_at(&part))?;
    match extract_entries(
        &mut file,
        path,
        &part,
        &project_name,
        &entries,
        cancel,
        progress,
    ) {
        Ok(report) => {
            if let Err(e) = fs::rename(&part, dest) {
                let _ = fs::remove_dir_all(&part);
                return Err(io_at(dest)(e));
            }
            let project = dest.join(&project_name);
            Ok(Extracted {
                root: dest.to_path_buf(),
                project,
                report,
            })
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&part);
            Err(e)
        }
    }
}

fn extract_entries<R: Read + Seek>(
    file: &mut R,
    path: &Path,
    part: &Path,
    project_name: &str,
    entries: &[Entry],
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<ArchiveReport, ArchiveError> {
    let mut report = ArchiveReport {
        project_bytes: 0,
        audio_files: 0,
        bytes: 0,
    };
    let mut buf = vec![0u8; BUFFER];
    for entry in entries {
        let target = match &entry.kind {
            Kind::Folder => continue,
            Kind::Project => part.join(project_name),
            Kind::Audio(hash) => part.join(AUDIO_DIR).join(format!("{hash}.wav")),
        };
        let start = data_start(file, path, entry)?;
        file.seek(SeekFrom::Start(start)).map_err(io_at(path))?;
        let mut out =
            BufWriter::with_capacity(BUFFER, File::create_new(&target).map_err(io_at(&target))?);
        let mut left = entry.size;
        let mut crc = Crc::new();
        while left > 0 {
            check_cancel(cancel)?;
            let want = left.min(BUFFER as u64) as usize;
            file.read_exact(&mut buf[..want])
                .map_err(|_| corrupt(format!("{:?} is cut short", entry.name)))?;
            crc.update(&buf[..want]);
            out.write_all(&buf[..want]).map_err(io_at(&target))?;
            left -= want as u64;
            progress.fetch_add(want as u64, Ordering::Relaxed);
        }
        let out = out
            .into_inner()
            .map_err(|e| io_at(&target)(e.into_error()))?;
        out.sync_all().map_err(io_at(&target))?;
        if crc.value() != entry.crc {
            return Err(corrupt(format!("{:?} is damaged", entry.name)));
        }
        report.bytes += entry.size;
        match entry.kind {
            Kind::Project => report.project_bytes = entry.size,
            _ => report.audio_files += 1,
        }
    }
    Ok(report)
}

/// The total bytes [`extract_archive`] writes for the archive `path` (for a progress bar).
///
/// # Errors
/// As [`extract_archive`].
pub fn archive_size(path: &Path, limits: &ArchiveLimits) -> Result<u64, ArchiveError> {
    let mut file = BufReader::new(File::open(path).map_err(io_at(path))?);
    Ok(read_directory(&mut file, path, limits)?
        .iter()
        .map(|e| e.size)
        .sum())
}

#[cfg(test)]
mod tests;
