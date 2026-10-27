//! The building blocks of the FM synth: its operator routings ("algorithms"), the operator
//! frequency rule and the bandwidth limiter that keeps bright patches from aliasing.
//!
//! * **Operators.** Four sine operators, numbered 1 to 4. Every operator is phase-modulated by
//!   the outputs of the operators the routing names, so the routing is a small acyclic graph
//!   in which an operator is only ever modulated by operators with a **higher** number. The
//!   voice computes operator 4 first and operator 1 last; no routing needs a delay.
//! * **Algorithms.** [`ALGORITHMS`] lists twelve routings: the operators that are heard (the
//!   carriers), the operators that modulate others, and the one operator that feeds back into
//!   itself. They are plain arithmetic, written down here; the panel draws them from the same
//!   table.
//! * **Frequency.** [`op_frequency`]: a ratio of the played pitch (coarse step plus a fraction)
//!   or a fixed frequency that ignores the key.
//! * **Bandwidth limit.** Phase modulation puts a partial at every multiple of the modulator's
//!   frequency, out to about `(index + 1)` times it (Carson's rule), and a partial above Nyquist
//!   folds back as an alias. [`limits`] walks the routing from operator 4 down, estimates the
//!   highest partial each operator produces and scales the modulation index of any operator
//!   whose estimate would pass `bw_max`, so a bright patch gets duller on a high key instead of
//!   aliasing. The scale is continuous in the frequencies, so a pitch glide or a vibrato never
//!   jumps. Feedback is limited the same way.

/// Number of operators.
pub const OPERATORS: usize = 4;
/// Number of algorithms.
pub const ALGORITHM_COUNT: usize = 12;
/// Phase deviation in radians of a modulator at full level and a modulation index of 1.
pub const MAX_INDEX_RAD: f32 = 8.0;
/// Phase deviation in radians of the feedback at `feedback` = 1.
pub const MAX_FEEDBACK_RAD: f32 = 4.0;
/// Coarse steps of an operator (`0..=31`; step 0 is the ratio 0.5).
pub const COARSE_STEPS: usize = 32;

/// One operator routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Algorithm {
    /// Name shown in the algorithm selector.
    pub name: &'static str,
    /// For each operator, the bit mask of the operators that modulate it (bit `j` is operator
    /// `j + 1`). Every set bit is above the operator's own, so operator 4 is computed first.
    pub mods: [u8; OPERATORS],
    /// Bit mask of the carriers: the operators that reach the output.
    pub carriers: u8,
    /// The operator (0-based) whose output feeds back into its own phase.
    pub feedback: u8,
}

impl Algorithm {
    /// Whether operator `op` (0-based) is modulated by operator `from`.
    pub fn modulates(&self, from: usize, op: usize) -> bool {
        from < OPERATORS && op < OPERATORS && self.mods[op] >> from & 1 == 1
    }

    /// Whether operator `op` (0-based) reaches the output.
    pub fn is_carrier(&self, op: usize) -> bool {
        op < OPERATORS && self.carriers >> op & 1 == 1
    }

    /// How many carriers there are.
    pub fn carrier_count(&self) -> usize {
        self.carriers.count_ones() as usize
    }

    /// The gain of the carrier sum: `1 / sqrt(carriers)`, so adding carriers adds loudness
    /// more slowly than the number of carriers.
    pub fn carrier_gain(&self) -> f32 {
        1.0 / (self.carrier_count().max(1) as f32).sqrt()
    }
}

const fn alg(name: &'static str, edges: &[(u8, u8)], carriers: &[u8], feedback: u8) -> Algorithm {
    // `edges` are (modulator, target) operator numbers (1-based).
    let mut mods = [0u8; OPERATORS];
    let mut i = 0;
    while i < edges.len() {
        mods[edges[i].1 as usize - 1] |= 1 << (edges[i].0 - 1);
        i += 1;
    }
    let mut mask = 0u8;
    let mut c = 0;
    while c < carriers.len() {
        mask |= 1 << (carriers[c] - 1);
        c += 1;
    }
    Algorithm {
        name,
        mods,
        carriers: mask,
        feedback: feedback - 1,
    }
}

