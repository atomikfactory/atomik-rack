//! The transport bar (UI): return-to-zero, stop, play, record, loop and metronome
//! buttons, the position in bars.beats.ticks and mm:ss.mmm (through the song's tempo map),
//! an editable tempo readout and the meter.
//!
//! The app owns the transport: it passes the state in with [`TransportBar::set_state`] and
//! maps the emitted [`TransportAction`]s to engine calls (no time source here).

use crate::action::{TransportAction, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::timeline::{Meter, format_bbt, format_time, tick_to_seconds};
use atomik_device_sdk::Registry;
use atomik_model::{Command, RackDoc, SongCommand, TEMPO_MAX, TEMPO_MIN, TempoEvent};
use vello::kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

/// Recommended height of the bar, logical pixels.
pub const TRANSPORT_HEIGHT: f64 = 44.0;
const BUTTON_W: f64 = 34.0;
const BUTTON_H: f64 = 26.0;
const TEMPO_TEXT_MAX: usize = 7;

/// The transport state shown by the bar (owned by the app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransportState {
    /// Playing.
    pub playing: bool,
    /// Recording.
    pub recording: bool,
    /// Metronome on.
    pub metronome: bool,
    /// Position in song ticks.
    pub position: i64,
}

/// A transport bar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportButton {
    /// Return to zero.
    ReturnToZero,
    /// Stop.
    Stop,
    /// Play.
    Play,
    /// Record.
    Record,
    /// Loop on/off.
    Loop,
    /// Metronome on/off.
    Metronome,
}

const BUTTONS: [TransportButton; 6] = [
    TransportButton::ReturnToZero,
    TransportButton::Stop,
    TransportButton::Play,
    TransportButton::Record,
    TransportButton::Loop,
    TransportButton::Metronome,
];

/// The transport keyboard shortcuts, for views and for an app that routes them globally:
/// Space toggles play, Enter returns to zero, L toggles the loop, R toggles recording.
/// Shortcut-modified (Ctrl/Cmd/Alt) keys are not transport keys.
pub fn transport_shortcut(key: &Key, modifiers: Modifiers) -> Option<TransportAction> {
    if modifiers.command() || modifiers.alt {
        return None;
    }
    match key {
        Key::Space => Some(TransportAction::TogglePlay),
        Key::Enter => Some(TransportAction::ReturnToZero),
        Key::Char(c) if c.eq_ignore_ascii_case(&'l') => Some(TransportAction::ToggleLoop),
        Key::Char(c) if c.eq_ignore_ascii_case(&'r') => Some(TransportAction::ToggleRecord),
        _ => None,
    }
}

/// The transport bar.
#[derive(Debug)]
pub struct TransportBar {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    state: TransportState,
    tempo: Vec<TempoEvent>,
    meter: Meter,
    loop_enabled: bool,
    editing: Option<(String, bool)>,
    hover: Option<TransportButton>,
    focused: bool,
    repaint: bool,
}

