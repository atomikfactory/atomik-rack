//! Dragging devices in the rack: the 2-D move between and inside columns, and dragging a row
//! out of the device browser. Both find the drop slot (column and index) under the pointer,
//! draw the same drop line and the new-column zone, and end in one slot command.

use super::columns::column_left;
use super::{Drag, RackView, UiAction};
use crate::browser::{BrowserRow, CreateKind};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::Modifiers;
use crate::paint;
use atomik_device_sdk::{PluginKindHint, RACK_WIDTH};
use atomik_model::{
    Command, CreateOptions, CreateSource, DeviceKey, PluginCommand, RackDoc, RackSlot,
    WorkflowCommand,
};
use vello::kurbo::{Line, Point, RoundedRect, Stroke};

/// Where a dragged device would drop.
pub(super) struct DropPoint {
    /// The target column; `RackView::columns` is the new column.
    pub column: u16,
    /// Whether the target is the new column.
    pub new_column: bool,
    /// How many of the column's devices that stay lie above the pointer (the locked Master
    /// Section counts as above anything in column 0).
    pub above: usize,
    /// The devices of the column that stay and are shown, top to bottom.
    pub keys: Vec<DeviceKey>,
    /// Height of the drop line, rack coordinates.
    pub line_y: f64,
}

impl RackView {
    /// The drop point at rack position `rp`; `moving[i]` marks the device views being moved.
    pub(super) fn drop_point(&self, rp: Point, moving: &[bool]) -> DropPoint {
        let column = self.drop_column(rp.x);
        let new_column = column >= self.columns;
        let members: Vec<usize> = if new_column {
            Vec::new()
        } else {
            self.inner_range()
                .filter(|&i| !moving[i] && self.devices[i].column == column)
                .collect()
        };
        // The line starts at the top of the column; column 0 starts below the Audio I/O strip
        // and below the locked Master Section.
        let mut line_y = if column == 0 {
            self.devices
                .iter()
                .skip(1)
                .find(|d| d.column == 0)
                .map_or_else(
                    || self.devices.first().map_or(0.0, |d| d.y + d.height),
                    |d| {
                        if self.locked.as_ref() == Some(&d.key) {
                            d.y + d.height
                        } else {
                            d.y
                        }
                    },
                )
        } else {
            0.0
        };
        let mut above = 0;
        for &i in &members {
            let d = &self.devices[i];
            if d.y + d.height / 2.0 < rp.y {
                line_y = d.y + d.height;
                above += 1;
            }
        }
        // Nothing goes above the locked Master Section (the first device of column 0).
        if column == 0
            && members
                .first()
                .is_some_and(|&i| self.locked.as_ref() == Some(&self.devices[i].key))
        {
            above = above.max(1);
        }
        DropPoint {
            column,
            new_column,
            above,
            keys: members
                .iter()
                .map(|&i| self.devices[i].key.clone())
                .collect(),
            line_y,
        }
    }

    /// The index within the column, counting every device of the document in it (hidden
    /// console devices too) except `moving`: where `dp` lands among them.
    pub(super) fn slot_index(&self, doc: &RackDoc, dp: &DropPoint, moving: &[DeviceKey]) -> usize {
        if dp.new_column {
            return 0;
        }
        let others: Vec<&DeviceKey> = doc.devices[doc.column_range(dp.column)]
            .iter()
            .map(|d| &d.key)
            .filter(|k| !moving.contains(k))
            .collect();
        let pos = |k: &DeviceKey| others.iter().position(|o| *o == k);
        match dp.above.checked_sub(1) {
            None => dp.keys.first().and_then(pos).unwrap_or(others.len()),
            Some(n) => dp.keys.get(n).and_then(pos).map_or(others.len(), |p| p + 1),
        }
    }

    // ------------------------------------------------------------------ the move drag

    /// The release of a device move: one command that puts the dragged device, or the whole
    /// selection it belongs to, at the slot under the pointer. Inside a container (one
    /// column) it is the flat move command.
    pub(super) fn move_drop(&mut self, dev: usize, doc: &RackDoc, out: &mut Vec<UiAction>) {
        let key = self.devices[dev].key.clone();
        // The dragged device, or the whole selection when it is part of one.
        let moving: Vec<DeviceKey> = if self.drags_as_group(dev) {
            self.local_selection()
        } else {
            vec![key.clone()]
        };
        if self.locked.as_ref().is_some_and(|m| moving.contains(m)) {
            self.toast = Some(Self::MASTER_LOCKED_TEXT.to_string());
            return;
        }
        let flags: Vec<bool> = self
            .devices
            .iter()
            .map(|d| moving.contains(&d.key))
            .collect();
        let dp = self.drop_point(self.view_to_rack(self.pointer), &flags);
        let index = self.slot_index(doc, &dp, &moving);
        if !self.scope.is_empty() {
            flat_move(doc, key, moving, index, out);
            return;
        }
        let slot = RackSlot {
            column: dp.column,
            index,
        };
        if move_changes(doc, &moving, slot, self.locked.as_ref()) {
            out.push(UiAction::Command(Command::Workflow(
                WorkflowCommand::MoveDevicesToSlot { keys: moving, slot },
            )));
        }
    }

