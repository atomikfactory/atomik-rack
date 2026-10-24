//! Engine features: summing inputs, delay compensation (PDC) and lock-free meters.
//!
//! # Summing inputs
//!
//! A socket with `Direction::SummingInput` accepts any number of distinct cables (direction
//! and kind rules as for a plain input; a plain `Input` still takes one cable). The cycle check
//! sees summing edges like any other. Audio/Cv contributions are summed into the input's
//! scratch as a left fold, starting from `0.0`, in ascending **source endpoint** order
//! (device id, then socket). Notes contributions are merged by offset; on equal offsets,
//! scheduled notes come first, then the cables in the same source order. The order is fixed at
//! routing compile, so the sum is bit-identical for any host block size and worker count
//! (gathering always runs on the audio thread).
//!
//! # Delay compensation
//!
//! At routing compile, in topological order: every device's output latency is
//! `max(latency of the sources of each of its inputs) + latency(device)`, where the interface's
//! host inputs have latency 0 and `latency(device)` is `Device::latency_frames()` (read after
//! `prepare` and the stored params by `Graph::compile`; supplied per node through
//! `Topology::set_latency`, typically from `EngineHandle::fill_latencies`). At every **Audio/Cv
//! summing input**, each cable is delayed by `max_in − lat_in`, so every contribution arrives
//! aligned with the slowest one. Plain inputs are never delayed: a split whose branches meet
//! again at plain inputs is not compensated (the device's latency then follows its slowest
//! input). Notes contributions to a summing input are not delayed either. If one delay would
//! exceed [`MAX_COMPENSATION_SECONDS`] at the session rate, compensation is disabled for the
//! whole plan and [`PdcReport::limit_exceeded`] is set, so nothing is ever silently
//! misaligned. Compensation off (`set_compensation(false)`) means every delay is 0.
//!
//! Delay lines are allocated at compile, sized exactly to their delay, owned by the
//! routing plan and dropped with it on the non-RT side. When a routing replaces another at a
//! batch boundary, a cable whose (source, input, delay) is unchanged keeps its delay line
//! (the buffers are swapped, not copied, so this is O(1) and allocation-free); a cable whose
//! delay changed starts from a silent line (its contribution has a gap of the new delay). Other
//! cables, and every other summing input, are untouched.
//!
//! # Meters
//!
//! [`Meters`] is a fixed bank of [`METER_CAPACITY`] slots of atomics, allocated with the
//! engine. Metered output endpoints are registered on the topology; compile assigns slot `i` to
//! the `i`-th metered endpoint in ascending order. When the routing becomes current, the audio
//! thread writes each slot's endpoint key (and clears slots whose key changed). After each
//! batch, it publishes per metered endpoint the batch's peak (`fetch_max` on the f32 bits:
//! non-negative floats order like their bits) and RMS (`store`). Readers never block the
//! engine and the engine never waits for readers.

use crate::graph::Endpoint;
use atomik_core::{BATCH, Batch};
use atomik_device_sdk::ScopeTap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Maximum automatic compensation delay (decision: 10 s at the session rate).
pub const MAX_COMPENSATION_SECONDS: f64 = 10.0;

/// Maximum number of values per device in [`Meters::device_values`].
pub use atomik_device_sdk::MAX_METER_VALUES;

/// Number of meter slots in a [`Meters`] bank. Metered endpoints beyond it (in ascending
/// endpoint order) are not published and read as `None`.
pub const METER_CAPACITY: usize = 512;

/// Peak/RMS reading of one metered endpoint (linear amplitude), published once per batch.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterReading {
    /// Peak absolute sample since the previous read (reset on read).
    pub peak: f32,
    /// RMS over the last batch.
    pub rms: f32,
}

/// Compensation outcome of a compiled routing, for UI readouts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PdcReport {
    /// Whether compensation is active in this plan.
    pub active: bool,
    /// Set when compensation was requested but disabled because a delay exceeded the limit.
    pub limit_exceeded: bool,
    /// Added delay in frames per cable into a summing input: (from, to, frames).
    pub cable_delays: Vec<(Endpoint, Endpoint, u32)>,
    /// Output latency in frames per node output endpoint that feeds a summing input.
    pub latencies: Vec<(Endpoint, u32)>,
}

