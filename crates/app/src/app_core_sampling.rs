//! Sampling into devices: the SAMPLE button of the sampler, drum machine, loop slicer and
//! granular sampler records from an interface input and lands the result in the device.
//!
//! # Flow
//!
//! * **Start** ([`UiAction::SampleInto`], first press): an engine record tap opens on the
//!   sampling input (a stereo pair or one channel of the interface, [`SamplingInput`]) with a
//!   preallocated ring of [`SAMPLE_SECONDS`] seconds; the audio thread only copies into it (a
//!   ring that overflows counts the dropped frames, like a take). Each UI frame
//!   ([`AppCore::feed_sampling`]) moves the ring into a buffer reserved for the whole length up
//!   front. The input goes through no gain strip: the sample is what the interface delivers.
//! * **Stop** (second press, or the 30 s limit): the tap is stopped and emptied, and the audio
//!   from the first frame at or above [`TRIGGER_DB`] on is kept; the silence before it is cut
//!   off by the detector of strip silence ([`atomik_model::leading_silence`]). A recording that
//!   never rose above the threshold changes nothing and says so. The rest is stored in the
//!   project bundle as one asset (`Sample N`), registered in the pool and handed to
//!   [`AppCore::finish_sample_import`] like a dropped file: **one `SetZones` step** (a sampler
//!   maps it from `at_key`, a drum machine puts it on its pad, a slicer slices it as a loop, a
//!   granular sampler takes it as its sample). Undo removes the zone and leaves the asset in the
//!   pool for redo; a save prunes it once the history no longer needs it.
//! * **Cancel** ([`AppCore::cancel_sampling`], Esc, an engine restart, a take starting): the tap
//!   is dropped and nothing is written.
//!
//! One tap serves the engine at a time, so sampling and recording audio tracks exclude each
//! other. The input pair persists for the session ([`UiAction::CycleSamplingInput`] picks the
//! next); the stored asset is written on the UI thread at stop (at most 30 s of audio).

use super::audio::SampleImport;
use super::*;
use crate::audio_import::Loaded;
use atomik_engine::{RecordTap, SongAsset, TapChannel, TapControls};
use atomik_model::{AssetMeta, leading_silence};

/// The longest sample, in seconds: the length of the tap's ring.
pub const SAMPLE_SECONDS: usize = 30;

/// A recording starts at the first frame this loud (dBFS): above the noise floor of an
/// interface, below anything played.
pub const TRIGGER_DB: f32 = -60.0;

/// Which interface inputs the SAMPLE button records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplingInput {
    /// The first channel (0-based).
    pub first: u16,
    /// Only that channel (a mono sample), else it and the next.
    pub mono: bool,
}

impl SamplingInput {
    /// The host channels recorded.
    fn channels(self) -> Vec<usize> {
        let first = usize::from(self.first);
        if self.mono {
            vec![first]
        } else {
            vec![first, first + 1]
        }
    }

    /// The text that names it: `Inputs 1-2` or `Input 3`.
    pub fn label(self) -> String {
        if self.mono {
            format!("Input {}", self.first + 1)
        } else {
            format!("Inputs {}-{}", self.first + 1, self.first + 2)
        }
    }
}

/// The inputs an interface with `channels` inputs offers: its stereo pairs (1-2, 3-4, ...), then
/// each channel alone.
pub fn sampling_inputs(channels: u16) -> Vec<SamplingInput> {
    let pairs = (0..channels.saturating_sub(1))
        .step_by(2)
        .map(|first| SamplingInput { first, mono: false });
    let singles = (0..channels).map(|first| SamplingInput { first, mono: true });
    pairs.chain(singles).collect()
}

/// A running sampling.
struct Running {
    device: DeviceKey,
    at_key: Option<u8>,
    tap: RecordTap,
    planes: Vec<Vec<f32>>,
    limit: usize,
}

/// The sampling state of the core.
#[derive(Default)]
pub(super) struct Sampling {
    running: Option<Running>,
    /// The chosen input (`None`: the first the interface offers).
    input: Option<SamplingInput>,
    /// Samples taken this session, for the names.
    count: u32,
    /// Sample edits whose worker has not answered, by job.
    pub(super) edits: std::collections::BTreeMap<u64, super::sample_edit::EditJob>,
    /// The job number of the next sample edit.
    pub(super) next_edit: u64,
}

impl<S: EngineSink> AppCore<S> {
    /// The device that is being sampled into, if a sampling runs.
    pub fn sampling_device(&self) -> Option<&DeviceKey> {
        self.sampling.running.as_ref().map(|r| &r.device)
    }

    /// The input the SAMPLE button records now: the chosen one if the interface still has it,
    /// else the first it offers; `None` without inputs.
    pub fn sampling_input(&self) -> Option<SamplingInput> {
        let options = sampling_inputs(self.doc().input_channels);
        self.sampling
            .input
            .filter(|i| options.contains(i))
            .or_else(|| options.first().copied())
    }

    /// Seconds recorded so far by a running sampling.
    pub fn sampling_seconds(&self) -> f64 {
        self.sampling.running.as_ref().map_or(0.0, |r| {
            r.planes.first().map_or(0, Vec::len) as f64 / f64::from(self.doc().sample_rate.max(1))
        })
    }

    /// Picks the next sampling input (a right-click on a SAMPLE button) and says which.
    pub(super) fn cycle_sampling_input(&mut self) {
        if self.sampling.running.is_some() {
            return self.reject("Stop sampling before changing the input".to_owned());
        }
        let options = sampling_inputs(self.doc().input_channels);
        let Some(current) = self.sampling_input() else {
            return self.reject("The audio interface has no inputs to sample from".to_owned());
        };
        let at = options.iter().position(|o| *o == current).unwrap_or(0);
        let next = options[(at + 1) % options.len()];
        self.sampling.input = Some(next);
        self.reject(format!("Sampling from {}", next.label()));
    }

