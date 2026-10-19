//! The slot, the message ring, the hand-shake rings, the reaper queue, the event list and the
//! `NullBackend`.

use atomik_core::{BATCH, ExprKind};
use atomik_device_sdk::PluginStateBlob;
use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, EventList, HostHandle, InstanceId, LogLevel, NullBackend,
    NullBehavior, NullPlugin, PluginBackend, PluginEventKind, PluginIo, PluginMessage,
    PluginTransport, ProcessStatus, ReaperQueue, plugin_channel,
};
use std::path::Path;

mod support;
use support::*;

// ----- the event list -------------------------------------------------------------------------

#[test]
fn the_event_list_clamps_offsets_keeps_them_non_decreasing_and_counts_drops() {
    let mut l = EventList::with_capacity(3);
    assert!(l.push(10, PluginEventKind::PitchBend { value: 0.0 }));
    assert!(
        l.push(4, PluginEventKind::PitchBend { value: 0.1 }),
        "never earlier"
    );
    assert!(
        l.push(500, PluginEventKind::PitchBend { value: 0.2 }),
        "clamped to the batch"
    );
    assert!(
        !l.push(1, PluginEventKind::PitchBend { value: 0.3 }),
        "full"
    );
    let offsets: Vec<u16> = l.as_slice().iter().map(|e| e.offset).collect();
    assert_eq!(offsets, vec![10, 10, BATCH as u16 - 1]);
    assert_eq!(l.len(), 3);
    assert_eq!(l.capacity(), 3);
    assert_eq!(l.dropped(), 1);
    l.clear();
    assert!(l.is_empty());
    assert_eq!(
        l.dropped(),
        1,
        "the drop count survives a clear until taken"
    );
    assert_eq!(l.take_dropped(), 1);
    assert_eq!(l.dropped(), 0);
    assert!(
        l.push(2, PluginEventKind::PitchBend { value: 0.0 }),
        "offsets restart"
    );
    let _ = ExprKind::Pan;
}

// ----- the slot and the message ring -----------------------------------------------------------

#[test]
fn messages_flow_from_any_thread_to_the_manager_and_a_full_ring_drops_and_counts() {
    let (_device, manager) = plugin_channel(InstanceId(7));
    assert_eq!(manager.slot().id(), InstanceId(7));
    let host = manager.messenger();
    let h2 = host.clone();
    let t = std::thread::spawn(move || h2.post(PluginMessage::LatencyChanged));
    assert!(t.join().unwrap());
    assert!(host.post(PluginMessage::Log {
        level: LogLevel::Info,
        text: "hello".to_string()
    }));
    assert_eq!(manager.try_recv(), Some(PluginMessage::LatencyChanged));
    assert!(matches!(
        manager.try_recv(),
        Some(PluginMessage::Log { .. })
    ));
    assert_eq!(manager.try_recv(), None);
    // Fill the ring: further posts are refused, never blocked, and counted.
    let mut accepted = 0;
    for _ in 0..10_000 {
        if host.post(PluginMessage::StateDirty) {
            accepted += 1;
        }
    }
    assert!((64..10_000).contains(&accepted), "{accepted}");
    assert_eq!(host.dropped(), 10_000 - accepted as u64);
    assert_eq!(manager.dropped_messages(), host.dropped());
    // Draining makes room again.
    while manager.try_recv().is_some() {}
    assert!(host.post(PluginMessage::StateDirty));
}

#[test]
fn the_slot_atomics_round_trip() {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let s = manager.slot();
    assert!(s.powered(), "powered by default");
    s.set_powered(false);
    assert!(!s.powered());
    assert!(!s.pinned());
    s.set_pinned(true);
    assert!(s.pinned());
    assert_eq!(s.latency(), 0);
    assert!(!s.set_latency(100, 48_000.0));
    assert_eq!(s.latency(), 100);
    assert!(s.set_latency(96_000, 48_000.0));
    assert_eq!(s.latency(), 48_000);
    assert!(!s.stop_requested());
    s.request_stop();
    assert!(s.stop_requested());
    s.resume_run();
    assert!(!s.stop_requested());
}

// ----- the reaper ------------------------------------------------------------------------------

#[test]
fn the_reaper_queue_is_shared_by_its_clones() {
    let q = ReaperQueue::new();
    let q2 = q.clone();
    assert!(q.drain().is_empty());
    let (probe, _s) = Probe::new();
    let (device_end, _manager) = plugin_channel(InstanceId(3));
    q2.post(atomik_plugin_host::ReapItem {
        slot: device_end.slot().clone(),
        processor: Some(Box::new(probe)),
    });
    let items = q.drain();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slot.id(), InstanceId(3));
    assert!(q2.drain().is_empty());
}

