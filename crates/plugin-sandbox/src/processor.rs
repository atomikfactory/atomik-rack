//! The engine-side half of a remote instance: [`RemoteProcessor`] exchanges 64-frame blocks
//! with the child through the shared block.
//!
//! Hard invariant HI-1: nothing here allocates, takes a lock or makes a blocking call. The
//! port's buffers are the mapping; the one wait is the port's bounded spin on its own child
//! (one batch period live, the watchdog offline). A child that dies is seen through the
//! block's death flag, so a dead child answers [`ProcessStatus::Lost`] within one call. When
//! the offline watchdog runs out the processor only marks the block dead: the main thread
//! (the instance's `poll`) kills the process, because killing needs a lock.
//!
//! Starting, stopping and resetting are not requests the audio thread waits for. A start (and a
//! reset) is owed to the child and rides on the next batch, which the child serves in the order
//! stop, start, reset, process. The audio thread then waits for it exactly as for any batch: a
//! plugin that is slow to start only makes that batch late (and the ones behind it, which are
//! polled while the child is busy) and is never failed for it. A start the plugin refuses comes
//! back as [`ProcessStatus::StartFailed`] with the answer of the batch that carried it, or with
//! the first batch after it when that one was late. A stop is sent at once without waiting; when
//! a request is still in flight it is owed and rides on the next batch, ahead of its start.
//!
//! Live, a batch is waited for one period, or until the moment the running audio callback
//! published as the end of its plugin budget when that is later (one atomic load): a plugin
//! with bursty work uses the callback's slack instead of dropping the batch. A plugin that
//! misses [`COOL_AFTER`] full waits in a row stops being spun for during [`COOL_BATCHES`]
//! batches: each is polled once and counted late at once, then a full wait is tried again.

use atomik_core::CallbackDeadline;
use atomik_plugin_host::{
    PluginEvent, PluginIo, PluginProcessor, ProcessStatus, SleepRule, TailLength,
};
use atomik_plugin_ipc::{AudioOp, Before, Deadline, MAX_EVENTS_IN, ParentPort, Poll, Ticket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The longest a live batch is waited for, whatever deadline the callback published: the
/// callback's slack is a buffer (a few ten milliseconds at most), never more.
pub(crate) const MAX_LIVE_WAIT: Duration = Duration::from_millis(100);

/// How many full waits in a row may run out before the processor stops spinning for the plugin.
pub(crate) const COOL_AFTER: u32 = 4;

/// How many batches are polled once instead of waited for after [`COOL_AFTER`] misses. A
/// 480-frame callback holds seven or eight batches: this is about three callbacks.
pub(crate) const COOL_BATCHES: u32 = 24;

/// How many callbacks in a row may use up the whole published budget (a wait ran out at the
/// published moment) before the processor goes back to one batch period per wait.
pub(crate) const EXHAUSTED_AFTER: u32 = 3;

/// Callbacks the first fall-back lasts (about a second of 480-frame callbacks); each fall-back
/// that follows renewed exhaustion lasts twice as long, up to [`FALLBACK_MAX_CALLBACKS`].
pub(crate) const FALLBACK_CALLBACKS: u32 = 100;

/// The longest fall-back, in callbacks (about five seconds).
pub(crate) const FALLBACK_MAX_CALLBACKS: u32 = 500;

/// What a live batch did to get its answer (kept in tests, to see what the processor asked).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitKind {
    /// A full wait up to this deadline.
    Full(Deadline),
    /// One poll, no waiting (cool-down).
    Poll,
    /// The budget was used up too often: waits are one period for this many callbacks.
    Fallback(u32),
}

/// Where a processor puts its port back when it is dropped, so that the instance can activate
/// again.
pub(crate) type PortCell = Arc<Mutex<Option<ParentPort>>>;

/// Whether the published budget is worth using for one processor: plain integers, audio
/// thread only. A callback is exhausted when a wait ran out at the published moment (the plugin
/// used the whole budget and still was not done). A new callback is seen as the published value
/// changing.
#[derive(Debug, Default)]
struct BudgetState {
    seen: Option<Instant>,
    /// Callbacks left in the fall-back (0: the budget is used).
    fallback: u32,
    /// How long the next fall-back lasts (0: the first length).
    span: u32,
    /// Exhausted callbacks in a row.
    run: u32,
    /// The current callback had a wait up to the published moment.
    waited: bool,
    /// The current callback ran out of budget.
    exhausted: bool,
}

