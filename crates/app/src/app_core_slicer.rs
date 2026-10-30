//! Loop slicers in the app core: the slice
//! editor pane, loading a loop, re-slicing and "To track".
//!
//! # Slice editor pane
//!
//! [`Pane::SliceEditor`](crate::Pane::SliceEditor) shows [`SliceEditorView`] in place of the
//! arrangement, exclusive with the piano roll, the sample editor, the step editor and the
//! console (the shell owns the console). It opens on `UiAction::OpenSliceEditor` and on View ▸
//! Slice Editor (the selected loop slicer, else the first), copying the rack panel's
//! sensitivity into the editor; it closes on `CloseSliceEditor`, when the slicer goes away
//! (`is_present`), and on a new or opened document. Peaks and missing assets are fed as for the
//! sample editor.
//!
//! # Loading a loop
//!
//! `UiAction::ImportSamples` on a loop slicer (drops and the Load button; `at_key` is ignored)
//! imports **only the first file** (the others are reported as ignored) through the ordinary
//! import worker. When it is stored, a second worker runs transient detection on the loop's
//! samples at the current sensitivity (the editor's when it is open on that slicer, else the
//! rack panel's) so the UI thread never analyses audio. The result (dropped when the document
//! generation changed meanwhile) is applied as **one undo step** (a `Command::Batch` of `SetParam loop.beats` =
//! [`default_loop_beats`] at the song tempo and `SetZones` with the new slices; the previous
//! zones are replaced, no fields kept); one undo takes the slices away and restores the beats.
//!
//! # `SliceLoop` and `SlicesToTrack`
//!
//! `SliceLoop` works on the asset of the device's slice zones (ignored, with a message, when it
//! has none). `Transients` runs detection on a worker, `Grid` uses
//! [`grid_markers`] with the device's `loop.beats` at once. Either ends in **one** `SetZones` with
//! [`slices_from_markers`] over the current zones (per-slice fields stay with their index); a
//! result equal to the current zones adds no step.
//!
//! `SlicesToTrack` writes [`to_track_notes`] as **one undo step**:
//!
//! * the selected clip, if it is a note clip on the slicer's track, gets the new notes and the
//!   new length (`loop.beats` × PPQ); its start, mute flag and step grid stay, its content
//!   offset and loop length are cleared (the content is one pass of the loop). Two song
//!   commands (`SetClipGeometry`, `SetClipContent`) share one merge group, which the model
//!   joins into a single step;
//! * otherwise a new note clip is added at the playhead snapped to the bar, in a `Batch` with
//!   `AddTrack` when the slicer has no track yet.

use super::AppCore;
use crate::audio_import::{LoopSource, SliceJob, spawn_onsets};
use crate::sink::EngineSink;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, RackDevice, SongCommand, ZoneDoc,
    default_loop_beats, grid_markers, slices_from_markers, to_track_notes,
};
use atomik_render::samples::song_asset_buffer;
use atomik_ui::{SliceEditorView, SliceMode};
use std::sync::Arc;

/// The loop slicer's device type.
pub(super) const LOOP_SLICER: &str = "atomik.loop-slicer";

/// The slice zones' key range (`SLICE_KEY_BASE..SLICE_KEY_BASE + MAX_SLICES`).
const SLICE_KEYS: std::ops::Range<u8> =
    atomik_model::SLICE_KEY_BASE..atomik_model::SLICE_KEY_BASE + atomik_model::MAX_SLICES as u8;
/// The merge group of the clip edit of "To track" (always closed before and after).
const TO_TRACK_MERGE: u64 = 7 << 40;
/// The `loop.beats` param.
const BEATS_PARAM: &str = "loop.beats";
/// Sensitivity of a slicer nobody has touched.
const DEFAULT_SENSITIVITY: u8 = 50;

