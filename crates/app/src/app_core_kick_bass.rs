//! The kick and bass check in the app: the two picked tracks, the worker that runs the check, and
//! applying a fix (see `kick_bass.rs` for what the check does).
//!
//! * The check runs on the worker `atomik-kick-bass` on a copy of the song; the song, the
//!   selection and the undo history are never touched by it. A new check, a cancel or closing
//!   the app stops it.
//! * The result belongs to the song as it was when the check started
//!   ([`Document::revision`]): once the song changes, the fixes are not offered any more (the
//!   panel says to check again), so a fix is never applied on numbers that no longer hold.
//! * Applying a fix runs its steps on the open song as **one undo step** and says in the status
//!   line what it did; the check then runs again, so the strip shows the song as it is now.

use super::*;
use crate::kick_bass::{
    self as kb, Analysis, Fix, MIN_GAIN, Setup, Steps, analyse, apply_fix, registry_for,
};
use atomik_dsp::kick_bass::Check;
use atomik_ui::{FixView, KickBassInfo, RunView, StripHit, StripView};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The worker's answer: the analysis, `None` when it was cancelled, or why it failed.
type Answer = Result<Option<Analysis>, String>;

/// A check that is running.
#[derive(Debug)]
struct Job {
    answers: Receiver<Answer>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    /// The revision of the song the check was started on.
    revision: u64,
}

/// What the app keeps of the kick and bass check.
#[derive(Debug, Default)]
pub(super) struct KickBass {
    kick: Option<DeviceKey>,
    bass: Option<DeviceKey>,
    job: Option<Job>,
    /// The last result, with the revision of the song it is about.
    pub(super) result: Option<(Analysis, u64)>,
    failure: Option<String>,
}

/// The open song as the steps of a fix: each command goes through the app's own apply path, and
/// all but the first join the undo step of the first.
struct Live<'a, S: EngineSink>(&'a mut AppCore<S>);

impl<S: EngineSink> Steps for Live<'_, S> {
    fn doc(&self) -> &RackDoc {
        self.0.doc()
    }

    fn registry(&self) -> &Registry {
        &self.0.registry
    }

    fn run(&mut self, command: Command, join: bool) -> Result<(), String> {
        self.0.join_next = join;
        let applied = self.0.apply_command(command);
        self.0.join_next = false;
        applied.map_err(|e| e.to_string())
    }
}

fn strip_of(check: &Check) -> StripView {
    let total = check.frames.max(1) as f32;
    StripView {
        hits: check
            .hits
            .iter()
            .map(|h| StripHit {
                start: h.frame as f32 / total,
                len: h.frames as f32 / total,
                delta_db: h.delta_db,
            })
            .collect(),
    }
}

fn summary_of(check: &Check) -> String {
    if check.hits.is_empty() {
        return "No kick hits were found in the checked stretch".to_owned();
    }
    format!(
        "{} hits: {} cancel, {} build up. Together {:+.1} dB against the power sum of the two \
         (worst hit {:+.1} dB)",
        check.hits.len(),
        check.cancelling(),
        check.building(),
        check.overall_db,
        check.worst_db
    )
}

