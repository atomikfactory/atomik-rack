//! The loudness history of the loudness meter: the short-term loudness once a second for a graph,
//! and the loudness range (LRA) of everything since the last reset, as EBU Tech 3342 defines it.
//!
//! It runs on the display side from the short-term values the meter device publishes, never on
//! the audio thread. Time is audio time: [`LoudnessHistory::push`] takes the frames that passed
//! since the previous call, so a paused engine adds nothing and the graph does not depend on how
//! often the display redraws.
//!
//! * **Graph.** The last [`GRAPH_POINTS`] seconds of short-term loudness, one value a second
//!   (-120 for silence), oldest first.
//! * **LRA.** Every short-term value above the absolute gate of -70 LUFS goes into a histogram
//!   of 0.1 LU bins (fixed storage, any programme length). The relative gate is the loudness of
//!   the mean energy of those values minus 20 LU; the range is the distance between the 10th and
//!   the 95th percentile of the values above it. A single value, or a steady tone, reads 0 LU.
//!   The percentiles are those of the bin centres, so they are good to a tenth of an LU.

use std::collections::VecDeque;

/// Seconds (values) the graph holds.
pub const GRAPH_POINTS: usize = 600;
/// The reading for silence, LUFS.
pub const FLOOR_LUFS: f32 = -120.0;
/// The absolute gate, LUFS.
pub const ABS_GATE_LUFS: f32 = -70.0;
/// The relative gate below the mean energy, LU.
pub const REL_GATE_LU: f32 = 20.0;
/// Histogram bin width, LU.
const BIN: f32 = 0.1;
/// Bins from -70 to +10 LUFS (louder values land in the last).
const BINS: usize = 801;

/// The short-term history and the loudness range (module docs).
#[derive(Debug, Clone)]
pub struct LoudnessHistory {
    points: VecDeque<f32>,
    /// Frames not yet worth a second.
    pending: u64,
    counts: Vec<u32>,
}

impl Default for LoudnessHistory {
    fn default() -> Self {
        Self::new()
    }
}

fn bin_of(lufs: f32) -> usize {
    (((lufs - ABS_GATE_LUFS) / BIN).floor().max(0.0) as usize).min(BINS - 1)
}

fn centre_of(bin: usize) -> f32 {
    ABS_GATE_LUFS + (bin as f32 + 0.5) * BIN
}

impl LoudnessHistory {
    /// An empty history.
    pub fn new() -> Self {
        Self {
            points: VecDeque::new(),
            pending: 0,
            counts: vec![0; BINS],
        }
    }

    /// Forgets the graph and the range.
    pub fn reset(&mut self) {
        self.points.clear();
        self.pending = 0;
        self.counts.fill(0);
    }

    /// Values in the graph.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// The graph has no value yet.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Notes that `elapsed_frames` of audio passed at `sample_rate` and records `short_term`
    /// (LUFS) once for every full second of them (at most [`GRAPH_POINTS`] per call). Non-finite
    /// values read as silence.
    pub fn push(&mut self, short_term: f32, elapsed_frames: u64, sample_rate: f32) {
        self.pending = self.pending.saturating_add(elapsed_frames);
        let second = (f64::from(sample_rate.max(1.0)) as u64).max(1);
        let due = (self.pending / second).min(GRAPH_POINTS as u64);
        self.pending = if self.pending / second > GRAPH_POINTS as u64 {
            0
        } else {
            self.pending % second
        };
        let v = if short_term.is_finite() {
            short_term.max(FLOOR_LUFS)
        } else {
            FLOOR_LUFS
        };
        for _ in 0..due {
            self.points.push_back(v);
            if self.points.len() > GRAPH_POINTS {
                self.points.pop_front();
            }
            if v > ABS_GATE_LUFS {
                self.counts[bin_of(v)] += 1;
            }
        }
    }

