//! Audio clips while the player follows an external clock.
//!
//! The regions of an audio track are placed in frames of the song's *own* tempo map, which the
//! followed clock replaces with its own constant tempo, so a region cannot be read by the
//! position of the player. Each region is played like a note instead: it starts when the song
//! reaches its first frame and then runs at the clip's own rate to its end (a warped clip is its
//! finished, already stretched buffer, so it keeps the stretch of the song's own tempo; an
//! interim warp plays through its time map as it always does).
//!
//! The song's own timeline is the common ground. For a stretch of the player's frames
//! `[a, b)` the caller gives its ends on that timeline, `u0` and `u1` (the tick of the frame
//! through the followed tempo, back to a frame through the song's own map), and the renderer
//! does the rest:
//!
//! * a region whose first frame lies in `[last, u1)` starts at the player's frame where `u`
//!   crosses it (`last` is where the previous stretch ended: a position nudged forward by the
//!   phase lock cannot step over a start);
//! * on a *re-seek* (a locate, the transport starting, a loop wrap, a new song) the regions that
//!   are already underway at `u0` start from the frame `u0` is inside, so a song started or
//!   located into the middle of a clip hears it from there;
//! * a region plays `frames` frames and ends, whatever the tempo does meanwhile, so a clip is
//!   re-synchronised with the song at every locate, loop wrap and region start, not within a
//!   long clip: a take far longer than a bar drifts by the share the followed tempo differs
//!   from the song's own (warp it, or give the song the sender's tempo, for a tight lock).
//!
//! At most [`MAX_ACTIVE`] regions of a track play at once (more than the two a track may
//! overlap by on its own timeline, for clips that run on past their neighbours' start). Streamed
//! assets are silent while following: the streamer reads in the song's own order. The renderer
//! allocates nothing: the state is a small array in the track's RT scratch.

use crate::audio::{AudioRt, CompiledAudio, LaneData, render_region};
use atomik_core::{BATCH, Batch};

/// Regions of one track that play at once while following.
pub(crate) const MAX_ACTIVE: usize = 4;

/// The regions playing on a track and where the last stretch ended.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FollowRt {
    /// `(region index, frames already played)`.
    active: [Option<(u32, u64)>; MAX_ACTIVE],
    /// The song-timeline frame the previous stretch ended at.
    last_u: f64,
}

/// Renders the stretch of `n` player frames at batch offset `off` whose ends on the song's own
/// timeline are `u0` and `u1` into `rt.pb` (which holds zeros or earlier output: the regions
/// add). `reseek` rebuilds which regions are underway (see the module docs).
pub(crate) fn render_segment(
    track: &CompiledAudio,
    rt: &mut AudioRt,
    (u0, u1): (f64, f64),
    n: usize,
    off: usize,
    reseek: bool,
) {
    let data = &*track.data;
    let AudioRt { pb, follow, .. } = rt;
    let (u0, u1) = (u0.max(0.0), u1.max(0.0));
    if reseek {
        follow.active = [None; MAX_ACTIVE];
        follow.last_u = u0;
        let first = data.regions.partition_point(|r| r.end() as f64 <= u0);
        for (idx, r) in data.regions.iter().enumerate().skip(first) {
            if r.song_start_frame as f64 >= u0 {
                break;
            }
            let played = (u0 - r.song_start_frame as f64).floor() as u64;
            start(follow, idx, played);
        }
    }
    // Regions already playing run on from the start of the stretch.
    for slot in &mut follow.active {
        if let Some((idx, played)) = slot {
            let r = &data.regions[*idx as usize];
            *played += play(data, *idx as usize, *played, 0, n, off, pb);
            if *played >= r.frames {
                *slot = None;
            }
        }
    }
    // Regions whose start the song reached since the last stretch begin where it was crossed.
    let lo = follow.last_u;
    let first = data
        .regions
        .partition_point(|r| (r.song_start_frame as f64) < lo);
    for (idx, r) in data.regions.iter().enumerate().skip(first) {
        let start_at = r.song_start_frame as f64;
        if start_at >= u1 {
            break;
        }
        let k = if u1 > u0 && start_at > u0 {
            (((start_at - u0) / (u1 - u0) * n as f64).ceil() as usize).min(n.saturating_sub(1))
        } else {
            0
        };
        let played = play(data, idx, 0, k, n, off, pb);
        if played < r.frames {
            start(follow, idx, played);
        }
    }
    follow.last_u = follow.last_u.max(u1);
}

