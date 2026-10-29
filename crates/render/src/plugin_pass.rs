//! The bounce loop for projects with plugins.
//!
//! A plugin is not a pure function of its inputs the way the built-in devices are: it may ask the
//! host to restart it (CLAP `request_restart`, VST3 `restartComponent`), and a restart is how a
//! changed **latency** reaches delay compensation. The live app handles both on its main thread
//! (`AppCore::frame`): the manager restarts the plugin, the engine publishes the new latency, the
//! app recompiles the routing. A bounce that only renders never did, so a plugin whose latency
//! comes from a param (a saved value, an automation lane) was bounced without the compensation the
//! live app applies. This module is the offline form of that loop:
//!
//! 1. **Pre-roll.** Before the song starts the engine renders silent 64-frame batches with the
//!    transport stopped, and after each batch the plugin manager is polled and the routing is
//!    recompiled when a latency changed. The pre-roll ends after [`QUIET_BATCHES`] batches in a
//!    row without a restart or a latency change (at most [`MAX_PREROLL_BATCHES`]). The plugins
//!    have then applied their saved params, restarted if they wanted to, and the routing carries
//!    their settled latencies, so the song is aligned from its first frame.
//! 2. **Song.** The engine renders in **64-frame batches** (one `Engine::render` per batch) and
//!    after every batch polls the manager and recompiles the routing if the engine published a
//!    new latency, exactly the order of `AppCore::frame` driven once per batch.
//!
//! Polling per batch, never by wall clock, is what keeps the offline output a function of the
//! project: the same restart takes the same batches for any worker count, and because the engine
//! is driven in batches the host block size cannot change when a restart lands, so the output
//! does not depend on `--block` (which only sizes the plugin-free path). A latency that changes
//! mid-song (an automated latency param) is followed the way live follows it, with the one-click
//! discontinuity of the new delay lines; a changed param table (`LayoutChanged`) is not
//! followed (the bounce keeps its devices), a failure is reported as a warning.

use crate::Warning;
use crate::plugins::PluginHost;
use atomik_core::BATCH;
use atomik_engine::{
    Engine, EngineHandle, OfflineError, OfflineJob, SongSnapshot, TimedEdit, Topology,
    TransportCommand,
};
use atomik_plugin_host::ManagerEvent;
use std::sync::atomic::{AtomicBool, Ordering};

/// Batches in a row without a restart or a latency change that end the pre-roll (a restart takes
/// about four batches from the request to the aligned routing).
pub const QUIET_BATCHES: usize = 8;

/// The most batches the pre-roll runs (a plugin that restarts forever must not hang the bounce).
pub const MAX_PREROLL_BATCHES: usize = 256;

/// How a caller watches and stops a long render: a flag the loop reads once per batch (the
/// render then ends early and returns `None`) and a callback that gets the fraction done every
/// so many batches. Neither changes a sample of the output.
#[derive(Clone, Copy, Default)]
pub struct RenderControl<'a> {
    /// Stops the render when it reads `true`.
    pub cancel: Option<&'a AtomicBool>,
    /// Told how far the render is, `0.0..=1.0`, about every [`PROGRESS_BATCHES`] batches.
    pub progress: Option<&'a (dyn Fn(f32) + Sync)>,
}

impl std::fmt::Debug for RenderControl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderControl")
            .field("cancel", &self.cancel.is_some())
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl RenderControl<'_> {
    /// True once the caller asked to stop.
    pub fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// Batches between two progress reports (about a quarter of a second of audio at 48 kHz).
pub const PROGRESS_BATCHES: usize = 192;

/// What the loop needs besides the engine: the plugin host to poll and the routing model to
/// recompile.
pub struct PluginPass<'a> {
    /// The host whose manager is polled after every batch.
    pub host: &'a mut PluginHost,
    /// The routing of the engine's plan (`Graph::topology`), recompiled with the published
    /// latencies when one changes.
    pub topology: Topology,
    /// Problems the plugins reported while the pass ran.
    pub warnings: Vec<Warning>,
    epoch: u64,
}

impl<'a> PluginPass<'a> {
    /// A pass over `topology` polling `host`.
    pub fn new(host: &'a mut PluginHost, topology: Topology) -> Self {
        Self {
            host,
            topology,
            warnings: Vec::new(),
            epoch: 0,
        }
    }

    /// One main-thread step after a batch: polls the manager, and when the engine published a
    /// new device latency recompiles the routing with it. Returns whether anything restarted or
    /// moved (the pre-roll is quiet only while this is false).
    fn after_batch(&mut self, handle: &mut EngineHandle) -> bool {
        let events = self.host.poll();
        let mut moved = digest(events, &mut self.warnings);
        if follow_latency(
            handle,
            &mut self.topology,
            &mut self.epoch,
            &mut self.warnings,
        ) {
            moved = true;
        }
        moved
    }
}

