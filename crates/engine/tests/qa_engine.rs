//! QA gap tests for the engine.
//!
//! * `qa_render_offline_params_apply_before_first_batch_guarded`: the guarded equivalent of
//!   `offline.rs::render_offline_params_apply_before_first_batch`, which calls the unguarded
//!   `render_offline` (was not checked on that path).
//! * `qa_ac_s3_*`: through the engine, with buffer slots reused by other devices, so the
//!   zeros cannot come from a freshly zeroed buffer.
//! * `qa_plan_swap_*`: a param queued together with a plan swap reaches the new plan.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Graph, OfflineJob, RenderPlan, new_engine, render_offline_with};
use atomik_testkit::fakes::{ConstantSource, Passthrough, RecordingSink};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::*;

#[test]
fn qa_render_offline_params_apply_before_first_batch_guarded() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(0.0));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let params = [(k, ConstantSource::VALUE, 0.5)];
    for host_block in [1, 64, 100] {
        let plan = {
            let mut g = Graph::new(&c);
            let k2 = g.add_device(ConstantSource::boxed(0.0));
            assert_eq!(k2, k);
            g.connect(ep(k2, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
                .unwrap();
            g.compile().unwrap()
        };
        let job = OfflineJob {
            frames: 500,
            host_block,
            notes: &[],
            params: &params,
            inputs: &[],
        };
        let out = render_offline_with(plan, c, &job, |f| assert_no_alloc(f));
        assert_eq!(out[0].len(), 500);
        assert!(
            out[0].iter().all(|s| (s - 0.5).abs() < 1e-7),
            "block {host_block}: param applies from frame 0"
        );
    }
    drop(g);
}

#[test]
fn qa_ac_s3_unconnected_input_reads_64_zeros_every_batch_despite_slot_reuse() {
    // A chain that writes non-zero data into slots that compile may reuse, then a sink with
    // no cable on its input. The sink records exactly what `ctx.input()` returned.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(0.9));
    let p1 = g.add_device(Passthrough::boxed(1.0));
    let p2 = g.add_device(Passthrough::boxed(-1.0));
    let (sink, rec) = RecordingSink::boxed(10 * 64);
    g.add_device(sink);
    g.connect(
        ep(k, ConstantSource::AUDIO_OUT),
        ep(p1, Passthrough::AUDIO_IN),
    )
    .unwrap();
    g.connect(
        ep(p1, Passthrough::AUDIO_OUT),
        ep(p2, Passthrough::AUDIO_IN),
    )
    .unwrap();
    g.connect(ep(p2, Passthrough::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let out = render_blocks(&mut engine, &[], 1, 10 * 64 + 64, 37);
    assert!(
        out[0][64..].iter().all(|s| (s + 0.9).abs() < 1e-7),
        "the chain really wrote non-zero data"
    );
    let seen = rec.samples();
    assert_eq!(seen.len(), 10 * 64, "ten full batches reached the sink");
    for (n, s) in seen.iter().enumerate() {
        assert_eq!(s.to_bits(), 0, "batch {}, sample {}: {s}", n / 64, n % 64);
    }
}

fn constant_plan(value: f32) -> (RenderPlan, atomik_engine::DeviceInstanceId) {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(value));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    (g.compile().unwrap(), k)
}

#[test]
fn qa_plan_swap_param_sent_with_the_plan_applies_to_the_new_plan_in_the_same_batch() {
    // Boundary order is: swap plans, then apply params (engine.rs `run_batch`).
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    let (a, _) = constant_plan(0.25);
    handle.submit_plan(a).unwrap();
    let mut out = render_blocks(&mut engine, &[], 1, 128, 128).remove(0);
    let (b, kb) = constant_plan(0.0);
    handle.submit_plan(b).unwrap();
    handle.set_param(kb, ConstantSource::VALUE, 0.5).unwrap();
    out.extend(render_blocks(&mut engine, &[], 1, 128, 7).remove(0));
    let timeline = &out[64..];
    assert!(
        timeline[..128].iter().all(|s| (s - 0.25).abs() < 1e-7),
        "plan A (batches 0 and 1)"
    );
    assert_eq!(timeline.len(), 192);
    assert!(
        timeline[128..].iter().all(|s| (s - 0.5).abs() < 1e-7),
        "plan B with its param from its first batch (never its default 0.0)"
    );
    assert_eq!(handle.collect_garbage(), 1);
}