fn pitches_of(check: &Check) -> String {
    let (Some(k), Some(b), Some(semitones)) = (check.kick_hz, check.bass_hz, check.semitones)
    else {
        return String::new();
    };
    let base = format!("Kick {k:.1} Hz, bass {b:.1} Hz: {semitones:.2} semitones apart");
    match check.beat_hz {
        Some(beat) if check.beating => format!(
            "{base}. They beat at {beat:.1} Hz: the low end swells and fades from note to note"
        ),
        _ if semitones < atomik_dsp::kick_bass::SAME_SEMITONES + 0.1 => {
            format!("{base}: the same pitch, so only their phase decides how they add")
        }
        _ => base,
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The tracks that can be picked: those with a mixer channel, in song order, with their
    /// names.
    pub fn kick_bass_tracks(&self) -> Vec<(DeviceKey, String)> {
        let doc = self.doc();
        doc.console
            .channels
            .iter()
            .filter_map(|c| c.source.as_ref())
            .filter(|source| doc.song.track(source).is_some())
            .map(|source| {
                let name = doc
                    .device(source)
                    .map(|d| d.name.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| source.0.clone());
                (source.clone(), name)
            })
            .collect()
    }

    /// Opens the check: a track that is no longer there is forgotten, and a missing choice is
    /// guessed from the names (a track called kick, a track called bass or sub).
    pub fn kick_bass_open(&mut self) {
        let tracks = self.kick_bass_tracks();
        let has = |k: &Option<DeviceKey>| {
            k.as_ref()
                .is_some_and(|k| tracks.iter().any(|(t, _)| t == k))
        };
        if !has(&self.kick_bass.kick) {
            self.kick_bass.kick = None;
        }
        if !has(&self.kick_bass.bass) {
            self.kick_bass.bass = None;
        }
        let guess = |words: &[&str]| {
            tracks
                .iter()
                .find(|(_, name)| {
                    let lower = name.to_lowercase();
                    words.iter().any(|w| lower.contains(w))
                })
                .map(|(k, _)| k.clone())
        };
        if self.kick_bass.kick.is_none() {
            self.kick_bass.kick = guess(&["kick", "bd"]);
        }
        if self.kick_bass.bass.is_none() {
            self.kick_bass.bass =
                guess(&["bass", "sub"]).filter(|b| Some(b) != self.kick_bass.kick.as_ref());
        }
    }

    /// Steps the kick (or bass) pick by `step` tracks, wrapping round. Any earlier result is
    /// dropped.
    pub fn kick_bass_pick(&mut self, bass: bool, step: i32) {
        if self.kick_bass.job.is_some() {
            return;
        }
        let tracks = self.kick_bass_tracks();
        if tracks.is_empty() {
            return;
        }
        let current = if bass {
            &self.kick_bass.bass
        } else {
            &self.kick_bass.kick
        };
        let at = current
            .as_ref()
            .and_then(|k| tracks.iter().position(|(t, _)| t == k));
        let n = tracks.len() as i32;
        let next = match at {
            Some(i) => (i as i32 + step).rem_euclid(n),
            None if step < 0 => n - 1,
            None => 0,
        } as usize;
        let key = Some(tracks[next].0.clone());
        if bass {
            self.kick_bass.bass = key;
        } else {
            self.kick_bass.kick = key;
        }
        self.kick_bass.result = None;
        self.kick_bass.failure = None;
    }

    /// Starts the check of the picked tracks on a worker.
    pub fn kick_bass_run(&mut self) {
        self.kick_bass.failure = None;
        let (Some(kick), Some(bass)) = (self.kick_bass.kick.clone(), self.kick_bass.bass.clone())
        else {
            self.kick_bass.failure = Some("Pick a kick track and a bass track first".to_owned());
            return;
        };
        if kick == bass {
            self.kick_bass.failure = Some("The kick and the bass must be two tracks".to_owned());
            return;
        }
        if let Some(job) = &self.kick_bass.job {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.kick_bass.result = None;
        let doc = self.doc().clone();
        let root = self.bundle_root().map(Path::to_path_buf);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let (send, answers) = channel();
        let setup = Setup { kick, bass };
        let worker = {
            let (cancel, progress) = (cancel.clone(), progress.clone());
            std::thread::Builder::new()
                .name("atomik-kick-bass".into())
                .spawn(move || {
                    let report = |f: f32| progress.store(f.to_bits(), Ordering::Relaxed);
                    let answer = catch_unwind(AssertUnwindSafe(|| {
                        let registry = registry_for(&doc)?;
                        analyse(&doc, &registry, root.as_deref(), &setup, (&cancel, &report))
                    }))
                    .unwrap_or_else(|_| Err("a device failed while the check rendered".to_owned()));
                    let _ = send.send(answer);
                })
        };
        match worker {
            Ok(_) => {
                self.kick_bass.job = Some(Job {
                    answers,
                    cancel,
                    progress,
                    revision: self.document.revision(),
                });
            }
            Err(e) => {
                self.kick_bass.failure = Some(format!("The check could not start: {e}"));
            }
        }
    }

    /// Stops the check that runs; its answer is dropped.
    pub fn kick_bass_cancel(&mut self) {
        if let Some(job) = self.kick_bass.job.take() {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Whether a check is running.
    pub fn kick_bass_running(&self) -> bool {
        self.kick_bass.job.is_some()
    }

    /// One frame: takes the worker's answer when it is there.
    pub(super) fn poll_kick_bass(&mut self) {
        let Some(job) = &self.kick_bass.job else {
            return;
        };
        let answer = match job.answers.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the check stopped unexpectedly".to_owned()),
        };
        let revision = job.revision;
        self.kick_bass.job = None;
        match answer {
            Ok(Some(analysis)) => self.kick_bass.result = Some((analysis, revision)),
            Ok(None) => {}
            Err(why) => self.kick_bass.failure = Some(why),
        }
    }

    /// Applies fix number `n` of the last result as one undo step (see the module docs), says in
    /// the status line what it did, and checks again.
    pub fn kick_bass_apply(&mut self, n: usize) {
        let Some((analysis, revision)) = self.kick_bass.result.clone() else {
            return self.report("Run the kick and bass check first");
        };
        if revision != self.document.revision() {
            return self.report("The song changed since the check: check again before a fix");
        }
        let Some(trial) = analysis.trials.get(n) else {
            return;
        };
        let rate = self.doc().sample_rate;
        let fix: Fix = trial.fix.clone();
        let applied = apply_fix(&mut Live(self), &analysis.setup, &fix, rate);
        match applied {
            Ok(message) => {
                self.report(message);
                self.kick_bass_run();
            }
            Err(why) => self.report(format!("Kick and Bass: {why}")),
        }
    }

    /// What the panel draws now.
    pub fn kick_bass_info(&self) -> KickBassInfo {
        let tracks = self.kick_bass_tracks();
        let index = |k: &Option<DeviceKey>| {
            k.as_ref()
                .and_then(|k| tracks.iter().position(|(t, _)| t == k))
        };
        let mut info = KickBassInfo {
            tracks: tracks.iter().map(|(_, n)| n.clone()).collect(),
            kick: index(&self.kick_bass.kick),
            bass: index(&self.kick_bass.bass),
            ..KickBassInfo::default()
        };
        if let Some(job) = &self.kick_bass.job {
            info.run = RunView::Running(f32::from_bits(job.progress.load(Ordering::Relaxed)));
        } else if let Some(why) = &self.kick_bass.failure {
            info.run = RunView::Failed(why.clone());
        }
        if let Some((analysis, revision)) = &self.kick_bass.result
            && self.kick_bass.job.is_none()
        {
            info.run = RunView::Done;
            info.strip = Some(strip_of(&analysis.base));
            info.pitches = pitches_of(&analysis.base);
            info.beating = analysis.base.beating;
            info.notes.clone_from(&analysis.notes);
            if *revision != self.document.revision() {
                info.summary = "The song changed since this check: check again".to_owned();
            } else {
                info.summary = summary_of(&analysis.base);
                info.fixes = analysis
                    .trials
                    .iter()
                    .map(|t| FixView {
                        label: t.fix.label(analysis.base.sample_rate),
                        strip: strip_of(&t.check),
                        gain: format!("{:+.1} dB", t.check.overall_db - analysis.base.overall_db),
                    })
                    .collect();
                if analysis.trials.is_empty() && kb::needs_fix(&analysis.base) {
                    info.notes.push(format!(
                        "No change that was tried gains {MIN_GAIN:.1} dB or more"
                    ));
                } else if !kb::needs_fix(&analysis.base) {
                    info.notes
                        .push("Nothing cancels and the fundamentals do not beat".to_owned());
                }
            }
        }
        info
    }
}
