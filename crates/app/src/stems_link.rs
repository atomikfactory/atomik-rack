//! The seam between the app and stem separation: one trait, [`StemBackend`], for the state of the
//! model files, their one-time download and the separation itself.
//!
//! * [`SystemStems`] is the real one (the weights in the settings folder, the HTTPS client of the
//!   download, the network of `atomik-stems`); only the application window installs it.
//! * [`NoStems`] is what a core without a window has: no models, nothing to download.
//! * `FakeStems` (tests only) separates a signal into scaled copies of itself, can hold until
//!   cancelled and serves a download from memory, so no test needs the network or a model.

use atomik_download::{DownloadError, Fetcher, ModelStore, Progress, Status, UreqFetcher};
use atomik_stems::{Models, Stem, StemError, StemSink};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The state of the model files.
pub type ModelState = Status;

/// Everything the app needs from the separation, behind one seam.
pub trait StemBackend: Send + Sync {
    /// Whether the model files are there and intact.
    fn models(&self) -> ModelState;
    /// The bytes a full download fetches.
    fn download_bytes(&self) -> u64;
    /// Where the files come from (a host name).
    fn download_source(&self) -> String;
    /// Downloads the model files (blocking: the app calls it on a worker). `progress` gets the
    /// bytes received and expected; setting `cancel` stops it and leaves nothing behind.
    fn download(
        &self,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), String>;
    /// The sample rate the audio must have when it is given to [`StemBackend::separate`].
    fn rate(&self) -> u32;
    /// Separates a stereo song (at [`StemBackend::rate`]) and gives the wanted stems to `sink`.
    fn separate(
        &self,
        left: &[f32],
        right: &[f32],
        wanted: &[Stem],
        sink: &mut dyn StemSink,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<(), StemError>;
}

/// The backend of a core without a window: nothing is available.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStems;

impl StemBackend for NoStems {
    fn models(&self) -> ModelState {
        Status::Missing
    }
    fn download_bytes(&self) -> u64 {
        0
    }
    fn download_source(&self) -> String {
        String::new()
    }
    fn download(&self, _: &mut dyn FnMut(u64, u64), _: &AtomicBool) -> Result<(), String> {
        Err("Stem separation is not available here".into())
    }
    fn rate(&self) -> u32 {
        atomik_stems::RATE
    }
    fn separate(
        &self,
        _: &[f32],
        _: &[f32],
        _: &[Stem],
        _: &mut dyn StemSink,
        _: &AtomicBool,
        _: &mut dyn FnMut(f32),
    ) -> Result<(), StemError> {
        Err(StemError::Weights(
            "stem separation is not available here".into(),
        ))
    }
}

/// The real backend: the models in the settings folder and the one HTTPS client of the download.
pub struct SystemStems {
    store: ModelStore,
    fetcher: Arc<dyn Fetcher>,
}

impl SystemStems {
    /// The backend that keeps its models under `data_dir` (`models/stems/`).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            store: atomik_stems::files::stems_store(data_dir),
            fetcher: Arc::new(UreqFetcher),
        }
    }
}

fn describe(e: &DownloadError) -> String {
    match e {
        DownloadError::Cancelled => "The download was cancelled".to_owned(),
        other => other.to_string(),
    }
}

impl StemBackend for SystemStems {
    fn models(&self) -> ModelState {
        self.store.status()
    }

    fn download_bytes(&self) -> u64 {
        self.store.total_bytes()
    }

    fn download_source(&self) -> String {
        atomik_stems::files::MODEL_FILES
            .first()
            .and_then(|f| f.url.strip_prefix("https://"))
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default()
            .to_owned()
    }

    fn download(
        &self,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.store
            .download(
                self.fetcher.as_ref(),
                &mut |p: Progress| progress(p.received, p.total),
                cancel,
            )
            .map_err(|e| describe(&e))
    }

    fn rate(&self) -> u32 {
        atomik_stems::RATE
    }

