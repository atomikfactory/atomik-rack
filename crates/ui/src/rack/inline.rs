//! Unfolded containers: the inside of a container drawn in place, under its panel, in the same
//! rack column.
//!
//! # How an unfolded container is drawn
//!
//! A container whose [`atomik_model::ContainerDoc::open`] flag is set is shown **inline**. The
//! rack view works on a **flat document** ([`flatten`]) instead of the project: after the
//! container's own entry come a pseudo-device `<path>/boundary` (the INPUT strip), the inner
//! devices (with path keys, `layer/synth`), the pseudo-device `<path>/boundary~out` (the OUTPUT
//! strip, the footer that closes the frame) and the inner cables with path keys (the container's
//! inside edge is `<path>/boundary`). A container nested in an unfolded container is flattened
//! the same way when it is unfolded itself. Every panel, hit test and gesture is therefore the
//! one of a top-level device; a key the view emits is already the path key the project uses, so
//! nothing is lifted (compare the scope in [`super::container_input`], which swaps the whole
//! document for the inside of one container).
//!
//! The strips are folded (half a rack unit) on the front and full height on the back, where they
//! carry the container's own jacks, so the cables between the container's sockets and its inner
//! devices are patched as in the scope. A **frame** in the container's accent colour is drawn
//! around the inner devices in the margin beside the column, one step further out per level, so
//! the inner devices read as "inside". A frame is also a drop target: a device dropped in it
//! goes inside the container.
//!
//! Positions the gestures compute (a drop slot, the place of a created device) are flat
//! positions; [`RackView::top_position`] and [`RackView::inner_position`] turn them into the
//! top-level or the container-local index the commands take.

use super::{DeviceView, Flip, PluginDeviceStatus, RackView, autolayout};
use crate::cables::SocketEnd;
use crate::interface::{
    BOUNDARY_IN_TYPE_ID, BOUNDARY_OUT_TYPE_ID, boundary_in_descriptor, boundary_out_descriptor,
};
use atomik_device_sdk::{RACK_UNIT, RACK_WIDTH};
use atomik_model::{BOUNDARY_KEY, Cable, DeviceKey, RackDevice, RackDoc, SocketRef, split_path};
use std::collections::BTreeMap;
use vello::kurbo::Point;
use vello::kurbo::{Rect, RoundedRect, Stroke};
use vello::peniko::Color;

/// How far a frame sits from the column per level, rack units of length. Three levels at most
/// fit the margin of the rack and half the gap between two columns.
pub(super) const FRAME_STEP: f64 = 5.0;
/// Stroke width of a frame.
const FRAME_WIDTH: f64 = 2.5;

/// The frame around the inside of one unfolded container.
#[derive(Debug, Clone)]
pub(super) struct Frame {
    /// The container (path key).
    pub container: DeviceKey,
    /// How deep the container is (1 at the top level): the frames of deeper containers sit
    /// closer to the panels.
    pub depth: usize,
    /// The part of the column the inside covers: from the bottom of the container's panel to
    /// the bottom of its OUTPUT strip, one panel wide.
    pub rect: Rect,
    /// The container's accent colour.
    pub accent: Color,
}

impl Frame {
    /// The frame drawn around [`Self::rect`]: grown by its distance from the column.
    pub fn outline(&self) -> Rect {
        let grow = FRAME_STEP * (4 - self.depth.min(3)) as f64;
        self.rect.inflate(grow, 0.0).inflate(0.0, 2.0)
    }
}

/// Whether `key` names a device inside a container (a path key).
pub(super) fn is_inner(key: &DeviceKey) -> bool {
    key.0.contains('/')
}

/// The container a path key lives in (`None` for a top-level key). The two strips of an
/// unfolded container live in it too.
pub(super) fn parent_of(key: &DeviceKey) -> Option<DeviceKey> {
    split_path(key).0
}

/// Which strip a flat entry is: `Some(false)` the INPUT strip, `Some(true)` the OUTPUT strip,
/// `None` for a real device.
pub(super) fn strip_of(d: &RackDevice) -> Option<bool> {
    match d.type_id.as_str() {
        BOUNDARY_IN_TYPE_ID => Some(false),
        BOUNDARY_OUT_TYPE_ID => Some(true),
        _ => None,
    }
}

