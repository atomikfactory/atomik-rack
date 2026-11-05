//! Mixer snapshots in the app core: the Mixer menu's Store, Update, Recall, Recall Next and
//! Previous, Compare A/B and Delete. The model does the work (`ConsoleCommand::StoreSnapshot` and
//! friends, each one undo step; a recall is ordinary param edits, which the engine smooths); the
//! core picks the snapshot a command means and keeps the one the mix was last recalled from, which
//! the console view uses to light a chip when no snapshot matches the mix exactly.
//!
//! The **current** snapshot is the one a compare is on, else the first the mix matches exactly,
//! else the last one stored, updated, recalled or compared with (while it exists).
//!
//! A **morph** moves the mix from the current snapshot to another over a time (1, 2, 4 or 8
//! seconds, Mixer > Morph Time). The core steps it once per frame ([`AppCore::transport_frame`]):
//! each step is the model's `MorphSnapshots` at the position the clock gives, ordinary param edits
//! the engine smooths, all folded into one undo step. Any other edit takes the mix over and ends
//! the morph where it is; a morph does not start while another plays or a compare is on.

use super::*;
use crate::commands::SnapshotMenu;
use atomik_model::{ConsoleCommand, SnapshotScope};

/// The morph time a new core starts with, in seconds.
pub(super) const DEFAULT_MORPH_SECONDS: f32 = 2.0;

/// A morph that is playing.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SnapshotMorph {
    from: u32,
    to: u32,
    seconds: f64,
    /// The clock reading of the first step.
    started: Option<f64>,
    /// Steps applied so far (the later ones join the first one's undo step).
    steps: u32,
}

impl<S: EngineSink> AppCore<S> {
    /// The hint that follows `command` once it succeeds: the snapshot it stored, recalled,
    /// updated or compared with, or none when it deleted the hinted one. `None` leaves the hint.
    pub(super) fn snapshot_hint_after(&self, command: &Command) -> Option<Option<u32>> {
        let Command::Console(c) = command else {
            return None;
        };
        match c {
            ConsoleCommand::StoreSnapshot { .. } => {
                Some(Some(self.doc().console.next_snapshot_id()))
            }
            ConsoleCommand::RecallSnapshot { id, .. }
            | ConsoleCommand::UpdateSnapshot { id }
            | ConsoleCommand::Compare { id } => Some(Some(*id)),
            ConsoleCommand::DeleteSnapshot { id } if self.snapshot_hint == Some(*id) => Some(None),
            _ => None,
        }
    }

    /// The snapshot that counts as current (see the module documentation).
    pub fn current_snapshot(&self) -> Option<u32> {
        let doc = self.doc();
        if let Some(c) = &doc.console.compare {
            return Some(c.snapshot);
        }
        doc.console
            .snapshots
            .iter()
            .find(|s| doc.snapshot_matches(s.id, &self.registry, SnapshotScope::ALL))
            .map(|s| s.id)
            .or(self
                .snapshot_hint
                .filter(|h| doc.console.snapshot(*h).is_some()))
    }

    /// What the Mixer menu's snapshot items can do now.
    pub fn snapshot_menu(&self) -> SnapshotMenu {
        let doc = self.doc();
        let console = &doc.console;
        let current = self.current_snapshot();
        SnapshotMenu {
            can_store: console.snapshots.len() < atomik_model::MAX_SNAPSHOTS,
            can_update: current.is_some_and(|id| {
                console.compare.is_none()
                    && !doc.snapshot_matches(id, &self.registry, SnapshotScope::ALL)
            }),
            can_recall: current.is_some(),
            can_cycle: !console.snapshots.is_empty(),
            can_compare: !console.snapshots.is_empty(),
            comparing: console.compare.is_some(),
            can_delete: current.is_some(),
            can_morph: console.snapshots.len() > 1
                && current.is_some()
                && console.compare.is_none()
                && self.snapshot_morph.is_none(),
            can_effects: current.is_some() && console.compare.is_none(),
            effects: current
                .and_then(|id| console.snapshot(id))
                .is_some_and(|s| s.state.effects),
            morph_seconds: self.morph_seconds.round().clamp(1.0, 8.0) as u8,
        }
    }