    fn separate(
        &self,
        left: &[f32],
        right: &[f32],
        wanted: &[Stem],
        sink: &mut dyn StemSink,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<(), StemError> {
        // The files are checked against their pinned SHA-256 before they are read.
        self.store
            .verified_paths()
            .map_err(|e| StemError::Weights(e.to_string()))?;
        let models = Models::load(self.store.dir())?;
        if models.rate() != atomik_stems::RATE {
            return Err(StemError::Weights("the models run at another rate".into()));
        }
        atomik_stems::separate(&models, left, right, wanted, sink, cancel, progress)
    }
}

#[cfg(test)]
pub(crate) use fake::FakeStems;

#[cfg(test)]
mod fake {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A backend for tests. The models are missing until a download makes them ready; a
    /// separation gives each stem as a copy of the input times `0.1 * (index + 1)`, in a few
    /// pieces; it can hold (until cancelled or released) so a test can cancel a run.
    #[derive(Clone)]
    pub(crate) struct FakeStems {
        inner: Arc<Inner>,
    }

    struct Inner {
        ready: AtomicBool,
        hold: AtomicBool,
        fail_download: Mutex<Option<String>>,
        downloads: AtomicUsize,
        separations: AtomicUsize,
        rate: u32,
    }

    impl FakeStems {
        /// A fake whose models are `ready` (or missing), running at `rate`.
        pub(crate) fn new(ready: bool, rate: u32) -> Self {
            Self {
                inner: Arc::new(Inner {
                    ready: AtomicBool::new(ready),
                    hold: AtomicBool::new(false),
                    fail_download: Mutex::new(None),
                    downloads: AtomicUsize::new(0),
                    separations: AtomicUsize::new(0),
                    rate,
                }),
            }
        }
        /// Makes a separation wait until it is cancelled or [`Self::release`] is called.
        pub(crate) fn hold(&self) {
            self.inner.hold.store(true, Ordering::SeqCst);
        }
        /// Lets a held separation go on.
        pub(crate) fn release(&self) {
            self.inner.hold.store(false, Ordering::SeqCst);
        }
        /// Makes the next download fail with `message`.
        pub(crate) fn fail_download(&self, message: &str) {
            *self.inner.fail_download.lock().unwrap() = Some(message.to_owned());
        }
        /// How many downloads started.
        pub(crate) fn downloads(&self) -> usize {
            self.inner.downloads.load(Ordering::SeqCst)
        }
        /// How many separations started.
        pub(crate) fn separations(&self) -> usize {
            self.inner.separations.load(Ordering::SeqCst)
        }
    }

    impl StemBackend for FakeStems {
        fn models(&self) -> ModelState {
            if self.inner.ready.load(Ordering::SeqCst) {
                Status::Ready
            } else {
                Status::Missing
            }
        }
        fn download_bytes(&self) -> u64 {
            142_551_184
        }
        fn download_source(&self) -> String {
            "models.example".into()
        }
        fn download(
            &self,
            progress: &mut dyn FnMut(u64, u64),
            cancel: &AtomicBool,
        ) -> Result<(), String> {
            self.inner.downloads.fetch_add(1, Ordering::SeqCst);
            if let Some(m) = self.inner.fail_download.lock().unwrap().take() {
                return Err(m);
            }
            let total = self.download_bytes();
            for i in 1..=4u64 {
                if cancel.load(Ordering::SeqCst) {
                    return Err("The download was cancelled".into());
                }
                progress(total / 4 * i, total);
            }
            self.inner.ready.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn rate(&self) -> u32 {
            self.inner.rate
        }
        fn separate(
            &self,
            left: &[f32],
            right: &[f32],
            wanted: &[Stem],
            sink: &mut dyn StemSink,
            cancel: &AtomicBool,
            progress: &mut dyn FnMut(f32),
        ) -> Result<(), StemError> {
            self.inner.separations.fetch_add(1, Ordering::SeqCst);
            while self.inner.hold.load(Ordering::SeqCst) {
                if cancel.load(Ordering::SeqCst) {
                    return Err(StemError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let len = left.len().min(right.len());
            let pieces = 4;
            for p in 0..pieces {
                if cancel.load(Ordering::SeqCst) {
                    return Err(StemError::Cancelled);
                }
                let (a, b) = (len * p / pieces, len * (p + 1) / pieces);
                for stem in wanted {
                    let g = 0.1 * (stem.index() as f32 + 1.0);
                    let l: Vec<f32> = left[a..b].iter().map(|v| v * g).collect();
                    let r: Vec<f32> = right[a..b].iter().map(|v| v * g).collect();
                    sink.write(*stem, &l, &r)?;
                }
                progress((p + 1) as f32 / pieces as f32);
            }
            Ok(())
        }
    }
}
