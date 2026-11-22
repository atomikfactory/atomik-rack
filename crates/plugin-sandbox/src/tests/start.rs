//! A sandboxed plugin's start, stop and reset are never waited for on the audio thread: the
//! start and the reset ride on the next batch (the child serves them in the order stop, start,
//! reset, process), a stop is sent at once, and a start that is slow is a late batch, not a
//! failure. Waits are read from the processor's trace and from what the child was asked, never
//! timed.

use super::processor::{Bufs, active};
use super::*;
use crate::fake::{AudioMode, test_timeouts};
use crate::processor::WaitKind;
use atomik_plugin_host::{
    HUNG, PluginEvent, PluginEventKind, PluginInstance, PluginProcessor, ProcessStatus,
};
use atomik_plugin_ipc::{AudioOp, Before};
use std::sync::Mutex;

/// A period of 128 ms: short enough to wait for, long enough for a loaded machine to answer.
const FAST: f64 = 500.0;

fn note(offset: u16) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOn {
            key: 60,
            velocity: 0.8,
            note_id: 1,
        },
    }
}

struct Traced {
    r: Rig,
    p: Box<dyn PluginProcessor>,
    trace: Arc<Mutex<Vec<WaitKind>>>,
}

fn traced(rate: f64, mode: RunMode) -> Traced {
    let mut r = rig(FakeOptions::default());
    let trace = Arc::new(Mutex::new(Vec::new()));
    r.instance.trace = Some(Arc::clone(&trace));
    let p = active(&mut r, rate, mode);
    Traced { r, p, trace }
}

impl Traced {
    fn fulls(&self) -> usize {
        self.waits()
            .iter()
            .filter(|w| matches!(w, WaitKind::Full(_)))
            .count()
    }

    fn waits(&self) -> Vec<WaitKind> {
        self.trace.lock().expect("trace").clone()
    }

    /// Processes until a batch is taken (an earlier request may still be in flight), giving up
    /// after a generous bound.
    fn until_taken(&mut self, b: &mut Bufs) -> ProcessStatus {
        let mut status = ProcessStatus::Late;
        eventually("a batch to be taken", || {
            status = self.p.process(&mut b.io());
            status != ProcessStatus::Late
        });
        status
    }
}

fn half(b: &Bufs, ch: usize, i: usize) -> u32 {
    (b.main_in[ch][i] * 0.5).to_bits()
}

#[test]
fn a_start_with_an_instant_answer_is_not_late_and_the_first_note_arrives_with_it() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    b.events_in = vec![note(5)];
    assert!(t.p.start_processing());
    assert!(
        t.r.fake.ctl.audio_ops().is_empty(),
        "the call asked the child nothing"
    );
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(t.fulls(), 1, "one wait, the batch's own");
    assert_eq!(
        t.r.fake.ctl.audio_ops(),
        vec![AudioOp::Start, AudioOp::Process]
    );
    assert_eq!(t.r.fake.ctl.audio_events(), vec![vec![note(5)]]);
    assert_eq!(b.main_out[0][3].to_bits(), half(&b, 0, 3));
}

#[test]
fn a_slow_start_is_a_late_batch_not_a_failure_and_the_note_is_not_lost() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    b.events_in = vec![note(5)];
    t.r.fake.ctl.hold_start();
    // The call returns at once and says yes: the plugin is not asked yet.
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    // The batches behind it are polled while the child is busy with the start: no wait, no
    // second request, and the note is not repeated.
    b.events_in.clear();
    for _ in 0..30 {
        assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    }
    assert_eq!(t.fulls(), 1, "only the batch that carried the start waited");
    assert!(!t.r.instance.is_lost());
    t.r.fake.ctl.release_start();
    eventually("the start and its batch to be answered", || {
        t.r.fake.ctl.audio_ops().len() == 2
    });
    // The note came with the start, at its place in the batch, and nothing else was sent.
    assert_eq!(t.r.fake.ctl.audio_events(), vec![vec![note(5)]]);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(b.main_out[1][7].to_bits(), half(&b, 1, 7));
    assert_eq!(
        t.r.fake.ctl.audio_ops(),
        vec![AudioOp::Start, AudioOp::Process, AudioOp::Process]
    );
}

