//! Block files do not pile up: the OS removes the file when the last handle closes (Windows),
//! and the parent unlinks it once the child has mapped it (unix).

mod support;

use atomik_plugin_host::{EventList, PluginIo, PluginTransport, ProcessStatus};
use atomik_plugin_ipc::{AudioOp, Deadline, Doorbell, ParentPort, Poll, SharedBlock};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const ROLE: &str = "ATOMIK_IPC_CLEANUP_CHILD";
const TEST: &str = "a_creator_that_aborts_leaves_no_file_on_windows";

#[test]
fn a_creator_that_aborts_leaves_no_file_on_windows() {
    if let Ok(path) = std::env::var(ROLE) {
        let _block = SharedBlock::create(&PathBuf::from(path)).unwrap();
        // Nothing is dropped: the process dies the way a crashed app does.
        std::process::abort();
    }
    let (path, _) = support::names();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, &path)
        .status()
        .unwrap();
    assert!(!status.success(), "the child aborted");
    let limit = Deadline::after(Duration::from_secs(5));
    while path.exists() && !limit.passed() {
        std::thread::sleep(Duration::from_millis(10));
    }
    if cfg!(windows) {
        assert!(!path.exists(), "the OS removed the file at process end");
    } else {
        // Unix keeps the file: only an explicit unlink removes it, which is why the parent
        // unlinks as soon as the child has mapped the block.
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn unlink_removes_the_path_while_both_mappings_keep_working() {
    let (path, name) = support::names();
    let parent_block = SharedBlock::create(&path).unwrap();
    let child_block = SharedBlock::open(&path).unwrap();
    let parent_bell = Doorbell::create(&name, &parent_block).unwrap();
    let child_bell = Doorbell::open(&name, &child_block).unwrap();
    parent_block.unlink().unwrap();
    if cfg!(unix) {
        assert!(!path.exists());
        assert_eq!(
            SharedBlock::open(&path).err().unwrap().kind(),
            std::io::ErrorKind::NotFound,
            "open after unlink fails"
        );
    }
    parent_block.unlink().unwrap();
    let mut parent = ParentPort::new(parent_block, parent_bell);
    let mut child = atomik_plugin_ipc::ChildPort::new(child_block, child_bell);
    let server = std::thread::spawn(move || {
        child
            .wait_request(Duration::from_secs(5))
            .unwrap()
            .complete(|_| (ProcessStatus::Tail, true));
    });
    let mut main_out = [[0.0; 64]; 2];
    let mut events_out = EventList::with_capacity(4);
    let mut io = PluginIo {
        frames: 64,
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
    assert_eq!(
        parent.wait_done(t, Deadline::after(Duration::from_secs(5))),
        Poll::Done
    );
    assert_eq!(parent.take_response(t, &mut io).status, ProcessStatus::Tail);
    server.join().unwrap();
}

#[test]
fn only_the_creator_may_unlink() {
    let (path, _) = support::names();
    let parent = SharedBlock::create(&path).unwrap();
    let child = SharedBlock::open(&path).unwrap();
    assert_eq!(
        child.unlink().unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    drop(child);
    drop(parent);
}

#[test]
fn the_port_unlinks_its_block_and_the_exchange_still_works() {
    let (path, name) = support::names();
    let parent_block = SharedBlock::create(&path).unwrap();
    let child_block = SharedBlock::open(&path).unwrap();
    let parent_bell = Doorbell::create(&name, &parent_block).unwrap();
    let child_bell = Doorbell::open(&name, &child_block).unwrap();
    let mut parent = ParentPort::new(parent_block, parent_bell);
    let mut child = atomik_plugin_ipc::ChildPort::new(child_block, child_bell);
    parent.unlink_block().unwrap();
    parent.unlink_block().unwrap();
    if cfg!(unix) {
        assert!(!path.exists());
    }
    let server = std::thread::spawn(move || {
        child
            .wait_request(Duration::from_secs(5))
            .unwrap()
            .complete(|_| (ProcessStatus::Continue, true));
    });
    let t = parent.submit_op(AudioOp::Start);
    assert_eq!(
        parent.wait_done(t, Deadline::after(Duration::from_secs(5))),
        Poll::Done
    );
    server.join().unwrap();
}
