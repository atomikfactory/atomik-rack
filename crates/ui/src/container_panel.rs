//! The container's rack panel: the name line with a summary of the inside,
//! the OPEN, EDIT and TO CH buttons, the eight macro knobs with their names and a mapped-count
//! badge each, and a small map of the note splits.
//!
//! ```text
//! CONTAINER            MACROS
//! +--------------+     Macro 1  Macro 2  ...                   Macro 8
//! | 3 devices    |      (knob)   (knob)                         (knob)
//! | 2 splits ... |
//! +--------------+     SPLITS
//! [OPEN] [EDIT]        +----------------------------------------------+
//! [TO CH]              | key map: one coloured bar per split          |
//!                      +----------------------------------------------+
//! ```
//!
//! The macro knobs are the container's `macro1` to `macro8` params, so a drag, a double-click
//! (reset) and the wheel are ordinary `SetParam`s. A double-click on a macro's name renames it
//! (`ContainerCommand::RenameMacro`). A double-click on the panel outside its controls, or OPEN,
//! enters the container (the rack view's scope, see [`crate::RackView::set_scope`]).
//!
//! This module holds the geometry and the pure helpers; the rack view drives the gestures and
//! `paint_panel` draws the extras.

use crate::autolayout::{CONTENT_X, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{edit_button, fit_label};
use crate::seq_panel::{back_layout, place_controls};
use crate::theme::{Theme, hsl, mix};
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_model::{BOUNDARY_KEY, ContainerDoc, MACRO_COUNT, NoteSplit, RackDevice};
use vello::kurbo::{Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the container (it has its own panel, [`container_layout`]).
pub const CONTAINER_TYPE: &str = atomik_model::CONTAINER_TYPE;
/// Height of the container panel in rack units.
pub const CONTAINER_UNITS: u8 = 2;
const LEFT_W: f64 = 108.0;
const KNOBS_X: f64 = CONTENT_X.0 + LEFT_W + 8.0;
const COL_W: f64 = 64.0;
/// Height of a macro knob's control cell: the knob only, the name is a separate target.
const KNOB_CELL_H: f64 = 44.0;
const NAME_H: f64 = 13.0;

/// A button or area of the container panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerButton {
    /// Enter the container.
    Open,
    /// Open the container editor.
    Edit,
    /// Add the container to the console.
    ToChannel,
}

/// Geometry of the container panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContainerPanel {
    /// Title strip of the left block.
    pub title: Rect,
    /// The summary window.
    pub summary: Rect,
    /// OPEN.
    pub open: Rect,
    /// EDIT.
    pub edit: Rect,
    /// TO CH.
    pub to_channel: Rect,
    /// The name target of each macro (below its knob).
    pub names: [Rect; MACRO_COUNT],
    /// Title strip of the splits map.
    pub splits_title: Rect,
    /// The splits map.
    pub splits: Rect,
}

impl ContainerPanel {
    /// The rectangle of a button.
    pub fn button(&self, b: ContainerButton) -> Rect {
        match b {
            ContainerButton::Open => self.open,
            ContainerButton::Edit => self.edit,
            ContainerButton::ToChannel => self.to_channel,
        }
    }
}

/// The macro index (0-based) of a param key `macroN`.
pub fn macro_index(param: &str) -> Option<usize> {
    let n: usize = param.strip_prefix("macro")?.parse().ok()?;
    (1..=MACRO_COUNT).contains(&n).then(|| n - 1)
}

