//! Mixer snapshots in the app core: the Mixer menu's Store, Update, Recall, Recall Next and
//! Previous, Compare A/B and Delete. The model does the work (`ConsoleCommand::StoreSnapshot` and
//! friends, each one undo step; a recall is ordinary param edits, which the engine smooths); the
//! core picks the snapshot a command means and keeps the one the mix was last recalled from, which
//! the console view uses to light a chip when no snapshot matches the mix exactly.
//!
//! The **current** snapshot is the one a compare is on, else the first the mix matches exactly,
//! else the last one stored, updated, recalled or compared with (while it exists).

use super::*;
use crate::commands::SnapshotMenu;
use atomik_model::{ConsoleCommand, SnapshotScope};

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
}
