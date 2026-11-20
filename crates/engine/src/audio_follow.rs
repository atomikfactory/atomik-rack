//! Audio clips while the player follows an external clock.
//!
//! The regions of an audio track are placed in frames of the song's *own* tempo map, which the
//! followed clock replaces with its own constant tempo, so a region cannot be read by the
//! position of the player. Each region is played like a note instead: it starts when the song
//! reaches its first frame and then runs to its end.
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
//!   located into the middle of a clip hears it from there.
//!
//! # Warped clips follow the clock
//!
//! A region that plays the buffer of a clip warped to the song's tempo
//! (`SongAudioRegion::tempo_warp`, a buffer rendered at the song's own tempo map) is read at the
//! pace of the song's own timeline, `(u1 - u0) / n` buffer frames per player frame, so it keeps
//! the clock's tempo for as long as it plays instead of drifting by the share the two tempos
//! differ. Beats and Tonal go through the track's real-time stretcher (`audio_stretch.rs`, the
//! stretcher of the interim warps: the pitch stays); Repitch is read by four-point Hermite
//! interpolation, so its pitch moves with the clock as the mode always does; when no stretcher
//! is free the read is the Hermite one too. Paces are limited to a quarter to four times the
//! song's own.
//!
//! The pace comes from the stretch, which already includes the tempo of the clock; a small term
//! closes what the phase lock's nudges leave (the difference between where the song is on the
//! clip and where the clip has played, over about 200 ms, at most 2% of the pace), and a
//! difference of more than 50 ms jumps. Fades, gain and envelope are read at the clip frame an
//! output frame stands for. A clip ends where the song says it does, so a clip of many bars
//! ends in sync with the clock, not with its own length.
//!
//! # Clips that are not warped
//!
//! Any other region plays at its own rate from where it started: a region plays `frames` frames
//! and ends, whatever the tempo does meanwhile, so it is re-synchronised with the song at every
//! locate, loop wrap and region start, not within a long clip (warp it for a tight lock). At
//! most [`MAX_ACTIVE`] regions of a track play at once (more than the two a track may overlap by
//! on its own timeline, for clips that run on past their neighbours' start). The renderer
//! allocates nothing: the state is a small array in the track's RT scratch.
//!
//! # Streamed regions
//!
//! A track that reads streamed assets has one ring (see `stream.rs`) that holds the finished
//! output of its streamed regions in the song's own order. While following, the lane's cursor
//! sits at the song's place while no streamed region plays (it skips ahead, never back), and a
//! region that the clock reaches plays from the ring at the clip's own rate, like an unwarped
//! memory region. (A warped clip plays its rendered buffer, which is in memory, and follows the
//! clock as above.) A streamed region that starts while another still plays takes over from the
//! ring at its start: the other is cut where the cursor moves on, and with a clock slower than
//! the song a region that runs past its neighbour's start simply continues. A loop wrap or a
//! locate opens a new epoch of the ring, so the first frames of a streamed clip after it may be
//! late (an underrun is counted, never a wait).

use crate::audio::{
    AudioRt, CompiledAudio, LaneData, SongAsset, SongAudioRegion, hermite_at, render_region,
};
use crate::audio_stretch::StretchBank;
use crate::stream::{StreamLaneRt, StreamRequest, StreamStats};
use atomik_core::{BATCH, Batch};
use rtrb::Producer;
use std::sync::atomic::Ordering;

/// Regions of one track that play at once while following.
pub(crate) const MAX_ACTIVE: usize = 4;

/// Slowest and fastest pace of a warped clip, in song frames per player frame.
const RATE_MIN: f64 = 0.25;
const RATE_MAX: f64 = 4.0;
/// The seconds over which a warped clip closes its difference with the song.
const CATCH_SECONDS: f64 = 0.2;
/// The most the catch-up changes the pace, in frames per frame.
const CATCH_MAX: f64 = 0.02;
/// A difference (in seconds) beyond which a warped clip jumps to the song's place.
const JUMP_SECONDS: f64 = 0.05;
/// Gains one stretch of a warped clip reads at most (the fastest pace over a batch, plus two).
const GAINS: usize = (RATE_MAX as usize + 1) * BATCH + 2;

