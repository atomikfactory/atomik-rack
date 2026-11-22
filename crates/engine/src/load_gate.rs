//! Whether the audio thread needs the render workers to meet its deadline.
//!
//! A live host asks the engine for one batch per callback period; the question is whether the
//! devices fit in that period on the audio thread alone. While they do, waking the workers only
//! burns CPU (they spin through the callback), so the engine keeps a smoothed cost per node
//! ([`crate::plan::Node::cost_ns`]), sums it over the plan each batch and compares it with the
//! batch budget ([`crate::timing::batch_budget_ns`]). The decision reads clocks; the audio does
//! not: a node's output is the same on any thread, so a bounce stays byte-identical whichever
//! way the gate decides.
//!
//! # The rule
//!
//! The plan is *busy* (workers are used for every level they can take) when its summed cost
//! reaches [`ENTER_PERCENT`] of the batch budget, and *calm* (every level runs inline, no
//! worker is woken) after it has stayed under [`LEAVE_PERCENT`] for [`LEAVE_AFTER_NS`] of audio.
//! The hysteresis is asymmetric: entering is immediate (the batch after the smoothed load
//! crosses the share; a wrong "busy" costs a little CPU), leaving is slow (a wrong "calm" on a
//! heavy project is a missed deadline). Between the two shares the state does not change. The
//! cost an inline plan can run up before the gate reacts is bounded by the enter share plus the
//! growth of one batch; a calm plan is sampled on every batch (see [`crate::workers`]), so the
//! worst-case delay to notice is one batch plus the smoothing.
//!
//! # Inline probes
//!
//! A node timed on a worker takes longer than the same node on the audio thread (its data moves
//! between cores), so a plan that is busy and dispatching would read as busier than it is and
//! never calm down. Once busy, the costs are therefore only updated by a probe: one batch in
//! [`PROBE_EVERY_BATCHES`] runs inline and timed, and only while the last inline load was under
//! [`PROBE_BELOW_PERCENT`] of the budget (a probe cannot be a heavy batch). A calm plan is inline
//! anyway and is timed on every batch.
//!
//! # Holds
//!
//! A plan whose nodes have no cost yet (a new plan, a new device) is busy, as the engine was
//! before the gate existed, and so is the first second after the transport starts: a stopped
//! project with silent, cheap devices must not begin playback inline and find the load one
//! batch late. The hold counts audio time, not wall time. Offline renders do not use the gate.

/// Share of the batch budget, in percent, at which a plan becomes busy.
pub(crate) const ENTER_PERCENT: u64 = 30;
/// Share of the batch budget, in percent, under which a busy plan starts to calm down.
pub(crate) const LEAVE_PERCENT: u64 = 20;
/// Nanoseconds of audio a busy plan must stay under the leave share before it is calm.
pub(crate) const LEAVE_AFTER_NS: u64 = 1_000_000_000;
/// Nanoseconds of audio the busy state is held after the transport starts.
pub(crate) const HOLD_NS: u64 = 1_000_000_000;
/// While busy, one batch in this many runs inline and timed, to see what the plan costs on the
/// audio thread alone (a run on a worker costs more: the node's data moves between cores).
pub(crate) const PROBE_EVERY_BATCHES: u32 = 64;
/// A busy plan is probed inline only while its last inline cost was under this share of the
/// budget, in percent, so a probe batch cannot be a heavy one.
pub(crate) const PROBE_BELOW_PERCENT: u64 = 50;

/// What the nodes of a plan cost in one batch, summed. Built by [`PlanLoad::add`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PlanLoad {
    total: u64,
    unknown: bool,
}

impl PlanLoad {
    /// Adds one step: its node's smoothed cost in nanoseconds, or `None` while it has none yet.
    pub(crate) fn add(&mut self, cost_ns: Option<u64>) {
        match cost_ns {
            Some(ns) => self.total = self.total.saturating_add(ns),
            None => self.unknown = true,
        }
    }
}

/// The state of the gate. Plain integers, owned by the worker pool and touched by the audio
/// thread only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoadGate {
    busy: bool,
    /// Nanoseconds of audio spent under the leave share while busy.
    calm_ns: u64,
    /// Nanoseconds of audio the busy state is still held.
    hold_ns: u64,
    was_playing: bool,
    /// The last known load, in percent of the budget (0 while a cost is unknown).
    share_percent: u64,
    /// Batches since the last inline probe.
    since_probe: u32,
}

impl LoadGate {
    /// A gate that has seen nothing yet: busy (see the module doc).
    pub(crate) const fn new() -> Self {
        Self {
            busy: true,
            calm_ns: 0,
            hold_ns: 0,
            was_playing: false,
            share_percent: u64::MAX,
            since_probe: 0,
        }
    }

    /// Whether the plan is busy now.
    pub(crate) fn busy(self) -> bool {
        self.busy
    }

