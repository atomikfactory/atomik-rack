//! Reversible edits. Every applied command becomes one [`Edit`] holding everything needed to
//! perform it again or invert it; undo performs the inverse.

use crate::WriteMode;
use crate::console::{CompareState, ConsoleBus, ConsoleChannel, MixerSnapshot, VcaGroup};
use crate::container::{ContainerDoc, MacroDoc, NoteSplit, collect_device_assets, join};
use crate::curves::CurveDoc;
use crate::drums::DrumDoc;
use crate::freeze::Frozen;
use crate::groove::Groove;
use crate::macros::MacroSet;
use crate::midi_map::MidiMapping;
use crate::organise::TrackLayout;
use crate::plugin_cmd::RelinkData;
use crate::song::{Clip, ClipContent, Lane, Marker, MeterEvent, RangeMarker, TempoEvent, Track};
use crate::steps::StepSeqDoc;
use crate::takes::TakeGroup;
use crate::{Cable, DeviceKey, RackDevice, RackDoc, SyncOp, ZoneDoc};
use std::collections::{BTreeMap, BTreeSet};

/// One reversible, already-validated change to a [`RackDoc`].
// An edit lives in the undo history, never on a hot path: a clip edit holds two whole clips,
// and boxing them would cost an indirection at every site that builds one for no gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    /// Insert `device` at `index` together with `cables` (all attached to it).
    Insert {
        index: usize,
        device: RackDevice,
        cables: Vec<Cable>,
    },
    /// Remove the device at `index` (which is `device`) and its `cables`.
    Remove {
        index: usize,
        device: RackDevice,
        cables: Vec<Cable>,
    },
    /// Re-arrange the top-level devices: `old` and `new` hold every device's key and column in
    /// rack order (the same keys in both). Used by every move and by column removal.
    Layout {
        old: Vec<(DeviceKey, u16)>,
        new: Vec<(DeviceKey, u16)>,
    },
    /// The number of rack columns.
    Columns {
        old: u16,
        new: u16,
    },
    /// Change presentation fields of a device.
    Folded {
        key: DeviceKey,
        old: bool,
        new: bool,
    },
    Name {
        key: DeviceKey,
        old: String,
        new: String,
    },
    Color {
        key: DeviceKey,
        old: Option<[u8; 3]>,
        new: Option<[u8; 3]>,
    },
    Connect(Cable),
    Disconnect(Cable),
    /// Set (`Some`) or reset to the default (`None`) a param; `default` is the descriptor
    /// default, reported to the engine when the value becomes `None`.
    Param {
        key: DeviceKey,
        param: String,
        old: Option<f32>,
        new: Option<f32>,
        default: f32,
    },
    /// The rack tempo; also the song's first tempo event (`song.tempo[0].bpm == tempo_bpm`).
    Tempo {
        old: f64,
        new: f64,
    },
    /// Insert (`old: None`), remove (`new: None`) a whole track at `index`.
    Track {
        index: usize,
        old: Option<Track>,
        new: Option<Track>,
    },
    /// Track mute, solo, arm.
    TrackFlags {
        device: DeviceKey,
        old: [bool; 3],
        new: [bool; 3],
    },
    /// Track chase flag.
    TrackChase {
        device: DeviceKey,
        old: bool,
        new: bool,
    },
    /// Where a track's notes enter (`None`: its own device).
    TrackEntry {
        device: DeviceKey,
        old: Option<DeviceKey>,
        new: Option<DeviceKey>,
    },
    /// Write mode of an automation lane (`None` is the default).
    LaneWrite {
        device: DeviceKey,
        param: String,
        old: Option<WriteMode>,
        new: Option<WriteMode>,
    },
    /// The name and mute state of a note lane (`(name, muted)`).
    LaneMeta {
        device: DeviceKey,
        lane: usize,
        old: (Option<String>, bool),
        new: (Option<String>, bool),
    },
    /// A track's frozen state (see [`crate::freeze`]).
    Frozen {
        device: DeviceKey,
        old: Option<Frozen>,
        new: Option<Frozen>,
    },
    /// Insert or remove lane `index` of a track.
    Lane {
        device: DeviceKey,
        index: usize,
        old: Option<Lane>,
        new: Option<Lane>,
    },
    /// Insert (`old: None`), remove (`new: None`) or replace (same id) a clip on a lane. The
    /// lane stays sorted by `(start, id)`.
    Clip {
        device: DeviceKey,
        lane: usize,
        old: Option<Clip>,
        new: Option<Clip>,
    },
    /// A lane's whole take group list.
    TakeGroups {
        device: DeviceKey,
        lane: usize,
        old: Vec<TakeGroup>,
        new: Vec<TakeGroup>,
    },
    /// The song's next clip id.
    NextClipId {
        old: u64,
        new: u64,
    },
    /// The whole tempo map; the rack tempo follows its first event.
    TempoMap {
        old: Vec<TempoEvent>,
        new: Vec<TempoEvent>,
    },
    /// The whole meter map.
    MeterMap {
        old: Vec<MeterEvent>,
        new: Vec<MeterEvent>,
    },
    /// The whole marker list.
    Markers {
        old: Vec<Marker>,
        new: Vec<Marker>,
    },
    /// The whole range list.
    Ranges {
        old: Vec<RangeMarker>,
        new: Vec<RangeMarker>,
    },
    /// The song's groove library.
    Grooves {
        old: Vec<Groove>,
        new: Vec<Groove>,
    },
    /// The track heights.
    TrackHeights {
        old: BTreeMap<DeviceKey, u16>,
        new: BTreeMap<DeviceKey, u16>,
    },
    /// The track order and the organisation (folders, colours, comments, folds).
    Organise {
        old: Box<TrackLayout>,
        new: Box<TrackLayout>,
    },
    Loop {
        old: ((i64, i64), bool),
        new: ((i64, i64), bool),
    },
    SongEnd {
        old: i64,
        new: i64,
    },
    /// Insert (`old: None`), remove (`new: None`) or replace a console channel at `index`.
    ConsoleChannel {
        index: usize,
        old: Option<ConsoleChannel>,
        new: Option<ConsoleChannel>,
    },
    /// Move the console channel at `from` to `to`.
    ConsoleMove {
        from: usize,
        to: usize,
    },
    /// Insert (`old: None`), remove (`new: None`) or replace (a rename) a console bus at `index`.
    ConsoleBus {
        index: usize,
        old: Option<ConsoleBus>,
        new: Option<ConsoleBus>,
    },
    /// Move the console bus at `from` to `to`.
    ConsoleBusMove {
        from: usize,
        to: usize,
    },
    /// The project's whole macro set (assignments and names).
    Macros {
        old: MacroSet,
        new: MacroSet,
    },
    /// The project's whole MIDI mapping list.
    MidiMaps {
        old: Vec<MidiMapping>,
        new: Vec<MidiMapping>,
    },
    /// The console's whole VCA group list.
    ConsoleVcas {
        old: Vec<VcaGroup>,
        new: Vec<VcaGroup>,
    },
    /// The console's whole snapshot list.
    ConsoleSnapshots {
        old: Vec<MixerSnapshot>,
        new: Vec<MixerSnapshot>,
    },
    /// The A/B comparison state of the console (not saved).
    SnapshotCompare {
        old: Option<Box<CompareState>>,
        new: Option<Box<CompareState>>,
    },
    /// The console master.
    ConsoleMaster {
        old: Option<DeviceKey>,
        new: Option<DeviceKey>,
    },
    /// The effect on FX return `slot`.
    ConsoleReturn {
        slot: usize,
        old: Option<DeviceKey>,
        new: Option<DeviceKey>,
    },
    /// The delay compensation switch.
    Compensation {
        old: bool,
        new: bool,
    },
    /// A sampler's whole zone list.
    Zones {
        key: DeviceKey,
        old: Vec<ZoneDoc>,
        new: Vec<ZoneDoc>,
    },
    /// A step sequencer's patterns (`None` is "none stored").
    Steps {
        key: DeviceKey,
        old: Option<StepSeqDoc>,
        new: Option<StepSeqDoc>,
    },
    /// A CV sequencer's curves (`None` is "none stored": flat).
    Curves {
        key: DeviceKey,
        old: Option<CurveDoc>,
        new: Option<CurveDoc>,
    },
    /// A drum generator's patterns (`None` is "none stored").
    Drums {
        key: DeviceKey,
        old: Option<DrumDoc>,
        new: Option<DrumDoc>,
    },
    /// The interface's input channel count.
    InputChannels {
        old: u16,
        new: u16,
    },
    /// Insert `device` into the container at path key `container` at `index`, with its inner
    /// `cables` (local keys, all attached to it).
    InnerInsert {
        container: DeviceKey,
        index: usize,
        device: RackDevice,
        cables: Vec<Cable>,
    },
    /// Remove the inner device at `index` (which is `device`) and its `cables`.
    InnerRemove {
        container: DeviceKey,
        index: usize,
        device: RackDevice,
        cables: Vec<Cable>,
    },
    /// Move an inner device within its container.
    InnerMove {
        container: DeviceKey,
        from: usize,
        to: usize,
    },
    /// An inner cable (local keys).
    InnerConnect {
        container: DeviceKey,
        cable: Cable,
    },
    InnerDisconnect {
        container: DeviceKey,
        cable: Cable,
    },
    /// A container's whole split list.
    Splits {
        container: DeviceKey,
        old: Vec<NoteSplit>,
        new: Vec<NoteSplit>,
    },
    /// One macro of a container (its name and targets).
    Macro {
        container: DeviceKey,
        index: u8,
        old: MacroDoc,
        new: MacroDoc,
    },
    /// A plugin device's pinned params.
    PluginPins {
        key: DeviceKey,
        old: Vec<u32>,
        new: Vec<u32>,
    },
    /// A plugin device's RUN ON AUDIO THREAD switch.
    PluginRun {
        key: DeviceKey,
        old: bool,
        new: bool,
    },
    /// A plugin device's power switch.
    PluginPower {
        key: DeviceKey,
        old: bool,
        new: bool,
    },
    /// A plugin device pointed at another plugin (type, format, id, hint, layout, pins).
    PluginRelink {
        key: DeviceKey,
        old: Box<RelinkData>,
        new: Box<RelinkData>,
    },
    /// Several edits performed in order as one undo step; the inverse undoes them in reverse.
    Compound(Vec<Edit>),
}

