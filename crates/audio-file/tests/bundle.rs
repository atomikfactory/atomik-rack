//! (bundle part): import, atomic save, copy, orphans, missing assets, streaming.
#![allow(missing_docs)]

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use atomik_audio_file::{
    AudioData, AudioFileError, Bundle, PEAK_LEVELS, PeakPyramid, WavStreamReader, content_hash,
    decode_file, is_valid_hash, resample, save_text_atomic,
};
use common::*;

/// Every file under `root` as (relative path, bytes), sorted.
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn recording(frames: usize, seed: f32) -> AudioData {
    let l: Vec<f32> = (0..frames)
        .map(|i| ((i as f32) * 0.01 + seed).sin() * 0.5)
        .collect();
    let r: Vec<f32> = l.iter().map(|v| -v).collect();
    AudioData::new(48_000, vec![l, r]).unwrap()
}

#[test]
fn create_and_open_set_up_the_layout() {
    let root = tmp_dir("layout").join("Song");
    let b = Bundle::create(&root).unwrap();
    assert_eq!(b.audio_dir(), root.join("Audio"));
    assert_eq!(b.peaks_dir(), root.join("Audio").join(".peaks"));
    assert!(b.audio_dir().is_dir() && b.peaks_dir().is_dir());
    Bundle::open(&root).unwrap();
    assert!(Bundle::open(&root.join("nope")).is_err());
    // Creating again is harmless.
    Bundle::create(&root).unwrap();
}

#[test]
fn import_decodes_converts_hashes_and_stores() {
    let root = tmp_dir("import");
    let mut b = Bundle::create(&root.join("P")).unwrap();
    let info = b.import(&fixture("stereo_flac.flac"), 48_000).unwrap();
    assert_eq!(
        (
            info.channels,
            info.sample_rate,
            info.frames,
            info.original_name.as_str()
        ),
        (2, 48_000, 4800, "stereo_flac.flac")
    );
    assert!(is_valid_hash(&info.hash));
    assert!(b.exists(&info.hash));
    assert_eq!(
        b.asset_path(&info.hash),
        b.audio_dir().join(format!("{}.wav", info.hash))
    );

    // What is stored is the decoded, resampled audio, and its hash is the content hash.
    let expect = resample(&decode_file(&fixture("stereo_flac.flac")).unwrap(), 48_000).unwrap();
    let stored = b.load(&info.hash).unwrap();
    assert_eq!(stored, expect);
    assert_eq!(content_hash(&stored), info.hash);
    // Peaks were written next to it.
    assert!(b.peaks_dir().read_dir().unwrap().count() >= 1);
}

#[test]
fn import_without_rate_change_keeps_the_samples() {
    let mut b = Bundle::create(&tmp_dir("import-same").join("P")).unwrap();
    let info = b.import(&fixture("mono_wav24.wav"), RATE).unwrap();
    assert_eq!(
        (info.channels, info.sample_rate, info.frames),
        (1, RATE, FRAMES as u64)
    );
    assert_eq!(b.load(&info.hash).unwrap().channels, source_f32(false));
}

#[test]
fn import_is_idempotent() {
    let mut b = Bundle::create(&tmp_dir("idem").join("P")).unwrap();
    let a = b.import(&fixture("stereo_wav16.wav"), 48_000).unwrap();
    let before = tree(b.audio_dir().as_path());
    let again = b.import(&fixture("stereo_wav16.wav"), 48_000).unwrap();
    assert_eq!(a, again);
    assert_eq!(
        before,
        tree(b.audio_dir().as_path()),
        "second import changes nothing on disk"
    );
    // The same audio from a different container is the same asset (same hash), stored once.
    let wav24 = b.import(&fixture("stereo_wav24.wav"), 48_000).unwrap();
    assert_eq!(wav24.hash, a.hash);
    assert_eq!(wav24.original_name, "stereo_wav24.wav");
    let wavs = std::fs::read_dir(b.audio_dir()).unwrap().filter(|e| {
        e.as_ref()
            .unwrap()
            .path()
            .extension()
            .is_some_and(|x| x == "wav")
    });
    assert_eq!(wavs.count(), 1);
}

