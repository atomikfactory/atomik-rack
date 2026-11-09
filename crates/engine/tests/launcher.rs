//! The clip launcher in the engine: launch quantize to a bar through a tempo ramp, looping,
//! scene launches, stops that release their notes, a slot that overrides the arrangement until
//! it stops, requests that wait for the transport, and snapshot swaps. Every render is
//! allocation-guarded (the launcher's audio-thread path allocates nothing).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{
    DeviceInstanceId, LaunchCommand, LaunchGrid, LaunchSlotInput, LaunchState, LaunchTrackInput,
    SongInput, SongNote, SongSnapshot, TransportCommand, compile_song,
};
use atomik_testkit::fakes::NoteLog;
use common::song::*;

const BAR: i64 = 4 * PPQ;
/// One bar at 120 BPM and 48 kHz.
const BAR_FRAMES: u64 = 96_000;

fn slot(length: i64, notes: Vec<SongNote>) -> Option<LaunchSlotInput> {
    Some(LaunchSlotInput {
        length,
        notes,
        controllers: Vec::new(),
        expression: Vec::new(),
    })
}

fn launch_track(device: DeviceInstanceId, slots: Vec<Option<LaunchSlotInput>>) -> LaunchTrackInput {
    LaunchTrackInput {
        device,
        notes_socket: NoteLog::NOTES_IN,
        slots,
    }
}

fn snapshot(input: &SongInput, launch: &[LaunchTrackInput]) -> SongSnapshot {
    compile_song(input).unwrap().with_launch(launch).unwrap()
}

fn go(slot: u16, device: DeviceInstanceId, grid: LaunchGrid) -> TransportCommand {
    TransportCommand::Launch(LaunchCommand::Launch { device, slot, grid })
}

fn stop(device: DeviceInstanceId, grid: LaunchGrid) -> TransportCommand {
    TransportCommand::Launch(LaunchCommand::Stop { device, grid })
}

const PLAY: TransportCommand = TransportCommand::Play;
const BARS: LaunchGrid = LaunchGrid::Bars(1);

fn locate(tick: i64) -> TransportCommand {
    TransportCommand::Locate(Ticks(tick))
}

fn ons(ev: &[(u64, NoteKind)], key: u8) -> Vec<u64> {
    ev.iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { key: k, .. } if *k == key))
        .map(|(f, _)| *f)
        .collect()
}

fn offs(ev: &[(u64, NoteKind)], key: u8) -> Vec<u64> {
    ev.iter()
        .filter(|(_, k)| matches!(k, NoteKind::Off { key: k } if *k == key))
        .map(|(f, _)| *f)
        .collect()
}

/// One launch track and one arrangement track on a fresh engine; commands land in the first
/// batch.
fn rig(
    arrangement: Vec<SongNote>,
    slots: Vec<Option<LaunchSlotInput>>,
    commands: &[TransportCommand],
    frames: usize,
) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, arrangement)], &[(0, 120.0)]);
    h.submit_song(snapshot(&input, &[launch_track(dev, slots)]))
        .unwrap();
    for c in commands {
        h.transport(*c).unwrap();
    }
    run(&mut e, frames, 64);
    logs[0].1.events()
}

#[test]
fn a_launch_waits_for_the_next_bar_through_a_tempo_ramp() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let mut input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    input.tempo = tempo_ramps(&[(0, 120.0, true), (2 * BAR, 180.0, false)]);
    let snap = snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, PPQ, 60)])])],
    );
    let frame = |t: i64| snap.song_frame(Ticks(t), SR);
    let from = 3 * BAR / 10;
    h.submit_song(snap.clone()).unwrap();
    h.transport(locate(from)).unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(0, dev, BARS)).unwrap();
    let total = frame(3 * BAR) - frame(from) + 64;
    run(&mut e, total as usize, 64);
    let ev = logs[0].1.events();
    // The slot starts on the bar line and again one bar (a ramped one) later: exact frames.
    let first = frame(BAR) - frame(from);
    let second = frame(2 * BAR) - frame(from);
    assert_eq!(ons(&ev, 60)[..2], [first, second]);
    assert_eq!(offs(&ev, 60)[0], frame(BAR + PPQ) - frame(from));
}

