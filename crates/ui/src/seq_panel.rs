//! The step sequencer's rack panel: the eight pattern slots, a name
//! line, a compact piano-roll preview of the edited pattern with the playing-step cursor, the
//! pattern buttons (ADD, DUP, DEL, COPY, PASTE, CLEAR, EDIT) and the nine global knobs.
//!
//! ```text
//! PATTERNS                                PLAYER
//! [1][2][3][4][5][6][7][8]                Pattern  Switch  Transpose  Follow  Prob
//! Bass  ·  16 x 1/16  ·  swing 0 %        Seed     Mute    Vel scale  Gate scale
//! +--------------------------------+
//! |  compact piano-roll preview    |
//! +--------------------------------+
//! [ADD][DUP][DEL][COPY][PASTE][CLEAR][EDIT]
//! ```
//!
//! # Meaning of the slots
//!
//! The panel distinguishes the **edited** pattern (view state: the slot with the accent frame;
//! the preview, the name line and COPY / PASTE / CLEAR / DUP / DEL act on it) from the
//! **playing** pattern (the device's `pattern` param, a dot on its slot) and the **queued**
//! pattern (the param points elsewhere while the device finishes the running pattern; a small
//! triangle on its slot). A click on a slot selects the edited pattern; Ctrl/Cmd+click also
//! plays it (`SetParam pattern`); a double-click renames it in place (type, Enter commits,
//! Esc cancels). The device publishes `[playing pattern (0-based), step, running]` as its
//! meter values.
//!
//! This module holds the geometry and the pure helpers (play state, rename editing, paste and
//! clear) shared with the grid editor ([`crate::seq_editor`]); the rack view drives the
//! gestures and `paint_panel` draws the extras.

use crate::autolayout::{
    CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP, MAX_UNITS, PanelSpec, PlacedControl, PlacedSocket, ROW_H,
    SOCKET_CELL, Section, TITLE_H, back_groups, cell_size, choose_widget, fit_with,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::Key;
use crate::paint::{edit_button, fit_label};
use crate::theme::{Theme, mix};
use atomik_device_sdk::{ControlKind, DeviceDescriptor, RACK_UNIT};
use atomik_model::{MAX_NAME_CHARS, MAX_PATTERNS, PatternDoc, SeqResolution, StepDoc};
use vello::kurbo::{BezPath, Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the step sequencer (it has its own panel, [`seq_layout`]).
pub const SEQ_TYPE: &str = "atomik.step-sequencer";
/// Height of the sequencer panel in rack units.
pub const SEQ_UNITS: u8 = 2;
/// Number of pattern slots.
pub const SLOTS: usize = MAX_PATTERNS;
const SLOT_W: f64 = 36.0;
const GAP: f64 = 2.0;
const LEFT_W: f64 = SLOTS as f64 * SLOT_W + (SLOTS as f64 - 1.0) * GAP;
const KNOBS_X: f64 = CONTENT_X.0 + LEFT_W + 8.0;
const COL_W: f64 = 63.0;
const PER_ROW: usize = 5;
/// The sequencer's device params in front layout order (five per row).
pub const SEQ_PARAMS: [&str; 9] = [
    "pattern",
    "pattern.switch",
    "transpose",
    "follow",
    "probability.amount",
    "random.seed",
    "mute",
    "velocity.scale",
    "gate.scale",
];

/// A button or area of the sequencer panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqButton {
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
    /// Open the grid editor.
    Edit,
    /// The preview (a double-click opens the grid editor).
    Preview,
}

/// Geometry of the sequencer panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeqPanel {
    /// Title of the pattern section.
    pub title: Rect,
    /// The eight pattern slots.
    pub slots: [Rect; SLOTS],
    /// The name line of the edited pattern.
    pub name: Rect,
    /// The compact preview.
    pub preview: Rect,
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
    /// EDIT.
    pub edit: Rect,
}

