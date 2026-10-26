//! Recording conversion and clip creation through a fake transport timeline: a real
//! compiled snapshot and scripted `TransportState`s, as the engine publishes them.

use super::*;
use atomik_core::{Tempo, Ticks};
use atomik_device_sdk::SocketId;
use atomik_engine::{SongInput, SongSnapshot, SongTempo, compile_song};

const SR: f64 = 48_000.0;
const BAR: i64 = 4 * PPQ;
/// Engine frame at which the transport started (song frame 0).
const E0: u64 = 10_000;
const DEV: DeviceInstanceId = DeviceInstanceId(1);
const OTHER: DeviceInstanceId = DeviceInstanceId(2);

fn snapshot(bpm: f64) -> SongSnapshot {
    compile_song(&SongInput {
        tracks: vec![],
        tempo: vec![SongTempo {
            tick: Ticks(0),
            tempo: Tempo::new(bpm).unwrap(),
            ramp: false,
        }],
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    })
    .unwrap()
}

/// The state the engine publishes right after starting at tick 0, engine frame `E0`.
fn state(loop_range: Option<(i64, i64)>) -> TransportState {
    let (loop_start, loop_end) = loop_range.unwrap_or((0, 4 * BAR));
    TransportState {
        playing: true,
        tick: 0,
        engine_frame: E0,
        loop_count: 0,
        song_frame: 0,
        loop_start,
        loop_end,
        loop_enabled: loop_range.is_some(),
    }
}

/// Engine frame of song tick `tick` in loop lap `lap` (loop of `lap_ticks` from tick 0).
fn frame_at(snap: &SongSnapshot, tick: i64, lap: u64, lap_ticks: i64) -> u64 {
    E0 + lap * snap.song_frame(Ticks(lap_ticks), SR) + snap.song_frame(Ticks(tick), SR)
}

fn on(key: u8, frame: u64) -> RecordedNote {
    RecordedNote {
        device: DEV,
        kind: NoteKind::On {
            key,
            velocity: 0.75,
        },
        frame,
    }
}

fn off(key: u8, frame: u64) -> RecordedNote {
    RecordedNote {
        device: DEV,
        kind: NoteKind::Off { key },
        frame,
    }
}

fn take(loop_range: Option<(i64, i64)>) -> Take {
    Take::new(
        [(DEV, DeviceKey("bass".into()), 0)],
        loop_range,
        atomik_core::MeterTable::constant(4, 4),
    )
}

fn feed(t: &mut Take, snap: &SongSnapshot, st: &TransportState, events: &[RecordedNote]) {
    t.feed(events, st, |s, f| snap.position_at(s, f, SR));
}

fn stop(snap: &SongSnapshot, st: &TransportState, frame: u64) -> SongPosition {
    snap.position_at(st, frame, SR)
}

fn notes(clip: &Clip) -> &[Note] {
    match &clip.content {
        ClipContent::Notes(n) => n,
        ClipContent::Automation(_) | ClipContent::Audio(_) => panic!("not a note clip"),
    }
}

fn near(a: i64, b: i64) -> bool {
    (a - b).abs() <= 1
}

#[test]
fn ac_s5_notes_land_at_their_ticks_without_quantize() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let ticks = [0, PPQ + 37, 2 * PPQ + 5000, BAR + 3 * PPQ];
    let mut events = Vec::new();
    for (i, &tick) in ticks.iter().enumerate() {
        let key = 40 + i as u8;
        events.push(on(key, frame_at(&snap, tick, 0, 0)));
        events.push(off(key, frame_at(&snap, tick + PPQ / 2, 0, 0)));
    }
    feed(&mut t, &snap, &st, &events);
    let clips = t.finish(
        stop(&snap, &st, frame_at(&snap, 2 * BAR - 100, 0, 0)),
        InputQuantize::Off,
    );
    assert_eq!(clips.len(), 1);
    let (key, clip) = &clips[0];
    assert_eq!(key.as_str(), "bass");
    assert_eq!(
        (clip.start, clip.length),
        (0, 2 * BAR),
        "rounded out to bars"
    );
    assert_eq!(clip.offset, 0);
    let n = notes(clip);
    assert_eq!(n.len(), 4);
    for (note, &tick) in n.iter().zip(&ticks) {
        assert!(near(note.tick, tick), "{} vs {tick}", note.tick);
        assert!(near(note.length, PPQ / 2), "{}", note.length);
        assert!((note.velocity - 0.75).abs() < 1e-6);
    }
}

