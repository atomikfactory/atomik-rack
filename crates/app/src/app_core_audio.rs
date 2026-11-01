//! Audio in the app core: project bundles, import, peaks and the engine assets.
//!
//! # Project bundle rules
//!
//! The layout rules live in [`atomik_render::bundle`] (shared with the bounce): a project
//! with audio is a folder `<name>/` holding `<name>.atomik` and `Audio/`.
//!
//! * **Save As** with audio: the picked path `D/name.atomik` becomes `D/name/name.atomik`
//!   (the bundle folder is made next to the picked path), unless the picked file already
//!   sits in a folder named `name`, which then is the bundle. Without audio the project is
//!   one plain `.atomik` file at the picked path, exactly as before audio existed.
//! * **Save** never moves the file. Its bundle is [`atomik_render::bundle::existing_bundle_root`]
//!   or, when there is none, the sibling folder `D/name/` (the layout of the repository
//!   examples).
//! * **Open** accepts an `.atomik` file or a bundle folder (the project file inside it).
//! * **Session scratch bundle:** before the project has a bundle, imports go to a temporary
//!   bundle under the OS temp dir (one folder per [`AppCore`], removed when it is dropped
//!   or the project is replaced). The first save copies the assets still in use into the
//!   destination bundle and switches to it.
//! * **Save** writes the pruned document (`RackDoc::prune_unreferenced_assets`: only assets
//!   the song uses) with `save_text_atomic`, then deletes the files of assets that neither
//!   the saved song nor the undo/redo history ([`atomik_model::Document::referenced_assets`])
//!   reference. Assets only the history references stay on disk (and in the live pool).
//! * **Open** verifies every pool asset: missing files are listed
//!   ([`AppCore::missing_assets`], and a toast); those clips play silence. They can be relinked
//!   (see below).
//!
//! # Relink
//!
//! A missing asset is fixed with [`AppCore::relink_asset`] (any audio file the user picks for
//! it) or [`AppCore::relink_search_folder`] (a bounded folder scan that matches the missing
//! assets by original file name, see [`crate::audio_relink`]). The file goes through the import
//! worker (decode, resample, store, peaks). If its content hash **is** the missing hash
//! (the file was only moved or renamed) the bundle simply has its file back: the clips load,
//! the document does not change and there is no undo step. Otherwise the new asset is registered
//! (`RegisterAsset`, under the new file's name) and **one** `RelinkAsset` step swaps every clip
//! and zone reference from the old hash to the new one (clips, samplers, slicers, drum pads); undo
//! brings the old, missing state back (and the missing list with it). The old pool entry stays
//! until the history no longer needs it. A file that does not decode, or that is too short for the
//! zones that used the old one, is reported by name and changes nothing. Each finished batch
//! reports `relinked N, M still missing` as a toast.
//!
//! # Import
//!
//! [`AppCore::import_audio`] starts a worker thread per batch (decode, resample to the
//! session rate, store, read back, peaks); the UI thread keeps running and applies each
//! finished file in [`AppCore::poll_background`] (called every frame): `RegisterAsset`, then
//! for `NewTracks` `AddAudioTrack` and `AddAudioClip`, for `ExistingTrack` `AddAudioClip` at
//! the running tick. **Undo steps:** `RegisterAsset` is not a step; an existing-track import
//! is one step per file, a new-track import two (the track, then its clip), because the
//! model has no command that does both. A failing file is reported by name and the others
//! still import.
//!
//! # Engine assets
//!
//! The assets the engine plays live in a cache keyed by hash ([`AppCore::song_assets`]).
//! The loading workers decide how an asset reaches the engine ([`crate::audio_import::load_asset`]):
//! up to [`AppCore::stream_threshold`] bytes of decoded f32 audio (64 MiB, see
//! [`crate::stream_source`]) it is loaded into RAM (`SongAsset::Memory`); above it the engine
//! streams it from the bundle's WAV file (`SongAsset::Stream`), with at least 1 s of
//! read-ahead. Imports and recordings follow the same rule. Bounces never stream.

use super::rec::AudioRecording;
use super::{AppCore, AppError};
use crate::audio_import::{
    Loaded, Msg, display_name, spawn_adopt, spawn_import, spawn_load, ticks_for_frames,
};
use crate::sink::EngineSink;
use crate::stream_source::STREAM_THRESHOLD_BYTES;
use atomik_audio_file::{AudioFileError, Bundle, PeakPyramid, save_text_atomic};
use atomik_device_sdk::SampleBuffer;
use atomik_engine::SongAsset;
use atomik_model::{AssetMeta, Command, DeviceKey, SongCommand};
use atomik_render::bundle::{
    MissingAsset, existing_bundle_root, missing_assets, save_as_layout, sibling_bundle_root,
};
use atomik_ui::ImportTarget;
use atomik_ui::timeline::{Meter, snap_round};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// The bundle the project's audio lives in.
pub(super) struct ProjectBundle {
    pub(super) bundle: Bundle,
    /// A temporary session bundle (not the project's own folder yet).
    pub(super) scratch: bool,
}

/// What a sample import (a drop on a sampler, or its Load button) collects until the batch is
/// done: the files that imported, mapped into one `SetZones` step.
pub(super) struct SampleImport {
    pub(super) device: DeviceKey,
    pub(super) at_key: Option<u8>,
    /// Files a loop slicer left out (it takes one loop, the first).
    pub(super) ignored: Vec<String>,
    /// `(asset id, original file name, frames)` in the order the files finished.
    pub(super) files: Vec<(String, String, u64)>,
}

