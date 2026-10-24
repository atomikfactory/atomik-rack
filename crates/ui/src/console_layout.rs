//! Console geometry: where every control of a channel strip and of the master
//! section goes. Pure maths over a [`DeviceDescriptor`]; no document, no drawing.
//!
//! # Strip layout rules
//! - A strip is [`STRIP_W`] wide; the height is the console's strip area.
//! - **Sections**, top to bottom (order): Input, Filters, Dynamics, Parallel, EQ, Inserts, Sends,
//!   CV trim (the fader, pan and send-level CV amounts), More (params this table does not know), then Pan/Width, Fader + meters, Mute/Solo and
//!   the Name plate. Each section has a [`HEADER_H`] header with a fold chevron; a folded
//!   section is just its header. The name plate cannot fold.
//! - The first group (Input … More) is the **upper** part: it starts at y = 0 and scrolls
//!   vertically. Pan, Fader, Mute/Solo and Name are **pinned** to the bottom of the strip
//!   (`pinned_top` … height) so the faders are always visible. Just above the name plate sits
//!   the **output selector** row ([`OUT_H`]; the master's reads its main outputs), so the
//!   faders of the strips and the master line up.
//! - **Short panes:** the upper part keeps at least [`UPPER_MIN_H`] of view. The fader body
//!   shrinks first (down to [`FADER_MIN_H`]); when a pinned Pan would leave less than
//!   [`FADER_COMFORT_H`] of fader, Pan becomes the last upper section and the fader readout
//!   moves into the fader header ([`StripLayout::is_pinned`] tells which).
//! - Rows inside a section are grids of 2 or 3 equal columns across the inner width
//!   ([`PAD`] on each side). Cell heights: knob [`KNOB_H`] (label below), button/choice
//!   [`BUTTON_H`]; a row is as tall as its tallest cell and shorter cells are centred.
//!   Sends are 8 rows of [`SEND_ROW_H`]: on (with the send number), a level slider, pre.
//!   Dynamics has a gain-reduction bar row; Inserts holds the latency-adjust knob. Parallel is
//!   the enable button over three knobs (threshold, ratio, mix); its attack, release and makeup
//!   are in the rack's channel panel only.
//! - The fader body is `clamp(0.42·height, FADER_MIN_H, FADER_MAX_H)` (less in short
//!   panes, see above), with a dB readout ([`LCD_H`]) above it and the stereo meter beside
//!   it. The per-channel latency readout sits in the name plate, right of the caption.
//! - A param missing from the descriptor leaves its cell empty; a row with no params left is
//!   dropped; a section with nothing left is dropped (the name plate always stays). Params
//!   in the `internal.` namespace are never placed ([`is_hidden_param`]).
//!
//! # Master layout rules
//! Three columns, [`MASTER_W`] wide in total: FX returns (8 rows: picker, level, pan, mute),
//! the bus column (inserts and bus compressor with its GR bar, monitoring, then any unknown
//! params) and the master fader column, pinned like a strip so its fader lines up with the
//! channel faders; its mute/solo row holds "clear solos" and "clear mutes", and the space
//! above its fader holds the OUTPUT readouts (peak L/R, bus gain reduction, status lamps;
//! rows that do not fit are dropped). The first two columns scroll vertically together.

use crate::autolayout::control_label;
use atomik_core::{ParamDescriptor, Unit};
use atomik_device_sdk::DeviceDescriptor;
use vello::kurbo::Rect;

/// Type id of a console channel strip device.
pub const CHANNEL_TYPE: &str = "atomik.channel";
/// Type id of the console master section device.
pub const MASTER_TYPE: &str = "atomik.master";
/// Type id of a console output bus strip device (created by console commands only).
pub const BUS_TYPE: &str = "atomik.bus";
/// Prefix of params the UI never shows (set by the app, not saved, not undoable).
pub const HIDDEN_PREFIX: &str = "internal.";

/// Strip width.
pub const STRIP_W: f64 = 92.0;
/// Horizontal gap between strips.
pub const STRIP_GAP: f64 = 2.0;
/// Inner padding on each side of a strip column.
pub const PAD: f64 = 4.0;
/// Section header height.
pub const HEADER_H: f64 = 12.0;
/// Vertical gap after each section.
pub const SECTION_GAP: f64 = 3.0;
/// Knob cell height (knob and label).
pub const KNOB_H: f64 = 40.0;
/// Button / choice cell height.
pub const BUTTON_H: f64 = 20.0;
/// One send row.
pub const SEND_ROW_H: f64 = 19.0;
/// One insert chain slot row.
pub const INSERT_ROW_H: f64 = 15.0;
/// Slots shown in an insert chain.
pub const INSERT_SLOTS: usize = atomik_model::INSERT_SLOTS;
/// Gain-reduction bar row.
pub const GR_ROW_H: f64 = 12.0;
/// Fader dB readout height.
pub const LCD_H: f64 = 18.0;
/// Mute/solo row.
pub const MS_ROW_H: f64 = 24.0;
/// Name plate height.
pub const NAME_H: f64 = 30.0;
/// The output selector row above the name plate (the master shows its main outputs there).
pub const OUT_H: f64 = 16.0;
/// Smallest fader body.
pub const FADER_MIN_H: f64 = 48.0;
/// Largest fader body.
pub const FADER_MAX_H: f64 = 260.0;
/// Smallest visible height of the scrolling upper sections (kept down to a 300 px pane).
pub const UPPER_MIN_H: f64 = 100.0;
/// Fader body below which Pan joins the scrolling sections and the fader readout moves into
/// the fader header (compact pinned stack).
pub const FADER_COMFORT_H: f64 = 120.0;
/// One row of the master output readouts.
pub const OUTPUT_ROW_H: f64 = 18.0;
/// FX returns column width (master).
pub const RETURNS_W: f64 = 184.0;
/// Bus column width (master).
pub const BUS_W: f64 = 132.0;
/// Gap between master columns.
pub const COLUMN_GAP: f64 = 6.0;
/// Master section width.
pub const MASTER_W: f64 = RETURNS_W + COLUMN_GAP + BUS_W + COLUMN_GAP + STRIP_W;
/// One FX return row (master).
pub const RETURN_ROW_H: f64 = 28.0;

