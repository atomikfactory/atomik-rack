//! The decision whether a dependency level is worth handing to the render workers.
//!
//! Handing a level over costs queue hand-offs, a possible wake-up, the barrier wait and the
//! migration of every node's data to another core. For a few microseconds of work that is more
//! than running the level inline, so the engine keeps a smoothed cost per node
//! ([`crate::plan::Node::cost_ns`]) and decides per level, on the audio thread, before the level
//! runs. The decision reads clocks; the audio does not: a node's output is the same on any
//! thread, so a bounce stays byte-identical whichever way each level is decided.
//!
//! # The rule
//!
//! Spreading a level over `p` threads takes about `max(longest node, total / p)` instead of
//! `total`, so the time it can save is `total - max(longest node, total / p)`: one heavy node
//! among many trivial ones saves only what the trivial ones cost, and a level of equal nodes
//! saves most of `total`. A level is *dispatched* when that saving reaches [`ENTER_SAVED_NS`] and
//! returns to *inline* when it drops below [`LEAVE_SAVED_NS`]. The leave threshold is lower than
//! the enter one, and a change of mode also needs the opposite verdict for [`DWELL_BATCHES`]
//! batches in a row, so a level whose cost hovers around a threshold does not flap.
//!
//! # Start mode
//!
//! A level whose nodes have no cost yet (a new plan with a new device) starts dispatched, which
//! is what the engine did before the gate existed and is safe for the deadline: a heavy device
//! is never run inline unmeasured. The first batch measures every node, and the first verdict
//! with all costs known is applied at once, without the dwell.

/// A level whose spreading over the workers would save at least this many nanoseconds is
/// dispatched.
pub(crate) const ENTER_SAVED_NS: u64 = 7_000;
/// A dispatched level stays dispatched while the spreading saves at least this many nanoseconds.
pub(crate) const LEAVE_SAVED_NS: u64 = 3_500;
/// Batches in a row the opposite verdict must hold before a level changes mode.
pub(crate) const DWELL_BATCHES: u8 = 64;

/// What the nodes of one level cost. Built by [`LevelCost::add`], one node at a time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LevelCost {
    total: u64,
    longest: u64,
    unknown: bool,
}

impl LevelCost {
    /// Adds one node: its smoothed cost in nanoseconds, or `None` while it has none yet.
    pub(crate) fn add(&mut self, cost_ns: Option<u64>) {
        match cost_ns {
            Some(ns) => {
                self.total = self.total.saturating_add(ns);
                self.longest = self.longest.max(ns);
            }
            None => self.unknown = true,
        }
    }

    /// Nanoseconds spreading the level over `participants` threads would save.
    pub(crate) fn saved_ns(self, participants: usize) -> u64 {
        let share = self.total / participants.max(1) as u64;
        self.total - self.longest.max(share)
    }
}

/// The mode of one level. Plain integers, owned by the plan and touched by the audio thread
/// only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LevelGate {
    dispatch: bool,
    /// A verdict with every cost known has been applied.
    settled: bool,
    /// Batches in a row the opposite verdict held.
    streak: u8,
}

impl LevelGate {
    /// A level that has not been judged yet: dispatched (see the module doc).
    pub(crate) const fn fresh() -> Self {
        Self {
            dispatch: true,
            settled: false,
            streak: 0,
        }
    }

    /// A level of at most one device: never handed over.
    pub(crate) const fn inline() -> Self {
        Self {
            dispatch: false,
            settled: true,
            streak: 0,
        }
    }

    /// Whether the level is handed to the workers now.
    pub(crate) fn dispatched(self) -> bool {
        self.dispatch
    }

    /// Takes one batch's costs and returns whether the level is dispatched for this batch;
    /// `participants` is how many threads would share it (the workers and the audio thread, at
    /// most one per device).
    pub(crate) fn update(&mut self, cost: LevelCost, participants: usize) -> bool {
        if cost.unknown {
            return self.dispatch;
        }
        let saved = cost.saved_ns(participants);
        if !self.settled {
            self.settled = true;
            self.dispatch = saved >= ENTER_SAVED_NS;
            self.streak = 0;
            return self.dispatch;
        }
        let opposite = if self.dispatch {
            saved < LEAVE_SAVED_NS
        } else {
            saved >= ENTER_SAVED_NS
        };
        if opposite {
            self.streak = self.streak.saturating_add(1);
            if self.streak >= DWELL_BATCHES {
                self.dispatch = !self.dispatch;
                self.streak = 0;
            }
        } else {
            self.streak = 0;
        }
        self.dispatch
    }
}

/// How the engine decides each level (test and benchmark switch; see
/// `Engine::set_level_gate_mode`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GateMode {
    /// Decide from the measured costs.
    #[default]
    Auto,
    /// Run every level inline on the audio thread.
    ForceInline,
    /// Hand every level of two or more devices to the workers, as before the gate existed.
    ForceDispatch,
}

