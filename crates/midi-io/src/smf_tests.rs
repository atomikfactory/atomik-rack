//! Tests of the Standard MIDI File reader and writer.

use super::*;
use proptest::prelude::*;

fn header(format: u16, tracks: u16, division: u16) -> Vec<u8> {
    let mut v = b"MThd".to_vec();
    v.extend(6u32.to_be_bytes());
    v.extend(format.to_be_bytes());
    v.extend(tracks.to_be_bytes());
    v.extend(division.to_be_bytes());
    v
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend((body.len() as u32).to_be_bytes());
    v.extend(body);
    v
}

fn file(format: u16, division: u16, tracks: &[&[u8]]) -> Vec<u8> {
    let mut v = header(format, tracks.len() as u16, division);
    for t in tracks {
        v.extend(chunk(b"MTrk", t));
    }
    v
}

fn ev(tick: u64, message: SmfMessage) -> SmfEvent {
    SmfEvent { tick, message }
}

#[test]
fn reads_a_note_with_running_status_and_a_velocity_zero_off() {
    // note on 60, running-status note on 64, then note on 60 vel 0 and 64 vel 0.
    let track = [
        0x00, 0x90, 60, 100, 0x00, 64, 90, 0x60, 60, 0, 0x00, 64, 0, 0x00, 0xFF, 0x2F, 0x00,
    ];
    let f = read(&file(0, 96, &[&track])).unwrap();
    assert_eq!((f.format, f.ppq, f.tracks.len()), (0, 96, 1));
    let on = |tick, key, velocity| {
        ev(
            tick,
            SmfMessage::NoteOn {
                channel: 0,
                key,
                velocity,
            },
        )
    };
    assert_eq!(
        f.tracks[0].events,
        vec![on(0, 60, 100), on(0, 64, 90), on(96, 60, 0), on(96, 64, 0)]
    );
    assert_eq!(f.tracks[0].end_tick, 96);
}

#[test]
fn reads_meta_events_and_skips_sysex_and_unknown_meta() {
    let track = [
        0x00, 0xFF, 0x03, 0x04, b'L', b'e', b'a', b'd', // track name
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // tempo 500000
        0x00, 0xFF, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, // 3/4
        0x00, 0xF0, 0x03, 0x7E, 0x01, 0xF7, // sysex
        0x00, 0xFF, 0x7F, 0x02, 0x01, 0x02, // sequencer specific
        0x10, 0xFF, 0x06, 0x02, b'A', b'B', // marker
        0x00, 0xFF, 0x2F, 0x00,
    ];
    let f = read(&file(1, 480, &[&track])).unwrap();
    assert_eq!(
        f.tracks[0].events,
        vec![
            ev(0, SmfMessage::TrackName("Lead".into())),
            ev(0, SmfMessage::Tempo(500_000)),
            ev(
                0,
                SmfMessage::TimeSignature {
                    numerator: 3,
                    denominator_power: 2
                }
            ),
            ev(16, SmfMessage::Marker("AB".into())),
        ]
    );
}

#[test]
fn running_status_survives_sysex_and_meta() {
    let track = [
        0x00, 0x90, 60, 100, 0x00, 0xF0, 0x01, 0xF7, 0x00, 0xFF, 0x01, 0x00, 0x00, 62, 100,
    ];
    let f = read(&file(0, 96, &[&track])).unwrap();
    assert_eq!(f.tracks[0].events.len(), 2);
}

#[test]
fn reads_every_channel_message() {
    let track = [
        0x00, 0xA2, 60, 10, // poly pressure
        0x00, 0xB3, 7, 99, // controller
        0x00, 0xC4, 5, // program
        0x00, 0xD5, 33, // channel pressure
        0x00, 0xE6, 0x7F, 0x7F, // pitch bend max
    ];
    let f = read(&file(0, 96, &[&track])).unwrap();
    assert_eq!(
        f.tracks[0]
            .events
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        vec![
            &SmfMessage::PolyPressure {
                channel: 2,
                key: 60,
                pressure: 10
            },
            &SmfMessage::Controller {
                channel: 3,
                controller: 7,
                value: 99
            },
            &SmfMessage::Program {
                channel: 4,
                program: 5
            },
            &SmfMessage::ChannelPressure {
                channel: 5,
                pressure: 33
            },
            &SmfMessage::PitchBend {
                channel: 6,
                value: 16383
            },
        ]
    );
}

