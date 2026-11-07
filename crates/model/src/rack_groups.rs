//! Device groups and the rack sorting modes.
//!
//! A **device group** is everything that belongs to one track: its instrument (or audio track
//! device), the note players of its stack, its console channel with the insert effects of the
//! channel, and the effects cabled after the instrument ahead of the channel. A group is
//! derived from the document ([`RackDoc::device_groups`]); nothing about it is stored. A
//! device that two instruments both reach (a shared effect) belongs to neither, and an effect
//! on a send/return slot is the console's, not a track's, so two instruments that share an
//! effect return stay separate groups.
//!
//! [`RackDoc::sort_mode`] says how the rack keeps its devices arranged:
//!
//! * [`RackSort::Free`]: nothing is arranged for the user (the columns are manual); the default,
//!   and what a project saved without the field loads as.
//! * [`RackSort::PerTrack`]: the devices outside any group (the master, loose devices) stay in
//!   column 0 and every group has a column of its own, in rack order; no column is ever empty.
//! * [`RackSort::AutoGroup`]: the columns stay as they are but the members of a group are
//!   gathered into one run at the place of the group's first device.
//!
//! While the mode is not Free, every edit that adds, removes or moves a top-level device ends
//! with the layout that mode asks for, inside the same compound edit, so one undo step undoes
//! the edit and its re-sorting together. A move of one device in such a mode moves its whole
//! group. [`crate::Command::SetRackSort`] changes the mode and arranges the rack in one step.
//! The mode is view data: it never changes a sound and reports
//! [`crate::SyncOp::ViewChanged`] only. It is saved as the root field `"rack_sort"` (project
//! format 42, ignorable) only when it is not Free.

use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, string};
use crate::{DeviceKey, ModelError, RackDoc, WorkflowCommand, player_stack};
use atomik_device_sdk::{DeviceKind, Registry};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// How the rack arranges its devices (see the module documentation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RackSort {
    /// Manual columns; nothing is arranged.
    #[default]
    Free,
    /// One column per track group.
    PerTrack,
    /// The members of a group stay together.
    AutoGroup,
}

impl RackSort {
    /// The word the project file uses (`Free` is never written).
    pub fn word(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::PerTrack => "per_track",
            Self::AutoGroup => "auto_group",
        }
    }

    /// The mode a word names.
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Free, Self::PerTrack, Self::AutoGroup]
            .into_iter()
            .find(|m| m.word() == word)
    }
}

/// The devices of one track (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGroup {
    /// The instrument or audio track device the group hangs on.
    pub anchor: DeviceKey,
    /// The song track of the anchor, when it has one.
    pub track: Option<DeviceKey>,
    /// Every top-level member, anchor included, in rack order.
    pub members: Vec<DeviceKey>,
}

impl RackDoc {
    /// Every device group, ordered by the rack position of its first member.
    pub fn device_groups(&self, registry: &Registry) -> Vec<DeviceGroup> {
        let index: BTreeMap<&DeviceKey, usize> = self
            .devices
            .iter()
            .enumerate()
            .map(|(i, d)| (&d.key, i))
            .collect();
        // Anchors: the devices of the song tracks, then the sources of channels without one.
        let mut anchors: Vec<DeviceKey> = Vec::new();
        let candidates = self.song.tracks.iter().map(|t| &t.device).chain(
            self.console
                .channels
                .iter()
                .filter_map(|c| c.source.as_ref()),
        );
        for key in candidates {
            if index.contains_key(key) && !anchors.contains(key) {
                anchors.push(key.clone());
            }
        }
        let is_anchor: BTreeSet<&DeviceKey> = anchors.iter().collect();
        let mut claims: BTreeMap<DeviceKey, Vec<usize>> = BTreeMap::new();
        for (n, anchor) in anchors.iter().enumerate() {
            for key in self.claimed_by(registry, anchor, &is_anchor) {
                if index.contains_key(&key) && !is_anchor.contains(&key) {
                    claims.entry(key).or_default().push(n);
                }
            }
        }
        let mut groups: Vec<DeviceGroup> = anchors
            .iter()
            .map(|a| DeviceGroup {
                anchor: a.clone(),
                track: self.song.track(a).map(|t| t.device.clone()),
                members: vec![a.clone()],
            })
            .collect();
        for (key, by) in claims {
            if let [only] = by[..] {
                groups[only].members.push(key);
            }
        }
        for g in &mut groups {
            g.members.sort_by_key(|k| index[k]);
        }
        groups.sort_by_key(|g| index[&g.members[0]]);
        groups
    }

