//! The doorbell and the exchange across two real processes. The child is this test binary run
//! again (`--exact <test name>`) with environment variables that make the test play the child.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind, PluginIo, PluginTransport, ProcessStatus,
};
use atomik_plugin_ipc::{
    AudioOp, ChildPort, Deadline, Doorbell, DoorbellName, ParentPort, Poll, SharedBlock,
};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use support::{names, ramp};

const ROLE: &str = "ATOMIK_IPC_TEST_ROLE";
const BLOCK: &str = "ATOMIK_IPC_TEST_BLOCK";
const BELL: &str = "ATOMIK_IPC_TEST_BELL";
const PATIENCE: Duration = Duration::from_secs(20);

/// In the child process of `test`: the block path and the doorbell name to open.
fn child_role(test: &str) -> Option<(PathBuf, DoorbellName)> {
    if std::env::var(ROLE).ok()?.as_str() != test {
        return None;
    }
    let path = PathBuf::from(std::env::var(BLOCK).expect("block path"));
    let bell = DoorbellName(std::env::var(BELL).expect("doorbell name"));
    Some((path, bell))
}

/// Starts this test binary as the child of `test`.
fn spawn_child(test: &str, path: &std::path::Path, bell: &DoorbellName) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(ROLE, test)
        .env(BLOCK, path)
        .env(BELL, &bell.0)
        .stdout(Stdio::piped())
        .spawn()
        .expect("the test binary starts again")
}

/// Reads the child's stdout until it says READY (it has opened everything and is about to wait).
fn wait_ready(child: &mut Child) {
    let out = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(out).lines();
    loop {
        match lines.next() {
            Some(Ok(l)) if l.trim().ends_with("READY") => break,
            Some(Ok(_)) => {}
            _ => panic!("the child ended before it was ready"),
        }
    }
    // libtest prefixes the line with the test name, hence `ends_with`. Keep draining so the child never blocks on a full pipe.
    std::thread::spawn(move || for _ in lines {});
}

