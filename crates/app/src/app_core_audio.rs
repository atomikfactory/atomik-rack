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
//!   ([`AppCore::missing_assets`], and a toast); those clips play silence. Relinking is not
//!   implemented yet.
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
use crate::audio_import::{Loaded, Msg, display_name, spawn_import, spawn_load, ticks_for_frames};
use crate::sink::EngineSink;
use crate::stream_source::STREAM_THRESHOLD_BYTES;
use atomik_audio_file::{AudioFileError, Bundle, PeakPyramid, save_text_atomic};
use atomik_device_sdk::SampleBuffer;
use atomik_engine::SongAsset;
use atomik_model::{Command, DeviceKey, SongCommand, save_project};
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
struct SampleImport {
    device: DeviceKey,
    at_key: Option<u8>,
    /// Files a loop slicer left out (it takes one loop, the first).
    ignored: Vec<String>,
    /// `(asset id, original file name, frames)` in the order the files finished.
    files: Vec<(String, String, u64)>,
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
}

/// Everything audio of an open project.
pub(super) struct AudioState {
    bundle: Option<ProjectBundle>,
    pub(super) assets: BTreeMap<String, SongAsset>,
    pub(super) peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// Whole-asset RAM buffers for samplers: shared with the memory assets of
    /// audio tracks where those exist, loaded by a worker for streamed ones. Keeping the
    /// `Arc`s is what lets a sampler voice survive a zone edit (same buffer, same pointer).
    pub(super) buffers: BTreeMap<String, Arc<SampleBuffer>>,
    /// Assets a worker is loading into RAM for samplers.
    pub(super) ram_pending: BTreeSet<String>,
    pub(super) missing: Vec<MissingAsset>,
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
}

impl AudioState {
    pub(super) fn new() -> Self {
        static SESSIONS: AtomicU64 = AtomicU64::new(0);
        let (tx, rx) = channel();
        let session = SESSIONS.fetch_add(1, Ordering::Relaxed);
        Self {
            bundle: None,
            assets: BTreeMap::new(),
            peaks: BTreeMap::new(),
            buffers: BTreeMap::new(),
            ram_pending: BTreeSet::new(),
            missing: Vec::new(),
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
        }
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
        // Assets in RAM share their samples with the views: below the finest peak level the
        // waveform reads them. Streamed assets have none and keep the finest peak level.
        if let SongAsset::Memory { data, .. } = &loaded.asset {
            self.arrangement.set_samples(&loaded.hash, Arc::clone(data));
            self.sample_editor
                .set_samples(&loaded.hash, Arc::clone(data));
            self.slice_editor
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
                    let sampler_batch = self
                        .audio
                        .batches
                        .get(&batch)
                        .is_some_and(|b| b.samples.is_some());
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
                Msg::Sliced {
                    generation,
                    job,
                    result,
                } if generation == current => self.apply_sliced(job, result),
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
                        self.audio.missing.push(MissingAsset {
                            hash,
                            name: name.clone(),
                        });
                        self.reject(format!("audio {name}: {e}"));
                    }
                },
                // A message of a document that was closed meanwhile.
                Msg::Imported { .. }
                | Msg::Loaded { .. }
                | Msg::Sampled { .. }
                | Msg::Sliced { .. } => {}
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
            if !batch.errors.is_empty() {
                report.push(format!("audio import: {}", batch.errors.join("; ")));
            }
            report.extend(note);
            if !report.is_empty() {
                self.reject(report.join(" \u{b7} "));
            }
        }
    }

    // ------------------------------------------------------------------ import

    pub(super) fn import_bundle(&mut self) -> Result<Bundle, String> {
        if self.audio.bundle.is_none() {
            self.audio.scratch_count += 1;
            let dir = self
                .audio
                .scratch_root
                .join(self.audio.scratch_count.to_string());
            let bundle = Bundle::create(&dir).map_err(|e| format!("session bundle: {e}"))?;
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
        if self.is_loop_slicer(&device) && paths.len() > 1 {
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

    /// Adds the zones of a finished sample import in one undo step. Returns a note for the
    /// status message when files found no pad (a drum machine).
    fn finish_sample_import(&mut self, import: SampleImport) -> Option<String> {
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
        // A rejection is already reported.
        let _ = self.apply_command(Command::SetZones {
            device: import.device,
            zones,
            merge: None,
        });
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
        if paths.is_empty() {
            return;
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => return self.reject(e),
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
            Ok(()) => self.audio.pending += 1,
            Err(e) => {
                self.audio.batches.remove(&batch);
                self.reject(format!("audio import: cannot start a worker: {e}"));
            }
        }
    }

    fn apply_imported(&mut self, batch: u64, path: &Path, result: Result<Loaded, String>) {
        let name = display_name(path);
        let outcome = result.and_then(|loaded| {
            self.insert_loaded(&loaded);
            if self
                .audio
                .batches
                .get(&batch)
                .is_some_and(|b| b.samples.is_some())
            {
                self.register_sample(batch, &loaded)
            } else {
                self.place(batch, &loaded)
            }
        });
        if let Some(b) = self.audio.batches.get_mut(&batch) {
            b.done += 1;
            if let Err(e) = outcome {
                b.errors.push(format!("{name}: {e}"));
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
    fn place(&mut self, batch: u64, loaded: &Loaded) -> Result<(), String> {
        let info = loaded
            .info
            .clone()
            .ok_or_else(|| "not an imported asset".to_owned())?;
        let (target, tick, cursor) = {
            let b = self.audio.batches.get(&batch).ok_or("import cancelled")?;
            (b.target.clone(), b.tick, b.cursor)
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
                let stem = Path::new(&info.original_name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.trim().is_empty());
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
        let length = ticks_for_frames(&self.doc().song, start, info.frames, rate);
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
        if let Some(b) = self.audio.batches.get_mut(&batch) {
            b.cursor = start + length;
        }
        Ok(())
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
        self.audio.buffers.clear();
        self.audio.ram_pending.clear();
        self.audio.missing.clear();
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
        self.audio.missing = missing_assets(doc, self.audio.bundle.as_ref().map(|b| &b.bundle));
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

    /// Opens a project file or bundle folder (the project file inside it). A project's audio is
    /// loaded in the background; missing files are reported, never fatal. On error nothing
    /// changes.
    pub fn open(&mut self, path: &Path) -> Result<(), AppError> {
        let file = atomik_render::bundle::resolve_project_path(path);
        let text = std::fs::read_to_string(&file).map_err(|source| AppError::Io {
            path: file.clone(),
            source,
        })?;
        self.open_str(&text, Some(file))
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
        if self.document.referenced_assets().is_empty() {
            self.write_project(path, None)
        } else {
            let (file, root) = save_as_layout(path);
            self.write_project(&file, Some(root))
        }
    }

    fn write_project(&mut self, file: &Path, root: Option<PathBuf>) -> Result<(), AppError> {
        // The plugins' own state goes into the document first.
        self.plugins_capture_all();
        let keep = self.document.referenced_assets();
        let mut saved = self.doc().clone();
        saved.prune_unreferenced_assets();
        let text = save_project(&saved);
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
        self.saved = self.doc().clone();
        Ok(())
    }

    /// Imports files dropped outside the arrangement as new tracks at the snapped playhead.
    pub fn import_at_playhead(&mut self, paths: Vec<PathBuf>) {
        let tick = self.snapped_playhead();
        self.import_audio(paths, ImportTarget::NewTracks, tick);
    }
}

#[cfg(test)]
#[path = "app_core_audio_tests.rs"]
mod tests;
