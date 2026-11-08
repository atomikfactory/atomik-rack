//! The arithmetic of dragging a film on the arrangement: where a moved or trimmed edge lands
//! when it snaps to the song's grid and to whole video frames.
//!
//! A film has no tempo, so the two snaps are separate steps and they compose: the grid (a
//! function the caller passes in, since the grid lives in ticks and the tempo map) picks a time
//! in song time, then the **frame snap** rounds it to a whole frame:
//!
//! * a **move** puts the film's start on a whole frame of the song's clock (`k` frames from the
//!   start of the song at the film's rate), so the film's frame boundaries line up with a
//!   timecode counted from bar 1;
//! * a **trim** puts the new in or out point on a whole frame of the source (`k` frames from
//!   the film's first frame), so a cut never lands inside a picture. The trimmed edge keeps
//!   the picture that stays where it is, as [`crate::VideoCommand::Trim`] does.
//!
//! Everything is integer microseconds on `i128` (a frame rate is a fraction, a frame of
//! 30000/1001 fps is 33 366.7 us and is rounded to the nearest microsecond, never accumulated),
//! so the same drag always gives the same numbers. A snap that is switched off (the no-snap
//! modifier) is the caller's: it passes `frame: false` and no grid.

use crate::video::{MAX_START_US, MIN_TRIM_US, VideoClip};

/// The start of frame `k` in microseconds, for a rate of `num / den` frames per second (rounded
/// to the nearest microsecond; `k` may be negative).
pub fn frame_start_us(k: i64, num: u32, den: u32) -> i64 {
    let num = i128::from(num.max(1));
    let micros = i128::from(k) * i128::from(den.max(1)) * 1_000_000;
    let rounded = (2 * micros + num).div_euclid(2 * num);
    rounded.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// The number of the whole frame nearest to `us` (a time exactly between two frames goes to the
/// later one).
pub fn nearest_frame(us: i64, num: u32, den: u32) -> i64 {
    let per_second = i128::from(den.max(1)) * 1_000_000;
    let scaled = i128::from(us) * i128::from(num);
    let k = (2 * scaled + per_second).div_euclid(2 * per_second);
    k.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// `us` rounded to the start of the nearest whole frame.
pub fn snap_to_frame(us: i64, num: u32, den: u32) -> i64 {
    frame_start_us(nearest_frame(us, num, den), num, den)
}

/// What a drag may snap to: the grid and the frames. Each is optional (the no-snap modifier
/// switches both off).
#[derive(Clone, Copy)]
pub struct Snap<'a> {
    /// Rounds a song time in microseconds to the grid's nearest line; `None`: no grid.
    pub grid: Option<&'a dyn Fn(i64) -> i64>,
    /// Round to whole frames.
    pub frame: bool,
}

impl Snap<'_> {
    /// No snapping at all.
    pub const OFF: Snap<'static> = Snap {
        grid: None,
        frame: false,
    };

    fn song_time(&self, us: i64) -> i64 {
        self.grid.map_or(us, |g| g(us))
    }
}

/// The shortest trim a drag leaves: one frame, never less than the model's minimum.
pub fn min_trim_us(num: u32, den: u32) -> u64 {
    (frame_start_us(1, num, den).max(0) as u64).max(MIN_TRIM_US)
}

/// The start a move to the raw song time `raw_us` gives: the grid first, then the frame snap,
/// kept inside the model's limits.
pub fn moved_start(raw_us: i64, num: u32, den: u32, snap: Snap<'_>) -> i64 {
    let mut us = snap.song_time(raw_us);
    if snap.frame {
        us = snap_to_frame(us, num, den);
    }
    us.clamp(-MAX_START_US, MAX_START_US)
}

