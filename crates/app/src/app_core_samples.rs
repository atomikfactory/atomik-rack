//! Samplers in the app core: sample maps for the engine, the sample
//! editor pane and live auditions.
//!
//! # Sample maps
//!
//! A sampler's zones name pool assets. [`AppCore::send_samples`] builds the sampler's
//! [`atomik_device_sdk::SampleMap`] with the shared builder ([`atomik_render::samples`]) from the
//! asset cache and sends it with [`EngineSink::set_device_samples`]. **Always RAM**: an
//! asset the audio tracks hold in memory shares its sample data (and one `Arc<SampleBuffer>`
//! per asset is kept, so a zone edit keeps playing voices on the same buffers); an asset held
//! only as a streamed source (an audio-track asset above the streaming threshold) is loaded
//! fully into RAM by a worker. Until it arrives, and for assets whose file is missing, the
//! zone is left out of the map and plays silence. Maps are built on the UI thread; a worker
//! only reads files.
//!
//! The map goes to the engine
//!
//! * on every [`atomik_model::SyncOp::ZonesChanged`] (one call per edit),
//! * right after a sampler is inserted (a restart, an added, pasted or un-removed device), in
//!   the restart order: insert, then samples,
//! * and when an asset finishes loading (open, import, a RAM copy of a streamed asset): every
//!   sampler that uses it gets a new map.
//!
//! # Sample editor pane
//!
//! [`Pane::SampleEditor`](crate::Pane::SampleEditor) shows [`SampleEditorView`] in place of the
//! arrangement. [`AppCore::open_sample_editor`] opens it on a sampler (it replaces the piano
//! roll); it closes on `CloseSampleEditor` and when the sampler is removed
//! ([`SampleEditorView::is_present`] turns false). Peaks are fed with the arrangement's, and the
//! assets whose file is missing are reported to it.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_core::NoteKind;
use atomik_device_sdk::SampleBuffer;
use atomik_model::DeviceKey;
use atomik_render::samples::{build_sample_map, song_asset_buffer};
use atomik_ui::SampleEditorView;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The granular sampler's device type: it plays one sample, so a drop replaces its zone.
pub(super) const GRANULAR: &str = "atomik.granular";

impl<S: EngineSink> AppCore<S> {
    pub(super) fn is_granular(&self, device: &DeviceKey) -> bool {
        self.doc()
            .resolve(device)
            .is_some_and(|d| d.type_id == GRANULAR)
    }

    // ------------------------------------------------------------------ sample editor pane

    /// The sample editor.
    pub fn sample_editor(&self) -> &SampleEditorView {
        &self.sample_editor
    }

    /// The sample editor, mutably (bounds, scale, focus).
    pub fn sample_editor_mut(&mut self) -> &mut SampleEditorView {
        &mut self.sample_editor
    }

    /// The sampler open in the sample editor (shown in place of the arrangement).
    pub fn sample_editor_device(&self) -> Option<&DeviceKey> {
        self.sample_editor.device()
    }

    /// Opens the sample editor on a sampler (ignored for other devices); it replaces the
    /// piano roll.
    pub fn open_sample_editor(&mut self, device: DeviceKey) {
        let takes = self
            .doc()
            .resolve(&device)
            .is_some_and(|d| self.registry.takes_samples(&d.type_id));
        if !takes {
            return;
        }
        self.close_piano_roll();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.sample_editor.open(device);
        for (hash, peaks) in &self.audio.peaks {
            self.sample_editor.set_peaks(hash, Arc::clone(peaks));
        }
        self.sample_editor
            .set_missing_assets(self.audio.missing.iter().map(|m| m.hash.clone()));
        self.sample_editor.sync(self.document.doc(), &self.registry);
    }

    /// Opens the sample editor on the selected sampler, else the first sampler of the rack.
    /// False when the rack has no sampler.
    pub fn open_sample_editor_default(&mut self) -> bool {
        let doc = self.doc();
        let is_sampler = |d: &atomik_model::RackDevice| self.registry.takes_samples(&d.type_id);
        // The selection may be an inner device (a path key); the fallback searches the insides too.
        let key = self
            .view
            .selected()
            .filter(|k| doc.resolve(k).is_some_and(is_sampler))
            .cloned()
            .or_else(|| {
                self.flat_devices()
                    .into_iter()
                    .find(|(_, d)| is_sampler(d))
                    .map(|(k, _)| k)
            });
        match key {
            Some(key) => {
                self.open_sample_editor(key);
                true
            }
            None => false,
        }
    }