/// The panel of `atomik.container` (two rack units, see the module documentation); the back is
/// the generic socket layout.
pub fn container_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let keys = [
        "macro1", "macro2", "macro3", "macro4", "macro5", "macro6", "macro7", "macro8",
    ];
    let mut front = place_controls(desc, &keys, KNOBS_X, COL_W, MACRO_COUNT, top);
    for pc in &mut front {
        pc.kind = ControlKind::Knob;
        pc.rect.y1 = pc.rect.y0 + KNOB_CELL_H;
    }
    let names = std::array::from_fn(|i| {
        let cx = KNOBS_X + i as f64 * COL_W + COL_W / 2.0;
        let y = top + 45.0;
        Rect::new(
            cx - COL_W / 2.0 + 2.0,
            y,
            cx + COL_W / 2.0 - 2.0,
            y + NAME_H,
        )
    });
    let right = KNOBS_X + MACRO_COUNT as f64 * COL_W;
    let btn_y = top + 62.0;
    let panel = ContainerPanel {
        title: Rect::new(x0, MARGIN_TOP, x0 + LEFT_W, MARGIN_TOP + TITLE_H),
        summary: Rect::new(x0, top + 2.0, x0 + LEFT_W, top + 56.0),
        open: Rect::new(x0, btn_y, x0 + 52.0, btn_y + 20.0),
        edit: Rect::new(x0 + 56.0, btn_y, x0 + LEFT_W, btn_y + 20.0),
        to_channel: Rect::new(x0, btn_y + 24.0, x0 + LEFT_W, btn_y + 44.0),
        names,
        splits_title: Rect::new(KNOBS_X, top + 62.0, right, top + 62.0 + TITLE_H),
        splits: Rect::new(KNOBS_X, top + 76.0, right, top + 76.0 + 46.0),
    };
    let front_sections = vec![
        Section {
            label: "CONTAINER".to_string(),
            rect: panel.title,
        },
        Section {
            label: "MACROS".to_string(),
            rect: Rect::new(KNOBS_X, MARGIN_TOP, right, MARGIN_TOP + TITLE_H),
        },
        Section {
            label: "SPLITS".to_string(),
            rect: panel.splits_title,
        },
    ];
    let (units, back, back_sections) = back_layout(desc, CONTAINER_UNITS);
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        arp: None,
        container: Some(panel),
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ pure helpers

/// What a container panel shows about the inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// First line.
    pub devices: String,
    /// Second line.
    pub splits: String,
    /// Third line: a hint (empty or unconnected container) or the call to open it.
    pub hint: String,
}

/// The summary lines of a container's inside (an empty or unconnected container says so).
pub fn summary(inner: Option<&ContainerDoc>) -> Summary {
    let Some(inner) = inner else {
        return Summary {
            devices: "EMPTY".into(),
            splits: String::new(),
            hint: "double-click to open".into(),
        };
    };
    let n = inner.devices.len();
    let mapped: usize = inner.macros.iter().map(|m| m.targets.len()).sum();
    let s = inner.splits.len();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let cabled_out = inner.cables.iter().any(|c| c.to.device.0 == BOUNDARY_KEY);
    Summary {
        devices: if n == 0 {
            "EMPTY".into()
        } else {
            plural(n, "device", "devices")
        },
        splits: format!("{} · {} mapped", plural(s, "split", "splits"), mapped),
        hint: if n == 0 {
            "double-click to open".into()
        } else if !cabled_out {
            "output not cabled".into()
        } else {
            String::new()
        },
    }
}

/// The colour of split `i` (shared by the panel's map and the editor).
pub fn split_color(i: usize) -> Color {
    hsl(((i * 47 + 20) % 360) as f32, 0.55, 0.58)
}

/// The label of a macro knob: the name being edited (with a caret), else the macro's name.
pub fn macro_label(device: &RackDevice, index: usize, editing: Option<&str>) -> String {
    if let Some(t) = editing {
        return format!("{t}|");
    }
    match &device.inner {
        Some(inner) => inner.macros[index].display_name(index),
        None => format!("Macro {}", index + 1),
    }
}

/// How many targets macro `index` drives.
pub fn mapped_count(device: &RackDevice, index: usize) -> usize {
    device
        .inner
        .as_ref()
        .map_or(0, |i| i.macros[index].targets.len())
}

// ------------------------------------------------------------------ painting

/// What the panel draws besides the document.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContainerInfo<'a> {
    /// The macro name being edited: (macro index, text).
    pub rename: Option<(usize, &'a str)>,
    /// The container already feeds a console channel (TO CH is dim).
    pub has_channel: bool,
}

