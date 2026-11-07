//! Masking analysis: how much of one track's content in a frequency band is covered by another
//! track that is louder in the same band at the same time.
//!
//! It runs on the UI side's workers from rendered audio, never on the audio thread, so nothing
//! here has to be real-time safe. The FFT is the in-house radix-2 transform, the window and the
//! band edges use the deterministic sine and exponential, and the dB come from [`log2_det`], so
//! the same audio gives bit-identical numbers on every platform.
//!
//! # Band map
//!
//! [`analyse`] turns a stereo track into a [`BandMap`]: for every block of [`HOP`] frames the level
//! of each of the [`BANDS`] one-third-octave bands from 40 Hz to about 18 kHz.
//!
//! * **Blocks.** Block `i` is a periodic Hann window of [`FFT_SIZE`] frames centred on the middle of
//!   frames `i * HOP .. (i + 1) * HOP`, so consecutive windows overlap by half and a block reads
//!   the audio around its own span (a sound at the very start or end of the track still counts).
//! * **Mono.** The two channels are summed to `(L + R) / 2` before the transform.
//! * **Level.** The power of the bins inside the band edges, scaled so that a sine of amplitude `A`
//!   inside a band reads `20 log10(A)` (0 dB is a full-scale sine; the scalloping of the window
//!   loses at most 1.4 dB between two bins). A band with no power reads [`SILENT_DB`]. The lowest
//!   bands are narrower than a bin at low rates and read the power density times their width, as
//!   the analyser device does, so they under-read a pure tone.
//!
//! # Pair score
//!
//! [`rank_pairs`] compares every pair of tracks. A **cell** is one band of one block.
//!
//! * A track is **active** in a cell when its level is at least [`ACTIVE_DB`] and within
//!   [`ACTIVE_SPAN_DB`] of the loudest cell of that track, so the noise floor of a band-limited
//!   track does not count as content.
//! * A cell **clashes** for the ordered pair (masker, masked) when both tracks are active in it and
//!   the masker is louder by at least [`MIN_GAP_DB`].
//! * A clashing cell scores `gap / FULL_GAP_DB` (at most 1, so a gap of [`FULL_GAP_DB`] or more
//!   counts fully) times a weight that grows from 0 at [`ACTIVE_DB`] to 1 at 40 dB above it
//!   (a clash far below the audible level is worth less).
//! * The **band score** is the sum of the cell scores of the band divided by the number of blocks,
//!   times 100: the percentage of the analysed time the band is covered, in full.
//! * Of the two directions of a pair the one with the larger total over the bands is the pair's
//!   (the masker is the track that is louder where they meet more). The **pair score** is the
//!   highest band score of that direction; the pair is ranked by it (then by the total of its band
//!   scores, then by the order of the tracks). Pairs below [`MIN_SCORE`] are left out.
//! * The time ranges of a pair are the runs of blocks in which its worst band clashes, with runs
//!   separated by at most [`MERGE_GAP`] blocks joined, the [`MAX_RANGES`] longest, in time order.
//!   The two spectra a pair carries are the mean level of each track over those ranges.

use crate::det_trig::cos_cycles;
use crate::spectral::Fft;
use crate::{exp2_det_f64, log2_det};

/// Number of bands: one-third octaves, the first centred on [`FIRST_HZ`].
pub const BANDS: usize = 27;
/// The centre of the first band in Hz.
pub const FIRST_HZ: f64 = 40.0;
/// Frames of one analysis window.
pub const FFT_SIZE: usize = 8192;
/// Frames between two blocks.
pub const HOP: usize = FFT_SIZE / 2;
/// The level of a band with no power, dB.
pub const SILENT_DB: f32 = -120.0;
/// The level below which a track has no content in a cell, dB.
pub const ACTIVE_DB: f32 = -60.0;
/// A cell is content only within this many dB of the loudest cell of its track.
pub const ACTIVE_SPAN_DB: f32 = 40.0;
/// The smallest lead that makes one track the masker of another in a cell, dB.
pub const MIN_GAP_DB: f32 = 3.0;
/// The lead that scores a cell in full, dB.
pub const FULL_GAP_DB: f32 = 18.0;
/// Pairs scoring less than this (percent) are not reported.
pub const MIN_SCORE: f32 = 2.0;
/// Runs of clashing blocks this close together (in blocks) are one time range.
pub const MERGE_GAP: usize = 2;
/// The most time ranges a pair reports.
pub const MAX_RANGES: usize = 16;
/// The most tracks one analysis compares.
pub const MAX_TRACKS: usize = 48;