impl SeqPanel {
    /// Every button and the preview, in drawing order.
    pub fn buttons(&self) -> Vec<(SeqButton, Rect)> {
        let mut v: Vec<(SeqButton, Rect)> = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, r)| (SeqButton::Slot(i as u8), *r))
            .collect();
        v.extend([
            (SeqButton::Add, self.add),
            (SeqButton::Duplicate, self.duplicate),
            (SeqButton::Remove, self.remove),
            (SeqButton::Copy, self.copy),
            (SeqButton::Paste, self.paste),
            (SeqButton::Clear, self.clear),
            (SeqButton::Edit, self.edit),
        ]);
        v
    }

    /// The rectangle of a button or of the preview.
    pub fn rect(&self, b: SeqButton) -> Rect {
        match b {
            SeqButton::Slot(i) => self.slots[usize::from(i).min(SLOTS - 1)],
            SeqButton::Add => self.add,
            SeqButton::Duplicate => self.duplicate,
            SeqButton::Remove => self.remove,
            SeqButton::Copy => self.copy,
            SeqButton::Paste => self.paste,
            SeqButton::Clear => self.clear,
            SeqButton::Edit => self.edit,
            SeqButton::Preview => self.preview,
        }
    }
}

/// Places the params `keys` of `desc` that exist as controls: `per_row` columns of `col_w`
/// from `x0`, rows of [`ROW_H`] from `top`. Faders become knobs.
pub(crate) fn place_controls(
    desc: &DeviceDescriptor,
    keys: &[&str],
    x0: f64,
    col_w: f64,
    per_row: usize,
    top: f64,
) -> Vec<PlacedControl> {
    let mut front = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let Some(param) = desc.params.iter().position(|p| p.key == *key) else {
            continue;
        };
        let kind = match choose_widget(&desc.params[param]) {
            ControlKind::Fader => ControlKind::Knob,
            k => k,
        };
        let (w, h) = cell_size(kind);
        let h = h.unwrap_or(ROW_H);
        let cx = x0 + (i % per_row) as f64 * col_w + col_w / 2.0;
        let y0 = top + (i / per_row) as f64 * ROW_H + (ROW_H - h) / 2.0;
        front.push(PlacedControl {
            param,
            kind,
            rect: Rect::new(cx - w / 2.0, y0, cx + w / 2.0, y0 + h),
        });
    }
    front
}

/// The back panel of a player: the generic socket layout at `units` rack units at least.
pub(crate) fn back_layout(
    desc: &DeviceDescriptor,
    min_units: u8,
) -> (u8, Vec<PlacedSocket>, Vec<Section>) {
    let back = back_groups(desc);
    let units = (min_units..=MAX_UNITS)
        .find(|&u| fit_with(&back, u, SOCKET_CELL.1, 0.0, 0.0).is_some())
        .unwrap_or(MAX_UNITS);
    let (cells, sections) = fit_with(&back, units, SOCKET_CELL.1, 0.0, 0.0).unwrap_or_default();
    (
        units,
        cells
            .into_iter()
            .map(|(socket, rect)| PlacedSocket { socket, rect })
            .collect(),
        sections,
    )
}

/// The panel of `atomik.step-sequencer` (two rack units, see the module documentation); the
/// back is the generic socket layout.
pub fn seq_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let slot_y = (top + 2.0, top + 22.0);
    let slots = std::array::from_fn(|i| {
        let x = x0 + i as f64 * (SLOT_W + GAP);
        Rect::new(x, slot_y.0, x + SLOT_W, slot_y.1)
    });
    let right = x0 + LEFT_W;
    let name = Rect::new(x0, top + 26.0, right, top + 42.0);
    let preview = Rect::new(x0, top + 46.0, right, top + 94.0);
    let by = (top + 99.0, top + 119.0);
    let mut x = x0;
    let mut button = |w: f64| {
        let r = Rect::new(x, by.0, x + w, by.1);
        x += w + GAP;
        r
    };
    let panel = SeqPanel {
        title: Rect::new(x0, MARGIN_TOP, right, MARGIN_TOP + TITLE_H),
        slots,
        name,
        preview,
        add: button(38.0),
        duplicate: button(38.0),
        remove: button(38.0),
        copy: button(42.0),
        paste: button(46.0),
        clear: button(46.0),
        edit: button(42.0),
    };
    let front = place_controls(desc, &SEQ_PARAMS, KNOBS_X, COL_W, PER_ROW, top);
    let front_sections = vec![
        Section {
            label: "PATTERNS".to_string(),
            rect: panel.title,
        },
        Section {
            label: "PLAYER".to_string(),
            rect: Rect::new(
                KNOBS_X,
                MARGIN_TOP,
                KNOBS_X + PER_ROW as f64 * COL_W,
                MARGIN_TOP + TITLE_H,
            ),
        },
    ];
    let (units, back, back_sections) = back_layout(desc, SEQ_UNITS);
    debug_assert!(f64::from(units) * f64::from(RACK_UNIT) > by.1 + MARGIN_BOTTOM);
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: Some(panel),
        arp: None,
        modsrc: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

