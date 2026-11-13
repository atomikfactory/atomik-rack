//! The hand models are not part of the installer: the user downloads them once, after a dialog
//! that says what is fetched, from where and how big. The store, the [`Fetcher`] trait and the
//! download are in the shared `atomik-download` crate (re-exported here); this module holds the
//! list of the hand model files.
//!
//! The addresses, sizes and SHA-256 hashes are constants in this file ([`MODEL_FILES`]) and
//! nowhere else.

pub use atomik_download::{
    CONNECT_TIMEOUT, DownloadError, FILE_TIMEOUT, FakeFetcher, FetchError, Fetcher, ModelFile,
    ModelStore, Progress, Status, StoreError, UreqFetcher, sha256_hex,
};
use std::path::Path;

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

/// The store of the hand models in the app's data folder: `<data>/models/hands/`.
pub fn hands_store(data_dir: &Path) -> ModelStore {
    ModelStore::in_data_dir(data_dir, "hands", MODEL_FILES)
}
