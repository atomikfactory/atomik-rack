//! Non-RT offline rendering around [`Engine`](crate::Engine) (used by the CLI and the tests).

use crate::engine::{
    Capacities, DEFAULT_ARENA_CAPACITY, NOTE_QUEUE_CAPACITY, PARAM_QUEUE_CAPACITY, ScheduledNote,
    new_engine_with,
};
use crate::engine::{Engine, EngineHandle, TimedEdit};
use crate::graph::{DeviceInstanceId, EngineConfig};
use crate::plan::RenderPlan;
use atomik_core::ParamId;

/// What [`render_offline_with`] renders, besides the plan and config.
#[derive(Debug, Clone, Copy)]
pub struct OfflineJob<'a> {
    /// Timeline frames to produce per output channel.
    pub frames: usize,
    /// Host block size the engine is driven with (≥ 1). The output does not depend on it.
    pub host_block: usize,
    /// Notes to schedule, in any order (they are sorted by frame, stably).
    pub notes: &'a [ScheduledNote],
    /// `(device, param, plain)` values applied before the first batch.
    pub params: &'a [(DeviceInstanceId, ParamId, f32)],
    /// Host input channels (timeline-aligned); missing channels and frames read as silence.
    pub inputs: &'a [Vec<f32>],
}

/// Renders `frames` timeline frames of `plan` offline, driving the engine with host blocks of
/// `host_block` frames. `params` apply before the first batch; each note is fed through the
/// handle before the host block in which it is due.
///
/// The output is latency-compensated: `output[ch][n]` is timeline frame `n`, and every channel
/// has exactly `frames` samples. There is one channel per `cfg.output_channels`.
///
/// # Panics
/// If `host_block` is 0.
pub fn render_offline(
    plan: RenderPlan,
    cfg: EngineConfig,
    frames: usize,
    host_block: usize,
    notes: &[ScheduledNote],
    params: &[(DeviceInstanceId, ParamId, f32)],
) -> Vec<Vec<f32>> {
    let job = OfflineJob {
        frames,
        host_block,
        notes,
        params,
        inputs: &[],
    };
    render_offline_with(plan, cfg, &job, |render| render())
}

/// Renders `job` on a caller-made, fresh engine while applying scripted hot `edits` at their
/// frames. For the app's tests and QA: the same script gives the same output
/// for any `job.host_block` and worker count.
///
/// The caller creates the engine (so it chooses workers, arena size and `on_thread_start`,
/// and can check worker allocation counters before dropping it) and may submit an initial plan
/// or devices through `handle` before calling. Each edit goes through
/// [`EngineHandle::schedule`] before the host block in which it is due and is applied at the
/// first batch boundary at or after its frame; `job.params` apply before the first batch and
/// `job.notes` are fed like [`render_offline`] does. Every `Engine::render` call is wrapped by
/// `guard`, and `collect_garbage` runs (outside `guard`) after every host block. Output is
/// latency-compensated, one channel per engine output, `job.frames` samples each.
///
/// # Panics
/// If `job.host_block` is 0, if the engine has already rendered, or if an edit is rejected
/// (the error says which).
pub fn render_offline_scripted(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    mut edits: Vec<TimedEdit>,
    mut guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    assert!(
        job.host_block > 0,
        "render_offline_scripted: host_block must be at least 1"
    );
    assert_eq!(
        engine.host_frames(),
        0,
        "render_offline_scripted: the engine must be fresh"
    );
    let mut notes = job.notes.to_vec();
    notes.sort_by_key(|n| n.frame);
    edits.sort_by_key(|e| e.frame);
    let mut edits = edits.into_iter().peekable();
    for &(device, param, plain) in job.params {
        handle
            .set_param(device, param, plain)
            .expect("render_offline_scripted: param queue full");
    }
    let latency = engine.latency_frames() as usize;
    let total = job.frames + latency;
    let inputs = engine.input_channels();
    let mut out = vec![vec![0.0f32; total]; engine.output_channels()];
    let mut block_in = vec![vec![0.0f32; job.host_block]; inputs];
    let mut next_note = 0;
    let mut pos = 0;
    while pos < total {
        let n = job.host_block.min(total - pos);
        let end = (pos + n) as u64;
        while let Some(edit) = edits.next_if(|e| e.frame < end) {
            let frame = edit.frame;
            if let Err(e) = handle.schedule(frame, edit.edit) {
                panic!("render_offline_scripted: edit at frame {frame} rejected: {e}");
            }
        }
        while let Some(note) = notes.get(next_note).filter(|note| note.frame < end) {
            handle
                .send_note(*note)
                .expect("render_offline_scripted: note queue full");
            next_note += 1;
        }
        for (ch, buf) in block_in.iter_mut().enumerate() {
            let src = job.inputs.get(ch).map_or(&[][..], Vec::as_slice);
            for (k, dst) in buf[..n].iter_mut().enumerate() {
                *dst = src.get(pos + k).copied().unwrap_or(0.0);
            }
        }
        let ins: Vec<&[f32]> = block_in.iter().map(|b| &b[..n]).collect();
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(|o| &mut o[pos..pos + n]).collect();
        guard(&mut || engine.render(&ins, &mut outs));
        handle.collect_garbage();
        pos += n;
    }
    for channel in &mut out {
        channel.drain(..latency);
    }
    out
}

