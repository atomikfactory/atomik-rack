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
//! A snapshot may also hold **every parameter of the effect devices** on the insert chains and
//! the returns ([`MixerState::effects`], chosen per snapshot with `SetSnapshotEffects`); without
//! it only their bypass is kept. It carries an optional **note** and **colour**, for the chip.
//!
//! It also holds the **wiring** ([`MixerState::wiring`], see [`crate::console_snapshot_wiring`]):
//! where each strip plays (the master, a bus or an Audio I/O pair) and which effects sit on its
//! insert chain, in order. A snapshot without it (made by an older build) leaves the routing and
//! the chains alone on recall.
//!
//! **What it does not hold**: the arrangement (clips, automation, tempo, markers) and the rest of
//! the structure of the rack (devices added or removed, other cables, the order and the names of
//! the strips). A recall moves the cables between the devices the rack has; it never adds or
//! removes one.
//!
//! A snapshot may name a **marker** (`SetSnapshotMarker`): the app recalls the snapshot when the
//! playhead crosses a marker of that name while the song plays (the name is matched ignoring
//! case; the arrangement is not touched).
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
//! **Morph.** `MorphSnapshots` sets the mix to a point between two snapshots: a continuous param
//! moves along its own taper (a dB param linearly in dB, a frequency in octaves), a switch or a
//! choice flips at the half way point, a VCA level moves in dB and the groups' mutes, solos and
//! members flip at the half way point. It is the same ordinary param edits as a recall and a pure
//! function of the two snapshots and the position, so the app plays a morph over a time by
//! applying successive positions, all folded into one undo step; position 1 is exactly a recall
//! of the second snapshot.
//!
//! `MorphFromMix` is the same with a mix the caller captured as the start, so a morph can begin
//! from the mix as it is, whether or not it matches a snapshot.
//!
//! **Compare** keeps the working mix in a hidden slot ([`CompareState`], not saved) while a
//! snapshot is recalled, and flips between the two. `CompareStrip` does the same for one strip
//! and the effects on its insert chain: the slot and the recall hold only those devices, so the
//! rest of the mix and the groups are not touched. The flip swaps the live mix with the slot, so
//! an edit made while one side is live belongs to that side and comes back when the side is
//! flipped to again; nothing is lost either way. The compare state is part of the undo history:
//! beginning, flipping and ending are one undo step each. Recalling, deleting the compared
//! snapshot or ending the compare keeps the side that is live.

use crate::console::{Console, ConsoleCommand, is_console_type, is_internal_param};
use crate::console_cmd::{Planner, bad};
use crate::console_insert::insert_chain;
use crate::console_snapshot_wiring::{self as wiring, SnapshotWiring, WiringChanges};
use crate::console_vca::{VcaGroup, canonical};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

/// Longest snapshot name, in characters.
pub const MAX_SNAPSHOT_NAME_CHARS: usize = 32;
/// Longest snapshot note, in characters.
pub const MAX_SNAPSHOT_NOTE_CHARS: usize = 200;
/// Most snapshots in a console.
pub const MAX_SNAPSHOTS: usize = 64;

/// The param of an insert or return effect that a snapshot keeps.
const BYPASS: &str = "bypass";

/// Categories of a recall: the bits of a [`SnapshotScope`].
pub(crate) mod bits {
    pub(crate) const LEVELS: u8 = 1;
    pub(crate) const MUTES: u8 = 2;
    pub(crate) const SENDS: u8 = 4;
    pub(crate) const INSERTS: u8 = 8;
    pub(crate) const PROCESSING: u8 = 16;
    pub(crate) const GROUPS: u8 = 32;
    pub(crate) const ROUTING: u8 = 64;
}

/// Which categories a recall applies. `LEVELS` is the fader, pan, width and input trim of the
/// strips and the VCA levels; `MUTES` the mute and solo of the strips and the groups; `SENDS` the
/// send switches, levels and pre/post; `INSERTS` the bypass of the insert effects and the insert
/// switches of the strips; `PROCESSING` every other param of a strip (EQ, dynamics, parallel
/// compressor, polarity, latency adjust, ...); `GROUPS` the membership of the VCA groups;
/// `ROUTING` the outputs of the strips and the effects on their insert chains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotScope(u8);

