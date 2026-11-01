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
//! **Buses.** A bus (`atomik.bus`, [`ConsoleBus`]) is a console strip with the channel's DSP and
//! a summing input `bus_l`/`bus_r`; channels and other buses route to it with their `out_l`/`out_r`
//! cables, and its own `out_l`/`out_r` go to the master, another bus or an Audio I/O stereo pair.
//! Every bus is an existing `atomik.bus` device, listed once, with a unique non-empty trimmed
//! name; buses exist only while there is a master. The routing is cables (hidden console links,
//! see [`link_kind`]); the bus commands live in `console_bus.rs`.
//!
//! **VCA groups.** [`VcaGroup`] (`Console::vcas`, in `console_vca.rs`) is a master fader, mute and
//! solo over a set of strips (channels and buses): console state only, no device, no cables, no
//! audio. A strip is in at most one group; a removed strip leaves its group in the same undo step.
//!
//! **Order:** the console order is the strip order in the console view. The engine sums a
//! summing input's cables by source endpoint (device id, socket), not by console order, so
//! moving a channel changes no audio (summing is deterministic either way).

pub use crate::console_bus::{OutputAvail, OutputOption, output_options};
use crate::console_settings::ChannelSettingsClip;
pub use crate::console_snapshot::{
    CompareState, MAX_SNAPSHOT_NAME_CHARS, MAX_SNAPSHOTS, MixerSnapshot, MixerState, SnapshotScope,
    SnapshotVca,
};
pub use crate::console_vca::{
    MAX_VCA_NAME_CHARS, MAX_VCAS, VCA_PALETTE, VcaFeed, VcaGroup, clamp_vca_db,
};
use crate::{Cable, DeviceKey, INTERFACE_KEY, RackDoc, SocketRef};

/// The key the next bus device gets (`bus`, `bus-2`, ...): what `CreateBus` and `RouteToNewBus`
/// will create in `doc`, so a caller can name the new bus before it exists.
pub fn new_bus_key(doc: &RackDoc) -> DeviceKey {
    crate::rules::generate_key(doc, BUS_TYPE)
}

/// Number of FX sends/returns.
pub const FX_SLOTS: usize = 8;

/// Most effects in one insert chain (a channel's or the master's).
pub const INSERT_SLOTS: usize = 4;

/// Type id of a channel strip device.
pub const CHANNEL_TYPE: &str = "atomik.channel";
/// Type id of the master section device.
pub const MASTER_TYPE: &str = "atomik.master";
/// Type id of an output bus strip (created by the bus commands only).
pub const BUS_TYPE: &str = "atomik.bus";
/// Longest bus name, in characters.
pub const MAX_BUS_NAME_CHARS: usize = 32;
/// The channel param the app sets from [`crate::RackDoc::solo_silence`]; never stored.
pub const SOLO_SILENCE_PARAM: &str = "internal.solo_silence";
/// The channel params the app sets from [`crate::RackDoc::vca_feeds`]; never stored.
pub const VCA_DB_PARAM: &str = "internal.vca_db";
/// See [`VCA_DB_PARAM`].
pub const VCA_MUTE_PARAM: &str = "internal.vca_mute";
/// See [`VCA_DB_PARAM`].
pub const VCA_SOLO_PARAM: &str = "internal.vca_solo";
/// The channel and master param the app sets to 1 while a window analyses the strip: the device
/// then writes its analyser tap (nothing otherwise); never stored.
pub const TAP_LIVE_PARAM: &str = "internal.tap_live";
/// Params with this key prefix are set by the app, never by the user: they are not stored in
/// documents, not saved, not undoable and cannot be automated.
pub const INTERNAL_PARAM_PREFIX: &str = "internal.";

/// True for `internal.*` param keys.
pub fn is_internal_param(param: &str) -> bool {
    param.starts_with(INTERNAL_PARAM_PREFIX)
}

/// True for the console device types (`atomik.channel`, `atomik.bus`, `atomik.master`).
pub fn is_console_type(type_id: &str) -> bool {
    type_id == CHANNEL_TYPE || type_id == MASTER_TYPE || type_id == BUS_TYPE
}

/// One console channel strip.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleChannel {
    /// The `atomik.channel` device key.
    pub channel: DeviceKey,
    /// The instrument (or any device) feeding it, if any.
    pub source: Option<DeviceKey>,
}