    /// While busy: whether this batch should run inline and timed (one in
    /// [`PROBE_EVERY_BATCHES`], never in a hold, never while a cost is unknown or the last
    /// inline load was heavy).
    pub(crate) fn probe_due(&mut self) -> bool {
        self.since_probe = self.since_probe.saturating_add(1);
        if self.since_probe >= PROBE_EVERY_BATCHES
            && self.hold_ns == 0
            && self.share_percent < PROBE_BELOW_PERCENT
        {
            self.since_probe = 0;
            return true;
        }
        false
    }

    /// Takes one batch: the plan's load, the batch budget in nanoseconds and whether the
    /// transport is playing; returns whether the plan is busy for this batch.
    pub(crate) fn update(&mut self, load: PlanLoad, budget_ns: u64, playing: bool) -> bool {
        if playing && !self.was_playing {
            self.hold_ns = HOLD_NS;
        }
        self.was_playing = playing;
        if self.hold_ns > 0 {
            self.hold_ns = self.hold_ns.saturating_sub(budget_ns.max(1));
            self.busy = true;
            self.calm_ns = 0;
            return true;
        }
        if load.unknown {
            self.busy = true;
            self.calm_ns = 0;
            self.share_percent = u64::MAX;
            return true;
        }
        let scaled = u128::from(load.total) * 100;
        let budget = u128::from(budget_ns);
        self.share_percent = u64::try_from(scaled / budget.max(1)).unwrap_or(u64::MAX);
        if scaled >= budget * u128::from(ENTER_PERCENT) {
            self.busy = true;
            self.calm_ns = 0;
        } else if self.busy {
            if scaled < budget * u128::from(LEAVE_PERCENT) {
                self.calm_ns = self.calm_ns.saturating_add(budget_ns);
                if self.calm_ns >= LEAVE_AFTER_NS {
                    self.busy = false;
                    self.calm_ns = 0;
                }
            } else {
                self.calm_ns = 0;
            }
        }
        self.busy
    }
}

/// How the engine decides when the workers are used (test and benchmark switch; see
/// `Engine::set_gate_mode`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GateMode {
    /// What the host needs: the load rule for a live engine (`EngineOptions::realtime`), every
    /// level handed over for an offline one.
    #[default]
    Auto,
    /// The load rule, whatever the host.
    Load,
    /// Run every level inline on the audio thread.
    ForceInline,
    /// Hand every level of two or more devices to the workers, as before the gate existed.
    ForceDispatch,
}

impl GateMode {
    /// The rule in force for an engine that is `realtime` or not.
    pub(crate) fn resolve(self, realtime: bool) -> Self {
        match self {
            Self::Auto if realtime => Self::Load,
            Self::Auto => Self::ForceDispatch,
            other => other,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Load => 1,
            Self::ForceInline => 2,
            Self::ForceDispatch => 3,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Load,
            2 => Self::ForceInline,
            3 => Self::ForceDispatch,
            _ => Self::Auto,
        }
    }
}

