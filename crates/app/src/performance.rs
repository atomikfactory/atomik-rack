//! The Performance setting: **Auto**, **Full** or **Low power**.
//!
//! Low power does three things and only these: the windows are drawn at 30 Hz instead of 60
//! ([`Level::frame_period`], the shared frame slot of `platform::window_logic::paint_gate`),
//! each window's graphics target is rendered at three quarters of its physical size and
//! stretched to the window ([`Level::render_scale`], [`target_size`]), and an automatic render
//! worker count is the conservative one ([`workers_for`]). Nothing about the audio changes: the
//! workers never change a rendered sample, so a bounce is the same at either level.
//!
//! **Auto** picks Low power on a machine with [`LOW_POWER_CPUS`] or fewer logical CPUs and Full
//! otherwise ([`auto_level`]). That is decided once when the program starts
//! ([`auto_level_here`]) and never changes while it runs. The choice is saved in
//! `workflow.json` with the other interface preferences; a file without it loads as Auto.

use crate::keymap_store;
use std::path::Path;
use std::time::Duration;

/// A machine with this many logical CPUs or fewer runs Low power under Auto.
pub const LOW_POWER_CPUS: usize = 4;

/// The most render workers an automatic count starts at Low power: one helper keeps a heavy
/// rack from missing its deadline on a small machine, and no more than that costs power.
pub const LOW_POWER_WORKERS: u16 = 1;

/// The fraction of its physical size a window's target is rendered at in Low power.
pub const LOW_POWER_SCALE: f64 = 0.75;

/// The Performance setting as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Performance {
    /// Low power on a machine with [`LOW_POWER_CPUS`] or fewer logical CPUs, else Full.
    #[default]
    Auto,
    /// The full frame rate and the full render size.
    Full,
    /// 30 Hz, a smaller render size and fewer automatic workers.
    LowPower,
}

impl Performance {
    /// Every choice, in the order the settings row steps through them.
    pub const ALL: [Performance; 3] = [Performance::Auto, Performance::Full, Performance::LowPower];

    /// The id used in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Performance::Auto => "auto",
            Performance::Full => "full",
            Performance::LowPower => "low_power",
        }
    }

    /// The choice with this id.
    pub fn from_id(id: &str) -> Option<Performance> {
        Performance::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The text on the settings button, `auto` being what Auto picked on this machine.
    pub fn label(self, auto: Level) -> &'static str {
        match (self, auto) {
            (Performance::Auto, Level::Full) => "Auto (Full)",
            (Performance::Auto, Level::Low) => "Auto (Low power)",
            (Performance::Full, _) => "Full",
            (Performance::LowPower, _) => "Low power",
        }
    }

    /// The next choice (wrapping), `back` going the other way.
    pub fn step(self, back: bool) -> Performance {
        let all = Performance::ALL;
        let at = all.iter().position(|p| *p == self).unwrap_or(0);
        let n = all.len();
        all[if back { (at + n - 1) % n } else { (at + 1) % n }]
    }

    /// The level this choice runs at, `auto` being what Auto picked on this machine.
    pub fn level(self, auto: Level) -> Level {
        match self {
            Performance::Auto => auto,
            Performance::Full => Level::Full,
            Performance::LowPower => Level::Low,
        }
    }
}

/// What the program runs at: the setting with Auto resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// 60 Hz, full render size, the normal automatic worker count.
    #[default]
    Full,
    /// 30 Hz, [`LOW_POWER_SCALE`] of the render size, at most [`LOW_POWER_WORKERS`] automatic
    /// workers.
    Low,
}

impl Level {
    /// The length of the frame slot windows are painted in (the period of the frame cap).
    pub const fn frame_period(self) -> Duration {
        match self {
            Level::Full => Duration::from_micros(16_667),
            Level::Low => Duration::from_micros(33_333),
        }
    }

    /// The fraction of a window's physical size its graphics target is rendered at.
    pub const fn render_scale(self) -> f64 {
        match self {
            Level::Full => 1.0,
            Level::Low => LOW_POWER_SCALE,
        }
    }

    /// Whether this is Low power.
    pub const fn is_low(self) -> bool {
        matches!(self, Level::Low)
    }
}

/// What Auto picks for a machine with `logical` logical CPUs.
pub fn auto_level(logical: usize) -> Level {
    if logical <= LOW_POWER_CPUS {
        Level::Low
    } else {
        Level::Full
    }
}

