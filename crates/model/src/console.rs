//! Console (main mixer) data and commands.
//!
//! Channel strips and the master section are ordinary rack devices (`atomik.channel`,
//! `atomik.master`) whose params are normal device params (undo, automation work as usual).
//! The console section records which devices form the console, in what order, and which
//! effect device sits on each FX return. Cables between instrument → channel → master use the
//! normal cable list; master bus inputs are `Direction::SummingInput` sockets.
//!
//! **Invariants** (kept by every command and loader): every console channel is an
//! existing `atomik.channel` device, listed once; a channel's source, when set, is an
//! existing device that is not a console device (a source may feed several channels, see
//! `ConsoleCommand::AddChannelFor`; `AddChannel` gives it one); the master,
//! when set, is an existing `atomik.master` device; returns are only set while there is a
//! master, each on an existing non-console device, no device on two slots. Channel and
//! master devices outside the console (e.g. from a patch file) are allowed: they are plain
//! rack devices.
//!
//! **Order:** the console order is the strip order in the console view. The engine sums a
//! summing input's cables by source endpoint (device id, socket), not by console order, so
//! moving a channel changes no audio (summing is deterministic either way).

use crate::{Cable, DeviceKey, RackDoc, SocketRef};

/// Number of FX sends/returns.
pub const FX_SLOTS: usize = 8;

/// Most effects in one insert chain (a channel's or the master's).
pub const INSERT_SLOTS: usize = 4;

/// Type id of a channel strip device.
pub const CHANNEL_TYPE: &str = "atomik.channel";
/// Type id of the master section device.
pub const MASTER_TYPE: &str = "atomik.master";
/// The channel param the app sets from [`crate::RackDoc::solo_silence`]; never stored.
pub const SOLO_SILENCE_PARAM: &str = "internal.solo_silence";
/// Params with this key prefix are set by the app, never by the user: they are not stored in
/// documents, not saved, not undoable and cannot be automated.
pub const INTERNAL_PARAM_PREFIX: &str = "internal.";

/// True for `internal.*` param keys.
pub fn is_internal_param(param: &str) -> bool {
    param.starts_with(INTERNAL_PARAM_PREFIX)
}

/// True for the console device types (`atomik.channel`, `atomik.master`).
pub fn is_console_type(type_id: &str) -> bool {
    type_id == CHANNEL_TYPE || type_id == MASTER_TYPE
}

/// One console channel strip.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleChannel {
    /// The `atomik.channel` device key.
    pub channel: DeviceKey,
    /// The instrument (or any device) feeding it, if any.
    pub source: Option<DeviceKey>,
}

/// The console section of a document. The default is empty with compensation on.
#[derive(Debug, Clone, PartialEq)]
pub struct Console {
    /// Channel strips in console order (left to right; also the bus summing order).
    pub channels: Vec<ConsoleChannel>,
    /// The `atomik.master` device key (created with the first channel).
    pub master: Option<DeviceKey>,
    /// Effect device on each FX return slot (send k → effect → return k).
    pub returns: [Option<DeviceKey>; FX_SLOTS],
    /// Delay compensation enabled (global switch).
    pub compensation: bool,
}

impl Default for Console {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            master: None,
            returns: Default::default(),
            compensation: true,
        }
    }
}

impl Console {
    /// Index of channel device `channel` in console order.
    pub fn channel_index(&self, channel: &DeviceKey) -> Option<usize> {
        self.channels.iter().position(|c| &c.channel == channel)
    }

    /// The first channel fed by `source`, if any (the one `AddChannel` made, in console order).
    pub fn channel_of(&self, source: &DeviceKey) -> Option<&ConsoleChannel> {
        self.channels
            .iter()
            .find(|c| c.source.as_ref() == Some(source))
    }

    /// The return slot holding `effect`, if any.
    pub fn return_slot(&self, effect: &DeviceKey) -> Option<usize> {
        self.returns.iter().position(|r| r.as_ref() == Some(effect))
    }
}

/// Socket keys of the console devices.
pub(crate) mod sockets {
    use super::FX_SLOTS;

    /// `send{k}_{side}` on a channel, k = 1..=8.
    pub(crate) fn channel_send(k: usize, side: &str) -> String {
        format!("send{k}_{side}")
    }

    /// `send_bus{k}_{side}` on the master.
    pub(crate) fn master_send_bus(k: usize, side: &str) -> String {
        format!("send_bus{k}_{side}")
    }

