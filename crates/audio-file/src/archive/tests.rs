use super::*;
use proptest::prelude::*;
use std::io::Cursor as IoCursor;
use std::sync::atomic::AtomicU32;

fn dir(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir()
        .join("atomik-audio-file-archive-unit")
        .join(format!(
            "{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn hash(i: usize) -> String {
    format!("{i:064x}")
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// Writes `files` (hash index, bytes) as audio files in `folder` and returns the list the writer
/// takes.
fn audio_files(folder: &Path, files: &[Vec<u8>]) -> Vec<(String, PathBuf)> {
    files
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let path = folder.join(format!("source-{i}.bin"));
            fs::write(&path, bytes).unwrap();
            (hash(i + 1), path)
        })
        .collect()
}

fn pack(d: &Path, project: &[u8], files: &[Vec<u8>]) -> (PathBuf, Vec<(String, PathBuf)>) {
    let audio = audio_files(d, files);
    let archive = d.join("song.atomikz");
    write_archive(
        &archive,
        "song",
        project,
        &audio,
        &no_cancel(),
        &AtomicU64::new(0),
    )
    .unwrap();
    (archive, audio)
}

fn unpack(archive: &Path, dest: &Path) -> Result<Extracted, ArchiveError> {
    extract_archive(
        archive,
        dest,
        &ArchiveLimits::default(),
        &no_cancel(),
        &AtomicU64::new(0),
    )
}

/// Builds an archive by hand from `(name, data)` pairs, with any names.
fn raw_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = ZipOut::new(Vec::new());
    for (name, data) in entries {
        let mut crc = Crc::new();
        crc.update(data);
        z.begin(name, crc.value(), data.len() as u64).unwrap();
        z.write(data).unwrap();
    }
    z.finish().unwrap()
}

fn raw_file(d: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = d.join("raw.atomikz");
    fs::write(&path, raw_archive(entries)).unwrap();
    path
}

/// Nothing but the archive and what the test made is left in `d`: no `.part` anywhere.
fn no_part_left(d: &Path) {
    for e in fs::read_dir(d).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.ends_with(PART_SUFFIX), "left over: {name}");
    }
}

#[test]
fn the_crc_is_the_zip_one() {
    let mut crc = Crc::new();
    crc.update(b"123456789");
    assert_eq!(crc.value(), 0xCBF4_3926);
}

#[test]
fn a_round_trip_gives_the_same_project_text_and_audio_bytes() {
    let d = dir("round");
    let project = b"{\n  \"format\": \"atomik-project\"\n}\n".to_vec();
    let files = vec![vec![1u8, 2, 3], vec![], vec![9u8; 3 * BUFFER + 17]];
    let (archive, audio) = pack(&d, &project, &files);
    let out = d.join("unpacked");
    let got = unpack(&archive, &out).unwrap();
    assert_eq!(got.root, out);
    assert_eq!(got.project, out.join("unpacked.atomik"));
    assert_eq!(fs::read(&got.project).unwrap(), project);
    for ((hash, source), bytes) in audio.iter().zip(&files) {
        let copy = fs::read(out.join(AUDIO_DIR).join(format!("{hash}.wav"))).unwrap();
        assert_eq!(&copy, bytes);
        assert_eq!(&fs::read(source).unwrap(), bytes);
    }
    assert_eq!(got.report.audio_files, 3);
    assert_eq!(got.report.project_bytes, project.len() as u64);
    no_part_left(&d);
    assert_eq!(
        read_archive_project(&archive, &ArchiveLimits::default()).unwrap(),
        project
    );
}

#[test]
fn the_same_content_is_the_same_archive_bytes() {
    let d = dir("same");
    let (a, _) = pack(&d, b"text", &[vec![5; 100]]);
    let first = fs::read(&a).unwrap();
    let (b, _) = pack(&d, b"text", &[vec![5; 100]]);
    assert_eq!(first, fs::read(&b).unwrap());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    #[test]
    fn random_trees_round_trip_byte_for_byte(
        project in proptest::collection::vec(any::<u8>(), 0..3000),
        files in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..6000), 0..6),
    ) {
        let d = dir("prop");
        let (archive, audio) = pack(&d, &project, &files);
        let out = d.join("out");
        let got = unpack(&archive, &out).unwrap();
        prop_assert_eq!(&fs::read(&got.project).unwrap(), &project);
        for ((hash, _), bytes) in audio.iter().zip(&files) {
            let copy = fs::read(out.join(AUDIO_DIR).join(format!("{hash}.wav"))).unwrap();
            prop_assert_eq!(&copy, bytes);
        }
        prop_assert_eq!(archive_size(&archive, &ArchiveLimits::default()).unwrap(),
            project.len() as u64 + files.iter().map(|f| f.len() as u64).sum::<u64>());
        let _ = fs::remove_dir_all(&d);
    }
}

