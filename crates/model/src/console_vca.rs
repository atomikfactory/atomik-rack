//! Mixer groups: VCA groups, the commands on them and the state the engine is fed.
//!
//! A VCA group ([`VcaGroup`]) is a master fader over a set of console strips (channels and
//! buses). It has **no audio and no routing**: it is console state only (the project format
//! stores it in `"console"."vcas"`), so a project without groups is exactly what it was.
//!
//! **Gain.** The group's `level_db` is an offset added to each member's own fader, a
//! multiplier on the member's fader gain ([`atomik_dsp::fader_gain_vca`] in the strip). The
//! members' faders keep their own position; what they play is `fader dB + level_db`, which the
//! console shows on the strip. `level_db` is in `VCA_MIN_DB..=VCA_MAX_DB`; the bottom is off
//! (members silent), 0 is the plain fader.
//!
//! **Mute and solo.** A member's *effective* flags are its own or its group's:
//! `effective_mute = mute || vca.mute`, `effective_solo = solo || vca.solo`. The strip's rules
//! then apply unchanged: solo beats mute (a member of a soloed group plays even if it, or the
//! group, is muted), and with any effective solo the strips that are neither soloed nor on the
//! path of a soloed strip are silenced ([`RackDoc::solo_silence`], which reads the effective
//! solo). Members stay individually controllable: their own M and S still work and combine by
//! the rule above. A soloed group with no members soloes nothing.
//!
//! **Membership.** A strip is in at most one group (assigning it to another moves it). Members
//! are kept in console order (channels, then buses), whatever order they were added in. Removing
//! a strip (a channel, a bus) takes it out of its group in the same undo step; an empty group
//! stays until it is deleted. Every command is one undo step on one [`Edit::ConsoleVcas`]; a
//! level drag carries a merge id like a fader drag.
//!
//! The group ids count from 1 and are never reused while the group lives (`max + 1`).

use crate::console::{Console, ConsoleCommand};
use crate::console_cmd::{Planner, bad};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_core::{VCA_MAX_DB, VCA_MIN_DB};
use atomik_device_sdk::Registry;
use std::collections::BTreeSet;

/// Longest VCA group name, in characters.
pub const MAX_VCA_NAME_CHARS: usize = 32;
/// Most VCA groups in a console.
pub const MAX_VCAS: usize = 64;

/// The colours new groups take in turn (group `id` takes entry `(id - 1) % 6`), the palette of the
/// markers and tracks: red, orange, yellow, green, blue, purple.
pub const VCA_PALETTE: [[u8; 3]; 6] = [
    [0xe0, 0x5a, 0x4f],
    [0xe8, 0x9a, 0x3c],
    [0xe0, 0xc8, 0x4a],
    [0x5a, 0xb8, 0x6a],
    [0x3f, 0x8f, 0xd2],
    [0x9a, 0x6a, 0xd0],
];

/// One VCA group.
#[derive(Debug, Clone, PartialEq)]
pub struct VcaGroup {
    /// The group's id (from 1, unique).
    pub id: u32,
    /// The name: trimmed, non-empty, at most [`MAX_VCA_NAME_CHARS`] characters, unique among the
    /// groups (compared ignoring case).
    pub name: String,
    /// The chip colour on the strips of the members and on the track headers, or none.
    pub color: Option<[u8; 3]>,
    /// The offset in dB added to the members' faders, within `VCA_MIN_DB..=VCA_MAX_DB`.
    pub level_db: f32,
    /// The group is muted: every member counts as muted.
    pub mute: bool,
    /// The group is soloed: every member counts as soloed.
    pub solo: bool,
    /// The member strips (console channels and buses), in console order, none in another group.
    pub members: Vec<DeviceKey>,
}

/// What the engine strip of a member is fed from its group (`internal.vca_*`); all at their
/// defaults for a strip outside any group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VcaFeed {
    /// `internal.vca_db`.
    pub db: f32,
    /// `internal.vca_mute`.
    pub mute: bool,
    /// `internal.vca_solo`.
    pub solo: bool,
}