    /// `fx_send{k}_{side}` on the master.
    pub(crate) fn master_fx_send(k: usize, side: &str) -> String {
        format!("fx_send{k}_{side}")
    }

    /// `return{k}_{side}` on the master.
    pub(crate) fn master_return(k: usize, side: &str) -> String {
        format!("return{k}_{side}")
    }

    /// The send numbers 1..=8.
    pub(crate) fn sends() -> impl Iterator<Item = usize> {
        1..=FX_SLOTS
    }
}

/// A console link: one of the cables `ConsoleCommand::AddChannel` makes between a channel and the
/// master. The rack hides these by default and shows "Output: Master" on the
/// channel instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `out_l -> bus_l` or `out_r -> bus_r`.
    ChannelOut,
    /// `send{k}_{l,r} -> send_bus{k}_{l,r}`, `k` in 1..=8.
    ChannelSend(u8),
}

/// The number in `text` when it is canonical decimal (no sign, no leading zero) and in `1..=8`.
fn send_number(text: &str) -> Option<usize> {
    let n: usize = text.parse().ok()?;
    ((1..=FX_SLOTS).contains(&n) && n.to_string() == text).then_some(n)
}

/// `Some` when `cable` is a console link, and which: its source is a console channel (listed in
/// `console.channels` or any device of type [`CHANNEL_TYPE`]), its destination is the document's
/// master, and the sockets are exactly `(out_l, bus_l)`, `(out_r, bus_r)` or
/// `(send{k}_{side}, send_bus{k}_{side})` for the same `k` in 1..=8 and the same side. Every
/// other cable, including everything of a document without a master, is `None`: ordinary, and
/// always drawn. Pure and deterministic.
pub fn link_kind(doc: &RackDoc, cable: &Cable) -> Option<LinkKind> {
    let master = doc.console.master.as_ref()?;
    if &cable.to.device != master {
        return None;
    }
    if !is_channel(doc, &cable.from.device) {
        return None;
    }
    let (from, to) = (cable.from.socket.as_str(), cable.to.socket.as_str());
    match (from, to) {
        ("out_l", "bus_l") | ("out_r", "bus_r") => return Some(LinkKind::ChannelOut),
        _ => {}
    }
    for side in ["l", "r"] {
        let number = from
            .strip_prefix("send")
            .and_then(|r| r.strip_suffix(&format!("_{side}")))
            .and_then(send_number);
        if let Some(k) = number {
            return (to == sockets::master_send_bus(k, side))
                .then_some(LinkKind::ChannelSend(k as u8));
        }
    }
    None
}

/// True for a console channel: listed in the console, or any device of type [`CHANNEL_TYPE`].
fn is_channel(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.console.channel_index(key).is_some()
        || doc.device(key).is_some_and(|d| d.type_id == CHANNEL_TYPE)
}

/// Where a channel's output goes, for the "Output:" display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDest {
    /// Every output cable is a console link to the master (the normal case).
    Master,
    /// The output cables leave the master: their destinations, in cable order.
    Direct(Vec<SocketRef>),
    /// Some output cables go to the master and some elsewhere.
    Mixed,
    /// The channel has no output cable (or is no channel).
    None,
}

/// Reads the cables out of `out_l` and `out_r` of console channel `channel`: all of them console
/// links is [`OutputDest::Master`], none is [`OutputDest::Direct`] with the destinations, both
/// kinds is [`OutputDest::Mixed`], no cable (or an unknown or non-channel device) is
/// [`OutputDest::None`]. The sends play no part. Pure and deterministic.
pub fn output_dest(doc: &RackDoc, channel: &DeviceKey) -> OutputDest {
    if !is_channel(doc, channel) {
        return OutputDest::None;
    }
    let outs: Vec<&Cable> = doc
        .cables
        .iter()
        .filter(|c| {
            &c.from.device == channel && matches!(c.from.socket.as_str(), "out_l" | "out_r")
        })
        .collect();
    let links = outs.iter().filter(|c| link_kind(doc, c).is_some()).count();
    if outs.is_empty() {
        OutputDest::None
    } else if links == outs.len() {
        OutputDest::Master
    } else if links == 0 {
        OutputDest::Direct(outs.iter().map(|c| c.to.clone()).collect())
    } else {
        OutputDest::Mixed
    }
}