/// The centre of band `i` in Hz.
pub fn band_centre(i: usize) -> f32 {
    (FIRST_HZ * exp2_det_f64(i as f64 / 3.0)) as f32
}

/// The lower and upper edge of band `i` in Hz (a sixth of an octave either side of the centre).
pub fn band_edges(i: usize) -> (f32, f32) {
    let c = FIRST_HZ * exp2_det_f64(i as f64 / 3.0);
    (
        (c * exp2_det_f64(-1.0 / 6.0)) as f32,
        (c * exp2_det_f64(1.0 / 6.0)) as f32,
    )
}

/// The band that holds `hz` (the nearest centre on a log scale), clamped to the map.
pub fn band_of(hz: f32) -> usize {
    if hz <= 0.0 {
        return 0;
    }
    let steps = 3.0 * log2_det(hz / FIRST_HZ as f32);
    (steps.round().max(0.0) as usize).min(BANDS - 1)
}

/// A short label for band `i`: `63 Hz`, `1.6 kHz`.
pub fn band_label(i: usize) -> String {
    let hz = band_centre(i);
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k >= 10.0 {
            format!("{k:.0} kHz")
        } else {
            format!("{k:.1} kHz")
        }
    } else {
        format!("{hz:.0} Hz")
    }
}

/// The level of every band over time for one track.
#[derive(Debug, Clone, PartialEq)]
pub struct BandMap {
    /// Sample rate of the analysed audio.
    pub rate: u32,
    /// The level in dB of each band, one row per block of [`HOP`] frames.
    pub levels: Vec<[f32; BANDS]>,
}

impl BandMap {
    /// The loudest cell of the map, [`SILENT_DB`] for an empty one.
    pub fn peak(&self) -> f32 {
        self.levels
            .iter()
            .flatten()
            .fold(SILENT_DB, |p, l| p.max(*l))
    }

    /// The first frame of block `i`.
    pub fn block_start(i: usize) -> u64 {
        (i * HOP) as u64
    }

    /// The mean level (power mean, in dB) of each band over the `ranges` of blocks (half open),
    /// or over the whole map when `ranges` is empty.
    pub fn mean(&self, ranges: &[(usize, usize)]) -> [f32; BANDS] {
        let mut power = [0.0f64; BANDS];
        let mut count = 0usize;
        let mut add = |row: &[f32; BANDS]| {
            for (p, l) in power.iter_mut().zip(row) {
                *p += db_to_power(*l);
            }
            count += 1;
        };
        if ranges.is_empty() {
            self.levels.iter().for_each(&mut add);
        } else {
            for &(from, to) in ranges {
                self.levels
                    .iter()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .for_each(&mut add);
            }
        }
        std::array::from_fn(|b| {
            if count == 0 {
                SILENT_DB
            } else {
                power_to_db(power[b] / count as f64)
            }
        })
    }
}

fn db_to_power(db: f32) -> f64 {
    if db <= SILENT_DB {
        0.0
    } else {
        exp2_det_f64(f64::from(db) * 0.1 * std::f64::consts::LOG2_10)
    }
}

fn power_to_db(p: f64) -> f32 {
    if p <= 0.0 {
        SILENT_DB
    } else {
        (10.0 * log2_det(p as f32) * std::f32::consts::LOG10_2).max(SILENT_DB)
    }
}

/// The bin range `[lo, hi)` of every band for a window of `FFT_SIZE` frames at `rate`, as
/// fractional bin positions (a bin that straddles an edge counts in proportion).
fn edges_in_bins(rate: u32) -> [(f64, f64); BANDS] {
    let per_bin = f64::from(rate) / FFT_SIZE as f64;
    std::array::from_fn(|b| {
        let (lo, hi) = band_edges(b);
        (f64::from(lo) / per_bin, f64::from(hi) / per_bin)
    })
}