/// The trim `(in_us, out_us)` after the left edge of `origin` was dragged to the raw song time
/// `edge_us`. The out point stays; the model moves the start with the in point.
pub fn trimmed_in(origin: &VideoClip, edge_us: i64, snap: Snap<'_>) -> (u64, u64) {
    let (num, den) = (origin.facts.fps_num, origin.facts.fps_den);
    let at = snap.song_time(edge_us);
    let mut source = i128::from(origin.in_us) + i128::from(at) - i128::from(origin.start_us);
    if snap.frame {
        source = i128::from(snap_to_frame(clamp_i64(source), num, den));
    }
    let latest = i128::from(origin.out_us) - i128::from(min_trim_us(num, den));
    let new_in = source.clamp(0, latest.max(0));
    (new_in as u64, origin.out_us)
}

/// The trim `(in_us, out_us)` after the right edge of `origin` was dragged to the raw song
/// time `edge_us`. The in point and the start stay.
pub fn trimmed_out(origin: &VideoClip, edge_us: i64, snap: Snap<'_>) -> (u64, u64) {
    let (num, den) = (origin.facts.fps_num, origin.facts.fps_den);
    let at = snap.song_time(edge_us);
    let mut source = i128::from(origin.in_us) + i128::from(at) - i128::from(origin.start_us);
    let duration = i128::from(origin.facts.duration_us);
    if snap.frame {
        // The end of the file is not always on a whole frame: an edge within half a frame of it
        // takes the file's end itself, so the whole film can always be restored.
        let half = i128::from(frame_start_us(1, num, den)) / 2;
        source = if duration - source < half.max(0) {
            duration
        } else {
            i128::from(snap_to_frame(clamp_i64(source), num, den))
        };
    }
    let earliest = i128::from(origin.in_us) + i128::from(min_trim_us(num, den));
    let new_out = source.clamp(earliest, duration.max(earliest));
    (origin.in_us, new_out as u64)
}

