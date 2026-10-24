//! Mixer snapshots: named copies of the console's state that can be recalled, and an A/B
//! comparison between the working mix and one of them.
//!
//! **What a snapshot holds** ([`MixerState`]): every param of every console strip (channels, output
//! buses and the master) except the app-driven `internal.*` ones, captured generically from the
//! device descriptors (fader, pan, width, mute, solo, input trim and polarity, the sends, the
//! dynamics, EQ and parallel compressor, the insert switches, latency adjust, whatever a strip
//! gets later), the `bypass` of every effect on a strip's insert chain and on an FX return, and the
//! VCA groups' level, mute, solo and membership. Params are stored sparsely: a param that is
//! missing from a snapshot is its default, as in a device's own params.
//!
//! **What it does not hold**: the arrangement (clips, automation, tempo, markers), the cables and
//! the structure of the rack (devices added, removed or re-cabled, the order and the names of the
//! strips, the output routing, the insert chains' members), and the parameters of the effect
//! devices themselves (only their bypass); including those is a follow-up (see the roadmap).
//!
//! **Drift.** A snapshot may name strips, effects and groups that were deleted since: recall skips
//! them ([`RackDoc::snapshot_notes`] lists what it skips), and a strip or group that the snapshot
//! never knew stays as it is. A VCA group is matched by id; a group that is gone is not brought
//! back, a member that is gone is left out, and a strip the snapshot knew that is no longer a
//! member of the group in the snapshot leaves it.
//!
//! **Recall** sets the stored params of the strips through ordinary `Edit::Param` steps (so the
//! app pushes them to the engine like any param edit and the engine's own 5 ms smoothers remove
//! the clicks) plus one `Edit::ConsoleVcas` when the groups differ; the whole recall is one undo
//! step. A [`SnapshotScope`] restricts it to some categories.
//!
//! **Compare** keeps the working mix in a hidden slot ([`CompareState`], not saved) while a
//! snapshot is recalled, and flips between the two. The flip swaps the live mix with the slot, so
//! an edit made while one side is live belongs to that side and comes back when the side is
//! flipped to again; nothing is lost either way. The compare state is part of the undo history:
//! beginning, flipping and ending are one undo step each. Recalling, deleting the compared
//! snapshot or ending the compare keeps the side that is live.

use crate::console::{Console, ConsoleCommand, is_console_type, is_internal_param};
use crate::console_cmd::{Planner, bad};
use crate::console_insert::insert_chain;
use crate::console_vca::{VcaGroup, canonical};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

/// Longest snapshot name, in characters.
pub const MAX_SNAPSHOT_NAME_CHARS: usize = 32;
/// Most snapshots in a console.
pub const MAX_SNAPSHOTS: usize = 16;

/// The param of an insert or return effect that a snapshot keeps.
const BYPASS: &str = "bypass";

/// Categories of a recall: the bits of a [`SnapshotScope`].
mod bits {
    pub(super) const LEVELS: u8 = 1;
    pub(super) const MUTES: u8 = 2;
    pub(super) const SENDS: u8 = 4;
    pub(super) const INSERTS: u8 = 8;
    pub(super) const PROCESSING: u8 = 16;
    pub(super) const GROUPS: u8 = 32;
}

/// Which categories a recall applies. `LEVELS` is the fader, pan, width and input trim of the
/// strips and the VCA levels; `MUTES` the mute and solo of the strips and the groups; `SENDS` the
/// send switches, levels and pre/post; `INSERTS` the bypass of the insert effects and the insert
/// switches of the strips; `PROCESSING` every other param of a strip (EQ, dynamics, parallel
/// compressor, polarity, latency adjust, ...); `GROUPS` the membership of the VCA groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotScope(u8);