impl VcaFeed {
    /// The feed of a strip outside any group: 0 dB, no mute, no solo.
    pub const NONE: VcaFeed = VcaFeed {
        db: 0.0,
        mute: false,
        solo: false,
    };

    /// True when the feed is the default (a strip that gets it is as if it had no group).
    pub fn is_none(&self) -> bool {
        self.db == 0.0 && !self.mute && !self.solo
    }
}

/// Clamps a VCA level to the range (NaN reads as 0 dB).
pub fn clamp_vca_db(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(VCA_MIN_DB, VCA_MAX_DB)
    }
}

impl Console {
    /// Index of the group `id`.
    pub fn vca_index(&self, id: u32) -> Option<usize> {
        self.vcas.iter().position(|v| v.id == id)
    }

    /// The group `id`.
    pub fn vca(&self, id: u32) -> Option<&VcaGroup> {
        self.vcas.iter().find(|v| v.id == id)
    }

    /// The group `strip` belongs to, if any.
    pub fn vca_of(&self, strip: &DeviceKey) -> Option<&VcaGroup> {
        self.vcas.iter().find(|v| v.members.contains(strip))
    }

    /// The group called `name` (trimmed, ignoring case).
    pub fn vca_by_name(&self, name: &str) -> Option<&VcaGroup> {
        let wanted = name.trim().to_lowercase();
        self.vcas
            .iter()
            .find(|v| v.name.trim().to_lowercase() == wanted)
    }

    /// The first of `VCA 1`, `VCA 2`, … that no group is called: the name the UI proposes.
    pub fn default_vca_name(&self) -> String {
        (1usize..)
            .map(|n| format!("VCA {n}"))
            .find(|n| self.vca_by_name(n).is_none())
            .expect("a free name exists")
    }

    /// The id the next group gets (what `CreateVca` will use).
    pub fn next_vca_id(&self) -> u32 {
        self.vcas.iter().map(|v| v.id).max().map_or(1, |m| m + 1)
    }

    /// What the strip `strip` is fed from its group (the default outside a group).
    pub fn vca_feed(&self, strip: &DeviceKey) -> VcaFeed {
        self.vca_of(strip).map_or(VcaFeed::NONE, |v| VcaFeed {
            db: v.level_db,
            mute: v.mute,
            solo: v.solo,
        })
    }

    /// The strips in console order: the channels, then the buses.
    pub fn strip_keys(&self) -> impl Iterator<Item = &DeviceKey> {
        self.channels
            .iter()
            .map(|c| &c.channel)
            .chain(self.buses.iter().map(|b| &b.bus))
    }

    fn is_strip(&self, key: &DeviceKey) -> bool {
        self.channel_index(key).is_some() || self.bus_index(key).is_some()
    }
}

impl RackDoc {
    /// The derived `internal.vca_*` feed of every console strip, channels then buses (the order
    /// of [`RackDoc::solo_silence`]), including the strips outside any group (they get
    /// [`VcaFeed::NONE`], which is how a strip that left a group is reset). The app pushes
    /// these values to the engine; a bounce sets those that are not the default.
    pub fn vca_feeds(&self) -> Vec<(DeviceKey, VcaFeed)> {
        self.console
            .strip_keys()
            .map(|k| (k.clone(), self.console.vca_feed(k)))
            .collect()
    }

    /// The level in dB a strip actually plays at: its own fader's dB plus its group's offset
    /// (`-inf` when either is off). `None` for a strip without a fader.
    pub fn effective_fader_db(&self, strip: &DeviceKey, registry: &Registry) -> Option<f32> {
        let pos = self.param_value(strip, "fader", registry)?;
        Some(fader_db_with_offset(pos, self.console.vca_feed(strip).db))
    }
}

/// The fader taper of `atomik-dsp` is not a dependency of the model, so the combination is
/// written here from the same cubic law: `gain = (pos / 0.75)³`, then the offset in dB.
fn fader_db_with_offset(pos: f32, vca_db: f32) -> f32 {
    let p = if pos.is_nan() {
        0.0
    } else {
        pos.clamp(0.0, 1.0)
    } / 0.75;
    let own = 60.0 * p.log10();
    if vca_db <= VCA_MIN_DB {
        f32::NEG_INFINITY
    } else {
        own + clamp_vca_db(vca_db)
    }
}

