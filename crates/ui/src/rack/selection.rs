//! Rack multi-selection: a set of selected devices with a primary
//! one (the last clicked, the one the editors and the live keyboard follow), the toggle and
//! range roles, select all, the neighbour keys and the selection commands (delete, duplicate,
//! disconnect). The selection is view state: it is never in the document and never an undo
//! step. Keys in the set are the view's local keys; the public accessors give path keys.

use super::*;
use crate::keymap::{GestureClass, Role, Roles};
use atomik_model::{CreateOptions, WorkflowCommand};
use std::collections::BTreeSet;

impl RackView {
    /// The role table the click gestures follow (toggle and range selection).
    pub fn roles(&self) -> &Roles {
        &self.roles
    }

    /// Replaces the role table (the shell pushes it on every keymap change).
    pub fn set_roles(&mut self, roles: Roles) {
        self.roles = roles;
    }

    /// Whether the fine-adjust role is held with `modifiers` (a drag or a wheel notch of a
    /// control moves it by a tenth).
    /// Whether the reset role is held with `modifiers` (a click on a control sets its default).
    pub(super) fn reset_click(&self, modifiers: Modifiers) -> bool {
        self.roles
            .is_active(Role::ResetParam, modifiers, GestureClass::ClickControl)
    }

    /// Whether the edit-automation role is held with `modifiers` (a click on a control shows
    /// its automation lane).
    pub(super) fn edit_automation_click(&self, modifiers: Modifiers) -> bool {
        self.roles
            .is_active(Role::EditAutomation, modifiers, GestureClass::ClickControl)
    }

    pub(super) fn fine_adjust(&self, modifiers: Modifiers) -> bool {
        self.roles
            .is_active(Role::FineAdjust, modifiers, GestureClass::DragControl)
    }

    /// The selected devices in rack order, as the document names them (path keys inside a
    /// container).
    pub fn selection(&self) -> Vec<DeviceKey> {
        self.local_selection()
            .iter()
            .map(|k| self.abs_key(k))
            .collect()
    }

    /// How many devices are selected.
    pub fn selection_len(&self) -> usize {
        self.selection.len()
    }

    /// Whether `key` (local or path key) is selected.
    pub fn is_selected(&self, key: &DeviceKey) -> bool {
        self.selection.contains(&self.local_key(key))
    }

    /// Replaces the selection (no action emitted): `keys` that are not shown are ignored and
    /// `primary` falls back to the last of them.
    pub fn set_selection(&mut self, keys: &[DeviceKey], primary: Option<&DeviceKey>) {
        let shown: Vec<DeviceKey> = self.shown_keys();
        let set: BTreeSet<DeviceKey> = keys
            .iter()
            .map(|k| self.local_key(k))
            .filter(|k| shown.contains(k))
            .collect();
        let primary = primary
            .map(|k| self.local_key(k))
            .filter(|k| set.contains(k))
            .or_else(|| shown.iter().rev().find(|k| set.contains(*k)).cloned());
        self.anchor = primary.clone();
        self.selection = set;
        self.selected = primary;
        self.repaint = true;
    }

    /// The keys of the real devices of the view (not the boundary panels), top to bottom.
    pub(super) fn shown_keys(&self) -> Vec<DeviceKey> {
        self.devices[self.inner_range()]
            .iter()
            .filter(|d| !d.is_interface())
            .map(|d| d.key.clone())
            .collect()
    }

    /// The selection as local keys in rack order.
    pub(super) fn local_selection(&self) -> Vec<DeviceKey> {
        self.shown_keys()
            .into_iter()
            .filter(|k| self.selection.contains(k))
            .collect()
    }

    /// Drops selected keys that are no longer shown.
    pub(super) fn prune_selection(&mut self) {
        let shown = self.shown_keys();
        self.selection.retain(|k| shown.contains(k));
        if self.selected.as_ref().is_some_and(|k| !shown.contains(k)) {
            self.selected = self.local_selection().last().cloned();
        }
        if self.selected.is_none() {
            self.selected = self.local_selection().last().cloned();
        }
        if self.anchor.as_ref().is_some_and(|k| !shown.contains(k)) {
            self.anchor = self.selected.clone();
        }
    }