impl SnapshotScope {
    /// Everything.
    pub const ALL: SnapshotScope = SnapshotScope(63);
    /// Faders, pans, widths, input trims and the VCA levels.
    pub const LEVELS: SnapshotScope = SnapshotScope(bits::LEVELS);
    /// Mutes and solos, of strips and groups.
    pub const MUTES: SnapshotScope = SnapshotScope(bits::MUTES);
    /// The sends.
    pub const SENDS: SnapshotScope = SnapshotScope(bits::SENDS);
    /// The insert bypass.
    pub const INSERTS: SnapshotScope = SnapshotScope(bits::INSERTS);

    /// The scope named `name` (`all`, `levels`, `mutes`, `sends`, `inserts`), ignoring case.
    pub fn from_name(name: &str) -> Option<SnapshotScope> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "all" => Self::ALL,
            "levels" | "faders" => Self::LEVELS,
            "mutes" | "solos" => Self::MUTES,
            "sends" => Self::SENDS,
            "inserts" | "bypass" => Self::INSERTS,
            _ => return None,
        })
    }

    fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// The category bit of a strip's param key.
fn category(param: &str) -> u8 {
    match param {
        "fader" | "pan" | "width" | "input.gain" => bits::LEVELS,
        "mute" | "solo" => bits::MUTES,
        p if p.starts_with("send") => bits::SENDS,
        p if p.starts_with("insert.") || p.starts_with("route.insert") => bits::INSERTS,
        _ => bits::PROCESSING,
    }
}

/// A VCA group as a snapshot keeps it (the colour and the name stay the group's own).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotVca {
    /// The group's id.
    pub id: u32,
    /// The group's name when the snapshot was taken (for the notes).
    pub name: String,
    /// The offset in dB.
    pub level_db: f32,
    /// The group's mute.
    pub mute: bool,
    /// The group's solo.
    pub solo: bool,
    /// The members, in console order.
    pub members: Vec<DeviceKey>,
}

/// The mixer state a snapshot (or the hidden compare slot) holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixerState {
    /// Each captured device with the params that are not at their default: the strips (all
    /// their params) and the insert and return effects (their `bypass`). A device with an empty
    /// map was captured with every param at its default.
    pub devices: BTreeMap<DeviceKey, BTreeMap<String, f32>>,
    /// The VCA groups, in the console's order.
    pub vcas: Vec<SnapshotVca>,
}

/// One stored snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct MixerSnapshot {
    /// The id (from 1, unique, never reused while the snapshot lives).
    pub id: u32,
    /// The name: trimmed, non-empty, at most [`MAX_SNAPSHOT_NAME_CHARS`] characters, unique among
    /// the snapshots (compared ignoring case).
    pub name: String,
    /// What it holds.
    pub state: MixerState,
}

/// An A/B comparison in progress (see the module documentation). Not saved.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareState {
    /// The snapshot being compared with.
    pub snapshot: u32,
    /// The side that is not live: the working mix while the snapshot is live, and the other way
    /// round.
    pub other: MixerState,
    /// True while the snapshot's side is live, false while the working mix is.
    pub on_snapshot: bool,
}

impl Console {
    /// The snapshot `id`.
    pub fn snapshot(&self, id: u32) -> Option<&MixerSnapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }

    /// The snapshot called `name` (trimmed, ignoring case).
    pub fn snapshot_by_name(&self, name: &str) -> Option<&MixerSnapshot> {
        let wanted = name.trim().to_lowercase();
        self.snapshots
            .iter()
            .find(|s| s.name.trim().to_lowercase() == wanted)
    }

    /// The id the next snapshot gets (what `StoreSnapshot` will use).
    pub fn next_snapshot_id(&self) -> u32 {
        self.snapshots
            .iter()
            .map(|s| s.id)
            .max()
            .map_or(1, |m| m + 1)
    }

    /// The name the UI proposes for a new snapshot: `A`, `B`, then `1`, `2`, ... the first one
    /// that is free.
    pub fn default_snapshot_name(&self) -> String {
        ["A".to_owned(), "B".to_owned()]
            .into_iter()
            .chain((1usize..).map(|n| n.to_string()))
            .find(|n| self.snapshot_by_name(n).is_none())
            .expect("a free name exists")
    }
}

