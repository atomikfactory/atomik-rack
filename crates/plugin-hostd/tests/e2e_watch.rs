//! A plugin process that dies while a helper process it started still holds its pipes open is
//! noticed dead at once, from the process itself, once; its whole group with it. Without the
//! watcher of the process the pipe stays open and nothing would tell until the helper ended
//! (minutes). Each test kills only the process it holds the id of.
// Justification: the tests wait for child processes against the wall clock; nothing here is in a
// signal path.
#![allow(clippy::disallowed_methods)]

mod sandbox_support;

use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{InstanceId, ManagerEvent};
use sandbox_support::{Sandbox, config_with_env, request};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One test at a time spawns plugin processes here.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const BOUND: Duration = Duration::from_secs(60);

fn mix(class: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: format!("atomik.test.mix.{class}"),
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-e2e-watch-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wait_for_pid(file: &Path) -> u32 {
    let t = Instant::now();
    loop {
        if let Some(pid) = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            return pid;
        }
        assert!(t.elapsed() < BOUND, "no helper started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    } else {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        let stat = stat.trim();
        !stat.is_empty() && !stat.starts_with('Z')
    }
}

/// Ends the one process `pid`, which this test holds the id of (not its children).
fn kill_process(pid: u32) {
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .map(|o| o.status)
    } else {
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .map(|o| o.status)
    };
    assert!(status.expect("the kill command runs").success());
}

fn end_helper(pid: u32) {
    if process_alive(pid) {
        kill_process(pid);
    }
    let t = Instant::now();
    while process_alive(pid) {
        assert!(t.elapsed() < BOUND, "the helper {pid} did not end");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Polls until `want` crashes were reported; the list of the instances, in order.
fn crashes(sb: &mut Sandbox, want: usize) -> Vec<InstanceId> {
    let mut got = Vec::new();
    let t = Instant::now();
    while got.len() < want {
        for e in sb.manager.poll() {
            if let ManagerEvent::Crashed { instance, .. } = e {
                got.push(instance);
            }
        }
        assert!(
            t.elapsed() < BOUND,
            "{} of {want} crashes reported",
            got.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    got
}

/// No second report comes for a while after the first.
fn assert_no_more_crashes(sb: &mut Sandbox) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(500) {
        for e in sb.manager.poll() {
            assert!(
                !matches!(e, ManagerEvent::Crashed { .. }),
                "a second report: {e:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_plugin_process_killed_while_a_helper_holds_its_pipes_is_noticed_dead_once() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = temp("single");
    let pid_file = dir.join("helper.pid");
    let file = clap_test_plugin("atomik-test-clap-mix").expect("the test plugin builds");
    let plugin = mix("helper");
    let mut sb = Sandbox::new(
        PluginFormat::Clap,
        config_with_env(
            "ATOMIK_TEST_MIX_HELPER_PID_FILE",
            pid_file.to_str().unwrap(),
        ),
        &[(file, vec![plugin.clone()])],
    );
    let handle = sb
        .manager
        .instantiate(&request(&plugin, RunMode::Realtime))
        .expect("the plugin starts");
    let id = handle.id;
    let helper = wait_for_pid(&pid_file);
    let plugin_process =
        u32::try_from(sb.manager.native_handle(id).expect("a process id")).expect("a process id");
    assert!(process_alive(helper));

    kill_process(plugin_process);
    let reported = crashes(&mut sb, 1);
    assert_eq!(reported, vec![id]);
    assert!(
        process_alive(helper),
        "the death was reported while the helper still held the pipes"
    );
    assert_no_more_crashes(&mut sb);

    end_helper(helper);
    assert_no_more_crashes(&mut sb);
    drop(handle);
    sb.manager.poll();
    sb.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_group_process_killed_while_a_helper_holds_its_pipes_loses_every_member_once() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = temp("group");
    let pid_file = dir.join("helper.pid");
    let file = clap_test_plugin("atomik-test-clap-mix").expect("the test plugin builds");
    let (a, helper_class, b) = (mix("a"), mix("helper"), mix("b"));
    let mut sb = Sandbox::new(
        PluginFormat::Clap,
        config_with_env(
            "ATOMIK_TEST_MIX_HELPER_PID_FILE",
            pid_file.to_str().unwrap(),
        ),
        &[(file, vec![a.clone(), helper_class.clone(), b.clone()])],
    );
    sb.backend
        .set_grouping(atomik_plugin_host::ProcessMode::Shared, BTreeMap::new());
    let handles: Vec<_> = [&a, &helper_class, &b]
        .iter()
        .map(|p| {
            sb.manager
                .instantiate(&request(p, RunMode::Realtime))
                .expect("the plugin starts")
        })
        .collect();
    assert_eq!(sb.backend.grouped_instances(), 3);
    let helper = wait_for_pid(&pid_file);
    let group_process = u32::try_from(
        sb.manager
            .native_handle(handles[0].id)
            .expect("a process id"),
    )
    .expect("a process id");

    kill_process(group_process);
    let mut reported = crashes(&mut sb, 3);
    reported.sort_by_key(|i| i.0);
    let mut want: Vec<_> = handles.iter().map(|h| h.id).collect();
    want.sort_by_key(|i| i.0);
    assert_eq!(reported, want);
    assert!(process_alive(helper));
    assert_no_more_crashes(&mut sb);

    end_helper(helper);
    drop(handles);
    sb.manager.poll();
    sb.finish();
    let _ = std::fs::remove_dir_all(&dir);
}
