//! What the interface thread may wait for a plugin's cosmetic answers, and the answers that come
//! after the call that asked has returned.
//!
//! A cosmetic ("soft") request only polishes the interface: the text of a parameter value, the
//! size of an editor, the default pins, a layout change. Its answer comes from the plugin's main
//! thread, which a plugin keeps busy while it loads a preset or builds a window, so the interface
//! never waits for it for long: [`SoftBudget`] is the one allowance a frame has for all instances
//! together, and an answer that is not there in time is delivered later as a [`SoftAnswer`].

use crate::backend::PresetInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The most the interface thread waits in one frame, in total over every plugin, for cosmetic
/// answers (most plugins answer in well under a millisecond, so this is rarely used up).
pub const SOFT_FRAME_BUDGET: Duration = Duration::from_millis(20);

/// The allowance of waiting one frame has for cosmetic answers, shared by every instance of a
/// manager (clones share the count). [`SoftBudget::begin_frame`] opens the next frame; each wait
/// spends what it really waited.
#[derive(Debug, Clone, Default)]
pub struct SoftBudget {
    shared: Arc<BudgetCells>,
}

#[derive(Debug, Default)]
struct BudgetCells {
    /// Microseconds waited since the frame began.
    spent_us: AtomicU64,
    /// The number of the frame.
    frame: AtomicU64,
    /// Microseconds the waits of this frame were asked to last.
    asked_us: AtomicU64,
    /// How many waits this frame granted.
    waits: AtomicU64,
}

impl SoftBudget {
    /// Starts a frame: the allowance is whole again and every instance may be waited for once
    /// more.
    pub fn begin_frame(&self) {
        self.shared.spent_us.store(0, Ordering::Relaxed);
        self.shared.asked_us.store(0, Ordering::Relaxed);
        self.shared.waits.store(0, Ordering::Relaxed);
        self.shared.frame.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of the current frame (it changes at every [`SoftBudget::begin_frame`]).
    pub fn frame(&self) -> u64 {
        self.shared.frame.load(Ordering::Relaxed)
    }

    /// How long may still be waited in this frame.
    pub fn left(&self) -> Duration {
        SOFT_FRAME_BUDGET.saturating_sub(self.spent())
    }

    /// How long has been waited in this frame.
    pub fn spent(&self) -> Duration {
        Duration::from_micros(self.shared.spent_us.load(Ordering::Relaxed))
    }

    /// Records that a wait of `asked` was granted in this frame (what the code asked for, whatever
    /// the operating system then made of it).
    pub fn grant(&self, asked: Duration) {
        let us = u64::try_from(asked.as_micros()).unwrap_or(u64::MAX);
        self.shared.asked_us.fetch_add(us, Ordering::Relaxed);
        self.shared.waits.fetch_add(1, Ordering::Relaxed);
    }

    /// The total time the waits granted in this frame were asked to last.
    pub fn asked(&self) -> Duration {
        Duration::from_micros(self.shared.asked_us.load(Ordering::Relaxed))
    }

    /// How many waits were granted in this frame.
    pub fn waits(&self) -> u64 {
        self.shared.waits.load(Ordering::Relaxed)
    }

    /// Records a wait of `waited`.
    pub fn spend(&self, waited: Duration) {
        let us = u64::try_from(waited.as_micros()).unwrap_or(u64::MAX);
        self.shared.spent_us.fetch_add(us, Ordering::Relaxed);
    }
}

/// A cosmetic answer that arrived after the call that asked for it had returned (see
/// [`crate::PluginInstance::take_soft_answers`]).
#[derive(Debug, Clone, PartialEq)]
pub enum SoftAnswer {
    /// The display text of a parameter value: the value it was asked for and the text.
    ParamText {
        /// The parameter.
        param: u32,
        /// The plain value the text was asked for.
        plain: f64,
        /// The text.
        text: String,
    },
    /// The editor was asked to resize and answered: the size asked for and the size it took
    /// (`None`: it refused, as an editor of a fixed size does). Only the last of a run of
    /// resizes that were merged into one is answered.
    GuiSized {
        /// The size the last request asked for.
        asked: (u32, u32),
        /// The size the editor took.
        took: Option<(u32, u32)>,
    },
    /// The param ids a panel should pin by default.
    DefaultPins(Vec<u32>),
    /// The presets the plugin offers, asked for with
    /// [`crate::PluginInstance::begin_list_presets`] (`None`: the plugin did not list them).
    Presets(Option<Vec<PresetInfo>>),
    /// The param values the plugin holds, asked for with
    /// [`crate::PluginInstance::begin_read_params`] (`None`: the plugin did not say).
    Params(Option<Vec<(u32, f64)>>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_has_one_allowance_that_every_wait_draws_on() {
        let budget = SoftBudget::default();
        let other = budget.clone();
        assert_eq!(budget.left(), SOFT_FRAME_BUDGET);
        budget.spend(Duration::from_millis(7));
        other.spend(Duration::from_millis(8));
        assert_eq!(budget.spent(), Duration::from_millis(15));
        assert_eq!(other.left(), Duration::from_millis(5));
        other.spend(Duration::from_millis(50));
        assert_eq!(budget.left(), Duration::ZERO, "never below nothing");
    }

    #[test]
    fn a_new_frame_makes_the_allowance_whole_and_changes_the_frame_number() {
        let budget = SoftBudget::default();
        let frame = budget.frame();
        budget.spend(SOFT_FRAME_BUDGET);
        budget.begin_frame();
        assert_eq!(budget.left(), SOFT_FRAME_BUDGET);
        assert_ne!(budget.frame(), frame);
    }
}
