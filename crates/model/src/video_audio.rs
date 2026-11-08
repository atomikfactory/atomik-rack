//! The film's own sound as an audio clip: where it goes so that it plays exactly with the picture.
//!
//! Importing the audio of a film puts the whole track in the song as an asset; the clip that
//! plays it must start where the film's first frame is, in **samples** (the song places a clip at
//! a tick, a fraction of a beat, so a tick alone could be a few dozen samples off). [`placement`]
//! works it out in frames of the song's rate, once, for the film as it is now (its start and its
//! trim) and for the sound as it was found (its [`crate::AudioLink::lead_us`]: how long after the
//! first frame the track's first sample sits, from the file's start times):
//!
//! * the track's first sample belongs at the song frame of
//!   `film start + lead - trim in` (the film plays its in point at its start);
//! * only the part of the track inside the part of the film that plays is shown: from the later
//!   of the track's start and the film's in point (and the song's start), to the earlier of the
//!   track's end and the film's out point;
//! * the clip starts on the first tick at or after that frame and skips into the asset by the
//!   frames between the two (`source_offset`), so the sample under the first frame plays on
//!   exactly that frame; its length is whole ticks that stop at the film's out point (not past
//!   it), or cover the track to its end.
//!
//! The same function places the clip at import and moves it when the film moves or is trimmed
//! (see [`crate::VideoCommand`]), so the two always agree, and it is pure.

use crate::VideoClip;
use crate::freeze::ticks_for_frames;
use crate::song::{MAX_TICK, Song};

/// Where an audio clip goes: its start tick, its length in ticks and the asset frame it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPlacement {
    /// The clip's start tick.
    pub start: i64,
    /// The clip's length in ticks.
    pub length: i64,
    /// The first asset frame the clip plays.
    pub source_offset: u64,
}

/// Microseconds as song frames at `rate`, rounded to the nearest (halves up).
fn frames_of(us: i64, rate: u32) -> i128 {
    (i128::from(us) * i128::from(rate) + 500_000).div_euclid(1_000_000)
}

