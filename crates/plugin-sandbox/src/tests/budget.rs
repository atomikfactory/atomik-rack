//! The live wait of a batch: one period, or the callback's published budget when that is later,
//! and the cool-down after a run of waits that ran out. Every wait is read from the processor's
//! trace (what it asked for), not timed.

use super::processor::{Bufs, active};
use super::*;
use crate::clock::now;
use crate::fake::AudioMode;
use crate::processor::{
    COOL_AFTER, COOL_BATCHES, EXHAUSTED_AFTER, FALLBACK_CALLBACKS, FALLBACK_MAX_CALLBACKS,
    MAX_LIVE_WAIT, WaitKind,
};
use atomik_core::CallbackDeadline;
use atomik_plugin_host::{PluginInstance, PluginProcessor, ProcessStatus};
use atomik_plugin_ipc::Deadline;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A 64-frame batch at 48 kHz is 1.3 ms: far less than the budgets below.
const RATE: f64 = 48_000.0;

struct Traced {
    r: Rig,
    p: Box<dyn PluginProcessor>,
    deadline: Arc<CallbackDeadline>,
    trace: Arc<Mutex<Vec<WaitKind>>>,
}

fn traced() -> Traced {
    let mut r = rig(FakeOptions::default());
    let deadline = Arc::new(CallbackDeadline::new(now()));
    let trace = Arc::new(Mutex::new(Vec::new()));
    r.instance.set_callback_deadline(Arc::clone(&deadline));
    r.instance.trace = Some(Arc::clone(&trace));
    let p = active(&mut r, RATE, RunMode::Realtime);
    Traced {
        r,
        p,
        deadline,
        trace,
    }
}

impl Traced {
    fn waits(&self) -> Vec<WaitKind> {
        self.trace.lock().expect("trace").clone()
    }

    /// The deadline of the last full wait.
    fn last_full(&self) -> Instant {
        match self.waits().last() {
            Some(WaitKind::Full(Deadline::At(at))) => *at,
            other => panic!("a full wait with a deadline, got {other:?}"),
        }
    }

    /// A batch the child cannot answer yet; then it answers (the old request completes).
    fn batch_that_misses(&mut self, b: &mut Bufs, answered_before: usize) -> ProcessStatus {
        self.r.fake.ctl.set_audio(AudioMode::Stall);
        let status = self.p.process(&mut b.io());
        self.r.fake.ctl.set_audio(AudioMode::Echo);
        eventually("the old request to complete", || {
            self.r.fake.ctl.audio_ops().len() > answered_before
        });
        status
    }
}

#[test]
fn a_slow_batch_is_taken_when_the_callback_has_slack() {
    let mut t = traced();
    let mut b = Bufs::new();
    // The answer takes a few milliseconds: several batch periods.
    t.r.fake.ctl.set_audio_delay(Duration::from_millis(5));
    t.deadline.publish(now() + Duration::from_millis(90));
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
}

#[test]
fn without_slack_a_batch_waits_one_period_and_is_late_as_before() {
    let mut t = traced();
    let mut b = Bufs::new();
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    // A published time that has passed is no slack either: the wait is the period's.
    let mut t = traced();
    let passed = now();
    t.deadline.publish(passed);
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    assert!(
        t.last_full() > passed,
        "the period's deadline, not the stale one"
    );
}

#[test]
fn a_wait_ends_at_the_published_deadline_and_never_past_it() {
    let mut t = traced();
    let mut b = Bufs::new();
    let published = now() + Duration::from_millis(40);
    t.deadline.publish(published);
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    assert_eq!(
        t.last_full(),
        published,
        "the wait used exactly the published deadline"
    );
}

#[test]
fn a_far_published_deadline_is_capped() {
    let mut t = traced();
    let mut b = Bufs::new();
    t.deadline.publish(now() + Duration::from_secs(3600));
    t.r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Late);
    let limit = now() + MAX_LIVE_WAIT;
    assert!(
        t.last_full() <= limit,
        "never more than the cap past the call"
    );
}