impl Edit {
    /// Folds `next`, the next clip edit of a drag-merge group, into this one (the last undo
    /// step of the group), so the group stays one step. A clip edit on the lane of an earlier
    /// edit of the same clip updates it in place (a drag keeps one edit per clip, not one per
    /// move); any other edit is appended and undone first.
    pub(crate) fn fold_clip_edit(&mut self, next: Edit) {
        let mut parts = match std::mem::replace(self, Edit::Compound(Vec::new())) {
            Edit::Compound(parts) => parts,
            one => vec![one],
        };
        let incoming = match next {
            Edit::Compound(more) => more,
            one => vec![one],
        };
        for edit in incoming {
            let Edit::Clip {
                device,
                lane,
                old: Some(old),
                new,
            } = &edit
            else {
                parts.push(edit);
                continue;
            };
            // The latest earlier edit that touches this clip, when it left the clip on this lane.
            let at = parts.iter().rposition(|p| match p {
                Edit::Clip { old: o, new: n, .. } => {
                    [o, n].into_iter().flatten().any(|c| c.id == old.id)
                }
                _ => false,
            });
            match at.map(|i| (i, &mut parts[i])) {
                Some((
                    _,
                    Edit::Clip {
                        device: d,
                        lane: l,
                        new: slot @ Some(_),
                        ..
                    },
                )) if d == device && l == lane => slot.clone_from(new),
                _ => parts.push(edit),
            }
        }
        *self = Edit::Compound(parts);
    }