/// Adds region `idx` to the first free slot (when none is free the region is not played).
fn start(follow: &mut FollowRt, idx: usize, played: u64) {
    if let Some(slot) = follow.active.iter_mut().find(|s| s.is_none()) {
        *slot = Some((idx as u32, played));
    }
}

/// Plays region `idx` from its frame `played` into frames `k..n` of the stretch; returns the
/// frames played (the region's end cuts it).
fn play(
    data: &LaneData,
    idx: usize,
    played: u64,
    k: usize,
    n: usize,
    off: usize,
    pb: &mut [Batch; 2],
) -> u64 {
    let r = &data.regions[idx];
    let frames = ((n - k) as u64).min(r.frames.saturating_sub(played)) as usize;
    if frames > 0 && frames <= BATCH {
        render_region(data, r, pb, played, frames, off + k, frames);
    }
    frames as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AssetId, SongAsset, SongAudioRegion, SongAudioTrack, compile_audio};
    use crate::graph::DeviceInstanceId;

    /// A distinct, nonzero sample for asset frame `i`.
    fn sample(i: usize) -> f32 {
        (i as f32 + 1.0) * 1.0e-4
    }

    fn track(regions: Vec<SongAudioRegion>) -> CompiledAudio {
        let data: Vec<Vec<f32>> = (0..2)
            .map(|_| (0..4000).map(sample).collect::<Vec<f32>>())
            .collect();
        let asset = SongAsset::memory(data.into());
        let track = SongAudioTrack {
            device: DeviceInstanceId(1),
            regions,
            envelopes: Vec::new(),
            warp_maps: Vec::new(),
        };
        compile_audio(&[track], &[asset]).unwrap().remove(0)
    }

    fn region(start: u64, offset: u64, frames: u64) -> SongAudioRegion {
        SongAudioRegion::plain(AssetId(0), start, offset, frames)
    }

    /// Renders stretches of 64 frames whose ends on the song's own timeline come from `ends`
    /// (`u0, u1` per stretch) and returns the left output; `reseek_first` re-seeks the first.
    fn play_through(t: &CompiledAudio, ends: &[(f64, f64)], reseek_first: bool) -> Vec<f32> {
        let mut rt = AudioRt::new();
        let mut out = Vec::new();
        for (k, u) in ends.iter().enumerate() {
            rt.pb = [[0.0; BATCH]; 2];
            render_segment(t, &mut rt, *u, BATCH, 0, reseek_first && k == 0);
            out.extend_from_slice(&rt.pb[0]);
        }
        out
    }

    /// A steady song-timeline pace of `per_batch` frames a batch from `from`.
    fn steady(from: f64, per_batch: f64, batches: usize) -> Vec<(f64, f64)> {
        (0..batches)
            .map(|k| {
                let u0 = from + k as f64 * per_batch;
                (u0, u0 + per_batch)
            })
            .collect()
    }

    #[test]
    fn a_region_starts_where_the_song_reaches_it_and_plays_at_its_own_rate() {
        let t = track(vec![region(1000, 0, 300)]);
        // The followed tempo is twice as slow as the song's own: 32 timeline frames a batch.
        let out = play_through(&t, &steady(0.0, 32.0, 100), true);
        // The song reaches frame 1000 after 2000 player frames; from there the asset plays 1:1.
        let first = out.iter().position(|v| *v != 0.0).unwrap();
        assert_eq!(first, 2000);
        for i in 0..300 {
            assert_eq!(out[first + i], sample(i), "frame {i}");
        }
        assert!(out[first + 300..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_pace_of_the_song_does_not_change_the_pace_of_a_clip() {
        let t = track(vec![region(64, 10, 500)]);
        let slow = play_through(&t, &steady(0.0, 10.0, 20), true);
        let fast = play_through(&t, &steady(0.0, 100.0, 20), true);
        // Both play the same 500 asset frames; only the start differs (6.4 and 0.64 batches in).
        let slow_at = slow.iter().position(|v| *v != 0.0).unwrap();
        let fast_at = fast.iter().position(|v| *v != 0.0).unwrap();
        assert_eq!((slow_at, fast_at), (410, 41));
        for i in 0..500 {
            assert_eq!(slow[slow_at + i], sample(10 + i));
            assert_eq!(fast[fast_at + i], sample(10 + i));
        }
    }

    #[test]
    fn a_re_seek_into_a_clip_plays_it_from_there() {
        let t = track(vec![region(1000, 0, 300)]);
        let out = play_through(&t, &steady(1100.0, 64.0, 5), true);
        for (i, v) in out.iter().take(200).enumerate() {
            assert_eq!(*v, sample(100 + i), "frame {i}");
        }
        assert!(out[200..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn without_a_re_seek_a_clip_underway_is_not_started_again() {
        let t = track(vec![region(100, 0, 300)]);
        let mut ends = steady(0.0, 64.0, 3);
        // A stretch that begins beyond where the last one ended (a position nudged forward).
        ends.extend(steady(260.0, 64.0, 4));
        let out = play_through(&t, &ends, true);
        let first = out.iter().position(|v| *v != 0.0).unwrap();
        assert_eq!(first, 100);
        // The start fell inside the stretch 64..128: played once, from its first frame.
        for i in 0..300 {
            assert_eq!(out[first + i], sample(i), "frame {i}");
        }
    }

    #[test]
    fn a_start_inside_a_skipped_gap_still_plays() {
        let t = track(vec![region(150, 0, 100)]);
        // The first stretch ends at 64; the next begins at 200: the clip started in between.
        let ends = [(0.0, 64.0), (200.0, 264.0), (264.0, 328.0)];
        let out = play_through(&t, &ends, true);
        assert_eq!(out[64], sample(0));
        assert_eq!(out[64 + 63], sample(63));
        assert_eq!(out[128], sample(64));
        assert_eq!(out[128 + 35], sample(99));
        assert_eq!(out[128 + 36], 0.0);
    }

    #[test]
    fn overlapping_clips_mix_and_a_full_track_drops_the_extra() {
        let t = track(vec![region(0, 0, 200), region(64, 0, 200)]);
        let out = play_through(&t, &steady(0.0, 64.0, 5), true);
        assert_eq!(out[10], sample(10));
        assert_eq!(out[100], sample(100) + sample(100 - 64));
        assert_eq!(out[250], sample(250 - 64));
        // Eight clips that all start inside one stretch of a fast song: the first four go on
        // playing, the rest are left out after that stretch (no crash, no allocation).
        let many: Vec<_> = (0..8).map(|k| region(k * 100, 0, 150)).collect();
        let t = track(many);
        let out = play_through(&t, &steady(0.0, 1000.0, 2), true);
        let at = 64 + 10;
        let starts = [0usize, 7, 13, 20];
        let want: f32 = starts.iter().map(|k| sample(at - k)).sum();
        assert!((out[at] - want).abs() < 1.0e-6, "{} vs {want}", out[at]);
        assert_eq!(MAX_ACTIVE, 4);
    }

    #[test]
    fn identical_inputs_render_identical_audio() {
        let t = track(vec![region(70, 3, 400), region(300, 0, 250)]);
        let ends = steady(0.0, 37.5, 40);
        let a = play_through(&t, &ends, true);
        let b = play_through(&t, &ends, true);
        assert_eq!(a, b);
        assert!(a.iter().any(|v| *v != 0.0));
    }
}
