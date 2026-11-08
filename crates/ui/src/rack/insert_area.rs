//! The insert area of a mix channel or output bus in the rack: the insert effects stacked
//! inside a frame under the strip's panel.
//!
//! The effects of a strip's insert chain ([`atomik_model::insert_chain`], derived from cables)
//! are drawn **stacked directly under the strip's panel**, in chain order, in the strip's
//! column, inside a frame in the strip's accent colour in the margin beside the column (like the
//! inside of an unfolded container). The frame ends with an empty **drop row**, "drop or create
//! an effect here". With SHOW INSERT FX off the stack is hidden and the frame shrinks to one
//! **header line** that names the effects; a click on it shows them again.
//!
//! The layout is the view's: [`RackView::shown`](super::RackView) lists a strip's inserts right
//! after it whatever the document order is (the model keeps them stacked, a project made by
//! hand may not), and the stack is cut at the first insert that sits in another column. The
//! area and its rows are not devices: the view works out their rectangles after the layout
//! ([`InsertArea`]).
//!
//! Gestures, each one undo step through the planned commands of the model:
//!
//! * a device dragged from the rack into the frame is added to the chain at the place under the
//!   pointer ([`ConsoleCommand::AddInsert`]); one that already is an insert of the same strip
//!   is moved in the chain ([`ConsoleCommand::MoveInsert`]); a device the chain cannot take
//!   is refused with the model's reason in the status line;
//! * a browser row dropped in the frame creates the effect there (`CreateEffectAt` at the slot
//!   under the effect above the pointer, which the model routes into the chain);
//! * an insert dragged out of the frame to the plain rack leaves the chain, which closes behind
//!   it ([`ConsoleCommand::ExtractInsert`]); the effect stays in the rack, uncabled;
//! * a click on the header line shows or hides the stack; a click on the drop row selects the
//!   strip, so that creating an effect appends it to the chain.
//!
//! Moving a strip moves its stack along: the drag moves the strip together with its inserts.
//!
//! The master section has the same insert area, and under it the effects of the shared sends:
//! each send (`Console::returns`, the chain cabled from `fx_send{k}`) is its own group under the
//! master's area, framed and labelled "Send N" ([`SendGroup`]), in send order. The send effects
//! are ordinary devices; the groups have no drop row (a send effect is made from the master
//! panel's selection or by "Use as Send Effect") and move with the master.

