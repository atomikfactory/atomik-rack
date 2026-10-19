//! `atomik.note-split`: routes the notes of one input to up to eight outputs by key and velocity
//! range. Internal: the model's expansion creates one per container.

use atomik_core::{NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

/// Type id of the split device.
pub const TYPE_ID: &str = "atomik.note-split";

/// Number of output slots.
pub const SLOTS: usize = 8;
/// Params per slot.
pub const PARAMS_PER_SLOT: usize = 6;

const fn sock(
    id: u16,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 9] = [
    sock(
        0,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    sock(1, "out_1", "Out 1", SocketKind::Notes, Direction::Output),
    sock(2, "out_2", "Out 2", SocketKind::Notes, Direction::Output),
    sock(3, "out_3", "Out 3", SocketKind::Notes, Direction::Output),
    sock(4, "out_4", "Out 4", SocketKind::Notes, Direction::Output),
    sock(5, "out_5", "Out 5", SocketKind::Notes, Direction::Output),
    sock(6, "out_6", "Out 6", SocketKind::Notes, Direction::Output),
    sock(7, "out_7", "Out 7", SocketKind::Notes, Direction::Output),
    sock(8, "out_8", "Out 8", SocketKind::Notes, Direction::Output),
];

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: range.0,
        max: range.1,
        default: range.2,
        taper: Taper::Linear,
        unit,
        discrete: true,
    }
}

macro_rules! slot_params {
    ($(($n:literal, $b:literal)),* $(,)?) => {
        [$(
            param($b, concat!("s", $n, ".on"), concat!("Slot ", $n, " on"), (0.0, 1.0, 0.0), Unit::Toggle),
            param($b + 1, concat!("s", $n, ".key_lo"), concat!("Slot ", $n, " low key"), (0.0, 127.0, 0.0), Unit::None),
            param($b + 2, concat!("s", $n, ".key_hi"), concat!("Slot ", $n, " high key"), (0.0, 127.0, 127.0), Unit::None),
            param($b + 3, concat!("s", $n, ".vel_lo"), concat!("Slot ", $n, " low velocity"), (1.0, 127.0, 1.0), Unit::None),
            param($b + 4, concat!("s", $n, ".vel_hi"), concat!("Slot ", $n, " high velocity"), (1.0, 127.0, 127.0), Unit::None),
            param($b + 5, concat!("s", $n, ".transpose"), concat!("Slot ", $n, " transpose"), (-48.0, 48.0, 0.0), Unit::Semitones),
        )*]
    };
}

static PARAMS: [ParamDescriptor; SLOTS * PARAMS_PER_SLOT] = slot_params!(
    (1, 0),
    (2, 6),
    (3, 12),
    (4, 18),
    (5, 24),
    (6, 30),
    (7, 36),
    (8, 42),
);

/// Static description of the split device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: TYPE_ID,
    name: "Note split",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`NoteSplit`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(NoteSplit::new()),
    }
}

/// "No route" marker of the route table.
const NO_ROUTE: u8 = 0xFF;
/// Keys per slot table.
const KEYS: usize = 128;
const NOTES_IN: SocketId = SocketId(0);

/// One output slot: its zone and the per-key route table (output key sent with the On, or
/// [`NO_ROUTE`]). The zone only affects new Ons; an Off always uses the stored route.
#[derive(Clone)]
struct Slot {
    on: bool,
    key_lo: u8,
    key_hi: u8,
    vel_lo: u8,
    vel_hi: u8,
    transpose: i16,
    route: [u8; KEYS],
    /// Per output key, the input key that currently owns the open note there (or [`NO_ROUTE`]):
    /// two input keys that transpose onto one output key never leave two open Ons.
    owner: [u8; KEYS],
    /// Offset of the last event written to this slot's output in this batch.
    last_offset: u16,
}

impl Slot {
    const fn new() -> Self {
        Self {
            on: false,
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            transpose: 0,
            route: [NO_ROUTE; KEYS],
            owner: [NO_ROUTE; KEYS],
            last_offset: 0,
        }
    }

    fn clear(&mut self) {
        self.route = [NO_ROUTE; KEYS];
        self.owner = [NO_ROUTE; KEYS];
    }

    /// Closes the note that input key `key` opened, if it still owns its output key.
    fn close(&mut self, ctx: &mut ProcessContext<'_>, out: SocketId, offset: u16, key: u8) {
        let routed = self.route[usize::from(key)];
        if routed == NO_ROUTE {
            return;
        }
        self.route[usize::from(key)] = NO_ROUTE;
        if self.owner[usize::from(routed)] == key {
            self.owner[usize::from(routed)] = NO_ROUTE;
            self.emit(ctx, out, offset, NoteKind::Off { key: routed });
        }
    }

    fn holds(&self) -> bool {
        self.owner.iter().any(|r| *r != NO_ROUTE)
    }

