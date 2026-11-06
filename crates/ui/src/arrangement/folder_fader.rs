//! The level bar on the header of a folder in the [`ArrangementView`].
//!
//! A folder that plays into a bus shows the bus fader as a thin bar along the bottom of its
//! header; a folder that a VCA group follows (and has no bus) shows the group's level on the same
//! bar. Pressing the bar and dragging sets the level from the pointer (one drag is one undo
//! step, Esc puts the level back, a double-click resets it to unity). Nothing is stored in the
//! view: the bar mirrors the document, and a folder with neither a bus nor a followed group has
//! no bar.

use super::{ArrangementView, Drag, Hit};
use crate::action::UiAction;
use crate::canvas::{Canvas, CanvasExt};
use crate::seq_paint as sp;
use atomik_core::{VCA_MAX_DB, VCA_MIN_DB};
use atomik_device_sdk::Registry;
use atomik_model::{Command, ConsoleCommand, DeviceKey, RackDoc};
use std::collections::BTreeMap;
use vello::kurbo::{Point, Rect, RoundedRect};

/// The fader position of 0 dB on a console fader (its cubic taper reaches unity there).
const BUS_UNITY: f32 = 0.75;
/// Height of the hit zone along the bottom of the header, and of the bar inside it.
const ZONE: f64 = 8.0;
const BAR: f64 = 3.0;
/// What the level of a followed group moves by, in dB.
const VCA_STEP_DB: f32 = 0.5;

/// What a folder's bar sets.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    /// The fader of the bus the folder plays into.
    Bus(DeviceKey),
    /// The level of the VCA group that follows the folder.
    Vca(u32),
}

/// The bar of one folder.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FolderFader {
    target: Target,
    /// The level on the bar, `0..=1`.
    pos: f32,
    /// Where unity sits on the bar.
    unity: f32,
}

/// A drag of a folder's bar.
#[derive(Debug, Clone)]
pub(super) struct FaderDrag {
    group: u32,
    target: Target,
    merge: u64,
    original: f32,
    last: f32,
    emitted: bool,
}

/// The bar position of a followed group's level in dB.
fn vca_pos(db: f32) -> f32 {
    ((db - VCA_MIN_DB) / (VCA_MAX_DB - VCA_MIN_DB)).clamp(0.0, 1.0)
}

/// The bars of the folders in `doc`: the bus fader of a routed folder, else the level of the
/// group that follows it.
pub(super) fn faders_of(doc: &RackDoc, registry: &Registry) -> BTreeMap<u32, FolderFader> {
    let mut out = BTreeMap::new();
    for r in &doc.console.folder_routes {
        if let Some(pos) = doc.param_value(&r.bus, "fader", registry) {
            out.insert(
                r.folder,
                FolderFader {
                    target: Target::Bus(r.bus.clone()),
                    pos: pos.clamp(0.0, 1.0),
                    unity: BUS_UNITY,
                },
            );
        }
    }
    for v in &doc.console.vcas {
        if let Some(folder) = v.follows {
            out.entry(folder).or_insert_with(|| FolderFader {
                target: Target::Vca(v.id),
                pos: vca_pos(v.level_db),
                unity: vca_pos(0.0),
            });
        }
    }
    out
}

fn command(target: &Target, pos: f32, merge: Option<u64>) -> UiAction {
    UiAction::Command(match target {
        Target::Bus(bus) => Command::SetParam {
            key: bus.clone(),
            param: "fader".to_owned(),
            value: pos,
            merge,
        },
        Target::Vca(vca) => {
            let db = VCA_MIN_DB + pos * (VCA_MAX_DB - VCA_MIN_DB);
            let db = (db / VCA_STEP_DB).round() * VCA_STEP_DB;
            Command::Console(ConsoleCommand::SetVcaLevel {
                vca: *vca,
                db: if db.abs() < 0.01 { 0.0 } else { db },
                merge,
            })
        }
    })
}

impl ArrangementView {
    /// The bar of folder `group` in its header row `hr`, when it has one.
    pub(super) fn folder_fader_rect(&self, hr: Rect, group: u32) -> Option<Rect> {
        self.folder_faders.get(&group)?;
        let (x, w) = self.folder_name_box(hr, group);
        Some(Rect::new(x, hr.y1 - ZONE, x + w, hr.y1))
    }