impl SnapshotScope {
    /// Everything.
    pub const ALL: SnapshotScope = SnapshotScope(127);
    /// Faders, pans, widths, input trims and the VCA levels.
    pub const LEVELS: SnapshotScope = SnapshotScope(bits::LEVELS);
    /// Mutes and solos, of strips and groups.
    pub const MUTES: SnapshotScope = SnapshotScope(bits::MUTES);
    /// The sends.
    pub const SENDS: SnapshotScope = SnapshotScope(bits::SENDS);
    /// The insert bypass.
    pub const INSERTS: SnapshotScope = SnapshotScope(bits::INSERTS);

    /// The output routing and the insert chain members.
    pub const ROUTING: SnapshotScope = SnapshotScope(bits::ROUTING);

    /// The scope named `name` (`all`, `levels`, `mutes`, `sends`, `inserts`, `routing`), ignoring
    /// case.
    pub fn from_name(name: &str) -> Option<SnapshotScope> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "all" => Self::ALL,
            "levels" | "faders" => Self::LEVELS,
            "mutes" | "solos" => Self::MUTES,
            "sends" => Self::SENDS,
            "inserts" | "bypass" => Self::INSERTS,
            "routing" | "wiring" => Self::ROUTING,
            _ => return None,
        })
    }

    /// The other processing of a strip: EQ, dynamics, parallel compressor, polarity, latency
    /// adjust and every param the other categories do not name.
    pub const PROCESSING: SnapshotScope = SnapshotScope(bits::PROCESSING);

    /// The categories of `self` and `other` together.
    #[must_use]
    pub const fn with(self, other: SnapshotScope) -> SnapshotScope {
        SnapshotScope(self.0 | other.0)
    }

    /// Whether the category of the strip param `param` is in the scope.
    pub fn covers_param(self, param: &str) -> bool {
        self.has(category(param))
    }

    /// Whether the insert chains are in the scope.
    pub fn covers_inserts(self) -> bool {
        self.has(bits::INSERTS)
    }

    pub(crate) fn has(self, bit: u8) -> bool {
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
    /// The effect devices hold all their params, not only the bypass.
    pub effects: bool,
    /// The output and the insert chain of each strip (see [`crate::console_snapshot_wiring`]);
    /// `None` for a state that holds none, which leaves the wiring alone on recall.
    pub wiring: Option<BTreeMap<DeviceKey, SnapshotWiring>>,
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
    /// A free note shown with the chip (at most [`MAX_SNAPSHOT_NOTE_CHARS`] characters; empty
    /// when there is none).
    pub note: String,
    /// The chip colour, `None` for the default.
    pub color: Option<[u8; 3]>,
    /// The name of a marker (compared ignoring case) that recalls the snapshot when the playhead
    /// crosses it while the song plays; `None` for no recall.
    pub marker: Option<String>,
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
    /// The strip the compare is about: only it (and its insert chain) is held and flipped, the
    /// rest of the mix stays as it is. `None` compares the whole mix.
    pub strip: Option<DeviceKey>,
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

    /// The first snapshot that recalls at the marker called `name` (ignoring case).
    pub fn snapshot_for_marker(&self, name: &str) -> Option<u32> {
        let wanted = name.trim().to_lowercase();
        self.snapshots
            .iter()
            .find(|s| {
                s.marker
                    .as_ref()
                    .is_some_and(|m| m.trim().to_lowercase() == wanted)
            })
            .map(|s| s.id)
    }

    /// The id the next snapshot gets (what `StoreSnapshot` will use).
    pub fn next_snapshot_id(&self) -> u32 {
        self.snapshots
            .iter()
            .map(|s| s.id)
            .max()
            .map_or(1, |m| m + 1)
    }

    /// The name the UI proposes for a new snapshot: the next free letter, `A` to `Z`, then
    /// `1`, `2`, ... It goes on after the last name of that run that is in use and after as many
    /// names as there are snapshots, so renaming `A` does not bring `A` back for the next one.
    pub fn default_snapshot_name(&self) -> String {
        let name_at = |i: usize| match u8::try_from(i) {
            Ok(i) if i < 26 => char::from(b'A' + i).to_string(),
            _ => (i - 25).to_string(),
        };
        let used = (0usize..)
            .take(26 + self.snapshots.len() + 1)
            .filter(|i| self.snapshot_by_name(&name_at(*i)).is_some())
            .max()
            .map_or(0, |i| i + 1);
        (used.max(self.snapshots.len())..)
            .map(name_at)
            .find(|n| self.snapshot_by_name(n).is_none())
            .expect("a free name exists")
    }
}

