//! "Will the bass read?" in the app core: **Mixer > Will the Bass Read?** renders one strip (a
//! track's channel or the master) and reads how much of its low-end note energy a small speaker
//! still plays through the harmonics of the note ([`atomik_dsp::bass_read`] has the rules).
//!
//! [`AppCore::start_bass_read`] renders the strip on a worker thread ([`crate::bass_read_worker`],
//! through [`atomik_render::translation`]) and turns the reading into a [`BassReadingRow`] for the
//! panel. The reading never changes the song. [`AppCore::bass_read_progress`] and
//! [`AppCore::cancel_bass_read`] watch and stop it; opening or creating a project drops it. The
//! panel shows the last reading faded once the song changes after it was made.
//!
//! The strips to choose from are the master first, then every channel in mixer order; the list is
//! rebuilt when the document changes. Opening the panel chooses the selected track's channel when
//! there is one, else the master.

use super::*;
use crate::bass_read_worker::{self, Work};
use crate::freeze_worker::PluginEnv;
use atomik_dsp::bass_read::{Reading, Verdict};
use atomik_model::{StemKind, StemSource, stem_sources};
use atomik_ui::{BassLevel, BassReadData, BassReadingRow};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The reading that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    /// The strip it reads.
    source: String,
}

/// A finished reading and the revision of the document it read.
struct Measured {
    source: String,
    reading: Reading,
    revision: u64,
}

/// The bass-read state of an open project.
#[derive(Default)]
pub(super) struct BassReadState {
    running: Option<Running>,
    next_job: u64,
    /// The revision of the document the running reading started from.
    started_at: u64,
    /// The strips of the list: the master first, then the channels, and the revision they were
    /// made for.
    sources: Vec<StemSource>,
    sources_revision: Option<u64>,
    chosen: usize,
    result: Option<Measured>,
}

impl BassReadState {
    /// Stops the running reading and forgets everything (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.sources.clear();
        self.sources_revision = None;
        self.chosen = 0;
        self.result = None;
    }
}

/// The detail line of a reading: the pitch of the low notes and how many windows were found.
fn detail(reading: &Reading) -> String {
    if reading.windows == 0 {
        return "No note between 25 and 150 Hz was found".to_owned();
    }
    format!(
        "Low notes around {:.0} Hz; {:.0} % of their energy is in harmonics 2 to 5 that a small speaker plays ({} windows)",
        reading.note_hz,
        reading.share * 100.0,
        reading.windows
    )
}

