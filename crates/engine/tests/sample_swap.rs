//! Engine part: sample maps reach devices through the
//! control queue (`EngineHandle::set_device_samples`, `HotEdit::Samples`) or, before compile,
//! through `Graph::set_device_samples`. Replaced maps and the maps of removed devices come back
//! to the handle and are freed by `collect_garbage`, never on the audio thread; every render
//! runs under `assert_no_alloc` (and workers under `forbid_on_this_thread`), and the output is
//! bit-identical across host block sizes and worker counts. Own binary: the worker violation
//! counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_device_sdk::{LoopMode, SampleBuffer, SampleMap, SampleMapRef, TriggerMode, Zone};
use atomik_devices::{line_mixer, sampler};
use atomik_engine::{
    CONTROL_QUEUE_CAPACITY, DeviceInstanceId, EngineError, Graph, HotEdit, OfflineJob,
    RETIRE_QUEUE_CAPACITY, ScheduledNote, TimedEdit, render_offline, render_offline_scripted,
};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::hot::{Rig, energy};
use common::*;
use std::sync::{Arc, Mutex};

const FRAMES: usize = 9000;

/// Two stereo buffers of deterministic waves; `shift` makes a same-shaped but distinct set.
fn buffers(shift: f32) -> Vec<Arc<SampleBuffer>> {
    [97.0f32, 131.0]
        .iter()
        .map(|&period| {
            let ch = |phase: f32| -> Vec<f32> {
                (0..6000)
                    .map(|i| {
                        0.5 * ((i as f32 + shift) * std::f32::consts::TAU / period + phase).sin()
                    })
                    .collect()
            };
            Arc::new(SampleBuffer::new(vec![ch(0.0), ch(1.0)].into()).unwrap())
        })
        .collect()
}

/// Both buffers layered over every key, looped forward with a crossfade.
fn map_of(bufs: &[Arc<SampleBuffer>], gain_db: f32) -> SampleMapRef {
    let zone = |buffer: u32| Zone {
        buffer,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 60,
        transpose: 0,
        tune_cents: 0.0,
        gain_db,
        pan: 0.0,
        start: 0,
        end: 6000,
        loop_mode: LoopMode::Forward,
        loop_start: 1000,
        loop_end: 5000,
        crossfade: 200,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: atomik_device_sdk::ZoneExt::NONE,
    };
    Arc::new(SampleMap::new(bufs.to_vec(), vec![zone(0), zone(1)]).unwrap())
}

fn note(frame: u64, device: DeviceInstanceId, on: bool, key: u8) -> ScheduledNote {
    let socket = sampler::factory()
        .descriptor
        .socket_by_key("notes_in")
        .unwrap()
        .id;
    ScheduledNote {
        frame,
        device,
        socket,
        kind: if on {
            NoteKind::On { key, velocity: 0.8 }
        } else {
            NoteKind::Off { key }
        },
    }
}

fn samples_edit(frame: u64, device: DeviceInstanceId, map: Option<SampleMapRef>) -> TimedEdit {
    TimedEdit {
        frame,
        edit: HotEdit::Samples { device, map },
    }
}

/// A sampler (id 1) wired to the interface outputs, as the hot path builds it.
fn single_rig(map: Option<SampleMapRef>) -> Rig {
    let mut rig = Rig::new(&cfg(0, 2));
    let s = rig.insert(0, 1, sampler::factory(), &[]);
    rig.wire_outputs(s, "out_1", "out_2");
    if map.is_some() {
        rig.edits.push(samples_edit(0, s, map));
    }
    rig.route(0);
    rig
}

/// Serializes the runs with workers: the violation counter is process-global, and the exit of one
/// run's workers must not land in another run's window.
static WORKER_RUNS: Mutex<()> = Mutex::new(());