    /// Closes the sample editor (the arrangement shows again).
    pub fn close_sample_editor(&mut self) {
        self.sample_editor.close();
    }

    // ------------------------------------------------------------------ audition

    /// Plays (`on`) or releases a key on a device at a 7-bit velocity through the live note
    /// path (the sample editor's velocity-aware audition: `velocity / 127`).
    pub fn audition_velocity(&mut self, device: &DeviceKey, key: u8, velocity: u8, on: bool) {
        let kind = if on {
            NoteKind::On {
                key,
                velocity: f32::from(velocity.clamp(1, 127)) / 127.0,
            }
        } else {
            NoteKind::Off { key }
        };
        self.live_note_to(device, kind);
    }

    // ------------------------------------------------------------------ sample maps

    /// The RAM buffer of an asset, if its audio is available now. A streamed asset starts a
    /// worker that loads it fully (`None` until it arrives).
    fn zone_buffer(&mut self, hash: &str) -> Option<Arc<SampleBuffer>> {
        if let Some(b) = self.audio.buffers.get(hash) {
            return Some(Arc::clone(b));
        }
        let asset = self.audio.assets.get(hash)?;
        if asset.is_stream() {
            self.load_into_ram(hash);
            return None;
        }
        let buffer = song_asset_buffer(asset)?;
        self.audio
            .buffers
            .insert(hash.to_owned(), Arc::clone(&buffer));
        Some(buffer)
    }

    /// Starts the worker that reads asset `hash` of the bundle fully into RAM (once).
    fn load_into_ram(&mut self, hash: &str) {
        if self.audio.ram_pending.contains(hash) {
            return;
        }
        let Some(bundle) = self.bundle_clone() else {
            return;
        };
        match crate::audio_import::spawn_sample_load(
            self.audio.tx.clone(),
            self.audio.generation,
            bundle,
            vec![hash.to_owned()],
        ) {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.ram_pending.insert(hash.to_owned());
            }
            Err(e) => self.reject(format!("sampler audio: cannot start the loader: {e}")),
        }
    }

    /// Builds the sampler's map from its zones and sends it. With `quiet_if_empty` a map that
    /// would be empty is not sent (a fresh device has none to clear).
    fn send_map(&mut self, key: &DeviceKey, quiet_if_empty: bool) {
        let Some(id) = self.ids.get(key).copied() else {
            return;
        };
        let Some(device) = self.doc().resolve(key) else {
            return;
        };
        if !self.registry.takes_samples(&device.type_id) {
            return;
        }
        let (zones, name) = (device.zones.clone(), device.name.clone());
        let built = build_sample_map(&zones, |hash| self.zone_buffer(hash));
        for problem in built.rejected {
            self.reject(format!("sampler {name}: {problem}"));
        }
        if quiet_if_empty && built.map.is_none() {
            return;
        }
        let result = self.sink.set_device_samples(id, built.map);
        self.engine_result(result);
    }

    /// Sends a sampler its map after a zone edit (`SyncOp::ZonesChanged`).
    pub(super) fn send_samples(&mut self, key: &DeviceKey) {
        self.send_map(key, false);
    }

    /// Sends a freshly inserted sampler its map, if it has playable zones (after the insert, in
    /// queue order).
    pub(super) fn send_samples_after_insert(&mut self, key: &DeviceKey) {
        let has_zones = self.doc().resolve(key).is_some_and(|d| !d.zones.is_empty());
        if has_zones {
            self.send_map(key, true);
        }
    }

    /// Sends every sampler that has a zone on one of `assets` a new map (their audio arrived).
    pub(super) fn send_samples_using(&mut self, assets: &BTreeSet<String>) {
        let keys: Vec<DeviceKey> = self
            .flat_devices()
            .into_iter()
            .filter(|(_, d)| d.zones.iter().any(|z| assets.contains(&z.asset)))
            .map(|(k, _)| k)
            .collect();
        for key in keys {
            self.send_samples(&key);
        }
    }
}