/// One output bus strip.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleBus {
    /// The `atomik.bus` device key.
    pub bus: DeviceKey,
    /// The bus name: trimmed, non-empty, at most [`MAX_BUS_NAME_CHARS`] characters, unique among
    /// the buses (compared ignoring case). The bus device carries the same name.
    pub name: String,
}

/// The console section of a document. The default is empty with compensation on.
#[derive(Debug, Clone, PartialEq)]
pub struct Console {
    /// Channel strips in console order (left to right; also the bus summing order).
    pub channels: Vec<ConsoleChannel>,
    /// Output buses in console order (shown between the channels and the master).
    pub buses: Vec<ConsoleBus>,
    /// VCA groups in strip order (see [`crate::console_vca`]); none in a plain project.
    pub vcas: Vec<VcaGroup>,
    /// Mixer snapshots in order (see [`crate::console_snapshot`]); none in a plain project.
    pub snapshots: Vec<MixerSnapshot>,
    /// The A/B comparison in progress, if any. Not saved.
    pub compare: Option<CompareState>,
    /// The `atomik.master` device key (created with the first channel).
    pub master: Option<DeviceKey>,
    /// Effect device on each FX return slot (send k → effect → return k).
    pub returns: [Option<DeviceKey>; FX_SLOTS],
    /// Delay compensation enabled (global switch).
    pub compensation: bool,
    /// FX returns and sidechain keys are delay-compensated like the summing inputs (see the
    /// engine's console docs). Off in a project saved before it existed, on in a new one.
    pub align_returns: bool,
    /// The sends of a strip that plays into a bus follow the bus: a post-fader send is scaled by
    /// the bus fader and mute (and by the buses above it), see [`LinkKind::FollowLink`]. Off in
    /// a project saved before it existed, on in a new one.
    pub send_follow: bool,
    /// Parallel channels as `(parent strip, child channel)` pairs, in creation order: the child
    /// is a source-less channel fed from the parent's parallel output. Names follow the parent
    /// and the child goes with it (see `ConsoleCommand::CreateParallel`).
    pub parallels: Vec<(DeviceKey, DeviceKey)>,
}

impl Default for Console {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            buses: Vec::new(),
            vcas: Vec::new(),
            snapshots: Vec::new(),
            compare: None,
            master: None,
            returns: Default::default(),
            compensation: true,
            align_returns: false,
            send_follow: false,
            parallels: Vec::new(),
        }
    }
}

impl Console {
    /// Index of channel device `channel` in console order.
    pub fn channel_index(&self, channel: &DeviceKey) -> Option<usize> {
        self.channels.iter().position(|c| &c.channel == channel)
    }

    /// Index of bus device `bus` in console order.
    pub fn bus_index(&self, bus: &DeviceKey) -> Option<usize> {
        self.buses.iter().position(|b| &b.bus == bus)
    }

    /// The bus called `name` (trimmed, ignoring case), if any.
    pub fn bus_by_name(&self, name: &str) -> Option<&ConsoleBus> {
        let wanted = name.trim().to_lowercase();
        self.buses
            .iter()
            .find(|b| b.name.trim().to_lowercase() == wanted)
    }

    /// The first of `Bus 1`, `Bus 2`, … that no bus is called: the name the UI proposes.
    pub fn default_bus_name(&self) -> String {
        (1usize..)
            .map(|n| format!("Bus {n}"))
            .find(|n| self.bus_by_name(n).is_none())
            .expect("a free name exists")
    }

    /// The first channel fed by `source`, if any (the one `AddChannel` made, in console order).
    pub fn channel_of(&self, source: &DeviceKey) -> Option<&ConsoleChannel> {
        self.channels
            .iter()
            .find(|c| c.source.as_ref() == Some(source))
    }

