//! The parameters a plugin writes during `process` (output parameters), as the processor
//! reports them to the app.
//!
//! A plugin may write a queue for any parameter in any block, and a metering plugin does so for
//! every block whether or not the value moved. Posting each of them would fill the message ring
//! with values nobody needs, so the processor keeps one table: the parameters of the layout the
//! host shows (sorted when the plugin is activated, off the audio thread), the latest value
//! written for each, the last value reported, and the list of those that differ. A pending value
//! is reported at most once every [`OUT_PARAM_EVERY`] blocks, and stays pending when the ring
//! refuses it. Everything is sized at activation: nothing here allocates while processing.

use atomik_core::BATCH;

/// Blocks between two reports of the pending values: about 20 ms at 48 kHz. Counted in blocks,
/// never read from a clock, so a bounce and a live run report at the same blocks.
pub(crate) const OUT_PARAM_EVERY: u32 = (960 / BATCH) as u32;

/// The output-parameter table of one processor.
pub(crate) struct OutParams {
    /// The layout's parameter ids, sorted.
    ids: Vec<u32>,
    /// The latest host value written for each id.
    latest: Vec<f64>,
    /// The last host value reported for each id (NaN: none yet).
    posted: Vec<f64>,
    /// Whether the id is on the pending list.
    queued: Vec<bool>,
    /// The indices of the ids whose latest value was not reported yet.
    pending: Vec<u32>,
    /// Blocks since the pending values were last offered to the ring.
    since: u32,
}

impl OutParams {
    /// A table for the parameters `ids` (any order; duplicates are one).
    pub(crate) fn new(mut ids: Vec<u32>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        let n = ids.len();
        Self {
            ids,
            latest: vec![0.0; n],
            posted: vec![f64::NAN; n],
            queued: vec![false; n],
            pending: Vec::with_capacity(n),
            // The first value is reported at once.
            since: OUT_PARAM_EVERY,
        }
    }

    /// The index of `id`, when the layout has it.
    pub(crate) fn index_of(&self, id: u32) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    /// The plugin wrote `value` (the host's value) for the parameter at `index`.
    pub(crate) fn note(&mut self, index: usize, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.latest[index] = value;
        if !self.queued[index] && value.to_bits() != self.posted[index].to_bits() {
            self.queued[index] = true;
            self.pending.push(index as u32);
        }
    }

    /// The app set `value` for `id` itself (an edit made in the plugin's window): it is what the
    /// app knows, so a value the plugin wrote before it is not reported over it.
    pub(crate) fn supersede(&mut self, id: u32, value: f64) {
        if let Some(index) = self.index_of(id)
            && value.is_finite()
        {
            self.latest[index] = value;
            self.posted[index] = value;
        }
    }

    /// Counts one block and tells whether the pending values are to be offered now.
    pub(crate) fn due(&mut self) -> bool {
        self.since = self.since.saturating_add(1);
        !self.pending.is_empty() && self.since >= OUT_PARAM_EVERY
    }

    /// Offers the pending values to `send` (id, value) in the order they became pending; it
    /// answers false when the ring has no room, and that value and the ones after it stay pending.
    pub(crate) fn flush(&mut self, mut send: impl FnMut(u32, f64) -> bool) {
        self.since = 0;
        let mut kept = 0;
        let mut refused = false;
        for k in 0..self.pending.len() {
            let index = self.pending[k] as usize;
            if !refused {
                let value = self.latest[index];
                if value.to_bits() == self.posted[index].to_bits() {
                    // Back to what the app already has.
                    self.queued[index] = false;
                    continue;
                }
                if send(self.ids[index], value) {
                    self.posted[index] = value;
                    self.queued[index] = false;
                    continue;
                }
                refused = true;
            }
            self.pending[kept] = index as u32;
            kept += 1;
        }
        self.pending.truncate(kept);
    }

