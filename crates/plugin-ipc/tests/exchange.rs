//! The audio exchange: a parent port and a child port over two mappings of one file, with a
//! thread playing the child.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

mod support;

use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind, PluginIo, PluginTransport, ProcessStatus, SleepRule,
    TailLength,
};
use atomik_plugin_ipc::{
    AudioOp, Deadline, MAX_EVENTS_IN, MAX_EVENTS_OUT, Poll, ProcessorInfo, Response, Ticket,
};
use std::thread;
use std::time::Duration;
use support::{pair, ramp};

const SECOND: Duration = Duration::from_secs(5);

fn wait(deadline: Duration) -> Deadline {
    Deadline::after(deadline)
}

fn note(offset: u16, key: u8) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOn {
            key,
            velocity: 0.5,
            note_id: i32::from(key),
        },
    }
}

#[test]
fn an_exchange_returns_the_exact_outputs_events_and_status() {
    let (mut parent, mut child) = pair();
    let events = [
        note(0, 60),
        PluginEvent {
            offset: 7,
            kind: PluginEventKind::Param {
                id: 5,
                value: f64::from_bits(0x7ff8_0000_dead_beef),
            },
        },
    ];
    let transport = PluginTransport {
        playing: true,
        tempo_bpm: 133.25,
        song_pos_beats: 12.5,
        steady_frames: 99,
        ..PluginTransport::default()
    };

    let server = thread::spawn(move || {
        let req = child.wait_request(SECOND).expect("a request");
        assert_eq!(req.op(), AudioOp::Process);
        req.complete(|io| {
            assert_eq!(io.frames, BATCH);
            assert_eq!(io.main_in.len(), 2);
            assert!(!io.main_in_silent);
            assert_eq!(io.sidechain.len(), 2);
            assert!(io.sidechain_silent);
            assert_eq!(io.main_out.len(), 2);
            assert!(io.main_out.iter().all(|c| c.iter().all(|s| *s == 0.0)));
            assert_eq!(io.events_in.len(), 2);
            assert_eq!(io.events_in[0], note(0, 60));
            match io.events_in[1].kind {
                PluginEventKind::Param { id, value } => {
                    assert_eq!(id, 5);
                    assert_eq!(value.to_bits(), 0x7ff8_0000_dead_beef);
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(io.transport.tempo_bpm, 133.25);
            assert!(io.transport.playing);
            assert_eq!(io.transport.steady_frames, 99);
            for ch in 0..2 {
                for (o, s) in io.main_out[ch].iter_mut().zip(&io.main_in[ch]) {
                    *o = *s * 0.5;
                }
            }
            assert!(io.events_out.push(
                3,
                PluginEventKind::NoteOff {
                    key: 60,
                    note_id: 60
                }
            ));
            io.dropped = 2;
            (ProcessStatus::Tail, false)
        });
    });

    let main_in: [Batch; 2] = [ramp(1.0), ramp(-2.0)];
    let sidechain = [[0.0; BATCH]; 2];
    let mut main_out = [[9.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(MAX_EVENTS_OUT);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &main_in,
        main_in_silent: false,
        sidechain: &sidechain,
        sidechain_silent: true,
        main_out: &mut main_out,
        events_in: &events,
        events_out: &mut events_out,
        transport,
        dropped: 1,
    };
    let t = parent.submit(AudioOp::Process, &io);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    let r = parent.take_response(t, &mut io);
    assert_eq!(
        r,
        Response {
            status: ProcessStatus::Tail,
            ok: false
        }
    );
    assert_eq!(io.dropped, 3, "the parent's count plus the child's");
    server.join().unwrap();

    for i in 0..BATCH {
        assert_eq!(main_out[0][i], main_in[0][i] * 0.5);
        assert_eq!(main_out[1][i], main_in[1][i] * 0.5);
    }
    assert_eq!(events_out.len(), 1);
    assert_eq!(events_out.as_slice()[0].offset, 3);
    assert_eq!(parent.outstanding(), None);
}

#[test]
fn a_mono_block_with_a_silent_input_keeps_its_shape() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        child.wait_request(SECOND).unwrap().complete(|io| {
            assert_eq!(io.main_in.len(), 1);
            assert!(io.main_in_silent);
            assert!(io.sidechain.is_empty());
            assert_eq!(io.main_out.len(), 1);
            io.main_out[0] = [0.25; BATCH];
            (ProcessStatus::Continue, true)
        });
    });
    let main_in = [ramp(1.0)];
    let mut main_out = [[0.0; BATCH]; 1];
    let mut events_out = EventList::with_capacity(4);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &main_in,
        main_in_silent: true,
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut main_out,
        events_in: &[],
        events_out: &mut events_out,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    let t = parent.submit(AudioOp::Process, &io);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    parent.take_response(t, &mut io);
    server.join().unwrap();
    assert_eq!(main_out[0], [0.25; BATCH]);
}