#[test]
fn ac_s5_input_quantize_snaps_starts_to_the_grid() {
    let snap = snapshot(97.0);
    let st = state(None);
    for (q, grid) in [
        (InputQuantize::Sixteenth, PPQ / 4),
        (InputQuantize::Eighth, PPQ / 2),
    ] {
        let mut t = take(None);
        let played = [3 * grid + 700, 5 * grid - 900, 9 * grid + grid / 2 - 10];
        let mut events = Vec::new();
        for (i, &tick) in played.iter().enumerate() {
            let key = 60 + i as u8;
            events.push(on(key, frame_at(&snap, tick, 0, 0)));
            events.push(off(key, frame_at(&snap, tick + 1000, 0, 0)));
        }
        feed(&mut t, &snap, &st, &events);
        let clips = t.finish(stop(&snap, &st, frame_at(&snap, BAR, 0, 0)), q);
        let n = notes(&clips[0].1);
        let starts: Vec<i64> = n.iter().map(|n| n.tick).collect();
        assert_eq!(starts, vec![3 * grid, 5 * grid, 9 * grid], "{q:?}");
        for note in n {
            assert!(near(note.length, 1000), "lengths are kept: {}", note.length);
        }
    }
}

#[test]
fn ac_s5_a_loop_wrap_starts_a_new_lap_clip_and_cuts_held_notes() {
    let snap = snapshot(120.0);
    let lap = 2 * BAR;
    let st = state(Some((0, lap)));
    let mut t = take(Some((0, lap)));
    let events = [
        on(48, frame_at(&snap, PPQ, 0, lap)),
        off(48, frame_at(&snap, 2 * PPQ, 0, lap)),
        // Held across the wrap: cut at the loop end, its late off is ignored.
        on(50, frame_at(&snap, lap - PPQ, 0, lap)),
        off(50, frame_at(&snap, PPQ / 2, 1, lap)),
        // Second pass.
        on(52, frame_at(&snap, 3 * PPQ + 11, 1, lap)),
        off(52, frame_at(&snap, 4 * PPQ, 1, lap)),
    ];
    feed(&mut t, &snap, &st, &events);
    let end = stop(&snap, &st, frame_at(&snap, BAR + 10, 1, lap));
    assert_eq!(end.loop_count, 1);
    let clips = t.finish(end, InputQuantize::Off);
    assert_eq!(clips.len(), 2, "one clip per pass");
    let (first, second) = (&clips[0].1, &clips[1].1);
    assert_eq!((first.start, first.length), (0, lap));
    assert_eq!((second.start, second.length), (0, 2 * BAR));
    let a = notes(first);
    assert_eq!(a.len(), 2);
    assert!(near(a[0].tick, PPQ) && near(a[0].length, PPQ));
    assert!(near(a[1].tick, lap - PPQ));
    assert!(
        near(a[1].length, PPQ),
        "cut at the loop end: {}",
        a[1].length
    );
    let b = notes(second);
    assert_eq!(b.len(), 1);
    assert!(near(b[0].tick, 3 * PPQ + 11) && near(b[0].length, PPQ - 11));
}

#[test]
fn notes_held_at_stop_are_closed_at_the_stop_position() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    feed(
        &mut t,
        &snap,
        &st,
        &[on(60, frame_at(&snap, BAR + PPQ, 0, 0))],
    );
    let clips = t.finish(
        stop(&snap, &st, frame_at(&snap, BAR + 3 * PPQ, 0, 0)),
        InputQuantize::Off,
    );
    let n = notes(&clips[0].1);
    assert!(near(n[0].length, 2 * PPQ), "{}", n[0].length);
    assert_eq!((clips[0].1.start, clips[0].1.length), (0, 2 * BAR));
}