/// Control label of a player param.
pub fn player_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "pattern" => "Pattern",
        "pattern.switch" => "Switch",
        "transpose" => "Transpose",
        "follow" => "Follow",
        "probability.amount" => "Prob",
        "random.seed" => "Seed",
        "mute" => "Mute",
        "velocity.scale" => "Vel scale",
        "gate.scale" => "Gate scale",
        "gate" => "Gate",
        "swing" => "Swing",
        "hold" => "Hold",
        "run" => "Run",
        "velocity.mode" => "Vel",
        "velocity.fixed" => "Fixed",
        _ => return None,
    })
}

// ------------------------------------------------------------------ pure helpers

/// The label of a resolution (`1/4` ... `1/32T`).
pub fn resolution_label(r: SeqResolution) -> &'static str {
    match r {
        SeqResolution::Quarter => "1/4",
        SeqResolution::Eighth => "1/8",
        SeqResolution::EighthTriplet => "1/8T",
        SeqResolution::Sixteenth => "1/16",
        SeqResolution::SixteenthTriplet => "1/16T",
        SeqResolution::ThirtySecond => "1/32",
        SeqResolution::ThirtySecondTriplet => "1/32T",
    }
}

/// What the device publishes, turned into what the panel and the editor draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayState {
    /// The playing pattern, 0-based.
    pub playing: usize,
    /// The pattern the `pattern` param points to while the running one has not ended.
    pub queued: Option<usize>,
    /// The step that started last, while the transport runs.
    pub cursor: Option<usize>,
}

/// The play state from the `pattern` param (1-based plain value), the published meter values
/// `[playing pattern (0-based), step, running]` and the pattern count. Without a feed, or
/// while the device is stopped (it then adopts the param at once), the param is the playing
/// pattern and nothing is queued.
pub fn play_state(param: f32, meters: &[f32], count: usize) -> PlayState {
    let last = count.max(1) - 1;
    let want = if param.is_finite() && param >= 1.0 {
        (param.round() as usize - 1).min(last)
    } else {
        0
    };
    let whole = |v: f32| (v.is_finite() && v >= 0.0).then(|| v.round() as usize);
    match meters {
        [pattern, step, running, ..] if *running > 0.5 => {
            let playing = whole(*pattern).map_or(want, |p| p.min(last));
            PlayState {
                playing,
                queued: (playing != want).then_some(want),
                cursor: whole(*step),
            }
        }
        _ => PlayState {
            playing: want,
            queued: None,
            cursor: None,
        },
    }
}

/// The lowest and highest key of the steps that play (`on`, inside the length), if any.
pub fn key_span(p: &PatternDoc) -> Option<(u8, u8)> {
    let on = p.steps.iter().take(usize::from(p.length)).filter(|s| s.on);
    on.fold(None, |m, s| {
        Some(m.map_or((s.key, s.key), |(lo, hi)| (lo.min(s.key), hi.max(s.key))))
    })
}

/// The result of a key press while a pattern name is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameKey {
    /// The text changed or the key was ignored; keep editing.
    Editing,
    /// Enter: commit.
    Commit,
    /// Escape: cancel.
    Cancel,
}

