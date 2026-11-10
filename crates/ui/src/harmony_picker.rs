//! The key and chord picker: a small modal panel that sets the song key, a key change or a chord of
//! the chord lane.
//!
//! ```text
//! CHORD AT BAR 5                                                         [ CLOSE ]
//! Chord   [ Am7                               ]
//!         A  C  E  G
//! Root    [C][C#][D][D#][E][F][F#][G][G#][A][A#][B]
//! Quality [Maj][Min][Dim][Aug][sus2][sus4][6] ...
//! Bass    [-][C][C#][D] ...
//! [ APPLY ] [ REMOVE ]   Enter applies, Esc closes
//! ```
//!
//! A key (the song key or a key change) has the same root row and a row of scales instead of the
//! chord rows. The panel shows a value and nothing else: it does not read or change the song.
//! The app opens it for a [`HarmonyTarget`] ([`HarmonyPickerView::new`], asked for by
//! [`UiAction::HarmonyPicker`]) and, when [`UiAction::Harmony`] arrives, builds the new harmony
//! with [`HarmonyPickerView::apply_to`] or [`HarmonyPickerView::remove_from`].
//!
//! The chord symbol field takes `Am7`, `F#m7b5`, `C/G` and the like (see
//! [`atomik_model::chord_symbol`]); the buttons and the field follow each other. A symbol that
//! cannot be read is shown as a hint under the field and `APPLY` waits.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key as UiKey, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_model::chord_symbol::{format_chord, parse_chord};
use atomik_model::harmony::{ChordEvent, Harmony, Key};
use atomik_model::theory::{ChordKind, ScaleKind, pitch_name};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_GAP: f64 = 8.0;
const BTN_H: f64 = 22.0;
const FIELD_H: f64 = 26.0;
const NOTE_H: f64 = 18.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 520.0;
const LABEL_W: f64 = 56.0;
const FONT: f32 = 10.5;
const QUALITY_COLUMNS: usize = 7;
const SCALE_COLUMNS: usize = 5;
const GAP: f64 = 3.0;

/// What the picker sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarmonyTarget {
    /// The song key.
    SongKey,
    /// The key change that starts at this tick.
    KeyChange {
        /// Song tick of the change.
        tick: i64,
    },
    /// The chord that starts at this tick.
    Chord {
        /// Song tick the chord starts at.
        tick: i64,
        /// Its length in ticks.
        length: i64,
    },
}

/// What the picker asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarmonyPickerAction {
    /// Make the picker's value the key or chord.
    Apply,
    /// Take the key, key change or chord away.
    Remove,
    /// Close the picker.
    Close,
}

/// The value being picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerValue {
    /// A key.
    Key(Key),
    /// A chord.
    Chord {
        /// The root pitch class.
        root: u8,
        /// The quality.
        quality: ChordKind,
        /// The bass note when it is not the root.
        bass: Option<u8>,
    },
}

/// The scale kinds a key can have, in menu order.
fn key_scales() -> impl Iterator<Item = ScaleKind> {
    ScaleKind::ALL
        .into_iter()
        .filter(|s| *s != ScaleKind::Chromatic)
}

/// The pitch names of a chord's tones, root first.
fn tones(root: u8, quality: ChordKind, bass: Option<u8>) -> String {
    let mut names: Vec<&str> = quality
        .intervals()
        .iter()
        .map(|i| pitch_name((root + i % 12) % 12))
        .collect();
    names.dedup();
    if let Some(b) = bass {
        let name = pitch_name(b);
        if !names.contains(&name) {
            names.insert(0, name);
        }
    }
    names.join("  ")
}

/// The key and chord picker. See the module documentation.
#[derive(Debug)]
pub struct HarmonyPickerView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    target: HarmonyTarget,
    title: String,
    value: PickerValue,
    /// The item exists already (the panel shows REMOVE).
    existing: bool,
    /// The chord symbol field (a chord target only).
    text: TextField,
    /// Why the symbol cannot be read.
    error: Option<String>,
    dragging: bool,
    hover: Option<Point>,
    repaint: bool,
}

