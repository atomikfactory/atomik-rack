//! Panel geometry: generated from a descriptor or taken from a hand-made
//! [`PanelLayout`].
//!
//! # Auto-layout rules
//! - **Grouping:** params are grouped by key prefix (everything before the last `.`):
//!   `osc1.wave` → `OSC 1`, `env.amp.attack` → `ENV AMP`, `ch1.level` → `CH 1`; keys without a
//!   dot form the `MAIN` group. Groups keep descriptor order (first appearance).
//! - **Widgets:** `Unit::Choice` → selector, `Unit::Toggle` → button, keys ending in `.level`
//!   with a `Taper::Decibel` → fader, everything else → knob.
//! - **Cells:** fixed sizes (no text measurement, so layouts are platform-independent):
//!   knob 48×58, selector 62×36, button 44×28, fader 36×(all rows), display 62×36.
//!   Items fill a section column by column (top to bottom), a fader takes a whole column.
//! - **Sections** are packed left to right into shelves within the content area (right of the
//!   name plate); a section that does not fit starts a new shelf.
//! - **Height:** the smallest whole number of rack units where both sides fit; for each height,
//!   1, 2, … rows per section are tried and the first packing that fits wins. Content is
//!   centred vertically.
//! - **Back panel:** sockets are grouped by direction and kind (`AUDIO IN`, `CV IN`,
//!   `NOTES IN`, then summing inputs `AUDIO BUS IN`…, then `AUDIO OUT`, `CV OUT`,
//!   `NOTES OUT`) in 50×44 cells, packed the same way. Summing inputs (console buses, which
//!   accept many cables) are drawn like inputs with a second, dashed jack ring.

use atomik_core::{ParamDescriptor, Taper, Unit};
use atomik_device_sdk::{
    ControlKind, DeviceDescriptor, Direction, PanelLayout, RACK_UNIT, RACK_WIDTH, SocketKind,
};
use vello::kurbo::{Point, Rect, Vec2};

/// Rack ear (mounting flange) width on each side.
pub const EAR: f64 = 16.0;
/// Name plate rectangle x range.
pub const PLATE_X: (f64, f64) = (EAR + 4.0, EAR + 4.0 + 80.0);
/// Content area x range (controls and sockets).
pub const CONTENT_X: (f64, f64) = (PLATE_X.1 + 8.0, RACK_WIDTH as f64 - EAR - 8.0);
/// Top margin inside a panel.
pub const MARGIN_TOP: f64 = 4.0;
/// Bottom margin inside a panel.
pub const MARGIN_BOTTOM: f64 = 3.0;
/// Section title height.
pub const TITLE_H: f64 = 11.0;
/// Row height of a section.
pub const ROW_H: f64 = 58.0;
/// Horizontal gap between columns inside a section.
pub const COL_GAP: f64 = 4.0;
/// Horizontal gap between sections.
pub const SECTION_GAP: f64 = 10.0;
/// Vertical gap between shelves.
pub const SHELF_GAP: f64 = 4.0;
/// Socket cell size.
pub const SOCKET_CELL: (f64, f64) = (50.0, 44.0);
/// Jack centre offset from the top of its cell.
pub const JACK_DY: f64 = 14.0;
/// Largest generated height, in rack units.
pub const MAX_UNITS: u8 = 16;

const EPS: f64 = 1e-9;

/// Cell size of a control kind; `None` height means "the full section height" (faders).
pub fn cell_size(kind: ControlKind) -> (f64, Option<f64>) {
    match kind {
        ControlKind::Knob => (48.0, Some(ROW_H)),
        ControlKind::Choice | ControlKind::Display => (62.0, Some(36.0)),
        ControlKind::Toggle => (44.0, Some(28.0)),
        ControlKind::Fader => (36.0, None),
    }
}

/// A placed front-panel control.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedControl {
    /// Index in the descriptor's params.
    pub param: usize,
    /// Widget.
    pub kind: ControlKind,
    /// Cell rectangle (widget and label), panel coordinates.
    pub rect: Rect,
}

/// A placed back-panel socket.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedSocket {
    /// Index in the descriptor's sockets.
    pub socket: usize,
    /// Cell rectangle (jack and label), panel coordinates.
    pub rect: Rect,
}

impl PlacedSocket {
    /// Jack centre.
    pub fn center(&self) -> Point {
        Point::new(self.rect.center().x, self.rect.y0 + JACK_DY)
    }
}

/// Geometry of an effect panel's visual (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxPanel {
    /// Which effect.
    pub kind: crate::fx::FxKind,
    /// The whole visual cell, below its section title.
    pub visual: Rect,
    /// The main plot (curve, response, sketch, crossover display).
    pub plot: Rect,
    /// The meter bars.
    pub meters: Rect,
    /// A numeric readout area (mastering loudness), empty otherwise.
    pub readout: Rect,
    /// A second readout area (the phase meter's loudness), empty otherwise.
    pub aux: Rect,
    /// The compressor's "SC from" dropdown button.
    pub button: Option<Rect>,
    /// The Character panel's matrix, screen, badge and meter strip.
    pub character: Option<crate::character_panel::CharacterGeom>,
}

/// A labelled group.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Title (upper case).
    pub label: String,
    /// Rectangle including the title.
    pub rect: Rect,
}