#[test]
fn only_the_channels_the_caller_has_are_copied_back() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        // The parent asks for one output channel; the child sees exactly one.
        child.wait_request(SECOND).unwrap().complete(|io| {
            assert_eq!(io.main_out.len(), 1);
            io.main_out[0] = [0.5; BATCH];
            (ProcessStatus::Continue, true)
        });
    });
    let mut main_out = [[0.0; BATCH]; 1];
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
    let t = parent.submit(AudioOp::Process, &io);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    parent.take_response(t, &mut io);
    server.join().unwrap();
    assert_eq!(main_out[0], [0.5; BATCH]);
}

#[test]
fn events_beyond_the_request_capacity_are_dropped_and_counted() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        child.wait_request(SECOND).unwrap().complete(|io| {
            assert_eq!(io.events_in.len(), MAX_EVENTS_IN);
            assert_eq!(io.events_in[MAX_EVENTS_IN - 1], note(1, 1));
            (ProcessStatus::Continue, true)
        });
    });
    let events = vec![note(1, 1); MAX_EVENTS_IN + 6];
    let mut main_out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(4);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &[],
        main_in_silent: true,
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut main_out,
        events_in: &events,
        events_out: &mut events_out,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    let t = parent.submit(AudioOp::Process, &io);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    parent.take_response(t, &mut io);
    assert_eq!(io.dropped, 6);
    server.join().unwrap();
}

#[test]
fn events_from_the_child_beyond_the_response_capacity_are_dropped_by_the_child() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        child.wait_request(SECOND).unwrap().complete(|io| {
            for _ in 0..MAX_EVENTS_OUT + 4 {
                io.events_out
                    .push(0, PluginEventKind::PitchBend { value: 0.25 });
            }
            (ProcessStatus::Continue, true)
        });
    });
    let mut main_out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(MAX_EVENTS_OUT);
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
    let t = parent.submit(AudioOp::Process, &io);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    parent.take_response(t, &mut io);
    assert_eq!(io.dropped, 4);
    server.join().unwrap();
    assert_eq!(events_out.len(), MAX_EVENTS_OUT);
}

#[test]
fn start_stop_and_reset_are_exchanges_with_an_op_and_no_audio() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        for (want, ok) in [
            (AudioOp::Start, true),
            (AudioOp::Stop, false),
            (AudioOp::Reset, true),
        ] {
            let req = child.wait_request(SECOND).unwrap();
            assert_eq!(req.op(), want);
            req.complete(|io| {
                assert!(io.main_in.is_empty());
                assert!(io.main_out.is_empty());
                assert!(io.events_in.is_empty());
                (ProcessStatus::Continue, ok)
            });
        }
    });
    for (op, ok) in [
        (AudioOp::Start, true),
        (AudioOp::Stop, false),
        (AudioOp::Reset, true),
    ] {
        let t = parent.submit_op(op);
        assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
        let mut main_out: [Batch; 0] = [];
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
        let r = parent.take_response(t, &mut io);
        assert_eq!(r.ok, ok);
    }
    server.join().unwrap();
}

/// Runs one request with empty buffers and returns what the parent saw.
fn exchange_once(parent: &mut atomik_plugin_ipc::ParentPort, t: Ticket) -> Response {
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
    parent.take_response(t, &mut io)
}

