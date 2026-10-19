use super::*;
use crate::app_core::default_registry;
use atomik_model::{Cable, Command, Document, SocketRef, SongCommand, empty_doc};

fn key(s: &str) -> DeviceKey {
    DeviceKey(s.to_owned())
}

fn socket(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_owned(),
    }
}

/// A document with one audio track `audio-1`, `inputs` host inputs, and the track's default
/// input cables (in_1 -> in_l, in_2 -> in_r, clamped to the input count).
fn doc_with_track(inputs: u16) -> (Document, DeviceKey) {
    let registry = default_registry();
    let mut doc = Document::new(empty_doc());
    doc.apply(Command::SetInputChannels(inputs), &registry)
        .unwrap();
    doc.apply(
        Command::Song(SongCommand::AddAudioTrack { name: None }),
        &registry,
    )
    .unwrap();
    let track = doc.doc().song.tracks[0].device.clone();
    (doc, track)
}

fn reconnect(doc: &mut Document, track: &DeviceKey, in_l: Option<&str>, in_r: Option<&str>) {
    let registry = default_registry();
    for (sock, from) in [("in_l", in_l), ("in_r", in_r)] {
        let _ = doc.apply(
            Command::Disconnect {
                to: socket(&track.0, sock),
            },
            &registry,
        );
        if let Some(from) = from {
            doc.apply(
                Command::Connect(Cable {
                    from: socket("interface", from),
                    to: socket(&track.0, sock),
                }),
                &registry,
            )
            .unwrap();
        }
    }
}

#[test]
fn the_offset_is_the_sum_of_both_latencies_the_engine_fifo_and_the_user_offset() {
    assert_eq!(record_offset_frames(0, 0, 0), 64, "the engine FIFO alone");
    assert_eq!(record_offset_frames(192, 576, 0), 192 + 576 + 64);
    assert_eq!(record_offset_frames(192, 576, 100), 192 + 576 + 64 + 100);
    assert_eq!(
        record_offset_frames(100, 100, -264),
        0,
        "a negative user offset"
    );
    assert_eq!(
        record_offset_frames(0, 0, -1000),
        64 - 1000,
        "may go negative"
    );
    assert_eq!(ENGINE_FIFO_FRAMES, atomik_core::BATCH as i64);
}

#[test]
fn a_default_audio_track_records_a_stereo_pair() {
    let (doc, track) = doc_with_track(2);
    assert_eq!(
        track_input(doc.doc(), &track),
        Some(TrackInput::Stereo(0, 1))
    );
    assert_eq!(TrackInput::Stereo(0, 1).channels(), vec![0, 1]);
}

#[test]
fn one_host_input_gives_a_mono_track() {
    // With a single input both sockets are cabled from in_1.
    let (doc, track) = doc_with_track(1);
    assert_eq!(track_input(doc.doc(), &track), Some(TrackInput::Mono(0)));
    // Only in_l cabled from in_2.
    let (mut doc, track) = doc_with_track(2);
    reconnect(&mut doc, &track, Some("in_2"), None);
    assert_eq!(track_input(doc.doc(), &track), Some(TrackInput::Mono(1)));
    // Only in_r cabled.
    reconnect(&mut doc, &track, None, Some("in_2"));
    assert_eq!(track_input(doc.doc(), &track), Some(TrackInput::Mono(1)));
    assert_eq!(TrackInput::Mono(1).channels(), vec![1]);
}

#[test]
fn a_track_can_take_any_two_channels() {
    let (mut doc, track) = doc_with_track(4);
    reconnect(&mut doc, &track, Some("in_4"), Some("in_3"));
    assert_eq!(
        track_input(doc.doc(), &track),
        Some(TrackInput::Stereo(3, 2)),
        "left from channel 4, right from channel 3"
    );
}

#[test]
fn a_track_without_input_cables_has_no_input() {
    let (mut doc, track) = doc_with_track(2);
    reconnect(&mut doc, &track, None, None);
    assert_eq!(track_input(doc.doc(), &track), None);
    // No host inputs at all: the track was never cabled.
    let (doc, track) = doc_with_track(0);
    assert_eq!(track_input(doc.doc(), &track), None);
    assert_eq!(track_input(doc.doc(), &key("nope")), None);
}