/// Smooths a node's cost: the first sample is taken as it is, later ones move the value half
/// way. Pure, so the smoothing is testable.
pub(crate) fn smooth(previous: Option<u64>, sample_ns: u64) -> u64 {
    match previous {
        None => sample_ns,
        Some(old) => old / 2 + sample_ns / 2 + (old & sample_ns & 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: usize = 3;

    fn level(costs: &[u64]) -> LevelCost {
        let mut c = LevelCost::default();
        costs.iter().for_each(|&ns| c.add(Some(ns)));
        c
    }

    fn settled(dispatch: bool) -> LevelGate {
        LevelGate {
            dispatch,
            settled: true,
            streak: 0,
        }
    }

    /// Feeds the same level until the gate changes, up to a bound; returns the batches it took.
    fn run(gate: &mut LevelGate, cost: LevelCost, batches: usize) -> usize {
        for k in 1..=batches {
            let before = gate.dispatched();
            if gate.update(cost, P) != before {
                return k;
            }
        }
        0
    }

    #[test]
    fn the_saving_is_what_the_longest_node_leaves_to_spread() {
        assert_eq!(level(&[30_000; 3]).saved_ns(3), 60_000);
        assert_eq!(level(&[30_000; 3]).saved_ns(1), 0);
        // One heavy node and trivial ones: only the trivial ones can be moved.
        assert_eq!(level(&[200_000, 100, 100, 100]).saved_ns(3), 300);
        // Many equal nodes: two thirds of the total with three threads.
        assert_eq!(level(&[1_000; 90]).saved_ns(3), 60_000);
    }

    #[test]
    fn a_fresh_level_starts_dispatched_until_its_nodes_have_a_cost() {
        let mut gate = LevelGate::fresh();
        assert!(gate.dispatched());
        let mut unknown = level(&[1_000, 1_000]);
        unknown.add(None);
        for _ in 0..50 {
            assert!(gate.update(unknown, P), "no verdict without every cost");
        }
    }

    #[test]
    fn the_first_verdict_with_known_costs_applies_at_once() {
        let mut cheap = LevelGate::fresh();
        assert!(!cheap.update(level(&[500; 8]), P), "cheap: inline at once");
        let mut heavy = LevelGate::fresh();
        assert!(heavy.update(level(&[20_000, 20_000]), P), "heavy: stays");
        let mut lone = LevelGate::fresh();
        assert!(
            !lone.update(level(&[400_000, 100, 100, 100]), P),
            "one heavy node among trivial ones gains nothing"
        );
    }

    #[test]
    fn a_cheap_level_stays_inline_and_a_heavy_one_dispatches() {
        let mut gate = settled(false);
        for _ in 0..100 {
            assert!(
                !gate.update(level(&[200; 40]), P),
                "8 us in all: nothing to save"
            );
            assert!(
                !gate.update(level(&[6_000, 5_000]), P),
                "two nodes, 5 us to save"
            );
        }
        assert_eq!(
            run(&mut gate, level(&[20_000, 20_000]), 200),
            DWELL_BATCHES as usize
        );
        assert!(gate.dispatched());
    }

    #[test]
    fn a_level_of_many_small_nodes_dispatches_when_the_total_is_large() {
        // 64 nodes of 3 us: none of them is heavy, together they are 192 us.
        let mut gate = settled(false);
        assert_eq!(
            run(&mut gate, level(&[3_000; 64]), 200),
            DWELL_BATCHES as usize
        );
        assert!(gate.dispatched());
    }

    #[test]
    fn one_heavy_node_among_many_trivial_ones_stays_inline() {
        let mut gate = settled(false);
        let mut costs = vec![200_000];
        costs.extend([100; 30]); // 3 us of trivial nodes
        for _ in 0..100 {
            assert!(!gate.update(level(&costs), P));
        }
        // And a dispatched level that is left with only the trivial ones returns inline.
        let mut gate = settled(true);
        assert!(run(&mut gate, level(&costs), 200) > 0);
        assert!(!gate.dispatched());
    }

    #[test]
    fn a_cost_hovering_around_a_threshold_does_not_flap() {
        // 3 nodes of 2.5 us save 5 us with three threads: between leave and enter.
        let between = level(&[2_500; 3]);
        assert_eq!(between.saved_ns(P), 5_000);
        let mut inline = settled(false);
        let mut disp = settled(true);
        for _ in 0..500 {
            assert!(!inline.update(between, P));
            assert!(disp.update(between, P));
        }
        // A cost that crosses the enter threshold only now and then never reaches the dwell.
        let over = level(&[4_000; 3]);
        let mut gate = settled(false);
        let mut flips = 0;
        for k in 0..600 {
            let cost = if k % (DWELL_BATCHES as usize) == 0 {
                over
            } else {
                between
            };
            let before = gate.dispatched();
            flips += usize::from(gate.update(cost, P) != before);
        }
        assert_eq!(flips, 0);
    }

    #[test]
    fn a_real_change_of_load_flips_within_the_dwell() {
        let heavy = level(&[30_000, 30_000, 30_000]);
        let cheap = level(&[800; 3]);
        let mut gate = settled(false);
        assert_eq!(run(&mut gate, heavy, 200), DWELL_BATCHES as usize);
        assert!(gate.dispatched());
        assert_eq!(run(&mut gate, cheap, 200), DWELL_BATCHES as usize);
        assert!(!gate.dispatched());
    }

    #[test]
    fn a_level_of_one_device_is_never_dispatched() {
        assert!(!LevelGate::inline().dispatched());
        assert!(
            LevelGate::fresh().dispatched(),
            "unlike a level not judged yet"
        );
    }

    #[test]
    fn smoothing_takes_the_first_sample_and_then_moves_half_way() {
        assert_eq!(smooth(None, 7_000), 7_000);
        assert_eq!(smooth(Some(1_000), 3_000), 2_000);
        assert_eq!(smooth(Some(3), 5), 4);
        assert_eq!(smooth(Some(u64::MAX), u64::MAX), u64::MAX);
        let mut v = smooth(None, 100_000);
        for _ in 0..40 {
            v = smooth(Some(v), 0);
        }
        assert!(v < 100, "a node that became cheap decays: {v}");
    }
}
