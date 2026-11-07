//! The mix check in the app core: **Mixer > Check Mix** analyses the song and lists what the mix
//! needs, and a finding can be fixed in one click.
//!
//! # Analysis
//!
//! [`AppCore::start_mixcheck`] renders the channels, the FX returns and the master of the console,
//! and the feed of every non-linear device on an insert chain, as stems on a worker thread
//! ([`crate::mixcheck_worker`], through [`atomik_render::mixcheck`]) over the whole song, or over
//! the time selection when there is one. The result, [`AppCore::mixcheck`], never changes the
//! song. [`AppCore::mixcheck_progress`] and [`AppCore::cancel_mixcheck`] watch and stop it;
//! opening or creating a project drops it. [`AppCore::locate_mixcheck`] puts the playhead at the
//! time of a finding.
//!
//! # Fixes
//!
//! [`AppCore::fix_mixcheck`] applies the fix of a finding in **one undo step** and says in the
//! status line what it did. A finding whose track or device is gone, a chain that is full or a
//! step the model refuses is reported and leaves the song as it was.
//!
//! * **Trim.** A Gain Tool is added to the insert chain of the strip just before the device that
//!   is fed too loud or too soft, with the gain that brings its feed to the reference level.
//! * **Cut the low mids.** A bell band of a Dynamic EQ on the channel (a free one on its chain, or
//!   a new device at the end) is set to a static cut at the loudest low-mid band of the channel.
//! * **Make mono.** A Stereo Tool with Mono on is added at the end of the channel's chain.
//!
//! # Tools
//!
//! A finding may name the tool that works on it ([`atomik_render::mixcheck::Tool`]).
//! [`AppCore::mixcheck_tool`] answers with the tool a button opens: the masking map, the kick and
//! bass check (with the tracks of the finding picked, see [`AppCore::mixcheck_kick_bass`]) or a
//! Clipper on the master ([`AppCore::mixcheck_clipper`], one undo step).

use super::*;
use crate::freeze_worker::PluginEnv;
use crate::mixcheck_worker::{self, CheckAnalysis, Work};
use atomik_dsp::mixcheck::{LOW_MID_CUT_DB, LOW_MID_CUT_Q};
use atomik_model::{ConsoleCommand, INSERT_SLOTS, InsertEffect, insert_chain};
use atomik_render::export::{ExportSpan, span_frames};
use atomik_render::mixcheck::{Finding, Fix, Kind, Tool};
use atomik_ui::{MixCheckData, MixCheckRow};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::masking::{BELLS, DYNAMIC_EQ};

/// The Gain Tool's type id.
const GAIN_TOOL: &str = "atomik.gain-tool";
/// The Stereo Tool's type id.
const STEREO_TOOL: &str = "atomik.stereo-tool";
/// The Clipper's type id.
const CLIPPER: &str = "atomik.clipper";
/// How far under the loudest master peak the Clipper's ceiling is set (dB).
const CLIPPER_TRIM_DB: f32 = 1.0;
/// The lowest ceiling the Clipper offers (dB).
const CLIPPER_MIN_CEILING: f32 = -24.0;
/// The highest ceiling the Clipper is set to (dB), so it always takes something off.
const CLIPPER_MAX_CEILING: f32 = -0.1;

/// The check that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// The mix check state of an open project.
#[derive(Default)]
pub(super) struct MixCheckState {
    running: Option<Running>,
    next_job: u64,
    result: Option<CheckAnalysis>,
    /// The findings whose fix was applied.
    fixed: BTreeSet<usize>,
}

impl MixCheckState {
    /// Stops the running check and forgets the result (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.result = None;
        self.fixed.clear();
    }
}

fn tag(kind: Kind) -> &'static str {
    match kind {
        Kind::BassRoot => "BASS ROOT",
        Kind::WeakFundamental => "FUNDAMENTAL",
        Kind::LowMid => "LOW MIDS",
        Kind::Level => "LEVEL",
        Kind::Peaks => "PEAKS",
        Kind::Mono => "MONO",
    }
}

