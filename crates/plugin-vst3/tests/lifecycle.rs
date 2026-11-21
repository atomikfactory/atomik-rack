//! against both real backends through the `PluginManager`: lifecycle, restart hand-shake, latency and param-table changes, the reaper, engine
//! restarts that keep instances, and the callbacks plugins make into the host.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{
    InstanceId, LogLevel, ManagerEvent, PluginHandle, PluginManager, RESTART_BURST,
};
use atomik_test_plugin_core as core;
use atomik_testkit::harness::DeviceHarness;
use support::*;

fn open(
    m: &mut PluginManager,
    plugin: &atomik_device_sdk::PluginRef,
) -> (DeviceHarness, InstanceId, usize) {
    let handle: PluginHandle = m.instantiate(&request(plugin)).expect("instantiates");
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

/// Makes the plugin change its latency to `frames` by editing its latency param. Both test
/// effects announce the change themselves (a CLAP plugin asks for a restart, a VST3 plugin calls
/// `restartComponent(kLatencyChanged)`), the way real plugins do, so nothing else is needed.
fn change_latency(k: &Kit, h: &mut DeviceHarness, frames: f64) {
    let (host, _) = k.host_value(Plug::Gain, core::GainFx::LATENCY, frames);
    h.set_param("p3", host as f32);
}

// ----- create, activate, tear down --------------------------------------------------------------

#[test]
fn an_instance_is_created_activated_and_after_its_device_is_dropped_deactivated_and_destroyed() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let before = (
            lib.counter(counter::CREATED),
            lib.counter(counter::ACTIVATED),
            lib.counter(counter::DEACTIVATED),
            lib.counter(counter::DESTROYED),
        );
        let (mut h, id, _) = open(&mut m, &k.plugin(Plug::Gain));
        assert_eq!(
            lib.counter(counter::CREATED),
            before.0 + 1,
            "{}",
            format.name()
        );
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
}

#[test]
fn a_device_removed_from_a_running_engine_is_torn_down_by_the_reaper() {
    use atomik_core::{SampleRate, Tempo};
    use atomik_engine::{DeviceInstanceId, EngineConfig, new_engine};
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let handle = m.instantiate(&request(&k.plugin(Plug::Gain))).unwrap();
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
        assert_eq!(
            lib.counter(counter::DESTROYED),
            destroyed + 1,
            "{}",
            format.name()
        );
    }
}

// ----- restart hand-shake ----------------------------------------------------------------------------

#[test]
fn a_sample_rate_change_restarts_the_plugin_at_the_new_rate_with_a_bounded_gap() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.plugin(Plug::Gain));
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
        assert!(gap <= 2, "{}: the gap was {gap} batches", format.name());
        assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
        run(&mut h, 2);
        assert_eq!(
            lib.probe_raw(native, probe::SAMPLE_RATE),
            44_100f64.to_bits()
        );
        assert_eq!(lib.probe_raw(native, probe::ACTIVATIONS), 2);
        assert_eq!(lib.probe_raw(native, probe::DEACTIVATIONS), 1);
        assert_eq!(
            lib.counter(counter::CREATED),
            created,
            "no re-instantiation"
        );
    }
}

#[test]
fn a_latency_change_made_by_the_plugin_restarts_it_and_publishes_the_new_latency() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, _) = open(&mut m, &k.plugin(Plug::Gain));
        run(&mut h, 2);
        assert_eq!(m.slot(id).unwrap().latency(), 0);
        change_latency(&k, &mut h, 100.0);
        let mut events = Vec::new();
        for _ in 0..8 {
            h.process();
            events.extend(m.poll());
        }
        assert!(
            events.contains(&ManagerEvent::Restarted(id)),
            "{}: {events:?}",
            format.name()
        );
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
        assert!(
            heard.iter().any(|s| s.abs() > 0.5),
            "{}: the impulse comes out, delayed by the restored latency",
            format.name()
        );
    }
}