    /// The parallel channels hanging off `parent`, in creation order.
    pub fn parallels_of<'a>(
        &'a self,
        parent: &'a DeviceKey,
    ) -> impl Iterator<Item = &'a DeviceKey> {
        self.parallels
            .iter()
            .filter(move |(p, _)| p == parent)
            .map(|(_, c)| c)
    }

    /// The strip `child` is a parallel channel of, if it is one.
    pub fn parallel_parent(&self, child: &DeviceKey) -> Option<&DeviceKey> {
        self.parallels
            .iter()
            .find(|(_, c)| c == child)
            .map(|(p, _)| p)
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

/// A console link: one of the cables the console commands make between a strip (channel or bus)
/// and the master or a bus. The rack hides these by default and shows "Output: ..." on the strip
/// instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Channel `out_l -> bus_l` or `out_r -> bus_r` of the master.
    ChannelOut,
    /// Channel `send{k}_{l,r} -> send_bus{k}_{l,r}` of the master, `k` in 1..=8.
    ChannelSend(u8),
    /// Channel `out_l -> bus_l` or `out_r -> bus_r` of an output bus.
    ChannelToBus,
    /// Bus `out_l -> bus_l` or `out_r -> bus_r` of the master.
    BusOut,
    /// Bus `out_l -> bus_l` or `out_r -> bus_r` of another bus.
    BusToBus,
    /// Bus `send{k}_{l,r} -> send_bus{k}_{l,r}` of the master, `k` in 1..=8.
    BusSend(u8),
    /// Bus `follow_out -> follow_in` of a strip that plays into it (a channel or another bus):
    /// the bus gain the strip's post-fader sends follow (see [`Console::send_follow`]).
    FollowLink,
}

/// Socket key of the bus gain a strip's sends follow (a bus output, a strip input).
pub const FOLLOW_OUT: &str = "follow_out";
/// See [`FOLLOW_OUT`].
pub const FOLLOW_IN: &str = "follow_in";
/// Socket keys of a strip's parallel output (the signal before the fader, before the pan).
pub const PARALLEL_OUT: [&str; 2] = ["parallel_out_l", "parallel_out_r"];

/// The number in `text` when it is canonical decimal (no sign, no leading zero) and in `1..=8`.
fn send_number(text: &str) -> Option<usize> {
    let n: usize = text.parse().ok()?;
    ((1..=FX_SLOTS).contains(&n) && n.to_string() == text).then_some(n)
}

/// `Some` when `cable` is a console link, and which: its source is a console strip (a channel,
/// listed in `console.channels` or any device of type [`CHANNEL_TYPE`], or a bus, listed in
/// `console.buses` or any device of type [`BUS_TYPE`]), its destination is the document's master
/// or a bus, and the sockets are exactly `(out_l, bus_l)` or `(out_r, bus_r)` (to the master or
/// a bus other than the source) or, to the master only,
/// `(send{k}_{side}, send_bus{k}_{side})` for the same `k` in 1..=8 and the same side. Every
/// other cable, including a cable to an Audio I/O output and everything of a document without a
/// master, is `None`: ordinary, and always drawn. Pure and deterministic.
pub fn link_kind(doc: &RackDoc, cable: &Cable) -> Option<LinkKind> {
    let from_channel = is_channel(doc, &cable.from.device);
    if !from_channel && !is_bus(doc, &cable.from.device) {
        return None;
    }
    if cable.from.socket == FOLLOW_OUT {
        let valid = !from_channel
            && cable.to.socket == FOLLOW_IN
            && cable.to.device != cable.from.device
            && is_strip(doc, &cable.to.device);
        return valid.then_some(LinkKind::FollowLink);
    }
    let to_master = doc.console.master.as_ref() == Some(&cable.to.device);
    if !to_master && (cable.to.device == cable.from.device || !is_bus(doc, &cable.to.device)) {
        return None;
    }
    let (from, to) = (cable.from.socket.as_str(), cable.to.socket.as_str());
    if matches!((from, to), ("out_l", "bus_l") | ("out_r", "bus_r")) {
        return Some(match (from_channel, to_master) {
            (true, true) => LinkKind::ChannelOut,
            (true, false) => LinkKind::ChannelToBus,
            (false, true) => LinkKind::BusOut,
            (false, false) => LinkKind::BusToBus,
        });
    }
    if !to_master {
        return None;
    }
    for side in ["l", "r"] {
        let number = from
            .strip_prefix("send")
            .and_then(|r| r.strip_suffix(&format!("_{side}")))
            .and_then(send_number);
        if let Some(k) = number {
            return (to == sockets::master_send_bus(k, side)).then_some(if from_channel {
                LinkKind::ChannelSend(k as u8)
            } else {
                LinkKind::BusSend(k as u8)
            });
        }
    }
    None
}

/// True for a console channel: listed in the console, or any device of type [`CHANNEL_TYPE`].
fn is_channel(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.console.channel_index(key).is_some()
        || doc.device(key).is_some_and(|d| d.type_id == CHANNEL_TYPE)
}

/// True for a console bus: listed in the console, or any device of type [`BUS_TYPE`].
pub(crate) fn is_bus(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.console.bus_index(key).is_some() || doc.device(key).is_some_and(|d| d.type_id == BUS_TYPE)
}

