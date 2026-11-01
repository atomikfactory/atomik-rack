//! The drum generator's rack panel (four rack units): eight pattern slots of the shown bank (A to
//! D, 32 patterns in all), a name line, a resolution and a shuffle stepper, the step grid (16
//! lanes by 16 visible steps with a page selector), the pattern buttons (ADD, DUP, DEL, COPY,
//! PASTE, CLEAR), GENERATE, RANDOMISE and the cell mode, a length stepper and the twelve knobs.
//!
//! ```text
//! STEPS                                  PATTERNS
//! kick    [M] [] [] [] [] ...16 steps    [1][2][3][4][5][6][7][8]
//! snare   [M] [] [] [] [] ...            Groove  ·  16 steps        [A][B][C][D]
//! ...                                    [< RES 1/16 >]  [- SHUF 0 % +]
//! 16 lanes                               [ADD][DUP][DEL][COPY][PASTE][CLEAR]
//! [1-16][17-32][33-48][49-64]   LEN - +  [GENERATE] [RANDOMISE] [VEL]
//!                                        knobs
//! ```
//!
//! # Meaning of the grid
//!
//! A cell is one step of one lane; its brightness is the velocity and a dot at its top marks a
//! probability below 100 %. A click toggles the step (a hit at velocity 100, or none); a vertical
//! drag on a cell sets its velocity (one undo step). Cells at or beyond the pattern length are
//! dimmed and cannot be edited; LEN - and LEN + change the length (Shift steps by four). The
//! lane label is the pad name of the drum machine the generator is cabled to (directly or through
//! other players) for the keys 36 to 51, else the note name of the lane's key; the square left of
//! the cells mutes the lane.
//!
//! The mode button next to RANDOMISE chooses what the cells edit: **VEL** (velocity, as above),
//! **FLAM** or **RATCH**. In those two modes only a cell with a hit takes the press: a click steps
//! through the flam values (0, 12, 25, 50 %) or the ratchet values (0, 2, 3, 4, 6, 8 hits), a
//! vertical drag sets the value (100 px are the whole flam range, 12 px a ratchet hit); each is
//! one undo step. A flam shows as a bar along the bottom edge of the cell, a ratchet as one tick
//! per hit.
//!
//! The bank buttons A to D show eight patterns each (the slot buttons then number 1 to 8, 9 to 16,
//! ...); a click on a bank selects the same slot in it, or its last pattern when it is only partly
//! filled, and a bank without a pattern does nothing. RES steps the edited pattern through the
//! resolutions 1/4 to 1/32T and SHUF moves its swing by 5 % (Shift: 1 %).
//!
//! The panel distinguishes the **edited** pattern (view state; the grid, the name line and the
//! buttons act on it) from the **playing** one (the device's `pattern` param, a dot on its slot)
//! and the **queued** one (a small triangle), like the step sequencer's panel. The device
//! publishes `[playing pattern (0-based), step, running]` as its meter values, which light the
//! playing step's column.

