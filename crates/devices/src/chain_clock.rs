//! The clock of a chain of patterns: where in the chain a tick lies, as a pure function.
//!
//! A chain plays patterns `start`, `start + 1`, ... (wrapping at the pattern count) one after the
//! other, each for its length in its own step grid, and repeats. The steps of the chain are
//! numbered `k = 0, 1, 2, ...` across passes of the whole chain: with `N` steps per pass and `P`
//! ticks per pass, step `k` is step `j` of one pattern in pass `k / N` and starts at
//! `(k / N) * P + origin + grid.start(j)`, where `origin` is the total length of the patterns
//! before it in the chain and `grid.start` is the swing rule of `crate::note_clock::Grid` with the
//! swing counted from the first step of the pattern. Nothing here accumulates: the position of
//! a tick, and the tick of a position, depend on the chain and the tick alone, so a locate, a
//! loop wrap and a bounce all land on the same step. Only integer ticks and f64 `+ - * /` and
//! `floor` are used (HI-2).

use atomik_device_sdk::{MAX_PATTERNS, MAX_STEPS, StepSeqData};

use crate::note_clock::{EPS_FRAMES, Grid};

/// Longest chain.
pub(crate) const MAX_CHAIN: usize = MAX_PATTERNS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// The pattern slot played.
    pattern: u8,
    /// Steps played, 1..=64.
    len: i64,
    grid: Grid,
    /// Ticks from the start of the pass to the first step of this pattern.
    origin: i64,
    /// Chain step number of this pattern's first step within a pass.
    first: i64,
}

/// A chain of up to [`MAX_CHAIN`] patterns (fixed arrays: no allocation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainClock {
    n: usize,
    entries: [Entry; MAX_CHAIN],
    /// Steps in a pass.
    steps: i64,
    /// Ticks in a pass.
    period: i64,
}

/// A step of the chain: its pattern slot and step index in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainStep {
    pub pattern: usize,
    pub index: usize,
}

impl ChainClock {
    /// The chain of `chain` patterns from slot `start`, wrapping at the pattern count of
    /// `notes`. Without data: one step of one default pattern (like the step sequencer's).
    pub fn new(notes: Option<&StepSeqData>, start: usize, chain: usize) -> Self {
        let count = notes.map_or(1, |d| usize::from(d.count).clamp(1, MAX_PATTERNS));
        let start = start.min(count - 1);
        let n = chain.clamp(1, MAX_CHAIN);
        let mut c = Self {
            n,
            entries: [Entry {
                pattern: 0,
                len: 1,
                grid: Grid::DEFAULT,
                origin: 0,
                first: 0,
            }; MAX_CHAIN],
            steps: 0,
            period: 0,
        };
        for i in 0..n {
            let pattern = (start + i) % count;
            let (len, grid) =
                notes
                    .and_then(|d| d.pattern(pattern))
                    .map_or((1, Grid::DEFAULT), |p| {
                        (
                            i64::from(p.length.clamp(1, MAX_STEPS as u8)),
                            Grid {
                                s: p.resolution.step_ticks(),
                                swing: p.swing,
                            },
                        )
                    });
            c.entries[i] = Entry {
                pattern: pattern as u8,
                len,
                grid,
                origin: c.period,
                first: c.steps,
            };
            c.steps += len;
            c.period += len * i64::from(grid.s);
        }
        c
    }

    /// Steps in one pass of the chain.
    pub fn steps_per_pass(&self) -> i64 {
        self.steps
    }

    /// Ticks in one pass of the chain.
    #[cfg(test)]
    pub fn period(&self) -> i64 {
        self.period
    }

    /// The entry of chain step `k`, its step index within the pattern and its pass.
    fn entry_of(&self, k: i64) -> (&Entry, i64, i64) {
        let pass = k.div_euclid(self.steps);
        let c = k.rem_euclid(self.steps);
        let e = self.entries[..self.n]
            .iter()
            .rev()
            .find(|e| e.first <= c)
            .unwrap_or(&self.entries[0]);
        (e, c - e.first, pass)
    }

    /// The pattern slot and step index of chain step `k`.
    pub fn step(&self, k: i64) -> ChainStep {
        let (e, j, _) = self.entry_of(k);
        ChainStep {
            pattern: usize::from(e.pattern),
            index: j as usize,
        }
    }

    /// The step grid of chain step `k`.
    pub fn grid_of(&self, k: i64) -> Grid {
        self.entry_of(k).0.grid
    }

    /// The start tick of chain step `k`.
    pub fn start(&self, k: i64) -> f64 {
        let (e, j, pass) = self.entry_of(k);
        (pass * self.period + e.origin) as f64 + e.grid.start(j)
    }

    /// An estimate of the step a tick lies in (within a step or two of the answer).
    fn estimate(&self, tick: f64) -> i64 {
        let pass = (tick / self.period as f64).floor() as i64;
        let r = tick - (pass * self.period) as f64;
        let e = self.entries[..self.n]
            .iter()
            .rev()
            .find(|e| e.origin as f64 <= r)
            .unwrap_or(&self.entries[0]);
        let j = ((r - e.origin as f64) / f64::from(e.grid.s)).floor();
        pass * self.steps + e.first + (j as i64).clamp(0, e.len - 1)
    }

