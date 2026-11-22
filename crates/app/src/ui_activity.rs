//! How often the UI wakes up and paints, and why: the counters behind the `ui activity` debug
//! line and the matching lines of Help > Export Diagnostics.
//!
//! [`Activity`] counts the event-loop wake-ups and the paints of every window, each with the
//! [`PaintReason`] it was caused by, in whole seconds of the time since the app started. The
//! caller passes that time to [`Activity::advance`]; nothing here reads a clock, so the tests
//! drive it with plain durations. It lives on the UI thread and is plain integers. The platform
//! publishes an [`ActivitySnapshot`] (the last full second and the totals since start) whenever a
//! second ends, and the diagnostics report reads the latest one with [`latest`].

use crate::windows::WindowKind;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// How often the debug line is written.
pub const LINE_PERIOD_SECS: u64 = 5;

/// Why a window was painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaintReason {
    /// The person did something (pointer, wheel, keys, text input).
    Input,
    /// A view that keeps itself painting: the playhead, the meters, a scroll or a fade.
    Live,
    /// Something marked the window changed on a tick: a poll of the engine or a plugin, a
    /// finished load, a live value. Also the paints at start-up.
    Update,
    /// The window changed size or scale.
    Resize,
    /// The window gained focus or was uncovered.
    Focus,
    /// The surface was reconfigured or lost, or a frame was not shown and is painted again.
    Surface,
}

impl PaintReason {
    /// Every reason, in the order the lines list them.
    pub const ALL: [PaintReason; 6] = [
        PaintReason::Input,
        PaintReason::Live,
        PaintReason::Update,
        PaintReason::Resize,
        PaintReason::Focus,
        PaintReason::Surface,
    ];

    /// The word the lines use.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            PaintReason::Input => "input",
            PaintReason::Live => "live",
            PaintReason::Update => "update",
            PaintReason::Resize => "resize",
            PaintReason::Focus => "focus",
            PaintReason::Surface => "surface",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// The word a window is called in the lines.
#[must_use]
pub fn window_label(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Main => "main",
        WindowKind::Rack => "rack",
        WindowKind::Console => "console",
        WindowKind::SpectrumEq => "spectrum-eq",
        WindowKind::Browser => "browser",
        WindowKind::PianoKeys => "piano-keys",
        WindowKind::Video => "video",
        WindowKind::Fretboard => "fretboard",
    }
}

/// Wake-ups and paints over one stretch of time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Event-loop wake-ups (`about_to_wait`).
    pub wakes: u64,
    /// Paints per window, each split by reason (indexed like [`PaintReason::ALL`]).
    pub paints: BTreeMap<WindowKind, [u64; PaintReason::ALL.len()]>,
    /// Paints per window that the frame cap held back (a paint wanted before the window's frame
    /// period was over; each is painted later, so these are not paints). A window that is asked
    /// again before its period ends counts each time.
    pub deferred: BTreeMap<WindowKind, u64>,
}

impl Counts {
    /// The paints of `kind` over this stretch.
    #[must_use]
    pub fn paints_of(&self, kind: WindowKind) -> u64 {
        self.paints.get(&kind).map_or(0, |p| p.iter().sum())
    }

    /// The text of the debug line and the report: `60 wakes/s, main 60 paints/s (live 58, input
    /// 2)`. `unit` is `/s` for one second and empty for the totals.
    #[must_use]
    pub fn summary(&self, unit: &str) -> String {
        let mut out = format!("{} wakes{unit}", self.wakes);
        if self.paints.is_empty() {
            out.push_str(", no paints");
        }
        for (kind, by_reason) in &self.paints {
            let total: u64 = by_reason.iter().sum();
            let causes: Vec<String> = PaintReason::ALL
                .iter()
                .filter(|r| by_reason[r.index()] > 0)
                .map(|r| format!("{} {}", r.label(), by_reason[r.index()]))
                .collect();
            out.push_str(&format!(
                ", {} {total} paints{unit} ({})",
                window_label(*kind),
                causes.join(", ")
            ));
        }
        for (kind, held) in &self.deferred {
            out.push_str(&format!(", {} {held} deferred{unit}", window_label(*kind)));
        }
        out
    }
}