/// One running import (a drop or a dialog).
struct Batch {
    samples: Option<SampleImport>,
    target: ImportTarget,
    tick: i64,
    /// Where the next clip of an existing-track import starts.
    cursor: i64,
    total: usize,
    done: usize,
    errors: Vec<String>,
    /// Files that imported but had something repaired on the way (non-finite samples).
    notes: Vec<String>,
    /// Set when the files relink missing assets instead of adding clips.
    relink: Option<relink::RelinkJob>,
    /// Track names that replace the file name (recovered recordings), by file.
    names: BTreeMap<PathBuf, String>,
}

/// Everything audio of an open project.
pub(super) struct AudioState {
    bundle: Option<ProjectBundle>,
    pub(super) assets: BTreeMap<String, SongAsset>,
    /// The rendered buffers of warped clips by job key, and the workers making more
    /// ([`super::warp`]).
    pub(super) warps: atomik_render::warp::WarpStore,
    pub(super) warp_work: super::warp::WarpWork,
    /// The freeze and bounce renders ([`super::freeze`]).
    pub(super) freeze: super::freeze::FreezeState,
    /// The audio export that runs ([`super::export`]).
    pub(super) export: super::export::ExportState,
    /// The Audio to Notes extraction that is running ([`super::notes`]).
    pub(super) notes: super::notes::NotesState,
    /// The transients of the audio clip editor ([`super::clip_editor`]).
    pub(super) onsets: super::clip_editor::OnsetState,
    /// The speaker tool's audition ([`super::audition`]).
    pub(super) audition: super::audition::AuditionState,
    pub(super) peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// Whole-asset RAM buffers for samplers: shared with the memory assets of
    /// audio tracks where those exist, loaded by a worker for streamed ones. Keeping the
    /// `Arc`s is what lets a sampler voice survive a zone edit (same buffer, same pointer).
    pub(super) buffers: BTreeMap<String, Arc<SampleBuffer>>,
    /// Assets a worker is loading into RAM for samplers.
    pub(super) ram_pending: BTreeSet<String>,
    /// The sample rate a conversion worker is resampling the project's audio to.
    pub(super) converting: Option<u32>,
    pub(super) missing: Vec<MissingAsset>,
    /// Assets that were missing and have been relinked away: they are missing again when an
    /// undo makes the document use them (see `AppCore::refresh_missing`).
    relinked: Vec<MissingAsset>,
    pub(super) generation: u64,
    next_batch: u64,
    /// Workers that have not sent their completion yet.
    pub(super) pending: usize,
    batches: BTreeMap<u64, Batch>,
    /// The running audio take, if any ([`super::rec`]).
    pub(super) recording: Option<AudioRecording>,
    /// Audio take writers that have not delivered their result yet.
    pub(super) rec_pending: usize,
    /// Writers of cancelled takes, still deleting their partial files.
    pub(super) cancelled: Vec<std::thread::JoinHandle<()>>,
    pub(super) next_take: u64,
    /// `internal.monitor_live` as the engine has it, per audio track.
    pub(super) monitor_sent: BTreeMap<DeviceKey, bool>,
    pub(super) tx: Sender<Msg>,
    pub(super) rx: Receiver<Msg>,
    scratch_root: PathBuf,
    scratch_count: u32,
    /// The name of this core's folder in the scratch folder (`atomik-rack-session-<pid>-<n>`).
    session_name: String,
    /// The folder the session scratch bundle is made in (`None`: the OS temporary folder). A
    /// change counts from the next import that needs a scratch bundle.
    pub(super) scratch_base: Option<PathBuf>,
    /// The folder of a template or an item whose audio the next opened document copies into its
    /// session bundle (see [`AppCore::open_template`]).
    pub(super) adopt_from: Option<PathBuf>,
    /// Unfinished recordings of a dead session already taken up this run.
    recovered: BTreeSet<PathBuf>,
    /// The `rec-*.wav` files whose audio is now in the project, deleted once it is saved.
    recovered_files: Vec<PathBuf>,
}

/// How long a recording in the shared scratch folder must have been left alone before it is
/// taken for an orphan (a live recorder refreshes its file about once a second).
const ORPHAN_QUIET: Duration = Duration::from_secs(60);

impl AudioState {
    pub(super) fn new() -> Self {
        static SESSIONS: AtomicU64 = AtomicU64::new(0);
        let (tx, rx) = channel();
        let session = SESSIONS.fetch_add(1, Ordering::Relaxed);
        Self {
            bundle: None,
            assets: BTreeMap::new(),
            warps: atomik_render::warp::WarpStore::new(),
            warp_work: super::warp::WarpWork::default(),
            freeze: super::freeze::FreezeState::default(),
            export: super::export::ExportState::default(),
            notes: super::notes::NotesState::default(),
            onsets: super::clip_editor::OnsetState::default(),
            audition: super::audition::AuditionState::default(),
            peaks: BTreeMap::new(),
            buffers: BTreeMap::new(),
            ram_pending: BTreeSet::new(),
            converting: None,
            missing: Vec::new(),
            relinked: Vec::new(),
            generation: 0,
            next_batch: 0,
            pending: 0,
            batches: BTreeMap::new(),
            recording: None,
            rec_pending: 0,
            cancelled: Vec::new(),
            next_take: 0,
            monitor_sent: BTreeMap::new(),
            tx,
            rx,
            scratch_root: std::env::temp_dir().join(format!(
                "atomik-rack-session-{}-{session}",
                std::process::id()
            )),
            scratch_count: 0,
            session_name: format!("atomik-rack-session-{}-{session}", std::process::id()),
            scratch_base: None,
            adopt_from: None,
            recovered: BTreeSet::new(),
            recovered_files: Vec::new(),
        }
    }

    /// The bundle the project's audio lives in, the session scratch bundle before the first save.
    pub(super) fn bundle_ref(&self) -> Option<&Bundle> {
        self.bundle.as_ref().map(|b| &b.bundle)
    }

    /// The asset cache the engine song is built from.
    pub(super) fn assets(&self) -> &BTreeMap<String, SongAsset> {
        &self.assets
    }