use crate::autolayout::{CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{button_sized, edit_button, fit_label};
use crate::seq_panel::{PlayerInfo, back_layout, place_controls, play_state, resolution_label};
use crate::theme::{Theme, mix};
use atomik_device_sdk::{DRUM_LANES, RACK_UNIT};
use atomik_device_sdk::{MAX_FLAM, MAX_RATCHET};
use atomik_model::{DrumDoc, DrumPatternDoc, MAX_DRUM_PATTERNS, SeqResolution};
use vello::kurbo::{BezPath, Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the drum generator.
pub const DRUM_GEN_TYPE: &str = "atomik.drum-generator";
/// Height of the panel in rack units.
pub const DRUM_GEN_UNITS: u8 = 4;
/// Steps shown at once.
pub const VISIBLE_STEPS: usize = 16;
/// Pages of the step selector (64 steps).
pub const PAGES: usize = 4;
/// Velocity of a hit placed by a click.
pub const DEFAULT_VELOCITY: u8 = 100;
/// Pattern slots.
pub const SLOTS: usize = 8;
/// Banks of pattern slots (the slot buttons show one bank of [`SLOTS`] at a time).
pub const BANKS: usize = MAX_DRUM_PATTERNS / SLOTS;
/// What a click on a cell sets in the flam mode, in percent of the step.
pub const FLAM_CLICKS: [u8; 4] = [0, 12, 25, MAX_FLAM];
/// What a click on a cell sets in the ratchet mode, in hits per step.
pub const RATCHET_CLICKS: [u8; 6] = [0, 2, 3, 4, 6, MAX_RATCHET];

const SLOT_GAP: f64 = 2.0;
const LABEL_W: f64 = 52.0;
const MUTE_W: f64 = 11.0;
const CELL_W: f64 = 14.0;
const CELL_H: f64 = 13.0;
const GRID_W: f64 = VISIBLE_STEPS as f64 * CELL_W;
const GRID_X: f64 = CONTENT_X.0 + LABEL_W + MUTE_W + 5.0;
const KNOBS_X: f64 = GRID_X + GRID_W + 8.0;
const RIGHT_W: f64 = CONTENT_X.1 - KNOBS_X;
const COL_W: f64 = 64.0;
const PER_ROW: usize = 5;

/// The drum generator's device params in front layout order (five per row).
pub const DRUM_GEN_PARAMS: [&str; 12] = [
    "pattern",
    "pattern.switch",
    "mute",
    "random.seed",
    "style",
    "density",
    "variation",
    "fill.every",
    "fill.amount",
    "accent",
    "velocity.scale",
    "gate",
];

/// A button or area of the drum generator panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumButton {
    /// Pattern slot `0..8`.
    Slot(u8),
    /// Append an empty pattern.
    Add,
    /// Append a copy of the edited pattern.
    Duplicate,
    /// Remove the edited pattern.
    Remove,
    /// Copy the edited pattern to the clipboard.
    Copy,
    /// Paste the clipboard into the edited pattern.
    Paste,
    /// Clear the steps of the edited pattern.
    Clear,
    /// Write the edited pattern from the role templates.
    Generate,
    /// Write it again from the next seed.
    Randomise,
    /// One step shorter (Shift: four).
    LenDec,
    /// One step longer (Shift: four).
    LenInc,
    /// Page `0..4` of the steps.
    Page(u8),
    /// The mute square of lane `0..16`.
    Mute(u8),
    /// Bank `0..4` of pattern slots.
    Bank(u8),
    /// The previous step resolution of the edited pattern.
    ResDec,
    /// The next step resolution of the edited pattern.
    ResInc,
    /// Less shuffle (swing) on the edited pattern (Shift: finer).
    ShufDec,
    /// More shuffle (swing) on the edited pattern (Shift: finer).
    ShufInc,
    /// What the cells edit: velocity, flam or ratchet.
    Mode,
    /// The step cells.
    Grid,
}

/// Geometry of the drum generator panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumGenPanel {
    /// Title of the step section.
    pub title: Rect,
    /// The eight pattern slots.
    pub slots: [Rect; SLOTS],
    /// The name line of the edited pattern.
    pub name: Rect,
    /// The 16 by 16 cells of the visible page.
    pub grid: Rect,
    /// The lane labels.
    pub labels: [Rect; DRUM_LANES],
    /// The lane mute squares.
    pub mutes: [Rect; DRUM_LANES],
    /// The page selector.
    pub pages: [Rect; PAGES],
    /// ADD.
    pub add: Rect,
    /// DUP.
    pub duplicate: Rect,
    /// DEL.
    pub remove: Rect,
    /// COPY.
    pub copy: Rect,
    /// PASTE.
    pub paste: Rect,
    /// CLEAR.
    pub clear: Rect,
    /// GENERATE.
    pub generate: Rect,
    /// RANDOMISE.
    pub randomise: Rect,
    /// LEN -.
    pub len_dec: Rect,
    /// LEN +.
    pub len_inc: Rect,
    /// The length read-out between the two.
    pub len_text: Rect,
    /// The four bank buttons.
    pub banks: [Rect; BANKS],
    /// Previous resolution.
    pub res_dec: Rect,
    /// The resolution read-out.
    pub res_text: Rect,
    /// Next resolution.
    pub res_inc: Rect,
    /// Less shuffle.
    pub shuf_dec: Rect,
    /// The shuffle read-out.
    pub shuf_text: Rect,
    /// More shuffle.
    pub shuf_inc: Rect,
    /// The cell edit mode button.
    pub mode: Rect,
}