impl BudgetState {
    /// Called for every live batch with the published value; returns the value the wait may
    /// use (`None` in a fall-back) and the length of a fall-back that begins now.
    fn begin(&mut self, published: Option<Instant>) -> (Option<Instant>, Option<u32>) {
        let mut entered = None;
        if published.is_some() && published != self.seen {
            if self.fallback > 0 {
                self.fallback -= 1;
            } else if self.waited {
                if self.exhausted {
                    self.run += 1;
                    if self.run >= EXHAUSTED_AFTER {
                        let span = self.span.clamp(FALLBACK_CALLBACKS, FALLBACK_MAX_CALLBACKS);
                        self.fallback = span;
                        self.span = span.saturating_mul(2).min(FALLBACK_MAX_CALLBACKS);
                        self.run = 0;
                        entered = Some(span);
                    }
                } else {
                    self.run = 0;
                    self.span = 0;
                }
            }
            self.waited = false;
            self.exhausted = false;
        }
        self.seen = published;
        let usable = if self.fallback > 0 { None } else { published };
        (usable, entered)
    }
}

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
    /// Late batches in a row during which the child completed nothing.
    late_streak: u32,
    /// The child's completed-request counter as of the previous batch.
    last_done: u64,
    /// How many make the child hung (0: never; offline batches are not counted).
    hang_after: u32,
    /// Set when the child is declared hung, for the main thread to give the reason.
    hung: Option<Arc<AtomicBool>>,
    /// When the running callback must be done (live only; `None`: a batch gets one period).
    callback: Option<Arc<CallbackDeadline>>,
    /// Full waits that ran out in a row (reset by an answer).
    misses: u32,
    /// Batches left in the cool-down (0: not cooling).
    cooling: u32,
    /// Whether the published budget is in use.
    budget: BudgetState,
    /// Work owed to the child, which rides on the next batch (see the module documentation).
    owed: Before,
    /// The batch that carried a start, until its answer has been looked at: the refusal must not
    /// be lost when that batch was late.
    start_ticket: Option<Ticket>,
    /// A refusal found just before its answer would be overwritten: the next batch reports it.
    start_refused: bool,
    #[cfg(test)]
    trace: Option<Arc<Mutex<Vec<WaitKind>>>>,
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
            last_done: 0,
            hang_after: 0,
            hung: None,
            callback: None,
            misses: 0,
            cooling: 0,
            budget: BudgetState::default(),
            owed: Before::NONE,
            start_ticket: None,
            start_refused: false,
            #[cfg(test)]
            trace: None,
        }
    }

    /// Lets a live batch use what is left of the audio callback's budget: `callback` holds the
    /// moment the running callback published as the end of its plugin budget. Read with one
    /// atomic load per batch; offline processors never read it.
    pub fn with_callback_deadline(mut self, callback: Arc<CallbackDeadline>) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Records every live wait of this processor in `trace`.
    #[cfg(test)]
    pub fn with_trace(mut self, trace: Arc<Mutex<Vec<WaitKind>>>) -> Self {
        self.trace = Some(trace);
        self
    }

    /// Declares the child hung after `batches` late batches in a row in which it completed no
    /// request at all: the block is marked dead (so the engine sees `Lost` from then on) and
    /// `flag` is set; the main thread ends the process. A child that keeps completing
    /// requests, however late, is slow rather than hung and is never declared so. Only atomic
    /// loads and stores happen on the audio thread.
    pub fn with_hang_guard(mut self, batches: u32, flag: Arc<AtomicBool>) -> Self {
        self.hang_after = batches;
        self.hung = Some(flag);
        self
    }

    /// Counts the batch's result for the hang guard.
    fn note(&mut self, status: ProcessStatus) -> ProcessStatus {
        let done = self
            .port
            .as_ref()
            .map_or(self.last_done, ParentPort::done_seq);
        let progressed = done != self.last_done;
        self.last_done = done;
        if status != ProcessStatus::Late || progressed {
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

    /// Submits the batch (with the work owed before it) and the carried events in front (all at
    /// offset 0), and clears the carry.
    fn submit_with_carry(
        carry: &mut Vec<PluginEvent>,
        merged: &mut Vec<PluginEvent>,
        port: &mut ParentPort,
        before: Before,
        io: &mut PluginIo<'_>,
    ) -> Ticket {
        if carry.is_empty() {
            return port.submit_before(AudioOp::Process, before, io);
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
                aux_out: &mut *io.aux_out,
                events_in: merged,
                events_out: &mut *io.events_out,
                transport: io.transport,
                dropped: 0,
            };
            port.submit_before(AudioOp::Process, before, &view)
        };
        if !t.is_none() {
            carry.clear();
        }
        t
    }

    /// Handles a request left in flight by an earlier call: live it is polled, never waited for
    /// (a batch behind it is late); offline it is waited for up to the watchdog, since an
    /// offline batch is never late.
    fn drain(port: &ParentPort, offline: bool, watchdog: Duration) -> Drain {
        let Some(t) = port.outstanding() else {
            return Drain::Clear;
        };
        let polled = if offline {
            port.wait_done(t, Deadline::after(watchdog))
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

    /// The status of an answered request whose audio nobody wants (the answer of a batch that
    /// was late, or of a stop). Nothing is copied: the output side of the view is empty.
    fn take_status(port: &mut ParentPort, t: Ticket) -> ProcessStatus {
        let mut events_out = atomik_plugin_host::EventList::with_capacity(0);
        let mut main_out: [atomik_core::Batch; 0] = [];
        let mut io = PluginIo {
            frames: atomik_core::BATCH,
            main_in: &[],
            main_in_silent: true,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            aux_out: &mut [],
            events_in: &[],
            events_out: &mut events_out,
            transport: atomik_plugin_host::PluginTransport::default(),
            dropped: 0,
        };
        port.take_response(t, &mut io).status
    }

    /// Whether the answer of the batch that carried a start says the plugin refused it. Must be
    /// asked before another request is submitted (it would overwrite the answer).
    fn start_was_refused(port: &mut ParentPort, t: Ticket) -> bool {
        Self::take_status(port, t) == ProcessStatus::StartFailed
    }

    /// The work to ride on the next batch: a stop owed with nothing to start afterwards is
    /// dropped (a batch only follows a start), the rest goes as it is.
    fn riding(owed: Before) -> Before {
        if owed.has(Before::START) {
            owed
        } else {
            owed.without(Before::STOP)
        }
    }
}

impl RemoteProcessor {
    /// Runs one batch, without the hang guard.
    fn process_batch(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let (offline, watchdog, period) = (self.offline, self.watchdog, self.period);
        // A batch of the cool-down is used up whatever becomes of it.
        let cool = !offline && self.cooling > 0;
        if cool {
            self.cooling -= 1;
        }
        let published = if offline {
            None
        } else {
            let seen = self.callback.as_ref().and_then(|c| c.read());
            let (usable, _entered) = self.budget.begin(seen);
            #[cfg(test)]
            if let Some(span) = _entered {
                Self::record(&self.trace, WaitKind::Fallback(span));
            }
            usable
        };
        let Self {
            port,
            carry,
            merged,
            misses,
            cooling,
            budget,
            owed,
            start_ticket,
            start_refused,
            ..
        } = self;
        let Some(port) = port.as_mut() else {
            return ProcessStatus::Lost;
        };
        if port.is_dead() {
            return ProcessStatus::Lost;
        }
        match Self::drain(port, offline, watchdog) {
            Drain::Clear => {}
            Drain::Busy => {
                Self::carry_over(carry, io);
                return ProcessStatus::Late;
            }
            Drain::Lost => return ProcessStatus::Lost,
        }
        // A start that rode on an earlier batch (one that was late) has been answered: say so if
        // the plugin refused it, before the next request overwrites the answer.
        let refused = match start_ticket.take() {
            Some(t) => Self::start_was_refused(port, t),
            None => false,
        };
        if refused || std::mem::take(start_refused) {
            return ProcessStatus::StartFailed;
        }
        let before = Self::riding(*owed);
        let t = Self::submit_with_carry(carry, merged, port, before, io);
        if t.is_none() {
            Self::carry_over(carry, io);
            return ProcessStatus::Late;
        }
        *owed = Before::NONE;
        if before.has(Before::START) {
            *start_ticket = Some(t);
        }
        let polled = if offline {
            port.wait_done(t, Deadline::after(watchdog))
        } else if cool {
            #[cfg(test)]
            Self::record(&self.trace, WaitKind::Poll);
            port.poll(t)
        } else {
            let deadline = Deadline::after_or_until(period, published, MAX_LIVE_WAIT);
            #[cfg(test)]
            Self::record(&self.trace, WaitKind::Full(deadline));
            let used_budget = published.is_some();
            let polled = port.wait_done(t, deadline);
            if used_budget {
                budget.waited = true;
                if let (Poll::TimedOut, Deadline::At(at), Some(until)) =
                    (polled, deadline, published)
                {
                    // The wait ran to the published moment: the callback's budget is gone.
                    budget.exhausted |= at >= until;
                }
            }
            polled
        };
        match polled {
            Poll::Done => {
                *misses = 0;
                *cooling = 0;
                *start_ticket = None;
                port.take_response(t, io).status
            }
            Poll::Dead => ProcessStatus::Lost,
            Poll::TimedOut if offline => {
                port.mark_dead();
                ProcessStatus::Lost
            }
            Poll::TimedOut => {
                // A full wait ran out: enough of them in a row and the plugin is not spun for.
                *misses += 1;
                if *misses >= COOL_AFTER {
                    *cooling = COOL_BATCHES;
                    // After the cool-down one more miss starts the next one.
                    *misses = COOL_AFTER - 1;
                }
                ProcessStatus::Late
            }
            Poll::Pending => ProcessStatus::Late,
        }
    }

    #[cfg(test)]
    fn record(trace: &Option<Arc<Mutex<Vec<WaitKind>>>>, kind: WaitKind) {
        if let Some(trace) = trace {
            trace
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(kind);
        }
    }
}

impl PluginProcessor for RemoteProcessor {
    /// Owes the start to the child: it rides on the next batch, so nothing waits for it here. A
    /// plugin whose process is gone cannot start (the batch would only say it is lost).
    fn start_processing(&mut self) -> bool {
        if self.port.as_ref().is_none_or(ParentPort::is_dead) {
            return false;
        }
        self.owed = self.owed.with(Before::START);
        true
    }

    /// Sends the stop at once and does not wait for it: the next request finds it in flight and
    /// is handled like it handles any earlier one. A start that was owed and not yet sent is
    /// cancelled (the child never heard of it); a stop that cannot be sent now, because a
    /// request is in flight, is owed and rides on the next batch, ahead of the start that
    /// batch will carry.
    fn stop_processing(&mut self) {
        let Self {
            port,
            owed,
            start_ticket,
            start_refused,
            ..
        } = self;
        let Some(port) = port.as_mut() else {
            return;
        };
        if port.is_dead() {
            return;
        }
        let (owed_start, owed_stop) = (owed.has(Before::START), owed.has(Before::STOP));
        *owed = owed.without(Before::START);
        if owed_start && !owed_stop {
            // The child never heard of that start: there is nothing to stop.
            *owed = Before::NONE;
            return;
        }
        if port.outstanding().is_some() {
            *owed = owed.with(Before::STOP);
            return;
        }
        // Nothing is in flight: the answer of a start that rode on a late batch is read before
        // the stop overwrites it.
        if let Some(t) = start_ticket.take()
            && Self::start_was_refused(port, t)
        {
            *start_refused = true;
        }
        let first = owed.without(Before::STOP);
        if !port.submit_op_before(AudioOp::Stop, first).is_none() {
            *owed = Before::NONE;
        } else {
            *owed = owed.with(Before::STOP);
        }
    }

    /// Owes the reset to the child: it rides on the next batch, ahead of that batch's events.
    fn reset(&mut self) {
        if self.port.as_ref().is_some_and(|p| !p.is_dead()) {
            self.owed = self.owed.with(Before::RESET);
        }
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