#[test]
fn import_errors_are_reported_not_fatal() {
    let mut b = Bundle::create(&tmp_dir("import-err").join("P")).unwrap();
    assert!(matches!(
        b.import(&fixture("missing.wav"), 48_000),
        Err(AudioFileError::Io(_))
    ));
    let junk = tmp_dir("import-junk").join("junk.wav");
    std::fs::write(&junk, b"not audio").unwrap();
    assert!(b.import(&junk, 48_000).is_err());
    assert!(b.import(&fixture("stereo_wav16.wav"), 0).is_err());
    assert_eq!(
        b.orphans(&BTreeSet::new()),
        Vec::<String>::new(),
        "nothing half-written"
    );
}

#[test]
fn add_recording_stores_bit_exactly() {
    let mut b = Bundle::create(&tmp_dir("rec").join("P")).unwrap();
    let rec = recording(10_000, 0.3);
    let info = b.add_recording(&rec, "Take 1").unwrap();
    assert_eq!(
        (
            info.channels,
            info.sample_rate,
            info.frames,
            info.original_name.as_str()
        ),
        (2, 48_000, 10_000, "Take 1")
    );
    assert_eq!(info.hash, content_hash(&rec));
    let back = b.load(&info.hash).unwrap();
    for (a, c) in rec
        .channels
        .iter()
        .flatten()
        .zip(back.channels.iter().flatten())
    {
        assert_eq!(a.to_bits(), c.to_bits());
    }
    assert_eq!(
        b.add_recording(&rec, "Take 1 again").unwrap().hash,
        info.hash
    );
    let empty = AudioData::new(48_000, vec![vec![]]).unwrap();
    assert!(matches!(
        b.add_recording(&empty, "x"),
        Err(AudioFileError::Empty)
    ));
}