/// True for params the UI never shows (`internal.*`).
pub fn is_hidden_param(key: &str) -> bool {
    key.starts_with(HIDDEN_PREFIX)
}

/// A console section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionId {
    /// Input gain and phase.
    Input,
    /// High- and low-pass filters.
    Filters,
    /// Compressor, gate/expander, key.
    Dynamics,
    /// The parallel compressor: enable, threshold, ratio and mix (the rest is in the rack panel).
    Parallel,
    /// 4-band EQ.
    Eq,
    /// Insert bypass, routing switches, latency.
    Inserts,
    /// 8 FX sends.
    Sends,
    /// Params the console does not know (shown generically).
    More,
    /// Pan and width.
    Pan,
    /// Fader and meters.
    Fader,
    /// Mute and solo (on the master: clear solos / clear mutes).
    MuteSolo,
    /// The name plate.
    Name,
    /// Master: FX returns.
    Returns,
    /// Master: inserts and bus compressor.
    Bus,
    /// Master: monitoring.
    Monitor,
    /// Master: output readouts above the master fader.
    Output,
    /// Channel: the CV amounts of the fader, pan and send-level CV inputs.
    Cv,
}

/// The sections of a channel strip, top to bottom.
pub const STRIP_SECTIONS: [SectionId; 13] = [
    SectionId::Input,
    SectionId::Filters,
    SectionId::Dynamics,
    SectionId::Parallel,
    SectionId::Eq,
    SectionId::Inserts,
    SectionId::Sends,
    SectionId::Cv,
    SectionId::More,
    SectionId::Pan,
    SectionId::Fader,
    SectionId::MuteSolo,
    SectionId::Name,
];

impl SectionId {
    /// Header title.
    pub fn title(self) -> &'static str {
        match self {
            SectionId::Input => "INPUT",
            SectionId::Filters => "FILTERS",
            SectionId::Dynamics => "DYNAMICS",
            SectionId::Parallel => "PARALLEL",
            SectionId::Eq => "EQ",
            SectionId::Inserts => "INSERTS",
            SectionId::Sends => "SENDS",
            SectionId::More => "MORE",
            SectionId::Pan => "PAN",
            SectionId::Fader => "FADER",
            SectionId::MuteSolo => "M / S",
            SectionId::Name => "",
            SectionId::Returns => "FX RETURNS",
            SectionId::Bus => "MASTER BUS",
            SectionId::Monitor => "MONITOR",
            SectionId::Output => "OUTPUT",
            SectionId::Cv => "CV TRIM",
        }
    }

    /// Pinned to the bottom of a strip by default. In short panes Pan scrolls instead: use
    /// [`StripLayout::is_pinned`] for a given layout.
    pub fn pinned(self) -> bool {
        matches!(
            self,
            SectionId::Pan | SectionId::Fader | SectionId::MuteSolo | SectionId::Name
        )
    }

    /// Whether the user can fold it (every strip section except the name plate).
    pub fn foldable(self) -> bool {
        STRIP_SECTIONS.contains(&self) && self != SectionId::Name
    }

    fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// Which strip sections are folded. View state only (never in the document); shared by every
/// strip so the sections stay aligned across the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Folds {
    bits: u32,
}

impl Folds {
    /// Every foldable section folded (`true`) or unfolded.
    pub fn all(folded: bool) -> Self {
        let mut f = Self::default();
        for s in STRIP_SECTIONS {
            f.set(s, folded);
        }
        f
    }

    /// Whether `section` is folded.
    pub fn is_folded(self, section: SectionId) -> bool {
        self.bits & section.bit() != 0
    }

    /// Folds or unfolds `section` (ignored for sections that cannot fold).
    pub fn set(&mut self, section: SectionId, folded: bool) {
        if !section.foldable() {
            return;
        }
        if folded {
            self.bits |= section.bit();
        } else {
            self.bits &= !section.bit();
        }
    }

    /// Whether any section is folded.
    pub fn any_folded(self) -> bool {
        self.bits != 0
    }

    /// Whether any foldable section is unfolded.
    pub fn any_unfolded(self) -> bool {
        STRIP_SECTIONS
            .iter()
            .any(|s| s.foldable() && !self.is_folded(*s))
    }
}

/// How a control is drawn and dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlKind {
    /// Small rotary knob (vertical drag).
    Knob,
    /// Latching button (click flips; the label is inside the button).
    Button,
    /// Horizontal level bar (drag in either direction).
    Slider,
    /// Channel or master fader (drag follows the pointer along the travel).
    Fader,
    /// Choice readout (click cycles).
    Choice,
}

/// A placed control.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCtrl {
    /// Param key (from the descriptor).
    pub param: &'static str,
    /// Widget.
    pub kind: CtrlKind,
    /// Short label (inside buttons, under knobs).
    pub label: String,
    /// Cell rectangle in strip (or master) coordinates.
    pub rect: Rect,
    /// Owning section.
    pub section: SectionId,
}

/// A placed section.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedSection {
    /// Which section.
    pub id: SectionId,
    /// Whole section including its header.
    pub rect: Rect,
    /// Header strip (empty for the name plate).
    pub header: Rect,
    /// Folded to its header.
    pub folded: bool,
}

