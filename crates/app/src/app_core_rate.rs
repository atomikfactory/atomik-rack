//! The project's sample rate and channel counts in the app core.
//!
//! * **New at a rate** ([`AppCore::new_document_at`]) is the starter project with its rate set.
//! * **Convert** ([`AppCore::convert_sample_rate`]) resamples every asset the project uses on a
//!   worker (`atomik-audio-file`'s sinc resampler, into the project's bundle, the originals stay),
//!   then registers the copies (not an undo step) and applies one `ConvertSampleRate` step that
//!   switches the rate and points the clips and zones at the copies. Undo brings the old rate and
//!   audio back: the originals are still loaded. Clip positions are in ticks and do not move.
//! * **Channel counts** are plain commands ([`AppCore::set_input_channels`],
//!   [`AppCore::set_output_channels`]).
//!
//! A new rate or channel count rebuilds the engine (see `rebuild_engine_in_place`): the transport
//! comes back at the tick it was at and keeps playing, but effect tails do not survive.

use super::AppCore;
use crate::audio_import::Loaded;
use crate::audio_import::spawn_convert;
use crate::sink::EngineSink;
use atomik_model::{AssetMeta, Command, SAMPLE_RATES};

impl<S: EngineSink> AppCore<S> {
    /// Replaces the project with the starter project at `rate` Hz (44100, 48000 or 96000).
    pub fn new_document_at(&mut self, rate: u32) {
        let mut doc = super::new_project_doc();
        if SAMPLE_RATES.contains(&rate) {
            doc.sample_rate = rate;
        }
        self.replace(doc, None);
    }

    /// Sets the interface's input channel count (one undo step); a rejection is reported.
    pub fn set_input_channels(&mut self, n: u16) {
        if self.doc().input_channels == n {
            return;
        }
        if let Err(e) = self.apply_command(Command::SetInputChannels(n)) {
            self.reject(e.to_string());
        }
    }

    /// Sets the interface's output channel count (one undo step); a rejection is reported.
    pub fn set_output_channels(&mut self, n: u16) {
        if self.doc().output_channels == n {
            return;
        }
        if let Err(e) = self.apply_command(Command::SetOutputChannels(n)) {
            self.reject(e.to_string());
        }
    }

    /// Moves the project to `rate` Hz in one undo step. The audio is resampled on a worker, so
    /// the step lands a moment later (see the module docs). Reports an error and changes nothing
    /// when the rate is unsupported, equal to the current one, a conversion is already running
    /// or an asset the project uses is missing.
    pub fn convert_sample_rate(&mut self, rate: u32) {
        if !SAMPLE_RATES.contains(&rate) {
            return self.reject(format!("{rate} Hz is not a supported sample rate"));
        }
        if rate == self.doc().sample_rate {
            return self.reject(format!("the project already runs at {rate} Hz"));
        }
        if self.audio.converting.is_some() {
            return self.reject("a sample rate conversion is already running".to_owned());
        }
        let used: Vec<(String, String)> = self
            .doc()
            .referenced_assets()
            .into_iter()
            .map(|hash| {
                let name = self.asset_name(&hash);
                (hash, name)
            })
            .collect();
        if let Some((_, name)) = used
            .iter()
            .find(|(h, _)| self.audio.missing.iter().any(|m| &m.hash == h))
        {
            return self.reject(format!(
                "audio {name} is missing: relink it before converting the sample rate"
            ));
        }
        if used.is_empty() {
            return self.apply_conversion(rate, Vec::new());
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => return self.reject(e),
        };
        let threshold = self.stream_threshold();
        match spawn_convert(
            self.audio.tx.clone(),
            self.audio.generation,
            bundle,
            rate,
            threshold,
            used,
        ) {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.converting = Some(rate);
                log::info!("converting the project to {rate} Hz");
            }
            Err(e) => self.reject(format!(
                "sample rate conversion: cannot start the worker: {e}"
            )),
        }
    }

    /// The worker finished: registers the copies and applies the conversion.
    pub(super) fn apply_converted(
        &mut self,
        rate: u32,
        result: Result<Vec<(String, Loaded)>, String>,
    ) {
        self.audio.converting = None;
        let copies = match result {
            Ok(c) => c,
            Err(e) => return self.reject(format!("sample rate conversion: {e}")),
        };
        let mut pairs = Vec::with_capacity(copies.len());
        for (from, loaded) in copies {
            let Some(info) = loaded.info.clone() else {
                return self.reject("sample rate conversion: a copy has no asset info".to_owned());
            };
            self.insert_loaded(&loaded);
            if !self.doc().assets.contains_key(&info.hash) {
                let meta = AssetMeta {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                    frames: info.frames,
                    original_name: info.original_name.clone(),
                };
                if let Err(e) = self.apply_command(Command::RegisterAsset {
                    hash: info.hash.clone(),
                    meta,
                }) {
                    return self.reject(format!("sample rate conversion: {e}"));
                }
            }
            pairs.push((from, info.hash));
        }
        self.apply_conversion(rate, pairs);
    }

    fn apply_conversion(&mut self, rate: u32, assets: Vec<(String, String)>) {
        match self.apply_command(Command::ConvertSampleRate { rate, assets }) {
            Ok(()) => log::info!("project converted to {rate} Hz"),
            Err(e) => self.reject(format!("sample rate conversion: {e}")),
        }
    }
}