fn fix_label(fix: &Fix) -> String {
    match fix {
        Fix::Trim { db, .. } => format!("TRIM {db:+.1} dB"),
        Fix::CutLowMids { hz, .. } => format!("CUT {hz:.0} Hz"),
        Fix::MonoLow { .. } => "MAKE MONO".to_owned(),
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The range the check covers: the time selection when there is one, else the song.
    fn mixcheck_span(&self) -> ExportSpan {
        match self.export_selection() {
            Some((start, end)) => ExportSpan::Range(start, end),
            None => ExportSpan::Song,
        }
    }

    /// Starts the check of the song (see the module docs). One runs at a time.
    ///
    /// # Errors
    /// A check already runs, the song has no channel or the range is out of bounds (the reason is
    /// also in the status line); nothing starts.
    pub fn start_mixcheck(&mut self) -> Result<(), String> {
        let refusal = if self.audio.mixcheck.running.is_some() {
            Some("The mix check is already running".to_owned())
        } else if self.doc().console.channels.is_empty() {
            Some("The mix check reads the mixer channels: the song has none".to_owned())
        } else {
            span_frames(self.doc(), self.mixcheck_span())
                .err()
                .map(|e| e.to_string())
        };
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.mixcheck.next_job;
        self.audio.mixcheck.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            span: self.mixcheck_span(),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
        };
        let started = mixcheck_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.mixcheck.running = Some(Running {
                    job,
                    cancel,
                    progress,
                });
                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the check: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// Whether a check runs.
    pub fn mixcheck_running(&self) -> bool {
        self.audio.mixcheck.running.is_some()
    }

    /// How far the running check is, `0.0..=1.0`.
    pub fn mixcheck_progress(&self) -> Option<f32> {
        let r = self.audio.mixcheck.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running check.
    pub fn cancel_mixcheck(&mut self) {
        if let Some(r) = &self.audio.mixcheck.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The result of the last check of this project.
    pub fn mixcheck(&self) -> Option<&CheckAnalysis> {
        self.audio.mixcheck.result.as_ref()
    }

    /// A check finished (or failed, or was stopped).
    pub(super) fn apply_checked(
        &mut self,
        job: u64,
        result: Result<Option<CheckAnalysis>, String>,
    ) {
        if self
            .audio
            .mixcheck
            .running
            .take_if(|r| r.job == job)
            .is_none()
        {
            return;
        }
        match result {
            Err(e) => self.reject(format!("Mix check failed: {e}")),
            Ok(None) => self.reject("Mix check cancelled".to_owned()),
            Ok(Some(found)) => {
                let n = found.findings.len();
                let message = if n == 0 {
                    format!("Mix check: nothing to report in {} strips", found.strips)
                } else {
                    format!("Mix check: {n} finding{}", if n == 1 { "" } else { "s" })
                };
                self.audio.mixcheck.fixed.clear();
                self.audio.mixcheck.result = Some(found);
                self.reject(message);
            }
        }
    }

    /// The result as the panel draws it.
    pub fn mixcheck_data(&self) -> Option<MixCheckData> {
        let found = self.audio.mixcheck.result.as_ref()?;
        let origin = found.from_frame as f64 / f64::from(found.rate.max(1));
        let rows = found
            .findings
            .iter()
            .enumerate()
            .map(|(n, f)| MixCheckRow {
                tag: tag(f.kind).to_owned(),
                track: f.track.clone(),
                measured: f.measured.clone(),
                why: f.why.to_owned(),
                at: f.at.map(|a| origin + a),
                fix: f.fix.as_ref().map(fix_label),
                tool: f
                    .tool
                    .filter(|t| t.available())
                    .map(|t| t.label().to_owned()),
                done: self.audio.mixcheck.fixed.contains(&n),
            })
            .collect();
        Some(MixCheckData {
            strips: found.strips,
            rows,
        })
    }

    fn finding(&self, n: usize) -> Option<&Finding> {
        self.audio.mixcheck.result.as_ref()?.findings.get(n)
    }

    /// Puts the playhead where finding `n` happens (a finding with no time does nothing).
    pub fn locate_mixcheck(&mut self, n: usize) {
        let Some(found) = self.audio.mixcheck.result.as_ref() else {
            return;
        };
        let Some(at) = found.findings.get(n).and_then(|f| f.at) else {
            return;
        };
        let frame = found.from_frame + (at * f64::from(found.rate.max(1))).round() as u64;
        let clock = self.doc().song.frame_clock(self.doc().sample_rate);
        let tick = clock.tick_at(frame).max(0);
        self.transport_action(TransportAction::Locate(tick));
    }

    /// The tool finding `n` names, when the application can open it now. Another tool is a hook
    /// for the tools that are built separately: the status line says there is none yet.
    pub fn mixcheck_tool(&mut self, n: usize) -> Option<Tool> {
        let tool = self.finding(n)?.tool?;
        if !tool.available() {
            self.reject(format!(
                "There is no {} tool in this version yet",
                tool.label().to_lowercase()
            ));
            return None;
        }
        Some(tool)
    }

    /// Picks the tracks of finding `n` for the kick and bass check: the track of the finding's
    /// channel becomes the bass (a bass root, or a name that is not a kick) or the kick. The other
    /// pick is left to the check's own guess from the names.
    pub fn mixcheck_kick_bass(&mut self, n: usize) {
        let Some(finding) = self.finding(n) else {
            return;
        };
        let (kind, channel) = (finding.kind, finding.channel.clone());
        let Some(channel) = channel else {
            return;
        };
        let Some(source) = self
            .doc()
            .console
            .channels
            .iter()
            .find(|c| c.channel == channel)
            .and_then(|c| c.source.clone())
        else {
            return;
        };
        let name = self.strip_label(&channel).to_lowercase();
        let kick = kind != Kind::BassRoot
            && name
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with("kick") || w == "bd");
        self.kick_bass_preselect(!kick, source);
    }

    /// Adds a Clipper to the end of the master's inserts for the peaks finding `n`, in one undo
    /// step, and says what it did in the status line. The ceiling is set a decibel under the
    /// loudest peak the check found (hard curve, 4x oversampling), so it takes the tallest spots
    /// off. A master that has a Clipper already is left as it is and the status line says so.
    ///
    /// # Errors
    /// There is no such finding, the master is gone or its chain is full; the song is left as it
    /// was.
    pub fn mixcheck_clipper(&mut self, n: usize) -> Result<(), String> {
        let result = self.clipper_inner(n);
        match &result {
            Ok(message) => self.reject(message.clone()),
            Err(why) => self.reject(why.clone()),
        }
        result.map(|_| ())
    }

    fn clipper_inner(&mut self, n: usize) -> Result<String, String> {
        let peak = self.finding(n).ok_or("Run the mix check first")?.value;
        let master = self
            .doc()
            .console
            .master
            .clone()
            .ok_or("The song has no master: run the check again")?;
        let chain = insert_chain(self.doc(), &self.registry, &master);
        if chain
            .iter()
            .any(|k| self.doc().device(k).is_some_and(|d| d.type_id == CLIPPER))
        {
            return Ok("The master has a Clipper already: set its ceiling there".to_owned());
        }
        let ceiling = (peak - CLIPPER_TRIM_DB).clamp(CLIPPER_MIN_CEILING, CLIPPER_MAX_CEILING);
        let depth = self.document.undo_depth();
        let clipper = self.add_insert(&master, CLIPPER)?;
        let set = |param: &str, value: f32| Command::SetParam {
            key: clipper.clone(),
            param: param.to_owned(),
            value,
            merge: None,
        };
        self.finish_fix(
            depth,
            vec![
                set("ceiling", ceiling),
                set("curve", 0.0),
                set("oversample", 2.0),
            ],
        )?;
        Ok(format!(
            "Added a Clipper to the master: ceiling {ceiling:.1} dB, hard curve, 4x oversampling"
        ))
    }

    /// Installs a result as if a check had found it (tests).
    #[cfg(test)]
    pub(crate) fn set_mixcheck_for_test(&mut self, found: CheckAnalysis) {
        self.audio.mixcheck.fixed.clear();
        self.audio.mixcheck.result = Some(found);
    }

    // ------------------------------------------------------------------ fixes

    /// The name the mixer shows for a strip: the track a channel plays, else the device.
    fn strip_label(&self, strip: &DeviceKey) -> String {
        let doc = self.doc();
        let device = doc
            .console
            .channels
            .iter()
            .find(|c| c.channel == *strip)
            .and_then(|c| c.source.clone())
            .unwrap_or_else(|| strip.clone());
        doc.device(&device)
            .map(|d| d.name.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| strip.0.clone())
    }

    /// Adds a new `type_id` effect at the end of the insert chain of `strip`; the first step of a
    /// fix (it starts the undo step).
    fn add_insert(&mut self, strip: &DeviceKey, type_id: &str) -> Result<DeviceKey, String> {
        let name = self.strip_label(strip);
        if self.doc().console.channel_index(strip).is_none()
            && self.doc().console.bus_index(strip).is_none()
            && self.doc().console.master.as_ref() != Some(strip)
        {
            return Err(format!(
                "{name} is no longer on the mixer: run the check again"
            ));
        }
        let chain = insert_chain(self.doc(), &self.registry, strip);
        if chain.len() >= INSERT_SLOTS {
            return Err(format!(
                "The insert chain of {name} is full: free a slot first"
            ));
        }
        let before: Vec<DeviceKey> = self.doc().devices.iter().map(|d| d.key.clone()).collect();
        self.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel: strip.clone(),
            slot: chain.len(),
            effect: Some(InsertEffect::New {
                type_id: type_id.to_owned(),
            }),
        }))
        .map_err(|e| format!("Cannot add a device to {name}: {e}"))?;
        self.doc()
            .devices
            .iter()
            .find(|d| d.type_id == type_id && !before.contains(&d.key))
            .map(|d| d.key.clone())
            .ok_or_else(|| "The device was not added".to_owned())
    }

    /// Applies `commands` after the first step of a fix, folded into it. A refusal undoes the
    /// whole step.
    fn finish_fix(&mut self, depth: usize, commands: Vec<Command>) -> Result<(), String> {
        for command in commands {
            self.join_next = self.document.undo_depth() > depth;
            let result = self.apply_command(command);
            self.join_next = false;
            if let Err(e) = result {
                if self.document.undo_depth() > depth {
                    self.undo();
                }
                return Err(format!("The fix was refused: {e}"));
            }
        }
        Ok(())
    }

    /// Applies the fix of finding `n` in one undo step and says what it did (see the module docs).
    ///
    /// # Errors
    /// There is no such finding or it has no fix, it was fixed already, its track or device is
    /// gone, the chain is full or the model refuses a step; the song is left as it was.
    pub fn fix_mixcheck(&mut self, n: usize) -> Result<(), String> {
        let result = self.fix_inner(n);
        match &result {
            Ok(message) => {
                self.audio.mixcheck.fixed.insert(n);
                self.reject(message.clone());
            }
            Err(why) => self.reject(why.clone()),
        }
        result.map(|_| ())
    }

    fn fix_inner(&mut self, n: usize) -> Result<String, String> {
        let fix = self
            .finding(n)
            .ok_or("Run the mix check first")?
            .fix
            .clone()
            .ok_or("This finding has no fix")?;
        if self.audio.mixcheck.fixed.contains(&n) {
            return Err("This finding was fixed already: run the check again".to_owned());
        }
        match fix {
            Fix::Trim { strip, device, db } => self.fix_trim(&strip, &device, db),
            Fix::CutLowMids { channel, hz } => self.fix_cut(&channel, hz),
            Fix::MonoLow { channel } => self.fix_mono(&channel),
        }
    }

    fn fix_trim(
        &mut self,
        strip: &DeviceKey,
        device: &DeviceKey,
        db: f32,
    ) -> Result<String, String> {
        let name = self.strip_label(strip);
        let chain = insert_chain(self.doc(), &self.registry, strip);
        let slot = chain.iter().position(|k| k == device).ok_or_else(|| {
            format!("The device is no longer on the chain of {name}: run the check again")
        })?;
        let device_name = self
            .doc()
            .device(device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        let depth = self.document.undo_depth();
        let gain = self.add_insert(strip, GAIN_TOOL)?;
        let moved = vec![
            Command::SetParam {
                key: gain.clone(),
                param: "gain".to_owned(),
                value: db,
                merge: None,
            },
            Command::Console(ConsoleCommand::MoveInsert {
                channel: strip.clone(),
                from: chain.len(),
                to: slot,
            }),
        ];
        self.finish_fix(depth, moved)?;
        Ok(format!(
            "Trimmed {db:+.1} dB before {device_name} on {name}: a Gain Tool is insert {} of the chain",
            slot + 1
        ))
    }

    /// A Dynamic EQ on the chain of `channel` with a free bell band.
    fn free_bell(&self, channel: &DeviceKey) -> Option<(DeviceKey, usize)> {
        let doc = self.doc();
        insert_chain(doc, &self.registry, channel)
            .into_iter()
            .filter(|k| doc.device(k).is_some_and(|d| d.type_id == DYNAMIC_EQ))
            .find_map(|k| {
                let band = BELLS.clone().find(|b| {
                    doc.param_value(&k, &format!("band{b}.on"), &self.registry)
                        .is_some_and(|v| v < 0.5)
                })?;
                Some((k, band))
            })
    }

    fn fix_cut(&mut self, channel: &DeviceKey, hz: f32) -> Result<String, String> {
        let name = self.strip_label(channel);
        let depth = self.document.undo_depth();
        let (device, band, added) = match self.free_bell(channel) {
            Some((device, band)) => (device, band, false),
            None => (self.add_insert(channel, DYNAMIC_EQ)?, *BELLS.start(), true),
        };
        let p = |param: &str| format!("band{band}.{param}");
        let set = |param: String, value: f32| Command::SetParam {
            key: device.clone(),
            param,
            value,
            merge: None,
        };
        let commands = vec![
            set(p("freq"), hz.clamp(20.0, 20_000.0)),
            set(p("q"), LOW_MID_CUT_Q),
            set(p("gain"), LOW_MID_CUT_DB),
            set(p("range"), 0.0),
            set(p("source"), 0.0),
            set(p("channel"), 0.0),
            set(p("on"), 1.0),
        ];
        // With a new device its insert starts the step; otherwise the first command does.
        self.finish_fix(depth, commands)?;
        Ok(format!(
            "Cut {:.0} dB at {hz:.0} Hz on {name}: Dynamic EQ band {band}{}",
            -LOW_MID_CUT_DB,
            if added { " (new device)" } else { "" }
        ))
    }

    fn fix_mono(&mut self, channel: &DeviceKey) -> Result<String, String> {
        let name = self.strip_label(channel);
        let depth = self.document.undo_depth();
        let tool = self.add_insert(channel, STEREO_TOOL)?;
        self.finish_fix(
            depth,
            vec![Command::SetParam {
                key: tool,
                param: "mono".to_owned(),
                value: 1.0,
                merge: None,
            }],
        )?;
        Ok(format!(
            "Made {name} mono: a Stereo Tool with Mono on is at the end of its inserts"
        ))
    }
}
