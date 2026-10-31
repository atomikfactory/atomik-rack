//! The second cluster of the transport bar, right of the four readouts: Rewind and Forward
//! (held, they repeat and speed up), tap tempo, the left and right locator readouts with their
//! go-to buttons, the click-level slider, and the Q Rec switch with its grid and the QUANTIZE
//! button.
//!
//! **Space.** The bar is tight: the recording-input strip needs about 998 px, and a clip
//! inspector wants the room too. The cluster is built of four groups in priority order (the
//! helpers, the locators, the click level, the quantize controls). It takes the room left after
//! the recording strip and the whole clip inspector; the groups that do not fit drop out from
//! the end, and an overflow button (`...`) takes their place. It opens a menu with every one of
//! these controls, so nothing is out of reach at 1000 px.
//!
//! The bar has no clock: the held buttons repeat from [`TransportBar::tick`], which the app
//! calls every frame with a monotonic time in seconds.

use super::*;
use crate::quantize_panel::{GRIDS, QuantizeAction};

/// Gap between two controls of a group.
const IN_GAP: f64 = 4.0;
/// Gap between two groups.
const GROUP_GAP: f64 = 10.0;
/// Gap between the readouts and the cluster.
pub(super) const CLUSTER_LEAD: f64 = 6.0;
const MORE_W: f64 = 22.0;
const SMALL_W: f64 = 30.0;
const TAP_W: f64 = 36.0;
const GO_W: f64 = 18.0;
const LOC_W: f64 = 78.0;
const CLICK_W: f64 = 70.0;
const QREC_W: f64 = 40.0;
const GRID_W: f64 = 54.0;
const QUANT_W: f64 = 58.0;
/// The seconds a held Rewind or Forward waits before it repeats.
pub const REPEAT_DELAY: f64 = 0.4;
/// The seconds between two repeats.
pub const REPEAT_EVERY: f64 = 0.1;
/// The click level a double-click on the slider goes back to.
const DEFAULT_CLICK: f32 = 0.5;

/// A control of the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterPart {
    /// Rewind (held: repeats).
    Rewind,
    /// Fast forward (held: repeats).
    Forward,
    /// Tap tempo.
    Tap,
    /// Go to the left locator.
    GoLeft,
    /// Go to the right locator.
    GoRight,
    /// The click-level slider.
    Click,
    /// The Q Rec switch.
    QRec,
    /// The Q Rec grid.
    Grid,
    /// Quantize the selection with the quantize panel's last settings.
    Quantize,
    /// The overflow button.
    More,
}

/// A readout that takes typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Readout {
    Pos,
    Time,
    Left,
    Right,
}

/// A typed entry in a readout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadoutEdit {
    pub which: Readout,
    pub text: String,
    pub fresh: bool,
}

/// A held Rewind or Forward.
#[derive(Debug, Clone, Copy)]
struct Repeat {
    forward: bool,
    /// Seconds the next repeat is due at (`None` until the first tick sees the press).
    next: Option<f64>,
    /// Bars moved by the repeats so far.
    moved: i64,
}

/// The cluster's own state.
#[derive(Debug, Default)]
pub(super) struct ClusterState {
    pub hover: Option<ClusterPart>,
    repeat: Option<Repeat>,
    /// The grid last picked (an index into [`GRIDS`]), kept while Q Rec is off.
    grid: usize,
    grid_drag: Option<StepDrag>,
    click_drag: bool,
    pub edit: Option<ReadoutEdit>,
}

impl ClusterState {
    /// The state a new bar starts with: the 1/16 grid chosen.
    pub(super) fn new() -> Self {
        Self {
            grid: 3,
            ..Self::default()
        }
    }
}

/// Where the controls are.
#[derive(Debug, Clone, Default)]
pub(super) struct ClusterLayout {
    pub parts: Vec<(ClusterPart, Rect)>,
    pub left: Option<Rect>,
    pub right: Option<Rect>,
    /// The right end of the cluster (the readouts' end when it is empty).
    pub end: f64,
}

/// How many bars a repeat moves after `moved` bars have been moved: one, then more.
pub fn repeat_step(moved: i64) -> i64 {
    match moved {
        ..20 => 1,
        20..80 => 4,
        _ => 16,
    }
}