impl HarmonyPickerView {
    /// A picker for `target` showing `value`. `title` names what is picked (the app puts the bar
    /// in it); `existing` says whether the key, key change or chord exists already (REMOVE).
    pub fn new(
        theme: Theme,
        target: HarmonyTarget,
        title: impl Into<String>,
        value: PickerValue,
        existing: bool,
    ) -> Self {
        let text = match value {
            PickerValue::Chord {
                root,
                quality,
                bass,
            } => format_chord(root, quality, bass),
            PickerValue::Key(_) => String::new(),
        };
        let mut field = TextField::new(&text, 24, false);
        field.select_all();
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            target,
            title: title.into(),
            value,
            existing,
            text: field,
            error: None,
            dragging: false,
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// What the picker sets.
    pub fn target(&self) -> HarmonyTarget {
        self.target
    }

    /// The value as it is now.
    pub fn value(&self) -> PickerValue {
        self.value
    }

    /// The hint under the symbol field (why it cannot be read), if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The text of the symbol field.
    pub fn symbol(&self) -> &str {
        self.text.text()
    }

    /// Whether the panel takes typed text: a chord picker has a symbol field.
    pub fn is_text_owner(&self) -> bool {
        self.is_chord()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn is_chord(&self) -> bool {
        matches!(self.value, PickerValue::Chord { .. })
    }

    /// Whether `APPLY` can go ahead (a chord symbol that cannot be read waits).
    pub fn can_apply(&self) -> bool {
        self.error.is_none()
    }

    /// The harmony with the picked key or chord set.
    pub fn apply_to(&self, harmony: &Harmony) -> Result<Harmony, String> {
        match (self.target, self.value) {
            (HarmonyTarget::SongKey, PickerValue::Key(k)) => harmony.with_key(Some(k)),
            (HarmonyTarget::KeyChange { tick }, PickerValue::Key(k)) => {
                harmony.with_key_change(tick, Some(k))
            }
            (
                HarmonyTarget::Chord { tick, length },
                PickerValue::Chord {
                    root,
                    quality,
                    bass,
                },
            ) => {
                let chord = ChordEvent {
                    tick,
                    length,
                    root,
                    quality,
                    bass,
                };
                if harmony.chord_index(tick).is_some() {
                    harmony.with_chord_replaced(tick, chord)
                } else {
                    harmony.with_chord(chord)
                }
            }
            _ => Err("the picker holds the wrong kind of value".to_owned()),
        }
    }

    /// The harmony without the key, key change or chord the picker is on.
    pub fn remove_from(&self, harmony: &Harmony) -> Result<Harmony, String> {
        match self.target {
            HarmonyTarget::SongKey => harmony.with_key(None),
            HarmonyTarget::KeyChange { tick } => harmony.with_key_change(tick, None),
            HarmonyTarget::Chord { tick, .. } => Ok(harmony.without_chord(tick)),
        }
    }

    // ------------------------------------------------------------------ values

    fn set_root(&mut self, pc: u8) {
        match &mut self.value {
            PickerValue::Key(k) => k.root = pc,
            PickerValue::Chord { root, bass, .. } => {
                *root = pc;
                if *bass == Some(pc) {
                    *bass = None;
                }
            }
        }
        self.sync_text();
    }

    fn set_quality(&mut self, quality: ChordKind) {
        if let PickerValue::Chord { quality: q, .. } = &mut self.value {
            *q = quality;
        }
        self.sync_text();
    }

    fn set_bass(&mut self, bass: Option<u8>) {
        if let PickerValue::Chord { root, bass: b, .. } = &mut self.value {
            *b = bass.filter(|x| x != root);
        }
        self.sync_text();
    }

    fn set_scale_kind(&mut self, scale: ScaleKind) {
        if let PickerValue::Key(k) = &mut self.value {
            k.scale = scale;
        }
        self.repaint = true;
    }

    /// Writes the chord of the buttons into the symbol field.
    fn sync_text(&mut self) {
        if let PickerValue::Chord {
            root,
            quality,
            bass,
        } = self.value
        {
            self.text.set_text(&format_chord(root, quality, bass));
            self.error = None;
        }
        self.repaint = true;
    }

    /// Reads the symbol field into the buttons' value.
    fn sync_value(&mut self) {
        match parse_chord(self.text.text()) {
            Ok(c) => {
                self.value = PickerValue::Chord {
                    root: c.root,
                    quality: c.quality,
                    bass: c.bass,
                };
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    fn field_row_h(&self) -> f64 {
        if self.is_chord() {
            FIELD_H + NOTE_H + ROW_GAP
        } else {
            0.0
        }
    }

    fn lower_rows_h(&self) -> f64 {
        let root = BTN_H + ROW_GAP;
        if self.is_chord() {
            let rows = ChordKind::ALL.len().div_ceil(QUALITY_COLUMNS) as f64;
            root + rows * (BTN_H + GAP) - GAP + ROW_GAP + BTN_H + ROW_GAP
        } else {
            let rows = key_scales().count().div_ceil(SCALE_COLUMNS) as f64;
            root + rows * (BTN_H + GAP) - GAP + ROW_GAP
        }
    }

    fn height(&self) -> f64 {
        HEADER_H + ROW_GAP + self.field_row_h() + self.lower_rows_h() + FOOT_H
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + self.height().min(room))
    }

    fn content_x(&self) -> (f64, f64) {
        let p = self.panel();
        (p.x0 + PAD + LABEL_W, p.x1 - PAD)
    }

    fn top(&self) -> f64 {
        self.panel().y0 + HEADER_H + ROW_GAP
    }

    /// The symbol field (a chord picker only).
    pub fn field_rect(&self) -> Rect {
        let (x0, x1) = self.content_x();
        Rect::new(x0, self.top(), x1, self.top() + FIELD_H)
    }

    fn root_top(&self) -> f64 {
        self.top() + self.field_row_h()
    }

    fn grid_button(&self, top: f64, i: usize, columns: usize) -> Rect {
        let (x0, x1) = self.content_x();
        let w = (x1 - x0 - GAP * (columns as f64 - 1.0)) / columns as f64;
        let (row, col) = (i / columns, i % columns);
        let x = x0 + col as f64 * (w + GAP);
        let y = top + row as f64 * (BTN_H + GAP);
        Rect::new(x, y, x + w, y + BTN_H)
    }

    /// The button of pitch class `pc` in the root row.
    pub fn root_rect(&self, pc: u8) -> Rect {
        self.grid_button(self.root_top(), usize::from(pc % 12), 12)
    }

    fn lower_top(&self) -> f64 {
        self.root_top() + BTN_H + ROW_GAP
    }

    /// The button of the chord quality `i` (an index into [`ChordKind::ALL`]).
    pub fn quality_rect(&self, i: usize) -> Rect {
        self.grid_button(self.lower_top(), i, QUALITY_COLUMNS)
    }

    /// The button of scale `i` (an index into the key scales: every kind but the chromatic).
    pub fn scale_rect(&self, i: usize) -> Rect {
        self.grid_button(self.lower_top(), i, SCALE_COLUMNS)
    }

    fn bass_top(&self) -> f64 {
        let rows = ChordKind::ALL.len().div_ceil(QUALITY_COLUMNS) as f64;
        self.lower_top() + rows * (BTN_H + GAP) - GAP + ROW_GAP
    }

    /// The bass button: `None` is the "no bass" button, `Some(pc)` a note.
    pub fn bass_rect(&self, bass: Option<u8>) -> Rect {
        let i = bass.map_or(0, |b| usize::from(b % 12) + 1);
        self.grid_button(self.bass_top(), i, 13)
    }

    fn foot_top(&self) -> f64 {
        let p = self.panel();
        p.y1 - FOOT_H + (FOOT_H - BTN_H) / 2.0
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.foot_top();
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 90.0, y + BTN_H)
    }

    /// The REMOVE button (only shown for a key, key change or chord that exists).
    pub fn remove_rect(&self) -> Rect {
        let a = self.apply_rect();
        Rect::new(a.x1 + 8.0, a.y0, a.x1 + 8.0 + 90.0, a.y1)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the picker without changing anything",
            );
        }
        if self.apply_rect().contains(pos) {
            return help(self.apply_rect(), "Set this (one undo step)");
        }
        if self.existing && self.remove_rect().contains(pos) {
            let text = match self.target {
                HarmonyTarget::SongKey => "Take the song key away (one undo step)",
                HarmonyTarget::KeyChange { .. } => "Delete this key change (one undo step)",
                HarmonyTarget::Chord { .. } => "Delete this chord (one undo step)",
            };
            return help(self.remove_rect(), text);
        }
        if self.is_chord() && self.field_rect().contains(pos) {
            return help(
                self.field_rect(),
                "Type a chord: Am7, F#m7b5, C/G, Bbmaj7, Dsus4",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if self.dragging {
                    self.text.drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging = false,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => self.press(*pos, *clicks, modifiers.shift, &mut out),
            UiEvent::Ime(ime) => {
                if self.is_chord() && self.text.handle_ime(ime) {
                    self.sync_value();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let used = self.is_chord() && self.text.handle_key(key, *modifiers).handled();
                if used {
                    self.sync_value();
                } else if matches!(key, UiKey::Enter) && self.can_apply() {
                    out.push(UiAction::Harmony(HarmonyPickerAction::Apply));
                }
            }
            _ => {}
        }
        out
    }

    fn press(&mut self, pos: Point, clicks: u8, shift: bool, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if self.close_rect().contains(pos) {
            out.push(UiAction::Harmony(HarmonyPickerAction::Close));
        } else if self.apply_rect().contains(pos) {
            if self.can_apply() {
                out.push(UiAction::Harmony(HarmonyPickerAction::Apply));
            }
        } else if self.existing && self.remove_rect().contains(pos) {
            out.push(UiAction::Harmony(HarmonyPickerAction::Remove));
        } else if self.is_chord() && self.field_rect().contains(pos) {
            self.dragging = true;
            self.text.press(pos, clicks, shift);
        } else if let Some(pc) = (0..12u8).find(|pc| self.root_rect(*pc).contains(pos)) {
            self.set_root(pc);
        } else if self.is_chord() {
            if let Some(q) = (0..ChordKind::ALL.len()).find(|i| self.quality_rect(*i).contains(pos))
            {
                self.set_quality(ChordKind::ALL[q]);
            } else if self.bass_rect(None).contains(pos) {
                self.set_bass(None);
            } else if let Some(pc) = (0..12u8).find(|pc| self.bass_rect(Some(*pc)).contains(pos)) {
                self.set_bass(Some(pc));
            }
        } else if let Some(kind) = key_scales()
            .enumerate()
            .find(|(i, _)| self.scale_rect(*i).contains(pos))
            .map(|(_, k)| k)
        {
            self.set_scale_kind(kind);
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            &self.title,
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let label = |c: &mut dyn Canvas, text: &str, y: f64| {
            sp::label(c, text, p.x0 + PAD, y + 15.0, FONT, t.text, Align::Start);
        };
        let lit = |on: bool| on.then_some(t.highlight);
        if let PickerValue::Chord {
            root,
            quality,
            bass,
        } = self.value
        {
            let r = self.field_rect();
            label(c, "Chord", r.y0 + 2.0);
            sp::lcd(c, st, r, Some(t.focus));
            let colors = FieldColors {
                text: t.text,
                caret: t.text,
                selection: mix(t.lcd, t.focus, 0.5),
            };
            self.text.paint(c, r, FONT, 14.0, colors, true);
            let (x0, x1) = self.content_x();
            let (note, color) = match &self.error {
                Some(e) => (e.clone(), st.mute),
                None => (tones(root, quality, bass), t.text_muted),
            };
            sp::label(
                c,
                &fit_label(&note, x1 - x0, 9.5),
                x0,
                r.y1 + 13.0,
                9.5,
                color,
                Align::Start,
            );
            label(c, "Root", self.root_rect(0).y0);
            for pc in 0..12u8 {
                let r = self.root_rect(pc);
                sp::button(c, st, r, pitch_name(pc), lit(pc == root), hovered(r));
            }
            label(c, "Quality", self.quality_rect(0).y0);
            for (i, q) in ChordKind::ALL.into_iter().enumerate() {
                let r = self.quality_rect(i);
                sp::button(c, st, r, q.short(), lit(q == quality), hovered(r));
            }
            label(c, "Bass", self.bass_rect(None).y0);
            let r = self.bass_rect(None);
            sp::button(c, st, r, "-", lit(bass.is_none()), hovered(r));
            for pc in 0..12u8 {
                let r = self.bass_rect(Some(pc));
                sp::button(c, st, r, pitch_name(pc), lit(bass == Some(pc)), hovered(r));
            }
        } else if let PickerValue::Key(key) = self.value {
            label(c, "Root", self.root_rect(0).y0);
            for pc in 0..12u8 {
                let r = self.root_rect(pc);
                sp::button(c, st, r, pitch_name(pc), lit(pc == key.root), hovered(r));
            }
            label(c, "Scale", self.scale_rect(0).y0);
            for (i, kind) in key_scales().enumerate() {
                let r = self.scale_rect(i);
                let text = fit_label(kind.label(), r.width() - 6.0, 9.0);
                sp::button(c, st, r, &text, lit(kind == key.scale), hovered(r));
            }
        }
        let apply = self.apply_rect();
        let apply_lit = self.can_apply().then_some(st.loop_on);
        sp::button(c, st, apply, "APPLY", apply_lit, hovered(apply));
        let mut hint_x = apply.x1 + 12.0;
        if self.existing {
            let remove = self.remove_rect();
            sp::button(c, st, remove, "REMOVE", None, hovered(remove));
            hint_x = remove.x1 + 12.0;
        }
        sp::label(
            c,
            &fit_label("Enter applies, Esc closes", p.x1 - PAD - hint_x, 9.5),
            hint_x,
            apply.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;

    fn click(v: &mut HarmonyPickerView, r: Rect) -> Vec<UiAction> {
        v.handle(&UiEvent::PointerDown {
            pos: r.center(),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        })
    }

    fn chord_picker(text: &str) -> HarmonyPickerView {
        let c = parse_chord(text).unwrap();
        HarmonyPickerView::new(
            Theme::dark(),
            HarmonyTarget::Chord {
                tick: 0,
                length: 15_360,
            },
            "CHORD",
            PickerValue::Chord {
                root: c.root,
                quality: c.quality,
                bass: c.bass,
            },
            true,
        )
    }

    #[test]
    fn the_buttons_write_the_symbol_and_the_symbol_moves_the_buttons() {
        let mut v = chord_picker("C");
        let r = v.root_rect(9);
        click(&mut v, r);
        let r = v.quality_rect(
            ChordKind::ALL
                .iter()
                .position(|k| *k == ChordKind::MinorSeventh)
                .unwrap(),
        );
        click(&mut v, r);
        assert_eq!(v.symbol(), "Am7");
        let r = v.bass_rect(Some(7));
        click(&mut v, r);
        assert_eq!(v.symbol(), "Am7/G");
        let typed = [UiKey::Char('F'), UiKey::Char('#'), UiKey::Char('m')];
        let mut v = chord_picker("C");
        for k in typed {
            v.handle(&UiEvent::Key {
                key: k,
                modifiers: Modifiers::NONE,
                pressed: true,
            });
        }
        assert_eq!(
            v.value(),
            PickerValue::Chord {
                root: 6,
                quality: ChordKind::Minor,
                bass: None
            },
            "the whole text was replaced: {}",
            v.symbol()
        );
    }

    #[test]
    fn a_symbol_that_cannot_be_read_blocks_apply_and_says_why() {
        let mut v = chord_picker("C");
        for k in [UiKey::Char('x'), UiKey::Char('z')] {
            v.handle(&UiEvent::Key {
                key: k,
                modifiers: Modifiers::NONE,
                pressed: true,
            });
        }
        assert!(v.error().is_some(), "{:?}", v.symbol());
        assert!(!v.can_apply());
        let r = v.apply_rect();
        assert!(click(&mut v, r).is_empty());
        let enter = v.handle(&UiEvent::Key {
            key: UiKey::Enter,
            modifiers: Modifiers::NONE,
            pressed: true,
        });
        assert!(enter.is_empty());
    }

    #[test]
    fn apply_and_remove_build_the_new_harmony() {
        let mut v = chord_picker("Am7");
        let r = v.apply_rect();
        let out = click(&mut v, r);
        assert!(matches!(
            out.as_slice(),
            [UiAction::Harmony(HarmonyPickerAction::Apply)]
        ));
        let h = v.apply_to(&Harmony::default()).unwrap();
        assert_eq!(h.chords.len(), 1);
        assert_eq!(h.chords[0].symbol(), "Am7");
        assert_eq!(h.chords[0].length, 15_360);
        let gone = v.remove_from(&h).unwrap();
        assert!(gone.chords.is_empty());
        let r = v.remove_rect();
        let out = click(&mut v, r);
        assert!(matches!(
            out.as_slice(),
            [UiAction::Harmony(HarmonyPickerAction::Remove)]
        ));
    }

    #[test]
    fn a_key_picker_sets_the_song_key_or_a_change() {
        let mut v = HarmonyPickerView::new(
            Theme::dark(),
            HarmonyTarget::SongKey,
            "SONG KEY",
            PickerValue::Key(Key::new(0, ScaleKind::Major)),
            false,
        );
        let r = v.root_rect(9);
        click(&mut v, r);
        let minor = key_scales()
            .position(|k| k == ScaleKind::NaturalMinor)
            .unwrap();
        let r = v.scale_rect(minor);
        click(&mut v, r);
        assert_eq!(
            v.apply_to(&Harmony::default()).unwrap().key,
            Some(Key::new(9, ScaleKind::NaturalMinor))
        );
        assert!(!v.is_text_owner());
        let mut change = HarmonyPickerView::new(
            Theme::dark(),
            HarmonyTarget::KeyChange { tick: 30_720 },
            "KEY CHANGE",
            v.value(),
            false,
        );
        let h = change.apply_to(&Harmony::default()).unwrap();
        assert_eq!(h.changes[0].tick, 30_720);
        let r = change.remove_rect();
        assert!(
            click(&mut change, r).is_empty(),
            "no REMOVE on a key change that is not there"
        );
    }

    #[test]
    fn a_root_that_is_the_bass_drops_the_bass() {
        let mut v = chord_picker("C/G");
        let r = v.root_rect(7);
        click(&mut v, r);
        assert_eq!(v.symbol(), "G");
    }

    #[test]
    fn the_panel_fits_its_bounds_and_every_button_lies_inside_it() {
        let v = chord_picker("C");
        let p = v.panel();
        for pc in 0..12 {
            assert!(p.contains_rect(v.root_rect(pc)));
        }
        for i in 0..ChordKind::ALL.len() {
            assert!(p.contains_rect(v.quality_rect(i)), "{i}");
        }
        assert!(p.contains_rect(v.bass_rect(None)));
        assert!(p.contains_rect(v.bass_rect(Some(11))));
        assert!(p.contains_rect(v.apply_rect()) && p.contains_rect(v.remove_rect()));
    }
}