#[test]
fn entry_names_are_classified_by_shape() {
    let ok = |name: &str| classify(name, &mut None);
    assert_eq!(ok("song/song.atomik"), Ok(Kind::Project));
    assert_eq!(ok("song/"), Ok(Kind::Folder));
    assert_eq!(ok("song/Audio/"), Ok(Kind::Folder));
    assert_eq!(
        ok(&format!("song/Audio/{}.wav", hash(7))),
        Ok(Kind::Audio(hash(7)))
    );
    for bad in [
        "../evil.atomik",
        "song/../evil.atomik",
        "song/Audio/../x.wav",
        "/song/song.atomik",
        "C:/song/song.atomik",
        &format!("C:{0}song{0}song.atomik", '\\'),
        "song\\song.atomik",
        "song//song.atomik",
        "song/other.atomik",
        "song/Audio/kick.wav",
        "song/Audio/ABCDEF.wav",
        &format!("song/Audio/{}.WAV", hash(1)),
        &format!("song/Audio/{}.wav.exe", hash(1)),
        &format!("song/Audio/sub/{}.wav", hash(1)),
        "song/song.atomik/",
        "song/Audio/nul\u{0}.wav",
        "./song/song.atomik",
        "song./song./song..atomik",
        "..",
        "",
    ] {
        assert!(classify(bad, &mut None).is_err(), "{bad:?} must be refused");
    }
    // Every entry shares the folder of the first one.
    let mut root = None;
    classify("song/song.atomik", &mut root).unwrap();
    assert!(classify("other/other.atomik", &mut root).is_err());
}

#[test]
fn an_archive_with_a_name_that_leaves_the_folder_is_refused_before_anything_is_written() {
    let d = dir("traversal");
    for name in [
        "../evil.atomik",
        "song/../../evil.atomik",
        "/etc/passwd",
        "song/Audio/../../x.wav",
    ] {
        let file = raw_file(&d, &[("song/song.atomik", b"{}"), (name, b"x")]);
        let out = d.join("out");
        let e = unpack(&file, &out).unwrap_err();
        assert!(matches!(e, ArchiveError::Unsafe(_)), "{name}: {e}");
        assert!(!out.exists());
        no_part_left(&d);
        assert!(!d.parent().unwrap().join("evil.atomik").exists());
    }
}

#[test]
fn a_second_project_a_duplicate_or_no_project_is_refused() {
    let d = dir("shape");
    let out = d.join("out");
    let none = raw_file(&d, &[(&format!("song/Audio/{}.wav", hash(1)), b"x")]);
    assert!(matches!(unpack(&none, &out), Err(ArchiveError::Unsafe(_))));
    let twice = raw_file(
        &d,
        &[("song/song.atomik", b"{}"), ("song/song.atomik", b"{}")],
    );
    assert!(matches!(unpack(&twice, &out), Err(ArchiveError::Unsafe(_))));
    let other_root = raw_file(&d, &[("song/song.atomik", b"{}"), ("x/x.atomik", b"{}")]);
    assert!(matches!(
        unpack(&other_root, &out),
        Err(ArchiveError::Unsafe(_))
    ));
    assert!(!out.exists());
}

#[test]
fn compressed_and_encrypted_entries_are_refused() {
    let d = dir("method");
    let mut bytes = raw_archive(&[("song/song.atomik", b"{}")]);
    let at = bytes
        .windows(4)
        .position(|w| w == SIG_CENTRAL.to_le_bytes())
        .unwrap();
    let mut deflated = bytes.clone();
    deflated[at + 10] = 8;
    let file = d.join("deflated.atomikz");
    fs::write(&file, &deflated).unwrap();
    let e = unpack(&file, &d.join("out")).unwrap_err();
    assert!(matches!(e, ArchiveError::Unsafe(_)), "{e}");
    bytes[at + 8] |= 1;
    fs::write(&file, &bytes).unwrap();
    let e = unpack(&file, &d.join("out")).unwrap_err();
    assert!(matches!(e, ArchiveError::Unsafe(_)), "{e}");
    assert!(!d.join("out").exists());
}