impl MixerState {
    /// The value this state holds for param `p` of `key`: `None` when the device is not in the
    /// state or an effect that holds only its bypass has no value for `p`.
    fn value(&self, key: &DeviceKey, p: &atomik_core::ParamDescriptor, strip: bool) -> Option<f32> {
        let stored = self.devices.get(key)?;
        if !strip && !self.effects && p.key != BYPASS {
            return None;
        }
        Some(p.clamp(stored.get(p.key).copied().unwrap_or(p.default)))
    }
}

/// A param value `at` (0..=1) of the way from `x` to `y`: along the param's own taper, a
/// switch or a choice staying at `x` until the half way point.
fn morph_value(p: &atomik_core::ParamDescriptor, x: f32, y: f32, at: f32) -> f32 {
    if at <= 0.0 {
        return x;
    }
    if at >= 1.0 {
        return y;
    }
    if p.discrete {
        return if at < 0.5 { x } else { y };
    }
    if p.taper == atomik_core::Taper::Exponential && p.min > 0.0 {
        let (nx, ny) = (p.to_normalized(x), p.to_normalized(y));
        return p.from_normalized(nx + (ny - nx) * at);
    }
    x + (y - x) * at
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
    /// The outputs and insert chains that change.
    wiring: WiringChanges,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.params.is_empty() && self.vcas.is_none() && self.wiring.is_empty()
    }
}

impl RackDoc {
    /// The mixer state now: see the module documentation for what it covers. The effect devices
    /// give their bypass only.
    pub fn capture_mixer(&self, registry: &Registry) -> MixerState {
        self.capture_mixer_with(registry, false)
    }

    /// The mixer state now, wiring included; with `effects` the effect devices give all their
    /// params.
    pub fn capture_mixer_with(&self, registry: &Registry, effects: bool) -> MixerState {
        self.capture_mixer_opts(registry, effects, true)
    }

