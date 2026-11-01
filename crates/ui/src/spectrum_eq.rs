//! The Spectrum EQ window: a real-time analyser of one console strip with its filters and EQ
//! drawn over it as a response curve whose points can be dragged.
//!
//! * **The analyser** shows what the strip hears after its EQ and before its fader: the app
//!   switches the strip's analyser tap on while the window is open, reads it, runs the FFT and
//!   hands the trace over ([`SpectrumEqView::set_trace`]). The master has the analyser only.
//! * **The curve** is [`atomik_dsp::strip_eq::StripCurve`]: the same sections the strip runs.
//!   The four EQ points (HF, HMF, LMF, LF) sit at their frequency and gain; the high-pass and
//!   low-pass corners sit on the curve.
//! * **Drags** emit `SetParam` commands with one merge id, so a whole drag is one undo step and
//!   records automation like a knob turn. A point moves in frequency and gain together (the
//!   frequency and the gain of a band go out as one batch every time, so they always merge); the
//!   constrain role locks the drag to its dominant axis, Alt-drag changes the Q of a bell, the
//!   reset role (Ctrl or Cmd click) puts the gain (or the corner) back to its default, and the
//!   wheel over a bell changes its Q. Escape cancels a drag.
//! * **Checkboxes** switch the EQ, the filters and the shelf-or-bell shape of the outer bands.
//! * **The strip** is chosen with the arrows; Follow keeps it on the selected strip.

use crate::action::UiAction;
use crate::analyser::{PLOT_HI_HZ, PLOT_LO_HZ, freq_x, level_y};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::{GestureClass, Role, Roles};
use crate::seq_paint::SeqTheme;
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_device_sdk::DeviceDescriptor;
use atomik_dsp::strip_eq::{StripBand, StripCurve};
use atomik_model::{Command, DeviceKey, RackDevice};
use vello::kurbo::{BezPath, Circle, Line, Point, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// The size a Spectrum EQ window opens with, in logical pixels.
pub const DEFAULT_SIZE: (f64, f64) = (760.0, 440.0);
/// The merge ids of this window start here (like the other views' ranges).
const MERGE_BASE: u64 = 16 << 40;
const BAR_H: f64 = 32.0;
/// The level range of the analyser, dB below full scale.
pub const RANGE_DB: f32 = 90.0;
/// The gain the EQ axis shows above and below its middle line, dB.
pub const EQ_SPAN_DB: f64 = 24.0;
/// How close the pointer must be to a point to take it.
const HIT_RADIUS: f64 = 10.0;
/// The drag distance before a constrained drag picks its axis.
const AXIS_LOCK_PX: f64 = 3.0;
/// Pixels of a vertical Alt-drag that double the Q.
const Q_PX_PER_OCTAVE: f64 = 60.0;
/// The change of Q per wheel notch, as a factor.
const Q_WHEEL_STEP: f64 = 1.12;
const GRID_FREQS: [f64; 10] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0,
];

/// A strip the window can show.
#[derive(Debug, Clone, PartialEq)]
pub struct StripChoice {
    /// The strip's device.
    pub key: DeviceKey,
    /// The name its plate shows.
    pub name: String,
    /// The master section: analyser only.
    pub master: bool,
}

/// What the window draws for one strip: the values of its filter and EQ params.
#[derive(Debug, Clone)]
pub struct StripEqState {
    /// The strip.
    pub key: DeviceKey,
    /// Its name.
    pub name: String,
    /// The master section (no EQ to draw).
    pub master: bool,
    /// The strip's descriptor (ranges and defaults of the params).
    pub desc: &'static DeviceDescriptor,
    /// The sample rate the curve is drawn for.
    pub sample_rate: f64,
    curve: StripCurve,
    hpf_on: bool,
    lpf_on: bool,
}

/// The two filters and the four bands, as the points are numbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Band(usize),
    Hpf,
    Lpf,
}

/// The band prefixes of the strip's params, in the order HF, HMF, LMF, LF.
const BAND_KEYS: [&str; 4] = ["eq.hf", "eq.hmf", "eq.lmf", "eq.lf"];
const BAND_NAMES: [&str; 4] = ["HF", "HMF", "LMF", "LF"];