/// One param a recall sets: the stored value before and after (none is the default).
struct ParamChange {
    key: DeviceKey,
    param: String,
    old: Option<f32>,
    new: Option<f32>,
    default: f32,
}

/// What recalling a state changes, computed without touching the document.
struct Changes {
    params: Vec<ParamChange>,
    /// The groups after the recall, when they differ from the live ones.
    vcas: Option<Vec<VcaGroup>>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.params.is_empty() && self.vcas.is_none()
    }
}

impl RackDoc {
    /// The mixer state now: see the module documentation for what it covers.
    pub fn capture_mixer(&self, registry: &Registry) -> MixerState {
        let mut state = MixerState::default();
        let strips = self
            .console
            .channels
            .iter()
            .map(|c| &c.channel)
            .chain(self.console.buses.iter().map(|b| &b.bus))
            .chain(self.console.master.iter());
        let mut effects: Vec<DeviceKey> = Vec::new();
        for strip in strips {
            if let Some(params) = self.captured_params(registry, strip, true) {
                state.devices.insert(strip.clone(), params);
                effects.extend(insert_chain(self, registry, strip));
            }
        }
        effects.extend(self.console.returns.iter().flatten().cloned());
        for key in effects {
            if let Some(params) = self.captured_params(registry, &key, false) {
                state.devices.entry(key).or_insert(params);
            }
        }
        state.vcas = self
            .console
            .vcas
            .iter()
            .map(|v| SnapshotVca {
                id: v.id,
                name: v.name.clone(),
                level_db: v.level_db,
                mute: v.mute,
                solo: v.solo,
                members: v.members.clone(),
            })
            .collect();
        state
    }

    /// The params of `key` that a snapshot keeps and that are not at their default: all of a
    /// strip's (`strip`), only the bypass of an effect. `None` when the device or its type is
    /// unknown or an effect has no bypass.
    fn captured_params(
        &self,
        registry: &Registry,
        key: &DeviceKey,
        strip: bool,
    ) -> Option<BTreeMap<String, f32>> {
        let device = self.device(key)?;
        let desc = registry.get(&device.type_id)?.descriptor;
        if !strip && desc.param_by_key(BYPASS).is_none() {
            return None;
        }
        let mut out = BTreeMap::new();
        for p in kept_params(desc, strip) {
            let v = p.clamp(device.params.get(p.key).copied().unwrap_or(p.default));
            if v.to_bits() != p.default.to_bits() {
                out.insert(p.key.to_owned(), v);
            }
        }
        Some(out)
    }

    /// What recalling `state` with `scope` would change. Entries for things that no longer exist
    /// are skipped; things the state does not know are left alone.
    fn recall_changes(
        &self,
        registry: &Registry,
        state: &MixerState,
        scope: SnapshotScope,
    ) -> Changes {
        let mut params = Vec::new();
        for (key, stored) in &state.devices {
            let Some(device) = self.device(key) else {
                continue;
            };
            let Some(factory) = registry.get(&device.type_id) else {
                continue;
            };
            let strip = is_console_type(&device.type_id);
            for p in kept_params(factory.descriptor, strip) {
                let bit = if strip {
                    category(p.key)
                } else {
                    bits::INSERTS
                };
                if !scope.has(bit) {
                    continue;
                }
                let target = p.clamp(stored.get(p.key).copied().unwrap_or(p.default));
                let new = (target.to_bits() != p.default.to_bits()).then_some(target);
                let old = device.params.get(p.key).copied();
                let now = p.clamp(old.unwrap_or(p.default));
                if now.to_bits() != target.to_bits() {
                    params.push(ParamChange {
                        key: key.clone(),
                        param: p.key.to_owned(),
                        old,
                        new,
                        default: p.default,
                    });
                }
            }
        }
        let vcas = self.recalled_vcas(state, scope);
        Changes { params, vcas }
    }

