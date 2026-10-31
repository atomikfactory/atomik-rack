//! Histograms of the render callback: how long each callback took and how long the engine's
//! level barrier waited, in fixed log-scale buckets.
//!
//! The audio thread records with two relaxed atomic increments and a relaxed maximum (no
//! allocation, no lock, no clock); the control thread reads a [`CallbackStats`] snapshot and
//! asks it for percentiles. The counts are totals since the stream started (they do not reset
//! on read, unlike the DSP load average), so the p99.9 of a long run is that of the whole run,
//! not of the last second.
//!
//! Time buckets cover 0 ns to about 8 s: one bucket per nanosecond below 4 ns, then four buckets
//! per power of two (a bucket spans at most a quarter of its lower bound, so a percentile is
//! exact to 25 %). Spin buckets are powers of two: 0, 1, 2-3, 4-7, and so on.

use std::sync::atomic::{AtomicU64, Ordering};

/// Buckets of the callback-time histogram.
pub const TIME_BUCKETS: usize = 128;
/// Buckets of the barrier-spin histogram.
pub const SPIN_BUCKETS: usize = 32;

/// The bucket of a callback that took `ns` nanoseconds.
pub fn time_bucket(ns: u64) -> usize {
    if ns < 4 {
        return ns as usize;
    }
    let octave = 63 - ns.leading_zeros();
    let sub = (ns >> (octave - 2)) & 3;
    (4 + (octave as usize - 2) * 4 + sub as usize).min(TIME_BUCKETS - 1)
}

/// The largest duration, in nanoseconds, that falls into time bucket `index` (the last bucket
/// holds everything above, so its bound is `u64::MAX`).
pub fn time_bucket_max_ns(index: usize) -> u64 {
    if index < 4 {
        return index as u64;
    }
    if index >= TIME_BUCKETS - 1 {
        return u64::MAX;
    }
    let j = index - 4;
    let octave = j / 4 + 2;
    let sub = (j % 4) as u64;
    ((5 + sub) << (octave - 2)) - 1
}

/// The bucket of a callback whose level barrier waited `spins` spins.
pub fn spin_bucket(spins: u64) -> usize {
    ((u64::BITS - spins.leading_zeros()) as usize).min(SPIN_BUCKETS - 1)
}

/// The largest spin count that falls into spin bucket `index` (`u64::MAX` for the last).
pub fn spin_bucket_max(index: usize) -> u64 {
    match index {
        0 => 0,
        i if i >= SPIN_BUCKETS - 1 => u64::MAX,
        i => (1u64 << i) - 1,
    }
}

/// The audio thread's side: the live counters.
#[derive(Debug)]
pub(crate) struct CallbackHistogram {
    time: [AtomicU64; TIME_BUCKETS],
    spins: [AtomicU64; SPIN_BUCKETS],
    max_ns: AtomicU64,
    max_spins: AtomicU64,
}

impl Default for CallbackHistogram {
    fn default() -> Self {
        Self {
            time: std::array::from_fn(|_| AtomicU64::new(0)),
            spins: std::array::from_fn(|_| AtomicU64::new(0)),
            max_ns: AtomicU64::new(0),
            max_spins: AtomicU64::new(0),
        }
    }
}

impl CallbackHistogram {
    /// Records one callback (audio thread; wait-free, never allocates).
    pub(crate) fn record(&self, render_ns: u64, barrier_spins: u64) {
        self.time[time_bucket(render_ns)].fetch_add(1, Ordering::Relaxed);
        self.spins[spin_bucket(barrier_spins)].fetch_add(1, Ordering::Relaxed);
        self.max_ns.fetch_max(render_ns, Ordering::Relaxed);
        self.max_spins.fetch_max(barrier_spins, Ordering::Relaxed);
    }

    /// The counts so far.
    pub(crate) fn snapshot(&self) -> CallbackStats {
        let mut out = CallbackStats::default();
        for (dst, src) in out.time.iter_mut().zip(&self.time) {
            *dst = src.load(Ordering::Relaxed);
        }
        for (dst, src) in out.spins.iter_mut().zip(&self.spins) {
            *dst = src.load(Ordering::Relaxed);
        }
        out.max_ns = self.max_ns.load(Ordering::Relaxed);
        out.max_spins = self.max_spins.load(Ordering::Relaxed);
        out
    }
}

/// A snapshot of the callback histograms (totals since the stream started).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackStats {
    /// Callbacks per time bucket (see [`time_bucket`]).
    pub time: [u64; TIME_BUCKETS],
    /// Callbacks per barrier-spin bucket (see [`spin_bucket`]).
    pub spins: [u64; SPIN_BUCKETS],
    /// The slowest callback, in nanoseconds.
    pub max_ns: u64,
    /// The longest barrier wait of any callback, in spins.
    pub max_spins: u64,
}

impl Default for CallbackStats {
    fn default() -> Self {
        Self {
            time: [0; TIME_BUCKETS],
            spins: [0; SPIN_BUCKETS],
            max_ns: 0,
            max_spins: 0,
        }
    }
}

/// The upper bound (by `max_of`) of the bucket holding the nearest-rank percentile `p`
/// (0..=100) of `counts`; 0 for no samples.
fn percentile(counts: &[u64], p: f64, max_of: impl Fn(usize) -> u64) -> u64 {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return 0;
    }
    // The factor keeps a product like 0.999 * 1000 = 999.0000000000001 from rounding up a rank.
    let rank = ((p / 100.0 * total as f64 * (1.0 - 1e-12)).ceil() as u64).clamp(1, total);
    let mut seen = 0;
    for (i, c) in counts.iter().enumerate() {
        seen += c;
        if seen >= rank {
            return max_of(i);
        }
    }
    max_of(counts.len() - 1)
}

