//! The name of a device, typed in place on its plate: a double-click on the name opens a field
//! (the editor of the pattern names), Enter commits `Command::Rename`, Escape cancels, a press
//! anywhere else commits. A blank name asks for the type's default name (the model decides it).
//! The Master Section, whose name is fixed, and the interface have no name to edit.

use super::{Hit, RackView, Rename, RenameKind, UiAction};
use crate::event::{Key, Modifiers};
use crate::seq_panel::{self, RenameKey};
use atomik_model::{Command, DeviceKey};
use vello::kurbo::Point;

impl RackView {
    /// Centre of the name on the plate of a device (logical view coordinates), where a
    /// double-click types a new name; `None` for the interface and the Master Section.
    pub fn name_center(&self, key: &DeviceKey) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if d.is_interface() || self.locked.as_ref() == Some(&d.key) {
            return None;
        }
        let r =
            crate::paint::name_rect_for(d.height, d.folded, d.has_strip_bypass(), d.has_preview());
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// The name being typed in place on the plate of `key`.
    pub fn renaming_device(&self, key: &DeviceKey) -> Option<&str> {
        let r = self
            .rename
            .as_ref()
            .filter(|r| r.kind == RenameKind::Device)?;
        (self.devices.get(r.dev)?.key == self.local_key(key)).then_some(r.field.text())
    }

    /// Opens the field on the name of device `i`, holding its current name.
    pub(super) fn begin_device_rename(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        self.rename = Some(Rename {
            dev: i,
            index: 0,
            field: seq_panel::name_field(self.devices[i].plate_name()),
            kind: RenameKind::Device,
        });
        self.bump(i);
    }

    /// One key press while a device name is typed.
    pub(super) fn device_rename_key(
        &mut self,
        mut r: Rename,
        key: &Key,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        match seq_panel::rename_key(&mut r.field, key, modifiers) {
            RenameKey::Editing => {
                let dev = r.dev;
                self.rename = Some(r);
                self.bump(dev);
            }
            RenameKey::Commit => self.commit_device_rename(&r, out),
            RenameKey::Cancel => self.bump(r.dev),
        }
    }

    /// The pointer in the panel coordinates of the device whose name is being typed.
    fn rename_local(&self, pos: Point) -> Option<Point> {
        let r = self.rename.as_ref()?;
        let d = self.devices.get(r.dev)?;
        Some(self.view_to_rack(pos) - d.offset())
    }

    /// A press on the name being typed (any kind of name): the caret goes there, a double click
    /// selects a word and a drag selects. True when the press was inside the field.
    pub(super) fn rename_field_press(
        &mut self,
        pos: Point,
        clicks: u8,
        shift: bool,
        button: crate::event::PointerButton,
    ) -> bool {
        let Some(local) = self.rename_local(pos) else {
            return false;
        };
        let Some(r) = &mut self.rename else {
            return false;
        };
        if !r.field.painted_rect().contains(local) {
            return false;
        }
        if button == crate::event::PointerButton::Secondary {
            return r.field.right_click(local);
        }
        if button != crate::event::PointerButton::Primary {
            return true;
        }
        r.field.press(local, clicks, shift);
        let dev = r.dev;
        self.rename_drag = true;
        self.bump(dev);
        true
    }

    /// A pointer move while the mouse selects in the name being typed. True when it did.
    pub(super) fn rename_field_drag(&mut self, pos: Point) -> bool {
        if !self.rename_drag {
            return false;
        }
        if let Some(local) = self.rename_local(pos)
            && let Some(r) = &mut self.rename
        {
            r.field.drag(local);
            let dev = r.dev;
            self.bump(dev);
        }
        true
    }

    /// A press while a device name is typed: on the field it keeps editing (true: the press is
    /// taken), anywhere else it commits the name and the press goes on.
    pub(super) fn device_rename_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        let Some(dev) = self
            .rename
            .as_ref()
            .filter(|r| r.kind == RenameKind::Device)
            .map(|r| r.dev)
        else {
            return false;
        };
        if self.hit(pos) == Some(Hit::Name(dev)) {
            return true;
        }
        if let Some(r) = self.rename.take() {
            self.commit_device_rename(&r, out);
        }
        false
    }

    /// Ends the edit of `r`, renaming the device unless the name is unchanged.
    fn commit_device_rename(&mut self, r: &Rename, out: &mut Vec<UiAction>) {
        self.bump(r.dev);
        let d = &self.devices[r.dev];
        let name = r.field.text().trim();
        if name != d.plate_name() {
            out.push(UiAction::Command(Command::Rename {
                key: d.name_target().clone(),
                name: name.to_string(),
            }));
        }
    }
}
