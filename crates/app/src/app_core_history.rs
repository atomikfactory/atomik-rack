//! The Undo History panel in the core: the list of undo steps, kept in step with the document
//! while it shows, and the jump to any of them.
//!
//! The model names every step and gives it a stamp ([`atomik_model::HistoryStep`]) but reads no
//! clock. The core dates the steps itself: each time it looks at the history it gives the
//! steps it has not seen before the time of that look ([`StepTimes`]), which is the time they
//! were made to within one edit. The times are not saved with the project.
//!
//! A click on a row goes to the state right after that step with repeated [`AppCore::undo`] and
//! [`AppCore::redo`] calls, so the engine sync, the plugin states and the dirty flag are the
//! ones a key press would have; the views sync once at the end.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_ui::{HistoryRow, HistoryView};
use std::collections::BTreeMap;
use std::time::Instant;

/// How often, in seconds, the relative times of the rows are worked out again.
const TIME_BUCKET: f64 = 20.0;

/// The clock that dates the undo steps: seconds since the core started.
pub(super) struct HistoryClock {
    start: Instant,
    /// A fixed time for tests (`None`: the real clock).
    fixed: Option<f64>,
}

impl HistoryClock {
    #[allow(clippy::disallowed_methods)]
    // Justification: the times only label rows of the history list; no audio, no saved state
    // and no edit depends on them.
    pub(super) fn start() -> Self {
        Self {
            start: Instant::now(),
            fixed: None,
        }
    }

    /// Seconds since the clock started.
    pub(super) fn now(&self) -> f64 {
        self.fixed
            .unwrap_or_else(|| self.start.elapsed().as_secs_f64())
    }
}

/// When the steps were seen being made: every look at the history that found new steps adds
/// the time of the look under the newest stamp it found.
#[derive(Default)]
pub(super) struct StepTimes {
    batches: BTreeMap<u64, f64>,
    /// The newest stamp seen so far.
    seen: u64,
}

impl StepTimes {
    /// Notes that steps up to `newest` exist at `now` (nothing when none is new); `oldest` is
    /// the oldest stamp still in the history, so older looks can go.
    fn note(&mut self, oldest: u64, newest: u64, now: f64) {
        if newest > self.seen {
            self.batches.insert(newest, now);
            self.seen = newest;
        }
        self.batches = self.batches.split_off(&oldest);
    }

    /// The time of the look that found the step with `stamp`.
    fn at(&self, stamp: u64) -> Option<f64> {
        self.batches.range(stamp..).next().map(|(_, t)| *t)
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The Undo History panel.
    pub fn history(&self) -> &HistoryView {
        &self.history
    }

    /// The Undo History panel, mutably (bounds, scale).
    pub fn history_mut(&mut self) -> &mut HistoryView {
        &mut self.history
    }

    /// Whether the Undo History panel is shown.
    pub fn history_visible(&self) -> bool {
        self.history_on
    }

    /// Shows or hides the Undo History panel; showing it builds the list.
    pub fn set_history_visible(&mut self, visible: bool) {
        self.history_on = visible;
        self.history_stamp = None;
        self.sync_history();
    }

    /// Fixes the time the undo steps are dated with (seconds; `None`: the real clock), for
    /// tests.
    #[doc(hidden)]
    pub fn set_history_clock(&mut self, now: Option<f64>) {
        self.history_clock.fixed = now;
        self.history_stamp = None;
    }

    /// Dates the steps made since the last look.
    pub(super) fn note_history_times(&mut self) {
        let now = self.history_clock.now();
        let doc = &self.document;
        if let (Some(first), Some(last)) = (doc.history().next(), doc.history().next_back()) {
            self.history_times.note(first.stamp, last.stamp, now);
        }
    }

    /// Rebuilds the list when it shows and the history, the time or the file changed since the
    /// last build. The steps are dated whether it shows or not.
    pub fn sync_history(&mut self) {
        self.note_history_times();
        if !self.history_on {
            return;
        }
        let now = self.history_clock.now();
        let stamp = (
            self.document.revision(),
            (now / TIME_BUCKET).floor() as u64,
            self.file.is_some(),
        );
        if self.history_stamp == Some(stamp) {
            return;
        }
        let age = |at: f64| (now - at).max(0.0) as u64;
        let rows: Vec<HistoryRow> = self
            .document
            .history()
            .map(|step| HistoryRow {
                label: step.label.to_owned(),
                age_secs: self.history_times.at(step.stamp).map(age),
            })
            .collect();
        let start = if self.file.is_some() {
            "Opened"
        } else {
            "New song"
        };
        self.history.sync(
            start,
            Some(age(self.history_opened)),
            rows,
            self.document.history_position(),
        );
        self.history_stamp = Some(stamp);
    }

    /// Goes to the state right after the `row`-th undo step (0: the state the song started in)
    /// by undoing or redoing as many steps as it takes. Stops early when a step cannot be
    /// undone or redone (the toast says why). Returns whether the position changed.
    pub fn history_goto(&mut self, row: usize) -> bool {
        let target = row.min(self.document.history_len());
        let from = self.document.history_position();
        while self.document.history_position() > target {
            if !self.undo() {
                break;
            }
        }
        while self.document.history_position() < target {
            if !self.redo() {
                break;
            }
        }
        self.document.end_merge();
        self.after_view_change();
        self.document.history_position() != from
    }
}
