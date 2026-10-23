//! The engine-side processor: the batch flow (done, late, offline, dead), the ops and the
//! facts read from the block.

use super::*;
use crate::clock::now;
use crate::fake::AudioMode;
use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind, PluginInstance, PluginIo, PluginProcessor,
    PluginTransport, ProcessStatus, SleepRule, TailLength,
};
use atomik_plugin_ipc::AudioOp;

/// A period so long that a healthy batch is never late.
const SLOW: f64 = 100.0;
/// A period of 128 ms: short enough to wait for, long enough for a loaded machine to answer.
const FAST: f64 = 500.0;

pub(super) struct Bufs {
    pub main_in: [Batch; 2],
    pub main_out: [Batch; 2],
    pub events_out: EventList,
    pub events_in: Vec<PluginEvent>,
}

impl Bufs {
    pub fn new() -> Self {
        let mut main_in = [[0.0; BATCH]; 2];
        for (i, s) in main_in[0].iter_mut().enumerate() {
            *s = (i as f32 + 1.0) / 8.0;
        }
        main_in[1] = main_in[0].map(|s| -s);
        Self {
            main_in,
            main_out: [[0.0; BATCH]; 2],
            events_out: EventList::with_capacity(16),
            events_in: Vec::new(),
        }
    }

    pub fn io(&mut self) -> PluginIo<'_> {
        PluginIo {
            frames: BATCH,
            main_in: &self.main_in,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut self.main_out,
            events_in: &self.events_in,
            events_out: &mut self.events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        }
    }
}

pub(super) fn active(r: &mut Rig, rate: f64, mode: RunMode) -> Box<dyn PluginProcessor> {
    r.instance.activate(&cfg(rate, mode)).expect("activation")
}

fn half(b: &Bufs, ch: usize, i: usize) -> u32 {
    (b.main_in[ch][i] * 0.5).to_bits()
}

#[test]
fn a_batch_the_child_answers_returns_its_status_and_its_exact_output() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, SLOW, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(p.start_processing());
    for _ in 0..3 {
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
        for ch in 0..2 {
            for i in 0..BATCH {
                assert_eq!(b.main_out[ch][i].to_bits(), half(&b, ch, i));
            }
        }
    }
    r.fake.ctl.set_audio_status(ProcessStatus::Sleep);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Sleep);
    p.stop_processing();
    p.reset();
    assert_eq!(
        r.fake.ctl.audio_ops(),
        vec![
            AudioOp::Start,
            AudioOp::Process,
            AudioOp::Process,
            AudioOp::Process,
            AudioOp::Process,
            AudioOp::Stop,
            AudioOp::Reset
        ]
    );
}

#[test]
fn a_live_batch_that_is_not_answered_in_time_is_late_and_the_request_is_not_repeated() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    let t = now();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    assert!(
        now().saturating_duration_since(t) < Duration::from_millis(500),
        "bounded by about a period"
    );
    // The request is still in flight: no new one is submitted.
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    // The child recovers: it answers the old request, which is dropped, then the next batch
    // is a normal one.
    r.fake.ctl.set_audio(AudioMode::Echo);
    eventually("the old request to complete", || {
        r.fake.ctl.audio_ops().len() == 1
    });
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(b.main_out[0][1].to_bits(), half(&b, 0, 1));
    assert_eq!(
        r.fake.ctl.audio_ops(),
        vec![AudioOp::Process, AudioOp::Process]
    );
}

#[test]
fn an_offline_batch_is_never_late_however_long_the_child_takes() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, FAST, RunMode::Offline);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio_delay(Duration::from_millis(40));
    for _ in 0..3 {
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
        assert_eq!(b.main_out[0][3].to_bits(), half(&b, 0, 3));
    }
    assert!(p.start_processing(), "offline ops wait as long");
}