impl DrumGenPanel {
    /// Every button, in drawing order (the grid is not one of them).
    pub fn buttons(&self) -> Vec<(DrumButton, Rect)> {
        let mut v: Vec<(DrumButton, Rect)> = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, r)| (DrumButton::Slot(i as u8), *r))
            .collect();
        v.extend(
            self.pages
                .iter()
                .enumerate()
                .map(|(i, r)| (DrumButton::Page(i as u8), *r)),
        );
        v.extend(
            self.mutes
                .iter()
                .enumerate()
                .map(|(i, r)| (DrumButton::Mute(i as u8), *r)),
        );
        v.extend(
            self.banks
                .iter()
                .enumerate()
                .map(|(i, r)| (DrumButton::Bank(i as u8), *r)),
        );
        v.extend([
            (DrumButton::Add, self.add),
            (DrumButton::Duplicate, self.duplicate),
            (DrumButton::Remove, self.remove),
            (DrumButton::Copy, self.copy),
            (DrumButton::Paste, self.paste),
            (DrumButton::Clear, self.clear),
            (DrumButton::Generate, self.generate),
            (DrumButton::Randomise, self.randomise),
            (DrumButton::LenDec, self.len_dec),
            (DrumButton::LenInc, self.len_inc),
            (DrumButton::ResDec, self.res_dec),
            (DrumButton::ResInc, self.res_inc),
            (DrumButton::ShufDec, self.shuf_dec),
            (DrumButton::ShufInc, self.shuf_inc),
            (DrumButton::Mode, self.mode),
        ]);
        v
    }

    /// The rectangle of a button or of the grid.
    pub fn rect(&self, b: DrumButton) -> Rect {
        match b {
            DrumButton::Slot(i) => self.slots[usize::from(i).min(SLOTS - 1)],
            DrumButton::Add => self.add,
            DrumButton::Duplicate => self.duplicate,
            DrumButton::Remove => self.remove,
            DrumButton::Copy => self.copy,
            DrumButton::Paste => self.paste,
            DrumButton::Clear => self.clear,
            DrumButton::Generate => self.generate,
            DrumButton::Randomise => self.randomise,
            DrumButton::LenDec => self.len_dec,
            DrumButton::LenInc => self.len_inc,
            DrumButton::ResDec => self.res_dec,
            DrumButton::ResInc => self.res_inc,
            DrumButton::ShufDec => self.shuf_dec,
            DrumButton::ShufInc => self.shuf_inc,
            DrumButton::Mode => self.mode,
            DrumButton::Bank(i) => self.banks[usize::from(i).min(BANKS - 1)],
            DrumButton::Page(i) => self.pages[usize::from(i).min(PAGES - 1)],
            DrumButton::Mute(i) => self.mutes[usize::from(i).min(DRUM_LANES - 1)],
            DrumButton::Grid => self.grid,
        }
    }

    /// The rectangle of the cell of `lane` and visible column `col` (0..16).
    pub fn cell_rect(&self, lane: usize, col: usize) -> Rect {
        let x = self.grid.x0 + col as f64 * CELL_W;
        let y = self.grid.y0 + lane as f64 * CELL_H;
        Rect::new(x, y, x + CELL_W, y + CELL_H)
    }

    /// The lane and visible column under a panel point, if it lies on the cells.
    pub fn cell_at(&self, p: vello::kurbo::Point) -> Option<(usize, usize)> {
        if !self.grid.contains(p) {
            return None;
        }
        let col = ((p.x - self.grid.x0) / CELL_W).floor() as usize;
        let lane = ((p.y - self.grid.y0) / CELL_H).floor() as usize;
        (col < VISIBLE_STEPS && lane < DRUM_LANES).then_some((lane, col))
    }
}

