//! `CallbackAdapter` de-interleaves, renders and re-interleaves exactly and without
//! allocating, for any host buffer size (including odd sizes, 0 and sizes above `max_frames`).

use atomik_audio_io::CallbackAdapter;
use atomik_core::{SampleRate, Tempo};
use atomik_device_sdk::SocketId;
use atomik_engine::{Endpoint, Engine, EngineConfig, EngineHandle, Graph, new_engine};
use atomik_testkit::no_alloc::assert_no_alloc;
use proptest::prelude::*;

atomik_testkit::install_alloc_guard!();

const LATENCY: usize = 64;

/// An engine whose interface routes `in_k` straight to `out_k` for every channel.
fn passthrough_engine(channels: u16) -> (Engine, EngineHandle) {
    let cfg = EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: channels,
        output_channels: channels,
        seed: 1,
    };
    let mut g = Graph::new(&cfg);
    for k in 0..channels {
        g.connect(
            Endpoint {
                device: Graph::INTERFACE,
                socket: SocketId(k),
            },
            Endpoint {
                device: Graph::INTERFACE,
                socket: SocketId(channels + k),
            },
        )
        .unwrap();
    }
    let (engine, mut handle) = new_engine(cfg);
    assert_eq!(engine.latency_frames() as usize, LATENCY);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle)
}

/// Feeds `input` (interleaved, `in_ch` channels) through the adapter in host blocks cycling
/// through `blocks`, then `LATENCY` frames of silence; returns the interleaved output.
fn run(
    adapter: &mut CallbackAdapter,
    engine: &mut Engine,
    input: &[f32],
    in_ch: usize,
    out_ch: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let frames = input.len() / in_ch;
    let total = frames + LATENCY;
    let mut padded = input.to_vec();
    padded.resize(total * in_ch, 0.0);
    let mut out = vec![f32::NAN; total * out_ch];
    let mut done = 0;
    let mut i = 0;
    while done < total {
        let n = blocks[i % blocks.len()].min(total - done);
        i += 1;
        let src = &padded[done * in_ch..(done + n) * in_ch];
        let dst = &mut out[done * out_ch..(done + n) * out_ch];
        assert_no_alloc(|| adapter.process(engine, src, dst));
        done += n;
    }
    out
}

/// Deterministic, non-trivial test signal (distinct per sample and channel).
fn signal(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|i| ((i * 7919) % 2003) as f32 / 1001.5 - 1.0)
        .collect()
}

fn assert_bits_eq(a: &[f32], b: &[f32]) {
    assert_eq!(a.len(), b.len(), "length");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i}: {x} vs {y}");
    }
}

#[test]
fn process_odd_block_sizes_round_trips_exactly() {
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(2, 2, 64);
    let x = signal(2000, 2);
    let out = run(
        &mut adapter,
        &mut engine,
        &x,
        2,
        2,
        &[1, 7, 63, 65, 100, 33],
    );
    assert!(out[..LATENCY * 2].iter().all(|s| s.to_bits() == 0));
    assert_bits_eq(&out[LATENCY * 2..], &x);
}

#[test]
fn process_buffers_larger_than_max_frames_are_chunked() {
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(2, 2, 16);
    let x = signal(3000, 2);
    let out = run(&mut adapter, &mut engine, &x, 2, 2, &[1000, 17, 513]);
    assert_bits_eq(&out[LATENCY * 2..], &x);
}

#[test]
fn process_zero_frames_is_a_no_op() {
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(2, 2, 64);
    let mut out: [f32; 0] = [];
    assert_no_alloc(|| adapter.process(&mut engine, &[], &mut out));
    // The engine did not advance: the first real sample still comes out after LATENCY frames.
    let x = signal(200, 2);
    let out = run(&mut adapter, &mut engine, &x, 2, 2, &[50]);
    assert_bits_eq(&out[LATENCY * 2..], &x);
}

#[test]
fn process_more_host_channels_than_engine_zero_fills_extra_outputs() {
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(4, 4, 64);
    let x = signal(500, 4);
    let out = run(&mut adapter, &mut engine, &x, 4, 4, &[37]);
    for f in 0..500 {
        let o = &out[(f + LATENCY) * 4..(f + LATENCY + 1) * 4];
        let i = &x[f * 4..(f + 1) * 4];
        assert_eq!(o[0].to_bits(), i[0].to_bits(), "frame {f} ch 0");
        assert_eq!(o[1].to_bits(), i[1].to_bits(), "frame {f} ch 1");
        assert_eq!(o[2].to_bits(), 0, "frame {f} ch 2");
        assert_eq!(o[3].to_bits(), 0, "frame {f} ch 3");
    }
}

#[test]
fn process_output_only_adapter_renders_engine_outputs() {
    // No host inputs: the engine's inputs read silence, outputs are still written.
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(0, 2, 64);
    let mut out = vec![f32::NAN; 300 * 2];
    for chunk in out.chunks_mut(2 * 75) {
        assert_no_alloc(|| adapter.process(&mut engine, &[], chunk));
    }
    assert!(out.iter().all(|s| s.to_bits() == 0));
}

#[test]
fn process_fewer_host_outputs_than_engine_drops_extra_channels() {
    let (mut engine, _h) = passthrough_engine(2);
    let mut adapter = CallbackAdapter::new(2, 1, 64);
    let x = signal(300, 2);
    let out = run(&mut adapter, &mut engine, &x, 2, 1, &[64]);
    let expected: Vec<f32> = x.chunks(2).map(|f| f[0]).collect();
    assert_bits_eq(&out[LATENCY..], &expected);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn process_interleaving_round_trips_exactly(
        channels in 1u16..=4,
        max_frames in 1usize..=128,
        frames in 0usize..=400,
        blocks in proptest::collection::vec(1usize..=300, 1..6),
        seed in any::<u32>(),
    ) {
        let ch = usize::from(channels);
        let (mut engine, _h) = passthrough_engine(channels);
        let mut adapter = CallbackAdapter::new(channels, channels, max_frames);
        let x: Vec<f32> = (0..frames * ch)
            .map(|i| f32::from_bits((seed as usize).wrapping_mul(2654435761).wrapping_add(i * 40503) as u32 & 0x3F7F_FFFF))
            .collect();
        let out = run(&mut adapter, &mut engine, &x, ch, ch, &blocks);
        prop_assert!(out[..LATENCY * ch].iter().all(|s| s.to_bits() == 0));
        for (i, (a, b)) in out[LATENCY * ch..].iter().zip(&x).enumerate() {
            prop_assert_eq!(a.to_bits(), b.to_bits(), "sample {}", i);
        }
    }
}
