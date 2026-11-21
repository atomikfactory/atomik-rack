//! The parent (`SandboxBackend` + `PluginManager` + `PluginDevice`) and the child together:
//! life cycle, crashes, hangs, state, latency and the allocation rule (AC-1, 4, 5, 6, 7, 8, 9,
//! 10, 12 and the noisy plugin). The in-process reference is the same plugin through the real
//! backend.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_host::{DeviceState, LOST, ManagerEvent, PluginHandle};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use sandbox_support::SR;
use sandbox_support::{
    CRASH_DEADLINE, FORMATS, Format, METER_LATE, METER_STATE, Plug, SETTLE, Sandbox, bits, config,
    config_with_env, effect, meters, null_plugin, out_l, process_settled, request,
    request_with_state, run_to_crash,
};
use std::time::Duration;

fn null_sandbox() -> Sandbox {
    Sandbox::null(config())
}

fn instantiate_null(sb: &mut Sandbox, id: &str, mode: RunMode) -> PluginHandle {
    sb.manager
        .instantiate(&request(&null_plugin(id), mode))
        .expect("the null plugin starts in a child")
}

// ----- AC-1, AC-12 -----------------------------------------------------------------------------

#[test]
fn a_sandboxed_clap_effect_runs_in_its_own_child_and_dropping_its_device_ends_the_child() {
    let mut sb = Sandbox::real(Format::Clap);
    assert_eq!(sb.backend.live_children(), 0);
    let handle = sb
        .manager
        .instantiate(&request(
            &Format::Clap.plugin(Plug::Gain),
            RunMode::Realtime,
        ))
        .unwrap();
    assert_eq!(sb.backend.live_children(), 1, "one child for the instance");
    let mut h = effect(handle, 0.25);
    for _ in 0..20 {
        h.process();
    }
    assert!(
        out_l(&h).iter().any(|s| s.abs() > 0.01),
        "the plugin in the child made sound"
    );
    drop(h);
    let events = sb.manager.poll();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ManagerEvent::Destroyed(_))),
        "{events:?}"
    );
    assert!(
        sb.wait_children(0, Duration::from_secs(2)),
        "the child ends once its device is gone"
    );
    sb.finish();
}

#[test]
fn dropping_the_manager_ends_every_child() {
    let mut sb = null_sandbox();
    let a = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let b = instantiate_null(&mut sb, "null.latency", RunMode::Realtime);
    assert_eq!(sb.backend.live_children(), 2);
    let (_ha, _hb) = (effect(a, 0.1), effect(b, 0.1));
    sb.finish();
}

// ----- the null plugins through the device -----------------------------------------------------

#[test]
fn the_null_gain_halves_its_input_through_the_device_and_the_pass_param_passes_it() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let mut h = effect(handle, 0.5);
    for _ in 0..5 {
        h.process();
    }
    assert!(
        out_l(&h).iter().all(|s| *s == 0.25),
        "{:?}",
        &out_l(&h)[..4]
    );
    h.set_param("p0", 1.0);
    h.process();
    h.process();
    assert!(out_l(&h).iter().all(|s| *s == 0.5));
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_that_prints_to_stdout_keeps_working_through_the_sandbox() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.noisy", RunMode::Realtime);
    let mut h = effect(handle, 0.5);
    for _ in 0..200 {
        h.process();
        sb.manager.poll();
    }
    assert!(out_l(&h).iter().all(|s| *s == 0.5), "a passthrough");
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
    drop(h);
    sb.manager.poll();
    sb.finish();
}

// ----- AC-4 -------------------------------------------------------------------------------------