#[test]
fn the_offline_watchdog_declares_the_child_lost_and_the_main_thread_kills_it() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, FAST, RunMode::Offline);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    let t = now();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
    let took = now().saturating_duration_since(t);
    assert!(
        took >= Duration::from_millis(400) && took < Duration::from_secs(5),
        "{took:?}"
    );
    assert!(r.instance.is_lost(), "the block is dead");
    assert!(!r.fake.ctl.was_killed(), "the processor never kills");
    r.instance.poll();
    assert!(r.fake.ctl.was_killed(), "the main thread does");
    let reason = loop {
        if let PluginMessage::ProcessExited { reason } = next_message(&r.manager) {
            break reason;
        }
    };
    assert!(reason.contains("stopped answering audio"), "{reason}");
    // From then on a call returns at once.
    let t = now();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
    assert!(now().saturating_duration_since(t) < Duration::from_millis(1000));
}

#[test]
fn a_dead_child_is_lost_within_one_call_even_when_a_request_is_in_flight() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, SLOW, RunMode::Realtime);
    let mut b = Bufs::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    r.fake.ctl.set_audio(AudioMode::Stall);
    r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || r.instance.is_lost());
    let t = now();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
    assert!(!p.start_processing());
    p.stop_processing();
    p.reset();
    assert!(
        now().saturating_duration_since(t) < Duration::from_millis(1000),
        "no waiting for a dead child"
    );
}

#[test]
fn a_wait_for_a_stalled_child_ends_when_the_child_dies() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, SLOW, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    let ctl = Arc::clone(&r.fake.ctl);
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        ctl.exit("the plugin process exited with code 9 (0x9)");
    });
    // The deadline is 640 ms away: the death ends the spin long before it.
    let t = now();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
    assert!(now().saturating_duration_since(t) < Duration::from_millis(600));
    killer.join().unwrap();
}

#[test]
fn start_processing_that_is_not_answered_in_time_is_false_not_a_kill() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, FAST, RunMode::Realtime);
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert!(!p.start_processing());
    assert!(!r.instance.is_lost(), "late is not lost");
    r.fake.ctl.set_audio(AudioMode::Echo);
    eventually("the old op to complete", || {
        r.fake.ctl.audio_ops().len() == 1
    });
    assert!(p.start_processing());
}

#[test]
fn the_sleep_rule_the_tail_and_the_wake_request_come_from_the_block() {
    let mut r = rig(FakeOptions::default());
    let p = active(&mut r, SLOW, RunMode::Realtime);
    assert_eq!(p.sleep_rule(), SleepRule::PluginDecides);
    assert!(!p.wake_requested());
    r.fake
        .ctl
        .set_info(SleepRule::ZeroTail, TailLength::Finite(77));
    eventually("the info", || p.sleep_rule() == SleepRule::ZeroTail);
    assert_eq!(p.tail_samples(), TailLength::Finite(77));
    r.fake
        .ctl
        .set_info(SleepRule::PluginDecides, TailLength::Infinite);
    eventually("the tail", || p.tail_samples() == TailLength::Infinite);
    r.fake.ctl.request_wake();
    eventually("the wake request", || p.wake_requested());
    assert!(!p.wake_requested(), "read and clear");
}

#[test]
fn the_port_comes_back_with_the_processor_so_the_instance_can_activate_again() {
    let mut r = rig(FakeOptions::default());
    let p = active(&mut r, SLOW, RunMode::Realtime);
    assert!(r.instance.activate(&cfg(SLOW, RunMode::Realtime)).is_err());
    r.instance.deactivate(p);
    let mut p2 = active(&mut r, SLOW, RunMode::Realtime);
    let mut b = Bufs::new();
    assert_eq!(p2.process(&mut b.io()), ProcessStatus::Continue);
}

fn off(offset: u16, key: u8) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOff { key, note_id: 1 },
    }
}

fn on(offset: u16, key: u8) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOn {
            key,
            velocity: 0.5,
            note_id: 2,
        },
    }
}

fn param(offset: u16, value: f64) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::Param { id: 1, value },
    }
}

