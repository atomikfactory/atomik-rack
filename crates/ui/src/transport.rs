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
use crate::step_drag::{PX_PER_STEP, StepDrag, step_index, wheel_steps};
use crate::theme::{Theme, mix};
use crate::timeline::{Bars, format_bbt, format_time, tick_to_seconds};
use crate::tooltip::Help;
use atomik_core::{BEAT_UNITS, MAX_BEATS_PER_BAR, TempoPoint, TempoTable};
use atomik_device_sdk::Registry;
use atomik_model::{Command, MeterEvent, RackDoc, SongCommand, TEMPO_MAX, TEMPO_MIN, TempoEvent};
use vello::kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

mod cluster;
mod inspector;
mod readout;
mod rec_input;
pub use cluster::{ClusterPart, REPEAT_DELAY, REPEAT_EVERY, repeat_step};
use cluster::{ClusterState, Readout};
use inspector::Inspector;
pub use inspector::{InspClip, InspectorField, InspectorSource};
use rec_input::RecInputState;
pub use rec_input::{RecInput, RecInputPart};

/// Recommended height of the bar, logical pixels.
pub const TRANSPORT_HEIGHT: f64 = 44.0;
const BUTTON_W: f64 = 34.0;
const BUTTON_H: f64 = 26.0;
/// The width of the auto-create-lanes switch, a small one next to Record.
const AUTO_LANES_W: f64 = 20.0;
/// The width of a transport button.
fn button_width(b: TransportButton) -> f64 {
    if b == TransportButton::AutoLanes {
        AUTO_LANES_W
    } else {
        BUTTON_W
    }
}
const TEMPO_TEXT_MAX: usize = 7;
/// `32/16`.
const METER_TEXT_MAX: usize = 5;
/// Font size of the tempo readout.
const TEMPO_SIZE: f32 = 13.0;
/// The tempo's right padding in its readout.
const LCD_PAD: f64 = 8.0;
/// The width of the painted decimals (`.00`) at the right end of the tempo readout, from the
/// same advance estimate the readout is laid out with: a drag or a wheel there steps hundredths,
/// on the rest whole BPM.
fn tempo_decimals_w() -> f64 {
    crate::paint::estimate_width(".00", TEMPO_SIZE)
}

/// Whole-BPM step with Shift (tenths).
const TEMPO_FINE: f64 = 0.1;
/// Merge ids of the transport's drags start here (the views use multiples of 2^40 up to 12).
pub const TRANSPORT_MERGE_BASE: u64 = 13 << 40;

/// The part of a readout a drag or the wheel changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The whole BPM (tenths with Shift).
    TempoWhole,
    /// The two decimals of the BPM.
    TempoDecimals,
    /// Beats per bar.
    MeterNum,
    /// The beat unit (2, 4, 8, 16).
    MeterDen,
}

/// A drag on a readout: the value is tracked here so the drag never depends on the document
/// catching up between two moves.
#[derive(Debug, Clone, Copy)]
struct ValueDrag {
    field: Field,
    steps: StepDrag,
    /// The tempo or meter event the drag changes (fixed at the press).
    event: usize,
    bpm: f64,
    num: u8,
    den: u8,
    /// The last value sent, so a drag held against the end of the range sends nothing.
    sent: (f64, u8, u8),
    merge: u64,
    changed: bool,
}

/// The BPM `notches` steps from `bpm` on `field`: whole BPM (tenths when `fine`) or hundredths,
/// clamped to the tempo range and rounded to hundredths.
fn step_bpm(bpm: f64, field: Field, notches: i32, fine: bool) -> f64 {
    let step = match field {
        Field::TempoDecimals => 0.01,
        _ if fine => TEMPO_FINE,
        _ => 1.0,
    };
    let v = (bpm + f64::from(notches) * step).clamp(TEMPO_MIN, TEMPO_MAX);
    (v * 100.0).round() / 100.0
}

/// Beats per bar `notches` steps from `num`, within 1..=32.
fn step_num(num: u8, notches: i32) -> u8 {
    (i32::from(num) + notches).clamp(1, i32::from(MAX_BEATS_PER_BAR)) as u8
}

