//! A paste that brings audio from a song at another sample rate.
//!
//! Clips, takes, devices (sampler zones), tracks and strips copied in one song carry the pool
//! entries of their audio. When one of them is at another rate than the song the paste goes
//! into, [`AppCore::convert_pasted_audio`] holds the paste back, resamples that audio on a
//! worker (`atomik-paste-rate`, never the audio thread or the UI thread) and applies the paste
//! once the copies are in the song's bundle and registered: the snapshot is rewritten to name
//! the copies and every position that counts source frames is scaled, so the paste is one undo
//! step and sounds like the original. The original file stays where it was and is never
//! changed; the converted copy goes into the bundle of the song that is pasted into (a scratch
//! bundle until that song has a folder). A copy made once is remembered, so a second paste of
//! the same audio does not convert it again.

use super::*;
use crate::audio_import::{Loaded, spawn_convert_paste};
use atomik_model::{AssetMeta, ModelError, RateCopy, WorkflowCommand, assets_off_rate};
use std::collections::BTreeMap;

/// A paste that waits for its converted audio.
#[derive(Debug)]
struct Waiting {
    /// The paste, as it was asked for.
    command: WorkflowCommand,
    /// The song's rate when it was asked for.
    rate: u32,
    /// The copies made earlier that the paste needs too.
    known: Vec<RateCopy>,
}

/// The pastes waiting for converted audio and the copies made so far.
#[derive(Debug, Default)]
pub(super) struct RatePaste {
    next: u64,
    waiting: BTreeMap<u64, Waiting>,
    /// `(asset, rate)` to the copy of the asset at that rate.
    copies: BTreeMap<(String, u32), RateCopy>,
}

impl RatePaste {
    /// Forgets everything (another song is open now).
    pub(super) fn reset(&mut self) {
        self.waiting.clear();
        self.copies.clear();
    }

    /// How many pastes wait for their audio.
    pub(super) fn waiting(&self) -> usize {
        self.waiting.len()
    }
}

/// The pool entries a paste command brings, `None` for any other command.
fn brought(command: &WorkflowCommand) -> Option<&BTreeMap<String, AssetMeta>> {
    match command {
        WorkflowCommand::PasteDevices { snapshot, .. }
        | WorkflowCommand::PasteDevicesAt { snapshot, .. } => Some(&snapshot.assets),
        WorkflowCommand::PasteClips { clips, .. } => Some(&clips.assets),
        WorkflowCommand::PasteTakes { takes, .. } => Some(&takes.assets),
        _ => None,
    }
}

/// The paste `command` pointing at the copies `copies` of its audio, made for `rate`.
fn retarget(
    command: &WorkflowCommand,
    rate: u32,
    copies: &[RateCopy],
) -> Result<WorkflowCommand, ModelError> {
    Ok(match command {
        WorkflowCommand::PasteDevices {
            snapshot,
            container,
            at,
            route,
        } => WorkflowCommand::PasteDevices {
            snapshot: snapshot.at_rate(rate, copies)?,
            container: container.clone(),
            at: *at,
            route: *route,
        },
        WorkflowCommand::PasteDevicesAt {
            snapshot,
            container,
            slot,
            route,
        } => WorkflowCommand::PasteDevicesAt {
            snapshot: snapshot.at_rate(rate, copies)?,
            container: container.clone(),
            slot: *slot,
            route: *route,
        },
        WorkflowCommand::PasteClips {
            track,
            lane,
            at_tick,
            clips,
        } => WorkflowCommand::PasteClips {
            track: track.clone(),
            lane: *lane,
            at_tick: *at_tick,
            clips: clips.at_rate(rate, copies)?,
        },
        WorkflowCommand::PasteTakes {
            target,
            at_tick,
            takes,
        } => WorkflowCommand::PasteTakes {
            target: target.clone(),
            at_tick: *at_tick,
            takes: takes.at_rate(rate, copies)?,
        },
        other => other.clone(),
    })
}