#[test]
fn the_hosts_latency_changed_callback_restarts_the_plugin_too() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.plugin(Plug::Gain));
        run(&mut h, 2);
        // A state with a 64 frame latency arrives while the plugin runs; the plugin tells the host.
        let mut fx = core::GainFx::new(SR);
        fx.set_param(core::GainFx::LATENCY, 64.0);
        m.restore_state(id, &wrap_state(format, &fx.save_state()))
            .unwrap();
        lib.command_raw(native, command::LATENCY_CHANGED, 0, 0);
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
            "{}: {events:?}",
            format.name()
        );
        assert_eq!(m.slot(id).unwrap().latency(), 64);
    }
}

#[test]
fn a_plugin_that_adds_a_param_changes_the_layout_through_a_restart() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.plugin(Plug::Params));
        assert_eq!(m.layout(id).unwrap().params.len(), 2_000);
        let created = lib.counter(counter::CREATED);
        match format {
            // "Grow": the CLAP plugin asks for a restart itself.
            Format::Clap => h.set_param("p1999", 1.0),
            // The VST3 plugin tells the host its parameter titles changed.
            Format::Vst3 => {
                lib.command_raw(native, command::RESCAN_PARAMS, 0, 0);
            }
        }
        let mut events = Vec::new();
        for _ in 0..10 {
            h.process();
            events.extend(m.poll());
        }
        let layout = events.iter().find_map(|e| match e {
            ManagerEvent::LayoutChanged { instance, layout } if *instance == id => Some(layout),
            _ => None,
        });
        let layout =
            layout.unwrap_or_else(|| panic!("{}: no LayoutChanged in {events:?}", format.name()));
        assert_eq!(layout.params.len(), 2_001);
        assert_eq!(layout.params[2_000].id, 2_000);
        assert_eq!(m.layout(id).unwrap().params.len(), 2_001);
        assert_eq!(lib.counter(counter::CREATED), created, "the same instance");
    }
}

#[test]
fn reload_after_a_failure_restarts_the_device() {
    // Only a CLAP error status fails a device; a VST3 non-OK result is refused (see process.rs).
    for format in [Format::Clap] {
        let k = kit_variants(format);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, _) = open(&mut m, &k.variant(Variant::Error));
        *h.input_mut("in_l") = [0.25; BATCH];
        // It fails after a run of bad blocks, not at the first one.
        run(&mut h, atomik_plugin_host::bad_run_limit(48_000.0) as usize);
        let events = m.poll();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ManagerEvent::Failed { instance, .. } if *instance == id)),
            "{}: {events:?}",
            format.name()
        );
        m.reload(id).unwrap();
        let mut events = Vec::new();
        for _ in 0..6 {
            h.process();
            events.extend(m.poll());
        }
        assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
    }
}

// ----- engine restarts keep the instance ------------------------------------------------------------------

#[test]
fn an_engine_restart_re_activates_the_instance_without_creating_it_again() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let mut ids = Vec::new();
        let mut harnesses = Vec::new();
        for _ in 0..4 {
            let (h, id, _) = open(&mut m, &k.plugin(Plug::Gain));
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
            "{}: instance counter unchanged",
            format.name()
        );
        assert_eq!(lib.counter(counter::DESTROYED), destroyed);
    }
}

// ----- callbacks into the host ----------------------------------------------------------------------------

fn from_another_thread(lib_path: std::path::PathBuf, handle: usize, cmd: u32, a: u64, b: u64) {
    std::thread::spawn(move || {
        let l = TestLib::open_path(&lib_path);
        l.command_raw(handle, cmd, a, b);
    })
    .join()
    .unwrap();
}