/// Draws the container panel's extras (and the macro names with their badges).
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ContainerPanel,
    device: &RackDevice,
    info: &ContainerInfo<'_>,
    accent: Color,
) {
    let s = summary(device.inner.as_deref());
    c.fill_with(RoundedRect::from_rect(panel.summary, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(panel.summary.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let x = panel.summary.x0 + 7.0;
    let mut y = panel.summary.y0 + 14.0;
    for (text, color, size) in [
        (&s.devices, theme.text, 10.0),
        (&s.splits, theme.text_muted, 8.5),
        (&s.hint, accent, 8.0),
    ] {
        if !text.is_empty() {
            c.text(&TextRun::new(
                fit_label(text, panel.summary.width() - 12.0, size),
                pt(x, y),
                size,
                color,
            ));
        }
        y += 15.0;
    }
    edit_button(c, theme, panel.open, "OPEN", true);
    edit_button(c, theme, panel.edit, "EDIT", true);
    edit_button(c, theme, panel.to_channel, "TO CH", !info.has_channel);
    // Macro names and badges (the knobs themselves are ordinary controls).
    for (i, r) in panel.names.iter().enumerate() {
        let editing = info.rename.filter(|(k, _)| *k == i).map(|(_, t)| t);
        let n = mapped_count(device, i);
        if editing.is_some() {
            c.fill_with(RoundedRect::from_rect(r.inset(1.0), 2.0), theme.lcd);
            c.stroke_with(RoundedRect::from_rect(r.inset(1.0), 2.0), 1.0, accent);
        }
        let label = macro_label(device, i, editing);
        c.text(
            &TextRun::new(
                fit_label(&label, r.width() + 2.0, theme.label_size),
                pt(r.center().x, r.y0 + 9.0),
                theme.label_size,
                if editing.is_some() {
                    theme.text
                } else {
                    theme.text_muted
                },
            )
            .align(Align::Center),
        );
        if n > 0 {
            let at = pt(r.center().x + 19.0, r.y0 - 36.0);
            c.fill_with(Circle::new(at, 5.5), accent.with_alpha(0.85));
            c.text(
                &TextRun::new(n.to_string(), pt(at.x, at.y + 3.0), 7.5, theme.plate)
                    .align(Align::Center)
                    .weight(700),
            );
        }
    }
    paint_split_map(
        c,
        theme,
        panel.splits,
        device.inner.as_deref().map_or(&[][..], |i| &i.splits),
    );
}

/// The map of the splits: the 128 keys across, one coloured bar per split.
fn paint_split_map<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, splits: &[NoteSplit]) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let inner = r.inset(-4.0);
    let w = inner.width() / 128.0;
    for oct in 0..11u32 {
        let x = inner.x0 + f64::from(oct * 12) * w;
        c.stroke_with(
            vello::kurbo::Line::new(pt(x.round() + 0.5, inner.y0), pt(x.round() + 0.5, inner.y1)),
            1.0,
            Color::WHITE.with_alpha(if oct == 5 { 0.16 } else { 0.06 }),
        );
    }
    if splits.is_empty() {
        c.text(
            &TextRun::new(
                "no splits: no notes reach the inside",
                pt(r.center().x, r.center().y + 3.0),
                8.0,
                theme.text_muted,
            )
            .align(Align::Center),
        );
        return;
    }
    let rows = splits.len().max(1) as f64;
    let row_h = inner.height() / rows;
    for (i, s) in splits.iter().enumerate() {
        let y0 = inner.y0 + i as f64 * row_h;
        let bar = Rect::new(
            inner.x0 + f64::from(s.key_lo) * w,
            y0 + 1.0,
            inner.x0 + (f64::from(s.key_hi) + 1.0) * w,
            y0 + row_h - 1.0,
        );
        c.fill_with(
            RoundedRect::from_rect(bar, 1.5),
            mix(split_color(i), theme.lcd, 0.15),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::{MacroTarget, NoteSplit};

    #[test]
    fn macro_indices_parse() {
        assert_eq!(macro_index("macro1"), Some(0));
        assert_eq!(macro_index("macro8"), Some(7));
        assert_eq!(macro_index("macro9"), None);
        assert_eq!(macro_index("macro0"), None);
        assert_eq!(macro_index("level"), None);
    }

    #[test]
    fn the_summary_says_empty_and_unconnected() {
        assert_eq!(summary(None).devices, "EMPTY");
        let mut inner = ContainerDoc::default();
        assert_eq!(summary(Some(&inner)).hint, "double-click to open");
        inner.devices.push(RackDevice {
            key: atomik_model::DeviceKey("a".into()),
            type_id: "atomik.poly-synth".into(),
            name: "A".into(),
            params: Default::default(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            inner: None,
            plugin: None,
            column: 0,
        });
        inner
            .splits
            .push(NoteSplit::full(atomik_model::DeviceKey("a".into())));
        inner.macros[0].targets.push(MacroTarget {
            device: atomik_model::DeviceKey("a".into()),
            param: "p".into(),
            min: 0.0,
            max: 1.0,
        });
        let s = summary(Some(&inner));
        assert_eq!(s.devices, "1 device");
        assert_eq!(s.splits, "1 split · 1 mapped");
        assert_eq!(s.hint, "output not cabled");
    }

    #[test]
    fn split_colours_differ() {
        let cs: Vec<_> = (0..8).map(split_color).collect();
        for i in 0..8 {
            for j in (i + 1)..8 {
                assert_ne!(cs[i], cs[j]);
            }
        }
    }
}