/// Geometry of one channel strip (strip coordinates: x from 0 to [`STRIP_W`], y from the top
/// of the strip area; upper sections are unscrolled).
#[derive(Debug, Clone, PartialEq)]
pub struct StripLayout {
    /// Sections, top to bottom.
    pub sections: Vec<PlacedSection>,
    /// Every placed control.
    pub controls: Vec<PlacedCtrl>,
    /// Total height of the upper (scrolling) part.
    pub upper_height: f64,
    /// Top of the pinned part; the upper part is visible above it.
    pub pinned_top: f64,
    /// Strip height.
    pub height: f64,
    /// Dynamics gain-reduction bar (upper part).
    pub gr_meter: Option<Rect>,
    /// Insert chain slot rows, top to bottom (upper part; empty without an insert loop).
    pub insert_slots: Vec<Rect>,
    /// Latency readout, in the name plate (pinned).
    pub latency: Option<Rect>,
    /// Stereo output meter beside the fader.
    pub meter: Option<Rect>,
    /// Fader dB readout.
    pub fader_lcd: Option<Rect>,
    /// Name plate.
    pub name: Rect,
    /// The output selector row, just above the name plate (pinned).
    pub output: Option<Rect>,
    /// Whether Pan is pinned (false in short panes, where it scrolls).
    pub pan_pinned: bool,
}

impl StripLayout {
    /// Whether `section` is in the pinned part of this layout.
    pub fn is_pinned(&self, section: SectionId) -> bool {
        match section {
            SectionId::Pan => self.pan_pinned,
            s => s.pinned(),
        }
    }
}

/// A readout in the master's OUTPUT section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputItem {
    /// Left peak-hold level.
    PeakLeft,
    /// Right peak-hold level.
    PeakRight,
    /// Bus compressor gain reduction.
    BusGr,
    /// Status lamps (dim, solo, compensation).
    Status,
}

/// Geometry of the master section (x from 0 to [`MASTER_W`], y from the top of the strip
/// area; the returns and bus columns are unscrolled).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterLayout {
    /// Sections.
    pub sections: Vec<PlacedSection>,
    /// Every placed control.
    pub controls: Vec<PlacedCtrl>,
    /// FX return effect pickers: (slot 0..8, rectangle).
    pub return_pickers: Vec<(usize, Rect)>,
    /// Bus compressor gain-reduction bar.
    pub gr_meter: Option<Rect>,
    /// Insert chain slot rows (bus column; empty without an insert loop).
    pub insert_slots: Vec<Rect>,
    /// Master output meter.
    pub meter: Option<Rect>,
    /// Master fader dB readout.
    pub fader_lcd: Option<Rect>,
    /// "Clear solos" button.
    pub clear_solo: Rect,
    /// "Clear mutes" button.
    pub clear_mute: Rect,
    /// Name plate ("MASTER").
    pub name: Rect,
    /// The row above the name plate, aligned with the strips' output selectors: it reads the
    /// main outputs.
    pub output_row: Rect,
    /// Height of the scrolling columns' content.
    pub upper_height: f64,
    /// Top of the pinned fader column part.
    pub pinned_top: f64,
    /// x of the fader column.
    pub fader_x: f64,
    /// OUTPUT readouts above the master fader (only the rows that fit).
    pub output: Vec<(OutputItem, Rect)>,
}

// ------------------------------------------------------------------ static strip map

type Cell = Option<(&'static str, CtrlKind, &'static str)>;

const fn k(key: &'static str, label: &'static str) -> Cell {
    Some((key, CtrlKind::Knob, label))
}
const fn b(key: &'static str, label: &'static str) -> Cell {
    Some((key, CtrlKind::Button, label))
}
const fn ch(key: &'static str, label: &'static str) -> Cell {
    Some((key, CtrlKind::Choice, label))
}