    /// The graph, oldest value first.
    pub fn graph(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.points.iter().copied());
    }

    /// The loudness range in LU of everything since the reset, `None` before a value above the
    /// absolute gate.
    pub fn lra(&self) -> Option<f32> {
        let total: u64 = self.counts.iter().map(|c| u64::from(*c)).sum();
        if total == 0 {
            return None;
        }
        let energy: f64 = self
            .counts
            .iter()
            .enumerate()
            .map(|(i, c)| f64::from(*c) * 10f64.powf(f64::from(centre_of(i)) / 10.0))
            .sum();
        let gate = 10.0 * (energy / total as f64).log10() as f32 - REL_GATE_LU;
        let first = bin_of(gate.max(ABS_GATE_LUFS));
        // Bins whose centre is below the gate do not count.
        let first = if centre_of(first) < gate {
            first + 1
        } else {
            first
        };
        let kept: u64 = self.counts[first.min(BINS)..]
            .iter()
            .map(|c| u64::from(*c))
            .sum();
        if kept == 0 {
            return None;
        }
        // The value at sorted position `at` (0-based) among the kept ones.
        let at_rank = |rank: u64| -> f32 {
            let mut seen = 0u64;
            for (i, c) in self.counts.iter().enumerate().skip(first) {
                seen += u64::from(*c);
                if seen > rank {
                    return centre_of(i);
                }
            }
            centre_of(BINS - 1)
        };
        let low = at_rank(((kept - 1) as f64 * 0.10) as u64);
        let high = at_rank(((kept - 1) as f64 * 0.95) as u64);
        Some((high - low).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn feed(h: &mut LoudnessHistory, values: &[f32]) {
        for v in values {
            h.push(*v, SR as u64, SR);
        }
    }

    #[test]
    fn one_value_is_recorded_per_second_of_audio() {
        let mut h = LoudnessHistory::new();
        h.push(-20.0, 47_999, SR);
        assert!(h.is_empty());
        h.push(-20.0, 1, SR);
        assert_eq!(h.len(), 1);
        h.push(-18.0, 3 * 48_000, SR);
        assert_eq!(h.len(), 4);
        let mut g = Vec::new();
        h.graph(&mut g);
        assert_eq!(g, vec![-20.0, -18.0, -18.0, -18.0]);
    }

    #[test]
    fn the_graph_keeps_the_last_ten_minutes_and_a_gap_does_not_loop_forever() {
        let mut h = LoudnessHistory::new();
        h.push(-23.0, u64::MAX, SR);
        assert_eq!(h.len(), GRAPH_POINTS);
        feed(&mut h, &[-30.0]);
        let mut g = Vec::new();
        h.graph(&mut g);
        assert_eq!(g.len(), GRAPH_POINTS);
        assert_eq!(*g.last().unwrap(), -30.0);
    }

    #[test]
    fn a_steady_level_has_no_range_and_silence_has_none_at_all() {
        let mut h = LoudnessHistory::new();
        assert_eq!(h.lra(), None);
        feed(&mut h, &[-120.0; 5]);
        assert_eq!(h.lra(), None);
        feed(&mut h, &[-23.0; 30]);
        assert!(h.lra().unwrap() < 0.11);
    }

    #[test]
    fn the_range_is_the_spread_between_the_tenth_and_ninety_fifth_percentile() {
        let mut h = LoudnessHistory::new();
        // 100 seconds, evenly spread over -30 to -20 LUFS: the 10th to 95th percentile of a
        // uniform spread over 10 LU is 8.5 LU.
        let values: Vec<f32> = (0..100).map(|i| -30.0 + i as f32 * 0.1).collect();
        feed(&mut h, &values);
        let lra = h.lra().unwrap();
        assert!((lra - 8.5).abs() < 0.3, "{lra}");
    }

    #[test]
    fn values_far_below_the_programme_are_gated_out() {
        let mut h = LoudnessHistory::new();
        // A quiet intro 30 LU under the programme is more than 20 LU below its mean energy.
        feed(&mut h, &[-60.0; 10]);
        feed(&mut h, &[-25.0; 60]);
        let lra = h.lra().unwrap();
        assert!(lra < 0.5, "{lra}");
        // Without the gate the intro would be the lowest tenth.
        let mut h = LoudnessHistory::new();
        feed(&mut h, &[-32.0; 10]);
        feed(&mut h, &[-25.0; 60]);
        assert!(h.lra().unwrap() > 5.0);
    }

    #[test]
    fn reset_clears_the_graph_and_the_range() {
        let mut h = LoudnessHistory::new();
        feed(&mut h, &[-20.0; 10]);
        h.reset();
        assert!(h.is_empty());
        assert_eq!(h.lra(), None);
        // The partial second is forgotten too.
        h.push(-20.0, 47_000, SR);
        h.push(-20.0, 1_000, SR);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn non_finite_values_read_as_silence() {
        let mut h = LoudnessHistory::new();
        feed(&mut h, &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        let mut g = Vec::new();
        h.graph(&mut g);
        assert_eq!(g[0], FLOOR_LUFS);
        assert_eq!(g[1], FLOOR_LUFS);
        assert_eq!(h.lra(), None);
    }
}