use super::columns::column_left;
use super::inline::is_inner;
use super::{Hit, RackView, UiAction};
use crate::browser::BrowserRow;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::mix_panel::has_mix_top;
use crate::paint::fit_label;
use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateTarget, DeviceKey, FX_SLOTS, InsertEffect,
    RackDevice, RackDoc, RackSlot, WorkflowCommand, insert_chain, send_chain,
};
use std::collections::{BTreeMap, BTreeSet};
use vello::kurbo::{Line, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

/// Height of the header line of a folded insert area, rack units of length.
pub(super) const HEADER_H: f64 = 18.0;
/// Height of the drop row that ends an unfolded insert area.
pub(super) const DROP_H: f64 = 24.0;
/// How far the frame sits outside the column.
const GROW: f64 = 10.0;
/// Height of the header line of a send group.
pub(super) const SEND_HEADER_H: f64 = 20.0;

/// The insert area of one strip, as laid out.
#[derive(Debug, Clone)]
pub(super) struct InsertArea {
    /// The channel or bus.
    pub owner: DeviceKey,
    /// The stacked inserts in chain order (drawn while the area is shown).
    pub members: Vec<DeviceKey>,
    /// For the master: the effects of its send groups, which follow the area and move with it.
    pub tail: Vec<DeviceKey>,
    /// Their names, for the header line.
    pub names: Vec<String>,
    /// Whether the stack is shown (SHOW INSERT FX).
    pub shown: bool,
    /// The area under the strip's panel: the stacked inserts and the row, one panel wide.
    pub rect: Rect,
    /// The header line (folded) or the drop row (shown).
    pub row: Rect,
    /// The strip's accent colour.
    pub accent: Color,
}

/// The effects of one shared send, grouped under the master section.
#[derive(Debug, Clone)]
pub(super) struct SendGroup {
    /// The send, 0-based.
    pub slot: usize,
    /// The effects of the send, in signal order.
    pub members: Vec<DeviceKey>,
    /// Their names, for the header.
    pub names: Vec<String>,
    /// The header line ("Send N" and the names), above the first effect.
    pub header: Rect,
    /// The whole group: header and effects, one panel wide.
    pub rect: Rect,
    /// The colour of the frame.
    pub accent: Color,
}

impl SendGroup {
    /// The frame drawn around the group.
    pub fn outline(&self) -> Rect {
        self.rect.inflate(GROW, 0.0).inflate(0.0, 2.0)
    }
}

/// What a test (or a host) can read of a send group.
#[derive(Debug, Clone, PartialEq)]
pub struct SendGroupInfo {
    /// The send, 0-based.
    pub slot: usize,
    /// The effects of the send, in signal order.
    pub members: Vec<DeviceKey>,
    /// The header line, rack coordinates.
    pub header: Rect,
    /// The frame around the group, rack coordinates.
    pub frame: Rect,
}

impl InsertArea {
    /// The frame drawn around [`Self::rect`].
    pub fn outline(&self) -> Rect {
        self.rect.inflate(GROW, 0.0).inflate(0.0, 2.0)
    }
}

/// What a test (or a host) can read of an insert area.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertAreaInfo {
    /// The channel or bus.
    pub owner: DeviceKey,
    /// The stacked inserts, in chain order.
    pub members: Vec<DeviceKey>,
    /// Whether the stack is shown.
    pub shown: bool,
    /// The frame around the area, rack coordinates.
    pub frame: Rect,
    /// The header line (folded) or the drop row (shown), rack coordinates.
    pub row: Rect,
}

impl RackView {
    /// The stacks of the document: for every channel or bus with an insert chain, the effects of
    /// the chain that are drawn under it (the chain up to the first effect in another column).
    /// Empty inside a container.
    pub(super) fn stacks(
        &self,
        doc: &RackDoc,
        registry: &Registry,
    ) -> BTreeMap<DeviceKey, Vec<DeviceKey>> {
        let mut out = BTreeMap::new();
        if !self.scope.is_empty() {
            return out;
        }
        let sending: BTreeSet<&DeviceKey> = doc
            .cables
            .iter()
            .filter(|c| c.from.socket == "insert_send_l")
            .map(|c| &c.from.device)
            .collect();
        if sending.is_empty() {
            return out;
        }
        let mut taken: BTreeSet<DeviceKey> = BTreeSet::new();
        for d in doc
            .devices
            .iter()
            .filter(|d| has_mix_top(&d.type_id) && sending.contains(&d.key))
        {
            let members: Vec<DeviceKey> = insert_chain(doc, registry, &d.key)
                .into_iter()
                .take_while(|k| {
                    !is_inner(k)
                        && !taken.contains(k)
                        && doc.device(k).is_some_and(|m| m.column == d.column)
                })
                .collect();
            if !members.is_empty() {
                taken.extend(members.iter().cloned());
                out.insert(d.key.clone(), members);
            }
        }
        out
    }