#[test]
fn events_that_arrive_while_the_start_is_slow_are_delivered_after_it() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    t.r.fake.ctl.hold_start();
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    b.events_in = vec![note(9)];
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    b.events_in.clear();
    t.r.fake.ctl.release_start();
    eventually("the first batch to be answered", || {
        t.r.fake.ctl.audio_ops().len() == 2
    });
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    // The event of the late batch was kept and went with the next request, at the start of it.
    let events = t.r.fake.ctl.audio_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].len(), 1);
    assert_eq!(events[1][0].offset, 0);
    assert_eq!(events[1][0].kind, note(9).kind);
}

#[test]
fn many_slow_starts_never_look_like_a_broken_plugin() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(t.p.start_processing());
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    // More cycles than the misses that start a cool-down: each start is one late batch, which an
    // answered batch after it forgives.
    for cycle in 0..6 {
        t.p.stop_processing();
        eventually("the stop to be answered", || {
            t.r.fake.ctl.audio_ops().last() == Some(&AudioOp::Stop)
        });
        t.r.fake.ctl.hold_start();
        assert!(t.p.start_processing());
        assert_eq!(
            t.p.process(&mut b.io()),
            ProcessStatus::Late,
            "cycle {cycle}"
        );
        t.r.fake.ctl.release_start();
        assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    }
    assert!(
        !t.waits().contains(&WaitKind::Poll),
        "no cool-down: {:?}",
        t.waits()
    );
    assert!(!t.r.instance.is_lost());
}

#[test]
fn a_start_the_plugin_refuses_is_reported_once_by_the_batch_that_carried_it() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    t.r.fake.ctl.refuse_start(true);
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::StartFailed);
    assert_eq!(t.r.fake.ctl.audio_ops(), vec![AudioOp::Start]);
    assert!(!t.r.instance.is_lost());
}

#[test]
fn a_refusal_that_comes_after_a_late_batch_is_reported_by_the_next_one() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    t.r.fake.ctl.refuse_start(true);
    t.r.fake.ctl.hold_start();
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    t.r.fake.ctl.release_start();
    eventually("the child to answer", || {
        !t.r.fake.ctl.audio_before().is_empty()
    });
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::StartFailed);
    // Told once: the batches after it are ordinary ones.
    t.r.fake.ctl.refuse_start(false);
    assert_ne!(t.p.process(&mut b.io()), ProcessStatus::StartFailed);
}

#[test]
fn a_refusal_is_not_lost_when_a_stop_follows_the_late_batch() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    t.r.fake.ctl.refuse_start(true);
    t.r.fake.ctl.hold_start();
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    t.r.fake.ctl.release_start();
    eventually("the child to answer", || {
        !t.r.fake.ctl.audio_before().is_empty()
    });
    // The stop would overwrite the answer: it is read first and reported by the next batch.
    t.p.stop_processing();
    t.r.fake.ctl.refuse_start(false);
    assert!(t.p.start_processing());
    let mut status = ProcessStatus::Late;
    eventually("the refusal to be reported", || {
        status = t.p.process(&mut b.io());
        status != ProcessStatus::Late
    });
    assert_eq!(status, ProcessStatus::StartFailed);
}

#[test]
fn a_stop_is_sent_without_waiting_and_a_start_right_after_it_leaves_the_plugin_started() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(t.p.start_processing());
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    // A child that does not answer: the stop is sent (not waited for) and the call returns.
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    t.p.stop_processing();
    assert!(t.p.start_processing());
    t.r.fake.ctl.set_audio(AudioMode::Echo);
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    assert_eq!(
        t.r.fake.ctl.audio_ops(),
        vec![
            AudioOp::Start,
            AudioOp::Process,
            AudioOp::Stop,
            AudioOp::Start,
            AudioOp::Process
        ]
    );
}

