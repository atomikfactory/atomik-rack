//! Sampled instruments in the app core: SFZ and SoundFont files dropped on a sampler.
//!
//! `UiAction::ImportSamples` with an `.sfz` or `.sf2` file (a drop on the sampler or its sample
//! editor, or the editor's Load button) loads it as an **instrument**: a worker
//! ([`crate::instrument_import`]) reads the file, stores its samples in the project's bundle
//! at the session rate and builds the zones; the UI thread then registers the assets and
//! **replaces the sampler's zones** in one undo step (an instrument is a whole mapping; audio
//! files dropped next to it are ignored, as are further instrument files). What was left out
//! (opcodes that are not read, regions that do not fit a zone, samples that could not be read)
//! comes back as one message. Other devices refuse an instrument file with a message.

use super::AppCore;
use crate::audio_import::{display_name, spawn_instrument};
use crate::instrument_import::ImportedInstrument;
use crate::sink::EngineSink;
use atomik_model::{AssetMeta, Command, DeviceKey};
use std::path::{Path, PathBuf};

/// The sampler's device type: the only device an instrument file loads into.
pub(super) const SAMPLER: &str = "atomik.sampler";

impl<S: EngineSink> AppCore<S> {
    /// Starts loading the first of `instruments` into the sampler `device`; `other_files` is how
    /// many non-instrument files came with it (ignored). A device that is not a sampler refuses.
    /// True when the instrument load was started or refused (the caller imports nothing else).
    pub(super) fn import_instrument(
        &mut self,
        device: &DeviceKey,
        instruments: &[PathBuf],
        other_files: usize,
    ) -> bool {
        let Some(info) = self.doc().resolve(device) else {
            return false;
        };
        if info.type_id != SAMPLER {
            let name = info.name.clone();
            self.reject(format!(
                "{name}: SFZ and SoundFont files load into a sampler"
            ));
            return false;
        }
        let Some(path) = instruments.first().cloned() else {
            return false;
        };
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => {
                self.reject(e);
                return true;
            }
        };
        let rate = self.doc().sample_rate;
        let threshold = self.stream_threshold();
        let started = spawn_instrument(
            self.audio.tx.clone(),
            self.audio.generation,
            bundle,
            rate,
            threshold,
            device.clone(),
            path.clone(),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                let extra = instruments.len() - 1 + other_files;
                if extra > 0 {
                    self.report(format!(
                        "{}: an instrument file loads alone; ignored {extra} other file(s)",
                        display_name(&path)
                    ));
                }
            }
            Err(e) => self.reject(format!("instrument import: cannot start a worker: {e}")),
        }
        true
    }

    /// Applies a finished instrument load: the assets are registered and the sampler's zones
    /// replaced in one undo step.
    pub(super) fn apply_instrument(
        &mut self,
        device: DeviceKey,
        path: &Path,
        result: Result<ImportedInstrument, String>,
    ) {
        let file = display_name(path);
        let imported = match result {
            Ok(i) => i,
            Err(e) => return self.reject(format!("{file}: {e}")),
        };
        let Some(sampler) = self.doc().resolve(&device) else {
            return self.reject(format!("{device} was removed; {file} was not loaded"));
        };
        let name = sampler.name.clone();
        for loaded in &imported.loaded {
            self.insert_loaded(loaded);
            let Some(info) = &loaded.info else { continue };
            let registered = self.apply_command(Command::RegisterAsset {
                hash: info.hash.clone(),
                meta: AssetMeta {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                    frames: info.frames,
                    original_name: info.original_name.clone(),
                },
            });
            if registered.is_err() {
                // Already reported.
                return;
            }
        }
        let count = imported.zones.len();
        let applied = self.apply_command(Command::SetZones {
            device,
            zones: imported.zones,
            merge: None,
        });
        if applied.is_err() {
            return;
        }
        let mut message = format!("{name}: loaded {} ({count} zones)", imported.name);
        if !imported.warnings.is_empty() {
            message.push_str(" \u{b7} ");
            message.push_str(&imported.warnings.join(" \u{b7} "));
        }
        self.report(message);
    }
}