static DEFAULT_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The mode new engines start in (benchmark switch: a process-wide default read when an engine
/// is created; not a stable API).
#[doc(hidden)]
pub fn set_default_gate_mode(mode: GateMode) {
    DEFAULT_MODE.store(mode.code(), std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn default_gate_mode() -> GateMode {
    GateMode::from_code(DEFAULT_MODE.load(std::sync::atomic::Ordering::Relaxed))
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

    /// 64 frames at 48 kHz.
    const BUDGET: u64 = 1_333_333;

    fn load(share_percent: u64) -> PlanLoad {
        let mut l = PlanLoad::default();
        l.add(Some(BUDGET * share_percent / 100));
        l
    }

    fn batches_in(ns: u64) -> u64 {
        ns / BUDGET + 1
    }

    fn calm_gate() -> LoadGate {
        let mut gate = LoadGate::new();
        // Known costs, nothing playing: calm down after the leave dwell.
        for _ in 0..batches_in(LEAVE_AFTER_NS) + 1 {
            gate.update(load(1), BUDGET, false);
        }
        assert!(!gate.busy(), "calm after a second of nothing");
        gate
    }

    #[test]
    fn a_new_gate_is_busy_and_stays_busy_while_a_cost_is_unknown() {
        let mut gate = LoadGate::new();
        assert!(gate.busy());
        let mut unknown = load(1);
        unknown.add(None);
        for _ in 0..5_000 {
            assert!(gate.update(unknown, BUDGET, false));
        }
    }

    #[test]
    fn a_light_plan_calms_down_after_a_second_and_not_before() {
        let mut gate = LoadGate::new();
        let mut k = 0;
        while gate.update(load(10), BUDGET, false) {
            k += 1;
            assert!(k < 10_000);
        }
        let waited = (k + 1) * BUDGET;
        assert!(waited >= LEAVE_AFTER_NS, "{waited} ns of audio");
        assert!(waited < LEAVE_AFTER_NS + 2 * BUDGET, "{waited} ns of audio");
    }

    #[test]
    fn a_plan_that_reaches_the_enter_share_is_busy_in_the_same_batch() {
        let mut gate = calm_gate();
        assert!(!gate.update(load(29), BUDGET, false));
        assert!(
            gate.update(load(31), BUDGET, false),
            "no dwell on the way in"
        );
        assert!(gate.busy());
    }

    #[test]
    fn leaving_needs_a_whole_second_under_the_leave_share() {
        let mut gate = LoadGate::new();
        gate.update(load(60), BUDGET, false);
        // Almost a second under the leave share, then one batch above it: the count restarts.
        for _ in 0..batches_in(LEAVE_AFTER_NS) - 2 {
            assert!(gate.update(load(10), BUDGET, false));
        }
        assert!(gate.update(load(25), BUDGET, false));
        for _ in 0..batches_in(LEAVE_AFTER_NS) - 2 {
            assert!(gate.update(load(10), BUDGET, false));
        }
        let mut left = false;
        for _ in 0..4 {
            left |= !gate.update(load(10), BUDGET, false);
        }
        assert!(left, "calm once the second is complete");
    }

    #[test]
    fn a_load_between_the_shares_changes_nothing_in_either_state() {
        let mut calm = calm_gate();
        let mut busy = LoadGate::new();
        busy.update(load(60), BUDGET, false);
        for k in 0..10_000 {
            let share = 21 + k % 8; // 21..=28
            assert!(!calm.update(load(share), BUDGET, false), "batch {k}");
            assert!(busy.update(load(share), BUDGET, false), "batch {k}");
        }
    }

    #[test]
    fn a_load_hovering_over_the_enter_share_does_not_flap_down() {
        let mut gate = calm_gate();
        let mut changes = 0;
        let mut last = gate.busy();
        for k in 0..20_000 {
            let share = if k % 2 == 0 { 31 } else { 5 };
            let now = gate.update(load(share), BUDGET, false);
            changes += u32::from(now != last);
            last = now;
        }
        assert_eq!(changes, 1, "one entry, and it holds");
    }

    #[test]
    fn the_transport_start_holds_the_busy_state_for_a_second() {
        let mut gate = calm_gate();
        assert!(!gate.update(load(1), BUDGET, false));
        // Playing starts: busy at once although the load is tiny, for a second of audio.
        let mut held = 0u64;
        while gate.update(load(1), BUDGET, true) {
            held += 1;
            assert!(held < 10_000);
        }
        assert!(held * BUDGET >= HOLD_NS - BUDGET, "held {held} batches");
        // Playing on does not hold it again.
        assert!(!gate.update(load(1), BUDGET, true));
    }

    #[test]
    fn each_start_holds_again() {
        let mut gate = calm_gate();
        for _ in 0..batches_in(HOLD_NS + LEAVE_AFTER_NS) + 4 {
            gate.update(load(1), BUDGET, true);
        }
        assert!(!gate.busy());
        gate.update(load(1), BUDGET, false);
        assert!(
            gate.update(load(1), BUDGET, true),
            "a second start holds again"
        );
    }

    #[test]
    fn a_busy_plan_is_probed_inline_one_batch_in_sixty_four_and_only_while_light() {
        let mut gate = LoadGate::new();
        gate.update(load(40), BUDGET, false);
        let mut probes = 0;
        for _ in 0..(PROBE_EVERY_BATCHES * 10) {
            gate.update(load(40), BUDGET, false);
            probes += u32::from(gate.probe_due());
        }
        assert_eq!(probes, 10, "light enough to probe: every 64th batch");
        // Heavier than the probe share: never.
        let mut heavy = LoadGate::new();
        for _ in 0..(PROBE_EVERY_BATCHES * 10) {
            heavy.update(load(70), BUDGET, false);
            assert!(!heavy.probe_due());
        }
        // Never while a cost is unknown, nor in a hold.
        let mut unknown = LoadGate::new();
        let mut l = load(10);
        l.add(None);
        for _ in 0..(PROBE_EVERY_BATCHES * 3) {
            unknown.update(l, BUDGET, false);
            assert!(!unknown.probe_due());
        }
        let mut held = LoadGate::new();
        for _ in 0..(PROBE_EVERY_BATCHES * 3) {
            held.update(load(10), BUDGET, true);
            assert!(!held.probe_due(), "the hold after the transport starts");
        }
    }

    #[test]
    fn the_resolved_rule_follows_the_host() {
        assert_eq!(GateMode::Auto.resolve(true), GateMode::Load);
        assert_eq!(GateMode::Auto.resolve(false), GateMode::ForceDispatch);
        assert_eq!(GateMode::ForceInline.resolve(true), GateMode::ForceInline);
        assert_eq!(GateMode::Load.resolve(false), GateMode::Load);
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