    /// The groups after recalling `state` with `scope`, or `None` when nothing changes.
    fn recalled_vcas(&self, state: &MixerState, scope: SnapshotScope) -> Option<Vec<VcaGroup>> {
        let levels = scope.has(bits::LEVELS);
        let mutes = scope.has(bits::MUTES);
        let groups = scope.has(bits::GROUPS);
        let mut out = self.console.vcas.clone();
        for snap in &state.vcas {
            let Some(g) = out.iter().position(|g| g.id == snap.id) else {
                continue;
            };
            if levels {
                out[g].level_db = snap.level_db;
            }
            if mutes {
                out[g].mute = snap.mute;
                out[g].solo = snap.solo;
            }
            if groups {
                let wanted: Vec<DeviceKey> = snap
                    .members
                    .iter()
                    .filter(|m| self.is_console_strip(m))
                    .cloned()
                    .collect();
                // A strip the snapshot knew leaves when it was not a member; a new strip stays.
                out[g]
                    .members
                    .retain(|m| !state.devices.contains_key(m) || wanted.contains(m));
                for m in wanted {
                    out.iter_mut().for_each(|o| o.members.retain(|x| *x != m));
                    out[g].members.push(m);
                }
            }
        }
        canonical(&self.console, &mut out);
        (out != self.console.vcas).then_some(out)
    }

    fn is_console_strip(&self, key: &DeviceKey) -> bool {
        self.console.channel_index(key).is_some() || self.console.bus_index(key).is_some()
    }

    /// True when recalling snapshot `id` with `scope` would change nothing (the part of the mix
    /// it covers is as it was stored). `false` for an unknown snapshot.
    pub fn snapshot_matches(&self, id: u32, registry: &Registry, scope: SnapshotScope) -> bool {
        self.console
            .snapshot(id)
            .is_some_and(|s| self.recall_changes(registry, &s.state, scope).is_empty())
    }

    /// What recalling snapshot `id` would skip because it no longer exists, one line each: the
    /// strips and effects, the VCA groups and the members of a group.
    pub fn snapshot_notes(&self, id: u32) -> Vec<String> {
        let Some(snapshot) = self.console.snapshot(id) else {
            return Vec::new();
        };
        let mut notes = Vec::new();
        for key in snapshot.state.devices.keys() {
            if self.device(key).is_none() {
                notes.push(format!("{key} no longer exists"));
            }
        }
        for v in &snapshot.state.vcas {
            if self.console.vca(v.id).is_none() {
                notes.push(format!("the VCA group {} no longer exists", v.name));
                continue;
            }
            for m in v.members.iter().filter(|m| !self.is_console_strip(m)) {
                notes.push(format!(
                    "{m} is no longer in the console (group {})",
                    v.name
                ));
            }
        }
        notes
    }
}

/// The descriptor params a snapshot keeps for a strip (every one but `internal.*`) or an effect
/// (its bypass).
fn kept_params(
    desc: &'static atomik_device_sdk::DeviceDescriptor,
    strip: bool,
) -> impl Iterator<Item = &'static atomik_core::ParamDescriptor> {
    desc.params
        .iter()
        .filter(move |p| (strip && !is_internal_param(p.key)) || (!strip && p.key == BYPASS))
}

/// A snapshot name checked for the snapshot `except` (none for a new one), trimmed.
fn checked_name(console: &Console, name: &str, except: Option<u32>) -> Result<String, ModelError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad("a snapshot name cannot be empty"));
    }
    if name.chars().count() > MAX_SNAPSHOT_NAME_CHARS {
        return Err(bad(format!(
            "a snapshot name has at most {MAX_SNAPSHOT_NAME_CHARS} characters"
        )));
    }
    if let Some(other) = console
        .snapshot_by_name(name)
        .filter(|s| except != Some(s.id))
    {
        return Err(bad(format!(
            "a snapshot is already called \"{}\"; snapshot names are unique",
            other.name
        )));
    }
    Ok(name.to_owned())
}