/// The panel of `atomik.drum-generator` (four rack units, see the module documentation); the
/// back is the generic socket layout.
pub fn drum_gen_layout(desc: &atomik_device_sdk::DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let slot_w = (RIGHT_W - (SLOTS as f64 - 1.0) * SLOT_GAP) / SLOTS as f64;
    let slots = std::array::from_fn(|i| {
        let x = KNOBS_X + i as f64 * (slot_w + SLOT_GAP);
        Rect::new(x, top + 2.0, x + slot_w, top + 20.0)
    });
    // The name line, with the four bank buttons at its right end.
    let bank_w = 16.0;
    let name = Rect::new(
        KNOBS_X,
        top + 24.0,
        KNOBS_X + RIGHT_W - BANKS as f64 * (bank_w + 2.0),
        top + 38.0,
    );
    let banks = std::array::from_fn(|i| {
        let x = KNOBS_X + RIGHT_W - (BANKS - i) as f64 * (bank_w + 2.0) + 2.0;
        Rect::new(x, top + 24.0, x + bank_w, top + 38.0)
    });
    // The groove row: resolution and shuffle steppers.
    let groove = (top + 42.0, top + 58.0);
    let gh = (RIGHT_W - 8.0) / 2.0;
    let stepper = |x: f64| {
        (
            Rect::new(x, groove.0, x + 16.0, groove.1),
            Rect::new(x + 18.0, groove.0, x + gh - 18.0, groove.1),
            Rect::new(x + gh - 16.0, groove.0, x + gh, groove.1),
        )
    };
    let (res_dec, res_text, res_inc) = stepper(KNOBS_X);
    let (shuf_dec, shuf_text, shuf_inc) = stepper(KNOBS_X + gh + 8.0);
    let grid = Rect::new(
        GRID_X,
        top + 2.0,
        GRID_X + GRID_W,
        top + 2.0 + DRUM_LANES as f64 * CELL_H,
    );
    let labels = std::array::from_fn(|l| {
        let y = grid.y0 + l as f64 * CELL_H;
        Rect::new(x0, y, x0 + LABEL_W, y + CELL_H)
    });
    let mutes = std::array::from_fn(|l| {
        let y = grid.y0 + l as f64 * CELL_H + 1.0;
        Rect::new(
            x0 + LABEL_W + 2.0,
            y,
            x0 + LABEL_W + 2.0 + MUTE_W,
            y + CELL_H - 2.0,
        )
    });
    let page_y = (grid.y1 + 5.0, grid.y1 + 23.0);
    let pages = std::array::from_fn(|i| {
        let x = GRID_X + i as f64 * 34.0;
        Rect::new(x, page_y.0, x + 32.0, page_y.1)
    });
    let len_x = GRID_X + 4.0 * 34.0 + 4.0;
    let len_dec = Rect::new(len_x, page_y.0, len_x + 18.0, page_y.1);
    let len_text = Rect::new(len_x + 20.0, page_y.0, len_x + 60.0, page_y.1);
    let len_inc = Rect::new(len_x + 62.0, page_y.0, len_x + 80.0, page_y.1);
    let row1 = (top + 62.0, top + 80.0);
    let btn_w = (RIGHT_W - 5.0 * SLOT_GAP) / 6.0;
    let mut bx = KNOBS_X;
    let mut small = |_: ()| {
        let r = Rect::new(bx, row1.0, bx + btn_w, row1.1);
        bx += btn_w + SLOT_GAP;
        r
    };
    let (add, duplicate, remove, copy, paste, clear) = (
        small(()),
        small(()),
        small(()),
        small(()),
        small(()),
        small(()),
    );
    let row2 = (top + 84.0, top + 102.0);
    let third = (RIGHT_W - 2.0 * SLOT_GAP) / 3.0;
    let generate = Rect::new(KNOBS_X, row2.0, KNOBS_X + third, row2.1);
    let randomise = Rect::new(
        KNOBS_X + third + SLOT_GAP,
        row2.0,
        KNOBS_X + 2.0 * third + SLOT_GAP,
        row2.1,
    );
    let mode = Rect::new(
        KNOBS_X + 2.0 * (third + SLOT_GAP),
        row2.0,
        KNOBS_X + RIGHT_W,
        row2.1,
    );
    let panel = DrumGenPanel {
        title: Rect::new(x0, MARGIN_TOP, GRID_X + GRID_W, MARGIN_TOP + TITLE_H),
        slots,
        name,
        grid,
        labels,
        mutes,
        pages,
        add,
        duplicate,
        remove,
        copy,
        paste,
        clear,
        generate,
        randomise,
        len_dec,
        len_inc,
        len_text,
        banks,
        res_dec,
        res_text,
        res_inc,
        shuf_dec,
        shuf_text,
        shuf_inc,
        mode,
    };
    let front = place_controls(desc, &DRUM_GEN_PARAMS, KNOBS_X, COL_W, PER_ROW, top + 106.0);
    let front_sections = vec![
        Section {
            label: "STEPS".to_string(),
            rect: panel.title,
        },
        Section {
            label: "PATTERNS".to_string(),
            rect: Rect::new(KNOBS_X, MARGIN_TOP, KNOBS_X + RIGHT_W, MARGIN_TOP + TITLE_H),
        },
    ];
    let (units, back, back_sections) = back_layout(desc, DRUM_GEN_UNITS);
    debug_assert!(f64::from(units) * f64::from(RACK_UNIT) > page_y.1 + MARGIN_BOTTOM);
    debug_assert!(
        f64::from(units) * f64::from(RACK_UNIT)
            > front.iter().map(|c| c.rect.y1).fold(0.0, f64::max) + MARGIN_BOTTOM
    );
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: Some(panel),
        arp: None,
        key_chord: None,
        modsrc: None,
        wavetable: None,
        fm: None,
        granular: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ pure helpers

/// The pages a pattern of `length` steps spans (1 to 4).
pub fn pages_for(length: u8) -> usize {
    usize::from(length.clamp(1, 64)).div_ceil(VISIBLE_STEPS)
}

/// `page` limited to the pages a pattern of `length` steps spans.
pub fn clamp_page(page: usize, length: u8) -> usize {
    page.min(pages_for(length) - 1)
}

/// The label of lane `key`: the pad name for the keys 36 to 51 when `pads` (the 16 names of the
/// drum machine the generator is cabled to) names that pad, else the note name of the key.
pub fn lane_label(key: u8, pads: &[String]) -> String {
    let pad = key
        .checked_sub(36)
        .and_then(|i| pads.get(usize::from(i)))
        .filter(|n| !n.is_empty());
    pad.cloned()
        .unwrap_or_else(|| crate::zone_edit::note_name(key))
}

/// The velocity of a hit dragged `dy` logical pixels upwards from `start` (100 px span the whole
/// range), 1 to 127.
pub fn dragged_velocity(start: u8, dy: f64) -> u8 {
    (f64::from(start) + dy * 1.27).round().clamp(1.0, 127.0) as u8
}

/// The velocity a click on a cell of velocity `now` leaves: none for a hit, [`DEFAULT_VELOCITY`]
/// for an empty step.
pub fn toggled(now: u8) -> u8 {
    if now > 0 { 0 } else { DEFAULT_VELOCITY }
}

/// The pattern `target` after pasting `clip`: the clip's grid, lanes and steps, the name of the
/// slot.
pub fn pasted(target: &DrumPatternDoc, clip: &DrumPatternDoc) -> DrumPatternDoc {
    DrumPatternDoc {
        name: target.name.clone(),
        ..clip.clone()
    }
}

/// `p` with its length changed by `delta` steps (1 to 64).
pub fn resized(p: &DrumPatternDoc, delta: i32) -> DrumPatternDoc {
    DrumPatternDoc {
        length: (i32::from(p.length) + delta).clamp(1, 64) as u8,
        ..p.clone()
    }
}

/// The bank (0-based) holding pattern `index`.
pub fn bank_of(index: usize) -> usize {
    (index / SLOTS).min(BANKS - 1)
}

/// The pattern slot `slot` of the bank that holds `edit` selects.
pub fn slot_pattern(edit: usize, slot: usize) -> usize {
    bank_of(edit) * SLOTS + slot.min(SLOTS - 1)
}

/// The pattern a click on bank button `bank` selects when `edit` is the edited one: the same
/// slot in that bank, or the last pattern when the bank is only partly filled; `None` for a bank
/// with no pattern yet.
pub fn bank_target(edit: usize, bank: usize, count: usize) -> Option<usize> {
    let first = bank * SLOTS;
    (first < count).then(|| (first + edit % SLOTS).min(count - 1))
}

/// `p` with its resolution moved by `delta` entries of [`SeqResolution::ALL`] (clamped).
pub fn stepped_resolution(p: &DrumPatternDoc, delta: i32) -> DrumPatternDoc {
    let i = SeqResolution::ALL
        .iter()
        .position(|r| *r == p.resolution)
        .unwrap_or(3) as i32;
    let i = (i + delta).clamp(0, SeqResolution::ALL.len() as i32 - 1) as usize;
    DrumPatternDoc {
        resolution: SeqResolution::ALL[i],
        ..p.clone()
    }
}

/// `p` with its swing moved by `delta` percent (0 to 75).
pub fn with_swing(p: &DrumPatternDoc, delta: i32) -> DrumPatternDoc {
    DrumPatternDoc {
        swing: (i32::from(p.swing) + delta).clamp(0, 75) as u8,
        ..p.clone()
    }
}

/// What the cells edit.
pub const MODE_LABELS: [&str; 3] = ["VEL", "FLAM", "RATCH"];

/// The flam or ratchet a click leaves on a step that has `now`: the next of the click values.
pub fn clicked_fx(mode: u8, now: u8) -> u8 {
    let list: &[u8] = if mode == 1 {
        &FLAM_CLICKS
    } else {
        &RATCHET_CLICKS
    };
    list.iter().copied().find(|v| *v > now).unwrap_or(0)
}

/// The flam (mode 1, 100 px = the whole range) or ratchet (mode 2, 12 px per hit) of a step
/// dragged `dy` logical pixels upwards from `start`.
pub fn dragged_fx(mode: u8, start: u8, dy: f64) -> u8 {
    if mode == 1 {
        (f64::from(start) + dy * f64::from(MAX_FLAM) / 100.0)
            .round()
            .clamp(0.0, f64::from(MAX_FLAM)) as u8
    } else {
        (f64::from(start) + (dy / 12.0).round()).clamp(0.0, f64::from(MAX_RATCHET)) as u8
    }
}

// ------------------------------------------------------------------ painting

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    device: &atomik_model::RackDevice,
    pattern_param: f32,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    let doc: DrumDoc = device.drums.clone().unwrap_or_default();
    let count = doc.patterns.len();
    let edit = info.edit.min(count.saturating_sub(1));
    let state = play_state(pattern_param, info.meters, count);
    paint_slots(c, theme, panel, count, edit, &state, accent);
    paint_banks(c, theme, panel, count, edit, &state, accent);
    let pattern = doc.patterns.get(edit);
    paint_name(c, theme, panel, pattern, info, accent);
    if let Some(p) = pattern {
        let page = clamp_page(info.page, p.length);
        let cursor = state.cursor.filter(|_| state.playing == edit);
        paint_grid(c, theme, panel, p, page, cursor, info.pad_names, accent);
        paint_pages(c, theme, panel, p, page);
        paint_groove(c, theme, panel, p);
    }
    let one = count <= 1;
    let full = count >= MAX_DRUM_PATTERNS;
    edit_button(c, theme, panel.add, "ADD", !full);
    edit_button(c, theme, panel.duplicate, "DUP", !full);
    edit_button(c, theme, panel.remove, "DEL", !one);
    edit_button(c, theme, panel.copy, "COPY", true);
    edit_button(c, theme, panel.paste, "PASTE", true);
    edit_button(c, theme, panel.clear, "CLEAR", true);
    button_sized(c, theme, panel.generate, "GENERATE", true, 9.0, 1.0);
    button_sized(c, theme, panel.randomise, "RANDOMISE", true, 9.0, 1.0);
    let mode = usize::from(info.fx_mode).min(MODE_LABELS.len() - 1);
    button_sized(c, theme, panel.mode, MODE_LABELS[mode], true, 9.0, 1.0);
    if mode > 0 {
        c.stroke_with(
            RoundedRect::from_rect(panel.mode.inset(-0.5), 3.5),
            1.5,
            theme.highlight,
        );
    }
}

