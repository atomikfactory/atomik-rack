//! The seam between the app and the hand tracker.
//!
//! [`VisionBackend`] is everything the app asks of the outside world for the webcam controller:
//! the cameras there are, the state of the model files, starting a tracker, the one-time download
//! of the models and deleting them. [`VisionSession`] is a running tracker: the app polls its
//! events once a frame. The real backend ([`SystemVision`]) wraps `atomik_vision`; tests use
//! [`FakeVision`], which needs no camera, no models and no network.
//!
//! Frames live in memory only: nothing here writes a picture anywhere, and a session that is
//! stopped or dropped has released the camera and forgotten its last picture.

use atomik_vision::camera::{Camera, CameraId, SystemCamera};
use atomik_vision::download::{DownloadError, Fetcher, ModelStore, Progress, Status, UreqFetcher};
use atomik_vision::gesture::GestureConfig;
use atomik_vision::models::PipelineConfig;
pub use atomik_vision::tracker::{FrameReport, Preview};
use atomik_vision::tracker::{Tracker, TrackerConfig, TrackerEvent, models_factory};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One camera of the machine, as the settings list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraEntry {
    /// The id the tracker opens (kept in the settings).
    pub id: String,
    /// The name the system gives it.
    pub name: String,
}

/// Whether the hand models are on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    /// Not downloaded.
    Missing,
    /// Every file is there and checked.
    Ready,
    /// A file is there and does not match; it has to be downloaded again.
    Corrupt,
}

/// What a tracker is started with.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionConfig {
    /// The camera to open (`None`: the first one).
    pub camera: Option<String>,
    /// Scales the pinch thresholds: above 1 the fingers may be further apart.
    pub sensitivity: f32,
}

/// What a download tells while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes received over all files.
    pub received: u64,
    /// Bytes expected over all files.
    pub total: u64,
}

/// Why something could not be done, in words for the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VisionError {
    /// No camera was found.
    #[error("No camera was found")]
    NoCamera,
    /// The tracker could not be started.
    #[error("{0}")]
    Failed(String),
    /// The download did not finish.
    #[error("{0}")]
    Download(String),
    /// The models could not be deleted.
    #[error("{0}")]
    Delete(String),
}

/// A running tracker.
pub trait VisionSession: Send {
    /// The events since the last call, oldest first.
    fn poll(&mut self) -> Vec<TrackerEvent>;
    /// The newest preview picture, once.
    fn preview(&mut self) -> Option<Preview>;
    /// Asks for the preview picture (or no longer wants it).
    fn set_preview(&mut self, wanted: bool);
    /// Switches to another camera.
    fn switch_camera(&mut self, id: &str);
    /// Whether the worker is still running (the camera is in use).
    fn running(&self) -> bool;
    /// Stops the tracker and releases the camera.
    fn stop(&mut self);
}

/// What the app asks of the camera, the models and the network.
pub trait VisionBackend: Send + Sync {
    /// The cameras there are now.
    fn cameras(&self) -> Vec<CameraEntry>;
    /// The state of the model files.
    fn models(&self) -> ModelState;
    /// The bytes a full download fetches.
    fn download_bytes(&self) -> u64;
    /// The address the models come from, as text for the consent dialog.
    fn download_source(&self) -> String;
    /// Starts a tracker. Needs the models; the camera is in use from here on.
    fn start(&self, config: &VisionConfig) -> Result<Box<dyn VisionSession>, VisionError>;
    /// Downloads the models (blocking: the app calls it on a worker). `cancel` stops it.
    fn download(
        &self,
        progress: &mut dyn FnMut(DownloadProgress),
        cancel: &AtomicBool,
    ) -> Result<(), VisionError>;
    /// Deletes the downloaded models.
    fn delete_models(&self) -> Result<(), VisionError>;
}

/// A backend with no camera: what a shell that is not the application window has. It lists
/// nothing, says the models are missing and refuses to start or download.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoVision;

impl VisionBackend for NoVision {
    fn cameras(&self) -> Vec<CameraEntry> {
        Vec::new()
    }

    fn models(&self) -> ModelState {
        ModelState::Missing
    }

    fn download_bytes(&self) -> u64 {
        0
    }

    fn download_source(&self) -> String {
        String::new()
    }

    fn start(&self, _config: &VisionConfig) -> Result<Box<dyn VisionSession>, VisionError> {
        Err(VisionError::Failed("The camera is not available".into()))
    }

    fn download(
        &self,
        _progress: &mut dyn FnMut(DownloadProgress),
        _cancel: &AtomicBool,
    ) -> Result<(), VisionError> {
        Err(VisionError::Download("The camera is not available".into()))
    }

    fn delete_models(&self) -> Result<(), VisionError> {
        Ok(())
    }
}

/// The real backend: the system's cameras, the models in the settings folder and the one HTTPS
/// client of the download.
pub struct SystemVision {
    store: ModelStore,
    fetcher: Arc<dyn Fetcher>,
}

