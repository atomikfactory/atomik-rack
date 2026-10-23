//! The parent end of the audio exchange, used by the engine-side processor.
//!
//! Every method is wait-free except [`ParentPort::wait_done`], which spins (never sleeps).
//! Nothing here allocates, locks or makes a blocking call: the buffers are the mapping, and the
//! only system call is the doorbell's non-blocking wake-up.

use crate::block::SharedBlock;
use crate::codec::{AudioOp, sleep_rule_from_code, status_from_code, tail_from_code};
use crate::deadline::{Deadline, now};
use crate::doorbell::Doorbell;
use crate::layout::{MAX_CHANNELS, MAX_EVENTS_IN, MAX_EVENTS_OUT, WireEvent, WireTransport};
use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{PluginIo, PluginTransport, ProcessStatus, SleepRule, TailLength};
use std::ptr::{addr_of, addr_of_mut, copy_nonoverlapping};
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Static facts the child writes at activation and the parent reads (sleep rule, tail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorInfo {
    /// Which skipping rule the device applies.
    pub sleep_rule: SleepRule,
    /// How long the plugin keeps sounding after its input stops.
    pub tail: TailLength,
}

/// The receipt of one submitted request: its sequence number. Tickets are never reused, so a
/// result that arrives late cannot be taken for the answer to a later request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

impl Ticket {
    /// The ticket of a request that was not submitted (one was still in flight).
    pub const NONE: Ticket = Ticket(0);

    /// The sequence number (first request: 1).
    pub fn sequence(self) -> u64 {
        self.0
    }

    /// True for [`Ticket::NONE`].
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// How a wait or a poll ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// The child answered (or the ticket was superseded: there is nothing left to wait for).
    Done,
    /// Not answered yet.
    Pending,
    /// The child is gone.
    Dead,
    /// The deadline came first (the request is still in flight).
    TimedOut,
}

/// What the child answered besides the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    /// The processor's status ([`ProcessStatus::Late`] for a ticket that is no longer current).
    pub status: ProcessStatus,
    /// The op's boolean result (`start_processing`).
    pub ok: bool,
}

/// The parent end of one block. One thread at a time (`&mut self` for everything that writes the
/// request area or reads the response area).
#[derive(Debug)]
pub struct ParentPort {
    block: SharedBlock,
    doorbell: Doorbell,
    /// Events dropped at submit since the last response was taken.
    dropped_in: u32,
}

impl ParentPort {
    /// A port over `block`, which `doorbell` was made for. Construction may allocate.
    pub fn new(block: SharedBlock, doorbell: Doorbell) -> Self {
        Self {
            block,
            doorbell,
            dropped_in: 0,
        }
    }

    /// The ticket the next submission gets, or `None` while a request is in flight.
    fn next_ticket(&self) -> Option<u64> {
        let header = self.block.header();
        let sent = header.req_seq.load(Ordering::Relaxed);
        // Acquire: everything the child did with the request happens before we overwrite it.
        let done = header.done_seq.load(Ordering::Acquire);
        (done == sent).then_some(sent + 1)
    }

    /// Publishes the request written under `ticket` and rings. The release store makes the
    /// request area visible to the child before it sees the new sequence number.
    fn publish(&self, ticket: u64) -> Ticket {
        self.block.header().req_seq.store(ticket, Ordering::Release);
        self.doorbell.ring();
        Ticket(ticket)
    }

