//! Non-RT offline rendering around [`Engine`](crate::Engine) (used by the CLI and the tests).

use crate::engine::{
    Capacities, DEFAULT_ARENA_CAPACITY, NOTE_QUEUE_CAPACITY, PARAM_QUEUE_CAPACITY, ScheduledNote,
    new_engine_with,
};
use crate::engine::{Engine, EngineError, EngineHandle, TimedEdit};
use crate::graph::{DeviceInstanceId, EngineConfig};
use crate::plan::RenderPlan;
use atomik_core::ParamId;

/// Why an offline render stopped without producing audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OfflineError {
    /// A scripted hot edit was rejected by the engine.
    #[error("the edit at frame {frame} was rejected: {source}")]
    Edit {
        /// Timeline frame of the edit.
        frame: u64,
        /// The engine's reason.
        source: EngineError,
    },
    /// A queue to the engine had no room for something the render had to hand over.
    #[error("the engine's {queue} queue is full")]
    QueueFull {
        /// Which queue (`"parameter"`, `"note"`, `"plan"`, `"song"` or `"transport"`).
        queue: &'static str,
    },
    /// A device panicked while rendering. The engine contained the panic and silenced the
    /// device, but audio rendered from then on is incomplete, so no audio is returned.
    #[error("device {} panicked while rendering", .0.0)]
    DeviceFaulted(DeviceInstanceId),
}

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
/// If `host_block` is 0, or if the render fails (see [`try_render_offline`], which returns
/// the error instead; use it outside tests).
pub fn render_offline(
    plan: RenderPlan,
    cfg: EngineConfig,
    frames: usize,
    host_block: usize,
    notes: &[ScheduledNote],
    params: &[(DeviceInstanceId, ParamId, f32)],
) -> Vec<Vec<f32>> {
    try_render_offline(plan, cfg, frames, host_block, notes, params).unwrap_or_else(fail)
}

/// [`render_offline`] that returns an [`OfflineError`] instead of panicking.
///
/// # Panics
/// If `host_block` is 0.
pub fn try_render_offline(
    plan: RenderPlan,
    cfg: EngineConfig,
    frames: usize,
    host_block: usize,
    notes: &[ScheduledNote],
    params: &[(DeviceInstanceId, ParamId, f32)],
) -> Result<Vec<Vec<f32>>, OfflineError> {
    let job = OfflineJob {
        frames,
        host_block,
        notes,
        params,
        inputs: &[],
    };
    try_render_offline_with(plan, cfg, &job, |render| render())
}

fn fail<T>(error: OfflineError) -> T {
    panic!("offline render failed: {error}")
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
/// If `job.host_block` is 0, if the engine has already rendered, or if the render fails (see
/// [`try_render_offline_scripted`], which returns the error instead).
pub fn render_offline_scripted(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    edits: Vec<TimedEdit>,
    guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    try_render_offline_scripted(engine, handle, job, edits, guard).unwrap_or_else(fail)
}

/// [`render_offline_scripted`] that returns an [`OfflineError`] instead of panicking: a
/// rejected edit, a full queue, or a device that panicked while rendering (the render stops at
/// the end of the host block in which it was seen; no audio is returned).
///
/// # Panics
/// If `job.host_block` is 0 or if the engine has already rendered.
pub fn try_render_offline_scripted(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    mut edits: Vec<TimedEdit>,
    mut guard: impl FnMut(&mut dyn FnMut()),
) -> Result<Vec<Vec<f32>>, OfflineError> {
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
            .map_err(|_| OfflineError::QueueFull { queue: "parameter" })?;
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
            handle
                .schedule(frame, edit.edit)
                .map_err(|source| OfflineError::Edit { frame, source })?;
        }
        while let Some(note) = notes.get(next_note).filter(|note| note.frame < end) {
            handle
                .send_note(*note)
                .map_err(|_| OfflineError::QueueFull { queue: "note" })?;
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
        if let Some(device) = engine.fault() {
            return Err(OfflineError::DeviceFaulted(device));
        }
        pos += n;
    }
    for channel in &mut out {
        channel.drain(..latency);
    }
    Ok(out)
}

/// Bounces a song: queues `song` with [`EngineHandle::submit_song`] and
/// `Play`, then renders exactly like [`render_offline_scripted`] (same `job`, `edits`, `guard`
/// and panics). The caller sets the engine up first (plan or devices, and any transport
/// command such as `SetLoop`/`Metronome`/`Locate` through `handle.transport`), so the song
/// plays from the transport position at timeline frame 0. `job.frames` is typically the song
/// length plus a release tail.
///
/// # Panics
/// Like [`render_offline_scripted`], or if the song or `Play` cannot be queued (see
/// [`try_render_offline_song`], which returns the error instead).
pub fn render_offline_song(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: crate::SongSnapshot,
    edits: Vec<TimedEdit>,
    guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    try_render_offline_song(engine, handle, job, song, edits, guard).unwrap_or_else(fail)
}

/// [`render_offline_song`] that returns an [`OfflineError`] instead of panicking.
///
/// # Panics
/// If `job.host_block` is 0 or if the engine has already rendered.
pub fn try_render_offline_song(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: crate::SongSnapshot,
    edits: Vec<TimedEdit>,
    guard: impl FnMut(&mut dyn FnMut()),
) -> Result<Vec<Vec<f32>>, OfflineError> {
    handle
        .submit_song(song)
        .map_err(|_| OfflineError::QueueFull { queue: "song" })?;
    handle
        .transport(crate::TransportCommand::Play)
        .map_err(|_| OfflineError::QueueFull { queue: "transport" })?;
    try_render_offline_scripted(engine, handle, job, edits, guard)
}

/// Like [`render_offline`], with host inputs, and with every `Engine::render` call wrapped by
/// `guard` (tests pass `|f| assert_no_alloc(f)` to prove HI-1 for the whole offline run).
///
/// # Panics
/// If `job.host_block` is 0, or if the render fails (see [`try_render_offline_with`]).
pub fn render_offline_with(
    plan: RenderPlan,
    cfg: EngineConfig,
    job: &OfflineJob<'_>,
    guard: impl FnMut(&mut dyn FnMut()),
) -> Vec<Vec<f32>> {
    try_render_offline_with(plan, cfg, job, guard).unwrap_or_else(fail)
}

/// [`render_offline_with`] that returns an [`OfflineError`] instead of panicking.
///
/// # Panics
/// If `job.host_block` is 0.
pub fn try_render_offline_with(
    plan: RenderPlan,
    cfg: EngineConfig,
    job: &OfflineJob<'_>,
    mut guard: impl FnMut(&mut dyn FnMut()),
) -> Result<Vec<Vec<f32>>, OfflineError> {
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
        .map_err(|_| OfflineError::QueueFull { queue: "plan" })?;
    for &(device, param, plain) in job.params {
        handle
            .set_param(device, param, plain)
            .map_err(|_| OfflineError::QueueFull { queue: "parameter" })?;
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
                .map_err(|_| OfflineError::QueueFull { queue: "note" })?;
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
        if let Some(device) = engine.fault() {
            return Err(OfflineError::DeviceFaulted(device));
        }
        pos += n;
    }
    handle.collect_garbage();
    for channel in &mut out {
        channel.drain(..latency);
    }
    Ok(out)
}
