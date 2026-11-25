//! The model store through the fake fetcher: no network.

use atomik_download::{DownloadError, FakeFetcher, ModelFile, ModelStore, Progress, sha256_hex};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "atomik-download-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One file of `size` bytes whose server sends `body`.
fn one_file(size: u64, body: Vec<u8>) -> (Vec<ModelFile>, FakeFetcher) {
    let url = "https://example.invalid/model.bin";
    let mut fetcher = FakeFetcher::new();
    fetcher.serve(url, body.clone());
    let sha: &'static str = Box::leak(sha256_hex(&body).into_boxed_str());
    let file = ModelFile {
        name: "model.bin",
        url,
        size,
        sha256: sha,
    };
    (vec![file], fetcher)
}

#[test]
fn the_store_names_its_folder_and_its_files() {
    let dir = temp_dir("names");
    let (files, _) = one_file(4, vec![1, 2, 3, 4]);
    let store = ModelStore::with_files(dir.clone(), files.clone());
    assert_eq!(store.dir(), dir.as_path());
    assert_eq!(store.files().len(), 1);
    assert_eq!(store.files()[0].name, "model.bin");
    assert_eq!(store.total_bytes(), 4);
}

#[test]
fn a_server_that_sends_more_than_the_file_is_stopped_and_leaves_nothing() {
    let dir = temp_dir("oversize");
    std::fs::create_dir_all(&dir).unwrap();
    // The file is 1500 bytes; the server sends 5000 in chunks of 1000.
    let (files, fetcher) = one_file(1500, vec![7u8; 5000]);
    let store = ModelStore::with_files(dir.clone(), files);
    let result = store.download(&fetcher, &mut |_: Progress| {}, &AtomicBool::new(false));
    assert!(
        matches!(
            result,
            Err(DownloadError::WrongSize { ref name, got, expected: 1500 })
                if name == "model.bin" && got > 1500
        ),
        "{result:?}"
    );
    assert!(!dir.join("model.bin").exists());
    assert!(!dir.join("model.bin.part").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancel_during_the_download_stops_it_and_leaves_nothing() {
    let dir = temp_dir("cancel");
    std::fs::create_dir_all(&dir).unwrap();
    let body: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let (files, fetcher) = one_file(body.len() as u64, body);
    let store = ModelStore::with_files(dir.clone(), files);
    let cancel = AtomicBool::new(false);
    let result = store.download(
        &fetcher,
        &mut |_: Progress| cancel.store(true, Ordering::SeqCst),
        &cancel,
    );
    assert_eq!(result, Err(DownloadError::Cancelled));
    assert!(!dir.join("model.bin").exists());
    assert!(!dir.join("model.bin.part").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