/// The routing half of one main-thread step after a batch (what `AppCore::frame` does when the
/// engine's latency epoch moved): when the engine published a new device latency since `epoch`,
/// recompiles `topology` with the published latencies and submits the routing, so delay
/// compensation follows the change from the next batch on. Returns whether the epoch moved. A
/// routing that could not be compiled or submitted is a warning, the bounce goes on.
fn follow_latency(
    handle: &mut EngineHandle,
    topology: &mut Topology,
    epoch: &mut u64,
    warnings: &mut Vec<Warning>,
) -> bool {
    let now = handle.latency_epoch();
    if now == *epoch {
        return false;
    }
    handle.fill_latencies(topology);
    match topology.compile() {
        Ok(routing) => {
            if let Err(e) = handle.submit_routing(routing) {
                warnings.push(warning(format!(
                    "the routing could not follow a latency change: {e}"
                )));
            }
        }
        Err(e) => warnings.push(warning(format!(
            "the routing could not follow a latency change: {e}"
        ))),
    }
    *epoch = now;
    true
}

fn warning(message: String) -> Warning {
    Warning {
        path: "plugins".to_string(),
        message,
    }
}

/// What a batch's manager events mean for the pass: `true` when a plugin restarted, changed its
/// latency or its param table (the pre-roll is not quiet yet); a failure or a restart that could
/// not finish adds a warning (the bounce goes on, the plugin's device outputs silence or dry).
fn digest(events: Vec<ManagerEvent>, warnings: &mut Vec<Warning>) -> bool {
    let mut moved = false;
    for event in events {
        match event {
            ManagerEvent::Restarted(_)
            | ManagerEvent::LatencyChanged { .. }
            | ManagerEvent::LayoutChanged { .. } => moved = true,
            ManagerEvent::RestartFailed { reason, .. } => {
                moved = true;
                warnings.push(warning(format!("a plugin could not restart: {reason}")));
            }
            ManagerEvent::Failed { reason, .. } => {
                warnings.push(warning(format!("a plugin failed: {reason}")));
            }
            _ => {}
        }
    }
    moved
}

/// Renders `job` as [`atomik_engine::render_offline_song`] does, with the plugin pre-roll and
/// the per-batch plugin step of the module docs. The transport commands that precede the song
/// (loop, metronome) are queued by the caller before this call.
///
/// `control` can end the render early (the result is then `Ok(None)`); the pre-roll is short and
/// is not interrupted. A rejected edit, a full queue or a device that panicked while rendering
/// is an [`OfflineError`]: no audio is returned for a render that lost a device.
///
/// # Panics
/// If the engine has already rendered.
pub fn render_song_with_plugins(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: SongSnapshot,
    edits: Vec<TimedEdit>,
    pass: &mut PluginPass<'_>,
    control: &RenderControl<'_>,
) -> Result<Option<Vec<Vec<f32>>>, OfflineError> {
    assert_eq!(
        engine.host_frames(),
        0,
        "render_song_with_plugins: the engine must be fresh"
    );
    let channels = engine.output_channels();
    let inputs = engine.input_channels();
    pass.epoch = handle.latency_epoch();

    // 1. Pre-roll: silence, transport stopped, until the plugins are quiet.
    let mut quiet = 0;
    for _ in 0..MAX_PREROLL_BATCHES {
        let ins = vec![vec![0.0f32; BATCH]; inputs];
        let ins: Vec<&[f32]> = ins.iter().map(Vec::as_slice).collect();
        let mut outs = vec![vec![0.0f32; BATCH]; channels];
        let mut outs: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
        engine.render(&ins, &mut outs);
        handle.collect_garbage();
        if let Some(device) = engine.fault() {
            return Err(OfflineError::DeviceFaulted(device));
        }
        if pass.after_batch(handle) {
            quiet = 0;
        } else {
            quiet += 1;
            if quiet >= QUIET_BATCHES {
                break;
            }
        }
    }

    // 2. The song, one batch at a time.
    song_in_batches(engine, handle, job, song, edits, control, |handle| {
        pass.after_batch(handle);
    })
}