/// What the report shows: the last full second and everything since the start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivitySnapshot {
    /// The last full second.
    pub last_second: Counts,
    /// Everything since the app started.
    pub total: Counts,
    /// Whole seconds since the app started when the snapshot was taken.
    pub seconds: u64,
}

/// The counters themselves.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// Whole seconds since the start of the second being counted.
    second: u64,
    current: Counts,
    last: Counts,
    total: Counts,
    /// The second the debug line was last written for.
    logged: u64,
}

impl Activity {
    /// Empty counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves to the second `since_start` falls in. Returns whether a second ended: the counts of
    /// the second that ended become the last second (a gap of idle seconds in between makes it
    /// empty: the loop slept).
    pub fn advance(&mut self, since_start: Duration) -> bool {
        let now = since_start.as_secs();
        if now <= self.second {
            return false;
        }
        let ended = std::mem::take(&mut self.current);
        self.last = if now == self.second + 1 {
            ended
        } else {
            Counts::default()
        };
        self.second = now;
        true
    }

    /// An event-loop wake-up.
    pub fn wake(&mut self) {
        self.current.wakes += 1;
        self.total.wakes += 1;
    }

    /// A paint of `kind` caused by `reason`.
    pub fn paint(&mut self, kind: WindowKind, reason: PaintReason) {
        let at = reason.index();
        self.current.paints.entry(kind).or_default()[at] += 1;
        self.total.paints.entry(kind).or_default()[at] += 1;
    }

    /// A paint of `kind` that the frame cap held back: not a paint.
    pub fn deferred(&mut self, kind: WindowKind) {
        *self.current.deferred.entry(kind).or_default() += 1;
        *self.total.deferred.entry(kind).or_default() += 1;
    }

    /// The last full second and the totals.
    #[must_use]
    pub fn snapshot(&self) -> ActivitySnapshot {
        ActivitySnapshot {
            last_second: self.last.clone(),
            total: self.total.clone(),
            seconds: self.second,
        }
    }

    /// The debug line, once per [`LINE_PERIOD_SECS`]; call it after [`Self::advance`] returned
    /// true.
    pub fn debug_line(&mut self) -> Option<String> {
        if self.second < self.logged + LINE_PERIOD_SECS {
            return None;
        }
        self.logged = self.second;
        Some(format!("ui activity: {}", self.last.summary("/s")))
    }
}

static LATEST: Mutex<Option<ActivitySnapshot>> = Mutex::new(None);

/// Keeps `snapshot` for the diagnostics report.
pub fn publish(snapshot: ActivitySnapshot) {
    if let Ok(mut latest) = LATEST.lock() {
        *latest = Some(snapshot);
    }
}

