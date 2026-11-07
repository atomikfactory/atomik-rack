//! Delivery targets in the app core: **File > Delivery Targets...** renders the master, measures
//! its loudness and true peak, and lists what each of the common targets
//! ([`atomik_model::TARGETS`]) would do to it: the gain a normalising chain applies, and a
//! warning where a quiet master with peaks near 0 dBFS would be turned up into a limiter.
//!
//! [`AppCore::start_delivery`] renders the master on a worker thread ([`crate::delivery_worker`],
//! through [`atomik_render::delivery`]). The measurement never changes the song.
//! [`AppCore::delivery_progress`] and [`AppCore::cancel_delivery`] watch and stop it; opening or
//! creating a project drops it. The panel shows the last measurement faded once the song changes
//! after it was made.
//!
//! **Set Ceiling** ([`AppCore::delivery_set_ceiling`]) is one undo step: the ceiling of the
//! master's Mastering device (else the master's Clipper) goes to the chosen target's ceiling, and
//! the chosen target becomes the project's delivery target, which the export's loudness report
//! compares against. A master with neither device only gets the target.

use super::*;
use crate::delivery_worker::{self, Work};
use crate::freeze_worker::PluginEnv;
use atomik_model::{
    Delivery, DeliveryTarget, DeliveryVerdict, TARGETS, assess_delivery, insert_chain,
};
use atomik_render::loudness::LoudnessReport;
use atomik_ui::{DeliveryData, DeliveryPage, PeaksData, TargetMark, TargetRow, TargetsData};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MASTERING: &str = "atomik.mastering";
const CLIPPER: &str = "atomik.clipper";
/// The target the panel starts on: the plain streaming one.
const DEFAULT_TARGET: usize = 1;

/// The measurement that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// A finished measurement and the revision of the document it measured.
struct Measured {
    report: LoudnessReport,
    revision: u64,
}

/// The delivery state of an open project.
#[derive(Default)]
pub(super) struct DeliveryState {
    pub(super) page: DeliveryPage,
    running: Option<Running>,
    next_job: u64,
    /// The revision of the document the running measurement started from.
    started_at: u64,
    result: Option<Measured>,
    chosen: usize,
}

impl DeliveryState {
    /// Stops the running measurement and forgets the result (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.result = None;
        self.chosen = 0;
        self.page = DeliveryPage::Targets;
    }
}

/// The measured numbers as one line.
fn master_line(r: &LoudnessReport) -> String {
    if r.is_silent() {
        return "silent".to_owned();
    }
    let mut parts = Vec::new();
    if let Some(l) = r.integrated_lufs {
        parts.push(format!("{l:.1} LUFS"));
    }
    if let Some(p) = r.true_peak_dbtp {
        parts.push(format!("{p:.1} dBTP"));
    }
    if let Some(l) = r.range_lu {
        parts.push(format!("range {l:.1} LU"));
    }
    parts.join("   ")
}

impl<S: EngineSink> AppCore<S> {
    /// The device whose ceiling Set Ceiling sets: the Mastering device on the master's insert
    /// chain, else its Clipper, with the lowest ceiling the device takes.
    fn ceiling_device(&self) -> Option<(DeviceKey, &'static str, f32)> {
        let doc = self.doc();
        let master = doc.console.master.clone()?;
        let chain = insert_chain(doc, &self.registry, &master);
        for (type_id, name, lowest) in
            [(MASTERING, "Mastering", -12.0), (CLIPPER, "Clipper", -24.0)]
        {
            if let Some(k) = chain
                .iter()
                .find(|k| doc.device(k).is_some_and(|d| d.type_id == type_id))
            {
                return Some((k.clone(), name, lowest));
            }
        }
        None
    }

    /// Opens the panel on the Targets page, on the project's target when it has one.
    pub fn delivery_open(&mut self, page: DeliveryPage) {
        self.delivery.page = page;
        if self.delivery.result.is_none() && self.delivery.chosen == 0 {
            self.delivery.chosen = DEFAULT_TARGET;
        }
        let own = self
            .doc()
            .delivery
            .as_ref()
            .and_then(|d| TARGETS.iter().position(|t| t.name == d.name));
        if let Some(i) = own {
            self.delivery.chosen = i;
        }
        if page == DeliveryPage::Peaks {
            self.surgeon_scan();
        }
    }