    /// How many values wait to be reported.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flushed(t: &mut OutParams) -> Vec<(u32, f64)> {
        let mut sent = Vec::new();
        t.flush(|id, v| {
            sent.push((id, v));
            true
        });
        sent
    }

    /// Runs blocks until the table says the pending values are due.
    fn wait_due(t: &mut OutParams) -> u32 {
        let mut blocks = 1;
        while !t.due() {
            blocks += 1;
            assert!(blocks < 1_000, "never due");
        }
        blocks
    }

    #[test]
    fn only_the_ids_of_the_layout_have_an_index() {
        let t = OutParams::new(vec![9, 3, 3, 5]);
        assert_eq!(t.index_of(3), Some(0));
        assert_eq!(t.index_of(5), Some(1));
        assert_eq!(t.index_of(9), Some(2));
        assert_eq!(t.index_of(4), None);
    }

    #[test]
    fn a_value_rewritten_unchanged_is_reported_once() {
        let mut t = OutParams::new(vec![1]);
        for _ in 0..50 {
            t.note(0, 0.25);
            if t.due() {
                let sent = flushed(&mut t);
                assert!(sent.len() <= 1);
            }
        }
        assert_eq!(t.pending(), 0);
        let mut again = OutParams::new(vec![1]);
        again.note(0, 0.25);
        assert!(again.due());
        assert_eq!(flushed(&mut again), vec![(1, 0.25)]);
        again.note(0, 0.25);
        assert!(!again.due(), "nothing pending, nothing to offer");
        again.note(0, 0.5);
        assert!(wait_due(&mut again) <= OUT_PARAM_EVERY);
        assert_eq!(flushed(&mut again), vec![(1, 0.5)]);
    }

    #[test]
    fn the_latest_value_of_a_param_is_the_one_reported() {
        let mut t = OutParams::new(vec![1, 2]);
        t.note(0, 0.1);
        t.note(1, 0.2);
        t.note(0, 0.3);
        assert!(t.due());
        assert_eq!(flushed(&mut t), vec![(1, 0.3), (2, 0.2)]);
    }

    #[test]
    fn values_are_reported_at_most_once_per_period() {
        let mut t = OutParams::new(vec![1]);
        t.note(0, 0.1);
        assert!(t.due());
        flushed(&mut t);
        t.note(0, 0.2);
        let mut blocks = 0;
        while !t.due() {
            blocks += 1;
            t.note(0, 0.2 + f64::from(blocks) * 1e-3);
        }
        assert_eq!(blocks + 1, OUT_PARAM_EVERY);
    }

    #[test]
    fn a_refused_value_stays_pending_with_what_is_written_next() {
        let mut t = OutParams::new(vec![1, 2]);
        t.note(0, 0.1);
        t.note(1, 0.2);
        assert!(t.due());
        t.flush(|_, _| false);
        assert_eq!(t.pending(), 2);
        // The plugin writes nothing more; the values are offered again a period later.
        wait_due(&mut t);
        let mut seen = Vec::new();
        t.flush(|id, v| {
            seen.push((id, v));
            id == 1
        });
        assert_eq!(seen, vec![(1, 0.1), (2, 0.2)]);
        assert_eq!(t.pending(), 1, "the refused one is still waiting");
        t.note(1, 0.7);
        wait_due(&mut t);
        assert_eq!(flushed(&mut t), vec![(2, 0.7)]);
    }

    #[test]
    fn a_value_that_went_back_to_the_reported_one_is_not_sent() {
        let mut t = OutParams::new(vec![1]);
        t.note(0, 0.1);
        assert!(t.due());
        flushed(&mut t);
        t.note(0, 0.9);
        t.note(0, 0.1);
        wait_due(&mut t);
        assert!(flushed(&mut t).is_empty());
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_value_the_app_set_is_not_overwritten_by_an_older_written_one() {
        let mut t = OutParams::new(vec![1]);
        t.note(0, 0.4);
        t.supersede(1, 0.8);
        wait_due(&mut t);
        assert!(flushed(&mut t).is_empty(), "0.4 is older than the edit");
        t.note(0, 0.5);
        wait_due(&mut t);
        assert_eq!(flushed(&mut t), vec![(1, 0.5)]);
    }

    #[test]
    fn a_value_that_is_not_a_number_is_ignored() {
        let mut t = OutParams::new(vec![1]);
        t.note(0, f64::NAN);
        t.note(0, f64::INFINITY);
        assert_eq!(t.pending(), 0);
    }
}