fn submit_empty(parent: &mut atomik_plugin_ipc::ParentPort) -> Ticket {
    let mut main_out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(4);
    let io = PluginIo {
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
    parent.submit(AudioOp::Process, &io)
}

#[test]
fn a_request_is_pending_until_the_child_completes_it() {
    let (mut parent, mut child) = pair();
    assert_eq!(parent.outstanding(), None);
    let t = submit_empty(&mut parent);
    assert_eq!(parent.outstanding(), Some(t));
    assert_eq!(parent.poll(t), Poll::Pending);
    let req = child.wait_request(SECOND).unwrap();
    assert_eq!(parent.poll(t), Poll::Pending, "taken is not completed");
    req.complete(|_| (ProcessStatus::Continue, true));
    assert_eq!(parent.poll(t), Poll::Done);
    assert_eq!(parent.outstanding(), None);
    assert_eq!(
        exchange_once(&mut parent, t).status,
        ProcessStatus::Continue
    );
}

#[test]
fn tickets_grow_by_one_per_submission() {
    let (mut parent, mut child) = pair();
    let a = submit_empty(&mut parent);
    child
        .wait_request(SECOND)
        .unwrap()
        .complete(|_| (ProcessStatus::Continue, true));
    let b = submit_empty(&mut parent);
    assert_ne!(a, b);
    assert_eq!(b.sequence(), a.sequence() + 1);
}

#[test]
fn a_late_result_is_not_taken_for_the_next_ticket() {
    let (mut parent, mut child) = pair();
    let first = submit_empty(&mut parent);
    // The child is slow: it holds the request without answering.
    let held = child.wait_request(SECOND).unwrap();
    assert_eq!(parent.poll(first), Poll::Pending);
    assert!(
        submit_empty(&mut parent).is_none(),
        "no second request while one is in flight"
    );
    assert_eq!(parent.outstanding(), Some(first));
    held.complete(|_| (ProcessStatus::Continue, true));

    // The result of `first` arrived late; the parent never took it and moves on.
    let second = submit_empty(&mut parent);
    assert_ne!(first, second);
    assert_eq!(
        parent.poll(first),
        Poll::Done,
        "a superseded ticket has nothing to wait for"
    );
    assert_eq!(
        exchange_once(&mut parent, first).status,
        ProcessStatus::Late
    );

    child
        .wait_request(SECOND)
        .unwrap()
        .complete(|_| (ProcessStatus::Tail, true));
    assert_eq!(parent.poll(second), Poll::Done);
    assert_eq!(
        exchange_once(&mut parent, second).status,
        ProcessStatus::Tail
    );
    assert_eq!(
        exchange_once(&mut parent, first).status,
        ProcessStatus::Late,
        "the old ticket stays stale"
    );
}

#[test]
fn a_dead_child_is_reported_and_a_finished_result_still_counts() {
    let (mut parent, mut child) = pair();
    assert!(!parent.is_dead());
    let t = submit_empty(&mut parent);
    parent.mark_dead();
    assert!(parent.is_dead());
    assert_eq!(parent.poll(t), Poll::Dead);
    assert_eq!(parent.wait_done(t, Deadline::Never), Poll::Dead);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Dead);
    // A result that was published before the death is still a result.
    child
        .wait_request(SECOND)
        .unwrap()
        .complete(|_| (ProcessStatus::Continue, true));
    assert_eq!(parent.poll(t), Poll::Done);
}

#[test]
fn a_deadline_in_the_past_times_out_at_once() {
    let (mut parent, _child) = pair();
    let t = submit_empty(&mut parent);
    assert_eq!(
        parent.wait_done(t, Deadline::after(Duration::ZERO)),
        Poll::TimedOut
    );
    assert_eq!(
        parent.outstanding(),
        Some(t),
        "a timeout leaves the request in flight"
    );
}

