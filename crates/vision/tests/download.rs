//! The model download through a fake fetcher: no network.

use atomik_vision::download::{
    DownloadError, FakeFetcher, MODEL_FILES, ModelFile, ModelStore, Progress, Status, sha256_hex,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("atomik-vision-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Two small files standing in for the models, and a fetcher that serves them.
fn files() -> (Vec<ModelFile>, FakeFetcher, Vec<Vec<u8>>) {
    let bodies: Vec<Vec<u8>> = vec![
        (0..5000u32).map(|i| (i % 251) as u8).collect(),
        (0..3000u32).map(|i| (i % 13) as u8).collect(),
    ];
    let mut fetcher = FakeFetcher::new();
    let mut files = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let name: &'static str = Box::leak(format!("m{i}.tflite").into_boxed_str());
        let url: &'static str =
            Box::leak(format!("https://example.invalid/{name}").into_boxed_str());
        let sha: &'static str = Box::leak(sha256_hex(body).into_boxed_str());
        fetcher.serve(url, body.clone());
        files.push(ModelFile {
            name,
            url,
            size: body.len() as u64,
            sha256: sha,
        });
    }
    (files, fetcher, bodies)
}

fn no_progress(_: Progress) {}

fn leftovers(dir: &PathBuf) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn the_pinned_constants_are_well_formed() {
    assert_eq!(MODEL_FILES.len(), 2);
    for f in MODEL_FILES {
        assert!(f.url.starts_with("https://"), "{}", f.url);
        assert_eq!(f.sha256.len(), 64);
        assert!(
            f.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert!(f.size > 1_000_000);
        assert!(f.name.ends_with(".tflite"));
    }
}

#[test]
fn a_good_download_leaves_the_files_ready() {
    let dir = temp_dir("good");
    let (files, fetcher, bodies) = files();
    let store = ModelStore::with_files(dir.clone(), files.clone());
    assert_eq!(store.status(), Status::Missing);
    let mut last = None;
    store
        .download(&fetcher, &mut |p| last = Some(p), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(store.status(), Status::Ready);
    for (f, body) in files.iter().zip(&bodies) {
        assert_eq!(&std::fs::read(dir.join(f.name)).unwrap(), body);
    }
    assert_eq!(
        leftovers(&dir).len(),
        2,
        "no temp files: {:?}",
        leftovers(&dir)
    );
    let last = last.expect("progress was reported");
    assert_eq!(last.received, 8000);
    assert_eq!(last.total, 8000);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hash_mismatch_leaves_nothing_and_missing() {
    let dir = temp_dir("mismatch");
    let (files, mut fetcher, _) = files();
    // The second file arrives with the right size and the wrong bytes.
    fetcher.serve(files[1].url, vec![7u8; files[1].size as usize]);
    let store = ModelStore::with_files(dir.clone(), files);
    let err = store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(err, DownloadError::HashMismatch { .. }), "{err}");
    assert_eq!(store.status(), Status::Missing);
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_size_leaves_nothing() {
    let dir = temp_dir("size");
    let (files, mut fetcher, _) = files();
    fetcher.serve(files[0].url, vec![1u8; 10]);
    let store = ModelStore::with_files(dir.clone(), files);
    let err = store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(err, DownloadError::WrongSize { .. }), "{err}");
    assert_eq!(store.status(), Status::Missing);
    assert!(leftovers(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_request_leaves_nothing() {
    let dir = temp_dir("failed");
    let (files, mut fetcher, _) = files();
    fetcher.fail(files[1].url, "no network");
    let store = ModelStore::with_files(dir.clone(), files);
    let err = store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(err, DownloadError::Fetch(_)), "{err}");
    assert_eq!(store.status(), Status::Missing);
    assert!(
        leftovers(&dir).is_empty(),
        "the first file is not kept either: {:?}",
        leftovers(&dir)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_half_way_leaves_nothing() {
    let dir = temp_dir("cancel");
    let (files, fetcher, _) = files();
    let store = ModelStore::with_files(dir.clone(), files);
    let cancel = AtomicBool::new(false);
    let err = store
        .download(
            &fetcher,
            &mut |p: Progress| {
                if p.received > 6000 {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, DownloadError::Cancelled), "{err}");
    assert_eq!(store.status(), Status::Missing);
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_file_on_disk_is_corrupt_and_a_download_repairs_it() {
    let dir = temp_dir("corrupt");
    let (files, fetcher, bodies) = files();
    let store = ModelStore::with_files(dir.clone(), files.clone());
    store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap();
    // Same size, one byte changed.
    let mut damaged = bodies[0].clone();
    damaged[100] ^= 0xFF;
    std::fs::write(dir.join(files[0].name), &damaged).unwrap();
    assert_eq!(store.status(), Status::Corrupt);
    // A file of the wrong size is corrupt too.
    std::fs::write(dir.join(files[0].name), b"short").unwrap();
    assert_eq!(store.status(), Status::Corrupt);
    assert!(store.verified_paths().is_err());
    store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(store.status(), Status::Ready);
    assert_eq!(
        fetcher.requests(),
        3,
        "two files, then the one that was corrupt"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn nothing_is_requested_when_the_files_are_ready() {
    let dir = temp_dir("ready");
    let (files, fetcher, _) = files();
    let store = ModelStore::with_files(dir.clone(), files);
    store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap();
    let before = fetcher.requests();
    store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fetcher.requests(), before);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_files_are_checked_again_at_load_once_per_run() {
    let dir = temp_dir("load");
    let (files, fetcher, bodies) = files();
    let store = ModelStore::with_files(dir.clone(), files.clone());
    assert!(store.verified_paths().is_err(), "missing files do not load");
    store
        .download(&fetcher, &mut no_progress, &AtomicBool::new(false))
        .unwrap();
    let paths = store.verified_paths().unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|p| p.starts_with(&dir)));
    // The second load of the run does not read the files again: a change after the first
    // check is not seen until the next run (status() always looks).
    let mut damaged = bodies[1].clone();
    damaged[0] ^= 1;
    std::fs::write(dir.join(files[1].name), &damaged).unwrap();
    assert!(store.verified_paths().is_ok());
    assert_eq!(store.status(), Status::Corrupt);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_model_folder_is_under_the_data_folder() {
    let store = atomik_vision::download::hands_store(std::path::Path::new("data"));
    assert_eq!(
        store.dir(),
        std::path::Path::new("data").join("models").join("hands")
    );
}