/// The loop asset of a slicer: the asset of its first slice zone.
fn loop_asset(device: &RackDevice) -> Option<&ZoneDoc> {
    device
        .zones
        .iter()
        .find(|z| z.key_lo == z.key_hi && SLICE_KEYS.contains(&z.key_lo))
}

/// The shortest slice: 1 ms of frames.
fn min_len(rate: u32) -> u64 {
    u64::from(rate / 1000).max(1)
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ slice editor pane

    /// The slice editor.
    pub fn slice_editor(&self) -> &SliceEditorView {
        &self.slice_editor
    }

    /// The slice editor, mutably (bounds, scale, focus).
    pub fn slice_editor_mut(&mut self) -> &mut SliceEditorView {
        &mut self.slice_editor
    }

    /// The loop slicer open in the slice editor (shown in place of the arrangement).
    pub fn slice_editor_device(&self) -> Option<&DeviceKey> {
        self.slice_editor.device()
    }

    pub(super) fn is_loop_slicer(&self, device: &DeviceKey) -> bool {
        self.doc()
            .resolve(device)
            .is_some_and(|d| d.type_id == LOOP_SLICER)
    }

    /// Opens the slice editor on a loop slicer (ignored for other devices); it replaces the
    /// piano roll, the sample editor and the step editor. The rack panel's sensitivity is
    /// copied into the editor.
    pub fn open_slice_editor(&mut self, device: DeviceKey) {
        if !self.is_loop_slicer(&device) {
            return;
        }
        self.close_piano_roll();
        self.close_audio_editor();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_seq_editor();
        self.close_container_editor();
        let sensitivity = self.view.slice_sensitivity(&device);
        self.slice_editor.open(device);
        if let Some(percent) = sensitivity {
            self.slice_editor.set_sensitivity(percent);
        }
        for (hash, peaks) in &self.audio.peaks {
            self.slice_editor.set_peaks(hash, Arc::clone(peaks));
        }
        self.slice_editor
            .set_missing_assets(self.audio.missing.iter().map(|m| m.hash.clone()));
        self.slice_editor.sync(self.document.doc(), &self.registry);
    }

    /// Opens the slice editor on the selected loop slicer, else the first one of the rack.
    /// False when the rack has none.
    pub fn open_slice_editor_default(&mut self) -> bool {
        let doc = self.doc();
        let is_slicer = |d: &RackDevice| d.type_id == LOOP_SLICER;
        let key = self
            .view
            .selected()
            .filter(|k| doc.resolve(k).is_some_and(is_slicer))
            .cloned()
            .or_else(|| {
                self.flat_devices()
                    .into_iter()
                    .find(|(_, d)| is_slicer(d))
                    .map(|(k, _)| k)
            });
        match key {
            Some(key) => {
                self.open_slice_editor(key);
                true
            }
            None => false,
        }
    }

    /// Closes the slice editor (the arrangement shows again).
    pub fn close_slice_editor(&mut self) {
        self.slice_editor.close();
    }

    // ------------------------------------------------------------------ detection

    /// The sensitivity (percent) of a slicer: the editor's while it is open on it, else the
    /// rack panel's.
    fn sensitivity_of(&self, device: &DeviceKey) -> u8 {
        if self.slice_editor.device() == Some(device) {
            self.slice_editor.sensitivity()
        } else {
            self.view
                .slice_sensitivity(device)
                .unwrap_or(DEFAULT_SENSITIVITY)
        }
    }

    /// Where a detection worker reads the loop `hash` from: the RAM cache, a RAM copy of a
    /// memory asset, or (streamed assets) the bundle file. `None` when the asset is not
    /// available (its file is missing).
    fn loop_source(&mut self, hash: &str) -> Option<LoopSource> {
        if let Some(b) = self.audio.buffers.get(hash) {
            return Some(LoopSource::Buffer(Arc::clone(b)));
        }
        let asset = self.audio.assets.get(hash)?;
        if asset.is_stream() {
            return self
                .bundle_clone()
                .map(|bundle| LoopSource::File(bundle, hash.to_owned()));
        }
        let buffer = song_asset_buffer(asset)?;
        self.audio
            .buffers
            .insert(hash.to_owned(), Arc::clone(&buffer));
        Some(LoopSource::Buffer(buffer))
    }

    fn spawn_detection(&mut self, job: SliceJob) {
        let Some(source) = self.loop_source(&job.asset) else {
            let name = self.asset_name(&job.asset);
            return self.reject(format!("loop slicer: the audio of {name} is missing"));
        };
        match spawn_onsets(self.audio.tx.clone(), self.audio.generation, job, source) {
            Ok(()) => self.audio.pending += 1,
            Err(e) => self.reject(format!("loop slicer: cannot start the detector: {e}")),
        }
    }

    /// A freshly imported loop: detection at the current sensitivity, then the beats and the
    /// slices are applied by [`Self::apply_sliced`].
    pub(super) fn start_loop_slicing(&mut self, device: DeviceKey, file: &(String, String, u64)) {
        let (asset, _name, frames) = file;
        let rate = self.doc().sample_rate;
        let beats = default_loop_beats(*frames, rate, self.doc().tempo_bpm);
        let sensitivity = self.sensitivity_of(&device);
        self.spawn_detection(SliceJob {
            device,
            asset: asset.clone(),
            frames: *frames,
            rate,
            sensitivity,
            load_beats: Some(beats),
        });
    }

    /// Applies the markers a detection worker found (the document generation was checked).
    pub(super) fn apply_sliced(&mut self, job: SliceJob, result: Result<Vec<u64>, String>) {
        let markers = match result {
            Ok(m) => m,
            Err(e) => return self.reject(format!("loop slicer: {e}")),
        };
        let Some(device) = self.doc().resolve(&job.device) else {
            return;
        };
        if device.type_id != LOOP_SLICER || !self.doc().assets.contains_key(&job.asset) {
            return;
        }
        let min_len = min_len(job.rate);
        match job.load_beats {
            Some(beats) => {
                let zones = slices_from_markers(&job.asset, job.frames, &markers, &[], min_len);
                let _ = self.apply_command(Command::Batch(vec![
                    Command::SetParam {
                        key: job.device.clone(),
                        param: BEATS_PARAM.to_owned(),
                        value: beats as f32,
                        merge: None,
                    },
                    Command::SetZones {
                        device: job.device,
                        zones,
                        merge: None,
                    },
                ]));
            }
            None => {
                // The loop was replaced meanwhile: the markers belong to another asset.
                if loop_asset(device).map(|z| z.asset.as_str()) != Some(job.asset.as_str()) {
                    return;
                }
                let zones =
                    slices_from_markers(&job.asset, job.frames, &markers, &device.zones, min_len);
                self.set_slices(job.device, zones);
            }
        }
    }

    /// One `SetZones` with `zones`, unless they equal the current ones.
    fn set_slices(&mut self, device: DeviceKey, zones: Vec<ZoneDoc>) {
        if self
            .doc()
            .resolve(&device)
            .is_some_and(|d| d.zones == zones)
        {
            return;
        }
        let _ = self.apply_command(Command::SetZones {
            device,
            zones,
            merge: None,
        });
    }

    // ------------------------------------------------------------------ slicing

    /// `UiAction::SliceLoop`: re-slices the loop of `device` (see the module docs).
    pub fn slice_loop(&mut self, device: DeviceKey, mode: SliceMode) {
        let doc = self.doc();
        let Some(dev) = doc.resolve(&device).filter(|d| d.type_id == LOOP_SLICER) else {
            return self.reject(format!("{device} is not a loop slicer"));
        };
        let Some(asset) = loop_asset(dev).map(|z| z.asset.clone()) else {
            return self.reject(format!("{}: load a loop first", dev.name));
        };
        let Some(frames) = doc.assets.get(&asset).map(|m| m.frames) else {
            return;
        };
        let rate = doc.sample_rate;
        match mode {
            SliceMode::Transients { sensitivity } => self.spawn_detection(SliceJob {
                device,
                asset,
                frames,
                rate,
                sensitivity: sensitivity.min(100),
                load_beats: None,
            }),
            SliceMode::Grid { division } => {
                let beats = dev.params.get(BEATS_PARAM).map_or(4, |b| b.round() as u32);
                let markers = grid_markers(frames, beats, division);
                let zones =
                    slices_from_markers(&asset, frames, &markers, &dev.zones, min_len(rate));
                self.set_slices(device, zones);
            }
        }
    }

    // ------------------------------------------------------------------ to track

    /// `UiAction::SlicesToTrack`: writes the slices as a note clip, one undo step (see the
    /// module docs).
    pub fn slices_to_track(&mut self, device: DeviceKey) {
        if self.refuse_inner(&device, "writing slices to a track") {
            return;
        }
        let doc = self.doc();
        let Some(dev) = doc.device(&device).filter(|d| d.type_id == LOOP_SLICER) else {
            return self.reject(format!("{device} is not a loop slicer"));
        };
        let Some(asset) = loop_asset(dev).map(|z| z.asset.clone()) else {
            return self.reject(format!("{}: load a loop first", dev.name));
        };
        let Some(frames) = doc.assets.get(&asset).map(|m| m.frames) else {
            return;
        };
        let beats = dev.params.get(BEATS_PARAM).map_or(4, |b| b.round() as u32);
        let (notes, length) = to_track_notes(&dev.zones, frames, beats);
        if notes.is_empty() {
            return self.reject(format!("{}: no slices to write", dev.name));
        }
        let length = length as i64;
        let content = ClipContent::Notes(notes);
        let song = &doc.song;
        let selected = self
            .arrangement
            .selected_clips()
            .first()
            .and_then(|&id| song.clip(id))
            .filter(|(track, _, clip)| track.device == device && clip.content.is_notes())
            .map(|(_, _, clip)| clip.clone());
        self.document.end_merge();
        let id = match selected {
            Some(clip) => self.replace_clip(&clip, content, length),
            None => self.add_clip(&device, content, length),
        };
        self.document.end_merge();
        if let Some(id) = id {
            self.arrangement.set_selected_clips(&[id]);
        }
    }

    /// Replaces a clip's notes and length (one step through a shared merge group).
    fn replace_clip(&mut self, clip: &Clip, content: ClipContent, length: i64) -> Option<ClipId> {
        let geometry = Command::Song(SongCommand::SetClipGeometry {
            id: clip.id,
            start: clip.start,
            length,
            offset: 0,
            loop_length: None,
            muted: clip.muted,
            merge: Some(TO_TRACK_MERGE),
        });
        self.apply_command(geometry).ok()?;
        let set = Command::Song(SongCommand::SetClipContent {
            id: clip.id,
            content,
            merge: Some(TO_TRACK_MERGE),
        });
        if self.apply_command(set).is_err() {
            // Keep the one-step promise: nothing changes if the notes were refused.
            self.undo();
            return None;
        }
        Some(clip.id)
    }

    /// Adds the clip at the playhead snapped to the bar (with the track, if the slicer has
    /// none, in one `Batch`).
    fn add_clip(
        &mut self,
        device: &DeviceKey,
        content: ClipContent,
        length: i64,
    ) -> Option<ClipId> {
        let song = &self.doc().song;
        let start = song.nearest_bar_line(self.position).max(0);
        let has_track = song.track(device).is_some();
        let add = Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content,
            },
        });
        let command = if has_track {
            add
        } else {
            Command::Batch(vec![
                Command::Song(SongCommand::AddTrack {
                    device: device.clone(),
                }),
                add,
            ])
        };
        self.apply_command(command).ok()?;
        self.document.last_clip_id()
    }
}