    /// Forgets the selection (a scope change).
    pub(super) fn clear_selection(&mut self) {
        self.selection.clear();
        self.selected = None;
        self.anchor = None;
    }

    /// The action that reports the selection: `Select` for one device, `SelectMany` for more,
    /// nothing when empty.
    pub(super) fn selection_action(&self) -> Option<UiAction> {
        let primary = self.selected.clone()?;
        let keys = self.local_selection();
        Some(if keys.len() == 1 {
            UiAction::Select(primary)
        } else {
            UiAction::SelectMany { primary, keys }
        })
    }

    fn report_selection(&mut self, out: &mut Vec<UiAction>) {
        out.extend(self.selection_action());
        self.repaint = true;
    }

    /// Selects device view `i` alone (a click on one of its controls, buttons or panels).
    pub(super) fn select_only(&mut self, i: usize, out: &mut Vec<UiAction>) {
        let key = self.devices[i].key.clone();
        if self.devices[i].is_interface()
            || (self.selected.as_ref() == Some(&key) && self.selection.len() == 1)
        {
            return;
        }
        self.selection = BTreeSet::from([key.clone()]);
        self.selected = Some(key.clone());
        self.anchor = Some(key);
        self.report_selection(out);
    }

    /// A primary click on a device panel, honouring the roles: toggle (Ctrl on Windows and
    /// Linux, Command on macOS), range (Shift) or a plain click. Returns true when the click
    /// landed on a member of a multi-selection: the selection stays for a possible group drag
    /// and collapses to the device when the pointer is released without moving.
    pub(super) fn select_click(
        &mut self,
        i: usize,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let key = self.devices[i].key.clone();
        if self.devices[i].is_interface() {
            return false;
        }
        let toggle = self
            .roles
            .is_active(Role::ToggleSelect, modifiers, GestureClass::ClickItem);
        let range = self
            .roles
            .is_active(Role::RangeSelect, modifiers, GestureClass::ClickItem);
        if toggle {
            if self.selection.remove(&key) {
                if self.selected.as_ref() == Some(&key) {
                    self.selected = self.local_selection().last().cloned();
                }
            } else {
                self.selection.insert(key.clone());
                self.selected = Some(key.clone());
            }
            self.anchor = Some(key);
            self.report_selection(out);
            return false;
        }
        if range {
            let shown = self.shown_keys();
            let anchor = self
                .anchor
                .clone()
                .filter(|a| shown.contains(a))
                .unwrap_or_else(|| key.clone());
            let a = shown.iter().position(|k| *k == anchor).unwrap_or(0);
            let b = shown.iter().position(|k| *k == key).unwrap_or(a);
            let (lo, hi) = (a.min(b), a.max(b));
            self.selection = shown[lo..=hi].iter().cloned().collect();
            self.selected = Some(key);
            self.anchor = Some(anchor);
            self.report_selection(out);
            return false;
        }
        if self.selection.len() > 1 && self.selection.contains(&key) {
            // The group may be about to move: keep it, make this the primary.
            let changed = self.selected.as_ref() != Some(&key);
            self.selected = Some(key);
            if changed {
                self.report_selection(out);
            }
            return true;
        }
        self.select_only(i, out);
        false
    }

    /// A right click on a device: a member keeps the selection, anything else becomes the
    /// selection (so the menu works on what the user pointed at).
    pub(super) fn select_for_menu(&mut self, i: usize, out: &mut Vec<UiAction>) {
        let key = self.devices[i].key.clone();
        if self.selection.contains(&key) {
            if self.selected.as_ref() != Some(&key) {
                self.selected = Some(key);
                self.report_selection(out);
            }
            return;
        }
        self.select_only(i, out);
    }

    /// Collapses a multi-selection to device view `dev` (a click released in place).
    pub(super) fn collapse_to(&mut self, dev: usize, out: &mut Vec<UiAction>) {
        let key = self.devices[dev].key.clone();
        self.selection = BTreeSet::from([key.clone()]);
        self.selected = Some(key.clone());
        self.anchor = Some(key);
        self.report_selection(out);
    }