impl SystemVision {
    /// The backend that keeps its models under `data_dir` (`models/hands/`).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            store: ModelStore::in_data_dir(data_dir),
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

impl VisionBackend for SystemVision {
    fn cameras(&self) -> Vec<CameraEntry> {
        SystemCamera
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|c| CameraEntry {
                id: c.id.0,
                name: c.name,
            })
            .collect()
    }

    fn models(&self) -> ModelState {
        match self.store.status() {
            Status::Missing => ModelState::Missing,
            Status::Ready => ModelState::Ready,
            Status::Corrupt => ModelState::Corrupt,
        }
    }

    fn download_bytes(&self) -> u64 {
        self.store.total_bytes()
    }

    fn download_source(&self) -> String {
        let mut hosts: Vec<&str> = self
            .store
            .files()
            .iter()
            .filter_map(|f| f.url.strip_prefix("https://"))
            .filter_map(|rest| rest.split('/').next())
            .collect();
        hosts.dedup();
        hosts.join(", ")
    }

    fn start(&self, config: &VisionConfig) -> Result<Box<dyn VisionSession>, VisionError> {
        if self.models() != ModelState::Ready {
            return Err(VisionError::Failed(
                "The hand models are not downloaded".into(),
            ));
        }
        let camera: Arc<dyn Camera> = Arc::new(SystemCamera);
        let id = match &config.camera {
            Some(id) => CameraId(id.clone()),
            None => camera
                .list()
                .map_err(|e| VisionError::Failed(e.to_string()))?
                .into_iter()
                .next()
                .map(|c| c.id)
                .ok_or(VisionError::NoCamera)?,
        };
        let base = GestureConfig::default();
        let scale = config.sensitivity.clamp(0.5, 1.5);
        let cfg = TrackerConfig {
            gesture: GestureConfig {
                pinch_on: base.pinch_on * scale,
                pinch_off: base.pinch_off * scale,
                ..base
            },
            ..TrackerConfig::default()
        };
        let tracker = Tracker::start(
            camera,
            id,
            models_factory(self.store.dir().to_path_buf(), PipelineConfig::default()),
            cfg,
        );
        Ok(Box::new(RealSession {
            events: tracker.events(),
            tracker,
        }))
    }

    fn download(
        &self,
        progress: &mut dyn FnMut(DownloadProgress),
        cancel: &AtomicBool,
    ) -> Result<(), VisionError> {
        self.store
            .download(
                self.fetcher.as_ref(),
                &mut |p: Progress| {
                    progress(DownloadProgress {
                        received: p.received,
                        total: p.total,
                    });
                },
                cancel,
            )
            .map_err(|e| VisionError::Download(describe(&e)))
    }

    fn delete_models(&self) -> Result<(), VisionError> {
        let dir = self.store.dir();
        if !dir.exists() {
            return Ok(());
        }
        for f in self.store.files() {
            let path: PathBuf = dir.join(f.name);
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| VisionError::Delete(e.to_string()))?;
            }
        }
        // The folder goes when nothing else is in it.
        let _ = std::fs::remove_dir(dir);
        Ok(())
    }
}

struct RealSession {
    tracker: Tracker,
    events: atomik_vision::tracker::Events,
}

impl VisionSession for RealSession {
    fn poll(&mut self) -> Vec<TrackerEvent> {
        let mut out = Vec::new();
        while let Some(e) = self.events.try_recv() {
            out.push(e);
        }
        out
    }

    fn preview(&mut self) -> Option<Preview> {
        self.tracker.take_preview()
    }

    fn set_preview(&mut self, wanted: bool) {
        self.tracker.set_preview(wanted);
    }

    fn switch_camera(&mut self, id: &str) {
        self.tracker.switch_camera(CameraId(id.to_owned()));
    }

    fn running(&self) -> bool {
        self.tracker.is_running()
    }

    fn stop(&mut self) {
        self.tracker.stop();
    }
}

/// What a [`FakeVision`] was asked, for tests.
#[derive(Debug, Default)]
pub struct FakeStats {
    /// Trackers started.
    pub started: AtomicUsize,
    /// Trackers stopped (or dropped).
    pub stopped: AtomicUsize,
    /// Requests the fake fetcher served (a download is two files).
    pub fetches: AtomicUsize,
    /// Times the models were deleted.
    pub deleted: AtomicUsize,
    /// The last camera asked for.
    pub camera: Mutex<Option<String>>,
    /// The last sensitivity asked for, as thousandths.
    pub sensitivity: AtomicUsize,
}

/// A backend for tests: scripted events, scripted cameras and models that a download makes
/// ready.
#[derive(Clone)]
pub struct FakeVision {
    /// What it was asked.
    pub stats: Arc<FakeStats>,
    models: Arc<Mutex<ModelState>>,
    cameras: Arc<Mutex<Vec<CameraEntry>>>,
    script: Arc<Mutex<Vec<TrackerEvent>>>,
    preview: Arc<Mutex<Option<Preview>>>,
    fail_start: Arc<Mutex<Option<String>>>,
    fail_download: Arc<Mutex<Option<String>>>,
}