/// The short text of grid `i` (`1/16T` for a triplet).
fn grid_label(i: usize) -> String {
    GRIDS[i.min(GRIDS.len() - 1)].0.replace(" triplet", "T")
}

/// The index of the grid of `ticks`.
fn grid_index(ticks: i64) -> Option<usize> {
    GRIDS.iter().position(|g| g.1 == ticks)
}

/// The group widths: helpers, locators, click level, quantize.
fn group_widths() -> [f64; 4] {
    [
        2.0 * SMALL_W + TAP_W + 2.0 * IN_GAP,
        2.0 * (GO_W + LOC_W) + 2.0 * IN_GAP + 2.0,
        CLICK_W,
        QREC_W + GRID_W + QUANT_W + 2.0 * IN_GAP,
    ]
}

impl TransportBar {
    /// The room the cluster may use: from after the readouts to before the clip inspector and
    /// the recording strip.
    fn cluster_budget(&self) -> (f64, f64) {
        let x0 = self.lcd_end() + CLUSTER_LEAD;
        // The recording strip keeps its own lead; next to the cluster a short one is enough.
        let rec = match self.rec_reserved() {
            r if r > 0.0 => r - 12.0,
            r => r,
        };
        let limit = self.bounds.x1 - super::rec_input::TAIL - rec - self.inspector_demand();
        (x0, limit)
    }

    pub(super) fn cluster_layout(&self) -> ClusterLayout {
        let (x0, limit) = self.cluster_budget();
        let widths = group_widths();
        let budget = limit - x0;
        let total: f64 = widths.iter().sum::<f64>() + GROUP_GAP * 3.0;
        let mut out = ClusterLayout {
            end: self.lcd_end(),
            ..ClusterLayout::default()
        };
        let shown = if budget >= total {
            4
        } else {
            let mut n = 0;
            let mut used = MORE_W;
            for w in widths {
                if used + GROUP_GAP + w > budget {
                    break;
                }
                used += GROUP_GAP + w;
                n += 1;
            }
            n
        };
        let more = shown < 4 && budget >= MORE_W;
        if shown == 0 && !more {
            return out;
        }
        let y = self.bounds.center().y;
        let boxed = |x: f64, w: f64, h: f64| Rect::new(x, y - h / 2.0, x + w, y + h / 2.0);
        let mut x = x0;
        for g in 0..shown {
            match g {
                0 => {
                    for (part, w) in [
                        (ClusterPart::Rewind, SMALL_W),
                        (ClusterPart::Forward, SMALL_W),
                        (ClusterPart::Tap, TAP_W),
                    ] {
                        out.parts.push((part, boxed(x, w, BUTTON_H)));
                        x += w + IN_GAP;
                    }
                    x -= IN_GAP;
                }
                1 => {
                    out.parts
                        .push((ClusterPart::GoLeft, boxed(x, GO_W, BUTTON_H)));
                    x += GO_W + IN_GAP;
                    out.left = Some(boxed(x, LOC_W, 28.0));
                    x += LOC_W + IN_GAP + 2.0;
                    out.parts
                        .push((ClusterPart::GoRight, boxed(x, GO_W, BUTTON_H)));
                    x += GO_W + IN_GAP;
                    out.right = Some(boxed(x, LOC_W, 28.0));
                    x += LOC_W;
                }
                2 => {
                    out.parts
                        .push((ClusterPart::Click, boxed(x, CLICK_W, 28.0)));
                    x += CLICK_W;
                }
                _ => {
                    for (part, w) in [
                        (ClusterPart::QRec, QREC_W),
                        (ClusterPart::Grid, GRID_W),
                        (ClusterPart::Quantize, QUANT_W),
                    ] {
                        let h = if part == ClusterPart::Grid {
                            28.0
                        } else {
                            BUTTON_H
                        };
                        out.parts.push((part, boxed(x, w, h)));
                        x += w + IN_GAP;
                    }
                    x -= IN_GAP;
                }
            }
            x += GROUP_GAP;
        }
        if more {
            out.parts
                .push((ClusterPart::More, boxed(x, MORE_W, BUTTON_H)));
            x += MORE_W;
        } else {
            x -= GROUP_GAP;
        }
        out.end = x;
        out
    }