fn run(rig: Rig, notes: &[ScheduledNote], threads: u16, block: usize) -> Vec<Vec<f32>> {
    let job = OfflineJob {
        frames: FRAMES,
        host_block: block,
        notes,
        params: &[],
        inputs: &[],
    };
    let _serial = WORKER_RUNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let baseline = worker_violations();
    let (mut engine, mut handle) = engine_with(cfg(0, 2), threads, Some(forbid_on_this_thread));
    let out = render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
        assert_no_alloc(f)
    });
    assert_eq!(worker_violations() - baseline, 0, "threads {threads}");
    drop(engine);
    out
}

#[test]
fn insert_then_samples_matches_the_graph_path_before_compile() {
    let bufs = buffers(0.0);
    let id = DeviceInstanceId(1);
    let notes = [
        note(0, id, true, 60),
        note(500, id, true, 67),
        note(4000, id, false, 60),
    ];
    // Plain path: the map is installed before compile.
    let mut g = Graph::new(&cfg(0, 2));
    let s = g.add_device((sampler::factory().create)());
    assert_eq!(s, id);
    g.set_device_samples(s, Some(map_of(&bufs, -6.0))).unwrap();
    for (out, key) in [("out_l", "out_1"), ("out_r", "out_2")] {
        let from = ep(s, g.descriptor(s).unwrap().socket_by_key(out).unwrap().id);
        g.connect(from, iface(&g, key)).unwrap();
    }
    let reference = render_offline(g.compile().unwrap(), cfg(0, 2), FRAMES, 64, &notes, &[]);
    assert!(
        energy(&reference[0], 0..2000) > 1.0,
        "the note sounds from the first batch"
    );
    // Hot path: insert, routing and samples in the same queue window.
    for (threads, block) in [(0u16, 64usize), (3, 1), (0, 100)] {
        let out = run(
            single_rig(Some(map_of(&bufs, -6.0))),
            &notes,
            threads,
            block,
        );
        assert_eq!(
            bits(&out),
            bits(&reference),
            "threads {threads}, block {block}"
        );
    }
    // A map set directly on the boxed device before `insert_device` is the same thing.
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 0, None);
    let mut rig = single_rig(None);
    let mut device = (sampler::factory().create)();
    assert!(device.swap_samples(Some(map_of(&bufs, -6.0))).is_none());
    handle.insert_device(id, device, &[]).unwrap();
    for e in rig.edits.drain(1..) {
        handle.schedule(e.frame, e.edit).unwrap();
    }
    for n in notes {
        handle.send_note(n).unwrap();
    }
    let out = render_blocks(&mut engine, &[], 2, FRAMES + 64, 64);
    assert_eq!(out[0][64..], reference[0][..]);
}

#[test]
fn a_swapped_out_map_is_freed_by_collect_garbage_not_the_render_thread() {
    let bufs = buffers(0.0);
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 0, None);
    let id = DeviceInstanceId(1);
    for e in single_rig(None).edits {
        handle.schedule(e.frame, e.edit).unwrap();
    }
    let first = map_of(&bufs, -6.0);
    let weak_first = Arc::downgrade(&first);
    handle.set_device_samples(id, Some(first)).unwrap();
    render_blocks(&mut engine, &[], 2, 256, 64);
    // Replaced while no voice plays: the device hands the old map back at once.
    handle
        .set_device_samples(id, Some(map_of(&bufs, -12.0)))
        .unwrap();
    render_blocks(&mut engine, &[], 2, 256, 64);
    assert!(
        weak_first.upgrade().is_some(),
        "the render thread did not free the replaced map"
    );
    assert!(handle.collect_garbage() >= 1);
    assert!(weak_first.upgrade().is_none(), "freed on the handle side");
    // Clearing the map hands the current one back the same way.
    handle.set_device_samples(id, None).unwrap();
    render_blocks(&mut engine, &[], 2, 128, 64);
    assert!(handle.collect_garbage() >= 1);
}