/// Bounces a song: queues `song` with [`EngineHandle::submit_song`] and
/// `Play`, then renders exactly like [`render_offline_scripted`] (same `job`, `edits`, `guard`
/// and panics). The caller sets the engine up first (plan or devices, and any transport
/// command such as `SetLoop`/`Metronome`/`Locate` through `handle.transport`), so the song
/// plays from the transport position at timeline frame 0. `job.frames` is typically the song
/// length plus a release tail.
///
/// # Panics
/// Like [`render_offline_scripted`], or if the song or `Play` cannot be queued.
pub fn render_offline_song(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: crate::SongSnapshot,
    edits: Vec<TimedEdit>,
    guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    handle
        .submit_song(song)
        .expect("render_offline_song: the control queue has room for the song");
    handle
        .transport(crate::TransportCommand::Play)
        .expect("render_offline_song: the control queue has room for Play");
    render_offline_scripted(engine, handle, job, edits, guard)
}

/// Like [`render_offline`], with host inputs, and with every `Engine::render` call wrapped by
/// `guard` (tests pass `|f| assert_no_alloc(f)` to prove HI-1 for the whole offline run).
///
/// # Panics
/// If `job.host_block` is 0.
pub fn render_offline_with(
    plan: RenderPlan,
    cfg: EngineConfig,
    job: &OfflineJob<'_>,
    mut guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    assert!(
        job.host_block > 0,
        "render_offline: host_block must be at least 1"
    );
    let mut notes = job.notes.to_vec();
    notes.sort_by_key(|n| n.frame);
    // Queues sized so that all notes and params always fit.
    let caps = Capacities {
        params: PARAM_QUEUE_CAPACITY.max(job.params.len() + 1),
        notes: NOTE_QUEUE_CAPACITY.max(notes.len() + 1),
        arena: DEFAULT_ARENA_CAPACITY.max(plan.nodes.len()),
    };
    let (mut engine, mut handle) = new_engine_with(cfg, caps);
    handle
        .submit_plan(plan)
        .expect("a fresh plan queue has room");
    for &(device, param, plain) in job.params {
        handle
            .set_param(device, param, plain)
            .expect("param queue sized for every param");
    }
    let latency = engine.latency_frames() as usize;
    let total = job.frames + latency;
    let mut out = vec![vec![0.0f32; total]; usize::from(cfg.output_channels)];
    let mut block_in = vec![vec![0.0f32; job.host_block]; usize::from(cfg.input_channels)];
    let mut next_note = 0;
    let mut pos = 0;
    while pos < total {
        let n = job.host_block.min(total - pos);
        let end = (pos + n) as u64;
        while let Some(note) = notes.get(next_note).filter(|note| note.frame < end) {
            handle
                .send_note(*note)
                .expect("note queue sized for every note");
            next_note += 1;
        }
        for (ch, buf) in block_in.iter_mut().enumerate() {
            let src = job.inputs.get(ch).map_or(&[][..], Vec::as_slice);
            for (k, dst) in buf[..n].iter_mut().enumerate() {
                *dst = src.get(pos + k).copied().unwrap_or(0.0);
            }
        }
        let ins: Vec<&[f32]> = block_in.iter().map(|b| &b[..n]).collect();
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(|o| &mut o[pos..pos + n]).collect();
        guard(&mut || engine.render(&ins, &mut outs));
        pos += n;
    }
    handle.collect_garbage();
    for channel in &mut out {
        channel.drain(..latency);
    }
    out
}