/// The routings, in selector order. Edges read "operator a modulates operator b".
pub const ALGORITHMS: [Algorithm; ALGORITHM_COUNT] = [
    alg("Stack", &[(4, 3), (3, 2), (2, 1)], &[1], 4),
    alg("Twin stack", &[(4, 3), (2, 1)], &[3, 1], 4),
    alg("Y", &[(4, 2), (3, 2), (2, 1)], &[1], 4),
    alg("Fork", &[(4, 1), (4, 2), (4, 3)], &[1, 2, 3], 4),
    alg("Three into one", &[(4, 1), (3, 1), (2, 1)], &[1], 4),
    alg("Stack and one", &[(4, 3), (3, 2)], &[2, 1], 4),
    alg("Branch", &[(4, 3), (3, 2), (3, 1)], &[2, 1], 4),
    alg("Two chains", &[(4, 3), (3, 1), (2, 1)], &[1], 4),
    alg(
        "Pair into pair",
        &[(4, 1), (4, 2), (3, 1), (3, 2)],
        &[1, 2],
        4,
    ),
    alg("Stack and two", &[(4, 3)], &[3, 2, 1], 4),
    alg("Tapped stack", &[(4, 3), (3, 2), (2, 1)], &[2, 1], 4),
    alg("Additive", &[], &[1, 2, 3, 4], 4),
];

/// The algorithm names, in selector order (for the choice parameter).
pub const ALGORITHM_NAMES: [&str; ALGORITHM_COUNT] = [
    ALGORITHMS[0].name,
    ALGORITHMS[1].name,
    ALGORITHMS[2].name,
    ALGORITHMS[3].name,
    ALGORITHMS[4].name,
    ALGORITHMS[5].name,
    ALGORITHMS[6].name,
    ALGORITHMS[7].name,
    ALGORITHMS[8].name,
    ALGORITHMS[9].name,
    ALGORITHMS[10].name,
    ALGORITHMS[11].name,
];

/// The algorithm of a choice index (out of range clamps).
pub fn algorithm(index: usize) -> &'static Algorithm {
    &ALGORITHMS[index.min(ALGORITHM_COUNT - 1)]
}

/// The frequency of an operator in Hz.
///
/// * Ratio mode (`fixed` false): `key_hz * ratio`, with the ratio the coarse step (step 0 is
///   0.5, step `n` is `n`) plus the `fine` fraction `0..1`.
/// * Fixed mode: `10^((coarse + fine) / 8)` Hz, whatever the key: 1 Hz at step 0, a decade
///   every eight steps, about 10 kHz at the top.
pub fn op_frequency(fixed: bool, coarse: f32, fine: f32, key_hz: f32) -> f32 {
    if fixed {
        crate::exp2_det((coarse + fine) * 0.125 * std::f32::consts::LOG2_10)
    } else {
        key_hz * (coarse.max(0.5) + fine)
    }
}

/// What [`limits`] returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// For each operator, the scale `0..=1` of the modulation index it receives.
    pub index: [f32; OPERATORS],
    /// The scale `0..=1` of the feedback.
    pub feedback: f32,
}

