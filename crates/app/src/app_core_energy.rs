//! The energy map in the app core: **View > Energy Lane** shows one row above the arrangement with
//! how much energy every section has, and flags the places where the shape of the song works
//! against itself ([`atomik_model::energy_map`] has the rules).
//!
//! [`AppCore::start_energy`] renders the master of the song on a worker thread
//! ([`crate::energy_worker`], through [`atomik_render::energy`]), measures each section's
//! loudness, brightness and width, counts the tracks that play in it and combines them into one
//! 0-1 value. The result never changes the song. [`AppCore::energy_progress`] and
//! [`AppCore::cancel_energy`] watch and stop it; opening or creating a project drops it. The lane
//! shows the last result faded and says `out of date` once the song changes after it was
//! measured; **View > Refresh Energy Lane** measures again. Turning the lane on measures when
//! there is no result yet.

use super::*;
use crate::energy_worker::{self, Work};
use crate::freeze_worker::PluginEnv;
use atomik_model::energy_map::{EnergyMap, sections};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The measurement that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// A finished measurement and the revision of the document it measured.
struct Measured {
    map: Arc<EnergyMap>,
    revision: u64,
}

/// The energy state of an open project.
#[derive(Default)]
pub(super) struct EnergyState {
    running: Option<Running>,
    next_job: u64,
    /// The revision of the document the running measurement started from.
    started_at: u64,
    result: Option<Measured>,
    /// Why the last measurement gave nothing (shown in the lane header).
    note: Option<String>,
}

impl EnergyState {
    /// Stops the running measurement and forgets the result (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.result = None;
        self.note = None;
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Starts measuring the master (see the module docs). One runs at a time.
    ///
    /// # Errors
    /// A measurement already runs, the song is empty or the document has no master section (the
    /// reason is also in the status line and in the lane header); nothing starts.
    pub fn start_energy(&mut self) -> Result<(), String> {
        let refusal = if self.audio.energy.running.is_some() {
            Some("The energy map is already being measured".to_owned())
        } else if self.doc().console.master.is_none() {
            Some("The energy map measures the master: the song has none".to_owned())
        } else if sections(&self.doc().song).is_empty() {
            Some("The song is empty: there is nothing to measure".to_owned())
        } else {
            None
        };
        if let Some(why) = refusal {
            self.audio.energy.note = Some("nothing to measure".to_owned());
            self.reject(why.clone());
            return Err(why);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.energy.next_job;
        self.audio.energy.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
        };
        let started = energy_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.energy.started_at = self.document.revision();
                self.audio.energy.note = None;
                self.audio.energy.running = Some(Running {
                    job,
                    cancel,
                    progress,
                });
                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the measurement: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// Whether a measurement runs.
    pub fn energy_running(&self) -> bool {
        self.audio.energy.running.is_some()
    }

    /// How far the running measurement is, `0.0..=1.0`.
    pub fn energy_progress(&self) -> Option<f32> {
        let r = self.audio.energy.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running measurement.
    pub fn cancel_energy(&mut self) {
        if let Some(r) = &self.audio.energy.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last measured map of this project, and whether the song changed after it.
    pub fn energy(&self) -> Option<(&Arc<EnergyMap>, bool)> {
        let m = self.audio.energy.result.as_ref()?;
        Some((&m.map, m.revision != self.document.revision()))
    }

    /// The sentences of the flags of the last map.
    pub fn energy_flags(&self) -> Vec<String> {
        self.audio
            .energy
            .result
            .as_ref()
            .map(|m| m.map.flags.iter().map(|f| f.message(&m.map)).collect())
            .unwrap_or_default()
    }

    /// A measurement finished (or failed, or was stopped).
    pub(super) fn apply_energy(&mut self, job: u64, result: Result<Option<EnergyMap>, String>) {
        if self
            .audio
            .energy
            .running
            .take_if(|r| r.job == job)
            .is_none()
        {
            return;
        }
        match result {
            Err(e) => {
                self.audio.energy.note = Some("measuring failed".to_owned());
                self.reject(format!("Energy map failed: {e}"));
            }
            Ok(None) => self.reject("Energy map cancelled".to_owned()),
            Ok(Some(map)) => {
                let flags = map.flags.len();
                let mut message = format!(
                    "Energy map: {} section{}, {} flag{}",
                    map.sections.len(),
                    if map.sections.len() == 1 { "" } else { "s" },
                    flags,
                    if flags == 1 { "" } else { "s" },
                );
                if let Some(first) = map.flags.first() {
                    message.push_str(": ");
                    message.push_str(&first.message(&map));
                }
                self.audio.energy.result = Some(Measured {
                    map: Arc::new(map),
                    revision: self.audio.energy.started_at,
                });
                self.reject(message);
            }
        }
    }

    /// Once per frame: hands the arrangement's energy lane what it draws.
    pub(super) fn energy_frame(&mut self) {
        if !self.arrangement.energy_lane_visible() {
            return;
        }
        let (map, stale) = match self.energy() {
            Some((map, stale)) => (Some(Arc::clone(map)), stale),
            None => (None, false),
        };
        let progress = self.energy_progress();
        let note = self.audio.energy.note.clone();
        self.arrangement.set_energy_map(map, stale);
        self.arrangement.set_energy_progress(progress);
        self.arrangement.set_energy_note(note);
    }

    /// Installs a map as if a measurement had found it (tests).
    #[cfg(test)]
    pub(super) fn set_energy_for_test(&mut self, map: EnergyMap) {
        self.audio.energy.result = Some(Measured {
            map: Arc::new(map),
            revision: self.document.revision(),
        });
    }
}
