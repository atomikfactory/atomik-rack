//! The child end of the audio exchange, used by the child's audio thread.
//!
//! The child may allocate when the port is made; after that the exchange does not: the decoded
//! input events and the output event list are preallocated at their fixed capacities.

use crate::block::SharedBlock;
use crate::codec::{AudioOp, sleep_rule_code, status_code, tail_code};
use crate::deadline::now;
use crate::doorbell::Doorbell;
use crate::layout::{MAX_AUX_CHANNELS, MAX_CHANNELS, MAX_EVENTS_IN, MAX_EVENTS_OUT, WireEvent};
use crate::parent::ProcessorInfo;
use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{EventList, PluginEvent, PluginIo, ProcessStatus};
use std::ptr::{addr_of, addr_of_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the child spins on the request counter before it parks on the doorbell. A parent
/// that rings once per 64-frame batch is back within this time, so the child never sleeps.
/// Only a call that follows an answered request spins: a wait that timed out means the parent
/// is not streaming, and the next call parks at once.
const SPIN: Duration = Duration::from_micros(50);

/// The longest the child parks on the doorbell without showing a sign of life. A call with a
/// longer timeout parks in slices of this length and bumps the pulse before each, so the pulse
/// moves at least this often whatever wait the caller asked for (the parent judges the child
/// alive from it over a window of half a second).
pub const MAX_PARK: Duration = Duration::from_millis(100);

/// The child end of one block.
#[derive(Debug)]
pub struct ChildPort {
    block: SharedBlock,
    doorbell: Arc<Doorbell>,
    /// Set by a [`ChildWaker`]: the next (or the running) wait returns `None` at once.
    kicked: Arc<AtomicBool>,
    /// The last call returned a request: the next one spins before it parks.
    spin_next: bool,
    /// The last ticket answered; the child is the only writer of `done_seq`.
    last_done: u64,
    /// Passes of the wait loop so far: the value of the pulse the parent reads.
    beats: u64,
    events_in: Vec<PluginEvent>,
    events_out: EventList,
}

impl ChildPort {
    /// A port over `block`, which `doorbell` was opened for. Allocates the event buffers.
    pub fn new(block: SharedBlock, doorbell: Doorbell) -> Self {
        let last_done = block.header().done_seq.load(Ordering::Acquire);
        Self {
            block,
            doorbell: Arc::new(doorbell),
            kicked: Arc::new(AtomicBool::new(false)),
            spin_next: false,
            last_done,
            beats: 0,
            events_in: Vec::with_capacity(MAX_EVENTS_IN),
            events_out: EventList::with_capacity(MAX_EVENTS_OUT),
        }
    }

    /// The ticket of a request the child has not answered, if one is there.
    fn pending(&self) -> Option<u64> {
        // Acquire: the request area, written before the parent's release store, is visible.
        let seq = self.block.header().req_seq.load(Ordering::Acquire);
        (seq > self.last_done).then_some(seq)
    }

    /// Answers a request this port cannot serve (an op nobody writes) with an error status.
    fn refuse(&mut self, ticket: u64) {
        let resp = self.block.response();
        // SAFETY: the request of `ticket` is in flight, so the parent does not read the
        // response area before the release store below.
        unsafe {
            addr_of_mut!((*resp).ticket).write(ticket);
            addr_of_mut!((*resp).status).write(status_code(ProcessStatus::Error));
            addr_of_mut!((*resp).ok).write(0);
            addr_of_mut!((*resp).dropped).write(0);
            addr_of_mut!((*resp).n_events).write(0);
        }
        self.last_done = ticket;
        self.spin_next = true;
        self.block
            .header()
            .done_seq
            .store(ticket, Ordering::Release);
    }

    /// A handle another thread of the child uses to end a wait early (see [`ChildWaker`]).
    pub fn waker(&self) -> ChildWaker {
        ChildWaker {
            doorbell: Arc::clone(&self.doorbell),
            kicked: Arc::clone(&self.kicked),
        }
    }

    /// Shows the parent that this thread is running: one relaxed store of a growing number, no
    /// allocation and no lock. [`ChildPort::wait_request`] does it on every pass, so a child whose
    /// audio thread loops on it is seen alive whatever its main thread is doing.
    fn beat(&mut self) {
        self.beats = self.beats.wrapping_add(1);
        self.block
            .header()
            .pulse
            .store(self.beats, Ordering::Relaxed);
    }

    /// Blocks until a request or `timeout`: right after an answered request it spins for about
    /// 50 us, then it parks on the doorbell (at once when the last call timed out). `None` on
    /// timeout, or when a [`ChildWaker`] ended the wait. A request taken but not completed is
    /// returned again by the next call. Every call (and every park inside it, which lasts at
    /// most [`MAX_PARK`]) bumps the block's pulse, the sign of life the parent reads without the
    /// plugin's main thread.
    pub fn wait_request(&mut self, timeout: Duration) -> Option<Request<'_>> {
        self.beat();
        let start = now();
        let end = start.checked_add(timeout).unwrap_or(start);
        let spin_end = if std::mem::take(&mut self.spin_next) {
            (start + SPIN).min(end)
        } else {
            start
        };
        loop {
            if let Some(ticket) = self.pending() {
                let req = self.block.request();
                // SAFETY: the acquire load in `pending` made the request visible, and the
                // parent writes nothing until we answer.
                let code = unsafe { addr_of!((*req).op).read() };
                match AudioOp::from_code(code) {
                    Some(op) => {
                        return Some(Request {
                            port: self,
                            ticket,
                            op,
                        });
                    }
                    None => {
                        self.refuse(ticket);
                        continue;
                    }
                }
            }
            if self.kicked.swap(false, Ordering::AcqRel) {
                return None;
            }
            let t = now();
            if t >= end {
                return None;
            }
            if t < spin_end {
                std::hint::spin_loop();
            } else {
                self.beat();
                self.doorbell.wait((end - t).min(MAX_PARK));
            }
        }
    }

    /// Writes the facts the parent reads when it builds its processor.
    pub fn set_info(&self, info: ProcessorInfo) {
        let header = self.block.header();
        header
            .info_rule
            .store(sleep_rule_code(info.sleep_rule), Ordering::Release);
        header
            .info_tail
            .store(tail_code(info.tail), Ordering::Release);
    }

    /// The plugin asked to be processed: the parent's `wake_requested` returns true once.
    pub fn request_wake(&self) {
        self.block.header().wake.store(1, Ordering::Release);
    }
}