    /// The group `key` belongs to, if any.
    pub fn device_group(&self, registry: &Registry, key: &DeviceKey) -> Option<DeviceGroup> {
        self.device_groups(registry)
            .into_iter()
            .find(|g| g.members.contains(key))
    }

    /// The non-anchor devices `anchor` reaches: its players, its channels and their inserts,
    /// and the effects cabled after it.
    fn claimed_by(
        &self,
        registry: &Registry,
        anchor: &DeviceKey,
        anchors: &BTreeSet<&DeviceKey>,
    ) -> Vec<DeviceKey> {
        let mut out: Vec<DeviceKey> = Vec::new();
        if let Some(track) = self.song.track(anchor) {
            out.extend(player_stack(self, registry, track));
        }
        for channel in self
            .console
            .channels
            .iter()
            .filter(|c| c.source.as_ref() == Some(anchor))
        {
            out.push(channel.channel.clone());
            out.extend(crate::insert_chain(self, registry, &channel.channel));
        }
        // Effects cabled after the instrument, ahead of the console.
        let mut frontier = vec![anchor.clone()];
        let mut seen: BTreeSet<DeviceKey> = BTreeSet::new();
        while let Some(from) = frontier.pop() {
            for cable in self.cables.iter().filter(|c| c.from.device == from) {
                let to = &cable.to.device;
                if seen.contains(to)
                    || anchors.contains(to)
                    || self.console.return_slot(to).is_some()
                {
                    continue;
                }
                let is_effect = self
                    .device(to)
                    .and_then(|d| registry.get(&d.type_id))
                    .is_some_and(|f| {
                        matches!(
                            f.descriptor.kind,
                            DeviceKind::Effect | DeviceKind::Container
                        )
                    });
                let is_console = self
                    .device(to)
                    .is_some_and(|d| crate::console::is_console_type(&d.type_id));
                if is_effect && !is_console {
                    seen.insert(to.clone());
                    out.push(to.clone());
                    frontier.push(to.clone());
                }
            }
        }
        out
    }
}

/// The layout the mode asks for: the new column count and every device's key and column in the
/// new rack order. `None` for Free.
fn arranged(doc: &RackDoc, registry: &Registry) -> Option<(u16, Vec<(DeviceKey, u16)>)> {
    if doc.sort_mode == RackSort::Free {
        return None;
    }
    let groups = doc.device_groups(registry);
    let group_of: BTreeMap<&DeviceKey, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(n, g)| g.members.iter().map(move |k| (k, n)))
        .collect();
    let last = crate::MAX_RACK_COLUMNS - 1;
    match doc.sort_mode {
        RackSort::Free => None,
        RackSort::PerTrack => {
            let loose = doc.devices.iter().any(|d| !group_of.contains_key(&d.key));
            let base = u16::from(loose);
            let column_of = |key: &DeviceKey| match group_of.get(key) {
                None => 0,
                Some(n) => (base + *n as u16).min(last),
            };
            let mut layout: Vec<(DeviceKey, u16)> = doc
                .devices
                .iter()
                .map(|d| (d.key.clone(), column_of(&d.key)))
                .collect();
            // Stable: members keep their relative order inside a column.
            layout.sort_by_key(|(_, c)| *c);
            let columns = layout.iter().map(|(_, c)| *c).max().map_or(1, |c| c + 1);
            Some((columns, layout))
        }
        RackSort::AutoGroup => {
            let mut layout: Vec<(DeviceKey, u16)> = Vec::with_capacity(doc.devices.len());
            let mut done: BTreeSet<usize> = BTreeSet::new();
            for d in &doc.devices {
                match group_of.get(&d.key) {
                    None => layout.push((d.key.clone(), d.column)),
                    Some(n) if done.insert(*n) => {
                        layout.extend(groups[*n].members.iter().map(|k| (k.clone(), d.column)));
                    }
                    Some(_) => {}
                }
            }
            Some((doc.columns, layout))
        }
    }
}