    /// The SAMPLE button: starts a sampling into `device`, or, when one runs, stops it.
    pub(super) fn sample_into(&mut self, device: DeviceKey, at_key: Option<u8>) {
        if self.sampling.running.is_some() {
            return self.finish_sampling(false);
        }
        let takes = self
            .doc()
            .resolve(&device)
            .is_some_and(|d| self.registry.takes_samples(&d.type_id));
        if !takes {
            return self.reject(format!("{device} does not take samples"));
        }
        if self.audio.recording.is_some() || self.audio.rec_pending > 0 {
            return self.reject("Stop recording before sampling".to_owned());
        }
        let Some(input) = self.sampling_input() else {
            return self.reject("The audio interface has no inputs to sample from".to_owned());
        };
        let rate = self.doc().sample_rate.max(1) as usize;
        let limit = SAMPLE_SECONDS * rate;
        let channels: Vec<TapChannel> = input
            .channels()
            .into_iter()
            .map(|c| TapChannel::host(c, 0))
            .collect();
        let controls = Arc::new(TapControls::new(1));
        let Some(tap) = self.sink.start_audio_record(&channels, limit, controls) else {
            return self.reject(
                "Sampling needs a running audio engine (audio is off or its stream did not open)"
                    .to_owned(),
            );
        };
        let planes = (0..channels.len())
            .map(|_| Vec::with_capacity(limit))
            .collect();
        self.sampling.running = Some(Running {
            device,
            at_key,
            tap,
            planes,
            limit,
        });
    }

    /// Moves what the engine tapped into the sample buffer; the 30 s limit ends the sampling.
    /// Called every frame.
    pub(super) fn feed_sampling(&mut self) {
        let shown = self.sampling_device().cloned();
        self.view.set_sampling(shown);
        let Some(running) = &mut self.sampling.running else {
            return;
        };
        while running.tap.drain(&mut running.planes).is_some() {}
        if running.planes.first().map_or(0, Vec::len) >= running.limit {
            self.finish_sampling(true);
        }
    }

    /// Throws a running sampling away without touching the document.
    pub fn cancel_sampling(&mut self) {
        if self.sampling.running.take().is_some() {
            self.sink.stop_audio_record();
        }
    }

    /// Ends the sampling and lands the sample (module docs). `limit` says the 30 s ran out.
    fn finish_sampling(&mut self, limit: bool) {
        let Some(mut running) = self.sampling.running.take() else {
            return;
        };
        self.sink.stop_audio_record();
        while running.tap.drain(&mut running.planes).is_some() {}
        let overflow = running.tap.overflow_frames();
        let frames = running
            .limit
            .min(running.planes.first().map_or(0, Vec::len));
        for plane in &mut running.planes {
            plane.truncate(frames);
        }
        let Some(start) = leading_silence(&running.planes, TRIGGER_DB) else {
            return self.reject(if frames == 0 {
                "Nothing was recorded".to_owned()
            } else {
                "The input was silent: no sample was made".to_owned()
            });
        };
        let start = usize::try_from(start).unwrap_or(0);
        for plane in &mut running.planes {
            plane.drain(..start);
        }
        let rate = self.doc().sample_rate;
        let data = match atomik_audio_file::AudioData::new(rate, running.planes) {
            Ok(d) => d,
            Err(e) => return self.reject(format!("sampling: {e}")),
        };
        self.sampling.count += 1;
        let name = format!("Sample {}", self.sampling.count);
        let (device, at_key) = (running.device, running.at_key);
        let note = match self.store_sample(data, &name, device, at_key) {
            Ok(note) => note,
            Err(e) => return self.reject(format!("sampling: {e}")),
        };
        let mut report = Vec::new();
        if limit {
            report.push(format!("Sampling stopped at {SAMPLE_SECONDS} s"));
        }
        if overflow > 0 {
            report.push(format!(
                "sampling dropped {overflow} input frames (the app could not keep up)"
            ));
        }
        report.extend(note);
        if !report.is_empty() {
            self.reject(report.join(" \u{b7} "));
        }
    }

    /// Stores `data` in the project bundle, registers it and gives it to `device` in one
    /// `SetZones` step. Returns the note of the import (a drum machine with no empty pad).
    fn store_sample(
        &mut self,
        data: atomik_audio_file::AudioData,
        name: &str,
        device: DeviceKey,
        at_key: Option<u8>,
    ) -> Result<Option<String>, String> {
        let mut bundle = self.import_bundle()?;
        let info = bundle
            .add_recording(&data, name)
            .map_err(|e| e.to_string())?;
        let peaks = Arc::new(bundle.load_peaks(&info.hash).map_err(|e| e.to_string())?);
        let loaded = Loaded {
            hash: info.hash.clone(),
            info: Some(info.clone()),
            asset: SongAsset::memory(Arc::from(data.channels)),
            peaks,
            meta: Default::default(),
        };
        self.insert_loaded(&loaded);
        self.apply_command(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        })
        .map_err(|e| e.to_string())?;
        let import = SampleImport {
            device,
            at_key,
            ignored: Vec::new(),
            // The name that picks the root key reads digits as a key number: the asset is
            // called `Sample N`, the zone is mapped from a name that says nothing.
            files: vec![(info.hash, "Sample".to_owned(), info.frames)],
        };
        Ok(self.finish_sample_import(import))
    }
}