#[test]
fn the_limits_refuse_oversize_entries_totals_projects_and_counts() {
    let d = dir("limits");
    let files = vec![vec![0u8; 1000], vec![0u8; 1000]];
    let (archive, _) = pack(&d, &[b'x'; 100], &files);
    let try_with = |limits: ArchiveLimits| {
        extract_archive(
            &archive,
            &d.join("out"),
            &limits,
            &no_cancel(),
            &AtomicU64::new(0),
        )
    };
    let base = ArchiveLimits::default();
    for limits in [
        ArchiveLimits {
            max_entry_bytes: 999,
            ..base
        },
        ArchiveLimits {
            max_total_bytes: 2099,
            ..base
        },
        ArchiveLimits {
            max_project_bytes: 99,
            ..base
        },
        ArchiveLimits {
            max_entries: 2,
            ..base
        },
    ] {
        let e = try_with(limits).unwrap_err();
        assert!(matches!(e, ArchiveError::TooLarge(_)), "{limits:?}: {e}");
        assert!(!d.join("out").exists());
        no_part_left(&d);
    }
    // The same archive passes at the limits themselves.
    try_with(ArchiveLimits {
        max_entry_bytes: 1000,
        max_total_bytes: 2100,
        max_project_bytes: 100,
        max_entries: 3,
    })
    .unwrap();
}

#[test]
fn a_damaged_entry_fails_and_removes_everything_written() {
    let d = dir("damaged");
    let (archive, _) = pack(&d, b"project text", &[vec![7u8; 5000], vec![8u8; 5000]]);
    let mut bytes = fs::read(&archive).unwrap();
    // Flip a byte inside the last audio file's data.
    let at = bytes
        .windows(4)
        .position(|w| w == SIG_CENTRAL.to_le_bytes())
        .unwrap();
    bytes[at - 100] ^= 0xFF;
    fs::write(&archive, &bytes).unwrap();
    let out = d.join("out");
    let e = unpack(&archive, &out).unwrap_err();
    assert!(matches!(e, ArchiveError::Corrupt(_)), "{e}");
    assert!(!out.exists());
    no_part_left(&d);
    // A truncated file is not an archive.
    fs::write(&archive, &bytes[..bytes.len() - 30]).unwrap();
    assert!(matches!(
        unpack(&archive, &out),
        Err(ArchiveError::Corrupt(_))
    ));
    fs::write(&archive, b"not a zip file at all").unwrap();
    assert!(matches!(
        unpack(&archive, &out),
        Err(ArchiveError::Corrupt(_))
    ));
    assert!(!out.exists());
}

#[test]
fn an_entry_that_lies_about_its_size_is_refused() {
    let d = dir("lies");
    let mut bytes = raw_archive(&[("song/song.atomik", b"0123456789")]);
    let at = bytes
        .windows(4)
        .position(|w| w == SIG_CENTRAL.to_le_bytes())
        .unwrap();
    // The directory claims 4 GiB - 2 bytes of data that is not there.
    bytes[at + 20..at + 24].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    bytes[at + 24..at + 28].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    let file = d.join("lies.atomikz");
    fs::write(&file, &bytes).unwrap();
    let e = unpack(&file, &d.join("out")).unwrap_err();
    assert!(
        matches!(e, ArchiveError::TooLarge(_) | ArchiveError::Corrupt(_)),
        "{e}"
    );
    assert!(!d.join("out").exists());
}

#[test]
fn an_existing_destination_is_never_touched() {
    let d = dir("exists");
    let (archive, _) = pack(&d, b"text", &[]);
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("keep.txt"), b"mine").unwrap();
    assert!(matches!(
        unpack(&archive, &out),
        Err(ArchiveError::Exists(_))
    ));
    assert_eq!(fs::read(out.join("keep.txt")).unwrap(), b"mine");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn cancelling_removes_the_partial_output_in_both_directions() {
    let d = dir("cancel");
    let audio = audio_files(&d, &[vec![1u8; 4 * BUFFER], vec![2u8; 4 * BUFFER]]);
    let target = d.join("song.atomikz");
    let cancel = AtomicBool::new(true);
    let e = write_archive(&target, "song", b"x", &audio, &cancel, &AtomicU64::new(0)).unwrap_err();
    assert!(matches!(e, ArchiveError::Cancelled));
    assert!(!target.exists());
    no_part_left(&d);
    // A write that existed keeps its file when a later write is cancelled.
    let (archive, _) = pack(&d, b"x", &[vec![1u8; 10]]);
    let before = fs::read(&archive).unwrap();
    let e = write_archive(&archive, "song", b"y", &audio, &cancel, &AtomicU64::new(0)).unwrap_err();
    assert!(matches!(e, ArchiveError::Cancelled));
    assert_eq!(fs::read(&archive).unwrap(), before);
    no_part_left(&d);
    let out = d.join("out");
    let e = extract_archive(
        &archive,
        &out,
        &ArchiveLimits::default(),
        &cancel,
        &AtomicU64::new(0),
    )
    .unwrap_err();
    assert!(matches!(e, ArchiveError::Cancelled) || matches!(e, ArchiveError::Corrupt(_)));
    assert!(!out.exists());
    no_part_left(&d);
}