fn paint_slots<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    count: usize,
    edit: usize,
    state: &crate::seq_panel::PlayState,
    accent: Color,
) {
    for (slot, r) in panel.slots.iter().enumerate() {
        let i = slot_pattern(edit, slot);
        let exists = i < count;
        let edited = i == edit;
        let face = if !exists {
            theme.lcd.with_alpha(0.7)
        } else if edited {
            mix(theme.button, accent, 0.4)
        } else {
            theme.button
        };
        c.fill_with(RoundedRect::from_rect(*r, 3.0), face);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 3.5),
            if edited && exists { 2.0 } else { 1.0 },
            if edited && exists {
                accent
            } else {
                theme.bevel_dark
            },
        );
        let col = if exists {
            theme.text
        } else {
            theme.text_muted.with_alpha(0.45)
        };
        c.text(
            &TextRun::new(
                (i + 1).to_string(),
                pt(r.center().x, r.center().y + 3.5),
                9.5,
                col,
            )
            .align(Align::Center)
            .weight(700),
        );
        if exists && i == state.playing {
            c.fill_with(Circle::new(pt(r.x1 - 5.0, r.y0 + 5.0), 2.4), accent);
        }
        if exists && state.queued == Some(i) {
            let mut p = BezPath::new();
            p.move_to(pt(r.x0 + 3.0, r.y0 + 3.0));
            p.line_to(pt(r.x0 + 3.0, r.y0 + 8.0));
            p.line_to(pt(r.x0 + 8.0, r.y0 + 5.5));
            p.close_path();
            c.fill_with(p, theme.highlight);
        }
    }
}