#[test]
fn peaks_are_cached_and_regenerated() {
    let mut b = Bundle::create(&tmp_dir("peaks").join("P")).unwrap();
    let rec = recording(30_000, 1.0);
    let info = b.add_recording(&rec, "r").unwrap();
    let expect = PeakPyramid::build(&rec).unwrap();
    assert_eq!(b.load_peaks(&info.hash).unwrap(), expect);
    assert_eq!(expect.levels().len(), PEAK_LEVELS.len());

    // Delete the cache: it is regenerated (and written back).
    let cached: Vec<_> = std::fs::read_dir(b.peaks_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(cached.len(), 1);
    std::fs::remove_file(&cached[0]).unwrap();
    assert_eq!(b.load_peaks(&info.hash).unwrap(), expect);
    assert!(cached[0].exists(), "regenerated cache is written back");

    // A corrupt cache is treated like a missing one.
    std::fs::write(&cached[0], b"garbage").unwrap();
    assert_eq!(b.load_peaks(&info.hash).unwrap(), expect);
    assert_eq!(
        PeakPyramid::from_bytes(&std::fs::read(&cached[0]).unwrap()).unwrap(),
        expect
    );
}

#[test]
fn missing_assets_are_clear_errors() {
    let mut b = Bundle::create(&tmp_dir("missing").join("P")).unwrap();
    let hash = "0".repeat(64);
    assert!(!b.exists(&hash));
    assert!(matches!(b.load(&hash), Err(AudioFileError::AssetMissing { hash: h }) if h == hash));
    assert!(matches!(
        b.load_peaks(&hash),
        Err(AudioFileError::AssetMissing { .. })
    ));
    assert!(matches!(
        b.stream_reader(&hash),
        Err(AudioFileError::AssetMissing { .. })
    ));

    // Deleting a real asset afterwards is also reported, never a panic.
    let info = b.add_recording(&recording(500, 0.0), "r").unwrap();
    std::fs::remove_file(b.asset_path(&info.hash)).unwrap();
    assert!(!b.exists(&info.hash));
    assert!(matches!(
        b.load(&info.hash),
        Err(AudioFileError::AssetMissing { .. })
    ));
    let e = b.load(&info.hash).unwrap_err();
    assert!(e.to_string().contains(&info.hash), "{e}");
}

#[test]
fn hostile_hashes_cannot_escape_the_bundle() {
    let b = Bundle::create(&tmp_dir("hostile").join("P")).unwrap();
    for bad in [
        "../../etc/passwd",
        "",
        "ABC",
        &"g".repeat(64),
        &"A".repeat(64),
        "a/b",
    ] {
        assert!(!is_valid_hash(bad), "{bad}");
        assert!(!b.exists(bad));
        assert!(b.load(bad).is_err());
        assert!(b.asset_path(bad).starts_with(b.audio_dir()));
        assert!(!b.asset_path(bad).to_string_lossy().contains(".."));
    }
}

#[test]
fn orphans_are_detected_and_removed() {
    let mut b = Bundle::create(&tmp_dir("orphans").join("P")).unwrap();
    let a = b.add_recording(&recording(1000, 0.1), "a").unwrap();
    let c = b.add_recording(&recording(1000, 0.2), "c").unwrap();
    let d = b.add_recording(&recording(1000, 0.3), "d").unwrap();
    // Unrelated files in Audio/ are never touched or reported.
    std::fs::write(b.audio_dir().join("notes.txt"), b"keep me").unwrap();
    std::fs::write(b.audio_dir().join("stale.wav.tmp~"), b"half a write").unwrap();

    let referenced: BTreeSet<String> = [a.hash.clone()].into();
    let mut expect = vec![c.hash.clone(), d.hash.clone()];
    expect.sort();
    assert_eq!(b.orphans(&referenced), expect);
    assert_eq!(
        b.orphans(&[a.hash.clone(), c.hash.clone(), d.hash.clone()].into()),
        Vec::<String>::new()
    );

    let removed = b.remove_orphans(&referenced).unwrap();
    assert_eq!(removed, expect);
    assert!(b.exists(&a.hash) && !b.exists(&c.hash) && !b.exists(&d.hash));
    assert!(b.audio_dir().join("notes.txt").exists());
    assert!(
        !b.audio_dir().join("stale.wav.tmp~").exists(),
        "stale temp files are cleaned"
    );
    // Peak caches of removed assets go with them; the kept one stays.
    assert_eq!(std::fs::read_dir(b.peaks_dir()).unwrap().count(), 1);
    assert!(b.load_peaks(&a.hash).is_ok());
    assert_eq!(b.orphans(&referenced), Vec::<String>::new());
}

#[test]
fn copy_bundle_gives_identical_trees() {
    let root = tmp_dir("copy");
    let mut src = Bundle::create(&root.join("Src")).unwrap();
    src.import(&fixture("stereo_flac.flac"), 48_000).unwrap();
    src.add_recording(&recording(4000, 0.5), "rec").unwrap();
    let before = tree(src.audio_dir().as_path());
    assert!(before.len() >= 4, "two assets plus their peaks");

    let dst = Bundle::copy_bundle(&root.join("Src"), &root.join("Dst")).unwrap();
    assert_eq!(before, tree(dst.audio_dir().as_path()));
    assert_eq!(before, tree(src.audio_dir().as_path()), "source untouched");
    // Copying again over an existing copy is fine and stays identical.
    Bundle::copy_bundle(&root.join("Src"), &root.join("Dst")).unwrap();
    assert_eq!(before, tree(dst.audio_dir().as_path()));
    // A copy of a bundle that was never created is an error, not a panic.
    assert!(Bundle::copy_bundle(&root.join("Nope"), &root.join("Dst2")).is_err());
}

#[test]
fn save_text_atomic_replaces_whole_file_and_leaves_no_temp() {
    let dir = tmp_dir("atomic");
    let target = dir.join("song.atomik");
    save_text_atomic(&target, "{\"v\":1}").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"v\":1}");
    save_text_atomic(&target, "{\"v\":2,\"longer\":true}").unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "{\"v\":2,\"longer\":true}"
    );
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["song.atomik"], "no temp file remains");
}

#[test]
fn failed_atomic_save_keeps_the_old_file_and_cleans_up() {
    let dir = tmp_dir("atomic-fail");
    let target = dir.join("song.atomik");
    save_text_atomic(&target, "good").unwrap();
    // The destination is not writable as a file: the rename fails after the temp file is complete.
    let blocked = dir.join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    assert!(save_text_atomic(&blocked, "data").is_err());
    // The directory for the temp file does not exist.
    assert!(save_text_atomic(&dir.join("missing-dir").join("x"), "data").is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "good");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["blocked", "song.atomik"],
        "no temp file after failures"
    );
}

