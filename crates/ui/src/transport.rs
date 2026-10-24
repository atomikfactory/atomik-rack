//! The transport bar (UI): return-to-zero, stop, play, record, loop and metronome
//! buttons, the position in bars.beats.ticks and mm:ss.mmm (through the song's tempo map),
//! an editable tempo readout and an editable meter readout. The tempo and the meter shown are
//! those at the playhead; typing a tempo changes the tempo event in effect there, typing a
//! signature (`3/4`) changes the meter change in effect there (the first one before bar 2).
//!
//! The app owns the transport: it passes the state in with [`TransportBar::set_state`] and
//! maps the emitted [`TransportAction`]s to engine calls (no time source here).

use crate::action::{TransportAction, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::timeline::{Bars, format_bbt, format_time, tick_to_seconds};
use atomik_core::{TempoPoint, TempoTable};
use atomik_device_sdk::Registry;
use atomik_model::{Command, MeterEvent, RackDoc, SongCommand, TEMPO_MAX, TEMPO_MIN, TempoEvent};
use vello::kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

/// Recommended height of the bar, logical pixels.
pub const TRANSPORT_HEIGHT: f64 = 44.0;
const BUTTON_W: f64 = 34.0;
const BUTTON_H: f64 = 26.0;
const TEMPO_TEXT_MAX: usize = 7;
/// `32/16`.
const METER_TEXT_MAX: usize = 5;

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
    meter: Vec<MeterEvent>,
    bars: Bars,
    loop_enabled: bool,
    editing: Option<(String, bool)>,
    /// The tempo event the typed value goes to (fixed when the entry starts).
    edit_event: usize,
    /// The signature being typed (`3/4`) and whether the text is still the untouched original.
    meter_editing: Option<(String, bool)>,
    /// The meter change the typed signature goes to (fixed when the entry starts).
    meter_edit_event: usize,
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
            meter: vec![MeterEvent {
                bar: 1,
                num: 4,
                den: 4,
            }],
            bars: Bars::default(),
            loop_enabled: false,
            editing: None,
            edit_event: 0,
            meter_editing: None,
            meter_edit_event: 0,
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

    /// True while the signature is being typed: route every key event here.
    pub fn is_editing_meter(&self) -> bool {
        self.meter_editing.is_some()
    }

    /// The signature text being typed.
    pub fn meter_text(&self) -> Option<&str> {
        self.meter_editing.as_ref().map(|(s, _)| s.as_str())
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

    /// Centre of the meter readout (double-click to type a signature).
    pub fn meter_center(&self) -> Point {
        self.lcd_rects()[3].center()
    }

    /// The position readouts: (bars.beats.ticks, mm:ss.mmm).
    pub fn position_text(&self) -> (String, String) {
        let p = self.state.position;
        (
            format_bbt(p, &self.bars),
            format_time(tick_to_seconds(p, &self.tempo)),
        )
    }

    /// Reads the tempo map, meter and loop flag from the document.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let bars = Bars::of(&doc.song);
        if self.tempo != doc.song.tempo
            || self.meter != doc.song.meter
            || self.bars != bars
            || self.loop_enabled != doc.song.loop_enabled
        {
            self.tempo.clone_from(&doc.song.tempo);
            self.meter.clone_from(&doc.song.meter);
            self.bars = bars;
            self.loop_enabled = doc.song.loop_enabled;
            self.repaint = true;
        }
    }

    /// The index of the tempo event in effect at the playhead.
    fn event_at_playhead(&self) -> usize {
        let at = self.state.position.max(0);
        self.tempo
            .partition_point(|e| e.tick <= at)
            .saturating_sub(1)
    }

    /// The tempo at the playhead: the event's own BPM, or inside a ramp the tempo there.
    fn bpm(&self) -> f64 {
        let points: Vec<TempoPoint> = self
            .tempo
            .iter()
            .map(|e| TempoPoint {
                tick: e.tick,
                bpm: e.bpm,
                ramp: e.ramp,
            })
            .collect();
        TempoTable::new(&points).map_or_else(
            |_| self.tempo.first().map_or(120.0, |e| e.bpm),
            |t| t.map().bpm(self.state.position.max(0) as f64),
        )
    }

    /// The BPM of tempo event `i`.
    fn event_bpm(&self, i: usize) -> f64 {
        self.tempo.get(i).map_or(120.0, |e| e.bpm)
    }

    /// Starts typing a tempo for event `event` (the one in effect at the playhead when `None`).
    /// The text field shows its BPM; Enter changes that event.
    pub fn begin_tempo_edit(&mut self, event: Option<usize>) {
        self.edit_event = event
            .filter(|i| *i < self.tempo.len())
            .unwrap_or_else(|| self.event_at_playhead());
        self.editing = Some((format_bpm(self.event_bpm(self.edit_event)), true));
        self.repaint = true;
    }

    /// The index of the meter change in effect at the playhead.
    fn meter_event_at_playhead(&self) -> usize {
        let at = self.state.position.max(0);
        self.bars
            .segments()
            .partition_point(|s| s.tick <= at)
            .saturating_sub(1)
    }

    /// Starts typing a signature for the meter change in effect at the playhead. The text
    /// field shows it; Enter changes that change.
    pub fn begin_meter_edit(&mut self) {
        self.meter_edit_event = self
            .meter_event_at_playhead()
            .min(self.meter.len().saturating_sub(1));
        let (num, den) = self
            .meter
            .get(self.meter_edit_event)
            .map_or((4, 4), |e| (e.num, e.den));
        self.meter_editing = Some((format!("{num}/{den}"), true));
        self.repaint = true;
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

    /// The command that sets the BPM of event `event` (`None` when nothing changes).
    fn tempo_cmd(&self, event: usize, bpm: f64) -> Option<UiAction> {
        let bpm = bpm.clamp(TEMPO_MIN, TEMPO_MAX);
        if !bpm.is_finite() || (bpm - self.event_bpm(event)).abs() < 1e-9 {
            return None;
        }
        let mut map = self.tempo.clone();
        match map.get_mut(event) {
            Some(e) => e.bpm = bpm,
            None => map.push(TempoEvent::step(0, bpm)),
        }
        Some(UiAction::Command(Command::Song(SongCommand::SetTempoMap(
            map,
        ))))
    }

    /// The command that sets the signature of meter change `event` to `text` (`3/4`, or `3`
    /// to keep the beat unit); `None` when it does not parse, is not a valid signature or
    /// changes nothing.
    fn meter_cmd(&self, event: usize, text: &str) -> Option<UiAction> {
        let (n, d) = match text.trim().split_once('/') {
            Some((n, d)) => (n, Some(d)),
            None => (text.trim(), None),
        };
        let num: u8 = n.parse().ok()?;
        let den: u8 = match d {
            Some(d) => d.parse().ok()?,
            None => self.meter.get(event)?.den,
        };
        let map = crate::tempo_lane::set_meter(&self.meter, event, num, den)?;
        (map != self.meter).then_some(UiAction::Command(Command::Song(SongCommand::SetMeterMap(
            map,
        ))))
    }

    fn commit_meter(&mut self, out: &mut Vec<UiAction>) {
        if let Some((text, _)) = self.meter_editing.take() {
            out.extend(self.meter_cmd(self.meter_edit_event, &text));
        }
        self.repaint = true;
    }

    fn commit(&mut self, out: &mut Vec<UiAction>) {
        if let Some((text, _)) = self.editing.take()
            && let Ok(v) = text.trim().parse::<f64>()
        {
            out.extend(self.tempo_cmd(self.edit_event, v));
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
                if self.meter_editing.is_some() {
                    self.commit_meter(&mut out);
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
                    self.begin_tempo_edit(None);
                } else if *clicks >= 2 && self.lcd_rects()[3].contains(*pos) {
                    self.begin_meter_edit();
                }
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } if self.lcd_rects()[2].contains(*pos) && self.editing.is_none() => {
                let step = if modifiers.shift { 0.1 } else { 1.0 };
                let at = self.event_at_playhead();
                let v = ((self.event_bpm(at) + delta.y.signum() * step) * 100.0).round() / 100.0;
                out.extend(self.tempo_cmd(at, v));
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
        if let Some((text, fresh)) = &mut self.meter_editing {
            match key {
                Key::Char(c) if c.is_ascii_digit() || *c == '/' => {
                    if *fresh {
                        text.clear();
                        *fresh = false;
                    }
                    if text.len() < METER_TEXT_MAX {
                        text.push(*c);
                    }
                }
                Key::Backspace => {
                    *fresh = false;
                    text.pop();
                }
                Key::Enter => self.commit_meter(out),
                Key::Escape => self.meter_editing = None,
                _ => {}
            }
            self.repaint = true;
            return;
        }
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
        let meter_edit = self.meter_editing.as_ref();
        sp::lcd(c, st, meter_r, meter_edit.map(|_| t.focus));
        let meter_text = match meter_edit {
            Some((s, _)) => format!("{s}_"),
            None => self.bars.meter_at(self.state.position).label(),
        };
        c.text(
            &TextRun::new(
                meter_text,
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
