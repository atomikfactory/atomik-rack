//! The span of its file an audio clip plays from a point on: what the speaker tool auditions.
//!
//! Auditioning plays the audio *unprocessed*: the frames of the file at the file's own rate,
//! without the clip's warp, fades, envelope or crossfade. The span starts at the frame the clip
//! plays at the pressed tick and runs to the end of what the clip plays; for a reversed clip it
//! is the frames below that one, played backwards.

use crate::song::{AudioClipContent, Song};
use crate::warp::source_at_tick;

/// Frames of an asset to audition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditionSpan {
    /// First asset frame of the span.
    pub start_frame: u64,
    /// The asset frame after the last one (greater than `start_frame`).
    pub end_frame: u64,
    /// Play the span from its end to its start.
    pub reverse: bool,
}

/// The span the clip (which starts at tick `start` and lasts `length` ticks) plays from the tick
/// `at` on, in frames of its asset (`asset_frames` long, `rate` the project rate). `None` when the
/// clip plays nothing from there (the press is at or past the end of its audio).
pub fn audition_span(
    song: &Song,
    rate: u32,
    start: i64,
    a: &AudioClipContent,
    length: i64,
    at: i64,
    asset_frames: u64,
) -> Option<AuditionSpan> {
    let at = at.clamp(start, start + length.max(0));
    let clip_end = source_at_tick(song, rate, start, a, start + length);
    let here = source_at_tick(song, rate, start, a, at);
    let frame = |f: f64| (f.max(0.0).round() as u64).min(asset_frames);
    let span = if a.reverse {
        // The clip plays `[offset, offset + played)` backwards: from its top down.
        let played = (clip_end - a.source_offset as f64).max(0.0);
        let top = a.source_offset as f64 + played - (here - a.source_offset as f64);
        AuditionSpan {
            start_frame: frame(a.source_offset as f64),
            end_frame: frame(top),
            reverse: true,
        }
    } else {
        AuditionSpan {
            start_frame: frame(here),
            end_frame: frame(clip_end),
            reverse: false,
        }
    };
    (span.end_frame > span.start_frame).then_some(span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::Song;

    const RATE: u32 = 48_000;

    fn song() -> Song {
        crate::new_project_doc().song
    }

    fn clip(offset: u64, reverse: bool) -> AudioClipContent {
        let mut a = AudioClipContent::new("a".repeat(64), offset);
        a.reverse = reverse;
        a
    }

    /// Frames per tick at the default tempo.
    fn per_tick() -> f64 {
        let s = song();
        source_at_tick(&s, RATE, 0, &clip(0, false), 1000) / 1000.0
    }

    #[test]
    fn a_forward_clip_plays_from_the_pressed_tick_to_its_end() {
        let s = song();
        let a = clip(1000, false);
        let spt = per_tick();
        let span = audition_span(&s, RATE, 0, &a, 480, 240, 10_000_000).unwrap();
        assert!(!span.reverse);
        assert_eq!(span.start_frame, (1000.0 + 240.0 * spt).round() as u64);
        assert_eq!(span.end_frame, (1000.0 + 480.0 * spt).round() as u64);
    }

    #[test]
    fn a_reversed_clip_plays_the_frames_below_the_press_backwards() {
        let s = song();
        let a = clip(1000, true);
        let spt = per_tick();
        let span = audition_span(&s, RATE, 0, &a, 480, 120, 10_000_000).unwrap();
        assert!(span.reverse);
        assert_eq!(span.start_frame, 1000);
        assert_eq!(
            span.end_frame,
            (1000.0 + 480.0 * spt - 120.0 * spt).round() as u64
        );
        // Pressed at the clip start: the whole span, from its top.
        let whole = audition_span(&s, RATE, 0, &a, 480, 0, 10_000_000).unwrap();
        assert_eq!(whole.end_frame, (1000.0 + 480.0 * spt).round() as u64);
    }

    #[test]
    fn nothing_plays_from_the_end_or_past_the_asset() {
        let s = song();
        let a = clip(0, false);
        assert_eq!(audition_span(&s, RATE, 0, &a, 480, 480, 10_000_000), None);
        // A press after the clip counts as its end; a press before it as its start.
        assert_eq!(audition_span(&s, RATE, 0, &a, 480, 9999, 10_000_000), None);
        let early = audition_span(&s, RATE, 100, &a, 480, 0, 10_000_000).unwrap();
        assert_eq!(early.start_frame, 0);
        // The asset ends before the pressed frame.
        assert_eq!(
            audition_span(&s, RATE, 0, &clip(500, false), 480, 0, 100),
            None
        );
    }
}