impl StripEqState {
    /// Reads the strip's params from its device in the document.
    pub fn read(
        choice: &StripChoice,
        desc: &'static DeviceDescriptor,
        dev: &RackDevice,
        sample_rate: f64,
    ) -> Self {
        let get = |key: &str| {
            desc.param_by_key(key)
                .map_or(0.0, |p| crate::widget::current_value(dev, p))
        };
        let band = |i: usize| {
            let k = BAND_KEYS[i];
            StripBand {
                gain_db: f64::from(get(&format!("{k}.gain"))),
                freq: f64::from(get(&format!("{k}.freq"))),
                q: if desc.param_by_key(&format!("{k}.q")).is_some() {
                    f64::from(get(&format!("{k}.q")))
                } else {
                    0.0
                },
                bell: get(&format!("{k}.bell")) >= 0.5,
            }
        };
        let (hpf_on, lpf_on) = (get("hpf.on") >= 0.5, get("lpf.on") >= 0.5);
        let curve = StripCurve {
            sample_rate,
            eq_on: get("eq.on") >= 0.5,
            hpf: hpf_on.then(|| f64::from(get("hpf.freq"))),
            lpf: lpf_on.then(|| f64::from(get("lpf.freq"))),
            hf: band(0),
            hmf: band(1),
            lmf: band(2),
            lf: band(3),
        };
        Self {
            key: choice.key.clone(),
            name: choice.name.clone(),
            master: choice.master,
            desc,
            sample_rate,
            curve,
            hpf_on,
            lpf_on,
        }
    }

    /// The drawn curve.
    pub fn curve(&self) -> &StripCurve {
        &self.curve
    }

    fn band(&self, i: usize) -> &StripBand {
        match i {
            0 => &self.curve.hf,
            1 => &self.curve.hmf,
            2 => &self.curve.lmf,
            _ => &self.curve.lf,
        }
    }

    /// The corner of the filter, shown even while the filter is off.
    fn filter_hz(&self, part: Part) -> f64 {
        let key = if part == Part::Hpf {
            "hpf.freq"
        } else {
            "lpf.freq"
        };
        let from = match part {
            Part::Hpf => self.curve.hpf,
            _ => self.curve.lpf,
        };
        from.unwrap_or_else(|| {
            self.desc
                .param_by_key(key)
                .map_or(1_000.0, |p| f64::from(p.default))
        })
    }
}

/// A control of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    EqOn,
    Hpf,
    Lpf,
    HfBell,
    LfBell,
}

impl Check {
    const ALL: [Check; 5] = [
        Check::EqOn,
        Check::Hpf,
        Check::Lpf,
        Check::HfBell,
        Check::LfBell,
    ];

    fn label(self) -> &'static str {
        match self {
            Check::EqOn => "EQ",
            Check::Hpf => "HPF",
            Check::Lpf => "LPF",
            Check::HfBell => "HF BELL",
            Check::LfBell => "LF BELL",
        }
    }

    fn width(self) -> f64 {
        14.0 + self.label().len() as f64 * 6.4 + 12.0
    }

    fn param(self) -> &'static str {
        match self {
            Check::EqOn => "eq.on",
            Check::Hpf => "hpf.on",
            Check::Lpf => "lpf.on",
            Check::HfBell => "eq.hf.bell",
            Check::LfBell => "eq.lf.bell",
        }
    }

    fn checked(self, s: &StripEqState) -> bool {
        match self {
            Check::EqOn => s.curve.eq_on,
            Check::Hpf => s.hpf_on,
            Check::Lpf => s.lpf_on,
            Check::HfBell => s.curve.hf.bell,
            Check::LfBell => s.curve.lf.bell,
        }
    }

    fn help(self) -> &'static str {
        match self {
            Check::EqOn => "Switch the strip's four-band EQ on or off",
            Check::Hpf => "Switch the strip's high-pass filter on or off",
            Check::Lpf => "Switch the strip's low-pass filter on or off",
            Check::HfBell => "The high band is a bell instead of a shelf",
            Check::LfBell => "The low band is a bell instead of a shelf",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Prev,
    Next,
    Follow,
    Check(Check),
    Point(Part),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Freq,
    Gain,
}

/// A drag of a point in progress.
#[derive(Debug, Clone)]
struct Drag {
    part: Part,
    merge: u64,
    /// Alt: the drag changes Q instead of moving the point.
    q_mode: bool,
    origin: Point,
    grab: Vec2,
    axis: Option<Axis>,
    start: (f64, f64, f64),
    last: (f64, f64, f64),
    emitted: bool,
}