#[test]
fn a_take_starting_mid_song_starts_its_clip_on_the_bar() {
    let snap = snapshot(120.0);
    let mut st = state(None);
    st.tick = 3 * BAR + PPQ;
    st.song_frame = snap.song_frame(Ticks(st.tick), SR);
    let mut t = take(None);
    let f = |tick: i64| E0 + snap.song_frame(Ticks(tick), SR) - st.song_frame;
    feed(
        &mut t,
        &snap,
        &st,
        &[on(60, f(3 * BAR + 2 * PPQ)), off(60, f(3 * BAR + 3 * PPQ))],
    );
    let clips = t.finish(stop(&snap, &st, f(4 * BAR + PPQ)), InputQuantize::Off);
    let clip = &clips[0].1;
    assert_eq!((clip.start, clip.length), (3 * BAR, 2 * BAR));
    assert!(near(notes(clip)[0].tick, 2 * PPQ));
}

#[test]
fn events_before_the_engine_plays_wait_for_the_first_playing_state() {
    let snap = snapshot(120.0);
    let mut t = take(None);
    let stopped = TransportState::default();
    feed(
        &mut t,
        &snap,
        &stopped,
        &[on(60, frame_at(&snap, PPQ, 0, 0))],
    );
    assert_eq!(t.origin(), None);
    let st = state(None);
    feed(
        &mut t,
        &snap,
        &st,
        &[off(60, frame_at(&snap, 2 * PPQ, 0, 0))],
    );
    assert_eq!(t.origin(), Some((0, 0)));
    let clips = t.finish(
        stop(&snap, &st, frame_at(&snap, BAR, 0, 0)),
        InputQuantize::Off,
    );
    let n = notes(&clips[0].1);
    assert!(near(n[0].tick, PPQ) && near(n[0].length, PPQ));
}

#[test]
fn unarmed_devices_retriggers_and_quantize_duplicates_are_handled() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let mut foreign = on(70, frame_at(&snap, 0, 0, 0));
    foreign.device = OTHER;
    feed(
        &mut t,
        &snap,
        &st,
        &[
            foreign,
            on(60, frame_at(&snap, 0, 0, 0)),
            // Retrigger without an off: the first note ends here.
            on(60, frame_at(&snap, 100, 0, 0)),
            off(60, frame_at(&snap, PPQ, 0, 0)),
        ],
    );
    let clips = t.finish(
        stop(&snap, &st, frame_at(&snap, BAR, 0, 0)),
        InputQuantize::Off,
    );
    assert_eq!(clips.len(), 1, "only the armed device");
    let n = notes(&clips[0].1);
    assert_eq!(n.len(), 2);
    assert!(near(n[0].length, 100));
    // Quantized to the same 1/8 start: one note survives, the list stays valid.
    let mut t = take(None);
    feed(
        &mut t,
        &snap,
        &st,
        &[
            on(60, frame_at(&snap, 0, 0, 0)),
            off(60, frame_at(&snap, 50, 0, 0)),
            on(60, frame_at(&snap, 100, 0, 0)),
            off(60, frame_at(&snap, 200, 0, 0)),
        ],
    );
    let clips = t.finish(
        stop(&snap, &st, frame_at(&snap, BAR, 0, 0)),
        InputQuantize::Eighth,
    );
    assert_eq!(notes(&clips[0].1).len(), 1);
}

#[test]
fn a_take_without_notes_makes_no_clip() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    feed(&mut t, &snap, &st, &[]);
    assert!(
        t.finish(stop(&snap, &st, E0 + 1000), InputQuantize::Off)
            .is_empty()
    );
}

fn ctl(kind: NoteKind, frame: u64) -> RecordedNote {
    RecordedNote {
        device: DEV,
        kind,
        frame,
    }
}

fn cc(controller: u8, value: f32) -> NoteKind {
    NoteKind::Cc { controller, value }
}

fn lane(clip: &Clip, kind: ControllerKind) -> &ControllerLane {
    clip.controllers
        .iter()
        .find(|l| l.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} lane in {:?}", clip.controllers))
}