    /// The rectangle of a cluster control (`None` while it is not shown).
    pub fn cluster_rect(&self, part: ClusterPart) -> Option<Rect> {
        self.cluster_layout()
            .parts
            .into_iter()
            .find(|(p, _)| *p == part)
            .map(|(_, r)| r)
    }

    /// The rectangle of the left (`false`) or right (`true`) locator readout.
    pub fn locator_rect(&self, right: bool) -> Option<Rect> {
        let l = self.cluster_layout();
        if right { l.right } else { l.left }
    }

    /// The rectangle of the position readout (POS).
    pub fn position_rect(&self) -> Rect {
        self.lcd_rects()[0]
    }

    /// The rectangle of the time readout (TIME).
    pub fn time_rect(&self) -> Rect {
        self.lcd_rects()[1]
    }

    /// The right end of the cluster, where the clip inspector starts after.
    pub(super) fn cluster_end(&self) -> f64 {
        self.cluster_layout().end
    }

    /// Hands the bar the metronome level shown by the click slider (0..=1).
    pub fn set_click_gain(&mut self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            DEFAULT_CLICK
        };
        if (gain - self.click_gain).abs() > f32::EPSILON {
            self.click_gain = gain;
            self.repaint = true;
        }
    }

    /// The click level shown.
    pub fn click_gain(&self) -> f32 {
        self.click_gain
    }

    /// The grid index (into [`GRIDS`]) the Q Rec selector shows.
    pub fn q_grid_index(&self) -> usize {
        self.grid_shown()
    }

    fn grid_shown(&self) -> usize {
        match self.state.quantize.and_then(grid_index) {
            Some(i) => i,
            None => self.cl.grid.min(GRIDS.len() - 1),
        }
    }

    /// A held Rewind or Forward is repeating.
    pub fn is_repeating(&self) -> bool {
        self.cl.repeat.is_some()
    }

    /// Advances the held Rewind or Forward to `now_seconds` (any monotonic clock): after a
    /// short delay it moves again every [`REPEAT_EVERY`], by a larger step the longer it has
    /// moved. Returns the moves to make.
    pub fn tick(&mut self, now_seconds: f64) -> Vec<UiAction> {
        let mut out = Vec::new();
        if self.state.following {
            self.cl.repeat = None;
            return out;
        }
        let Some(mut r) = self.cl.repeat else {
            return out;
        };
        let Some(mut next) = r.next else {
            r.next = Some(now_seconds + REPEAT_DELAY);
            self.cl.repeat = Some(r);
            return out;
        };
        // A long pause (a stalled frame) moves once, not as often as it was missed.
        let mut guard = 0;
        while now_seconds >= next && guard < 4 {
            let step = repeat_step(r.moved);
            r.moved += step;
            out.push(UiAction::Transport(TransportAction::Bars(if r.forward {
                step
            } else {
                -step
            })));
            next += REPEAT_EVERY;
            guard += 1;
        }
        if now_seconds >= next {
            next = now_seconds + REPEAT_EVERY;
        }
        r.next = Some(next);
        self.cl.repeat = Some(r);
        out
    }

    pub(super) fn cluster_hit(&self, pos: Point) -> Option<ClusterPart> {
        self.cluster_layout()
            .parts
            .into_iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(p, _)| p)
    }

    /// The readout with typed entry under `pos`.
    pub(super) fn readout_at(&self, pos: Point) -> Option<Readout> {
        let l = self.cluster_layout();
        let [pos_r, time_r, _, _] = self.lcd_rects();
        if pos_r.contains(pos) {
            Some(Readout::Pos)
        } else if time_r.contains(pos) {
            Some(Readout::Time)
        } else if l.left.is_some_and(|r| r.contains(pos)) {
            Some(Readout::Left)
        } else if l.right.is_some_and(|r| r.contains(pos)) {
            Some(Readout::Right)
        } else {
            None
        }
    }

    /// The hover state follows the pointer.
    pub(super) fn cluster_hover(&mut self, pos: Point) {
        let h = self.cluster_hit(pos);
        if h != self.cl.hover {
            self.cl.hover = h;
            self.repaint = true;
        }
    }

    /// The pointer left the bar: the hover goes and a held repeat stops.
    pub(super) fn cluster_leave(&mut self) {
        self.cl.repeat = None;
        if self.cl.hover.take().is_some() {
            self.repaint = true;
        }
    }

    pub(super) fn click_level_at(&self, x: f64) -> f32 {
        match self.cluster_rect(ClusterPart::Click) {
            Some(r) => {
                let (a, b) = (r.x0 + 6.0, r.x1 - 6.0);
                (((x - a) / (b - a)).clamp(0.0, 1.0) * 100.0).round() as f32 / 100.0
            }
            None => self.click_gain,
        }
    }

    fn quantize_request(&self, on: bool, grid: usize) -> UiAction {
        UiAction::SetInputQuantize(on.then(|| GRIDS[grid.min(GRIDS.len() - 1)].1))
    }

    /// A primary press on a cluster control.
    pub(super) fn cluster_press(
        &mut self,
        part: ClusterPart,
        pos: Point,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        match part {
            ClusterPart::Rewind | ClusterPart::Forward => {
                let forward = part == ClusterPart::Forward;
                out.push(UiAction::Transport(TransportAction::Bars(if forward {
                    1
                } else {
                    -1
                })));
                if !self.state.following {
                    self.cl.repeat = Some(Repeat {
                        forward,
                        next: None,
                        moved: 1,
                    });
                }
            }
            ClusterPart::Tap => out.push(UiAction::Transport(TransportAction::TapTempo)),
            ClusterPart::GoLeft => out.push(UiAction::Transport(TransportAction::GoLocator(false))),
            ClusterPart::GoRight => out.push(UiAction::Transport(TransportAction::GoLocator(true))),
            ClusterPart::Click => {
                if clicks >= 2 {
                    out.push(UiAction::SetMetronomeGain(DEFAULT_CLICK));
                } else {
                    self.cl.click_drag = true;
                    let v = self.click_level_at(pos.x);
                    out.push(UiAction::SetMetronomeGain(v));
                }
            }
            ClusterPart::QRec => {
                let on = self.state.quantize.is_none();
                out.push(self.quantize_request(on, self.grid_shown()));
            }
            ClusterPart::Grid => self.cl.grid_drag = Some(StepDrag::new(pos.y)),
            ClusterPart::Quantize => out.push(UiAction::Quantize(QuantizeAction::ApplyLast)),
            ClusterPart::More => out.push(UiAction::ContextMenu {
                target: crate::action::ContextTarget::TransportMore,
                at: pos,
            }),
        }
    }

    /// The pointer moved with a cluster drag held.
    pub(super) fn cluster_drag_to(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        if self.cl.click_drag {
            let v = self.click_level_at(pos.x);
            if (v - self.click_gain).abs() > f32::EPSILON {
                self.click_gain = v;
                self.repaint = true;
                out.push(UiAction::SetMetronomeGain(v));
            }
        }
        if let Some(d) = &mut self.cl.grid_drag {
            let n = d.update(pos.y, PX_PER_STEP);
            if n != 0 {
                let at = self.grid_shown();
                self.step_grid(at, n, out);
            }
        }
    }

    fn step_grid(&mut self, at: usize, notches: i32, out: &mut Vec<UiAction>) {
        let i = step_index(at, notches, GRIDS.len());
        if i != at {
            self.cl.grid = i;
            self.repaint = true;
            if self.state.quantize.is_some() {
                out.push(self.quantize_request(true, i));
            }
        }
    }

    /// The primary button went up: ends a held repeat, a slider or a grid drag. A click on the
    /// grid that did not drag moves to the next one.
    pub(super) fn cluster_release(&mut self, out: &mut Vec<UiAction>) {
        if self.cl.repeat.take().is_some() {
            self.repaint = true;
        }
        self.cl.click_drag = false;
        if let Some(d) = self.cl.grid_drag.take() {
            if !d.moved() {
                let at = self.grid_shown();
                let next = (at + 1) % GRIDS.len();
                self.cl.grid = next;
                if self.state.quantize.is_some() {
                    out.push(self.quantize_request(true, next));
                }
            }
            self.repaint = true;
        }
    }

    /// True while a cluster slider or grid drag is held.
    pub(super) fn is_dragging_cluster(&self) -> bool {
        self.cl.click_drag || self.cl.grid_drag.is_some() || self.cl.repeat.is_some()
    }

    /// One wheel event over a cluster control; true when it was one.
    pub(super) fn cluster_wheel(
        &mut self,
        pos: Point,
        notches: i32,
        out: &mut Vec<UiAction>,
    ) -> bool {
        match self.cluster_hit(pos) {
            Some(ClusterPart::Grid) => {
                let at = self.grid_shown();
                self.step_grid(at, notches, out);
                true
            }
            Some(ClusterPart::Click) => {
                let v = (self.click_gain + notches as f32 * 0.05).clamp(0.0, 1.0);
                let v = (v * 100.0).round() / 100.0;
                if (v - self.click_gain).abs() > f32::EPSILON {
                    self.click_gain = v;
                    self.repaint = true;
                    out.push(UiAction::SetMetronomeGain(v));
                }
                true
            }
            _ => false,
        }
    }

    /// What the cluster control at `pos` does, for its tooltip.
    pub(super) fn cluster_help(&self, pos: Point) -> Option<Help> {
        let l = self.cluster_layout();
        let help = |r: Rect, text: &'static str, command: Option<&'static str>| {
            let h = Help::new(r, text);
            match command {
                Some(c) => h.with_command(c),
                None => h,
            }
        };
        for (part, r) in &l.parts {
            if !r.contains(pos) {
                continue;
            }
            return Some(match part {
                ClusterPart::Rewind => help(
                    *r,
                    "Rewind by bars: hold to repeat, faster the longer it is held",
                    Some("transport.rewind_bar"),
                ),
                ClusterPart::Forward => help(
                    *r,
                    "Fast forward by bars: hold to repeat, faster the longer it is held",
                    Some("transport.forward_bar"),
                ),
                ClusterPart::Tap => help(
                    *r,
                    "Tap tempo: tap the beat (the average of the last taps sets the tempo)",
                    Some("transport.tap_tempo"),
                ),
                ClusterPart::GoLeft => help(*r, "Go to the left locator", None),
                ClusterPart::GoRight => help(*r, "Go to the right locator", None),
                ClusterPart::Click => help(
                    *r,
                    "Click level: drag or scroll, double-click for the default",
                    None,
                ),
                ClusterPart::QRec => help(
                    *r,
                    "Q Rec: snap the notes you record to the grid",
                    Some("transport.input_quantize"),
                ),
                ClusterPart::Grid => {
                    help(*r, "Q Rec grid: click, drag or scroll to change it", None)
                }
                ClusterPart::Quantize => help(
                    *r,
                    "Quantize the selected clips or notes with the quantize panel's last settings",
                    None,
                ),
                ClusterPart::More => help(*r, "More transport controls", None),
            });
        }
        if let Some(r) = l.left.filter(|r| r.contains(pos)) {
            return Some(help(
                r,
                "Left locator: double-click to type a position (bar.beat.tick)",
                None,
            ));
        }
        if let Some(r) = l.right.filter(|r| r.contains(pos)) {
            return Some(help(
                r,
                "Right locator: double-click to type a position (bar.beat.tick)",
                None,
            ));
        }
        None
    }

    /// Paints the cluster.
    pub(super) fn paint_cluster(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let l = self.cluster_layout();
        for (part, r) in &l.parts {
            let hover = self.cl.hover == Some(*part);
            match part {
                ClusterPart::Rewind | ClusterPart::Forward => {
                    let held = self
                        .cl
                        .repeat
                        .is_some_and(|rep| rep.forward == (*part == ClusterPart::Forward));
                    sp::button(c, st, *r, "", held.then_some(t.focus), hover);
                    paint_wind(c, r.center(), *part == ClusterPart::Forward, t.text);
                }
                ClusterPart::Tap => sp::button(c, st, *r, "TAP", None, hover),
                ClusterPart::GoLeft => sp::button(c, st, *r, "|<", None, hover),
                ClusterPart::GoRight => sp::button(c, st, *r, ">|", None, hover),
                ClusterPart::QRec => sp::button(
                    c,
                    st,
                    *r,
                    "Q",
                    self.state.quantize.is_some().then_some(st.record),
                    hover,
                ),
                ClusterPart::Quantize => sp::button(c, st, *r, "QUANT", None, hover),
                ClusterPart::More => sp::button(c, st, *r, "...", None, hover),
                ClusterPart::Grid => {
                    sp::lcd(c, st, *r, None);
                    sp::label(
                        c,
                        "GRID",
                        r.x0 + 6.0,
                        r.y0 + 10.0,
                        6.5,
                        t.text_muted,
                        Align::Start,
                    );
                    let color = if self.state.quantize.is_some() {
                        t.highlight
                    } else {
                        t.text_muted
                    };
                    c.text(
                        &TextRun::new(
                            grid_label(self.grid_shown()),
                            pt(r.x1 - 6.0, r.y1 - 6.0),
                            11.0,
                            color,
                        )
                        .weight(600)
                        .align(Align::End),
                    );
                }
                ClusterPart::Click => {
                    sp::lcd(c, st, *r, None);
                    sp::label(
                        c,
                        "CLICK",
                        r.x0 + 6.0,
                        r.y0 + 10.0,
                        6.5,
                        t.text_muted,
                        Align::Start,
                    );
                    let (a, b) = (r.x0 + 6.0, r.x1 - 6.0);
                    let y = r.y1 - 8.0;
                    sp::line(c, a, y, b, y, t.bevel_dark);
                    let x = a + (b - a) * f64::from(self.click_gain);
                    sp::line(c, a, y, x, y, t.highlight);
                    c.fill_with(Circle::new(pt(x, y), 3.5), t.highlight);
                }
            }
        }
        for (which, rect, label) in [(Readout::Left, l.left, "L"), (Readout::Right, l.right, "R")] {
            let Some(r) = rect else { continue };
            let typing = self
                .cl
                .edit
                .as_ref()
                .filter(|e| e.which == which)
                .map(|e| e.text.as_str());
            sp::lcd(c, st, r, typing.map(|_| t.focus));
            sp::label(
                c,
                label,
                r.x0 + 6.0,
                r.y0 + 10.0,
                6.5,
                t.text_muted,
                Align::Start,
            );
            let text = match typing {
                Some(s) => format!("{s}_"),
                None => format_bbt(
                    if which == Readout::Left {
                        self.loop_range.0
                    } else {
                        self.loop_range.1
                    },
                    &self.bars,
                ),
            };
            c.text(
                &TextRun::new(text, pt(r.x1 - 6.0, r.y1 - 7.0), 11.0, t.highlight)
                    .weight(600)
                    .align(Align::End),
            );
        }
    }
}

/// Two triangles, pointing left (rewind) or right (forward).
fn paint_wind(c: &mut dyn Canvas, ctr: Point, forward: bool, col: Color) {
    let s = if forward { 1.0 } else { -1.0 };
    for dx in [-4.5, 1.5] {
        let x = ctr.x + dx;
        let mut p = BezPath::new();
        p.move_to(pt(x - 3.0 * s, ctr.y - 5.0));
        p.line_to(pt(x - 3.0 * s, ctr.y + 5.0));
        p.line_to(pt(x + 3.0 * s, ctr.y));
        p.close_path();
        c.fill_with(p, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repeat_step_grows_with_the_distance() {
        assert_eq!(repeat_step(1), 1);
        assert_eq!(repeat_step(19), 1);
        assert_eq!(repeat_step(20), 4);
        assert_eq!(repeat_step(80), 16);
    }

    #[test]
    fn grid_labels_are_short() {
        assert_eq!(grid_label(3), "1/16");
        assert_eq!(grid_label(4), "1/16T");
        assert_eq!(grid_index(GRIDS[4].1), Some(4));
    }
}
