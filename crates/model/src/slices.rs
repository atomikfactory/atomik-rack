//! Loop-slicer helpers: pure functions that turn slice
//! markers into zones, a beat grid into markers, and slices into a note clip.
//!
//! A **slice zone** is a zone with `key_lo == key_hi` in `SLICE_KEY_BASE..SLICE_KEY_BASE +
//! MAX_SLICES` (keys 24..=87); slice *n* (0-based) sits on key `24 + n` and covers
//! `[start, end)` of the loop asset. Zones on other keys are not slices and are ignored here.

use crate::song::Note;
use crate::zones::ZoneDoc;
use atomik_core::PPQ;
use atomik_device_sdk::{LoopMode, TriggerMode};

/// Key of slice 1. Equal to `atomik_devices::loop_slicer::SLICE_KEY_BASE` (a test checks).
pub const SLICE_KEY_BASE: u8 = 24;
/// Most slices of a loop slicer. Equal to `atomik_devices::loop_slicer::SLICE_COUNT`.
pub const MAX_SLICES: usize = 64;

/// The slice zones of `zones` (exact single key in the slice range), sorted by key (ties keep
/// list order).
fn slice_zones(zones: &[ZoneDoc]) -> Vec<&ZoneDoc> {
    let keys = SLICE_KEY_BASE..SLICE_KEY_BASE + MAX_SLICES as u8;
    let mut out: Vec<&ZoneDoc> = zones
        .iter()
        .filter(|z| z.key_lo == z.key_hi && keys.contains(&z.key_lo))
        .collect();
    out.sort_by_key(|z| z.key_lo);
    out
}

/// The slice starts of the slice zones of `zones`, in key order (slice 1 first).
pub fn markers_of(zones: &[ZoneDoc]) -> Vec<u64> {
    slice_zones(zones).iter().map(|z| z.start).collect()
}

/// The contiguous slice zones for `markers` over a loop asset of `frames` frames.
///
/// **Normalisation** of `markers`: sorted and deduplicated; markers at or beyond `frames`
/// are dropped; **frame 0 is always a marker** (the whole loop is covered, so the first slice
/// starts at 0 even when the first given marker is later); then, from the left, a marker
/// closer than `min_len` frames to the previous kept one is dropped, and a last marker closer
/// than `min_len` to `frames` is dropped (the previous slice grows). `min_len` is at least 1.
/// At most [`MAX_SLICES`] slices are kept: **extra markers are dropped from the end**. No
/// frames give no zones; a loop shorter than `min_len` is one slice.
///
/// **Slices**: `[m_i, m_(i+1))`, the last one ends at `frames`; key `SLICE_KEY_BASE + i`,
/// root key = key, all velocities, loop off, one-shot, forward by default.
///
/// **Per-slice fields** (gain, pan, transpose, tune, reverse) are kept **by slice index** from
/// the slice zones of `previous` (in key order): slice *i* takes slice *i*'s fields, so moving
/// one marker changes no other slice's fields. A slice past the previous count copies the
/// **last** previous slice's fields (defaults when there are no previous slices). Adding or
/// deleting a marker therefore shifts the fields of the slices after it by one index.
pub fn slices_from_markers(
    asset: &str,
    frames: u64,
    markers: &[u64],
    previous: &[ZoneDoc],
    min_len: u64,
) -> Vec<ZoneDoc> {
    if frames == 0 {
        return Vec::new();
    }
    let min_len = min_len.max(1);
    let mut sorted: Vec<u64> = markers.iter().copied().filter(|&m| m < frames).collect();
    sorted.push(0);
    sorted.sort_unstable();
    sorted.dedup();
    let mut kept: Vec<u64> = Vec::with_capacity(sorted.len().min(MAX_SLICES));
    for m in sorted {
        if kept.last().is_none_or(|&last| m - last >= min_len) {
            kept.push(m);
        }
    }
    while kept.len() > 1 && frames - kept[kept.len() - 1] < min_len {
        kept.pop();
    }
    kept.truncate(MAX_SLICES);
    let old = slice_zones(previous);
    kept.iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = kept.get(i + 1).copied().unwrap_or(frames);
            let key = SLICE_KEY_BASE + i as u8;
            let mut z = ZoneDoc::new(asset, frames);
            (z.key_lo, z.key_hi, z.root_key) = (key, key, key);
            (z.start, z.end) = (start, end);
            (z.loop_start, z.loop_end) = (start, end);
            z.loop_mode = LoopMode::Off;
            z.trigger = TriggerMode::OneShot;
            if let Some(p) = old.get(i).or(old.last()) {
                z.gain_db = p.gain_db;
                z.pan = p.pan;
                z.transpose = p.transpose;
                z.tune_cents = p.tune_cents;
                z.reverse = p.reverse;
            }
            z
        })
        .collect()
}

