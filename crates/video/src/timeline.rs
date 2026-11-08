//! Which frame shows at a song position, and what timecode it carries.
//!
//! Everything here is integer arithmetic on microseconds and frame rates as fractions, so the
//! frame of a position is exact for 24, 25, 30000/1001 and every other rate: a position that is
//! one microsecond before the start of frame `n` shows frame `n - 1`, never frame `n`.
//!
//! * [`locate`]: the film's clip (where it starts in the song, its trim) and a song time in,
//!   [`Shot`] out: the frame to show, or "before the film" / "after the film" with the frame
//!   number the film would have there (the timecode is still shown on the black picture).
//! * [`Timecode`]: the film's start timecode (`01:00:00:00`, or `;` before the frames for drop
//!   frame) and the text of the timecode at any frame, drop frame included.
//! * [`wanted`]: which frames the viewer decodes next, nearest first (the decode-ahead).

use crate::probe::Rational;

/// Where the picture of a song time comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shot {
    /// Before the film starts: black. `number` is the frame number the film would have there
    /// (negative), for the timecode.
    Before {
        /// The frame number, counted from the first frame of the trim.
        number: i64,
    },
    /// The film shows its frame `n` (counted from the first frame of the proxy).
    Frame {
        /// The frame index in the proxy.
        n: u32,
        /// The same number counted from the trim's first frame (what the timecode uses).
        number: i64,
    },
    /// After the film ended: black, with the frame number the film would have there.
    After {
        /// The frame number, counted from the first frame of the trim.
        number: i64,
    },
}

impl Shot {
    /// The proxy frame to decode, `None` for black.
    pub fn frame(self) -> Option<u32> {
        match self {
            Shot::Frame { n, .. } => Some(n),
            _ => None,
        }
    }

    /// The frame number for the overlay: counted from the first frame of the film's source
    /// (before the trim), so it is the same number the timecode counts.
    pub fn number(self) -> i64 {
        match self {
            Shot::Before { number } | Shot::Frame { number, .. } | Shot::After { number } => number,
        }
    }
}

/// The number of the frame that shows at `us` microseconds from a film's first frame: the floor
/// of `us * rate`, negative before the first frame. Exact for every rate.
pub fn frame_number(us: i64, rate: Rational) -> i64 {
    let scaled = i128::from(us) * i128::from(rate.num);
    let per_second = i128::from(rate.den) * 1_000_000;
    scaled.div_euclid(per_second) as i64
}

/// The song time `at_us` as a [`Shot`] of a film that plays its source `in_us..out_us` starting
/// at `start_us` in the song, with a proxy of `frames` frames at `rate`.
///
/// The frame number in the result counts from the start of the source (`in_us` is part of it),
/// so the timecode of a trimmed film is the timecode of the picture that shows. A proxy shorter
/// than the film (a variable-rate source) shows its last frame to the end of the trim.
pub fn locate(
    at_us: i64,
    start_us: i64,
    in_us: u64,
    out_us: u64,
    rate: Rational,
    frames: u32,
) -> Shot {
    let into = i128::from(at_us) - i128::from(start_us);
    let source = i128::from(in_us) + into;
    let number = frame_number(
        source.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
        rate,
    );
    if into < 0 {
        return Shot::Before { number };
    }
    if source >= i128::from(out_us) {
        return Shot::After { number };
    }
    let last = i64::from(frames.saturating_sub(1));
    Shot::Frame {
        n: number.clamp(0, last) as u32,
        number,
    }
}

/// A start timecode and the frame rate it counts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    /// The frame count of the start (from `00:00:00:00`).
    start: i64,
    /// Frames per second as the timecode counts them: 24, 25, 30, 60...
    base: i64,
    /// Drop frame: the first frames of most minutes are skipped in the count.
    drop: bool,
}

impl Timecode {
    /// The timecode of a film that starts at `00:00:00:00`.
    pub fn zero(rate: Rational) -> Self {
        Self {
            start: 0,
            base: base_of(rate),
            drop: false,
        }
    }

