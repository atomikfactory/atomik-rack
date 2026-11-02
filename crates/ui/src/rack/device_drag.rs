//! Dragging devices in the rack: the 2-D move between and inside columns, and dragging a row
//! out of the device browser. Both find the drop slot (column and index) under the pointer,
//! draw the same drop line and the new-column zone, and end in one slot command.

use super::columns::column_left;
use super::inline::{is_inner, parent_of, strip_of};
use super::{Drag, RackView, UiAction};
use crate::browser::{BrowserRow, CreateKind};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::Modifiers;
use crate::paint;
use atomik_device_sdk::{PluginKindHint, RACK_WIDTH, Registry};
use atomik_model::{
    Command, ContainerCommand, CreateOptions, CreateSource, DeviceKey, PluginCommand, RackDoc,
    RackSlot, WorkflowCommand,
};
use std::collections::BTreeSet;
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
        self.drop_point_in(rp, moving, None)
    }

    /// [`Self::drop_point`] in column `force` instead of the one under the pointer (a device
    /// that leaves a container drops in the column of that container). Only the devices of the
    /// rack itself are members: an unfolded container counts as one block with its inside.
    pub(super) fn drop_point_in(
        &self,
        rp: Point,
        moving: &[bool],
        force: Option<u16>,
    ) -> DropPoint {
        let column = force.unwrap_or_else(|| self.drop_column(rp.x));
        let new_column = column >= self.columns;
        let members: Vec<usize> = if new_column {
            Vec::new()
        } else {
            self.inner_range()
                .filter(|&i| {
                    !moving[i]
                        && !is_inner(&self.devices[i].key)
                        && self.devices[i].column == column
                })
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
            let bottom = self.block_bottom(i);
            if (d.y + bottom) / 2.0 < rp.y {
                line_y = bottom;
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
            .filter(|k| !moving.contains(k) && !is_inner(k))
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
    /// column) it is the flat move command; on an unfolded rack a device dropped in the frame
    /// of a container goes inside it, one dropped outside the frame of its own container leaves
    /// it.
    pub(super) fn move_drop(
        &mut self,
        dev: usize,
        doc: &RackDoc,
        registry: &Registry,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
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
        let flags = self.moving_flags(&moving);
        // Over a container's panel the device goes inside, over the breadcrumb's project
        // segment it leaves the open container.
        if self.container_drop(&key, &moving, &flags, out) {
            return;
        }
        if self.inline {
            self.inline_drop(&moving, &flags, doc, registry, modifiers, out);
            return;
        }
        let dp = self.drop_point(self.view_to_rack(self.pointer), &flags);
        let index = self.slot_index(doc, &dp, &moving);
        if !self.scope.is_empty() {
            let list: Vec<&DeviceKey> = doc.devices.iter().map(|d| &d.key).collect();
            flat_move(&list, key, moving, index, out);
            return;
        }
        let slot = RackSlot {
            column: dp.column,
            index,
        };
        if move_changes(doc, &moving, slot, self.locked.as_ref()) {
            out.push(UiAction::Command(Command::Workflow(
                WorkflowCommand::MoveDevicesToSlot {
                    keys: moving,
                    slot,
                    route: self.auto_route != modifiers.shift,
                },
            )));
        }
    }

    /// The release of a device move on an unfolded rack (`doc` is the flat document): the
    /// devices keep their container (reordered in it), enter the unfolded container whose frame
    /// they were dropped in, or leave their container for the rack. A drop the model would
    /// refuse says why instead.
    fn inline_drop(
        &mut self,
        moving: &[DeviceKey],
        flags: &[bool],
        doc: &RackDoc,
        registry: &Registry,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let rp = self.view_to_rack(self.pointer);
        let homes: BTreeSet<Option<DeviceKey>> = moving.iter().map(parent_of).collect();
        if homes.len() > 1 {
            self.toast = Some(ONE_CONTAINER_AT_A_TIME.to_string());
            return;
        }
        let home: Option<DeviceKey> = homes.into_iter().next().flatten();
        let target = self.frame_at(rp, moving).map(|f| f.container.clone());
        match (home, target) {
            // Dropped in the frame of the devices' own container: a reorder.
            (Some(h), Some(t)) if h == t => {
                let (index, _, _) = self.inside_drop(&t, rp, flags);
                inner_move(doc, &t, moving, index, out);
            }
            // Into an unfolded container of the rack, or from one container into another.
            (home, Some(t)) => {
                let [key] = moving else {
                    self.toast = Some(super::container_move::ONE_AT_A_TIME.to_string());
                    return;
                };
                if home.is_some() || is_inner(&t) {
                    self.toast = Some(NOT_BETWEEN_CONTAINERS.to_string());
                    return;
                }
                let (index, _, _) = self.inside_drop(&t, rp, flags);
                let command = Command::Container(ContainerCommand::MoveIntoContainer {
                    key: key.clone(),
                    container: t,
                    index,
                });
                self.emit_planned(command, registry, out);
            }
            // Out of the frame of its container to the rack, in the container's column.
            (Some(h), None) => {
                let [key] = moving else {
                    self.toast = Some(super::container_move::ONE_AT_A_TIME.to_string());
                    return;
                };
                if is_inner(&h) {
                    self.toast = Some(NOT_BETWEEN_CONTAINERS.to_string());
                    return;
                }
                let column = self
                    .devices
                    .iter()
                    .find(|d| d.key == h)
                    .map_or(0, |d| d.column);
                let dp = self.drop_point_in(rp, flags, Some(column));
                let index = self.slot_index(doc, &dp, moving);
                let before = doc
                    .devices
                    .iter()
                    .filter(|d| !is_inner(&d.key) && d.column < column)
                    .count();
                let command = Command::Container(ContainerCommand::MoveOutOfContainer {
                    key: key.clone(),
                    index: before + index,
                });
                self.emit_planned(command, registry, out);
            }
            // Within the rack itself.
            (None, None) => {
                let dp = self.drop_point(rp, flags);
                let index = self.slot_index(doc, &dp, moving);
                let slot = RackSlot {
                    column: dp.column,
                    index,
                };
                if move_changes(doc, moving, slot, self.locked.as_ref()) {
                    out.push(UiAction::Command(Command::Workflow(
                        WorkflowCommand::MoveDevicesToSlot {
                            keys: moving.to_vec(),
                            slot,
                            route: self.auto_route != modifiers.shift,
                        },
                    )));
                }
            }
        }
    }

    /// Emits `command` when the model takes it on the project, else tells the user why not.
    fn emit_planned(&mut self, command: Command, registry: &Registry, out: &mut Vec<UiAction>) {
        let reason = self
            .host_doc
            .as_ref()
            .and_then(|host| super::container_move::plan_reason(host, registry, command.clone()));
        match reason {
            None => out.push(UiAction::Command(command)),
            Some(why) => self.toast = Some(why),
        }
    }

    // ------------------------------------------------------------------ the browser drag

    /// The release of a browser row: over the rack it creates the device at the slot under
    /// the pointer; anywhere else the drag is cancelled and nothing is emitted.
    pub(super) fn browser_drop(
        &mut self,
        row: &BrowserRow,
        doc: &RackDoc,
        registry: &Registry,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        if !self.rack_area().contains(self.pointer) {
            return;
        }
        // Dropped in the frame of an unfolded container the device is added inside it.
        let rp = self.view_to_rack(self.pointer);
        if let Some(container) = self.frame_at(rp, &[]).map(|f| f.container.clone()) {
            let none = vec![false; self.devices.len()];
            let (index, _, _) = self.inside_drop(&container, rp, &none);
            if let Some(action) = self.add_row_inside(row, &container, index) {
                out.push(action);
                if self.browser.create.is_some() && !self.browser.keep_open {
                    self.set_browser_visible(false);
                }
            }
            return;
        }
        // Over the middle of a device of the same kind the row replaces it; a replacement the
        // model refuses is an ordinary drop at the slot.
        if let Some(target) = self.replace_target(row, self.view_to_rack(self.pointer))
            && let Some(action) = self.replace_action(row, target, self.host(doc), registry)
        {
            out.push(action);
            if self.browser.create.is_some() && !self.browser.keep_open {
                self.set_browser_visible(false);
            }
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

    /// The action that adds a browser row inside the unfolded container `container` at
    /// `index` among its devices: a plain add (an instrument also gets its full-range split).
    pub(super) fn add_row_inside(
        &self,
        row: &BrowserRow,
        container: &DeviceKey,
        index: usize,
    ) -> Option<UiAction> {
        match row {
            BrowserRow::Device(d) => {
                Some(self.add_device_inside(d.type_id, d.kind, container, index))
            }
            BrowserRow::Plugin(n) => {
                let e = self.plugin_catalog.get(*n)?;
                Some(UiAction::Command(Command::Plugin(PluginCommand::Add {
                    index,
                    container: Some(container.clone()),
                    plugin: e.plugin.clone(),
                    layout: e.layout.clone(),
                    path_hint: e.path.clone(),
                    key: None,
                    auto_split: e.kind == PluginKindHint::Instrument,
                })))
            }
            _ => None,
        }
    }

    /// The action that adds the built-in device `type_id` inside `container` at `index`.
    pub(super) fn add_device_inside(
        &self,
        type_id: &str,
        kind: atomik_device_sdk::DeviceKind,
        container: &DeviceKey,
        index: usize,
    ) -> UiAction {
        UiAction::Command(Command::Container(ContainerCommand::AddInner {
            container: container.clone(),
            index,
            type_id: type_id.to_string(),
            key: None,
            auto_split: kind == atomik_device_sdk::DeviceKind::Instrument,
        }))
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
        // A player dropped at a slot stays where it was put and is not cabled: the stack is
        // built from the selection by the browser's add.
        let create = self
            .creation_of(kind)
            .filter(|_| !crate::browser::is_player(type_id));
        match create {
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
            CreateKind::Effect | CreateKind::Player => WorkflowCommand::CreateEffectAt {
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

    // ------------------------------------------------------------------ replace by drop

    /// What kind of device a browser row is, for replacing: instrument, effect or note player.
    fn row_family(&self, row: &BrowserRow) -> Option<&'static str> {
        match row {
            BrowserRow::Device(d) => replace_family(d),
            BrowserRow::Plugin(n) => match self.plugin_catalog.get(*n)?.kind {
                PluginKindHint::Instrument => Some("instrument"),
                PluginKindHint::Effect => Some("effect"),
                _ => None,
            },
            _ => None,
        }
    }

    /// What dropping `row` creates, for [`WorkflowCommand::ReplaceDevice`].
    fn row_source(&self, row: &BrowserRow) -> Option<CreateSource> {
        match row {
            BrowserRow::Device(d) => Some(CreateSource::Type(d.type_id.to_string())),
            BrowserRow::Plugin(n) => {
                let e = self.plugin_catalog.get(*n)?;
                Some(CreateSource::Plugin {
                    plugin: e.plugin.clone(),
                    layout: e.layout.clone(),
                    path_hint: e.path.clone(),
                })
            }
            _ => None,
        }
    }

    /// The device a browser row replaces when dropped at rack position `rp`: the pointer is over
    /// the middle of the panel of a top-level device of the same kind (the edges of a panel are
    /// the gaps between devices, which insert). Only the top level, never the interface strip.
    pub(super) fn replace_target(&self, row: &BrowserRow, rp: Point) -> Option<usize> {
        if !self.scope.is_empty() {
            return None;
        }
        let family = self.row_family(row)?;
        self.inner_range().find(|&i| {
            let d = &self.devices[i];
            let r = d.rect();
            let band = r.height() / 4.0;
            !is_inner(&d.key)
                && r.x0 <= rp.x
                && rp.x <= r.x1
                && r.y0 + band <= rp.y
                && rp.y <= r.y1 - band
                && d.desc.as_ref().and_then(replace_family) == Some(family)
        })
    }

    /// The replace command of dropping `row` on device `target`, when the model accepts it.
    fn replace_action(
        &self,
        row: &BrowserRow,
        target: usize,
        doc: &RackDoc,
        registry: &Registry,
    ) -> Option<UiAction> {
        let source = self.row_source(row)?;
        let key = self.devices[target].key.clone();
        if atomik_model::replace_reason(doc, registry, &key, &source).is_some() {
            return None;
        }
        Some(UiAction::Command(Command::Workflow(
            WorkflowCommand::ReplaceDevice {
                target: key,
                source,
            },
        )))
    }

    // ------------------------------------------------------------------ painting

    /// The feedback of a device drag, in rack coordinates: the devices being moved are dimmed,
    /// the drop line shows in the target column, and the new-column zone is outlined.
    pub(super) fn paint_drag_feedback(&self, c: &mut dyn Canvas) {
        if !self.drop_drag_active() {
            return;
        }
        let theme = &self.theme;
        let (moving, moving_keys): (Vec<bool>, Vec<DeviceKey>) = match &self.drag {
            Drag::Move { dev, .. } => {
                // The dragged device, or every device of the selection it belongs to.
                let group = self.drags_as_group(*dev);
                let keys: Vec<DeviceKey> = if group {
                    self.local_selection()
                } else {
                    vec![self.devices[*dev].key.clone()]
                };
                (self.moving_flags(&keys), keys)
            }
            _ => (vec![false; self.devices.len()], Vec::new()),
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
        if self.paint_container_drop(c, &moving) {
            return;
        }
        // A row over the middle of a device of its kind replaces it: the device is outlined
        // instead of the drop line.
        if let Drag::Browser { row, .. } = &self.drag
            && let Some(i) = self.replace_target(row, self.view_to_rack(self.pointer))
        {
            let outline = RoundedRect::from_rect(self.devices[i].rect(), 6.0);
            c.fill_with(outline, theme.highlight.with_alpha(0.12));
            c.stroke(&outline.into(), &Stroke::new(3.0), &theme.highlight.into());
            return;
        }
        let rp = self.view_to_rack(self.pointer);
        // Over the frame of an unfolded container the line shows among its devices.
        if let Some(f) = self.frame_at(rp, &moving_keys) {
            let (_, line_y, column) = self.inside_drop(&f.container, rp, &moving);
            let x = column_left(column);
            let outline = RoundedRect::from_rect(f.outline(), 5.0);
            c.stroke(&outline.into(), &Stroke::new(3.0), &theme.highlight.into());
            c.stroke(
                &Line::new((x - 4.0, line_y), (x + f64::from(RACK_WIDTH) + 4.0, line_y)).into(),
                &Stroke::new(3.0),
                &theme.highlight.into(),
            );
            return;
        }
        // A device of an unfolded container leaves it in the column of the container.
        let leaving = if self.inline {
            moving_keys
                .first()
                .and_then(parent_of)
                .and_then(|h| self.devices.iter().find(|d| d.key == h).map(|d| d.column))
        } else {
            None
        };
        let dp = self.drop_point_in(rp, &moving, leaving);
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

/// The flat move inside one list of devices (a container's inside): the plain commands.
/// `list` holds the keys of the devices in order.
fn flat_move(
    list: &[&DeviceKey],
    key: DeviceKey,
    moving: Vec<DeviceKey>,
    to: usize,
    out: &mut Vec<UiAction>,
) {
    if moving.len() > 1 {
        // Where the group would end up, compared with where it is now.
        let others: Vec<&DeviceKey> = list
            .iter()
            .copied()
            .filter(|d| !moving.contains(d))
            .collect();
        let mut target = others.clone();
        let at = to.min(target.len());
        for (n, m) in moving.iter().enumerate() {
            target.insert(at + n, m);
        }
        if target != list {
            out.push(UiAction::Command(Command::Workflow(
                WorkflowCommand::MoveDevices {
                    keys: moving,
                    to,
                    route: false,
                },
            )));
        }
    } else if list.iter().position(|d| **d == key) != Some(to) {
        out.push(UiAction::Command(Command::MoveDevice { key, to }));
    }
}

/// [`flat_move`] inside the unfolded container `container` of the flat document `doc` (keys
/// are path keys).
fn inner_move(
    doc: &RackDoc,
    container: &DeviceKey,
    moving: &[DeviceKey],
    to: usize,
    out: &mut Vec<UiAction>,
) {
    let list: Vec<&DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| strip_of(d).is_none() && parent_of(&d.key).as_ref() == Some(container))
        .map(|d| &d.key)
        .collect();
    if let Some(first) = moving.first() {
        flat_move(&list, first.clone(), moving.to_vec(), to, out);
    }
}

/// Why a device cannot be dropped from one container into another.
const NOT_BETWEEN_CONTAINERS: &str = "A device moves between the rack and a container of the rack, not from one container into another";

/// What a refused move says when the devices come from different containers.
const ONE_CONTAINER_AT_A_TIME: &str =
    "Devices of different containers move one container at a time";

/// Whether moving `moving` to `slot` changes the rack (the model rejects a move that does
/// not): simulates the move on (key, column) pairs, with the same clamps as the model.
fn move_changes(
    doc: &RackDoc,
    moving: &[DeviceKey],
    slot: RackSlot,
    locked: Option<&DeviceKey>,
) -> bool {
    let now: Vec<(&DeviceKey, u16)> = doc
        .devices
        .iter()
        .filter(|d| !is_inner(&d.key))
        .map(|d| (&d.key, d.column))
        .collect();
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

/// The kind of device `desc` is for replacing: only an instrument, an effect and a note player
/// can replace or be replaced, and the console devices never.
fn replace_family(desc: &atomik_device_sdk::DeviceDescriptor) -> Option<&'static str> {
    use atomik_device_sdk::DeviceKind;
    if atomik_model::console::is_console_type(desc.type_id) {
        return None;
    }
    if atomik_model::is_note_player(desc) {
        return Some("player");
    }
    match desc.kind {
        DeviceKind::Instrument => Some("instrument"),
        DeviceKind::Effect => Some("effect"),
        _ => None,
    }
}
