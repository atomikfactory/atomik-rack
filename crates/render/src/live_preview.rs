//! A scene that is played by hand: the scratch project of a library preview built into an engine
//! of its own that the caller drives block by block, with notes sent while it renders.
//!
//! The song of the project is never played (the transport stands still): the notes of the
//! caller go to the notes input of the first track's device, the one the phrase of an offline
//! preview plays, so the keys of a keyboard sound the patch through the same instrument,
//! players, effects and channel as the phrase does. The devices are built like a bounce builds
//! them (the same expansion, sample maps and pattern data) with no plugins unless the caller
//! supplies them, and the engine has no workers: everything runs on the thread that calls
//! [`LivePreview::render`], never an audio thread.

use crate::build::{SuppliedDevices, build_with_devices};
use crate::project::{
    console_options, expanded_ids, player_data_maps, sampler_maps, song_input_with_assets,
};
use crate::{RenderError, flat};
use atomik_core::NoteKind;
use atomik_device_sdk::{Device, Registry};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, EngineOptions, ScheduledNote, WorkerConfig,
    new_engine_with_options,
};
use atomik_model::{DeviceKey, RackDoc};
use std::collections::BTreeMap;
use std::path::Path;

/// The devices a caller supplies for keys of the project (plugins it hosts itself).
pub type LiveDevices = BTreeMap<DeviceKey, Box<dyn Device>>;

/// A scratch project playing live.
pub struct LivePreview {
    engine: Engine,
    handle: EngineHandle,
    entry: (DeviceInstanceId, atomik_device_sdk::SocketId),
    rate: u32,
    inputs: Vec<Vec<f32>>,
    out: Vec<Vec<f32>>,
}

impl std::fmt::Debug for LivePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivePreview")
            .field("entry", &self.entry)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl LivePreview {
    /// Builds the engine of `doc` (audio of its pool is read from `bundle_root`, as a bounce
    /// does). `supplied` holds devices built by the caller for keys of `doc` (a plugin).
    ///
    /// # Errors
    /// The audio of the pool cannot be loaded, the rack does not build, or the project has no
    /// track on a device that takes notes.
    pub fn build(
        doc: &RackDoc,
        registry: &Registry,
        bundle_root: Option<&Path>,
        supplied: LiveDevices,
    ) -> Result<Self, RenderError> {
        let bundle = bundle_root.and_then(|root| atomik_audio_file::Bundle::open(root).ok());
        let (assets, _) = crate::bundle::load_song_assets_counted(doc, bundle.as_ref())
            .map_err(RenderError::Project)?;
        let expanded = doc.expand(registry);
        let (patch, patch_ids) = flat::expanded_patch(doc, &expanded);
        let patch_id =
            |key: &DeviceKey| patch_ids.get(key).cloned().unwrap_or_else(|| key.0.clone());
        let mut options = console_options(doc, registry);
        let (maps, _) = sampler_maps(&expanded, registry, &assets);
        options.samples = maps
            .into_iter()
            .map(|(k, m)| (patch_id(&DeviceKey(k)), m))
            .collect();
        options.player_data = player_data_maps(doc, registry, &expanded)
            .into_iter()
            .map(|(k, d)| (patch_id(&DeviceKey(k)), d))
            .collect();
        let supplied: SuppliedDevices = supplied
            .into_iter()
            .map(|(key, device)| (patch_id(&key), device))
            .collect();
        let built = build_with_devices(&patch, registry, &options, supplied)?;
        let ids = expanded_ids(&expanded);
        let input = song_input_with_assets(doc, registry, |k| ids.get(k).copied(), &assets);
        let entry = input
            .tracks
            .first()
            .map(|t| (t.device, t.notes_socket))
            .ok_or_else(|| RenderError::Project("it has no device that takes notes".into()))?;
        let rate = doc.sample_rate;
        let channels = usize::from(built.config.output_channels);
        let inputs = vec![Vec::new(); usize::from(built.config.input_channels)];
        let engine_options = EngineOptions {
            workers: WorkerConfig {
                threads: 0,
                on_thread_start: None,
            },
            worker_linger_spins: 0,
            realtime: false,
        };
        let (engine, mut handle) = new_engine_with_options(built.config, engine_options);
        handle
            .submit_plan(built.plan)
            .map_err(|e| RenderError::Project(format!("engine: {e}")))?;
        Ok(Self {
            engine,
            handle,
            entry,
            rate,
            inputs,
            out: vec![Vec::new(); channels],
        })
    }

    /// The sample rate of what [`Self::render`] makes.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Sends a note event to the entry device; it sounds in the next block rendered. A full
    /// queue drops the event (a held key's off is sent again by an all-notes-off).
    pub fn note(&mut self, kind: NoteKind) {
        let note = ScheduledNote {
            frame: self.engine.host_frames(),
            device: self.entry.0,
            socket: self.entry.1,
            kind,
        };
        // A full queue means the engine fell far behind the caller: the event is lost.
        let _ = self.handle.send_note(note);
    }

    /// What the last [`Self::render`] made: one plane per output channel.
    pub fn output(&self) -> &[Vec<f32>] {
        &self.out
    }

    /// Renders the next `frames` frames: one plane per output channel.
    ///
    /// # Errors
    /// A device panicked while rendering (the engine contained it and silenced it).
    pub fn render(&mut self, frames: usize) -> Result<&[Vec<f32>], RenderError> {
        for plane in &mut self.inputs {
            plane.clear();
            plane.resize(frames, 0.0);
        }
        for plane in &mut self.out {
            plane.clear();
            plane.resize(frames, 0.0);
        }
        let ins: Vec<&[f32]> = self.inputs.iter().map(Vec::as_slice).collect();
        let mut outs: Vec<&mut [f32]> = self.out.iter_mut().map(Vec::as_mut_slice).collect();
        self.engine.render(&ins, &mut outs);
        self.handle.collect_garbage();
        if self.engine.fault().is_some() {
            return Err(RenderError::Project(
                "a device failed while the preview played".into(),
            ));
        }
        Ok(&self.out)
    }
}