fn clamp_i64(v: i128) -> i64 {
    v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::VideoFacts;

    fn clip(fps: (u32, u32), duration_us: u64) -> VideoClip {
        VideoClip::new(
            "film.mp4",
            VideoFacts {
                duration_us,
                fps_num: fps.0,
                fps_den: fps.1,
                width: 640,
                height: 360,
                timecode: None,
                has_audio: false,
            },
        )
    }

    #[test]
    fn frames_start_on_exact_microseconds_at_whole_rates() {
        assert_eq!(frame_start_us(0, 25, 1), 0);
        assert_eq!(frame_start_us(1, 25, 1), 40_000);
        assert_eq!(frame_start_us(250, 25, 1), 10_000_000);
        assert_eq!(frame_start_us(-3, 25, 1), -120_000);
    }

    #[test]
    fn fractional_rates_do_not_drift() {
        // 30000/1001: 30 000 frames last exactly 1001 seconds.
        assert_eq!(frame_start_us(30_000, 30_000, 1001), 1_001_000_000);
        assert_eq!(frame_start_us(1, 30_000, 1001), 33_367);
        for k in [0, 1, 2, 29, 30, 1799, 107_892] {
            let us = frame_start_us(k, 30_000, 1001);
            assert_eq!(nearest_frame(us, 30_000, 1001), k, "frame {k}");
        }
    }

    #[test]
    fn the_nearest_frame_rounds_halves_up_and_negatives_down() {
        // 25 fps: 20 ms is half a frame.
        assert_eq!(nearest_frame(19_999, 25, 1), 0);
        assert_eq!(nearest_frame(20_000, 25, 1), 1);
        assert_eq!(nearest_frame(-19_999, 25, 1), 0);
        assert_eq!(nearest_frame(-20_001, 25, 1), -1);
        assert_eq!(snap_to_frame(1_013_000, 25, 1), 1_000_000);
    }

    #[test]
    fn a_move_snaps_to_the_grid_then_to_a_whole_frame() {
        // A grid line at 1.01 s (not a whole frame at 25 fps): the start lands on the frame next
        // to it, 1.00 s.
        let grid = |us: i64| {
            if (us - 1_010_000).abs() < 50_000 {
                1_010_000
            } else {
                us
            }
        };
        let snap = Snap {
            grid: Some(&grid),
            frame: true,
        };
        assert_eq!(moved_start(1_030_000, 25, 1, snap), 1_000_000);
        // Away from the grid it is the frame alone.
        assert_eq!(moved_start(2_013_000, 25, 1, snap), 2_000_000);
        // Frame snap alone (the selector's snap off).
        let frames = Snap {
            grid: None,
            frame: true,
        };
        assert_eq!(moved_start(2_029_999, 25, 1, frames), 2_040_000);
        // The no-snap modifier: the raw time.
        assert_eq!(moved_start(2_013_007, 25, 1, Snap::OFF), 2_013_007);
    }

    #[test]
    fn a_move_stays_inside_the_models_limits() {
        assert_eq!(moved_start(i64::MAX, 25, 1, Snap::OFF), MAX_START_US);
        assert_eq!(moved_start(i64::MIN, 25, 1, Snap::OFF), -MAX_START_US);
        let frames = Snap {
            grid: None,
            frame: true,
        };
        assert!(moved_start(i64::MAX, 25, 1, frames) <= MAX_START_US);
    }

    #[test]
    fn the_in_edge_cuts_on_a_source_frame_and_keeps_the_out_point() {
        let mut c = clip((25, 1), 10_000_000);
        c.start_us = 4_000_000;
        let frames = Snap {
            grid: None,
            frame: true,
        };
        // The edge dragged 1.013 s right of the start: in = 1.00 s of the source.
        assert_eq!(trimmed_in(&c, 5_013_000, frames), (1_000_000, 10_000_000));
        // Without the snap the cut is where the pointer is.
        assert_eq!(
            trimmed_in(&c, 5_013_000, Snap::OFF),
            (1_013_000, 10_000_000)
        );
        // Dragged left of the start: never before the file's first frame.
        assert_eq!(trimmed_in(&c, 1_000_000, frames).0, 0);
        // Dragged past the out point: one frame is left.
        assert_eq!(trimmed_in(&c, 99_000_000, frames).0, 9_960_000);
    }

    #[test]
    fn a_source_cut_is_independent_of_where_the_film_starts() {
        let mut c = clip((25, 1), 10_000_000);
        c.start_us = 4_003_000;
        let frames = Snap {
            grid: None,
            frame: true,
        };
        // The edge 1.01 s into the film (start + 1.01 s): a whole source frame, 1.00 s.
        assert_eq!(trimmed_in(&c, 5_013_000, frames).0, 1_000_000);
    }

    #[test]
    fn the_out_edge_cuts_on_a_source_frame_and_can_restore_the_end_of_the_file() {
        // 10.01 s at 25 fps: the file ends between frames.
        let mut c = clip((25, 1), 10_010_000);
        c.out_us = 5_000_000;
        let frames = Snap {
            grid: None,
            frame: true,
        };
        assert_eq!(trimmed_out(&c, 3_013_000, frames), (0, 3_000_000));
        // Within half a frame of the end (or past it): the whole file.
        assert_eq!(trimmed_out(&c, 9_970_000, frames).1, 9_960_000);
        assert_eq!(trimmed_out(&c, 9_995_000, frames).1, 10_010_000);
        assert_eq!(trimmed_out(&c, 10_005_000, frames).1, 10_010_000);
        assert_eq!(trimmed_out(&c, 90_000_000, frames).1, 10_010_000);
        // Dragged before the in point: one frame is left.
        c.in_us = 2_000_000;
        assert_eq!(trimmed_out(&c, -5_000_000, frames), (2_000_000, 2_040_000));
    }

    #[test]
    fn the_grid_decides_the_song_time_of_the_edge_before_the_frame_snap() {
        let mut c = clip((25, 1), 60_000_000);
        c.start_us = 2_000_000;
        // Grid lines every 0.5 s of song time.
        let grid = |us: i64| (us + 250_000).div_euclid(500_000) * 500_000;
        let snap = Snap {
            grid: Some(&grid),
            frame: true,
        };
        // 3.61 s of song time -> grid 3.5 s -> source 1.5 s -> frame 37.5 -> 1.52 s (a half goes up).
        assert_eq!(trimmed_in(&c, 3_610_000, snap).0, 1_520_000);
        assert_eq!(trimmed_out(&c, 9_760_000, snap).1, 8_000_000);
    }
}