#[test]
fn vst3_host_callbacks_from_any_thread_arrive_as_events_on_the_main_thread() {
    let _s = serial();
    let k = kit(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    let mut m = manager_for(Format::Vst3, std::slice::from_ref(&k.path));
    let (mut h, id, native) = open(&mut m, &k.plugin(Plug::Gain));
    h.process();
    let _ = m.poll();
    let go = |cmd: u32, a: u64, b: u64| from_another_thread(lib.path.clone(), native, cmd, a, b);
    go(command::MARK_DIRTY, 0, 0);
    go(command::PARAM_VALUES_CHANGED, 0, 0);
    // The editor is not open, so a resize request has no frame to go through; open it first.
    let (w, hgt) = m.open_gui(id, native_parent(), 1.0).expect("opens");
    assert_eq!((w, hgt), (320, 200));
    go(command::GUI_REQUEST_RESIZE, 420, 310);
    let events = m.poll();
    assert_eq!(
        events,
        vec![
            ManagerEvent::StateDirty(id),
            ManagerEvent::StateDirty(id),
            ManagerEvent::GuiResize {
                instance: id,
                w: 420,
                h: 310
            },
        ]
    );
}

fn native_parent() -> atomik_plugin_host::NativeParent {
    if cfg!(windows) {
        atomik_plugin_host::NativeParent::Win32(0x1234)
    } else if cfg!(target_os = "macos") {
        atomik_plugin_host::NativeParent::Cocoa(0x1234)
    } else {
        atomik_plugin_host::NativeParent::X11(0x1234)
    }
}

#[test]
fn the_processor_runs_on_whichever_thread_the_device_is_driven_from() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, _id, native) = open(&mut m, &k.plugin(Plug::Gain));
        *h.input_mut("in_l") = [0.5; BATCH];
        // The audio callback is another thread than the main one.
        let h = std::thread::spawn(move || {
            run(&mut h, 20);
            h
        })
        .join()
        .unwrap();
        assert_eq!(
            lib.probe_raw(native, probe::PROCESS_CALLS),
            20,
            "{}",
            format.name()
        );
        drop(h);
    }
}

// ----- the module outlives the audio half -------------------------------------------------------

#[test]
fn a_device_that_outlives_the_manager_and_the_backend_keeps_the_module_loaded_until_it_is_dropped()
{
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        // A second handle on the library, to read its entry and exit counters (it also keeps the
        // file mapped, so a plugin released too late is seen in the counters, not as a crash).
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, _id, _) = open(&mut m, &k.plugin(Plug::Gain));
        run(&mut h, 3);
        let init = lib.counter(counter::ENTRY_INIT);
        let deinit = lib.counter(counter::ENTRY_DEINIT);
        // The host goes away first (an engine restart or the exit), the device last: its
        // processor is released after the manager and the backend that loaded the module.
        drop(m);
        assert_eq!(
            lib.counter(counter::ENTRY_DEINIT),
            deinit,
            "{}: the module's exit ran while a device still held its processor",
            format.name()
        );
        drop(h);
        assert_eq!(lib.counter(counter::ENTRY_INIT), init);
        if format == Format::Vst3 {
            assert_eq!(
                lib.counter(counter::ENTRY_DEINIT),
                deinit + 1,
                "the exit runs once the last device is gone"
            );
        }
    }
}

// ----- a restart only when something changed ----------------------------------------------------------

fn restarts(events: &[ManagerEvent], id: InstanceId) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ManagerEvent::Restarted(i) if *i == id))
        .count()
}

#[test]
fn a_latency_signal_that_changes_nothing_restarts_nothing() {
    let _s = serial();
    for format in FORMATS {
        let k = kit(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.plugin(Plug::Gain));
        run(&mut h, 2);
        let _ = m.poll();
        let mut events = Vec::new();
        for _ in 0..3 {
            lib.command_raw(native, command::LATENCY_CHANGED, 0, 0);
            for _ in 0..4 {
                h.process();
                events.extend(m.poll());
            }
        }
        assert_eq!(restarts(&events, id), 0, "{}: {events:?}", format.name());
        assert_eq!(lib.probe_raw(native, probe::ACTIVATIONS), 1);
        assert_eq!(lib.probe_raw(native, probe::DEACTIVATIONS), 0);
        assert_eq!(m.latency(id), Some(0));
    }
}