fn finish(mut child: Child) -> std::process::ExitStatus {
    let limit = Deadline::after(PATIENCE);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if limit.passed() {
            let _ = child.kill();
            panic!("the child did not finish");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn make_parent(path: &std::path::Path, name: &DoorbellName) -> ParentPort {
    let block = SharedBlock::create(path).unwrap();
    let bell = Doorbell::create(name, &block).unwrap();
    ParentPort::new(block, bell)
}

fn open_child(path: &std::path::Path, name: &DoorbellName) -> ChildPort {
    let block = SharedBlock::open(path).unwrap();
    let bell = Doorbell::open(name, &block).unwrap();
    ChildPort::new(block, bell)
}

#[test]
fn a_ring_wakes_a_waiting_doorbell_in_another_process() {
    const TEST: &str = "a_ring_wakes_a_waiting_doorbell_in_another_process";
    if let Some((path, name)) = child_role(TEST) {
        let block = SharedBlock::open(&path).unwrap();
        let bell = Doorbell::open(&name, &block).unwrap();
        println!("READY");
        let rung = bell.wait(Duration::from_secs(15));
        std::process::exit(if rung { 0 } else { 7 });
    }
    let (path, name) = names();
    let block = SharedBlock::create(&path).unwrap();
    let bell = Doorbell::create(&name, &block).unwrap();
    let mut child = spawn_child(TEST, &path, &name);
    wait_ready(&mut child);
    // The child parks within microseconds; this leaves it ample time.
    std::thread::sleep(Duration::from_millis(300));
    bell.ring();
    assert_eq!(finish(child).code(), Some(0), "the child was woken");
}

#[test]
fn a_ring_before_the_wait_is_not_lost_across_processes() {
    const TEST: &str = "a_ring_before_the_wait_is_not_lost_across_processes";
    if let Some((path, name)) = child_role(TEST) {
        let block = SharedBlock::open(&path).unwrap();
        let bell = Doorbell::open(&name, &block).unwrap();
        println!("READY");
        // Let the parent ring first.
        std::thread::sleep(Duration::from_millis(500));
        let rung = bell.wait(Duration::from_secs(15));
        std::process::exit(if rung { 0 } else { 7 });
    }
    let (path, name) = names();
    let block = SharedBlock::create(&path).unwrap();
    let bell = Doorbell::create(&name, &block).unwrap();
    let mut child = spawn_child(TEST, &path, &name);
    wait_ready(&mut child);
    bell.ring();
    assert_eq!(finish(child).code(), Some(0));
}

#[test]
fn requests_and_responses_cross_between_two_processes() {
    const TEST: &str = "requests_and_responses_cross_between_two_processes";
    const ROUNDS: u32 = 100;
    if let Some((path, name)) = child_role(TEST) {
        let mut port = open_child(&path, &name);
        println!("READY");
        for _ in 0..ROUNDS {
            let req = port
                .wait_request(Duration::from_secs(15))
                .expect("a request");
            let op = req.op();
            req.complete(|io| {
                for (ch, out) in io.main_out.iter_mut().enumerate() {
                    for (o, s) in out.iter_mut().zip(&io.main_in[ch]) {
                        *o = *s * 0.5;
                    }
                }
                for e in io.events_in {
                    io.events_out.push(e.offset, e.kind);
                }
                (ProcessStatus::Tail, op == AudioOp::Process)
            });
        }
        return;
    }
    let (path, name) = names();
    let mut parent = make_parent(&path, &name);
    let mut child = spawn_child(TEST, &path, &name);
    wait_ready(&mut child);

    for round in 0..ROUNDS {
        let main_in = [ramp(1.0 + round as f32), ramp(-0.5)];
        let events = [PluginEvent {
            offset: (round % 64) as u16,
            kind: PluginEventKind::Param {
                id: round,
                value: f64::from_bits(0x7ff8_0000_0000_0000 | u64::from(round)),
            },
        }];
        let mut main_out = [[0.0; BATCH]; 2];
        let mut events_out = EventList::with_capacity(8);
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
        let t = parent.submit(AudioOp::Process, &io);
        assert_eq!(parent.wait_done(t, Deadline::after(PATIENCE)), Poll::Done);
        let r = parent.take_response(t, &mut io);
        assert_eq!(r.status, ProcessStatus::Tail);
        assert!(r.ok);
        for i in 0..BATCH {
            assert_eq!(main_out[0][i], main_in[0][i] * 0.5);
            assert_eq!(main_out[1][i], main_in[1][i] * 0.5);
        }
        assert_eq!(events_out.len(), 1);
        match events_out.as_slice()[0].kind {
            PluginEventKind::Param { id, value } => {
                assert_eq!(id, round);
                assert_eq!(value.to_bits(), 0x7ff8_0000_0000_0000 | u64::from(round));
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(finish(child).code(), Some(0));
}

#[test]
fn a_child_that_exits_without_answering_is_reported_dead() {
    const TEST: &str = "a_child_that_exits_without_answering_is_reported_dead";
    if let Some((path, name)) = child_role(TEST) {
        let mut port = open_child(&path, &name);
        println!("READY");
        let _request = port
            .wait_request(Duration::from_secs(15))
            .expect("a request");
        std::process::exit(9);
    }
    let (path, name) = names();
    let mut parent = make_parent(&path, &name);
    let mut child = spawn_child(TEST, &path, &name);
    wait_ready(&mut child);
    let t = parent.submit_op(AudioOp::Start);
    // The supervisor's job, played here: the process ended, so mark the block.
    let status = finish(child);
    assert_eq!(status.code(), Some(9));
    assert_eq!(parent.poll(t), Poll::Pending);
    parent.mark_dead();
    assert_eq!(parent.wait_done(t, Deadline::Never), Poll::Dead);
}
