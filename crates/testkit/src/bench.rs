//! Support for the `benches/` harnesses: a recorder of per-run wall-clock times that prints the
//! median and the 99th percentile next to criterion's own report, and the one-shot mode the
//! scheduled performance job uses to count instructions.
//!
//! Benches use criterion as the loop and this module as the summary:
//!
//! ```ignore
//! let rec = Recorder::new();
//! group.bench_function("rack/64", |b| b.iter_custom(rec.runner(|| render_once())));
//! rec.report("rack/64");
//! ```
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Environment variable: when set (to anything), a bench runs each scenario exactly once and
/// skips criterion, so a tool that counts instructions sees a single deterministic pass.
pub const ONCE_VAR: &str = "ATOMIK_BENCH_ONCE";

/// True when the benches should run each scenario once ([`ONCE_VAR`] is set).
pub fn once_mode() -> bool {
    std::env::var_os(ONCE_VAR).is_some()
}

/// Median, 99th percentile, extremes and run count of a set of timings, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Runs measured.
    pub runs: usize,
    /// Fastest run.
    pub min: f64,
    /// Median run.
    pub median: f64,
    /// 99th percentile (nearest rank).
    pub p99: f64,
    /// Slowest run.
    pub max: f64,
}

impl Summary {
    /// Summarises `seconds` (any order); `None` when it is empty.
    pub fn of(seconds: &[f64]) -> Option<Summary> {
        if seconds.is_empty() {
            return None;
        }
        let mut sorted = seconds.to_vec();
        sorted.sort_by(f64::total_cmp);
        Some(Summary {
            runs: sorted.len(),
            min: sorted[0],
            median: percentile(&sorted, 50.0),
            p99: percentile(&sorted, 99.0),
            max: sorted[sorted.len() - 1],
        })
    }

    /// One line: `median 1.234 ms  p99 1.500 ms  min .. max ..  (n=21)`.
    pub fn line(&self) -> String {
        format!(
            "median {}  p99 {}  min {}  max {}  (n={})",
            fmt_time(self.median),
            fmt_time(self.p99),
            fmt_time(self.min),
            fmt_time(self.max),
            self.runs
        )
    }
}

/// The nearest-rank percentile `p` (0..=100) of the ascending `sorted` (not empty).
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// A time with a unit that keeps three or four significant digits.
pub fn fmt_time(seconds: f64) -> String {
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else if seconds >= 1e-3 {
        format!("{:.3} ms", seconds * 1e3)
    } else if seconds >= 1e-6 {
        format!("{:.3} us", seconds * 1e6)
    } else {
        format!("{:.1} ns", seconds * 1e9)
    }
}

/// Collects the duration of every individual run a criterion sample makes.
#[derive(Debug, Default)]
pub struct Recorder {
    runs: RefCell<Vec<f64>>,
}

impl Recorder {
    /// An empty recorder.
    pub fn new() -> Recorder {
        Recorder::default()
    }

    /// The routine for `Bencher::iter_custom`: runs `f` `iters` times, records each run and
    /// returns the total. The result of `f` is kept alive through `black_box`.
    pub fn runner<'a, R>(
        &'a self,
        mut f: impl FnMut() -> R + 'a,
    ) -> impl FnMut(u64) -> Duration + 'a {
        move |iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                std::hint::black_box(f());
                let took = start.elapsed();
                self.runs.borrow_mut().push(took.as_secs_f64());
                total += took;
            }
            total
        }
    }

    /// The summary of the recorded runs without the first (the warm-up pass), if any remain.
    pub fn summary(&self) -> Option<Summary> {
        let runs = self.runs.borrow();
        Summary::of(runs.get(1..).unwrap_or(&[]))
    }

    /// Prints `label: <summary line>` to standard output; nothing when no run was timed (a
    /// criterion name filter skipped the case).
    pub fn report(&self, label: &str) {
        if let Some(s) = self.summary() {
            println!("{label}: {}", s.line());
        }
    }
}

/// Runs `f` once and returns its wall-clock seconds: the single pass of [`once_mode`].
pub fn time_once<R>(f: impl FnOnce() -> R) -> f64 {
    let start = Instant::now();
    std::hint::black_box(f());
    start.elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert!((percentile(&v, 50.0) - 50.0).abs() < 1e-9);
        assert!((percentile(&v, 99.0) - 99.0).abs() < 1e-9);
        assert!((percentile(&v, 100.0) - 100.0).abs() < 1e-9);
        assert!((percentile(&[7.0], 99.0) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn a_summary_of_21_runs_has_the_middle_run_as_median() {
        let v: Vec<f64> = (0..21).rev().map(f64::from).collect();
        let s = Summary::of(&v).unwrap();
        assert_eq!(s.runs, 21);
        assert!((s.median - 10.0).abs() < 1e-9);
        assert!((s.p99 - 20.0).abs() < 1e-9);
        assert!((s.min - 0.0).abs() < 1e-9 && (s.max - 20.0).abs() < 1e-9);
        assert!(Summary::of(&[]).is_none());
    }

    #[test]
    fn the_recorder_counts_every_run_and_drops_the_warm_up() {
        let rec = Recorder::new();
        let mut n = 0;
        let mut run = rec.runner(|| {
            n += 1;
            n
        });
        let _ = run(1);
        let _ = run(5);
        assert_eq!(rec.summary().unwrap().runs, 5);
    }

    #[test]
    fn times_print_with_a_fitting_unit() {
        assert_eq!(fmt_time(2.0), "2.000 s");
        assert_eq!(fmt_time(0.0015), "1.500 ms");
        assert_eq!(fmt_time(2.5e-6), "2.500 us");
        assert_eq!(fmt_time(8e-9), "8.0 ns");
    }
}