/// The Spectrum EQ view (see the module docs).
#[derive(Debug, Clone)]
pub struct SpectrumEqView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    roles: Roles,
    strips: Vec<StripChoice>,
    follow: bool,
    state: Option<StripEqState>,
    trace: Vec<f32>,
    peaks: Vec<f32>,
    hover: Option<Hit>,
    drag: Option<Drag>,
    next_merge: u64,
    repaint: bool,
    focused: bool,
}

impl SpectrumEqView {
    /// A view with nothing to show.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            scale: 1.0,
            roles: Roles::defaults(crate::keymap::Os::host()),
            strips: Vec::new(),
            follow: true,
            state: None,
            trace: Vec::new(),
            peaks: Vec::new(),
            hover: None,
            drag: None,
            next_merge: MERGE_BASE,
            repaint: true,
            focused: false,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Switches to `theme`.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// The modifier roles in force.
    pub fn set_roles(&mut self, roles: Roles) {
        self.roles = roles;
    }

    /// Where the view is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Where the view is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The display scale factor (the view paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Whether the window has the keyboard focus (Escape cancels a drag only then).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The strips the arrows step through, in order.
    pub fn set_strips(&mut self, strips: Vec<StripChoice>) {
        if self.strips != strips {
            self.strips = strips;
            self.repaint = true;
        }
    }

    /// The strips the arrows step through.
    pub fn strips(&self) -> &[StripChoice] {
        &self.strips
    }

    /// Whether the window follows the selected strip.
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Sets Follow Selection.
    pub fn set_follow(&mut self, follow: bool) {
        if self.follow != follow {
            self.follow = follow;
            self.repaint = true;
        }
    }

    /// The strip shown and its values (`None`: no strip).
    pub fn set_state(&mut self, state: Option<StripEqState>) {
        // A drag keeps the values it started from while it runs: the model answers each step.
        self.state = state;
        self.repaint = true;
    }

    /// The strip shown.
    pub fn state(&self) -> Option<&StripEqState> {
        self.state.as_ref()
    }

    /// The spectrum trace in dB per column (the log frequency axis), and the held peaks (empty
    /// without hold).
    pub fn set_trace(&mut self, trace: &[f32], peaks: &[f32]) {
        self.trace.clear();
        self.trace.extend_from_slice(trace);
        self.peaks.clear();
        self.peaks.extend_from_slice(peaks);
        self.repaint = true;
    }

    /// Forgets the trace (another strip, or the analyser stopped).
    pub fn clear_trace(&mut self) {
        if !self.trace.is_empty() || !self.peaks.is_empty() {
            self.trace.clear();
            self.peaks.clear();
            self.repaint = true;
        }
    }

    /// The trace shown.
    pub fn trace(&self) -> &[f32] {
        &self.trace
    }

    /// Whether the view has something to paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// A drag is running.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    // ------------------------------------------------------------------ layout

    /// The plot (logical pixels).
    pub fn plot(&self) -> Rect {
        let b = self.bounds;
        let x0 = b.x0 + 40.0;
        let y0 = b.y0 + BAR_H + 10.0;
        Rect::new(
            x0,
            y0,
            (b.x1 - 44.0).max(x0 + 1.0),
            (b.y1 - 22.0).max(y0 + 1.0),
        )
    }

