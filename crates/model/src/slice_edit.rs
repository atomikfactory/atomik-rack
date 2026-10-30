//! Marker editing for the audio clip editor's Slice mode: pure functions from the warp settings
//! of a clip to a new marker list.
//!
//! A warp marker pins a source frame to a tick of the clip (see [`crate::warp`]). Between two
//! markers the audio is stretched evenly, before the first and after the last it plays at its
//! natural speed. The editor works on the *framed* list ([`framed`]): the clip's markers with one
//! more at the clip start (tick 0) and one at the clip end (tick `length`), so the two ends stay
//! where they are whatever happens to the markers between them. Every function here takes the
//! warp settings, the document sample rate and the clip length in ticks, and returns the whole
//! new framed list, or says why nothing can be done ([`SliceError`]). The `index` of an operation
//! counts in the framed list; the first and the last marker are the pinned ends and cannot be
//! moved or deleted.
//!
//! Every result is strictly increasing in tick and in source frame, starts at tick 0, ends at
//! tick `length` and keeps the two ends (tick and source) of the list it started from. That is
//! what [`crate::SongCommand::SetWarpMarkers`] accepts.
//!
//! | Operation | Moves | Sound |
//! |---|---|---|
//! | [`add_at_tick`], [`add_at_source`] | nothing: the marker is pinned at the position the clip plays that frame at (rounded to a source frame) | unchanged |
//! | [`move_stretch`] | the marker in time, its audio frame with it | the audio on both sides stretches |
//! | [`move_decoupled`] | the marker along the audio: tick and source move together on the current time map | the audio around it keeps its place |
//! | [`move_group`] | the marker in time and every marker after it (or before it) in proportion, as an accordion | the audio between them stretches evenly |
//! | [`nudge`] | the marker by a number of ticks (a stretch move) | as `move_stretch` |
//! | [`delete`] | removes a marker | the audio on both sides stretches evenly between the neighbours |
//! | [`revert`] | removes every marker | the clip plays at its natural speed |

use crate::warp::{MAX_WARP_MARKERS, WarpMarker, WarpSettings};

/// Why a marker edit could not be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SliceError {
    /// No marker has that index.
    #[error("there is no marker {0}")]
    NoMarker(usize),
    /// The first and the last marker are the clip's ends.
    #[error("the ends of the clip cannot be moved or deleted")]
    Pinned,
    /// A marker is already there, or the position is outside the clip.
    #[error("there is no room for a marker at that position")]
    NoRoom,
    /// The clip holds the most markers.
    #[error("a clip holds at most {MAX_WARP_MARKERS} markers")]
    TooMany,
    /// The clip is empty.
    #[error("the clip has no length")]
    Empty,
}

/// Which markers a group move carries along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The markers before the moved one, back to the clip start.
    Before,
    /// The markers after the moved one, up to the clip end.
    After,
}

/// The markers of `warp` with the clip's ends added: a marker at tick 0 (at the source frame the
/// clip plays there, at least 0) and one at tick `length`, unless the list has them. The map the
/// framed list describes is the one `warp` describes, to a rounding of the added frames.
pub fn framed(warp: &WarpSettings, rate: u32, length: i64) -> Vec<WarpMarker> {
    let mut out: Vec<WarpMarker> = warp
        .markers
        .iter()
        .copied()
        .filter(|m| m.tick <= length)
        .collect();
    if out.first().is_none_or(|m| m.tick > 0) {
        let src = warp.source_frames_at(rate, 0.0).round().max(0.0) as u64;
        // A first marker whose frame is not above the start frame cannot happen for a valid list
        // (the start frame is below it by the natural speed), but the list stays valid anyway.
        if out.first().is_none_or(|m| m.source > src) {
            out.insert(
                0,
                WarpMarker {
                    tick: 0,
                    source: src,
                },
            );
        }
    }
    if out.last().is_none_or(|m| m.tick < length) {
        let src = warp.source_frames_at(rate, length as f64).round().max(0.0) as u64;
        if out.last().is_none_or(|m| m.source < src) {
            out.push(WarpMarker {
                tick: length,
                source: src,
            });
        }
    }
    out
}

/// The ticks, strictly inside the clip, of the markers of `warp`: where Split at Markers cuts.
pub fn split_ticks(warp: &WarpSettings, length: i64) -> Vec<i64> {
    warp.markers
        .iter()
        .map(|m| m.tick)
        .filter(|t| *t > 0 && *t < length)
        .collect()
}

/// The list that removes every marker: the clip plays at its natural speed.
pub fn revert() -> Vec<WarpMarker> {
    Vec::new()
}