    /// Adds the asset ids of the audio clips and zones this edit holds (both sides) to `out`.
    pub(crate) fn collect_assets(&self, out: &mut BTreeSet<String>) {
        let clip = |c: &Clip, out: &mut BTreeSet<String>| {
            if let ClipContent::Audio(a) = &c.content {
                out.insert(a.asset.clone());
            }
        };
        let lane = |l: &Lane, out: &mut BTreeSet<String>| {
            l.clips.iter().for_each(|c| clip(c, out));
            l.take_groups
                .iter()
                .flat_map(|g| &g.takes)
                .for_each(|c| clip(c, out));
        };
        match self {
            Edit::Track { old, new, .. } => {
                for t in old.iter().chain(new) {
                    t.lanes.iter().for_each(|l| lane(l, out));
                    out.extend(t.frozen.iter().map(|f| f.audio.asset.clone()));
                }
            }
            Edit::Frozen { old, new, .. } => {
                out.extend(old.iter().chain(new).map(|f| f.audio.asset.clone()));
            }
            Edit::Lane { old, new, .. } => old.iter().chain(new).for_each(|l| lane(l, out)),
            Edit::Clip { old, new, .. } => old.iter().chain(new).for_each(|c| clip(c, out)),
            Edit::TakeGroups { old, new, .. } => old
                .iter()
                .chain(new)
                .flat_map(|g| &g.takes)
                .for_each(|c| clip(c, out)),
            Edit::Compound(edits) => edits.iter().for_each(|e| e.collect_assets(out)),
            Edit::Insert { device, .. }
            | Edit::Remove { device, .. }
            | Edit::InnerInsert { device, .. }
            | Edit::InnerRemove { device, .. } => collect_device_assets(device, out),
            Edit::Zones { old, new, .. } => {
                out.extend(old.iter().chain(new).map(|z| z.asset.clone()));
            }
            _ => {}
        }
    }

