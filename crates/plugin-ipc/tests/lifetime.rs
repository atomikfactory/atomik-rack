//! A child bound to its parent ends when the parent is killed. Windows only: elsewhere the
//! child watches its parent itself, which the sandbox child's own tests cover.
#![cfg(windows)]

use atomik_plugin_ipc::Deadline;
use atomik_plugin_ipc::lifetime::bind_child;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const ROLE: &str = "ATOMIK_IPC_LIFETIME_ROLE";
const PID_FILE: &str = "ATOMIK_IPC_LIFETIME_PIDFILE";
const TEST: &str = "a_child_in_the_job_ends_when_its_parent_is_killed";

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} "))
}

fn sleep_bounded() {
    // Bounded, so that a failed test cannot leave a process behind for long.
    for _ in 0..60 {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn this_test(role: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, role);
    cmd
}

#[test]
fn a_child_in_the_job_ends_when_its_parent_is_killed() {
    match std::env::var(ROLE).as_deref() {
        Ok("sleeper") => {
            sleep_bounded();
            return;
        }
        Ok("middle") => {
            let mut child = this_test("sleeper").spawn().unwrap();
            bind_child(&child).unwrap();
            std::fs::write(std::env::var(PID_FILE).unwrap(), child.id().to_string()).unwrap();
            sleep_bounded();
            let _ = child.wait();
            return;
        }
        _ => {}
    }
    let file: PathBuf =
        std::env::temp_dir().join(format!("atomik-lifetime-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut middle = this_test("middle").env(PID_FILE, &file).spawn().unwrap();
    let limit = Deadline::after(Duration::from_secs(20));
    let pid: u32 = loop {
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Ok(pid) = text.trim().parse()
        {
            break pid;
        }
        assert!(
            !limit.passed(),
            "the middle process never started its child"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(alive(pid), "the child runs while its parent does");
    middle.kill().unwrap();
    middle.wait().unwrap();
    let limit = Deadline::after(Duration::from_secs(5));
    while alive(pid) && !limit.passed() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let still = alive(pid);
    if still {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
    let _ = std::fs::remove_file(&file);
    assert!(!still, "the child ended with its parent");
}