/// Applies one key to the name being edited (at most [`MAX_NAME_CHARS`] characters).
pub fn rename_key(text: &mut String, key: &Key) -> RenameKey {
    match key {
        Key::Enter => return RenameKey::Commit,
        Key::Escape => return RenameKey::Cancel,
        Key::Backspace => {
            text.pop();
        }
        Key::Space => push_char(text, ' '),
        Key::Char(c) if !c.is_control() => push_char(text, *c),
        _ => {}
    }
    RenameKey::Editing
}

fn push_char(text: &mut String, c: char) {
    if text.chars().count() < MAX_NAME_CHARS {
        text.push(c);
    }
}

/// `p` with its name replaced by the edited `text`; `None` when the name is empty after
/// trimming or unchanged.
pub fn renamed(p: &PatternDoc, text: &str) -> Option<PatternDoc> {
    let name = text.trim();
    (!name.is_empty() && name != p.name).then(|| PatternDoc {
        name: name.to_string(),
        ..p.clone()
    })
}

/// The pattern `target` after pasting `clip`: the clip's length, resolution, swing and steps,
/// the name of the slot.
pub fn pasted(target: &PatternDoc, clip: &PatternDoc) -> PatternDoc {
    PatternDoc {
        name: target.name.clone(),
        ..clip.clone()
    }
}

/// The pattern `p` with every step reset (length, resolution, swing and name stay).
pub fn cleared(p: &PatternDoc) -> PatternDoc {
    PatternDoc {
        steps: vec![StepDoc::default(); p.steps.len()],
        ..p.clone()
    }
}

// ------------------------------------------------------------------ painting

/// What the panel draws besides the document.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInfo<'a> {
    /// The edited pattern (sequencer panel).
    pub edit: usize,
    /// The published meter values (quantised, see [`crate::RackView::set_device_meter_values`]).
    pub meters: &'a [f32],
    /// The Mod Source scope history (Mod Source panel).
    pub scope: Option<&'a crate::modsrc_panel::Scope>,
    /// The pattern name being edited, if any.
    pub rename: Option<&'a str>,
}

/// Draws the sequencer panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &SeqPanel,
    device: &atomik_model::RackDevice,
    pattern_param: f32,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    let doc = device.steps.clone().unwrap_or_default();
    let count = doc.patterns.len();
    let edit = info.edit.min(count.saturating_sub(1));
    let state = play_state(pattern_param, info.meters, count);
    // Slots.
    for (i, r) in panel.slots.iter().enumerate() {
        let exists = i < count;
        let edited = i == edit;
        let face = if !exists {
            theme.lcd.with_alpha(0.7)
        } else if edited {
            mix(theme.button, accent, 0.4)
        } else {
            theme.button
        };
        let rr = RoundedRect::from_rect(*r, 3.0);
        c.fill_with(rr, face);
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
            c.fill_with(Circle::new(pt(r.x1 - 6.0, r.y0 + 6.0), 2.6), accent);
        }
        if exists && state.queued == Some(i) {
            let mut p = BezPath::new();
            p.move_to(pt(r.x0 + 4.0, r.y0 + 3.0));
            p.line_to(pt(r.x0 + 4.0, r.y0 + 9.0));
            p.line_to(pt(r.x0 + 9.0, r.y0 + 6.0));
            p.close_path();
            c.fill_with(p, theme.highlight);
        }
    }
    // Name line.
    c.fill_with(RoundedRect::from_rect(panel.name, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(panel.name.inset(-0.5), 3.5),
        1.0,
        info.rename
            .map_or(Color::WHITE.with_alpha(0.07), |_| accent),
    );
    let pattern = doc.patterns.get(edit);
    let (text, col) = match (info.rename, pattern) {
        (Some(t), _) => (format!("{t}|"), accent),
        (None, Some(p)) => (
            format!(
                "{}  ·  {} x {}  ·  swing {} %",
                p.name,
                p.length,
                resolution_label(p.resolution),
                p.swing
            ),
            theme.text,
        ),
        (None, None) => (String::new(), theme.text),
    };
    c.text(&TextRun::new(
        fit_label(&text, panel.name.width() - 12.0, 9.0),
        pt(panel.name.x0 + 6.0, panel.name.y0 + 11.5),
        9.0,
        col,
    ));
    // Preview.
    if let Some(p) = pattern {
        paint_preview(
            c,
            theme,
            panel.preview,
            p,
            state.cursor.filter(|_| state.playing == edit),
            accent,
        );
    }
    // Buttons.
    let one = count <= 1;
    let full = count >= MAX_PATTERNS;
    edit_button(c, theme, panel.add, "ADD", !full);
    edit_button(c, theme, panel.duplicate, "DUP", !full);
    edit_button(c, theme, panel.remove, "DEL", !one);
    edit_button(c, theme, panel.copy, "COPY", true);
    edit_button(c, theme, panel.paste, "PASTE", true);
    edit_button(c, theme, panel.clear, "CLEAR", true);
    edit_button(c, theme, panel.edit, "EDIT", true);
}

