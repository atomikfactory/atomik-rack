//! Containing a panic of the UI thread: the windowed event loop catches it, saves a recovery
//! copy, tells the user and keeps running.
//!
//! # Design
//!
//! * **Where it lives.** Only the windowed loop (`platform::app`) wraps its dispatch points in
//!   [`run`]: an event of a window, the per-tick work and a window's redraw ([`Dispatch`]). The
//!   headless shell, the QA server and the tests drive the shell directly and never pass
//!   through it, so a panic there still fails the test that caused it. The panic hook is
//!   untouched: the crash report is written exactly as before, then the loop goes on.
//! * **Unwind safety.** [`run`] asserts unwind safety; what a panic can leave half done is
//!   repaired by the caller (`Shell::after_ui_panic`: the gestures and menus in flight are
//!   dropped, everything repaints). A lock held across the panic is poisoned, not stuck: every
//!   lock on the UI thread's paths recovers a poisoned guard, and a borrow ends when the stack
//!   unwinds.
//! * **No loop.** [`UiGuard`] counts consecutive panics per point. The notice is raised once and
//!   not again within [`NOTICE_QUIET`]; after [`MAX_CONSECUTIVE_PANICS`] panicking frames or
//!   paints in a row the window's content stops painting (the notice and its SAVE button stay)
//!   until the app is restarted.
//! * **Test switch.** `ATOMIK_UI_PANIC=<point>[:<times>]` (`event`, `frame` or `paint`) makes
//!   the point panic that many times (default once), like `ATOMIK_GPU_LOSS_EVERY` does for the
//!   graphics device. It is not a command: no menu, key or command count changes.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

/// Consecutive panicking frames (ticks or paints) after which the window's content stops
/// painting.
pub const MAX_CONSECUTIVE_PANICS: u32 = 5;

/// A panic within this long of the last notice does not raise a new one.
pub const NOTICE_QUIET: Duration = Duration::from_secs(30);

/// How long the loop waits after a panic in the per-tick work before it ticks again (so a tick
/// that always panics cannot spin the loop).
pub const RETRY_AFTER_PANIC: Duration = Duration::from_millis(100);

/// The environment variable of the test switch.
pub const FORCE_PANIC_ENV: &str = "ATOMIK_UI_PANIC";

/// A dispatch point of the windowed loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// An event of a window (input, resize, focus, close).
    Event,
    /// The per-tick work between events.
    Frame,
    /// A window's redraw.
    Paint,
}

impl Dispatch {
    const ALL: [Dispatch; 3] = [Dispatch::Event, Dispatch::Frame, Dispatch::Paint];

    fn index(self) -> usize {
        match self {
            Dispatch::Event => 0,
            Dispatch::Frame => 1,
            Dispatch::Paint => 2,
        }
    }

    /// The word for the log and the test switch.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Dispatch::Event => "event",
            Dispatch::Frame => "frame",
            Dispatch::Paint => "paint",
        }
    }

    fn parse(word: &str) -> Option<Dispatch> {
        Self::ALL.into_iter().find(|p| p.name() == word)
    }
}

/// What to do about one caught panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Raise the notice (it is not up already, or the situation got worse).
    pub notice: bool,
    /// The window's content stops painting from now on.
    pub halt: bool,
}

/// The counters of caught panics. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct UiGuard {
    streak: [u32; 3],
    last_notice: Option<Duration>,
    halted: bool,
    caught: u64,
}

impl UiGuard {
    /// A panic was caught at `point`, `at` being the time since the start.
    pub fn caught(&mut self, point: Dispatch, at: Duration) -> Verdict {
        self.caught += 1;
        let streak = &mut self.streak[point.index()];
        *streak = streak.saturating_add(1);
        let halt = !self.halted && point != Dispatch::Event && *streak >= MAX_CONSECUTIVE_PANICS;
        self.halted |= halt;
        let quiet = self
            .last_notice
            .is_some_and(|t| at.saturating_sub(t) < NOTICE_QUIET);
        let notice = halt || !quiet;
        if notice {
            self.last_notice = Some(at);
        }
        Verdict { notice, halt }
    }

    /// `point` ran without a panic: its run of panics is over.
    pub fn completed(&mut self, point: Dispatch) {
        self.streak[point.index()] = 0;
    }

    /// Whether the content has stopped painting.
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// How many panics were caught since the start.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.caught
    }
}

/// Runs `f`, catching a panic: the text of the panic when there was one.
pub fn run<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no text".to_owned())
    })
}

/// Why an unattended run (a script or smoke test: `--quit-after`, `--screenshot`, a stress load)
/// must end with a failure code: it contained `caught` panics and went on, which a script that
/// only reads the exit code would take for a success. `None` for an attended session, which exits
/// normally when the user quits, and for a run that caught nothing.
#[must_use]
pub fn exit_failure(unattended: bool, caught: u64) -> Option<String> {
    (unattended && caught > 0).then(|| {
        format!(
            "{caught} internal error(s) of the interface were contained during this unattended run"
        )
    })
}

/// The test switch: makes a point panic a number of times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedPanic {
    point: Dispatch,
    left: u32,
}

impl ForcedPanic {
    /// Reads the switch (`event`, `frame:3`, `paint:10`); `None` when it is off or unreadable.
    #[must_use]
    pub fn parse(value: Option<&str>) -> Option<Self> {
        let value = value?.trim();
        let (word, times) = match value.split_once(':') {
            Some((word, times)) => (word, times.trim().parse::<u32>().ok()?),
            None => (value, 1),
        };
        let point = Dispatch::parse(word.trim())?;
        (times > 0).then_some(Self { point, left: times })
    }

    /// Whether `point` is to panic now (one of the forced panics is used up).
    pub fn take(&mut self, point: Dispatch) -> bool {
        if self.left == 0 || self.point != point {
            return false;
        }
        self.left -= 1;
        true
    }
}

#[cfg(test)]
#[path = "ui_guard_tests.rs"]
mod tests;