    /// Shows a page of the panel.
    pub fn delivery_page(&mut self, page: DeliveryPage) {
        self.delivery.page = page;
        if page == DeliveryPage::Peaks && !self.surgeon_has_scan() {
            self.surgeon_scan();
        }
    }

    /// Chooses target `n` of the table (clamped).
    pub fn delivery_choose(&mut self, n: usize) {
        self.delivery.chosen = n.min(TARGETS.len() - 1);
    }

    /// Starts measuring the master (see the module docs). One runs at a time.
    ///
    /// # Errors
    /// A measurement already runs, the song is empty or the document has no master section (the
    /// reason is also in the status line); nothing starts.
    pub fn start_delivery(&mut self) -> Result<(), String> {
        let refusal = if self.delivery.running.is_some() {
            Some("The master is already being measured".to_owned())
        } else if self.doc().console.master.is_none() {
            Some("The delivery targets measure the master: the song has none".to_owned())
        } else if self.doc().song.tracks.is_empty() {
            Some("The song is empty: there is nothing to measure".to_owned())
        } else {
            None
        };
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.delivery.next_job;
        self.delivery.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
        };
        let started = delivery_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.delivery.started_at = self.document.revision();
                self.delivery.running = Some(Running {
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
    pub fn delivery_running(&self) -> bool {
        self.delivery.running.is_some()
    }

    /// How far the running measurement is, `0.0..=1.0`.
    pub fn delivery_progress(&self) -> Option<f32> {
        let r = self.delivery.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running measurement.
    pub fn cancel_delivery(&mut self) {
        if let Some(r) = &self.delivery.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last measurement of this project, and whether the song changed after it.
    pub fn delivery_report(&self) -> Option<(&LoudnessReport, bool)> {
        let m = self.delivery.result.as_ref()?;
        Some((&m.report, m.revision != self.document.revision()))
    }

    /// A measurement finished (or failed, or was stopped).
    pub(super) fn apply_delivery(
        &mut self,
        job: u64,
        result: Result<Option<LoudnessReport>, String>,
    ) {
        if self.delivery.running.take_if(|r| r.job == job).is_none() {
            return;
        }
        match result {
            Err(e) => self.reject(format!("Measuring the master failed: {e}")),
            Ok(None) => self.reject("Measuring the master cancelled".to_owned()),
            Ok(Some(report)) => {
                let message = format!("Master: {}", master_line(&report));
                self.delivery.result = Some(Measured {
                    report,
                    revision: self.delivery.started_at,
                });
                self.reject(message);
            }
        }
    }

    /// What one target does to the measured master, with its row colour.
    fn target_row(&self, target: &DeliveryTarget) -> TargetRow {
        let current = self
            .doc()
            .delivery
            .as_ref()
            .is_some_and(|d| d.name == target.name);
        let measured = self.delivery.result.as_ref().and_then(|m| {
            assess_delivery(target, m.report.integrated_lufs, m.report.true_peak_dbtp)
        });
        let (effect, mark) = match &measured {
            None => ("not measured".to_owned(), TargetMark::None),
            Some(a) => (
                a.sentence(target),
                match a.verdict {
                    DeliveryVerdict::Unchanged | DeliveryVerdict::TurnedDown => TargetMark::Fine,
                    DeliveryVerdict::TurnedUp => TargetMark::Up,
                    DeliveryVerdict::IntoLimiter => TargetMark::Limiter,
                },
            ),
        };
        TargetRow {
            name: target.name.to_owned(),
            effect,
            mark,
            current,
        }
    }

    /// The warning under the list: the chosen target's, else a count of the red rows.
    fn delivery_warning(&self, rows: &[TargetRow]) -> String {
        let Some(m) = &self.delivery.result else {
            return String::new();
        };
        let target = &TARGETS[self.delivery.chosen.min(TARGETS.len() - 1)];
        let chosen = assess_delivery(target, m.report.integrated_lufs, m.report.true_peak_dbtp);
        if let Some(a) = chosen.filter(|a| a.verdict == DeliveryVerdict::IntoLimiter) {
            return format!(
                "This master is quiet and its peaks stand near 0 dBFS: {} would turn it up {:.1} dB \
                 and push the peaks {:.1} dB past its {:.1} dBTP ceiling, so its limiter would act \
                 on it. Trim the peaks (Peak Surgeon), raise the loudness and set the ceiling here.",
                target.name,
                a.gain_db,
                a.over_dbtp(target),
                target.ceiling_dbtp
            );
        }
        let red = rows
            .iter()
            .filter(|r| r.mark == TargetMark::Limiter)
            .count();
        if red > 0 {
            return format!(
                "{red} target{} would turn the master up into a limiter (red rows)",
                if red == 1 { "" } else { "s" }
            );
        }
        String::new()
    }

    /// What the Targets page shows.
    fn targets_data(&self) -> TargetsData {
        let rows: Vec<TargetRow> = TARGETS.iter().map(|t| self.target_row(t)).collect();
        let chosen = self.delivery.chosen.min(TARGETS.len() - 1);
        let target = &TARGETS[chosen];
        let ceiling_note = match self.ceiling_device() {
            Some((_, name, lowest)) => format!(
                "Set Ceiling: the {name} ceiling to {:.1} dB and the report target, one undo step",
                target.ceiling_dbtp.clamp(lowest, 0.0)
            ),
            None => {
                "No Mastering or Clipper on the master: Set Ceiling sets the report target only"
                    .to_owned()
            }
        };
        TargetsData {
            running: self.delivery_progress(),
            master: self
                .delivery
                .result
                .as_ref()
                .map(|m| master_line(&m.report)),
            stale: self
                .delivery
                .result
                .as_ref()
                .is_some_and(|m| m.revision != self.document.revision()),
            warning: self.delivery_warning(&rows),
            rows,
            chosen,
            ceiling_note,
        }
    }

    /// What the panel shows.
    pub fn delivery_data(&mut self) -> DeliveryData {
        DeliveryData {
            page: self.delivery.page,
            targets: self.targets_data(),
            peaks: if self.delivery.page == DeliveryPage::Peaks {
                self.peaks_data()
            } else {
                PeaksData::default()
            },
        }
    }

    /// Set Ceiling: the master limiter's ceiling and the project's delivery target become the
    /// chosen target's, as one undo step (see the module docs).
    pub fn delivery_set_ceiling(&mut self) {
        let target = TARGETS[self.delivery.chosen.min(TARGETS.len() - 1)];
        let mut commands = Vec::new();
        let mut parts = Vec::new();
        if let Some((key, name, lowest)) = self.ceiling_device() {
            let ceiling = target.ceiling_dbtp.clamp(lowest, 0.0);
            let now = self.doc().param_value(&key, "ceiling", &self.registry);
            if now.is_none_or(|v| (v - ceiling).abs() > 1e-4) {
                commands.push(Command::SetParam {
                    key,
                    param: "ceiling".to_owned(),
                    value: ceiling,
                    merge: None,
                });
            }
            parts.push(format!("{name} ceiling {ceiling:.1} dB"));
        }
        let wanted = Some(Delivery::of(&target));
        if self.doc().delivery != wanted {
            commands.push(Command::SetDelivery(wanted));
        }
        parts.push(format!("report target {}", target.name));
        let Some(command) = Self::one_step(commands) else {
            return self.reject(format!("Already set to {}", target.name));
        };
        if self.apply_command(command).is_ok() {
            self.reject(format!("Delivery: {}", parts.join(", ")));
        }
    }

    /// Installs a measurement as if the worker had found it (tests).
    #[cfg(test)]
    pub(super) fn set_delivery_for_test(&mut self, report: LoudnessReport) {
        self.delivery.result = Some(Measured {
            report,
            revision: self.document.revision(),
        });
    }
}
