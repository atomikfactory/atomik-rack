//! Stem separation in the app core: **Separate Stems** (`audio.separate_stems`) splits the
//! selected audio clip into vocals, drums, bass and other, each as a new audio track under the
//! clip.
//!
//! [`AppCore::separate_stems`] opens the dialog ([`atomik_ui::StemsDialogView`]) for the one
//! selected audio clip: which stems to make and whether the original is muted. **Separate**
//! starts the worker ([`crate::stems_worker`]). The separation is a signal-processing split that
//! needs no model, so there is nothing to download and no network is used. The reading of the
//! clip's file, the separation and the writing of the stems all run on the worker with progress
//! and **Cancel**; nothing is changed until it is done.
//!
//! When it is done the stems are in the project's bundle and registered, and one
//! `SongCommand::AddStemTracks` step adds the tracks: a copy of the clip on each (the same place,
//! offset, gain, fades and warp, playing the stem's file) and the original muted when asked. One
//! undo removes all of it. The files go at the next save unless a clip or the undo history still
//! uses them.
//!
//! The stems are made from the clip's whole file. A clip, track or project that changed while the
//! worker ran (the clip was deleted or points at another file) gets nothing added, and the
//! message says so.

use super::*;
use crate::stems_link::{DspStems, StemBackend};
use crate::stems_worker::{self, Done, Phase, StemShared, Work};
use atomik_model::{AssetMeta, StemTrack};
use atomik_stems::Stem;
use atomik_ui::{STEM_COUNT, StemsDialogChoice, StemsDialogState, StemsPhase};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
enum Stage {
    /// The choices.
    Options,
    /// The worker runs (reading, separating or writing).
    Running,
    /// It did not finish.
    Failed { message: String },
}

/// The dialog and what it was opened for.
struct Dialog {
    clip: ClipId,
    /// The asset id of the clip's file.
    asset: String,
    /// The file has one channel.
    mono: bool,
    /// The clip's name for the dialog and the stem files.
    name: String,
    stems: [bool; STEM_COUNT],
    mute_original: bool,
    stage: Stage,
}

/// The worker that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    shared: Arc<StemShared>,
}

/// The stem separation state of the app core.
pub(super) struct StemsState {
    backend: Arc<dyn StemBackend>,
    dialog: Option<Dialog>,
    running: Option<Running>,
    next_job: u64,
}

impl Default for StemsState {
    fn default() -> Self {
        Self {
            backend: Arc::new(DspStems),
            dialog: None,
            running: None,
            next_job: 0,
        }
    }
}

impl StemsState {
    /// Stops the worker and closes the dialog (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.dialog = None;
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Installs another separation backend (a learned model, when one exists, or a test fake).
    /// A core starts with the signal-processing split.
    pub fn set_stems(&mut self, backend: Arc<dyn StemBackend>) {
        self.stems.reset();
        self.stems.backend = backend;
    }

    /// Opens the Separate Stems dialog for the selected audio clip.
    ///
    /// # Errors
    /// Nothing is selected, the selection is not one audio clip, or a separation already runs
    /// (the reason is also in the status line); the dialog stays closed.
    pub fn separate_stems(&mut self) -> Result<(), String> {
        if self.stems.running.is_some() || self.stems.dialog.is_some() {
            let why = "A stem separation is already open".to_owned();
            self.reject(why.clone());
            return Err(why);
        }
        let selected = self.selected_audio();
        let [one] = selected.as_slice() else {
            let why = if selected.is_empty() {
                "Select the audio clip to separate"
            } else {
                "Separate one audio clip at a time"
            }
            .to_owned();
            self.reject(why.clone());
            return Err(why);
        };
        let doc = self.doc();
        let Some((track, _, clip)) = doc.song.clip(one.id) else {
            return Err("The clip is gone".to_owned());
        };
        if track.frozen.is_some() {
            let why = "The track is frozen: unfreeze it to separate its clip".to_owned();
            self.reject(why.clone());
            return Err(why);
        }
        let name = clip.name.clone().unwrap_or_else(|| {
            doc.device(&track.device)
                .map_or_else(|| track.device.0.clone(), |d| d.name.clone())
        });
        let mono = doc
            .assets
            .get(&one.content.asset)
            .is_some_and(|m| m.channels == 1);
        self.stems.dialog = Some(Dialog {
            clip: one.id,
            asset: one.content.asset.clone(),
            mono,
            name,
            stems: [true; STEM_COUNT],
            mute_original: true,
            stage: Stage::Options,
        });
        Ok(())
    }

    /// What the dialog shows, while it is open.
    pub fn stems_dialog(&self) -> Option<StemsDialogState> {
        let d = self.stems.dialog.as_ref()?;
        Some(match &d.stage {
            Stage::Options => StemsDialogState::Options {
                clip: d.name.clone(),
                stems: d.stems,
                mute_original: d.mute_original,
                mono: d.mono,
            },
            Stage::Running => match &self.stems.running {
                Some(r) => StemsDialogState::Working {
                    phase: match r.shared.phase() {
                        Phase::Separate => StemsPhase::Separating,
                        Phase::Store => StemsPhase::Storing,
                        Phase::Read => StemsPhase::Reading,
                    },
                    progress: r.shared.fraction(),
                },
                None => StemsDialogState::Working {
                    phase: StemsPhase::Reading,
                    progress: 0.0,
                },
            },
            Stage::Failed { message } => StemsDialogState::Failed {
                message: message.clone(),
            },
        })
    }