/// Geometry of both panels of one device type.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSpec {
    /// Height in rack units.
    pub height_units: u8,
    /// The sample strip of a device that takes samples (front, panel coordinates).
    pub samples: Option<Rect>,
    /// The drum panel of an `atomik.drum-machine` (pad grid, selected-pad readout, Edit button).
    pub drum: Option<DrumPanel>,
    /// The loop slicer panel of an `atomik.loop-slicer` (overview strip, sensitivity, buttons).
    pub slicer: Option<SlicerPanel>,
    /// The visual of one of the six effects (see [`crate::fx_layout`]).
    pub fx: Option<FxPanel>,
    /// The step sequencer panel of an `atomik.step-sequencer` (see
    /// [`crate::seq_panel`]).
    pub seq: Option<crate::seq_panel::SeqPanel>,
    /// The drum generator panel of an `atomik.drum-generator` (see [`crate::drum_gen_panel`]).
    pub drum_gen: Option<crate::drum_gen_panel::DrumGenPanel>,
    /// The arpeggiator panel of an `atomik.arpeggiator` (see [`crate::arp_panel`]).
    pub arp: Option<crate::arp_panel::ArpPanel>,
    /// The Key & Chord panel of an `atomik.key-chord` (see [`crate::key_chord_panel`]).
    pub key_chord: Option<crate::key_chord_panel::KeyChordPanel>,
    /// The Mod Source panel of an `atomik.mod-source` (see [`crate::modsrc_panel`]).
    pub modsrc: Option<crate::modsrc_panel::ModSourcePanel>,
    /// The container panel of an `atomik.container` (see
    /// [`crate::container_panel`]).
    pub container: Option<crate::container_panel::ContainerPanel>,
    /// The plugin panel of a third-party plugin device (see
    /// [`crate::plugin_panel`]).
    pub plugin: Option<crate::plugin_panel::PluginPanel>,
    /// Front controls.
    pub front: Vec<PlacedControl>,
    /// Front sections.
    pub front_sections: Vec<Section>,
    /// Back sockets.
    pub back: Vec<PlacedSocket>,
    /// Back sections.
    pub back_sections: Vec<Section>,
}

impl PanelSpec {
    /// Whether a placed control sits on the **back** face (the Character CV trims, placed in
    /// back-panel coordinates under their jacks): it is drawn and hit on the back only.
    pub fn on_back(&self, pc: &PlacedControl) -> bool {
        self.fx
            .and_then(|f| f.character)
            .is_some_and(|g| g.back_params.contains(&pc.param))
            || self.drum.is_some_and(|d| d.back_params.contains(&pc.param))
    }

    /// Height in logical units.
    pub fn height(&self) -> f64 {
        f64::from(self.height_units) * f64::from(RACK_UNIT)
    }
}

/// The key of the `bypass` param of a built-in effect.
pub const BYPASS_KEY: &str = "bypass";

/// Whether a device gets the BYPASS switch of the name strip: a built-in effect (a plugin
/// keeps its own BYPASS button inside its panel) or a note player that has the `bypass` param.
/// Such a param is placed by that switch, never as a knob of the panel body: the auto-layout
/// skips it, the hand-made layouts do not list it.
pub fn has_strip_bypass(desc: &DeviceDescriptor) -> bool {
    let takes_one = (desc.kind == atomik_device_sdk::DeviceKind::Effect
        && !atomik_device_sdk::is_plugin_type_id(desc.type_id))
        || crate::browser::is_player(desc.type_id);
    takes_one && desc.param_by_key(BYPASS_KEY).is_some()
}

/// The widget chosen for a param.
pub fn choose_widget(p: &ParamDescriptor) -> ControlKind {
    match p.unit {
        Unit::Choice(_) => ControlKind::Choice,
        Unit::Toggle => ControlKind::Toggle,
        _ if p.key.ends_with(".level") && matches!(p.taper, Taper::Decibel) => ControlKind::Fader,
        _ => ControlKind::Knob,
    }
}

/// The group prefix of a param key (`"env.amp.attack"` → `"env.amp"`, `"voices"` → `""`).
pub fn group_prefix(key: &str) -> &str {
    key.rfind('.').map_or("", |i| &key[..i])
}

/// Section title for a prefix (`"env.amp"` → `"ENV AMP"`, `"osc1"` → `"OSC 1"`, `""` → `"MAIN"`).
pub fn section_label(prefix: &str) -> String {
    if prefix.is_empty() {
        return "MAIN".to_string();
    }
    let mut out = String::new();
    let mut prev_alpha = false;
    for ch in prefix.chars() {
        match ch {
            '.' | '_' | '-' => {
                out.push(' ');
                prev_alpha = false;
            }
            c if c.is_ascii_digit() && prev_alpha => {
                out.push(' ');
                out.push(c);
                prev_alpha = false;
            }
            c => {
                out.extend(c.to_uppercase());
                prev_alpha = c.is_alphabetic();
            }
        }
    }
    out
}

/// Control label: the humanised last key segment (`"filter.env_amount"` → `"Env amount"`),
/// or the param name for ungrouped params.
pub fn control_label(p: &ParamDescriptor) -> String {
    if group_prefix(p.key).is_empty() {
        return p.name.to_string();
    }
    let last = p.key.rsplit('.').next().unwrap_or(p.key).replace('_', " ");
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => p.name.to_string(),
    }
}

/// Back-panel group title.
pub fn socket_group_label(kind: SocketKind, direction: Direction) -> &'static str {
    match (kind, direction) {
        (SocketKind::Audio, Direction::Input) => "AUDIO IN",
        (SocketKind::Cv, Direction::Input) => "CV IN",
        (SocketKind::Notes, Direction::Input) => "NOTES IN",
        (SocketKind::Audio, Direction::Output) => "AUDIO OUT",
        (SocketKind::Cv, Direction::Output) => "CV OUT",
        (SocketKind::Notes, Direction::Output) => "NOTES OUT",
        (SocketKind::Audio, Direction::SummingInput) => "AUDIO BUS IN",
        (SocketKind::Cv, Direction::SummingInput) => "CV BUS IN",
        (SocketKind::Notes, Direction::SummingInput) => "NOTES BUS IN",
    }
}