/// The scales that keep every operator's partials below `bw_max` Hz.
///
/// `freq` is each operator's frequency in Hz, `depth` the modulation index in radians each
/// operator would give its targets (its level times the index, before limiting), `feedback_rad`
/// the feedback in radians. An operator with a `depth` of nothing does not count.
pub fn limits(
    alg: &Algorithm,
    freq: &[f32; OPERATORS],
    depth: &[f32; OPERATORS],
    feedback_rad: f32,
    bw_max: f32,
) -> Limits {
    const USED: f32 = 1e-4;
    let mut out = Limits {
        index: [1.0; OPERATORS],
        feedback: 1.0,
    };
    let mut bw = [0.0f32; OPERATORS];
    for op in (0..OPERATORS).rev() {
        let f = freq[op].max(0.0);
        let (mut b_in, mut i_nom) = (0.0f32, 0.0f32);
        for m in (op + 1)..OPERATORS {
            if alg.modulates(m, op) && depth[m] > USED {
                b_in = b_in.max(bw[m]);
                i_nom += depth[m];
            }
        }
        let mut b = f;
        if i_nom > 0.0 {
            // Carson: the highest partial is about f + (index + 1) * (modulator bandwidth).
            let room = ((bw_max - f) / b_in.max(1e-3) - 1.0).max(0.0);
            let s = if i_nom > room { room / i_nom } else { 1.0 };
            out.index[op] = s;
            if s > 0.0 {
                b = f + (i_nom * s + 1.0) * b_in;
            }
        }
        if op == usize::from(alg.feedback) && feedback_rad > USED {
            let room = ((bw_max - b) / f.max(1e-3)).max(0.0);
            let s = (room / feedback_rad).min(1.0);
            out.feedback = s;
            b += s * feedback_rad * f;
        }
        bw[op] = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modulator_is_above_its_target_so_the_order_is_four_down_to_one() {
        for a in &ALGORITHMS {
            for op in 0..OPERATORS {
                for from in 0..OPERATORS {
                    if a.modulates(from, op) {
                        assert!(from > op, "{}: {} modulates {}", a.name, from + 1, op + 1);
                    }
                }
            }
        }
    }

    #[test]
    fn the_table_is_well_formed() {
        assert_eq!(ALGORITHMS.len(), ALGORITHM_COUNT);
        let mut names = std::collections::BTreeSet::new();
        for (i, a) in ALGORITHMS.iter().enumerate() {
            assert!(names.insert(a.name), "{} twice", a.name);
            assert_eq!(ALGORITHM_NAMES[i], a.name);
            assert!(a.carriers != 0 && a.carriers < 16, "{}: carriers", a.name);
            assert!(usize::from(a.feedback) < OPERATORS, "{}: feedback", a.name);
            // Every operator is heard: a carrier, or a modulator of something that is.
            let mut heard = a.carriers;
            for op in 0..OPERATORS {
                if heard >> op & 1 == 1 {
                    heard |= a.mods[op];
                }
            }
            assert_eq!(heard, 15, "{}: an operator that is never heard", a.name);
            // The routings differ from each other.
            for b in &ALGORITHMS[..i] {
                assert!(
                    (a.mods, a.carriers) != (b.mods, b.carriers),
                    "{} = {}",
                    a.name,
                    b.name
                );
            }
        }
        assert_eq!(algorithm(99).name, "Additive");
    }

    #[test]
    fn the_carrier_gain_shrinks_with_the_carrier_count() {
        assert_eq!(ALGORITHMS[0].carrier_count(), 1);
        assert_eq!(ALGORITHMS[0].carrier_gain(), 1.0);
        assert_eq!(ALGORITHMS[11].carrier_count(), 4);
        assert!((ALGORITHMS[11].carrier_gain() - 0.5).abs() < 1e-6);
        assert!(ALGORITHMS[0].is_carrier(0) && !ALGORITHMS[0].is_carrier(3));
        assert!(!ALGORITHMS[0].is_carrier(9));
    }

    #[test]
    fn ratio_and_fixed_frequencies() {
        let hz = 220.0;
        assert_eq!(op_frequency(false, 1.0, 0.0, hz), 220.0);
        assert_eq!(op_frequency(false, 0.0, 0.0, hz), 110.0);
        assert!((op_frequency(false, 3.0, 0.5, hz) - 770.0).abs() < 1e-3);
        assert!((op_frequency(false, 14.0, 0.0, hz) - 3080.0).abs() < 1e-3);
        // Fixed: the key does not matter, a decade per eight steps.
        assert!((op_frequency(true, 0.0, 0.0, hz) - 1.0).abs() < 1e-6);
        assert!((op_frequency(true, 8.0, 0.0, hz) - 10.0).abs() < 1e-4);
        assert!((op_frequency(true, 24.0, 0.0, 55.0) - 1000.0).abs() < 0.1);
        assert_eq!(
            op_frequency(true, 16.0, 0.0, 55.0),
            op_frequency(true, 16.0, 0.0, 880.0)
        );
    }

    fn stack_limits(f0: f32, ratio: f32, depth: f32) -> Limits {
        let freq = [f0, f0 * ratio, 0.0, 0.0];
        let alg = Algorithm {
            name: "pair",
            mods: [0b0010, 0, 0, 0],
            carriers: 1,
            feedback: 3,
        };
        limits(&alg, &freq, &[0.0, depth, 0.0, 0.0], 0.0, 19_200.0)
    }

    #[test]
    fn a_low_note_keeps_the_whole_index_and_a_high_one_loses_it() {
        let low = stack_limits(110.0, 1.0, 8.0);
        assert_eq!(low.index[0], 1.0);
        let mid = stack_limits(2_000.0, 1.0, 8.0);
        let high = stack_limits(4_000.0, 1.0, 8.0);
        assert!(mid.index[0] < 1.0 && high.index[0] < mid.index[0]);
        // A modulator above the limit gives no modulation at all.
        assert_eq!(stack_limits(4_000.0, 6.0, 1.0).index[0], 0.0);
    }

    #[test]
    fn the_scale_is_continuous_in_the_pitch() {
        let mut last = 1.0f32;
        for k in 0..400 {
            let f = 200.0 + k as f32 * 20.0;
            let s = stack_limits(f, 2.0, 6.0).index[0];
            assert!(s <= last + 1e-6, "monotone at {f}");
            assert!(last - s < 0.02, "no jump at {f}: {last} -> {s}");
            last = s;
        }
        assert_eq!(last, 0.0);
    }

    #[test]
    fn an_unused_modulator_does_not_count() {
        let l = stack_limits(8_000.0, 2.0, 0.0);
        assert_eq!(l.index, [1.0; OPERATORS]);
    }

    #[test]
    fn limited_bandwidth_never_passes_the_limit() {
        for alg in &ALGORITHMS {
            for f0 in [55.0f32, 220.0, 880.0, 3_520.0] {
                let freq = [f0, f0 * 2.0, f0 * 3.5, f0 * 7.0];
                let depth = [MAX_INDEX_RAD; OPERATORS];
                let l = limits(alg, &freq, &depth, MAX_FEEDBACK_RAD, 19_200.0);
                for s in l.index.iter().chain([&l.feedback]) {
                    assert!((0.0..=1.0).contains(s), "{}: {s}", alg.name);
                }
                // Re-estimate the bandwidth with the scales applied: wherever a modulation or
                // feedback is left, its estimate stays under the limit.
                let mut bw = [0.0f32; OPERATORS];
                for op in (0..OPERATORS).rev() {
                    let (mut b_in, mut i) = (0.0f32, 0.0f32);
                    for m in (op + 1)..OPERATORS {
                        if alg.modulates(m, op) {
                            b_in = b_in.max(bw[m]);
                            i += depth[m] * l.index[op];
                        }
                    }
                    bw[op] = freq[op] + if i > 0.0 { (i + 1.0) * b_in } else { 0.0 };
                    let fb = op == usize::from(alg.feedback) && l.feedback > 0.0;
                    if fb {
                        bw[op] += l.feedback * MAX_FEEDBACK_RAD * freq[op];
                    }
                    if i > 0.0 || fb {
                        assert!(
                            bw[op] <= 19_200.0 + 1.0,
                            "{} f0 {f0} op {op}: {}",
                            alg.name,
                            bw[op]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn feedback_is_limited_on_a_high_note() {
        let alg = &ALGORITHMS[11];
        let freq = [1_000.0, 1_000.0, 1_000.0, 4_000.0];
        let l = limits(alg, &freq, &[0.0; OPERATORS], MAX_FEEDBACK_RAD, 19_200.0);
        assert!(l.feedback < 1.0 && l.feedback > 0.0, "{}", l.feedback);
        let low = limits(
            alg,
            &[100.0; OPERATORS],
            &[0.0; OPERATORS],
            MAX_FEEDBACK_RAD,
            19_200.0,
        );
        assert_eq!(low.feedback, 1.0);
    }
}