fn level_of(verdict: Verdict) -> BassLevel {
    match verdict {
        Verdict::NoBass => BassLevel::None,
        Verdict::Lost => BassLevel::Lost,
        Verdict::Partly => BassLevel::Partly,
        Verdict::Reads => BassLevel::Reads,
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Rebuilds the list of strips when the document changed since it was made.
    fn refresh_bass_read_sources(&mut self) {
        let revision = self.document.revision();
        if self.bass_read.sources_revision == Some(revision) {
            return;
        }
        let all = stem_sources(self.doc(), &self.registry);
        let mut list: Vec<StemSource> = all
            .iter()
            .filter(|s| s.kind == StemKind::Master)
            .cloned()
            .collect();
        list.extend(all.into_iter().filter(|s| s.kind == StemKind::Channel));
        // The chosen strip stays chosen by its key while the list changes under it.
        let key = self
            .bass_read
            .sources
            .get(self.bass_read.chosen)
            .map(|s| s.key.clone());
        self.bass_read.chosen = key
            .and_then(|k| list.iter().position(|s| s.key == k))
            .unwrap_or(0);
        self.bass_read.sources = list;
        self.bass_read.sources_revision = Some(revision);
    }

    /// Chooses the strip the panel opens on: the channel of the selected track, else the master.
    pub fn bass_read_preselect(&mut self) {
        self.bass_read.sources_revision = None;
        self.refresh_bass_read_sources();
        let track = self.arrangement.selected_track().cloned();
        let chosen = track.and_then(|device| {
            let channel = self
                .doc()
                .console
                .channels
                .iter()
                .find(|c| c.source.as_ref() == Some(&device))?
                .channel
                .clone();
            self.bass_read.sources.iter().position(|s| s.key == channel)
        });
        self.bass_read.chosen = chosen.unwrap_or(0);
    }

    /// Chooses strip `n` of the list (clamped).
    pub fn bass_read_choose(&mut self, n: usize) {
        self.refresh_bass_read_sources();
        let last = self.bass_read.sources.len().saturating_sub(1);
        self.bass_read.chosen = n.min(last);
    }

    /// Starts reading the chosen strip (see the module docs). One runs at a time.
    ///
    /// # Errors
    /// A reading already runs, there is no strip (no master or channel) or the song is empty (the
    /// reason is also in the status line); nothing starts.
    pub fn start_bass_read(&mut self) -> Result<(), String> {
        self.refresh_bass_read_sources();
        let source = self.bass_read.sources.get(self.bass_read.chosen).cloned();
        let refusal = if self.bass_read.running.is_some() {
            Some("The bass is already being read".to_owned())
        } else if source.is_none() {
            Some("There is no master or track to read".to_owned())
        } else if self.doc().song.tracks.is_empty() {
            Some("The song is empty: there is nothing to read".to_owned())
        } else {
            None
        };
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        let Some(source) = source else {
            return Err("There is no master or track to read".to_owned());
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.bass_read.next_job;
        self.bass_read.next_job += 1;
        let name = source.name.clone();
        let work = Work {
            doc: self.doc().clone(),
            source,
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
        };
        let started = bass_read_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.bass_read.started_at = self.document.revision();
                self.bass_read.running = Some(Running {
                    job,
                    cancel,
                    progress,
                    source: name,
                });
                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the reading: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// Whether a reading runs.
    pub fn bass_read_running(&self) -> bool {
        self.bass_read.running.is_some()
    }

    /// How far the running reading is, `0.0..=1.0`.
    pub fn bass_read_progress(&self) -> Option<f32> {
        let r = self.bass_read.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running reading.
    pub fn cancel_bass_read(&mut self) {
        if let Some(r) = &self.bass_read.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last reading, and whether the song changed after it.
    pub fn bass_reading(&self) -> Option<(&Reading, bool)> {
        let m = self.bass_read.result.as_ref()?;
        Some((&m.reading, m.revision != self.document.revision()))
    }

    /// What the panel shows.
    pub fn bass_read_data(&mut self) -> BassReadData {
        self.refresh_bass_read_sources();
        let reading = self.bass_read.result.as_ref().map(|m| BassReadingRow {
            source: m.source.clone(),
            share: m.reading.share,
            verdict: m.reading.verdict.text().to_owned(),
            advice: m.reading.verdict.advice().to_owned(),
            detail: detail(&m.reading),
            level: level_of(m.reading.verdict),
            stale: m.revision != self.document.revision(),
        });
        BassReadData {
            sources: self
                .bass_read
                .sources
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            chosen: self.bass_read.chosen,
            running: self.bass_read_progress(),
            reading,
        }
    }

    /// A reading finished (or failed, or was stopped).
    pub(super) fn apply_bass_read(&mut self, job: u64, result: Result<Option<Reading>, String>) {
        let Some(running) = self.bass_read.running.take_if(|r| r.job == job) else {
            return;
        };
        match result {
            Err(e) => self.reject(format!("Bass reading failed: {e}")),
            Ok(None) => self.reject("Bass reading cancelled".to_owned()),
            Ok(Some(reading)) => {
                let message = if reading.windows == 0 {
                    format!("{}: no low notes to judge", running.source)
                } else {
                    format!(
                        "{}: {:.0} % of the bass note survives a small speaker. {}",
                        running.source,
                        reading.share * 100.0,
                        reading.verdict.text()
                    )
                };
                self.bass_read.result = Some(Measured {
                    source: running.source,
                    reading,
                    revision: self.bass_read.started_at,
                });
                self.reject(message);
            }
        }
    }
}