fn paint_name<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    pattern: Option<&DrumPatternDoc>,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    c.fill_with(RoundedRect::from_rect(panel.name, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(panel.name.inset(-0.5), 3.5),
        1.0,
        info.rename.map_or(theme.ink.with_alpha(0.07), |_| accent),
    );
    let (text, col) = match (info.rename, pattern) {
        (Some(t), _) => (format!("{t}|"), accent),
        (None, Some(p)) => (format!("{}  ·  {} steps", p.name, p.length), theme.text),
        (None, None) => (String::new(), theme.text),
    };
    c.text(&TextRun::new(
        fit_label(&text, panel.name.width() - 12.0, 8.5),
        pt(panel.name.x0 + 6.0, panel.name.y0 + 10.0),
        8.5,
        col,
    ));
}

#[allow(clippy::too_many_arguments)]
fn paint_grid<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    p: &DrumPatternDoc,
    page: usize,
    cursor: Option<usize>,
    pads: &[String],
    accent: Color,
) {
    let len = usize::from(p.length);
    c.fill_with(
        RoundedRect::from_rect(panel.grid.inset(2.0), 3.0),
        theme.lcd,
    );
    for lane in 0..DRUM_LANES {
        let l = &p.lanes[lane];
        let dim = if l.mute { 0.4 } else { 1.0 };
        // The label.
        let label = lane_label(l.key, pads);
        c.text(&TextRun::new(
            fit_label(&label, LABEL_W - 4.0, 7.5),
            pt(panel.labels[lane].x0, panel.labels[lane].y0 + 9.5),
            7.5,
            theme.text.with_alpha(0.9 * dim),
        ));
        // The mute square.
        let m = panel.mutes[lane];
        c.fill_with(
            RoundedRect::from_rect(m, 2.0),
            if l.mute {
                mix(theme.button, accent, 0.5)
            } else {
                theme.lcd
            },
        );
        c.stroke_with(
            RoundedRect::from_rect(m.inset(-0.5), 2.5),
            1.0,
            theme.ink.with_alpha(0.12),
        );
        if l.mute {
            c.text(
                &TextRun::new("M", pt(m.center().x, m.center().y + 2.6), 7.0, theme.text)
                    .align(Align::Center)
                    .weight(700),
            );
        }
        // The cells.
        for col in 0..VISIBLE_STEPS {
            let step = page * VISIBLE_STEPS + col;
            let r = panel.cell_rect(lane, col).inset(-0.75);
            let live = step < len;
            let s = p.steps[lane][step];
            let beat = (col / 4) % 2 == 0;
            let base = if beat { 0.07 } else { 0.03 };
            c.fill_with(
                RoundedRect::from_rect(r, 1.5),
                theme.ink.with_alpha(if live { base } else { 0.0 }),
            );
            if cursor == Some(step) {
                c.fill_with(
                    RoundedRect::from_rect(r, 1.5),
                    theme.knob_pointer.with_alpha(0.28),
                );
            }
            if s.velocity > 0 {
                let a = (0.3 + 0.7 * f32::from(s.velocity) / 127.0)
                    * if live { 1.0 } else { 0.25 }
                    * dim;
                c.fill_with(RoundedRect::from_rect(r, 1.5), accent.with_alpha(a));
                paint_cell_fx(c, theme, r, &s, if live { 1.0 } else { 0.25 } * dim);
                if s.prob < 100 {
                    c.fill_with(
                        Rect::new(r.x0 + 2.0, r.y0 + 1.0, r.x1 - 2.0, r.y0 + 2.5),
                        theme.text.with_alpha(0.85),
                    );
                }
            }
        }
    }
}