    /// The edit that undoes this one.
    pub(crate) fn inverse(&self) -> Edit {
        match self.clone() {
            Edit::Insert {
                index,
                device,
                cables,
            } => Edit::Remove {
                index,
                device,
                cables,
            },
            Edit::Remove {
                index,
                device,
                cables,
            } => Edit::Insert {
                index,
                device,
                cables,
            },
            Edit::Layout { old, new } => Edit::Layout { old: new, new: old },
            Edit::Columns { old, new } => Edit::Columns { old: new, new: old },
            Edit::Folded { key, old, new } => Edit::Folded {
                key,
                old: new,
                new: old,
            },
            Edit::Name { key, old, new } => Edit::Name {
                key,
                old: new,
                new: old,
            },
            Edit::Color { key, old, new } => Edit::Color {
                key,
                old: new,
                new: old,
            },
            Edit::Connect(c) => Edit::Disconnect(c),
            Edit::Disconnect(c) => Edit::Connect(c),
            Edit::Param {
                key,
                param,
                old,
                new,
                default,
            } => Edit::Param {
                key,
                param,
                old: new,
                new: old,
                default,
            },
            Edit::Tempo { old, new } => Edit::Tempo { old: new, new: old },
            Edit::Track { index, old, new } => Edit::Track {
                index,
                old: new,
                new: old,
            },
            Edit::TrackFlags { device, old, new } => Edit::TrackFlags {
                device,
                old: new,
                new: old,
            },
            Edit::TrackChase { device, old, new } => Edit::TrackChase {
                device,
                old: new,
                new: old,
            },
            Edit::TrackEntry { device, old, new } => Edit::TrackEntry {
                device,
                old: new,
                new: old,
            },
            Edit::LaneMeta {
                device,
                lane,
                old,
                new,
            } => Edit::LaneMeta {
                device,
                lane,
                old: new,
                new: old,
            },
            Edit::Frozen { device, old, new } => Edit::Frozen {
                device,
                old: new,
                new: old,
            },
            Edit::LaneWrite {
                device,
                param,
                old,
                new,
            } => Edit::LaneWrite {
                device,
                param,
                old: new,
                new: old,
            },
            Edit::Lane {
                device,
                index,
                old,
                new,
            } => Edit::Lane {
                device,
                index,
                old: new,
                new: old,
            },
            Edit::Clip {
                device,
                lane,
                old,
                new,
            } => Edit::Clip {
                device,
                lane,
                old: new,
                new: old,
            },
            Edit::TakeGroups {
                device,
                lane,
                old,
                new,
            } => Edit::TakeGroups {
                device,
                lane,
                old: new,
                new: old,
            },
            Edit::NextClipId { old, new } => Edit::NextClipId { old: new, new: old },
            Edit::TempoMap { old, new } => Edit::TempoMap { old: new, new: old },
            Edit::MeterMap { old, new } => Edit::MeterMap { old: new, new: old },
            Edit::Markers { old, new } => Edit::Markers { old: new, new: old },
            Edit::Ranges { old, new } => Edit::Ranges { old: new, new: old },
            Edit::Grooves { old, new } => Edit::Grooves { old: new, new: old },
            Edit::TrackHeights { old, new } => Edit::TrackHeights { old: new, new: old },
            Edit::Organise { old, new } => Edit::Organise { old: new, new: old },
            Edit::Loop { old, new } => Edit::Loop { old: new, new: old },
            Edit::SongEnd { old, new } => Edit::SongEnd { old: new, new: old },
            Edit::ConsoleChannel { index, old, new } => Edit::ConsoleChannel {
                index,
                old: new,
                new: old,
            },
            Edit::ConsoleMove { from, to } => Edit::ConsoleMove { from: to, to: from },
            Edit::ConsoleBus { index, old, new } => Edit::ConsoleBus {
                index,
                old: new,
                new: old,
            },
            Edit::ConsoleBusMove { from, to } => Edit::ConsoleBusMove { from: to, to: from },
            Edit::Macros { old, new } => Edit::Macros { old: new, new: old },
            Edit::MidiMaps { old, new } => Edit::MidiMaps { old: new, new: old },
            Edit::ConsoleVcas { old, new } => Edit::ConsoleVcas { old: new, new: old },
            Edit::ConsoleSnapshots { old, new } => Edit::ConsoleSnapshots { old: new, new: old },
            Edit::SnapshotCompare { old, new } => Edit::SnapshotCompare { old: new, new: old },
            Edit::ConsoleMaster { old, new } => Edit::ConsoleMaster { old: new, new: old },
            Edit::ConsoleReturn { slot, old, new } => Edit::ConsoleReturn {
                slot,
                old: new,
                new: old,
            },
            Edit::Compensation { old, new } => Edit::Compensation { old: new, new: old },
            Edit::Zones { key, old, new } => Edit::Zones {
                key,
                old: new,
                new: old,
            },
            Edit::Steps { key, old, new } => Edit::Steps {
                key,
                old: new,
                new: old,
            },
            Edit::Curves { key, old, new } => Edit::Curves {
                key,
                old: new,
                new: old,
            },
            Edit::Drums { key, old, new } => Edit::Drums {
                key,
                old: new,
                new: old,
            },
            Edit::InputChannels { old, new } => Edit::InputChannels { old: new, new: old },
            Edit::InnerInsert {
                container,
                index,
                device,
                cables,
            } => Edit::InnerRemove {
                container,
                index,
                device,
                cables,
            },
            Edit::InnerRemove {
                container,
                index,
                device,
                cables,
            } => Edit::InnerInsert {
                container,
                index,
                device,
                cables,
            },
            Edit::InnerMove {
                container,
                from,
                to,
            } => Edit::InnerMove {
                container,
                from: to,
                to: from,
            },
            Edit::InnerConnect { container, cable } => Edit::InnerDisconnect { container, cable },
            Edit::InnerDisconnect { container, cable } => Edit::InnerConnect { container, cable },
            Edit::Splits {
                container,
                old,
                new,
            } => Edit::Splits {
                container,
                old: new,
                new: old,
            },
            Edit::Macro {
                container,
                index,
                old,
                new,
            } => Edit::Macro {
                container,
                index,
                old: new,
                new: old,
            },
            Edit::PluginPins { key, old, new } => Edit::PluginPins {
                key,
                old: new,
                new: old,
            },
            Edit::PluginRun { key, old, new } => Edit::PluginRun {
                key,
                old: new,
                new: old,
            },
            Edit::PluginPower { key, old, new } => Edit::PluginPower {
                key,
                old: new,
                new: old,
            },
            Edit::PluginRelink { key, old, new } => Edit::PluginRelink {
                key,
                old: new,
                new: old,
            },
            Edit::Compound(edits) => {
                Edit::Compound(edits.iter().rev().map(Edit::inverse).collect())
            }
        }
    }

