//! Output is bit-identical for any host block size.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, ParamId};
use atomik_engine::{
    DeviceInstanceId, EngineConfig, Graph, OfflineJob, RenderPlan, ScheduledNote,
    render_offline_with,
};
use atomik_testkit::fakes::{ConstantSource, ImpulseOnNote, NoteThru, Passthrough};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::*;
use proptest::prelude::*;

const BLOCKS: [usize; 8] = [1, 7, 63, 64, 65, 100, 512, 4096];
const FRAMES: usize = 6000;

struct Patch {
    plan: RenderPlan,
    thru: DeviceInstanceId,
    impulse: DeviceInstanceId,
    chain: [DeviceInstanceId; 3],
}

/// in_1 -> p0 -> p1 -> p2 -> out_1   (Passthrough chain)
/// in_1 -> out_4                      (interface in to out)
/// const -> {out_2, p3 -> out_3}      (fan-out)
/// notes -> thru -> impulse -> {out_5, p4 -> out_6}; notes also go straight to impulse.
fn patch(c: &EngineConfig, gains: [f32; 5]) -> Patch {
    let mut g = Graph::new(c);
    let chain = [0, 1, 2].map(|k| g.add_device(Passthrough::boxed(gains[k])));
    let k = g.add_device(ConstantSource::boxed(0.3));
    let p3 = g.add_device(Passthrough::boxed(gains[3]));
    let thru = g.add_device(NoteThru::boxed());
    let impulse = g.add_device(ImpulseOnNote::boxed());
    let p4 = g.add_device(Passthrough::boxed(gains[4]));
    let a = Passthrough::AUDIO_IN;
    let o = Passthrough::AUDIO_OUT;
    g.connect(iface(&g, "in_1"), ep(chain[0], a)).unwrap();
    g.connect(ep(chain[0], o), ep(chain[1], a)).unwrap();
    g.connect(ep(chain[1], o), ep(chain[2], a)).unwrap();
    g.connect(ep(chain[2], o), iface(&g, "out_1")).unwrap();
    g.connect(iface(&g, "in_1"), iface(&g, "out_4")).unwrap();
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_2"))
        .unwrap();
    g.connect(ep(k, ConstantSource::AUDIO_OUT), ep(p3, a))
        .unwrap();
    g.connect(ep(p3, o), iface(&g, "out_3")).unwrap();
    g.connect(
        ep(thru, NoteThru::NOTES_OUT),
        ep(impulse, ImpulseOnNote::NOTES_IN),
    )
    .unwrap();
    g.connect(ep(impulse, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_5"))
        .unwrap();
    g.connect(ep(impulse, ImpulseOnNote::AUDIO_OUT), ep(p4, a))
        .unwrap();
    g.connect(ep(p4, o), iface(&g, "out_6")).unwrap();
    Patch {
        plan: g.compile().unwrap(),
        thru,
        impulse,
        chain,
    }
}

fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect()
}

fn render(
    block: usize,
    gains: [f32; 5],
    seed: u64,
    note_specs: &[(u64, bool)],
    param: f32,
) -> Vec<Vec<f32>> {
    let c = cfg(1, 6);
    let p = patch(&c, gains);
    let notes: Vec<ScheduledNote> = note_specs
        .iter()
        .map(|&(frame, direct)| ScheduledNote {
            frame,
            device: if direct { p.impulse } else { p.thru },
            socket: if direct {
                ImpulseOnNote::NOTES_IN
            } else {
                NoteThru::NOTES_IN
            },
            kind: NoteKind::On {
                key: 64,
                velocity: 0.9,
            },
        })
        .collect();
    let params = [(p.chain[1], ParamId(0), param)];
    let inputs = [noise(seed, FRAMES)];
    let job = OfflineJob {
        frames: FRAMES,
        host_block: block,
        notes: &notes,
        params: &params,
        inputs: &inputs,
    };
    render_offline_with(p.plan, c, &job, |f| assert_no_alloc(f))
}

fn bits(x: &[Vec<f32>]) -> Vec<Vec<u32>> {
    x.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn render_any_host_block_is_bit_identical(
        gains in proptest::array::uniform5(0.01f32..4.0),
        seed in any::<u64>(),
        notes in proptest::collection::vec((0u64..FRAMES as u64, any::<bool>()), 0..40),
        param in 0.0f32..4.0,
    ) {
        let reference = bits(&render(BLOCKS[0], gains, seed, &notes, param));
        prop_assert!(reference.iter().all(|ch| ch.len() == FRAMES));
        for block in &BLOCKS[1..] {
            let other = bits(&render(*block, gains, seed, &notes, param));
            prop_assert!(other == reference, "host block {} differs", block);
        }
    }
}

#[test]
fn render_determinism_patch_is_not_trivial() {
    let notes = [(10, true), (10, false), (4000, false)];
    let out = render(100, [0.7, 1.3, 0.9, 0.5, 2.0], 99, &notes, 1.1);
    assert!(out[0].iter().any(|s| s.abs() > 0.01), "chain carries noise");
    assert!(out[3].iter().any(|s| s.abs() > 0.01), "interface in to out");
    assert!((out[2][0] - 0.15).abs() < 1e-6, "fan-out through p3");
    assert!((out[4][10] - 1.0).abs() < 1e-7 && (out[4][4000] - 1.0).abs() < 1e-7);
    assert!((out[5][4000] - 2.0).abs() < 1e-6);
}