/// Even-grid markers: `floor(i × frames / (beats × divisions_per_beat))` for `i` in
/// `0..beats × divisions_per_beat`, exact integer arithmetic (`u128`). `beats` is clamped to
/// 1..=64 and `divisions_per_beat` to at least 1. When `beats × divisions_per_beat` exceeds
/// [`MAX_SLICES`], the divisions are **reduced** to the finest grid that fits
/// (`min(divisions_per_beat, 64 / beats)`, at least 1), so the grid always spans the whole
/// loop. Markers that collide on very short loops are deduplicated.
pub fn grid_markers(frames: u64, beats: u32, divisions_per_beat: u32) -> Vec<u64> {
    let beats = u64::from(beats.clamp(1, MAX_SLICES as u32));
    let div = u64::from(divisions_per_beat.max(1))
        .min(MAX_SLICES as u64 / beats)
        .max(1);
    let n = beats * div;
    let mut out: Vec<u64> = (0..n)
        .map(|i| (u128::from(i) * u128::from(frames) / u128::from(n)) as u64)
        .collect();
    out.dedup();
    out
}

/// A tick position: `round_half_up(at × beats × PPQ / frames)`, in `u128`.
fn tick_of(at: u64, frames: u64, beats: u32) -> i64 {
    let num = u128::from(at) * u128::from(beats) * PPQ as u128;
    ((2 * num + u128::from(frames)) / (2 * u128::from(frames))) as i64
}

/// The note clip that replays a loop of `frames` frames and `beats` beats: the notes
/// and the clip length in ticks (`beats × PPQ`).
///
/// One note per slice zone ([`markers_of`] order = key order), on the slice's key, at tick
/// `round_half_up(start × beats × PPQ / frames)` (`u128` intermediates; `.5` rounds up), length
/// up to the next slice's tick (the last one up to the clip length), velocity 100/127. A slice
/// whose note would have no length (its tick equals the next one's, or the clip's end) gets no
/// note. Notes are sorted by (tick, key). No frames or no beats give no notes.
pub fn to_track_notes(zones: &[ZoneDoc], frames: u64, beats: u32) -> (Vec<Note>, u64) {
    let clip_len = u64::from(beats) * PPQ as u64;
    if frames == 0 || beats == 0 {
        return (Vec::new(), clip_len);
    }
    let slices = slice_zones(zones);
    let ticks: Vec<i64> = slices
        .iter()
        .map(|z| tick_of(z.start, frames, beats))
        .collect();
    let mut notes = Vec::with_capacity(slices.len());
    for (i, z) in slices.iter().enumerate() {
        let end = ticks.get(i + 1).copied().unwrap_or(clip_len as i64);
        if end > ticks[i] {
            notes.push(Note {
                tick: ticks[i],
                length: end - ticks[i],
                key: z.key_lo,
                velocity: 100.0 / 127.0,
            });
        }
    }
    notes.sort_by_key(|n| (n.tick, n.key));
    (notes, clip_len)
}