/// The compact piano-roll preview of a pattern: one column per step, a bar at the key's height
/// (width = gate, brightness = probability), tie bridges, beat lines and the cursor.
pub(crate) fn paint_preview<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    p: &PatternDoc,
    cursor: Option<usize>,
    accent: Color,
) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let inner = r.inset(-4.0);
    let n = usize::from(p.length.max(1));
    let cw = inner.width() / n as f64;
    let (lo, hi) = key_span(p).map_or((55, 67), |(lo, hi)| {
        // At least an octave of room so a single note sits in the middle.
        let mid = (u16::from(lo) + u16::from(hi)) / 2;
        let half = ((u16::from(hi) - u16::from(lo)) / 2).max(6);
        (mid.saturating_sub(half) as u8, (mid + half).min(127) as u8)
    });
    let span = f64::from(hi - lo).max(1.0);
    let y_of = |key: u8| {
        let f = (f64::from(key.clamp(lo, hi) - lo)) / span;
        inner.y1 - 3.0 - f * (inner.height() - 6.0)
    };
    let per_beat = beat_steps(p.resolution);
    for k in 0..n {
        let x = inner.x0 + k as f64 * cw;
        if k % per_beat == 0 {
            c.stroke_with(
                vello::kurbo::Line::new(
                    pt(x.round() + 0.5, inner.y0),
                    pt(x.round() + 0.5, inner.y1),
                ),
                1.0,
                Color::WHITE.with_alpha(if k == 0 { 0.16 } else { 0.07 }),
            );
        }
    }
    if let Some(k) = cursor.filter(|k| *k < n) {
        let x = inner.x0 + k as f64 * cw;
        c.fill_with(
            Rect::new(x, inner.y0, x + cw, inner.y1),
            theme.knob_pointer.with_alpha(0.25),
        );
    }
    for (k, s) in p.steps.iter().take(n).enumerate() {
        if !s.on {
            continue;
        }
        let x = inner.x0 + k as f64 * cw;
        let y = y_of(s.key);
        let alpha = 0.3 + 0.7 * f64::from(s.prob) / 100.0;
        let w = (cw * f64::from(s.gate) / 100.0).max(1.5) - 0.5;
        let h = 2.0 + 3.0 * f64::from(s.velocity) / 127.0;
        c.fill_with(
            Rect::new(x + 0.5, y - h / 2.0, x + 0.5 + w.max(1.0), y + h / 2.0),
            accent.with_alpha(alpha as f32),
        );
        if s.tie && k + 1 < n {
            c.fill_with(
                Rect::new(x + 0.5 + w, y - 0.75, x + cw + 0.5, y + 0.75),
                accent.with_alpha(alpha as f32),
            );
        }
    }
}

/// Steps per quarter note, at least 1 (beat lines of the preview).
fn beat_steps(r: SeqResolution) -> usize {
    (15_360 / r.step_ticks() as usize).max(1)
}
