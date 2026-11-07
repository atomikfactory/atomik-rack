//! The transition kit in the app core: **Add Transition** at a section boundary.
//!
//! # Where
//!
//! A transition belongs to a boundary, the tick where a section starts. It is the marker or the
//! range a context menu opened on, or the bar line nearest to the pointer when it opened in the
//! ruler; from a menu bar, the selected marker or range, else the first boundary at or after the
//! playhead (the last one before it when there is none after). A boundary at the very start of
//! the song is refused.
//!
//! # What
//!
//! * **Riser** (1, 2 or 4 bars): the first beat of the section, dry, from the track that plays the
//!   first hit ([`atomik_model::transition::first_hit`]), is rendered offline on the freeze
//!   worker, reversed into the large reverb and reversed again, the reverse reverb print of
//!   [`super::rumble`], and the swell is put on a new track so that it ends at the first frame of
//!   the section.
//! * **Impact**: the same first beat, dry, is saturated and goes into a three second reverb
//!   ([`atomik_dsp::transition::impact`]); the print starts on the downbeat and rings on after it.
//! * **Filter Down** (1, 2 or 4 bars) on the master, or **Group Filter Down** on the group bus of
//!   the selected track: a low-pass insert closed by automation over the last bars.
//! * **Silence on Beat One**: the master fader at zero for the first beat of the section.
//! * **Fill**: the last beat before the boundary of the selected note track (else the first note
//!   track that plays through it, a drum machine first) played twice at double rate.
//!
//! The two prints are renders on the freeze worker and queue (see [`crate::freeze_worker`]); the
//! other three are one `AddTransition` command each. Each is one undo step and the status line
//! says what was done.
//!
//! # Counting
//!
//! Each transition is remembered for the session with its boundary and kind. A kind that is on
//! more than half of the boundaries (and on at least three) is named in the status line the next
//! time it is used again: a transition that is everywhere stops being a surprise. An undo takes
//! the transition it undid off the count; a project change starts it again.

use super::*;
use crate::transition_choice::TransitionChoice;
use atomik_model::transition::{
    TransitionEdit, TransitionKind, fill_tracks, first_hit, hit_span, nearest_boundary, output_bus,
    overuse, riser_seconds,
};

/// The reverb of an impact, in seconds.
const IMPACT_DECAY: f32 = 3.0;

impl<S: EngineSink> AppCore<S> {
    /// The bar number (1-based) that holds the boundary `at`, for the status line.
    pub(super) fn transition_bar(&self, at: i64) -> u32 {
        self.doc().song.meter_table().map().bar_at(at)
    }

    /// The boundary a transition goes at (see the module docs).
    fn transition_boundary(&mut self) -> Option<i64> {
        let playhead = self.arrangement.playhead();
        self.arrangement
            .take_transition_boundary()
            .or_else(|| nearest_boundary(&self.doc().song, playhead))
    }

    /// Adds the transition `choice` at the section boundary (see the module docs): a render on
    /// the freeze worker for a riser or an impact, else one edit. Says in the status line what it
    /// did or why it did nothing.
    pub fn add_transition(&mut self, choice: TransitionChoice) {
        let Some(at) = self.transition_boundary() else {
            return self.reject(
                "Select a marker or right-click a bar line in the ruler to add a transition there"
                    .into(),
            );
        };
        if at <= 0 {
            return self.reject("A transition needs a boundary after the start of the song".into());
        }
        match choice {
            TransitionChoice::Riser(bars) => self.print_transition(at, Some(bars)),
            TransitionChoice::Impact => self.print_transition(at, None),
            TransitionChoice::FilterDown(bars) => {
                let Some(master) = self.doc().console.master.clone() else {
                    return self.reject("The project has no master to filter".into());
                };
                let said = format!(
                    "Added a low-pass on the master that closes over the last {} before bar {}",
                    bar_count(bars),
                    self.transition_bar(at)
                );
                let edit = TransitionEdit::FilterDown {
                    target: master,
                    bars,
                };
                self.apply_transition(at, edit, &said);
            }
            TransitionChoice::GroupFilterDown(bars) => {
                let Some((target, name)) = self.group_of_selection() else {
                    return self.reject(
                        "Select a track that plays into a group bus to filter its group \
                         (Mixer > Route Group to New Bus makes one)"
                            .into(),
                    );
                };
                let said = format!(
                    "Added a low-pass on {name} that closes over the last {} before bar {}",
                    bar_count(bars),
                    self.transition_bar(at)
                );
                let edit = TransitionEdit::FilterDown { target, bars };
                self.apply_transition(at, edit, &said);
            }
            TransitionChoice::Silence => {
                let said = format!(
                    "Silenced the master for beat one of bar {} (one beat of fader automation)",
                    self.transition_bar(at)
                );
                self.apply_transition(at, TransitionEdit::Silence, &said);
            }
            TransitionChoice::Fill => {
                let candidates = fill_tracks(self.doc(), at);
                let selected = self.arrangement.command_tracks();
                let Some(track) = selected
                    .into_iter()
                    .find(|k| candidates.contains(k))
                    .or_else(|| candidates.first().cloned())
                else {
                    return self.reject(format!(
                        "No note clip plays through the last beat before bar {}: nothing to fill",
                        self.transition_bar(at)
                    ));
                };
                let said = format!(
                    "Filled the last beat before bar {} on {}: it plays twice at double rate",
                    self.transition_bar(at),
                    self.track_name(&track)
                );
                self.apply_transition(at, TransitionEdit::Fill { track }, &said);
            }
        }
    }