#[test]
fn stream_reader_over_a_stored_asset_matches_a_full_load() {
    let mut b = Bundle::create(&tmp_dir("stream").join("P")).unwrap();
    let rec = recording(20_000, 0.9);
    let info = b.add_recording(&rec, "r").unwrap();
    let full = b.load(&info.hash).unwrap();
    let mut r: WavStreamReader = b.stream_reader(&info.hash).unwrap();
    assert_eq!(
        (r.sample_rate(), r.channel_count(), r.frames()),
        (48_000, 2, 20_000)
    );
    let mut out = vec![vec![0.0f32; 4096]; 2];
    for start in [0u64, 4096, 17_000, 3, 19_999, 8192] {
        let n = r.read_frames(start, &mut out).unwrap();
        assert_eq!(n, 4096usize.min(20_000 - start as usize));
        for (got, want) in out.iter().zip(&full.channels) {
            assert_eq!(&got[..n], &want[start as usize..start as usize + n]);
        }
    }
}

#[test]
fn convert_rate_stores_an_aligned_copy_and_keeps_the_original() {
    let root = tmp_dir("convert_rate");
    let mut b = Bundle::create(&root.join("P")).unwrap();
    let tone = AudioData::new(48_000, vec![sine(48_000, 1_000.0, 0.5, 48_000)]).unwrap();
    let a = b.add_recording(&tone, "tone").unwrap();
    for rate in [44_100u32, 96_000] {
        let c = b.convert_rate(&a.hash, rate, "tone").unwrap();
        assert_ne!(c.hash, a.hash);
        assert_eq!((c.sample_rate, c.channels), (rate, 1));
        assert_eq!(c.frames, u64::from(rate));
        let out = b.load(&c.hash).unwrap();
        let want = sine(rate, 1_000.0, 0.5, rate as usize);
        // Away from the edges the copy matches the ideal tone sample for sample.
        let worst = (2_000..out.channels[0].len() - 2_000)
            .map(|i| (out.channels[0][i] - want[i]).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 2e-3, "{rate} Hz: worst error {worst}");
    }
    assert!(b.exists(&a.hash), "the original stays");
}

#[test]
fn convert_rate_from_stores_the_copy_here_and_never_writes_the_source() {
    let root = tmp_dir("convert_from");
    let mut other = Bundle::create(&root.join("Other")).unwrap();
    let tone = AudioData::new(48_000, vec![sine(48_000, 1_000.0, 0.5, 4_800)]).unwrap();
    let a = other.add_recording(&tone, "tone").unwrap();
    let mut own = Bundle::create(&root.join("Own")).unwrap();
    let before = tree(&root.join("Other"));
    let sources = vec![root.join("No Such Folder"), root.join("Other")];

    let c = own
        .convert_rate_from(&sources, &a.hash, 44_100, "tone")
        .unwrap();
    assert_eq!((c.sample_rate, c.frames), (44_100, 4_410));
    assert!(own.exists(&c.hash), "the copy is in the target");
    assert!(!own.exists(&a.hash), "the original is not copied over");
    assert_eq!(tree(&root.join("Other")), before, "the source is untouched");

    // An asset the target holds is read from it; one nobody holds is missing.
    let own_asset = own.add_recording(&tone, "tone").unwrap();
    assert_eq!(own_asset.hash, a.hash);
    assert!(own.convert_rate_from(&[], &a.hash, 96_000, "tone").is_ok());
    let ghost = "f".repeat(64);
    assert!(matches!(
        own.convert_rate_from(&sources, &ghost, 44_100, "x"),
        Err(AudioFileError::AssetMissing { .. })
    ));
}