#[test]
fn a_slot_loops_from_its_own_start() {
    let ev = rig(
        vec![],
        vec![slot(
            BAR / 2,
            vec![snote(0, PPQ / 2, 60), snote(PPQ, PPQ / 2, 64)],
        )],
        &[PLAY, go(0, DeviceInstanceId(1), LaunchGrid::Ticks(1))],
        (4 * BAR_FRAMES) as usize,
    );
    // Half a bar is 48000 frames; the launch is at once (the next tick, here 0).
    let half = BAR_FRAMES / 2;
    assert_eq!(ons(&ev, 60), (0..8).map(|k| k * half).collect::<Vec<_>>());
    assert_eq!(ons(&ev, 64).len(), 8);
}

#[test]
fn a_scene_launches_every_track_at_the_same_boundary() {
    let (mut e, mut h, logs) = note_logs(2, 0);
    let (a, b) = (logs[0].0, logs[1].0);
    let input = song_input(vec![strack(a, vec![]), strack(b, vec![])], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[
            launch_track(a, vec![slot(BAR, vec![snote(0, PPQ, 60)])]),
            launch_track(b, vec![slot(BAR, vec![snote(PPQ, PPQ, 67)])]),
        ],
    ))
    .unwrap();
    h.transport(locate(PPQ)).unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(0, a, BARS)).unwrap();
    h.transport(go(0, b, BARS)).unwrap();
    run(&mut e, (2 * BAR_FRAMES) as usize, 64);
    // From a quarter into the bar to the next bar line: three quarters.
    let boundary = 3 * 24_000;
    assert_eq!(ons(&logs[0].1.events(), 60)[0], boundary);
    assert_eq!(ons(&logs[1].1.events(), 67)[0], boundary + 24_000);
}

#[test]
fn a_stop_waits_for_the_boundary_and_releases_the_notes() {
    // A note as long as the loop, so it is sounding when the stop comes.
    let ev = rig(
        vec![],
        vec![slot(BAR, vec![snote(0, BAR, 60)])],
        &[PLAY, go(0, DeviceInstanceId(1), BARS)],
        (BAR_FRAMES / 2) as usize,
    );
    assert_eq!(ons(&ev, 60), vec![0]);
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, BAR, 60)])])],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(0, dev, BARS)).unwrap();
    run(&mut e, (BAR_FRAMES / 2) as usize, 64);
    h.transport(stop(dev, BARS)).unwrap();
    run(&mut e, (3 * BAR_FRAMES) as usize, 64);
    let ev = logs[0].1.events();
    // The note-off is at the bar line the stop waited for; nothing plays after it.
    assert_eq!(offs(&ev, 60), vec![BAR_FRAMES]);
    assert_eq!(ons(&ev, 60), vec![0]);
    assert!(stuck(&ev).is_empty());
}

#[test]
fn a_launched_slot_silences_the_arrangement_until_the_track_is_stopped() {
    // The arrangement plays key 50 in every quarter of bars 1 to 4.
    let arrangement: Vec<SongNote> = (0..16).map(|q| snote(q * PPQ, PPQ / 2, 50)).collect();
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, arrangement)], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, PPQ, 72)])])],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, BAR_FRAMES as usize, 64);
    h.transport(go(0, dev, BARS)).unwrap();
    run(&mut e, (BAR_FRAMES + 8 * 24_000 / 2) as usize, 64);
    h.transport(TransportCommand::Launch(LaunchCommand::StopAll {
        grid: BARS,
    }))
    .unwrap();
    run(&mut e, (2 * BAR_FRAMES) as usize, 64);
    let ev = logs[0].1.events();
    let a = ons(&ev, 50);
    // Bar 1 plays the arrangement (four notes); bars 2 and 3 are the slot's; from bar 4 the
    // arrangement is back.
    assert_eq!(a[..4], [0, 24_000, 48_000, 72_000]);
    assert!(a[4] >= 3 * BAR_FRAMES, "silent while launched: {a:?}");
    assert_eq!(ons(&ev, 72)[0], BAR_FRAMES);
    assert!(stuck(&ev).is_empty());
}

#[test]
fn going_back_to_the_arrangement_starts_the_notes_that_span_the_boundary() {
    // One long arrangement note; the slot takes over in bar 2 and gives back in bar 3.
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(
        vec![strack(dev, vec![snote(0, 8 * BAR, 40)])],
        &[(0, 120.0)],
    );
    h.submit_song(snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, PPQ, 72)])])],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, BAR_FRAMES as usize, 64);
    h.transport(go(0, dev, BARS)).unwrap();
    run(&mut e, BAR_FRAMES as usize, 64);
    h.transport(stop(dev, BARS)).unwrap();
    run(&mut e, (2 * BAR_FRAMES) as usize, 64);
    let ev = logs[0].1.events();
    // Released where the slot takes over, chased where the arrangement comes back.
    assert_eq!(ons(&ev, 40), vec![0, 2 * BAR_FRAMES]);
    assert_eq!(offs(&ev, 40), vec![BAR_FRAMES]);
}