    /// Mixer > Store Snapshot: the mix as a new snapshot called `name` (trimmed), or the next
    /// of `A`, `B`, `1`, `2`, ... when none is given, in one undo step. False, with a message,
    /// when the model refuses (a taken name, sixteen snapshots).
    pub fn store_snapshot(&mut self, name: Option<String>) -> bool {
        let name = match name {
            Some(n) => n,
            None => self.doc().console.default_snapshot_name(),
        };
        self.apply_command(Command::Console(ConsoleCommand::StoreSnapshot { name }))
            .is_ok()
    }

    /// The snapshot `id`, or the current one; a message and `None` when there is none.
    fn snapshot_or_current(&mut self, id: Option<u32>, what: &str) -> Option<u32> {
        let id = id.or_else(|| self.current_snapshot());
        if id.is_none() {
            self.reject(format!("Store or recall a snapshot first to {what} it"));
        }
        id
    }

    /// Mixer > Update Snapshot: overwrites snapshot `id` (the current one by default) with the
    /// mix, in one undo step.
    pub fn update_snapshot(&mut self, id: Option<u32>) -> bool {
        match self.snapshot_or_current(id, "update") {
            Some(id) => self
                .apply_command(Command::Console(ConsoleCommand::UpdateSnapshot { id }))
                .is_ok(),
            None => false,
        }
    }

    /// Mixer > Recall Snapshot: applies snapshot `id` (the current one by default, which puts
    /// back what was moved since) in one undo step; it ends a compare. Recalling the mix that is
    /// already there is not a step, and not an error.
    pub fn recall_snapshot(&mut self, id: Option<u32>) -> bool {
        match self.snapshot_or_current(id, "recall") {
            Some(id) => self.recall_scoped(id, SnapshotScope::ALL),
            None => false,
        }
    }

    /// Recalls the categories of `scope` of snapshot `id` (see [`Self::recall_snapshot`]).
    pub fn recall_scoped(&mut self, id: u32, scope: SnapshotScope) -> bool {
        if self.doc().console.compare.is_none()
            && self.doc().snapshot_matches(id, &self.registry, scope)
        {
            self.snapshot_hint = Some(id);
            self.console.set_snapshot_hint(Some(id));
            return true;
        }
        self.apply_command(Command::Console(ConsoleCommand::RecallSnapshot {
            id,
            scope,
        }))
        .is_ok()
    }

    /// Mixer > Recall Next Snapshot / Recall Previous Snapshot: the neighbour of the current
    /// snapshot in the list, wrapping round; the first (next) or the last (previous) when none is
    /// current.
    pub fn recall_adjacent(&mut self, forward: bool) -> bool {
        let ids: Vec<u32> = self.doc().console.snapshots.iter().map(|s| s.id).collect();
        if ids.is_empty() {
            self.reject("There are no snapshots to recall".into());
            return false;
        }
        let n = ids.len();
        let next = match self
            .current_snapshot()
            .and_then(|c| ids.iter().position(|i| *i == c))
        {
            Some(at) if forward => ids[(at + 1) % n],
            Some(at) => ids[(at + n - 1) % n],
            None if forward => ids[0],
            None => ids[n - 1],
        };
        self.recall_snapshot(Some(next))
    }

    /// Mixer > Compare A/B: starts comparing with snapshot `id` (the one being compared with,
    /// else the current one, else the first), or flips between the two mixes when a compare is on.
    pub fn compare_snapshot(&mut self, id: Option<u32>) -> bool {
        let console = &self.doc().console;
        let target = id
            .or(console.compare.as_ref().map(|c| c.snapshot))
            .or_else(|| self.current_snapshot())
            .or_else(|| console.snapshots.first().map(|s| s.id));
        match target {
            Some(id) => self
                .apply_command(Command::Console(ConsoleCommand::Compare { id }))
                .is_ok(),
            None => {
                self.reject("Store a snapshot first to compare with it".into());
                false
            }
        }
    }

    /// Mixer > End Compare: keeps the mix that is live, whichever side it is.
    pub fn end_compare(&mut self) -> bool {
        self.apply_command(Command::Console(ConsoleCommand::EndCompare))
            .is_ok()
    }