/// The edit that takes `key` out of its group when it is a member, so a removed strip does not
/// leave a stale member behind (and its undo puts it back).
pub(crate) fn leave_edit(console: &Console, key: &DeviceKey) -> Option<Edit> {
    console.vca_of(key)?;
    let old = console.vcas.clone();
    let mut new = old.clone();
    new.iter_mut().for_each(|v| v.members.retain(|m| m != key));
    Some(Edit::ConsoleVcas { old, new })
}

/// A group name checked for the group `except` (none for a new group), trimmed.
fn checked_name(console: &Console, name: &str, except: Option<u32>) -> Result<String, ModelError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad("a VCA name cannot be empty"));
    }
    if name.chars().count() > MAX_VCA_NAME_CHARS {
        return Err(bad(format!(
            "a VCA name has at most {MAX_VCA_NAME_CHARS} characters"
        )));
    }
    if let Some(other) = console.vca_by_name(name).filter(|v| except != Some(v.id)) {
        return Err(bad(format!(
            "a VCA is already called \"{}\"; VCA names are unique",
            other.name
        )));
    }
    Ok(name.to_owned())
}

/// Checks that `strips` are distinct console channels or buses and returns them without
/// repeats.
fn checked_strips(console: &Console, strips: &[DeviceKey]) -> Result<Vec<DeviceKey>, ModelError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for s in strips {
        if !console.is_strip(s) {
            return Err(bad(format!("{s} is not a console channel or bus")));
        }
        if seen.insert(s) {
            out.push(s.clone());
        }
    }
    Ok(out)
}

/// Puts the members of every group in console order.
fn canonical(console: &Console, groups: &mut [VcaGroup]) {
    let order: Vec<&DeviceKey> = console.strip_keys().collect();
    for g in groups {
        g.members
            .sort_by_key(|m| order.iter().position(|k| *k == m).unwrap_or(usize::MAX));
    }
}

/// Pushes one `ConsoleVcas` step built by `change` on the current groups; nothing is pushed
/// when `change` leaves them as they were.
fn edit_groups(
    p: &mut Planner<'_>,
    change: impl FnOnce(&mut Vec<VcaGroup>) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let old = p.doc().console.vcas.clone();
    let mut new = old.clone();
    change(&mut new)?;
    canonical(&p.doc().console, &mut new);
    if new != old {
        p.push(Edit::ConsoleVcas { old, new });
    }
    Ok(())
}

fn group_mut(groups: &mut [VcaGroup], id: u32) -> Result<&mut VcaGroup, ModelError> {
    groups
        .iter_mut()
        .find(|v| v.id == id)
        .ok_or_else(|| bad(format!("there is no VCA group {id}")))
}