// ----- the null backend ------------------------------------------------------------------------

fn clap(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

fn null_backend() -> NullBackend {
    NullBackend::new().with_plugin(NullPlugin {
        path: "plugins/gain.clap".into(),
        plugin: clap("null.gain"),
        layout: fx_layout(),
        behavior: NullBehavior::Gain(0.5),
        latency: 12,
    })
}

#[test]
fn the_null_backend_scans_instantiates_activates_and_counts() {
    let backend = null_backend();
    assert_eq!(backend.format(), PluginFormat::Clap);
    let scanned = backend.scan_file(Path::new("plugins/gain.clap")).unwrap();
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].plugin, clap("null.gain"));
    assert_eq!(scanned[0].layout.name, "Test Gain");
    assert!(matches!(
        backend.scan_file(Path::new("plugins/other.clap")),
        Err(BackendError::NotAPlugin(_))
    ));

    let (_dev, manager) = plugin_channel(InstanceId(1));
    let host = HostHandle::new(manager.messenger());
    assert!(backend.instantiate(&clap("nope"), host.clone()).is_err());
    let stats = backend.stats();
    assert_eq!((stats.created(), stats.destroyed()), (0, 0));
    let mut inst = backend.instantiate(&clap("null.gain"), host).unwrap();
    assert_eq!(stats.created(), 1);
    assert_eq!(inst.layout().name, "Test Gain");
    assert_eq!(inst.latency_frames(), 12);

    let mut proc = inst
        .activate(&ActivateConfig {
            sample_rate: 48_000.0,
            frames: 64,
            mode: RunMode::Realtime,
        })
        .unwrap();
    assert_eq!(stats.activated(), 1);
    assert!(proc.start_processing());
    // Gain 0.5: the processor halves its input.
    let input = [[1.0; BATCH]; 2];
    let mut out = [[0.0; BATCH]; 2];
    let mut events_out = EventList::with_capacity(4);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &input,
        main_in_silent: false,
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut out,
        events_in: &[],
        events_out: &mut events_out,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    assert_eq!(proc.process(&mut io), ProcessStatus::Continue);
    assert!(out[0].iter().all(|s| s.to_bits() == 0.5f32.to_bits()));
    proc.stop_processing();
    inst.deactivate(proc);
    assert_eq!(stats.deactivated(), 1);
    drop(inst);
    assert_eq!(stats.destroyed(), 1);
}

#[test]
fn the_null_instance_keeps_state_text_and_gui_hooks() {
    let backend = null_backend();
    let (_dev, manager) = plugin_channel(InstanceId(1));
    let mut inst = backend
        .instantiate(&clap("null.gain"), HostHandle::new(manager.messenger()))
        .unwrap();
    let blob = PluginStateBlob(vec![9, 8, 7]);
    inst.restore_state(&blob).unwrap();
    assert_eq!(inst.capture_state().unwrap(), blob);
    assert_eq!(inst.param_text(1, 0.5).as_deref(), Some("0.5"));
    inst.poll();
    assert!(matches!(
        inst.open_gui(atomik_plugin_host::NativeParent::Win32(0), 1.0),
        Err(BackendError::Unsupported(_))
    ));
    inst.close_gui();
}

#[test]
fn the_null_behaviours_are_silent_passthrough_and_gain() {
    for (behavior, want) in [
        (NullBehavior::Silent, 0.0f32),
        (NullBehavior::Passthrough, 1.0),
        (NullBehavior::Gain(0.25), 0.25),
    ] {
        let backend = NullBackend::new().with_plugin(NullPlugin {
            path: "p.clap".into(),
            plugin: clap("b"),
            layout: fx_layout(),
            behavior,
            latency: 0,
        });
        let (_dev, manager) = plugin_channel(InstanceId(1));
        let mut inst = backend
            .instantiate(&clap("b"), HostHandle::new(manager.messenger()))
            .unwrap();
        let mut proc = inst
            .activate(&ActivateConfig {
                sample_rate: 48_000.0,
                frames: 64,
                mode: RunMode::Offline,
            })
            .unwrap();
        let input = [[1.0; BATCH]; 2];
        let mut out = [[9.0; BATCH]; 2];
        let mut events_out = EventList::with_capacity(1);
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &input,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut out,
            events_in: &[],
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        proc.process(&mut io);
        assert!(
            out[1].iter().all(|s| s.to_bits() == want.to_bits()),
            "{want}"
        );
        inst.deactivate(proc);
    }
}
