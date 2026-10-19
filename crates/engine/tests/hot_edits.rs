//! Hot editing: the persistent device arena, routing-only plans, the
//! ordered control queue, tempo changes, and that edits never reset untouched devices.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, Tempo};
use atomik_devices::{poly_synth, stereo_delay};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineConfig, EngineError, EngineOptions, Graph, OfflineJob,
    RenderPlan, ScheduledNote, Topology, new_engine_with_arena, new_engine_with_options,
    render_offline_scripted,
};
use atomik_testkit::fakes::{
    CALL_LOG, CONSTANT_SOURCE, CallLog, ConstantSource, DropProbe, PASSTHROUGH, TEMPO_PROBE,
    TempoProbe,
};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::hot::{Rig, energy, note};
use common::*;
use std::sync::mpsc;
use std::thread::JoinHandle;

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn out(t: &Topology, key: &str) -> atomik_engine::Endpoint {
    let d = t.descriptor(Graph::INTERFACE).unwrap();
    ep(Graph::INTERFACE, d.socket_by_key(key).unwrap().id)
}

/// An engine rendering on its own thread, stepped from the test (non-RT) thread.
struct Rt {
    frames: mpsc::Sender<usize>,
    output: mpsc::Receiver<Vec<Vec<f32>>>,
    join: JoinHandle<Engine>,
}

impl Rt {
    fn spawn(mut engine: Engine, outputs: usize) -> Self {
        let (frames, rx) = mpsc::channel::<usize>();
        let (tx, output) = mpsc::channel();
        let join = std::thread::spawn(move || {
            while let Ok(n) = rx.recv() {
                tx.send(render_blocks(&mut engine, &[], outputs, n, n))
                    .unwrap();
            }
            engine
        });
        Self {
            frames,
            output,
            join,
        }
    }

    /// Renders `n` host frames on the RT thread under the allocation guard.
    fn render(&self, n: usize) -> Vec<Vec<f32>> {
        self.frames.send(n).unwrap();
        self.output.recv().unwrap()
    }

    fn stop(self) -> Engine {
        drop(self.frames);
        self.join.join().expect("render thread")
    }
}

fn all(samples: &[f32], value: f32) -> bool {
    samples.iter().all(|s| s.to_bits() == value.to_bits())
}

#[test]
fn arena_insert_route_play_remove_drops_off_the_rt_thread() {
    let c = cfg(0, 2);
    let (engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 16);
    let (k, l, p) = (id(12), id(10), id(11));
    let (log, counts) = CallLog::boxed();
    let (probe, record) = DropProbe::boxed();
    handle.insert_device(l, log, &[(CallLog::P, 0.25)]).unwrap();
    // Prepared and configured on this (non-RT) thread, before anything renders.
    assert_eq!((counts.prepare(), counts.set_param()), (1, 1));
    assert_eq!(counts.last_param(), 0.25);
    handle.insert_device(p, probe, &[]).unwrap();
    handle
        .insert_device(
            k,
            ConstantSource::boxed(0.0),
            &[(ConstantSource::VALUE, 0.5)],
        )
        .unwrap();
    let mut t = Topology::new(&c);
    t.add_node(k, &CONSTANT_SOURCE).unwrap();
    t.add_node(l, &CALL_LOG).unwrap();
    t.connect(ep(k, ConstantSource::AUDIO_OUT), ep(l, CallLog::AUDIO_IN))
        .unwrap();
    t.connect(ep(l, CallLog::AUDIO_OUT), out(&t, "out_1"))
        .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();

    let rt = Rt::spawn(engine, 2);
    let first = rt.render(128); // batches 0 and 1; host 64..128 is timeline batch 0
    assert!(all(&first[0][64..], 0.5) && all(&first[1], 0.0));
    assert_eq!(counts.process(), 2);

    // Params address arena devices by id.
    handle.set_param(k, ConstantSource::VALUE, 0.75).unwrap();
    let second = rt.render(192); // shows batches 1..4: 0.5 then 0.75 from batch 2
    assert!(all(&second[0][..64], 0.5) && all(&second[0][64..], 0.75));

    // The never-routed probe is removed: handed back, dropped here by collect_garbage.
    handle.remove_device(p).unwrap();
    rt.render(64);
    assert_eq!(record.thread(), None, "the RT thread must not drop it");
    assert_eq!(handle.collect_garbage(), 1);
    assert_eq!(record.thread(), Some(std::thread::current().id()));

    // A routed device cannot be removed before a routing without it.
    assert_eq!(handle.remove_device(l), Err(EngineError::DeviceInUse));
    t.remove_node(l).unwrap();
    t.connect(ep(k, ConstantSource::AUDIO_OUT), out(&t, "out_1"))
        .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    handle.remove_device(l).unwrap();
    let third = rt.render(128);
    assert!(all(&third[0], 0.75));
    let processed = counts.process();
    rt.render(256);
    assert_eq!(
        counts.process(),
        processed,
        "removed devices are not processed"
    );
    assert_eq!(counts.reset(), 0, "nothing was ever reset");
    assert_eq!(handle.collect_garbage(), 2, "old routing + removed device");
    drop(rt.stop());
}

