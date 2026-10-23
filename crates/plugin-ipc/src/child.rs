//! The child end of the audio exchange, used by the child's audio thread.
//!
//! The child may allocate when the port is made; after that the exchange does not: the decoded
//! input events and the output event list are preallocated at their fixed capacities.

use crate::block::SharedBlock;
use crate::codec::{AudioOp, sleep_rule_code, status_code, tail_code};
use crate::deadline::now;
use crate::doorbell::Doorbell;
use crate::layout::{MAX_CHANNELS, MAX_EVENTS_IN, MAX_EVENTS_OUT, WireEvent};
use crate::parent::ProcessorInfo;
use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{EventList, PluginEvent, PluginIo, ProcessStatus};
use std::ptr::{addr_of, addr_of_mut};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// How long the child spins on the request counter before it parks on the doorbell. A parent
/// that rings once per 64-frame batch is back within this time, so the child never sleeps.
const SPIN: Duration = Duration::from_micros(50);

/// The child end of one block.
#[derive(Debug)]
pub struct ChildPort {
    block: SharedBlock,
    doorbell: Doorbell,
    /// The last ticket answered; the child is the only writer of `done_seq`.
    last_done: u64,
    events_in: Vec<PluginEvent>,
    events_out: EventList,
}

impl ChildPort {
    /// A port over `block`, which `doorbell` was opened for. Allocates the event buffers.
    pub fn new(block: SharedBlock, doorbell: Doorbell) -> Self {
        let last_done = block.header().done_seq.load(Ordering::Acquire);
        Self {
            block,
            doorbell,
            last_done,
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
        self.block
            .header()
            .done_seq
            .store(ticket, Ordering::Release);
    }

    /// Blocks until a request or `timeout`: spins for about 50 us, then parks on the doorbell.
    /// `None` on timeout. A request taken but not completed is returned again by the next call.
    pub fn wait_request(&mut self, timeout: Duration) -> Option<Request<'_>> {
        let start = now();
        let end = start.checked_add(timeout).unwrap_or(start);
        let spin_end = (start + SPIN).min(end);
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
            let t = now();
            if t >= end {
                return None;
            }
            if t < spin_end {
                std::hint::spin_loop();
            } else {
                self.doorbell.wait(end - t);
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
        // Release: the response written above is visible to the parent that sees this value.
        port.block
            .header()
            .done_seq
            .store(ticket, Ordering::Release);
    }
}