    fn prev_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x0 + 8.0, b.y0 + 5.0, b.x0 + 30.0, b.y0 + 27.0)
    }

    fn name_rect(&self) -> Rect {
        let p = self.prev_rect();
        Rect::new(p.x1 + 4.0, p.y0, p.x1 + 154.0, p.y1)
    }

    fn next_rect(&self) -> Rect {
        let n = self.name_rect();
        Rect::new(n.x1 + 4.0, n.y0, n.x1 + 26.0, n.y1)
    }

    fn follow_rect(&self) -> Rect {
        let n = self.next_rect();
        Rect::new(n.x1 + 12.0, n.y0, n.x1 + 12.0 + 96.0, n.y1)
    }

    /// The checkboxes, from the right edge, in the order of [`Check::ALL`].
    fn check_rects(&self) -> Vec<(Check, Rect)> {
        let b = self.bounds;
        let mut x = b.x1 - 8.0;
        let mut out: Vec<(Check, Rect)> = Vec::new();
        for c in Check::ALL.iter().rev() {
            let w = c.width();
            out.push((*c, Rect::new(x - w, b.y0 + 5.0, x, b.y0 + 27.0)));
            x -= w + 6.0;
        }
        out.reverse();
        out
    }

    fn freq_of_x(&self, x: f64) -> f64 {
        let plot = self.plot();
        let t = ((x - plot.x0) / plot.width()).clamp(0.0, 1.0);
        PLOT_LO_HZ * (PLOT_HI_HZ / PLOT_LO_HZ).powf(t)
    }

    fn gain_y(&self, db: f64) -> f64 {
        let plot = self.plot();
        plot.center().y - db / EQ_SPAN_DB * plot.height() / 2.0
    }

    fn gain_of_y(&self, y: f64) -> f64 {
        let plot = self.plot();
        (plot.center().y - y) / (plot.height() / 2.0) * EQ_SPAN_DB
    }

    /// Where a point sits in the plot.
    fn point_pos(&self, part: PointId) -> Option<Point> {
        let s = self.state.as_ref().filter(|s| !s.master)?;
        let plot = self.plot();
        Some(match part.0 {
            Part::Band(i) => {
                let b = s.band(i);
                pt(freq_x(plot, b.freq), self.gain_y(b.gain_db))
            }
            filter => {
                let hz = s.filter_hz(filter);
                pt(freq_x(plot, hz), self.gain_y(s.curve.filters_db(hz)))
            }
        })
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        if self.state.as_ref().is_some_and(|s| !s.master) {
            let mut best: Option<(f64, Part)> = None;
            for part in [
                Part::Band(0),
                Part::Band(1),
                Part::Band(2),
                Part::Band(3),
                Part::Hpf,
                Part::Lpf,
            ] {
                if let Some(p) = self.point_pos(PointId(part)) {
                    let d = (p - pos).hypot();
                    if d <= HIT_RADIUS && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, part));
                    }
                }
            }
            if let Some((_, part)) = best {
                return Some(Hit::Point(part));
            }
            for (c, r) in self.check_rects() {
                if r.contains(pos) {
                    return Some(Hit::Check(c));
                }
            }
        }
        if self.prev_rect().contains(pos) {
            return Some(Hit::Prev);
        }
        if self.next_rect().contains(pos) {
            return Some(Hit::Next);
        }
        if self.follow_rect().contains(pos) {
            return Some(Hit::Follow);
        }
        None
    }

    /// The tooltip of what is under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let hit = self.hit(pos)?;
        let (rect, text): (Rect, String) = match hit {
            Hit::Prev => (self.prev_rect(), "The previous strip".into()),
            Hit::Next => (self.next_rect(), "The next strip".into()),
            Hit::Follow => (
                self.follow_rect(),
                "Show the strip that is selected in the mixer or the arrangement".into(),
            ),
            Hit::Check(c) => (
                self.check_rects().into_iter().find(|(k, _)| *k == c)?.1,
                c.help().into(),
            ),
            Hit::Point(part) => {
                let p = self.point_pos(PointId(part))?;
                let what = match part {
                    Part::Band(i) => format!(
                        "{} band: drag to set frequency and gain (Shift keeps one axis, Alt-drag \
                         changes Q, Ctrl-click resets the gain)",
                        BAND_NAMES[i]
                    ),
                    Part::Hpf => {
                        "High-pass corner: drag to set the frequency (Ctrl-click resets)".into()
                    }
                    Part::Lpf => {
                        "Low-pass corner: drag to set the frequency (Ctrl-click resets)".into()
                    }
                };
                (Rect::new(p.x - 8.0, p.y - 8.0, p.x + 8.0, p.y + 8.0), what)
            }
        };
        Some(Help::new(rect, text))
    }

    // ------------------------------------------------------------------ input

    /// Handles an event in the window's logical coordinates.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => self.press(*pos, *modifiers),
            UiEvent::PointerMove { pos, modifiers } => {
                if self.drag.is_some() {
                    return self.drag_to(*pos, *modifiers);
                }
                let hover = self.hit(*pos);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint = true;
                }
                Vec::new()
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                if self.drag.take().is_some() {
                    self.repaint = true;
                    return vec![UiAction::EndMerge];
                }
                Vec::new()
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint = true;
                }
                Vec::new()
            }
            UiEvent::Wheel { delta, pos, .. } => self.wheel(delta.y, *pos),
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } if self.drag.is_some() => self.cancel_drag(),
            UiEvent::Resize(size) => {
                self.set_bounds(Rect::new(0.0, 0.0, size.width, size.height));
                Vec::new()
            }
            UiEvent::ScaleChanged(s) => {
                self.set_scale(*s);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn params(&self, part: Part) -> (String, Option<String>, Option<String>) {
        match part {
            Part::Band(i) => {
                let k = BAND_KEYS[i];
                (
                    format!("{k}.freq"),
                    Some(format!("{k}.gain")),
                    (i == 1 || i == 2).then(|| format!("{k}.q")),
                )
            }
            Part::Hpf => ("hpf.freq".into(), None, None),
            Part::Lpf => ("lpf.freq".into(), None, None),
        }
    }

    fn param_cmd(key: &DeviceKey, param: &str, value: f32, merge: Option<u64>) -> Command {
        Command::SetParam {
            key: key.clone(),
            param: param.to_string(),
            value,
            merge,
        }
    }

    /// The command that sets a point's values: the frequency and the gain of a band as one batch
    /// (always both, so every step of a drag merges), a filter's frequency, or the Q alone.
    fn set_command(&self, drag: &Drag, v: (f64, f64, f64)) -> Option<Command> {
        let s = self.state.as_ref()?;
        let (freq_key, gain_key, q_key) = self.params(drag.part);
        let clamp = |key: &str, v: f64| {
            s.desc
                .param_by_key(key)
                .map_or(v as f32, |p| p.clamp(v as f32))
        };
        let merge = Some(drag.merge);
        if drag.q_mode {
            let key = q_key?;
            return Some(Self::param_cmd(&s.key, &key, clamp(&key, v.2), merge));
        }
        let freq = Self::param_cmd(&s.key, &freq_key, clamp(&freq_key, v.0), merge);
        match gain_key {
            Some(g) => Some(Command::Batch(vec![
                freq,
                Self::param_cmd(&s.key, &g, clamp(&g, v.1), merge),
            ])),
            None => Some(freq),
        }
    }

    fn press(&mut self, pos: Point, modifiers: Modifiers) -> Vec<UiAction> {
        let Some(hit) = self.hit(pos) else {
            return Vec::new();
        };
        self.repaint = true;
        match hit {
            Hit::Prev => vec![UiAction::SpectrumEqStep(-1)],
            Hit::Next => vec![UiAction::SpectrumEqStep(1)],
            Hit::Follow => vec![UiAction::SpectrumEqFollow(!self.follow)],
            Hit::Check(c) => {
                let Some(s) = &self.state else {
                    return Vec::new();
                };
                let value = if c.checked(s) { 0.0 } else { 1.0 };
                vec![UiAction::Command(Self::param_cmd(
                    &s.key,
                    c.param(),
                    value,
                    None,
                ))]
            }
            Hit::Point(part) => self.press_point(part, pos, modifiers),
        }
    }

    fn press_point(&mut self, part: Part, pos: Point, modifiers: Modifiers) -> Vec<UiAction> {
        let Some(s) = &self.state else {
            return Vec::new();
        };
        let (freq_key, gain_key, _) = self.params(part);
        if self
            .roles
            .is_active(Role::ResetParam, modifiers, GestureClass::ClickControl)
        {
            // The reset role: the gain (a filter: the corner) back to its default.
            let key = gain_key.unwrap_or(freq_key);
            let Some(default) = s.desc.param_by_key(&key).map(|p| p.default) else {
                return Vec::new();
            };
            return vec![UiAction::Command(Self::param_cmd(
                &s.key, &key, default, None,
            ))];
        }
        let (freq, gain, q) = match part {
            Part::Band(i) => {
                let b = s.band(i);
                (b.freq, b.gain_db, b.q)
            }
            filter => (s.filter_hz(filter), 0.0, 0.0),
        };
        let at = self.point_pos(PointId(part)).unwrap_or(pos);
        let merge = self.next_merge;
        self.next_merge += 1;
        self.drag = Some(Drag {
            part,
            merge,
            q_mode: modifiers.alt && matches!(part, Part::Band(1 | 2)),
            origin: pos,
            grab: at - pos,
            axis: None,
            start: (freq, gain, q),
            last: (freq, gain, q),
            emitted: false,
        });
        Vec::new()
    }

    fn drag_to(&mut self, pos: Point, modifiers: Modifiers) -> Vec<UiAction> {
        let Some(mut d) = self.drag.take() else {
            return Vec::new();
        };
        let constrain = self
            .roles
            .is_active(Role::Constrain, modifiers, GestureClass::DragItem);
        let mut next = d.last;
        if d.q_mode {
            let up = d.origin.y - pos.y;
            next.2 = d.start.2 * 2f64.powf(up / Q_PX_PER_OCTAVE);
        } else {
            let (dx, dy) = (pos.x - d.origin.x, pos.y - d.origin.y);
            if constrain {
                if d.axis.is_none() && dx.hypot(dy) >= AXIS_LOCK_PX {
                    d.axis = Some(if dx.abs() >= dy.abs() {
                        Axis::Freq
                    } else {
                        Axis::Gain
                    });
                }
            } else {
                d.axis = None;
            }
            let target = pos + d.grab;
            let band = matches!(d.part, Part::Band(_));
            if d.axis != Some(Axis::Gain) {
                next.0 = self.freq_of_x(target.x);
            } else {
                next.0 = d.start.0;
            }
            if band && d.axis != Some(Axis::Freq) {
                next.1 = (self.gain_of_y(target.y) * 10.0).round() / 10.0;
            } else {
                next.1 = d.start.1;
            }
        }
        let mut out = Vec::new();
        if next != d.last || !d.emitted {
            let moved = next != d.start || d.emitted;
            if moved && let Some(c) = self.set_command(&d, next) {
                out.push(UiAction::Command(c));
                d.emitted = true;
            }
            d.last = next;
        }
        self.drag = Some(d);
        self.repaint = true;
        out
    }

    fn cancel_drag(&mut self) -> Vec<UiAction> {
        let Some(d) = self.drag.take() else {
            return Vec::new();
        };
        self.repaint = true;
        let mut out = Vec::new();
        if d.emitted
            && let Some(c) = self.set_command(&d, d.start)
        {
            out.push(UiAction::Command(c));
        }
        out.push(UiAction::EndMerge);
        out
    }

    fn wheel(&mut self, notches: f64, pos: Point) -> Vec<UiAction> {
        let Some(Hit::Point(Part::Band(i @ (1 | 2)))) = self.hit(pos) else {
            return Vec::new();
        };
        let Some(s) = &self.state else {
            return Vec::new();
        };
        let key = format!("{}.q", BAND_KEYS[i]);
        let q = s.band(i).q * Q_WHEEL_STEP.powf(notches.signum() * notches.abs().min(3.0));
        let Some(p) = s.desc.param_by_key(&key) else {
            return Vec::new();
        };
        vec![UiAction::Command(Self::param_cmd(
            &s.key,
            &key,
            p.clamp(q as f32),
            None,
        ))]
    }

    // ------------------------------------------------------------------ paint

    /// Paints the window (physical pixels).
    pub fn paint(&self, c: &mut dyn Canvas) {
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        self.paint_logical(c);
        c.pop_transform();
    }

    fn text(c: &mut dyn Canvas, theme: &Theme, s: &str, at: Point, align: Align) {
        c.text(&TextRun::new(s, at, 9.0, theme.text_muted).align(align));
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(b, self.st.background);
        c.fill_with(Rect::new(b.x0, b.y0, b.x1, b.y0 + BAR_H), self.st.header);
        c.fill_with(
            Rect::new(b.x0, b.y0 + BAR_H, b.x1, b.y0 + BAR_H + 1.0),
            self.st.separator,
        );
        self.paint_bar(c);
        let plot = self.plot();
        crate::seq_paint::lcd(c, &self.st, plot, None);
        c.push_clip(&plot.into());
        self.paint_grid(c, plot);
        self.paint_trace(c, plot);
        if let Some(s) = self.state.as_ref().filter(|s| !s.master) {
            self.paint_curve(c, plot, s);
        }
        c.pop_clip();
        self.paint_axes(c, plot);
        if let Some(s) = self.state.as_ref().filter(|s| !s.master) {
            self.paint_points(c, s);
        }
        if self.state.is_none() {
            c.text(
                &TextRun::new("No strip to show", plot.center(), 12.0, t.text_muted)
                    .align(Align::Center),
            );
        } else if self.trace.is_empty() {
            c.text(
                &TextRun::new(
                    "Play to see the spectrum",
                    pt(plot.center().x, plot.y1 - 14.0),
                    10.0,
                    t.text_muted,
                )
                .align(Align::Center),
            );
        }
    }

    fn paint_bar(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let hovered = |h: Hit| self.hover == Some(h);
        crate::seq_paint::button(c, &self.st, self.prev_rect(), "<", None, hovered(Hit::Prev));
        crate::seq_paint::button(c, &self.st, self.next_rect(), ">", None, hovered(Hit::Next));
        let name = self.name_rect();
        crate::seq_paint::lcd(c, &self.st, name, None);
        let label = self.state.as_ref().map_or("-", |s| {
            if s.name.is_empty() {
                "-"
            } else {
                s.name.as_str()
            }
        });
        c.push_clip(&name.into());
        c.text(
            &TextRun::new(
                label,
                pt(name.center().x, name.center().y + 4.0),
                11.0,
                t.text,
            )
            .align(Align::Center)
            .weight(600),
        );
        c.pop_clip();
        self.check(
            c,
            self.follow_rect(),
            "FOLLOW SELECTION",
            self.follow,
            hovered(Hit::Follow),
            true,
        );
        if self.state.as_ref().is_some_and(|s| !s.master) {
            for (chk, r) in self.check_rects() {
                let on = self.state.as_ref().is_some_and(|s| chk.checked(s));
                self.check(c, r, chk.label(), on, hovered(Hit::Check(chk)), true);
            }
        }
    }

    fn check(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        label: &str,
        on: bool,
        hover: bool,
        enabled: bool,
    ) {
        let t = &self.st.base;
        let bx = Rect::new(r.x0, r.center().y - 6.0, r.x0 + 12.0, r.center().y + 6.0);
        let face = if hover {
            mix(t.button, t.text, 0.12)
        } else {
            t.button
        };
        c.fill_with(RoundedRect::from_rect(bx, 2.0), face);
        c.stroke_with(
            RoundedRect::from_rect(bx.inset(-0.5), 2.0),
            1.0,
            t.bevel_dark,
        );
        if on {
            c.fill_with(RoundedRect::from_rect(bx.inset(-3.0), 1.0), t.highlight);
        }
        let ink = if enabled { t.text } else { t.text_muted };
        c.text(&TextRun::new(
            label,
            pt(bx.x1 + 5.0, r.center().y + 3.5),
            9.0,
            ink,
        ));
    }

    fn paint_grid(&self, c: &mut dyn Canvas, plot: Rect) {
        let t = &self.st.base;
        let alpha = |a: f32| t.ink.with_alpha(a);
        let mut db = 0.0f32;
        while db <= RANGE_DB + 0.01 {
            let y = level_y(plot, -db, RANGE_DB).round() + 0.5;
            c.stroke_with(
                Line::new(pt(plot.x0, y), pt(plot.x1, y)),
                1.0,
                alpha(if db == 0.0 { 0.2 } else { 0.06 }),
            );
            db += 10.0;
        }
        for f in GRID_FREQS {
            let x = freq_x(plot, f).round() + 0.5;
            let major = matches!(f as u32, 100 | 1000 | 10_000);
            c.stroke_with(
                Line::new(pt(x, plot.y0), pt(x, plot.y1)),
                1.0,
                alpha(if major { 0.14 } else { 0.05 }),
            );
        }
        if self.state.as_ref().is_some_and(|s| !s.master) {
            // The EQ's own zero line.
            let y = self.gain_y(0.0).round() + 0.5;
            c.stroke_with(Line::new(pt(plot.x0, y), pt(plot.x1, y)), 1.0, alpha(0.3));
        }
    }

    fn paint_trace(&self, c: &mut dyn Canvas, plot: Rect) {
        let n = self.trace.len();
        if n < 2 {
            return;
        }
        let accent = self.st.base.highlight;
        let x_of = |i: usize| plot.x0 + (i as f64 + 0.5) / n as f64 * plot.width();
        let line = |values: &[f32]| {
            let mut path = BezPath::new();
            for (i, v) in values.iter().enumerate() {
                let p = pt(x_of(i), level_y(plot, *v, RANGE_DB));
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            path
        };
        let mut fill = line(&self.trace);
        fill.line_to(pt(x_of(n - 1), plot.y1));
        fill.line_to(pt(x_of(0), plot.y1));
        fill.close_path();
        c.fill_with(fill, accent.with_alpha(0.22));
        c.stroke_with(
            line(&self.trace),
            1.5,
            mix(accent, Color::WHITE, 0.3).with_alpha(0.9),
        );
        if self.peaks.len() == n {
            c.stroke_with(line(&self.peaks), 1.0, self.st.base.ink.with_alpha(0.4));
        }
    }

    fn paint_curve(&self, c: &mut dyn Canvas, plot: Rect, s: &StripEqState) {
        let steps = (plot.width() / 2.0).max(8.0) as usize;
        let mut path = BezPath::new();
        for i in 0..=steps {
            let x = plot.x0 + plot.width() * i as f64 / steps as f64;
            let db = s.curve.response_db(self.freq_of_x(x));
            let p = pt(x, self.gain_y(db.clamp(-60.0, 60.0)));
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        let zero = self.gain_y(0.0);
        let mut fill = path.clone();
        fill.line_to(pt(plot.x1, zero));
        fill.line_to(pt(plot.x0, zero));
        fill.close_path();
        let ink = self.st.base.text;
        c.fill_with(fill, ink.with_alpha(0.08));
        c.stroke_with(path, 2.0, ink.with_alpha(0.95));
    }

    fn paint_axes(&self, c: &mut dyn Canvas, plot: Rect) {
        let t = &self.st.base;
        for f in [
            20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0,
        ] {
            let label = match f as u32 {
                1_000 => "1k".to_string(),
                2_000 => "2k".to_string(),
                5_000 => "5k".to_string(),
                10_000 => "10k".to_string(),
                20_000 => "20k".to_string(),
                v => v.to_string(),
            };
            Self::text(
                c,
                t,
                &label,
                pt(
                    freq_x(plot, f).clamp(plot.x0 + 6.0, plot.x1 - 6.0),
                    plot.y1 + 12.0,
                ),
                Align::Center,
            );
        }
        let mut db = 0.0f32;
        while db <= RANGE_DB + 0.01 {
            let text = if db == 0.0 {
                "0".to_string()
            } else {
                format!("-{db:.0}")
            };
            Self::text(
                c,
                t,
                &text,
                pt(plot.x0 - 4.0, level_y(plot, -db, RANGE_DB) + 3.0),
                Align::End,
            );
            db += 10.0;
        }
        if self.state.as_ref().is_some_and(|s| !s.master) {
            for g in [-24, -12, 0, 12, 24] {
                let text = if g > 0 {
                    format!("+{g}")
                } else {
                    g.to_string()
                };
                Self::text(
                    c,
                    t,
                    &text,
                    pt(plot.x1 + 4.0, self.gain_y(f64::from(g)) + 3.0),
                    Align::Start,
                );
            }
        }
    }

    fn paint_points(&self, c: &mut dyn Canvas, s: &StripEqState) {
        let t = &self.st.base;
        let accent = t.highlight;
        let parts = [
            Part::Band(0),
            Part::Band(1),
            Part::Band(2),
            Part::Band(3),
            Part::Hpf,
            Part::Lpf,
        ];
        for part in parts {
            let Some(p) = self.point_pos(PointId(part)) else {
                continue;
            };
            let (on, label) = match part {
                Part::Band(i) => (s.curve.eq_on, BAND_NAMES[i]),
                Part::Hpf => (s.hpf_on, "HPF"),
                Part::Lpf => (s.lpf_on, "LPF"),
            };
            let hot = self.hover == Some(Hit::Point(part))
                || self.drag.as_ref().is_some_and(|d| d.part == part);
            let r = if hot { 7.5 } else { 6.0 };
            let ring = if on { accent } else { t.text_muted };
            c.fill_with(Circle::new(p, r), t.lcd);
            c.stroke_with(Circle::new(p, r), 2.0, ring);
            if on {
                c.fill_with(Circle::new(p, r - 3.5), accent);
            }
            c.text(
                &TextRun::new(
                    label,
                    pt(p.x, p.y - r - 4.0),
                    9.0,
                    if on { t.text } else { t.text_muted },
                )
                .align(Align::Center),
            );
        }
    }
}

/// A point of the curve, for tests and hosts that ask where it is.
#[derive(Debug, Clone, Copy)]
pub struct PointId(Part);

impl PointId {
    /// The EQ band 0..4 in the order HF, HMF, LMF, LF.
    pub fn band(i: usize) -> Self {
        Self(Part::Band(i.min(3)))
    }
    /// The high-pass corner.
    pub const HPF: Self = Self(Part::Hpf);
    /// The low-pass corner.
    pub const LPF: Self = Self(Part::Lpf);
}

impl SpectrumEqView {
    /// Where a point is, for hosts and tests.
    pub fn point(&self, id: PointId) -> Option<Point> {
        self.point_pos(id)
    }
}