/// A processor with its first request stalled in flight.
fn stalled(r: &mut Rig) -> (Box<dyn PluginProcessor>, Bufs) {
    let mut p = active(r, FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    (p, b)
}

fn resume(r: &Rig) {
    r.fake.ctl.set_audio(AudioMode::Echo);
    eventually("the old request to complete", || {
        r.fake.ctl.audio_ops().len() == 1
    });
}

#[test]
fn the_events_of_a_late_batch_reach_the_child_first_at_offset_zero() {
    let mut r = rig(FakeOptions::default());
    let (mut p, mut b) = stalled(&mut r);
    b.events_in = vec![off(10, 60)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    resume(&r);
    b.events_in = vec![on(5, 62)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    let seen = r.fake.ctl.audio_events();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1], vec![off(0, 60), on(5, 62)]);
    // The carry is spent: the next batch carries nothing.
    b.events_in = vec![on(1, 64)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(r.fake.ctl.audio_events()[2], vec![on(1, 64)]);
}

#[test]
fn several_late_batches_keep_their_order_and_the_last_param_value_last() {
    let mut r = rig(FakeOptions::default());
    let (mut p, mut b) = stalled(&mut r);
    b.events_in = vec![on(2, 60), param(3, 0.1)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    b.events_in = vec![param(7, 0.9), off(8, 60)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    resume(&r);
    b.events_in = vec![param(1, 0.5)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(
        r.fake.ctl.audio_events()[1],
        vec![
            on(0, 60),
            param(0, 0.1),
            param(0, 0.9),
            off(0, 60),
            param(1, 0.5)
        ]
    );
}

#[test]
fn a_carry_that_overflows_drops_the_newest_events_and_counts_them() {
    let mut r = rig(FakeOptions::default());
    let (mut p, mut b) = stalled(&mut r);
    b.events_in = vec![off(0, 60); atomik_plugin_ipc::MAX_EVENTS_IN];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    b.events_in = vec![on(0, 61); 5];
    let mut io = b.io();
    assert_eq!(p.process(&mut io), ProcessStatus::Late);
    assert_eq!(io.dropped, 5);
    resume(&r);
    b.events_in = Vec::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    let seen = &r.fake.ctl.audio_events()[1];
    assert_eq!(seen.len(), atomik_plugin_ipc::MAX_EVENTS_IN);
    assert!(seen.iter().all(|e| *e == off(0, 60)));
}

#[test]
fn a_request_that_timed_out_is_not_carried_because_the_child_has_its_events() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    b.events_in = vec![off(4, 60)];
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    resume(&r);
    b.events_in = Vec::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    let seen = r.fake.ctl.audio_events();
    assert_eq!(seen[0], vec![off(4, 60)]);
    assert!(seen[1].is_empty(), "no duplicate");
}

fn bounce_rig(bounce: bool) -> Rig {
    let mut timeouts = crate::fake::test_timeouts();
    timeouts.bounce = bounce;
    let started = crate::fake::start_with(FakeOptions::default(), timeouts);
    Rig {
        instance: started.instance.map_err(|e| e.to_string()).unwrap(),
        fake: started.fake,
        manager: started.manager,
        live: started.live,
    }
}

#[test]
fn a_bounce_in_realtime_mode_waits_for_every_batch_and_flushes() {
    let mut r = bounce_rig(true);
    let mut p = active(&mut r, FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio_delay(Duration::from_millis(300));
    assert_eq!(
        p.process(&mut b.io()),
        ProcessStatus::Continue,
        "never late"
    );
    r.instance.poll();
    let flushes = r
        .fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, atomik_plugin_ipc::ToChild::Flush { .. }))
        .count();
    assert_eq!(flushes, 1);
    // The plugin is still told the mode it was given.
    assert!(r.fake.ctl.requests().iter().any(|q| matches!(
        q,
        atomik_plugin_ipc::ToChild::Activate { offline: false, .. }
    )));
}

#[test]
fn a_live_config_in_realtime_mode_keeps_the_one_batch_deadline_and_sends_no_flush() {
    let mut r = bounce_rig(false);
    let mut p = active(&mut r, FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    r.fake.ctl.set_audio_delay(Duration::from_millis(300));
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    r.instance.poll();
    assert!(
        !r.fake
            .ctl
            .requests()
            .iter()
            .any(|q| matches!(q, atomik_plugin_ipc::ToChild::Flush { .. }))
    );
}