/// Where the clip of the film's sound goes (see the module docs): `asset_frames` is the length of
/// the imported track at the song's `rate`. `None` when no part of the track is inside the film
/// that plays.
pub fn placement(
    song: &Song,
    rate: u32,
    film: &VideoClip,
    lead_us: i64,
    asset_frames: u64,
) -> Option<AudioPlacement> {
    let in_us = i64::try_from(film.in_us).ok()?;
    let asset_start = frames_of(
        film.start_us.saturating_add(lead_us).saturating_sub(in_us),
        rate,
    );
    let asset_end = asset_start + i128::from(asset_frames);
    let first = asset_start.max(frames_of(film.start_us, rate)).max(0);
    let last = asset_end.min(frames_of(film.end_us(), rate));
    if last <= first {
        return None;
    }
    let clock = song.frame_clock(rate);
    let wanted = u64::try_from(first).ok()?;
    let mut tick = clock.tick_at(wanted).clamp(0, MAX_TICK);
    while clock.at(tick) < wanted && tick < MAX_TICK {
        tick += 1;
    }
    while tick > 0 && clock.at(tick - 1) >= wanted {
        tick -= 1;
    }
    let at = i128::from(clock.at(tick));
    if at >= last {
        return None;
    }
    let source_offset = u64::try_from(at - asset_start).ok()?;
    if source_offset >= asset_frames {
        return None;
    }
    let frames = u64::try_from(last - at).ok()?;
    let mut length = ticks_for_frames(song, tick, frames, rate);
    // Stop at the film's end rather than play past it (the track's own end needs no care: the
    // clip is simply cut there).
    if last < asset_end && length > 1 && clock.at(tick + length) - u64::try_from(at).ok()? > frames
    {
        length -= 1;
    }
    Some(AudioPlacement {
        start: tick,
        length,
        source_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::TempoEvent;
    use crate::{AudioLink, ClipId, VideoFacts};

    const RATE: u32 = 48_000;

    fn song(bpm: f64) -> Song {
        crate::song::empty_song(bpm)
    }

    fn film(start_us: i64, in_us: u64, out_us: u64) -> VideoClip {
        let mut c = VideoClip::new(
            "film.mov",
            VideoFacts {
                duration_us: 60_000_000,
                fps_num: 25,
                fps_den: 1,
                width: 1920,
                height: 1080,
                timecode: None,
                has_audio: true,
            },
        );
        c.start_us = start_us;
        c.in_us = in_us;
        c.out_us = out_us;
        c.audio_link = Some(AudioLink {
            clip: ClipId(1),
            lead_us: 0,
            keep: true,
        });
        c
    }

    /// The song frame of the first played sample of a placement.
    fn first_frame(s: &Song, p: &AudioPlacement) -> u64 {
        s.frame_at(p.start, RATE)
    }

    /// The song frame of the asset's frame 0 of a placement (the sample that was placed).
    fn asset_zero(s: &Song, p: &AudioPlacement) -> i128 {
        i128::from(first_frame(s, p)) - i128::from(p.source_offset)
    }

    #[test]
    fn a_film_at_the_song_start_puts_the_sound_at_tick_zero() {
        let s = song(120.0);
        let p = placement(&s, RATE, &film(0, 0, 60_000_000), 0, 48_000 * 60).unwrap();
        assert_eq!((p.start, p.source_offset), (0, 0));
        // 60 s at 120 BPM: 2 quarter notes per second.
        assert_eq!(p.length, 60 * 2 * atomik_core::PPQ);
    }

    #[test]
    fn the_first_sample_sits_on_the_frame_of_the_first_picture_to_the_sample() {
        // 97 BPM makes every tick a fractional number of samples: a start that is not on a tick
        // is made exact by skipping into the asset.
        let s = song(97.0);
        for start_us in [1_000_000, 1_234_567, 7_000_001, 12_345_678] {
            let f = film(start_us, 0, 60_000_000);
            let p = placement(&s, RATE, &f, 0, 48_000 * 60).unwrap();
            let wanted = frames_of(start_us, RATE);
            assert_eq!(asset_zero(&s, &p), wanted, "start {start_us}");
            // The clip starts at the earliest tick at or after the frame: never a tick late.
            assert!(i128::from(first_frame(&s, &p)) >= wanted);
            assert!(s.frame_at(p.start - 1, RATE) < wanted as u64);
            assert!(p.source_offset < 48_000, "less than a tick of skipping");
        }
    }

    #[test]
    fn the_lead_delays_or_advances_the_sound_against_the_picture() {
        let s = song(120.0);
        // The sound starts 500 ms after the picture: its first sample is 24 000 frames later.
        let late = placement(
            &s,
            RATE,
            &film(2_000_000, 0, 60_000_000),
            500_000,
            48_000 * 10,
        )
        .unwrap();
        assert_eq!(asset_zero(&s, &late), 96_000 + 24_000);
        // 40 ms before the picture: the part before the first frame is skipped (only that part of
        // the film plays), so the clip starts at the picture with the first 1 920 frames skipped.
        let early = placement(
            &s,
            RATE,
            &film(2_000_000, 0, 60_000_000),
            -40_000,
            48_000 * 10,
        )
        .unwrap();
        assert_eq!(asset_zero(&s, &early), 96_000 - 1_920);
        assert!(early.source_offset >= 1_920);
    }

    #[test]
    fn a_trim_shows_only_the_part_of_the_sound_inside_it() {
        let s = song(120.0);
        // The film plays from 10 s on its own clock to 20 s, starting at 4 s of the song.
        let f = film(4_000_000, 10_000_000, 20_000_000);
        let asset = 48_000 * 60;
        let p = placement(&s, RATE, &f, 0, asset).unwrap();
        // The sound's frame 0 is 10 s before the trim's start: 6 s into the song minus 10 s.
        assert_eq!(asset_zero(&s, &p), frames_of(-6_000_000, RATE));
        // The clip shows exactly the trimmed 10 s (whole seconds at 120 BPM are whole ticks).
        let first = first_frame(&s, &p);
        assert_eq!(first, 4 * 48_000);
        assert_eq!(p.source_offset, 10 * 48_000);
        let end = s.frame_at(p.start + p.length, RATE);
        assert_eq!(end, 14 * 48_000);
    }

    #[test]
    fn a_clip_stops_at_the_trim_and_never_plays_past_it() {
        let s = song(97.0);
        let f = film(0, 0, 10_123_457);
        let p = placement(&s, RATE, &f, 0, 48_000 * 60).unwrap();
        let end_frame = frames_of(10_123_457, RATE) as u64;
        let end = s.frame_at(p.start + p.length, RATE);
        assert!(end <= end_frame, "{end} <= {end_frame}");
        // The next tick would pass it.
        assert!(s.frame_at(p.start + p.length + 1, RATE) > end_frame);
    }

    #[test]
    fn a_short_track_covers_to_its_own_end() {
        let s = song(97.0);
        let p = placement(&s, RATE, &film(0, 0, 60_000_000), 0, 48_000 * 3 + 7).unwrap();
        let end = s.frame_at(p.start + p.length, RATE);
        assert!(end >= 48_000 * 3 + 7, "the clip covers the track");
        assert!(s.frame_at(p.start + p.length - 1, RATE) < 48_000 * 3 + 7);
    }

    #[test]
    fn a_film_before_the_song_start_skips_the_part_before_it() {
        let s = song(120.0);
        let p = placement(&s, RATE, &film(-1_000_000, 0, 60_000_000), 0, 48_000 * 60).unwrap();
        assert_eq!(p.start, 0);
        assert_eq!(p.source_offset, 48_000);
        assert_eq!(asset_zero(&s, &p), -48_000);
    }

    #[test]
    fn nothing_of_the_track_inside_the_film_gives_no_placement() {
        let s = song(120.0);
        // The track ends at 1 s, the film plays from 5 s of its own clock.
        assert_eq!(
            placement(&s, RATE, &film(0, 5_000_000, 20_000_000), 0, 48_000),
            None
        );
        // The track starts after the trim's end.
        assert_eq!(
            placement(&s, RATE, &film(0, 0, 2_000_000), 3_000_000, 48_000),
            None
        );
        // The whole film is before the song's start.
        assert_eq!(
            placement(&s, RATE, &film(-100_000_000, 0, 10_000_000), 0, 48_000 * 60),
            None
        );
    }

    #[test]
    fn a_tempo_change_moves_the_ticks_not_the_frames() {
        let mut s = song(120.0);
        s.tempo.push(TempoEvent::step(960 * 4, 77.0));
        let f = film(9_876_543, 0, 60_000_000);
        let p = placement(&s, RATE, &f, 250_000, 48_000 * 40).unwrap();
        assert_eq!(asset_zero(&s, &p), frames_of(9_876_543 + 250_000, RATE));
    }
}