/// True for a console strip: a channel or a bus.
pub(crate) fn is_strip(doc: &RackDoc, key: &DeviceKey) -> bool {
    is_channel(doc, key) || is_bus(doc, key)
}

/// Where a strip's output goes, for the "Output:" display and `ConsoleCommand::SetOutput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDest {
    /// Every output cable is a console link to the master (the normal case).
    Master,
    /// Every output cable is a console link to this output bus.
    Bus(DeviceKey),
    /// `out_l` and `out_r` go to the interface outputs `out_{2n-1}` and `out_{2n}`: the stereo
    /// pair `n` (1-based: 1 is outputs 1+2, 2 is 3+4, ...). Not a console link (the cables are
    /// drawn like any other), but a first-class destination: no warning.
    AudioIo(u16),
    /// The output cables go elsewhere: their destinations, in cable order.
    Direct(Vec<SocketRef>),
    /// Some output cables are console links and some are not, or they go to different places.
    Mixed,
    /// The strip has no output cable (or is no strip).
    None,
}

/// The Audio I/O pair `n` when `outs` are exactly `out_l -> interface:out_{2n-1}` and
/// `out_r -> interface:out_{2n}`.
fn audio_io_pair(outs: &[&Cable]) -> Option<u16> {
    if outs.len() != 2 {
        return None;
    }
    let target = |socket: &str| -> Option<u16> {
        let c = outs.iter().find(|c| c.from.socket == socket)?;
        (c.to.device.0 == INTERFACE_KEY)
            .then(|| c.to.socket.strip_prefix("out_")?.parse::<u16>().ok())?
    };
    let (l, r) = (target("out_l")?, target("out_r")?);
    (l % 2 == 1 && r == l + 1).then_some(l.div_ceil(2))
}