#[derive(Debug, Clone, Copy)]
enum Row {
    Grid(&'static [Cell]),
    GrMeter,
    Sends,
    /// The insert chain slots (only for devices with an insert loop).
    Chain,
}

const INPUT: &[Row] = &[Row::Grid(&[k("input.gain", "GAIN"), b("input.phase", "Ø")])];
const FILTERS: &[Row] = &[
    Row::Grid(&[b("hpf.on", "HPF"), b("lpf.on", "LPF")]),
    Row::Grid(&[k("hpf.freq", "HP"), k("lpf.freq", "LP")]),
];
const DYNAMICS: &[Row] = &[
    Row::Grid(&[
        b("comp.on", "COMP"),
        b("comp.peak", "PEAK"),
        b("comp.fast", "FAST"),
    ]),
    Row::Grid(&[
        k("comp.threshold", "THR"),
        k("comp.ratio", "RATIO"),
        k("comp.release", "REL"),
    ]),
    Row::GrMeter,
    Row::Grid(&[
        b("gate.on", "GATE"),
        b("gate.expander", "EXP"),
        b("gate.fast", "FAST"),
    ]),
    Row::Grid(&[
        k("gate.threshold", "THR"),
        k("gate.range", "RANGE"),
        k("gate.hold", "HOLD"),
    ]),
    Row::Grid(&[k("gate.release", "REL"), b("key.on", "KEY"), None]),
];
const PARALLEL: &[Row] = &[
    Row::Grid(&[b("pcomp.on", "PAR COMP"), None, None]),
    Row::Grid(&[
        k("pcomp.threshold", "THR"),
        k("pcomp.ratio", "RATIO"),
        k("pcomp.mix", "MIX"),
    ]),
];

/// Params of the strip's descriptor that the console does not place (they are in the rack's
/// channel panel) and that the MORE section must not pick up either.
const RACK_ONLY: [&str; 3] = ["pcomp.attack", "pcomp.release", "pcomp.makeup"];

const EQ: &[Row] = &[
    Row::Grid(&[b("eq.on", "EQ"), None, None]),
    Row::Grid(&[
        k("eq.hf.gain", "HF"),
        k("eq.hf.freq", "FREQ"),
        b("eq.hf.bell", "BELL"),
    ]),
    Row::Grid(&[
        k("eq.hmf.gain", "HMF"),
        k("eq.hmf.freq", "FREQ"),
        k("eq.hmf.q", "Q"),
    ]),
    Row::Grid(&[
        k("eq.lmf.gain", "LMF"),
        k("eq.lmf.freq", "FREQ"),
        k("eq.lmf.q", "Q"),
    ]),
    Row::Grid(&[
        k("eq.lf.gain", "LF"),
        k("eq.lf.freq", "FREQ"),
        b("eq.lf.bell", "BELL"),
    ]),
];
const INSERTS: &[Row] = &[
    Row::Chain,
    Row::Grid(&[
        b("insert.bypass", "BYP ALL"),
        b("route.insert_pre", "INS PRE"),
    ]),
    Row::Grid(&[
        b("route.dyn_post_eq", "EQ>DYN"),
        b("route.filters_to_sc", "FLT>SC"),
    ]),
    Row::Grid(&[k("latency.adjust", "LAT ADJ"), None, None]),
];
const SENDS: &[Row] = &[Row::Sends];
const CV: &[Row] = &[
    Row::Grid(&[k("fader_cv_trim", "FADER"), k("pan_cv_trim", "PAN"), None]),
    Row::Grid(&[
        k("send1.level_cv_trim", "S1"),
        k("send2.level_cv_trim", "S2"),
        k("send3.level_cv_trim", "S3"),
    ]),
    Row::Grid(&[
        k("send4.level_cv_trim", "S4"),
        k("send5.level_cv_trim", "S5"),
        k("send6.level_cv_trim", "S6"),
    ]),
    Row::Grid(&[
        k("send7.level_cv_trim", "S7"),
        k("send8.level_cv_trim", "S8"),
        None,
    ]),
];
const PAN: &[Row] = &[Row::Grid(&[k("pan", "PAN"), k("width", "WIDTH")])];
const MUTE_SOLO: &[Row] = &[Row::Grid(&[b("mute", "M"), b("solo", "S")])];

const BUS: &[Row] = &[
    Row::Chain,
    Row::Grid(&[
        b("insert.bypass", "INS BYP"),
        b("insert.pre_comp", "PRE COMP"),
    ]),
    Row::Grid(&[b("comp.on", "COMP"), b("comp.key", "KEY")]),
    Row::Grid(&[
        k("comp.threshold", "THR"),
        k("comp.ratio", "RATIO"),
        k("comp.attack", "ATK"),
    ]),
    Row::Grid(&[k("comp.release", "REL"), k("comp.makeup", "GAIN"), None]),
    Row::GrMeter,
];
const MONITOR: &[Row] = &[Row::Grid(&[
    k("monitor.level", "LEVEL"),
    b("monitor.dim", "DIM"),
    ch("monitor.source", "SRC"),
])];

/// The master's send trims: `send{k}.level` of each bus, two rows of four.
const MASTER_SENDS: &[Row] = &[
    Row::Grid(&[
        k("send1.level", "S1"),
        k("send2.level", "S2"),
        k("send3.level", "S3"),
        k("send4.level", "S4"),
    ]),
    Row::Grid(&[
        k("send5.level", "S5"),
        k("send6.level", "S6"),
        k("send7.level", "S7"),
        k("send8.level", "S8"),
    ]),
];

fn strip_rows(id: SectionId) -> &'static [Row] {
    match id {
        SectionId::Input => INPUT,
        SectionId::Filters => FILTERS,
        SectionId::Dynamics => DYNAMICS,
        SectionId::Parallel => PARALLEL,
        SectionId::Eq => EQ,
        SectionId::Inserts => INSERTS,
        SectionId::Sends => SENDS,
        SectionId::Cv => CV,
        SectionId::Pan => PAN,
        SectionId::MuteSolo => MUTE_SOLO,
        SectionId::Bus => BUS,
        SectionId::Monitor => MONITOR,
        _ => &[],
    }
}

/// Send-row keys for send `n` (1-based).
fn send_keys(n: usize) -> [String; 3] {
    [
        format!("send{n}.on"),
        format!("send{n}.level"),
        format!("send{n}.pre"),
    ]
}

/// Keys of the master's FX return `n` (1-based): level, mute, pan.
fn return_keys(n: usize) -> [String; 3] {
    [
        format!("return{n}.level"),
        format!("return{n}.mute"),
        format!("return{n}.pan"),
    ]
}

fn table_keys(rows: &[Row]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows {
        match r {
            Row::Grid(cells) => out.extend(cells.iter().flatten().map(|c| c.0.to_string())),
            Row::Sends => {
                for n in 1..=8 {
                    out.extend(send_keys(n));
                }
            }
            Row::GrMeter | Row::Chain => {}
        }
    }
    out
}

fn has(desc: &DeviceDescriptor, key: &str) -> Option<&'static str> {
    // Descriptor params are 'static data; the key is copied out as such.
    desc.params
        .iter()
        .find(|p| p.key == key && !is_hidden_param(p.key))
        .map(|p| p.key)
}

fn generic_kind(p: &ParamDescriptor) -> CtrlKind {
    match p.unit {
        Unit::Toggle => CtrlKind::Button,
        Unit::Choice(_) => CtrlKind::Choice,
        _ => CtrlKind::Knob,
    }
}

