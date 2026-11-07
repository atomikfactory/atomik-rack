//! The state of a blind A/B test: which of two sounds is the processed one in each trial, what
//! the listener picked, and what the picks say.
//!
//! The listener hears two sounds, X and Y. In every trial one of them is the effect on and the
//! other the effect bypassed; which one is drawn at random for each trial (a seeded generator,
//! so a test can pin the order). The listener may play X and Y as often as they like and then
//! prefers one of them. After the last trial the picks are counted: how often the processed
//! sound won, and how likely that many wins (or a more lopsided score either way) would be by
//! pure guessing, an exact two-sided binomial test at one half. Below [`SIGNIFICANT`] the score
//! is called better than chance.
//!
//! Nothing here touches the document or the engine; the app turns the answers of
//! [`BlindTest::listen`] into engine params and restores the device when the test ends.

/// The number of trials of a new test.
pub const DEFAULT_TRIALS: usize = 10;
/// The most trials a test may have.
pub const MAX_TRIALS: usize = 40;
/// The probability under which a score is called better than chance.
pub const SIGNIFICANT: f64 = 0.05;

/// What the picks of a finished test say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlindScore {
    /// Trials in which the processed sound was picked.
    pub processed: usize,
    /// Trials judged.
    pub trials: usize,
    /// The chance of a score at least this lopsided (either way) by guessing.
    pub p: f64,
}

impl BlindScore {
    /// The score is unlikely to come from guessing.
    pub fn better_than_chance(&self) -> bool {
        self.p < SIGNIFICANT
    }

    /// The processed sound won more trials than the bypassed one.
    pub fn prefers_processed(&self) -> bool {
        self.processed * 2 > self.trials
    }

    /// The result as the lines the panel shows.
    pub fn lines(&self, device: &str) -> Vec<String> {
        let bypassed = self.trials - self.processed;
        let verdict = if !self.better_than_chance() {
            "No reliable preference: that score is within what guessing gives."
        } else if self.prefers_processed() {
            "You reliably prefer it with the effect on."
        } else {
            "You reliably prefer it with the effect bypassed."
        };
        vec![
            format!("{device}: {} trials", self.trials),
            format!(
                "Effect on picked {}, bypassed picked {bypassed}",
                self.processed
            ),
            format!(
                "Chance of a score this lopsided by guessing: {:.1} %",
                self.p * 100.0
            ),
            verdict.to_string(),
        ]
    }
}

/// The exact two-sided binomial probability (success chance one half) of a split at least as
/// lopsided as `wins` of `trials`. 1.0 for no trials.
pub fn two_sided_p(wins: usize, trials: usize) -> f64 {
    if trials == 0 {
        return 1.0;
    }
    let wins = wins.min(trials);
    let extreme = wins.max(trials - wins);
    // C(n, k) / 2^n by the running product, from the middle outwards is not needed: n <= 40.
    let mut coeff = 1.0f64;
    let mut tail = 0.0f64;
    for k in 0..=trials {
        if k > 0 {
            coeff *= (trials - k + 1) as f64 / k as f64;
        }
        if k >= extreme {
            tail += coeff;
        }
    }
    (2.0 * tail / 2f64.powi(trials as i32)).min(1.0)
}

/// A seeded generator (splitmix64): the order of a test is a pure function of its seed.
#[derive(Debug, Clone)]
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A blind A/B test (see the module docs).
#[derive(Debug, Clone)]
pub struct BlindTest {
    /// Per trial: slot 0 (X) is the processed sound.
    x_processed: Vec<bool>,
    /// Per judged trial: the slot picked.
    picks: Vec<u8>,
    /// Slots heard in the current trial.
    heard: [bool; 2],
    /// The slot playing now.
    listening: Option<u8>,
}

impl BlindTest {
    /// A test of `trials` trials (1 to [`MAX_TRIALS`]) whose order comes from `seed`.
    pub fn new(trials: usize, seed: u64) -> Self {
        let trials = trials.clamp(1, MAX_TRIALS);
        let mut draw = Draw(seed);
        Self {
            x_processed: (0..trials).map(|_| draw.next() & 1 == 1).collect(),
            picks: Vec::with_capacity(trials),
            heard: [false; 2],
            listening: None,
        }
    }

    /// How many trials the test has.
    pub fn trials(&self) -> usize {
        self.x_processed.len()
    }

    /// The trial being judged, counting from 1 (the last one once the test is over).
    pub fn trial(&self) -> usize {
        (self.picks.len() + 1).min(self.trials())
    }

    /// Whether every trial was judged.
    pub fn finished(&self) -> bool {
        self.picks.len() >= self.trials()
    }

    /// The slot playing now.
    pub fn listening(&self) -> Option<u8> {
        self.listening
    }

    /// Whether the current trial may be judged: both sounds were heard.
    pub fn can_pick(&self) -> bool {
        !self.finished() && self.heard == [true, true]
    }

    /// Whether slot `slot` (0 or 1) is the processed sound in the current trial. The app keeps
    /// this to itself until the test is over.
    fn processed(&self, slot: u8) -> bool {
        let x = self.x_processed[self.picks.len().min(self.trials() - 1)];
        (slot == 0) == x
    }