/// A region that is playing.
#[derive(Debug, Clone, Copy)]
struct Active {
    idx: u32,
    /// Frames already played (a region that is not warped).
    played: u64,
    /// Where a warped region reads, in frames from its start.
    pos: f64,
    /// Output frames given to its stretcher so far.
    out: u64,
}

/// The regions playing on a track and where the last stretch ended.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FollowRt {
    active: [Option<Active>; MAX_ACTIVE],
    /// The song-timeline frame the previous stretch ended at.
    last_u: f64,
    /// Frames of the ring the streamed region under way still has.
    stream_left: u64,
}

/// What the streamed regions of a track need from the player.
pub(crate) struct StreamIo<'a> {
    pub(crate) requests: &'a mut Producer<StreamRequest>,
    pub(crate) stats: &'a StreamStats,
}

/// Renders the stretch of `n` player frames at batch offset `off` whose ends on the song's own
/// timeline are `u0` and `u1` into `rt.pb` (which holds zeros or earlier output: the regions
/// add). `reseek` rebuilds which regions are underway (see the module docs); `sr` is the sample
/// rate (for the real-time stretchers of warped clips).
pub(crate) fn render_segment(
    track: &CompiledAudio,
    rt: &mut AudioRt,
    (u0, u1): (f64, f64),
    n: usize,
    off: usize,
    (reseek, sr): (bool, u32),
    io: &mut StreamIo<'_>,
) {
    let data = &*track.data;
    let (u0, u1) = (u0.max(0.0), u1.max(0.0));
    let rate = ((u1 - u0) / n.max(1) as f64).clamp(RATE_MIN, RATE_MAX);
    let lo = {
        let AudioRt {
            pb,
            follow,
            stretch,
            ..
        } = rt;
        stretch.begin(sr);
        if reseek {
            follow.active = [None; MAX_ACTIVE];
            follow.last_u = u0;
            let first = data.regions.partition_point(|r| r.end() as f64 <= u0);
            for (idx, r) in data.regions.iter().enumerate().skip(first) {
                if r.song_start_frame as f64 >= u0 {
                    break;
                }
                let pos = u0 - r.song_start_frame as f64;
                start(follow, idx, pos.floor() as u64, pos);
            }
        }
        // Regions already playing run on from the start of the stretch.
        for slot in &mut follow.active {
            let Some(a) = slot else { continue };
            let r = &data.regions[a.idx as usize];
            let done = if r.tempo_warp.is_some() {
                let ahead = u0 - r.song_start_frame as f64 - a.pos;
                let pace = if ahead.abs() > JUMP_SECONDS * f64::from(sr) {
                    // Too far from the song to catch up: go to its place (a new stream).
                    a.pos = (u0 - r.song_start_frame as f64).max(0.0);
                    a.out += 1;
                    rate
                } else {
                    let catch =
                        (ahead / (CATCH_SECONDS * f64::from(sr))).clamp(-CATCH_MAX, CATCH_MAX);
                    rate + catch
                };
                play_warped(
                    data,
                    a,
                    pace.clamp(RATE_MIN, RATE_MAX),
                    (0, n, off),
                    pb,
                    stretch,
                )
            } else {
                a.played += play(data, a.idx as usize, a.played, (0, n, off), pb, stretch);
                a.played >= r.frames
            };
            if done {
                *slot = None;
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
            let k = crossing(start_at, (u0, u1), n);
            if r.tempo_warp.is_some() {
                let mut a = Active {
                    idx: idx as u32,
                    played: 0,
                    pos: 0.0,
                    out: 0,
                };
                if !play_warped(data, &mut a, rate, (k, n, off), pb, stretch) {
                    start_active(follow, a);
                }
            } else {
                let played = play(data, idx, 0, (k, n, off), pb, stretch);
                if played < r.frames {
                    start(follow, idx, played, played as f64);
                }
            }
        }
        lo
    };
    stream_segment(track, rt, (u0, u1), lo, (n, off), reseek, io);
    rt.follow.last_u = rt.follow.last_u.max(u1);
}

/// The player frame, in `0..n`, where the song-timeline frame `at` falls in a stretch whose ends
/// are `u0` and `u1`.
fn crossing(at: f64, (u0, u1): (f64, f64), n: usize) -> usize {
    if u1 > u0 && at > u0 {
        (((at - u0) / (u1 - u0) * n as f64).ceil() as usize).min(n.saturating_sub(1))
    } else {
        0
    }
}

/// Adds region `idx` to the first free slot (when none is free the region is not played).
fn start(follow: &mut FollowRt, idx: usize, played: u64, pos: f64) {
    start_active(
        follow,
        Active {
            idx: idx as u32,
            played,
            pos,
            out: 0,
        },
    );
}

fn start_active(follow: &mut FollowRt, a: Active) {
    if let Some(slot) = follow.active.iter_mut().find(|s| s.is_none()) {
        *slot = Some(a);
    }
}

/// Plays region `idx` from its frame `played` into frames `k..n` of the stretch; returns the
/// frames played (the region's end cuts it).
fn play(
    data: &LaneData,
    idx: usize,
    played: u64,
    (k, n, off): (usize, usize, usize),
    pb: &mut [Batch; 2],
    stretch: &mut StretchBank,
) -> u64 {
    let r = &data.regions[idx];
    let frames = ((n - k) as u64).min(r.frames.saturating_sub(played)) as usize;
    if frames > 0 && frames <= BATCH {
        render_region(data, idx, pb, (played, frames, off + k, frames), stretch);
    }
    frames as u64
}

/// Plays the tempo-warped region `a` into frames `k..n` of the stretch, reading its buffer
/// `rate` frames per output frame from `a.pos`; returns true when it has ended.
fn play_warped(
    data: &LaneData,
    a: &mut Active,
    rate: f64,
    (k, n, off): (usize, usize, usize),
    pb: &mut [Batch; 2],
    stretch: &mut StretchBank,
) -> bool {
    let idx = a.idx as usize;
    let r = &data.regions[idx];
    let end = r.frames as f64;
    let want = n - k;
    // Output frames before the region's end.
    let m = if a.pos + want as f64 * rate <= end {
        want
    } else {
        (((end - a.pos) / rate).ceil().max(0.0) as usize).min(want)
    };
    if m == 0 || m > BATCH {
        return a.pos >= end;
    }
    let SongAsset::Memory { data: planes, .. } = &data.assets[r.asset.0 as usize] else {
        return true;
    };
    let l = &planes[0];
    let rr = &planes[planes.len().min(2) - 1];
    // Source frame of output frame 0 in the buffer.
    let base = r.source_offset as f64 + a.pos;
    let mut v = [[0.0f32; BATCH]; 2];
    let sr = stretch.sample_rate();
    let stretcher = data.stretch_slot[idx]
        .filter(|_| r.follow_keeps_pitch())
        .and_then(|s| stretch.take(idx as u32, s));
    if let Some(st) = stretcher {
        let knots = [
            (a.out as f64, base),
            ((a.out + m as u64) as f64, base + m as f64 * rate),
        ];
        let [vl, vr] = &mut v;
        st.process(
            [&l[..], &rr[..]],
            &knots,
            1.0,
            sr,
            a.out,
            [&mut vl[..m], &mut vr[..m]],
        );
    } else {
        let [vl, vr] = &mut v;
        for (i, (ol, or)) in vl[..m].iter_mut().zip(&mut vr[..m]).enumerate() {
            let s = base + i as f64 * rate;
            *ol = hermite_at(l, s);
            *or = hermite_at(rr, s);
        }
    }
    // The gain of the clip frame each output frame stands for.
    let frame_of = |i: usize| ((a.pos + i as f64 * rate).floor() as u64).min(r.frames - 1);
    let first = frame_of(0);
    let span = ((frame_of(m - 1) - first) as usize + 1).min(GAINS);
    let mut g = [0.0f32; GAINS];
    r.gains(data.envelope(r), first, &mut g[..span]);
    for i in 0..m {
        let gi = ((frame_of(i) - first) as usize).min(span - 1);
        pb[0][off + k + i] += v[0][i] * g[gi];
        pb[1][off + k + i] += v[1][i] * g[gi];
    }
    a.pos += m as f64 * rate;
    a.out += m as u64;
    a.pos >= end
}

/// Streamed regions of the track (see the module docs): plays what the ring carries for the
/// regions the clock has reached and keeps the lane's cursor at the song's place between them.
/// `lo` is where the previous stretch ended.
fn stream_segment(
    track: &CompiledAudio,
    rt: &mut AudioRt,
    (u0, u1): (f64, f64),
    lo: f64,
    (n, off): (usize, usize),
    reseek: bool,
    io: &mut StreamIo<'_>,
) {
    let AudioRt {
        pb, stream, follow, ..
    } = rt;
    let Some(lane) = stream.as_mut().filter(|_| track.streams) else {
        return;
    };
    let data = &*track.data;
    let is_stream = |r: &SongAudioRegion| data.assets[r.asset.0 as usize].is_stream();
    match lane.follow_origin() {
        Some(_) if !reseek => lane.follow_retry(io.requests, io.stats),
        _ => {
            let origin = u0.floor() as u64;
            lane.follow_seek(origin, io.requests, io.stats);
            // The regions already under way at the origin continue from the ring's first frame.
            follow.stream_left = data
                .regions
                .iter()
                .filter(|r| is_stream(r) && r.song_start_frame <= origin && r.end() > origin)
                .map(|r| r.end() - origin)
                .max()
                .unwrap_or(0);
        }
    }
    let Some(origin) = lane.follow_origin() else {
        return;
    };
    let mut at = 0usize;
    let first = data
        .regions
        .partition_point(|r| (r.song_start_frame as f64) < lo);
    for r in data.regions[first..].iter().filter(|r| is_stream(r)) {
        let start_at = r.song_start_frame as f64;
        if start_at >= u1 {
            break;
        }
        let k = crossing(start_at, (u0, u1), n);
        take_ring(lane, follow, pb, io.stats, (at, k, off));
        at = at.max(k);
        // The ring index of the region's first frame; the cursor never goes back.
        lane.skip_to(r.song_start_frame.saturating_sub(origin));
        let from = origin + lane.played();
        follow.stream_left = follow.stream_left.max(r.end().saturating_sub(from));
    }
    take_ring(lane, follow, pb, io.stats, (at, n, off));
    if follow.stream_left == 0 {
        lane.skip_to((u1.floor() as u64).saturating_sub(origin));
    }
}

/// Adds the ring's next frames to output frames `from..to` while a streamed region is under
/// way.
fn take_ring(
    lane: &mut StreamLaneRt,
    follow: &mut FollowRt,
    pb: &mut [Batch; 2],
    stats: &StreamStats,
    (from, to, off): (usize, usize, usize),
) {
    let m = (to.saturating_sub(from) as u64).min(follow.stream_left) as usize;
    if m == 0 {
        return;
    }
    let (mut l, mut r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
    if lane.take(m, &mut l[..m], &mut r[..m]) > 0 {
        stats.underruns.fetch_add(1, Ordering::Relaxed);
    }
    for i in 0..m {
        pb[0][off + from + i] += l[i];
        pb[1][off + from + i] += r[i];
    }
    follow.stream_left -= m as u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        AssetId, SongAsset, SongAudioRegion, SongAudioTrack, SongWarpMode, compile_audio,
    };
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
        let (mut requests, _rx) = rtrb::RingBuffer::new(4);
        let stats = StreamStats::default();
        let mut io = StreamIo {
            requests: &mut requests,
            stats: &stats,
        };
        for (k, u) in ends.iter().enumerate() {
            rt.pb = [[0.0; BATCH]; 2];
            render_segment(
                t,
                &mut rt,
                *u,
                BATCH,
                0,
                (reseek_first && k == 0, 48_000),
                &mut io,
            );
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

    /// A track of one tempo-warped region over a ramp buffer (`asset[i] = sample(i)`).
    fn warped_track(mode: SongWarpMode, frames: u64) -> CompiledAudio {
        let r = SongAudioRegion {
            tempo_warp: Some(mode),
            ..region(0, 0, frames)
        };
        track(vec![r])
    }

    #[test]
    fn a_warped_clip_reads_its_buffer_at_the_pace_of_the_song_timeline() {
        // Repitch reads by interpolation: the ramp comes out at the pace, 1.25 frames a frame.
        let t = warped_track(SongWarpMode::Repitch, 3_000);
        let out = play_through(&t, &steady(0.0, 80.0, 50), true);
        // 64 player frames cover 80 timeline frames.
        for (i, v) in out.iter().enumerate().take(1000).skip(4) {
            let want = sample(0) + (i as f32 * 1.25) * 1.0e-4;
            assert!((*v - want).abs() < 2.0e-4, "frame {i}: {v} vs {want}");
        }
        // A clip of 3000 frames ends after 3000 / 1.25 player frames.
        let last = out.iter().rposition(|v| *v != 0.0).unwrap();
        assert!(last.abs_diff(2400) <= 2, "ends at {last}");
    }

    #[test]
    fn a_warped_clip_at_the_songs_own_pace_is_the_buffer_itself() {
        for mode in [
            SongWarpMode::Repitch,
            SongWarpMode::Beats,
            SongWarpMode::Tonal,
        ] {
            let t = warped_track(mode, 1_000);
            let out = play_through(&t, &steady(0.0, 64.0, 20), true);
            for (i, v) in out.iter().enumerate().take(1000) {
                assert!((*v - sample(i)).abs() < 1.0e-6, "{mode:?} frame {i}");
            }
            assert!(out[1000..].iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn a_stretched_warped_clip_ends_with_the_song_not_with_its_length() {
        // The followed tempo is 5% faster than the song's: the clip ends 5% sooner.
        let t = warped_track(SongWarpMode::Tonal, 3_000);
        let out = play_through(&t, &steady(0.0, 67.2, 60), true);
        let last = out.iter().rposition(|v| *v != 0.0).unwrap();
        assert!(last.abs_diff((3_000.0f64 / 1.05) as usize) <= 40, "{last}");
    }

    #[test]
    fn an_unwarped_clip_keeps_its_own_rate() {
        let t = track(vec![region(0, 0, 500)]);
        let out = play_through(&t, &steady(0.0, 80.0, 20), true);
        for (i, v) in out.iter().enumerate().take(500) {
            assert_eq!(*v, sample(i));
        }
    }

    #[test]
    fn a_warped_clip_follows_a_changing_pace_without_losing_the_song() {
        let t = warped_track(SongWarpMode::Beats, 6_000);
        let mut ends = steady(0.0, 64.0, 30);
        let from = ends.last().unwrap().1;
        ends.extend(steady(from, 72.0, 30));
        let out = play_through(&t, &ends, true);
        let a = play_through(&t, &ends, true);
        assert_eq!(out, a, "deterministic");
        assert!(out.iter().any(|v| *v != 0.0));
    }
}