    /// Reads `HH:MM:SS:FF` (`;` or `.` before the frames: drop frame, valid at 30000/1001 and
    /// 60000/1001 only). `None` for text that is not a timecode or has frames past the rate.
    pub fn parse(text: &str, rate: Rational) -> Option<Self> {
        let text = text.trim();
        let drop_marker = text.contains([';', '.']);
        let parts: Vec<&str> = text.split([':', ';', '.']).collect();
        if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || p.len() > 3) {
            return None;
        }
        let n: Vec<i64> = parts
            .iter()
            .map(|p| p.parse::<i64>().ok())
            .collect::<Option<_>>()?;
        let (h, m, s, f) = (n[0], n[1], n[2], n[3]);
        let base = base_of(rate);
        if m > 59 || s > 59 || f >= base {
            return None;
        }
        let dropped = drop_per_minute(rate, base);
        let drop = drop_marker && dropped > 0;
        let minutes = 60 * h + m;
        let mut start = (3_600 * h + 60 * m + s) * base + f;
        if drop {
            start -= dropped * (minutes - minutes / 10);
        }
        Some(Self { start, base, drop })
    }

    /// The text of the timecode of frame `number` of the film (the first frame of the source is
    /// 0). A negative count shows with a minus sign.
    pub fn text(&self, number: i64) -> String {
        let total = self.start + number;
        let sign = if total < 0 { "-" } else { "" };
        let count = total.abs();
        let count = if self.drop { self.undrop(count) } else { count };
        let base = self.base;
        let (f, s) = (count % base, count / base);
        let (h, m, s) = (s / 3_600, s / 60 % 60, s % 60);
        let mark = if self.drop { ';' } else { ':' };
        format!("{sign}{h:02}:{m:02}:{s:02}{mark}{f:02}")
    }

    /// The frame count with the dropped numbers added back, so the usual split by `base`
    /// reads the drop-frame timecode.
    fn undrop(&self, count: i64) -> i64 {
        let dropped = self.base / 15;
        let per_minute = self.base * 60 - dropped;
        let per_ten = self.base * 600 - dropped * 9;
        let tens = count / per_ten;
        let rest = count % per_ten;
        let extra = if rest > dropped {
            dropped * ((rest - dropped) / per_minute)
        } else {
            0
        };
        count + 9 * dropped * tens + extra
    }
}

/// The whole frames per second a timecode counts: 30 for 29.97, 24 for 23.976.
fn base_of(rate: Rational) -> i64 {
    ((f64::from(rate.num) / f64::from(rate.den)).round() as i64).max(1)
}

/// The frame numbers dropped at the start of most minutes: 2 at 29.97, 4 at 59.94, none at any
/// other rate (drop frame only exists for those two).
fn drop_per_minute(rate: Rational, base: i64) -> i64 {
    let ntsc = u64::from(rate.den) == 1_001 && matches!(base, 30 | 60);
    if ntsc { base / 15 } else { 0 }
}

/// How many frames the viewer decodes ahead of the one shown while the song plays, and either
/// side of it while it is stopped (scrubbing goes both ways).
pub const AHEAD: u32 = 12;
/// How many frames either side are kept ready while stopped.
pub const AROUND: u32 = 3;