#[test]
fn a_removed_samplers_maps_are_dropped_on_the_handle_side() {
    let bufs = buffers(0.0);
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 0, None);
    let id = DeviceInstanceId(1);
    let mut rig = single_rig(None);
    let map = map_of(&bufs, -6.0);
    let weak_map = Arc::downgrade(&map);
    let weak_buf = Arc::downgrade(&bufs[0]);
    for e in std::mem::take(&mut rig.edits) {
        handle.schedule(e.frame, e.edit).unwrap();
    }
    handle.set_device_samples(id, Some(map)).unwrap();
    drop(bufs);
    render_blocks(&mut engine, &[], 2, 256, 64);
    assert!(weak_map.upgrade().is_some());
    // Routing without the sampler, then its removal.
    rig.topo.remove_node(id).unwrap();
    handle.submit_routing(rig.topo.compile().unwrap()).unwrap();
    handle.remove_device(id).unwrap();
    render_blocks(&mut engine, &[], 2, 256, 64);
    assert!(
        weak_map.upgrade().is_some(),
        "not freed by the render thread"
    );
    assert!(weak_buf.upgrade().is_some());
    handle.collect_garbage();
    assert!(weak_map.upgrade().is_none());
    assert!(weak_buf.upgrade().is_none());
}

#[test]
fn unknown_devices_and_a_full_queue_are_rejected_on_the_handle_side() {
    let bufs = buffers(0.0);
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 0, None);
    let map = map_of(&bufs, -6.0);
    let weak = Arc::downgrade(&map);
    assert_eq!(
        handle.set_device_samples(DeviceInstanceId(7), Some(map)),
        Err(EngineError::UnknownDevice)
    );
    assert!(weak.upgrade().is_none(), "dropped on the caller's side");
    // Queue order: a removed device is unknown at once.
    let id = DeviceInstanceId(1);
    let new = || (sampler::factory().create)();
    handle.insert_device(id, new(), &[]).unwrap();
    handle.set_device_samples(id, None).unwrap();
    handle.remove_device(id).unwrap();
    assert_eq!(
        handle.set_device_samples(id, None),
        Err(EngineError::UnknownDevice)
    );
    render_blocks(&mut engine, &[], 2, 128, 64);
    handle.collect_garbage();
    // Fill the control queue.
    handle.insert_device(id, new(), &[]).unwrap();
    let mut sent = 0;
    let (error, weak) = loop {
        let map = map_of(&bufs, -6.0);
        let weak = Arc::downgrade(&map);
        match handle.set_device_samples(id, Some(map)) {
            Ok(()) => sent += 1,
            Err(e) => break (e, weak),
        }
    };
    assert_eq!(error, EngineError::QueueFull);
    assert!(weak.upgrade().is_none(), "dropped on the caller's side");
    assert_eq!(sent, CONTROL_QUEUE_CAPACITY - 1);
}

#[test]
fn many_swaps_between_collections_never_block_lose_order_or_free_on_the_audio_thread() {
    let bufs = buffers(0.0);
    let id = DeviceInstanceId(1);
    let total = RETIRE_QUEUE_CAPACITY * 3 + 5;
    let note_on = [note(0, id, true, 60)];
    let last = map_of(&bufs, -9.0);
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 0, None);
    for e in single_rig(None).edits {
        handle.schedule(e.frame, e.edit).unwrap();
    }
    let mut weaks = Vec::new();
    for i in 0..total {
        let map = map_of(&bufs, -(i as f32 % 40.0));
        weaks.push(Arc::downgrade(&map));
        handle.set_device_samples(id, Some(map)).unwrap();
    }
    handle
        .set_device_samples(id, Some(Arc::clone(&last)))
        .unwrap();
    // Render without collecting: the retire queue fills, the engine parks one item and
    // postpones the rest, but `render` never blocks and frees nothing.
    render_blocks(&mut engine, &[], 2, 64 * 8, 64);
    assert_eq!(
        weaks.iter().filter(|w| w.upgrade().is_some()).count(),
        total,
        "no map was freed on the audio thread"
    );
    // Collect between renders until every swap has been applied, in order.
    for _ in 0..8 {
        handle.collect_garbage();
        render_blocks(&mut engine, &[], 2, 64 * 8, 64);
    }
    handle.collect_garbage();
    assert!(
        weaks.iter().all(|w| w.upgrade().is_none()),
        "every replaced map was freed by the handle"
    );
    // The last map won: a note sounds exactly as on an engine that only got that map.
    handle.send_note(note_on[0]).unwrap();
    let out = render_blocks(&mut engine, &[], 2, 2048, 64);
    let (mut fresh, mut fresh_handle) = engine_with(cfg(0, 2), 0, None);
    for e in single_rig(Some(last)).edits {
        fresh_handle.schedule(e.frame, e.edit).unwrap();
    }
    fresh_handle.send_note(note_on[0]).unwrap();
    let expected = render_blocks(&mut fresh, &[], 2, 2048, 64);
    assert!(energy(&out[0], 0..2048) > 1.0);
    assert_eq!(bits(&out), bits(&expected));
}