/// The latest published snapshot (empty before the first second ended).
#[must_use]
pub fn latest() -> ActivitySnapshot {
    LATEST
        .lock()
        .ok()
        .and_then(|l| l.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn a_second_ends_with_its_counts_as_the_last_second() {
        let mut a = Activity::new();
        assert!(!a.advance(secs(0.5)));
        for _ in 0..60 {
            a.wake();
            a.paint(WindowKind::Main, PaintReason::Live);
        }
        a.paint(WindowKind::Main, PaintReason::Input);
        assert!(!a.advance(secs(0.99)));
        assert_eq!(a.snapshot().last_second, Counts::default());
        assert!(a.advance(secs(1.0)));
        let s = a.snapshot();
        assert_eq!(s.last_second.wakes, 60);
        assert_eq!(s.last_second.paints_of(WindowKind::Main), 61);
        assert_eq!(s.total.wakes, 60);
        assert_eq!(s.seconds, 1);
    }

    #[test]
    fn the_totals_keep_growing_and_the_last_second_is_replaced() {
        let mut a = Activity::new();
        a.wake();
        a.advance(secs(1.2));
        a.wake();
        a.wake();
        a.advance(secs(2.1));
        let s = a.snapshot();
        assert_eq!(s.last_second.wakes, 2);
        assert_eq!(s.total.wakes, 3);
    }

    #[test]
    fn a_sleeping_loop_leaves_an_empty_last_second() {
        let mut a = Activity::new();
        a.wake();
        a.paint(WindowKind::Main, PaintReason::Update);
        // Nothing woke the loop for ten seconds: the second before this one was empty.
        assert!(a.advance(secs(10.4)));
        let s = a.snapshot();
        assert_eq!(s.last_second, Counts::default());
        assert_eq!(s.total.wakes, 1);
        assert_eq!(s.total.paints_of(WindowKind::Main), 1);
    }

    #[test]
    fn the_summary_names_the_windows_and_the_reasons() {
        let mut a = Activity::new();
        for _ in 0..58 {
            a.wake();
            a.paint(WindowKind::Main, PaintReason::Live);
        }
        for _ in 0..2 {
            a.wake();
            a.paint(WindowKind::Main, PaintReason::Input);
        }
        a.advance(secs(1.0));
        assert_eq!(
            a.snapshot().last_second.summary("/s"),
            "60 wakes/s, main 60 paints/s (input 2, live 58)"
        );
        assert_eq!(
            a.snapshot().total.summary(""),
            "60 wakes, main 60 paints (input 2, live 58)"
        );
        assert_eq!(Counts::default().summary("/s"), "0 wakes/s, no paints");
    }

    #[test]
    fn the_summary_lists_every_window_that_painted() {
        let mut a = Activity::new();
        a.paint(WindowKind::Console, PaintReason::Live);
        a.paint(WindowKind::Main, PaintReason::Resize);
        a.advance(secs(1.0));
        assert_eq!(
            a.snapshot().last_second.summary("/s"),
            "0 wakes/s, main 1 paints/s (resize 1), console 1 paints/s (live 1)"
        );
    }

    #[test]
    fn a_deferred_paint_is_counted_apart_and_is_not_a_paint() {
        let mut a = Activity::new();
        a.paint(WindowKind::Rack, PaintReason::Input);
        for _ in 0..40 {
            a.deferred(WindowKind::Rack);
        }
        a.deferred(WindowKind::Console);
        a.advance(secs(1.0));
        let s = a.snapshot();
        assert_eq!(s.last_second.paints_of(WindowKind::Rack), 1);
        assert_eq!(s.last_second.paints_of(WindowKind::Console), 0);
        assert_eq!(
            s.last_second.summary("/s"),
            "0 wakes/s, rack 1 paints/s (input 1), rack 40 deferred/s, console 1 deferred/s"
        );
        assert_eq!(s.total.deferred.get(&WindowKind::Rack), Some(&40));
        // A second with none says nothing about it.
        a.advance(secs(2.0));
        assert_eq!(
            a.snapshot().last_second.summary("/s"),
            "0 wakes/s, no paints"
        );
    }

    #[test]
    fn the_debug_line_comes_every_five_seconds() {
        let mut a = Activity::new();
        a.wake();
        let mut lines = Vec::new();
        for tenth in 1..=130 {
            if a.advance(secs(f64::from(tenth) / 10.0))
                && let Some(line) = a.debug_line()
            {
                lines.push((tenth, line));
            }
        }
        let at: Vec<_> = lines.iter().map(|(t, _)| *t).collect();
        assert_eq!(at, vec![50, 100]);
        assert!(
            lines[0].1.starts_with("ui activity: 0 wakes/s"),
            "{lines:?}"
        );
    }

    #[test]
    fn the_published_snapshot_is_the_one_the_report_reads() {
        let mut a = Activity::new();
        a.wake();
        a.advance(secs(1.0));
        publish(a.snapshot());
        assert_eq!(latest().last_second.wakes, 1);
    }
}
