//! Truncated, corrupted and oversized Standard MIDI Files never panic and never make the reader
//! keep more than its caps; they read or return an error.
#![allow(missing_docs)]

use atomik_midi_io::smf::{
    MAX_EVENTS, MAX_EVENTS_PER_TRACK, MAX_TRACKS, SmfError, SmfEvent, SmfFile, SmfMessage,
    SmfTrack, read,
};
use proptest::prelude::*;

fn ev(tick: u64, message: SmfMessage) -> SmfEvent {
    SmfEvent { tick, message }
}

/// A few shapes of real files: one note track, a tempo map with a conductor track and several
/// channels, and a dense track using every kept message kind.
fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let note = |tick, key, velocity| {
        ev(
            tick,
            SmfMessage::NoteOn {
                channel: 0,
                key,
                velocity,
            },
        )
    };
    let single = SmfFile {
        format: 0,
        ppq: 480,
        tracks: vec![SmfTrack {
            events: vec![
                ev(0, SmfMessage::TrackName("lead".into())),
                note(0, 60, 100),
                note(240, 60, 0),
                note(480, 64, 90),
                note(960, 64, 0),
            ],
            end_tick: 960,
        }],
    };
    let multi = SmfFile {
        format: 1,
        ppq: 960,
        tracks: vec![
            SmfTrack {
                events: vec![
                    ev(0, SmfMessage::Tempo(500_000)),
                    ev(
                        0,
                        SmfMessage::TimeSignature {
                            numerator: 3,
                            denominator_power: 2,
                        },
                    ),
                    ev(1920, SmfMessage::Marker("chorus".into())),
                    ev(3840, SmfMessage::Tempo(400_000)),
                ],
                end_tick: 3840,
            },
            SmfTrack {
                events: (0..16u64)
                    .flat_map(|i| {
                        [
                            note(i * 240, 48 + i as u8, 80),
                            note(i * 240 + 120, 48 + i as u8, 0),
                        ]
                    })
                    .collect(),
                end_tick: 3840,
            },
        ],
    };
    let dense = SmfFile {
        format: 0,
        ppq: 96,
        tracks: vec![SmfTrack {
            events: vec![
                ev(
                    0,
                    SmfMessage::Program {
                        channel: 2,
                        program: 5,
                    },
                ),
                ev(
                    0,
                    SmfMessage::Controller {
                        channel: 2,
                        controller: 7,
                        value: 100,
                    },
                ),
                ev(
                    10,
                    SmfMessage::PitchBend {
                        channel: 2,
                        value: 9000,
                    },
                ),
                ev(
                    20,
                    SmfMessage::PolyPressure {
                        channel: 2,
                        key: 60,
                        pressure: 33,
                    },
                ),
                ev(
                    30,
                    SmfMessage::ChannelPressure {
                        channel: 2,
                        pressure: 12,
                    },
                ),
                ev(
                    40,
                    SmfMessage::NoteOff {
                        channel: 2,
                        key: 60,
                        velocity: 64,
                    },
                ),
            ],
            end_tick: 50,
        }],
    };
    vec![
        ("single.mid", single.to_bytes()),
        ("multi.mid", multi.to_bytes()),
        ("dense.mid", dense.to_bytes()),
    ]
}