#[test]
fn a_plugin_that_announces_from_inside_its_activation_is_activated_once() {
    let _s = serial();
    for format in FORMATS {
        let k = kit_variants(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.variant(Variant::Chatty));
        let mut events = Vec::new();
        for _ in 0..40 {
            h.process();
            events.extend(m.poll());
        }
        // Without the rule every activation asks for the next one, for ever.
        assert_eq!(restarts(&events, id), 0, "{}: {events:?}", format.name());
        assert_eq!(lib.probe_raw(native, probe::ACTIVATIONS), 1);
    }
}

#[test]
fn a_plugin_that_asks_for_a_restart_after_every_block_is_stopped_after_a_burst() {
    let _s = serial();
    for format in FORMATS {
        let k = kit_variants(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.variant(Variant::Storm));
        *h.input_mut("in_l") = [0.25; BATCH];
        let mut events = Vec::new();
        for _ in 0..120 {
            h.process();
            events.extend(m.poll());
        }
        let name = format.name();
        assert_eq!(
            restarts(&events, id),
            RESTART_BURST as usize,
            "{name}: {events:?}"
        );
        let warnings = events
            .iter()
            .filter(|e| {
                matches!(e, ManagerEvent::Log { instance, level: LogLevel::Warning, text }
                    if *instance == id && text.contains("keeps asking"))
            })
            .count();
        assert_eq!(warnings, 1, "{name}: said once: {events:?}");
        // The plugin stays in its last activation and goes on processing.
        let calls = lib.probe_raw(native, probe::PROCESS_CALLS);
        run(&mut h, 8);
        assert!(
            lib.probe_raw(native, probe::PROCESS_CALLS) >= calls + 8,
            "{name}"
        );
        assert!(!silent(&h), "{name}");
        assert_eq!(
            lib.probe_raw(native, probe::ACTIVATIONS),
            1 + u64::from(RESTART_BURST)
        );
    }
}

#[test]
fn a_change_of_param_titles_alone_is_shown_without_a_restart() {
    let _s = serial();
    for format in FORMATS {
        let k = kit_variants(format);
        let lib = TestLib::open_path(&k.path);
        let mut m = manager_for(format, std::slice::from_ref(&k.path));
        let (mut h, id, native) = open(&mut m, &k.variant(Variant::Retitle));
        let name = format.name();
        assert_eq!(m.layout(id).unwrap().params[0].name, "Level");
        match format {
            // The plugin is asked for a main-thread callback and rescans the titles there.
            Format::Clap => lib.command_raw(native, CLAP_REQUEST_CALLBACK, 0, 0),
            // The plugin renames and tells the host `kParamTitlesChanged`.
            Format::Vst3 => lib.command_raw(native, command::RESCAN_PARAMS, 0, 0),
        };
        let mut events = Vec::new();
        for _ in 0..8 {
            h.process();
            events.extend(m.poll());
        }
        let titles = events.iter().find_map(|e| match e {
            ManagerEvent::ParamTitlesChanged { instance, layout } if *instance == id => {
                Some(layout)
            }
            _ => None,
        });
        let titles =
            titles.unwrap_or_else(|| panic!("{name}: no ParamTitlesChanged in {events:?}"));
        assert_eq!(titles.params[0].name, "Drive");
        assert_eq!(m.layout(id).unwrap().params[0].name, "Drive");
        assert_eq!(restarts(&events, id), 0, "{name}: {events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ManagerEvent::LayoutChanged { .. })),
            "{name}: {events:?}"
        );
        assert_eq!(lib.probe_raw(native, probe::ACTIVATIONS), 1, "{name}");
        assert_eq!(lib.probe_raw(native, probe::DEACTIVATIONS), 0, "{name}");
    }
}

/// The CLAP test command `request_callback()`.
const CLAP_REQUEST_CALLBACK: u32 = 6;
