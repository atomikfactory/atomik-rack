//! The stem worker: reads the audio file of a clip, separates it and stores each stem in the project's bundle (see
//! [`crate::AppCore::separate_stems`]).
//!
//! It owns what it needs (a clone of the bundle, the backend) and talks to the UI thread only
//! through [`Msg::Stems`]; the progress is a few shared atomics ([`StemShared`]) and cancel is
//! one more. The whole run is off the UI thread and the audio thread.
//!
//! The stems are made from the whole file of the clip, so each stem file is as long as the clip's
//! file and the clip that plays it keeps its offset, warp and everything else. Audio at another
//! rate than the models is resampled to theirs and back; a mono file gives mono stems.

use crate::audio_import::{Loaded, Msg, load_asset};
use crate::stems_link::StemBackend;
use atomik_audio_file::{AudioData, Bundle, resample};
use atomik_stems::{Stem, StemError, StemSink};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// What the worker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Reading the file of the clip.
    Read = 0,
    /// Running the split.
    Separate = 1,
    /// Writing the stems.
    Store = 2,
}

/// The progress the UI thread reads once a frame.
#[derive(Debug)]
pub(crate) struct StemShared {
    phase: AtomicU8,
    fraction: AtomicU32,
}

impl StemShared {
    /// A progress that has not started.
    pub(crate) fn new() -> Self {
        Self {
            phase: AtomicU8::new(Phase::Read as u8),
            fraction: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// What the worker is doing.
    pub(crate) fn phase(&self) -> Phase {
        match self.phase.load(Ordering::Relaxed) {
            0 => Phase::Read,
            1 => Phase::Separate,
            _ => Phase::Store,
        }
    }

    /// How far the separation is, `0.0..=1.0`.
    pub(crate) fn fraction(&self) -> f32 {
        f32::from_bits(self.fraction.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    fn set_fraction(&self, p: f32) {
        self.fraction.store(p.to_bits(), Ordering::Relaxed);
    }
}

/// One separation.
pub(crate) struct Work {
    /// The project bundle (or the session's scratch bundle): the clip's file is read from it and
    /// the stems are stored in it.
    pub bundle: Bundle,
    /// The asset id of the clip's file.
    pub asset: String,
    /// The name the stem files get in the pool: `<this> <Stem>.wav`.
    pub name: String,
    /// The stems to keep.
    pub wanted: Vec<Stem>,
    /// The decoded size above which an asset streams.
    pub threshold: u64,
    /// The separation.
    pub backend: Arc<dyn StemBackend>,
}

/// A finished separation: each stem stored and loaded.
pub(crate) struct Done {
    /// The stems in the order asked, each as an import gives it (`info` is set).
    pub stems: Vec<(Stem, Loaded)>,
}

/// Collects the pieces the separation hands over.
struct Collect {
    parts: Vec<(Stem, [Vec<f32>; 2])>,
}

impl StemSink for Collect {
    fn write(&mut self, stem: Stem, left: &[f32], right: &[f32]) -> Result<(), StemError> {
        if let Some((_, [l, r])) = self.parts.iter_mut().find(|(s, _)| *s == stem) {
            l.extend_from_slice(left);
            r.extend_from_slice(right);
        }
        Ok(())
    }
}

/// Makes `x` exactly `len` samples long (a resampler may be a sample off).
fn fit(mut x: Vec<f32>, len: usize) -> Vec<f32> {
    x.resize(len, 0.0);
    x
}

/// Runs `work`. `None`: the user cancelled.
fn run(work: Work, shared: &StemShared, cancel: &AtomicBool) -> Result<Option<Done>, String> {
    let Work {
        mut bundle,
        asset,
        name,
        wanted,
        threshold,
        backend,
    } = work;
    shared.set_phase(Phase::Read);
    let data = bundle.load(&asset).map_err(|e| e.to_string())?;
    let source_rate = data.sample_rate;
    let frames = data.channels.first().map_or(0, Vec::len);
    let mono = data.channels.len() == 1;
    let rate = backend.rate();
    let stereo: Vec<Vec<f32>> = if mono {
        vec![data.channels[0].clone(), data.channels[0].clone()]
    } else {
        data.channels.iter().take(2).cloned().collect()
    };
    drop(data);
    let model_input = if source_rate == rate {
        AudioData::new(rate, stereo).map_err(|e| e.to_string())?
    } else {
        let input = AudioData::new(source_rate, stereo).map_err(|e| e.to_string())?;
        resample(&input, rate).map_err(|e| e.to_string())?
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    shared.set_phase(Phase::Separate);
    let mut collect = Collect {
        parts: wanted
            .iter()
            .map(|&s| (s, [Vec::new(), Vec::new()]))
            .collect(),
    };
    let result = backend.separate(
        &model_input.channels[0],
        &model_input.channels[1],
        &wanted,
        &mut collect,
        cancel,
        &mut |p| shared.set_fraction(p),
    );
    drop(model_input);
    match result {
        Ok(()) => {}
        Err(StemError::Cancelled) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    shared.set_phase(Phase::Store);
    let mut stems = Vec::new();
    for (stem, [l, r]) in collect.parts {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let planes = if mono { vec![l] } else { vec![l, r] };
        let mut audio = AudioData::new(rate, planes).map_err(|e| e.to_string())?;
        if source_rate != rate {
            audio = resample(&audio, source_rate).map_err(|e| e.to_string())?;
        }
        // The stem is exactly as long as the file it came from.
        let planes: Vec<Vec<f32>> = audio.channels.into_iter().map(|c| fit(c, frames)).collect();
        let audio = AudioData::new(source_rate, planes).map_err(|e| e.to_string())?;
        let file = format!("{name} {}.wav", stem.label());
        let info = bundle
            .add_recording(&audio, &file)
            .map_err(|e| format!("cannot store the {} stem: {e}", stem.label()))?;
        let mut loaded = load_asset(&bundle, &info.hash, threshold)?;
        loaded.info = Some(info);
        stems.push((stem, loaded));
    }
    Ok(Some(Done { stems }))
}

/// Sends the result, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<Done>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Stems {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the separation stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that separates `work` and sends [`Msg::Stems`]; `cancel` stops it early
/// and `shared` tells how far it is.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, shared): (Arc<AtomicBool>, Arc<StemShared>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-stems".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                generation,
                job,
                sent: false,
            };
            let result = run(work, &shared, &cancel);
            guard.send(result);
        })
        .map(|_| ())
}