/// The band levels of one block of `FFT_SIZE` mono frames (already windowed): see the module
/// docs for the scale.
fn block_levels(
    fft: &Fft,
    re: &mut [f32],
    im: &mut [f32],
    edges: &[(f64, f64); BANDS],
    scale: f64,
) -> [f32; BANDS] {
    im.fill(0.0);
    fft.forward(re, im);
    let bins = FFT_SIZE / 2;
    let power =
        |k: usize| f64::from(re[k]) * f64::from(re[k]) + f64::from(im[k]) * f64::from(im[k]);
    std::array::from_fn(|b| {
        let (lo, hi) = edges[b];
        let mut sum = 0.0f64;
        // The bins between the edges; the first and last count by their overlap with the band.
        let first = lo.floor().max(0.0) as usize;
        let last = (hi.ceil() as usize).min(bins);
        for k in first..last {
            // Bin k covers [k - 0.5, k + 0.5) around its centre frequency.
            let overlap = (hi.min(k as f64 + 0.5) - lo.max(k as f64 - 0.5)).clamp(0.0, 1.0);
            sum += power(k) * overlap;
        }
        if sum <= 0.0 {
            SILENT_DB
        } else {
            power_to_db(sum * scale)
        }
    })
}

/// The band map of a stereo track (see the module docs). `right` may be empty for a mono track;
/// the two are read up to the shorter one when both are given.
pub fn analyse(left: &[f32], right: &[f32], rate: u32) -> BandMap {
    let len = if right.is_empty() {
        left.len()
    } else {
        left.len().min(right.len())
    };
    let blocks = len.div_ceil(HOP);
    let fft = Fft::new(FFT_SIZE);
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| (0.5 - 0.5 * cos_cycles(i as f64 / FFT_SIZE as f64)) as f32)
        .collect();
    let energy: f64 = window.iter().map(|w| f64::from(*w) * f64::from(*w)).sum();
    // A sine of amplitude A: the one-sided power sum is N * A^2 * sum(w^2) / 4.
    let scale = 4.0 / (FFT_SIZE as f64 * energy);
    let edges = edges_in_bins(rate.max(1));
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    let sample = |i: isize| -> f32 {
        if i < 0 || i as usize >= len {
            return 0.0;
        }
        let i = i as usize;
        let l = left[i];
        let v = if right.is_empty() {
            l
        } else {
            (l + right[i]) * 0.5
        };
        if v.is_finite() { v } else { 0.0 }
    };
    let lead = ((FFT_SIZE - HOP) / 2) as isize;
    let levels = (0..blocks)
        .map(|b| {
            let start = (b * HOP) as isize - lead;
            for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
                *r = sample(start + i as isize) * w;
            }
            block_levels(&fft, &mut re, &mut im, &edges, scale)
        })
        .collect();
    BandMap { rate, levels }
}

/// A run of blocks, half open.
pub type BlockRange = (usize, usize);

/// How two tracks mask each other (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct PairScore {
    /// The index of the louder track in the list given to [`rank_pairs`].
    pub masker: usize,
    /// The index of the track that is covered.
    pub masked: usize,
    /// The score of each band, percent of the analysed time.
    pub bands: [f32; BANDS],
    /// The band with the highest score.
    pub worst: usize,
    /// The pair score: `bands[worst]`.
    pub score: f32,
    /// The time ranges in which the worst band clashes, in blocks.
    pub ranges: Vec<BlockRange>,
    /// The mean level of the masker over the ranges (the whole track without ranges), dB.
    pub masker_db: [f32; BANDS],
    /// The mean level of the masked track over the same ranges, dB.
    pub masked_db: [f32; BANDS],
}

impl PairScore {
    /// The bands that count as the clash zone: those scoring at least half of the worst and at
    /// least [`MIN_SCORE`].
    pub fn zone(&self) -> [bool; BANDS] {
        let floor = (self.score * 0.5).max(MIN_SCORE);
        std::array::from_fn(|b| self.bands[b] >= floor)
    }
}

