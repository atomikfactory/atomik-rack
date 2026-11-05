//! Tests of the MIDI file conversion: export, import and the round trips between them.

use super::*;
use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfEvent, SmfFile, SmfMessage, SmfTrack, read};
use atomik_model::song::{Marker, RangeMarker, empty_song};

const BAR: i64 = PPQ * 4;

fn vel(v: u8) -> f32 {
    f32::from(v) / 127.0
}

fn note(tick: i64, length: i64, key: u8, v: u8) -> Note {
    Note::new(tick, length, key, vel(v))
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn clip(start: i64, length: i64, notes: Vec<Note>, controllers: Vec<ControllerLane>) -> Clip {
    Clip {
        id: ClipId(1),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        name: None,
        color: None,
        grid: None,
        scale: None,
        controllers,
        param_lanes: Vec::new(),
        midi: Default::default(),
        content: ClipContent::Notes(notes.into()),
    }
}

fn track(device: &str, clips: Vec<Clip>) -> Track {
    Track {
        device: DeviceKey(device.to_owned()),
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        notes_to: None,
        direct_record: false,
        groove: None,
        frozen: None,
        lanes: vec![Lane::notes().with_clips(clips)],
    }
}

fn notes_of(c: &Clip) -> &[Note] {
    match &c.content {
        ClipContent::Notes(n) => n,
        _ => panic!("not a note clip"),
    }
}

/// A clip with odd ticks, every controller kind and a length that is not a whole bar.
fn busy_clip() -> Clip {
    clip(
        BAR,
        BAR + PPQ / 2,
        vec![
            note(0, PPQ, 60, 100),
            note(0, PPQ / 3, 64, 1),
            note(7, 3841, 67, 127),
            note(PPQ + 5, PPQ * 2, 36, 64),
        ],
        vec![
            ControllerLane::new(
                ControllerKind::PitchBend,
                vec![
                    point(0, 0.0, Curve::Step),
                    point(100, -1.0, Curve::Step),
                    point(5000, 8191.0 / 8192.0, Curve::Step),
                ],
            ),
            ControllerLane::new(
                ControllerKind::ChannelPressure,
                vec![point(10, vel(64), Curve::Step)],
            ),
            ControllerLane::new(
                ControllerKind::Cc(1),
                vec![
                    point(0, vel(0), Curve::Step),
                    point(77, vel(127), Curve::Step),
                ],
            ),
            ControllerLane::new(ControllerKind::Cc(74), vec![point(3, vel(64), Curve::Step)]),
            ControllerLane::new(
                ControllerKind::PolyPressure(60),
                vec![point(9, vel(33), Curve::Step)],
            ),
        ],
    )
}

#[test]
fn a_clip_survives_export_and_import_unchanged() {
    let song = empty_song(120.0);
    let original = busy_clip();
    let bytes = clip_file(
        &song,
        &original,
        "Busy",
        crate::midi_file::MidiResolution::Native,
    );
    let f = read(&bytes).unwrap();
    assert_eq!((f.format, f.ppq, f.tracks.len()), (0, 15_360, 1));
    let imported = read_song(&bytes).unwrap();
    assert_eq!(imported.tracks.len(), 1);
    assert_eq!(imported.tracks[0].name, "Busy");
    assert_eq!(imported.tracks[0].notes, notes_of(&original));
    assert_eq!(imported.tracks[0].controllers, original.controllers);
    let back = imported.merged().into_clip(original.start, &song);
    assert_eq!(
        back.length, original.length,
        "the clip length is the end of the track"
    );
    assert_eq!(
        back,
        Clip {
            id: ClipId(0),
            ..original
        }
    );
}

#[test]
fn a_clip_file_plays_what_the_clip_plays() {
    let song = empty_song(120.0);
    let mut c = clip(
        2 * BAR,
        2 * PPQ,
        vec![
            note(0, PPQ, 60, 100),
            note(PPQ, PPQ, 62, 100),
            note(3 * PPQ, PPQ, 64, 100),
        ],
        Vec::new(),
    );
    // Offset 0, a loop of one beat repeated over two beats: the second pass is a copy.
    c.loop_length = Some(PPQ);
    let got = read_song(&clip_file(
        &song,
        &c,
        "Loop",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap()
    .tracks
    .remove(0)
    .notes;
    let keys: Vec<(i64, u8)> = got.iter().map(|n| (n.tick, n.key)).collect();
    assert_eq!(keys, [(0, 60), (PPQ, 60)]);
    // A muted note and a muted clip flag are not exported; the clip itself still is.
    let mut m = clip(
        0,
        BAR,
        vec![note(0, PPQ, 60, 100), note(PPQ, PPQ, 62, 100)],
        Vec::new(),
    );
    if let ClipContent::Notes(n) = &mut m.content {
        n[1].muted = true;
    }
    m.muted = true;
    let got = read_song(&clip_file(
        &song,
        &m,
        "M",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap()
    .tracks
    .remove(0)
    .notes;
    assert_eq!(got.len(), 1);
}

#[test]
fn overlapping_notes_of_one_key_are_cut_so_each_off_pairs_with_its_on() {
    let song = empty_song(120.0);
    let c = clip(
        0,
        BAR,
        vec![
            note(0, 4 * PPQ, 60, 100),
            note(PPQ, PPQ, 60, 90),
            note(0, PPQ, 61, 80),
        ],
        Vec::new(),
    );
    let got = read_song(&clip_file(
        &song,
        &c,
        "O",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap()
    .tracks
    .remove(0)
    .notes;
    assert_eq!(
        got,
        vec![
            note(0, PPQ, 60, 100),
            note(0, PPQ, 61, 80),
            note(PPQ, PPQ, 60, 90),
        ]
    );
}

#[test]
fn a_linear_controller_segment_is_sampled_and_ends_on_its_value() {
    let song = empty_song(120.0);
    let lane = ControllerLane::new(
        ControllerKind::Cc(7),
        vec![point(0, 0.0, Curve::Linear), point(PPQ, 1.0, Curve::Step)],
    );
    let c = clip(0, BAR, vec![note(0, PPQ, 60, 100)], vec![lane]);
    let back = read_song(&clip_file(
        &song,
        &c,
        "R",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap()
    .tracks
    .remove(0);
    let pts = &back.controllers[0].points;
    assert!(pts.len() > 8 && pts.len() <= 128, "{} points", pts.len());
    assert_eq!((pts[0].tick, pts[0].value), (0, 0.0));
    let last = pts.last().unwrap();
    assert_eq!((last.tick, last.value), (PPQ, 1.0));
    assert!(
        pts.windows(2)
            .all(|w| w[0].tick < w[1].tick && w[0].value < w[1].value)
    );
    assert!(pts.iter().all(|p| p.curve == Curve::Step));
}

fn conducted_song() -> Song {
    let mut song = empty_song(120.0);
    song.tempo = vec![
        TempoEvent::step(0, 90.0),
        TempoEvent::step(2 * BAR, 140.0),
        TempoEvent::step(5 * BAR, 100.0),
    ];
    song.meter = vec![
        MeterEvent::new(1, 3, 4),
        MeterEvent::new(3, 7, 8),
        MeterEvent::new(5, 4, 4),
    ];
    song.markers = vec![
        Marker {
            id: 1,
            tick: 0,
            name: "Intro".into(),
            color: None,
        },
        Marker {
            id: 2,
            tick: 3 * BAR + 17,
            name: "Drop".into(),
            color: Some([1, 2, 3]),
        },
    ];
    song.tracks = vec![
        track(
            "lead",
            vec![clip(
                0,
                BAR,
                vec![note(0, PPQ, 60, 100), note(PPQ, PPQ, 62, 90)],
                Vec::new(),
            )],
        ),
        track(
            "bass",
            vec![clip(
                BAR,
                2 * BAR,
                vec![note(10, PPQ, 36, 127)],
                vec![ControllerLane::new(
                    ControllerKind::Cc(1),
                    vec![point(0, vel(5), Curve::Step)],
                )],
            )],
        ),
    ];
    song
}

#[test]
fn a_song_survives_export_and_import_unchanged() {
    let song = conducted_song();
    let name_of = |k: &DeviceKey| format!("dev {}", k.0);
    let bytes = song_file(
        &song,
        "My Song",
        &name_of,
        crate::midi_file::MidiResolution::Native,
    );
    let f = read(&bytes).unwrap();
    assert_eq!((f.format, f.tracks.len()), (1, 3));
    let imported = read_song(&bytes).unwrap();
    assert_eq!(imported.tempo.as_ref().unwrap(), &song.tempo);
    assert_eq!(imported.meter.as_ref().unwrap(), &song.meter);
    assert_eq!(
        imported.markers,
        vec![(0, "Intro".to_owned()), (3 * BAR + 17, "Drop".to_owned())]
    );
    let names: Vec<_> = imported.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["dev lead", "dev bass"]);
    let lead = &song.tracks[0].lanes[0].clips[0];
    let bass = &song.tracks[1].lanes[0].clips[0];
    // Tracks keep their song ticks.
    assert_eq!(imported.tracks[0].notes, notes_of(lead));
    let bass_notes: Vec<Note> = notes_of(bass)
        .iter()
        .map(|n| Note::new(n.tick + bass.start, n.length, n.key, n.velocity))
        .collect();
    assert_eq!(imported.tracks[1].notes, bass_notes);
    assert_eq!(imported.tracks[1].controllers[0].points[0].tick, bass.start);
    // The channels differ per track.
    let channels: Vec<u8> = f.tracks[1..]
        .iter()
        .filter_map(|t| {
            t.events.iter().find_map(|e| match e.message {
                SmfMessage::NoteOn { channel, .. } => Some(channel),
                _ => None,
            })
        })
        .collect();
    assert_eq!(channels, [0, 1]);
}

#[test]
fn a_track_file_is_type_zero_with_the_tempo_map_and_markers() {
    let song = conducted_song();
    let f = read(&track_file(
        &song,
        &song.tracks[1],
        "Bass",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap();
    assert_eq!((f.format, f.tracks.len()), (0, 1));
    let events = &f.tracks[0].events;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.message, SmfMessage::Tempo(428_571)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.message, SmfMessage::Marker(m) if m == "Drop"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.message, SmfMessage::NoteOn { key: 36, .. }))
    );
    let imported = read_song(&track_file(
        &song,
        &song.tracks[1],
        "Bass",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap();
    assert_eq!(imported.tracks[0].name, "Bass");
    assert_eq!(imported.tempo.unwrap().len(), 3);
}

#[test]
fn the_song_file_skips_audio_tracks() {
    let mut song = conducted_song();
    song.tracks.push(Track {
        lanes: vec![Lane::audio()],
        ..track("audio", Vec::new())
    });
    let f = read(&song_file(
        &song,
        "S",
        &|k| k.0.clone(),
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap();
    assert_eq!(f.tracks.len(), 3);
}

#[test]
fn a_tempo_ramp_becomes_a_staircase() {
    let mut song = empty_song(120.0);
    song.tempo = vec![
        TempoEvent {
            tick: 0,
            bpm: 60.0,
            ramp: true,
        },
        TempoEvent::step(BAR, 120.0),
    ];
    let c = clip(0, BAR, vec![note(0, PPQ, 60, 100)], Vec::new());
    let f = read(&track_file(
        &song,
        &track("t", vec![c]),
        "R",
        crate::midi_file::MidiResolution::Native,
    ))
    .unwrap();
    let micros: Vec<u32> = f.tracks[0]
        .events
        .iter()
        .filter_map(|e| match e.message {
            SmfMessage::Tempo(m) => Some(m),
            _ => None,
        })
        .collect();
    assert!(micros.len() > 10, "{} steps", micros.len());
    assert_eq!(micros[0], 1_000_000);
    assert_eq!(*micros.last().unwrap(), 500_000);
    assert!(micros.windows(2).all(|w| w[0] > w[1]));
    let ticks: Vec<u64> = f.tracks[0]
        .events
        .iter()
        .filter(|e| matches!(e.message, SmfMessage::Tempo(_)))
        .map(|e| e.tick)
        .collect();
    assert!(ticks.windows(2).all(|w| w[0] < w[1]));
}

fn ev(tick: u64, message: SmfMessage) -> SmfEvent {
    SmfEvent { tick, message }
}

fn on(channel: u8, key: u8, velocity: u8) -> SmfMessage {
    SmfMessage::NoteOn {
        channel,
        key,
        velocity,
    }
}

fn off(channel: u8, key: u8) -> SmfMessage {
    SmfMessage::NoteOff {
        channel,
        key,
        velocity: 64,
    }
}

fn smf(ppq: u16, tracks: Vec<Vec<SmfEvent>>) -> Vec<u8> {
    SmfFile {
        format: 1,
        ppq,
        tracks: tracks
            .into_iter()
            .map(|events| {
                let end_tick = events.last().map_or(0, |e| e.tick);
                SmfTrack { events, end_tick }
            })
            .collect(),
    }
    .to_bytes()
}

#[test]
fn a_type_zero_file_splits_by_channel_and_scales_the_division() {
    let bytes = smf(
        480,
        vec![vec![
            ev(0, on(0, 60, 100)),
            ev(0, on(9, 36, 90)),
            ev(480, off(0, 60)),
            ev(480, on(9, 36, 0)), // velocity 0 is an off
            ev(960, on(0, 62, 100)),
            ev(960, off(0, 62)),
        ]],
    );
    let s = read_song(&bytes).unwrap();
    let names: Vec<_> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Track 1 ch1", "Track 1 ch10"]);
    assert_eq!(
        s.tracks[0].notes,
        vec![note(0, PPQ, 60, 100), note(2 * PPQ, 1, 62, 100)]
    );
    assert_eq!(s.tracks[1].notes, vec![note(0, PPQ, 36, 90)]);
}

#[test]
fn notes_are_repaired_the_way_the_module_docs_say() {
    let bytes = smf(
        96,
        vec![vec![
            ev(0, on(0, 60, 100)),
            ev(96, on(0, 60, 80)), // re-struck: ends the first note here, hangs to the end
            ev(96, on(0, 61, 80)),
            ev(96, off(0, 61)), // zero length: one tick
            ev(192, on(0, 64, 70)),
            ev(192, on(0, 64, 71)), // same tick and key: replaces the first
            ev(300, off(0, 99)),    // an off with no on
            ev(400, on(0, 65, 60)), // hangs to the end of the track
            ev(500, SmfMessage::Marker("end".into())),
        ]],
    );
    let t = read_song(&bytes).unwrap().tracks.remove(0);
    let tick = |t: u64| t as i64 * PPQ / 96;
    assert_eq!(
        t.notes,
        vec![
            note(0, tick(96), 60, 100),
            Note::new(tick(96), tick(500) - tick(96), 60, vel(80)),
            Note::new(tick(96), 1, 61, vel(80)),
            Note::new(tick(192), tick(500) - tick(192), 64, vel(71)),
            Note::new(tick(400), tick(100), 65, vel(60)),
        ]
    );
}

#[test]
fn a_hanging_note_at_the_end_of_an_empty_tail_gets_a_sixteenth() {
    let bytes = smf(96, vec![vec![ev(0, on(0, 60, 100))]]);
    let t = read_song(&bytes).unwrap().tracks.remove(0);
    assert_eq!(t.notes, vec![note(0, PPQ / 4, 60, 100)]);
}

#[test]
fn controllers_become_step_points_and_noise_is_left_out() {
    let c = |controller, value| SmfMessage::Controller {
        channel: 0,
        controller,
        value,
    };
    let bytes = smf(
        96,
        vec![vec![
            ev(0, on(0, 60, 100)),
            ev(0, c(1, 10)),
            ev(0, c(1, 20)), // same tick: the last wins
            ev(10, c(1, 127)),
            ev(10, c(123, 0)), // all notes off
            ev(10, c(101, 0)), // RPN select
            ev(10, c(6, 12)),  // data entry
            ev(
                10,
                SmfMessage::Program {
                    channel: 0,
                    program: 5,
                },
            ),
            ev(
                20,
                SmfMessage::PitchBend {
                    channel: 0,
                    value: 0,
                },
            ),
            ev(
                30,
                SmfMessage::PitchBend {
                    channel: 0,
                    value: 8192,
                },
            ),
            ev(
                40,
                SmfMessage::ChannelPressure {
                    channel: 0,
                    pressure: 127,
                },
            ),
            ev(
                50,
                SmfMessage::PolyPressure {
                    channel: 0,
                    key: 60,
                    pressure: 0,
                },
            ),
            ev(96, off(0, 60)),
            // A second channel with controllers and no notes.
            ev(
                96,
                SmfMessage::Controller {
                    channel: 3,
                    controller: 7,
                    value: 9,
                },
            ),
        ]],
    );
    let s = read_song(&bytes).unwrap();
    assert_eq!(s.skipped, 1);
    let lanes = &s.tracks[0].controllers;
    let kinds: Vec<_> = lanes.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        [
            ControllerKind::PitchBend,
            ControllerKind::ChannelPressure,
            ControllerKind::Cc(1),
            ControllerKind::PolyPressure(60),
        ]
    );
    let t = |n: u64| n as i64 * PPQ / 96;
    assert_eq!(
        lanes[0].points,
        vec![
            point(t(20), -1.0, Curve::Step),
            point(t(30), 0.0, Curve::Step)
        ]
    );
    assert_eq!(
        lanes[2].points,
        vec![
            point(0, vel(20), Curve::Step),
            point(t(10), 1.0, Curve::Step)
        ]
    );
}

#[test]
fn tempo_meter_and_markers_are_read_from_any_track() {
    let bytes = smf(
        96,
        vec![
            vec![
                ev(0, SmfMessage::Tempo(500_000)),
                ev(
                    0,
                    SmfMessage::TimeSignature {
                        numerator: 4,
                        denominator_power: 2,
                    },
                ),
                ev(
                    96 * 4 + 10,
                    SmfMessage::TimeSignature {
                        numerator: 3,
                        denominator_power: 2,
                    },
                ),
                ev(
                    96 * 7,
                    SmfMessage::TimeSignature {
                        numerator: 5,
                        denominator_power: 5,
                    },
                ), // 5/32
                ev(
                    96 * 7,
                    SmfMessage::TimeSignature {
                        numerator: 0,
                        denominator_power: 2,
                    },
                ),
                ev(96 * 8, SmfMessage::Tempo(400_000)),
                ev(96 * 8, SmfMessage::Tempo(333_333)), // same tick: the last wins
                ev(96 * 9, SmfMessage::Marker("  Verse \u{0}1 ".into())),
                ev(96 * 9, SmfMessage::Marker("   ".into())),
                ev(96 * 10, SmfMessage::Marker("x".repeat(100))),
            ],
            vec![ev(0, on(0, 60, 100)), ev(10, off(0, 60))],
        ],
    );
    let s = read_song(&bytes).unwrap();
    // 4/4 at tick 0; 3/4 from the first bar line after tick 4 beats + 10, which is bar 3.
    assert_eq!(
        s.meter.unwrap(),
        vec![MeterEvent::new(1, 4, 4), MeterEvent::new(3, 3, 4)]
    );
    let tempo = s.tempo.unwrap();
    assert_eq!(tempo.len(), 2);
    assert!((tempo[0].bpm - 120.0).abs() < 1e-9);
    assert_eq!(tempo[1].tick, 8 * PPQ);
    assert!((tempo[1].bpm - 180.0).abs() < 0.01);
    assert_eq!(s.markers.len(), 2);
    assert_eq!(s.markers[0], (9 * PPQ, "Verse 1".to_owned()));
    assert_eq!(s.markers[1].1.chars().count(), 64);
    assert_eq!(s.tracks[0].name, "Track 2");
}

#[test]
fn a_tempo_map_that_starts_late_gets_the_default_tempo_first() {
    let bytes = smf(
        96,
        vec![vec![
            ev(0, on(0, 60, 100)),
            ev(96, SmfMessage::Tempo(250_000)),
            ev(96, off(0, 60)),
        ]],
    );
    let tempo = read_song(&bytes).unwrap().tempo.unwrap();
    assert_eq!(tempo.len(), 2);
    assert!((tempo[0].bpm - 120.0).abs() < 1e-9 && tempo[0].tick == 0);
    assert!((tempo[1].bpm - 240.0).abs() < 1e-9 && tempo[1].tick == PPQ);
}

#[test]
fn a_file_without_tempo_or_meter_leaves_them_alone() {
    let s = read_song(&smf(96, vec![vec![ev(0, on(0, 60, 1)), ev(5, off(0, 60))]])).unwrap();
    assert!(s.tempo.is_none() && s.meter.is_none() && s.markers.is_empty());
}

#[test]
fn files_without_notes_or_in_an_unsupported_form_are_clean_errors() {
    let only_tempo = smf(96, vec![vec![ev(0, SmfMessage::Tempo(500_000))]]);
    assert_eq!(read_song(&only_tempo), Err(MidiFileError::NoNotes));
    let only_controllers = smf(
        96,
        vec![vec![ev(
            0,
            SmfMessage::Controller {
                channel: 0,
                controller: 1,
                value: 3,
            },
        )]],
    );
    assert_eq!(read_song(&only_controllers), Err(MidiFileError::NoNotes));
    let err = read_song(b"definitely not midi").unwrap_err();
    assert!(err.to_string().starts_with("not a MIDI file"), "{err}");
    let mut smpte = smf(96, vec![vec![ev(0, on(0, 60, 1))]]);
    smpte[12] = 0xE7;
    let err = read_song(&smpte).unwrap_err();
    assert!(err.to_string().contains("SMPTE"), "{err}");
}

#[test]
fn huge_ticks_are_clamped_not_overflowed() {
    let bytes = smf(
        1,
        vec![vec![
            ev(0, on(0, 60, 100)),
            ev(u64::from(u32::MAX) * 4, off(0, 60)),
        ]],
    );
    let t = read_song(&bytes).unwrap().tracks.remove(0);
    assert!(t.notes[0].length <= MAX_TICK && t.end <= MAX_TICK);
    let clip = t.into_clip(0, &empty_song(120.0));
    assert!(clip.length >= 1 && clip.end() <= MAX_TICK);
}

#[test]
fn a_clip_takes_the_track_end_or_rounds_the_content_up_to_a_bar() {
    let song = empty_song(120.0);
    let make = |end| MidiTrack {
        name: String::new(),
        notes: vec![note(0, PPQ, 60, 100)],
        controllers: Vec::new(),
        midi: Default::default(),
        end,
    };
    assert_eq!(make(3 * PPQ).into_clip(0, &song).length, 3 * PPQ);
    assert_eq!(make(0).into_clip(0, &song).length, BAR);
    // Started mid-bar, the content ends at the next bar line of the song.
    assert_eq!(make(0).into_clip(PPQ, &song).length, 3 * PPQ);
}

#[test]
fn merging_tracks_joins_notes_and_controllers() {
    let lane =
        |tick, v| ControllerLane::new(ControllerKind::Cc(1), vec![point(tick, v, Curve::Step)]);
    let a = MidiTrack {
        name: "A".into(),
        notes: vec![note(0, PPQ, 60, 100)],
        controllers: vec![lane(0, 0.25), lane(0, 0.25)],
        midi: Default::default(),
        end: PPQ,
    };
    let b = MidiTrack {
        name: "B".into(),
        notes: vec![note(0, PPQ, 60, 50), note(PPQ, PPQ, 62, 50)],
        controllers: vec![lane(0, 0.75)],
        midi: Default::default(),
        end: 2 * PPQ,
    };
    let m = MidiSong {
        tracks: vec![a, b],
        tempo: None,
        meter: None,
        markers: Vec::new(),
        ranges: Vec::new(),
        skipped: 0,
    }
    .merged();
    assert_eq!(m.name, "A");
    assert_eq!(m.notes, vec![note(0, PPQ, 60, 100), note(PPQ, PPQ, 62, 50)]);
    assert_eq!(m.controllers.len(), 1);
    let pts = &m.controllers[0].points;
    assert_eq!(pts.len(), 1, "one point per tick");
    assert!((pts[0].value - 0.75).abs() < 1e-6, "the later track wins");
    assert_eq!(m.end, 2 * PPQ);
}

#[test]
fn midi_paths_are_recognised_in_any_case() {
    assert!(is_midi_path(Path::new("a/b.MID")));
    assert!(is_midi_path(Path::new("song.midi")));
    assert!(!is_midi_path(Path::new("song.wav")));
    assert!(!is_midi_path(Path::new("mid")));
}

// ------------------------------------------------------------ resolution, ranges, program, SysEx

fn events_of(bytes: &[u8]) -> Vec<SmfEvent> {
    read(bytes).unwrap().tracks.remove(0).events
}

fn note_ticks(events: &[SmfEvent]) -> Vec<(u64, bool, u8)> {
    events
        .iter()
        .filter_map(|e| match e.message {
            SmfMessage::NoteOn { key, .. } => Some((e.tick, true, key)),
            SmfMessage::NoteOff { key, .. } => Some((e.tick, false, key)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_resolution_is_a_choice_and_the_file_says_which() {
    let song = empty_song(120.0);
    let c = clip(0, BAR, vec![note(0, PPQ, 60, 100)], Vec::new());
    for (res, ppq, id) in [
        (MidiResolution::Native, 15_360, "native"),
        (MidiResolution::Ppq960, 960, "960"),
        (MidiResolution::Ppq480, 480, "480"),
    ] {
        let f = read(&clip_file(&song, &c, "C", res)).unwrap();
        assert_eq!(f.ppq, ppq);
        assert_eq!(res.ppq(), ppq);
        assert_eq!(MidiResolution::from_id(id), Some(res));
        assert_eq!(res.id(), id);
    }
    assert_eq!(MidiResolution::from_id("7680"), None);
    assert_eq!(MidiResolution::default(), MidiResolution::Native);
}

#[test]
fn a_coarser_resolution_rounds_ticks_and_a_round_trip_returns_grid_aligned_music() {
    let song = empty_song(120.0);
    // Whole 1/32 notes (480 ticks) survive 960 and 480 unchanged; a tick of the model between
    // two file ticks goes to the nearest.
    let aligned = clip(
        0,
        BAR,
        vec![
            note(0, PPQ, 60, 100),
            note(PPQ / 8, PPQ / 4, 62, 90),
            note(PPQ, PPQ * 2, 64, 80),
        ],
        vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![point(PPQ / 2, vel(64), Curve::Step)],
        )],
    );
    for res in [MidiResolution::Ppq960, MidiResolution::Ppq480] {
        let bytes = clip_file(&song, &aligned, "A", res);
        let back = read_song(&bytes).unwrap();
        assert_eq!(back.tracks[0].notes, notes_of(&aligned), "{res:?}");
        assert_eq!(back.tracks[0].controllers, aligned.controllers, "{res:?}");
    }
    // A note at model tick 24 is 1.5 file ticks at 960 (16 per tick): it lands on tick 2 (round
    // half up); its length of 40 model ticks (2.5) is tick 4, so the note lasts 2 file ticks.
    let odd = clip(0, BAR, vec![note(24, 16, 60, 100)], Vec::new());
    let events = events_of(&clip_file(&song, &odd, "O", MidiResolution::Ppq960));
    assert_eq!(note_ticks(&events), [(2, true, 60), (3, false, 60)]);
}

#[test]
fn a_note_shorter_than_a_file_tick_keeps_one_and_a_crowded_one_is_dropped() {
    let song = empty_song(120.0);
    // One model tick long: one file tick at 480.
    let tiny = clip(0, BAR, vec![note(0, 1, 60, 100)], Vec::new());
    let events = events_of(&clip_file(&song, &tiny, "T", MidiResolution::Ppq480));
    assert_eq!(note_ticks(&events), [(0, true, 60), (1, false, 60)]);
    // Two notes of one key 3 model ticks apart fall in the same 480 tick: the first has no room
    // and goes, the second stays. At the native resolution both are written.
    let crowded = clip(
        0,
        BAR,
        vec![note(0, 2, 60, 100), note(3, 100, 60, 90)],
        Vec::new(),
    );
    let at480 = events_of(&clip_file(&song, &crowded, "C", MidiResolution::Ppq480));
    assert_eq!(note_ticks(&at480), [(0, true, 60), (3, false, 60)]);
    let native = events_of(&clip_file(&song, &crowded, "C", MidiResolution::Native));
    assert_eq!(
        native
            .iter()
            .filter(|e| matches!(e.message, SmfMessage::NoteOn { .. }))
            .count(),
        2
    );
    // Every note-off still pairs with a note-on of its key, in order.
    for (res, notes) in [
        (MidiResolution::Ppq480, &at480),
        (MidiResolution::Native, &native),
    ] {
        let mut open = 0i32;
        for (_, on, _) in note_ticks(notes) {
            open += if on { 1 } else { -1 };
            assert!((0..=1).contains(&open), "{res:?}");
        }
        assert_eq!(open, 0, "{res:?}");
    }
}

#[test]
fn the_conductor_track_scales_tempo_meter_and_markers_too() {
    let mut song = empty_song(120.0);
    song.tempo = vec![TempoEvent::step(0, 100.0), TempoEvent::step(2 * BAR, 150.0)];
    song.markers = vec![Marker {
        id: 1,
        tick: 3 * BAR,
        name: "Drop".into(),
        color: None,
    }];
    song.tracks = vec![track(
        "a",
        vec![clip(0, 4 * BAR, vec![note(0, PPQ, 60, 90)], Vec::new())],
    )];
    let bytes = song_file(&song, "S", &|k| k.0.clone(), MidiResolution::Ppq480);
    let f = read(&bytes).unwrap();
    assert_eq!(f.ppq, 480);
    let tempo: Vec<u64> = f.tracks[0]
        .events
        .iter()
        .filter(|e| matches!(e.message, SmfMessage::Tempo(_)))
        .map(|e| e.tick)
        .collect();
    assert_eq!(tempo, [0, 2 * 4 * 480]);
    let marker = f.tracks[0]
        .events
        .iter()
        .find(|e| matches!(&e.message, SmfMessage::Marker(m) if m == "Drop"))
        .unwrap();
    assert_eq!(marker.tick, 3 * 4 * 480);
    // A file written at 480 reads back to the same song time.
    let back = read_song(&bytes).unwrap();
    assert_eq!(back.markers, [(3 * BAR, "Drop".to_owned())]);
    assert_eq!(back.tracks[0].notes, [note(0, PPQ, 60, 90)]);
}

#[test]
fn range_markers_are_written_as_a_start_and_an_end_marker() {
    let mut song = empty_song(120.0);
    song.ranges = vec![
        RangeMarker {
            id: 1,
            start: BAR,
            end: 3 * BAR,
            name: "Verse".into(),
            color: None,
        },
        RangeMarker {
            id: 2,
            start: 3 * BAR,
            end: 4 * BAR,
            name: "Chorus".into(),
            color: Some([1, 2, 3]),
        },
    ];
    song.tracks = vec![track(
        "a",
        vec![clip(0, 4 * BAR, vec![note(0, PPQ, 60, 90)], Vec::new())],
    )];
    for bytes in [
        song_file(&song, "S", &|k| k.0.clone(), MidiResolution::Native),
        track_file(&song, &song.tracks[0], "a", MidiResolution::Native),
    ] {
        let f = read(&bytes).unwrap();
        let markers: Vec<(u64, String)> = f.tracks[0]
            .events
            .iter()
            .filter_map(|e| match &e.message {
                SmfMessage::Marker(m) => Some((e.tick, m.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            markers,
            [
                (BAR as u64, "Verse".to_owned()),
                (3 * BAR as u64, "Verse (end)".to_owned()),
                (3 * BAR as u64, "Chorus".to_owned()),
                (4 * BAR as u64, "Chorus (end)".to_owned()),
            ]
        );
    }
    // A clip file has no song-level markers.
    let clip_bytes = clip_file(
        &song,
        &song.tracks[0].lanes[0].clips[0],
        "c",
        MidiResolution::Native,
    );
    assert!(
        !events_of(&clip_bytes)
            .iter()
            .any(|e| matches!(e.message, SmfMessage::Marker(_)))
    );
}

fn midi_clip() -> Clip {
    let mut c = clip(
        0,
        2 * BAR,
        vec![note(0, PPQ, 60, 100)],
        vec![
            ControllerLane::new(ControllerKind::Cc(0), vec![point(0, vel(1), Curve::Step)]),
            ControllerLane::new(ControllerKind::Cc(32), vec![point(0, vel(2), Curve::Step)]),
        ],
    );
    c.midi = MidiData {
        programs: vec![ProgramChange::new(0, 5), ProgramChange::new(BAR, 99)],
        sysex: vec![SysexEvent {
            tick: PPQ,
            data: vec![0x7E, 0x7F, 0x09, 0x01],
        }],
    }
    .into();
    c
}

#[test]
fn bank_selects_program_changes_and_sysex_survive_a_clip_round_trip() {
    let song = empty_song(120.0);
    let original = midi_clip();
    let bytes = clip_file(&song, &original, "M", MidiResolution::Native);
    let events = events_of(&bytes);
    // At tick 0 the bank select (CC 0, then CC 32) comes before the program change.
    let at_zero: Vec<&SmfMessage> = events
        .iter()
        .filter(|e| e.tick == 0)
        .map(|e| &e.message)
        .filter(|m| {
            matches!(
                m,
                SmfMessage::Controller { .. } | SmfMessage::Program { .. }
            )
        })
        .collect();
    assert_eq!(
        at_zero,
        [
            &SmfMessage::Controller {
                channel: 0,
                controller: 0,
                value: 1
            },
            &SmfMessage::Controller {
                channel: 0,
                controller: 32,
                value: 2
            },
            &SmfMessage::Program {
                channel: 0,
                program: 5
            },
        ]
    );
    assert!(
        events.iter().any(|e| e.tick == PPQ as u64
            && e.message == SmfMessage::SysEx(vec![0x7E, 0x7F, 0x09, 0x01]))
    );
    // Read back, the bank select that sat just before the program change joins it.
    let back = read_song(&bytes).unwrap();
    let t = &back.tracks[0];
    assert_eq!(
        t.midi.programs[0],
        ProgramChange {
            bank_msb: Some(1),
            bank_lsb: Some(2),
            ..ProgramChange::new(0, 5)
        }
    );
    assert_eq!(t.midi.programs[1], ProgramChange::new(BAR, 99));
    assert_eq!(t.midi.sysex, original.midi.sysex);
    assert!(t.controllers.is_empty(), "the bank select left the lanes");
}

fn banked_clip() -> Clip {
    let mut c = clip(0, 2 * BAR, vec![note(0, PPQ, 60, 100)], Vec::new());
    c.midi = MidiData {
        programs: vec![
            ProgramChange {
                bank_msb: Some(3),
                bank_lsb: Some(0),
                ..ProgramChange::new(0, 5)
            },
            ProgramChange {
                bank_msb: None,
                bank_lsb: Some(9),
                ..ProgramChange::new(BAR, 99)
            },
            ProgramChange::new(BAR + PPQ, 7),
        ],
        sysex: vec![SysexEvent {
            tick: PPQ,
            data: vec![0x7E, 0x7F, 0x09, 0x01],
        }],
    }
    .into();
    c
}

#[test]
fn a_bank_and_program_event_is_written_as_its_controllers_then_the_program_and_read_back_whole() {
    let song = empty_song(120.0);
    let original = banked_clip();
    let bytes = clip_file(&song, &original, "M", MidiResolution::Native);
    let events = events_of(&bytes);
    let messages: Vec<(u64, &SmfMessage)> = events
        .iter()
        .filter(|e| {
            matches!(
                e.message,
                SmfMessage::Controller { .. } | SmfMessage::Program { .. }
            )
        })
        .map(|e| (e.tick, &e.message))
        .collect();
    let cc = |controller, value| SmfMessage::Controller {
        channel: 0,
        controller,
        value,
    };
    let pc = |program| SmfMessage::Program {
        channel: 0,
        program,
    };
    assert_eq!(
        messages,
        [
            (0, &cc(0, 3)),
            (0, &cc(32, 0)),
            (0, &pc(5)),
            (BAR as u64, &cc(32, 9)),
            (BAR as u64, &pc(99)),
            ((BAR + PPQ) as u64, &pc(7)),
        ]
    );
    let back = read_song(&bytes).unwrap();
    assert_eq!(back.tracks[0].midi, *original.midi);
    assert!(back.tracks[0].controllers.is_empty());
}

#[test]
fn a_bank_select_that_is_not_just_before_a_program_change_stays_a_controller() {
    let cc = |t, controller, value| {
        ev(
            t,
            SmfMessage::Controller {
                channel: 0,
                controller,
                value,
            },
        )
    };
    let pc = |t, program| {
        ev(
            t,
            SmfMessage::Program {
                channel: 0,
                program,
            },
        )
    };
    let bytes = smf(
        96,
        vec![vec![
            ev(0, on(0, 60, 100)),
            cc(0, 0, 4),
            pc(0, 1),
            // A bank select with no program change at its tick, and a program change with no
            // bank select at its tick.
            cc(10, 32, 6),
            pc(20, 2),
            ev(96, off(0, 60)),
        ]],
    );
    let s = read_song(&bytes).unwrap();
    let t = &s.tracks[0];
    assert_eq!(t.midi.programs[0].bank_msb, Some(4));
    assert_eq!(
        t.midi.programs[1],
        ProgramChange::new(model_tick(20, 96), 2)
    );
    assert_eq!(t.controllers.len(), 1);
    assert_eq!(t.controllers[0].kind, ControllerKind::Cc(32));
}

#[test]
fn program_changes_and_sysex_follow_the_track_into_a_song_file() {
    let mut song = empty_song(120.0);
    song.tracks = vec![track("a", vec![banked_clip()])];
    let bytes = song_file(&song, "S", &|k| k.0.clone(), MidiResolution::Ppq960);
    let back = read_song(&bytes).unwrap();
    assert_eq!(back.tracks.len(), 1);
    assert_eq!(back.tracks[0].midi, *banked_clip().midi);
    // At 960 the ticks are whole file ticks and come back exactly.
    assert_eq!(back.merged().midi, *banked_clip().midi);
}

#[test]
fn import_keeps_the_last_program_of_a_tick_and_gives_a_sysex_only_track_a_clip_of_its_own() {
    let sysex = |t, b: u8| ev(t, SmfMessage::SysEx(vec![0x43, b]));
    let bytes = smf(
        96,
        vec![
            // A conductor track with SysEx and no notes: a clip without notes carries it.
            vec![sysex(0, 1), ev(0, SmfMessage::Tempo(500_000))],
            vec![
                ev(
                    0,
                    SmfMessage::Program {
                        channel: 0,
                        program: 3,
                    },
                ),
                ev(
                    0,
                    SmfMessage::Program {
                        channel: 0,
                        program: 8,
                    },
                ),
                ev(
                    0,
                    SmfMessage::Program {
                        channel: 1,
                        program: 40,
                    },
                ),
                sysex(48, 2),
                ev(0, on(0, 60, 100)),
                ev(96, off(0, 60)),
                ev(0, on(1, 62, 100)),
                ev(96, off(1, 62)),
                // A channel with a program change and no notes is left out.
                ev(
                    10,
                    SmfMessage::Program {
                        channel: 5,
                        program: 9,
                    },
                ),
            ],
        ],
    );
    let s = read_song(&bytes).unwrap();
    assert_eq!(s.tracks.len(), 3);
    let message = &s.tracks[0];
    assert!(message.notes.is_empty() && message.midi.programs.is_empty());
    assert_eq!(message.name, "Track 1");
    assert_eq!(message.midi.sysex.len(), 1);
    assert_eq!(s.tracks[1].midi.programs, [ProgramChange::new(0, 8)]);
    assert_eq!(s.tracks[2].midi.programs, [ProgramChange::new(0, 40)]);
    assert_eq!(
        s.tracks[1].midi.sysex.len(),
        1,
        "the first clip of the track carries it"
    );
    assert!(s.tracks[2].midi.sysex.is_empty());
    assert_eq!(s.skipped, 1);
    // Its clip covers the message and holds no notes.
    let clip = s.tracks[0].clone().into_clip(0, &empty_song(120.0));
    assert!(clip.content.is_notes() && clip.midi.sysex.len() == 1);
    // Merged into one clip: all programs, the later channel winning a shared tick.
    let m = s.merged();
    assert_eq!(m.midi.programs, [ProgramChange::new(0, 40)]);
    assert_eq!(m.midi.sysex.len(), 2);
}

#[test]
fn a_file_with_only_sysex_imports_as_a_clip_that_carries_it() {
    let bytes = smf(
        96,
        vec![vec![
            ev(0, SmfMessage::TrackName("Reset".into())),
            ev(48, SmfMessage::SysEx(vec![0x7E, 0x7F, 0x09, 0x01])),
        ]],
    );
    let s = read_song(&bytes).unwrap();
    assert_eq!(s.tracks.len(), 1);
    assert_eq!(s.tracks[0].name, "Reset");
    let clip = s.tracks[0].clone().into_clip(0, &empty_song(120.0));
    assert!(clip.length > PPQ / 2, "the clip reaches past the message");
    assert_eq!(clip.midi.sysex[0].data, [0x7E, 0x7F, 0x09, 0x01]);
}

#[test]
fn a_marker_pair_the_export_writes_is_read_back_as_a_range() {
    let marker = |t, name: &str| ev(t, SmfMessage::Marker(name.to_owned()));
    let bytes = smf(
        96,
        vec![vec![
            marker(0, "Intro"),
            marker(96, "Verse"),
            marker(192, "Verse (end)"),
            marker(192, "Chorus"),
            marker(288, "Chorus (end)"),
            // No start of this name: it stays a marker. A start with no end too.
            marker(300, "Bridge (end)"),
            marker(310, "Outro"),
            ev(0, on(0, 60, 100)),
            ev(10, off(0, 60)),
        ]],
    );
    let s = read_song(&bytes).unwrap();
    let ticks = |t: u64| t as i64 * PPQ / 96;
    assert_eq!(
        s.ranges,
        [
            (ticks(96), ticks(192), "Verse".to_owned()),
            (ticks(192), ticks(288), "Chorus".to_owned()),
        ]
    );
    let names: Vec<&str> = s.markers.iter().map(|m| m.1.as_str()).collect();
    assert_eq!(names, ["Intro", "Bridge (end)", "Outro"]);
}

#[test]
fn ranges_of_one_name_pair_nearest_start_first_and_a_song_round_trips_its_ranges() {
    let (markers, ranges) = pair_ranges(vec![
        (0, "A".into()),
        (10, "A".into()),
        (20, "A (end)".into()),
        (30, "A (end)".into()),
        (40, " (end)".into()),
    ]);
    assert_eq!(ranges, [(0, 30, "A".to_owned()), (10, 20, "A".to_owned())]);
    assert_eq!(markers, [(40, " (end)".to_owned())]);
    // The export writes ranges and the import reads them back.
    let mut song = empty_song(120.0);
    song.ranges = vec![RangeMarker {
        id: 1,
        start: BAR,
        end: 3 * BAR,
        name: "Verse".into(),
        color: None,
    }];
    song.tracks = vec![track(
        "a",
        vec![clip(0, 4 * BAR, vec![note(0, PPQ, 60, 90)], Vec::new())],
    )];
    let bytes = song_file(&song, "S", &|k| k.0.clone(), MidiResolution::Native);
    let back = read_song(&bytes).unwrap();
    assert_eq!(back.ranges, [(BAR, 3 * BAR, "Verse".to_owned())]);
    assert!(back.markers.is_empty());
}

#[test]
fn imported_data_stays_within_the_limits_of_a_clip() {
    let mut events = vec![ev(0, on(0, 60, 100)), ev(10, off(0, 60))];
    for i in 0..(MAX_PROGRAM_CHANGES as u64 + 50) {
        events.push(ev(
            i,
            SmfMessage::Program {
                channel: 0,
                program: (i % 128) as u8,
            },
        ));
    }
    for i in 0..(MAX_SYSEX_EVENTS as u64 + 10) {
        events.push(ev(i, SmfMessage::SysEx(vec![1, 2, 3])));
    }
    events.push(ev(5, SmfMessage::SysEx(vec![7; MAX_SYSEX_BYTES + 1])));
    events.sort_by_key(|e| e.tick);
    let s = read_song(&smf(96, vec![events])).unwrap();
    let midi = &s.tracks[0].midi;
    assert_eq!(midi.programs.len(), MAX_PROGRAM_CHANGES);
    assert_eq!(midi.sysex.len(), MAX_SYSEX_EVENTS);
    assert!(midi.sysex.iter().all(|m| m.data.len() <= MAX_SYSEX_BYTES));
    assert!(midi.check(MAX_TICK).is_ok());
}
