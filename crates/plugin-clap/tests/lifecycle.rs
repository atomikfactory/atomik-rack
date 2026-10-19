//! Against the real CLAP backend through the `PluginManager`:
//! lifecycle, restart hand-shake, latency and param-table changes, the reaper, engine restarts
//! that keep instances, and the callbacks plugins make into the host.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{InstanceId, LogLevel, ManagerEvent, PluginHandle, PluginManager};
use atomik_test_plugin_core as core;
use atomik_testkit::harness::DeviceHarness;
use support::*;

const PKG: &str = "atomik-test-clap";

fn open(m: &mut PluginManager, id: &str) -> (DeviceHarness, InstanceId, usize) {
    let handle: PluginHandle = m.instantiate(&request(id)).expect("instantiates");
    let native = m.native_handle(handle.id).unwrap();
    (
        DeviceHarness::new(Box::new(handle.device), SR),
        handle.id,
        native,
    )
}

fn run(h: &mut DeviceHarness, n: usize) {
    for _ in 0..n {
        h.process();
    }
}

fn silent(h: &DeviceHarness) -> bool {
    h.output("out_l").iter().all(|s| *s == 0.0)
}

// ----- create, activate, tear down --------------------------------------------------------------

#[test]
fn an_instance_is_created_activated_and_after_its_device_is_dropped_deactivated_and_destroyed() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let before = (
        lib.counter(counter::CREATED),
        lib.counter(counter::ACTIVATED),
        lib.counter(counter::DEACTIVATED),
        lib.counter(counter::DESTROYED),
    );
    let (mut h, id, _) = open(&mut m, "atomik.test.gain");
    assert_eq!(lib.counter(counter::CREATED), before.0 + 1);
    assert_eq!(lib.counter(counter::ACTIVATED), before.1 + 1);
    run(&mut h, 3);
    drop(h);
    assert_eq!(
        lib.counter(counter::DESTROYED),
        before.3,
        "not on the dropping thread"
    );
    let events = m.poll();
    assert_eq!(events, vec![ManagerEvent::Destroyed(id)]);
    assert_eq!(lib.counter(counter::DEACTIVATED), before.2 + 1);
    assert_eq!(lib.counter(counter::DESTROYED), before.3 + 1);
    assert_eq!(m.instance_count(), 0);
}

#[test]
fn a_device_removed_from_a_running_engine_is_torn_down_by_the_reaper() {
    use atomik_core::{SampleRate, Tempo};
    use atomik_engine::{DeviceInstanceId, EngineConfig, new_engine};
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let handle = m.instantiate(&request("atomik.test.gain")).unwrap();
    let id = handle.id;
    let cfg = EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let (mut engine, mut eh) = new_engine(cfg);
    eh.insert_device(DeviceInstanceId(1), Box::new(handle.device), &[])
        .unwrap();
    let (mut l, mut r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
    engine.render(&[], &mut [&mut l, &mut r]);
    let destroyed = lib.counter(counter::DESTROYED);
    eh.remove_device(DeviceInstanceId(1)).unwrap();
    engine.render(&[], &mut [&mut l, &mut r]);
    assert_eq!(eh.collect_garbage(), 1, "the engine handed the device back");
    assert_eq!(m.poll(), vec![ManagerEvent::Destroyed(id)]);
    assert_eq!(lib.counter(counter::DESTROYED), destroyed + 1);
}

// ----- restart hand-shake ----------------------------------------------------------------------------

#[test]
fn a_sample_rate_change_restarts_the_plugin_at_the_new_rate_with_a_bounded_gap() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (mut h, id, native) = open(&mut m, "atomik.test.gain");
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [0.5; BATCH];
    run(&mut h, 3);
    assert!(!silent(&h));
    let created = lib.counter(counter::CREATED);
    m.set_sample_rate(44_100.0);
    // The device answers dry (an effect) for a bounded number of batches, then resumes.
    let mut gap = 0;
    let mut events = Vec::new();
    for _ in 0..8 {
        h.process();
        events.extend(m.poll());
        if events.contains(&ManagerEvent::Restarted(id)) {
            break;
        }
        gap += 1;
    }
    assert!(gap <= 2, "the gap was {gap} batches");
    assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
    run(&mut h, 2);
    assert_eq!(lib.probe_f64_raw(native, probe::SAMPLE_RATE), 44_100.0);
    assert_eq!(lib.probe_raw(native, probe::ACTIVATIONS), 2);
    assert_eq!(lib.probe_raw(native, probe::DEACTIVATIONS), 1);
    assert_eq!(
        lib.counter(counter::CREATED),
        created,
        "no re-instantiation"
    );
}

