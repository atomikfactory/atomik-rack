//! Assigning a control to a macro by dragging it onto a macro knob.
//!
//! A knob or fader of a rack panel pressed with the assign role held ([`Role::AssignMacro`],
//! `Alt+Shift` by default) does not change its value: the press starts a drag that draws a line
//! from the control to the pointer, and the macro knob of a macro bank panel under the pointer is
//! outlined. Releasing over one asks the app to drive the control's param with that macro
//! ([`MacrosAction::Assign`]); releasing anywhere else drops the gesture and changes nothing. The
//! model decides whether the param can be driven and says why when not (a container's inside, a
//! bank's own knob, a param on an automation lane).
//!
//! Only the top level of the rack takes part: inside an open container the macros are the
//! container's own and are mapped from the control's right-click menu.

use super::inline::is_inner;
use super::{Drag, Hit, RackView, UiAction};
use crate::canvas::Canvas;
use crate::keymap::{GestureClass, Role};
use crate::macros_panel::MacrosAction;
use crate::paint;
use atomik_device_sdk::ControlKind;
use atomik_model::{DeviceKey, MACRO_BANK_TYPE, macro_index_of};
use vello::kurbo::{Line, Point, Stroke};

impl RackView {
    /// Whether the assign role is held with `modifiers` (a drag from a control onto a macro).
    pub(super) fn assign_drag_wanted(&self, modifiers: crate::event::Modifiers) -> bool {
        self.roles
            .is_active(Role::AssignMacro, modifiers, GestureClass::DragControl)
    }

    /// Starts the drag from control `slot` of device view `i`; false (nothing started) when the
    /// control cannot be assigned from here: inside a container, a macro bank's own knob, or a
    /// display.
    pub(super) fn assign_down(&mut self, i: usize, slot: usize) -> bool {
        let Some((p, kind, _)) = self.param_of(i, slot) else {
            return false;
        };
        let d = &self.devices[i];
        let bank = d.desc.is_some_and(|desc| desc.type_id == MACRO_BANK_TYPE);
        if !self.scope.is_empty()
            || is_inner(&d.key)
            || bank
            || kind == ControlKind::Display
            || atomik_model::console::is_internal_param(p.key)
        {
            return false;
        }
        let from = self.view_to_rack(self.pointer);
        self.drag = Drag::Assign {
            device: d.key.clone(),
            param: p.key.to_owned(),
            from,
            at: from,
        };
        self.repaint = true;
        true
    }

    /// The macro knob under `view_pos`: its bank and macro (0-based).
    pub(super) fn macro_knob_at(&self, view_pos: Point) -> Option<(DeviceKey, u8)> {
        let Some(Hit::Control(j, slot)) = self.hit(view_pos) else {
            return None;
        };
        let d = self.devices.get(j)?;
        if d.desc?.type_id != MACRO_BANK_TYPE {
            return None;
        }
        let (p, _, _) = self.param_of(j, slot)?;
        Some((d.key.clone(), macro_index_of(p.key)?))
    }

    /// The end of the drag: a release over a macro knob assigns the control to it.
    pub(super) fn assign_up(
        &mut self,
        device: &DeviceKey,
        param: &str,
        view_pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        if let Some((bank, index)) = self.macro_knob_at(view_pos) {
            out.push(UiAction::Macros(MacrosAction::Assign {
                bank,
                index,
                device: device.clone(),
                param: param.to_owned(),
            }));
        }
    }

    /// The line from the control to the pointer and the outline of the macro knob it would land
    /// on (rack coordinates).
    pub(super) fn paint_assign_drag(&self, c: &mut dyn Canvas) {
        let Drag::Assign { from, at, .. } = &self.drag else {
            return;
        };
        let theme = &self.theme;
        let target = self.macro_knob_at(self.pointer).and_then(|(bank, index)| {
            let j = self.device_index(&bank)?;
            let d = &self.devices[j];
            let desc = d.desc?;
            let pc = d.spec.front.iter().find(|pc| {
                macro_index_of(desc.params[d.spec.param_index(&desc, pc, d.pad)].key) == Some(index)
            })?;
            Some(pc.rect + d.offset())
        });
        let end = target.map_or(*at, |r| r.center());
        c.stroke(
            &Line::new(*from, end).into(),
            &Stroke::new(2.0),
            &theme.highlight.with_alpha(0.9).into(),
        );
        if let Some(r) = target {
            paint::outline(c, r.inset(-2.0), 4.0, 2.5, theme.highlight);
        }
    }
}
