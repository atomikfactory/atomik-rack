//! Grouping: several plugin instances in one child process (`SandboxBackend::set_grouping`),
//! through the real child with the built-in null plugins. The crash domain is the child: a
//! crash or a hang of one plugin takes every instance of the group with it.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]
// Justification: the tests wait for child processes against the wall clock; nothing here is in a
// signal path.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::{PluginRef, RunMode};
use atomik_plugin_host::{DeviceState, ManagerEvent, PluginHandle, ProcessChoice, ProcessMode};
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    CRASH_DEADLINE, METER_STATE, SETTLE, Sandbox, config, effect, meters, null_plugin, out_l,
    process_settled, request, run_to_crash,
};
use std::collections::BTreeMap;
use std::time::Duration;

fn grouped(mode: ProcessMode) -> Sandbox {
    let sb = Sandbox::null(config());
    sb.backend.set_grouping(mode, BTreeMap::new());
    sb
}

fn instantiate(sb: &mut Sandbox, id: &str, mode: RunMode) -> PluginHandle {
    sb.manager
        .instantiate(&request(&null_plugin(id), mode))
        .expect("the null plugin starts")
}

/// Polls until every instance in `ids` has reported a crash, and returns the reasons in order.
fn crash_reasons(sb: &mut Sandbox, count: usize) -> Vec<String> {
    let mut reasons = Vec::new();
    let limit = std::time::Instant::now() + CRASH_DEADLINE;
    while reasons.len() < count {
        for event in sb.manager.poll() {
            if let ManagerEvent::Crashed { reason, .. } = event {
                reasons.push(reason);
            }
        }
        assert!(
            std::time::Instant::now() < limit,
            "only {} of {count} crashes were reported",
            reasons.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    reasons
}

fn crashed(h: &DeviceHarness) -> bool {
    meters(h)[METER_STATE] == DeviceState::Crashed.as_f32()
}

#[test]
fn the_default_mode_keeps_one_child_per_instance_and_never_uses_the_group_wire() {
    let mut sb = Sandbox::null(config());
    let a = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    assert_eq!(sb.backend.live_children(), 2, "a child each");
    assert_eq!(sb.backend.grouped_instances(), 0, "no group was made");
    let (ha, hb) = (effect(a, 0.5), effect(b, 0.5));
    drop((ha, hb));
    sb.manager.poll();
    sb.finish();
}

#[test]
fn two_instances_of_the_shared_mode_live_in_one_child_and_both_make_sound() {
    let mut sb = grouped(ProcessMode::Shared);
    let a = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate(&mut sb, "null.latency", RunMode::Realtime);
    assert_eq!(sb.backend.live_children(), 1, "one child for both");
    assert_eq!(sb.backend.grouped_instances(), 2);
    let (mut ha, mut hb) = (effect(a, 0.5), effect(b, 0.5));
    process_settled(&mut ha, 20, SETTLE);
    process_settled(&mut hb, 20, SETTLE);
    assert!(
        out_l(&ha).iter().all(|s| *s == 0.25),
        "the gain halves: {:?}",
        &out_l(&ha)[..4]
    );
    assert!(
        out_l(&hb).iter().all(|s| s.is_finite()) && out_l(&hb).iter().any(|s| *s != 0.0),
        "the second instance made sound too"
    );
    assert_eq!(meters(&ha)[METER_STATE], DeviceState::Running.as_f32());
    assert_eq!(meters(&hb)[METER_STATE], DeviceState::Running.as_f32());
    drop((ha, hb));
    sb.manager.poll();
    sb.finish();
}

#[test]
fn the_per_file_mode_groups_the_plugins_of_one_file() {
    // The null plugins all come from one (placeholder) file.
    let mut sb = grouped(ProcessMode::PerFile);
    let a = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    assert_eq!(
        (sb.backend.live_children(), sb.backend.grouped_instances()),
        (1, 2)
    );
    drop((effect(a, 0.5), effect(b, 0.5)));
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_that_asks_for_its_own_process_gets_one_and_one_that_asks_for_the_shared_one_joins_it() {
    let sb = Sandbox::null(config());
    let mut choices: BTreeMap<PluginRef, ProcessChoice> = BTreeMap::new();
    choices.insert(null_plugin("null.latency"), ProcessChoice::Own);
    choices.insert(null_plugin("null.gain"), ProcessChoice::Shared);
    sb.backend.set_grouping(ProcessMode::PerPlugin, choices);
    let mut sb = sb;
    let g1 = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let g2 = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let own = instantiate(&mut sb, "null.latency", RunMode::Realtime);
    let other = instantiate(&mut sb, "null.synth", RunMode::Realtime);
    // The two gains share a child; the latency plugin and the synth (default: per plugin) each
    // have one.
    assert_eq!(sb.backend.live_children(), 3);
    assert_eq!(sb.backend.grouped_instances(), 2);
    drop((effect(g1, 0.1), effect(g2, 0.1), effect(own, 0.1)));
    drop(other);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn the_seventeenth_instance_starts_a_new_child() {
    let mut sb = grouped(ProcessMode::Shared);
    let mut handles = Vec::new();
    for i in 0..17 {
        handles.push(instantiate(&mut sb, "null.gain", RunMode::Realtime));
        let children = if i < 16 { 1 } else { 2 };
        assert_eq!(
            sb.backend.live_children(),
            children,
            "after instance {}",
            i + 1
        );
    }
    assert_eq!(sb.backend.grouped_instances(), 17);
    let mut devices: Vec<DeviceHarness> = handles.into_iter().map(|h| effect(h, 0.5)).collect();
    for h in &mut devices {
        process_settled(h, 5, SETTLE);
        assert!(out_l(h).iter().all(|s| *s == 0.25));
    }
    drop(devices);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_crash_in_a_group_marks_every_member_crashed_and_the_reason_names_the_group() {
    let mut sb = grouped(ProcessMode::Shared);
    let a = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate(&mut sb, "null.crash", RunMode::Realtime);
    let c = instantiate(&mut sb, "null.latency", RunMode::Realtime);
    assert_eq!(sb.backend.live_children(), 1);
    let (mut ha, mut hb, mut hc) = (effect(a, 0.25), effect(b, 0.25), effect(c, 0.25));
    // The bystanders are running when it happens.
    process_settled(&mut ha, 10, SETTLE);
    process_settled(&mut hc, 10, SETTLE);
    run_to_crash(&mut hb, 0.25, CRASH_DEADLINE);
    // The bystanders go with it: process them until they notice.
    let limit = std::time::Instant::now() + CRASH_DEADLINE;
    while !(crashed(&ha) && crashed(&hc)) {
        ha.process();
        hc.process();
        assert!(
            std::time::Instant::now() < limit,
            "the members did not turn Crashed: {:?} {:?} children {}",
            meters(&ha),
            meters(&hc),
            sb.backend.live_children()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let reasons = crash_reasons(&mut sb, 3);
    drop((ha, hb, hc));
    sb.manager.poll();
    for reason in &reasons {
        assert!(
            reason.contains("group of 3 plugins"),
            "the reason names the group: {reason}"
        );
        assert!(
            reason.contains("exited with code") || reason.contains("killed by signal"),
            "and how the process ended: {reason}"
        );
    }
    assert!(
        sb.wait_children(0, Duration::from_secs(5)),
        "the process is gone"
    );
    // RELOAD: a new instance starts a new group child.
    let again = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let again_too = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    assert_eq!(
        sb.backend.live_children(),
        1,
        "the restarted group is one child"
    );
    let (mut h1, mut h2) = (effect(again, 0.5), effect(again_too, 0.5));
    process_settled(&mut h1, 10, SETTLE);
    process_settled(&mut h2, 10, SETTLE);
    assert!(out_l(&h1).iter().all(|s| *s == 0.25) && out_l(&h2).iter().all(|s| *s == 0.25));
    drop((h1, h2));
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_hung_member_stops_the_whole_group_and_the_message_names_it() {
    let mut cfg = config();
    cfg.offline_watchdog = Duration::from_millis(300);
    let sb = Sandbox::null(cfg);
    sb.backend
        .set_grouping(ProcessMode::Shared, BTreeMap::new());
    let mut sb = sb;
    let ok = instantiate(&mut sb, "null.gain", RunMode::Offline);
    let hang = instantiate(&mut sb, "null.hang", RunMode::Offline);
    assert_eq!(sb.backend.live_children(), 1);
    let (mut h_ok, mut h_hang) = (effect(ok, 0.25), effect(hang, 0.25));
    h_ok.process();
    h_ok.process();
    h_hang.process();
    assert!(crashed(&h_hang), "the watchdog stopped the hung member");
    let limit = std::time::Instant::now() + Duration::from_secs(10);
    let mut reasons = Vec::new();
    while !crashed(&h_ok) {
        // The manager's poll is what stops the child of a member the watchdog gave up on.
        for event in sb.manager.poll() {
            if let ManagerEvent::Crashed { reason, .. } = event {
                reasons.push(reason);
            }
        }
        h_ok.process();
        assert!(
            std::time::Instant::now() < limit,
            "the bystander did not turn Crashed: {:?}",
            meters(&h_ok)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let limit = std::time::Instant::now() + CRASH_DEADLINE;
    while reasons.len() < 2 {
        for event in sb.manager.poll() {
            if let ManagerEvent::Crashed { reason, .. } = event {
                reasons.push(reason);
            }
        }
        assert!(
            std::time::Instant::now() < limit,
            "only {} crashes: {reasons:?}",
            reasons.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        reasons.iter().all(|r| r.contains("group of 2 plugins")),
        "{reasons:?}"
    );
    drop((h_ok, h_hang));
    sb.manager.poll();
    assert!(
        sb.wait_children(0, Duration::from_secs(5)),
        "the group's process is gone"
    );
    sb.finish();
}

#[test]
fn dropping_one_member_keeps_the_group_and_dropping_the_last_ends_the_child() {
    let mut sb = grouped(ProcessMode::Shared);
    let a = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate(&mut sb, "null.gain", RunMode::Realtime);
    let (ha, mut hb) = (effect(a, 0.5), effect(b, 0.5));
    drop(ha);
    sb.manager.poll();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        sb.backend.live_children(),
        1,
        "the group lives on for the other"
    );
    process_settled(&mut hb, 20, SETTLE);
    assert!(out_l(&hb).iter().all(|s| *s == 0.25), "and still works");
    assert_eq!(meters(&hb)[METER_STATE], DeviceState::Running.as_f32());
    drop(hb);
    sb.manager.poll();
    assert!(
        sb.wait_children(0, Duration::from_secs(5)),
        "the child ends with its last instance"
    );
    sb.finish();
}