/// Width of the sample strip of a sampler panel (zone count, Edit button, drop target).
pub const SAMPLE_STRIP_W: f64 = 120.0;

/// Generates both panels from a descriptor.
pub fn auto_layout(desc: &DeviceDescriptor) -> PanelSpec {
    layout(desc, false)
}

/// Like [`auto_layout`] for a device that takes samples: the front panel keeps a
/// [`SAMPLE_STRIP_W`] wide strip at the left of the content area ([`PanelSpec::samples`]) and the
/// controls are packed to its right.
pub fn auto_layout_with_samples(desc: &DeviceDescriptor) -> PanelSpec {
    layout(desc, true)
}

fn layout(desc: &DeviceDescriptor, samples: bool) -> PanelSpec {
    let reserve = if samples {
        SAMPLE_STRIP_W + SECTION_GAP
    } else {
        0.0
    };
    layout_with(desc, reserve, samples, group_prefix)
}

/// Like [`auto_layout`] with a `strip` wide column kept free at the left of the content area
/// (for a panel's own button) and `prefix_of` choosing the section of each param key instead of
/// [`group_prefix`]. The caller places what goes into the strip.
pub(crate) fn auto_layout_with_strip(
    desc: &DeviceDescriptor,
    strip: f64,
    prefix_of: for<'a> fn(&'a str) -> &'a str,
) -> PanelSpec {
    layout_with(desc, strip + SECTION_GAP, false, prefix_of)
}

fn layout_with(
    desc: &DeviceDescriptor,
    reserve: f64,
    samples: bool,
    prefix_of: for<'a> fn(&'a str) -> &'a str,
) -> PanelSpec {
    let front = front_groups(desc, prefix_of);
    let back = back_groups(desc);
    let units = (1..=MAX_UNITS)
        .find(|&u| {
            fit(&front, u, ROW_H, reserve).is_some() && fit(&back, u, SOCKET_CELL.1, 0.0).is_some()
        })
        .unwrap_or(MAX_UNITS);
    let (front_cells, front_sections) = fit(&front, units, ROW_H, reserve).unwrap_or_default();
    let (back_cells, back_sections) = fit(&back, units, SOCKET_CELL.1, 0.0).unwrap_or_default();
    let height = f64::from(units) * f64::from(RACK_UNIT);
    PanelSpec {
        height_units: units,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: None,
        samples: samples.then(|| {
            Rect::new(
                CONTENT_X.0,
                MARGIN_TOP,
                CONTENT_X.0 + SAMPLE_STRIP_W,
                height - MARGIN_BOTTOM,
            )
        }),
        front: front_cells
            .into_iter()
            .map(|(param, rect)| PlacedControl {
                param,
                kind: choose_widget(&desc.params[param]),
                rect,
            })
            .collect(),
        front_sections,
        back: back_cells
            .into_iter()
            .map(|(socket, rect)| PlacedSocket { socket, rect })
            .collect(),
        back_sections,
    }
}

/// An item to pack: (index, width, height or `None` for a full column).
pub(crate) type Item = (usize, f64, Option<f64>);

pub(crate) struct Group {
    pub(crate) label: String,
    pub(crate) items: Vec<Item>,
    /// Least section width (so a long title does not run into the next section); 0 for none.
    pub(crate) min_w: f64,
}

fn front_groups(desc: &DeviceDescriptor, prefix_of: for<'a> fn(&'a str) -> &'a str) -> Vec<Group> {
    let mut groups: Vec<(&str, Group)> = Vec::new();
    let strip_bypass = has_strip_bypass(desc);
    for (i, p) in desc.params.iter().enumerate() {
        if strip_bypass && p.key == BYPASS_KEY {
            continue;
        }
        // `internal.*` params are set by the app (solo silence, the VCA feed), never by hand.
        if atomik_model::console::is_internal_param(p.key) {
            continue;
        }
        let prefix = prefix_of(p.key);
        let (w, h) = cell_size(choose_widget(p));
        let item = (i, w, h);
        match groups.iter_mut().find(|(k, _)| *k == prefix) {
            Some((_, g)) => g.items.push(item),
            None => groups.push((
                prefix,
                Group {
                    label: section_label(prefix),
                    items: vec![item],
                    min_w: 0.0,
                },
            )),
        }
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

pub(crate) fn back_groups(desc: &DeviceDescriptor) -> Vec<Group> {
    back_groups_filtered(desc, |_| true)
}

fn back_groups_filtered(desc: &DeviceDescriptor, include: impl Fn(&str) -> bool) -> Vec<Group> {
    let mut out = Vec::new();
    for direction in [Direction::Input, Direction::SummingInput, Direction::Output] {
        for kind in [SocketKind::Audio, SocketKind::Cv, SocketKind::Notes] {
            let items: Vec<Item> = desc
                .sockets
                .iter()
                .enumerate()
                .filter(|(_, s)| s.kind == kind && s.direction == direction && include(s.key))
                .map(|(i, _)| (i, SOCKET_CELL.0, Some(SOCKET_CELL.1)))
                .collect();
            if !items.is_empty() {
                out.push(Group {
                    label: socket_group_label(kind, direction).to_string(),
                    items,
                    min_w: 0.0,
                });
            }
        }
    }
    out
}

/// Packs one section with `rows` rows; returns cells relative to the section origin and the
/// section size.
fn pack_section(items: &[Item], rows: usize, row_h: f64) -> (Vec<(usize, Rect)>, f64, f64) {
    let col_h = rows as f64 * row_h;
    let mut columns: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new()];
    let mut y = 0.0;
    for &(i, w, h) in items {
        let h = h.unwrap_or(col_h).min(col_h);
        let start_new = columns.last().is_some_and(|c| !c.is_empty()) && y + h > col_h + EPS;
        if start_new {
            columns.push(Vec::new());
            y = 0.0;
        }
        if let Some(col) = columns.last_mut() {
            col.push((i, w, h));
        }
        y += h;
    }
    let mut cells = Vec::new();
    let mut x = 0.0;
    for col in &columns {
        let cw = col.iter().map(|c| c.1).fold(0.0, f64::max);
        let mut y = TITLE_H;
        for &(i, w, h) in col {
            let x0 = x + (cw - w) / 2.0;
            cells.push((i, Rect::new(x0, y, x0 + w, y + h)));
            y += h;
        }
        x += cw + COL_GAP;
    }
    let width = (x - COL_GAP).max(0.0);
    (cells, width, TITLE_H + col_h)
}