#[test]
fn the_process_start_stop_and_reset_paths_of_a_sandboxed_device_allocate_nothing() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let id = handle.id;
    let mut h = effect(handle, 0.5);
    // Every `process` of the harness is under the allocation guard; the first one starts the
    // plugin.
    for i in 0..100 {
        if i == 50 {
            h.set_param("p0", 1.0);
        }
        h.process();
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.process();
    // A restart: the next batches stop the processor and hand it back, the manager activates it
    // again and the device starts it once more.
    sb.manager.restart(id).unwrap();
    let mut restarted = false;
    for _ in 0..400 {
        h.process();
        restarted |= sb
            .manager
            .poll()
            .iter()
            .any(|e| matches!(e, ManagerEvent::Restarted(_)));
        if restarted {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(restarted, "the restart finished");
    for _ in 0..50 {
        h.process();
    }
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
    assert!(out_l(&h).iter().all(|s| *s == 0.5));
    drop(h);
    sb.manager.poll();
    sb.finish();
}

// ----- AC-5 -------------------------------------------------------------------------------------

fn crash_case(mut sb: Sandbox, plugin: &PluginRef) {
    let handle = sb
        .manager
        .instantiate(&request(plugin, RunMode::Realtime))
        .unwrap();
    let id = handle.id;
    let slot = sb.manager.slot(id).unwrap();
    let mut h = effect(handle, 0.25);
    let batches = run_to_crash(&mut h, 0.25, CRASH_DEADLINE);
    println!("crashed after {batches} batches");
    assert_eq!(slot.device_state(), DeviceState::Crashed);
    for _ in 0..10 {
        h.process();
        assert!(out_l(&h).iter().all(|s| *s == 0.25), "dry from now on");
    }
    let mut events = Vec::new();
    for _ in 0..200 {
        events.extend(sb.manager.poll());
        if events
            .iter()
            .any(|e| matches!(e, ManagerEvent::Crashed { .. }))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let reason = events
        .iter()
        .find_map(|e| match e {
            ManagerEvent::Crashed { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no Crashed event: {events:?}"));
    // An abort is exit code 3 on Windows and SIGABRT on Unix.
    assert!(
        reason.contains("exited with code") || reason.contains("killed by signal"),
        "{reason}"
    );
    assert!(sb.manager.is_lost(id));
    assert!(
        sb.manager.restart(id).is_err(),
        "a lost instance is replaced, not restarted"
    );
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_crashing_null_plugin_turns_the_device_crashed_and_the_test_survives() {
    let sb = Sandbox::null(config());
    crash_case(sb, &null_plugin("null.crash"));
}

#[test]
fn clap_crash_turns_the_device_crashed_and_the_test_survives() {
    let (file, plugin) = Format::Clap.crash_plugin();
    let sb = Sandbox::new(
        PluginFormat::Clap,
        config_with_env("ATOMIK_TEST_CRASH", "1"),
        &[(file, vec![plugin.clone()])],
    );
    crash_case(sb, &plugin);
}

#[test]
fn vst3_crash_turns_the_device_crashed_and_the_test_survives() {
    let (file, plugin) = Format::Vst3.crash_plugin();
    let sb = Sandbox::new(
        PluginFormat::Vst3,
        config_with_env("ATOMIK_TEST_CRASH", "1"),
        &[(file, vec![plugin.clone()])],
    );
    crash_case(sb, &plugin);
}

#[test]
fn a_crashed_effect_with_no_input_cabled_is_silent() {
    // No test plugin is a crashing instrument; a crashed effect with no input cabled stands in:
    // its dry signal is silence, as an instrument's output is.
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.crash", RunMode::Realtime);
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    // The batch that finds the crash is the dry input, which is silence with no input cabled.
    run_to_crash(&mut h, 0.0, CRASH_DEADLINE);
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Crashed.as_f32());
    assert!(out_l(&h).iter().all(|s| *s == 0.0));
    drop(h);
    sb.manager.poll();
    sb.finish();
}

// ----- AC-7, AC-8 -------------------------------------------------------------------------------

#[test]
fn a_hung_plugin_live_is_late_not_crashed_and_the_effect_passes_its_dry_input() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.hang", RunMode::Realtime);
    let mut h = effect(handle, 0.25);
    for _ in 0..60 {
        h.process();
        assert!(out_l(&h).iter().all(|s| *s == 0.25), "dry");
    }
    let m = meters(&h);
    assert_eq!(m[METER_STATE], DeviceState::Running.as_f32());
    assert!(m[METER_LATE] >= 59.0, "late batches are counted: {m:?}");
    assert_eq!(sb.backend.live_children(), 1, "the child is still there");
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_hung_plugin_offline_is_stopped_by_the_watchdog_and_crashes_the_device() {
    let mut cfg = config();
    cfg.offline_watchdog = Duration::from_millis(300);
    let mut sb = Sandbox::null(cfg);
    let handle = instantiate_null(&mut sb, "null.hang", RunMode::Offline);
    let id = handle.id;
    let mut h = effect(handle, 0.25);
    h.process();
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Crashed.as_f32());
    assert!(out_l(&h).iter().all(|s| *s == 0.25), "dry");
    let mut crashed = false;
    for _ in 0..200 {
        crashed |= sb
            .manager
            .poll()
            .iter()
            .any(|e| matches!(e, ManagerEvent::Crashed { instance, .. } if *instance == id));
        if crashed {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(crashed, "the poll reports the crash");
    assert!(
        sb.wait_children(0, Duration::from_secs(5)),
        "the hung child was killed"
    );
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_that_hangs_in_activate_fails_instantiate_after_the_timeout_and_the_child_is_killed() {
    let mut cfg = config();
    cfg.create_timeout = Duration::from_millis(500);
    let mut sb = Sandbox::null(cfg);
    #[allow(clippy::disallowed_methods)]
    // Justification: this measures how long the main thread was blocked.
    let start = std::time::Instant::now();
    let result = sb.manager.instantiate(&request(
        &null_plugin("null.hang-activate"),
        RunMode::Realtime,
    ));
    #[allow(clippy::disallowed_methods)]
    // Justification: as above.
    let took = start.elapsed();
    assert!(result.is_err());
    assert!(took >= Duration::from_millis(400), "{took:?}");
    assert!(took < Duration::from_secs(60), "{took:?}");
    assert!(
        sb.wait_children(0, Duration::from_secs(5)),
        "the child is killed"
    );
    // The main thread is free: a good plugin starts right away.
    let ok = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    drop(ok);
    sb.manager.poll();
    sb.finish();
}

// ----- AC-9 -------------------------------------------------------------------------------------

#[test]
fn the_latency_a_null_plugin_reports_is_in_the_slot_after_instantiate() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.latency", RunMode::Realtime);
    let id = handle.id;
    assert_eq!(sb.manager.latency(id), Some(32));
    assert_eq!(sb.manager.slot(id).unwrap().latency(), 32);
    assert_eq!(
        atomik_device_sdk::Device::latency_frames(&handle.device),
        32
    );
    drop(handle);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_latency_change_in_a_sandboxed_plugin_restarts_it_and_the_slot_follows() {
    for format in FORMATS {
        let mut sb = Sandbox::real(format);
        let handle = sb
            .manager
            .instantiate(&request(&format.plugin(Plug::Gain), RunMode::Realtime))
            .unwrap();
        let id = handle.id;
        assert_eq!(sb.manager.latency(id), Some(0));
        let mut h = effect(handle, 0.25);
        for _ in 0..4 {
            h.process();
        }
        // The plugin delays by 100 frames: it tells the host, the manager restarts it in the
        // child and publishes the latency.
        h.set_param("p3", latency_value(format, 100.0));
        let mut events = Vec::new();
        for _ in 0..2000 {
            h.process();
            events.extend(sb.manager.poll());
            if sb.manager.slot(id).unwrap().latency() == 100 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            sb.manager.slot(id).unwrap().latency(),
            100,
            "{}: {events:?}",
            format.name()
        );
        assert!(
            events.contains(&ManagerEvent::LatencyChanged {
                instance: id,
                frames: 100
            }),
            "{}: {events:?}",
            format.name()
        );
        for _ in 0..50 {
            h.process();
        }
        assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
        drop(h);
        sb.manager.poll();
        sb.finish();
    }
}

/// The device value of the latency param (VST3 takes it normalised, over 0..512).
fn latency_value(format: Format, frames: f64) -> f32 {
    match format {
        Format::Clap => frames as f32,
        Format::Vst3 => frames as f32,
    }
}

// ----- AC-10 ------------------------------------------------------------------------------------

#[test]
fn a_state_captured_from_one_sandboxed_instance_restores_into_another_with_the_same_output() {
    for format in FORMATS {
        let mut sb = Sandbox::real(format);
        let plugin = format.plugin(Plug::Gain);
        let a = sb
            .manager
            .instantiate(&request(&plugin, RunMode::Realtime))
            .unwrap();
        let ida = a.id;
        let mut ha = effect(a, 0.25);
        ha.set_param("p1", format.gain_value(-9.0));
        process_settled(&mut ha, 10, SETTLE);
        let blob = sb.manager.capture_state(ida).unwrap();
        assert!(!blob.0.is_empty());
        let b = sb
            .manager
            .instantiate(&request_with_state(
                &plugin,
                RunMode::Realtime,
                blob.clone(),
            ))
            .unwrap();
        assert!(b.restore_error.is_none());
        let idb = b.id;
        let mut hb = effect(b, 0.25);
        process_settled(&mut hb, 10, SETTLE);
        assert_eq!(bits(&out_l(&hb)), bits(&out_l(&ha)), "{}", format.name());
        assert_eq!(sb.manager.capture_state(idb).unwrap(), blob);
        drop((ha, hb));
        sb.manager.poll();
        sb.finish();
    }
}

#[test]
fn param_text_of_a_sandboxed_plugin_is_the_plugins_own() {
    let mut sb = Sandbox::real(Format::Clap);
    let plugin = Format::Clap.plugin(Plug::Gain);
    let h = sb
        .manager
        .instantiate(&request(&plugin, RunMode::Realtime))
        .unwrap();
    let remote = sb.manager.param_text(h.id, 1, -6.0);
    let mut local = sandbox_support::in_process(Format::Clap, &[Format::Clap.main_file()]);
    let l = local
        .instantiate(&request(&plugin, RunMode::Realtime))
        .unwrap();
    assert_eq!(remote, local.param_text(l.id, 1, -6.0));
    assert!(remote.is_some_and(|t| !t.is_empty()));
    drop((h, l));
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_lost_instance_fails_every_call_with_the_process_stopped_text() {
    let mut sb = null_sandbox();
    let handle = instantiate_null(&mut sb, "null.crash", RunMode::Realtime);
    let id = handle.id;
    let mut h = effect(handle, 0.25);
    run_to_crash(&mut h, 0.25, CRASH_DEADLINE);
    let e = sb.manager.capture_state(id).unwrap_err();
    assert!(e.to_string().contains(LOST), "{e}");
    drop(h);
    sb.manager.poll();
    sb.finish();
}

// ----- state capture in the background ---------------------------------------------------------------

#[test]
fn the_state_of_a_busy_plugin_is_asked_for_in_the_background_without_a_stall_or_a_stop() {
    let mut sb = Sandbox::null(config_with_env("ATOMIK_NULL_CAPTURE_DELAY_MS", "600"));
    let handle = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let id = handle.id;
    let device = effect(handle, 0.25);
    let _ = sb.manager.poll();
    // The blocking calls above (instantiate, activate) may be slow on a loaded machine and leave
    // stall records of their own: only what the background phase adds counts.
    let _ = sb.manager.take_stalls();
    #[allow(clippy::disallowed_methods)]
    // Justification: this measures how long a busy plugin takes, and that the main thread kept polling.
    let began = std::time::Instant::now();
    sb.manager.request_capture(id).unwrap();
    let mut events = Vec::new();
    let mut polls = 0;
    let landed = loop {
        events.extend(sb.manager.poll());
        polls += 1;
        if let Some(landed) = sb.manager.poll_capture(id) {
            break landed;
        }
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the plugin answers"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(began.elapsed() >= Duration::from_millis(500), "it was busy");
    assert!(
        polls > 5,
        "the main thread went on polling meanwhile: {polls}"
    );
    assert!(landed.result.is_ok() && !landed.stale && !landed.again);
    let stalls = sb.manager.take_stalls();
    assert!(
        !stalls.iter().any(|s| s.call == "CaptureState"),
        "the background request waited for nothing: {stalls:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            ManagerEvent::Crashed { .. } | ManagerEvent::Failed { .. }
        )),
        "{events:?}"
    );
    // A call that waits for the same plugin is a stall, and the record says which call.
    sb.manager.capture_state(id).unwrap();
    let _ = sb.manager.poll();
    let stalls = sb.manager.take_stalls();
    assert!(
        stalls.iter().any(|s| s.call == "CaptureState"),
        "{stalls:?}"
    );
    drop(device);
    let _ = sb.manager.poll();
    sb.finish();
}

// ----- a plugin that is alive but busy is not stopped -----------------------------------------------

#[test]
fn a_plugin_whose_main_thread_sleeps_past_the_timeout_is_not_stopped_and_answers_afterwards() {
    let mut cfg = config_with_env("ATOMIK_NULL_CAPTURE_DELAY_MS", "2500");
    cfg.create_timeout = Duration::from_millis(500);
    let mut sb = Sandbox::null(cfg);
    let handle = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let id = handle.id;
    let mut device = effect(handle, 0.25);
    let _ = sb.manager.poll();
    assert!(sb.manager.not_responding(id).is_none());
    assert_eq!(sb.backend.live_children(), 1);

    // The child's main thread sleeps in the capture; its audio thread runs.
    let err = sb.manager.capture_state(id).unwrap_err();
    assert!(err.is_not_responding(), "{err}");
    assert_eq!(sb.backend.live_children(), 1, "the child was not stopped");
    assert_eq!(
        sb.manager.not_responding(id).as_deref(),
        Some("CaptureState")
    );
    let mut events = sb.manager.poll();
    for _ in 0..20 {
        device.process();
    }
    assert!(
        out_l(&device).iter().any(|s| s.abs() > 0.01),
        "its audio keeps running while it does not answer"
    );

    // A second call does not wait for it: it leaves no stall record (the first one, which timed
    // out, did; that record is taken away first).
    let _ = sb.manager.take_stalls();
    let again = sb.manager.capture_state(id).unwrap_err();
    assert!(again.is_not_responding(), "{again}");
    let stalls = sb.manager.take_stalls();
    assert!(
        !stalls.iter().any(|s| s.call == "CaptureState"),
        "the second call waited: {stalls:?}"
    );

    // It answers by itself once its main thread is free again.
    #[allow(clippy::disallowed_methods)]
    // Justification: a bound on how long the test waits for the child.
    let limit = std::time::Instant::now() + Duration::from_secs(30);
    while sb.manager.not_responding(id).is_some() {
        events.extend(sb.manager.poll());
        #[allow(clippy::disallowed_methods)]
        // Justification: as above.
        let now = std::time::Instant::now();
        assert!(now < limit, "the plugin answers in the end");
        std::thread::sleep(Duration::from_millis(5));
    }
    events.extend(sb.manager.poll());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ManagerEvent::NotResponding { instance, .. } if *instance == id)),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ManagerEvent::Responding(i) if *i == id)),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            ManagerEvent::Crashed { .. } | ManagerEvent::Failed { .. }
        )),
        "{events:?}"
    );
    assert_eq!(sb.backend.live_children(), 1, "still the same child");
    drop(device);
    let _ = sb.manager.poll();
    sb.finish();
}

#[test]
fn an_idle_child_at_its_longest_wait_is_seen_alive_when_its_main_thread_does_not_answer() {
    let mut cfg = config_with_env("ATOMIK_NULL_CAPTURE_DELAY_MS", "2500");
    cfg.create_timeout = Duration::from_millis(500);
    let mut sb = Sandbox::null(cfg);
    let handle = instantiate_null(&mut sb, "null.gain", RunMode::Realtime);
    let id = handle.id;
    let device = effect(handle, 0.25);
    let _ = sb.manager.poll();
    // No batch is processed: the child's audio thread idles through its wait schedule until it
    // sits at the longest wait, and the pulse it bumps is the only sign of life it gives.
    std::thread::sleep(Duration::from_millis(1500));
    let err = sb.manager.capture_state(id).unwrap_err();
    assert!(err.is_not_responding(), "{err}");
    assert_eq!(
        sb.backend.live_children(),
        1,
        "the idle child was not stopped"
    );
    assert_eq!(
        sb.manager.not_responding(id).as_deref(),
        Some("CaptureState")
    );
    #[allow(clippy::disallowed_methods)]
    // Justification: a bound on how long the test waits for the child.
    let limit = std::time::Instant::now() + Duration::from_secs(30);
    while sb.manager.not_responding(id).is_some() {
        let _ = sb.manager.poll();
        #[allow(clippy::disallowed_methods)]
        // Justification: as above.
        let now = std::time::Instant::now();
        assert!(now < limit, "the plugin answers in the end");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(sb.backend.live_children(), 1, "still the same child");
    drop(device);
    let _ = sb.manager.poll();
    sb.finish();
}

// ----- starting many plugins at once ------------------------------------------------------------

#[test]
// Justification: the test waits for child processes against the wall clock and measures a spawn.
#[allow(clippy::disallowed_methods)]
fn eight_null_plugins_start_together_and_a_ready_one_is_not_installed_until_taken() {
    const N: usize = 8;
    let mut sb = null_sandbox();
    let began = std::time::Instant::now();
    let mut tickets = Vec::new();
    let mut spawn = Vec::new();
    for _ in 0..N {
        let t = std::time::Instant::now();
        let ticket = sb
            .manager
            .begin_create(&request(&null_plugin("null.gain"), RunMode::Realtime))
            .expect("the child starts");
        spawn.push(t.elapsed());
        tickets.push(ticket);
    }
    let started = began.elapsed();
    // Each frame follows every creation; none of them is installed by being ready.
    let limit = std::time::Instant::now() + Duration::from_secs(60);
    let mut ready = vec![false; N];
    while ready.iter().any(|r| !r) {
        assert!(std::time::Instant::now() < limit, "{ready:?}");
        for (i, t) in tickets.iter().enumerate() {
            ready[i] = sb.manager.creation_ready(*t) == Some(true);
        }
        assert!(
            sb.manager.instance_ids().is_empty(),
            "a ready creation is not an instance yet"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut handles = Vec::new();
    for t in &tickets {
        handles.push(
            sb.manager
                .poll_create(*t)
                .expect("ready")
                .expect("the null plugin starts"),
        );
    }
    assert_eq!(sb.manager.instance_ids().len(), N);
    assert_eq!(sb.manager.creation_ready(tickets[0]), None, "spent");
    eprintln!(
        "spawn of {N} null children on the interface thread: total {started:?}, each {spawn:?}"
    );
    drop(handles);
    sb.finish();
}

#[test]
// Justification: the test waits for child processes against the wall clock and measures a spawn.
#[allow(clippy::disallowed_methods)]
fn a_start_that_is_skipped_at_once_leaves_no_child_and_the_others_come_up() {
    let mut sb = null_sandbox();
    let tickets: Vec<_> = (0..3)
        .map(|_| {
            sb.manager
                .begin_create(&request(&null_plugin("null.gain"), RunMode::Realtime))
                .expect("the child starts")
        })
        .collect();
    // The skip comes while the helper thread may still be starting the process.
    sb.manager.cancel_create(tickets[1]);
    assert_eq!(sb.manager.creation_ready(tickets[1]), None, "spent");
    let limit = std::time::Instant::now() + Duration::from_secs(60);
    let mut handles = Vec::new();
    for t in [tickets[0], tickets[2]] {
        let handle = loop {
            assert!(std::time::Instant::now() < limit, "not ready in time");
            sb.manager.poll();
            if let Some(answer) = sb.manager.poll_create(t) {
                break answer.expect("the null plugin starts");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        handles.push(handle);
    }
    // Only the two that were not skipped have a child; the skipped one's ends across polls.
    assert!(
        sb.wait_children(2, Duration::from_secs(30)),
        "{} children",
        sb.backend.live_children()
    );
    drop(handles);
    sb.finish();
}