#[test]
fn unrouted_devices_are_not_processed_and_keep_their_state() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 8);
    let l = id(3);
    let (log, counts) = CallLog::boxed();
    handle.insert_device(l, log, &[]).unwrap();
    let mut routed = Topology::new(&c);
    routed.add_node(l, &CALL_LOG).unwrap();
    routed
        .connect(ep(l, CallLog::AUDIO_OUT), out(&routed, "out_1"))
        .unwrap();
    handle.submit_routing(routed.compile().unwrap()).unwrap();
    render_blocks(&mut engine, &[], 2, 128, 128);
    assert_eq!(counts.process(), 2);

    // Inserted but not routed: silent, unprocessed, and notes to it are dropped.
    handle
        .submit_routing(Topology::new(&c).compile().unwrap())
        .unwrap();
    let note = ScheduledNote {
        frame: 200,
        device: l,
        socket: CallLog::NOTES_IN,
        kind: NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    };
    handle.send_note(note).unwrap();
    render_blocks(&mut engine, &[], 2, 256, 64);
    assert_eq!(counts.process(), 2);
    assert_eq!(handle.dropped_events(), 1);
    assert_eq!(counts.notes_seen(), 0);

    // Routed again: it resumes where it was, never reset.
    handle.submit_routing(routed.compile().unwrap()).unwrap();
    handle
        .send_note(ScheduledNote { frame: 400, ..note })
        .unwrap();
    render_blocks(&mut engine, &[], 2, 128, 128);
    assert_eq!(counts.process(), 4);
    assert_eq!(counts.last_frame(), 448, "batches 6 and 7 ran");
    assert_eq!(counts.notes_seen(), 1);
    assert_eq!((counts.prepare(), counts.reset()), (1, 0));
    assert_eq!(handle.collect_garbage(), 2, "two retired routings");
}

#[test]
fn handle_rejects_invalid_edits_and_nothing_reaches_the_engine() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 8);
    handle
        .insert_device(id(1), ConstantSource::boxed(1.0), &[])
        .unwrap();
    assert_eq!(
        handle.insert_device(id(1), ConstantSource::boxed(2.0), &[]),
        Err(EngineError::DuplicateDevice)
    );
    assert_eq!(
        handle.insert_device(Graph::INTERFACE, ConstantSource::boxed(2.0), &[]),
        Err(EngineError::DuplicateDevice)
    );
    assert_eq!(handle.remove_device(id(9)), Err(EngineError::UnknownDevice));
    // A routing naming a device that was never inserted.
    let mut ghost = Topology::new(&c);
    ghost.add_node(id(1), &CONSTANT_SOURCE).unwrap();
    ghost.add_node(id(2), &CONSTANT_SOURCE).unwrap();
    ghost
        .connect(ep(id(2), ConstantSource::AUDIO_OUT), out(&ghost, "out_1"))
        .unwrap();
    assert_eq!(
        handle.submit_routing(ghost.compile().unwrap()),
        Err(EngineError::UnknownDevice)
    );
    // A routing whose descriptor for id 1 differs from the arena instance.
    let mut wrong = Topology::new(&c);
    wrong.add_node(id(1), &PASSTHROUGH).unwrap();
    assert_eq!(
        handle.submit_routing(wrong.compile().unwrap()),
        Err(EngineError::TypeMismatch)
    );
    let out = render_blocks(&mut engine, &[], 2, 256, 64);
    assert!(
        out.iter().all(|ch| all(ch, 0.0)),
        "no routing was installed"
    );
    assert_eq!(handle.collect_garbage(), 0);
}