/// The edit that arranges `doc` for its sort mode, `None` when it already is (or the mode is
/// Free).
pub(crate) fn plan_arrange(doc: &RackDoc, registry: &Registry) -> Option<Edit> {
    let (columns, new) = arranged(doc, registry)?;
    let old: Vec<(DeviceKey, u16)> = doc
        .devices
        .iter()
        .map(|d| (d.key.clone(), d.column))
        .collect();
    if old == new && columns == doc.columns {
        return None;
    }
    let layout = Edit::Layout { old, new };
    let count = Edit::Swap(Swap::Columns {
        old: doc.columns,
        new: columns,
    });
    Some(match columns.cmp(&doc.columns) {
        std::cmp::Ordering::Equal => layout,
        std::cmp::Ordering::Greater => Edit::Compound(vec![count, layout]),
        std::cmp::Ordering::Less => Edit::Compound(vec![layout, count]),
    })
}

/// Whether `edit` adds, removes or moves a top-level device.
fn changes_devices(edit: &Edit) -> bool {
    match edit {
        Edit::Insert { .. } | Edit::Remove { .. } | Edit::Layout { .. } => true,
        Edit::Compound(parts) => parts.iter().any(changes_devices),
        _ => false,
    }
}

/// Appends the arranging layout to `edit` when the sort mode is not Free and `edit` changes the
/// devices, so both are one undo step.
pub(crate) fn finish(doc: &RackDoc, registry: &Registry, edit: Edit) -> Edit {
    if doc.sort_mode == RackSort::Free || !changes_devices(&edit) {
        return edit;
    }
    let mut scratch = doc.clone();
    if edit.perform(&mut scratch).is_err() {
        return edit;
    }
    match plan_arrange(&scratch, registry) {
        Some(more) => Edit::Compound(vec![edit, more]),
        None => edit,
    }
}

/// `Command::SetRackSort`: the mode changes and the rack is arranged for it in one step.
pub(crate) fn plan_set(
    doc: &RackDoc,
    registry: &Registry,
    mode: RackSort,
) -> Result<Edit, ModelError> {
    if doc.sort_mode == mode {
        return Err(ModelError::BadScope("the rack is sorted that way".into()));
    }
    let swap = Edit::Swap(Swap::RackSort {
        old: doc.sort_mode,
        new: mode,
    });
    let mut scratch = doc.clone();
    scratch.sort_mode = mode;
    Ok(match plan_arrange(&scratch, registry) {
        Some(more) => Edit::Compound(vec![swap, more]),
        None => swap,
    })
}