#[test]
fn a_launch_while_stopped_starts_with_play() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, PPQ, 60)])])],
    ))
    .unwrap();
    h.transport(locate(BAR + PPQ)).unwrap();
    h.transport(go(0, dev, BARS)).unwrap();
    // Stopped: the request waits; nothing sounds.
    run(&mut e, 4_096, 64);
    assert!(logs[0].1.events().is_empty());
    h.transport(PLAY).unwrap();
    run(&mut e, BAR_FRAMES as usize, 64);
    // Play from a quarter into bar 2: the slot starts at bar 3 (three quarters later).
    assert_eq!(ons(&logs[0].1.events(), 60), vec![3 * 24_000 + 4_096]);
}

#[test]
fn the_published_state_shows_queued_playing_and_the_loop_position() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[launch_track(
            dev,
            vec![slot(BAR, vec![snote(0, PPQ, 60)]), slot(BAR, vec![])],
        )],
    ))
    .unwrap();
    h.transport(locate(PPQ)).unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(1, dev, BARS)).unwrap();
    run(&mut e, 4_096, 64);
    let mut states: Vec<LaunchState> = Vec::new();
    h.launch_states(&mut states);
    assert_eq!(states.len(), 1);
    assert_eq!((states[0].playing, states[0].queued), (None, Some(1)));
    run(&mut e, 72_000, 64);
    h.launch_states(&mut states);
    assert_eq!((states[0].playing, states[0].queued), (Some(1), None));
    assert!(states[0].phase > 0.0 && states[0].phase < 1.0);
    let event = h.next_launch_event().expect("the launch is reported");
    assert_eq!((event.device, event.slot, event.tick), (dev, Some(1), BAR));
}

#[test]
fn a_new_snapshot_keeps_the_slot_playing_without_a_retrigger() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    let tracks = [launch_track(dev, vec![slot(BAR, vec![snote(0, BAR, 60)])])];
    h.submit_song(snapshot(&input, &tracks)).unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(0, dev, BARS)).unwrap();
    run(&mut e, (BAR_FRAMES / 2) as usize, 64);
    // The same song again (an edit elsewhere), then a different slot content.
    h.submit_song(snapshot(&input, &tracks)).unwrap();
    run(&mut e, (BAR_FRAMES / 4) as usize, 64);
    let changed = [launch_track(dev, vec![slot(BAR, vec![snote(0, BAR, 61)])])];
    h.submit_song(snapshot(&input, &changed)).unwrap();
    run(&mut e, (BAR_FRAMES / 4 + BAR_FRAMES) as usize, 64);
    let ev = logs[0].1.events();
    assert_eq!(ons(&ev, 60), vec![0]);
    // The new content plays from its next note-on; the old note was released at the swap.
    assert_eq!(ons(&ev, 61), vec![BAR_FRAMES]);
    assert!(stuck(&ev).len() <= 1);
}

#[test]
fn a_locate_and_the_transport_stop_release_a_slot_and_it_resumes_on_the_song_grid() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let dev = logs[0].0;
    let input = song_input(vec![strack(dev, vec![])], &[(0, 120.0)]);
    h.submit_song(snapshot(
        &input,
        &[launch_track(dev, vec![slot(BAR, vec![snote(0, PPQ, 60)])])],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    h.transport(go(0, dev, LaunchGrid::Ticks(1))).unwrap();
    run(&mut e, (BAR_FRAMES / 8) as usize, 64);
    h.transport(TransportCommand::Stop).unwrap();
    run(&mut e, 4_096, 64);
    h.transport(locate(3 * BAR)).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, (BAR_FRAMES / 2) as usize, 64);
    let ev = logs[0].1.events();
    assert!(stuck(&ev).is_empty());
    // The slot started at tick 0, so it is on the bar grid: bar 3 begins a cycle at once, the
    // moment play resumes (4096 frames after the stop was applied).
    let stopped = ev
        .iter()
        .find(|(_, k)| matches!(k, NoteKind::AllOff))
        .map(|(f, _)| *f)
        .expect("the stop released the slot");
    let later: Vec<u64> = ons(&ev, 60).into_iter().filter(|f| *f > 0).collect();
    assert_eq!(later, vec![stopped + 4_096]);
}