#[test]
fn insert_beyond_arena_capacity_errors_on_the_handle_side() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 2);
    handle
        .insert_device(id(1), ConstantSource::boxed(0.1), &[])
        .unwrap();
    handle
        .insert_device(id(2), ConstantSource::boxed(0.2), &[])
        .unwrap();
    let (log, counts) = CallLog::boxed();
    assert_eq!(
        handle.insert_device(id(3), log, &[]),
        Err(EngineError::ArenaFull)
    );
    assert_eq!(counts.prepare(), 0, "rejected before prepare");
    let (probe, record) = DropProbe::boxed();
    assert_eq!(
        handle.insert_device(id(4), probe, &[]),
        Err(EngineError::ArenaFull)
    );
    assert_eq!(
        record.thread(),
        Some(std::thread::current().id()),
        "dropped right here"
    );
    // A plan larger than the arena is rejected too.
    let mut g = Graph::new(&c);
    for _ in 0..3 {
        g.add_device(ConstantSource::boxed(0.0));
    }
    assert_eq!(
        handle.submit_plan(g.compile().unwrap()),
        Err(EngineError::ArenaFull)
    );
    render_blocks(&mut engine, &[], 2, 128, 128);
    assert_eq!(
        handle.collect_garbage(),
        0,
        "nothing happened on the RT side"
    );
    // A removal frees the slot for the next insert (in queue order).
    handle.remove_device(id(1)).unwrap();
    handle
        .insert_device(id(5), ConstantSource::boxed(0.5), &[])
        .unwrap();
    let mut t = Topology::new(&c);
    t.add_node(id(5), &CONSTANT_SOURCE).unwrap();
    t.add_node(id(2), &CONSTANT_SOURCE).unwrap();
    t.connect(ep(id(5), ConstantSource::AUDIO_OUT), out(&t, "out_1"))
        .unwrap();
    t.connect(ep(id(2), ConstantSource::AUDIO_OUT), out(&t, "out_2"))
        .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    let out = render_blocks(&mut engine, &[], 2, 128, 128);
    assert!(all(&out[0][64..], 0.5) && all(&out[1][64..], 0.2));
    assert_eq!(handle.collect_garbage(), 1, "the removed device");
}

#[test]
fn submit_plan_and_arena_edits_mix() {
    let c = cfg(0, 2);
    let constant_plan = |v: f32| -> RenderPlan {
        let mut g = Graph::new(&c);
        let k = g.add_device(ConstantSource::boxed(v));
        g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
            .unwrap();
        g.compile().unwrap()
    };
    let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
    handle.submit_plan(constant_plan(0.25)).unwrap(); // device id 1
    handle
        .insert_device(id(100), ConstantSource::boxed(0.5), &[])
        .unwrap();
    let mut t = Topology::new(&c);
    t.add_node(id(1), &CONSTANT_SOURCE).unwrap();
    t.add_node(id(100), &CONSTANT_SOURCE).unwrap();
    t.connect(ep(id(1), ConstantSource::AUDIO_OUT), out(&t, "out_1"))
        .unwrap();
    t.connect(ep(id(100), ConstantSource::AUDIO_OUT), out(&t, "out_2"))
        .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    let first = render_blocks(&mut engine, &[], 2, 128, 128);
    assert!(all(&first[0][64..], 0.25) && all(&first[1][64..], 0.5));

    // A new plan replaces the whole arena; the old devices come back as one retired item.
    handle.submit_plan(constant_plan(0.125)).unwrap();
    let second = render_blocks(&mut engine, &[], 2, 128, 128);
    assert!(all(&second[0][64..], 0.125) && all(&second[1][64..], 0.0));
    assert_eq!(
        handle.collect_garbage(),
        2,
        "the first plan's routing, then that arena"
    );
    // Id 100 left with the old arena, so it can be inserted again.
    handle
        .insert_device(id(100), ConstantSource::boxed(0.5), &[])
        .unwrap();
}