    /// Performs the edit on `doc` and returns the engine-sync operations, in the order the
    /// app must process them.
    ///
    /// # Panics
    /// If the edit does not fit `doc`; edits are only built from validated commands and
    /// replayed in history order, so this is an internal invariant.
    pub(crate) fn perform(&self, doc: &mut RackDoc) -> Vec<SyncOp> {
        match self {
            Edit::Insert {
                index,
                device,
                cables,
            } => {
                // A new device never breaks the column order: its column is held between its
                // neighbours' (a plain `AddDevice` carries column 0 and so joins the column of
                // the device before it).
                let mut device = device.clone();
                let (lo, hi) = doc.column_bounds(*index);
                device.column = device.column.clamp(lo, hi);
                doc.devices.insert(*index, device.clone());
                doc.cables.extend(cables.iter().cloned());
                doc.cables.sort();
                let mut ops = vec![SyncOp::DeviceAdded(device.key.clone())];
                ops.extend(
                    device
                        .params
                        .iter()
                        .map(|(param, &value)| SyncOp::ParamChanged {
                            key: device.key.clone(),
                            param: param.clone(),
                            value,
                        }),
                );
                ops.push(SyncOp::RoutingChanged);
                ops
            }
            Edit::Remove { index, device, .. } => {
                let removed = doc.devices.remove(*index);
                debug_assert_eq!(&removed.key, &device.key);
                doc.cables
                    .retain(|c| c.from.device != device.key && c.to.device != device.key);
                vec![
                    SyncOp::RoutingChanged,
                    SyncOp::DeviceRemoved(device.key.clone()),
                ]
            }
            Edit::Layout { new, .. } => {
                let mut by_key: BTreeMap<DeviceKey, RackDevice> =
                    doc.devices.drain(..).map(|d| (d.key.clone(), d)).collect();
                for (key, column) in new {
                    let mut d = by_key.remove(key).expect("edit targets an existing device");
                    d.column = *column;
                    doc.devices.push(d);
                }
                debug_assert!(by_key.is_empty(), "a layout names every device");
                vec![SyncOp::ViewChanged]
            }
            Edit::Columns { new, .. } => {
                doc.columns = *new;
                vec![SyncOp::ViewChanged]
            }
            Edit::Folded { key, new, .. } => {
                device_mut(doc, key).folded = *new;
                vec![SyncOp::ViewChanged]
            }
            Edit::Name { key, new, .. } => {
                device_mut(doc, key).name.clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::Color { key, new, .. } => {
                device_mut(doc, key).color = *new;
                vec![SyncOp::ViewChanged]
            }
            Edit::Connect(c) => {
                let at = doc.cables.binary_search(c).unwrap_err();
                doc.cables.insert(at, c.clone());
                vec![SyncOp::RoutingChanged]
            }
            Edit::Disconnect(c) => {
                let at = doc.cables.binary_search(c).expect("cable present");
                doc.cables.remove(at);
                vec![SyncOp::RoutingChanged]
            }
            Edit::Param {
                key,
                param,
                new,
                default,
                ..
            } => {
                let params = &mut device_mut(doc, key).params;
                match new {
                    Some(v) => {
                        params.insert(param.clone(), *v);
                    }
                    None => {
                        params.remove(param);
                    }
                }
                let mut ops = vec![SyncOp::ParamChanged {
                    key: key.clone(),
                    param: param.clone(),
                    value: new.unwrap_or(*default),
                }];
                // Solo silence derives from the channels' solo params.
                if param == "solo"
                    && (doc.console.channel_index(key).is_some()
                        || doc.console.bus_index(key).is_some())
                {
                    ops.push(SyncOp::ConsoleChanged);
                }
                ops
            }
            Edit::Tempo { new, .. } => {
                doc.tempo_bpm = *new;
                doc.song.tempo[0].bpm = *new;
                vec![SyncOp::TempoChanged(*new)]
            }
            Edit::Track { index, old, new } => {
                if let Some(old) = old {
                    let removed = doc.song.tracks.remove(*index);
                    debug_assert_eq!(&removed, old);
                }
                if let Some(new) = new {
                    doc.song.tracks.insert(*index, new.clone());
                }
                vec![SyncOp::SongChanged]
            }
            Edit::TrackFlags { device, new, .. } => {
                let t = track_mut(doc, device);
                [t.muted, t.solo, t.armed] = *new;
                vec![SyncOp::SongChanged]
            }
            Edit::TrackChase { device, new, .. } => {
                track_mut(doc, device).chase = *new;
                vec![SyncOp::SongChanged]
            }
            Edit::TrackEntry { device, new, .. } => {
                track_mut(doc, device).notes_to.clone_from(new);
                vec![SyncOp::SongChanged]
            }
            Edit::LaneMeta {
                device, lane, new, ..
            } => {
                let lane = &mut track_mut(doc, device).lanes[*lane];
                lane.name.clone_from(&new.0);
                lane.muted = new.1;
                vec![SyncOp::SongChanged]
            }
            Edit::Frozen { device, new, .. } => {
                track_mut(doc, device).frozen.clone_from(new);
                vec![SyncOp::SongChanged, SyncOp::FrozenChanged]
            }
            Edit::LaneWrite {
                device, param, new, ..
            } => {
                let key = (device.clone(), param.clone());
                match new {
                    Some(mode) => {
                        doc.song.lane_write.insert(key, *mode);
                    }
                    None => {
                        doc.song.lane_write.remove(&key);
                    }
                }
                vec![SyncOp::ViewChanged]
            }
            Edit::Lane {
                device,
                index,
                old,
                new,
            } => {
                let lanes = &mut track_mut(doc, device).lanes;
                if old.is_some() {
                    lanes.remove(*index);
                }
                if let Some(new) = new {
                    lanes.insert(*index, new.clone());
                }
                vec![SyncOp::SongChanged]
            }
            Edit::Clip {
                device,
                lane,
                old,
                new,
            } => {
                let clips = &mut track_mut(doc, device).lanes[*lane].clips;
                if let Some(old) = old {
                    let at = clips
                        .iter()
                        .position(|c| c.id == old.id)
                        .expect("edit targets an existing clip");
                    clips.remove(at);
                }
                if let Some(new) = new {
                    let at = clips.partition_point(|c| (c.start, c.id) < (new.start, new.id));
                    clips.insert(at, new.clone());
                }
                vec![SyncOp::SongChanged]
            }
            Edit::TakeGroups {
                device, lane, new, ..
            } => {
                track_mut(doc, device).lanes[*lane]
                    .take_groups
                    .clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::NextClipId { new, .. } => {
                doc.song.next_clip_id = *new;
                vec![SyncOp::SongChanged]
            }
            Edit::TempoMap { new, .. } => {
                doc.song.tempo.clone_from(new);
                doc.tempo_bpm = new[0].bpm;
                vec![SyncOp::TempoChanged(new[0].bpm), SyncOp::SongChanged]
            }
            Edit::MeterMap { new, .. } => {
                doc.song.meter.clone_from(new);
                vec![SyncOp::SongChanged]
            }
            Edit::Markers { new, .. } => {
                doc.song.markers.clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::Grooves { new, .. } => {
                doc.song.grooves.clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::Ranges { new, .. } => {
                doc.song.ranges.clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::TrackHeights { new, .. } => {
                doc.song.track_heights.clone_from(new);
                vec![SyncOp::ViewChanged]
            }
            Edit::Organise { new, .. } => {
                let song = &mut doc.song;
                let moved = song.tracks.iter().map(|t| &t.device).ne(new.order.iter());
                if moved {
                    song.tracks.sort_by_key(|t| {
                        new.order
                            .iter()
                            .position(|d| d == &t.device)
                            .unwrap_or(usize::MAX)
                    });
                }
                song.org.clone_from(&new.org);
                vec![if moved {
                    SyncOp::SongChanged
                } else {
                    SyncOp::ViewChanged
                }]
            }
            Edit::Loop { new, .. } => {
                (doc.song.loop_range, doc.song.loop_enabled) = *new;
                vec![SyncOp::SongChanged]
            }
            Edit::SongEnd { new, .. } => {
                doc.song.end = *new;
                vec![SyncOp::SongChanged]
            }
            Edit::ConsoleChannel { index, old, new } => {
                let channels = &mut doc.console.channels;
                if let Some(old) = old {
                    let removed = channels.remove(*index);
                    debug_assert_eq!(&removed, old);
                }
                if let Some(new) = new {
                    channels.insert(*index, new.clone());
                }
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleMove { from, to } => {
                let c = doc.console.channels.remove(*from);
                doc.console.channels.insert(*to, c);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleBus { index, old, new } => {
                let buses = &mut doc.console.buses;
                if let Some(old) = old {
                    let removed = buses.remove(*index);
                    debug_assert_eq!(&removed, old);
                }
                if let Some(new) = new {
                    buses.insert(*index, new.clone());
                }
                vec![SyncOp::ConsoleChanged]
            }
            Edit::Macros { old, new } => {
                let before = std::mem::replace(&mut doc.macros, new.clone());
                // Every bank that had or has an assignment or a name: the app re-sends its targets.
                let mut banks: Vec<DeviceKey> = Vec::new();
                for set in [old, new, &before] {
                    let named = set.names.iter().map(|n| &n.bank);
                    for bank in set.assigns.iter().map(|a| &a.bank).chain(named) {
                        if !banks.contains(bank) {
                            banks.push(bank.clone());
                        }
                    }
                }
                banks
                    .into_iter()
                    .map(|container| SyncOp::MacrosChanged { container })
                    .collect()
            }
            Edit::MidiMaps { new, .. } => {
                doc.midi_maps.clone_from(new);
                vec![SyncOp::MidiMapsChanged]
            }
            Edit::ConsoleVcas { new, .. } => {
                doc.console.vcas.clone_from(new);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleSnapshots { new, .. } => {
                doc.console.snapshots.clone_from(new);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::SnapshotCompare { new, .. } => {
                doc.console.compare = new.as_deref().cloned();
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleBusMove { from, to } => {
                let b = doc.console.buses.remove(*from);
                doc.console.buses.insert(*to, b);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleMaster { new, .. } => {
                doc.console.master.clone_from(new);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::ConsoleReturn { slot, new, .. } => {
                doc.console.returns[*slot].clone_from(new);
                vec![SyncOp::ConsoleChanged]
            }
            Edit::Compensation { new, .. } => {
                doc.console.compensation = *new;
                vec![SyncOp::ConsoleChanged]
            }
            Edit::Zones { key, new, .. } => {
                device_mut(doc, key).zones.clone_from(new);
                vec![SyncOp::ZonesChanged(key.clone())]
            }
            Edit::Steps { key, new, .. } => {
                device_mut(doc, key).steps.clone_from(new);
                vec![SyncOp::PatternsChanged {
                    device: key.clone(),
                }]
            }
            Edit::Curves { key, new, .. } => {
                device_mut(doc, key).curves.clone_from(new);
                vec![SyncOp::PatternsChanged {
                    device: key.clone(),
                }]
            }
            Edit::Drums { key, new, .. } => {
                device_mut(doc, key).drums.clone_from(new);
                vec![SyncOp::PatternsChanged {
                    device: key.clone(),
                }]
            }
            Edit::InputChannels { new, .. } => {
                doc.input_channels = *new;
                vec![SyncOp::RoutingChanged]
            }
            Edit::InnerInsert {
                container,
                index,
                device,
                cables,
            } => {
                let inner = container_mut(doc, container);
                inner.devices.insert(*index, device.clone());
                inner.cables.extend(cables.iter().cloned());
                inner.cables.sort();
                // A container that comes with its inside is announced once: the app expands it.
                let path = join(Some(container), &device.key);
                let mut ops = vec![SyncOp::DeviceAdded(path.clone())];
                ops.extend(
                    device
                        .params
                        .iter()
                        .map(|(param, &value)| SyncOp::ParamChanged {
                            key: path.clone(),
                            param: param.clone(),
                            value,
                        }),
                );
                ops.push(SyncOp::RoutingChanged);
                ops
            }
            Edit::InnerRemove {
                container,
                index,
                device,
                ..
            } => {
                let inner = container_mut(doc, container);
                let removed = inner.devices.remove(*index);
                debug_assert_eq!(&removed.key, &device.key);
                inner
                    .cables
                    .retain(|c| c.from.device != device.key && c.to.device != device.key);
                vec![
                    SyncOp::RoutingChanged,
                    SyncOp::DeviceRemoved(join(Some(container), &device.key)),
                ]
            }
            Edit::InnerMove {
                container,
                from,
                to,
            } => {
                let inner = container_mut(doc, container);
                let d = inner.devices.remove(*from);
                inner.devices.insert(*to, d);
                vec![SyncOp::ViewChanged]
            }
            Edit::InnerConnect { container, cable } => {
                let cables = &mut container_mut(doc, container).cables;
                let at = cables.binary_search(cable).unwrap_err();
                cables.insert(at, cable.clone());
                vec![SyncOp::RoutingChanged]
            }
            Edit::InnerDisconnect { container, cable } => {
                let cables = &mut container_mut(doc, container).cables;
                let at = cables.binary_search(cable).expect("cable present");
                cables.remove(at);
                vec![SyncOp::RoutingChanged]
            }
            Edit::Splits {
                container,
                old,
                new,
            } => {
                container_mut(doc, container).splits.clone_from(new);
                let targets =
                    |l: &[NoteSplit]| l.iter().map(|s| s.target.clone()).collect::<Vec<_>>();
                let mut ops = vec![SyncOp::SplitsChanged {
                    container: container.clone(),
                }];
                // The split device's outputs are cabled to the targets in slot order.
                if targets(old) != targets(new) {
                    ops.push(SyncOp::RoutingChanged);
                }
                ops
            }
            Edit::Macro {
                container,
                index,
                new,
                ..
            } => {
                container_mut(doc, container).macros[usize::from(*index)].clone_from(new);
                vec![SyncOp::MacrosChanged {
                    container: container.clone(),
                }]
            }
            Edit::PluginPins { key, new, .. } => {
                plugin_mut(doc, key).pinned.clone_from(new);
                vec![SyncOp::PluginChanged {
                    device: key.clone(),
                }]
            }
            Edit::PluginRun { key, new, .. } => {
                plugin_mut(doc, key).run_on_audio_thread = *new;
                vec![SyncOp::PluginChanged {
                    device: key.clone(),
                }]
            }
            Edit::PluginPower { key, new, .. } => {
                plugin_mut(doc, key).powered = *new;
                vec![SyncOp::PluginChanged {
                    device: key.clone(),
                }]
            }
            Edit::PluginRelink { key, new, .. } => {
                let device = device_mut(doc, key);
                device.type_id.clone_from(&new.type_id);
                let p = device
                    .plugin
                    .as_mut()
                    .expect("edit targets a plugin device");
                p.format = new.format;
                p.id.clone_from(&new.id);
                p.path_hint.clone_from(&new.path_hint);
                p.layout.clone_from(&new.layout);
                p.pinned.clone_from(&new.pinned);
                vec![SyncOp::PluginChanged {
                    device: key.clone(),
                }]
            }
            Edit::Compound(edits) => {
                let mut ops = CompoundOps::default();
                // A compound inside a compound (a planned track inside a bounce) is folded in:
                // its inserts must still count as inserts, or the routing would come before
                // the devices it names.
                let mut flat = Vec::new();
                flatten(edits, &mut flat);
                for edit in flat {
                    let inserted = matches!(edit, Edit::Insert { .. } | Edit::InnerInsert { .. });
                    for op in edit.perform(doc) {
                        ops.push(op, inserted);
                    }
                }
                ops.finish()
            }
        }
    }
}

/// The leaf edits of `edits` in order, nested compounds opened.
fn flatten<'e>(edits: &'e [Edit], out: &mut Vec<&'e Edit>) {
    for edit in edits {
        match edit {
            Edit::Compound(inner) => flatten(inner, out),
            leaf => out.push(leaf),
        }
    }
}

/// The sync ops of one compound step, in a canonical order. The app reads routing, song and
/// console from the final document, so each is reported once, where every device it can
/// reference exists:
///
/// 1. `DeviceAdded` with the stored params of each inserted device;
/// 2. without removals: `RoutingChanged`;
/// 3. other ops (`ParamChanged`, `TempoChanged`) in order, then `ViewChanged`, `ConsoleChanged`;
/// 4. `SongChanged`;
/// 5. with removals: `RoutingChanged`, then the `DeviceRemoved`s.
///
/// So the song changes after devices arrive and before they leave, and routing never names a
/// device the engine does not have.
#[derive(Default)]
struct CompoundOps {
    added: Vec<SyncOp>,
    other: Vec<SyncOp>,
    removed: Vec<SyncOp>,
    routing: bool,
    song: bool,
    view: bool,
    console: bool,
}

impl CompoundOps {
    fn push(&mut self, op: SyncOp, inserted: bool) {
        match op {
            SyncOp::RoutingChanged => self.routing = true,
            SyncOp::SongChanged => self.song = true,
            SyncOp::ViewChanged => self.view = true,
            SyncOp::ConsoleChanged => self.console = true,
            op @ SyncOp::DeviceRemoved(_) => self.removed.push(op),
            op if inserted => self.added.push(op),
            // One report per device, however many `SetZones` a batch holds.
            op @ (SyncOp::ZonesChanged(_)
            | SyncOp::PatternsChanged { .. }
            | SyncOp::SplitsChanged { .. }
            | SyncOp::MacrosChanged { .. }
            | SyncOp::PluginChanged { .. })
                if self.other.contains(&op) => {}
            op => self.other.push(op),
        }
    }

    fn finish(self) -> Vec<SyncOp> {
        let removing = !self.removed.is_empty();
        let mut ops = self.added;
        if self.routing && !removing {
            ops.push(SyncOp::RoutingChanged);
        }
        ops.extend(self.other);
        let flags = [
            (self.view, SyncOp::ViewChanged),
            (self.console, SyncOp::ConsoleChanged),
            (self.song, SyncOp::SongChanged),
            (self.routing && removing, SyncOp::RoutingChanged),
        ];
        ops.extend(flags.into_iter().filter(|(on, _)| *on).map(|(_, op)| op));
        ops.extend(self.removed);
        ops
    }
}

fn track_mut<'d>(doc: &'d mut RackDoc, device: &DeviceKey) -> &'d mut Track {
    doc.song
        .tracks
        .iter_mut()
        .find(|t| &t.device == device)
        .expect("edit targets an existing track")
}

fn device_mut<'d>(doc: &'d mut RackDoc, key: &DeviceKey) -> &'d mut RackDevice {
    doc.resolve_mut(key)
        .expect("edit targets an existing device")
}

fn plugin_mut<'d>(doc: &'d mut RackDoc, key: &DeviceKey) -> &'d mut crate::PluginDoc {
    device_mut(doc, key)
        .plugin
        .as_deref_mut()
        .expect("edit targets a plugin device")
}

fn container_mut<'d>(doc: &'d mut RackDoc, key: &DeviceKey) -> &'d mut ContainerDoc {
    doc.inner_mut(key)
        .expect("edit targets an existing container")
}

/// The edit that moves the song End out to the next bar line when `edit` (already performed on
/// `doc`) puts a clip past it, `None` when no clip reaches beyond the End. It never moves the
/// End back: shortening is `SongCommand::SetSongEnd`. The caller records it in the same undo
/// step as `edit`.
pub(crate) fn song_end_growth(doc: &RackDoc, edit: &Edit) -> Option<Edit> {
    fn farthest(edit: &Edit) -> Option<i64> {
        match edit {
            Edit::Clip { new: Some(c), .. } => Some(c.start + c.length),
            Edit::Compound(edits) => edits.iter().filter_map(farthest).max(),
            _ => None,
        }
    }
    let song = &doc.song;
    let reach = farthest(edit).filter(|r| *r > song.end)?;
    let table = song.meter_table();
    let end = table.map().next_bar_line(reach - 1).max(reach);
    crate::song_cmd::check_end(end).ok()?;
    Some(Edit::SongEnd {
        old: song.end,
        new: end,
    })
}