    // ------------------------------------------------------------------ the browser drag

    /// The release of a browser row: over the rack it creates the device at the slot under
    /// the pointer; anywhere else the drag is cancelled and nothing is emitted.
    pub(super) fn browser_drop(
        &mut self,
        row: &BrowserRow,
        doc: &RackDoc,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        if !self.rack_area().contains(self.pointer) {
            return;
        }
        let none = vec![false; self.devices.len()];
        let dp = self.drop_point(self.view_to_rack(self.pointer), &none);
        let index = self.slot_index(doc, &dp, &[]);
        let action = if self.scope.is_empty() {
            let slot = RackSlot {
                column: dp.column,
                index,
            };
            self.add_row_slot_action(row, slot, modifiers)
        } else {
            self.add_row_action_at(row, index, modifiers)
        };
        let Some(action) = action else { return };
        out.push(action);
        // As Enter does: a create browser closes after the device was created.
        if self.browser.create.is_some() && !self.browser.keep_open {
            self.set_browser_visible(false);
        }
    }

    /// The slot version of [`Self::add_row_action`] (top level only).
    fn add_row_slot_action(
        &self,
        row: &BrowserRow,
        slot: RackSlot,
        modifiers: Modifiers,
    ) -> Option<UiAction> {
        match row {
            BrowserRow::Device(d) => {
                Some(self.device_slot_action(d.type_id, d.kind, slot, modifiers))
            }
            BrowserRow::Plugin(n) => {
                let e = self.plugin_catalog.get(*n)?;
                let kind = match e.kind {
                    PluginKindHint::Instrument => Some(CreateKind::Instrument),
                    PluginKindHint::Effect => Some(CreateKind::Effect),
                    _ => None,
                };
                Some(match kind {
                    Some(kind) => self.create_action_at(
                        CreateSource::Plugin {
                            plugin: e.plugin.clone(),
                            layout: e.layout.clone(),
                            path_hint: e.path.clone(),
                        },
                        kind,
                        slot,
                        modifiers,
                    ),
                    // Another plugin kind: added at the slot (a new column included).
                    None => UiAction::Command(Command::Plugin(PluginCommand::AddAt {
                        slot,
                        plugin: e.plugin.clone(),
                        layout: e.layout.clone(),
                        path_hint: e.path.clone(),
                        key: None,
                    })),
                })
            }
            _ => None,
        }
    }

    /// The action that adds the built-in device `desc` at a rack slot, as dropping its browser
    /// row does (top level only): through the creation chain for an instrument or an effect,
    /// else a plain add. For callers that hold a device by type, such as the library's rows.
    pub fn add_device_slot_action_for(
        &self,
        desc: &atomik_device_sdk::DeviceDescriptor,
        slot: RackSlot,
        modifiers: Modifiers,
    ) -> UiAction {
        self.device_slot_action(desc.type_id, desc.kind, slot, modifiers)
    }

    fn device_slot_action(
        &self,
        type_id: &str,
        kind: atomik_device_sdk::DeviceKind,
        slot: RackSlot,
        modifiers: Modifiers,
    ) -> UiAction {
        match self.creation_of(kind) {
            Some(create) => self.create_action_at(
                CreateSource::Type(type_id.to_string()),
                create,
                slot,
                modifiers,
            ),
            None => UiAction::Command(Command::AddDeviceAt {
                slot,
                type_id: type_id.to_string(),
                key: None,
            }),
        }
    }

    /// The creation command of the browser at a slot: route = the setting, inverted by Shift;
    /// Alt skips the track. An effect is routed as if the device above the slot were selected.
    fn create_action_at(
        &self,
        source: CreateSource,
        kind: CreateKind,
        slot: RackSlot,
        modifiers: Modifiers,
    ) -> UiAction {
        let options = CreateOptions {
            route: self.auto_route != modifiers.shift,
            skip_track: modifiers.alt,
            disarm: Vec::new(),
        };
        UiAction::Command(Command::Workflow(match kind {
            CreateKind::Instrument => WorkflowCommand::CreateInstrumentAt {
                slot,
                source,
                key: None,
                options,
            },
            CreateKind::Effect => WorkflowCommand::CreateEffectAt {
                slot,
                source,
                key: None,
                options,
            },
        }))
    }

