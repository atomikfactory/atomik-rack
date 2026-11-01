//! `Song::frame_clock` answers exactly what `Song::frame_at` does, with the tempo map compiled
//! once.
#![allow(missing_docs)]

use atomik_model::{TempoEvent, empty_song};

#[test]
fn the_clock_matches_frame_at_through_steps_and_ramps() {
    let mut song = empty_song(120.0);
    song.tempo = vec![
        TempoEvent::step(0, 120.0),
        TempoEvent {
            tick: 3840,
            bpm: 90.0,
            ramp: true,
        },
        TempoEvent::step(15_360, 175.5),
        TempoEvent {
            tick: 40_000,
            bpm: 60.0,
            ramp: true,
        },
        TempoEvent::step(70_000, 133.0),
    ];
    for rate in [44_100, 48_000, 96_000] {
        let clock = song.frame_clock(rate);
        for tick in (0..200_000)
            .step_by(997)
            .chain([0, 1, 3840, 15_360, 70_000])
        {
            assert_eq!(
                clock.at(tick),
                song.frame_at(tick, rate),
                "{tick} at {rate}"
            );
        }
    }
}
