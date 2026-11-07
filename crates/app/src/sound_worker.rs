//! The Sound Analyst worker: decodes an audio file, measures it and tries a patch on its own
//! thread ([`atomik_dsp::sound_analysis`], [`atomik_render::sound_analyst`]); see
//! [`crate::AppCore::analyse_sound_file`].
//!
//! Like the bass-read worker it owns what it needs (its own device registry) and talks to the UI
//! thread only through [`Msg::Sound`]; progress and cancel are two shared atomics. The renders
//! are offline patch renders on this one thread (no engine workers: the live engine has the
//! cores). There are two jobs: a file to analyse, which also renders the proposed patch and
//! compares it ([`Work::File`]), and a patch that was made, rendered again against the analysed
//! sound ([`Work::Match`]).

use crate::audio_import::Msg;
use atomik_audio_file::decode_file_with;
use atomik_dsp::sound_analysis::{Distances, SoundProfile, analyse};
use atomik_render::sound_analyst::{Fit, Proposal, fit, measure};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// The most samples (all channels) a file may hold to be analysed.
pub const MAX_SAMPLES: u64 = 48_000 * 2 * 120;

/// One job.
pub(crate) enum Work {
    /// Decode, analyse and fit a file.
    File(PathBuf),
    /// Render `proposal` and compare it with `target`.
    Match {
        /// The analysed sound.
        target: Box<SoundProfile>,
        /// The patch to render, with the params it has now.
        proposal: Box<Proposal>,
    },
}

/// What a file analysis found.
#[derive(Debug, Clone)]
pub struct Analysed {
    /// The name of the file.
    pub name: String,
    /// What was measured.
    pub profile: SoundProfile,
    /// The proposed patch, its render and the distances.
    pub fit: Fit,
}

/// What a job produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A file was analysed.
    Analysed(Box<Analysed>),
    /// A patch was rendered against the target.
    Matched {
        /// The profile of the render.
        rendered: Box<SoundProfile>,
        /// How far it is from the target.
        distances: Distances,
    },
}

/// Runs `work`. `None`: the cancel flag stopped it.
fn run(work: Work, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<Option<Outcome>, String> {
    let registry = atomik_render::default_registry();
    match work {
        Work::File(path) => {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let decoded = decode_file_with(&path, MAX_SAMPLES).map_err(|e| e.to_string())?;
            progress(0.2);
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let audio = decoded.audio;
            let planes: Vec<&[f32]> = audio.channels.iter().map(Vec::as_slice).collect();
            let profile = analyse(&planes, audio.sample_rate).map_err(|e| e.to_string())?;
            progress(0.5);
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let fit = fit(&profile, &registry).map_err(|e| e.to_string())?;
            progress(1.0);
            Ok(Some(Outcome::Analysed(Box::new(Analysed {
                name,
                profile,
                fit,
            }))))
        }
        Work::Match { target, proposal } => {
            let rendered = measure(&proposal, &registry).map_err(|e| e.to_string())?;
            progress(1.0);
            let distances = atomik_dsp::sound_analysis::compare(&target, &rendered);
            Ok(Some(Outcome::Matched {
                rendered: Box::new(rendered),
                distances,
            }))
        }
    }
}

/// Sends the result of a job, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<Outcome>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Sound {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the analysis stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that runs `work` and sends [`Msg::Sound`]; `cancel` stops it early and
/// `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-sound-analyst".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                generation,
                job,
                sent: false,
            };
            let report = |p: f32| progress.store(p.to_bits(), Ordering::Relaxed);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(work, &cancel, &report)
            }))
            .unwrap_or_else(|_| Err("a device failed while the sound was analysed".to_owned()));
            guard.send(result);
        })
        .map(|_| ())
}