    /// The name shown on the ghost label of a dragged browser row.
    fn row_label(&self, row: &BrowserRow) -> Option<String> {
        match row {
            BrowserRow::Device(d) => Some(d.name.to_string()),
            BrowserRow::Plugin(n) => self.plugin_catalog.get(*n).map(|e| e.title()),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ painting

    /// The feedback of a device drag, in rack coordinates: the devices being moved are dimmed,
    /// the drop line shows in the target column, and the new-column zone is outlined.
    pub(super) fn paint_drag_feedback(&self, c: &mut dyn Canvas) {
        if !self.drop_drag_active() {
            return;
        }
        let theme = &self.theme;
        let moving: Vec<bool> = match &self.drag {
            Drag::Move { dev, .. } => {
                // The dragged device, or every device of the selection it belongs to.
                let group = self.drags_as_group(*dev);
                self.devices
                    .iter()
                    .enumerate()
                    .map(|(i, d)| i == *dev || (group && self.selection.contains(&d.key)))
                    .collect()
            }
            _ => vec![false; self.devices.len()],
        };
        for (d, _) in self.devices.iter().zip(&moving).filter(|(_, m)| **m) {
            paint::fill_rect(c, d.rect(), theme.dim.with_alpha(0.4));
        }
        // A browser row is only dropped over the rack area, so it shows nothing elsewhere.
        if matches!(self.drag, Drag::Browser { .. }) && !self.rack_area().contains(self.pointer) {
            return;
        }
        // A move released over the navigator is dropped: no line, no zone highlight.
        if self.in_navigator(self.pointer) {
            return;
        }
        let dp = self.drop_point(self.view_to_rack(self.pointer), &moving);
        if self.zone_visible() {
            let zone = self.zone_rect();
            let alpha = if dp.new_column { 1.0 } else { 0.45 };
            c.fill_with(
                RoundedRect::from_rect(zone, 6.0),
                theme
                    .highlight
                    .with_alpha(if dp.new_column { 0.16 } else { 0.05 }),
            );
            c.stroke(
                &RoundedRect::from_rect(zone, 6.0).into(),
                &Stroke::new(2.0).with_dashes(0.0, [9.0, 6.0]),
                &theme.highlight.with_alpha(alpha).into(),
            );
            c.text(
                &TextRun::new(
                    "New column",
                    pt(zone.center().x, zone.y0 + 28.0),
                    12.0,
                    theme.highlight.with_alpha(alpha),
                )
                .weight(600)
                .align(Align::Center),
            );
        }
        if !dp.new_column {
            let x = column_left(dp.column);
            c.stroke(
                &Line::new(
                    (x - 8.0, dp.line_y),
                    (x + f64::from(RACK_WIDTH) + 8.0, dp.line_y),
                )
                .into(),
                &Stroke::new(3.0),
                &theme.highlight.into(),
            );
        }
    }

    /// The ghost label of a browser row being dragged: its name follows the pointer (view
    /// coordinates).
    pub(super) fn paint_browser_ghost(&self, c: &mut dyn Canvas) {
        if let Drag::Browser {
            row, active: true, ..
        } = &self.drag
            && let Some(name) = self.row_label(row)
        {
            paint::tooltip(c, &self.theme, self.pointer, &name);
        }
    }
}

/// The flat move of a container's inside (one column): the plain commands.
fn flat_move(
    doc: &RackDoc,
    key: DeviceKey,
    moving: Vec<DeviceKey>,
    to: usize,
    out: &mut Vec<UiAction>,
) {
    if moving.len() > 1 {
        // Where the group would end up, compared with where it is now.
        let others: Vec<&DeviceKey> = doc
            .devices
            .iter()
            .map(|d| &d.key)
            .filter(|d| !moving.contains(d))
            .collect();
        let mut target = others.clone();
        let at = to.min(target.len());
        for (n, m) in moving.iter().enumerate() {
            target.insert(at + n, m);
        }
        let now: Vec<&DeviceKey> = doc.devices.iter().map(|d| &d.key).collect();
        if target != now {
            out.push(UiAction::Command(Command::Workflow(
                WorkflowCommand::MoveDevices { keys: moving, to },
            )));
        }
    } else if doc.devices.iter().position(|d| d.key == key) != Some(to) {
        out.push(UiAction::Command(Command::MoveDevice { key, to }));
    }
}

/// Whether moving `moving` to `slot` changes the rack (the model rejects a move that does
/// not): simulates the move on (key, column) pairs, with the same clamps as the model.
fn move_changes(
    doc: &RackDoc,
    moving: &[DeviceKey],
    slot: RackSlot,
    locked: Option<&DeviceKey>,
) -> bool {
    let now: Vec<(&DeviceKey, u16)> = doc.devices.iter().map(|d| (&d.key, d.column)).collect();
    let mut rest: Vec<(&DeviceKey, u16)> = now
        .iter()
        .copied()
        .filter(|(k, _)| !moving.contains(k))
        .collect();
    let group: Vec<(&DeviceKey, u16)> = now
        .iter()
        .copied()
        .filter(|(k, _)| moving.contains(k))
        .map(|(k, _)| (k, slot.column))
        .collect();
    let start = rest.partition_point(|(_, c)| *c < slot.column);
    let len = rest.partition_point(|(_, c)| *c <= slot.column) - start;
    let mut at = slot.index.min(len);
    if slot.column == 0 && rest.first().is_some_and(|(k, _)| Some(*k) == locked) {
        at = at.max(1).min(len);
    }
    let at = if slot.column >= doc.columns {
        rest.len()
    } else {
        start + at
    };
    rest.splice(at..at, group);
    rest != now
}
