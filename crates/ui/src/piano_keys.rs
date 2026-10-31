//! The Piano Keys strip: an on-screen keyboard and the computer keyboard as a note input.
//!
//! The strip shows two octaves and a note (25 keys, C to C) above a row of controls. It plays
//! live notes: every note it starts is a [`UiAction::KeysNote`], and every note on is followed
//! by a matching off (when the strip loses the focus, is closed or the mode changes, the
//! notes still sounding are released). The notes are live input: they are not part of the
//! project, and the velocity variation uses a UI-side generator that never reaches a render.
//!
//! * **Mouse** mode: a press plays the key with a velocity that follows the height of the
//!   click (low on the key is loud); dragging across keys glides from one to the next.
//!   **Hold** latches the keys (click again to release them) and **Sustain** keeps released
//!   keys sounding until it is switched off.
//! * **Keys** mode: the computer keyboard plays. Two rows play the keys of the strip, a piano
//!   layout of 18 keys from the strip's lowest C (the keymap decides which character is which
//!   key, see the `keys.note` commands), `Z` and `X` shift the strip by an octave, `1` to `0`
//!   set the velocity (1, 14, 28, 42, 56, 70, 84, 98, 112, 127; 98 on start) and `Shift` is the
//!   sustain pedal. **Var** adds a small random change to each velocity.
//!
//! The strip covers ten octaves (the lowest key of the strip is `12 * octave`, octave 0 to 9,
//! shown as the name of that C: `C-1` to `C8`); keys above 127 do not play.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::seq_paint::SeqTheme;
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use crate::zone_edit::note_name;
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the strip in logical pixels.
pub const PIANO_KEYS_HEIGHT: f64 = 112.0;
/// The keys the computer keyboard plays: the offsets 0 to 17 above the strip's lowest C.
pub const TYPED_KEYS: u8 = 18;
/// The lowest and highest octave of the strip.
pub const OCTAVES: (u8, u8) = (0, 9);
/// The octave the strip starts on (its lowest key is C4, MIDI 60).
pub const DEFAULT_OCTAVE: u8 = 5;
/// The velocity the typed keys start with.
pub const DEFAULT_VELOCITY: u8 = 98;
/// The velocities of the number keys `1` to `0`.
pub const VELOCITY_STEPS: [u8; 10] = [1, 14, 28, 42, 56, 70, 84, 98, 112, 127];
/// Keys on the strip: two octaves and the next C.
const KEY_COUNT: u8 = 25;
/// The most a velocity moves when the variation is on.
const VARIATION: i32 = 12;
const TOOLBAR_H: f64 = 28.0;
const WHITE_KEYS: usize = 15;
const GAP: f64 = 6.0;
/// The seed of the variation generator (any odd constant).
const SEED: u32 = 0x9E37_79B9;

/// How the strip is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysMode {
    /// The pointer plays; the computer keys keep their commands.
    Mouse,
    /// The computer keys play (while the strip has the focus).
    Computer,
}

/// A control of the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysButton {
    /// Switches between [`KeysMode::Mouse`] and [`KeysMode::Computer`].
    Mode,
    /// One octave down.
    OctaveDown,
    /// One octave up.
    OctaveUp,
    /// The velocity down by a step of the number keys.
    VelocityDown,
    /// The velocity up by a step of the number keys.
    VelocityUp,
    /// The sustain pedal.
    Sustain,
    /// Latch the keys pressed with the pointer.
    Hold,
    /// Random change of the typed velocities.
    Variation,
}

const BUTTONS: [KeysButton; 8] = [
    KeysButton::Mode,
    KeysButton::OctaveDown,
    KeysButton::OctaveUp,
    KeysButton::VelocityDown,
    KeysButton::VelocityUp,
    KeysButton::Sustain,
    KeysButton::Hold,
    KeysButton::Variation,
];

impl KeysButton {
    fn width(self) -> f64 {
        match self {
            KeysButton::Mode => 58.0,
            KeysButton::OctaveDown
            | KeysButton::OctaveUp
            | KeysButton::VelocityDown
            | KeysButton::VelocityUp => 22.0,
            KeysButton::Sustain => 62.0,
            KeysButton::Hold | KeysButton::Variation => 44.0,
        }
    }