#[test]
fn a_stop_that_cannot_be_sent_yet_rides_on_the_next_batch_ahead_of_its_start() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(t.p.start_processing());
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    // A batch in flight that the child does not answer: nothing can be submitted behind it.
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    t.p.stop_processing();
    assert!(t.p.start_processing());
    t.r.fake.ctl.set_audio(AudioMode::Echo);
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    assert_eq!(
        t.r.fake.ctl.audio_ops(),
        vec![
            AudioOp::Start,
            AudioOp::Process,
            AudioOp::Process,
            AudioOp::Stop,
            AudioOp::Start,
            AudioOp::Process
        ]
    );
    let work = t.r.fake.ctl.audio_before();
    assert_eq!(
        work.last().copied(),
        Some(Before::STOP.with(Before::START)),
        "one request carried both"
    );
}

#[test]
fn a_start_that_was_owed_and_not_sent_is_cancelled_by_a_stop() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(t.p.start_processing());
    t.p.stop_processing();
    // The child never heard of the start, so there is nothing to stop either: a batch (should one
    // come) carries neither.
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
    assert_eq!(t.r.fake.ctl.audio_before(), vec![Before::NONE]);
    assert_eq!(t.r.fake.ctl.audio_ops(), vec![AudioOp::Process]);
}

#[test]
fn a_reset_happens_before_the_notes_of_the_batch_that_follows_it() {
    let mut t = traced(FAST, RunMode::Realtime);
    let mut b = Bufs::new();
    assert!(t.p.start_processing());
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    t.p.reset();
    b.events_in = vec![note(3)];
    assert_eq!(t.until_taken(&mut b), ProcessStatus::Continue);
    assert_eq!(
        t.r.fake.ctl.audio_ops(),
        vec![
            AudioOp::Start,
            AudioOp::Process,
            AudioOp::Reset,
            AudioOp::Process
        ]
    );
    // The same request carried the reset and the note: the note was not swallowed.
    assert_eq!(
        t.r.fake.ctl.audio_before().last().copied(),
        Some(Before::RESET)
    );
    assert_eq!(t.r.fake.ctl.audio_events().last(), Some(&vec![note(3)]));
}

#[test]
fn a_start_that_never_answers_is_declared_hung_like_any_batch_that_never_answers() {
    let mut timeouts = test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = super::policy::started(FakeOptions::default(), timeouts);
    let mut p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .expect("activation");
    let mut b = Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert!(p.start_processing());
    let mut calls = 0;
    loop {
        calls += 1;
        match p.process(&mut b.io()) {
            ProcessStatus::Late => assert!(calls < 100, "never hung"),
            ProcessStatus::Lost => break,
            other => panic!("{other:?}"),
        }
    }
    // 60 ms of 64-frame batches at 48 kHz is 45 of them.
    assert!((40..=50).contains(&calls), "{calls}");
    assert!(r.instance.is_lost(), "the block is dead");
    r.instance.poll();
    assert!(r.fake.ctl.was_killed());
    let reason = super::policy::exit_reason(&r);
    assert!(reason.starts_with(HUNG), "{reason}");
}

#[test]
fn offline_a_slow_start_is_waited_for_like_every_batch_and_gives_the_same_output() {
    let mut instant = traced(FAST, RunMode::Offline);
    let mut slow = traced(FAST, RunMode::Offline);
    slow.r.fake.ctl.set_audio_delay(Duration::from_millis(60));
    let (mut a, mut c) = (Bufs::new(), Bufs::new());
    a.events_in = vec![note(5)];
    c.events_in = vec![note(5)];
    for t in [&mut instant, &mut slow] {
        assert!(t.p.start_processing());
    }
    assert_eq!(instant.p.process(&mut a.io()), ProcessStatus::Continue);
    assert_eq!(slow.p.process(&mut c.io()), ProcessStatus::Continue);
    assert_eq!(a.main_out, c.main_out);
    assert_eq!(
        slow.r.fake.ctl.audio_ops(),
        vec![AudioOp::Start, AudioOp::Process]
    );
}

#[test]
fn offline_a_refused_start_is_reported_by_the_batch_that_carried_it() {
    let mut t = traced(FAST, RunMode::Offline);
    let mut b = Bufs::new();
    t.r.fake.ctl.refuse_start(true);
    assert!(t.p.start_processing());
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::StartFailed);
}

#[test]
fn a_plugin_whose_process_is_gone_cannot_start() {
    let mut t = traced(FAST, RunMode::Realtime);
    t.r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || t.r.instance.is_lost());
    assert!(!t.p.start_processing());
}
