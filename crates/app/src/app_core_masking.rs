//! The masking map in the app core: **Mixer > Masking Map** analyses the channels of the song for
//! tracks that cover each other, and one click turns a finding into a dynamic cut.
//!
//! # Analysis
//!
//! [`AppCore::start_masking`] renders every channel of the console as a stem on a worker thread
//! ([`crate::masking_worker`], through [`atomik_render::masking`]) over the whole song, or over the
//! time selection when there is one, and ranks the pairs of channels by how much the louder one
//! covers the other in a band at the same time ([`atomik_dsp::masking`] has the rules and the
//! score). The result, [`AppCore::masking`], never changes the song. [`AppCore::masking_progress`]
//! and [`AppCore::cancel_masking`] watch and stop it; opening or creating a project drops it.
//! [`AppCore::locate_masking`] puts the playhead where a time range of a pair starts.
//!
//! # De-masking
//!
//! [`AppCore::demask`] takes a pair of the result and a band and makes the covered track yield to
//! the covering one in that band, in **one undo step**:
//!
//! * a Dynamic EQ on the covered channel's insert chain is used when one has a free bell band and
//!   no key, or a key already cabled from the covering channel; otherwise a new one is added at
//!   the end of the chain;
//! * the key of the device is cabled from the covering channel's insert send (the signal before its
//!   fader, as a compressor's key is: see the sidechain helper of the rack) when it is not yet;
//! * a free bell band (2 to 4) is switched on at the centre of the band, listening to the key, with
//!   a cut of half the lead the covering track has there, at least 2 dB and at most 6 dB, that
//!   starts 6 dB under the covering track's average level in the band.
//!
//! The status line says what was done. A pair whose tracks are gone, a chain that is full or has
//! no free band, or a cable the model refuses is reported and leaves the song as it was.

use super::*;
use crate::freeze_worker::PluginEnv;
use crate::masking_worker::{self, MaskAnalysis, Work};
use atomik_dsp::masking::{BANDS, BandMap, PairScore, band_centre, band_label};
use atomik_model::{Cable, ConsoleCommand, InsertEffect, SocketRef, insert_chain};
use atomik_render::export::{ExportSpan, span_frames};
use atomik_ui::{MaskingData, MaskingPair};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The Dynamic EQ's type id.
const DYNAMIC_EQ: &str = "atomik.dynamic-eq";
/// The first and last band of the Dynamic EQ a de-mask may use (the bells).
const BELLS: std::ops::RangeInclusive<usize> = 2..=4;
/// The width of the band, as a Q (a third of an octave is about 4.3; a little wider is gentler).
const CUT_Q: f32 = 3.0;
/// The shallowest and the deepest cut a de-mask asks for, dB.
const MIN_CUT_DB: f32 = 2.0;
const MAX_CUT_DB: f32 = 6.0;
/// How far under the covering track's average level in the band the cut starts, dB.
const THRESHOLD_BELOW_DB: f32 = 6.0;

/// The analysis that runs.
struct Running {
    job: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// The masking state of an open project.
#[derive(Default)]
pub(super) struct MaskingState {
    running: Option<Running>,
    next_job: u64,
    result: Option<MaskAnalysis>,
}

impl MaskingState {
    /// Stops the running analysis and forgets the result (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.result = None;
    }
}

/// What a de-mask did.
#[derive(Debug, Clone, PartialEq)]
pub struct DemaskReport {
    /// The Dynamic EQ that got the band.
    pub device: DeviceKey,
    /// Its band (2 to 4).
    pub band: usize,
    /// Whether the device was added (it was already on the chain otherwise).
    pub added: bool,
    /// The centre of the band, Hz.
    pub freq: f32,
    /// The cut, dB (negative).
    pub range_db: f32,
    /// Where the cut starts, dB.
    pub threshold_db: f32,
}

impl<S: EngineSink> AppCore<S> {
    /// The range the analysis covers: the time selection when there is one, else the song.
    fn masking_span(&self) -> ExportSpan {
        match self.export_selection() {
            Some((start, end)) => ExportSpan::Range(start, end),
            None => ExportSpan::Song,
        }
    }

    /// Starts the analysis of the channels (see the module docs). One runs at a time.
    ///
    /// # Errors
    /// An analysis already runs, the song is empty or has fewer than two channels (the reason is
    /// also in the status line); nothing starts.
    pub fn start_masking(&mut self) -> Result<(), String> {
        let refusal = if self.audio.masking.running.is_some() {
            Some("The masking analysis is already running".to_owned())
        } else if self.doc().console.channels.len() < 2 {
            Some("The masking map compares channels: the song needs at least two".to_owned())
        } else {
            span_frames(self.doc(), self.masking_span())
                .err()
                .map(|e| e.to_string())
        };
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.masking.next_job;
        self.audio.masking.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            span: self.masking_span(),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
        };
        let started = masking_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.masking.running = Some(Running {
                    job,
                    cancel,
                    progress,
                });
                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the analysis: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// Whether an analysis runs.
    pub fn masking_running(&self) -> bool {
        self.audio.masking.running.is_some()
    }