#[test]
fn the_tap_covers_the_sorted_union_of_the_channels() {
    let plan = plan_tap(&[
        (key("a"), TrackInput::Stereo(3, 2)),
        (key("b"), TrackInput::Mono(2)),
        (key("c"), TrackInput::Stereo(0, 1)),
    ]);
    assert_eq!(plan.channels, vec![0, 1, 2, 3]);
    assert_eq!(
        plan.tracks[0].tap_index,
        vec![3, 2],
        "left from 3, right from 2"
    );
    assert_eq!(plan.tracks[1].tap_index, vec![2]);
    assert_eq!(plan.tracks[2].tap_index, vec![0, 1]);
    assert_eq!(plan_tap(&[]), TapPlan::default());
}

/// A linear transport: song frame `engine - start`, no loop.
fn linear(start: u64) -> impl Fn(u64) -> SongPosition {
    move |e| SongPosition {
        song_frame: e.saturating_sub(start),
        tick: atomik_core::Ticks(0),
        loop_count: 0,
    }
}

/// A transport that plays `[ls, le)` in a loop from song frame `ls` at engine frame `start`.
fn looping(start: u64, ls: u64, le: u64) -> impl Fn(u64) -> SongPosition {
    move |e| {
        let n = e.saturating_sub(start);
        SongPosition {
            song_frame: ls + n % (le - ls),
            tick: atomik_core::Ticks(0),
            loop_count: n / (le - ls),
        }
    }
}

#[test]
fn without_a_loop_the_take_is_one_segment_shifted_by_the_compensation() {
    let range = TakeRange {
        first_engine_frame: 1_000,
        frames: 5_000,
        compensation: 300,
        valid_from: 1_000,
    };
    // The transport started at engine frame 1 000 (song frame 0).
    let segments = split_laps(&range, &linear(1_000));
    // File frame i is engine frame 1 000 + i, compensated 700 + i: song frame i - 300, so the
    // first 300 frames (before the transport started) are dropped.
    assert_eq!(
        segments,
        vec![Segment {
            lap: 0,
            first: 300,
            frames: 4_700,
            song_frame: 0
        }]
    );
}

#[test]
fn a_negative_compensation_moves_the_audio_later() {
    let range = TakeRange {
        first_engine_frame: 1_000,
        frames: 1_000,
        compensation: -200,
        valid_from: 0,
    };
    let segments = split_laps(&range, &linear(1_000));
    assert_eq!(
        segments,
        vec![Segment {
            lap: 0,
            first: 0,
            frames: 1_000,
            song_frame: 200
        }]
    );
}

#[test]
fn frames_before_the_start_of_the_transport_are_dropped() {
    // The tap started 500 frames before the transport (engine frame 1 500).
    let range = TakeRange {
        first_engine_frame: 1_000,
        frames: 3_000,
        compensation: 0,
        valid_from: 1_500,
    };
    let segments = split_laps(&range, &linear(1_500));
    assert_eq!(
        segments,
        vec![Segment {
            lap: 0,
            first: 500,
            frames: 2_500,
            song_frame: 0
        }]
    );
}

