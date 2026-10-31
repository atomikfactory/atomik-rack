//! Where the time goes when a project opens: a list of named stage durations (parse, build,
//! devices, compile, submit, sync) and the log line that prints them.
//!
//! [`Stages`] is plain data (so tests feed it durations); [`StageClock`] is the one wall-clock
//! read, a lap timer that appends to a [`Stages`]. Nothing here reaches the audio thread or any
//! rendered sample.

use std::time::{Duration, Instant};

/// Named stage durations, in the order they ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stages {
    items: Vec<(&'static str, Duration)>,
}

impl Stages {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a stage.
    pub fn push(&mut self, name: &'static str, took: Duration) {
        self.items.push((name, took));
    }

    /// Appends every stage of `other`.
    pub fn extend(&mut self, other: &Stages) {
        self.items.extend_from_slice(&other.items);
    }

    /// The stages in order.
    pub fn items(&self) -> &[(&'static str, Duration)] {
        &self.items
    }

    /// The duration of the first stage called `name`.
    pub fn get(&self, name: &str) -> Option<Duration> {
        self.items.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
    }

    /// The sum of all stages.
    pub fn total(&self) -> Duration {
        self.items.iter().map(|(_, d)| *d).sum()
    }

    /// One log line: `opened the project in 123.4 ms (parse 1.2 ms, build 3.4 ms, ...)`.
    pub fn line(&self, what: &str) -> String {
        let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1e3);
        let parts: Vec<String> = self
            .items
            .iter()
            .map(|(name, d)| format!("{name} {}", ms(*d)))
            .collect();
        format!("{what} in {} ({})", ms(self.total()), parts.join(", "))
    }
}

/// A lap timer: each [`StageClock::lap`] appends the time since the previous lap (or the start).
#[derive(Debug, Clone, Copy)]
pub struct StageClock {
    last: Instant,
}

impl StageClock {
    /// Starts timing now.
    pub fn start() -> Self {
        Self { last: now() }
    }

    /// Appends the time since the previous lap to `stages` as `name` and starts the next one.
    pub fn lap(&mut self, stages: &mut Stages, name: &'static str) {
        let at = now();
        stages.push(name, at.saturating_duration_since(self.last));
        self.last = at;
    }
}

/// The only wall-clock read of this module.
#[allow(clippy::disallowed_methods)] // Justification: diagnostics timing of the project open
// only; the durations go to the log and never influence the document, the engine or any sample.
fn now() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_lists_the_stages_in_order_with_their_sum() {
        let mut s = Stages::new();
        s.push("parse", Duration::from_micros(1_500));
        s.push("build", Duration::from_millis(3));
        let line = s.line("opened the project");
        assert_eq!(
            line,
            "opened the project in 4.5 ms (parse 1.5 ms, build 3.0 ms)"
        );
        assert_eq!(s.total(), Duration::from_micros(4_500));
        assert_eq!(s.get("build"), Some(Duration::from_millis(3)));
        assert_eq!(s.get("sync"), None);
    }

    #[test]
    fn a_lap_timer_appends_one_stage_per_lap() {
        let mut stages = Stages::new();
        let mut clock = StageClock::start();
        clock.lap(&mut stages, "a");
        clock.lap(&mut stages, "b");
        let names: Vec<&str> = stages.items().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["a", "b"]);
        let mut all = Stages::new();
        all.extend(&stages);
        all.extend(&stages);
        assert_eq!(all.items().len(), 4);
    }
}