#[test]
fn skips_unknown_chunks_and_reads_a_riff_wrapped_file() {
    let mut bytes = header(0, 1, 96);
    bytes.extend(chunk(b"XFIH", &[1, 2, 3]));
    bytes.extend(chunk(b"MTrk", &[0x00, 0x90, 60, 64]));
    let f = read(&bytes).unwrap();
    assert_eq!(f.tracks.len(), 1);
    let mut riff = b"RIFF".to_vec();
    riff.extend((bytes.len() as u32 + 12).to_le_bytes());
    riff.extend(b"RMIDdata");
    riff.extend((bytes.len() as u32).to_le_bytes());
    riff.extend(&bytes);
    assert_eq!(read(&riff).unwrap(), f);
}

#[test]
fn a_truncated_track_is_read_up_to_the_cut() {
    let mut bytes = file(0, 96, &[&[0x00, 0x90, 60, 100, 0x10, 0x80, 60, 0]]);
    bytes.truncate(bytes.len() - 2); // cut inside the note-off
    let f = read(&bytes).unwrap();
    assert_eq!(f.tracks[0].events.len(), 1);
    // A chunk length beyond the end of the file reads to the end too.
    let mut long = file(0, 96, &[&[0x00, 0x90, 60, 100]]);
    long[18..22].copy_from_slice(&1000u32.to_be_bytes());
    assert_eq!(read(&long).unwrap().tracks[0].events.len(), 1);
}

#[test]
fn malformed_files_are_errors_not_panics() {
    assert!(matches!(read(&[]), Err(SmfError::NotMidi(_))));
    assert!(matches!(read(b"MThd"), Err(SmfError::NotMidi(_))));
    assert!(matches!(read(b"RIFF....RMID"), Err(SmfError::NotMidi(_))));
    assert!(matches!(
        read(&header(0, 0, 96)),
        Err(SmfError::NotMidi(m)) if m.contains("no track")
    ));
    assert!(matches!(
        read(&file(2, 96, &[&[]])),
        Err(SmfError::Unsupported(_))
    ));
    assert!(matches!(
        read(&file(7, 96, &[&[]])),
        Err(SmfError::NotMidi(_))
    ));
    assert!(matches!(
        read(&file(0, 0xE728, &[&[]])),
        Err(SmfError::Unsupported(m)) if m.contains("SMPTE")
    ));
    assert!(matches!(
        read(&file(0, 0, &[&[]])),
        Err(SmfError::NotMidi(_))
    ));
    // A data byte with no status before it.
    assert!(matches!(
        read(&file(0, 96, &[&[0x00, 0x3C, 0x40]])),
        Err(SmfError::BadEvent { track: 0, .. })
    ));
    // A data byte above 127 where a data byte is due.
    assert!(matches!(
        read(&file(0, 96, &[&[0x00, 0x90, 0x3C, 0x40, 0x00, 0x90, 0x3C]])).map(|f| f.tracks.len()),
        Ok(1)
    ));
    assert!(matches!(
        read(&file(0, 96, &[&[0x00, 0xB0, 0x07, 0x80, 0x00]])),
        Err(SmfError::BadEvent { .. })
    ));
    // A delta of five bytes.
    assert!(matches!(
        read(&file(
            0,
            96,
            &[&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x90, 60, 64]]
        )),
        Err(SmfError::BadEvent { .. })
    ));
}

#[test]
fn a_tempo_of_zero_is_dropped() {
    let track = [0x00, 0xFF, 0x51, 0x03, 0, 0, 0, 0x00, 0xFF, 0x2F, 0x00];
    assert!(
        read(&file(0, 96, &[&track])).unwrap().tracks[0]
            .events
            .is_empty()
    );
}