#[test]
fn a_latency_change_made_by_the_plugin_restarts_it_and_publishes_the_new_latency() {
    let _s = serial();
    let mut m = manager_for(&[PKG]);
    let (mut h, id, _) = open(&mut m, "atomik.test.gain");
    run(&mut h, 2);
    assert_eq!(m.slot(id).unwrap().latency(), 0);
    h.set_param("p3", 100.0); // the plugin applies it and asks for a restart
    let mut events = Vec::new();
    for _ in 0..8 {
        h.process();
        events.extend(m.poll());
    }
    assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
    assert!(
        events.contains(&ManagerEvent::LatencyChanged {
            instance: id,
            frames: 100
        }),
        "{events:?}"
    );
    assert_eq!(m.slot(id).unwrap().latency(), 100);
    assert_eq!(m.latency(id), Some(100));
    // The rack's value was re-sent after the restart, so the plugin still delays by it.
    *h.input_mut("in_l") = {
        let mut b = [0.0; BATCH];
        b[0] = 1.0;
        b
    };
    h.process();
    h.process();
    *h.input_mut("in_l") = [0.0; BATCH];
    let mut heard = Vec::new();
    for _ in 0..4 {
        h.process();
        heard.extend_from_slice(h.output("out_l"));
    }
    assert!(heard.iter().all(|s| s.is_finite()));
}

#[test]
fn the_host_latency_changed_callback_restarts_the_plugin_too() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (mut h, id, native) = open(&mut m, "atomik.test.gain");
    run(&mut h, 2);
    // A state with a 64 frame latency arrives while the plugin runs; the plugin tells the host.
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::LATENCY, 64.0);
    m.restore_state(id, &atomik_device_sdk::PluginStateBlob(fx.save_state()))
        .unwrap();
    assert_eq!(lib.command_raw(native, command::LATENCY_CHANGED, 0, 0), 1);
    let mut events = m.poll(); // begins the restart
    for _ in 0..6 {
        h.process();
        events.extend(m.poll());
    }
    assert!(
        events.contains(&ManagerEvent::LatencyChanged {
            instance: id,
            frames: 64
        }),
        "{events:?}"
    );
    assert_eq!(m.slot(id).unwrap().latency(), 64);
}

#[test]
fn a_plugin_that_adds_a_param_while_inactive_changes_the_layout_through_a_restart() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (mut h, id, _) = open(&mut m, "atomik.test.params");
    assert_eq!(m.layout(id).unwrap().params.len(), 2_000);
    let created = lib.counter(counter::CREATED);
    h.set_param("p1999", 1.0); // "Grow": the plugin asks for a restart
    let mut events = Vec::new();
    for _ in 0..10 {
        h.process();
        events.extend(m.poll());
    }
    let layout = events.iter().find_map(|e| match e {
        ManagerEvent::LayoutChanged { instance, layout } if *instance == id => Some(layout),
        _ => None,
    });
    let layout = layout.unwrap_or_else(|| panic!("no LayoutChanged in {events:?}"));
    assert_eq!(layout.params.len(), 2_001);
    assert_eq!(layout.params[2_000].id, 2_000);
    assert_eq!(m.layout(id).unwrap().params.len(), 2_001);
    assert_eq!(lib.counter(counter::CREATED), created, "the same instance");
}

#[test]
fn reload_after_a_failure_restarts_the_device() {
    let mut m = manager_for(&["atomik-test-clap-variants"]);
    let (mut h, id, _) = open(&mut m, "atomik.test.error");
    *h.input_mut("in_l") = [0.25; BATCH];
    run(&mut h, 2);
    let events = m.poll();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ManagerEvent::Failed { instance, .. } if *instance == id)),
        "{events:?}"
    );
    m.reload(id).unwrap();
    let mut events = Vec::new();
    for _ in 0..6 {
        h.process();
        events.extend(m.poll());
    }
    assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
}

// ----- engine restarts keep the instance ------------------------------------------------------------------