/// What goes into an insert chain slot (`ConsoleCommand::SetInsert`).
#[derive(Debug, Clone, PartialEq)]
pub enum InsertEffect {
    /// Add a new effect device of this type, placed in the rack right after the previous
    /// chain member (or the channel).
    New {
        /// Registry type id; must be an effect with stereo `in_l/in_r/out_l/out_r`.
        type_id: String,
    },
    /// Use a device that already exists in the rack.
    Existing(DeviceKey),
}

/// Console edits (wrapped in `Command::Console`). Every command is undoable.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleCommand {
    /// Create a channel for `source` (auto-wires source outs → channel ins → master bus).
    AddChannel {
        /// Device feeding the channel.
        source: DeviceKey,
    },
    /// Create a channel for chosen outputs of `source` (a drum machine's individual pad
    /// outputs), in one undo step. Wired as `AddChannel` wires a channel (master created
    /// when missing, channel after the source in the rack, `out_l`/`out_r` of the channel
    /// to the master bus and sends), except that `source:out_l` feeds `in_l` and
    /// `source:out_r` feeds `in_r`, and the source may already feed other channels (the
    /// drum machine feeds its main channel and one per pad). Rejected when the source is
    /// unknown, a console device or a return effect, when `out_l`/`out_r` is not an Audio
    /// output of the source, or when either output is already cabled (nothing is
    /// re-cabled). The channel device is named `name`, or `"<source name> <socket stem>"`
    /// (e.g. `"Drums Pad 2"` for `pad2_out_l`). The channel's `source` is `source`, so removing
    /// an instrument removes all its channels. No persisted data beyond the channel entry:
    /// the project format is unchanged.
    AddChannelFor {
        /// Device feeding the channel.
        source: DeviceKey,
        /// Source output for the channel's left input.
        out_l: String,
        /// Source output for the channel's right input.
        out_r: String,
        /// Channel name, or `None` for the default.
        name: Option<String>,
    },
    /// Remove a channel strip (and its device and cables).
    RemoveChannel {
        /// Channel device key.
        channel: DeviceKey,
    },
    /// Move a channel to console position `to` (final index after removal).
    MoveChannel {
        /// Channel device key.
        channel: DeviceKey,
        /// New position.
        to: usize,
    },
    /// Put an effect device on FX return slot `slot` (wires master send k out → effect in →
    /// effect out → master return k), or clear it. The effect's previous cables into
    /// `in_l/in_r` and out of `out_l/out_r` are removed (it leaves any insert chain). Rejected
    /// without a master, for a console device, an effect already on another slot, or a
    /// channel source.
    SetReturn {
        /// Slot 0..FX_SLOTS.
        slot: usize,
        /// Effect device, or `None` to clear.
        effect: Option<DeviceKey>,
    },
    /// Set slot `slot` of the insert chain of `channel` (a console channel or the master).
    ///
    /// The **chain is the cable path** `insert_send → fx1 → fx2 → … → insert_return` (an
    /// effect follows when its `in_l/in_r` are fed by the previous stage's `out_l/out_r`); it
    /// is derived from cables ([`crate::insert_chain`]), nothing extra is persisted (project
    /// format unchanged). `slot` is `0..INSERT_SLOTS`; `slot == chain length` appends, a
    /// lower slot **replaces** that effect. `None` removes the slot and closes the chain. A
    /// removed or replaced effect loses its chain cables but **stays in the rack**. The whole
    /// chain is re-cabled in order in one undo step. Rejected (nothing changes) for a slot
    /// past the end or beyond 4, a device that is not a console channel or the master, an
    /// effect that is not a stereo effect, a console device, a return effect, a channel
    /// source, already in this or another chain (use `MoveInsert` within a chain), or when
    /// a cable would be rejected (cycle, insert return already fed by something else).
    /// Bypass is the device's own `insert.bypass` param (the return then plays the send).
    SetInsert {
        /// Console channel or master device key.
        channel: DeviceKey,
        /// Slot 0..INSERT_SLOTS.
        slot: usize,
        /// The effect, or `None` to remove the slot.
        effect: Option<InsertEffect>,
    },
    /// Move insert `from` to position `to` of `channel`'s chain and re-cable it in order.
    MoveInsert {
        /// Console channel or master device key.
        channel: DeviceKey,
        /// Current slot.
        from: usize,
        /// New slot (final index).
        to: usize,
    },
    /// Enable/disable delay compensation.
    SetCompensation(bool),
    /// Clear every channel solo.
    ClearSolos,
    /// Clear every channel mute.
    ClearMutes,
}