/// One meter slot: the endpoint key (0 = unused) and the published values (f32 bits).
#[derive(Default)]
struct MeterSlot {
    key: AtomicU64,
    peak: AtomicU32,
    rms: AtomicU32,
}

/// Lock-free meter bank shared by the engine (writer) and the UI (reader). Endpoints are
/// registered on the topology before compile; reading never blocks the engine.
pub struct Meters {
    slots: Box<[MeterSlot]>,
    devices: Box<[DeviceValueSlot]>,
    /// The scope taps of the arena devices that have one (`Device::scope_tap`). Only the control
    /// side (insert, remove, plan submit) and readers lock it; the audio thread never does.
    scopes: std::sync::Mutex<Vec<(crate::DeviceInstanceId, Arc<ScopeTap>)>>,
}

/// Published `Device::meter_values` of the device in one arena slot. `owner` is the device id
/// plus one (0 = empty); `seq` only ever grows (also across owners), so a reader that keeps
/// the last `seq` it saw can tell fresh from stale.
struct DeviceValueSlot {
    owner: AtomicU64,
    seq: AtomicU64,
    count: AtomicU32,
    values: [AtomicU32; MAX_METER_VALUES],
}

impl DeviceValueSlot {
    fn new() -> Self {
        Self {
            owner: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            count: AtomicU32::new(0),
            values: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Un-owns the slot and zeroes it (no sequence bump).
    fn wipe(&self) {
        self.owner.store(0, Ordering::Release);
        self.count.store(0, Ordering::Relaxed);
        for v in &self.values {
            v.store(0, Ordering::Relaxed);
        }
    }

    fn clear(&self) {
        self.wipe();
        self.seq.fetch_add(1, Ordering::Release);
    }
}

impl std::fmt::Debug for Meters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meters").finish_non_exhaustive()
    }
}

/// The non-zero key of an endpoint in a meter slot.
pub(crate) fn meter_key(e: Endpoint) -> u64 {
    ((u64::from(e.device.0) << 16) | u64::from(e.socket.0)) + 1
}

impl Meters {
    /// A bank of [`METER_CAPACITY`] empty endpoint slots and `arena_slots` empty device-value
    /// slots (non-RT).
    pub(crate) fn new(arena_slots: usize) -> Self {
        Self {
            slots: (0..METER_CAPACITY).map(|_| MeterSlot::default()).collect(),
            devices: (0..arena_slots).map(|_| DeviceValueSlot::new()).collect(),
            scopes: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Non-RT. Registers the scope tap of device `id` (replacing an earlier one of that id).
    pub(crate) fn register_scope(&self, id: crate::DeviceInstanceId, tap: Arc<ScopeTap>) {
        if let Ok(mut scopes) = self.scopes.lock() {
            scopes.retain(|(i, _)| *i != id);
            scopes.push((id, tap));
        }
    }

    /// Non-RT. Forgets the scope tap of device `id`.
    pub(crate) fn unregister_scope(&self, id: crate::DeviceInstanceId) {
        if let Ok(mut scopes) = self.scopes.lock() {
            scopes.retain(|(i, _)| *i != id);
        }
    }

    /// Non-RT. Forgets every scope tap (a plan replaced the whole arena).
    pub(crate) fn clear_scopes(&self) {
        if let Ok(mut scopes) = self.scopes.lock() {
            scopes.clear();
        }
    }

    /// The scope tap of device `id` (`Device::scope_tap`), if it has one and is in the engine.
    /// Read it with [`ScopeTap::read_latest`]: lock-free, the engine never waits for the reader.
    pub fn scope_tap(&self, id: crate::DeviceInstanceId) -> Option<Arc<ScopeTap>> {
        let scopes = self.scopes.lock().ok()?;
        scopes
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, tap)| Arc::clone(tap))
    }