fn snapshot_of(console: &Console, id: u32) -> Result<&MixerSnapshot, ModelError> {
    console
        .snapshot(id)
        .ok_or_else(|| bad(format!("there is no snapshot {id}")))
}

/// Pushes one `ConsoleSnapshots` step built by `change` on the current list.
fn edit_list(
    p: &mut Planner<'_>,
    change: impl FnOnce(&mut Vec<MixerSnapshot>) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let old = p.doc().console.snapshots.clone();
    let mut new = old.clone();
    change(&mut new)?;
    p.push(Edit::ConsoleSnapshots { old, new });
    Ok(())
}

/// Pushes the edits of `changes`.
fn apply(p: &mut Planner<'_>, changes: Changes) {
    for c in changes.params {
        p.push(Edit::Param {
            key: c.key,
            param: c.param,
            old: c.old,
            new: c.new,
            default: c.default,
        });
    }
    if let Some(new) = changes.vcas {
        let old = p.doc().console.vcas.clone();
        p.push(Edit::ConsoleVcas { old, new });
    }
}

/// Pushes the edit that sets the compare state to `new`.
fn set_compare(p: &mut Planner<'_>, new: Option<CompareState>) {
    let old = p.doc().console.compare.clone().map(Box::new);
    p.push(Edit::SnapshotCompare {
        old,
        new: new.map(Box::new),
    });
}

