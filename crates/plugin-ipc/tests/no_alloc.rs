//! HI-1: the parent's exchange (submit, wait, take, and the op submissions) allocates nothing,
//! and neither does the child's answer once its port is made.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind, PluginIo, PluginTransport, ProcessStatus,
};
use atomik_plugin_ipc::{AudioOp, Deadline, MAX_EVENTS_OUT, Poll};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::thread;
use std::time::Duration;
use support::{pair, ramp};

const SECOND: Duration = Duration::from_secs(5);

#[test]
fn the_parent_never_allocates_during_an_exchange() {
    let (mut parent, mut child) = pair();
    const ROUNDS: usize = 200;
    let server = thread::spawn(move || {
        for _ in 0..ROUNDS {
            assert_no_alloc(|| {
                let req = child.wait_request(SECOND).expect("a request");
                req.complete(|io| {
                    for ch in 0..io.main_out.len() {
                        io.main_out[ch] = io.main_in[ch];
                    }
                    io.events_out
                        .push(1, PluginEventKind::PitchBend { value: 0.5 });
                    (ProcessStatus::Continue, true)
                });
            });
        }
    });

    let main_in = [ramp(1.0), ramp(2.0)];
    let events: Vec<PluginEvent> = (0..40u16)
        .map(|i| PluginEvent {
            offset: i.min(63),
            kind: PluginEventKind::NoteOn {
                key: 60,
                velocity: 0.5,
                note_id: i32::from(i),
            },
        })
        .collect();
    let mut main_out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(MAX_EVENTS_OUT);

    for _ in 0..ROUNDS {
        events_out.clear();
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &main_in,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            events_in: &events,
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        assert_no_alloc(|| {
            let t = parent.submit(AudioOp::Process, &io);
            assert!(parent.outstanding().is_some() || parent.poll(t) == Poll::Done);
            assert_eq!(parent.wait_done(t, Deadline::after(SECOND)), Poll::Done);
            let r = parent.take_response(t, &mut io);
            assert_eq!(r.status, ProcessStatus::Continue);
            let _ = parent.wake_requested();
            assert!(!parent.is_dead());
            let _ = parent.info();
        });
        assert_eq!(events_out.len(), 1);
    }
    server.join().unwrap();
    assert_eq!(main_out[1], main_in[1]);
}

#[test]
fn start_stop_and_reset_submissions_never_allocate() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        for _ in 0..3 {
            child
                .wait_request(SECOND)
                .unwrap()
                .complete(|_| (ProcessStatus::Continue, true));
        }
    });
    for op in [AudioOp::Start, AudioOp::Stop, AudioOp::Reset] {
        let mut main_out = [[0.0; BATCH]; 0];
        let mut events_out = EventList::with_capacity(4);
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &[],
            main_in_silent: true,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            events_in: &[],
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        assert_no_alloc(|| {
            let t = parent.submit_op(op);
            assert_eq!(parent.wait_done(t, Deadline::after(SECOND)), Poll::Done);
            assert!(parent.take_response(t, &mut io).ok);
        });
    }
    server.join().unwrap();
}

#[test]
fn a_late_or_dead_exchange_never_allocates_either() {
    let (mut parent, _child) = pair();
    let mut main_out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(4);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &[],
        main_in_silent: true,
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut main_out,
        events_in: &[],
        events_out: &mut events_out,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    assert_no_alloc(|| {
        let t = parent.submit(AudioOp::Process, &io);
        assert_eq!(
            parent.wait_done(t, Deadline::after(Duration::ZERO)),
            Poll::TimedOut
        );
        assert!(parent.submit(AudioOp::Process, &io).is_none());
        assert_eq!(parent.take_response(t, &mut io).status, ProcessStatus::Late);
        parent.mark_dead();
        assert_eq!(parent.wait_done(t, Deadline::Never), Poll::Dead);
    });
}

#[test]
fn marking_and_reading_death_through_a_handle_never_allocates() {
    let (parent, _child) = pair();
    let handle = parent.death_handle();
    assert_no_alloc(|| {
        assert!(!handle.is_dead());
        handle.mark_dead();
        assert!(handle.is_dead());
    });
}