/// What Auto picks on this machine (an unknown CPU count counts as one CPU).
pub fn auto_level_here() -> Level {
    auto_level(std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get))
}

/// The level the saved preferences next to `settings_path` run at on this machine: what the
/// audio session is told before the first engine starts.
pub fn startup_level(settings_path: Option<&Path>) -> Level {
    let performance = settings_path
        .map(keymap_store::workflow_path)
        .map(|path| keymap_store::load_workflow(&path).value.performance)
        .unwrap_or_default();
    performance.level(auto_level_here())
}

/// The size of the graphics target of a window of `width` by `height` physical pixels drawn at
/// `scale` (never smaller than one pixel on a side, never larger than the window).
pub fn target_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    let scale = if scale.is_finite() {
        scale.clamp(0.1, 1.0)
    } else {
        1.0
    };
    let side = |n: u32| ((f64::from(n) * scale).round() as u32).clamp(1, n.max(1));
    (side(width), side(height))
}

/// The render worker count for `level` when the count is automatic: `auto` (what the machine
/// gets) at Full, at most [`LOW_POWER_WORKERS`] at Low power. A count the user fixed is never
/// passed through here.
pub fn workers_for(level: Level, auto: u16) -> u16 {
    match level {
        Level::Full => auto,
        Level::Low => auto.min(LOW_POWER_WORKERS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_picks_low_power_up_to_four_logical_cpus() {
        for logical in [0, 1, 2, 4] {
            assert_eq!(auto_level(logical), Level::Low, "{logical}");
        }
        for logical in [5, 6, 16, 128] {
            assert_eq!(auto_level(logical), Level::Full, "{logical}");
        }
    }

    #[test]
    fn the_setting_resolves_against_what_auto_picked() {
        for auto in [Level::Full, Level::Low] {
            assert_eq!(Performance::Auto.level(auto), auto);
            assert_eq!(Performance::Full.level(auto), Level::Full);
            assert_eq!(Performance::LowPower.level(auto), Level::Low);
        }
    }

    #[test]
    fn the_ids_round_trip_and_the_row_steps_both_ways() {
        for p in Performance::ALL {
            assert_eq!(Performance::from_id(p.id()), Some(p));
        }
        assert_eq!(Performance::from_id("fast"), None);
        assert_eq!(Performance::default(), Performance::Auto);
        assert_eq!(Performance::Auto.step(false), Performance::Full);
        assert_eq!(Performance::LowPower.step(false), Performance::Auto);
        assert_eq!(Performance::Auto.step(true), Performance::LowPower);
    }

    #[test]
    fn the_button_says_what_auto_picked() {
        assert_eq!(Performance::Auto.label(Level::Low), "Auto (Low power)");
        assert_eq!(Performance::Auto.label(Level::Full), "Auto (Full)");
        assert_eq!(Performance::Full.label(Level::Low), "Full");
        assert_eq!(Performance::LowPower.label(Level::Full), "Low power");
    }

    #[test]
    fn low_power_is_30_hz_and_full_is_60() {
        let hz = |l: Level| 1.0 / l.frame_period().as_secs_f64();
        assert!((hz(Level::Full) - 60.0).abs() < 0.01);
        assert!((hz(Level::Low) - 30.0).abs() < 0.01);
    }

    #[test]
    fn the_target_is_a_fraction_of_the_window_and_never_empty() {
        assert_eq!(target_size(1920, 1080, 1.0), (1920, 1080));
        assert_eq!(target_size(1920, 1080, 0.75), (1440, 810));
        assert_eq!(
            target_size(3840, 2160, Level::Low.render_scale()),
            (2880, 1620)
        );
        // Odd sizes round to the nearest pixel.
        assert_eq!(target_size(1001, 601, 0.75), (751, 451));
        assert_eq!(target_size(1, 1, 0.75), (1, 1));
        assert_eq!(target_size(2, 2, 0.1), (1, 1));
        // A scale that makes no sense is the full size, and the target never grows.
        assert_eq!(target_size(800, 600, f64::NAN), (800, 600));
        assert_eq!(target_size(800, 600, 2.0), (800, 600));
    }

    #[test]
    fn only_an_automatic_worker_count_is_made_conservative() {
        assert_eq!(workers_for(Level::Full, 7), 7);
        assert_eq!(workers_for(Level::Low, 7), LOW_POWER_WORKERS);
        assert_eq!(workers_for(Level::Low, 1), 1);
        // A machine with no spare core stays at none.
        assert_eq!(workers_for(Level::Low, 0), 0);
    }
}