/// The beat unit `notches` steps through 2, 4, 8, 16 from `den` (no wrap).
fn step_den(den: u8, notches: i32) -> u8 {
    let at = BEAT_UNITS.iter().position(|d| *d == den).unwrap_or(1);
    BEAT_UNITS[step_index(at, notches, BEAT_UNITS.len())]
}

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
    /// Bars of pre-count a recording starts with (0 = off).
    pub pre_count: u8,
    /// The beat of the bar being counted in (1-based) while the pre-count runs, else 0.
    pub count_in: u16,
    /// A take also creates the lanes of the parameters moved during it (AUTO LANES).
    pub auto_lanes: bool,
    /// How many lanes hold a value moved by hand while the song plays (0: no OVERRIDE lamp).
    pub overrides: u8,
    /// Recorded note starts snap to a grid of this many ticks (Q Rec on); `None` when off.
    pub quantize: Option<i64>,
    /// The transport follows an external MIDI clock: the position controls do not repeat.
    pub following: bool,
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
    /// Create automation lanes for the parameters moved during a take, on/off.
    AutoLanes,
    /// Loop on/off.
    Loop,
    /// Metronome on/off.
    Metronome,
}

const BUTTONS: [TransportButton; 7] = [
    TransportButton::ReturnToZero,
    TransportButton::Stop,
    TransportButton::Play,
    TransportButton::Record,
    TransportButton::AutoLanes,
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
    /// The song's loop range (the locators).
    loop_range: (i64, i64),
    /// The metronome level the click slider shows.
    click_gain: f32,
    /// The second cluster: wind buttons, locators, click level, Q Rec.
    cl: ClusterState,
    editing: Option<(String, bool)>,
    /// The tempo event the typed value goes to (fixed when the entry starts).
    edit_event: usize,
    /// The signature being typed (`3/4`) and whether the text is still the untouched original.
    meter_editing: Option<(String, bool)>,
    /// The meter change the typed signature goes to (fixed when the entry starts).
    meter_edit_event: usize,
    hover: Option<TransportButton>,
    drag: Option<ValueDrag>,
    /// The clip inspector at the right of the readouts.
    insp: Inspector,
    /// The recording input (gain, clip-safe, level) at the right end.
    rec: RecInputState,
    next_merge: u64,
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
            loop_range: (0, 0),
            click_gain: 0.5,
            cl: ClusterState::new(),
            editing: None,
            edit_event: 0,
            meter_editing: None,
            meter_edit_event: 0,
            hover: None,
            drag: None,
            insp: Inspector::default(),
            rec: RecInputState::default(),
            next_merge: TRANSPORT_MERGE_BASE,
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

    /// True while a drag on the tempo or the signature is held.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
            || self.is_dragging_inspector()
            || self.is_dragging_rec_input()
            || self.is_dragging_cluster()
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

    /// A point on the whole-BPM part of the tempo readout.
    pub fn tempo_whole_point(&self) -> Point {
        self.tempo_center()
    }

    /// A point on the decimals of the tempo readout.
    pub fn tempo_decimals_point(&self) -> Point {
        let r = self.lcd_rects()[2];
        pt(r.x1 - LCD_PAD - tempo_decimals_w() / 2.0, r.center().y)
    }

    /// A point on the beats-per-bar part of the signature readout.
    pub fn meter_num_point(&self) -> Point {
        let r = self.lcd_rects()[3];
        pt(r.center().x - 8.0, r.center().y)
    }

    /// A point on the beat-unit part of the signature readout.
    pub fn meter_den_point(&self) -> Point {
        let r = self.lcd_rects()[3];
        pt(r.center().x + 8.0, r.center().y)
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
            || self.loop_range != doc.song.loop_range
        {
            self.loop_range = doc.song.loop_range;
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
        let i = BUTTONS.iter().position(|x| *x == b).unwrap_or(0);
        // The buttons in a row, 4 px apart; the loop and the metronome stand a little apart.
        let before: f64 = BUTTONS[..i].iter().map(|x| button_width(*x) + 4.0).sum();
        let gap = if i >= 5 { 14.0 } else { 0.0 };
        let x = self.bounds.x0 + 12.0 + before + gap;
        let y = self.bounds.center().y - BUTTON_H / 2.0;
        Rect::new(x, y, x + button_width(b), y + BUTTON_H)
    }

    /// Position (bbt), time, tempo and meter readouts.
    fn lcd_rects(&self) -> [Rect; 4] {
        let x0 = self.button_rect(TransportButton::Metronome).x1 + 8.0;
        let y0 = self.bounds.center().y - 14.0;
        let mut x = x0;
        let mut out = [Rect::ZERO; 4];
        for (i, w) in [124.0, 104.0, 92.0, 46.0].into_iter().enumerate() {
            out[i] = Rect::new(x, y0, x + w, y0 + 28.0);
            x += w + 6.0;
        }
        out
    }

    /// The right end of the four readouts.
    fn lcd_end(&self) -> f64 {
        self.lcd_rects()[3].x1
    }

    /// The OVERRIDE lamp: in the top right corner of the time readout, shown only while some
    /// lane holds a hand-moved value.
    pub fn override_rect(&self) -> Option<Rect> {
        (self.state.overrides > 0).then(|| {
            let r = self.lcd_rects()[1];
            Rect::new(r.x1 - 56.0, r.y0 + 3.0, r.x1 - 4.0, r.y0 + 14.0)
        })
    }

    /// Centre of the OVERRIDE lamp (`None` while it is not shown).
    pub fn override_center(&self) -> Option<Point> {
        self.override_rect().map(|r| r.center())
    }

    /// The command that sets the BPM of event `event` (`None` when nothing changes).
    fn tempo_cmd(&self, event: usize, bpm: f64) -> Option<UiAction> {
        let map = self.tempo_map_with(event, bpm)?;
        Some(UiAction::Command(Command::Song(SongCommand::SetTempoMap(
            map,
        ))))
    }

    /// The tempo map with event `event` at `bpm` (clamped); `None` when nothing changes.
    fn tempo_map_with(&self, event: usize, bpm: f64) -> Option<Vec<TempoEvent>> {
        let bpm = bpm.clamp(TEMPO_MIN, TEMPO_MAX);
        if !bpm.is_finite() || (bpm - self.event_bpm(event)).abs() < 1e-9 {
            return None;
        }
        let mut map = self.tempo.clone();
        match map.get_mut(event) {
            Some(e) => e.bpm = bpm,
            None => map.push(TempoEvent::step(0, bpm)),
        }
        Some(map)
    }

    /// The meter map with change `event` set to `num`/`den`; `None` when invalid or unchanged.
    fn meter_map_with(&self, event: usize, num: u8, den: u8) -> Option<Vec<MeterEvent>> {
        let map = crate::tempo_lane::set_meter(&self.meter, event, num, den)?;
        (map != self.meter).then_some(map)
    }

    /// The readout field under `pos`, if it is one.
    fn field_at(&self, pos: Point) -> Option<Field> {
        let [_, _, tempo, meter] = self.lcd_rects();
        if tempo.contains(pos) {
            Some(if pos.x >= tempo.x1 - LCD_PAD - tempo_decimals_w() {
                Field::TempoDecimals
            } else {
                Field::TempoWhole
            })
        } else if meter.contains(pos) {
            Some(if pos.x < meter.center().x {
                Field::MeterNum
            } else {
                Field::MeterDen
            })
        } else {
            None
        }
    }

    /// A press on a readout field: the drag starts from the value in effect at the playhead.
    fn begin_drag(&mut self, field: Field, y: f64) {
        let event = match field {
            Field::TempoWhole | Field::TempoDecimals => self.event_at_playhead(),
            Field::MeterNum | Field::MeterDen => self
                .meter_event_at_playhead()
                .min(self.meter.len().saturating_sub(1)),
        };
        let bpm = self.event_bpm(event);
        let (num, den) = self.meter.get(event).map_or((4, 4), |e| (e.num, e.den));
        self.drag = Some(ValueDrag {
            field,
            steps: StepDrag::new(y),
            event,
            bpm,
            num,
            den,
            sent: (bpm, num, den),
            merge: self.next_merge,
            changed: false,
        });
        self.next_merge += 1;
    }

    /// The pointer moved with a drag held: steps the value and sends it live (Shift: the fine
    /// step of the whole BPM).
    fn drag_to(&mut self, y: f64, fine: bool, out: &mut Vec<UiAction>) {
        let Some(mut d) = self.drag else { return };
        let n = d.steps.update(y, PX_PER_STEP);
        if n != 0 {
            match d.field {
                Field::TempoWhole | Field::TempoDecimals => {
                    d.bpm = step_bpm(d.bpm, d.field, n, fine);
                }
                Field::MeterNum => d.num = step_num(d.num, n),
                Field::MeterDen => d.den = step_den(d.den, n),
            }
            if (d.bpm, d.num, d.den) != d.sent {
                d.sent = (d.bpm, d.num, d.den);
                let merge = d.merge;
                let cmd = match d.field {
                    Field::TempoWhole | Field::TempoDecimals => self
                        .tempo_map_with(d.event, d.bpm)
                        .map(|map| SongCommand::DragTempoMap { map, merge }),
                    Field::MeterNum | Field::MeterDen => self
                        .meter_map_with(d.event, d.num, d.den)
                        .map(|map| SongCommand::DragMeterMap { map, merge }),
                };
                if let Some(c) = cmd {
                    d.changed = true;
                    out.push(UiAction::Command(Command::Song(c)));
                }
            }
            self.repaint = true;
        }
        self.drag = Some(d);
    }

    /// One wheel event over a readout field: one notch is one step.
    fn wheel_field(&self, field: Field, notches: i32, fine: bool, out: &mut Vec<UiAction>) {
        match field {
            Field::TempoWhole | Field::TempoDecimals => {
                let at = self.event_at_playhead();
                let v = step_bpm(self.event_bpm(at), field, notches, fine);
                out.extend(self.tempo_cmd(at, v));
            }
            Field::MeterNum | Field::MeterDen => {
                let at = self
                    .meter_event_at_playhead()
                    .min(self.meter.len().saturating_sub(1));
                let (num, den) = self.meter.get(at).map_or((4, 4), |e| (e.num, e.den));
                let (num, den) = match field {
                    Field::MeterNum => (step_num(num, notches), den),
                    _ => (num, step_den(den, notches)),
                };
                out.extend(
                    self.meter_map_with(at, num, den)
                        .map(|m| UiAction::Command(Command::Song(SongCommand::SetMeterMap(m)))),
                );
            }
        }
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
            UiEvent::PointerMove { pos, modifiers } => {
                self.drag_to(pos.y, modifiers.shift, &mut out);
                self.inspector_drag_to(pos.y, modifiers.shift, &mut out);
                self.rec_drag_to(pos.y, modifiers.shift, &mut out);
                self.cluster_drag_to(*pos, &mut out);
                self.cluster_hover(*pos);
                let h = BUTTONS
                    .into_iter()
                    .find(|b| self.button_rect(*b).contains(*pos));
                if h != self.hover {
                    self.hover = h;
                    self.repaint = true;
                }
            }
            UiEvent::PointerLeave => {
                self.cluster_leave();
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
                if self.cl.edit.is_some() {
                    self.commit_readout(&mut out);
                }
                if self.insp.is_editing() {
                    self.inspector_commit(&mut out);
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
                        TransportButton::AutoLanes => TransportAction::ToggleAutoLanes,
                        TransportButton::Loop => TransportAction::ToggleLoop,
                        TransportButton::Metronome => TransportAction::ToggleMetronome,
                    }));
                } else if self.override_rect().is_some_and(|r| r.contains(*pos)) {
                    out.push(UiAction::Transport(TransportAction::ClearOverride));
                } else if let Some(part) = self.cluster_hit(*pos) {
                    self.cluster_press(part, *pos, *clicks, &mut out);
                } else if *clicks >= 2
                    && let Some(which) = self.readout_at(*pos)
                {
                    self.begin_readout_edit(which);
                } else if *clicks >= 2 && self.lcd_rects()[2].contains(*pos) {
                    self.begin_tempo_edit(None);
                } else if *clicks >= 2 && self.lcd_rects()[3].contains(*pos) {
                    self.begin_meter_edit();
                } else if let Some(part) = self.rec_hit(*pos) {
                    out.extend(self.rec_press(part, *clicks, pos.y));
                } else if let Some(field) = self.inspector_field_at(*pos) {
                    out.extend(self.inspector_press(field, *clicks, pos.y));
                } else if let Some(field) = self.field_at(*pos) {
                    self.begin_drag(field, pos.y);
                }
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(d) = self.drag.take() {
                    if d.changed {
                        out.push(UiAction::EndMerge);
                    }
                    self.repaint = true;
                }
                self.inspector_release(&mut out);
                self.rec_release(&mut out);
                self.cluster_release(&mut out);
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } if self.editing.is_none()
                && self.meter_editing.is_none()
                && self.drag.is_none()
                && !self.insp.is_editing()
                && !self.is_dragging_inspector()
                && !self.is_dragging_rec_input()
                && !self.is_dragging_cluster()
                && self.cl.edit.is_none() =>
            {
                if let Some(field) = self.field_at(*pos) {
                    self.wheel_field(field, wheel_steps(delta.y), modifiers.shift, &mut out);
                } else if let Some(part) = self.rec_hit(*pos) {
                    self.rec_wheel(part, wheel_steps(delta.y), modifiers.shift, &mut out);
                } else if self.cluster_wheel(*pos, wheel_steps(delta.y), &mut out) {
                } else if let Some(field) = self.inspector_field_at(*pos) {
                    self.inspector_wheel(field, wheel_steps(delta.y), modifiers.shift, &mut out);
                }
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

    /// What the button or readout at `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !self.bounds.contains(pos) {
            return None;
        }
        if let Some(b) = BUTTONS
            .into_iter()
            .find(|b| self.button_rect(*b).contains(pos))
        {
            return Some(Self::button_help(b, self.button_rect(b)));
        }
        if let Some(r) = self.override_rect()
            && r.contains(pos)
        {
            return Some(Help::new(
                r,
                "Some lanes hold a value you moved while the song plays: click to let them play again",
            ));
        }
        let [_, _, tempo, meter] = self.lcd_rects();
        if tempo.contains(pos) {
            return Some(Help::new(
                tempo,
                "Tempo in BPM: drag or scroll to change (Shift: fine), double-click to type",
            ));
        }
        if meter.contains(pos) {
            return Some(Help::new(
                meter,
                "Time signature: drag or scroll a number, double-click to type",
            ));
        }
        self.cluster_help(pos)
            .or_else(|| self.rec_help(pos))
            .or_else(|| self.inspector_help(pos))
    }

    fn button_help(b: TransportButton, rect: Rect) -> Help {
        let help =
            |text: &'static str, command: &'static str| Help::new(rect, text).with_command(command);
        match b {
            TransportButton::ReturnToZero => help("Return to the start", "transport.go_start"),
            TransportButton::Stop => help("Stop", "transport.stop"),
            TransportButton::Play => help("Play, or stop while playing", "transport.play_stop"),
            TransportButton::Record => help("Record into the armed tracks", "transport.record"),
            TransportButton::AutoLanes => help(
                "Auto-create lanes: a take also records the parameters you move, adding their lanes",
                "transport.auto_create_lanes",
            ),
            TransportButton::Loop => help("Loop the loop range on or off", "transport.loop"),
            TransportButton::Metronome => help("Metronome click on or off", "transport.metronome"),
        }
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if self.cl.edit.is_some() {
            self.readout_key(key, out);
            return;
        }
        if self.insp.is_editing() {
            self.inspector_key(key, out);
            return;
        }
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
                TransportButton::AutoLanes => self.state.auto_lanes.then_some(st.record),
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
        let counting = self.state.count_in > 0;
        sp::label(
            c,
            if counting { "COUNT" } else { "POS" },
            pos_r.x0 + 6.0,
            pos_r.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        if self.state.pre_count > 0 && !counting {
            sp::label(
                c,
                &format!("PRE {}", self.state.pre_count),
                pos_r.x1 - 8.0,
                pos_r.y0 + 10.0,
                6.5,
                t.text_muted,
                Align::End,
            );
        }
        // While the count-in runs the readout shows its beat; the position holds.
        let bbt = if counting {
            self.state.count_in.to_string()
        } else if let Some(s) = self.readout_typing(Readout::Pos) {
            format!("{s}_")
        } else {
            bbt
        };
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
        if let Some(r) = self.override_rect() {
            c.fill_with(RoundedRect::from_rect(r, 3.0), st.solo);
            sp::label(
                c,
                "OVERRIDE",
                r.center().x,
                r.y1 - 2.5,
                6.5,
                t.plate,
                Align::Center,
            );
        }
        let time = match self.readout_typing(Readout::Time) {
            Some(s) => format!("{s}_"),
            None => time,
        };
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
                pt(tempo_r.x1 - LCD_PAD, tempo_r.y1 - 7.0),
                TEMPO_SIZE,
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
        self.paint_cluster(c);
        self.paint_inspector(c, doc);
        self.paint_rec_input(c);
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
        TransportButton::AutoLanes => {
            // An automation curve with a small plus: a lane that appears by itself.
            let mut curve = BezPath::new();
            curve.move_to(pt(x - 6.0, y + 5.0));
            curve.line_to(pt(x - 3.0, y - 1.0));
            curve.line_to(pt(x, y + 3.0));
            curve.line_to(pt(x + 3.0, y - 0.5));
            c.stroke(&curve.into(), &Stroke::new(1.3), &col.into());
            c.fill_with(Rect::new(x + 1.0, y - 6.75, x + 6.0, y - 5.5), col);
            c.fill_with(Rect::new(x + 2.75, y - 8.5, x + 4.25, y - 3.75), col);
        }
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