    /// The mixer state now; `wiring` says whether it holds the outputs and insert chains.
    pub fn capture_mixer_opts(
        &self,
        registry: &Registry,
        effects: bool,
        wiring: bool,
    ) -> MixerState {
        let mut state = MixerState {
            effects,
            wiring: wiring.then(|| self.capture_wiring(registry)),
            ..MixerState::default()
        };
        let strips = self
            .console
            .channels
            .iter()
            .map(|c| &c.channel)
            .chain(self.console.buses.iter().map(|b| &b.bus))
            .chain(self.console.master.iter());
        let mut fx: Vec<DeviceKey> = Vec::new();
        for strip in strips {
            if let Some(params) = self.captured_params(registry, strip, true, effects) {
                state.devices.insert(strip.clone(), params);
                fx.extend(insert_chain(self, registry, strip));
            }
        }
        fx.extend(self.console.returns.iter().flatten().cloned());
        for key in fx {
            if let Some(params) = self.captured_params(registry, &key, false, effects) {
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
    /// strip's (`strip`), the bypass of an effect or, with `effects`, all of its params. `None`
    /// when the device or its type is unknown or an effect has no bypass.
    fn captured_params(
        &self,
        registry: &Registry,
        key: &DeviceKey,
        strip: bool,
        effects: bool,
    ) -> Option<BTreeMap<String, f32>> {
        let device = self.device(key)?;
        let desc = registry.get(&device.type_id)?.descriptor;
        if !strip && desc.param_by_key(BYPASS).is_none() {
            return None;
        }
        let mut out = BTreeMap::new();
        for p in kept_params(desc, strip, effects) {
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
            for p in kept_params(factory.descriptor, strip, state.effects) {
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
        let wiring = self.wiring_changes(registry, state, scope);
        Changes {
            params,
            vcas,
            wiring,
        }
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

    /// `state` cut down to the strip `strip` and the effects on its insert chain, with no groups:
    /// what a compare of one strip holds.
    fn strip_state(
        &self,
        registry: &Registry,
        mut state: MixerState,
        strip: &DeviceKey,
    ) -> MixerState {
        let mut keep: Vec<DeviceKey> = insert_chain(self, registry, strip).into_iter().collect();
        keep.push(strip.clone());
        state.devices.retain(|k, _| keep.contains(k));
        if let Some(w) = &mut state.wiring {
            w.retain(|k, _| k == strip);
        }
        state.vcas.clear();
        state
    }

    /// True when recalling snapshot `id` with `scope` would change nothing (the part of the mix
    /// it covers is as it was stored). `false` for an unknown snapshot.
    pub fn snapshot_matches(&self, id: u32, registry: &Registry, scope: SnapshotScope) -> bool {
        self.console
            .snapshot(id)
            .is_some_and(|s| self.recall_changes(registry, &s.state, scope).is_empty())
    }

    /// The mix at position `at` (0 is `a`, 1 is `b`) between two states: see the module
    /// documentation. Devices and groups only one of the two knows keep that one's values; a
    /// device that is gone is left out.
    pub fn morph_state(
        &self,
        registry: &Registry,
        a: &MixerState,
        b: &MixerState,
        at: f32,
    ) -> MixerState {
        let at = if at.is_nan() { 0.0 } else { at.clamp(0.0, 1.0) };
        let mut out = MixerState {
            effects: a.effects || b.effects,
            wiring: wiring::morph(&a.wiring, &b.wiring, at),
            ..MixerState::default()
        };
        let keys: std::collections::BTreeSet<&DeviceKey> =
            a.devices.keys().chain(b.devices.keys()).collect();
        for key in keys {
            let Some(device) = self.device(key) else {
                continue;
            };
            let Some(factory) = registry.get(&device.type_id) else {
                continue;
            };
            let strip = is_console_type(&device.type_id);
            let mut params = BTreeMap::new();
            for p in kept_params(factory.descriptor, strip, out.effects) {
                let value = match (a.value(key, p, strip), b.value(key, p, strip)) {
                    (Some(x), Some(y)) => morph_value(p, x, y, at),
                    (Some(x), None) | (None, Some(x)) => x,
                    (None, None) => continue,
                };
                let value = p.clamp(value);
                if value.to_bits() != p.default.to_bits() {
                    params.insert(p.key.to_owned(), value);
                }
            }
            out.devices.insert(key.clone(), params);
        }
        let ahead = at < 0.5;
        for g in &a.vcas {
            let Some(other) = b.vcas.iter().find(|o| o.id == g.id) else {
                out.vcas.push(g.clone());
                continue;
            };
            let near = if ahead { g } else { other };
            let (x, y) = (g.level_db, other.level_db);
            let level_db = if at <= 0.0 {
                x
            } else if at >= 1.0 {
                y
            } else {
                x + (y - x) * at
            };
            out.vcas.push(SnapshotVca {
                level_db,
                ..near.clone()
            });
        }
        out.vcas.extend(
            b.vcas
                .iter()
                .filter(|g| !a.vcas.iter().any(|o| o.id == g.id))
                .cloned(),
        );
        out
    }

    /// The state `MorphSnapshots { from, to, at }` sets; `None` for an unknown snapshot.
    fn morph_between(
        &self,
        registry: &Registry,
        from: u32,
        to: u32,
        at: f32,
    ) -> Option<MixerState> {
        let a = &self.console.snapshot(from)?.state;
        let b = &self.console.snapshot(to)?.state;
        Some(self.morph_state(registry, a, b, at))
    }

    /// True when `MorphFromMix { start, to, at }` would change nothing (or snapshot `to` is
    /// unknown): the app skips such steps while it plays a morph.
    pub fn morph_from_matches(
        &self,
        registry: &Registry,
        start: &MixerState,
        to: u32,
        at: f32,
    ) -> bool {
        self.console.snapshot(to).is_none_or(|b| {
            let state = self.morph_state(registry, start, &b.state, at);
            self.recall_changes(registry, &state, SnapshotScope::ALL)
                .is_empty()
        })
    }

    /// True when `MorphSnapshots { from, to, at }` would change nothing (or a snapshot is
    /// unknown): the app skips such steps while it plays a morph.
    pub fn morph_matches(&self, registry: &Registry, from: u32, to: u32, at: f32) -> bool {
        self.morph_between(registry, from, to, at)
            .is_none_or(|state| {
                self.recall_changes(registry, &state, SnapshotScope::ALL)
                    .is_empty()
            })
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
        notes.extend(self.wiring_notes(&snapshot.state));
        notes
    }
}

/// The descriptor params a snapshot keeps for a strip (every one but `internal.*`) or an effect
/// (its bypass, or with `effects` every one but `internal.*`).
pub(crate) fn kept_params(
    desc: &'static atomik_device_sdk::DeviceDescriptor,
    strip: bool,
    effects: bool,
) -> impl Iterator<Item = &'static atomik_core::ParamDescriptor> {
    desc.params.iter().filter(move |p| {
        if is_internal_param(p.key) {
            return false;
        }
        strip || effects || p.key == BYPASS
    })
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
    wiring::apply(p, changes.wiring);
}

/// Pushes the edit that sets the compare state to `new`.
fn set_compare(p: &mut Planner<'_>, new: Option<CompareState>) {
    let old = p.doc().console.compare.clone().map(Box::new);
    p.push(Edit::SnapshotCompare {
        old,
        new: new.map(Box::new),
    });
}

/// Flips an A/B compare: the live mix (of the compared strip only, for a strip compare) and the
/// hidden slot swap.
fn flip(p: &mut Planner<'_>, c: CompareState) {
    let registry = p.registry;
    let mut live = p
        .doc()
        .capture_mixer_opts(registry, c.other.effects, c.other.wiring.is_some());
    if let Some(strip) = &c.strip {
        live = p.doc().strip_state(registry, live, strip);
    }
    let changes = p
        .doc()
        .recall_changes(registry, &c.other, SnapshotScope::ALL);
    apply(p, changes);
    set_compare(
        p,
        Some(CompareState {
            snapshot: c.snapshot,
            other: live,
            on_snapshot: !c.on_snapshot,
            strip: c.strip,
        }),
    );
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
                list.push(MixerSnapshot {
                    id,
                    name,
                    state,
                    note: String::new(),
                    color: None,
                    marker: None,
                });
                Ok(())
            })
        }
        ConsoleCommand::UpdateSnapshot { id } => {
            let effects = snapshot_of(&p.doc().console, id)?.state.effects;
            let state = p.doc().capture_mixer_with(registry, effects);
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
        ConsoleCommand::SetSnapshotEffects { id, effects } => {
            let snapshot = snapshot_of(&p.doc().console, id)?;
            if p.doc()
                .console
                .compare
                .as_ref()
                .is_some_and(|c| c.snapshot == id)
            {
                return Err(bad("end the compare before changing the snapshot"));
            }
            if snapshot.state.effects == effects {
                return Err(bad(if effects {
                    "the snapshot already holds the effect parameters"
                } else {
                    "the snapshot holds no effect parameters"
                }));
            }
            let live = p.doc().capture_mixer_with(registry, true);
            let types: BTreeMap<DeviceKey, bool> = snapshot
                .state
                .devices
                .keys()
                .filter_map(|k| Some((k.clone(), is_console_type(&p.doc().device(k)?.type_id))))
                .collect();
            edit_list(p, |list| {
                let Some(s) = list.iter_mut().find(|s| s.id == id) else {
                    return Ok(());
                };
                s.state.effects = effects;
                // Turning it on takes the effects as they are now, the strips stay as stored;
                // turning it off keeps the bypass of each effect.
                for (key, params) in &mut s.state.devices {
                    if types.get(key).copied().unwrap_or(false) {
                        continue;
                    }
                    if !effects {
                        params.retain(|k, _| k == BYPASS);
                    } else if let Some(now) = live.devices.get(key) {
                        *params = now.clone();
                    }
                }
                Ok(())
            })
        }
        ConsoleCommand::SetSnapshotNote { id, note } => {
            let note = note.trim().to_owned();
            if note.chars().count() > MAX_SNAPSHOT_NOTE_CHARS {
                return Err(bad(format!(
                    "a snapshot note has at most {MAX_SNAPSHOT_NOTE_CHARS} characters"
                )));
            }
            if snapshot_of(&p.doc().console, id)?.note == note {
                return Err(bad("the snapshot already has that note"));
            }
            edit_list(p, |list| {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.note = note;
                }
                Ok(())
            })
        }
        ConsoleCommand::SetSnapshotColor { id, color } => {
            if snapshot_of(&p.doc().console, id)?.color == color {
                return Err(bad("the snapshot already has that colour"));
            }
            edit_list(p, |list| {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.color = color;
                }
                Ok(())
            })
        }
        ConsoleCommand::MorphSnapshots { from, to, at } => {
            if from == to {
                return Err(bad("a morph needs two different snapshots"));
            }
            if !at.is_finite() || !(0.0..=1.0).contains(&at) {
                return Err(bad("a morph position is between 0 and 1"));
            }
            if p.doc().console.compare.is_some() {
                return Err(bad("end the compare before morphing"));
            }
            let state = p
                .doc()
                .morph_between(registry, from, to, at)
                .ok_or_else(|| bad("a morph needs two existing snapshots"))?;
            let changes = p.doc().recall_changes(registry, &state, SnapshotScope::ALL);
            if changes.is_empty() {
                return Err(bad("the mix is already there"));
            }
            apply(p, changes);
            Ok(())
        }
        ConsoleCommand::MorphFromMix { start, to, at } => {
            if !at.is_finite() || !(0.0..=1.0).contains(&at) {
                return Err(bad("a morph position is between 0 and 1"));
            }
            if p.doc().console.compare.is_some() {
                return Err(bad("end the compare before morphing"));
            }
            let target = &snapshot_of(&p.doc().console, to)?.state;
            let state = p.doc().morph_state(registry, &start, target, at);
            let changes = p.doc().recall_changes(registry, &state, SnapshotScope::ALL);
            if changes.is_empty() {
                return Err(bad("the mix is already there"));
            }
            apply(p, changes);
            Ok(())
        }
        ConsoleCommand::SetSnapshotMarker { id, marker } => {
            let marker = marker.map(|m| m.trim().to_owned());
            if marker
                .as_ref()
                .is_some_and(|m| m.is_empty() || m.chars().count() > crate::MAX_MARKER_NAME)
            {
                return Err(bad("a marker name is not empty and at most 64 characters"));
            }
            if snapshot_of(&p.doc().console, id)?.marker == marker {
                return Err(bad("the snapshot already recalls at that marker"));
            }
            edit_list(p, |list| {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.marker = marker;
                }
                Ok(())
            })
        }
        ConsoleCommand::CompareStrip { id, strip } => {
            let snapshot = snapshot_of(&p.doc().console, id)?.state.clone();
            let master = p.doc().console.master.as_ref() == Some(&strip);
            if !master && !p.doc().is_console_strip(&strip) {
                return Err(bad("only a channel, a bus or the master can be compared"));
            }
            match p.doc().console.compare.clone() {
                None => {
                    if !snapshot.devices.contains_key(&strip) {
                        return Err(bad("the snapshot holds nothing of that strip"));
                    }
                    let held = p.doc().capture_mixer_with(registry, snapshot.effects);
                    let held = p.doc().strip_state(registry, held, &strip);
                    let theirs = p.doc().strip_state(registry, snapshot, &strip);
                    let changes = p
                        .doc()
                        .recall_changes(registry, &theirs, SnapshotScope::ALL);
                    apply(p, changes);
                    set_compare(
                        p,
                        Some(CompareState {
                            snapshot: id,
                            other: held,
                            on_snapshot: true,
                            strip: Some(strip),
                        }),
                    );
                }
                Some(c) if c.snapshot == id && c.strip.as_ref() == Some(&strip) => {
                    flip(p, c);
                }
                Some(_) => return Err(bad("end the compare before comparing another strip")),
            }
            Ok(())
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
                    let held = p.doc().capture_mixer_with(registry, snapshot.effects);
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
                            strip: None,
                        }),
                    );
                }
                Some(c) if c.snapshot == id => flip(p, c),
                Some(c) if c.strip.is_some() => {
                    return Err(bad(
                        "end the strip compare before comparing another snapshot",
                    ));
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
                            strip: None,
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
        assert_eq!(
            SnapshotScope::from_name("Routing"),
            Some(SnapshotScope::ROUTING)
        );
        for bit in [
            bits::LEVELS,
            bits::MUTES,
            bits::SENDS,
            bits::INSERTS,
            bits::PROCESSING,
            bits::GROUPS,
            bits::ROUTING,
        ] {
            assert!(SnapshotScope::ALL.has(bit));
        }
        assert!(!SnapshotScope::SENDS.has(bits::LEVELS));
    }

    #[test]
    fn default_names_run_a_to_z_then_numbers_and_never_go_back_to_a_renamed_name() {
        let mut c = Console::default();
        assert_eq!(c.default_snapshot_name(), "A");
        for (i, n) in ["A", "B", "1"].iter().enumerate() {
            c.snapshots.push(MixerSnapshot {
                id: i as u32 + 1,
                name: (*n).into(),
                state: MixerState::default(),
                note: String::new(),
                color: None,
                marker: None,
            });
        }
        assert_eq!(c.default_snapshot_name(), "2");
        c.snapshots.truncate(2);
        assert_eq!(c.default_snapshot_name(), "C");
        // The first one renamed: the next is not `A` again.
        c.snapshots[0].name = "Verse".into();
        assert_eq!(c.default_snapshot_name(), "C");
        c.snapshots.truncate(1);
        assert_eq!(c.default_snapshot_name(), "B");
    }
}
