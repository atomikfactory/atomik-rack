//! Quantize and groove in the app core: what the quantize panel works on, and its commands.
//!
//! The panel ([`atomik_ui::QuantizeView`]) has no document. The shell asks [`AppCore::quantize_target`]
//! what APPLY would change, and the apply, extract and delete buttons end up here as commands:
//! the piano roll's selected notes become one `QuantizeNotes` (all the notes of its clip when
//! none is selected), the arrangement's selected clips one `ApplyQuantize`, so either is one undo
//! step. The roll is the target while it is the focused pane, the arrangement otherwise (the
//! other one when the first has nothing selected).

use super::*;
use atomik_model::ClipContent;
use atomik_model::groove::builtin_grooves;
use atomik_model::{QuantizeParams, StepResolution};
use atomik_ui::GrooveEntry;

/// What the quantize panel's APPLY works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantizeTarget {
    /// Nothing to quantize.
    None,
    /// Notes of the clip open in the piano roll (`None`: all of them).
    Notes {
        /// The open clip.
        clip: ClipId,
        /// The selected notes, as indices (empty: all notes).
        notes: Vec<usize>,
    },
    /// The note clips selected in the arrangement.
    Clips(Vec<ClipId>),
}

impl QuantizeTarget {
    /// What the panel says under its title.
    pub fn text(&self) -> String {
        match self {
            QuantizeTarget::None => String::new(),
            QuantizeTarget::Notes { notes, .. } if notes.is_empty() => {
                "all notes of the open clip".to_owned()
            }
            QuantizeTarget::Notes { notes, .. } => {
                let s = if notes.len() == 1 { "" } else { "s" };
                format!("{} selected note{s}", notes.len())
            }
            QuantizeTarget::Clips(c) => {
                let s = if c.len() == 1 { "" } else { "s" };
                format!("{} selected clip{s}", c.len())
            }
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    fn is_note_clip(&self, id: ClipId) -> bool {
        self.doc()
            .song
            .clip(id)
            .is_some_and(|(_, _, c)| matches!(c.content, ClipContent::Notes(_)))
    }

    /// What a quantize from the focused `pane` would work on.
    pub fn quantize_target(&self, pane: Pane) -> QuantizeTarget {
        let roll = || {
            let clip = self.piano_clip.filter(|c| self.is_note_clip(*c))?;
            Some(QuantizeTarget::Notes {
                clip,
                notes: self.piano_roll.selected_notes(),
            })
        };
        let arrangement = || {
            let clips: Vec<ClipId> = self
                .arrangement
                .selected_clips()
                .into_iter()
                .filter(|c| self.is_note_clip(*c))
                .collect();
            (!clips.is_empty()).then_some(QuantizeTarget::Clips(clips))
        };
        let found = if pane == Pane::PianoRoll {
            roll().or_else(arrangement)
        } else {
            arrangement().or_else(roll)
        };
        found.unwrap_or(QuantizeTarget::None)
    }

    /// The one note clip a groove can be measured from: the open clip of the piano roll, or the
    /// only note clip selected in the arrangement.
    pub fn groove_source(&self, pane: Pane) -> Option<ClipId> {
        match self.quantize_target(pane) {
            QuantizeTarget::Notes { clip, .. } => Some(clip),
            QuantizeTarget::Clips(c) if c.len() == 1 => Some(c[0]),
            _ => None,
        }
    }

    /// The grooves the panel offers: the built-in ones, then the song's.
    pub fn groove_entries(&self) -> Vec<GrooveEntry> {
        let entry = |g: &atomik_model::Groove, builtin| GrooveEntry {
            name: g.name.clone(),
            step: g.step,
            slots: g.slots.len(),
            builtin,
        };
        builtin_grooves()
            .iter()
            .map(|g| entry(g, true))
            .chain(self.doc().song.grooves.iter().map(|g| entry(g, false)))
            .collect()
    }

    /// Quantizes the target of `pane` with `params`, as one undo step. A rejection (for
    /// instance "quantize changes no note") is reported.
    pub fn apply_quantize(&mut self, pane: Pane, params: QuantizeParams) {
        let command = match self.quantize_target(pane) {
            QuantizeTarget::None => return self.reject("nothing to quantize".to_owned()),
            QuantizeTarget::Notes { clip, notes } if !notes.is_empty() => {
                SongCommand::QuantizeNotes {
                    clip,
                    notes,
                    quantize: params,
                }
            }
            QuantizeTarget::Notes { clip, .. } => SongCommand::ApplyQuantize {
                clips: vec![clip],
                quantize: params,
            },
            QuantizeTarget::Clips(clips) => SongCommand::ApplyQuantize {
                clips,
                quantize: params,
            },
        };
        let _ = self.apply_command(Command::Song(command));
    }

    /// Measures a groove from the source clip of `pane` and adds it to the song under the first
    /// free name "Groove N"; returns the name. A rejection is reported.
    pub fn extract_groove(
        &mut self,
        pane: Pane,
        step: StepResolution,
        slots: usize,
    ) -> Option<String> {
        let Some(clip) = self.groove_source(pane) else {
            self.reject("select one note clip to measure a groove from".to_owned());
            return None;
        };
        let taken = |n: &str| self.doc().song.groove(n).is_some();
        let name = (1..)
            .map(|i| format!("Groove {i}"))
            .find(|n| !taken(n))
            .expect("an unused name");
        let command = SongCommand::ExtractGroove {
            clip,
            name: name.clone(),
            resolution: step,
            slots,
        };
        self.apply_command(Command::Song(command))
            .ok()
            .map(|()| name)
    }

    /// Removes the song's groove `name` (one undo step).
    pub fn delete_groove(&mut self, name: &str) {
        let command = SongCommand::DeleteGroove {
            name: name.to_owned(),
        };
        let _ = self.apply_command(Command::Song(command));
    }
}