    /// The space after the button: the readouts sit between the arrows.
    fn after(self) -> f64 {
        match self {
            KeysButton::OctaveDown | KeysButton::VelocityDown => 44.0,
            _ => GAP,
        }
    }
}

/// The strip's view state.
#[derive(Debug, Clone)]
pub struct PianoKeysView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    focused: bool,
    mode: KeysMode,
    octave: u8,
    velocity: u8,
    sustain: bool,
    shift: bool,
    hold: bool,
    variation: bool,
    rng: u32,
    /// The key the pointer holds, with the pointer gliding over the keys.
    pressed: Option<u8>,
    /// The keys latched by Hold.
    latched: BTreeSet<u8>,
    /// The computer keys held and the note each one started.
    typed: Vec<(Key, u8)>,
    /// Keys released while sustained, still sounding.
    sustained: BTreeSet<u8>,
    /// Every key that has had a note on and no note off.
    sounding: BTreeSet<u8>,
    /// What each of the 18 typed keys is called (the keymap's characters).
    labels: Vec<String>,
}

impl PianoKeysView {
    /// Switches to `theme` (the next paint uses it).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
    }

    /// A strip in Keys mode on [`DEFAULT_OCTAVE`] at [`DEFAULT_VELOCITY`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, PIANO_KEYS_HEIGHT),
            scale: 1.0,
            focused: false,
            mode: KeysMode::Computer,
            octave: DEFAULT_OCTAVE,
            velocity: DEFAULT_VELOCITY,
            sustain: false,
            shift: false,
            hold: false,
            variation: false,
            rng: SEED,
            pressed: None,
            latched: BTreeSet::new(),
            typed: Vec::new(),
            sustained: BTreeSet::new(),
            sounding: BTreeSet::new(),
            labels: Vec::new(),
        }
    }

    // ------------------------------------------------------------------ host API

    /// Where the strip is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    /// Display scale factor (the strip paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
    }

    /// Where the strip is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Whether the strip has the keyboard focus. Losing it releases every note still sounding
    /// (the returned note offs) and forgets the held keys: their releases will go elsewhere.
    pub fn set_focused(&mut self, focused: bool) -> Vec<UiAction> {
        if self.focused == focused {
            return Vec::new();
        }
        self.focused = focused;
        if focused {
            return Vec::new();
        }
        self.shift = false;
        self.release_all()
    }

    /// Whether the strip has the keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// The characters of the typed keys, as the keymap binds them (`labels[0]` plays the
    /// strip's lowest C): drawn on the keys in Keys mode.
    pub fn set_labels(&mut self, labels: Vec<String>) {
        self.labels = labels;
    }

    /// How the strip is played.
    pub fn mode(&self) -> KeysMode {
        self.mode
    }

    /// Whether the computer keys are notes now: Keys mode with the focus.
    pub fn is_typing(&self) -> bool {
        self.focused && self.mode == KeysMode::Computer
    }

    /// Switches the mode; the notes of the mode that ends are released.
    pub fn set_mode(&mut self, mode: KeysMode) -> Vec<UiAction> {
        if self.mode == mode {
            return Vec::new();
        }
        let off = self.release_all();
        self.mode = mode;
        off
    }

    /// The octave: the strip's lowest key is `12 * octave`.
    pub fn octave(&self) -> u8 {
        self.octave
    }

    /// The name of the strip's lowest key (`C4`).
    pub fn octave_name(&self) -> String {
        note_name(self.low())
    }

    /// Moves the strip by `by` octaves within the range. The notes sounding stay as they are.
    /// Returns whether it moved.
    pub fn shift_octave(&mut self, by: i8) -> bool {
        let next = (i16::from(self.octave) + i16::from(by))
            .clamp(i16::from(OCTAVES.0), i16::from(OCTAVES.1)) as u8;
        let moved = next != self.octave;
        self.octave = next;
        moved
    }

    /// The velocity of the typed keys.
    pub fn velocity(&self) -> u8 {
        self.velocity
    }

    /// Sets the velocity of the typed keys (1 to 127).
    pub fn set_velocity(&mut self, velocity: u8) {
        self.velocity = velocity.clamp(1, 127);
    }

    /// The velocity of number key `step` (1 to 10: the keys `1` to `0`); out of range does
    /// nothing.
    pub fn set_velocity_step(&mut self, step: usize) {
        if let Some(v) = step.checked_sub(1).and_then(|i| VELOCITY_STEPS.get(i)) {
            self.velocity = *v;
        }
    }

    /// The velocity one step of the number keys above or below the current one.
    pub fn step_velocity(&mut self, up: bool) {
        let i = VELOCITY_STEPS.iter().rposition(|v| *v <= self.velocity);
        let next = match (up, i) {
            (true, Some(i)) => i + 1,
            (true, None) => 0,
            (false, Some(i)) if VELOCITY_STEPS[i] == self.velocity => i.saturating_sub(1),
            (false, Some(i)) => i,
            (false, None) => 0,
        };
        self.velocity = VELOCITY_STEPS[next.min(VELOCITY_STEPS.len() - 1)];
    }

    /// Whether the velocity of the typed keys varies a little from one note to the next.
    pub fn variation(&self) -> bool {
        self.variation
    }

    /// Switches the velocity variation.
    pub fn set_variation(&mut self, on: bool) {
        self.variation = on;
    }

    /// Whether the sustain pedal is down: the button or the Shift key (while typing).
    pub fn sustaining(&self) -> bool {
        self.sustain || (self.shift && self.is_typing())
    }

    /// Whether Hold is on.
    pub fn holding(&self) -> bool {
        self.hold
    }

    /// The keys with a note on, ascending.
    pub fn sounding(&self) -> Vec<u8> {
        self.sounding.iter().copied().collect()
    }

    /// The lowest key of the strip.
    pub fn low(&self) -> u8 {
        self.octave.saturating_mul(12)
    }

    // ------------------------------------------------------------------ typing

    /// A computer key went down and plays the key `offset` semitones above the strip's lowest
    /// C. A key already held (auto-repeat) is ignored, and so is one above MIDI key 127.
    pub fn type_note(&mut self, key: &Key, offset: u8) -> Vec<UiAction> {
        let key = fold(key);
        if self.typed.iter().any(|(k, _)| *k == key) {
            return Vec::new();
        }
        let Some(note) = self
            .low()
            .checked_add(offset)
            .filter(|n| *n <= 127 && offset < TYPED_KEYS)
        else {
            return Vec::new();
        };
        self.typed.push((key, note));
        let velocity = self.typed_velocity();
        self.note_on(note, velocity)
    }

    /// A computer key came up: the note it started ends (unless the sustain pedal holds it).
    pub fn type_release(&mut self, key: &Key) -> Vec<UiAction> {
        let key = fold(key);
        let Some(i) = self.typed.iter().position(|(k, _)| *k == key) else {
            return Vec::new();
        };
        let (_, note) = self.typed.remove(i);
        self.let_go(note)
    }

    /// The Shift key went down or up (the sustain pedal of the typed keys).
    pub fn set_shift(&mut self, down: bool) -> Vec<UiAction> {
        let before = self.sustaining();
        self.shift = down;
        if before && !self.sustaining() {
            return self.drop_sustained();
        }
        Vec::new()
    }

    /// Releases every note: the held keys, the latched keys and the sustained ones.
    pub fn release_all(&mut self) -> Vec<UiAction> {
        self.pressed = None;
        self.latched.clear();
        self.typed.clear();
        self.sustained.clear();
        self.sustain = false;
        std::mem::take(&mut self.sounding)
            .into_iter()
            .map(|key| UiAction::KeysNote {
                key,
                velocity: 0,
                on: false,
            })
            .collect()
    }

    fn typed_velocity(&mut self) -> u8 {
        if !self.variation {
            return self.velocity;
        }
        // xorshift32: a UI-side generator, live input only.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        let delta = (x % (2 * VARIATION as u32 + 1)) as i32 - VARIATION;
        (i32::from(self.velocity) + delta).clamp(1, 127) as u8
    }

    // ------------------------------------------------------------------ notes

    fn held(&self, key: u8) -> bool {
        self.pressed == Some(key)
            || self.latched.contains(&key)
            || self.typed.iter().any(|(_, n)| *n == key)
    }

    fn note_on(&mut self, key: u8, velocity: u8) -> Vec<UiAction> {
        let mut out = Vec::new();
        // A key struck again while it sounds (sustained) is struck again.
        if self.sounding.contains(&key) {
            out.push(UiAction::KeysNote {
                key,
                velocity: 0,
                on: false,
            });
        }
        self.sustained.remove(&key);
        self.sounding.insert(key);
        out.push(UiAction::KeysNote {
            key,
            velocity: velocity.clamp(1, 127),
            on: true,
        });
        out
    }

    /// A holder of `key` let go: the note ends, or is sustained, unless something else holds it.
    fn let_go(&mut self, key: u8) -> Vec<UiAction> {
        if self.held(key) || !self.sounding.contains(&key) {
            return Vec::new();
        }
        if self.sustaining() {
            self.sustained.insert(key);
            return Vec::new();
        }
        self.sounding.remove(&key);
        vec![UiAction::KeysNote {
            key,
            velocity: 0,
            on: false,
        }]
    }

    fn drop_sustained(&mut self) -> Vec<UiAction> {
        let keys: Vec<u8> = std::mem::take(&mut self.sustained).into_iter().collect();
        let mut out = Vec::new();
        for key in keys {
            if !self.held(key) && self.sounding.remove(&key) {
                out.push(UiAction::KeysNote {
                    key,
                    velocity: 0,
                    on: false,
                });
            }
        }
        out
    }

    // ------------------------------------------------------------------ geometry

    fn button_rect(&self, which: KeysButton) -> Rect {
        let mut x = self.bounds.x0 + GAP + 2.0;
        for b in BUTTONS {
            if b == which {
                return Rect::new(
                    x,
                    self.bounds.y0 + 4.0,
                    x + b.width(),
                    self.bounds.y0 + TOOLBAR_H - 3.0,
                );
            }
            x += b.width() + b.after();
        }
        Rect::ZERO
    }

    fn button_at(&self, pos: Point) -> Option<KeysButton> {
        BUTTONS
            .into_iter()
            .find(|b| self.button_rect(*b).contains(pos))
    }

    /// The area of the keys, centred in the strip.
    fn keys_area(&self) -> Rect {
        let top = self.bounds.y0 + TOOLBAR_H + 2.0;
        let avail = (self.bounds.width() - 2.0 * GAP).max(WHITE_KEYS as f64);
        let white = (avail / WHITE_KEYS as f64).clamp(8.0, 52.0);
        let width = white * WHITE_KEYS as f64;
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        Rect::new(x0, top, x0 + width, self.bounds.y1 - 4.0)
    }

    /// The strip's key `i` (0 to 24) as a rectangle, and whether it is a black key.
    fn key_rect(&self, i: u8) -> (Rect, bool) {
        let area = self.keys_area();
        let white_w = area.width() / WHITE_KEYS as f64;
        let octave = f64::from(i / 12);
        let semitone = i % 12;
        // White keys before this one in the octave: C D E F G A B.
        const WHITE_BEFORE: [u8; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
        let black = matches!(semitone, 1 | 3 | 6 | 8 | 10);
        let index = octave * 7.0 + f64::from(WHITE_BEFORE[usize::from(semitone)]);
        if black {
            let w = white_w * 0.6;
            let x = area.x0 + (index + 1.0) * white_w - w / 2.0;
            (
                Rect::new(x, area.y0, x + w, area.y0 + area.height() * 0.62),
                true,
            )
        } else {
            let x = area.x0 + index * white_w;
            (Rect::new(x, area.y0, x + white_w, area.y1), false)
        }
    }

    /// The key (MIDI) under `pos` and the click height from 0 (top) to 1 (bottom).
    fn key_at(&self, pos: Point) -> Option<(u8, f64)> {
        let area = self.keys_area();
        if !area.contains(pos) {
            return None;
        }
        let blacks = (0..KEY_COUNT).filter(|i| self.key_rect(*i).1);
        let hit = blacks
            .clone()
            .find(|i| self.key_rect(*i).0.contains(pos))
            .or_else(|| {
                (0..KEY_COUNT).find(|i| {
                    let (r, black) = self.key_rect(*i);
                    !black && r.contains(pos)
                })
            })?;
        let height = ((pos.y - area.y0) / area.height().max(1.0)).clamp(0.0, 1.0);
        let key = u16::from(self.low()) + u16::from(hit);
        (key <= 127).then_some((key as u8, height))
    }

    // ------------------------------------------------------------------ events

    /// The controls of the strip at `pos`, for a tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let b = self.button_at(pos)?;
        let text = match b {
            KeysButton::Mode => {
                "Who plays: the computer keyboard (while this strip has the focus) or only the pointer"
            }
            KeysButton::OctaveDown => "Octave down",
            KeysButton::OctaveUp => "Octave up",
            KeysButton::VelocityDown => "Velocity of the typed keys down one step",
            KeysButton::VelocityUp => "Velocity of the typed keys up one step",
            KeysButton::Sustain => "Keep released keys sounding (Shift while typing)",
            KeysButton::Hold => "Latch the keys pressed with the pointer; press again to release",
            KeysButton::Variation => "Change each typed velocity a little at random",
        };
        Some(Help::new(self.button_rect(b), text))
    }

    /// Handles a pointer event (window logical coordinates). Keys of the computer keyboard are
    /// the shell's: it resolves them through the keymap and calls [`Self::type_note`].
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => self.press(*pos),
            UiEvent::PointerMove { pos, .. } if self.pressed.is_some() => self.glide(*pos),
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.lift(),
            _ => Vec::new(),
        }
    }

    fn press(&mut self, pos: Point) -> Vec<UiAction> {
        if let Some(b) = self.button_at(pos) {
            return self.push_button(b);
        }
        let Some((key, height)) = self.key_at(pos) else {
            return Vec::new();
        };
        if self.hold && self.latched.contains(&key) {
            self.latched.remove(&key);
            return self.let_go(key);
        }
        self.pressed = Some(key);
        if self.hold {
            self.latched.insert(key);
        }
        self.note_on(key, height_velocity(height))
    }

    fn glide(&mut self, pos: Point) -> Vec<UiAction> {
        let Some((key, height)) = self.key_at(pos) else {
            return Vec::new();
        };
        let Some(old) = self.pressed else {
            return Vec::new();
        };
        if old == key {
            return Vec::new();
        }
        self.pressed = Some(key);
        let mut out = if self.hold {
            Vec::new()
        } else {
            self.let_go(old)
        };
        if self.hold {
            self.latched.insert(key);
        }
        out.extend(self.note_on(key, height_velocity(height)));
        out
    }

    fn lift(&mut self) -> Vec<UiAction> {
        let Some(key) = self.pressed.take() else {
            return Vec::new();
        };
        if self.hold {
            return Vec::new();
        }
        self.let_go(key)
    }

    fn push_button(&mut self, b: KeysButton) -> Vec<UiAction> {
        match b {
            KeysButton::Mode => {
                let next = match self.mode {
                    KeysMode::Mouse => KeysMode::Computer,
                    KeysMode::Computer => KeysMode::Mouse,
                };
                return self.set_mode(next);
            }
            KeysButton::OctaveDown => {
                self.shift_octave(-1);
            }
            KeysButton::OctaveUp => {
                self.shift_octave(1);
            }
            KeysButton::VelocityDown => self.step_velocity(false),
            KeysButton::VelocityUp => self.step_velocity(true),
            KeysButton::Sustain => {
                let was = self.sustaining();
                self.sustain = !self.sustain;
                if was && !self.sustaining() {
                    return self.drop_sustained();
                }
            }
            KeysButton::Hold => {
                self.hold = !self.hold;
                if !self.hold {
                    let latched: Vec<u8> = std::mem::take(&mut self.latched).into_iter().collect();
                    return latched.into_iter().flat_map(|k| self.let_go(k)).collect();
                }
            }
            KeysButton::Variation => self.variation = !self.variation,
        }
        Vec::new()
    }

    // ------------------------------------------------------------------ paint

    /// Draws the strip inside its bounds.
    pub fn paint(&self, c: &mut dyn Canvas) {
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        self.paint_logical(c);
        c.pop_transform();
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(b, self.st.header);
        c.fill_with(Rect::new(b.x0, b.y0, b.x1, b.y0 + 1.0), self.st.separator);
        for which in BUTTONS {
            let r = self.button_rect(which);
            let (text, lit) = match which {
                KeysButton::Mode => (
                    match self.mode {
                        KeysMode::Mouse => "MOUSE",
                        KeysMode::Computer => "KEYS",
                    },
                    self.mode == KeysMode::Computer,
                ),
                KeysButton::OctaveDown | KeysButton::VelocityDown => ("-", false),
                KeysButton::OctaveUp | KeysButton::VelocityUp => ("+", false),
                KeysButton::Sustain => ("SUSTAIN", self.sustaining()),
                KeysButton::Hold => ("HOLD", self.hold),
                KeysButton::Variation => ("VAR", self.variation),
            };
            crate::seq_paint::button(c, &self.st, r, text, lit.then_some(t.highlight), false);
        }
        let readout = |c: &mut dyn Canvas, after: KeysButton, label: &str, value: String| {
            let r = self.button_rect(after);
            let x = r.x1 + 4.0;
            let lcd = Rect::new(x, r.y0, x + 36.0, r.y1);
            crate::seq_paint::lcd(c, &self.st, lcd, None);
            c.text(
                &TextRun::new(
                    value,
                    pt(lcd.center().x, lcd.center().y + 3.5),
                    10.0,
                    t.text,
                )
                .align(Align::Center)
                .weight(600),
            );
            c.text(&TextRun::new(label, pt(x, r.y0 - 0.5), 6.0, t.text_muted).weight(600));
        };
        readout(c, KeysButton::OctaveDown, "OCTAVE", self.octave_name());
        readout(
            c,
            KeysButton::VelocityDown,
            "VELOCITY",
            self.velocity.to_string(),
        );
        self.paint_keys(c);
    }

    fn paint_keys(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let typing = self.mode == KeysMode::Computer;
        let lit = |key: u8| self.sounding.contains(&key);
        // White keys first, then the black ones over them.
        for black_pass in [false, true] {
            for i in 0..KEY_COUNT {
                let (r, black) = self.key_rect(i);
                if black != black_pass {
                    continue;
                }
                let midi = u16::from(self.low()) + u16::from(i);
                let playable = midi <= 127;
                let on = playable && lit(midi as u8);
                let base = if black {
                    self.st.key_black
                } else {
                    self.st.key_white
                };
                let face = if on {
                    mix(base, t.highlight, 0.7)
                } else if playable {
                    base
                } else {
                    mix(base, self.st.background, 0.7)
                };
                c.fill_with(RoundedRect::from_rect(r, 2.0), face);
                c.stroke_with(
                    RoundedRect::from_rect(r.inset(-0.5), 2.0),
                    1.0,
                    t.bevel_dark,
                );
                let label = if typing && playable {
                    self.labels.get(usize::from(i)).filter(|_| i < TYPED_KEYS)
                } else {
                    None
                };
                if let Some(text) = label {
                    let color: Color = if black { t.on_dark } else { t.on_accent };
                    c.text(
                        &TextRun::new(
                            text.to_uppercase(),
                            pt(r.center().x, r.y1 - 20.0),
                            9.5,
                            color,
                        )
                        .align(Align::Center)
                        .weight(700),
                    );
                }
                // The C of each octave names itself.
                if !black && i % 12 == 0 && playable {
                    c.text(
                        &TextRun::new(
                            note_name(midi as u8),
                            pt(r.center().x, r.y1 - 6.0),
                            7.5,
                            if black { t.text } else { t.text_muted },
                        )
                        .align(Align::Center),
                    );
                }
            }
        }
    }
}

/// A key as the strip tracks it: letters do not change with Shift.
fn fold(key: &Key) -> Key {
    match key {
        Key::Char(c) => Key::Char(c.to_lowercase().next().unwrap_or(*c)),
        other => other.clone(),
    }
}

/// The velocity of a click at `height` (0 at the top of the keys, 1 at the bottom): soft at
/// the top, loud at the bottom.
fn height_velocity(height: f64) -> u8 {
    (20.0 + 107.0 * height.clamp(0.0, 1.0))
        .round()
        .clamp(1.0, 127.0) as u8
}

#[cfg(test)]
#[path = "piano_keys_tests.rs"]
mod tests;