/// The loop length in whole beats at `tempo_bpm`: `frames × tempo / (60 × sample_rate)`
/// rounded to the nearest beat (`.5` up), clamped to 1..=64. A zero rate or a tempo that is
/// not positive and finite gives 4.
pub fn default_loop_beats(frames: u64, sample_rate: u32, tempo_bpm: f64) -> u32 {
    if sample_rate == 0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
        return 4;
    }
    let beats = frames as f64 * tempo_bpm / (60.0 * f64::from(sample_rate));
    (beats + 0.5).floor().clamp(1.0, MAX_SLICES as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const A: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn fields(z: &ZoneDoc) -> (f32, f32, i8, f32, bool) {
        (z.gain_db, z.pan, z.transpose, z.tune_cents, z.reverse)
    }

    #[test]
    fn grid_is_exact_and_covers_the_loop() {
        let m = grid_markers(1000, 4, 4);
        assert_eq!(m.len(), 16);
        assert_eq!(m[0], 0);
        assert_eq!(m[1], 62); // floor(1000 / 16)
        assert_eq!(m[15], 937); // floor(15000 / 16)
        assert_eq!(grid_markers(100, 1, 1), [0]);
    }

    #[test]
    fn grid_reduces_divisions_past_64_slices() {
        assert_eq!(grid_markers(6400, 64, 4).len(), 64); // div 1
        assert_eq!(grid_markers(6400, 24, 4).len(), 48); // div 2
        assert_eq!(grid_markers(6400, 16, 4).len(), 64);
        assert_eq!(grid_markers(6400, 16, 8).len(), 64); // div 4
        assert_eq!(grid_markers(6400, 100, 4).len(), 64); // beats clamp 64
    }

    #[test]
    fn markers_are_normalised() {
        let z = slices_from_markers(A, 1000, &[500, 300, 300, 1000, 2000], &[], 1);
        assert_eq!(markers_of(&z), [0, 300, 500]);
        assert_eq!(z[2].end, 1000);
        assert_eq!((z[0].key_lo, z[0].key_hi, z[0].root_key), (24, 24, 24));
        assert_eq!(z[0].trigger, TriggerMode::OneShot);
        assert_eq!(z[0].loop_mode, LoopMode::Off);
        assert!(slices_from_markers(A, 0, &[0], &[], 1).is_empty());
    }

    #[test]
    fn min_len_drops_close_markers_and_a_short_tail() {
        let z = slices_from_markers(A, 1000, &[0, 10, 100, 995], &[], 50);
        assert_eq!(markers_of(&z), [0, 100]);
        assert_eq!(z[1].end, 1000);
        let z = slices_from_markers(A, 10, &[5], &[], 50);
        assert_eq!(markers_of(&z), [0]);
    }

    #[test]
    fn extra_markers_are_dropped_from_the_end() {
        let m: Vec<u64> = (0..100).map(|i| i * 10).collect();
        let z = slices_from_markers(A, 1000, &m, &[], 1);
        assert_eq!(z.len(), 64);
        assert_eq!(z[63].start, 630);
        assert_eq!(z[63].end, 1000);
        assert_eq!(z[63].key_lo, 87);
    }

    #[test]
    fn fields_follow_the_slice_index_and_new_slices_copy_the_last() {
        let mut prev = slices_from_markers(A, 1000, &[0, 500], &[], 1);
        prev[0].gain_db = -3.0;
        prev[1].pan = 0.5;
        prev[1].reverse = true;
        let z = slices_from_markers(A, 1000, &[0, 200, 600, 800], &prev, 1);
        assert_eq!(fields(&z[0]), fields(&prev[0]));
        assert_eq!(fields(&z[1]), fields(&prev[1]));
        assert_eq!(fields(&z[2]), fields(&prev[1]));
        assert_eq!(fields(&z[3]), fields(&prev[1]));
    }

    #[test]
    fn ticks_round_half_up() {
        // 3 frames over 1 beat: start 1 -> PPQ / 3 = 5120 exactly; use 2 beats / 3 frames
        let mut zones = slices_from_markers(A, 3, &[0, 1], &[], 1);
        let (n, len) = to_track_notes(&zones, 3, 1);
        assert_eq!(len, PPQ as u64);
        assert_eq!(n[1].tick, 5120);
        // frames 30720 = 2 PPQ, 1 beat: start 1 -> 0.5 tick -> 1
        zones = slices_from_markers(A, 30_720, &[0, 1, 30_000], &[], 1);
        let (n, _) = to_track_notes(&zones, 30_720, 1);
        assert_eq!(n[1].tick, 1);
        assert_eq!(n[0].length, 1);
    }

    #[test]
    fn notes_cover_the_clip() {
        let zones = slices_from_markers(A, 48_000, &grid_markers(48_000, 4, 2), &[], 1);
        let (n, len) = to_track_notes(&zones, 48_000, 4);
        assert_eq!(len, 4 * PPQ as u64);
        assert_eq!(n.len(), 8);
        assert_eq!(n[0].tick, 0);
        assert_eq!(n[0].key, 24);
        assert_eq!(n[0].length, PPQ / 2);
        assert_eq!(n[7].tick + n[7].length, len as i64);
        assert!((n[0].velocity * 127.0 - 100.0).abs() < 1e-3);
        for w in n.windows(2) {
            assert_eq!(w[0].tick + w[0].length, w[1].tick);
        }
    }

    #[test]
    fn default_beats_round_to_nearest() {
        // 2 s at 120 bpm = 4 beats
        assert_eq!(default_loop_beats(96_000, 48_000, 120.0), 4);
        assert_eq!(default_loop_beats(1, 48_000, 120.0), 1);
        assert_eq!(default_loop_beats(u64::MAX / 2, 48_000, 300.0), 64);
        // 2.5 beats rounds up to 3
        assert_eq!(default_loop_beats(60_000, 48_000, 120.0), 3);
        assert_eq!(default_loop_beats(10, 0, 120.0), 4);
    }

    proptest! {
        #[test]
        fn grid_slices_are_exact_contiguous_and_valid(
            frames in 4_000u64..2_000_000,
            beats in 1u32..=64,
            div in prop::sample::select(vec![1u32, 2, 4, 8]),
        ) {
            let m = grid_markers(frames, beats, div);
            let d = u64::from(div).min(64 / u64::from(beats)).max(1);
            let n = u64::from(beats) * d;
            prop_assert_eq!(m.len() as u64, n);
            prop_assert!(m.len() <= 64);
            for (i, &x) in m.iter().enumerate() {
                prop_assert_eq!(u128::from(x), i as u128 * u128::from(frames) / u128::from(n));
            }
            let z = slices_from_markers(A, frames, &m, &[], 1);
            prop_assert_eq!(z.len(), m.len());
            prop_assert_eq!(z[0].start, 0);
            prop_assert_eq!(z[z.len() - 1].end, frames);
            for w in z.windows(2) {
                prop_assert_eq!(w[0].end, w[1].start);
            }
            let min = frames / n;
            prop_assert!(z.iter().all(|s| s.end - s.start >= min));
            let sdk: Vec<_> = z.iter().map(|s| s.to_zone(0)).collect();
            let buf = std::sync::Arc::new(atomik_device_sdk::SampleBuffer {
                data: std::sync::Arc::new([Vec::new()]),
                frames,
                channels: 1,
            });
            prop_assert!(atomik_device_sdk::SampleMap::new(vec![buf], sdk).is_ok());
        }

        #[test]
        fn arbitrary_markers_give_valid_slices(
            frames in 1u64..100_000,
            markers in prop::collection::vec(0u64..200_000, 0..100),
            min_len in 1u64..500,
        ) {
            let z = slices_from_markers(A, frames, &markers, &[], min_len);
            prop_assert!(!z.is_empty() && z.len() <= 64);
            prop_assert_eq!(z[0].start, 0);
            prop_assert_eq!(z[z.len() - 1].end, frames);
            for w in z.windows(2) {
                prop_assert_eq!(w[0].end, w[1].start);
            }
            if frames >= min_len {
                prop_assert!(z.iter().all(|s| s.end - s.start >= min_len));
            }
            prop_assert_eq!(markers_of(&z).len(), z.len());
        }

        #[test]
        fn moving_a_marker_keeps_every_slices_fields(
            frames in 10_000u64..100_000,
            count in 2usize..20,
            which in 1usize..20,
            to in 0u64..100_000,
        ) {
            let m: Vec<u64> = (0..count as u64).map(|i| i * frames / count as u64).collect();
            let mut prev = slices_from_markers(A, frames, &m, &[], 1);
            for (i, z) in prev.iter_mut().enumerate() {
                z.gain_db = -(i as f32);
                z.pan = (i as f32) / 40.0;
                z.transpose = i as i8;
                z.tune_cents = i as f32;
                z.reverse = i % 2 == 1;
            }
            let which = 1 + (which - 1) % (count - 1);
            let mut moved = markers_of(&prev);
            moved[which] = to % frames;
            let next = slices_from_markers(A, frames, &moved, &prev, 1);
            // Same count unless the marker collided with another or fell off the end.
            for (i, z) in next.iter().enumerate().take(prev.len()) {
                prop_assert_eq!(fields(z), fields(&prev[i]));
            }
        }

        #[test]
        fn track_notes_round_trip_the_slice_positions(
            frames in 1_000u64..5_000_000,
            beats in 1u32..=64,
            div in prop::sample::select(vec![1u32, 2, 4]),
        ) {
            let z = slices_from_markers(A, frames, &grid_markers(frames, beats, div), &[], 1);
            let (n, len) = to_track_notes(&z, frames, beats);
            prop_assert_eq!(len, u64::from(beats) * PPQ as u64);
            prop_assert!(n.windows(2).all(|w| (w[0].tick, w[0].key) < (w[1].tick, w[1].key)));
            for (note, zone) in n.iter().zip(&z) {
                let exact = zone.start as f64 * f64::from(beats) * PPQ as f64 / frames as f64;
                prop_assert!((note.tick as f64 - exact).abs() <= 0.5 + 1e-6);
                prop_assert!(note.length > 0);
            }
        }
    }
}