/// Whether a flat entry is a container that is unfolded.
pub(super) fn is_open(d: &RackDevice) -> bool {
    d.inner.as_ref().is_some_and(|i| i.open)
}

/// The key a strip's sockets are addressed by: the container's `<path>/boundary`.
fn strip_socket_key(container: &str) -> DeviceKey {
    DeviceKey(format!("{container}/{BOUNDARY_KEY}"))
}

fn strip_entry(container: &str, out: bool, column: u16) -> RackDevice {
    let key = if out {
        format!("{container}/{BOUNDARY_KEY}~out")
    } else {
        format!("{container}/{BOUNDARY_KEY}")
    };
    RackDevice {
        key: DeviceKey(key),
        type_id: if out {
            BOUNDARY_OUT_TYPE_ID
        } else {
            BOUNDARY_IN_TYPE_ID
        }
        .to_string(),
        name: "Container".to_string(),
        params: BTreeMap::new(),
        folded: true,
        color: None,
        zones: Vec::new(),
        steps: None,
        curves: None,
        drums: None,
        inner: None,
        plugin: None,
        column,
        patch_name: String::new(),
        midi_out: None,
    }
}

/// An end of an inner cable of the container at `container`, as the flat document names it.
fn flat_socket(container: &str, s: &SocketRef) -> SocketRef {
    SocketRef {
        device: DeviceKey(format!("{container}/{}", s.device.0)),
        socket: s.socket.clone(),
    }
}

fn push_entry(
    d: &RackDevice,
    prefix: Option<&str>,
    column: u16,
    out: &mut Vec<RackDevice>,
    cables: &mut Vec<Cable>,
) {
    let mut entry = d.clone();
    if let Some(p) = prefix {
        entry.key = DeviceKey(format!("{p}/{}", d.key.0));
    }
    entry.column = column;
    let path = entry.key.0.clone();
    out.push(entry);
    let Some(inner) = d.inner.as_ref().filter(|i| i.open) else {
        return;
    };
    out.push(strip_entry(&path, false, column));
    for child in &inner.devices {
        push_entry(child, Some(&path), column, out, cables);
    }
    out.push(strip_entry(&path, true, column));
    cables.extend(inner.cables.iter().map(|c| Cable {
        from: flat_socket(&path, &c.from),
        to: flat_socket(&path, &c.to),
    }));
}

/// Replaces the devices and cables of `doc` by the flat form (see the module documentation).
/// Returns whether anything is unfolded; `doc` is left alone when nothing is.
pub(super) fn flatten(doc: &mut RackDoc) -> bool {
    if !doc.devices.iter().any(is_open) {
        return false;
    }
    let mut devices = Vec::with_capacity(doc.devices.len() + 8);
    let mut cables = doc.cables.clone();
    for d in &doc.devices {
        push_entry(d, None, d.column, &mut devices, &mut cables);
    }
    doc.devices = devices;
    doc.cables = cables;
    true
}

impl RackView {
    /// Whether the rack shows an unfolded container (the view works on the flat document).
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    /// The frames of the unfolded containers, outermost first: (container path, outline in rack
    /// coordinates).
    pub fn frames(&self) -> Vec<(DeviceKey, Rect)> {
        self.frames
            .iter()
            .map(|f| (f.container.clone(), f.outline()))
            .collect()
    }

    /// The height of the strips of an unfolded container: half a unit on the front (the INPUT
    /// strip heads the inside, the OUTPUT strip closes it), a whole one on the back where the
    /// jacks are.
    pub(super) fn strip_folded(&self) -> bool {
        self.flip == Flip::Front
    }