/// Whether `markers` equal the natural map of `warp` (every marker on the line the clip plays
/// without markers, to half a frame): such a clip sounds as if it had none.
pub fn is_natural(markers: &[WarpMarker], warp: &WarpSettings, rate: u32) -> bool {
    let bare = WarpSettings {
        markers: Vec::new(),
        ..warp.clone()
    };
    markers
        .iter()
        .all(|m| (bare.source_frames_at(rate, m.tick as f64) - m.source as f64).abs() <= 0.5)
}

fn check(length: i64) -> Result<(), SliceError> {
    if length < 2 {
        return Err(SliceError::Empty);
    }
    Ok(())
}

/// The marker at `index` and its neighbours, for an interior marker.
fn interior(list: &[WarpMarker], index: usize) -> Result<(WarpMarker, WarpMarker), SliceError> {
    if index >= list.len() {
        return Err(SliceError::NoMarker(index));
    }
    if index == 0 || index + 1 == list.len() {
        return Err(SliceError::Pinned);
    }
    Ok((list[index - 1], list[index + 1]))
}

/// Adds a marker at `tick` (strictly inside the clip, not on a marker), pinned at the source frame
/// the clip plays there now. The new marker is the one at the returned index.
pub fn add_at_tick(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    tick: i64,
) -> Result<(Vec<WarpMarker>, usize), SliceError> {
    check(length)?;
    let mut list = framed(warp, rate, length);
    if tick <= 0 || tick >= length {
        return Err(SliceError::NoRoom);
    }
    if list.len() >= MAX_WARP_MARKERS {
        return Err(SliceError::TooMany);
    }
    let at = list.partition_point(|m| m.tick < tick);
    if list.get(at).is_some_and(|m| m.tick == tick) {
        return Err(SliceError::NoRoom);
    }
    let source = warp.source_frames_at(rate, tick as f64).round().max(0.0) as u64;
    // Strictly between the neighbours (the list starts at tick 0 and ends at `length`).
    let before = at.checked_sub(1).and_then(|i| list.get(i));
    let (Some(before), Some(after)) = (before, list.get(at)) else {
        return Err(SliceError::NoRoom);
    };
    if source <= before.source || source >= after.source {
        return Err(SliceError::NoRoom);
    }
    list.insert(at, WarpMarker { tick, source });
    Ok((list, at))
}

/// Adds a marker at the source frame `source` (from the clip's source offset): at the tick the
/// clip plays it, which must lie strictly inside the clip. The marker is where the audio is, so
/// the sound does not change; it is the way to put a marker on a detected transient.
pub fn add_at_source(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    source: u64,
) -> Result<(Vec<WarpMarker>, usize), SliceError> {
    let tick = warp.ticks_at_source(rate, source as f64).round();
    if !tick.is_finite() || tick <= 0.0 || tick >= length as f64 {
        return Err(SliceError::NoRoom);
    }
    let (mut list, at) = add_at_tick(warp, rate, length, tick as i64)?;
    // Pin the marker exactly on the requested frame when that keeps the order.
    let (lo, hi) = (list[at - 1].source, list[at + 1].source);
    if source > lo && source < hi {
        list[at].source = source;
    }
    Ok((list, at))
}

/// Removes the marker at `index`.
pub fn delete(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    index: usize,
) -> Result<Vec<WarpMarker>, SliceError> {
    check(length)?;
    let mut list = framed(warp, rate, length);
    interior(&list, index)?;
    list.remove(index);
    Ok(list)
}

/// Moves the marker at `index` to `tick` (clamped between its neighbours, one tick away from
/// each), its source frame fixed: the audio on both sides stretches.
pub fn move_stretch(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    index: usize,
    tick: i64,
) -> Result<Vec<WarpMarker>, SliceError> {
    check(length)?;
    let mut list = framed(warp, rate, length);
    let (prev, next) = interior(&list, index)?;
    let (lo, hi) = (prev.tick + 1, next.tick - 1);
    if lo > hi {
        return Err(SliceError::NoRoom);
    }
    list[index].tick = tick.clamp(lo, hi);
    Ok(list)
}