    /// Reads the values device `id` last published through `Device::meter_values` into `out`
    /// (at most `out.len()`, and `MAX_METER_VALUES`). Returns `(values written, publish
    /// sequence)`; the sequence grows with every publish, so an unchanged sequence means no
    /// news. `None` if the device does not publish values (not in the engine, not opted in,
    /// removed or not yet published). Lock-free; each value is one atomic, so the values of
    /// one read may come from two adjacent batches.
    pub fn device_values(
        &self,
        id: crate::DeviceInstanceId,
        out: &mut [f32],
    ) -> Option<(usize, u64)> {
        let owner = u64::from(id.0) + 1;
        let slot = self
            .devices
            .iter()
            .find(|s| s.owner.load(Ordering::Acquire) == owner)?;
        let seq = slot.seq.load(Ordering::Acquire);
        let n = (slot.count.load(Ordering::Acquire) as usize)
            .min(out.len())
            .min(MAX_METER_VALUES);
        for (o, v) in out.iter_mut().zip(&slot.values[..n]) {
            *o = f32::from_bits(v.load(Ordering::Relaxed));
        }
        // Re-assigned meanwhile: the values are not ours.
        (slot.owner.load(Ordering::Acquire) == owner).then_some((n, seq))
    }

    /// RT: publishes `values` as the latest of the device `id` in arena slot `slot`. Claims the
    /// slot (clearing it) when another owner held it. Atomics only.
    pub(crate) fn publish_device_values(
        &self,
        slot: usize,
        id: crate::DeviceInstanceId,
        values: &[f32],
    ) {
        let Some(s) = self.devices.get(slot) else {
            return;
        };
        let owner = u64::from(id.0) + 1;
        if s.owner.load(Ordering::Relaxed) != owner {
            s.wipe();
            s.owner.store(owner, Ordering::Release);
        }
        let n = values.len().min(MAX_METER_VALUES);
        for (cell, v) in s.values.iter().zip(&values[..n]) {
            cell.store(v.to_bits(), Ordering::Relaxed);
        }
        s.count.store(n as u32, Ordering::Release);
        s.seq.fetch_add(1, Ordering::Release);
    }

    /// RT: empties the device-value slot `slot` (its device was removed).
    pub(crate) fn clear_device_values(&self, slot: usize) {
        if let Some(s) = self.devices.get(slot)
            && s.owner.load(Ordering::Relaxed) != 0
        {
            s.clear();
        }
    }

    /// RT: empties every device-value slot (the arena was swapped).
    pub(crate) fn clear_all_device_values(&self) {
        for i in 0..self.devices.len() {
            self.clear_device_values(i);
        }
    }

    /// Reads (and resets the peak of) a metered output endpoint; `None` if not metered.
    ///
    /// Lock-free: a few atomic operations plus a scan of the occupied slots. While the engine
    /// is installing a new routing, a read may briefly return `None`.
    pub fn read(&self, endpoint: Endpoint) -> Option<MeterReading> {
        let key = meter_key(endpoint);
        for slot in self.slots.iter() {
            match slot.key.load(Ordering::Acquire) {
                0 => return None,
                k if k == key => {
                    let peak = f32::from_bits(slot.peak.swap(0, Ordering::AcqRel));
                    let rms = f32::from_bits(slot.rms.load(Ordering::Acquire));
                    // The slot was re-assigned meanwhile: the values are not ours.
                    return (slot.key.load(Ordering::Acquire) == key)
                        .then_some(MeterReading { peak, rms });
                }
                _ => {}
            }
        }
        None
    }

    /// RT: makes slot `i` hold `keys[i]` (and empties the rest). Slots whose key changes are
    /// cleared. Only atomic stores; never allocates.
    pub(crate) fn assign(&self, keys: &[u64]) {
        for (i, slot) in self.slots.iter().enumerate() {
            let want = keys.get(i).copied().unwrap_or(0);
            if slot.key.load(Ordering::Relaxed) != want {
                slot.key.store(0, Ordering::Release);
                slot.peak.store(0, Ordering::Relaxed);
                slot.rms.store(0, Ordering::Relaxed);
                slot.key.store(want, Ordering::Release);
            }
        }
    }