fn paint_pages<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    p: &DrumPatternDoc,
    page: usize,
) {
    let have = pages_for(p.length);
    for (i, r) in panel.pages.iter().enumerate() {
        let label = format!("{}-{}", i * VISIBLE_STEPS + 1, (i + 1) * VISIBLE_STEPS);
        button_sized(c, theme, *r, &label, i < have, 8.0, 0.5);
        if i == page {
            c.stroke_with(
                RoundedRect::from_rect(r.inset(-0.5), 3.5),
                1.5,
                theme.highlight,
            );
        }
    }
    edit_button(c, theme, panel.len_dec, "-", p.length > 1);
    edit_button(c, theme, panel.len_inc, "+", p.length < 64);
    c.text(
        &TextRun::new(
            format!("{} STEPS", p.length),
            pt(panel.len_text.center().x, panel.len_text.center().y + 3.0),
            8.0,
            theme.text,
        )
        .align(Align::Center),
    );
}

fn paint_banks<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    count: usize,
    edit: usize,
    state: &crate::seq_panel::PlayState,
    accent: Color,
) {
    for (b, r) in panel.banks.iter().enumerate() {
        let exists = b * SLOTS < count;
        let active = b == bank_of(edit);
        let face = if active {
            mix(theme.button, accent, 0.4)
        } else if exists {
            theme.button
        } else {
            theme.lcd.with_alpha(0.7)
        };
        c.fill_with(RoundedRect::from_rect(*r, 3.0), face);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 3.5),
            if active { 2.0 } else { 1.0 },
            if active { accent } else { theme.bevel_dark },
        );
        c.text(
            &TextRun::new(
                char::from(b'A' + b as u8).to_string(),
                pt(r.center().x, r.center().y + 3.2),
                8.5,
                if exists {
                    theme.text
                } else {
                    theme.text_muted.with_alpha(0.45)
                },
            )
            .align(Align::Center)
            .weight(700),
        );
        if exists && bank_of(state.playing) == b && state.playing < count {
            c.fill_with(Circle::new(pt(r.x1 - 3.5, r.y0 + 3.5), 1.8), accent);
        }
    }
}