/// Plans the VCA commands (the arms of [`ConsoleCommand`] that are about groups).
pub(crate) fn plan(p: &mut Planner<'_>, command: ConsoleCommand) -> Result<(), ModelError> {
    match command {
        ConsoleCommand::CreateVca { name, members } => {
            let console = &p.doc().console;
            if console.vcas.len() >= MAX_VCAS {
                return Err(bad(format!("a console has at most {MAX_VCAS} VCA groups")));
            }
            let name = checked_name(console, &name, None)?;
            let members = checked_strips(console, &members)?;
            let id = console.next_vca_id();
            edit_groups(p, |groups| {
                groups
                    .iter_mut()
                    .for_each(|g| g.members.retain(|m| !members.contains(m)));
                groups.push(VcaGroup {
                    id,
                    name,
                    color: Some(VCA_PALETTE[(id as usize - 1) % VCA_PALETTE.len()]),
                    level_db: 0.0,
                    mute: false,
                    solo: false,
                    members,
                });
                Ok(())
            })
        }
        ConsoleCommand::RenameVca { vca, name } => {
            let name = checked_name(&p.doc().console, &name, Some(vca))?;
            edit_groups(p, |groups| {
                group_mut(groups, vca)?.name = name;
                Ok(())
            })
        }
        ConsoleCommand::SetVcaColor { vca, color } => edit_groups(p, |groups| {
            group_mut(groups, vca)?.color = color;
            Ok(())
        }),
        ConsoleCommand::SetVcaLevel { vca, db, .. } => edit_groups(p, |groups| {
            group_mut(groups, vca)?.level_db = clamp_vca_db(db);
            Ok(())
        }),
        ConsoleCommand::SetVcaMute { vca, on } => edit_groups(p, |groups| {
            group_mut(groups, vca)?.mute = on;
            Ok(())
        }),
        ConsoleCommand::SetVcaSolo { vca, on } => edit_groups(p, |groups| {
            group_mut(groups, vca)?.solo = on;
            Ok(())
        }),
        ConsoleCommand::AssignToVca { vca, strips } => {
            let console = &p.doc().console;
            if let Some(id) = vca
                && console.vca(id).is_none()
            {
                return Err(bad(format!("there is no VCA group {id}")));
            }
            let strips = checked_strips(console, &strips)?;
            let before = console.vcas.clone();
            edit_groups(p, |groups| {
                groups
                    .iter_mut()
                    .for_each(|g| g.members.retain(|m| !strips.contains(m)));
                if let Some(id) = vca {
                    group_mut(groups, id)?.members.extend(strips);
                }
                Ok(())
            })?;
            if p.doc().console.vcas == before {
                return Err(bad(match vca {
                    Some(_) => "those strips are already in that VCA group",
                    None => "none of those strips is in a VCA group",
                }));
            }
            Ok(())
        }
        ConsoleCommand::DeleteVca { vca } => edit_groups(p, |groups| {
            let index = groups
                .iter()
                .position(|v| v.id == vca)
                .ok_or_else(|| bad(format!("there is no VCA group {vca}")))?;
            groups.remove(index);
            Ok(())
        }),
        other => unreachable!("{other:?} is not a VCA command"),
    }
}

/// Clears the mute (`"mute"`) or solo (`"solo"`) of every group, for `ClearMutes` and
/// `ClearSolos`.
pub(crate) fn clear_flags(p: &mut Planner<'_>, param: &str) -> Result<(), ModelError> {
    edit_groups(p, |groups| {
        for g in groups {
            match param {
                "mute" => g.mute = false,
                _ => g.solo = false,
            }
        }
        Ok(())
    })
}

/// True when strip `key` counts as soloed: its own `solo` or its group's.
pub(crate) fn effective_solo(doc: &RackDoc, key: &DeviceKey, registry: &Registry) -> bool {
    doc.param_value(key, "solo", registry)
        .is_some_and(|v| v >= 0.5)
        || doc.console.vca_feed(key).solo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_clamps_and_nan_is_zero() {
        assert_eq!(clamp_vca_db(f32::NAN), 0.0);
        assert_eq!(clamp_vca_db(-90.0), VCA_MIN_DB);
        assert_eq!(clamp_vca_db(40.0), VCA_MAX_DB);
        assert_eq!(clamp_vca_db(-6.0), -6.0);
    }

    #[test]
    fn the_effective_level_adds_the_offset_in_db() {
        let unity = fader_db_with_offset(0.75, 0.0);
        assert!(unity.abs() < 1e-5);
        assert!((fader_db_with_offset(0.75, -6.0) + 6.0).abs() < 1e-4);
        let half = fader_db_with_offset(0.5, 0.0);
        assert!((fader_db_with_offset(0.5, 3.0) - (half + 3.0)).abs() < 1e-4);
        assert_eq!(fader_db_with_offset(0.75, VCA_MIN_DB), f32::NEG_INFINITY);
        assert_eq!(fader_db_with_offset(0.0, 0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn the_default_feed_is_none() {
        assert!(VcaFeed::NONE.is_none());
        assert!(
            !VcaFeed {
                db: -1.0,
                ..VcaFeed::NONE
            }
            .is_none()
        );
    }
}