    /// RT: publishes one batch of slot `index` (peak since the last read, RMS of this batch).
    pub(crate) fn publish(&self, index: usize, batch: &Batch) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        let mut peak = 0.0f32;
        let mut squares = 0.0f32;
        for s in batch {
            let a = s.abs();
            if a > peak {
                peak = a; // NaN never compares greater, so it is ignored
            }
            squares += s * s;
        }
        let rms = (squares / BATCH as f32).sqrt();
        slot.peak.fetch_max(peak.to_bits(), Ordering::AcqRel);
        slot.rms.store(rms.to_bits(), Ordering::Release);
    }
}

/// A PDC delay line of a cable into a summing input, sized exactly to its delay (≥ 1).
pub(crate) struct DelayLine {
    /// The cable (source, summing input), for keeping state across routing swaps.
    pub(crate) from: Endpoint,
    pub(crate) to: Endpoint,
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    /// A silent line of `delay` frames (non-RT; `delay` ≥ 1).
    pub(crate) fn new(from: Endpoint, to: Endpoint, delay: u32) -> Self {
        Self {
            from,
            to,
            buf: vec![0.0; delay.max(1) as usize],
            pos: 0,
        }
    }

    /// RT: `out[n] = input[n - delay]`, continuing from the previous batch.
    pub(crate) fn process(&mut self, input: &Batch, out: &mut Batch) {
        let len = self.buf.len();
        for (o, i) in out.iter_mut().zip(input) {
            *o = self.buf[self.pos];
            self.buf[self.pos] = *i;
            self.pos += 1;
            if self.pos == len {
                self.pos = 0;
            }
        }
    }

    /// RT: takes over `old`'s state if it is the same cable with the same delay. Swaps the
    /// buffers (no allocation, no free: `old` is retired and dropped off the audio thread).
    pub(crate) fn adopt(&mut self, old: &mut Self) -> bool {
        if (self.from, self.to, self.buf.len()) != (old.from, old.to, old.buf.len()) {
            return false;
        }
        std::mem::swap(&mut self.buf, &mut old.buf);
        self.pos = old.pos;
        true
    }

    fn key(&self) -> (Endpoint, Endpoint) {
        (self.to, self.from)
    }
}

/// RT: for every line of `new`, adopts the matching line of `old`. Both lists are sorted by
/// (input, source), so this is one merge walk.
pub(crate) fn adopt_delay_lines(new: &mut [DelayLine], old: &mut [DelayLine]) {
    let mut j = 0;
    for line in new.iter_mut() {
        while j < old.len() && old[j].key() < line.key() {
            j += 1;
        }
        if let Some(o) = old.get_mut(j)
            && o.key() == line.key()
        {
            line.adopt(o);
        }
    }
}

/// Device latencies published by the engine, per arena slot, plus a change counter. Written by
/// the handle when it inserts a device (synchronously known) and by the audio thread after each
/// batch boundary's params and automation, so a param that changes a device's latency (such as
/// a channel's `latency.adjust`) is seen without any engine round trip.
pub(crate) struct Latencies {
    slots: Box<[AtomicU32]>,
    epoch: AtomicU64,
}