impl TransportBar {
    /// A bar; call [`Self::set_bounds`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, TRANSPORT_HEIGHT),
            scale: 1.0,
            state: TransportState::default(),
            tempo: vec![TempoEvent::step(0, 120.0)],
            meter: Meter::default(),
            loop_enabled: false,
            editing: None,
            hover: None,
            focused: false,
            repaint: true,
        }
    }

    /// Places the bar in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// The bar's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Updates the shown transport state (call every frame while playing).
    pub fn set_state(&mut self, state: TransportState) {
        if state != self.state {
            self.state = state;
            self.repaint = true;
        }
    }

    /// The shown transport state.
    pub fn state(&self) -> TransportState {
        self.state
    }

    /// Marks the bar as focused (keys are routed to it).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// True while the tempo is being typed: route every key event here.
    pub fn is_editing_tempo(&self) -> bool {
        self.editing.is_some()
    }

    /// The tempo text being typed.
    pub fn tempo_text(&self) -> Option<&str> {
        self.editing.as_ref().map(|(s, _)| s.as_str())
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Never drags (pointer events can be routed by bounds alone).
    pub fn is_dragging(&self) -> bool {
        false
    }

    /// Centre of a button.
    pub fn button_center(&self, button: TransportButton) -> Point {
        self.button_rect(button).center()
    }

    /// Centre of the tempo readout (double-click to type a tempo).
    pub fn tempo_center(&self) -> Point {
        self.lcd_rects()[2].center()
    }

    /// The position readouts: (bars.beats.ticks, mm:ss.mmm).
    pub fn position_text(&self) -> (String, String) {
        let p = self.state.position;
        (
            format_bbt(p, self.meter),
            format_time(tick_to_seconds(p, &self.tempo)),
        )
    }

    /// Reads the tempo map, meter and loop flag from the document.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let meter = Meter::of(&doc.song);
        if self.tempo != doc.song.tempo
            || self.meter != meter
            || self.loop_enabled != doc.song.loop_enabled
        {
            self.tempo.clone_from(&doc.song.tempo);
            self.meter = meter;
            self.loop_enabled = doc.song.loop_enabled;
            self.repaint = true;
        }
    }

    fn bpm(&self) -> f64 {
        self.tempo.first().map_or(120.0, |e| e.bpm)
    }

    fn button_rect(&self, b: TransportButton) -> Rect {
        let i = BUTTONS.iter().position(|x| *x == b).unwrap_or(0) as f64;
        let gap = if i >= 4.0 { 14.0 } else { 0.0 };
        let x = self.bounds.x0 + 12.0 + i * (BUTTON_W + 4.0) + gap;
        let y = self.bounds.center().y - BUTTON_H / 2.0;
        Rect::new(x, y, x + BUTTON_W, y + BUTTON_H)
    }

    /// Position (bbt), time, tempo and meter readouts.
    fn lcd_rects(&self) -> [Rect; 4] {
        let x0 = self.button_rect(TransportButton::Metronome).x1 + 18.0;
        let y0 = self.bounds.center().y - 14.0;
        let mut x = x0;
        let mut out = [Rect::ZERO; 4];
        for (i, w) in [124.0, 104.0, 92.0, 46.0].into_iter().enumerate() {
            out[i] = Rect::new(x, y0, x + w, y0 + 28.0);
            x += w + 8.0;
        }
        out
    }

    fn tempo_cmd(&self, bpm: f64) -> Option<UiAction> {
        let bpm = bpm.clamp(TEMPO_MIN, TEMPO_MAX);
        if !bpm.is_finite() || (bpm - self.bpm()).abs() < 1e-9 {
            return None;
        }
        let mut map = self.tempo.clone();
        match map.first_mut() {
            Some(e) => e.bpm = bpm,
            None => map.push(TempoEvent { tick: 0, bpm }),
        }
        Some(UiAction::Command(Command::Song(SongCommand::SetTempoMap(
            map,
        ))))
    }

    fn commit(&mut self, out: &mut Vec<UiAction>) {
        if let Some((text, _)) = self.editing.take()
            && let Ok(v) = text.trim().parse::<f64>()
        {
            out.extend(self.tempo_cmd(v));
        }
        self.repaint = true;
    }

    /// Handles one event. `Resize` is ignored (use [`Self::set_bounds`]).
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                let h = BUTTONS
                    .into_iter()
                    .find(|b| self.button_rect(*b).contains(*pos));
                if h != self.hover {
                    self.hover = h;
                    self.repaint = true;
                }
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint = true;
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } => {
                if self.editing.is_some() {
                    self.commit(&mut out);
                }
                if let Some(b) = BUTTONS
                    .into_iter()
                    .find(|b| self.button_rect(*b).contains(*pos))
                {
                    out.push(UiAction::Transport(match b {
                        TransportButton::ReturnToZero => TransportAction::ReturnToZero,
                        TransportButton::Stop => TransportAction::Stop,
                        TransportButton::Play => TransportAction::Play,
                        TransportButton::Record => TransportAction::ToggleRecord,
                        TransportButton::Loop => TransportAction::ToggleLoop,
                        TransportButton::Metronome => TransportAction::ToggleMetronome,
                    }));
                } else if *clicks >= 2 && self.lcd_rects()[2].contains(*pos) {
                    self.editing = Some((format_bpm(self.bpm()), true));
                }
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } if self.lcd_rects()[2].contains(*pos) && self.editing.is_none() => {
                let step = if modifiers.shift { 0.1 } else { 1.0 };
                let v = ((self.bpm() + delta.y.signum() * step) * 100.0).round() / 100.0;
                out.extend(self.tempo_cmd(v));
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, &mut out),
            _ => {}
        }
        out
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if let Some((text, fresh)) = &mut self.editing {
            match key {
                Key::Char(c) if c.is_ascii_digit() || *c == '.' => {
                    if *fresh {
                        text.clear();
                        *fresh = false;
                    }
                    if text.len() < TEMPO_TEXT_MAX {
                        text.push(*c);
                    }
                }
                Key::Backspace => {
                    *fresh = false;
                    text.pop();
                }
                Key::Enter => self.commit(out),
                Key::Escape => self.editing = None,
                _ => {}
            }
            self.repaint = true;
            return;
        }
        if let Some(t) = transport_shortcut(key, modifiers) {
            out.push(UiAction::Transport(t));
        }
    }

    /// Paints the bar into its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        let st = &self.st;
        let t = &st.base;
        let b = self.bounds;
        let c = canvas;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&b.into());
        let grad = vello::peniko::Gradient::new_linear(pt(0.0, b.y0), pt(0.0, b.y1))
            .with_stops([t.panel_top, t.panel_bottom]);
        c.fill(&b.into(), &grad.into());
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, t.bevel_light);
        sp::line(c, b.x0, b.y1 - 0.5, b.x1, b.y1 - 0.5, t.bevel_dark);
        for btn in BUTTONS {
            let r = self.button_rect(btn);
            let lit = match btn {
                TransportButton::Play => self.state.playing.then_some(st.solo),
                TransportButton::Record => self.state.recording.then_some(st.record),
                TransportButton::Loop => self.loop_enabled.then_some(st.loop_on),
                TransportButton::Metronome => self.state.metronome.then_some(t.focus),
                _ => None,
            };
            sp::button(c, st, r, "", lit, self.hover == Some(btn));
            let icon = if lit.is_some() { t.plate } else { t.text };
            let icon = if btn == TransportButton::Record && lit.is_none() {
                mix(st.record, t.text, 0.15)
            } else {
                icon
            };
            paint_icon(c, btn, r.center(), icon);
        }
        let [pos_r, time_r, tempo_r, meter_r] = self.lcd_rects();
        let (bbt, time) = self.position_text();
        let lcd_text = t.highlight;
        sp::lcd(c, st, pos_r, None);
        sp::label(
            c,
            "POS",
            pos_r.x0 + 6.0,
            pos_r.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        c.text(
            &TextRun::new(bbt, pt(pos_r.x1 - 8.0, pos_r.y1 - 7.0), 13.0, lcd_text)
                .weight(600)
                .align(Align::End)
                .spacing(0.5),
        );
        sp::lcd(c, st, time_r, None);
        sp::label(
            c,
            "TIME",
            time_r.x0 + 6.0,
            time_r.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        c.text(
            &TextRun::new(time, pt(time_r.x1 - 8.0, time_r.y1 - 7.0), 12.0, lcd_text)
                .weight(600)
                .align(Align::End),
        );
        let editing = self.editing.as_ref();
        sp::lcd(c, st, tempo_r, editing.map(|_| t.focus));
        sp::label(
            c,
            "BPM",
            tempo_r.x0 + 6.0,
            tempo_r.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        let tempo_text = match editing {
            Some((s, _)) => format!("{s}_"),
            None => format!("{:.2}", self.bpm()),
        };
        c.text(
            &TextRun::new(
                tempo_text,
                pt(tempo_r.x1 - 8.0, tempo_r.y1 - 7.0),
                13.0,
                lcd_text,
            )
            .weight(600)
            .align(Align::End),
        );
        sp::lcd(c, st, meter_r, None);
        c.text(
            &TextRun::new(
                format!("{}/{}", self.meter.beats_per_bar, self.meter.beat_unit),
                pt(meter_r.center().x, meter_r.y1 - 9.0),
                12.0,
                lcd_text,
            )
            .weight(600)
            .align(Align::Center),
        );
        if self.focused {
            sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, t.focus);
        }
        c.pop_clip();
        c.pop_transform();
        self.repaint = false;
    }
}