impl FakeVision {
    /// A fake with the models in `models` and one camera.
    pub fn new(models: ModelState) -> Self {
        Self {
            stats: Arc::new(FakeStats::default()),
            models: Arc::new(Mutex::new(models)),
            cameras: Arc::new(Mutex::new(vec![CameraEntry {
                id: "fake-0".into(),
                name: "Fake camera".into(),
            }])),
            script: Arc::new(Mutex::new(Vec::new())),
            preview: Arc::new(Mutex::new(None)),
            fail_start: Arc::new(Mutex::new(None)),
            fail_download: Arc::new(Mutex::new(None)),
        }
    }

    /// Queues events for the running session to hand out at its next poll.
    pub fn push(&self, events: impl IntoIterator<Item = TrackerEvent>) {
        self.script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(events);
    }

    /// Sets the picture the session hands out as its preview.
    pub fn set_preview_picture(&self, preview: Option<Preview>) {
        *self.preview.lock().unwrap_or_else(|e| e.into_inner()) = preview;
    }

    /// Makes the next start fail with `message`.
    pub fn fail_start(&self, message: Option<&str>) {
        *self.fail_start.lock().unwrap_or_else(|e| e.into_inner()) = message.map(str::to_owned);
    }

    /// Makes the next download fail with `message`.
    pub fn fail_download(&self, message: Option<&str>) {
        *self.fail_download.lock().unwrap_or_else(|e| e.into_inner()) = message.map(str::to_owned);
    }

    /// The cameras the fake lists.
    pub fn set_cameras(&self, cameras: Vec<CameraEntry>) {
        *self.cameras.lock().unwrap_or_else(|e| e.into_inner()) = cameras;
    }

    /// How many sessions started.
    pub fn started(&self) -> usize {
        self.stats.started.load(Ordering::SeqCst)
    }

    /// How many sessions stopped.
    pub fn stopped(&self) -> usize {
        self.stats.stopped.load(Ordering::SeqCst)
    }

    /// How many files the fake fetcher served.
    pub fn fetches(&self) -> usize {
        self.stats.fetches.load(Ordering::SeqCst)
    }
}

struct FakeSession {
    stats: Arc<FakeStats>,
    script: Arc<Mutex<Vec<TrackerEvent>>>,
    preview: Arc<Mutex<Option<Preview>>>,
    wanted: bool,
    running: bool,
}

impl VisionSession for FakeSession {
    fn poll(&mut self) -> Vec<TrackerEvent> {
        std::mem::take(&mut *self.script.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn preview(&mut self) -> Option<Preview> {
        if self.wanted {
            self.preview
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            None
        }
    }

    fn set_preview(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    fn switch_camera(&mut self, id: &str) {
        *self.stats.camera.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_owned());
    }

    fn running(&self) -> bool {
        self.running
    }

    fn stop(&mut self) {
        if self.running {
            self.running = false;
            self.stats.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VisionBackend for FakeVision {
    fn cameras(&self) -> Vec<CameraEntry> {
        self.cameras
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn models(&self) -> ModelState {
        *self.models.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn download_bytes(&self) -> u64 {
        7_818_763
    }

    fn download_source(&self) -> String {
        "storage.googleapis.com".to_owned()
    }

    fn start(&self, config: &VisionConfig) -> Result<Box<dyn VisionSession>, VisionError> {
        if let Some(m) = self
            .fail_start
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(VisionError::Failed(m));
        }
        if self.models() != ModelState::Ready {
            return Err(VisionError::Failed("The models are missing".into()));
        }
        self.stats.started.fetch_add(1, Ordering::SeqCst);
        *self.stats.camera.lock().unwrap_or_else(|e| e.into_inner()) = config.camera.clone();
        self.stats.sensitivity.store(
            (config.sensitivity * 1000.0).round() as usize,
            Ordering::SeqCst,
        );
        Ok(Box::new(FakeSession {
            stats: Arc::clone(&self.stats),
            script: Arc::clone(&self.script),
            preview: Arc::clone(&self.preview),
            wanted: false,
            running: true,
        }))
    }

    fn download(
        &self,
        progress: &mut dyn FnMut(DownloadProgress),
        cancel: &AtomicBool,
    ) -> Result<(), VisionError> {
        if let Some(m) = self
            .fail_download
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(VisionError::Download(m));
        }
        let total = self.download_bytes();
        for part in 1..=2u64 {
            if cancel.load(Ordering::SeqCst) {
                return Err(VisionError::Download("The download was cancelled".into()));
            }
            self.stats.fetches.fetch_add(1, Ordering::SeqCst);
            progress(DownloadProgress {
                received: total * part / 2,
                total,
            });
        }
        *self.models.lock().unwrap_or_else(|e| e.into_inner()) = ModelState::Ready;
        Ok(())
    }

    fn delete_models(&self) -> Result<(), VisionError> {
        self.stats.deleted.fetch_add(1, Ordering::SeqCst);
        *self.models.lock().unwrap_or_else(|e| e.into_inner()) = ModelState::Missing;
        Ok(())
    }
}