#[test]
fn controller_events_become_step_lanes_next_to_the_notes() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let at = |tick| frame_at(&snap, tick, 0, 0);
    let events = [
        on(60, at(PPQ)),
        ctl(NoteKind::PitchBend { value: -0.5 }, at(PPQ)),
        ctl(cc(1, 0.25), at(PPQ + 5000)),
        ctl(cc(1, 0.5), at(2 * PPQ)),
        ctl(NoteKind::ChannelPressure { value: 1.0 }, at(2 * PPQ)),
        ctl(
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
            at(2 * PPQ),
        ),
        off(60, at(3 * PPQ)),
    ];
    feed(&mut t, &snap, &st, &events);
    let clips = t.finish(stop(&snap, &st, at(BAR - 100)), InputQuantize::Off);
    assert_eq!(clips.len(), 1);
    let clip = &clips[0].1;
    let kinds: Vec<_> = clip.controllers.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        [
            ControllerKind::PitchBend,
            ControllerKind::ChannelPressure,
            ControllerKind::Cc(1),
            ControllerKind::PolyPressure(60)
        ],
        "sorted by kind"
    );
    let cc1 = lane(clip, ControllerKind::Cc(1));
    assert_eq!(cc1.points.len(), 2);
    assert!(near(cc1.points[0].tick, PPQ + 5000));
    assert_eq!(cc1.points[0].value, 0.25);
    assert!(near(cc1.points[1].tick, 2 * PPQ));
    assert_eq!(cc1.points[1].value, 0.5);
    assert!(cc1.points.iter().all(|p| p.curve == Curve::Step));
    let bend = lane(clip, ControllerKind::PitchBend);
    assert!(near(bend.points[0].tick, PPQ));
    assert_eq!(bend.points[0].value, -0.5);
    assert_eq!(notes(clip).len(), 1);
}

#[test]
fn a_burst_keeps_its_last_value_and_repeats_are_dropped() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let at = |tick| frame_at(&snap, tick, 0, 0);
    // A sweep of ten events 10 ticks apart (well inside the coalescing window), then the same
    // value again later, then a new one.
    let mut events: Vec<RecordedNote> = (0..10)
        .map(|i| ctl(cc(11, i as f32 / 10.0), at(PPQ + i * 10)))
        .collect();
    events.push(ctl(cc(11, 0.9), at(2 * PPQ)));
    events.push(ctl(cc(11, 0.2), at(3 * PPQ)));
    feed(&mut t, &snap, &st, &events);
    let clips = t.finish(stop(&snap, &st, at(BAR - 100)), InputQuantize::Off);
    let pts = &lane(&clips[0].1, ControllerKind::Cc(11)).points;
    let values: Vec<f32> = pts.iter().map(|p| p.value).collect();
    assert_eq!(
        values,
        [0.9, 0.2],
        "the burst ends on 0.9; the repeat is dropped"
    );
    assert!(near(pts[0].tick, PPQ) && near(pts[1].tick, 3 * PPQ));
}

#[test]
fn a_take_with_only_controllers_makes_a_clip_without_notes() {
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let at = |tick| frame_at(&snap, tick, 0, 0);
    feed(&mut t, &snap, &st, &[ctl(cc(64, 1.0), at(BAR + PPQ))]);
    let clips = t.finish(stop(&snap, &st, at(BAR + 2 * PPQ)), InputQuantize::Off);
    assert_eq!(clips.len(), 1);
    let clip = &clips[0].1;
    assert!(notes(clip).is_empty());
    assert_eq!((clip.start, clip.length), (0, 2 * BAR));
    assert_eq!(lane(clip, ControllerKind::Cc(64)).points.len(), 1);
}