/// Two samplers through a mixer (so workers run them in parallel), with swaps while voices
/// sound: a map that shares the buffers (voices migrate), one with new buffers (voices drain),
/// a swap during the drain (cuts) and finally clearing the map.
fn swap_script() -> (Rig, Vec<ScheduledNote>) {
    let mut rig = Rig::new(&cfg(0, 2));
    let s1 = rig.insert(0, 1, sampler::factory(), &[]);
    let s2 = rig.insert(0, 2, sampler::factory(), &[]);
    let m = rig.insert(0, 3, line_mixer::factory(), &[]);
    rig.stereo(s1, "out_", m, "ch1_in_");
    rig.stereo(s2, "out_", m, "ch2_in_");
    rig.connect((m, "master_l"), (Graph::INTERFACE, "out_1"));
    rig.connect((m, "master_r"), (Graph::INTERFACE, "out_2"));
    let (a, other) = (buffers(0.0), buffers(17.0));
    rig.edits.push(samples_edit(0, s1, Some(map_of(&a, -6.0))));
    rig.edits.push(samples_edit(0, s2, Some(map_of(&a, -9.0))));
    rig.route(0);
    // Same buffers, other gain: sounding voices migrate to the new zones.
    rig.edits
        .push(samples_edit(2001, s1, Some(map_of(&a, -12.0))));
    // New buffers: sounding voices drain with a fade, new notes use the new map.
    rig.edits
        .push(samples_edit(4001, s1, Some(map_of(&other, -6.0))));
    rig.edits
        .push(samples_edit(4001, s2, Some(map_of(&a, -3.0))));
    // A second swap while voices still drain cuts them.
    rig.edits
        .push(samples_edit(4100, s1, Some(map_of(&buffers(33.0), -6.0))));
    // Clearing the map.
    rig.edits.push(samples_edit(7000, s1, None));
    let notes = vec![
        note(64, s1, true, 60),
        note(200, s1, true, 64),
        note(300, s2, true, 62),
        note(4500, s1, true, 67),
        note(4600, s2, true, 72),
        note(6000, s1, false, 60),
        note(6100, s2, false, 62),
    ];
    (rig, notes)
}

#[test]
fn swaps_while_voices_sound_are_bit_identical_and_allocation_free() {
    let go = |threads: u16, block: usize| {
        let (rig, notes) = swap_script();
        run(rig, &notes, threads, block)
    };
    let reference = go(0, 64);
    assert!(
        energy(&reference[0], 0..FRAMES) > 1.0,
        "the script is audible"
    );
    assert!(energy(&reference[0], 2100..2600) > 0.1);
    assert!(energy(&reference[0], 4600..5000) > 0.1);
    for threads in [0u16, 3] {
        for block in [1usize, 64, 100, 512] {
            for run in 0..2 {
                assert_eq!(
                    bits(&go(threads, block)),
                    bits(&reference),
                    "threads {threads}, block {block}, run {run}"
                );
            }
        }
    }
}