#[test]
fn after_a_run_of_missed_waits_the_plugin_is_polled_not_waited_for_then_tried_again() {
    let mut t = traced();
    let mut b = Bufs::new();
    let mut answered = 0;
    for _ in 0..COOL_AFTER {
        assert_eq!(t.batch_that_misses(&mut b, answered), ProcessStatus::Late);
        answered += 1;
    }
    assert!(t.waits().iter().all(|w| matches!(w, WaitKind::Full(_))));
    assert_eq!(t.waits().len(), COOL_AFTER as usize);
    // The cool-down: late at once, no wait, counted late every time.
    for _ in 0..COOL_BATCHES {
        assert_eq!(t.batch_that_misses(&mut b, answered), ProcessStatus::Late);
        answered += 1;
    }
    let waits = t.waits();
    assert_eq!(waits.len(), (COOL_AFTER + COOL_BATCHES) as usize);
    assert!(
        waits[COOL_AFTER as usize..]
            .iter()
            .all(|w| *w == WaitKind::Poll)
    );
    // A full wait is tried again; one more miss starts the next cool-down at once.
    assert_eq!(t.batch_that_misses(&mut b, answered), ProcessStatus::Late);
    answered += 1;
    assert!(matches!(t.waits().last(), Some(WaitKind::Full(_))));
    assert_eq!(t.batch_that_misses(&mut b, answered), ProcessStatus::Late);
    assert_eq!(t.waits().last(), Some(&WaitKind::Poll));
}

#[test]
fn an_answer_ends_the_run_of_misses() {
    let mut t = traced();
    let mut b = Bufs::new();
    let mut answered = 0;
    for _ in 0..COOL_AFTER - 1 {
        t.batch_that_misses(&mut b, answered);
        answered += 1;
    }
    // The child answers within the wait (a generous one: the answer is not in doubt).
    t.deadline.publish(now() + Duration::from_millis(90));
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
    t.deadline.clear();
    answered += 1;
    for _ in 0..COOL_AFTER - 1 {
        t.batch_that_misses(&mut b, answered);
        answered += 1;
    }
    assert!(
        t.waits().iter().all(|w| matches!(w, WaitKind::Full(_))),
        "fewer than {COOL_AFTER} misses in a row never cool the plugin down"
    );
}

/// One callback of a plugin that uses the whole budget: a fresh published moment, a batch the
/// child cannot answer, then the answer (so the next callback can submit).
fn exhausted_callback(t: &mut Traced, b: &mut Bufs, answered: &mut usize) -> Instant {
    let published = now() + Duration::from_millis(40);
    t.deadline.publish(published);
    t.batch_that_misses(b, *answered);
    *answered += 1;
    published
}

