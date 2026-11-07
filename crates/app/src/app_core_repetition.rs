//! Find Repetition in the app core: **Edit > Find Repetition** lists the ideas the song plays three
//! times or more unchanged ([`atomik_model::repetition`] has the rules) and offers three
//! variations for each.
//!
//! [`AppCore::find_repetition`] searches the song (a pure pass over the model, no worker) and plans
//! the variations against the document. [`AppCore::apply_repetition`] runs one variation that is
//! an edit as **one undo step** and says in the status line what it did; the song is searched
//! again afterwards, because an edit changes the run it was made at, and the variations made so
//! far stay marked for the idea they were made at (its track and start). A variation that is
//! advice only never changes the song. Searching never changes the song.

use super::*;
use atomik_model::repetition::{Idea, Variation, VariationKind, find, variations};
use atomik_ui::{RepetitionChoice, RepetitionData, RepetitionRow};
use std::collections::BTreeSet;

/// What the finder found, with the variations planned for it.
#[derive(Default)]
pub(super) struct RepetitionState {
    /// `None` until the song is searched.
    found: Option<Vec<(Idea, Vec<Variation>)>>,
    /// The tracks that were searched.
    tracks: usize,
    /// The variations made, by the track and start of the idea they were made at.
    done: BTreeSet<(DeviceKey, i64, u8)>,
}

impl RepetitionState {
    /// Forgets everything (the project changed).
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The index of a kind, for the set of made variations.
fn slot(kind: VariationKind) -> u8 {
    VariationKind::ALL
        .iter()
        .position(|k| *k == kind)
        .map_or(0, |i| i as u8)
}

impl<S: EngineSink> AppCore<S> {
    /// Searches the song and plans the variations. Returns how many ideas there are.
    pub fn find_repetition(&mut self) -> usize {
        let doc = self.document.doc();
        let found: Vec<(Idea, Vec<Variation>)> = find(&doc.song)
            .into_iter()
            .map(|i| {
                let v = variations(doc, &self.registry, &i);
                (i, v)
            })
            .collect();
        let count = found.len();
        let tracks = doc
            .song
            .tracks
            .iter()
            .filter(|t| !t.is_automation_only())
            .count();
        self.repetition.found = Some(found);
        self.repetition.tracks = tracks;
        count
    }

    /// Whether the song was searched.
    pub fn repetition_searched(&self) -> bool {
        self.repetition.found.is_some()
    }

    /// The ideas as the repetition panel draws them (`None` before the first search).
    pub fn repetition_data(&self) -> Option<RepetitionData> {
        let found = self.repetition.found.as_ref()?;
        let doc = self.doc();
        let rows = found
            .iter()
            .map(|(idea, vars)| {
                let (first, last) = idea.bars(&doc.song);
                let bars = if first == last {
                    format!("bar {first}")
                } else {
                    format!("bars {first}-{last}")
                };
                RepetitionRow {
                    track: atomik_ui::seq_paint::track_name(doc, &idea.track),
                    what: idea.what(),
                    repeats: idea.repeats,
                    bars,
                    choices: vars
                        .iter()
                        .map(|v| RepetitionChoice {
                            label: v.kind.label().to_owned(),
                            text: v.text.clone(),
                            can_apply: v.command.is_some(),
                            done: self.repetition.done.contains(&(
                                idea.track.clone(),
                                idea.start,
                                slot(v.kind),
                            )),
                        })
                        .collect(),
                }
            })
            .collect();
        Some(RepetitionData {
            tracks: self.repetition.tracks,
            rows,
        })
    }

    /// Puts the playhead at the start of idea `n`.
    pub fn locate_repetition(&mut self, n: usize) {
        let Some(start) = self
            .repetition
            .found
            .as_ref()
            .and_then(|f| f.get(n))
            .map(|(i, _)| i.start)
        else {
            return;
        };
        self.transport_action(TransportAction::Locate(start.max(0)));
    }

    /// Makes variation `k` of idea `n` in one undo step and searches again.
    ///
    /// # Errors
    /// The idea or the variation is not there, the variation is advice only, or the model
    /// refuses the edit (the reason is also in the status line); the song is unchanged.
    pub fn apply_repetition(&mut self, n: usize, k: usize) -> Result<(), String> {
        let picked = self
            .repetition
            .found
            .as_ref()
            .and_then(|f| f.get(n))
            .and_then(|(idea, vars)| Some((idea.clone(), vars.get(k)?.clone())));
        let Some((idea, variation)) = picked else {
            return Err("that suggestion is not there any more".to_owned());
        };
        let Some(command) = variation.command else {
            self.report(variation.text.clone());
            return Err(variation.text);
        };
        let (first, last) = idea.bars(&self.doc().song);
        let track = atomik_ui::seq_paint::track_name(self.doc(), &idea.track);
        if let Err(e) = self.apply_command(command) {
            let why = format!("Cannot make the variation: {e}");
            self.report(why.clone());
            return Err(why);
        }
        self.repetition
            .done
            .insert((idea.track.clone(), idea.start, slot(variation.kind)));
        self.find_repetition();
        self.report(format!(
            "{track}, bars {first}-{last}: {}",
            variation.text.to_lowercase()
        ));
        Ok(())
    }
}