    /// Removes the session scratch folders (best effort).
    pub(super) fn remove_scratch(&self) {
        let _ = std::fs::remove_dir_all(&self.scratch_root);
    }
}

fn io_error(path: &Path, e: AudioFileError) -> AppError {
    match e {
        AudioFileError::Io(source) => AppError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => AppError::Audio(other),
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// The pool assets of the open project whose files are not in its bundle (their clips
    /// play silence).
    pub fn missing_assets(&self) -> &[MissingAsset] {
        &self.audio.missing
    }

    /// The waveform peaks fed to the arrangement view, by asset hash.
    pub fn peaks(&self) -> &BTreeMap<String, Arc<PeakPyramid>> {
        &self.audio.peaks
    }

    /// The engine assets loaded so far, by asset hash (the cache `submit_song` reads).
    pub fn song_assets(&self) -> &BTreeMap<String, SongAsset> {
        &self.audio.assets
    }

    /// The project bundle (or the session scratch bundle), for workers.
    pub(super) fn bundle_clone(&self) -> Option<Bundle> {
        self.audio.bundle.as_ref().map(|b| b.bundle.clone())
    }

    /// The folder chosen for session scratch bundles (the Workflow preference), if any.
    pub(super) fn audio_scratch_base(&self) -> Option<PathBuf> {
        self.audio.scratch_base.clone()
    }

    /// The folder of the project bundle (the session scratch folder before the first save),
    /// if the project has one.
    pub fn bundle_root(&self) -> Option<&Path> {
        self.audio.bundle.as_ref().map(|b| b.bundle.root())
    }

    /// True while an import or an asset load runs, or the file browser has work in flight.
    pub fn is_busy(&self) -> bool {
        self.audio.pending > 0 || self.browser_busy()
    }

    /// A status-bar text while audio is being imported or loaded.
    pub fn import_progress(&self) -> Option<String> {
        let (done, total) = self
            .audio
            .batches
            .values()
            .fold((0, 0), |(d, t), b| (d + b.done, t + b.total));
        if total > 0 {
            Some(format!("importing audio {done}/{total}"))
        } else if self.audio.pending > 0 {
            Some("loading audio".to_owned())
        } else {
            None
        }
    }

    /// Where File > Import Audio puts files: the selected audio track (rack selection, else
    /// the track of the first selected arrangement clip), else new tracks; at the playhead
    /// snapped to the arrangement's grid.
    pub fn import_dialog_target(&self) -> (ImportTarget, i64) {
        let doc = self.doc();
        let selected = self.view.selected().cloned().or_else(|| {
            let id = *self.arrangement.selected_clips().first()?;
            Some(doc.song.clip(id)?.0.device.clone())
        });
        let target = match selected {
            Some(key) if doc.is_audio_track(&key, &self.registry) => {
                ImportTarget::ExistingTrack(key)
            }
            _ => ImportTarget::NewTracks,
        };
        (target, self.snapped_playhead())
    }

    /// The playhead snapped to the arrangement grid (never negative).
    pub fn snapped_playhead(&self) -> i64 {
        let step = self.arrangement.snap().step(Meter::of(&self.doc().song));
        snap_round(self.position, step).max(0)
    }

    // ------------------------------------------------------------------ the engine's assets

    /// Decoded bytes above which an asset streams from disk: the sink's override (the hidden
    /// `stream_threshold_bytes` setting) or [`STREAM_THRESHOLD_BYTES`].
    pub fn stream_threshold(&self) -> u64 {
        self.sink
            .stream_threshold_bytes()
            .unwrap_or(STREAM_THRESHOLD_BYTES)
    }

    pub(super) fn insert_loaded(&mut self, loaded: &Loaded) {
        self.audio
            .assets
            .insert(loaded.hash.clone(), loaded.asset.clone());
        // The file is in the bundle after all (a re-import of a missing asset).
        self.audio.missing.retain(|m| m.hash != loaded.hash);
        self.arrangement
            .set_peaks(&loaded.hash, Arc::clone(&loaded.peaks));
        self.sample_editor
            .set_peaks(&loaded.hash, Arc::clone(&loaded.peaks));
        self.slice_editor
            .set_peaks(&loaded.hash, Arc::clone(&loaded.peaks));
        self.audio_editor
            .set_peaks(&loaded.hash, Arc::clone(&loaded.peaks));
        // The waveform of a granular sampler's panel.
        self.view
            .set_sample_peaks(&loaded.hash, Arc::clone(&loaded.peaks));
        // Assets in RAM share their samples with the views: below the finest peak level the
        // waveform reads them. Streamed assets have none and keep the finest peak level.
        if let SongAsset::Memory { data, .. } = &loaded.asset {
            self.arrangement.set_samples(&loaded.hash, Arc::clone(data));
            self.sample_editor
                .set_samples(&loaded.hash, Arc::clone(data));
            self.slice_editor
                .set_samples(&loaded.hash, Arc::clone(data));
            self.audio_editor
                .set_samples(&loaded.hash, Arc::clone(data));
        }
        self.audio
            .peaks
            .insert(loaded.hash.clone(), Arc::clone(&loaded.peaks));
    }

    fn forget_asset(&mut self, hash: &str) {
        self.audio.assets.remove(hash);
        self.audio.buffers.remove(hash);
        self.audio.peaks.remove(hash);
        self.arrangement.clear_peaks(hash);
        self.sample_editor.clear_peaks(hash);
        self.slice_editor.clear_peaks(hash);
        self.audio_editor.clear_peaks(hash);
        self.view.clear_sample_peaks(hash);
    }

    // ------------------------------------------------------------------ background work

    /// Applies what the workers finished since the last call (call it every frame). True when
    /// something changed.
    pub fn poll_background(&mut self) -> bool {
        self.drain(false)
    }

    /// Blocks until every worker is done and applied (tests, scripts).
    pub fn wait_background(&mut self) {
        self.drain(true);
        for writer in self.audio.cancelled.drain(..) {
            let _ = writer.join();
        }
        // Plugin instances are created one per frame: wait for them too.
        self.wait_plugins();
    }

    fn drain(&mut self, wait: bool) -> bool {
        let (mut changed, mut resubmit) = (false, false);
        // Assets whose audio arrived: the samplers that use them get new maps.
        let mut fresh: BTreeSet<String> = BTreeSet::new();
        loop {
            let msg = match self.audio.rx.try_recv() {
                Ok(m) => m,
                Err(_) if wait && self.audio.pending + self.audio.rec_pending > 0 => {
                    match self.audio.rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(m) => m,
                        Err(_) => continue,
                    }
                }
                Err(_) => break,
            };
            changed = true;
            let current = self.audio.generation;
            match msg {
                Msg::BatchDone { generation } => {
                    self.audio.pending = self.audio.pending.saturating_sub(1);
                    if generation == current {
                        self.finish_batches();
                    }
                }
                Msg::LoadDone => {
                    self.audio.pending = self.audio.pending.saturating_sub(1);
                }
                Msg::Archived { job, result } => self.apply_archived(job, *result),
                Msg::Recorded { generation, result } => {
                    self.audio.rec_pending = self.audio.rec_pending.saturating_sub(1);
                    if generation == current {
                        self.apply_recorded(result);
                    }
                }
                Msg::Imported {
                    generation,
                    batch,
                    path,
                    result,
                } if generation == current => {
                    // A sample import sends the maps itself, through its one `SetZones`.
                    // A relink applies its own `RelinkAsset`, which sends them too.
                    let sampler_batch = self
                        .audio
                        .batches
                        .get(&batch)
                        .is_some_and(|b| b.samples.is_some() || b.relink.is_some());
                    if let (Ok(loaded), false) = (&result, sampler_batch) {
                        fresh.insert(loaded.hash.clone());
                    }
                    self.apply_imported(batch, &path, result);
                }
                Msg::Sampled {
                    generation,
                    hash,
                    result,
                } if generation == current => {
                    self.audio.ram_pending.remove(&hash);
                    match result {
                        Ok(buffer) => {
                            self.audio.buffers.insert(hash.clone(), buffer);
                            fresh.insert(hash);
                        }
                        Err(e) => {
                            let name = self.asset_name(&hash);
                            self.reject(format!("sampler audio {name}: {e}"));
                        }
                    }
                }
                Msg::Edited {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_edited(job, result),
                Msg::Sliced {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_sliced(job, result),
                Msg::Warped {
                    generation,
                    key,
                    result,
                } if generation == current => {
                    if self.apply_warped(key, result) {
                        resubmit = true;
                    }
                }
                Msg::Frozen {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_frozen(job, *result),
                Msg::Exported {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_exported(job, *result),
                Msg::Noted {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_noted(job, result),
                Msg::Onsets {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_onsets(job, result),
                Msg::Scanned {
                    generation,
                    root,
                    found,
                    truncated,
                } if generation == current => {
                    self.apply_scanned(&root, found, truncated);
                }
                Msg::Converted {
                    generation,
                    rate,
                    result,
                } if generation == current => {
                    self.apply_converted(rate, result);
                }
                Msg::Loaded {
                    generation,
                    hash,
                    result,
                } if generation == current => match result {
                    Ok(loaded) => {
                        self.insert_loaded(&loaded);
                        fresh.insert(loaded.hash.clone());
                        resubmit = true;
                    }
                    Err(e) => {
                        let name = self.asset_name(&hash);
                        if !self.audio.missing.iter().any(|m| m.hash == hash) {
                            self.audio.missing.push(MissingAsset {
                                hash,
                                name: name.clone(),
                            });
                        }
                        self.reject(format!("audio {name}: {e}"));
                    }
                },
                // A message of a document that was closed meanwhile.
                Msg::Imported { .. }
                | Msg::Loaded { .. }
                | Msg::Sampled { .. }
                | Msg::Sliced { .. }
                | Msg::Warped { .. }
                | Msg::Frozen { .. }
                | Msg::Exported { .. }
                | Msg::Noted { .. }
                | Msg::Onsets { .. }
                | Msg::Scanned { .. }
                | Msg::Edited { .. }
                | Msg::Converted { .. } => {}
            }
        }
        if resubmit {
            self.submit_song(false);
        }
        if !fresh.is_empty() {
            self.send_samples_using(&fresh);
        }
        changed
    }

    /// The pool name of an asset (its original file name), else its id.
    pub(super) fn asset_name(&self, hash: &str) -> String {
        self.doc()
            .assets
            .get(hash)
            .map_or_else(|| hash.to_owned(), |m| m.original_name.clone())
    }

    fn finish_batches(&mut self) {
        // One worker per batch and workers finish in start order per generation: a batch is
        // complete once all its files were applied.
        let finished: Vec<u64> = self
            .audio
            .batches
            .iter()
            .filter(|(_, b)| b.done >= b.total)
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            let Some(batch) = self.audio.batches.remove(&id) else {
                continue;
            };
            let note = batch
                .samples
                .and_then(|samples| self.finish_sample_import(samples));
            let mut report = Vec::new();
            if let Some(job) = &batch.relink {
                report.push(self.relink_summary(job));
            }
            if !batch.errors.is_empty() {
                let what = if batch.relink.is_some() {
                    "audio relink"
                } else {
                    "audio import"
                };
                report.push(format!("{what}: {}", batch.errors.join("; ")));
            }
            report.extend(note);
            if !batch.notes.is_empty() {
                report.push(format!("audio import: {}", batch.notes.join("; ")));
            }
            if !report.is_empty() {
                self.reject(report.join(" \u{b7} "));
            }
        }
    }

    // ------------------------------------------------------------------ import

    pub(super) fn import_bundle(&mut self) -> Result<Bundle, String> {
        if self.audio.bundle.is_none() {
            self.audio.scratch_count += 1;
            // The scratch folder of the preferences, else (or when it cannot be used) the OS
            // temporary folder. A folder left behind in another place is removed with the
            // bundle it held, so the root that is made now is the one that goes.
            let temporary = std::env::temp_dir().join(&self.audio.session_name);
            let chosen = self
                .audio
                .scratch_base
                .as_ref()
                .map(|base| base.join(&self.audio.session_name));
            let mut made = None;
            if let Some(root) = chosen {
                let dir = root.join(self.audio.scratch_count.to_string());
                match Bundle::create(&dir) {
                    Ok(bundle) => made = Some((root, bundle)),
                    Err(e) => {
                        log::warn!(
                            "the scratch folder cannot be used ({e}); using the temporary folder"
                        );
                        self.reject(format!(
                            "The scratch folder cannot be used ({e}); the temporary folder is used"
                        ));
                    }
                }
            }
            let (root, bundle) = match made {
                Some(m) => m,
                None => {
                    let dir = temporary.join(self.audio.scratch_count.to_string());
                    let bundle =
                        Bundle::create(&dir).map_err(|e| format!("session bundle: {e}"))?;
                    (temporary, bundle)
                }
            };
            self.audio.scratch_root = root;
            self.audio.bundle = Some(ProjectBundle {
                bundle,
                scratch: true,
            });
        }
        self.audio
            .bundle
            .as_ref()
            .map(|b| b.bundle.clone())
            .ok_or_else(|| "no bundle".to_owned())
    }

    /// Imports audio files (see the module docs): decoding runs on a worker thread, the clips
    /// appear as the files finish ([`Self::poll_background`]).
    pub fn import_audio(&mut self, paths: Vec<PathBuf>, target: ImportTarget, tick: i64) {
        self.start_import(paths, target, tick, None);
    }

    /// Imports sample files for a sampler: the same worker, scratch bundle, asset
    /// cache and peaks as [`Self::import_audio`]; when every file is done, **one** `SetZones`
    /// step adds the zones: a sampler maps them with `auto_map` (`at_key` as the base key), a drum machine
    /// puts them on the pad of `at_key` or fills its empty pads (see `app_core_drums.rs`). `RegisterAsset` is not a
    /// step, so undo removes all the new zones at once. A failing file is reported by name and
    /// the others still import.
    pub fn import_samples(&mut self, paths: Vec<PathBuf>, device: DeviceKey, at_key: Option<u8>) {
        let takes = self
            .doc()
            .resolve(&device)
            .is_some_and(|d| self.registry.takes_samples(&d.type_id));
        if !takes {
            return self.reject(format!("{device} does not take samples"));
        }
        let mut paths = paths;
        let mut ignored = Vec::new();
        // A loop slicer takes one loop and a granular sampler one sample.
        if (self.is_loop_slicer(&device) || self.is_granular(&device)) && paths.len() > 1 {
            ignored = paths.split_off(1).iter().map(|p| display_name(p)).collect();
        }
        let samples = SampleImport {
            device,
            at_key,
            ignored,
            files: Vec::new(),
        };
        self.start_import(paths, ImportTarget::NewTracks, 0, Some(samples));
    }

    /// Adds the zones of a finished sample import in one undo step (a granular sampler's sample
    /// replaces the old one). Returns a note for the status message when files found no pad (a
    /// drum machine) or were left out (a granular sampler takes one).
    pub(super) fn finish_sample_import(&mut self, import: SampleImport) -> Option<String> {
        if import.files.is_empty() {
            return None;
        }
        let Some(device) = self.doc().resolve(&import.device) else {
            self.reject(format!("{} was removed; no zones added", import.device));
            return None;
        };
        // A loop slicer replaces its loop: detection runs on a worker, then the beats and the
        // slices are applied (see `app_core_slicer.rs`).
        if device.type_id == super::slicer::LOOP_SLICER {
            let name = device.name.clone();
            let device = import.device.clone();
            self.start_loop_slicing(device, &import.files[0]);
            return (!import.ignored.is_empty()).then(|| {
                format!(
                    "{name}: a loop slicer takes one loop; ignored {} file(s): {}",
                    import.ignored.len(),
                    import.ignored.join(", ")
                )
            });
        }
        let (zones, dropped) =
            super::drums::zones_after_import(device, &import.files, import.at_key);
        let name = device.name.clone();
        let granular = device.type_id == super::samples::GRANULAR;
        // A rejection is already reported.
        let _ = self.apply_command(Command::SetZones {
            device: import.device,
            zones,
            merge: None,
        });
        if granular {
            return (!import.ignored.is_empty()).then(|| {
                format!(
                    "{name}: a granular sampler takes one sample; ignored {} file(s): {}",
                    import.ignored.len(),
                    import.ignored.join(", ")
                )
            });
        }
        (!dropped.is_empty()).then(|| {
            format!(
                "{name}: no empty pad for {} file(s): {}",
                dropped.len(),
                dropped.join(", ")
            )
        })
    }

    fn start_import(
        &mut self,
        paths: Vec<PathBuf>,
        target: ImportTarget,
        tick: i64,
        samples: Option<SampleImport>,
    ) {
        self.start_batch(paths, target, tick, samples, None);
    }

    /// Takes up the unfinished recordings a session that is gone left in this project's bundle
    /// and in the scratch bundles (see [`crate::recovery`]): each file becomes a track named
    /// `Recovered take N` with one clip at the start of the song, an undo step per track. The
    /// `rec-*.wav` file is deleted only after the project is saved, so discarding the project
    /// loses nothing. Returns how many files were queued. Without a recovery folder (no
    /// configuration directory) nothing is scanned: there is no way to tell a live recorder
    /// from a dead one.
    pub fn recover_orphan_recordings(&mut self) -> usize {
        let Some(folder) = self.recovery_folder() else {
            return 0;
        };
        let bundles: Vec<PathBuf> = self
            .audio
            .bundle
            .iter()
            .filter(|b| !b.scratch)
            .map(|b| b.bundle.root().to_path_buf())
            .collect();
        let scratch = self.scratch_scan_folder();
        let alive = |pid| crate::recovery::session_alive(&folder, pid);
        let found = crate::recovery::scan_orphans(
            &bundles,
            scratch.as_deref().map(|dir| (dir, ORPHAN_QUIET)),
            &alive,
        );
        let found: Vec<_> = found
            .into_iter()
            .filter(|o| !self.audio.recovered.contains(&o.path))
            .collect();
        if found.is_empty() {
            return 0;
        }
        let mut names = BTreeMap::new();
        for o in &found {
            let tracks = found
                .iter()
                .filter(|p| (p.pid, p.take) == (o.pid, o.take))
                .count();
            let name = if tracks > 1 {
                format!("Recovered take {} track {}", o.take + 1, o.track + 1)
            } else {
                format!("Recovered take {}", o.take + 1)
            };
            names.insert(o.path.clone(), name);
        }
        let paths: Vec<PathBuf> = found.iter().map(|o| o.path.clone()).collect();
        for p in &paths {
            self.audio.recovered.insert(p.clone());
        }
        let count = paths.len();
        let Some(batch) = self.start_batch(paths, ImportTarget::NewTracks, 0, None, None) else {
            return 0;
        };
        if let Some(b) = self.audio.batches.get_mut(&batch) {
            b.names = names;
        }
        count
    }

    fn start_batch(
        &mut self,
        paths: Vec<PathBuf>,
        target: ImportTarget,
        tick: i64,
        samples: Option<SampleImport>,
        relink: Option<relink::RelinkJob>,
    ) -> Option<u64> {
        if paths.is_empty() {
            return None;
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => {
                self.reject(e);
                return None;
            }
        };
        let batch = self.audio.next_batch;
        self.audio.next_batch += 1;
        let tick = tick.max(0);
        self.audio.batches.insert(
            batch,
            Batch {
                samples,
                target,
                tick,
                cursor: tick,
                total: paths.len(),
                done: 0,
                errors: Vec::new(),
                notes: Vec::new(),
                relink,
                names: BTreeMap::new(),
            },
        );
        let rate = self.doc().sample_rate;
        let started = spawn_import(
            self.audio.tx.clone(),
            self.audio.generation,
            batch,
            bundle,
            rate,
            self.stream_threshold(),
            paths,
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                Some(batch)
            }
            Err(e) => {
                self.audio.batches.remove(&batch);
                self.reject(format!("audio import: cannot start a worker: {e}"));
                None
            }
        }
    }

    fn apply_imported(&mut self, batch: u64, path: &Path, result: Result<Loaded, String>) {
        let name = display_name(path);
        let replaced = result
            .as_ref()
            .ok()
            .and_then(|l| l.info.as_ref())
            .map_or(0, |i| i.replaced_samples);
        let outcome = result.and_then(|loaded| {
            self.insert_loaded(&loaded);
            if self
                .audio
                .batches
                .get(&batch)
                .is_some_and(|b| b.relink.is_some())
            {
                self.relink_loaded(batch, &loaded)
            } else if self
                .audio
                .batches
                .get(&batch)
                .is_some_and(|b| b.samples.is_some())
            {
                self.register_sample(batch, &loaded)
            } else {
                self.place(batch, &loaded, path)
            }
        });
        let recovered = outcome.is_ok()
            && self
                .audio
                .batches
                .get(&batch)
                .is_some_and(|b| b.names.contains_key(path));
        if recovered {
            self.audio.recovered_files.push(path.to_path_buf());
        }
        if let Some(b) = self.audio.batches.get_mut(&batch) {
            b.done += 1;
            match outcome {
                Err(e) => b.errors.push(format!("{name}: {e}")),
                Ok(()) if replaced > 0 => {
                    let samples = if replaced == 1 { "sample" } else { "samples" };
                    log::warn!(
                        "{name}: {replaced} NaN or infinite {samples} replaced with silence"
                    );
                    b.notes.push(format!(
                        "{name}: {replaced} invalid {samples} (NaN or infinite) replaced with silence"
                    ));
                }
                Ok(()) => {}
            }
        }
    }

    /// Registers the asset of a sample import and remembers it for the batch's zones.
    fn register_sample(&mut self, batch: u64, loaded: &Loaded) -> Result<(), String> {
        let info = loaded
            .info
            .clone()
            .ok_or_else(|| "not an imported asset".to_owned())?;
        self.apply_command(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: atomik_model::AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        })
        .map_err(|e| e.to_string())?;
        if let Some(s) = self
            .audio
            .batches
            .get_mut(&batch)
            .and_then(|b| b.samples.as_mut())
        {
            s.files.push((info.hash, info.original_name, info.frames));
        }
        Ok(())
    }

    /// Registers the asset and adds its track and clip.
    fn place(&mut self, batch: u64, loaded: &Loaded, from: &Path) -> Result<(), String> {
        let info = loaded
            .info
            .clone()
            .ok_or_else(|| "not an imported asset".to_owned())?;
        let (target, tick, cursor, renamed) = {
            let b = self.audio.batches.get(&batch).ok_or("import cancelled")?;
            (
                b.target.clone(),
                b.tick,
                b.cursor,
                b.names.get(from).cloned(),
            )
        };
        let run = |core: &mut Self, command: Command| {
            core.apply_command(command).map_err(|e| e.to_string())
        };
        run(
            self,
            Command::RegisterAsset {
                hash: info.hash.clone(),
                meta: atomik_model::AssetMeta {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                    frames: info.frames,
                    original_name: info.original_name.clone(),
                },
            },
        )?;
        let (device, start) = match target {
            ImportTarget::ExistingTrack(key) => {
                if !self.doc().is_audio_track(&key, &self.registry) {
                    return Err(format!("{key} is not an audio track any more"));
                }
                (key, cursor)
            }
            ImportTarget::NewTracks => {
                let stem = renamed.or_else(|| {
                    Path::new(&info.original_name)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .filter(|s| !s.trim().is_empty())
                });
                run(
                    self,
                    Command::Song(SongCommand::AddAudioTrack { name: stem }),
                )?;
                let key: DeviceKey = self
                    .doc()
                    .song
                    .tracks
                    .last()
                    .map(|t| t.device.clone())
                    .ok_or("the audio track was not created")?;
                (key, tick)
            }
        };
        let rate = self.doc().sample_rate;
        let warp = {
            let song = &self.doc().song;
            crate::import_warp::import_warp(
                &loaded.meta,
                info.frames,
                rate,
                song.tempo_table().map().bpm(start as f64),
                song.bar_ticks_at(start),
                self.warp_imports,
            )
        };
        let length = match &warp {
            Some(w) => w.length,
            None => ticks_for_frames(&self.doc().song, start, info.frames, rate),
        };
        run(
            self,
            Command::Song(SongCommand::AddAudioClip {
                device,
                start,
                length,
                asset: info.hash,
                source_offset: 0,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: atomik_model::FadeCurve::Linear,
            }),
        )?;
        if let Some(w) = warp {
            self.warp_new_clip(w.source_bpm, length);
        }
        if let Some(b) = self.audio.batches.get_mut(&batch) {
            b.cursor = start + length;
        }
        Ok(())
    }

    /// Turns warp on at `source_bpm` for the clip just added, as part of the undo step that
    /// added it. A failure leaves the clip unwarped (it was added whole).
    fn warp_new_clip(&mut self, source_bpm: f64, length: i64) {
        let Some(id) = self.document.last_clip_id() else {
            return;
        };
        let Some((_, _, clip)) = self.doc().song.clip(id) else {
            return;
        };
        let atomik_model::ClipContent::Audio(content) = &clip.content else {
            return;
        };
        let mut content = content.clone();
        content.warp.enabled = true;
        content.warp.source_bpm =
            source_bpm.clamp(atomik_model::WARP_BPM_MIN, atomik_model::WARP_BPM_MAX);
        let command = Command::Song(SongCommand::SetAudioClip {
            id,
            start: clip.start,
            length,
            muted: clip.muted,
            content,
            merge: None,
        });
        self.join_next = true;
        let _ = self.apply_command(command);
        self.join_next = false;
    }

    // ------------------------------------------------------------------ open and replace

    /// Resets the audio state for the document just installed (`self.file` is set): drops
    /// the caches and finds the project's bundle. Call before the engine is rebuilt.
    pub(super) fn audio_reset(&mut self) {
        self.audio.generation += 1;
        self.audio.batches.clear();
        let stale: Vec<String> = self.audio.peaks.keys().cloned().collect();
        for hash in stale {
            self.forget_asset(&hash);
        }
        self.audio.assets.clear();
        self.audio.warps.clear();
        self.audio.warp_work.reset();
        self.audio.freeze.reset();
        self.audio.export.reset();
        self.audio.notes.reset();
        self.audio.buffers.clear();
        self.audio.ram_pending.clear();
        self.audio.missing.clear();
        self.audio.relinked.clear();
        if self.audio.bundle.as_ref().is_some_and(|b| b.scratch) && self.audio.pending == 0 {
            self.audio.remove_scratch();
        }
        self.audio.bundle = self
            .file
            .as_deref()
            .and_then(existing_bundle_root)
            .and_then(|root| Bundle::open(&root).ok())
            .map(|bundle| ProjectBundle {
                bundle,
                scratch: false,
            });
        let doc = self.doc();
        // A template has no bundle of its own yet: what it lacks is what its folder lacks.
        let template = self
            .audio
            .adopt_from
            .as_deref()
            .and_then(|root| Bundle::open(root).ok());
        let source = template
            .as_ref()
            .or_else(|| self.audio.bundle.as_ref().map(|b| &b.bundle));
        self.audio.missing = missing_assets(doc, source);
    }

    /// After the engine was rebuilt for a freshly opened document: reports missing assets and
    /// starts loading the present ones in the background.
    pub(super) fn audio_opened(&mut self) {
        if !self.audio.missing.is_empty() {
            let names: Vec<&str> = self.audio.missing.iter().map(|m| m.name.as_str()).collect();
            let message = format!(
                "missing audio files (their clips play silence): {}",
                names.join(", ")
            );
            self.reject(message);
        }
        if let Some(from) = self.audio.adopt_from.take() {
            // A template's audio is copied into the session bundle and loaded from there.
            let assets: BTreeMap<String, AssetMeta> = self
                .doc()
                .assets
                .iter()
                .filter(|(hash, _)| !self.audio.missing.iter().any(|m| &m.hash == *hash))
                .map(|(hash, meta)| (hash.clone(), meta.clone()))
                .collect();
            self.adopt_assets(&from, &assets);
            return;
        }
        let Some(bundle) = self.audio.bundle.as_ref().map(|b| b.bundle.clone()) else {
            return;
        };
        let missing: Vec<&String> = self.audio.missing.iter().map(|m| &m.hash).collect();
        let hashes: Vec<String> = self
            .doc()
            .assets
            .keys()
            .filter(|h| !missing.contains(h))
            .cloned()
            .collect();
        if hashes.is_empty() {
            return;
        }
        let threshold = self.stream_threshold();
        match spawn_load(
            self.audio.tx.clone(),
            self.audio.generation,
            bundle,
            threshold,
            hashes,
        ) {
            Ok(()) => self.audio.pending += 1,
            Err(e) => self.reject(format!("audio: cannot start the loader: {e}")),
        }
    }

    /// After a library item was pasted: the audio of its pool (`assets`, already registered)
    /// that the session has not loaded comes from the item's bundle folder `from`. A worker
    /// copies what the session bundle lacks and loads it; what the item does not hold is
    /// reported missing, like a project's missing audio.
    pub(super) fn adopt_assets(&mut self, from: &Path, assets: &BTreeMap<String, AssetMeta>) {
        let hashes: Vec<String> = assets
            .keys()
            .filter(|h| !self.audio.assets.contains_key(*h))
            .cloned()
            .collect();
        if hashes.is_empty() {
            return;
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => return self.reject(e),
        };
        let threshold = self.stream_threshold();
        let started = spawn_adopt(
            self.audio.tx.clone(),
            self.audio.generation,
            from.to_path_buf(),
            bundle,
            threshold,
            hashes,
        );
        match started {
            Ok(()) => self.audio.pending += 1,
            Err(e) => self.reject(format!("audio: cannot start the loader: {e}")),
        }
    }

    /// Opens a project file or bundle folder (the project file inside it). A project's audio is
    /// loaded in the background; missing files are reported, never fatal. On error nothing
    /// changes.
    pub fn open(&mut self, path: &Path) -> Result<(), AppError> {
        let file = atomik_render::bundle::resolve_project_path(path);
        let text = atomik_model::read_document_text(&file).map_err(|source| AppError::Io {
            path: file.clone(),
            source,
        })?;
        self.open_str(&text, Some(file))?;
        self.note_recent();
        self.recover_orphan_recordings();
        Ok(())
    }

    // ------------------------------------------------------------------ save

    /// Saves to the current file when it is a project (`.atomik`); a new rack or one opened
    /// from a patch has no project name yet ([`AppError::NoPath`]: use save-as), so a patch
    /// file is never overwritten with a project. The file does not move; see the module docs
    /// for its bundle.
    pub fn save(&mut self) -> Result<(), AppError> {
        let file = self
            .file
            .clone()
            .filter(|p| super::is_project_path(p))
            .ok_or(AppError::NoPath)?;
        let root = existing_bundle_root(&file).unwrap_or_else(|| sibling_bundle_root(&file));
        self.write_project(&file, Some(root))
    }

    /// Saves as a project to `path`, which becomes the current file; a project with audio
    /// becomes a bundle folder (module docs), so the current file may be
    /// `<dir>/<name>/<name>.atomik`.
    pub fn save_as(&mut self, path: &Path) -> Result<(), AppError> {
        let saved = if self.document.referenced_assets().is_empty() {
            self.write_project(path, None)
        } else {
            let (file, root) = save_as_layout(path);
            self.write_project(&file, Some(root))
        };
        if saved.is_ok() {
            self.note_recent();
        }
        saved
    }

    fn write_project(&mut self, file: &Path, root: Option<PathBuf>) -> Result<(), AppError> {
        // The plugins' own state goes into the document first.
        self.plugins_capture_all();
        let keep = self.document.referenced_assets();
        let mut saved = self.doc().clone();
        saved.prune_unreferenced_assets();
        let text = atomik_model::save_project_checked(&saved)?;
        let mut dest: Option<Bundle> = None;
        if let (Some(root), false) = (&root, keep.is_empty()) {
            let created = match &self.audio.bundle {
                Some(b) if b.bundle.root() == root.as_path() => Ok(b.bundle.clone()),
                Some(b) => Bundle::copy_bundle(b.bundle.root(), root),
                None => Bundle::create(root),
            };
            dest = Some(created.map_err(|e| io_error(root, e))?);
        } else if let (Some(root), Some(b)) = (&root, &self.audio.bundle)
            && !b.scratch
            && b.bundle.root() == root.as_path()
        {
            // The last audio was removed: its files still go.
            dest = Some(b.bundle.clone());
        }
        save_text_atomic(file, &text).map_err(|e| io_error(file, e))?;
        if let Some(bundle) = &mut dest {
            bundle
                .remove_orphans(&keep)
                .map_err(|e| io_error(bundle.root(), e))?;
        }
        // The project is on disk: switch to its bundle and tidy the live state.
        let was_scratch = self.audio.bundle.as_ref().is_some_and(|b| b.scratch);
        self.audio.bundle = dest.map(|bundle| ProjectBundle {
            bundle,
            scratch: false,
        });
        if was_scratch && self.audio.pending == 0 {
            self.audio.remove_scratch();
        }
        let before: Vec<String> = self.doc().assets.keys().cloned().collect();
        self.document.prune_unreferenced_assets();
        for hash in before {
            if !self.doc().assets.contains_key(&hash) {
                self.forget_asset(&hash);
            }
        }
        self.file = Some(file.to_path_buf());
        self.set_saved(self.doc().clone());
        // The recovered recordings are in the saved project now: their files can go.
        for path in std::mem::take(&mut self.audio.recovered_files) {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    /// Imports files dropped outside the arrangement as new tracks at the snapped playhead.
    pub fn import_at_playhead(&mut self, paths: Vec<PathBuf>) {
        let tick = self.snapped_playhead();
        self.import_audio(paths, ImportTarget::NewTracks, tick);
    }
}

#[path = "app_core_relink.rs"]
mod relink;

#[cfg(test)]
#[path = "app_core_audio_tests.rs"]
mod tests;
