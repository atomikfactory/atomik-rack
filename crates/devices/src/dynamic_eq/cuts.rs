//! Dynamic cuts for the resonances [`atomik_dsp::resonance`] found: which bands take them and
//! what each band is set to.
//!
//! A resonance becomes a bell (bands 2 to 4) that is on, has no static gain and a negative
//! `range`, so it cuts only while the band of its frequency is louder than the trend around it:
//!
//! * `freq` the resonance, `q` its estimated Q within 1 .. 18 (a very narrow bell needs a
//!   deliberate hand, so a cut never gets sharper than that is useful);
//! * `range` minus the prominence of the peak, at least 3 and at most 12 dB;
//! * `threshold` a quarter of the way from the level of the trend to the level of the peak (in the
//!   detector scale, within -60 .. 0), so ordinary material around the resonance does not move
//!   the band and the resonance does;
//! * `ratio` 20 and the soft knee, `attack` 8 ms, `release` 120 ms; `source` the input and
//!   `channel` stereo.
//!
//! The peaks are taken in the order they were ranked and go to the free bells in band order; a
//! bell that is on already is never touched.

use super::BANDS;
use atomik_dsp::resonance::Resonance;

/// The bands that can take a resonance: the three bells, as 0-based band indices.
pub const BELLS: [usize; 3] = [1, 2, 3];

/// The settings of one band that cuts a resonance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    /// Centre frequency, Hz.
    pub freq_hz: f32,
    /// Q of the bell.
    pub q: f32,
    /// Level above which the band cuts, dB.
    pub threshold_db: f32,
    /// How far the band can cut, dB (negative).
    pub range_db: f32,
}

impl Cut {
    /// The cut for one resonance (module docs).
    pub fn for_resonance(r: &Resonance) -> Self {
        let excess = (r.level_db - r.trend_db).max(0.0);
        Self {
            freq_hz: r.freq_hz.clamp(20.0, 20_000.0),
            q: r.q.clamp(1.0, 18.0),
            threshold_db: (r.trend_db + 0.25 * excess).clamp(-60.0, 0.0),
            range_db: -r.prominence_db.clamp(3.0, 12.0),
        }
    }

    /// The `(key, value)` rows that set band `band` (0-based) to this cut, in a fixed order.
    pub fn params(&self, band: usize) -> Vec<(String, f32)> {
        let n = band + 1;
        let row = |k: &str, v: f32| (format!("band{n}.{k}"), v);
        vec![
            row("freq", self.freq_hz),
            row("q", self.q),
            row("gain", 0.0),
            row("threshold", self.threshold_db),
            row("range", self.range_db),
            row("ratio", 20.0),
            row("attack", 8.0),
            row("release", 120.0),
            row("source", 0.0),
            row("channel", 0.0),
            row("on", 1.0),
        ]
    }
}

/// Which bells are free (off) given the `on` state of every band.
pub fn free_bells(on: &[bool; BANDS]) -> Vec<usize> {
    BELLS.into_iter().filter(|&b| !on[b]).collect()
}

/// The cuts for the best of `resonances` (already ranked) on the free bells `free`: one
/// `(band, cut)` per band used, as many as there are resonances and free bells.
pub fn plan(resonances: &[Resonance], free: &[usize]) -> Vec<(usize, Cut)> {
    free.iter()
        .zip(resonances)
        .map(|(&band, r)| (band, Cut::for_resonance(r)))
        .collect()
}