    fn accepts(&self, key: u8, velocity: u8) -> bool {
        self.on
            && (self.key_lo..=self.key_hi).contains(&key)
            && (self.vel_lo..=self.vel_hi).contains(&velocity)
    }

    /// Writes an event at `offset` (never earlier than the previous one); false if the buffer is
    /// full.
    fn emit(
        &mut self,
        ctx: &mut ProcessContext<'_>,
        out: SocketId,
        offset: u16,
        kind: NoteKind,
    ) -> bool {
        let offset = offset.max(self.last_offset);
        if ctx.notes_out(out).push(NoteEvent { offset, kind }).is_err() {
            return false;
        }
        self.last_offset = offset;
        true
    }
}

/// The split device.
pub struct NoteSplit {
    slots: [Slot; SLOTS],
    /// Send an `AllOff` on every output at the start of the next batch (after a `reset` that left
    /// notes open).
    flush_all_off: bool,
}

impl NoteSplit {
    /// A split with every slot off.
    pub fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; SLOTS],
            flush_all_off: false,
        }
    }
}

impl Default for NoteSplit {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for NoteSplit {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {
        self.reset();
        self.flush_all_off = false;
    }

    fn reset(&mut self) {
        if self.slots.iter().any(Slot::holds) {
            self.flush_all_off = true;
        }
        for s in &mut self.slots {
            s.clear();
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let id = id.0 as usize;
        let Some(slot) = self.slots.get_mut(id / PARAMS_PER_SLOT) else {
            return;
        };
        let v = plain.round();
        match id % PARAMS_PER_SLOT {
            0 => slot.on = plain >= 0.5,
            1 => slot.key_lo = v.clamp(0.0, 127.0) as u8,
            2 => slot.key_hi = v.clamp(0.0, 127.0) as u8,
            3 => slot.vel_lo = v.clamp(1.0, 127.0) as u8,
            4 => slot.vel_hi = v.clamp(1.0, 127.0) as u8,
            _ => slot.transpose = v.clamp(-48.0, 48.0) as i16,
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for s in &mut self.slots {
            s.last_offset = 0;
        }
        if self.flush_all_off {
            self.flush_all_off = false;
            for (k, s) in self.slots.iter_mut().enumerate() {
                s.emit(ctx, SocketId(1 + k as u16), 0, NoteKind::AllOff);
            }
        }
        let n = ctx.notes_in(NOTES_IN).len();
        for i in 0..n {
            let ev = ctx.notes_in(NOTES_IN)[i];
            for (k, s) in self.slots.iter_mut().enumerate() {
                let out = SocketId(1 + k as u16);
                match ev.kind {
                    NoteKind::On { key, velocity } => {
                        let key = key & 0x7F;
                        let ku = usize::from(key);
                        // A key that is still open on this slot is closed before it is replayed,
                        // so every output stays balanced.
                        s.close(ctx, out, ev.offset, key);
                        let v127 = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                        if s.accepts(key, v127) {
                            let shifted = (i16::from(key) + s.transpose).clamp(0, 127) as u8;
                            let su = usize::from(shifted);
                            // Another input key already sounds on this output key: close it.
                            let other = s.owner[su];
                            if other != NO_ROUTE {
                                s.route[usize::from(other)] = NO_ROUTE;
                                s.owner[su] = NO_ROUTE;
                                s.emit(ctx, out, ev.offset, NoteKind::Off { key: shifted });
                            }
                            if s.emit(
                                ctx,
                                out,
                                ev.offset,
                                NoteKind::On {
                                    key: shifted,
                                    velocity,
                                },
                            ) {
                                s.route[ku] = shifted;
                                s.owner[su] = key;
                            }
                        }
                    }
                    NoteKind::Off { key } => s.close(ctx, out, ev.offset, key & 0x7F),
                    NoteKind::AllOff => {
                        s.clear();
                        s.emit(ctx, out, ev.offset, NoteKind::AllOff);
                    }
                    // Channel messages go to every enabled slot, whatever its
                    // zone: a pitch bend or a CC belongs to the whole channel.
                    NoteKind::PitchBend { .. }
                    | NoteKind::Cc { .. }
                    | NoteKind::ChannelPressure { .. } => {
                        if s.on {
                            s.emit(ctx, out, ev.offset, ev.kind);
                        }
                    }
                    // Key messages follow the per-key route table: they reach only the slots that
                    // opened this key, with the output key of the transposed note.
                    NoteKind::PolyPressure { key, value } => {
                        let routed = s.route[usize::from(key & 0x7F)];
                        if routed != NO_ROUTE {
                            s.emit(
                                ctx,
                                out,
                                ev.offset,
                                NoteKind::PolyPressure { key: routed, value },
                            );
                        }
                    }
                    NoteKind::NoteExpr { key, kind, value } => {
                        let routed = s.route[usize::from(key & 0x7F)];
                        if routed != NO_ROUTE {
                            s.emit(
                                ctx,
                                out,
                                ev.offset,
                                NoteKind::NoteExpr {
                                    key: routed,
                                    kind,
                                    value,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
