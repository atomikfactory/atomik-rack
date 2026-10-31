//! Tests of the MIDI file conversion: export, import and the round trips between them.

use super::*;
use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfEvent, SmfFile, SmfMessage, SmfTrack, read};
use atomik_model::song::{Marker, empty_song};

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
    let bytes = clip_file(&song, &original, "Busy");
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
    let got = read_song(&clip_file(&song, &c, "Loop"))
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
    let got = read_song(&clip_file(&song, &m, "M"))
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
    let got = read_song(&clip_file(&song, &c, "O"))
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
    let back = read_song(&clip_file(&song, &c, "R"))
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
    let bytes = song_file(&song, "My Song", &name_of);
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
    let f = read(&track_file(&song, &song.tracks[1], "Bass")).unwrap();
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
    let imported = read_song(&track_file(&song, &song.tracks[1], "Bass")).unwrap();
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
    let f = read(&song_file(&song, "S", &|k| k.0.clone())).unwrap();
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
    let f = read(&track_file(&song, &track("t", vec![c]), "R")).unwrap();
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
        end: PPQ,
    };
    let b = MidiTrack {
        name: "B".into(),
        notes: vec![note(0, PPQ, 60, 50), note(PPQ, PPQ, 62, 50)],
        controllers: vec![lane(0, 0.75)],
        end: 2 * PPQ,
    };
    let m = MidiSong {
        tracks: vec![a, b],
        tempo: None,
        meter: None,
        markers: Vec::new(),
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