fn fixture(index: usize) -> Vec<u8> {
    let all = fixtures();
    all[index % all.len()].1.clone()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn truncated_files_never_panic(index in 0usize..3, frac in 0.0f64..1.0) {
        let bytes = fixture(index);
        let cut = (bytes.len() as f64 * frac) as usize;
        let _ = read(&bytes[..cut]);
    }

    #[test]
    fn corrupted_files_never_panic(
        index in 0usize..3,
        edits in proptest::collection::vec((0.0f64..1.0, any::<u8>()), 1..24),
    ) {
        let mut bytes = fixture(index);
        for (pos, value) in edits {
            let i = ((bytes.len() - 1) as f64 * pos) as usize;
            bytes[i] = value;
        }
        let _ = read(&bytes);
    }

    #[test]
    fn corrupted_headers_never_panic(
        index in 0usize..3,
        edits in proptest::collection::vec((0usize..40, any::<u8>()), 1..8),
    ) {
        let mut bytes = fixture(index);
        for (pos, value) in edits {
            let i = pos.min(bytes.len() - 1);
            bytes[i] = value;
        }
        let _ = read(&bytes);
    }

    #[test]
    fn inflated_chunk_and_event_lengths_never_panic(
        index in 0usize..3,
        at in 0usize..60,
        value in prop_oneof![Just(0xFFu8), Just(0x80u8), Just(0x7Fu8), any::<u8>()],
        repeat in 1usize..5,
    ) {
        // Length fields are the dangerous bytes: stamp runs of high values over the file.
        let mut bytes = fixture(index);
        for k in 0..repeat {
            let i = (at + k).min(bytes.len() - 1);
            bytes[i] = value;
        }
        let _ = read(&bytes);
    }

    #[test]
    fn arbitrary_bytes_behind_a_valid_header_never_panic(
        body in proptest::collection::vec(any::<u8>(), 0..2000),
    ) {
        let mut bytes = b"MThd\0\0\0\x06\0\x01\0\x01\x01\xE0MTrk".to_vec();
        bytes.extend((body.len() as u32).to_be_bytes());
        bytes.extend(body);
        let _ = read(&bytes);
    }
}

/// The committed copies of the fixtures (the seed corpus of the fuzz target). Regenerate with
/// `ATOMIK_UPDATE_FIXTURES=1 cargo test -p atomik-midi-io --test robustness`.
#[test]
fn the_committed_fixtures_are_the_generated_ones() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (name, bytes) in fixtures() {
        let path = dir.join(name);
        if std::env::var_os("ATOMIK_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &bytes).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
    }
}

#[test]
fn the_fixtures_read_and_every_truncation_of_them_is_handled() {
    for (name, bytes) in fixtures() {
        assert!(read(&bytes).is_ok(), "{name}");
        for cut in 0..bytes.len() {
            let _ = read(&bytes[..cut]);
        }
    }
}

/// A file with one track of `events` note events, three bytes each thanks to running status.
fn file_with_events(events: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(events * 3 + 4);
    body.extend_from_slice(&[0x00, 0x90, 60, 100]);
    for _ in 1..events {
        body.extend_from_slice(&[0x00, 60, 100]);
    }
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xE0MTrk".to_vec();
    bytes.extend((body.len() as u32).to_be_bytes());
    bytes.extend(body);
    bytes
}

#[test]
fn a_track_at_the_event_cap_reads_and_one_event_more_is_refused() {
    let at_cap = read(&file_with_events(MAX_EVENTS_PER_TRACK)).unwrap();
    assert_eq!(at_cap.tracks[0].events.len(), MAX_EVENTS_PER_TRACK);
    drop(at_cap);
    assert!(matches!(
        read(&file_with_events(MAX_EVENTS_PER_TRACK + 1)),
        Err(SmfError::Unsupported(m)) if m.contains("events")
    ));
}

#[test]
#[ignore = "builds about 8 million events (hundreds of MB); run by the scheduled job"]
fn the_events_of_all_tracks_together_are_capped() {
    let one = file_with_events(MAX_EVENTS_PER_TRACK);
    let track = &one[14..];
    let mut bytes = one[..14].to_vec();
    for _ in 0..=(MAX_EVENTS / MAX_EVENTS_PER_TRACK) {
        bytes.extend_from_slice(track);
    }
    assert!(matches!(read(&bytes), Err(SmfError::Unsupported(_))));
}

#[test]
fn more_tracks_than_the_cap_are_refused_without_keeping_them() {
    let mut bytes = b"MThd\0\0\0\x06\0\x01\0\x01\x01\xE0".to_vec();
    for _ in 0..=MAX_TRACKS {
        bytes.extend_from_slice(b"MTrk\0\0\0\x04\x00\xFF\x2F\x00");
    }
    assert!(matches!(read(&bytes), Err(SmfError::Unsupported(_))));
}

#[test]
fn a_chunk_that_claims_gigabytes_reads_only_what_is_there() {
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xE0MTrk\xFF\xFF\xFF\xFF".to_vec();
    bytes.extend_from_slice(&[0x00, 0x90, 60, 100, 0x00, 0x80, 60, 0]);
    let f = read(&bytes).unwrap();
    assert_eq!(f.tracks[0].events.len(), 2);
}