    /// The first step that starts at or after `tick` (0 for a tick at or before zero), a start
    /// within a millionth of a frame before `tick` counting as at it.
    pub fn first_at_or_after(&self, tick: f64, tpf: f64) -> i64 {
        let tol = tpf * EPS_FRAMES;
        if tick <= tol {
            return 0;
        }
        let mut k = self.estimate(tick);
        while k > 0 && self.start(k - 1) >= tick - tol {
            k -= 1;
        }
        while self.start(k) < tick - tol {
            k += 1;
        }
        k
    }

    /// The last step that started at or before `tick` (0 before the first), with the same
    /// tolerance: the step whose start frame is not after the frame of `tick`.
    pub fn last_at_or_before(&self, tick: f64, tpf: f64) -> i64 {
        let tol = tpf * EPS_FRAMES;
        if tick <= tol {
            return 0;
        }
        let mut k = self.estimate(tick);
        while k > 0 && self.start(k) > tick + tol {
            k -= 1;
        }
        while self.start(k + 1) <= tick + tol {
            k += 1;
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_device_sdk::{PatternData, SeqResolution};

    fn pat(res: SeqResolution, len: u8, swing: u8) -> PatternData {
        PatternData {
            length: len,
            resolution: res,
            swing,
            ..PatternData::default()
        }
    }

    fn data(p: &[PatternData]) -> StepSeqData {
        let mut d = StepSeqData {
            count: p.len() as u8,
            ..StepSeqData::default()
        };
        d.patterns[..p.len()].copy_from_slice(p);
        d
    }

    /// 4 sixteenths (3 840 each), 3 eighths (7 680 each), 2 swung quarters (15 360 each).
    fn three() -> StepSeqData {
        data(&[
            pat(SeqResolution::Sixteenth, 4, 0),
            pat(SeqResolution::Eighth, 3, 0),
            pat(SeqResolution::Quarter, 2, 50),
        ])
    }

    #[test]
    fn steps_follow_each_pattern_in_its_own_grid_and_repeat() {
        let d = three();
        let c = ChainClock::new(Some(&d), 0, 3);
        assert_eq!(
            (c.steps_per_pass(), c.period()),
            (9, 15_360 + 23_040 + 30_720)
        );
        let starts: Vec<f64> = (0..10).map(|k| c.start(k)).collect();
        // Pattern 1; pattern 2 from 15 360; pattern 3 from 38 400, its odd step swung by
        // (50 * 15 360 + 100) / 200 = 3 840; then the second pass from 69 120.
        assert_eq!(
            starts,
            [
                0.0, 3_840.0, 7_680.0, 11_520.0, 15_360.0, 23_040.0, 30_720.0, 38_400.0, 57_600.0,
                69_120.0,
            ]
        );
        assert_eq!(
            c.step(5),
            ChainStep {
                pattern: 1,
                index: 1
            }
        );
        assert_eq!(
            c.step(8),
            ChainStep {
                pattern: 2,
                index: 1
            }
        );
        assert_eq!(
            c.step(9),
            ChainStep {
                pattern: 0,
                index: 0
            }
        );
        assert_eq!(c.grid_of(7).s, 15_360);
    }

    #[test]
    fn chain_wraps_at_the_pattern_count_and_clamps_the_start() {
        let d = three();
        let c = ChainClock::new(Some(&d), 2, 3);
        let slots: Vec<usize> = (0..c.steps_per_pass()).map(|k| c.step(k).pattern).collect();
        assert_eq!(slots, [2, 2, 0, 0, 0, 0, 1, 1, 1]);
        let high = ChainClock::new(Some(&d), 99, 1);
        assert_eq!(high.step(0).pattern, 2);
        let none = ChainClock::new(None, 5, 4);
        assert_eq!((none.steps_per_pass(), none.period()), (4, 4 * 3_840));
    }

    #[test]
    fn first_and_last_step_agree_with_start_at_every_tick() {
        let d = three();
        let c = ChainClock::new(Some(&d), 0, 3);
        for k in 0..40 {
            let t = c.start(k);
            assert_eq!(c.first_at_or_after(t, 0.64), k, "first at start of {k}");
            assert_eq!(c.last_at_or_before(t, 0.64), k, "last at start of {k}");
            assert_eq!(c.first_at_or_after(t + 0.5, 0.64), k + 1);
            assert_eq!(c.last_at_or_before(t + 0.5, 0.64), k);
            assert_eq!(
                c.first_at_or_after(t - 1e-9, 0.64),
                k,
                "inside the tolerance"
            );
        }
        assert_eq!(c.first_at_or_after(-3.0, 0.64), 0);
        assert_eq!(c.last_at_or_before(-3.0, 0.64), 0);
    }
}