fn format_bpm(bpm: f64) -> String {
    let s = format!("{bpm:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn paint_icon(c: &mut dyn Canvas, b: TransportButton, ctr: Point, col: Color) {
    let (x, y) = (ctr.x, ctr.y);
    let tri = |pts: [(f64, f64); 3]| {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        p.line_to(pts[1]);
        p.line_to(pts[2]);
        p.close_path();
        p
    };
    match b {
        TransportButton::ReturnToZero => {
            c.fill_with(Rect::new(x - 6.0, y - 5.0, x - 4.0, y + 5.0), col);
            c.fill_with(
                tri([(x + 5.0, y - 5.0), (x + 5.0, y + 5.0), (x - 3.0, y)]),
                col,
            );
        }
        TransportButton::Stop => c.fill_with(Rect::new(x - 4.5, y - 4.5, x + 4.5, y + 4.5), col),
        TransportButton::Play => {
            c.fill_with(
                tri([(x - 4.0, y - 6.0), (x - 4.0, y + 6.0), (x + 6.0, y)]),
                col,
            );
        }
        TransportButton::Record => c.fill_with(Circle::new(ctr, 5.0), col),
        TransportButton::Loop => {
            let r = RoundedRect::new(x - 7.0, y - 4.0, x + 7.0, y + 4.0, 4.0);
            c.stroke(&r.into(), &Stroke::new(1.6), &col.into());
            c.fill_with(
                tri([(x + 1.0, y - 7.0), (x + 1.0, y - 1.0), (x + 4.5, y - 4.0)]),
                col,
            );
            c.fill_with(
                tri([(x - 1.0, y + 7.0), (x - 1.0, y + 1.0), (x - 4.5, y + 4.0)]),
                col,
            );
        }
        TransportButton::Metronome => {
            let mut body = BezPath::new();
            body.move_to(pt(x - 3.0, y - 6.0));
            body.line_to(pt(x + 3.0, y - 6.0));
            body.line_to(pt(x + 6.0, y + 6.0));
            body.line_to(pt(x - 6.0, y + 6.0));
            body.close_path();
            c.stroke(&body.into(), &Stroke::new(1.4), &col.into());
            c.stroke_with(
                vello::kurbo::Line::new(pt(x, y + 4.0), pt(x + 5.0, y - 7.0)),
                1.6,
                col,
            );
        }
    }
}
