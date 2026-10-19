//! Shared helpers for engine integration tests.
#![allow(dead_code)] // Justification: each test binary uses a different subset of helpers.

pub mod audio;
pub mod hot;
pub mod patches;
pub mod song;

use atomik_core::{SampleRate, Tempo};
use atomik_device_sdk::SocketId;
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineHandle, EngineOptions, Graph,
    WorkerConfig, new_engine_with_options,
};
use atomik_testkit::no_alloc::assert_no_alloc;

/// 48 kHz, 120 BPM, `inputs` in / `outputs` out, seed 1.
pub fn cfg(inputs: u16, outputs: u16) -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: inputs,
        output_channels: outputs,
        seed: 1,
    }
}

/// Endpoint shorthand.
pub fn ep(device: DeviceInstanceId, socket: SocketId) -> Endpoint {
    Endpoint { device, socket }
}

/// Endpoint of an AudioInterface socket by key.
pub fn iface(g: &Graph, key: &str) -> Endpoint {
    let s = g
        .descriptor(Graph::INTERFACE)
        .unwrap()
        .socket_by_key(key)
        .unwrap();
    ep(Graph::INTERFACE, s.id)
}

/// Renders one host block under the allocation guard and returns the output channels.
pub fn render_block(
    engine: &mut Engine,
    inputs: &[Vec<f32>],
    outputs: usize,
    frames: usize,
) -> Vec<Vec<f32>> {
    let ins: Vec<&[f32]> = inputs.iter().map(|c| &c[..frames]).collect();
    let mut outs = vec![vec![f32::NAN; frames]; outputs];
    {
        let mut refs: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
        assert_no_alloc(|| engine.render(&ins, &mut refs));
    }
    outs
}

/// Renders `total` frames in host blocks of `block`, concatenating the outputs.
pub fn render_blocks(
    engine: &mut Engine,
    inputs: &[Vec<f32>],
    outputs: usize,
    total: usize,
    block: usize,
) -> Vec<Vec<f32>> {
    let mut result = vec![Vec::with_capacity(total); outputs];
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        let ins: Vec<Vec<f32>> = inputs.iter().map(|c| c[done..done + n].to_vec()).collect();
        let outs = render_block(engine, &ins, outputs, n);
        for (r, o) in result.iter_mut().zip(outs) {
            r.extend_from_slice(&o);
        }
        done += n;
    }
    result
}

/// Asserts two signals are equal within `tol`.
pub fn assert_close(a: &[f32], b: &[f32], tol: f32) {
    assert_eq!(a.len(), b.len(), "length");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert!((x - y).abs() <= tol, "sample {i}: {x} vs {y}");
    }
}

/// An engine with `threads` workers, each running `on_thread_start` first.
pub fn engine_with(
    cfg: EngineConfig,
    threads: u16,
    on_thread_start: Option<fn()>,
) -> (Engine, EngineHandle) {
    new_engine_with_options(
        cfg,
        EngineOptions {
            workers: WorkerConfig {
                threads,
                on_thread_start,
            },
            ..EngineOptions::default()
        },
    )
}

/// Bit patterns of every sample, for bit-identity comparisons.
pub fn bits(channels: &[Vec<f32>]) -> Vec<Vec<u32>> {
    channels
        .iter()
        .map(|c| c.iter().map(|s| s.to_bits()).collect())
        .collect()
}