    /// How far the running analysis is, `0.0..=1.0`.
    pub fn masking_progress(&self) -> Option<f32> {
        let r = self.audio.masking.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running analysis.
    pub fn cancel_masking(&mut self) {
        if let Some(r) = &self.audio.masking.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The result of the last analysis of this project.
    pub fn masking(&self) -> Option<&MaskAnalysis> {
        self.audio.masking.result.as_ref()
    }

    /// An analysis finished (or failed, or was stopped).
    pub(super) fn apply_masked(&mut self, job: u64, result: Result<Option<MaskAnalysis>, String>) {
        if self
            .audio
            .masking
            .running
            .take_if(|r| r.job == job)
            .is_none()
        {
            return;
        }
        match result {
            Err(e) => self.reject(format!("Masking analysis failed: {e}")),
            Ok(None) => self.reject("Masking analysis cancelled".to_owned()),
            Ok(Some(found)) => {
                let message = match found.pairs.first() {
                    None => format!("No masking found between {} channels", found.tracks.len()),
                    Some(worst) => format!(
                        "Masking map: {} pair{} found; worst is {} over {} at {} ({:.0}%)",
                        found.pairs.len(),
                        if found.pairs.len() == 1 { "" } else { "s" },
                        found.tracks[worst.masker].name,
                        found.tracks[worst.masked].name,
                        band_label(worst.worst),
                        worst.score,
                    ),
                };
                self.audio.masking.result = Some(found);
                self.reject(message);
            }
        }
    }

    /// The result as the masking panel draws it: names, clash zones and the time ranges in seconds
    /// of the song.
    pub fn masking_data(&self) -> Option<MaskingData> {
        let found = self.audio.masking.result.as_ref()?;
        let rate = f64::from(found.rate.max(1));
        let origin = found.from_frame as f64 / rate;
        let at = |block: usize| origin + BandMap::block_start(block) as f64 / rate;
        let pairs = found
            .pairs
            .iter()
            .map(|p| MaskingPair {
                masker: found.tracks[p.masker].name.clone(),
                masked: found.tracks[p.masked].name.clone(),
                worst: p.worst,
                score: p.score,
                zone: p.zone(),
                masker_db: p.masker_db,
                masked_db: p.masked_db,
                ranges: p.ranges.iter().map(|&(a, b)| (at(a), at(b))).collect(),
            })
            .collect();
        Some(MaskingData {
            tracks: found.tracks.len(),
            pairs,
        })
    }

    /// Puts the playhead at the start of `range` (a half-open run of blocks of pair `pair`).
    pub fn locate_masking(&mut self, pair: usize, range: usize) {
        let Some(found) = self.audio.masking.result.as_ref() else {
            return;
        };
        let Some((from, _)) = found
            .pairs
            .get(pair)
            .and_then(|p| p.ranges.get(range))
            .copied()
        else {
            return;
        };
        let frame = found.from_frame + BandMap::block_start(from);
        let clock = self.doc().song.frame_clock(self.doc().sample_rate);
        let tick = clock.tick_at(frame).max(0);
        self.transport_action(TransportAction::Locate(tick));
    }

    /// Installs a result as if an analysis had found it (tests).
    #[cfg(test)]
    pub(super) fn set_masking_for_test(&mut self, found: MaskAnalysis) {
        self.audio.masking.result = Some(found);
    }

    // ------------------------------------------------------------------ de-masking

    /// The Dynamic EQ on the insert chain of `channel` that can take a band keyed from `masker`:
    /// the free bell band (2 to 4) it has, with the device.
    fn demask_target(&self, channel: &DeviceKey, masker: &DeviceKey) -> Option<(DeviceKey, usize)> {
        let doc = self.doc();
        let chain = insert_chain(doc, &self.registry, channel);
        chain
            .into_iter()
            .filter(|k| doc.device(k).is_some_and(|d| d.type_id == DYNAMIC_EQ))
            .filter(|k| {
                // The key is free or already the masker's.
                let keyed = |socket: &str| {
                    doc.cables
                        .iter()
                        .find(|c| c.to.device == *k && c.to.socket == socket)
                };
                keyed("sc_in_l").is_none_or(|c| c.from.device == *masker)
            })
            .find_map(|k| {
                let band = BELLS.clone().find(|n| {
                    doc.param_value(&k, &format!("band{n}.on"), &self.registry)
                        .is_some_and(|v| v < 0.5)
                })?;
                Some((k, band))
            })
    }

    /// Makes the covered track of pair `pair` of the analysis yield to the covering one in
    /// `band`, in one undo step (see the module docs).
    ///
    /// # Errors
    /// There is no such pair or band, a track of the pair is no longer a channel, the insert chain is
    /// full or the model refuses a step; the song is left as it was.
    pub fn demask(&mut self, pair: usize, band: usize) -> Result<DemaskReport, String> {
        let result = self.demask_inner(pair, band);
        if let Err(why) = &result {
            self.reject(why.clone());
        }
        result
    }

    fn demask_inner(&mut self, pair: usize, band: usize) -> Result<DemaskReport, String> {
        let found = self
            .audio
            .masking
            .result
            .as_ref()
            .ok_or("Run the masking analysis first")?;
        let p: PairScore = found
            .pairs
            .get(pair)
            .ok_or("There is no such pair")?
            .clone();
        if band >= BANDS {
            return Err("There is no such band".to_owned());
        }
        let covered = found.tracks[p.masked].clone();
        let covering = found.tracks[p.masker].clone();
        for t in [&covered, &covering] {
            if self.doc().console.channel_index(&t.channel).is_none() {
                return Err(format!(
                    "{} is no longer a channel: run the analysis again",
                    t.name
                ));
            }
        }
        let freq = band_centre(band).clamp(20.0, 20_000.0);
        let lead = p.masker_db[band] - p.masked_db[band];
        let range_db = -(lead * 0.5).clamp(MIN_CUT_DB, MAX_CUT_DB);
        let threshold_db = (p.masker_db[band] - THRESHOLD_BELOW_DB).clamp(-60.0, -6.0);
        // The device: one on the chain with a free bell and a free (or this) key, else a new one.
        let (device, eq_band, added) = match self.demask_target(&covered.channel, &covering.channel)
        {
            Some((device, eq_band)) => (device, eq_band, false),
            None => {
                let chain = insert_chain(self.doc(), &self.registry, &covered.channel);
                if chain.len() >= atomik_model::INSERT_SLOTS {
                    return Err(format!(
                        "The insert chain of {} is full: free a slot first",
                        covered.name
                    ));
                }
                let before: Vec<DeviceKey> =
                    self.doc().devices.iter().map(|d| d.key.clone()).collect();
                self.apply_command(Command::Console(ConsoleCommand::SetInsert {
                    channel: covered.channel.clone(),
                    slot: chain.len(),
                    effect: Some(InsertEffect::New {
                        type_id: DYNAMIC_EQ.to_owned(),
                    }),
                }))
                .map_err(|e| format!("Cannot add a Dynamic EQ to {}: {e}", covered.name))?;
                let device = self
                    .doc()
                    .devices
                    .iter()
                    .find(|d| d.type_id == DYNAMIC_EQ && !before.contains(&d.key))
                    .map(|d| d.key.clone())
                    .ok_or("The Dynamic EQ was not added")?;
                (device, *BELLS.start(), true)
            }
        };
        let command = self.demask_batch(
            &device,
            &covering.channel,
            eq_band,
            [freq, range_db, threshold_db],
        );
        self.join_next = added;
        let applied = self.apply_command(command);
        self.join_next = false;
        if let Err(e) = applied {
            if added {
                self.undo();
            }
            return Err(format!(
                "Cannot key {} from {}: {e}",
                covered.name, covering.name
            ));
        }
        let report = DemaskReport {
            device,
            band: eq_band,
            added,
            freq,
            range_db,
            threshold_db,
        };
        self.reject(format!(
            "De-masked {} under {}: Dynamic EQ band {} cuts {:.0} dB at {} when {} is above {:.0} dB{}",
            covered.name,
            covering.name,
            report.band,
            -report.range_db,
            band_label(band),
            covering.name,
            report.threshold_db,
            if added { " (new device)" } else { "" },
        ));
        Ok(report)
    }

    /// The batch that cables the key and sets the band.
    fn demask_batch(
        &self,
        device: &DeviceKey,
        masker: &DeviceKey,
        band: usize,
        [freq, range_db, threshold_db]: [f32; 3],
    ) -> Command {
        let doc = self.doc();
        let mut batch = Vec::new();
        for (side, to) in [("l", "sc_in_l"), ("r", "sc_in_r")] {
            let want = Cable {
                from: SocketRef {
                    device: masker.clone(),
                    socket: format!("insert_send_{side}"),
                },
                to: SocketRef {
                    device: device.clone(),
                    socket: to.to_owned(),
                },
            };
            if !doc.cables.contains(&want) {
                batch.push(Command::Connect(want));
            }
        }
        let set = |param: String, value: f32| Command::SetParam {
            key: device.clone(),
            param,
            value,
            merge: None,
        };
        let p = |name: &str| format!("band{band}.{name}");
        batch.extend([
            set(p("freq"), freq),
            set(p("q"), CUT_Q),
            set(p("gain"), 0.0),
            set(p("threshold"), threshold_db),
            set(p("range"), range_db),
            set(p("source"), 1.0),
            set(p("channel"), 0.0),
            set(p("on"), 1.0),
        ]);
        Command::Batch(batch)
    }
}