    /// Whether a separation runs.
    pub fn stems_running(&self) -> bool {
        self.stems.running.is_some()
    }

    /// A box or button of the dialog.
    pub fn stems_dialog_choice(&mut self, choice: StemsDialogChoice) {
        let Some(stage) = self.stems.dialog.as_ref().map(|d| d.stage.clone()) else {
            return;
        };
        match (choice, stage) {
            (StemsDialogChoice::Toggle(i), Stage::Options) if i < STEM_COUNT => {
                if let Some(d) = &mut self.stems.dialog {
                    d.stems[i] = !d.stems[i];
                }
            }
            (StemsDialogChoice::ToggleMute, Stage::Options) => {
                if let Some(d) = &mut self.stems.dialog {
                    d.mute_original = !d.mute_original;
                }
            }
            (StemsDialogChoice::Separate, Stage::Options) => {
                if self
                    .stems
                    .dialog
                    .as_ref()
                    .is_some_and(|d| !d.stems.contains(&true))
                {
                    self.reject("Check at least one stem".to_owned());
                } else {
                    self.start_stems();
                }
            }
            (StemsDialogChoice::Cancel, Stage::Running) => {
                if let Some(r) = &self.stems.running {
                    r.cancel.store(true, Ordering::Relaxed);
                }
            }
            (StemsDialogChoice::Cancel, Stage::Options)
            | (StemsDialogChoice::Close, Stage::Failed { .. }) => self.stems.dialog = None,
            _ => {}
        }
    }

    /// Starts the worker for the open dialog.
    fn start_stems(&mut self) {
        let Some(d) = &self.stems.dialog else {
            return;
        };
        let (asset, name) = (d.asset.clone(), d.name.clone());
        let wanted: Vec<Stem> = Stem::ALL
            .into_iter()
            .filter(|s| d.stems[s.index()])
            .collect();
        let fail = |core: &mut Self, message: String| {
            core.reject(message.clone());
            if let Some(d) = &mut core.stems.dialog {
                d.stage = Stage::Failed { message };
            }
        };
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => return fail(self, e),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(StemShared::new());
        let job = self.stems.next_job;
        self.stems.next_job += 1;
        let work = Work {
            bundle,
            asset,
            name,
            wanted,
            threshold: self.stream_threshold(),
            backend: Arc::clone(&self.stems.backend),
        };
        let started = stems_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&shared)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.stems.running = Some(Running {
                    job,
                    cancel,
                    shared,
                });
                if let Some(d) = &mut self.stems.dialog {
                    d.stage = Stage::Running;
                }
            }
            Err(e) => fail(self, format!("cannot start the separation: {e}")),
        }
    }

    /// The worker answered (or failed, or was stopped).
    pub(super) fn apply_stems(&mut self, job: u64, result: Result<Option<Done>, String>) {
        if self.stems.running.take_if(|r| r.job == job).is_none() {
            return;
        }
        match result {
            Err(message) => {
                self.reject(format!("Stem separation failed: {message}"));
                if let Some(d) = &mut self.stems.dialog {
                    d.stage = Stage::Failed { message };
                }
            }
            Ok(None) => {
                self.stems.dialog = None;
                self.reject("Stem separation cancelled".to_owned());
            }
            Ok(Some(done)) => {
                let Some(d) = self.stems.dialog.take() else {
                    return;
                };
                self.finish_stems(&d, done);
            }
        }
    }

    /// Registers the stems and adds their tracks in one undo step.
    fn finish_stems(&mut self, d: &Dialog, done: Done) {
        let still_there = self.doc().song.clip(d.clip).is_some_and(
            |(_, _, c)| matches!(&c.content, ClipContent::Audio(a) if a.asset == d.asset),
        );
        if !still_there {
            return self.reject(
                "The clip changed while it was separated: nothing was added, separate it again"
                    .to_owned(),
            );
        }
        let mut stems = Vec::new();
        for (stem, loaded) in &done.stems {
            let Some(info) = loaded.info.clone() else {
                return self.reject("A stem has no file".to_owned());
            };
            self.insert_loaded(loaded);
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
                return;
            }
            stems.push(StemTrack {
                name: format!("{} {}", d.name, stem.label()),
                asset: info.hash,
            });
        }
        let count = stems.len();
        let applied = self.apply_command(Command::Song(SongCommand::AddStemTracks {
            clip: d.clip,
            stems,
            mute_original: d.mute_original,
        }));
        if applied.is_ok() {
            self.reject(format!(
                "Separated {} into {count} track{}",
                d.name,
                if count == 1 { "" } else { "s" }
            ));
        }
    }
}