#[test]
fn cancelling_in_the_middle_leaves_nothing() {
    let d = dir("cancel-mid");
    let audio = audio_files(&d, &[vec![1u8; 8 * BUFFER]]);
    let target = d.join("song.atomikz");
    // The progress counter is the only signal the worker gives: a cancel raised once a part of
    // the copy is done stops it with the part file removed.
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let progress = std::sync::Arc::new(AtomicU64::new(0));
    let (c, p) = (cancel.clone(), progress.clone());
    let watcher = std::thread::spawn(move || {
        while p.load(Ordering::Relaxed) < 2 * BUFFER as u64 {
            std::thread::yield_now();
        }
        c.store(true, Ordering::Relaxed);
    });
    let result = write_archive(&target, "song", b"x", &audio, &cancel, &progress);
    watcher.join().unwrap();
    // The copy may have finished before the watcher looked; either way no part file is left.
    if result.is_err() {
        assert!(!target.exists());
    }
    no_part_left(&d);
}

#[test]
fn the_writer_refuses_names_and_ids_that_are_not_valid() {
    let d = dir("names");
    let target = d.join("a.atomikz");
    let go = |name: &str, audio: &[(String, PathBuf)]| {
        write_archive(&target, name, b"x", audio, &no_cancel(), &AtomicU64::new(0))
    };
    for bad in ["", "..", "a/b", "a\\b", "a:b", "x.", &"n".repeat(101)] {
        assert!(
            matches!(go(bad, &[]), Err(ArchiveError::Unsafe(_))),
            "{bad:?}"
        );
    }
    let e = go("ok", &[("not-a-hash".into(), d.join("x"))]);
    assert!(matches!(e, Err(ArchiveError::Unsafe(_))));
    let e = go("ok", &[(hash(1), d.join("missing.wav"))]);
    assert!(matches!(e, Err(ArchiveError::Io { .. })));
    assert!(!target.exists());
    no_part_left(&d);
}

#[test]
fn zip64_sizes_and_offsets_are_written_and_read() {
    // A header for a 5 GiB entry at an offset of 6 GiB, without the data: the writer only
    // records them, and the reader decodes the directory.
    let mut z = ZipOut::new(Vec::new());
    let name = format!("song/Audio/{}.wav", hash(1));
    z.at = 6 << 30;
    z.begin(&name, 0x1234_5678, 5 << 30).unwrap();
    z.at = z.out.len() as u64;
    let project_at = z.at;
    z.begin("song/song.atomik", 0, 0).unwrap();
    let bytes = z.finish().unwrap();
    let mut cursor = IoCursor::new(bytes);
    let entries = read_directory(&mut cursor, Path::new("x"), &ArchiveLimits::default()).unwrap();
    assert_eq!(entries[0].size, 5 << 30);
    assert_eq!(entries[0].offset, 6 << 30);
    assert_eq!(entries[0].crc, 0x1234_5678);
    assert_eq!(entries[1].offset, project_at);
    // And one above the entry limit is refused from the directory alone.
    let tight = ArchiveLimits {
        max_entry_bytes: 4 << 30,
        ..ArchiveLimits::default()
    };
    cursor.set_position(0);
    assert!(matches!(
        read_directory(&mut cursor, Path::new("x"), &tight),
        Err(ArchiveError::TooLarge(_))
    ));
}

#[test]
fn many_entries_use_the_zip64_end_record() {
    let mut z = ZipOut::new(Vec::new());
    z.begin("song/song.atomik", 0, 0).unwrap();
    for i in 0..70_000 {
        z.begin(&format!("song/Audio/{}.wav", hash(i + 1)), 0, 0)
            .unwrap();
    }
    let bytes = z.finish().unwrap();
    assert!(
        bytes.windows(4).any(|w| w == SIG_END64.to_le_bytes()),
        "a ZIP64 end record"
    );
    let mut cursor = IoCursor::new(bytes);
    let entries = read_directory(&mut cursor, Path::new("x"), &ArchiveLimits::default()).unwrap();
    assert_eq!(entries.len(), 70_001);
    // More than the limit is refused before the entries are read.
    cursor.set_position(0);
    let few = ArchiveLimits {
        max_entries: 1000,
        ..ArchiveLimits::default()
    };
    assert!(matches!(
        read_directory(&mut cursor, Path::new("x"), &few),
        Err(ArchiveError::TooLarge(_))
    ));
}