    /// Plays slot `slot` (0 or 1): returns whether the effect is on for it, or `None` when the
    /// test is over.
    pub fn listen(&mut self, slot: u8) -> Option<bool> {
        if self.finished() {
            return None;
        }
        let slot = slot.min(1);
        self.heard[usize::from(slot)] = true;
        self.listening = Some(slot);
        Some(self.processed(slot))
    }

    /// Judges the current trial for slot `slot`; false (nothing recorded) until both sounds
    /// were heard. The next trial starts with nothing heard.
    pub fn pick(&mut self, slot: u8) -> bool {
        if !self.can_pick() {
            return false;
        }
        self.picks.push(slot.min(1));
        // The pick is scored against the trial just judged, before the next one is drawn.
        self.heard = [false; 2];
        self.listening = None;
        true
    }

    /// What the picks say; `None` until every trial was judged.
    pub fn score(&self) -> Option<BlindScore> {
        if !self.finished() {
            return None;
        }
        let processed = self
            .picks
            .iter()
            .zip(&self.x_processed)
            .filter(|(pick, x)| (**pick == 0) == **x)
            .count();
        Some(BlindScore {
            processed,
            trials: self.trials(),
            p: two_sided_p(processed, self.trials()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exact_test_matches_known_values() {
        assert!((two_sided_p(5, 10) - 1.0).abs() < 1e-12);
        // 9 or 10 (or 0 or 1) of 10: 2 * 11 / 1024.
        assert!((two_sided_p(9, 10) - 22.0 / 1024.0).abs() < 1e-12);
        assert!((two_sided_p(1, 10) - 22.0 / 1024.0).abs() < 1e-12);
        // 10 of 10: 2 / 1024.
        assert!((two_sided_p(10, 10) - 2.0 / 1024.0).abs() < 1e-12);
        // 8 of 10: 2 * 56 / 1024, not significant at 5 %? 0.109.
        assert!((two_sided_p(8, 10) - 112.0 / 1024.0).abs() < 1e-12);
        assert!((two_sided_p(0, 0) - 1.0).abs() < 1e-12);
        assert!(two_sided_p(40, 40) > 0.0);
    }

    #[test]
    fn the_order_follows_the_seed_and_varies_between_trials() {
        let a = BlindTest::new(20, 7);
        let b = BlindTest::new(20, 7);
        assert_eq!(a.x_processed, b.x_processed);
        assert_ne!(a.x_processed, BlindTest::new(20, 8).x_processed);
        let wins = a.x_processed.iter().filter(|x| **x).count();
        assert!(wins > 2 && wins < 18, "both orders occur: {wins}");
    }

    #[test]
    fn a_pick_needs_both_sounds_heard_and_moves_to_the_next_trial() {
        let mut t = BlindTest::new(3, 1);
        assert_eq!(t.trial(), 1);
        assert!(!t.pick(0), "nothing heard yet");
        t.listen(0);
        assert!(!t.pick(0), "only X heard");
        t.listen(1);
        assert!(t.can_pick());
        assert!(t.pick(1));
        assert_eq!(t.trial(), 2);
        assert!(!t.can_pick(), "the next trial starts with nothing heard");
        assert_eq!(t.listening(), None);
    }

    #[test]
    fn the_two_slots_always_differ_in_a_trial() {
        let mut t = BlindTest::new(8, 99);
        for _ in 0..8 {
            let x = t.listen(0).unwrap();
            let y = t.listen(1).unwrap();
            assert_ne!(x, y);
            assert!(t.pick(0));
        }
        assert!(t.listen(0).is_none(), "over");
    }

    #[test]
    fn always_picking_the_processed_sound_is_better_than_chance() {
        let mut t = BlindTest::new(10, 5);
        for _ in 0..10 {
            let x_on = t.listen(0).unwrap();
            t.listen(1);
            t.pick(if x_on { 0 } else { 1 });
        }
        let s = t.score().unwrap();
        assert_eq!((s.processed, s.trials), (10, 10));
        assert!(s.better_than_chance() && s.prefers_processed());
        assert!(
            s.lines("Reverb")
                .last()
                .unwrap()
                .contains("with the effect on")
        );
    }

    #[test]
    fn picking_x_every_time_is_no_preference_for_a_balanced_order() {
        let mut t = BlindTest::new(10, 5);
        for _ in 0..10 {
            t.listen(0);
            t.listen(1);
            t.pick(0);
        }
        let s = t.score().unwrap();
        // Which sound X is changes with the trial, so picking X says nothing.
        assert!(s.p >= 0.02, "{s:?}");
        assert!(t.score().is_some());
        let mut u = BlindTest::new(10, 5);
        assert!(u.score().is_none(), "not before the last trial");
        u.listen(0);
    }

    #[test]
    fn always_picking_the_bypassed_sound_is_a_reliable_dislike() {
        let mut t = BlindTest::new(12, 3);
        for _ in 0..12 {
            let x_on = t.listen(0).unwrap();
            t.listen(1);
            t.pick(if x_on { 1 } else { 0 });
        }
        let s = t.score().unwrap();
        assert_eq!(s.processed, 0);
        assert!(s.better_than_chance() && !s.prefers_processed());
        assert!(s.lines("EQ").last().unwrap().contains("bypassed"));
    }
}