/// Reads the cables out of `out_l` and `out_r` of console strip `strip` (a channel or a bus): all
/// of them console links to the master is [`OutputDest::Master`], all of them links to one bus is
/// [`OutputDest::Bus`], exactly `out_l`/`out_r` to one Audio I/O stereo pair is
/// [`OutputDest::AudioIo`], other cables only is [`OutputDest::Direct`] with the destinations,
/// any other mix is [`OutputDest::Mixed`], no cable (or an unknown or non-strip device) is
/// [`OutputDest::None`]. The sends play no part. Pure and deterministic.
pub fn output_dest(doc: &RackDoc, strip: &DeviceKey) -> OutputDest {
    if !is_strip(doc, strip) {
        return OutputDest::None;
    }
    let outs: Vec<&Cable> = doc
        .cables
        .iter()
        .filter(|c| &c.from.device == strip && matches!(c.from.socket.as_str(), "out_l" | "out_r"))
        .collect();
    let links: Vec<Option<LinkKind>> = outs.iter().map(|c| link_kind(doc, c)).collect();
    let linked = links.iter().filter(|l| l.is_some()).count();
    if outs.is_empty() {
        OutputDest::None
    } else if linked == outs.len() {
        let to_master = |c: &&Cable| doc.console.master.as_ref() == Some(&c.to.device);
        if outs.iter().all(to_master) {
            OutputDest::Master
        } else if outs.iter().all(|c| c.to.device == outs[0].to.device) && !to_master(&outs[0]) {
            OutputDest::Bus(outs[0].to.device.clone())
        } else {
            OutputDest::Mixed
        }
    } else if linked == 0 {
        match audio_io_pair(&outs) {
            Some(n) => OutputDest::AudioIo(n),
            None => OutputDest::Direct(outs.iter().map(|c| c.to.clone()).collect()),
        }
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
    /// Create a Mix Channel: an `atomik.channel` with no source, wired to the master and its
    /// sends like any channel (the master is created when missing), named `name` (the default
    /// name when empty) and placed after the last channel in the rack. Its `in_l`/`in_r` are the
    /// Direct In: cable anything there. One undo step.
    AddMixChannel {
        /// The channel name.
        name: String,
    },
    /// Create a parallel channel of `strip` (a console channel or bus): a source-less channel fed
    /// from the strip's `parallel_out_l/r`, named `P<n>: <strip name>`, routed where the strip is
    /// routed. It is recorded in [`Console::parallels`]: the name follows the strip's renames,
    /// and removing the strip removes the copy. A parallel channel can have parallel channels
    /// of its own. One undo step.
    CreateParallel {
        /// Channel or bus device key.
        strip: DeviceKey,
    },
    /// Switch the sends of strips that play into a bus following the bus fader and mute on or off
    /// (see [`Console::send_follow`]); the follow links are made or removed in the same step.
    SetSendFollow(bool),
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
    /// Paste a copied strip (see [`crate::RackDoc::copy_channel_settings`]) onto `strips`
    /// (channels and buses; one undo step): the params of the clip's sections are set to the
    /// clip's values (a param the clip does not hold goes to its default), a param the target
    /// does not have is skipped, and when the clip holds the inserts the target's chain is
    /// replaced by new effects with the clip's params. The routing, the name, the sources and
    /// the VCA membership stay. Rejected for an empty `strips`, a device that is no channel or
    /// bus, or a strip whose inserts are frozen.
    PasteSettings {
        /// The strips that take the settings.
        strips: Vec<DeviceKey>,
        /// The copied settings.
        clip: ChannelSettingsClip,
    },
    /// Put a channel or bus back to a fresh strip: every param at its default, the insert chain
    /// cleared (the effects stay in the rack). With `keep_routing` false the output goes to the
    /// master too; the VCA membership and the name stay either way.
    ResetStrip {
        /// Channel or bus device key.
        strip: DeviceKey,
        /// Whether the output routing stays as it is.
        keep_routing: bool,
    },
    /// Empty the insert chain of a channel or bus (the effects stay in the rack, uncabled).
    ClearInserts {
        /// Channel or bus device key.
        strip: DeviceKey,
    },
    /// Enable/disable delay compensation.
    SetCompensation(bool),
    /// Align FX returns and sidechain keys with the dry path (see [`Console::align_returns`]).
    SetAlignReturns(bool),
    /// Clear every channel solo.
    ClearSolos,
    /// Clear every channel mute.
    ClearMutes,
    /// Create an output bus called `name` (see [`ConsoleBus::name`]; empty, too long or taken is
    /// rejected), in one undo step: the `atomik.bus` device (key [`crate::console::new_bus_key`]
    /// computed before the command; placed after the last bus, else at the end of the rack), its
    /// sends cabled to the master like a channel's, its output routed to `dest` (Master, another
    /// bus or a free Audio I/O pair, see [`ConsoleCommand::SetOutput`]), the console entry at
    /// the end of the bus list. The master is created when missing, as the first channel does.
    CreateBus {
        /// The bus name.
        name: String,
        /// Where the bus plays: `Master`, `Bus(key)` or `AudioIo(pair)`.
        dest: OutputDest,
    },
    /// Rename a bus (and its device). The name is trimmed; empty, over
    /// [`MAX_BUS_NAME_CHARS`] or already taken by another bus (ignoring case) is rejected.
    RenameBus {
        /// Bus device key.
        bus: DeviceKey,
        /// The new name.
        name: String,
    },
    /// Delete a bus: its feeders (channels and buses routed to it) move to the bus's own
    /// destination (the master, another bus, or its Audio I/O pair when it has exactly one feeder
    /// since a pair takes one cable; the master otherwise, also when the destination is
    /// hand-cabled), then the bus device and its cables go. Its insert effects stay in the rack.
    /// Deleting the bus device with `Command::RemoveDevice` or the workflow removal does the same.
    DeleteBus {
        /// Bus device key.
        bus: DeviceKey,
    },
    /// Move a bus to console position `to` among the buses (final index after removal). Changes
    /// no audio.
    ReorderBus {
        /// Bus device key.
        bus: DeviceKey,
        /// New position.
        to: usize,
    },
    /// Route a strip (a console channel or a bus) to `dest`: `Master`, `Bus(key)` or
    /// `AudioIo(pair)`, rewiring the strip's `out_l`/`out_r` cables (every cable leaving them is
    /// replaced) in one undo step; the sends are untouched. Rejected: a strip that is not in the
    /// console, a bus that is no bus, a bus destination that would make a cycle (the strip itself
    /// or a bus that the strip feeds, directly or not), an Audio I/O pair beyond
    /// `output_channels` or already fed by another cable (an interface output takes one cable),
    /// and `Direct`, `Mixed` and `None`. A strip already routed there is a no-op step.
    SetOutput {
        /// Channel or bus device key.
        strip: DeviceKey,
        /// The destination.
        dest: OutputDest,
    },
    /// Create a bus called `name` with its output on the master and route `strips` (channels and
    /// buses) to it, in one undo step (Ctrl/Cmd+G). `strips` must be non-empty and valid.
    RouteToNewBus {
        /// Strips to route to the new bus.
        strips: Vec<DeviceKey>,
        /// The bus name.
        name: String,
    },
    /// Create a VCA group called `name` over `members` (console channels and buses; a member
    /// leaves the group it was in), in one undo step. The name is trimmed; empty, over
    /// [`MAX_VCA_NAME_CHARS`] or taken (ignoring case) is rejected, so is a member that is not
    /// a console strip. The group starts at 0 dB, unmuted, unsoloed, with the colour [`VCA_PALETTE`] gives its id; its id is
    /// [`Console::next_vca_id`] computed before the command.
    CreateVca {
        /// The group name.
        name: String,
        /// The member strips (may be empty).
        members: Vec<DeviceKey>,
    },
    /// Rename a VCA group (the same rules as `CreateVca`).
    RenameVca {
        /// The group id.
        vca: u32,
        /// The new name.
        name: String,
    },
    /// Set (or clear) the colour of a VCA group.
    SetVcaColor {
        /// The group id.
        vca: u32,
        /// The colour, or none.
        color: Option<[u8; 3]>,
    },
    /// Set the offset of a VCA group in dB (clamped to `VCA_MIN_DB..=VCA_MAX_DB`; the bottom is
    /// off). Commands with the same `merge` id on one group are one undo step (a fader drag).
    SetVcaLevel {
        /// The group id.
        vca: u32,
        /// The offset in dB.
        db: f32,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Mute or unmute a VCA group (every member counts as muted).
    SetVcaMute {
        /// The group id.
        vca: u32,
        /// The new state.
        on: bool,
    },
    /// Solo or unsolo a VCA group (every member counts as soloed).
    SetVcaSolo {
        /// The group id.
        vca: u32,
        /// The new state.
        on: bool,
    },
    /// Put `strips` in the group `vca` (each leaves the group it was in), or take them out of
    /// any group with `None`, in one undo step. Rejected: a strip that is not a console channel
    /// or bus, an unknown group, and a command that changes nothing.
    AssignToVca {
        /// The group, or `None` to remove the strips from their groups.
        vca: Option<u32>,
        /// The strips.
        strips: Vec<DeviceKey>,
    },
    /// Delete a VCA group; its members keep their faders and play at their own level again.
    DeleteVca {
        /// The group id.
        vca: u32,
    },
    /// Store the mixer as a new snapshot called `name` (trimmed; empty, over
    /// [`MAX_SNAPSHOT_NAME_CHARS`] or taken ignoring case is rejected; at most [`MAX_SNAPSHOTS`]),
    /// its id [`Console::next_snapshot_id`] computed before the command. Changes no audio.
    StoreSnapshot {
        /// The snapshot name.
        name: String,
    },
    /// Overwrite snapshot `id` with the mixer as it is now. Rejected when it already holds it.
    UpdateSnapshot {
        /// The snapshot id.
        id: u32,
    },
    /// Apply snapshot `id` to the mixer in one undo step: what `scope` covers is set through
    /// ordinary param edits, so the engine's smoothers apply it without clicks. Ends a compare
    /// in progress (the recalled mix stays). Rejected when nothing would change and no compare
    /// ends.
    RecallSnapshot {
        /// The snapshot id.
        id: u32,
        /// The categories to recall ([`SnapshotScope::ALL`] for everything).
        scope: SnapshotScope,
    },
    /// Rename a snapshot (the rules of `StoreSnapshot`).
    RenameSnapshot {
        /// The snapshot id.
        id: u32,
        /// The new name.
        name: String,
    },
    /// Delete a snapshot; a compare with it ends first.
    DeleteSnapshot {
        /// The snapshot id.
        id: u32,
    },
    /// Move a snapshot to position `to` in the list (final index). Rejected when it does not
    /// move.
    MoveSnapshot {
        /// The snapshot id.
        id: u32,
        /// The new position.
        to: usize,
    },
    /// Compare with snapshot `id`: with no compare in progress the working mix goes to the hidden
    /// slot and the snapshot is recalled; while comparing with `id` the live mix and the slot
    /// swap (the A/B flip); while comparing with another snapshot that snapshot takes the place
    /// of the compared one (the working side stays as it is).
    Compare {
        /// The snapshot id.
        id: u32,
    },
    /// End the compare and keep the mix that is live (a snapshot's or the working one).
    EndCompare,
}