    /// Copies the request (op, the inputs of `io`, its events and transport) and rings. Never
    /// allocates. Events beyond [`MAX_EVENTS_IN`] are dropped and counted (they come back in
    /// `io.dropped` with the response). Returns [`Ticket::NONE`] and writes nothing when a
    /// request is still in flight: check [`ParentPort::outstanding`] first.
    pub fn submit(&mut self, op: AudioOp, io: &PluginIo<'_>) -> Ticket {
        let Some(ticket) = self.next_ticket() else {
            return Ticket::NONE;
        };
        let req = self.block.request();
        let main_in = &io.main_in[..io.main_in.len().min(MAX_CHANNELS)];
        let sidechain = &io.sidechain[..io.sidechain.len().min(MAX_CHANNELS)];
        let n_events = io.events_in.len().min(MAX_EVENTS_IN);
        self.dropped_in = self
            .dropped_in
            .saturating_add((io.events_in.len() - n_events) as u32);
        // SAFETY: no request is in flight (`next_ticket`), so the child does not touch the
        // request area, and the done_seq acquire there ordered its last reads before these
        // writes. Counts are clamped to the array sizes, so every pointer is inside the
        // mapping. `&mut self` keeps a second writer out.
        unsafe {
            addr_of_mut!((*req).ticket).write(ticket);
            addr_of_mut!((*req).op).write(op as u32);
            addr_of_mut!((*req).main_in_ch).write(main_in.len() as u32);
            addr_of_mut!((*req).main_in_silent).write(u32::from(io.main_in_silent));
            addr_of_mut!((*req).sc_ch).write(sidechain.len() as u32);
            addr_of_mut!((*req).sc_silent).write(u32::from(io.sidechain_silent));
            addr_of_mut!((*req).main_out_ch).write(io.main_out.len().min(MAX_CHANNELS) as u32);
            addr_of_mut!((*req).n_events).write(n_events as u32);
            addr_of_mut!((*req).transport).write(WireTransport::encode(&io.transport));
            copy_batches(addr_of_mut!((*req).main_in).cast::<f32>(), main_in);
            copy_batches(addr_of_mut!((*req).sc).cast::<f32>(), sidechain);
            let events = addr_of_mut!((*req).events).cast::<WireEvent>();
            for (i, event) in io.events_in[..n_events].iter().enumerate() {
                events.add(i).write(WireEvent::encode(event));
            }
        }
        self.publish(ticket)
    }

    /// Submits an op that carries no audio (`Start`, `Stop`, `Reset`). Never allocates. Returns
    /// [`Ticket::NONE`] when a request is still in flight.
    pub fn submit_op(&mut self, op: AudioOp) -> Ticket {
        let Some(ticket) = self.next_ticket() else {
            return Ticket::NONE;
        };
        let req = self.block.request();
        // SAFETY: as in `submit`: no request is in flight, every pointer is inside the mapping.
        unsafe {
            addr_of_mut!((*req).ticket).write(ticket);
            addr_of_mut!((*req).op).write(op as u32);
            addr_of_mut!((*req).main_in_ch).write(0);
            addr_of_mut!((*req).main_in_silent).write(1);
            addr_of_mut!((*req).sc_ch).write(0);
            addr_of_mut!((*req).sc_silent).write(1);
            addr_of_mut!((*req).main_out_ch).write(0);
            addr_of_mut!((*req).n_events).write(0);
            addr_of_mut!((*req).transport)
                .write(WireTransport::encode(&PluginTransport::default()));
        }
        self.publish(ticket)
    }

    /// Where the exchange of `t` stands, without waiting. A ticket that is not the latest has
    /// nothing left to wait for and is `Done` (its response is gone: see
    /// [`ParentPort::take_response`]). A result the child published before dying is `Done`.
    pub fn poll(&self, t: Ticket) -> Poll {
        let header = self.block.header();
        if t.is_none() || t.0 != header.req_seq.load(Ordering::Relaxed) {
            return Poll::Done;
        }
        if header.done_seq.load(Ordering::Acquire) >= t.0 {
            Poll::Done
        } else if header.child_dead.load(Ordering::Acquire) != 0 {
            Poll::Dead
        } else {
            Poll::Pending
        }
    }