/// The frames to have decoded, the one shown first, then the likeliest next ones. `current` is
/// the frame shown, `frames` the length of the proxy, `playing` whether the song plays.
/// `wrap` is the loop as frames `(end, start)` while it is on: near its end the first frames of
/// the loop are wanted too, so the jump back does not wait for a decode.
pub fn wanted(current: u32, frames: u32, playing: bool, wrap: Option<(u32, u32)>) -> Vec<u32> {
    if frames == 0 {
        return Vec::new();
    }
    let last = frames - 1;
    let current = current.min(last);
    let mut out = vec![current];
    if playing {
        out.extend((1..=AHEAD).map(|i| current + i).take_while(|n| *n <= last));
        if let Some((_, start)) = wrap.filter(|(end, _)| current + AHEAD >= *end) {
            out.extend((0..AHEAD / 2).map(|i| start + i).filter(|n| *n <= last));
        }
    } else {
        for i in 1..=AROUND {
            if current + i <= last {
                out.push(current + i);
            }
            if current >= i {
                out.push(current - i);
            }
        }
    }
    let mut seen = Vec::with_capacity(out.len());
    out.retain(|n| {
        let fresh = !seen.contains(n);
        seen.push(*n);
        fresh
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const R25: Rational = Rational { num: 25, den: 1 };
    const NTSC: Rational = Rational {
        num: 30_000,
        den: 1_001,
    };
    const FILM: Rational = Rational {
        num: 24_000,
        den: 1_001,
    };

    #[test]
    fn a_frame_changes_exactly_at_its_start() {
        // 25 fps: frame 1 starts at 40 000 us.
        assert_eq!(frame_number(39_999, R25), 0);
        assert_eq!(frame_number(40_000, R25), 1);
        // 30000/1001: frame 30 starts at 1.001 s exactly.
        assert_eq!(frame_number(1_000_999, NTSC), 29);
        assert_eq!(frame_number(1_001_000, NTSC), 30);
        // 24000/1001: frame 24 starts at 1.001 s.
        assert_eq!(frame_number(1_000_999, FILM), 23);
        assert_eq!(frame_number(1_001_000, FILM), 24);
        // Before the first frame the number is negative and still a floor.
        assert_eq!(frame_number(-1, R25), -1);
        assert_eq!(frame_number(-40_000, R25), -1);
        assert_eq!(frame_number(-40_001, R25), -2);
    }

    #[test]
    fn a_long_film_stays_exact() {
        // 10 hours of 29.97: no drift from floating point.
        let us = 36_000_000_000_i64;
        let n = frame_number(us, NTSC);
        assert_eq!(n, 36_000_000_000 * 30_000 / (1_001 * 1_000_000));
    }

    #[test]
    fn the_start_offset_and_the_trim_move_the_picture() {
        // Starts at 2 s, plays the source from 1 s to 5 s: song 2 s is source 1 s = frame 25.
        let at = |us| locate(us, 2_000_000, 1_000_000, 5_000_000, R25, 1_000);
        assert_eq!(at(2_000_000), Shot::Frame { n: 25, number: 25 });
        assert_eq!(at(2_039_999), Shot::Frame { n: 25, number: 25 });
        assert_eq!(at(2_040_000), Shot::Frame { n: 26, number: 26 });
        assert_eq!(at(1_999_999), Shot::Before { number: 24 });
        assert_eq!(
            at(5_999_999),
            Shot::Frame {
                n: 124,
                number: 124
            }
        );
        assert_eq!(at(6_000_000), Shot::After { number: 125 });
    }

    #[test]
    fn a_film_that_starts_before_the_song_shows_at_position_zero() {
        // Starts 1 s before the song: position 0 is source frame 25.
        let shot = locate(0, -1_000_000, 0, 10_000_000, R25, 250);
        assert_eq!(shot, Shot::Frame { n: 25, number: 25 });
    }

    #[test]
    fn before_and_after_carry_the_timecode_frame_number() {
        let before = locate(-2_000_000, 0, 0, 10_000_000, R25, 250);
        assert_eq!(before, Shot::Before { number: -50 });
        assert_eq!(before.frame(), None);
        let after = locate(12_000_000, 0, 0, 10_000_000, R25, 250);
        assert_eq!(after, Shot::After { number: 300 });
        assert_eq!(after.number(), 300);
    }

    #[test]
    fn a_short_proxy_holds_its_last_frame() {
        let shot = locate(9_000_000, 0, 0, 10_000_000, R25, 100);
        assert_eq!(shot, Shot::Frame { n: 99, number: 225 });
    }

    #[test]
    fn a_loop_wrap_is_a_jump_back_to_the_frame_of_the_loop_start() {
        // Loop 4 s..8 s of the song, film from 0: the position jumps from 8 s to 4 s.
        let end = locate(7_999_999, 0, 0, 20_000_000, R25, 500).frame();
        let start = locate(4_000_000, 0, 0, 20_000_000, R25, 500).frame();
        assert_eq!((end, start), (Some(199), Some(100)));
    }

    #[test]
    fn timecode_counts_from_the_start_timecode() {
        let tc = Timecode::parse("01:00:00:00", R25).unwrap();
        assert_eq!(tc.text(0), "01:00:00:00");
        assert_eq!(tc.text(24), "01:00:00:24");
        assert_eq!(tc.text(25), "01:00:01:00");
        assert_eq!(tc.text(25 * 3_600), "02:00:00:00");
        assert_eq!(tc.text(-1), "00:59:59:24");
        assert_eq!(Timecode::zero(R25).text(-1), "-00:00:00:01");
    }

    #[test]
    fn timecode_of_non_integer_rates_counts_whole_frames() {
        let tc = Timecode::parse("00:00:00:00", FILM).unwrap();
        assert_eq!(tc.text(23), "00:00:00:23");
        assert_eq!(tc.text(24), "00:00:01:00");
        let ntsc = Timecode::parse("00:00:00:00", NTSC).unwrap();
        assert_eq!(ntsc.text(29), "00:00:00:29");
        assert_eq!(ntsc.text(30), "00:00:01:00");
    }

    #[test]
    fn drop_frame_skips_two_numbers_every_minute_but_the_tenth() {
        let tc = Timecode::parse("00:00:00;00", NTSC).unwrap();
        assert_eq!(tc.text(1_799), "00:00:59;29");
        // Frame 1800 is 00:01:00;02 (numbers ;00 and ;01 are skipped).
        assert_eq!(tc.text(1_800), "00:01:00;02");
        // The tenth minute keeps its numbers: frame 17 982 is 00:10:00;00.
        assert_eq!(tc.text(17_981), "00:09:59;29");
        assert_eq!(tc.text(17_982), "00:10:00;00");
        // One hour of 29.97 drop frame is 107 892 frames.
        assert_eq!(tc.text(107_892), "01:00:00;00");
    }

    #[test]
    fn a_drop_frame_start_reads_back() {
        let tc = Timecode::parse("01:00:00;00", NTSC).unwrap();
        assert_eq!(tc.text(0), "01:00:00;00");
        assert_eq!(tc.text(1), "01:00:00;01");
        let one = Timecode::parse("00:01:00;02", NTSC).unwrap();
        assert_eq!(one.text(0), "00:01:00;02");
        assert_eq!(one.text(-1), "00:00:59;29");
    }

    #[test]
    fn a_semicolon_at_a_rate_without_drop_frame_counts_plain() {
        let tc = Timecode::parse("00:00:10;00", R25).unwrap();
        assert_eq!(tc.text(0), "00:00:10:00");
    }

    #[test]
    fn text_that_is_not_a_timecode_is_refused() {
        for text in [
            "",
            "1:2:3",
            "01:00:00",
            "aa:bb:cc:dd",
            "00:60:00:00",
            "00:00:00:25",
            "00:00:00:99:1",
        ] {
            assert_eq!(Timecode::parse(text, R25), None, "{text:?}");
        }
    }

    #[test]
    fn playing_decodes_ahead_and_stopped_decodes_around() {
        assert_eq!(wanted(10, 100, true, None), (10..=22).collect::<Vec<_>>());
        assert_eq!(wanted(10, 100, false, None), [10, 11, 9, 12, 8, 13, 7]);
        assert_eq!(wanted(0, 100, false, None), [0, 1, 2, 3]);
        assert_eq!(wanted(98, 100, true, None), [98, 99]);
        assert_eq!(wanted(500, 100, false, None), [99, 98, 97, 96]);
        assert!(wanted(0, 0, true, None).is_empty());
    }

    #[test]
    fn near_the_end_of_a_loop_its_start_is_wanted_too() {
        let near = wanted(190, 500, true, Some((200, 100)));
        assert_eq!(&near[..3], &[190, 191, 192]);
        assert!(near.ends_with(&[100, 101, 102, 103, 104, 105]));
        let far = wanted(150, 500, true, Some((200, 100)));
        assert!(!far.contains(&100));
    }
}