    /// The view of a strip: its sockets are the container's `<path>/boundary`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn strip_view(
        &mut self,
        d: &RackDevice,
        out: bool,
        x: f64,
        y: f64,
        column: u16,
        prev: Option<DeviceView>,
    ) -> DeviceView {
        let desc = if out {
            boundary_out_descriptor()
        } else {
            boundary_in_descriptor()
        };
        let folded = self.strip_folded();
        let spec = self.spec_for(desc.type_id, Some(&desc), false);
        let cache_id = self.cache_id(&d.key);
        let container = split_path(&d.key).0.map_or_else(String::new, |k| k.0);
        let height = if folded {
            f64::from(RACK_UNIT) / 2.0
        } else {
            spec.height()
        };
        let (revision, lists) = match prev {
            Some(p) if p.folded == folded => (p.revision, p.lists),
            Some(p) => (p.revision + 1, [None, None]),
            None => (self.epoch, [None, None]),
        };
        DeviceView {
            key: d.key.clone(),
            name: "Container".into(),
            caption: if out { "OUTPUT" } else { "INPUT" }.into(),
            accent: self.theme.accent_for_type(desc.type_id),
            desc: Some(desc),
            device: None,
            spec,
            samples: false,
            pad: 1,
            pad_names: Vec::new(),
            pad_cabled: [false; 16],
            sens: autolayout::DEFAULT_SLICE_SENSITIVITY,
            edit: 0,
            page: 0,
            fx_mode: 0,
            fx: None,
            meters: Vec::new(),
            scope: crate::modsrc_panel::Scope::new(),
            sample: None,
            analyser: Default::default(),
            master: Default::default(),
            sc: None,
            cv: [false; 2],
            anim: 0,
            live: 0,
            mini: None,
            driven: Vec::new(),
            automated: Vec::new(),
            has_channel: false,
            plugin_status: PluginDeviceStatus::default(),
            desc_id: 0,
            socket_device: Some(strip_socket_key(&container)),
            x,
            y,
            column,
            height,
            folded,
            cache_id,
            revision,
            lists,
        }
    }

    /// Adds the params the macros of the unfolded containers drive to `driven` (keys are path
    /// keys): `doc` is the project's own top level, before it is flattened.
    pub(super) fn compute_inline_driven(&mut self, doc: &RackDoc) {
        fn walk(
            devices: &[RackDevice],
            prefix: Option<&str>,
            driven: &mut BTreeMap<DeviceKey, Vec<(String, u8, bool)>>,
        ) {
            for d in devices {
                let path = match prefix {
                    Some(p) => format!("{p}/{}", d.key.0),
                    None => d.key.0.clone(),
                };
                let Some(inner) = d.inner.as_ref().filter(|i| i.open) else {
                    continue;
                };
                for (m, md) in inner.macros.iter().enumerate() {
                    for t in &md.targets {
                        let abs = DeviceKey(format!("{path}/{}", t.device.0));
                        let here = parent_of(&abs).is_some_and(|p| p.0 == path);
                        driven
                            .entry(abs)
                            .or_default()
                            .push((t.param.clone(), m as u8, here));
                    }
                }
                walk(&inner.devices, Some(&path), driven);
            }
        }
        walk(&doc.devices, None, &mut self.driven);
    }

    /// Computes the frames from the laid-out views (see [`Frame`]).
    pub(super) fn compute_frames(&mut self, doc: &RackDoc) {
        self.frames.clear();
        if !self.inline {
            return;
        }
        for d in doc.devices.iter().filter(|d| is_open(d)) {
            let Some(head) = self.devices.iter().find(|v| v.key == d.key) else {
                continue;
            };
            let prefix = format!("{}/", d.key.0);
            let bottom = self
                .devices
                .iter()
                .filter(|v| v.key.0.starts_with(&prefix))
                .map(|v| v.y + v.height)
                .fold(head.y + head.height, f64::max);
            self.frames.push(Frame {
                container: d.key.clone(),
                depth: super::inline::depth_of(&d.key),
                rect: Rect::new(
                    head.x,
                    head.y + head.height,
                    head.x + f64::from(RACK_WIDTH),
                    bottom,
                ),
                accent: head.accent,
            });
        }
    }

    // ------------------------------------------------------------------ cables

    /// Whether a cable may join the jacks `a` and `b`: the kinds and directions agree (see
    /// [`crate::cables::compatible`]) and both are in the same container (the rack's own
    /// devices share the top level; a container's own jacks and its INPUT and OUTPUT strips
    /// belong to the scope of the container's inside, as the model has it).
    pub(super) fn can_cable(&self, a: &SocketEnd, b: &SocketEnd) -> bool {
        crate::cables::compatible(a, b) && parent_of(&a.at.device) == parent_of(&b.at.device)
    }

    // ------------------------------------------------------------------ positions

    /// The project itself while a gesture runs on an unfolded rack: the view's own document is
    /// the flat one, but a command is planned on the project. Without it (the top level or a
    /// scope) the document the view was given is the project's.
    pub(super) fn host<'a>(&'a self, view: &'a RackDoc) -> &'a RackDoc {
        if self.inline {
            self.host_doc.as_ref().unwrap_or(view)
        } else {
            view
        }
    }

    /// The innermost frame whose inside holds the rack point `rp` (a frame also takes the
    /// margin around it, so a drop a little outside still counts), not counting the frames of
    /// the devices being moved or of anything inside them.
    pub(super) fn frame_at(&self, rp: Point, moving: &[DeviceKey]) -> Option<&Frame> {
        self.frames
            .iter()
            .filter(|f| {
                !moving
                    .iter()
                    .any(|m| f.container == *m || f.container.0.starts_with(&format!("{}/", m.0)))
            })
            .filter(|f| f.outline().contains(rp))
            .max_by_key(|f| f.depth)
    }

    /// The flags of the views that move with `moving`: the devices themselves and, for an
    /// unfolded container, everything drawn inside it.
    pub(super) fn moving_flags(&self, moving: &[DeviceKey]) -> Vec<bool> {
        self.devices
            .iter()
            .map(|d| {
                moving
                    .iter()
                    .any(|m| d.key == *m || d.key.0.starts_with(&format!("{}/", m.0)))
            })
            .collect()
    }

    /// Where the block of device view `i` ends: the bottom of its panel, or of its frame when
    /// it is an unfolded container (the inside moves and drops with it).
    pub(super) fn block_bottom(&self, i: usize) -> f64 {
        let d = &self.devices[i];
        self.frames
            .iter()
            .find(|f| f.container == d.key)
            .map_or(d.y + d.height, |f| f.rect.y1)
    }

    /// The place of a drop at `rp` inside the container `container` while the views flagged
    /// in `moving` are dragged: how many of its devices that stay lie above the pointer (the
    /// index the commands take, as the container's own list without the moved ones), the
    /// height of the drop line and the column.
    pub(super) fn inside_drop(
        &self,
        container: &DeviceKey,
        rp: Point,
        moving: &[bool],
    ) -> (usize, f64, u16) {
        let mut line_y = self
            .devices
            .iter()
            .find(|d| d.key.0 == format!("{}/{BOUNDARY_KEY}", container.0))
            .map_or(0.0, |d| d.y + d.height);
        let mut above = 0;
        let mut column = 0;
        for (i, d) in self.devices.iter().enumerate() {
            if parent_of(&d.key).as_ref() != Some(container) || d.is_interface() {
                continue;
            }
            column = d.column;
            if moving.get(i).copied().unwrap_or(false) {
                continue;
            }
            if (d.y + self.block_bottom(i)) / 2.0 < rp.y {
                line_y = self.block_bottom(i);
                above += 1;
            }
        }
        if let Some(c) = self.devices.iter().find(|d| d.key == *container) {
            column = c.column;
        }
        (above, line_y, column)
    }

    /// Paints the frames behind the panels (rack coordinates, clipped to `top..bottom`).
    pub(super) fn paint_frames(&self, c: &mut dyn crate::canvas::Canvas, top: f64, bottom: f64) {
        use crate::canvas::CanvasExt;
        for f in &self.frames {
            let r = f.outline();
            if r.y1 < top || r.y0 > bottom {
                continue;
            }
            let shape = RoundedRect::from_rect(r, 5.0);
            c.fill_with(shape, f.accent.with_alpha(0.07));
            c.stroke(
                &shape.into(),
                &Stroke::new(FRAME_WIDTH),
                &f.accent.with_alpha(0.8).into(),
            );
        }
    }
}

/// How many `/` the path key holds plus one for a container: the depth of its inside.
pub(super) fn depth_of(container: &DeviceKey) -> usize {
    container.0.matches('/').count() + 1
}