/// The fall-back lengths seen so far.
fn fallbacks(t: &Traced) -> Vec<u32> {
    t.waits()
        .iter()
        .filter_map(|w| match w {
            WaitKind::Fallback(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// Runs exhausted callbacks until `n` fall-backs have begun.
fn until_fallbacks(t: &mut Traced, b: &mut Bufs, answered: &mut usize, n: usize) {
    for _ in 0..5_000 {
        if fallbacks(t).len() >= n {
            return;
        }
        exhausted_callback(t, b, answered);
    }
    panic!("no fall-back number {n}");
}

#[test]
fn callbacks_that_use_the_whole_budget_make_the_waits_one_period_for_a_while() {
    let mut t = traced();
    let mut b = Bufs::new();
    let mut answered = 0;
    for _ in 0..EXHAUSTED_AFTER {
        let published = exhausted_callback(&mut t, &mut b, &mut answered);
        assert_eq!(t.last_full(), published, "the budget is used");
        assert!(fallbacks(&t).is_empty());
    }
    // The next callback begins the fall-back: its waits are the period's, not the budget.
    let published = now() + Duration::from_millis(40);
    t.deadline.publish(published);
    t.batch_that_misses(&mut b, answered);
    answered += 1;
    assert_eq!(fallbacks(&t), vec![FALLBACK_CALLBACKS]);
    assert!(
        t.waits()
            .iter()
            .rev()
            .find_map(|w| match w {
                WaitKind::Full(Deadline::At(at)) => Some(*at < published),
                _ => None,
            })
            .expect("a wait"),
        "one period, which ends before the published moment"
    );
    // The span is counted in callbacks; after it the budget is tried again.
    for _ in 0..FALLBACK_CALLBACKS {
        exhausted_callback(&mut t, &mut b, &mut answered);
    }
    let published = exhausted_callback_until_full(&mut t, &mut b, &mut answered);
    assert_eq!(t.last_full(), published, "the budget is tried again");
}

/// Callbacks until one makes a full wait (the miss-based cool-down may be polling instead).
fn exhausted_callback_until_full(t: &mut Traced, b: &mut Bufs, answered: &mut usize) -> Instant {
    for _ in 0..1_000 {
        let before = t.waits().len();
        let published = exhausted_callback(t, b, answered);
        if matches!(t.waits().get(before), Some(WaitKind::Full(_))) {
            return published;
        }
    }
    panic!("no full wait");
}

#[test]
fn repeated_exhaustion_doubles_the_fallback_up_to_the_cap() {
    let mut t = traced();
    let mut b = Bufs::new();
    let mut answered = 0;
    until_fallbacks(&mut t, &mut b, &mut answered, 6);
    let spans = fallbacks(&t);
    let mut want = FALLBACK_CALLBACKS;
    for got in spans {
        assert_eq!(got, want);
        want = (want * 2).min(FALLBACK_MAX_CALLBACKS);
    }
}

#[test]
fn a_callback_inside_the_budget_resets_the_run_and_the_span() {
    let mut t = traced();
    let mut b = Bufs::new();
    let mut answered = 0;
    for _ in 0..EXHAUSTED_AFTER - 1 {
        exhausted_callback(&mut t, &mut b, &mut answered);
    }
    // A good callback: the child answers within the generous budget.
    t.deadline.publish(now() + Duration::from_millis(90));
    assert_eq!(t.p.process(&mut b.io()), ProcessStatus::Continue);
    answered += 1;
    for _ in 0..EXHAUSTED_AFTER - 1 {
        exhausted_callback(&mut t, &mut b, &mut answered);
    }
    // One more callback to see the previous one end: still no fall-back.
    t.deadline.publish(now() + Duration::from_millis(40));
    t.batch_that_misses(&mut b, answered);
    assert!(fallbacks(&t).is_empty());
}

#[test]
fn a_silent_child_is_declared_hung_after_the_same_batches_in_a_fallback_too() {
    // A fresh callback at every call: the budget is used up, the fall-back begins, and the
    // count of batches without progress is the same.
    let plain = calls_until_hung(None);
    let mut timeouts = crate::fake::test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = super::policy::started(FakeOptions::default(), timeouts);
    let cell = Arc::new(CallbackDeadline::new(now()));
    r.instance.set_callback_deadline(Arc::clone(&cell));
    let mut p = r
        .instance
        .activate(&cfg(RATE, RunMode::Realtime))
        .expect("activation");
    let mut b = Bufs::new();
    eventually("an answered batch", || {
        p.process(&mut b.io()) == ProcessStatus::Continue
    });
    r.fake.ctl.set_audio(AudioMode::Stall);
    let mut calls = 0;
    let got = loop {
        calls += 1;
        cell.publish(now() + Duration::from_millis(3));
        match p.process(&mut b.io()) {
            ProcessStatus::Late => assert!(calls < 200, "never hung"),
            ProcessStatus::Lost => break calls,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(got, plain);
}

fn calls_until_hung(deadline: Option<Instant>) -> usize {
    let mut timeouts = crate::fake::test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = super::policy::started(FakeOptions::default(), timeouts);
    let cell = Arc::new(CallbackDeadline::new(now()));
    if let Some(at) = deadline {
        cell.publish(at);
    }
    r.instance.set_callback_deadline(cell);
    let mut p = r
        .instance
        .activate(&cfg(RATE, RunMode::Realtime))
        .expect("activation");
    let mut b = Bufs::new();
    eventually("an answered batch", || {
        p.process(&mut b.io()) == ProcessStatus::Continue
    });
    r.fake.ctl.set_audio(AudioMode::Stall);
    let mut calls = 0;
    loop {
        calls += 1;
        match p.process(&mut b.io()) {
            ProcessStatus::Late => assert!(calls < 200, "never hung"),
            ProcessStatus::Lost => return calls,
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_silent_child_is_declared_hung_after_the_same_batches_whatever_the_wait() {
    // The guard counts batches that made no progress, not time spent waiting: the one wait
    // that may last several periods is the batch that submitted; the rest of the callback's
    // batches find the request still out and are late at once.
    let plain = calls_until_hung(None);
    let with_slack = calls_until_hung(Some(now() + Duration::from_millis(40)));
    assert_eq!(plain, with_slack);
    assert!((40..=50).contains(&plain), "{plain}");
}

#[test]
fn the_deadline_path_never_allocates() {
    use atomik_testkit::no_alloc::assert_no_alloc;
    let mut r = rig(FakeOptions::default());
    let cell = Arc::new(CallbackDeadline::new(now()));
    r.instance.set_callback_deadline(Arc::clone(&cell));
    let mut p = active(&mut r, 100.0, RunMode::Realtime);
    let mut b = Bufs::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    assert_no_alloc(|| {
        for _ in 0..50 {
            cell.publish(now() + Duration::from_millis(5));
            assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
        }
    });
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert_no_alloc(|| {
        cell.publish(now() + Duration::from_millis(5));
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    });
}