#[test]
fn each_loop_pass_is_its_own_segment_and_the_compensation_moves_the_boundary() {
    // Loop [1 000, 3 000) of 2 000 frames, playing from song frame 1 000 at engine frame 10 000.
    let position = looping(10_000, 1_000, 3_000);
    let range = TakeRange {
        first_engine_frame: 10_000,
        frames: 6_500,
        compensation: 0,
        valid_from: 10_000,
    };
    let segments = split_laps(&range, &position);
    assert_eq!(
        segments,
        vec![
            Segment {
                lap: 0,
                first: 0,
                frames: 2_000,
                song_frame: 1_000
            },
            Segment {
                lap: 1,
                first: 2_000,
                frames: 2_000,
                song_frame: 1_000
            },
            Segment {
                lap: 2,
                first: 4_000,
                frames: 2_000,
                song_frame: 1_000
            },
            Segment {
                lap: 3,
                first: 6_000,
                frames: 500,
                song_frame: 1_000
            },
        ]
    );
    // With 120 frames of compensation the audio belongs 120 frames earlier on the timeline:
    // the pass boundary moves 120 frames later in the file, and the first 120 frames are lost.
    let range = TakeRange {
        compensation: 120,
        valid_from: 10_000,
        ..range
    };
    let segments = split_laps(&range, &position);
    assert_eq!(
        segments[0],
        Segment {
            lap: 0,
            first: 120,
            frames: 1_880 + 120,
            song_frame: 1_000
        }
    );
    assert_eq!(segments[1].first, 2_120);
    assert_eq!(segments[1].song_frame, 1_000);
    assert_eq!(segments.iter().map(|s| s.frames).sum::<u64>(), 6_500 - 120);
}

#[test]
fn laps_are_counted_from_the_first_kept_frame() {
    // The transport had already wrapped twice when the take started (loop count 2 at the
    // first frame): the first pass is still lap 0.
    let position = looping(0, 100, 600);
    let range = TakeRange {
        first_engine_frame: 1_000,
        frames: 1_200,
        compensation: 0,
        valid_from: 0,
    };
    let segments = split_laps(&range, &position);
    assert_eq!(segments[0].lap, 0);
    assert_eq!(segments[1].lap, 1);
    assert!(
        segments
            .windows(2)
            .all(|w| w[0].first + w[0].frames == w[1].first)
    );
    assert_eq!(segments.iter().map(|s| s.frames).sum::<u64>(), 1_200);
}

#[test]
fn an_empty_or_fully_dropped_take_has_no_segments() {
    let r = TakeRange {
        first_engine_frame: 0,
        frames: 0,
        compensation: 0,
        valid_from: 0,
    };
    assert!(split_laps(&r, &linear(0)).is_empty());
    let r = TakeRange {
        frames: 100,
        valid_from: 10_000,
        ..r
    };
    assert!(
        split_laps(&r, &linear(0)).is_empty(),
        "all before the start"
    );
}

/// 1.5625 frames per tick (120 BPM at 48 kHz).
fn frames_of_tick(tick: i64) -> u64 {
    (tick as f64 * 1.5625).round().max(0.0) as u64
}

#[test]
fn a_lap_starts_on_the_first_tick_at_or_after_its_first_frame() {
    // Frame 100 lies between tick 64 (frame 100) exactly: no offset.
    let p = place_lap(100, 5_000, &frames_of_tick).unwrap();
    assert_eq!((p.start_tick, p.source_offset, p.frames), (64, 0, 5_000));
    // Frame 101: tick 65 starts at frame 102 (101.56 rounds to 102): skip one frame.
    let p = place_lap(101, 5_000, &frames_of_tick).unwrap();
    assert_eq!(p.start_tick, 65);
    assert_eq!(p.source_offset, 1);
    assert_eq!(p.frames, 4_999);
    // Frame 0 is tick 0.
    let p = place_lap(0, 10, &frames_of_tick).unwrap();
    assert_eq!((p.start_tick, p.source_offset, p.frames), (0, 0, 10));
}

#[test]
fn the_placement_is_exact_for_every_start_frame() {
    for frame in (0..400u64).chain([48_000, 1_234_567, 99_999_999]) {
        let p = place_lap(frame, 1_000, &frames_of_tick).unwrap();
        let at = frames_of_tick(p.start_tick);
        assert_eq!(at, frame + p.source_offset, "frame {frame}");
        assert!(
            p.start_tick == 0 || frames_of_tick(p.start_tick - 1) < frame,
            "the first such tick: frame {frame}"
        );
        assert_eq!(p.frames, 1_000 - p.source_offset);
    }
}

#[test]
fn a_lap_shorter_than_the_gap_to_the_next_tick_is_dropped() {
    // Frame 101, one frame long: the next tick starts at frame 102.
    assert_eq!(place_lap(101, 1, &frames_of_tick), None);
    assert_eq!(place_lap(10, 0, &frames_of_tick), None);
}