#[test]
fn an_engine_restart_re_activates_the_instance_without_creating_it_again() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let mut ids = Vec::new();
    let mut harnesses = Vec::new();
    for _ in 0..4 {
        let (h, id, _) = open(&mut m, "atomik.test.gain");
        m.retain(id, true);
        ids.push(id);
        harnesses.push(h);
    }
    let created = lib.counter(counter::CREATED);
    let destroyed = lib.counter(counter::DESTROYED);
    drop(harnesses); // the old engine goes away
    assert!(m.poll().is_empty(), "retained instances are not destroyed");
    assert_eq!(m.instance_count(), 4);
    for id in &ids {
        let handle = m.reattach(*id, 44_100.0).unwrap();
        let mut h = DeviceHarness::new(Box::new(handle.device), 44_100.0);
        run(&mut h, 2);
    }
    assert_eq!(
        lib.counter(counter::CREATED),
        created,
        "instance counter unchanged"
    );
    assert_eq!(lib.counter(counter::DESTROYED), destroyed);
}

// ----- callbacks into the host ----------------------------------------------------------------------------

fn from_another_thread(lib_path: std::path::PathBuf, handle: usize, cmd: u32, a: u64, b: u64) {
    std::thread::spawn(move || {
        // SAFETY: the test library is loaded by the test; the handle is a live instance.
        let l = unsafe { libloading::Library::new(&lib_path) }.unwrap();
        // SAFETY: the symbol is exported by every test plugin library with this signature.
        let f: libloading::Symbol<'_, unsafe extern "C" fn(usize, u32, u64, u64) -> u64> =
            unsafe { l.get(b"atomik_test_command\0") }.unwrap();
        // SAFETY: the handle is a live instance of this library.
        unsafe { f(handle, cmd, a, b) };
    })
    .join()
    .unwrap();
}

#[test]
fn thread_safe_host_callbacks_from_any_thread_arrive_as_events_on_the_main_thread() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (mut h, id, native) = open(&mut m, "atomik.test.gain");
    h.process();
    let _ = m.poll();
    let go = |cmd: u32, a: u64, b: u64| from_another_thread(lib.path.clone(), native, cmd, a, b);
    go(command::GUI_REQUEST_RESIZE, 420, 310);
    go(command::GUI_REQUEST_SHOW, 0, 0);
    go(command::GUI_REQUEST_HIDE, 0, 0);
    go(command::GUI_CLOSED, 1, 0);
    go(command::MARK_DIRTY, 0, 0);
    go(command::LOG, 2, 0);
    go(command::REQUEST_FLUSH, 0, 0);
    go(command::REQUEST_PROCESS, 0, 0);
    let events = m.poll();
    assert_eq!(
        events,
        vec![
            ManagerEvent::GuiResize {
                instance: id,
                w: 420,
                h: 310
            },
            ManagerEvent::GuiRequestShow(id),
            ManagerEvent::GuiRequestHide(id),
            ManagerEvent::GuiClosed(id),
            ManagerEvent::StateDirty(id),
            ManagerEvent::Log {
                instance: id,
                level: LogLevel::Warning,
                text: "test log".to_string()
            },
        ]
    );
}

#[test]
fn a_request_callback_from_another_thread_runs_on_main_thread_in_poll() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (_h, _id, native) = open(&mut m, "atomik.test.gain");
    let calls = lib.probe_raw(native, probe::MAIN_CALLBACKS);
    from_another_thread(lib.path.clone(), native, command::REQUEST_CALLBACK, 0, 0);
    assert_eq!(
        lib.probe_raw(native, probe::MAIN_CALLBACKS),
        calls,
        "not run on the calling thread"
    );
    m.poll();
    assert_eq!(lib.probe_raw(native, probe::MAIN_CALLBACKS), calls + 1);
    m.poll();
    assert_eq!(
        lib.probe_raw(native, probe::MAIN_CALLBACKS),
        calls + 1,
        "once"
    );
}

#[test]
fn the_host_answers_thread_check_per_thread() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (mut h, _id, native) = open(&mut m, "atomik.test.gain");
    // The audio callback is another thread than the main one.
    let h = std::thread::spawn(move || {
        run(&mut h, 20);
        h
    })
    .join()
    .unwrap();
    assert_eq!(lib.probe_raw(native, probe::PROCESS_CALLS), 20);
    assert_eq!(
        lib.probe_raw(native, probe::AUDIO_THREAD_TRUE),
        20,
        "audio thread inside process"
    );
    assert_eq!(
        lib.probe_raw(native, probe::MAIN_THREAD_TRUE),
        0,
        "never the main thread"
    );
    drop(h);
}