/// Renders `job` like [`atomik_engine::render_offline_song`], for a project **without plugins**,
/// but in 64-frame batches with the latency step of [`render_song_with_plugins`] after every
/// batch: when a device publishes a new latency (a preset change of `atomik.character` across its
/// latency classes, an automated look-ahead) the routing is recompiled with it, exactly the order
/// of `AppCore::frame` driven once per batch. A bounce that did not follow the engine's latency
/// epoch kept the alignment it started with and no longer matched the live app after such a
/// change (found by the class-crossing test). The engine is driven in batches, so
/// the host block size cannot change when the recompile lands and the output stays a function of
/// the project; for a project whose latencies never change the samples are those of the plain
/// loop for any host block (the engine's host-block invariance).
///
/// `topology` is the routing model of the engine's plan (`Graph::topology`), whose epoch is read
/// before the first batch. Problems are returned as warnings.
///
/// `control` can end the render early (the result is then `Ok(None)`). Errors are those of
/// [`render_song_with_plugins`].
///
/// # Panics
/// If the engine has already rendered.
#[allow(
    clippy::too_many_arguments,
    reason = "the engine and its handle, the job, and what the loop follows and reports"
)]
pub fn render_song_following_latency(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: SongSnapshot,
    edits: Vec<TimedEdit>,
    mut topology: Topology,
    warnings: &mut Vec<Warning>,
    control: &RenderControl<'_>,
) -> Result<Option<Vec<Vec<f32>>>, OfflineError> {
    assert_eq!(
        engine.host_frames(),
        0,
        "render_song_following_latency: the engine must be fresh"
    );
    let mut epoch = handle.latency_epoch();
    song_in_batches(engine, handle, job, song, edits, control, |handle| {
        follow_latency(handle, &mut topology, &mut epoch, warnings);
    })
}

/// The song loop of the two bounces above: queues the song and `Play`, then renders in
/// [`BATCH`]-frame blocks and calls `after_batch` once after each batch (and its garbage
/// collection), before the next one. `Ok(None)` when `control` stopped it.
fn song_in_batches(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    job: &OfflineJob<'_>,
    song: SongSnapshot,
    mut edits: Vec<TimedEdit>,
    control: &RenderControl<'_>,
    mut after_batch: impl FnMut(&mut EngineHandle),
) -> Result<Option<Vec<Vec<f32>>>, OfflineError> {
    let channels = engine.output_channels();
    let inputs = engine.input_channels();
    handle
        .submit_song(song)
        .map_err(|_| OfflineError::QueueFull { queue: "song" })?;
    handle
        .transport(TransportCommand::Play)
        .map_err(|_| OfflineError::QueueFull { queue: "transport" })?;
    let mut notes = job.notes.to_vec();
    notes.sort_by_key(|n| n.frame);
    edits.sort_by_key(|e| e.frame);
    let mut edits = edits.into_iter().peekable();
    let latency = engine.latency_frames() as usize;
    let total = job.frames + latency;
    let mut out = vec![vec![0.0f32; total]; channels];
    let mut block_in = vec![vec![0.0f32; BATCH]; inputs];
    let mut next_note = 0;
    let mut pos = 0;
    let mut batches = 0usize;
    while pos < total {
        if control.cancelled() {
            return Ok(None);
        }
        if batches.is_multiple_of(PROGRESS_BATCHES)
            && let Some(progress) = control.progress
        {
            progress(pos as f32 / total as f32);
        }
        batches += 1;
        let n = BATCH.min(total - pos);
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
        engine.render(&ins, &mut outs);
        handle.collect_garbage();
        if let Some(device) = engine.fault() {
            return Err(OfflineError::DeviceFaulted(device));
        }
        after_batch(handle);
        pos += n;
    }
    if let Some(progress) = control.progress {
        progress(1.0);
    }
    for channel in &mut out {
        channel.drain(..latency);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_plugin_host::InstanceId;

    #[test]
    fn restarts_latency_and_layout_changes_keep_the_pre_roll_going_without_a_warning() {
        let id = InstanceId(1);
        for event in [
            ManagerEvent::Restarted(id),
            ManagerEvent::LatencyChanged {
                instance: id,
                frames: 64,
            },
            ManagerEvent::StateDirty(id),
        ] {
            let mut warnings = Vec::new();
            let moved = digest(vec![event.clone()], &mut warnings);
            assert_eq!(
                moved,
                !matches!(event, ManagerEvent::StateDirty(_)),
                "{event:?}"
            );
            assert!(warnings.is_empty(), "{event:?}");
        }
        assert!(!digest(Vec::new(), &mut Vec::new()), "a quiet batch");
    }

    #[test]
    fn a_failed_plugin_and_a_restart_that_could_not_finish_are_warnings_that_name_the_reason() {
        let id = InstanceId(2);
        let mut warnings = Vec::new();
        let moved = digest(
            vec![
                ManagerEvent::Failed {
                    instance: id,
                    reason: "returned NaN".to_string(),
                },
                ManagerEvent::RestartFailed {
                    instance: id,
                    reason: "could not activate".to_string(),
                },
            ],
            &mut warnings,
        );
        assert!(moved, "a failed restart is not quiet either");
        let text: Vec<String> = warnings.iter().map(ToString::to_string).collect();
        assert_eq!(
            text,
            [
                "plugins: a plugin failed: returned NaN",
                "plugins: a plugin could not restart: could not activate"
            ]
        );
    }
}