#[test]
fn controllers_follow_the_loop_laps_and_other_devices_are_ignored() {
    let snap = snapshot(120.0);
    let lap = 2 * BAR;
    let st = state(Some((0, lap)));
    let mut t = take(Some((0, lap)));
    let events = [
        ctl(cc(1, 0.5), frame_at(&snap, PPQ, 0, lap)),
        ctl(cc(1, 0.75), frame_at(&snap, 2 * PPQ, 1, lap)),
        RecordedNote {
            device: OTHER,
            kind: cc(1, 1.0),
            frame: frame_at(&snap, 2 * PPQ, 1, lap),
        },
    ];
    feed(&mut t, &snap, &st, &events);
    let end = stop(&snap, &st, frame_at(&snap, BAR, 1, lap));
    let clips = t.finish(end, InputQuantize::Off);
    assert_eq!(clips.len(), 2, "one clip per pass");
    let v = |i: usize| lane(&clips[i].1, ControllerKind::Cc(1)).points[0].value;
    assert_eq!((v(0), v(1)), (0.5, 0.75));
}

#[test]
fn recorded_clips_pass_the_models_validation() {
    // Out-of-range values never reach the clip: they are clamped on capture.
    let snap = snapshot(120.0);
    let st = state(None);
    let mut t = take(None);
    let at = |tick| frame_at(&snap, tick, 0, 0);
    let events = [
        ctl(NoteKind::PitchBend { value: 3.0 }, at(PPQ)),
        ctl(cc(7, -2.0), at(2 * PPQ)),
    ];
    feed(&mut t, &snap, &st, &events);
    let clips = t.finish(stop(&snap, &st, at(BAR)), InputQuantize::Off);
    let clip = &clips[0].1;
    assert_eq!(lane(clip, ControllerKind::PitchBend).points[0].value, 1.0);
    assert_eq!(lane(clip, ControllerKind::Cc(7)).points[0].value, 0.0);
}

fn scheduled(device: u32, kind: NoteKind, frame: u64) -> ScheduledNote {
    ScheduledNote {
        frame,
        device: DeviceInstanceId(device),
        socket: SocketId(0),
        kind,
    }
}

#[test]
fn the_tap_keeps_armed_devices_only_and_never_grows() {
    let tap = RecordTap::new();
    let on60 = NoteKind::On {
        key: 60,
        velocity: 1.0,
    };
    tap.offer(&scheduled(1, on60, 5));
    let mut out = Vec::new();
    tap.drain(&mut out);
    assert!(out.is_empty(), "disarmed");
    tap.set_armed(&[DeviceInstanceId(1)]);
    tap.offer(&scheduled(1, on60, 5));
    tap.offer(&scheduled(2, on60, 6));
    tap.offer(&scheduled(1, NoteKind::AllOff, 7));
    tap.drain(&mut out);
    assert_eq!(
        out,
        vec![RecordedNote {
            device: DeviceInstanceId(1),
            kind: on60,
            frame: 5
        }]
    );
    for i in 0..TAP_CAPACITY + 10 {
        tap.offer(&scheduled(1, on60, i as u64));
    }
    assert_eq!(tap.dropped(), 10);
    tap.set_armed(&[]);
    out.clear();
    tap.drain(&mut out);
    assert!(out.is_empty(), "disarming discards");
}

#[test]
fn quantize_settings_have_labels_and_grids() {
    assert_eq!(InputQuantize::default(), InputQuantize::Off);
    assert_eq!(InputQuantize::Off.grid(), None);
    assert_eq!(InputQuantize::Eighth.grid(), Some(PPQ / 2));
    assert_eq!(InputQuantize::Sixteenth.grid(), Some(PPQ / 4));
    let labels: Vec<_> = InputQuantize::ALL.iter().map(|q| q.label()).collect();
    assert_eq!(labels, ["Off", "1/8", "1/16"]);
    assert_eq!(
        serde_json::to_string(&InputQuantize::Sixteenth).unwrap(),
        "\"1/16\""
    );
}

#[test]
fn new_keeps_the_note_lane_of_each_armed_track() {
    let t = Take::new(
        [
            (DEV, DeviceKey("bass".into()), 2),
            (DeviceInstanceId(9), DeviceKey("lead".into()), 0),
        ],
        None,
        atomik_core::MeterTable::constant(4, 4),
    );
    assert_eq!(t.lanes().get(&DeviceKey("bass".into())), Some(&2));
    assert_eq!(t.lanes().get(&DeviceKey("lead".into())), Some(&0));
    assert_eq!(t.devices().len(), 2);
}