    /// Spins (with `spin_loop`, and a `yield_now` every 64 spins; never sleeps) until `t` is
    /// done, the child is dead or the deadline has come. [`Deadline::Never`] still ends when
    /// the child dies. The result is checked before the deadline, so a late answer that is
    /// already there wins.
    pub fn wait_done(&self, t: Ticket, deadline: Deadline) -> Poll {
        let mut spins: u32 = 0;
        loop {
            match self.poll(t) {
                Poll::Pending => {}
                other => return other,
            }
            // Reading the clock costs more than a spin: look every 16 spins, and at once.
            if let Deadline::At(at) = deadline
                && spins.is_multiple_of(16)
                && reached(at)
            {
                return Poll::TimedOut;
            }
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(64) {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }

    /// Copies the response of `t` into `io` (`main_out`, events into `events_out`, the drop
    /// counts into `dropped`) and returns it. Never allocates.
    ///
    /// Only the latest ticket, once done, has a response. For any other ticket, or one still in
    /// flight, nothing is copied and the answer is [`ProcessStatus::Late`].
    pub fn take_response(&mut self, t: Ticket, io: &mut PluginIo<'_>) -> Response {
        const LATE: Response = Response {
            status: ProcessStatus::Late,
            ok: false,
        };
        let header = self.block.header();
        if t.is_none()
            || t.0 != header.req_seq.load(Ordering::Relaxed)
            || header.done_seq.load(Ordering::Acquire) < t.0
        {
            return LATE;
        }
        let resp = self.block.response();
        // SAFETY: the acquire load of done_seq above pairs with the child's release store after
        // it wrote the response, and the child writes nothing more until the next request,
        // which cannot be submitted while we hold `&mut self`. Every pointer is inside the
        // mapping: the counts read are clamped to the array sizes.
        unsafe {
            if addr_of!((*resp).ticket).read() != t.0 {
                return LATE;
            }
            let channels = io.main_out.len().min(MAX_CHANNELS);
            let src = addr_of!((*resp).main_out).cast::<f32>();
            for (ch, out) in io.main_out[..channels].iter_mut().enumerate() {
                copy_nonoverlapping(src.add(ch * BATCH), out.as_mut_ptr(), BATCH);
            }
            let n = (addr_of!((*resp).n_events).read() as usize).min(MAX_EVENTS_OUT);
            let events = addr_of!((*resp).events).cast::<WireEvent>();
            for i in 0..n {
                match events.add(i).read().decode() {
                    Some(e) => {
                        io.events_out.push(e.offset, e.kind);
                    }
                    None => io.dropped = io.dropped.saturating_add(1),
                }
            }
            io.dropped = io
                .dropped
                .saturating_add(addr_of!((*resp).dropped).read())
                .saturating_add(std::mem::take(&mut self.dropped_in));
            Response {
                status: status_from_code(addr_of!((*resp).status).read()),
                ok: addr_of!((*resp).ok).read() != 0,
            }
        }
    }

    /// The request in flight (submitted, not yet answered), if any.
    pub fn outstanding(&self) -> Option<Ticket> {
        let header = self.block.header();
        let sent = header.req_seq.load(Ordering::Relaxed);
        (header.done_seq.load(Ordering::Acquire) < sent).then_some(Ticket(sent))
    }

    /// What the child wrote at activation.
    pub fn info(&self) -> ProcessorInfo {
        let header = self.block.header();
        ProcessorInfo {
            sleep_rule: sleep_rule_from_code(header.info_rule.load(Ordering::Acquire)),
            tail: tail_from_code(header.info_tail.load(Ordering::Acquire)),
        }
    }

    /// True once (and clears) when the plugin asked to be processed.
    pub fn wake_requested(&self) -> bool {
        self.block.header().wake.swap(0, Ordering::AcqRel) != 0
    }

    /// Marks the child as gone (the reader thread calls it on child exit): every wait ends.
    pub fn mark_dead(&self) {
        self.block.header().child_dead.store(1, Ordering::Release);
    }

    /// Removes the block file now (see [`SharedBlock::unlink`]): call it once the child has
    /// said Hello. Not an audio method: it does I/O.
    pub fn unlink_block(&self) -> std::io::Result<()> {
        self.block.unlink()
    }

    /// A handle on the same death flag that keeps the mapping alive by itself, for the thread
    /// that supervises the child. Allocation-free to use; taking it may allocate.
    pub fn death_handle(&self) -> DeathHandle {
        DeathHandle {
            block: self.block.share(),
        }
    }

    /// True once the child is gone.
    pub fn is_dead(&self) -> bool {
        self.block.header().child_dead.load(Ordering::Acquire) != 0
    }
}

/// The death flag of a block, owned apart from the [`ParentPort`]. It holds the mapping, so it
/// stays valid after the port is dropped; the block file is removed when the last owner of the
/// mapping (port, handle or doorbell) drops.
#[derive(Debug)]
pub struct DeathHandle {
    block: SharedBlock,
}

impl Clone for DeathHandle {
    fn clone(&self) -> Self {
        Self {
            block: self.block.share(),
        }
    }
}

impl DeathHandle {
    /// Marks the child as gone: every wait on the port ends with `Dead`.
    pub fn mark_dead(&self) {
        self.block.header().child_dead.store(1, Ordering::Release);
    }

    /// True once the child is gone.
    pub fn is_dead(&self) -> bool {
        self.block.header().child_dead.load(Ordering::Acquire) != 0
    }
}

fn reached(at: Instant) -> bool {
    now() >= at
}

/// Copies `batches` to consecutive 64-float slots at `dst`.
///
/// # Safety
/// `dst` must be valid for `batches.len() * BATCH` writes of `f32` and not overlap `batches`.
unsafe fn copy_batches(dst: *mut f32, batches: &[Batch]) {
    for (i, b) in batches.iter().enumerate() {
        // SAFETY: the caller guarantees room for every batch; `b` is a 64-float array.
        unsafe { copy_nonoverlapping(b.as_ptr(), dst.add(i * BATCH), BATCH) };
    }
}