/// Plans the snapshot commands (the arms of [`ConsoleCommand`] that are about snapshots).
pub(crate) fn plan(p: &mut Planner<'_>, command: ConsoleCommand) -> Result<(), ModelError> {
    let registry = p.registry;
    match command {
        ConsoleCommand::StoreSnapshot { name } => {
            let console = &p.doc().console;
            if console.snapshots.len() >= MAX_SNAPSHOTS {
                return Err(bad(format!(
                    "a console has at most {MAX_SNAPSHOTS} snapshots"
                )));
            }
            let name = checked_name(console, &name, None)?;
            let id = console.next_snapshot_id();
            let state = p.doc().capture_mixer(registry);
            edit_list(p, |list| {
                list.push(MixerSnapshot { id, name, state });
                Ok(())
            })
        }
        ConsoleCommand::UpdateSnapshot { id } => {
            snapshot_of(&p.doc().console, id)?;
            let state = p.doc().capture_mixer(registry);
            if p.doc().console.snapshot(id).map(|s| &s.state) == Some(&state) {
                return Err(bad("the snapshot already holds this mix"));
            }
            edit_list(p, |list| {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.state = state;
                }
                Ok(())
            })
        }
        ConsoleCommand::RecallSnapshot { id, scope } => {
            let state = snapshot_of(&p.doc().console, id)?.state.clone();
            let was_comparing = p.doc().console.compare.is_some();
            if was_comparing {
                set_compare(p, None);
            }
            let changes = p.doc().recall_changes(registry, &state, scope);
            if changes.is_empty() && !was_comparing {
                return Err(bad("the mix already matches that snapshot"));
            }
            apply(p, changes);
            Ok(())
        }
        ConsoleCommand::RenameSnapshot { id, name } => {
            let name = checked_name(&p.doc().console, &name, Some(id))?;
            snapshot_of(&p.doc().console, id)?;
            edit_list(p, |list| {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.name = name;
                }
                Ok(())
            })
        }
        ConsoleCommand::DeleteSnapshot { id } => {
            snapshot_of(&p.doc().console, id)?;
            if p.doc()
                .console
                .compare
                .as_ref()
                .is_some_and(|c| c.snapshot == id)
            {
                set_compare(p, None);
            }
            edit_list(p, |list| {
                list.retain(|s| s.id != id);
                Ok(())
            })
        }
        ConsoleCommand::MoveSnapshot { id, to } => {
            snapshot_of(&p.doc().console, id)?;
            edit_list(p, |list| {
                let before: Vec<u32> = list.iter().map(|s| s.id).collect();
                let from = before.iter().position(|i| *i == id).expect("checked");
                let item = list.remove(from);
                list.insert(to.min(list.len()), item);
                if list.iter().map(|s| s.id).eq(before) {
                    return Err(bad("the snapshot is already there"));
                }
                Ok(())
            })
        }
        ConsoleCommand::Compare { id } => {
            let snapshot = snapshot_of(&p.doc().console, id)?.state.clone();
            match p.doc().console.compare.clone() {
                None => {
                    let held = p.doc().capture_mixer(registry);
                    let changes = p
                        .doc()
                        .recall_changes(registry, &snapshot, SnapshotScope::ALL);
                    apply(p, changes);
                    set_compare(
                        p,
                        Some(CompareState {
                            snapshot: id,
                            other: held,
                            on_snapshot: true,
                        }),
                    );
                }
                Some(c) if c.snapshot == id => {
                    let live = p.doc().capture_mixer(registry);
                    let changes = p
                        .doc()
                        .recall_changes(registry, &c.other, SnapshotScope::ALL);
                    apply(p, changes);
                    set_compare(
                        p,
                        Some(CompareState {
                            snapshot: id,
                            other: live,
                            on_snapshot: !c.on_snapshot,
                        }),
                    );
                }
                // Another snapshot takes the place of the compared one: the snapshot side
                // changes, the working side stays where it is.
                Some(c) => {
                    let other = if c.on_snapshot {
                        let changes =
                            p.doc()
                                .recall_changes(registry, &snapshot, SnapshotScope::ALL);
                        apply(p, changes);
                        c.other
                    } else {
                        snapshot
                    };
                    set_compare(
                        p,
                        Some(CompareState {
                            snapshot: id,
                            other,
                            on_snapshot: c.on_snapshot,
                        }),
                    );
                }
            }
            Ok(())
        }
        ConsoleCommand::EndCompare => {
            if p.doc().console.compare.is_none() {
                return Err(bad("no snapshot is being compared"));
            }
            set_compare(p, None);
            Ok(())
        }
        other => unreachable!("{other:?} is not a snapshot command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_keys_fall_in_their_category() {
        assert_eq!(category("fader"), bits::LEVELS);
        assert_eq!(category("input.gain"), bits::LEVELS);
        assert_eq!(category("mute"), bits::MUTES);
        assert_eq!(category("send3.level"), bits::SENDS);
        assert_eq!(category("insert.bypass"), bits::INSERTS);
        assert_eq!(category("route.insert_pre"), bits::INSERTS);
        assert_eq!(category("pcomp.mix"), bits::PROCESSING);
        assert_eq!(category("eq.low.gain"), bits::PROCESSING);
    }

    #[test]
    fn scopes_are_named_and_all_has_every_category() {
        assert_eq!(
            SnapshotScope::from_name(" Levels "),
            Some(SnapshotScope::LEVELS)
        );
        assert_eq!(SnapshotScope::from_name("nope"), None);
        for bit in [
            bits::LEVELS,
            bits::MUTES,
            bits::SENDS,
            bits::INSERTS,
            bits::PROCESSING,
            bits::GROUPS,
        ] {
            assert!(SnapshotScope::ALL.has(bit));
        }
        assert!(!SnapshotScope::SENDS.has(bits::LEVELS));
    }

    #[test]
    fn default_names_run_a_b_then_numbers() {
        let mut c = Console::default();
        assert_eq!(c.default_snapshot_name(), "A");
        for (i, n) in ["A", "B", "1"].iter().enumerate() {
            c.snapshots.push(MixerSnapshot {
                id: i as u32 + 1,
                name: (*n).into(),
                state: MixerState::default(),
            });
        }
        assert_eq!(c.default_snapshot_name(), "2");
        c.snapshots.remove(0);
        assert_eq!(c.default_snapshot_name(), "A");
    }
}