/// Moves the marker at `index` along the audio to `tick` (clamped as [`move_stretch`]): its
/// source frame becomes the one the clip plays at `tick` now, so the audio under the marker keeps
/// its place and only the corner of the time map moves.
pub fn move_decoupled(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    index: usize,
    tick: i64,
) -> Result<Vec<WarpMarker>, SliceError> {
    check(length)?;
    let mut list = framed(warp, rate, length);
    let (prev, next) = interior(&list, index)?;
    let (lo, hi) = (prev.tick + 1, next.tick - 1);
    let (slo, shi) = (prev.source + 1, next.source.saturating_sub(1));
    if lo > hi || slo > shi {
        return Err(SliceError::NoRoom);
    }
    let tick = tick.clamp(lo, hi);
    let source = warp.source_frames_at(rate, tick as f64).round().max(0.0) as u64;
    list[index] = WarpMarker {
        tick,
        source: source.clamp(slo, shi),
    };
    Ok(list)
}

/// Moves the marker at `index` to `tick` and the markers on `side` of it with it, in proportion
/// (an accordion between the moved marker and the clip end, or the clip start). Source frames do
/// not change, so the audio between the markers stretches evenly. `tick` is clamped so every
/// marker keeps a tick of room.
pub fn move_group(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    index: usize,
    tick: i64,
    side: Side,
) -> Result<Vec<WarpMarker>, SliceError> {
    check(length)?;
    let mut list = framed(warp, rate, length);
    let (prev, next) = interior(&list, index)?;
    let n = list.len();
    let old = list[index].tick;
    let (lo, hi) = match side {
        Side::After => (prev.tick + 1, length - (n - index - 1) as i64),
        Side::Before => (index as i64, next.tick - 1),
    };
    if lo > hi {
        return Err(SliceError::NoRoom);
    }
    let new = tick.clamp(lo, hi);
    let range = match side {
        Side::After => index + 1..n - 1,
        Side::Before => 1..index,
    };
    for k in range.clone() {
        let t = list[k].tick as f64;
        let scaled = match side {
            Side::After => {
                new as f64 + (t - old as f64) * (length - new) as f64 / (length - old) as f64
            }
            Side::Before => t * new as f64 / old as f64,
        };
        list[k].tick = scaled.round() as i64;
    }
    list[index].tick = new;
    // Rounding can make two ticks meet: spread them out from the moved marker, then from the
    // pinned end, so the order holds.
    for k in 1..n - 1 {
        if list[k].tick <= list[k - 1].tick {
            list[k].tick = list[k - 1].tick + 1;
        }
    }
    for k in (1..n - 1).rev() {
        if list[k].tick >= list[k + 1].tick {
            list[k].tick = list[k + 1].tick - 1;
        }
    }
    if list.windows(2).any(|w| w[0].tick >= w[1].tick) || list[0].tick != 0 {
        return Err(SliceError::NoRoom);
    }
    Ok(list)
}