pub(crate) type Packed = (Vec<(usize, Rect)>, Vec<Section>);

/// Packs every group with `rows` rows per section into shelves; returns the cells, sections
/// and total height (origin at the content area's left, y = 0).
fn pack(groups: &[Group], rows: usize, row_h: f64, reserve: f64) -> Option<(Packed, f64)> {
    let content_w = CONTENT_X.1 - CONTENT_X.0 - reserve;
    let shelf_h = TITLE_H + rows as f64 * row_h;
    let (mut sx, mut sy) = (0.0, 0.0);
    let mut cells = Vec::new();
    let mut sections = Vec::new();
    for g in groups {
        let (local, w, h) = pack_section(&g.items, rows, row_h);
        let w = w.max(g.min_w);
        if w > content_w + EPS {
            return None;
        }
        if sx > 0.0 && sx + w > content_w + EPS {
            sx = 0.0;
            sy += shelf_h + SHELF_GAP;
        }
        let off = Vec2::new(CONTENT_X.0 + reserve + sx, sy);
        cells.extend(local.into_iter().map(|(i, r)| (i, r + off)));
        sections.push(Section {
            label: g.label.clone(),
            rect: Rect::new(off.x, off.y, off.x + w, off.y + h),
        });
        sx += w + SECTION_GAP;
    }
    let height = if groups.is_empty() { 0.0 } else { sy + shelf_h };
    Some(((cells, sections), height))
}

/// The first packing that fits in `units` rack units, centred vertically.
fn fit(groups: &[Group], units: u8, row_h: f64, reserve: f64) -> Option<Packed> {
    fit_with(groups, units, row_h, reserve, 0.0)
}

/// Like [`fit`] with `top` height reserved above the sections (a visual across the panel).
pub(crate) fn fit_with(
    groups: &[Group],
    units: u8,
    row_h: f64,
    reserve: f64,
    top: f64,
) -> Option<Packed> {
    let avail = f64::from(units) * f64::from(RACK_UNIT) - MARGIN_TOP - MARGIN_BOTTOM - top;
    if groups.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let mut rows = 1;
    while TITLE_H + rows as f64 * row_h <= avail + EPS {
        if let Some(((cells, sections), h)) = pack(groups, rows, row_h, reserve)
            && h <= avail + EPS
        {
            let off = Vec2::new(0.0, MARGIN_TOP + top + ((avail - h) / 2.0).floor());
            return Some((
                cells.into_iter().map(|(i, r)| (i, r + off)).collect(),
                sections
                    .into_iter()
                    .map(|s| Section {
                        rect: s.rect + off,
                        ..s
                    })
                    .collect(),
            ));
        }
        rows += 1;
    }
    None
}

// ------------------------------------------------------------------ drum machine panel

/// Type id of the drum machine, which has its own panel ([`drum_layout`]).
pub const DRUM_TYPE: &str = "atomik.drum-machine";
/// Height of the drum panel in rack units.
pub const DRUM_UNITS: u8 = 3;
/// Packing index of the global CV trim knob `i` (`DRUM_TRIM_ITEM + i`: 0 level, 1 pitch), above
/// any socket.
const DRUM_TRIM_ITEM: usize = 10_000;
/// The global CV inputs of the drum machine and the trims that go with them, in back order.
const DRUM_CV: [(&str, &str); 2] = [
    ("level_cv_in", "master.level_cv_trim"),
    ("pitch_cv_in", "pitch_cv_trim"),
];
/// Width of the trim knob cell on the drum back (narrower than a front knob).
const DRUM_TRIM_W: f64 = 40.0;
/// Size of one pad cell of the 4x4 pad grid (the pad itself is inset by 2 px).
pub const DRUM_PAD_CELL: (f64, f64) = (54.0, 48.0);
/// Width of one knob column of the selected pad's section.
const DRUM_COL_W: f64 = 64.0;
const DRUM_KNOBS_X: f64 = CONTENT_X.0 + 4.0 * DRUM_PAD_CELL.0 + 16.0;
const DRUM_MASTER_X: f64 = DRUM_KNOBS_X + 4.0 * DRUM_COL_W + 16.0;
/// The per-pad params of the drum panel, in layout order (knobs left to right, two rows).
pub const DRUM_PAD_PARAMS: [&str; 8] = [
    "level", "pan", "tune", "fine", "decay", "mode", "choke", "velocity",
];

