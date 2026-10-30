//! The engine-side half of a remote instance: [`RemoteProcessor`] exchanges 64-frame blocks
//! with the child through the shared block.
//!
//! Hard invariant HI-1: nothing here allocates, takes a lock or makes a blocking call. The
//! port's buffers are the mapping; the one wait is the port's bounded spin on its own child
//! (one batch period live, the watchdog offline). A child that dies is seen through the
//! block's death flag, so a dead child answers [`ProcessStatus::Lost`] within one call. When
//! the offline watchdog runs out the processor only marks the block dead: the main thread
//! (the instance's `poll`) kills the process, because killing needs a lock.

use atomik_plugin_host::{
    PluginEvent, PluginIo, PluginProcessor, ProcessStatus, SleepRule, TailLength,
};
use atomik_plugin_ipc::{AudioOp, Deadline, MAX_EVENTS_IN, ParentPort, Poll, Ticket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long a live `start_processing`, `stop_processing` or `reset` may take. These are not
/// audio: a plugin may need a few milliseconds to start, which is not a failure.
pub(crate) const LIVE_OP_DEADLINE: Duration = Duration::from_millis(50);

/// Where a processor puts its port back when it is dropped, so that the instance can activate
/// again.
pub(crate) type PortCell = Arc<Mutex<Option<ParentPort>>>;

/// The audio half of a sandboxed plugin.
pub(crate) struct RemoteProcessor {
    port: Option<ParentPort>,
    cell: PortCell,
    offline: bool,
    period: Duration,
    watchdog: Duration,
    /// Events of batches that were late without being submitted, waiting for the next submit.
    carry: Vec<PluginEvent>,
    /// The carry followed by the new batch's events, as submitted.
    merged: Vec<PluginEvent>,
    /// Late batches in a row.
    late_streak: u32,
    /// How many make the child hung (0: never; offline batches are not counted).
    hang_after: u32,
    /// Set when the child is declared hung, for the main thread to give the reason.
    hung: Option<Arc<AtomicBool>>,
}

/// What became of the request that was in flight before a new exchange.
enum Drain {
    /// Nothing is in flight now.
    Clear,
    /// It has not completed (live only).
    Busy,
    /// The child is gone.
    Lost,
}

impl RemoteProcessor {
    /// A processor over `port`. `period` is the live deadline of a batch, `watchdog` the
    /// offline one; `cell` takes the port back on drop.
    pub fn new(
        port: ParentPort,
        cell: PortCell,
        offline: bool,
        period: Duration,
        watchdog: Duration,
    ) -> Self {
        Self {
            port: Some(port),
            cell,
            offline,
            period,
            watchdog,
            carry: Vec::with_capacity(MAX_EVENTS_IN),
            merged: Vec::with_capacity(MAX_EVENTS_IN),
            late_streak: 0,
            hang_after: 0,
            hung: None,
        }
    }

    /// Declares the child hung after `batches` late batches in a row: the block is marked dead
    /// (so the engine sees `Lost` from then on) and `flag` is set; the main thread ends the
    /// process. Only atomic stores happen on the audio thread.
    pub fn with_hang_guard(mut self, batches: u32, flag: Arc<AtomicBool>) -> Self {
        self.hang_after = batches;
        self.hung = Some(flag);
        self
    }

    /// Counts the batch's result for the hang guard.
    fn note(&mut self, status: ProcessStatus) -> ProcessStatus {
        if status != ProcessStatus::Late {
            self.late_streak = 0;
        } else if self.hang_after > 0 && !self.offline {
            self.late_streak = self.late_streak.saturating_add(1);
            if self.late_streak >= self.hang_after {
                if let Some(flag) = &self.hung {
                    flag.store(true, Ordering::Release);
                }
                if let Some(port) = &self.port {
                    port.mark_dead();
                }
                return ProcessStatus::Lost;
            }
        }
        status
    }

    /// Keeps the events of a batch that was not submitted. When the carry is full the newest
    /// events are dropped and counted in `io.dropped` (no allocation either way).
    fn carry_over(carry: &mut Vec<PluginEvent>, io: &mut PluginIo<'_>) {
        for e in io.events_in {
            if carry.len() < MAX_EVENTS_IN {
                carry.push(*e);
            } else {
                io.dropped = io.dropped.saturating_add(1);
            }
        }
    }

    /// Submits the batch with the carried events in front (all at offset 0), and clears the
    /// carry.
    fn submit_with_carry(
        carry: &mut Vec<PluginEvent>,
        merged: &mut Vec<PluginEvent>,
        port: &mut ParentPort,
        io: &mut PluginIo<'_>,
    ) -> Ticket {
        if carry.is_empty() {
            return port.submit(AudioOp::Process, io);
        }
        merged.clear();
        merged.extend(carry.iter().map(|e| PluginEvent {
            offset: 0,
            kind: e.kind,
        }));
        let room = MAX_EVENTS_IN - merged.len();
        merged.extend(io.events_in.iter().take(room));
        let dropped_here = io.events_in.len() - room.min(io.events_in.len());
        io.dropped = io.dropped.saturating_add(dropped_here as u32);
        let t = {
            let view = PluginIo {
                frames: io.frames,
                main_in: io.main_in,
                main_in_silent: io.main_in_silent,
                sidechain: io.sidechain,
                sidechain_silent: io.sidechain_silent,
                main_out: &mut *io.main_out,
                events_in: merged,
                events_out: &mut *io.events_out,
                transport: io.transport,
                dropped: 0,
            };
            port.submit(AudioOp::Process, &view)
        };
        if !t.is_none() {
            carry.clear();
        }
        t
    }

    /// Handles a request left in flight by an earlier call. Live, a request that is still
    /// running is waited for no longer than `wait` (`None`: not at all); offline it is waited
    /// for up to the watchdog, since an offline batch is never late.
    fn drain(
        port: &ParentPort,
        offline: bool,
        watchdog: Duration,
        wait: Option<Duration>,
    ) -> Drain {
        let Some(t) = port.outstanding() else {
            return Drain::Clear;
        };
        let polled = if offline {
            port.wait_done(t, Deadline::after(watchdog))
        } else if let Some(wait) = wait {
            port.wait_done(t, Deadline::after(wait))
        } else {
            port.poll(t)
        };
        match polled {
            Poll::Done => Drain::Clear,
            Poll::Dead => Drain::Lost,
            Poll::Pending => Drain::Busy,
            Poll::TimedOut => {
                if offline {
                    // The offline watchdog ran out: the child counts as hung.
                    port.mark_dead();
                    Drain::Lost
                } else {
                    Drain::Busy
                }
            }
        }
    }

    /// The deadline of a block-less op.
    fn op_wait(&self) -> Duration {
        self.period.max(LIVE_OP_DEADLINE)
    }

    /// Runs one op without audio. `None`: not done (late, or the child is gone).
    fn run_op(&mut self, op: AudioOp) -> Option<bool> {
        let (offline, watchdog, wait) = (self.offline, self.watchdog, self.op_wait());
        let port = self.port.as_mut()?;
        if port.is_dead() {
            return None;
        }
        match Self::drain(port, offline, watchdog, Some(wait)) {
            Drain::Clear => {}
            Drain::Busy | Drain::Lost => return None,
        }
        let t = port.submit_op(op);
        if t.is_none() {
            return None;
        }
        let deadline = Deadline::after(if offline { watchdog } else { wait });
        match port.wait_done(t, deadline) {
            Poll::Done => Some(Self::take_ok(port, t)),
            Poll::TimedOut if offline => {
                port.mark_dead();
                None
            }
            Poll::TimedOut | Poll::Pending | Poll::Dead => None,
        }
    }

    /// The boolean result of a finished op. The op carries no audio, so the output side of
    /// the view is empty.
    fn take_ok(port: &mut ParentPort, t: Ticket) -> bool {
        let mut events_out = atomik_plugin_host::EventList::with_capacity(0);
        let mut main_out: [atomik_core::Batch; 0] = [];
        let mut io = PluginIo {
            frames: atomik_core::BATCH,
            main_in: &[],
            main_in_silent: true,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            events_in: &[],
            events_out: &mut events_out,
            transport: atomik_plugin_host::PluginTransport::default(),
            dropped: 0,
        };
        let response = port.take_response(t, &mut io);
        response.ok && response.status != ProcessStatus::Error
    }
}

impl RemoteProcessor {
    /// Runs one batch, without the hang guard.
    fn process_batch(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let (offline, watchdog, period) = (self.offline, self.watchdog, self.period);
        let Self {
            port,
            carry,
            merged,
            ..
        } = self;
        let Some(port) = port.as_mut() else {
            return ProcessStatus::Lost;
        };
        if port.is_dead() {
            return ProcessStatus::Lost;
        }
        match Self::drain(port, offline, watchdog, None) {
            Drain::Clear => {}
            Drain::Busy => {
                Self::carry_over(carry, io);
                return ProcessStatus::Late;
            }
            Drain::Lost => return ProcessStatus::Lost,
        }
        let t = Self::submit_with_carry(carry, merged, port, io);
        if t.is_none() {
            Self::carry_over(carry, io);
            return ProcessStatus::Late;
        }
        let deadline = Deadline::after(if offline { watchdog } else { period });
        match port.wait_done(t, deadline) {
            Poll::Done => port.take_response(t, io).status,
            Poll::Dead => ProcessStatus::Lost,
            Poll::TimedOut if offline => {
                port.mark_dead();
                ProcessStatus::Lost
            }
            Poll::TimedOut | Poll::Pending => ProcessStatus::Late,
        }
    }
}

impl PluginProcessor for RemoteProcessor {
    fn start_processing(&mut self) -> bool {
        self.run_op(AudioOp::Start).unwrap_or(false)
    }

    fn stop_processing(&mut self) {
        let _ = self.run_op(AudioOp::Stop);
    }

    fn reset(&mut self) {
        let _ = self.run_op(AudioOp::Reset);
    }

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let status = self.process_batch(io);
        self.note(status)
    }

    fn sleep_rule(&self) -> SleepRule {
        self.port
            .as_ref()
            .map_or(SleepRule::PluginDecides, |p| p.info().sleep_rule)
    }

    fn tail_samples(&self) -> TailLength {
        self.port
            .as_ref()
            .map_or(TailLength::Finite(0), |p| p.info().tail)
    }

    fn wake_requested(&self) -> bool {
        self.port.as_ref().is_some_and(ParentPort::wake_requested)
    }
}

impl Drop for RemoteProcessor {
    /// Hands the port back to the instance (and so the child's `Deactivate` can run with
    /// nothing in flight on this side). Not an audio method: the manager drops processors on
    /// the main thread.
    fn drop(&mut self) {
        if let Some(port) = self.port.take() {
            *self.cell.lock().unwrap_or_else(PoisonError::into_inner) = Some(port);
        }
    }
}