fn short_label(p: &ParamDescriptor) -> String {
    control_label(p).to_uppercase()
}

/// Lays out `rows` of a section starting at `y` within x range `x0..x1`; returns the content
/// height (0 when nothing of the section is present).
struct Placer<'a> {
    desc: &'a DeviceDescriptor,
    x0: f64,
    x1: f64,
    section: SectionId,
    controls: Vec<PlacedCtrl>,
    gr: Option<Rect>,
    slots: Vec<Rect>,
}

impl Placer<'_> {
    fn cell_h(kind: CtrlKind) -> f64 {
        match kind {
            CtrlKind::Knob => KNOB_H,
            _ => BUTTON_H,
        }
    }

    fn place(&mut self, key: &'static str, kind: CtrlKind, label: String, rect: Rect) {
        self.controls.push(PlacedCtrl {
            param: key,
            kind,
            label,
            rect,
            section: self.section,
        });
    }

    fn grid(&mut self, cells: &[(Option<&'static str>, CtrlKind, String)], y: f64) -> f64 {
        if cells.iter().all(|c| c.0.is_none()) {
            return 0.0;
        }
        let h = cells
            .iter()
            .filter(|c| c.0.is_some())
            .map(|c| Self::cell_h(c.1))
            .fold(0.0, f64::max);
        let w = (self.x1 - self.x0) / cells.len() as f64;
        for (i, (key, kind, label)) in cells.iter().enumerate() {
            let Some(key) = key else { continue };
            let ch = Self::cell_h(*kind);
            let x = self.x0 + w * i as f64;
            let top = y + ((h - ch) / 2.0).floor();
            self.place(
                key,
                *kind,
                label.clone(),
                Rect::new(x, top, x + w, top + ch),
            );
        }
        h
    }

    fn rows(&mut self, rows: &[Row], mut y: f64) -> f64 {
        let start = y;
        let mut any = false;
        for r in rows {
            let h = match r {
                Row::Grid(cells) => {
                    let cells: Vec<_> = cells
                        .iter()
                        .map(|c| match c {
                            Some((key, kind, label)) => {
                                (has(self.desc, key), *kind, (*label).to_string())
                            }
                            None => (None, CtrlKind::Knob, String::new()),
                        })
                        .collect();
                    self.grid(&cells, y)
                }
                Row::GrMeter => {
                    if any {
                        self.gr = Some(Rect::new(self.x0 + 2.0, y + 2.0, self.x1 - 2.0, y + 9.0));
                        GR_ROW_H
                    } else {
                        0.0
                    }
                }
                Row::Sends => self.sends(y),
                Row::Chain => self.chain(y),
            };
            any |= h > 0.0 && !matches!(r, Row::GrMeter);
            y += h;
        }
        y - start
    }

    /// The insert chain slot rows (`INSERT_SLOTS` fixed rows keep every strip aligned).
    fn chain(&mut self, y: f64) -> f64 {
        if self.desc.socket_by_key("insert_send_l").is_none() {
            return 0.0;
        }
        for i in 0..INSERT_SLOTS {
            let top = y + i as f64 * INSERT_ROW_H;
            self.slots.push(Rect::new(
                self.x0,
                top + 1.0,
                self.x1,
                top + INSERT_ROW_H - 1.0,
            ));
        }
        INSERT_SLOTS as f64 * INSERT_ROW_H + 2.0
    }

    fn sends(&mut self, mut y: f64) -> f64 {
        let start = y;
        for n in 1..=8 {
            let [on, level, pre] = send_keys(n);
            let (on, level, pre) = (
                has(self.desc, &on),
                has(self.desc, &level),
                has(self.desc, &pre),
            );
            if on.is_none() && level.is_none() && pre.is_none() {
                continue;
            }
            let (x0, x1) = (self.x0, self.x1);
            let (top, bot) = (y + 2.0, y + SEND_ROW_H - 2.0);
            if let Some(key) = on {
                self.place(
                    key,
                    CtrlKind::Button,
                    n.to_string(),
                    Rect::new(x0, top, x0 + 18.0, bot),
                );
            }
            if let Some(key) = level {
                let r = Rect::new(x0 + 21.0, top, x1 - 27.0, bot);
                self.place(key, CtrlKind::Slider, String::new(), r);
            }
            if let Some(key) = pre {
                self.place(
                    key,
                    CtrlKind::Button,
                    "PRE".into(),
                    Rect::new(x1 - 24.0, top, x1, bot),
                );
            }
            y += SEND_ROW_H;
        }
        y - start
    }

    fn extras(&mut self, known: &[String], y: f64) -> f64 {
        let extra: Vec<&'static ParamDescriptor> = self
            .desc
            .params
            .iter()
            .filter(|p| !is_hidden_param(p.key) && !known.iter().any(|k| k == p.key))
            .collect();
        let mut h = 0.0;
        for chunk in extra.chunks(3) {
            let mut cells: Vec<_> = chunk
                .iter()
                .map(|p| (Some(p.key), generic_kind(p), short_label(p)))
                .collect();
            while cells.len() < 3 {
                cells.push((None, CtrlKind::Knob, String::new()));
            }
            h += self.grid(&cells, y + h);
        }
        h
    }
}

/// How the pinned stack is arranged for a strip area `height` tall.
#[derive(Debug, Clone, Copy)]
struct Plan {
    body: f64,
    pan_pinned: bool,
    /// Fader readout inside the fader header (no readout row).
    lcd_in_header: bool,
}

const PAN_CONTENT: f64 = KNOB_H + 4.0;

fn plan(height: f64, folds: Folds, has_ms: bool, has_pan: bool) -> Plan {
    let fold = |id| folds.is_folded(id);
    let ms = if has_ms {
        HEADER_H
            + if fold(SectionId::MuteSolo) {
                0.0
            } else {
                MS_ROW_H
            }
            + SECTION_GAP
    } else {
        0.0
    };
    let fader_folded = fold(SectionId::Fader);
    let common = NAME_H
        + SECTION_GAP
        + OUT_H
        + SECTION_GAP
        + ms
        + HEADER_H
        + SECTION_GAP
        + if fader_folded { 0.0 } else { 4.0 };
    let pan = if has_pan {
        HEADER_H
            + if fold(SectionId::Pan) {
                0.0
            } else {
                PAN_CONTENT
            }
            + SECTION_GAP
    } else {
        0.0
    };
    let preferred = (height * 0.42).clamp(FADER_MIN_H, FADER_MAX_H);
    let roomy = height - common - LCD_H - pan - UPPER_MIN_H;
    let full = if fader_folded {
        roomy >= 0.0
    } else {
        roomy >= FADER_COMFORT_H
    };
    if full {
        return Plan {
            body: preferred.min(roomy).max(FADER_MIN_H),
            pan_pinned: true,
            lcd_in_header: false,
        };
    }
    let room = height - common - UPPER_MIN_H;
    Plan {
        body: preferred.min(room).clamp(FADER_MIN_H, FADER_MAX_H),
        pan_pinned: !has_pan,
        lcd_in_header: true,
    }
}

/// The placed fader rects: (fader, meter, readout).
type FaderRects = (Rect, Rect, Rect);

/// Places the fader section with its bottom at `bottom`; the rects only when unfolded.
fn fader_section(
    x: (f64, f64),
    bottom: f64,
    plan: Plan,
    folded: bool,
) -> (PlacedSection, Option<FaderRects>) {
    let inner = (x.0 + PAD, x.1 - PAD);
    let lcd_row = if plan.lcd_in_header { 0.0 } else { LCD_H };
    let content = lcd_row + plan.body + 4.0;
    let s = section(
        SectionId::Fader,
        bottom - if folded { HEADER_H } else { HEADER_H + content },
        x,
        content,
        folded,
    );
    if folded {
        return (s, None);
    }
    let lcd = if plan.lcd_in_header {
        Rect::new(
            inner.0 + 40.0,
            s.header.y0 + 1.0,
            inner.1,
            s.header.y1 - 1.0,
        )
    } else {
        let y = s.header.y1 + 2.0;
        Rect::new(inner.0 + 2.0, y, inner.1 - 2.0, y + LCD_H - 4.0)
    };
    let fy = s.header.y1 + 2.0 + lcd_row;
    let fader = Rect::new(inner.0, fy, inner.0 + 38.0, fy + plan.body);
    let meter = Rect::new(inner.1 - 30.0, fy, inner.1, fy + plan.body);
    (s, Some((fader, meter, lcd)))
}

fn section(id: SectionId, y: f64, w: (f64, f64), content: f64, folded: bool) -> PlacedSection {
    let h = if folded { HEADER_H } else { HEADER_H + content };
    PlacedSection {
        id,
        rect: Rect::new(w.0, y, w.1, y + h),
        header: Rect::new(w.0, y, w.1, y + HEADER_H),
        folded,
    }
}

/// Lays out one channel strip of `desc` for a strip area `height` tall.
pub fn strip_layout(desc: &DeviceDescriptor, folds: Folds, height: f64) -> StripLayout {
    let mut known: Vec<String> = Vec::new();
    for id in STRIP_SECTIONS {
        known.extend(table_keys(strip_rows(id)));
    }
    known.push("fader".into());
    known.extend(RACK_ONLY.iter().map(|k| (*k).to_owned()));

    let mut sections = Vec::new();
    let mut controls = Vec::new();
    let mut gr_meter = None;
    let mut insert_slots = Vec::new();
    let inner = (PAD, STRIP_W - PAD);
    let has_ms = has(desc, "mute").is_some() || has(desc, "solo").is_some();
    let has_pan = has(desc, "pan").is_some() || has(desc, "width").is_some();
    let plan = plan(height, folds, has_ms, has_pan);

    // Upper part, from the top (Pan last when it does not fit the pinned part).
    let mut y = 0.0;
    let upper_ids = STRIP_SECTIONS
        .iter()
        .copied()
        .filter(|s| !s.pinned() || (*s == SectionId::Pan && !plan.pan_pinned));
    for id in upper_ids {
        let mut p = Placer {
            desc,
            x0: inner.0,
            x1: inner.1,
            section: id,
            controls: Vec::new(),
            gr: None,
            slots: Vec::new(),
        };
        let top = y + HEADER_H + 2.0;
        let h = if id == SectionId::More {
            p.extras(&known, top)
        } else {
            p.rows(strip_rows(id), top)
        };
        if p.controls.is_empty() && p.slots.is_empty() {
            continue;
        }
        let folded = folds.is_folded(id);
        sections.push(section(id, y, (0.0, STRIP_W), h + 4.0, folded));
        if !folded {
            controls.extend(p.controls);
            gr_meter = gr_meter.or(p.gr);
            insert_slots.extend(p.slots);
        }
        y += sections
            .last()
            .map_or(0.0, |s: &PlacedSection| s.rect.height())
            + SECTION_GAP;
    }
    let upper_height = y;

    // Pinned part, from the bottom.
    let name = Rect::new(0.0, height - NAME_H, STRIP_W, height);
    let output = Rect::new(
        inner.0,
        name.y0 - SECTION_GAP - OUT_H,
        inner.1,
        name.y0 - SECTION_GAP,
    );
    let mut bottom = output.y0 - SECTION_GAP;
    let mut pinned: Vec<PlacedSection> = Vec::new();
    let (mut meter, mut fader_lcd) = (None, None);

    // Mute / solo.
    let ms_keys = (has(desc, "mute"), has(desc, "solo"));
    if ms_keys.0.is_some() || ms_keys.1.is_some() {
        let folded = folds.is_folded(SectionId::MuteSolo);
        let content = MS_ROW_H;
        let s = section(
            SectionId::MuteSolo,
            bottom - if folded { HEADER_H } else { HEADER_H + content },
            (0.0, STRIP_W),
            content,
            folded,
        );
        if !folded {
            let y = s.header.y1 + 2.0;
            let half = (inner.1 - inner.0) / 2.0;
            for (i, (key, label)) in [(ms_keys.0, "M"), (ms_keys.1, "S")].into_iter().enumerate() {
                if let Some(key) = key {
                    let x = inner.0 + half * i as f64;
                    controls.push(PlacedCtrl {
                        param: key,
                        kind: CtrlKind::Button,
                        label: label.into(),
                        rect: Rect::new(x + 1.0, y, x + half - 1.0, y + MS_ROW_H - 4.0),
                        section: SectionId::MuteSolo,
                    });
                }
            }
        }
        bottom = s.rect.y0 - SECTION_GAP;
        pinned.push(s);
    }

    // Fader and meters.
    if let Some(key) = has(desc, "fader") {
        let folded = folds.is_folded(SectionId::Fader);
        let (s, rects) = fader_section((0.0, STRIP_W), bottom, plan, folded);
        if let Some((fader, m, lcd)) = rects {
            fader_lcd = Some(lcd);
            meter = Some(m);
            controls.push(PlacedCtrl {
                param: key,
                kind: CtrlKind::Fader,
                label: String::new(),
                rect: fader,
                section: SectionId::Fader,
            });
        }
        bottom = s.rect.y0 - SECTION_GAP;
        pinned.push(s);
    }

    // Pan / width.
    let mut p = Placer {
        desc,
        x0: inner.0,
        x1: inner.1,
        section: SectionId::Pan,
        controls: Vec::new(),
        gr: None,
        slots: Vec::new(),
    };
    let pan_h = p.rows(PAN, 0.0);
    if plan.pan_pinned && !p.controls.is_empty() {
        let folded = folds.is_folded(SectionId::Pan);
        let content = pan_h + 4.0;
        let top = bottom - if folded { HEADER_H } else { HEADER_H + content };
        let s = section(SectionId::Pan, top, (0.0, STRIP_W), content, folded);
        if !folded {
            let dy = s.header.y1 + 2.0;
            controls.extend(p.controls.into_iter().map(|mut c| {
                c.rect = c.rect + vello::kurbo::Vec2::new(0.0, dy);
                c
            }));
        }
        bottom = s.rect.y0 - SECTION_GAP;
        pinned.push(s);
    }
    let pinned_top = bottom + SECTION_GAP;

    pinned.reverse();
    sections.extend(pinned);
    sections.push(PlacedSection {
        id: SectionId::Name,
        rect: name,
        header: Rect::new(0.0, name.y0, STRIP_W, name.y0),
        folded: false,
    });
    let latency = Some(Rect::new(
        STRIP_W / 2.0 - 4.0,
        name.y1 - 13.0,
        STRIP_W - 15.0,
        name.y1 - 3.0,
    ));
    StripLayout {
        sections,
        controls,
        upper_height,
        pinned_top,
        height,
        gr_meter,
        insert_slots,
        latency,
        meter,
        fader_lcd,
        name,
        output: Some(output),
        pan_pinned: plan.pan_pinned,
    }
}

/// Lays out the master section of `desc` for a strip area `height` tall.
pub fn master_layout(desc: &DeviceDescriptor, height: f64) -> MasterLayout {
    let mut sections = Vec::new();
    let mut controls = Vec::new();
    let mut return_pickers = Vec::new();

    // FX returns column.
    let ret = (0.0, RETURNS_W);
    let rows_top = HEADER_H + 12.0;
    for slot in 0..8 {
        let y = rows_top + RETURN_ROW_H * slot as f64;
        let mid = y + RETURN_ROW_H / 2.0;
        return_pickers.push((slot, Rect::new(16.0, mid - 8.0, 96.0, mid + 8.0)));
        let [level, mute, pan] = return_keys(slot + 1);
        for (key, kind, x0, x1) in [
            (level, CtrlKind::Knob, 98.0, 126.0),
            (pan, CtrlKind::Knob, 128.0, 156.0),
            (mute, CtrlKind::Button, 159.0, 181.0),
        ] {
            if let Some(key) = has(desc, &key) {
                let r = match kind {
                    CtrlKind::Button => Rect::new(x0, mid - 8.0, x1, mid + 8.0),
                    _ => Rect::new(x0, y, x1, y + RETURN_ROW_H),
                };
                controls.push(PlacedCtrl {
                    param: key,
                    kind,
                    label: if kind == CtrlKind::Button {
                        "M".into()
                    } else {
                        String::new()
                    },
                    rect: r,
                    section: SectionId::Returns,
                });
            }
        }
    }
    let ret_h = rows_top + RETURN_ROW_H * 8.0 + 4.0 - HEADER_H;
    sections.push(section(SectionId::Returns, 0.0, ret, ret_h, false));

    // Fader column, pinned like a strip.
    let bus = (RETURNS_W + COLUMN_GAP, RETURNS_W + COLUMN_GAP + BUS_W);
    let fx = bus.1 + COLUMN_GAP;
    let inner = (fx + PAD, fx + STRIP_W - PAD);
    let name = Rect::new(fx, height - NAME_H, fx + STRIP_W, height);
    let output_row = Rect::new(
        inner.0,
        name.y0 - SECTION_GAP - OUT_H,
        inner.1,
        name.y0 - SECTION_GAP,
    );
    let ms = section(
        SectionId::MuteSolo,
        output_row.y0 - SECTION_GAP - HEADER_H - MS_ROW_H,
        (fx, fx + STRIP_W),
        MS_ROW_H,
        false,
    );
    let half = (inner.1 - inner.0) / 2.0;
    let by = ms.header.y1 + 2.0;
    let clear_solo = Rect::new(inner.0 + 1.0, by, inner.0 + half - 1.0, by + MS_ROW_H - 4.0);
    let clear_mute = Rect::new(inner.0 + half + 1.0, by, inner.1 - 1.0, by + MS_ROW_H - 4.0);
    let mut pinned_top = ms.rect.y0;
    let (mut meter, mut fader_lcd) = (None, None);
    let mut fader_sec = None;
    if let Some(key) = has(desc, "fader") {
        // Same plan as a default strip, so the faders line up.
        let plan = plan(height, Folds::default(), true, true);
        let (s, rects) = fader_section((fx, fx + STRIP_W), ms.rect.y0 - SECTION_GAP, plan, false);
        if let Some((fader, m, lcd)) = rects {
            fader_lcd = Some(lcd);
            meter = Some(m);
            controls.push(PlacedCtrl {
                param: key,
                kind: CtrlKind::Fader,
                label: String::new(),
                rect: fader,
                section: SectionId::Fader,
            });
        }
        pinned_top = s.rect.y0;
        fader_sec = Some(s);
    }
    // OUTPUT readouts in the space above the fader: the rows that fit.
    let mut output = Vec::new();
    let limit = pinned_top - SECTION_GAP;
    let rows_y = HEADER_H + 2.0;
    let rows: [&[(OutputItem, f64, f64)]; 3] = [
        &[
            (OutputItem::PeakLeft, inner.0, inner.0 + half - 1.0),
            (OutputItem::PeakRight, inner.0 + half + 1.0, inner.1),
        ],
        &[(OutputItem::BusGr, inner.0, inner.1)],
        &[(OutputItem::Status, inner.0, inner.1)],
    ];
    for (i, row) in rows.iter().enumerate() {
        let y = rows_y + OUTPUT_ROW_H * i as f64;
        if y + OUTPUT_ROW_H > limit - 2.0 {
            break;
        }
        for (item, x0, x1) in row.iter() {
            output.push((*item, Rect::new(*x0, y + 1.0, *x1, y + OUTPUT_ROW_H - 2.0)));
        }
    }
    if let Some(last) = output.last() {
        let content = last.1.y1 + 3.0 - HEADER_H;
        sections.push(section(
            SectionId::Output,
            0.0,
            (fx, fx + STRIP_W),
            content,
            false,
        ));
    }
    // Bus column: inserts + compressor, monitor (unless above the fader), extras.
    let mut known: Vec<String> = table_keys(BUS);
    known.extend(table_keys(MONITOR));
    known.push("fader".into());
    for n in 1..=8 {
        known.extend(return_keys(n));
        // The send levels have their own section (`MASTER_SENDS`), not MORE.
        known.push(format!("send{n}.level"));
    }
    let mut y = 0.0;
    let mut gr_meter = None;
    let mut insert_slots = Vec::new();
    // Monitoring moves above the master fader when the fader column has room for it.
    let mon_y = output.last().map_or(0.0, |o| o.1.y1 + 3.0 + SECTION_GAP);
    let monitor_up = mon_y + HEADER_H + KNOB_H + 4.0 <= pinned_top - SECTION_GAP;
    for id in [
        SectionId::Bus,
        SectionId::Monitor,
        SectionId::Sends,
        SectionId::More,
    ] {
        let in_fader = id == SectionId::Monitor && monitor_up;
        let (col, sy) = if in_fader {
            ((fx, fx + STRIP_W), mon_y)
        } else {
            (bus, y)
        };
        let mut p = Placer {
            desc,
            x0: col.0 + if in_fader { PAD } else { 2.0 },
            x1: col.1 - if in_fader { PAD } else { 2.0 },
            section: id,
            controls: Vec::new(),
            gr: None,
            slots: Vec::new(),
        };
        let top = sy + HEADER_H + 2.0;
        let h = if id == SectionId::More {
            p.extras(&known, top)
        } else if id == SectionId::Sends {
            p.rows(MASTER_SENDS, top)
        } else {
            p.rows(strip_rows(id), top)
        };
        if p.controls.is_empty() && p.slots.is_empty() {
            continue;
        }
        let s = section(id, sy, col, h + 4.0, false);
        if !in_fader {
            y = s.rect.y1 + SECTION_GAP;
        }
        sections.push(s);
        controls.extend(p.controls);
        gr_meter = gr_meter.or(p.gr);
        insert_slots.extend(p.slots);
    }
    let upper_height = (ret_h + HEADER_H).max(y);

    sections.extend(fader_sec);
    sections.push(ms);
    sections.push(PlacedSection {
        id: SectionId::Name,
        rect: name,
        header: Rect::new(fx, name.y0, fx + STRIP_W, name.y0),
        folded: false,
    });
    MasterLayout {
        sections,
        controls,
        return_pickers,
        gr_meter,
        insert_slots,
        meter,
        fader_lcd,
        clear_solo,
        clear_mute,
        name,
        output_row,
        upper_height,
        pinned_top,
        fader_x: fx,
        output,
    }
}