#[test]
fn insert_prepares_and_renders_exactly_like_graph_compile() {
    // Same ids, devices and params through a Graph plan and through inserts + routing.
    let c = cfg(0, 2);
    let built = patches::real_patch(&c, 3);
    let notes = patches::notes_for(&built.note_targets, 6000);
    let job = OfflineJob {
        frames: 6000,
        host_block: 128,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let reference = atomik_engine::render_offline(built.plan, c, 6000, 128, &notes, &[]);

    // real_patch: synths 1..=3, delay 4, mixer 5, with the params below.
    let mut rig = Rig::new(&c);
    let synths: Vec<_> = (0..3u32)
        .map(|k| {
            rig.insert(
                0,
                k + 1,
                poly_synth::factory(),
                &[
                    ("osc1.wave", (k % 4) as f32),
                    ("filter.cutoff", 800.0 + 900.0 * k as f32),
                    ("lfo.to_cutoff", 1.0),
                ],
            )
        })
        .collect();
    let delay = rig.insert(
        0,
        4,
        stereo_delay::factory(),
        &[("time.mode", 0.0), ("time.ms", 37.0), ("feedback", 0.6)],
    );
    let mixer = rig.insert(0, 5, atomik_devices::line_mixer::factory(), &[]);
    rig.stereo(synths[0], "out_", delay, "in_");
    rig.stereo(delay, "out_", mixer, "ch1_in_");
    rig.stereo(synths[1], "out_", mixer, "ch2_in_");
    rig.stereo(synths[2], "out_", mixer, "ch3_in_");
    rig.connect((mixer, "master_l"), (Graph::INTERFACE, "out_1"));
    rig.connect((mixer, "master_r"), (Graph::INTERFACE, "out_2"));
    rig.route(0);
    let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
    let hot = render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
        assert_no_alloc(f)
    });
    assert!(energy(&reference[0], 0..6000) > 1.0);
    assert_eq!(bits(&hot), bits(&reference));
}

fn tempo_rig(c: &EngineConfig) -> Rig {
    let mut rig = Rig::new(c);
    rig.edits.push(atomik_engine::TimedEdit {
        frame: 0,
        edit: atomik_engine::HotEdit::Insert {
            id: id(1),
            device: TempoProbe::boxed(),
            params: vec![],
        },
    });
    rig.topo.add_node(id(1), &TEMPO_PROBE).unwrap();
    rig.connect((id(1), "audio_out"), (Graph::INTERFACE, "out_1"));
    rig.route(0);
    rig
}

#[test]
fn set_tempo_applies_at_the_next_batch_boundary() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
    let mut rig = tempo_rig(&c);
    for e in rig.edits.drain(..) {
        handle.schedule(e.frame, e.edit).unwrap();
    }
    let mut out = render_blocks(&mut engine, &[], 2, 100, 100).remove(0);
    handle.set_tempo(Tempo::new(140.0).unwrap()).unwrap();
    out.extend(render_blocks(&mut engine, &[], 2, 156, 13).remove(0));
    let timeline = &out[64..];
    assert!(
        all(&timeline[..64], 120.0),
        "batch 0 keeps the config tempo"
    );
    assert!(all(&timeline[64..], 140.0), "batch 1 onwards");
}