fn paint_groove<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &DrumGenPanel,
    p: &DrumPatternDoc,
) {
    edit_button(
        c,
        theme,
        panel.res_dec,
        "<",
        p.resolution != SeqResolution::ALL[0],
    );
    edit_button(
        c,
        theme,
        panel.res_inc,
        ">",
        p.resolution != SeqResolution::ALL[SeqResolution::ALL.len() - 1],
    );
    edit_button(c, theme, panel.shuf_dec, "-", p.swing > 0);
    edit_button(c, theme, panel.shuf_inc, "+", p.swing < 75);
    for (r, text) in [
        (
            panel.res_text,
            format!("RES {}", resolution_label(p.resolution)),
        ),
        (panel.shuf_text, format!("SHUF {} %", p.swing)),
    ] {
        c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
        c.text(
            &TextRun::new(text, pt(r.center().x, r.center().y + 3.0), 8.0, theme.text)
                .align(Align::Center),
        );
    }
}

/// Marks a hit's flam (a bar at the left edge, as long as the flam) and ratchet (one tick per
/// hit along the bottom) inside its cell.
fn paint_cell_fx<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    s: &atomik_model::DrumStepDoc,
    alpha: f32,
) {
    if s.flam > 0 {
        let w = (r.width() - 2.0) * f64::from(s.flam) / f64::from(MAX_FLAM);
        c.fill_with(
            Rect::new(r.x0 + 1.0, r.y1 - 3.5, r.x0 + 1.0 + w.max(1.5), r.y1 - 2.0),
            theme.text.with_alpha(0.9 * alpha),
        );
    }
    if s.ratchet > 1 {
        let n = f64::from(s.ratchet.min(MAX_RATCHET));
        for j in 0..s.ratchet.min(MAX_RATCHET) {
            let x = r.x0 + 1.0 + (r.width() - 2.0) * f64::from(j) / n;
            c.fill_with(
                Rect::new(x, r.y1 - 5.0, x + 1.0, r.y1 - 1.5),
                theme.text.with_alpha(0.9 * alpha),
            );
        }
    }
}