    /// Whether the multi-selection moves as a group when device view `dev` is dragged.
    pub(super) fn drags_as_group(&self, dev: usize) -> bool {
        self.selection.len() > 1 && self.selection.contains(&self.devices[dev].key)
    }

    // ------------------------------------------------------------------ commands

    /// The selection commands. `view` is the document of the current scope; `None` means the
    /// command is not one of these.
    pub(super) fn selection_command(
        &mut self,
        id: &str,
        view: &RackDoc,
        registry: &Registry,
    ) -> Option<CommandOutcome> {
        let mut out: Vec<UiAction> = Vec::new();
        match id {
            "edit.select_all" => {
                let shown = self.shown_keys();
                let (Some(first), Some(last)) = (shown.first().cloned(), shown.last().cloned())
                else {
                    return Some(CommandOutcome::Declined);
                };
                self.selection = shown.into_iter().collect();
                self.selected = Some(last);
                self.anchor = Some(first);
                self.report_selection(&mut out);
            }
            "rack.select_next" | "rack.select_prev" => {
                if self.control_has_focus() {
                    return Some(CommandOutcome::Declined);
                }
                let forward = id == "rack.select_next";
                let Some(target) = self.neighbour(forward, false) else {
                    return Some(CommandOutcome::Declined);
                };
                self.selection = BTreeSet::from([target.clone()]);
                self.selected = Some(target.clone());
                self.anchor = Some(target);
                self.report_selection(&mut out);
            }
            "rack.extend_next" | "rack.extend_prev" => {
                if self.control_has_focus() {
                    return Some(CommandOutcome::Declined);
                }
                let forward = id == "rack.extend_next";
                let Some(target) = self.neighbour(forward, true) else {
                    return Some(CommandOutcome::Declined);
                };
                let shown = self.shown_keys();
                let anchor = self
                    .anchor
                    .clone()
                    .filter(|a| shown.contains(a))
                    .unwrap_or_else(|| target.clone());
                let a = shown.iter().position(|k| *k == anchor).unwrap_or(0);
                let b = shown.iter().position(|k| *k == target).unwrap_or(a);
                self.selection = shown[a.min(b)..=a.max(b)].iter().cloned().collect();
                self.selected = Some(target);
                self.anchor = Some(anchor);
                self.report_selection(&mut out);
            }
            "edit.delete" => {
                let keys = self.local_selection();
                // With auto-routing the removal keeps the audio path of an effect that was
                // cabled between two devices, at the top level.
                let bridge = self.auto_route && self.scope.is_empty();
                let action = match keys.as_slice() {
                    [] => return Some(CommandOutcome::Declined),
                    _ if bridge => Command::Workflow(WorkflowCommand::DeleteDevices { keys }),
                    [one] => Command::RemoveDevice { key: one.clone() },
                    _ => Command::Workflow(WorkflowCommand::RemoveDevices { keys }),
                };
                out.push(UiAction::Command(action));
            }
            "edit.duplicate" => {
                let keys = self.local_selection();
                let action = match keys.as_slice() {
                    [] => return Some(CommandOutcome::Declined),
                    [one] if view.device(one).is_some_and(|d| d.inner.is_some()) => {
                        Command::Container(ContainerCommand::DuplicateContainer {
                            container: one.clone(),
                        })
                    }
                    _ => Command::Workflow(WorkflowCommand::DuplicateDevices {
                        keys,
                        route: self.auto_route,
                    }),
                };
                out.push(UiAction::Command(action));
            }
            "rack.disconnect" => {
                let keys = self.local_selection();
                match keys.as_slice() {
                    [] => return Some(CommandOutcome::Declined),
                    [one] => {
                        let cabled = view
                            .cables
                            .iter()
                            .any(|c| &c.from.device == one || &c.to.device == one);
                        if !cabled {
                            return Some(CommandOutcome::NotApplicable(format!(
                                "{} has no cables to remove",
                                self.devices
                                    .iter()
                                    .find(|d| &d.key == one)
                                    .map_or("The device", |d| d.name.as_str())
                            )));
                        }
                        out.push(UiAction::Command(Command::Workflow(
                            WorkflowCommand::DisconnectDevice { key: one.clone() },
                        )));
                    }
                    _ => out.push(UiAction::Command(Command::Workflow(
                        WorkflowCommand::DisconnectDevices { keys },
                    ))),
                }
            }
            "rack.assign_send" => {
                let keys = self.local_selection();
                if keys.is_empty() {
                    return Some(CommandOutcome::Declined);
                }
                if let Some(reason) = self.assign_send_reason(view, registry) {
                    return Some(CommandOutcome::NotApplicable(reason));
                }
                out.push(UiAction::Command(Command::Workflow(
                    WorkflowCommand::AssignSend {
                        effect: keys[0].clone(),
                    },
                )));
            }
            "rack.auto_route" => {
                let keys = self.local_selection();
                if keys.is_empty() {
                    return Some(CommandOutcome::Declined);
                }
                if !self.scope.is_empty() {
                    return Some(CommandOutcome::NotApplicable(
                        "Auto-route works on the top level of the rack".into(),
                    ));
                }
                out.push(UiAction::Command(Command::Workflow(
                    WorkflowCommand::AutoRoute {
                        devices: keys,
                        options: CreateOptions::default(),
                    },
                )));
            }
            "rack.add_column" | "rack.remove_column" => {
                if let Some(refusal) = self.column_command(id, &mut out) {
                    return Some(refusal);
                }
            }
            _ => return None,
        }
        self.repaint = true;
        Some(CommandOutcome::Handled(self.finish(out, registry)))
    }