/// Ends the waits of a [`ChildPort`] from another thread of the child: the audio thread then
/// looks at its other duties without waiting for its timeout.
#[derive(Debug, Clone)]
pub struct ChildWaker {
    doorbell: Arc<Doorbell>,
    kicked: Arc<AtomicBool>,
}

impl ChildWaker {
    /// Makes the running or the next `wait_request` return `None` at once. Never blocks.
    pub fn wake(&self) {
        self.kicked.store(true, Ordering::Release);
        self.doorbell.ring();
    }
}

/// A request the child has received. Answer it with [`Request::complete`]; if it is dropped
/// unanswered the parent keeps waiting, and the next `wait_request` returns it again.
#[derive(Debug)]
pub struct Request<'a> {
    port: &'a mut ChildPort,
    ticket: u64,
    op: AudioOp,
}

impl Request<'_> {
    /// The op to perform.
    pub fn op(&self) -> AudioOp {
        self.op
    }

    /// Runs `f` with a [`PluginIo`] over the request inputs and the response outputs
    /// (`main_out` arrives zeroed), then publishes the response with the status and the
    /// boolean `f` returned. Never allocates.
    pub fn complete(self, f: impl FnOnce(&mut PluginIo<'_>) -> (ProcessStatus, bool)) {
        let Request { port, ticket, .. } = self;
        let req = port.block.request();
        let resp = port.block.response();
        // SAFETY: the parent wrote the request before its release store of `req_seq` (acquired
        // in `pending`) and touches neither area until the final `done_seq` store, so both are
        // ours alone until then. Counts are clamped to the array sizes; the slices given to
        // `f` cover disjoint areas.
        let (status, ok, dropped) = unsafe {
            let main_in_ch = (addr_of!((*req).main_in_ch).read() as usize).min(MAX_CHANNELS);
            let sc_ch = (addr_of!((*req).sc_ch).read() as usize).min(MAX_CHANNELS);
            let main_out_ch = (addr_of!((*req).main_out_ch).read() as usize).min(MAX_CHANNELS);
            let aux_out_ch = (addr_of!((*req).aux_out_ch).read() as usize).min(MAX_AUX_CHANNELS);
            let n_in = (addr_of!((*req).n_events).read() as usize).min(MAX_EVENTS_IN);

            port.events_in.clear();
            let mut dropped = 0u32;
            let events = addr_of!((*req).events).cast::<WireEvent>();
            for i in 0..n_in {
                match events.add(i).read().decode() {
                    Some(e) => port.events_in.push(e),
                    None => dropped += 1,
                }
            }
            port.events_out.clear();
            port.events_out.take_dropped();

            let out = addr_of_mut!((*resp).main_out).cast::<Batch>();
            std::ptr::write_bytes(out, 0, MAX_CHANNELS);
            let aux = addr_of_mut!((*resp).aux_out).cast::<Batch>();
            std::ptr::write_bytes(aux, 0, aux_out_ch);

            let mut io = PluginIo {
                frames: BATCH,
                main_in: std::slice::from_raw_parts(
                    addr_of!((*req).main_in).cast::<Batch>(),
                    main_in_ch,
                ),
                main_in_silent: addr_of!((*req).main_in_silent).read() != 0,
                sidechain: std::slice::from_raw_parts(addr_of!((*req).sc).cast::<Batch>(), sc_ch),
                sidechain_silent: addr_of!((*req).sc_silent).read() != 0,
                main_out: std::slice::from_raw_parts_mut(out, main_out_ch),
                aux_out: std::slice::from_raw_parts_mut(aux, aux_out_ch),
                events_in: &port.events_in,
                events_out: &mut port.events_out,
                transport: addr_of!((*req).transport).read().decode(),
                dropped,
            };
            let (status, ok) = f(&mut io);
            (status, ok, io.dropped)
        };

        let dropped = dropped.saturating_add(port.events_out.take_dropped());
        // SAFETY: still ours alone (see above); `n` is at most the list capacity, which is
        // `MAX_EVENTS_OUT`, the size of the response array.
        unsafe {
            let events = addr_of_mut!((*resp).events).cast::<WireEvent>();
            let list = port.events_out.as_slice();
            for (i, e) in list.iter().take(MAX_EVENTS_OUT).enumerate() {
                events.add(i).write(WireEvent::encode(e));
            }
            addr_of_mut!((*resp).n_events).write(list.len().min(MAX_EVENTS_OUT) as u32);
            addr_of_mut!((*resp).ticket).write(ticket);
            addr_of_mut!((*resp).status).write(status_code(status));
            addr_of_mut!((*resp).ok).write(u32::from(ok));
            addr_of_mut!((*resp).dropped).write(dropped);
        }
        port.last_done = ticket;
        port.spin_next = true;
        // Release: the response written above is visible to the parent that sees this value.
        port.block
            .header()
            .done_seq
            .store(ticket, Ordering::Release);
    }
}
