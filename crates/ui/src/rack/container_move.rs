//! Combine, Uncombine and the moves in and out of containers, on the rack view's side: the
//! reasons the menus show for a refusal, the drag of a device onto a container (it goes inside),
//! and the drag of a device of an open container onto the breadcrumb's project segment (it goes
//! back to the rack).
//!
//! The model plans every one of these commands on a scratch copy of the document, so the
//! reasons are the model's own messages: the view asks it instead of repeating its rules.

use super::{RackView, UiAction};
use crate::breadcrumb::{self, CrumbHit};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::container_panel::CONTAINER_TYPE;
use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_model::{Command, ContainerCommand, DeviceKey, Document, RackDoc};
use vello::kurbo::{Point, Rect, RoundedRect, Stroke};

/// How far the top and the bottom of a container's panel are kept for placing a device above
/// or below it (a drop on the rest of the panel goes inside).
const EDGE: f64 = 14.0;

/// What a refused move says when a group is dragged.
const ONE_AT_A_TIME: &str = "Move devices into or out of a container one at a time, or select \
     them and use Combine";

impl RackView {
    // ------------------------------------------------------------------ reasons

    /// Why Combine cannot work on the selection (`None`: it can). The model plans the command
    /// on a scratch copy of `doc`, so a refusal reads as the model's message naming the cables
    /// or the limit; the menus enable their items from this.
    pub fn combine_reason(&self, doc: &RackDoc, registry: &Registry) -> Option<String> {
        if !self.scope.is_empty() {
            return Some("Combine works on the top level of the rack".into());
        }
        let keys = self.local_selection();
        if keys.is_empty() {
            return Some("Select the devices to combine first".into());
        }
        plan_reason(
            doc,
            registry,
            Command::Container(ContainerCommand::Combine {
                keys,
                name: String::new(),
            }),
        )
    }

    /// Why Uncombine cannot work on the selection (`None`: it can).
    pub fn uncombine_reason(&self, doc: &RackDoc, registry: &Registry) -> Option<String> {
        if !self.scope.is_empty() {
            return Some("Uncombine works on a container of the top level of the rack".into());
        }
        let container = self
            .local_selection()
            .into_iter()
            .find(|k| doc.inner(k).is_some());
        let Some(container) = container else {
            return Some("Select a container to uncombine".into());
        };
        plan_reason(
            doc,
            registry,
            Command::Container(ContainerCommand::Uncombine { container }),
        )
    }

    /// Why Move Out of Container cannot work on the selection (`None`: it can): one device of
    /// an open container goes back to the rack, after the container.
    pub fn move_out_reason(&self, doc: &RackDoc, registry: &Registry) -> Option<String> {
        if self.scope.is_empty() {
            return Some("Open a container to move a device out of it".into());
        }
        let keys = self.local_selection();
        let [key] = keys.as_slice() else {
            return Some(if keys.is_empty() {
                "Select the device to move out of the container first".into()
            } else {
                "Move devices out of a container one at a time".into()
            });
        };
        plan_reason(
            doc,
            registry,
            Command::Container(ContainerCommand::MoveOutOfContainer {
                key: self.abs_key(key),
                index: self.scope_rack_at,
            }),
        )
    }

    /// Where a device that leaves the open container lands among the devices of the rack: right
    /// after the container.
    pub fn move_out_index(&self) -> usize {
        self.scope_rack_at
    }

    // ------------------------------------------------------------------ the drag

    /// The container whose panel is under the rack point `rp` while the devices flagged in
    /// `moving` are dragged: a drop there puts the device inside. Only the top level has such
    /// targets, the top and bottom edge of a panel stay free for placing a device next to it,
    /// and the dragged devices themselves are never targets.
    pub(super) fn container_zone_at(&self, rp: Point, moving: &[bool]) -> Option<usize> {
        if !self.scope.is_empty() {
            return None;
        }
        self.inner_range().find(|&i| {
            let d = &self.devices[i];
            let container = d.desc.as_ref().is_some_and(|x| x.type_id == CONTAINER_TYPE);
            if !container || moving.get(i).copied().unwrap_or(false) {
                return false;
            }
            let edge = EDGE.min(d.height / 4.0);
            Rect::new(
                d.x,
                d.y + edge,
                d.x + f64::from(RACK_WIDTH),
                d.y + d.height - edge,
            )
            .contains(rp)
        })
    }

    /// Whether the pointer is on the part of the breadcrumb that stands for the rack, in a
    /// container of the top level: a device dropped there leaves the container.
    pub(super) fn crumb_drop_out(&self) -> bool {
        self.scope.len() == 1
            && matches!(
                breadcrumb::hit(self.bar_rect(), &self.breadcrumb_labels(), self.pointer),
                Some(CrumbHit::Back | CrumbHit::Segment(0))
            )
    }

    /// The release of a device drag over a container's panel or over the breadcrumb: the move
    /// command. Returns whether the drop was one of those (and so is not a move within the rack).
    pub(super) fn container_drop(
        &mut self,
        key: &DeviceKey,
        moving: &[DeviceKey],
        flags: &[bool],
        out: &mut Vec<UiAction>,
    ) -> bool {
        if self.scope.is_empty() {
            let rp = self.view_to_rack(self.pointer);
            let Some(target) = self.container_zone_at(rp, flags) else {
                return false;
            };
            if moving.len() > 1 {
                // A group over a container is placed like any other move.
                return false;
            }
            out.push(UiAction::Command(Command::Container(
                ContainerCommand::MoveIntoContainer {
                    key: key.clone(),
                    container: self.devices[target].key.clone(),
                    index: usize::MAX,
                },
            )));
            return true;
        }
        if !self.crumb_drop_out() {
            return false;
        }
        if moving.len() > 1 {
            self.toast = Some(ONE_AT_A_TIME.to_string());
            return true;
        }
        // The container's path is in the key already, so the command is not lifted.
        self.direct.push(UiAction::Command(Command::Container(
            ContainerCommand::MoveOutOfContainer {
                key: self.abs_key(key),
                index: self.scope_rack_at,
            },
        )));
        true
    }

    /// The feedback of a device drag over a drop target of a container: the container's panel
    /// is outlined with what the drop does. Returns whether it drew (the drop line is then not).
    pub(super) fn paint_container_drop(&self, c: &mut dyn Canvas, moving: &[bool]) -> bool {
        // A group is placed like any other move, so it shows the drop line.
        if moving.iter().filter(|m| **m).count() != 1 {
            return false;
        }
        if self.crumb_drop_out() {
            return true;
        }
        let rp = self.view_to_rack(self.pointer);
        let Some(target) = self.container_zone_at(rp, moving) else {
            return false;
        };
        let theme = &self.theme;
        let d = &self.devices[target];
        let rect = d.rect().inset(-1.0);
        c.fill_with(
            RoundedRect::from_rect(rect, 6.0),
            theme.highlight.with_alpha(0.12),
        );
        c.stroke(
            &RoundedRect::from_rect(rect, 6.0).into(),
            &Stroke::new(2.0),
            &theme.highlight.into(),
        );
        c.text(
            &TextRun::new(
                "Move into container",
                pt(rect.center().x, rect.center().y + 4.0),
                12.0,
                theme.highlight,
            )
            .weight(600)
            .align(Align::Center),
        );
        true
    }
}

/// The model's refusal of `command` planned on a scratch copy of `doc`, `None` when it takes.
fn plan_reason(doc: &RackDoc, registry: &Registry, command: Command) -> Option<String> {
    Document::new(doc.clone())
        .apply(command, registry)
        .err()
        .map(|e| e.to_string())
}