#[test]
fn collect_from_copies_only_the_wanted_assets_and_reports_the_rest() {
    let root = tmp_dir("collect");
    let mut shared = Bundle::create(&root.join("Shared")).unwrap();
    let a = shared.add_recording(&recording(4000, 0.1), "a").unwrap();
    let b = shared.add_recording(&recording(4100, 0.2), "b").unwrap();
    let unrelated = shared.add_recording(&recording(4200, 0.3), "c").unwrap();
    let mut elsewhere = Bundle::create(&root.join("Elsewhere")).unwrap();
    let d = elsewhere.add_recording(&recording(4300, 0.4), "d").unwrap();
    let mut own = Bundle::create(&root.join("Own")).unwrap();
    let held = own.add_recording(&recording(4400, 0.5), "e").unwrap();
    let before = tree(&root.join("Shared"));

    let ghost = "f".repeat(64);
    let wanted: BTreeSet<String> = [&a, &b, &d, &held]
        .iter()
        .map(|i| i.hash.clone())
        .chain([ghost.clone(), "not a hash".to_owned()])
        .collect();
    let sources = vec![
        root.join("Own"),
        root.join("Shared"),
        root.join("Elsewhere"),
        root.join("No Such Folder"),
    ];
    let report = own.collect_from(&sources, &wanted).unwrap();

    let mut copied = vec![a.hash.clone(), b.hash.clone(), d.hash.clone()];
    copied.sort();
    assert_eq!(report.copied, copied);
    assert_eq!(report.present, vec![held.hash.clone()]);
    let mut missing = vec![ghost, "not a hash".to_owned()];
    missing.sort();
    assert_eq!(report.missing, missing);
    assert!(report.bytes > 0);
    for hash in &copied {
        assert!(own.exists(hash));
        assert!(own.peaks_path(hash).is_file(), "the peak cache comes along");
        assert_eq!(
            std::fs::read(own.asset_path(hash)).unwrap().len() as u64,
            std::fs::metadata(
                [&shared, &elsewhere]
                    .iter()
                    .map(|s| s.asset_path(hash))
                    .find(|p| p.is_file())
                    .unwrap()
            )
            .unwrap()
            .len()
        );
    }
    assert!(
        !own.exists(&unrelated.hash),
        "what no one wants stays behind"
    );
    assert_eq!(
        before,
        tree(&root.join("Shared")),
        "a source is never changed"
    );

    // Collecting again copies nothing more.
    let again = own.collect_from(&sources, &wanted).unwrap();
    assert!(again.copied.is_empty() && again.bytes == 0);
    assert_eq!(again.present.len(), 4);
}

fn ramp(len: usize, step: f32) -> AudioData {
    AudioData::new(
        48_000,
        vec![(0..len).map(|i| i as f32 * step).collect::<Vec<f32>>()],
    )
    .unwrap()
}

#[test]
fn a_folder_with_another_project_file_counts_as_shared() {
    let dir = std::env::temp_dir().join(format!("atomik-bundle-shared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    let bundle = Bundle::create(&dir).unwrap();
    let mine = dir.join("mine.atomik");
    std::fs::write(&mine, "x").unwrap();
    assert!(
        !bundle.shared_with_other_projects(&mine),
        "its own file alone is not sharing"
    );
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    assert!(
        !bundle.shared_with_other_projects(&mine),
        "other files do not count"
    );
    std::fs::write(dir.join("nested").join("deep.atomik"), "x").unwrap();
    assert!(
        bundle.shared_with_other_projects(&mine),
        "a project one folder down counts"
    );
    std::fs::remove_file(dir.join("nested").join("deep.atomik")).unwrap();
    std::fs::write(dir.join("other.atomik"), "x").unwrap();
    assert!(bundle.shared_with_other_projects(&mine));
    // A project saved from elsewhere into this folder: every project file here is another.
    assert!(bundle.shared_with_other_projects(&dir.join("elsewhere").join("mine.atomik")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shared_audio_folder_is_recognised_for_a_bundle_the_project_lives_in_alone() {
    let dir = std::env::temp_dir().join(format!("atomik-bundle-alone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut bundle = Bundle::create(&dir).unwrap();
    let keep = bundle.add_recording(&ramp(100, 0.001), "keep").unwrap();
    let stray = bundle.add_recording(&ramp(100, 0.002), "stray").unwrap();
    let mine = dir.join("mine.atomik");
    std::fs::write(&mine, "x").unwrap();
    assert!(!bundle.shared_with_other_projects(&mine));
    let wanted: BTreeSet<String> = [keep.hash.clone()].into();
    bundle.remove_orphans(&wanted).unwrap();
    assert!(bundle.exists(&keep.hash));
    assert!(!bundle.exists(&stray.hash));
    let _ = std::fs::remove_dir_all(&dir);
}