#[test]
fn timed_tempo_change_lands_on_the_first_boundary_at_or_after_its_frame() {
    let c = cfg(0, 2);
    for (frame, switch) in [(1000u64, 1024usize), (1024, 1024), (1025, 1088), (0, 0)] {
        let mut reference: Option<Vec<Vec<u32>>> = None;
        for block in [1usize, 64, 333, 1000] {
            let mut rig = tempo_rig(&c);
            rig.tempo(frame, 90.0);
            let job = OfflineJob {
                frames: 2048,
                host_block: block,
                notes: &[],
                params: &[],
                inputs: &[],
            };
            let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
            let out = render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
                assert_no_alloc(f)
            });
            assert!(
                all(&out[0][..switch], 120.0),
                "frame {frame}, block {block}"
            );
            assert!(all(&out[0][switch..], 90.0), "frame {frame}, block {block}");
            match &reference {
                None => reference = Some(bits(&out)),
                Some(r) => assert_eq!(&bits(&out), r, "block {block}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// a held note and a delay tail survive a routing edit on an unrelated chain.
// ---------------------------------------------------------------------------------------------

const F: u64 = 4000; // the edit lands at the boundary 4032
const FRAMES: usize = 12_000;

/// Chain A: synth 1 -> delay 2 (feedback) -> out_1/out_2. Chain B: synth 3 -> delay 4 ->
/// out_3/out_4. With `edit`, at frame F only chain B's cables change.
fn ac_u4_render(threads: u16, block: usize, edit: bool) -> Vec<Vec<f32>> {
    let c = cfg(0, 4);
    let mut rig = Rig::new(&c);
    let a = rig.insert(0, 1, poly_synth::factory(), &[("osc1.wave", 1.0)]);
    let ad = rig.insert(
        0,
        2,
        stereo_delay::factory(),
        &[("time.mode", 0.0), ("time.ms", 37.0), ("feedback", 0.6)],
    );
    let b = rig.insert(0, 3, poly_synth::factory(), &[("osc1.wave", 2.0)]);
    let bd = rig.insert(
        0,
        4,
        stereo_delay::factory(),
        &[("time.mode", 0.0), ("time.ms", 120.0), ("feedback", 0.3)],
    );
    rig.stereo(a, "out_", ad, "in_");
    rig.wire_outputs(ad, "out_1", "out_2");
    rig.stereo(b, "out_", bd, "in_");
    rig.wire_outputs(bd, "out_3", "out_4");
    rig.route(0);
    if edit {
        rig.disconnect((Graph::INTERFACE, "out_3"));
        rig.disconnect((Graph::INTERFACE, "out_4"));
        rig.connect((b, "out_l"), (Graph::INTERFACE, "out_4"));
        rig.connect((b, "out_r"), (Graph::INTERFACE, "out_3"));
        rig.route(F);
    }
    // A held note on chain A, and a note that ends before F so its delay tail rings across it.
    let notes = [
        note(0, a, true, 60),
        note(0, a, true, 67),
        note(100, b, true, 48),
        note(2500, b, false, 48),
        note(3000, a, false, 67),
    ];
    let job = OfflineJob {
        frames: FRAMES,
        host_block: block,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let (mut engine, mut handle) = engine_with(c, threads, None);
    render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
        assert_no_alloc(f)
    })
}

#[test]
fn ac_u4_routing_edit_on_an_unrelated_chain_keeps_the_untouched_chain_sample_exact() {
    for threads in [0u16, 3] {
        let reference = ac_u4_render(threads, 128, false);
        let edited = ac_u4_render(threads, 333, true);
        let boundary = 4032;
        assert!(
            energy(&reference[0], boundary..FRAMES) > 1.0,
            "held note audible"
        );
        // Chain A (out_1/out_2): identical before and after the edit.
        assert_eq!(
            bits(&edited[..2]),
            bits(&reference[..2]),
            "threads {threads}"
        );
        // Chain B (out_3/out_4): identical before the edit, rerouted after it.
        for ch in 2..4 {
            assert_eq!(
                bits(&[edited[ch][..boundary].to_vec()]),
                bits(&[reference[ch][..boundary].to_vec()])
            );
            assert_ne!(
                bits(&[edited[ch][boundary..].to_vec()]),
                bits(&[reference[ch][boundary..].to_vec()])
            );
        }
    }
}

/// Synth 1 -> delay 2 (wet only) -> out_1/out_2; the note ends at 1000. At 4096 either the
/// delay's input is unplugged (`fresh = false`: the same instance keeps ringing) or the delay
/// is removed and a fresh one (id 5, same params) takes its place (`fresh = true`).
fn reinsert_render(fresh: bool) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let params = [
        ("time.mode", 0.0),
        ("time.ms", 50.0),
        ("feedback", 0.8),
        ("mix", 1.0),
    ];
    let mut rig = Rig::new(&c);
    let s = rig.insert(0, 1, poly_synth::factory(), &[]);
    let d = rig.insert(0, 2, stereo_delay::factory(), &params);
    rig.stereo(s, "out_", d, "in_");
    rig.wire_outputs(d, "out_1", "out_2");
    rig.route(0);
    if fresh {
        rig.unroute_and_remove(4096, d);
        let d5 = rig.insert(4096, 5, stereo_delay::factory(), &params);
        rig.wire_outputs(d5, "out_1", "out_2");
    } else {
        rig.disconnect((d, "in_l"));
        rig.disconnect((d, "in_r"));
    }
    rig.route(4096);
    let notes = [note(0, s, true, 60), note(1000, s, false, 60)];
    let job = OfflineJob {
        frames: 8192,
        host_block: 100,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
    let out = render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
        assert_no_alloc(f)
    });
    assert_eq!(
        handle.collect_garbage(),
        0,
        "the helper collects as it goes"
    );
    out
}

#[test]
fn ac_u4_reinserted_device_starts_from_reset_state() {
    let kept = reinsert_render(false);
    let fresh = reinsert_render(true);
    assert_eq!(
        bits(&[kept[0][..4096].to_vec()]),
        bits(&[fresh[0][..4096].to_vec()])
    );
    assert!(
        energy(&kept[0], 4096..8192) > 0.1,
        "the kept delay keeps ringing"
    );
    assert!(
        fresh
            .iter()
            .all(|ch| ch[4096..].iter().all(|s| s.abs().to_bits() == 0)),
        "a fresh delay with no input is silent: its state starts from reset"
    );
}