#[test]
fn writes_and_reads_back_every_message_with_running_status() {
    let track = SmfTrack {
        events: vec![
            ev(0, SmfMessage::TrackName("Piano".into())),
            ev(0, SmfMessage::Tempo(468_750)),
            ev(
                0,
                SmfMessage::TimeSignature {
                    numerator: 7,
                    denominator_power: 3,
                },
            ),
            ev(0, SmfMessage::Marker("Verse".into())),
            ev(
                0,
                SmfMessage::NoteOn {
                    channel: 1,
                    key: 60,
                    velocity: 100,
                },
            ),
            ev(
                0,
                SmfMessage::NoteOn {
                    channel: 1,
                    key: 64,
                    velocity: 90,
                },
            ),
            ev(
                15_360,
                SmfMessage::NoteOff {
                    channel: 1,
                    key: 60,
                    velocity: 0,
                },
            ),
            ev(
                15_360,
                SmfMessage::PitchBend {
                    channel: 1,
                    value: 8192,
                },
            ),
            ev(
                20_000,
                SmfMessage::Controller {
                    channel: 15,
                    controller: 74,
                    value: 127,
                },
            ),
            ev(
                20_001,
                SmfMessage::Program {
                    channel: 2,
                    program: 9,
                },
            ),
            ev(
                20_002,
                SmfMessage::ChannelPressure {
                    channel: 2,
                    pressure: 1,
                },
            ),
            ev(
                20_003,
                SmfMessage::PolyPressure {
                    channel: 2,
                    key: 3,
                    pressure: 4,
                },
            ),
        ],
        end_tick: 30_000,
    };
    let file = SmfFile {
        format: 1,
        ppq: 15_360,
        tracks: vec![track.clone(), SmfTrack::default()],
    };
    let bytes = file.to_bytes();
    assert_eq!(read(&bytes).unwrap(), file);
    // The second note-on shares the first one's status byte: 2 events cost 4 + 3 data bytes.
    let plain = bytes.len();
    let doubled = SmfFile {
        tracks: vec![SmfTrack {
            events: vec![
                ev(
                    0,
                    SmfMessage::NoteOn {
                        channel: 0,
                        key: 1,
                        velocity: 1,
                    },
                ),
                ev(
                    0,
                    SmfMessage::NoteOn {
                        channel: 0,
                        key: 2,
                        velocity: 1,
                    },
                ),
            ],
            end_tick: 0,
        }],
        ..file
    }
    .to_bytes();
    assert!(doubled.len() < plain);
    // MThd(14) + MTrk header(8) + 1 + 3 + 1 + 2 + 4 (end of track).
    assert_eq!(doubled.len(), 14 + 8 + 4 + 3 + 4);
}

#[test]
fn several_tracks_force_format_one_and_ppq_is_clamped() {
    let f = SmfFile {
        format: 0,
        ppq: 0,
        tracks: vec![SmfTrack::default(), SmfTrack::default()],
    };
    let r = read(&f.to_bytes()).unwrap();
    assert_eq!((r.format, r.ppq, r.tracks.len()), (1, 1, 2));
}

#[test]
fn big_deltas_use_four_bytes_and_clamp() {
    for v in [
        0u32,
        127,
        128,
        0x3FFF,
        0x4000,
        0x1F_FFFF,
        0x20_0000,
        0x0FFF_FFFF,
    ] {
        let mut out = Vec::new();
        write_vlq(&mut out, v);
        let mut c = Cursor {
            bytes: &out,
            pos: 0,
        };
        assert_eq!(read_vlq(&mut c), Ok(Some(v)));
    }
    let mut out = Vec::new();
    write_vlq(&mut out, u32::MAX);
    assert_eq!(out, [0xFF, 0xFF, 0xFF, 0x7F]);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = read(&bytes);
    }

    #[test]
    fn arbitrary_track_bytes_never_panic(body in proptest::collection::vec(any::<u8>(), 0..300),
                                         division in any::<u16>(), format in 0u16..3) {
        let _ = read(&file(format, division, &[&body]));
    }

    #[test]
    fn mutating_a_valid_file_never_panics(index in 0usize..80, value in any::<u8>(), cut in 0usize..80) {
        let track = [
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0x90, 60, 100, 0x60, 0x80, 60, 0,
            0x00, 0xE0, 1, 2, 0x00, 0xFF, 0x06, 0x01, b'x', 0x00, 0xFF, 0x2F, 0x00,
        ];
        let mut bytes = file(1, 96, &[&track]);
        let i = index % bytes.len();
        bytes[i] = value;
        bytes.truncate(bytes.len() - cut.min(bytes.len() - 1));
        let _ = read(&bytes);
    }

    #[test]
    fn written_files_read_back(notes in proptest::collection::vec((0u64..100_000, 0u8..16, 0u8..128, 1u8..128), 0..40)) {
        let mut events: Vec<SmfEvent> = notes
            .iter()
            .map(|&(tick, channel, key, velocity)| ev(tick, SmfMessage::NoteOn { channel, key, velocity }))
            .collect();
        events.sort_by_key(|e| e.tick);
        let end_tick = events.last().map_or(0, |e| e.tick);
        let f = SmfFile { format: 0, ppq: 960, tracks: vec![SmfTrack { events, end_tick }] };
        prop_assert_eq!(read(&f.to_bytes()).unwrap(), f);
    }
}
