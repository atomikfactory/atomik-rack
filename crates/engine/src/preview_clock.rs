//! The preview clock of a note player: what makes a player run while the song transport stands
//! still, so its notes can be heard without playing the track.
//!
//! A node with a clock is given a [`TimeInfo`] in which the transport is playing and the position
//! is the clock's own: it starts at tick 0 (the start of the player's pattern) and moves on by
//! one batch of ticks at the song's tempo per batch. Nothing else of the batch's time changes,
//! so the tempo, the time signature and the master tune are the song's. Taking the clock away
//! puts the player back on the stopped transport, which makes every player close the notes it
//! holds. The clock is a plain `Copy` value on the node, so starting, running and stopping it
//! never allocates.
//!
//! The clock never touches the song transport, the song position or any other device, and it
//! gives way when the song transport starts (see [`PreviewClock::time_for`]).

use atomik_core::{BATCH, TimeInfo};

/// The local transport of one previewing player.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct PreviewClock {
    /// Position in ticks at the first frame of the next batch.
    tick: f64,
}

impl PreviewClock {
    /// A clock at the start of the pattern.
    pub(crate) const fn new() -> Self {
        Self { tick: 0.0 }
    }

    /// The position in ticks the next batch starts at.
    #[cfg(test)]
    pub(crate) fn tick(&self) -> f64 {
        self.tick
    }

    /// The time the player sees for a batch whose song time is `song`: `None` when the song
    /// transport is playing (the song's own time is used and the clock should be dropped),
    /// else the song's time with the transport playing from the clock's position and no loop.
    pub(crate) fn time_for(&self, song: TimeInfo) -> Option<TimeInfo> {
        if song.playing {
            return None;
        }
        Some(TimeInfo {
            playing: true,
            tick: self.tick,
            loop_wrap: None,
            loop_ticks: None,
            ..song
        })
    }

    /// Moves the clock on by one batch at `ticks_per_frame`. A tempo that is not usable (zero,
    /// negative, infinite or NaN) leaves the clock where it is.
    pub(crate) fn advance(&mut self, ticks_per_frame: f64) {
        if ticks_per_frame.is_finite() && ticks_per_frame > 0.0 {
            self.tick += ticks_per_frame * BATCH as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::{SampleRate, Tempo};

    fn song(playing: bool) -> TimeInfo {
        let tempo = Tempo::new(120.0).unwrap();
        let mut t = TimeInfo::at(0, SampleRate::new(48_000.0).unwrap(), tempo, playing);
        // The song stands at 1000 ticks: a stopped playhead the preview must not follow.
        t.tick = 1000.0;
        t
    }

    #[test]
    fn a_new_clock_starts_at_the_pattern_start_whatever_the_song_position() {
        let clock = PreviewClock::new();
        let time = clock.time_for(song(false)).unwrap();
        assert!(time.playing);
        assert_eq!(time.tick, 0.0);
        // The tempo and the rest of the song's time are kept.
        assert_eq!(time.ticks_per_frame, song(false).ticks_per_frame);
        assert_eq!(time.tempo, song(false).tempo);
        assert_eq!(time.time_signature, (4, 4));
    }

    #[test]
    fn the_clock_moves_one_batch_of_ticks_per_batch_at_the_song_tempo() {
        let mut clock = PreviewClock::new();
        let tpf = song(false).ticks_per_frame;
        for n in 1..=10 {
            clock.advance(tpf);
            let want = tpf * (BATCH * n) as f64;
            assert!((clock.tick() - want).abs() < 1e-9, "batch {n}");
        }
        // A tempo change is followed from the next batch on, without a jump back.
        let before = clock.tick();
        clock.advance(tpf * 2.0);
        assert!((clock.tick() - before - tpf * 2.0 * BATCH as f64).abs() < 1e-9);
    }

    #[test]
    fn a_tempo_that_is_not_usable_leaves_the_clock_alone() {
        let mut clock = PreviewClock::new();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            clock.advance(bad);
            assert_eq!(clock.tick(), 0.0);
        }
    }

    #[test]
    fn the_loop_of_the_song_is_not_the_loop_of_the_preview() {
        let mut t = song(false);
        t.loop_ticks = Some((0.0, 960.0));
        t.loop_wrap = Some(atomik_core::LoopWrap {
            offset: 3,
            to_tick: 0.0,
        });
        let time = PreviewClock::new().time_for(t).unwrap();
        assert!(time.loop_ticks.is_none() && time.loop_wrap.is_none());
    }

    #[test]
    fn the_song_transport_playing_takes_over() {
        assert_eq!(PreviewClock::new().time_for(song(true)), None);
    }
}