/// Geometry of the drum panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumPanel {
    /// The 4x4 pad grid (pad 1 bottom left, pad 4 bottom right, pad 16 top right).
    pub pads: Rect,
    /// Readout of the selected pad (number, key, file name).
    pub info: Rect,
    /// Title strip of the selected pad's knob section (drawn with the pad number).
    pub pad_title: Rect,
    /// The Edit button.
    pub edit: Rect,
    /// The "TO CH" button: gives the selected pad its own console channel.
    pub to_channel: Rect,
    /// The descriptor indices of the `master.level_cv_trim` and `pitch_cv_trim` knobs, which sit
    /// on the back face next to their jacks (see [`PanelSpec::on_back`]); `usize::MAX` when
    /// missing.
    pub back_params: [usize; 2],
}

/// The cell rectangle of `pad` (1..=16) in the pad grid `pads`: pad 1 is bottom left, the
/// numbers run left to right and upwards, like the rows of the step editor.
pub fn drum_pad_rect(pads: Rect, pad: u8) -> Rect {
    let i = f64::from(pad.clamp(1, 16) - 1);
    let (col, row) = (i % 4.0, (i / 4.0).floor());
    let (w, h) = DRUM_PAD_CELL;
    let x0 = pads.x0 + col * w;
    let y0 = pads.y1 - (row + 1.0) * h;
    Rect::new(x0 + 2.0, y0 + 2.0, x0 + w - 2.0, y0 + h - 2.0)
}

/// The pad (1..=16) whose pad rectangle contains `p`; the gaps between pads belong to none.
pub fn drum_pad_at(pads: Rect, p: Point) -> Option<u8> {
    (1..=16).find(|pad| drum_pad_rect(pads, *pad).contains(p))
}

impl PanelSpec {
    /// The descriptor index of the param a front control stands for. On a drum panel the
    /// controls are laid out on `pad1.*`; they stand for the same param of the selected `pad`.
    pub fn param_index(&self, desc: &DeviceDescriptor, pc: &PlacedControl, pad: u8) -> usize {
        if self.drum.is_none() || self.on_back(pc) {
            return pc.param;
        }
        desc.params[pc.param]
            .key
            .strip_prefix("pad1.")
            .and_then(|rest| {
                let want = format!("pad{pad}.{rest}");
                desc.params.iter().position(|p| p.key == want)
            })
            .unwrap_or(pc.param)
    }
}

fn pad_of_socket(key: &str) -> Option<u8> {
    let rest = key.strip_prefix("pad")?;
    let (n, tail) = rest.split_once('_')?;
    (tail.starts_with("out") || tail.starts_with("gate_"))
        .then(|| n.parse().ok())
        .flatten()
}

/// The panel of `atomik.drum-machine`: a 4x4 pad grid, the eight knobs of the selected pad
/// (placed on `pad1.*`, see [`PanelSpec::param_index`]), `master.level` and an Edit button on
/// the front; on the back the sockets are grouped per pad (`PAD 1`..`PAD 16`, left and right
/// stacked), after the notes input and the main outputs. Three rack units.
pub fn drum_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let pads = Rect::new(
        CONTENT_X.0,
        top + 2.0,
        CONTENT_X.0 + 4.0 * DRUM_PAD_CELL.0,
        top + 2.0 + 4.0 * DRUM_PAD_CELL.1,
    );
    let index = |key: &str| desc.params.iter().position(|p| p.key == key);
    let cell = |param: usize, kind: ControlKind, cx: f64, row_top: f64| {
        let (w, h) = cell_size(kind);
        let h = h.unwrap_or(ROW_H);
        let y0 = row_top + (ROW_H - h) / 2.0;
        PlacedControl {
            param,
            kind,
            rect: Rect::new(cx - w / 2.0, y0, cx + w / 2.0, y0 + h),
        }
    };
    let mut front = Vec::new();
    for (i, suffix) in DRUM_PAD_PARAMS.iter().enumerate() {
        let Some(param) = index(&format!("pad1.{suffix}")) else {
            continue;
        };
        let kind = match choose_widget(&desc.params[param]) {
            ControlKind::Fader => ControlKind::Knob,
            k => k,
        };
        let cx = DRUM_KNOBS_X + (i % 4) as f64 * DRUM_COL_W + DRUM_COL_W / 2.0;
        front.push(cell(param, kind, cx, top + (i / 4) as f64 * ROW_H));
    }
    if let Some(param) = index("master.level") {
        front.push(cell(
            param,
            ControlKind::Knob,
            DRUM_MASTER_X + DRUM_COL_W / 2.0,
            top,
        ));
    }
    let knobs_w = 4.0 * DRUM_COL_W;
    let front_sections = vec![
        Section {
            label: "PADS".to_string(),
            rect: Rect::new(pads.x0, MARGIN_TOP, pads.x1, MARGIN_TOP + TITLE_H),
        },
        Section {
            label: "MASTER".to_string(),
            rect: Rect::new(
                DRUM_MASTER_X,
                MARGIN_TOP,
                DRUM_MASTER_X + DRUM_COL_W,
                MARGIN_TOP + TITLE_H,
            ),
        },
    ];
    let drum = DrumPanel {
        pads,
        info: Rect::new(
            DRUM_KNOBS_X,
            top + 2.0 * ROW_H + 8.0,
            DRUM_KNOBS_X + knobs_w,
            top + 2.0 * ROW_H + 44.0,
        ),
        pad_title: Rect::new(
            DRUM_KNOBS_X,
            MARGIN_TOP,
            DRUM_KNOBS_X + knobs_w,
            MARGIN_TOP + TITLE_H,
        ),
        edit: Rect::new(
            DRUM_MASTER_X,
            top + ROW_H + 12.0,
            DRUM_MASTER_X + DRUM_COL_W,
            top + ROW_H + 34.0,
        ),
        to_channel: Rect::new(
            DRUM_MASTER_X,
            top + ROW_H + 40.0,
            DRUM_MASTER_X + DRUM_COL_W,
            top + ROW_H + 62.0,
        ),
        back_params: DRUM_CV.map(|(_, trim)| index(trim).unwrap_or(usize::MAX)),
    };
    let is_global_cv = |k: &str| DRUM_CV.iter().any(|(s, _)| *s == k);
    let mut groups = back_groups_filtered(desc, |k| pad_of_socket(k).is_none() && !is_global_cv(k));
    // The global CV inputs, each next to its trim knob.
    let cv_items: Vec<Item> = DRUM_CV
        .iter()
        .enumerate()
        .flat_map(|(n, (socket, _))| {
            let jack = desc.sockets.iter().position(|s| s.key == *socket);
            let trim = (drum.back_params[n] != usize::MAX).then_some((
                DRUM_TRIM_ITEM + n,
                DRUM_TRIM_W,
                Some(ROW_H),
            ));
            jack.map(|i| (i, SOCKET_CELL.0, Some(SOCKET_CELL.1)))
                .into_iter()
                .chain(trim)
        })
        .collect();
    if !cv_items.is_empty() {
        groups.push(Group {
            label: "CV".to_string(),
            items: cv_items,
            min_w: 0.0,
        });
    }
    for pad in 1..=16u8 {
        let items: Vec<Item> = desc
            .sockets
            .iter()
            .enumerate()
            .filter(|(_, s)| pad_of_socket(s.key) == Some(pad))
            .map(|(i, _)| (i, SOCKET_CELL.0, Some(SOCKET_CELL.1)))
            .collect();
        if !items.is_empty() {
            groups.push(Group {
                label: format!("PAD {pad}"),
                items,
                min_w: 0.0,
            });
        }
    }
    // One row per section (a knob is as tall as a row): each pad is its four jacks side by side
    // (out L, out R, gate in, gate out).
    let units = (DRUM_UNITS..=MAX_UNITS)
        .find(|&u| fit(&groups, u, ROW_H, 0.0).is_some())
        .unwrap_or(MAX_UNITS);
    let (back_cells, back_sections) = fit(&groups, units, ROW_H, 0.0).unwrap_or_default();
    for (item, rect) in &back_cells {
        if *item >= DRUM_TRIM_ITEM {
            front.push(PlacedControl {
                param: drum.back_params[item - DRUM_TRIM_ITEM],
                kind: ControlKind::Knob,
                rect: *rect,
            });
        }
    }
    let back_cells: Vec<_> = back_cells
        .into_iter()
        .filter(|(item, _)| *item < DRUM_TRIM_ITEM)
        .collect();
    PanelSpec {
        height_units: units,
        samples: None,
        drum: Some(drum),
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back: back_cells
            .into_iter()
            .map(|(socket, rect)| PlacedSocket { socket, rect })
            .collect(),
        back_sections,
    }
}