    /// Mixer > Delete Snapshot: snapshot `id` (the current one by default).
    pub fn delete_snapshot(&mut self, id: Option<u32>) -> bool {
        match self.snapshot_or_current(id, "delete") {
            Some(id) => self
                .apply_command(Command::Console(ConsoleCommand::DeleteSnapshot { id }))
                .is_ok(),
            None => false,
        }
    }

    /// Mixer > Snapshot Includes Effects: switches the effect parameters of snapshot `id` (the
    /// current one by default) on or off, in one undo step. Turning them on takes the effects as
    /// they are now.
    pub fn toggle_snapshot_effects(&mut self, id: Option<u32>) -> bool {
        let Some(id) = self.snapshot_or_current(id, "change") else {
            return false;
        };
        let Some(held) = self.doc().console.snapshot(id).map(|s| s.state.effects) else {
            return false;
        };
        self.apply_command(Command::Console(ConsoleCommand::SetSnapshotEffects {
            id,
            effects: !held,
        }))
        .is_ok()
    }

    /// Mixer > Morph Time: how long a morph takes, in seconds (clamped to 1..=8).
    pub fn set_morph_seconds(&mut self, seconds: u8) {
        self.morph_seconds = f32::from(seconds.clamp(1, 8));
    }

    /// Mixer > Morph to Next / Previous Snapshot, and the chip menu's Morph To: moves the mix
    /// from the current snapshot to snapshot `to` (the neighbour of the current one when none is
    /// given, `forward` picking which) over the morph time. False, with a message, when it
    /// cannot start.
    pub fn morph_snapshot(&mut self, to: Option<u32>, forward: bool) -> bool {
        if self.snapshot_morph.is_some() {
            self.reject("A morph is already playing; edit the mix to stop it".into());
            return false;
        }
        if self.doc().console.compare.is_some() {
            self.reject("End the compare before morphing".into());
            return false;
        }
        let ids: Vec<u32> = self.doc().console.snapshots.iter().map(|s| s.id).collect();
        let Some(from) = self.current_snapshot() else {
            self.reject("Store or recall a snapshot first to morph from it".into());
            return false;
        };
        let to = match to {
            Some(to) => to,
            None => {
                let at = ids.iter().position(|i| *i == from).unwrap_or(0);
                let n = ids.len();
                ids[if forward {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                }]
            }
        };
        if to == from || !ids.contains(&to) {
            self.reject("A morph needs a second snapshot to move to".into());
            return false;
        }
        self.snapshot_morph = Some(SnapshotMorph {
            from,
            to,
            seconds: f64::from(self.morph_seconds),
            started: None,
            steps: 0,
        });
        true
    }

    /// Whether a morph is playing (the event loop keeps its frame rate for it).
    pub fn snapshot_morphing(&self) -> bool {
        self.snapshot_morph.is_some()
    }

    /// One frame of a playing morph at `now_seconds` (any monotonic clock): the mix goes to the
    /// position the elapsed time gives. A step that would change nothing is skipped, and the
    /// morph ends at its end, or when the model refuses a step (a snapshot deleted meanwhile).
    pub(super) fn snapshot_morph_tick(&mut self, now_seconds: f64) {
        let Some(m) = self.snapshot_morph.as_mut() else {
            return;
        };
        let started = *m.started.get_or_insert(now_seconds);
        let at = ((now_seconds - started) / m.seconds).clamp(0.0, 1.0) as f32;
        let (from, to, steps) = (m.from, m.to, m.steps);
        let done = at >= 1.0;
        if !self.doc().morph_matches(&self.registry, from, to, at) {
            self.join_next = steps > 0;
            let applied = self.apply_command(Command::Console(ConsoleCommand::MorphSnapshots {
                from,
                to,
                at,
            }));
            self.join_next = false;
            match applied {
                Ok(()) => {
                    if let Some(m) = self.snapshot_morph.as_mut() {
                        m.steps += 1;
                    }
                }
                Err(_) => {
                    self.snapshot_morph = None;
                    return;
                }
            }
        }
        if done {
            self.snapshot_morph = None;
            self.snapshot_hint = Some(to);
            self.console.set_snapshot_hint(Some(to));
        }
    }
}