impl Latencies {
    /// `capacity` zeroed slots (non-RT).
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            epoch: AtomicU64::new(0),
        }
    }

    /// Stores the latency of the device in `slot`; bumps the epoch if it changed.
    pub(crate) fn publish(&self, slot: usize, frames: u32) {
        if let Some(s) = self.slots.get(slot)
            && s.swap(frames, Ordering::AcqRel) != frames
        {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The latency last published for `slot`.
    pub(crate) fn get(&self, slot: usize) -> u32 {
        self.slots
            .get(slot)
            .map_or(0, |s| s.load(Ordering::Acquire))
    }

    /// Number of changes published so far.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DeviceInstanceId;
    use atomik_device_sdk::SocketId;

    fn ep(d: u32, s: u16) -> Endpoint {
        Endpoint {
            device: DeviceInstanceId(d),
            socket: SocketId(s),
        }
    }

    #[test]
    fn meter_keys_are_unique_non_zero_and_ordered_like_endpoints() {
        let a = meter_key(ep(0, 0));
        let b = meter_key(ep(0, 1));
        let c = meter_key(ep(1, 0));
        let d = meter_key(ep(u32::MAX, u16::MAX));
        assert!(0 < a && a < b && b < c && c < d);
    }

    #[test]
    fn meters_publish_read_assign_without_allocation() {
        let m = Meters::new(2);
        let keys = [meter_key(ep(1, 0)), meter_key(ep(2, 3))];
        let mut batch = [0.0f32; BATCH];
        batch[3] = -0.5;
        batch[4] = f32::NAN;
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            m.assign(&keys);
            m.publish(1, &batch);
            m.publish(METER_CAPACITY, &batch); // out of range: ignored
        });
        let r = m.read(ep(2, 3)).unwrap();
        assert_eq!(r.peak, 0.5);
        assert!(
            r.rms.is_nan(),
            "a NaN sample poisons the RMS, never the peak"
        );
        assert_eq!(m.read(ep(1, 0)), Some(MeterReading::default()));
        assert_eq!(m.read(ep(9, 9)), None);
        // Re-assigning a slot to another endpoint clears it.
        m.assign(&keys[1..]);
        assert_eq!(m.read(ep(1, 0)), None);
        let r = m.read(ep(2, 3)).unwrap();
        assert_eq!(r.peak, 0.0, "cleared on re-assignment");
        m.assign(&[]);
        assert_eq!(m.read(ep(2, 3)), None);
    }

    #[test]
    fn delay_line_delays_across_batches_and_adopts_matching_state() {
        let (a, b, c) = (ep(1, 0), ep(2, 0), ep(3, 0));
        let mut line = DelayLine::new(a, b, 70);
        let mut input = [0.0f32; BATCH];
        input[10] = 1.0;
        let mut out = [0.0f32; BATCH];
        let mut seen = Vec::new();
        for k in 0..3 {
            line.process(&input, &mut out);
            input = [0.0; BATCH];
            seen.extend(
                out.iter()
                    .enumerate()
                    .filter(|(_, s)| **s != 0.0)
                    .map(|(n, _)| k * BATCH + n),
            );
        }
        assert_eq!(seen, vec![80]);
        // Adoption: same cable and delay only.
        let mut old = DelayLine::new(a, b, 3);
        old.process(&[1.0; BATCH], &mut out);
        let mut same = DelayLine::new(a, b, 3);
        assert!(same.adopt(&mut old));
        same.process(&[0.0; BATCH], &mut out);
        assert_eq!(&out[..4], &[1.0, 1.0, 1.0, 0.0]);
        let mut other = DelayLine::new(a, c, 3);
        assert!(!other.adopt(&mut DelayLine::new(a, b, 3)));
        let mut longer = DelayLine::new(a, b, 4);
        assert!(!longer.adopt(&mut DelayLine::new(a, b, 3)));
        // Merge walk over sorted lists.
        let mut olds = vec![DelayLine::new(a, b, 2), DelayLine::new(c, b, 2)];
        olds[1].process(&[0.5; BATCH], &mut out);
        let mut news = vec![DelayLine::new(c, b, 2), DelayLine::new(a, c, 2)];
        news.sort_by_key(DelayLine::key);
        olds.sort_by_key(DelayLine::key);
        adopt_delay_lines(&mut news, &mut olds);
        let k = news.iter().position(|l| l.from == c).unwrap();
        news[k].process(&[0.0; BATCH], &mut out);
        assert_eq!(&out[..3], &[0.5, 0.5, 0.0]);
    }

    #[test]
    fn latencies_count_changes() {
        let l = Latencies::new(4);
        assert_eq!((l.get(2), l.epoch()), (0, 0));
        l.publish(2, 0);
        assert_eq!(l.epoch(), 0);
        l.publish(2, 64);
        l.publish(2, 64);
        l.publish(9, 1); // out of range: ignored
        assert_eq!((l.get(2), l.get(9), l.epoch()), (64, 0, 1));
    }
}