#[test]
fn a_late_deadline_waits_for_the_child() {
    let (mut parent, mut child) = pair();
    let t = submit_empty(&mut parent);
    let server = thread::spawn(move || {
        let req = child.wait_request(SECOND).unwrap();
        thread::sleep(Duration::from_millis(30));
        req.complete(|_| (ProcessStatus::Continue, true));
    });
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    server.join().unwrap();
}

#[test]
fn wait_done_with_no_deadline_wakes_when_the_child_dies() {
    let (mut parent, _child) = pair();
    let t = submit_empty(&mut parent);
    let port = &parent;
    thread::scope(|s| {
        s.spawn(|| {
            thread::sleep(Duration::from_millis(30));
            port.mark_dead();
        });
        assert_eq!(port.wait_done(t, Deadline::Never), Poll::Dead);
    });
}

#[test]
fn the_child_times_out_when_nothing_is_submitted() {
    let (_parent, mut child) = pair();
    assert!(child.wait_request(Duration::from_millis(20)).is_none());
}

#[test]
fn the_child_wakes_for_a_request_while_parked() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        // Long enough to leave the spin phase and park on the doorbell.
        let req = child.wait_request(SECOND).expect("woken");
        req.complete(|_| (ProcessStatus::Continue, true));
    });
    thread::sleep(Duration::from_millis(50));
    let t = submit_empty(&mut parent);
    assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
    server.join().unwrap();
}

#[test]
fn many_exchanges_in_a_row_stay_in_step() {
    let (mut parent, mut child) = pair();
    let server = thread::spawn(move || {
        for n in 0..500u32 {
            child.wait_request(SECOND).unwrap().complete(|io| {
                io.main_out[0][0] = n as f32;
                (ProcessStatus::Continue, true)
            });
        }
    });
    for n in 0..500u32 {
        let mut main_out = [[0.0; BATCH]; 1];
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
        let t = parent.submit(AudioOp::Process, &io);
        assert_eq!(parent.wait_done(t, wait(SECOND)), Poll::Done);
        parent.take_response(t, &mut io);
        assert_eq!(main_out[0][0], n as f32);
    }
    server.join().unwrap();
}

#[test]
fn the_processor_info_and_the_wake_flag_cross() {
    let (parent, child) = pair();
    let info = parent.info();
    assert_eq!(info.sleep_rule, SleepRule::PluginDecides);
    assert_eq!(info.tail, TailLength::Finite(0));
    child.set_info(ProcessorInfo {
        sleep_rule: SleepRule::ZeroTail,
        tail: TailLength::Infinite,
    });
    assert_eq!(
        parent.info(),
        ProcessorInfo {
            sleep_rule: SleepRule::ZeroTail,
            tail: TailLength::Infinite
        }
    );
    assert!(!parent.wake_requested());
    child.request_wake();
    assert!(parent.wake_requested());
    assert!(!parent.wake_requested(), "reading clears it");
}

#[test]
fn a_death_handle_outlives_the_port_and_still_works() {
    let (parent, _child) = pair();
    let handle = parent.death_handle();
    assert!(!handle.is_dead());
    drop(parent);
    handle.mark_dead();
    assert!(handle.is_dead());
    let copy = handle.clone();
    assert!(copy.is_dead());
}

#[test]
fn marking_death_through_the_handle_ends_a_wait_on_another_thread() {
    let (mut parent, _child) = pair();
    let handle = parent.death_handle();
    let t = submit_empty(&mut parent);
    let killer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        handle.mark_dead();
    });
    assert_eq!(parent.wait_done(t, Deadline::Never), Poll::Dead);
    assert!(parent.is_dead());
    killer.join().unwrap();
}

#[test]
fn the_file_goes_when_the_last_owner_of_the_mapping_drops() {
    let (path, name) = support::names();
    let block = atomik_plugin_ipc::SharedBlock::create(&path).unwrap();
    let bell = atomik_plugin_ipc::Doorbell::create(&name, &block).unwrap();
    let parent = atomik_plugin_ipc::ParentPort::new(block, bell);
    let handle = parent.death_handle();
    drop(parent);
    assert!(path.exists(), "the handle still holds the mapping");
    drop(handle);
    assert!(!path.exists());
}
