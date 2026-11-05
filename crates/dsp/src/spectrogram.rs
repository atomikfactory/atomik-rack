//! A scrolling time-frequency image for the spectrum analyser's spectrogram mode: one row of
//! levels per period, newest first, in 8 bits.
//!
//! The UI side feeds it the fine trace of [`crate::spectrum::Spectrum`] (dB per log-frequency
//! column) as often as it likes together with the number of audio frames since the last call; the
//! image advances by whole rows of [`Spectrogram::period_ms`] of audio, so how fast it scrolls does
//! not depend on how often the display redraws, and a paused engine does not scroll it.
//!
//! * **Columns.** The image has a fixed number of columns; a trace with more columns is reduced by
//!   taking the loudest of the columns that share one, so a narrow peak never disappears.
//! * **Levels.** A cell holds `(dB + 120) * 2` rounded and clamped to `0..=240`: half a dB of
//!   resolution over 120 dB. 0 is silence ([`FLOOR_DB`] and below).
//! * **Rows.** [`Spectrogram::rows_newest_first`] copies the image row by row, the newest row
//!   first; rows not yet written read 0.
//!
//! Nothing here runs on the audio thread.

use std::collections::VecDeque;

/// The level a cell of 0 stands for, dB.
pub const FLOOR_DB: f32 = -120.0;
/// Cell steps per dB.
pub const STEPS_PER_DB: f32 = 2.0;
/// The largest cell value (0 dB).
pub const MAX_CELL: u8 = 240;

/// The cell value of a level in dB.
pub fn quantise(db: f32) -> u8 {
    if db.is_nan() {
        return 0;
    }
    ((db - FLOOR_DB) * STEPS_PER_DB)
        .round()
        .clamp(0.0, f32::from(MAX_CELL)) as u8
}

/// The level in dB of a cell value.
pub fn level_db(cell: u8) -> f32 {
    f32::from(cell) / STEPS_PER_DB + FLOOR_DB
}

/// The scrolling image (module docs).
#[derive(Debug, Clone)]
pub struct Spectrogram {
    cols: usize,
    rows: usize,
    period_ms: f32,
    /// Newest row first; at most `rows` of them.
    history: VecDeque<Vec<u8>>,
    /// Frames of audio not yet worth a row.
    pending: u64,
}

impl Spectrogram {
    /// An empty image of `cols` columns and `rows` rows, one row per `period_ms` of audio.
    pub fn new(cols: usize, rows: usize, period_ms: f32) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            period_ms: period_ms.max(1.0),
            history: VecDeque::new(),
            pending: 0,
        }
    }

    /// Columns per row.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows of the image.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Milliseconds of audio per row.
    pub fn period_ms(&self) -> f32 {
        self.period_ms
    }

    /// Rows written so far (at most [`Self::rows`]).
    pub fn filled(&self) -> usize {
        self.history.len()
    }

    /// Forgets the image.
    pub fn clear(&mut self) {
        self.history.clear();
        self.pending = 0;
    }

    /// One row of `trace` reduced to the image's columns.
    fn row_of(&self, trace: &[f32]) -> Vec<u8> {
        let n = trace.len();
        let mut row = vec![0u8; self.cols];
        for (i, db) in trace.iter().enumerate() {
            let c = (i * self.cols / n).min(self.cols - 1);
            row[c] = row[c].max(quantise(*db));
        }
        row
    }

    /// Advances the image by the audio that passed (`elapsed_frames` at `sample_rate`): one row of
    /// `trace` for every full period, at most [`Self::rows`] per call. A call with an empty trace
    /// only counts the time. Returns the number of rows added.
    pub fn push(&mut self, trace: &[f32], elapsed_frames: u64, sample_rate: f32) -> usize {
        self.pending = self.pending.saturating_add(elapsed_frames);
        let step = ((f64::from(self.period_ms) * 0.001 * f64::from(sample_rate)) as u64).max(1);
        let due = (self.pending / step).min(self.rows as u64) as usize;
        if due == 0 || trace.is_empty() {
            return 0;
        }
        self.pending = if self.pending / step > self.rows as u64 {
            0
        } else {
            self.pending % step
        };
        let row = self.row_of(trace);
        for _ in 0..due {
            self.history.push_front(row.clone());
        }
        self.history.truncate(self.rows);
        due
    }

    /// The image row by row, the newest first, `rows * cols` cells (rows not yet written are 0).
    pub fn rows_newest_first(&self, out: &mut Vec<u8>) {
        out.clear();
        for row in &self.history {
            out.extend_from_slice(row);
        }
        out.resize(self.rows * self.cols, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_hold_half_a_db_over_120_db() {
        assert_eq!(quantise(0.0), MAX_CELL);
        assert_eq!(quantise(-120.0), 0);
        assert_eq!(quantise(-200.0), 0);
        assert_eq!(quantise(6.0), MAX_CELL);
        assert_eq!(quantise(f32::NAN), 0);
        assert_eq!(quantise(-60.0), 120);
        assert!((level_db(quantise(-33.5)) + 33.5).abs() <= 0.25);
    }

    #[test]
    fn the_image_advances_by_whole_periods_of_audio() {
        let mut g = Spectrogram::new(4, 5, 100.0);
        let trace = [-10.0f32; 4];
        // 100 ms at 48 kHz is 4800 frames.
        assert_eq!(g.push(&trace, 4799, 48_000.0), 0);
        assert_eq!(g.push(&trace, 1, 48_000.0), 1);
        assert_eq!(g.push(&trace, 9600, 48_000.0), 2);
        assert_eq!(g.filled(), 3);
        // The remainder is kept: a further 4799 frames are still short of a row.
        assert_eq!(g.push(&trace, 4799, 48_000.0), 0);
    }

    #[test]
    fn the_newest_row_is_first_and_unwritten_rows_are_silent() {
        let mut g = Spectrogram::new(2, 3, 10.0);
        g.push(&[-60.0, -60.0], 480, 48_000.0);
        g.push(&[-20.0, -20.0], 480, 48_000.0);
        let mut out = Vec::new();
        g.rows_newest_first(&mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(&out[..2], &[quantise(-20.0); 2]);
        assert_eq!(&out[2..4], &[quantise(-60.0); 2]);
        assert_eq!(&out[4..], &[0, 0]);
    }

    #[test]
    fn a_wide_trace_keeps_the_loudest_column_of_each_group() {
        let mut g = Spectrogram::new(2, 1, 10.0);
        g.push(&[-90.0, -30.0, -90.0, -90.0], 480, 48_000.0);
        let mut out = Vec::new();
        g.rows_newest_first(&mut out);
        assert_eq!(out, vec![quantise(-30.0), quantise(-90.0)]);
    }

    #[test]
    fn a_long_gap_fills_the_image_without_looping_forever() {
        let mut g = Spectrogram::new(1, 4, 10.0);
        assert_eq!(g.push(&[-10.0], u64::MAX, 48_000.0), 4);
        assert_eq!(g.filled(), 4);
        assert_eq!(g.push(&[-10.0], 479, 48_000.0), 0);
    }

    #[test]
    fn an_empty_trace_only_counts_the_time() {
        let mut g = Spectrogram::new(2, 2, 10.0);
        assert_eq!(g.push(&[], 4800, 48_000.0), 0);
        assert_eq!(g.filled(), 0);
        g.clear();
        assert_eq!(g.filled(), 0);
    }
}