    /// The group bus the first selected track plays into, with its name.
    fn group_of_selection(&self) -> Option<(DeviceKey, String)> {
        let doc = self.doc();
        let bus = self
            .arrangement
            .command_tracks()
            .iter()
            .find_map(|t| output_bus(doc, t))?;
        let name = doc
            .console
            .buses
            .iter()
            .find(|b| b.bus == bus)
            .map_or_else(|| bus.0.clone(), |b| b.name.clone());
        Some((bus, name))
    }

    /// Applies the transition `edit` at `at` as one step and, when it went through, says `said`
    /// and counts it.
    fn apply_transition(&mut self, at: i64, edit: TransitionEdit, said: &str) {
        let kind = edit.kind();
        let command = Command::Workflow(WorkflowCommand::AddTransition { at, edit });
        if self.apply_command(command).is_ok() {
            self.transition_done(at, kind, said);
        }
    }

    /// Counts a transition that was made, and says `said` in the status line, with the warning
    /// when `kind` is now on most of the boundaries.
    pub(super) fn transition_done(&mut self, at: i64, kind: TransitionKind, said: &str) {
        let depth = self.document.undo_depth();
        self.transitions.push((at, kind, depth));
        let uses: Vec<(i64, TransitionKind)> =
            self.transitions.iter().map(|t| (t.0, t.1)).collect();
        match overuse(&self.doc().song, &uses, kind) {
            Some(over) => self.reject(format!("{said}. {}", over.message())),
            None => self.reject(said.to_owned()),
        }
    }

    /// Prints a riser (`Some(bars)`) or an impact (`None`) of the first hit of the section at
    /// `at` on the freeze worker.
    fn print_transition(&mut self, at: i64, bars: Option<u8>) {
        let bar = self.transition_bar(at);
        let Some((device, _)) = first_hit(self.doc(), at) else {
            return self.reject(format!(
                "No track plays a note or an audio clip in the first beat of bar {bar}: there is \
                 no hit to make it from"
            ));
        };
        let name = self.track_name(&device);
        if let Err(why) = track_chain(self.doc(), &self.registry, &device) {
            return self.reject(why);
        }
        let span = hit_span(&self.doc().song, at);
        let kind = match bars {
            Some(bars) => {
                if at <= 0 {
                    return self.reject("There is no room before the start of the song".into());
                }
                Kind::Riser {
                    seconds: riser_seconds(self.doc(), at, bars),
                    anchor: at,
                    bars,
                }
            }
            None => Kind::Impact {
                decay: IMPACT_DECAY,
                at,
            },
        };
        let id = self.next_freeze_job();
        self.audio.freeze.queue.push_back(Job {
            id,
            kind,
            request: FreezeRequest {
                device,
                effects: 0,
                strip: false,
                clips: None,
                span,
                tail_seconds: 0.0,
            },
            name,
        });
        self.start_next_job();
    }
}

/// "1 bar", "2 bars".
fn bar_count(bars: u8) -> String {
    format!("{bars} {}", if bars == 1 { "bar" } else { "bars" })
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn the_bar_count_reads_in_the_singular_for_one() {
        assert_eq!(bar_count(1), "1 bar");
        assert_eq!(bar_count(4), "4 bars");
    }
}