/// Moves the marker at `index` by `delta` ticks (a stretch move, clamped between its neighbours).
pub fn nudge(
    warp: &WarpSettings,
    rate: u32,
    length: i64,
    index: usize,
    delta: i64,
) -> Result<Vec<WarpMarker>, SliceError> {
    let list = framed(warp, rate, length);
    let tick = list.get(index).ok_or(SliceError::NoMarker(index))?.tick;
    move_stretch(warp, rate, length, index, tick.saturating_add(delta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RATE: u32 = 48_000;
    // At 93.75 BPM the natural speed is 2 source frames per tick.

    fn warp(markers: Vec<WarpMarker>) -> WarpSettings {
        WarpSettings {
            enabled: true,
            source_bpm: 93.75,
            markers,
            ..WarpSettings::default()
        }
    }

    fn m(tick: i64, source: u64) -> WarpMarker {
        WarpMarker { tick, source }
    }

    /// A framed list over `length` ticks from gap lists (every gap at least 1).
    fn list_of(first_source: u64, gaps: &[(i64, u64)]) -> (Vec<WarpMarker>, i64) {
        let mut out = vec![m(0, first_source)];
        for (dt, ds) in gaps {
            let last = out[out.len() - 1];
            out.push(m(last.tick + dt, last.source + ds));
        }
        let length = out[out.len() - 1].tick;
        (out, length)
    }

    fn valid(list: &[WarpMarker], length: i64) -> bool {
        warp(list.to_vec()).validate().is_ok()
            && list.first().is_some_and(|f| f.tick == 0)
            && list.last().is_some_and(|l| l.tick == length)
    }

    #[test]
    fn framing_adds_the_ends_at_the_natural_frames() {
        // 93.75 BPM at 48 kHz: 2 frames per tick.
        let w = warp(vec![m(100, 200)]);
        assert_eq!(
            framed(&w, RATE, 400),
            vec![m(0, 0), m(100, 200), m(400, 800)]
        );
        assert_eq!(
            framed(&warp(Vec::new()), RATE, 400),
            vec![m(0, 0), m(400, 800)]
        );
        // A list that has the ends is left alone.
        let ends = vec![m(0, 10), m(400, 9000)];
        assert_eq!(framed(&warp(ends.clone()), RATE, 400), ends);
    }

    #[test]
    fn add_pins_the_marker_where_the_audio_plays_and_changes_no_sound() {
        let w = warp(vec![m(0, 0), m(200, 1000), m(400, 10_000)]);
        let (list, at) = add_at_tick(&w, RATE, 400, 100).unwrap();
        assert_eq!(list[at], m(100, 500));
        let after = warp(list);
        for t in [0.0, 50.0, 100.0, 150.0, 250.0, 399.0] {
            let d = (w.source_frames_at(RATE, t) - after.source_frames_at(RATE, t)).abs();
            assert!(d < 1e-9, "tick {t}");
        }
        assert_eq!(add_at_tick(&w, RATE, 400, 200), Err(SliceError::NoRoom));
        assert_eq!(add_at_tick(&w, RATE, 400, 0), Err(SliceError::NoRoom));
        assert_eq!(add_at_tick(&w, RATE, 400, 400), Err(SliceError::NoRoom));
    }

    #[test]
    fn add_at_a_source_frame_puts_the_marker_on_it() {
        let w = warp(vec![m(0, 0), m(400, 10_000)]);
        let (list, at) = add_at_source(&w, RATE, 400, 2510).unwrap();
        assert_eq!(list[at].source, 2510);
        assert_eq!(list[at].tick, 100);
        assert_eq!(
            add_at_source(&w, RATE, 400, 20_000),
            Err(SliceError::NoRoom)
        );
    }

    #[test]
    fn stretch_moves_the_tick_and_keeps_the_frame() {
        let w = warp(vec![m(0, 0), m(100, 2500), m(300, 7500), m(400, 10_000)]);
        let l = move_stretch(&w, RATE, 400, 1, 150).unwrap();
        assert_eq!(l[1], m(150, 2500));
        // Clamped one tick away from the neighbours.
        let l = move_stretch(&w, RATE, 400, 1, 900).unwrap();
        assert_eq!(l[1].tick, 299);
        let l = move_stretch(&w, RATE, 400, 1, -5).unwrap();
        assert_eq!(l[1].tick, 1);
        assert_eq!(move_stretch(&w, RATE, 400, 0, 5), Err(SliceError::Pinned));
        assert_eq!(move_stretch(&w, RATE, 400, 3, 5), Err(SliceError::Pinned));
        assert_eq!(
            move_stretch(&w, RATE, 400, 9, 5),
            Err(SliceError::NoMarker(9))
        );
    }

    #[test]
    fn decoupled_slides_the_marker_along_the_audio() {
        let w = warp(vec![m(0, 0), m(100, 2500), m(400, 10_000)]);
        let l = move_decoupled(&w, RATE, 400, 1, 140).unwrap();
        assert_eq!(l[1], m(140, 3500));
        // The audio at tick 140 is the audio that was there.
        let after = warp(l);
        assert!(
            (after.source_frames_at(RATE, 140.0) - w.source_frames_at(RATE, 140.0)).abs() < 1e-9
        );
    }

    #[test]
    fn group_move_is_an_accordion_on_one_side() {
        let w = warp(vec![
            m(0, 0),
            m(100, 2500),
            m(200, 5000),
            m(300, 7500),
            m(400, 10_000),
        ]);
        let l = move_group(&w, RATE, 400, 1, 200, Side::After).unwrap();
        // Markers after it keep their relative place between the moved marker and the end.
        assert_eq!(
            l.iter().map(|x| x.tick).collect::<Vec<_>>(),
            vec![0, 200, 267, 333, 400]
        );
        assert_eq!(
            l.iter().map(|x| x.source).collect::<Vec<_>>(),
            vec![0, 2500, 5000, 7500, 10_000]
        );
        let l = move_group(&w, RATE, 400, 3, 150, Side::Before).unwrap();
        assert_eq!(
            l.iter().map(|x| x.tick).collect::<Vec<_>>(),
            vec![0, 50, 100, 150, 400]
        );
    }

    #[test]
    fn delete_and_revert() {
        let w = warp(vec![m(0, 0), m(100, 2600), m(400, 10_000)]);
        assert_eq!(
            delete(&w, RATE, 400, 1).unwrap(),
            vec![m(0, 0), m(400, 10_000)]
        );
        assert_eq!(delete(&w, RATE, 400, 0), Err(SliceError::Pinned));
        assert!(revert().is_empty());
    }

    #[test]
    fn nudge_is_a_clamped_stretch() {
        let w = warp(vec![m(0, 0), m(100, 2500), m(400, 10_000)]);
        assert_eq!(nudge(&w, RATE, 400, 1, -10).unwrap()[1].tick, 90);
        assert_eq!(nudge(&w, RATE, 400, 1, 1000).unwrap()[1].tick, 399);
    }

    #[test]
    fn natural_markers_are_recognised() {
        let w = warp(Vec::new());
        assert!(is_natural(&[m(0, 0), m(100, 200), m(400, 800)], &w, RATE));
        assert!(!is_natural(&[m(0, 0), m(100, 260)], &w, RATE));
    }

    #[test]
    fn the_marker_count_is_limited() {
        let gaps: Vec<(i64, u64)> = (0..MAX_WARP_MARKERS - 1).map(|_| (2, 2)).collect();
        let (list, length) = list_of(0, &gaps);
        assert_eq!(list.len(), MAX_WARP_MARKERS);
        let w = warp(list);
        // Room in time and source, but no room in the list.
        assert_eq!(add_at_tick(&w, RATE, length, 1), Err(SliceError::TooMany));
    }

    fn gaps() -> impl Strategy<Value = (u64, Vec<(i64, u64)>)> {
        (
            0u64..2000,
            prop::collection::vec((1i64..400, 1u64..20_000), 1..40),
        )
    }

    proptest! {
        #[test]
        fn operations_keep_the_list_valid_and_the_ends_pinned(
            (first, g) in gaps(),
            index in 0usize..45,
            tick in -50i64..20_000,
            at in -50i64..20_000,
            source in 0u64..1_000_000,
            after in any::<bool>(),
        ) {
            let (list, length) = list_of(first, &g);
            let w = warp(list.clone());
            let ends = (list[0], list[list.len() - 1]);
            let side = if after { Side::After } else { Side::Before };
            let results = [
                move_stretch(&w, RATE, length, index, tick).ok(),
                move_decoupled(&w, RATE, length, index, tick).ok(),
                move_group(&w, RATE, length, index, tick, side).ok(),
                nudge(&w, RATE, length, index, tick).ok(),
                delete(&w, RATE, length, index).ok(),
                add_at_tick(&w, RATE, length, at).ok().map(|(l, _)| l),
                add_at_source(&w, RATE, length, source).ok().map(|(l, _)| l),
            ];
            for out in results.into_iter().flatten() {
                prop_assert!(valid(&out, length), "{out:?}");
                prop_assert_eq!(out[0], ends.0);
                prop_assert_eq!(out[out.len() - 1], ends.1);
                prop_assert!(out.len() <= MAX_WARP_MARKERS);
            }
        }

        #[test]
        fn an_added_marker_changes_the_time_map_by_at_most_half_a_frame(
            (first, g) in gaps(),
            at in 1i64..20_000,
        ) {
            let (list, length) = list_of(first, &g);
            let w = warp(list);
            if let Ok((out, _)) = add_at_tick(&w, RATE, length, at) {
                let after = warp(out);
                for t in 0..=length.min(500) {
                    let t = t as f64 * length as f64 / length.min(500) as f64;
                    let d = (w.source_frames_at(RATE, t) - after.source_frames_at(RATE, t)).abs();
                    prop_assert!(d <= 0.5 + 1e-6, "tick {t}: {d}");
                }
            }
        }

        #[test]
        fn a_stretch_never_changes_the_frames_of_the_markers(
            (first, g) in gaps(),
            index in 1usize..40,
            tick in -50i64..20_000,
        ) {
            let (list, length) = list_of(first, &g);
            let w = warp(list.clone());
            if let Ok(out) = move_stretch(&w, RATE, length, index, tick) {
                let a: Vec<u64> = list.iter().map(|x| x.source).collect();
                let b: Vec<u64> = out.iter().map(|x| x.source).collect();
                prop_assert_eq!(a, b);
            }
        }

        #[test]
        fn framing_any_valid_markers_is_valid(
            (first, g) in gaps(),
            drop_ends in any::<bool>(),
            length_extra in 0i64..300,
        ) {
            let (list, length) = list_of(first, &g);
            let inner: Vec<WarpMarker> = if drop_ends && list.len() > 2 {
                list[1..list.len() - 1].to_vec()
            } else {
                list
            };
            let w = warp(inner);
            let framed = framed(&w, RATE, length + length_extra);
            prop_assert!(warp(framed.clone()).validate().is_ok(), "{framed:?}");
        }
    }
}