    fn control_has_focus(&self) -> bool {
        matches!(
            self.tree.focused().map(|n| *self.tree.data(n)),
            Some(Hit::Control(..) | Hit::PresetGrid(_))
        )
    }

    /// The device the neighbour keys go to: from the primary (the end that moves when
    /// extending), `None` when nothing would change. With nothing selected the first device
    /// (going down) or the last (going up).
    fn neighbour(&self, forward: bool, extending: bool) -> Option<DeviceKey> {
        let shown = self.shown_keys();
        if shown.is_empty() {
            return None;
        }
        let from = self
            .selected
            .as_ref()
            .and_then(|p| shown.iter().position(|k| k == p));
        match from {
            None if extending => None,
            None if forward => shown.first().cloned(),
            None => shown.last().cloned(),
            Some(at) => {
                let to = if forward {
                    (at + 1).min(shown.len() - 1)
                } else {
                    at.saturating_sub(1)
                };
                shown.get(to).cloned()
            }
        }
    }
}

impl RackView {
    /// Why "Use as Send Effect" cannot work on the selection (`None`: it can). The model plans
    /// the command on a scratch copy of `doc`, so a rejected plan reads as not applicable with
    /// the model's own reason; the context menu enables its item from this.
    pub fn assign_send_reason(&self, doc: &RackDoc, registry: &Registry) -> Option<String> {
        let keys = self.local_selection();
        match keys.as_slice() {
            [] => return Some("Nothing is selected".to_string()),
            [_] => {}
            _ => return Some("Use as Send Effect works on one effect at a time".to_string()),
        }
        if !self.scope.is_empty() {
            return Some("Use as Send Effect works on the top level of the rack".to_string());
        }
        let command = Command::Workflow(WorkflowCommand::AssignSend {
            effect: keys[0].clone(),
        });
        atomik_model::Document::new(doc.clone())
            .apply(command, registry)
            .err()
            .map(|e| e.to_string())
    }

    /// A click on the empty rack clears the selection.
    pub(super) fn clear_selection_by_click(&mut self) {
        if !self.selection.is_empty() || self.selected.is_some() {
            self.clear_selection();
            self.repaint = true;
        }
    }
}

impl RackView {
    /// The primary selected device (the last clicked), as the document names it.
    pub fn primary(&self) -> Option<DeviceKey> {
        self.selected.as_ref().map(|k| self.abs_key(k))
    }
}