    /// The area of the bar of folder `group` in view coordinates (where a press sets the level),
    /// when it has one and its row is shown.
    pub fn folder_fader_bar(&self, group: u32) -> Option<Rect> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == super::RowKind::Folder { group })?;
        self.folder_fader_rect(self.row_rect(row), group)
    }

    /// The hit of the bar of `group` under `pos`.
    pub(super) fn folder_fader_hit(&self, group: u32, hr: Rect, pos: Point) -> Option<Hit> {
        self.folder_fader_rect(hr, group)
            .filter(|r| r.contains(pos))
            .map(|_| Hit::FolderFader(group))
    }

    /// What the bar of `group` sets, for the tooltip.
    pub(super) fn folder_fader_help(&self, group: u32) -> Option<&'static str> {
        Some(match self.folder_faders.get(&group)?.target {
            Target::Bus(_) => {
                "Folder level: the fader of its bus. Drag to set, double-click for unity"
            }
            Target::Vca(_) => {
                "Folder level: the level of its VCA group. Drag to set, double-click for unity"
            }
        })
    }

    /// Whether `group` plays into a bus (what Stop Routing Folder needs).
    pub(super) fn folder_routed(&self, group: u32) -> bool {
        matches!(
            self.folder_faders.get(&group).map(|f| &f.target),
            Some(Target::Bus(_))
        )
    }

    /// The position on the bar at pointer x.
    fn fader_pos_at(&self, group: u32, x: f64) -> Option<f32> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == super::RowKind::Folder { group })?;
        let bar = self.folder_fader_rect(self.row_rect(row), group)?;
        Some(((x - bar.x0) / bar.width().max(1.0)).clamp(0.0, 1.0) as f32)
    }

    /// A press on the bar: the level goes to the pointer and a drag starts; a double-click puts
    /// it at unity.
    pub(super) fn folder_fader_down(
        &mut self,
        group: u32,
        pos: Point,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some(f) = self.folder_faders.get(&group).cloned() else {
            return;
        };
        self.set_selected_group(Some(group));
        if clicks >= 2 {
            out.push(command(&f.target, f.unity, None));
            return;
        }
        let Some(first) = self.fader_pos_at(group, pos.x) else {
            return;
        };
        let merge = self.merge_id();
        out.push(command(&f.target, first, Some(merge)));
        self.drag = Drag::FolderFader(Box::new(FaderDrag {
            group,
            target: f.target,
            merge,
            original: f.pos,
            last: first,
            emitted: true,
        }));
    }

    /// The pointer moved during a drag of a bar.
    pub(super) fn folder_fader_move(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        let Drag::FolderFader(d) = &self.drag else {
            return;
        };
        let Some(value) = self.fader_pos_at(d.group, pos.x) else {
            return;
        };
        let Drag::FolderFader(d) = &mut self.drag else {
            return;
        };
        if (value - d.last).abs() < 1e-4 {
            return;
        }
        d.last = value;
        d.emitted = true;
        out.push(command(&d.target, value, Some(d.merge)));
    }

    /// The release: what was sent is one undo step.
    pub(super) fn folder_fader_up(&mut self, d: FaderDrag, out: &mut Vec<UiAction>) {
        if d.emitted {
            out.push(UiAction::EndMerge);
        }
    }

    /// Esc: the level goes back to what it was at the press.
    pub(super) fn folder_fader_cancel(&mut self, d: FaderDrag, out: &mut Vec<UiAction>) {
        if d.emitted {
            out.push(command(&d.target, d.original, Some(d.merge)));
            out.push(UiAction::EndMerge);
        }
    }

    /// Paints the bar in the header `hr` of `group`.
    pub(super) fn paint_folder_fader(&self, c: &mut dyn Canvas, hr: Rect, group: u32) {
        let Some(f) = self.folder_faders.get(&group) else {
            return;
        };
        let Some(zone) = self.folder_fader_rect(hr, group) else {
            return;
        };
        let st = &self.st;
        let bar = Rect::new(zone.x0, zone.y1 - BAR - 3.0, zone.x1, zone.y1 - 3.0);
        let accent = self.folder_accent(group);
        sp::rect(c, bar, st.base.bevel_dark);
        let fill = Rect::new(
            bar.x0,
            bar.y0,
            bar.x0 + bar.width() * f64::from(f.pos),
            bar.y1,
        );
        sp::rect(c, fill, accent);
        let unity = bar.x0 + bar.width() * f64::from(f.unity);
        sp::line(
            c,
            unity,
            bar.y0 - 1.0,
            unity,
            bar.y1 + 1.0,
            st.base.text_muted,
        );
        let hot = self.hover == Some(Hit::FolderFader(group))
            || matches!(&self.drag, Drag::FolderFader(d) if d.group == group);
        if hot {
            c.stroke_with(
                RoundedRect::from_rect(bar.inflate(1.0, 1.0), 1.0),
                1.0,
                st.base.text,
            );
        }
    }
}