// ------------------------------------------------------------------ loop slicer panel

/// Type id of the loop slicer, which has its own panel ([`slicer_layout`]).
pub const SLICER_TYPE: &str = "atomik.loop-slicer";
/// Height of the slicer panel in rack units.
pub const SLICER_UNITS: u8 = 2;
/// Default sensitivity (percent) of the slicer panel's Slice button.
pub const DEFAULT_SLICE_SENSITIVITY: u8 = 50;
/// Width of the slicer's loop strip (overview, sensitivity knob and buttons).
pub const SLICER_STRIP_W: f64 = 222.0;
/// Width of one knob column of the slicer panel.
const SLICER_COL_W: f64 = 64.0;
/// The slicer's device params in front layout order (six per row, two rows).
pub const SLICER_PARAMS: [&str; 12] = [
    "master.level",
    "voices",
    "tune.semi",
    "env.attack",
    "env.decay",
    "env.mode",
    "velocity.amp",
    "loop.beats",
    "cv.pitch_amount",
    "filter.cutoff",
    "filter.cutoff_cv_trim",
    "master.level_cv_trim",
];

/// Geometry of the slicer panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlicerPanel {
    /// The overview strip: slice count and slice boundaries (double-click opens the editor).
    pub strip: Rect,
    /// The cell of the sensitivity knob (a UI value, not a device param).
    pub sens: Rect,
    /// The Slice button.
    pub slice: Rect,
    /// The "To track" button.
    pub to_track: Rect,
    /// The Edit button.
    pub edit: Rect,
}

/// The centre of the sensitivity knob of a slicer panel, in panel coordinates.
pub fn slicer_sens_knob(sens: Rect) -> Point {
    Point::new(sens.center().x, sens.y0 + crate::paint::KNOB_DY)
}