/// One cell of the pair of maps: the score of the masker leading the masked track, 0 when there
/// is no clash.
fn cell_score(masker: f32, masked: f32, active: (f32, f32)) -> f32 {
    if masker < active.0 || masked < active.1 {
        return 0.0;
    }
    let gap = masker - masked;
    if gap < MIN_GAP_DB {
        return 0.0;
    }
    let weight = ((masker - ACTIVE_DB) / 40.0).clamp(0.0, 1.0);
    (gap / FULL_GAP_DB).min(1.0) * weight
}

/// The levels below which a track has no content: the larger of [`ACTIVE_DB`] and its peak minus
/// [`ACTIVE_SPAN_DB`].
fn active_floor(map: &BandMap) -> f32 {
    ACTIVE_DB.max(map.peak() - ACTIVE_SPAN_DB)
}

/// The band scores of `masker` covering `masked` over the first `blocks` blocks, and the cells'
/// scores of every band for the time ranges.
fn direction(masker: &BandMap, masked: &BandMap, blocks: usize) -> [f32; BANDS] {
    let active = (active_floor(masker), active_floor(masked));
    let mut sums = [0.0f64; BANDS];
    for (a, b) in masker.levels.iter().zip(&masked.levels).take(blocks) {
        for band in 0..BANDS {
            sums[band] += f64::from(cell_score(a[band], b[band], active));
        }
    }
    let total = blocks.max(1) as f64;
    std::array::from_fn(|b| (sums[b] / total * 100.0) as f32)
}

/// The runs of blocks where `band` clashes.
fn clash_ranges(masker: &BandMap, masked: &BandMap, band: usize, blocks: usize) -> Vec<BlockRange> {
    let active = (active_floor(masker), active_floor(masked));
    let mut runs: Vec<BlockRange> = Vec::new();
    for (t, (a, b)) in masker
        .levels
        .iter()
        .zip(&masked.levels)
        .take(blocks)
        .enumerate()
    {
        if cell_score(a[band], b[band], active) <= 0.0 {
            continue;
        }
        match runs.last_mut() {
            Some(last) if t <= last.1 + MERGE_GAP => last.1 = t + 1,
            _ => runs.push((t, t + 1)),
        }
    }
    if runs.len() > MAX_RANGES {
        runs.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)).then(a.0.cmp(&b.0)));
        runs.truncate(MAX_RANGES);
        runs.sort_unstable();
    }
    runs
}

/// Ranks every pair of `maps` by how much one covers the other (see the module docs) and
/// returns the worst `limit`, highest score first. Maps of different lengths are compared over
/// the shorter one.
pub fn rank_pairs(maps: &[&BandMap], limit: usize) -> Vec<PairScore> {
    let mut out: Vec<(PairScore, f32)> = Vec::new();
    for i in 0..maps.len() {
        for j in i + 1..maps.len() {
            let blocks = maps[i].levels.len().min(maps[j].levels.len());
            if blocks == 0 {
                continue;
            }
            let ij = direction(maps[i], maps[j], blocks);
            let ji = direction(maps[j], maps[i], blocks);
            let total = |s: &[f32; BANDS]| s.iter().sum::<f32>();
            let (masker, masked, bands) = if total(&ij) >= total(&ji) {
                (i, j, ij)
            } else {
                (j, i, ji)
            };
            let (worst, score) = bands.iter().enumerate().fold((0, 0.0f32), |best, (b, s)| {
                if *s > best.1 { (b, *s) } else { best }
            });
            if score < MIN_SCORE {
                continue;
            }
            let ranges = clash_ranges(maps[masker], maps[masked], worst, blocks);
            let sum = total(&bands);
            out.push((
                PairScore {
                    masker,
                    masked,
                    bands,
                    worst,
                    score,
                    masker_db: maps[masker].mean(&ranges),
                    masked_db: maps[masked].mean(&ranges),
                    ranges,
                },
                sum,
            ));
        }
    }
    out.sort_by(|(a, sa), (b, sb)| {
        b.score
            .total_cmp(&a.score)
            .then(sb.total_cmp(sa))
            .then(a.masker.min(a.masked).cmp(&b.masker.min(b.masked)))
            .then(a.masker.max(a.masked).cmp(&b.masker.max(b.masked)))
    });
    out.truncate(limit);
    out.into_iter().map(|(p, _)| p).collect()
}

#[cfg(test)]
mod tests;