    /// The send groups of the document: for every shared send with an effect, its effects up to
    /// the first one that sits in another column than the master (or is a member of a stack
    /// already), in send order. Empty inside a container and without a master.
    pub(super) fn send_stacks(
        &self,
        doc: &RackDoc,
        registry: &Registry,
        stacks: &BTreeMap<DeviceKey, Vec<DeviceKey>>,
    ) -> Vec<(usize, Vec<DeviceKey>)> {
        let Some(master) = doc
            .console
            .master
            .as_ref()
            .filter(|_| self.scope.is_empty())
        else {
            return Vec::new();
        };
        let Some(column) = doc.device(master).map(|d| d.column) else {
            return Vec::new();
        };
        let mut taken: BTreeSet<DeviceKey> = stacks.values().flatten().cloned().collect();
        let mut out = Vec::new();
        for slot in 0..FX_SLOTS {
            let members: Vec<DeviceKey> = send_chain(doc, registry, slot)
                .into_iter()
                .take_while(|k| {
                    !is_inner(k)
                        && !taken.contains(k)
                        && doc.device(k).is_some_and(|m| m.column == column)
                })
                .collect();
            if !members.is_empty() {
                taken.extend(members.iter().cloned());
                out.push((slot, members));
            }
        }
        out
    }

    /// The devices the rack draws, in rack order: the document order with every stack right
    /// under its strip (a folded strip leaves its stack out) and the send groups right under the
    /// master's stack.
    pub(super) fn displayed<'a>(
        &self,
        doc: &'a RackDoc,
        registry: &Registry,
    ) -> Vec<&'a RackDevice> {
        let stacks = self.stacks(doc, registry);
        let sends = self.send_stacks(doc, registry, &stacks);
        if stacks.is_empty() && sends.is_empty() {
            return doc.devices.iter().collect();
        }
        let members: BTreeSet<&DeviceKey> = stacks
            .values()
            .flatten()
            .chain(sends.iter().flat_map(|(_, m)| m))
            .collect();
        let mut out = Vec::with_capacity(doc.devices.len());
        for d in &doc.devices {
            if members.contains(&d.key) {
                continue;
            }
            out.push(d);
            if let Some(stack) = stacks.get(&d.key)
                && self.inserts_shown(&d.key)
            {
                out.extend(stack.iter().filter_map(|k| doc.device(k)));
            }
            if doc.console.master.as_ref() == Some(&d.key) {
                for (_, group) in &sends {
                    out.extend(group.iter().filter_map(|k| doc.device(k)));
                }
            }
        }
        out
    }

    /// The room each strip's area takes after a device: the header line under a folded strip,
    /// the drop row under the last effect of a shown stack (or the strip itself when it has
    /// none). A strip without an insert chain has no area (nothing to drop on is shown for it:
    /// the drop row of an empty chain appears once something is cabled).
    pub(super) fn area_gaps(
        &self,
        doc: &RackDoc,
        stacks: &BTreeMap<DeviceKey, Vec<DeviceKey>>,
        sends: &[(usize, Vec<DeviceKey>)],
    ) -> BTreeMap<DeviceKey, f64> {
        let mut gaps = BTreeMap::new();
        // The device right above the first send group: the end of the master's area.
        let mut above = None;
        for d in doc.devices.iter().filter(|d| has_mix_top(&d.type_id)) {
            if !self.has_area(doc, &d.key) {
                continue;
            }
            let end = if self.inserts_shown(&d.key) {
                let last = stacks
                    .get(&d.key)
                    .and_then(|s| s.last())
                    .unwrap_or(&d.key)
                    .clone();
                gaps.insert(last.clone(), DROP_H);
                last
            } else {
                gaps.insert(d.key.clone(), HEADER_H);
                d.key.clone()
            };
            if doc.console.master.as_ref() == Some(&d.key) {
                above = Some(end);
            }
        }
        // Each send group has a header line above its first effect.
        for (_, members) in sends {
            if let Some(prev) = above.take() {
                *gaps.entry(prev).or_insert(0.0) += SEND_HEADER_H;
            }
            above = members.last().cloned();
        }
        gaps
    }

    /// Whether the strip `key` has an insert area: every top-level channel or bus does, so a
    /// drop row is always there to take an effect (not inside a container).
    fn has_area(&self, doc: &RackDoc, key: &DeviceKey) -> bool {
        self.scope.is_empty()
            && !is_inner(key)
            && doc.device(key).is_some_and(|d| has_mix_top(&d.type_id))
    }

    /// Works out the areas from the laid-out views (see [`InsertArea`]).
    pub(super) fn compute_areas(
        &mut self,
        doc: &RackDoc,
        stacks: &BTreeMap<DeviceKey, Vec<DeviceKey>>,
        sends: &[(usize, Vec<DeviceKey>)],
    ) {
        self.areas.clear();
        self.stacked = stacks
            .values()
            .flatten()
            .chain(sends.iter().flat_map(|(_, m)| m))
            .cloned()
            .collect();
        self.compute_sends(doc, sends);
        for d in doc.devices.iter().filter(|d| has_mix_top(&d.type_id)) {
            if !self.has_area(doc, &d.key) {
                continue;
            }
            let Some(head) = self.devices.iter().find(|v| v.key == d.key) else {
                continue;
            };
            let members = stacks.get(&d.key).cloned().unwrap_or_default();
            let names = members
                .iter()
                .filter_map(|k| doc.device(k).map(|m| m.name.clone()))
                .collect();
            let shown = self.inserts_shown(&d.key);
            let top = head.y + head.height;
            let bottom = if shown {
                members
                    .last()
                    .and_then(|k| self.devices.iter().find(|v| v.key == *k))
                    .map_or(top, |v| v.y + v.height)
                    + DROP_H
            } else {
                top + HEADER_H
            };
            let width = f64::from(RACK_WIDTH);
            let rect = Rect::new(head.x, top, head.x + width, bottom);
            let row = if shown {
                Rect::new(head.x, bottom - DROP_H, head.x + width, bottom)
            } else {
                Rect::new(head.x, top, head.x + width, top + HEADER_H)
            };
            let tail = if doc.console.master.as_ref() == Some(&d.key) {
                sends.iter().flat_map(|(_, m)| m.iter().cloned()).collect()
            } else {
                Vec::new()
            };
            self.areas.push(InsertArea {
                owner: d.key.clone(),
                members,
                tail,
                names,
                shown,
                rect,
                row,
                accent: head.accent,
            });
        }
    }

    /// Works out the send groups from the laid-out views (see [`SendGroup`]).
    fn compute_sends(&mut self, doc: &RackDoc, sends: &[(usize, Vec<DeviceKey>)]) {
        self.sends.clear();
        let width = f64::from(RACK_WIDTH);
        for (slot, members) in sends {
            let view = |k: &DeviceKey| self.devices.iter().find(|v| v.key == *k);
            let (Some(first), Some(last)) = (
                members.first().and_then(view),
                members.last().and_then(view),
            ) else {
                continue;
            };
            let header = Rect::new(first.x, first.y - SEND_HEADER_H, first.x + width, first.y);
            let rect = Rect::new(first.x, header.y0, first.x + width, last.y + last.height);
            let names = members
                .iter()
                .filter_map(|k| doc.device(k).map(|m| m.name.clone()))
                .collect();
            self.sends.push(SendGroup {
                slot: *slot,
                members: members.clone(),
                names,
                header,
                rect,
                accent: first.accent,
            });
        }
    }

    /// The send groups under the master section, in send order (frames and headers in rack
    /// coordinates).
    pub fn send_groups(&self) -> Vec<SendGroupInfo> {
        self.sends
            .iter()
            .map(|g| SendGroupInfo {
                slot: g.slot,
                members: g.members.clone(),
                header: g.header,
                frame: g.outline(),
            })
            .collect()
    }

    /// What creating an effect with `selected` selected does: the master selected after a click
    /// on its insert area makes an insert effect, every other selection is the model's call.
    pub(super) fn effect_target(&self, selected: DeviceKey) -> CreateTarget {
        let master_area = self.area_pick.as_ref() == Some(&selected);
        if master_area {
            CreateTarget::MasterInserts
        } else {
            CreateTarget::Selected(selected)
        }
    }

    /// Whether the laid-out areas still say what the document does: the same strips with the
    /// same stacked effects under the same names (a hidden stack changes no device row, so the
    /// layout check alone would miss it).
    pub(super) fn areas_current(&self, doc: &RackDoc, registry: &Registry) -> bool {
        let stacks = self.stacks(doc, registry);
        let sends = self.send_stacks(doc, registry, &stacks);
        let owners: Vec<&RackDevice> = doc
            .devices
            .iter()
            .filter(|d| self.has_area(doc, &d.key))
            .collect();
        sends.len() == self.sends.len()
            && sends.iter().zip(&self.sends).all(|((slot, members), g)| {
                g.slot == *slot
                    && g.members == *members
                    && members
                        .iter()
                        .zip(&g.names)
                        .all(|(m, n)| doc.device(m).is_some_and(|x| x.name == *n))
            })
            && owners.len() == self.areas.len()
            && owners.iter().zip(&self.areas).all(|(d, a)| {
                let members = stacks.get(&d.key).map_or(&[][..], |m| &m[..]);
                a.owner == d.key
                    && a.members == members
                    && a.shown == self.inserts_shown(&d.key)
                    && a.names.len() == members.len()
                    && members
                        .iter()
                        .zip(&a.names)
                        .all(|(m, n)| doc.device(m).is_some_and(|x| x.name == *n))
            })
    }

    /// The insert areas of the rack, in rack order (frames and rows in rack coordinates).
    pub fn insert_areas(&self) -> Vec<InsertAreaInfo> {
        self.areas
            .iter()
            .map(|a| InsertAreaInfo {
                owner: a.owner.clone(),
                members: a.members.clone(),
                shown: a.shown,
                frame: a.outline(),
                row: a.row,
            })
            .collect()
    }

    /// The centre of the header line or the drop row of the strip `key`, in view coordinates.
    pub fn insert_row_center(&self, key: &DeviceKey) -> Option<Point> {
        let a = self.areas.iter().find(|a| a.owner == *key)?;
        Some(self.rack_to_view(a.row.center()))
    }

    /// Shows the insert effect `effect` in the rack: its strip's insert area is unfolded, the
    /// effect is selected and the rack scrolls to it. Returns whether the effect is in an
    /// insert stack the rack draws.
    pub fn reveal_insert(
        &mut self,
        doc: &RackDoc,
        registry: &Registry,
        effect: &DeviceKey,
    ) -> bool {
        let Some(owner) = atomik_model::insert_owner(doc, registry, effect) else {
            return false;
        };
        self.set_inserts_shown(&owner, true);
        self.sync(doc, registry);
        self.set_selected(Some(effect.clone()));
        self.reveal_key(effect)
    }

    /// The tree entries of the areas: the row of each (header line or drop row).
    pub(super) fn area_hits(&self) -> Vec<(Rect, Hit)> {
        self.areas
            .iter()
            .filter_map(|a| {
                let i = self.devices.iter().position(|d| d.key == a.owner)?;
                Some((a.row, Hit::AreaRow(i)))
            })
            .collect()
    }

    /// A press on the row of the area of strip `i`: the header line shows or hides the stack, the
    /// drop row selects the strip.
    pub(super) fn area_row_down(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let key = self.devices[i].key.clone();
        if !self.inserts_shown(&key) {
            self.set_inserts_shown(&key, true);
        }
        // The master is a send when selected; its insert area says "insert".
        self.area_pick = self.devices[i].is_master().then_some(key);
    }

    /// The tooltip of an area row.
    pub(super) fn area_tip(&self, i: usize) -> Option<String> {
        let key = &self.devices.get(i)?.key;
        let a = self.areas.iter().find(|a| a.owner == *key)?;
        Some(if a.shown {
            "Drop or create an effect here: it joins the insert effects of this strip".to_string()
        } else {
            "The insert effects are hidden. Click to show them".to_string()
        })
    }

    /// The area whose frame holds the rack point `rp`, not counting the areas of the devices
    /// being moved.
    pub(super) fn area_at(&self, rp: Point, moving: &[DeviceKey]) -> Option<usize> {
        self.areas
            .iter()
            .position(|a| !moving.contains(&a.owner) && a.outline().contains(rp))
    }

    /// Where a drop at `rp` lands in area `ai`: the chain position (the stacked effects above
    /// the pointer that stay, so also the final index of a moved insert) and the height of the
    /// drop line. A folded area appends.
    pub(super) fn area_slot(&self, ai: usize, rp: Point, moving: &[DeviceKey]) -> (usize, f64) {
        let a = &self.areas[ai];
        let head = self
            .devices
            .iter()
            .find(|v| v.key == a.owner)
            .map_or(a.rect.y0, |v| v.y + v.height);
        if !a.shown {
            let stay = a.members.iter().filter(|m| !moving.contains(m)).count();
            return (stay, a.row.y1);
        }
        let mut pos = 0;
        let mut line = head;
        for m in a.members.iter().filter(|m| !moving.contains(m)) {
            let Some(v) = self.devices.iter().find(|v| v.key == *m) else {
                continue;
            };
            if v.y + v.height / 2.0 < rp.y {
                pos += 1;
                line = v.y + v.height;
            }
        }
        (pos, line)
    }

    /// The keys of `keys` and, for a channel or bus among them, its stacked inserts.
    pub(super) fn with_stacks(&self, mut keys: Vec<DeviceKey>) -> Vec<DeviceKey> {
        let extra: Vec<DeviceKey> = self
            .areas
            .iter()
            .filter(|a| keys.contains(&a.owner))
            .flat_map(|a| a.members.iter().chain(&a.tail).cloned())
            .filter(|m| !keys.contains(m))
            .collect();
        keys.extend(extra);
        keys
    }

    /// The chain position of `key` among the stacked inserts of its strip: the strip and the
    /// position.
    fn member_slot(&self, key: &DeviceKey) -> Option<(&InsertArea, usize)> {
        self.areas.iter().find_map(|a| {
            a.members
                .iter()
                .position(|m| m == key)
                .map(|slot| (a, slot))
        })
    }

    /// The release of a device drag over the insert areas (see the module documentation):
    /// `true` when the drop was handled (a command emitted, or a toast that says why not).
    pub(super) fn stack_drop(
        &mut self,
        moving: &[DeviceKey],
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) -> bool {
        if !self.scope.is_empty() || self.areas.is_empty() {
            return false;
        }
        let rp = self.view_to_rack(self.pointer);
        let members: Vec<&DeviceKey> = moving.iter().filter(|k| self.stacked.contains(k)).collect();
        let carried_with_owner = !members.is_empty()
            && moving
                .iter()
                .any(|k| self.areas.iter().any(|a| a.owner == *k));
        if carried_with_owner {
            // The strip and its inserts: an ordinary move of the whole stack.
            return false;
        }
        let area = self.area_at(rp, moving);
        if moving.len() > 1 && (area.is_some() || !members.is_empty()) {
            self.toast = Some(ONE_AT_A_TIME.to_string());
            return true;
        }
        let [key] = moving else { return false };
        match (
            area,
            self.member_slot(key).map(|(a, s)| (a.owner.clone(), s)),
        ) {
            (Some(ai), home) => {
                let (pos, _) = self.area_slot(ai, rp, moving);
                let owner = self.areas[ai].owner.clone();
                match home {
                    Some((h, from)) if h == owner => {
                        if pos != from {
                            let command = Command::Console(ConsoleCommand::MoveInsert {
                                channel: owner,
                                from,
                                to: pos,
                            });
                            self.emit_checked(command, doc, registry, out);
                        }
                    }
                    _ => {
                        let command = Command::Console(ConsoleCommand::AddInsert {
                            channel: owner,
                            slot: pos,
                            effect: InsertEffect::Existing(key.clone()),
                        });
                        self.emit_checked(command, doc, registry, out);
                    }
                }
                true
            }
            // An insert dragged out of its frame to the rack leaves the chain.
            (None, Some((owner, slot))) => {
                let flags = self.moving_flags(moving);
                let dp = self.drop_point(rp, &flags);
                let index = self.slot_index(self.host(doc), &dp, moving);
                let command = Command::Console(ConsoleCommand::ExtractInsert {
                    channel: owner,
                    slot,
                    to: Some(RackSlot {
                        column: dp.column,
                        index,
                    }),
                });
                self.emit_checked(command, doc, registry, out);
                true
            }
            (None, None) => false,
        }
    }

    /// A browser row dropped over the insert area `ai`: the effect is created in the chain at
    /// the place under the pointer. Returns whether the drop was taken (a refusal says why in a
    /// toast and counts as taken).
    pub(super) fn area_browser_drop(
        &mut self,
        ai: usize,
        row: &BrowserRow,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) -> bool {
        if self.row_family(row) != Some("effect") {
            self.toast = Some(ONLY_EFFECTS.to_string());
            return true;
        }
        if let BrowserRow::Device(d) = row
            && let Some(desc) = registry.descriptor(d.type_id)
            && !is_stereo_effect(desc)
        {
            self.toast = Some(format!("{} is not a stereo effect", d.name));
            return true;
        }
        let Some(source) = self.row_source(row) else {
            return false;
        };
        let rp = self.view_to_rack(self.pointer);
        let (pos, _) = self.area_slot(ai, rp, &[]);
        let a = &self.areas[ai];
        let above = pos
            .checked_sub(1)
            .map_or_else(|| a.owner.clone(), |p| a.members[p].clone());
        let host = self.host(doc);
        let Some(at) = host.devices.iter().position(|d| d.key == above) else {
            return false;
        };
        let column = host.devices[at].column;
        let slot = RackSlot {
            column,
            index: at + 1 - host.column_range(column).start,
        };
        let command = Command::Workflow(WorkflowCommand::CreateEffectAt {
            slot,
            source,
            key: None,
            options: CreateOptions {
                route: true,
                skip_track: false,
                disarm: Vec::new(),
            },
        });
        self.emit_checked(command, doc, registry, out);
        true
    }

    /// Emits `command` when the model takes it on the project (planned on a scratch copy), else
    /// says why not in a toast: the model's own words, as the status line would.
    fn emit_checked(
        &mut self,
        command: Command,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        match super::container_move::plan_reason(self.host(doc), registry, command.clone()) {
            None => out.push(UiAction::Command(command)),
            Some(why) => self.toast = Some(why),
        }
    }

    /// Paints the frames, header lines and drop rows of the insert areas behind the panels.
    pub(super) fn paint_areas(&self, c: &mut dyn Canvas, top: f64, bottom: f64) {
        let theme = &self.theme;
        for a in &self.areas {
            let frame = a.outline();
            if frame.y1 < top || frame.y0 > bottom {
                continue;
            }
            let shape = RoundedRect::from_rect(frame, 5.0);
            c.fill_with(shape, a.accent.with_alpha(0.07));
            c.stroke(
                &shape.into(),
                &Stroke::new(2.0),
                &a.accent.with_alpha(0.55).into(),
            );
            let row = a.row.inset(-6.0);
            let size = theme.label_size + 1.0;
            if a.shown {
                c.stroke(
                    &RoundedRect::from_rect(row.inset(-2.0), 3.0).into(),
                    &Stroke::new(1.0).with_dashes(0.0, [5.0, 4.0]),
                    &theme.text_muted.with_alpha(0.55).into(),
                );
                c.text(
                    &TextRun::new(
                        "drop or create an effect here",
                        pt(row.center().x, row.center().y + 3.5),
                        size,
                        theme.text_muted,
                    )
                    .align(Align::Center),
                );
            } else {
                c.fill_with(RoundedRect::from_rect(row, 3.0), theme.lcd);
                let names = if a.names.is_empty() {
                    "no insert effects".to_string()
                } else {
                    a.names.join("  /  ")
                };
                c.text(
                    &TextRun::new(
                        "INSERT FX",
                        pt(row.x0 + 8.0, row.center().y + 3.5),
                        size,
                        a.accent,
                    )
                    .weight(600),
                );
                c.text(&TextRun::new(
                    fit_label(&names, row.width() - 100.0, size),
                    pt(row.x0 + 84.0, row.center().y + 3.5),
                    size,
                    theme.text_muted,
                ));
            }
        }
        for g in &self.sends {
            let frame = g.outline();
            if frame.y1 < top || frame.y0 > bottom {
                continue;
            }
            let shape = RoundedRect::from_rect(frame, 5.0);
            c.fill_with(shape, g.accent.with_alpha(0.07));
            c.stroke(
                &shape.into(),
                &Stroke::new(2.0),
                &g.accent.with_alpha(0.55).into(),
            );
            let size = theme.label_size + 1.0;
            let y = g.header.center().y + 3.5;
            c.text(
                &TextRun::new(
                    format!("Send {}", g.slot + 1),
                    pt(g.header.x0 + 6.0, y),
                    size,
                    g.accent,
                )
                .weight(600),
            );
            c.text(&TextRun::new(
                fit_label(&g.names.join("  /  "), g.header.width() - 100.0, size),
                pt(g.header.x0 + 70.0, y),
                size,
                theme.text_muted,
            ));
        }
    }

    /// The feedback of a drag over an insert area: the frame is outlined and the drop line
    /// shows among the stacked effects. Returns whether the pointer is over one.
    pub(super) fn paint_area_drop(&self, c: &mut dyn Canvas, moving: &[DeviceKey]) -> bool {
        let rp = self.view_to_rack(self.pointer);
        let Some(ai) = self.area_at(rp, moving) else {
            return false;
        };
        let theme = &self.theme;
        let a = &self.areas[ai];
        let (_, line_y) = self.area_slot(ai, rp, moving);
        let x = column_left(
            self.devices
                .iter()
                .find(|v| v.key == a.owner)
                .map_or(0, |v| v.column),
        );
        c.stroke(
            &RoundedRect::from_rect(a.outline(), 5.0).into(),
            &Stroke::new(3.0),
            &theme.highlight.into(),
        );
        c.stroke(
            &Line::new((x - 4.0, line_y), (x + f64::from(RACK_WIDTH) + 4.0, line_y)).into(),
            &Stroke::new(3.0),
            &theme.highlight.into(),
        );
        true
    }

    /// The chain position the drop under `rp` would take in the area of strip `owner` (tests).
    pub fn insert_drop_position(&self, owner: &DeviceKey, view_pos: Point) -> Option<usize> {
        let ai = self.areas.iter().position(|a| a.owner == *owner)?;
        Some(self.area_slot(ai, self.view_to_rack(view_pos), &[]).0)
    }
}

/// The refusal of a drop of several devices on an area.
pub(super) const ONE_AT_A_TIME: &str = "Drop one device at a time in an insert area";
/// The refusal of a browser row that is no effect.
const ONLY_EFFECTS: &str = "Only effects can be dropped in an insert area";

/// Whether the descriptor has the four stereo audio sockets of an insert effect.
fn is_stereo_effect(desc: &atomik_device_sdk::DeviceDescriptor) -> bool {
    ["in_l", "in_r", "out_l", "out_r"]
        .iter()
        .all(|k| desc.socket_by_key(k).is_some())
}