/// A move of devices takes what belongs to them along and never lands inside a stack: a
/// channel or bus takes the insert effects stacked under it (in every sort mode), and in a
/// sorted rack the whole groups of the moved devices go along: the keys of `command` grow to
/// every member of every group one of them is in. A move that would land between the members of
/// another strip's stack goes after the stack instead (an effect that joins the chain there, as
/// a routed move does, stays). Other commands are left alone.
pub(crate) fn expand_moves(doc: &RackDoc, registry: &Registry, command: &mut crate::Command) {
    let (keys, route) = match command {
        crate::Command::Workflow(
            WorkflowCommand::MoveDevices { keys, route, .. }
            | WorkflowCommand::MoveDevicesToSlot { keys, route, .. },
        ) => (keys, *route),
        _ => return,
    };
    if keys.iter().any(|k| doc.device(k).is_none()) {
        return;
    }
    let mut all: BTreeSet<DeviceKey> = keys.iter().cloned().collect();
    for k in keys.iter() {
        if doc.console.channel_index(k).is_some() || doc.console.bus_index(k).is_some() {
            all.extend(crate::insert_chain(doc, registry, k));
        }
    }
    if doc.sort_mode != RackSort::Free {
        for g in &doc.device_groups(registry) {
            if g.members.iter().any(|m| keys.contains(m)) {
                all.extend(g.members.iter().cloned());
            }
        }
    }
    if all.len() > keys.len() {
        *keys = doc
            .devices
            .iter()
            .map(|d| d.key.clone())
            .filter(|k| all.contains(k))
            .collect();
    }
    // A routed single effect may join a chain by landing inside its stack.
    let joins =
        route && keys.len() == 1 && crate::workflow_create::reroutable(doc, registry, &keys[0]);
    if joins {
        return;
    }
    let moving = keys.clone();
    match command {
        crate::Command::Workflow(WorkflowCommand::MoveDevices { to, .. }) => {
            let rest: Vec<&DeviceKey> = doc
                .devices
                .iter()
                .map(|d| &d.key)
                .filter(|k| !moving.contains(k))
                .collect();
            *to = past_stack(doc, registry, &rest, (*to).min(rest.len()));
        }
        crate::Command::Workflow(WorkflowCommand::MoveDevicesToSlot { slot, .. })
            if slot.column < doc.columns =>
        {
            let range = doc.column_range(slot.column);
            let rest: Vec<&DeviceKey> = doc.devices[range]
                .iter()
                .map(|d| &d.key)
                .filter(|k| !moving.contains(k))
                .collect();
            slot.index = past_stack(doc, registry, &rest, slot.index.min(rest.len()));
        }
        _ => {}
    }
}

/// `at` (an index into `rest`, the devices that stay) moved past the insert stack it falls in:
/// a place between a channel or bus and its inserts, or between two of them, becomes the place
/// after the last one.
fn past_stack(doc: &RackDoc, registry: &Registry, rest: &[&DeviceKey], at: usize) -> usize {
    let Some(next) = rest.get(at) else {
        return at;
    };
    let Some(strip) = at
        .checked_sub(1)
        .and_then(|i| stack_of(doc, registry, rest[i]))
    else {
        return at;
    };
    let chain = crate::insert_chain(doc, registry, &strip);
    if !chain.contains(next) {
        return at;
    }
    let mut end = at;
    while rest.get(end).is_some_and(|k| chain.contains(k)) {
        end += 1;
    }
    end
}

/// The channel or bus whose stack `key` belongs to: the strip itself or one of its inserts.
fn stack_of(doc: &RackDoc, registry: &Registry, key: &DeviceKey) -> Option<DeviceKey> {
    if doc.console.channel_index(key).is_some() || doc.console.bus_index(key).is_some() {
        return Some(key.clone());
    }
    crate::insert_owner(doc, registry, key)
}

/// Writes `"rack_sort"` (with its trailing comma and newline); nothing for Free.
pub(crate) fn write_sort(w: &mut String, mode: RackSort) {
    if mode != RackSort::Free {
        let _ = writeln!(w, "  \"rack_sort\": \"{}\",", mode.word());
    }
}

/// Reads the project's `"rack_sort"` word.
pub(crate) fn load_sort(v: &Value, path: &JsonPath) -> Result<RackSort, ModelError> {
    let word = string(v, path)?;
    RackSort::from_word(word)
        .ok_or_else(|| path.error(format!("\"{word}\" is not a rack sorting mode")))
}