impl CallbackStats {
    /// Callbacks recorded.
    pub fn callbacks(&self) -> u64 {
        self.time.iter().sum()
    }

    /// The callback time, in nanoseconds, that `p` percent (0..=100) of the callbacks stayed
    /// within: the upper bound of its bucket (at most 25 % above the true value), never above
    /// the slowest callback. 0 when nothing was recorded.
    pub fn time_percentile_ns(&self, p: f64) -> u64 {
        percentile(&self.time, p, time_bucket_max_ns).min(self.max_ns)
    }

    /// The barrier wait, in spins, that `p` percent of the callbacks stayed within (the upper
    /// bound of its power-of-two bucket, never above the longest wait).
    pub fn spin_percentile(&self, p: f64) -> u64 {
        percentile(&self.spins, p, spin_bucket_max).min(self.max_spins)
    }

    /// The callback time percentiles as the `--stats` summary prints them, in microseconds with
    /// one decimal, then the p99 barrier wait: `cb_p50_us=118.0 cb_p99_us=240.0
    /// cb_p999_us=410.0 cb_max_us=655.0 spins_p99=3`.
    pub fn summary_fields(&self) -> String {
        let us = |ns: u64| ns as f64 / 1_000.0;
        format!(
            "cb_p50_us={:.1} cb_p99_us={:.1} cb_p999_us={:.1} cb_max_us={:.1} spins_p99={}",
            us(self.time_percentile_ns(50.0)),
            us(self.time_percentile_ns(99.0)),
            us(self.time_percentile_ns(99.9)),
            us(self.max_ns),
            self.spin_percentile(99.0)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_duration_lands_in_a_bucket_that_contains_it() {
        let mut probes: Vec<u64> = (0..2_000).collect();
        probes.extend((0..60).flat_map(|b| [(1u64 << b) - 1, 1u64 << b, (1u64 << b) + 1]));
        probes.push(u64::MAX);
        for ns in probes {
            let i = time_bucket(ns);
            assert!(i < TIME_BUCKETS);
            assert!(ns <= time_bucket_max_ns(i), "{ns} ns above bucket {i}");
            if i > 0 {
                assert!(ns > time_bucket_max_ns(i - 1), "{ns} ns below bucket {i}");
            }
        }
    }

    #[test]
    fn a_bucket_is_at_most_a_quarter_wide_below_a_second() {
        for i in 8..100 {
            let (lo, hi) = (time_bucket_max_ns(i - 1) + 1, time_bucket_max_ns(i));
            assert!((hi - lo + 1) * 4 <= lo + 3, "bucket {i}: {lo}..={hi}");
        }
    }

    #[test]
    fn buckets_grow_with_the_duration() {
        let mut last = 0;
        for ns in (0..1_000_000u64).step_by(37) {
            let i = time_bucket(ns);
            assert!(i >= last);
            last = i;
        }
        assert_eq!(time_bucket(u64::MAX), TIME_BUCKETS - 1);
    }

    #[test]
    fn spin_buckets_are_powers_of_two() {
        assert_eq!(
            [0, 1, 2, 3, 4, 7, 8].map(spin_bucket),
            [0, 1, 2, 2, 3, 3, 4]
        );
        for spins in [0u64, 1, 2, 3, 5, 100, 70_000, u64::MAX] {
            let i = spin_bucket(spins);
            assert!(spins <= spin_bucket_max(i));
            if i > 0 {
                assert!(spins > spin_bucket_max(i - 1));
            }
        }
    }

    #[test]
    fn percentiles_follow_the_recorded_distribution() {
        let h = CallbackHistogram::default();
        // 990 callbacks of 100 us, 9 of 1 ms and one of 4 ms; the barrier waits 2 spins, once 40.
        for _ in 0..990 {
            h.record(100_000, 2);
        }
        for _ in 0..9 {
            h.record(1_000_000, 2);
        }
        h.record(4_000_000, 40);
        let s = h.snapshot();
        assert_eq!(s.callbacks(), 1_000);
        let near = |got: u64, want: u64| got >= want && got <= want + want / 4;
        assert!(near(s.time_percentile_ns(50.0), 100_000));
        assert!(near(s.time_percentile_ns(99.0), 100_000));
        assert!(near(s.time_percentile_ns(99.9), 1_000_000));
        assert_eq!(
            s.time_percentile_ns(100.0),
            4_000_000,
            "the maximum is exact"
        );
        assert_eq!(s.max_ns, 4_000_000);
        assert_eq!(s.spin_percentile(50.0), 3, "2 spins is the 2-3 bucket");
        assert_eq!(s.spin_percentile(100.0), 40);
        let line = s.summary_fields();
        assert!(line.starts_with("cb_p50_us=1"), "{line}");
        assert!(line.contains("cb_max_us=4000.0 spins_p99=3"), "{line}");
    }

    #[test]
    fn an_empty_histogram_reads_zero() {
        let s = CallbackStats::default();
        assert_eq!(s.callbacks(), 0);
        assert_eq!(s.time_percentile_ns(99.0), 0);
        assert_eq!(s.spin_percentile(99.0), 0);
    }

    #[test]
    fn recording_is_allocation_free() {
        let h = CallbackHistogram::default();
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            for i in 0..1_000u64 {
                h.record(i * 977, i % 9);
            }
        });
        assert_eq!(h.snapshot().callbacks(), 1_000);
    }
}
