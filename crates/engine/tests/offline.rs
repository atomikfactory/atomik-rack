//! `render_offline` / `render_offline_with` (offline, length contract).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_engine::{Graph, OfflineJob, ScheduledNote, render_offline, render_offline_with};
use atomik_testkit::fakes::{ConstantSource, ImpulseOnNote};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::*;
use proptest::prelude::*;

/// Runs every `Engine::render` call of the offline loop under the allocation guard.
fn guarded(f: &mut dyn FnMut()) {
    assert_no_alloc(f);
}

#[test]
fn render_offline_length_equals_requested_frames() {
    for (frames, block) in [
        (0, 1),
        (1, 1),
        (1000, 1),
        (1000, 64),
        (1000, 333),
        (1000, 4096),
    ] {
        let c = cfg(1, 3);
        let mut g = Graph::new(&c);
        let k = g.add_device(ConstantSource::boxed(0.5));
        g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_2"))
            .unwrap();
        let job = OfflineJob {
            frames,
            host_block: block,
            notes: &[],
            params: &[],
            inputs: &[],
        };
        let out = render_offline_with(g.compile().unwrap(), c, &job, guarded);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|ch| ch.len() == frames), "{frames}/{block}");
        assert!(
            out[1].iter().all(|s| (s - 0.5).abs() < 1e-7),
            "latency compensated"
        );
        assert!(out[0].iter().chain(&out[2]).all(|s| s.to_bits() == 0));
    }
}

#[test]
fn interface_passthrough_is_identity_after_latency_compensation() {
    //
    let c = cfg(2, 2);
    let mut g = Graph::new(&c);
    g.connect(iface(&g, "in_1"), iface(&g, "out_1")).unwrap();
    let x: Vec<f32> = (0..3000)
        .map(|i| ((i * 7919) % 2003) as f32 / 1001.5 - 1.0)
        .collect();
    for block in [1, 63, 64, 65, 512, 4096] {
        let plan = {
            let mut g = Graph::new(&c);
            g.connect(iface(&g, "in_1"), iface(&g, "out_1")).unwrap();
            g.compile().unwrap()
        };
        let inputs = [x.clone()];
        let job = OfflineJob {
            frames: x.len(),
            host_block: block,
            notes: &[],
            params: &[],
            inputs: &inputs,
        };
        let out = render_offline_with(plan, c, &job, guarded);
        assert_close(&out[0], &x, 0.0);
        assert!(out[1].iter().all(|s| s.to_bits() == 0));
    }
    drop(g);
}

#[test]
fn render_offline_params_apply_before_first_batch() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(0.0));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let out = render_offline(
        g.compile().unwrap(),
        c,
        500,
        100,
        &[],
        &[(k, ConstantSource::VALUE, 0.5)],
    );
    assert_eq!(out[0].len(), 500);
    assert!(out[0].iter().all(|s| (s - 0.5).abs() < 1e-7));
}

#[test]
fn render_offline_accepts_unsorted_and_many_notes() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let imp = g.add_device(ImpulseOnNote::boxed());
    g.connect(ep(imp, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    // More notes than the default queue holds, given out of order.
    let mut notes: Vec<ScheduledNote> = (0..10_000u64)
        .map(|i| ScheduledNote {
            frame: (i * 3) % 30_000,
            device: imp,
            socket: ImpulseOnNote::NOTES_IN,
            kind: NoteKind::On {
                key: 1,
                velocity: 1.0,
            },
        })
        .collect();
    notes.reverse();
    let job = OfflineJob {
        frames: 30_000,
        host_block: 4096,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let out = render_offline_with(g.compile().unwrap(), c, &job, guarded);
    for (n, s) in out[0].iter().enumerate() {
        let want = if n % 3 == 0 { 1.0 } else { 0.0 };
        assert!((s - want).abs() < 1e-7, "frame {n}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// Through `render_offline`: output sample f is the impulse, every other is 0.
    #[test]
    fn render_offline_note_at_frame_lands_at_frame(frame in 0u64..5000, block in 1usize..600) {
        let c = cfg(0, 1);
        let mut g = Graph::new(&c);
        let imp = g.add_device(ImpulseOnNote::boxed());
        g.connect(ep(imp, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_1")).unwrap();
        let notes = [ScheduledNote {
            frame,
            device: imp,
            socket: ImpulseOnNote::NOTES_IN,
            kind: NoteKind::On { key: 60, velocity: 0.5 },
        }];
        let job = OfflineJob { frames: 5000, host_block: block, notes: &notes, params: &[], inputs: &[] };
        let out = render_offline_with(g.compile().unwrap(), c, &job, guarded);
        for (n, s) in out[0].iter().enumerate() {
            let want = if n as u64 == frame { 1.0 } else { 0.0 };
            prop_assert!((s - want).abs() < 1e-7, "frame {}: {}", n, s);
        }
    }
}
