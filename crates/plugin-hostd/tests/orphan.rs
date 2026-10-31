//! A child whose parent dies ends by itself, even when its main thread is stuck inside a plugin
//! call and so can never see the closed control channel. The test re-runs its own binary as the
//! parent (it starts the real child, gets the main thread stuck in `activate`, then waits to be
//! killed), kills it from outside and waits for the child to go.

mod support;

use atomik_plugin_ipc::Deadline;
use atomik_plugin_ipc::ToChild;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use support::{Spawn, TestParent};

const ROLE: &str = "ATOMIK_ORPHAN_ROLE";
const INFO_FILE: &str = "ATOMIK_ORPHAN_INFO";
const TEST: &str = "a_child_with_a_stuck_main_thread_ends_when_its_parent_is_killed";

/// How long a child may outlive its parent.
const LIMIT: Duration = Duration::from_secs(5);

fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("tasklist runs");
        String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} "))
    } else {
        // A zombie still answers `kill -0`; `ps` shows its state.
        Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                !s.trim().is_empty() && !s.trim().starts_with('Z')
            })
    }
}

fn force_kill(pid: u32) {
    let _ = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).output()
    };
}

/// The parent role: starts a child whose main thread blocks, tells the test where it is, waits.
fn be_the_parent(info: &str) {
    let mut p = TestParent::spawn(&Spawn::null("null.hang-activate"));
    p.hello();
    p.instantiate();
    let id = p.id();
    // The child's main thread never returns from this.
    p.send(&ToChild::Activate {
        id,
        sample_rate: support::SR,
        frames: atomik_core::BATCH as u32,
        offline: false,
    });
    std::fs::write(info, format!("{}\n{}", p.pid(), p.block_path().display())).unwrap();
    // Bounded, so that a failed test cannot leave this process behind for long.
    for _ in 0..60 {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn a_child_with_a_stuck_main_thread_ends_when_its_parent_is_killed() {
    if let Ok(info) = std::env::var(INFO_FILE)
        && std::env::var(ROLE).is_ok()
    {
        be_the_parent(&info);
        return;
    }
    let info: PathBuf =
        std::env::temp_dir().join(format!("atomik-orphan-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&info);
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, "parent")
        .env(INFO_FILE, &info)
        .spawn()
        .unwrap();
    let limit = Deadline::after(Duration::from_secs(30));
    let (pid, block) = loop {
        if let Ok(text) = std::fs::read_to_string(&info)
            && let Some((pid, block)) = text.split_once('\n')
            && let Ok(pid) = pid.parse::<u32>()
        {
            break (pid, block.to_string());
        }
        assert!(!limit.passed(), "the parent never started its child");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(alive(pid), "the child runs while its parent does");
    // Let the main thread get into the stuck call.
    std::thread::sleep(Duration::from_millis(500));
    parent.kill().unwrap();
    parent.wait().unwrap();
    let limit = Deadline::after(LIMIT);
    while alive(pid) && !limit.passed() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let still_there = alive(pid);
    if still_there {
        force_kill(pid);
    }
    let _ = std::fs::remove_file(&info);
    let _ = std::fs::remove_file(block);
    assert!(
        !still_there,
        "the child outlived its parent by more than {LIMIT:?}"
    );
}