impl<S: EngineSink> AppCore<S> {
    /// Before a command is applied: a paste whose audio is at another rate than the song's is
    /// held back and made again with converted audio (`None`: it is waiting or was refused, and
    /// the reason is reported); every other command comes back unchanged.
    pub(super) fn convert_pasted_audio(&mut self, command: Command) -> Option<Command> {
        let Command::Workflow(w) = &command else {
            return Some(command);
        };
        let Some(assets) = brought(w) else {
            return Some(command);
        };
        let rate = self.doc().sample_rate;
        let off = assets_off_rate(assets, rate);
        if off.is_empty() {
            return Some(command);
        }
        // What an earlier paste converted is used as it is, when the song still has it.
        let mut known = Vec::new();
        let mut todo = Vec::new();
        for hash in off {
            let copy = self
                .audio
                .rate_paste
                .copies
                .get(&(hash.clone(), rate))
                .filter(|c| {
                    self.doc().assets.contains_key(&c.to) && self.audio.assets.contains_key(&c.to)
                });
            match copy {
                Some(copy) => known.push(copy.clone()),
                None => {
                    let name = assets[&hash].original_name.clone();
                    todo.push((hash, name));
                }
            }
        }
        if todo.is_empty() {
            return match retarget(w, rate, &known) {
                Ok(w) => Some(Command::Workflow(w)),
                Err(e) => {
                    self.reject(format!("paste: {e}"));
                    None
                }
            };
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => {
                self.reject(format!("paste: {e}"));
                return None;
            }
        };
        let rate_paste = &mut self.audio.rate_paste;
        let job = rate_paste.next;
        rate_paste.next += 1;
        let threshold = self.stream_threshold();
        let started = spawn_convert_paste(
            self.audio.tx.clone(),
            self.audio.generation,
            job,
            bundle,
            self.audio.lenders.clone(),
            rate,
            threshold,
            todo,
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.rate_paste.waiting.insert(
                    job,
                    Waiting {
                        command: w.clone(),
                        rate,
                        known,
                    },
                );
                log::info!("converting pasted audio to {rate} Hz");
            }
            Err(e) => self.reject(format!("paste: cannot start the converter: {e}")),
        }
        None
    }

    /// The worker finished: registers the copies and applies the paste that waited for them.
    pub(super) fn apply_paste_converted(
        &mut self,
        job: u64,
        result: Result<Vec<(String, Loaded)>, String>,
    ) {
        let Some(waiting) = self.audio.rate_paste.waiting.remove(&job) else {
            return;
        };
        let converted = match result {
            Ok(c) => c,
            Err(e) => return self.reject(format!("paste: {e}")),
        };
        if self.doc().sample_rate != waiting.rate {
            return self.reject("paste: the sample rate changed meanwhile: paste again".to_owned());
        }
        let mut copies = waiting.known;
        for (from, loaded) in converted {
            let Some(info) = loaded.info.clone() else {
                return self.reject("paste: a converted copy has no asset info".to_owned());
            };
            self.insert_loaded(&loaded);
            let meta = AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            };
            if !self.doc().assets.contains_key(&info.hash)
                && let Err(e) = self.apply_command(Command::RegisterAsset {
                    hash: info.hash.clone(),
                    meta: meta.clone(),
                })
            {
                return self.reject(format!("paste: {e}"));
            }
            let copy = RateCopy {
                from: from.clone(),
                to: info.hash,
                meta,
            };
            self.audio
                .rate_paste
                .copies
                .insert((from, waiting.rate), copy.clone());
            copies.push(copy);
        }
        match retarget(&waiting.command, waiting.rate, &copies) {
            // The command went through the checks once; the rewritten one has audio at the
            // song's rate, so it is applied as any paste.
            Ok(w) => {
                let _ = self.apply_command(Command::Workflow(w));
            }
            Err(e) => self.reject(format!("paste: {e}")),
        }
    }

    /// How many pastes wait for audio converted to the song's rate.
    pub fn pastes_converting(&self) -> usize {
        self.audio.rate_paste.waiting()
    }
}