/// The panel of `atomik.loop-slicer`: an overview strip with the sensitivity knob and the
/// Slice, To track and Edit buttons, then the device knobs in two rows (see
/// [`SLICER_PARAMS`]); the back is the generic socket layout. Two rack units.
pub fn slicer_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let strip_x1 = x0 + SLICER_STRIP_W;
    let strip = Rect::new(x0, top + 2.0, strip_x1, top + 46.0);
    let sens = Rect::new(x0, top + 52.0, x0 + 56.0, top + 52.0 + ROW_H);
    let bx = x0 + 64.0;
    let button = |i: f64| {
        Rect::new(
            bx,
            top + 54.0 + i * 22.0,
            strip_x1,
            top + 54.0 + i * 22.0 + 18.0,
        )
    };
    let panel = SlicerPanel {
        strip,
        sens,
        slice: button(0.0),
        to_track: button(1.0),
        edit: button(2.0),
    };
    let knobs_x = strip_x1 + 16.0;
    let mut front = Vec::new();
    for (i, key) in SLICER_PARAMS.iter().enumerate() {
        let Some(param) = desc.params.iter().position(|p| p.key == *key) else {
            continue;
        };
        let kind = match choose_widget(&desc.params[param]) {
            ControlKind::Fader => ControlKind::Knob,
            k => k,
        };
        let (w, h) = cell_size(kind);
        let h = h.unwrap_or(ROW_H);
        let cx = knobs_x + (i % 6) as f64 * SLICER_COL_W + SLICER_COL_W / 2.0;
        let y0 = top + (i / 6) as f64 * ROW_H + (ROW_H - h) / 2.0;
        front.push(PlacedControl {
            param,
            kind,
            rect: Rect::new(cx - w / 2.0, y0, cx + w / 2.0, y0 + h),
        });
    }
    let front_sections = vec![
        Section {
            label: "LOOP".to_string(),
            rect: Rect::new(x0, MARGIN_TOP, strip_x1, MARGIN_TOP + TITLE_H),
        },
        Section {
            label: "PLAYBACK".to_string(),
            rect: Rect::new(
                knobs_x,
                MARGIN_TOP,
                knobs_x + 6.0 * SLICER_COL_W,
                MARGIN_TOP + TITLE_H,
            ),
        },
    ];
    let back = back_groups(desc);
    let units = (SLICER_UNITS..=MAX_UNITS)
        .find(|&u| fit(&back, u, SOCKET_CELL.1, 0.0).is_some())
        .unwrap_or(MAX_UNITS);
    let (back_cells, back_sections) = fit(&back, units, SOCKET_CELL.1, 0.0).unwrap_or_default();
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: Some(panel),
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back: back_cells
            .into_iter()
            .map(|(socket, rect)| PlacedSocket { socket, rect })
            .collect(),
        back_sections,
    }
}

/// Converts a hand-made layout (control and jack centres) into a [`PanelSpec`]; params and
/// sockets missing from the layout are not shown, unknown keys are skipped.
pub fn from_panel_layout(layout: &PanelLayout, desc: &DeviceDescriptor) -> PanelSpec {
    let height = f64::from(layout.height_units) * f64::from(RACK_UNIT);
    let front = layout
        .front
        .iter()
        .filter_map(|c| {
            let param = desc.params.iter().position(|p| p.key == c.param)?;
            let (w, h) = cell_size(c.kind);
            let h = h.unwrap_or(height - MARGIN_TOP - MARGIN_BOTTOM - TITLE_H);
            let (x, y) = (f64::from(c.x), f64::from(c.y));
            Some(PlacedControl {
                param,
                kind: c.kind,
                rect: Rect::new(x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0),
            })
        })
        .collect();
    let back = layout
        .back
        .iter()
        .filter_map(|s| {
            let socket = desc.sockets.iter().position(|d| d.key == s.socket)?;
            let (x, y) = (f64::from(s.x), f64::from(s.y));
            let (w, h) = SOCKET_CELL;
            Some(PlacedSocket {
                socket,
                rect: Rect::new(x - w / 2.0, y - JACK_DY, x + w / 2.0, y - JACK_DY + h),
            })
        })
        .collect();
    PanelSpec {
        height_units: layout.height_units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: None,
        front,
        front_sections: Vec::new(),
        back,
        back_sections: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_devices::{line_mixer, poly_synth, stereo_delay};

    fn all() -> [&'static DeviceDescriptor; 3] {
        [
            &poly_synth::DESCRIPTOR,
            &line_mixer::DESCRIPTOR,
            &stereo_delay::DESCRIPTOR,
        ]
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
    }

    fn inside(r: &Rect, outer: &Rect) -> bool {
        r.x0 >= outer.x0 && r.y0 >= outer.y0 && r.x1 <= outer.x1 && r.y1 <= outer.y1
    }

    #[test]
    fn widget_choice_rules() {
        let p = |key| *poly_synth::DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(choose_widget(&p("osc1.wave")), ControlKind::Choice);
        assert_eq!(choose_widget(&p("filter.cutoff")), ControlKind::Knob);
        let m = |key| *line_mixer::DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(choose_widget(&m("ch1.level")), ControlKind::Fader);
        assert_eq!(choose_widget(&m("ch1.mute")), ControlKind::Toggle);
        assert_eq!(choose_widget(&m("ch1.pan")), ControlKind::Knob);
        let d = |key| *stereo_delay::DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(choose_widget(&d("ping_pong")), ControlKind::Toggle);
    }

    #[test]
    fn grouping_and_labels() {
        assert_eq!(group_prefix("env.amp.attack"), "env.amp");
        assert_eq!(group_prefix("osc1.wave"), "osc1");
        assert_eq!(group_prefix("voices"), "");
        assert_eq!(section_label("env.amp"), "ENV AMP");
        assert_eq!(section_label("osc1"), "OSC 1");
        assert_eq!(section_label("ch12"), "CH 12");
        assert_eq!(section_label(""), "MAIN");
        let p = |key| *poly_synth::DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(control_label(&p("filter.env_amount")), "Env amount");
        assert_eq!(control_label(&p("voices")), "Voices");
        assert_eq!(
            socket_group_label(SocketKind::Cv, Direction::Output),
            "CV OUT"
        );
    }

    #[test]
    fn every_param_and_socket_is_placed_exactly_once() {
        for d in all() {
            let spec = auto_layout(d);
            let mut params: Vec<usize> = spec.front.iter().map(|c| c.param).collect();
            // The `bypass` param of an effect is placed by the name strip's switch.
            if has_strip_bypass(d) {
                params.push(d.params.iter().position(|p| p.key == BYPASS_KEY).unwrap());
            }
            params.sort_unstable();
            assert_eq!(
                params,
                (0..d.params.len()).collect::<Vec<_>>(),
                "{}",
                d.type_id
            );
            let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
            sockets.sort_unstable();
            assert_eq!(
                sockets,
                (0..d.sockets.len()).collect::<Vec<_>>(),
                "{}",
                d.type_id
            );
            for c in &spec.front {
                assert_eq!(c.kind, choose_widget(&d.params[c.param]));
            }
        }
    }

    #[test]
    fn nothing_overlaps_and_everything_is_inside_the_panel() {
        for d in all() {
            let spec = auto_layout(d);
            assert!(spec.height_units >= 1);
            let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
            let rects: Vec<Rect> = spec.front.iter().map(|c| c.rect).collect();
            let back: Vec<Rect> = spec.back.iter().map(|s| s.rect).collect();
            for set in [&rects, &back] {
                for (i, a) in set.iter().enumerate() {
                    assert!(
                        inside(a, &content),
                        "{} {a:?} outside {content:?}",
                        d.type_id
                    );
                    for b in &set[i + 1..] {
                        assert!(!overlaps(a, b), "{}: {a:?} overlaps {b:?}", d.type_id);
                    }
                }
            }
            for secs in [&spec.front_sections, &spec.back_sections] {
                for (i, a) in secs.iter().enumerate() {
                    assert!(inside(&a.rect, &content), "{} {}", d.type_id, a.label);
                    for b in &secs[i + 1..] {
                        assert!(
                            !overlaps(&a.rect, &b.rect),
                            "{} {} {}",
                            d.type_id,
                            a.label,
                            b.label
                        );
                    }
                }
            }
            // Every control sits inside its section, below the title.
            for c in &spec.front {
                let prefix = group_prefix(d.params[c.param].key);
                let sec = spec
                    .front_sections
                    .iter()
                    .find(|s| s.label == section_label(prefix))
                    .unwrap();
                assert!(inside(&c.rect, &sec.rect));
                assert!(c.rect.y0 >= sec.rect.y0 + TITLE_H);
            }
        }
    }

    #[test]
    fn heights_are_compact() {
        assert_eq!(auto_layout(&stereo_delay::DESCRIPTOR).height_units, 1);
        assert_eq!(auto_layout(&line_mixer::DESCRIPTOR).height_units, 2);
        assert!(auto_layout(&poly_synth::DESCRIPTOR).height_units <= 4);
    }

    #[test]
    fn sections_follow_descriptor_order() {
        let spec = auto_layout(&poly_synth::DESCRIPTOR);
        let labels: Vec<&str> = spec
            .front_sections
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert_eq!(&labels[..3], &["OSC 1", "OSC 2", "OSC"]);
        assert!(labels.contains(&"ENV AMP"));
        let back: Vec<&str> = spec
            .back_sections
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert_eq!(back, ["CV IN", "NOTES IN", "AUDIO OUT", "CV OUT"]);
    }

    #[test]
    fn summing_inputs_are_placed_in_their_own_input_group() {
        use atomik_device_sdk::{DeviceKind, SocketDescriptor, SocketId};
        static SOCKETS: [SocketDescriptor; 3] = [
            SocketDescriptor {
                id: SocketId(0),
                key: "bus_l",
                name: "Bus L",
                kind: SocketKind::Audio,
                direction: Direction::SummingInput,
            },
            SocketDescriptor {
                id: SocketId(1),
                key: "in",
                name: "In",
                kind: SocketKind::Audio,
                direction: Direction::Input,
            },
            SocketDescriptor {
                id: SocketId(2),
                key: "out",
                name: "Out",
                kind: SocketKind::Audio,
                direction: Direction::Output,
            },
        ];
        static DESC: DeviceDescriptor = DeviceDescriptor {
            stages: &[],
            type_id: "test.bus",
            name: "Bus",
            kind: DeviceKind::Mixer,
            version: 1,
            params: &[],
            sockets: &SOCKETS,
        };
        assert_eq!(
            socket_group_label(SocketKind::Audio, Direction::SummingInput),
            "AUDIO BUS IN"
        );
        assert_eq!(
            socket_group_label(SocketKind::Cv, Direction::SummingInput),
            "CV BUS IN"
        );
        let spec = auto_layout(&DESC);
        let mut placed: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
        placed.sort_unstable();
        assert_eq!(placed, [0, 1, 2]);
        let labels: Vec<&str> = spec
            .back_sections
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert_eq!(labels, ["AUDIO IN", "AUDIO BUS IN", "AUDIO OUT"]);
    }

    #[test]
    fn hand_made_layout_is_used_verbatim() {
        static FRONT: [atomik_device_sdk::PanelControl; 1] = [atomik_device_sdk::PanelControl {
            param: "mix",
            kind: ControlKind::Knob,
            x: 200.0,
            y: 40.0,
        }];
        static BACK: [atomik_device_sdk::PanelSocket; 1] = [atomik_device_sdk::PanelSocket {
            socket: "out_l",
            x: 300.0,
            y: 30.0,
        }];
        let layout = PanelLayout {
            type_id: "atomik.stereo-delay",
            height_units: 2,
            front: &FRONT,
            back: &BACK,
        };
        let spec = from_panel_layout(&layout, &stereo_delay::DESCRIPTOR);
        assert_eq!(spec.height_units, 2);
        assert_eq!(spec.front.len(), 1);
        assert_eq!(spec.front[0].rect.center(), Point::new(200.0, 40.0));
        assert_eq!(spec.back[0].center(), Point::new(300.0, 30.0));
        assert_eq!(spec.back[0].socket, 2);
    }
}
