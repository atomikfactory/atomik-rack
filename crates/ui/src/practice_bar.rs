//! The practice bar: one row of controls under the toolbar of the Tab Editor and the Chord Sheet
//! panes (the pane hosts it, see [`crate::TabEditorView`] and [`crate::ChordSheetView`]).
//!
//! It holds no practice state of its own: the host gives it a [`PracticeState`] (the app owns the
//! speed, the count-in and the trainer) and the bar answers with actions, mostly
//! [`UiAction::RunCommand`] of the `practice.*` registry commands, so every control is also a
//! command with a menu entry. The controls, left to right: loop the selection, the metronome
//! click, the count-in length, the speed chips (50 / 75 / 100 %) with a slider and its readout, and
//! the speed trainer (its switch, the pass count and, while it is on, the four values it works
//! with). What does not fit the pane's width is left out from the right.

use crate::action::{TransportAction, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::i18n;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::tooltip::Help;
use vello::kurbo::{Circle, Point, Rect, RoundedRect};

/// Height of the bar.
pub const PRACTICE_BAR_HEIGHT: f64 = 26.0;
/// The slowest practice speed, in percent.
pub const MIN_SPEED_PERCENT: u16 = 25;
/// The fastest practice speed, in percent.
pub const MAX_SPEED_PERCENT: u16 = 150;
/// The step of the slider, of the faster / slower commands and of the trainer, in percent.
pub const SPEED_STEP_PERCENT: u16 = 5;

/// The speed trainer: raises the speed by `step` after `after` clean passes of the loop, from
/// `start` up to `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrainerState {
    /// The trainer is on.
    pub enabled: bool,
    /// The speed it starts at, in percent.
    pub start: u16,
    /// How much it adds each time, in percent.
    pub step: u16,
    /// Clean passes before each step up.
    pub after: u8,
    /// The speed it stops at, in percent.
    pub target: u16,
    /// Passes of the loop counted since it was switched on.
    pub passes: u32,
    /// Clean passes at the current speed.
    pub clean: u8,
}

impl Default for TrainerState {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 50,
            step: 5,
            after: 2,
            target: 100,
            passes: 0,
            clean: 0,
        }
    }
}

/// What the bar shows; the app pushes it with every transport refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PracticeState {
    /// The practice speed in percent (100 = as written).
    pub speed: u16,
    /// Bars of count-in before a plain playback (0, 1 or 2).
    pub count_in: u8,
    /// The metronome is on.
    pub metronome: bool,
    /// The song loop is on.
    pub looping: bool,
    /// The speed trainer.
    pub trainer: TrainerState,
}

impl Default for PracticeState {
    fn default() -> Self {
        Self {
            speed: 100,
            count_in: 0,
            metronome: false,
            looping: false,
            trainer: TrainerState::default(),
        }
    }
}

/// A value of the trainer the bar edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainerValue {
    /// The speed it starts at.
    Start,
    /// The step up.
    Step,
    /// Clean passes per step.
    After,
    /// The speed it stops at.
    Target,
}

impl TrainerValue {
    const ALL: [TrainerValue; 4] = [
        TrainerValue::Start,
        TrainerValue::Step,
        TrainerValue::After,
        TrainerValue::Target,
    ];

    fn command(self) -> &'static str {
        match self {
            TrainerValue::Start => "practice.trainer_start",
            TrainerValue::Step => "practice.trainer_step",
            TrainerValue::After => "practice.trainer_after",
            TrainerValue::Target => "practice.trainer_target",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            TrainerValue::Start => "Speed trainer: the speed it starts at",
            TrainerValue::Step => "Speed trainer: how much the speed rises each time",
            TrainerValue::After => "Speed trainer: clean passes before each rise",
            TrainerValue::Target => "Speed trainer: the speed it stops at",
        }
    }
}

/// A control of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PracticeItem {
    /// Loop the bars of the selection.
    Loop,
    /// The metronome click.
    Click,
    /// The count-in length (a click cycles off, 1, 2).
    CountIn,
    /// A speed preset.
    Chip(u16),
    /// One step slower.
    Slower,
    /// The speed slider.
    Slider,
    /// One step faster.
    Faster,
    /// The speed readout.
    Readout,
    /// The trainer switch.
    Trainer,
    /// The trainer's pass count.
    Passes,
    /// A trainer value and its readout.
    Value(TrainerValue),
    /// The arrow of a trainer value: `true` raises it.
    Arrow(TrainerValue, bool),
}

/// The practice bar: layout, painting and the controls' actions.
#[derive(Debug, Clone, Default)]
pub struct PracticeBar {
    state: PracticeState,
    dragging: bool,
    drag_speed: u16,
}

/// The values the speed commands take, in percent: a slider position snaps to one of them.
pub fn snap_speed(percent: f64) -> u16 {
    let step = f64::from(SPEED_STEP_PERCENT);
    let v = (percent / step).round() * step;
    v.clamp(f64::from(MIN_SPEED_PERCENT), f64::from(MAX_SPEED_PERCENT)) as u16
}

/// A row of items laid left to right; an item that does not fit is refused.
struct Flow {
    x: f64,
    right: f64,
    y0: f64,
    h: f64,
    out: Vec<(Rect, PracticeItem)>,
    full: bool,
}

impl Flow {
    fn put(&mut self, w: f64, item: PracticeItem) -> bool {
        if self.full || self.x + w > self.right {
            self.full = true;
            return false;
        }
        self.out.push((
            Rect::new(self.x, self.y0, self.x + w, self.y0 + self.h),
            item,
        ));
        self.x += w + 3.0;
        true
    }

    fn gap(&mut self, w: f64) {
        self.x += w;
    }
}

impl PracticeBar {
    /// A bar showing the default state (100 %, no count-in, trainer off).
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `state`; true when it differs from what was shown (a repaint is due).
    pub fn set_state(&mut self, state: PracticeState) -> bool {
        let changed = self.state != state;
        self.state = state;
        changed
    }

    /// What is shown.
    pub fn state(&self) -> PracticeState {
        self.state
    }

    /// True while the slider is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// The label of the count-in button.
    pub fn count_in_label(bars: u8) -> &'static str {
        match bars {
            0 => "Count-in off",
            1 => "Count-in 1",
            _ => "Count-in 2",
        }
    }

    /// The controls placed in `area`, left to right.
    pub fn items(&self, area: Rect) -> Vec<(Rect, PracticeItem)> {
        let mut f = Flow {
            x: area.x0 + 8.0,
            right: area.x1 - 6.0,
            y0: area.y0 + 3.0,
            h: area.height() - 6.0,
            out: Vec::new(),
            full: false,
        };
        f.put(38.0, PracticeItem::Loop);
        f.put(38.0, PracticeItem::Click);
        f.put(74.0, PracticeItem::CountIn);
        f.gap(8.0);
        for chip in [50, 75, 100] {
            f.put(34.0, PracticeItem::Chip(chip));
        }
        f.put(18.0, PracticeItem::Slower);
        f.put(90.0, PracticeItem::Slider);
        f.put(18.0, PracticeItem::Faster);
        f.put(36.0, PracticeItem::Readout);
        f.gap(8.0);
        f.put(56.0, PracticeItem::Trainer);
        if self.state.trainer.enabled {
            f.put(84.0, PracticeItem::Passes);
            for v in TrainerValue::ALL {
                // The arrows and the value are one unit: all or none of it.
                if f.x + 12.0 + 58.0 + 12.0 > f.right {
                    break;
                }
                f.put(12.0, PracticeItem::Arrow(v, false));
                f.put(58.0, PracticeItem::Value(v));
                f.put(12.0, PracticeItem::Arrow(v, true));
                f.gap(2.0);
            }
        }
        f.out
    }

    fn slider_value(track: Rect, x: f64) -> u16 {
        let usable = (track.width() - 12.0).max(1.0);
        let t = ((x - track.x0 - 6.0) / usable).clamp(0.0, 1.0);
        let span = f64::from(MAX_SPEED_PERCENT - MIN_SPEED_PERCENT);
        snap_speed(f64::from(MIN_SPEED_PERCENT) + t * span)
    }

    fn speed_command(percent: u16) -> UiAction {
        UiAction::RunCommand {
            id: "practice.speed".to_owned(),
            arg: Some(percent.to_string()),
        }
    }

    fn run(id: &str, arg: Option<&str>) -> UiAction {
        UiAction::RunCommand {
            id: id.to_owned(),
            arg: arg.map(str::to_owned),
        }
    }

    /// A press in `area`: the control under it acts. True when the press was inside the bar (it
    /// is the bar's whether or not a control was hit).
    pub fn pointer_down(&mut self, area: Rect, pos: Point, out: &mut Vec<UiAction>) -> bool {
        if !area.contains(pos) {
            return false;
        }
        let Some((rect, item)) = self.items(area).into_iter().find(|(r, _)| r.contains(pos)) else {
            return true;
        };
        match item {
            PracticeItem::Loop => out.push(Self::run("practice.loop_selection", None)),
            PracticeItem::Click => out.push(UiAction::Transport(TransportAction::ToggleMetronome)),
            PracticeItem::CountIn => {
                let next = match self.state.count_in {
                    0 => "1",
                    1 => "2",
                    _ => "off",
                };
                out.push(Self::run("practice.count_in", Some(next)));
            }
            PracticeItem::Chip(p) => out.push(Self::speed_command(p)),
            PracticeItem::Slower => out.push(Self::run("practice.speed_down", None)),
            PracticeItem::Faster => out.push(Self::run("practice.speed_up", None)),
            PracticeItem::Slider => {
                self.dragging = true;
                let v = Self::slider_value(rect, pos.x);
                self.drag_speed = v;
                if v != self.state.speed {
                    out.push(Self::speed_command(v));
                }
            }
            PracticeItem::Trainer => out.push(Self::run("practice.trainer", None)),
            PracticeItem::Arrow(v, up) => {
                out.push(Self::run(v.command(), Some(if up { "up" } else { "down" })))
            }
            PracticeItem::Readout | PracticeItem::Passes | PracticeItem::Value(_) => {}
        }
        true
    }

    /// The pointer moved: a drag of the slider follows it. True while a drag runs.
    pub fn pointer_move(&mut self, area: Rect, pos: Point, out: &mut Vec<UiAction>) -> bool {
        if !self.dragging {
            return false;
        }
        if let Some((rect, _)) = self
            .items(area)
            .into_iter()
            .find(|(_, i)| *i == PracticeItem::Slider)
        {
            let v = Self::slider_value(rect, pos.x);
            if v != self.drag_speed {
                self.drag_speed = v;
                out.push(Self::speed_command(v));
            }
        }
        true
    }

    /// The press ended; true when it ended a drag.
    pub fn pointer_up(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    /// The tooltip of the control at `pos`.
    pub fn help_at(&self, area: Rect, pos: Point) -> Option<Help> {
        let (rect, item) = self
            .items(area)
            .into_iter()
            .find(|(r, _)| r.contains(pos))?;
        let help = match item {
            PracticeItem::Loop => Help::new(
                rect,
                "Loop the bars of the selection (the bar under the cursor without one)",
            )
            .with_command("practice.loop_selection"),
            PracticeItem::Click => {
                Help::new(rect, "Metronome click").with_command("transport.metronome")
            }
            PracticeItem::CountIn => Help::new(
                rect,
                "Bars of count-in before playback starts (click to change)",
            )
            .with_command("practice.count_in"),
            PracticeItem::Chip(50) => Help::new(
                rect,
                "Play at half speed (audio clips are muted unless the speed is 100%)",
            )
            .with_arg("practice.speed", "50"),
            PracticeItem::Chip(75) => Help::new(
                rect,
                "Play at 75% speed (audio clips are muted unless the speed is 100%)",
            )
            .with_arg("practice.speed", "75"),
            PracticeItem::Chip(_) => {
                Help::new(rect, "Play at full speed").with_arg("practice.speed", "100")
            }
            PracticeItem::Slower => {
                Help::new(rect, "Slower by one step").with_command("practice.speed_down")
            }
            PracticeItem::Faster => {
                Help::new(rect, "Faster by one step").with_command("practice.speed_up")
            }
            PracticeItem::Slider | PracticeItem::Readout => Help::new(
                rect,
                "Practice speed, 25 to 150% (audio clips are muted unless it is 100%; not saved)",
            ),
            PracticeItem::Trainer => Help::new(
                rect,
                "Speed trainer: raise the speed after clean passes of the loop",
            )
            .with_command("practice.trainer"),
            PracticeItem::Passes => {
                Help::new(rect, "Passes of the loop and clean passes at this speed")
            }
            PracticeItem::Value(v) => Help::new(rect, v.tip()),
            PracticeItem::Arrow(v, _) => Help::new(rect, v.tip()),
        };
        Some(help)
    }

    fn value_text(&self, v: TrainerValue) -> String {
        let t = &self.state.trainer;
        match v {
            TrainerValue::Start => i18n::tf("From {v}", &[("v", &format!("{}%", t.start))]),
            TrainerValue::Step => i18n::tf("Step {v}", &[("v", &format!("+{}%", t.step))]),
            TrainerValue::After => i18n::tf("Every {n}", &[("n", &t.after.to_string())]),
            TrainerValue::Target => i18n::tf("To {v}", &[("v", &format!("{}%", t.target))]),
        }
    }

    /// The text of the pass counter: the passes so far and the clean ones at this speed.
    fn passes_text(&self) -> String {
        let t = &self.state.trainer;
        i18n::tf(
            "Pass {n} ({a}/{b})",
            &[
                ("n", &t.passes.to_string()),
                ("a", &t.clean.to_string()),
                ("b", &t.after.to_string()),
            ],
        )
    }

    /// Paints the bar into `area`; `hover` is the pointer, for the hot control.
    pub fn paint(&self, c: &mut dyn Canvas, st: &SeqTheme, area: Rect, hover: Option<Point>) {
        let t = &st.base;
        sp::rect(c, area, st.header);
        sp::line(
            c,
            area.x0,
            area.y1 - 0.5,
            area.x1,
            area.y1 - 0.5,
            st.separator,
        );
        let s = &self.state;
        for (r, item) in self.items(area) {
            let hot = hover.is_some_and(|p| r.contains(p));
            match item {
                PracticeItem::Loop => {
                    sp::button(c, st, r, "Loop", s.looping.then_some(t.highlight), hot);
                }
                PracticeItem::Click => {
                    sp::button(c, st, r, "Click", s.metronome.then_some(t.highlight), hot);
                }
                PracticeItem::CountIn => sp::button(
                    c,
                    st,
                    r,
                    Self::count_in_label(s.count_in),
                    (s.count_in > 0).then_some(t.highlight),
                    hot,
                ),
                PracticeItem::Chip(p) => {
                    let label = format!("{p}%");
                    paint_verbatim_button(
                        c,
                        st,
                        r,
                        &label,
                        (s.speed == p).then_some(t.highlight),
                        hot,
                    );
                }
                PracticeItem::Slower => paint_verbatim_button(c, st, r, "-", None, hot),
                PracticeItem::Faster => paint_verbatim_button(c, st, r, "+", None, hot),
                PracticeItem::Slider => self.paint_slider(c, st, r),
                PracticeItem::Readout => {
                    let colour = if s.speed == 100 {
                        t.text_muted
                    } else {
                        t.warning
                    };
                    c.text(
                        &TextRun::verbatim(
                            format!("{}%", s.speed),
                            pt(r.x0 + 2.0, r.center().y + 4.0),
                            11.0,
                            colour,
                        )
                        .weight(700),
                    );
                }
                PracticeItem::Trainer => sp::button(
                    c,
                    st,
                    r,
                    "Trainer",
                    s.trainer.enabled.then_some(t.highlight),
                    hot,
                ),
                PracticeItem::Passes => {
                    c.text(&TextRun::verbatim(
                        self.passes_text(),
                        pt(r.x0 + 2.0, r.center().y + 3.5),
                        10.0,
                        t.text_muted,
                    ));
                }
                PracticeItem::Value(v) => {
                    c.fill_with(RoundedRect::from_rect(r, 3.0), t.lcd);
                    c.text(
                        &TextRun::verbatim(
                            self.value_text(v),
                            pt(r.center().x, r.center().y + 3.5),
                            9.5,
                            t.text,
                        )
                        .align(Align::Center),
                    );
                }
                PracticeItem::Arrow(_, up) => {
                    paint_verbatim_button(c, st, r, if up { ">" } else { "<" }, None, hot);
                }
            }
        }
    }

    fn paint_slider(&self, c: &mut dyn Canvas, st: &SeqTheme, r: Rect) {
        let t = &st.base;
        let mid = r.center().y;
        let track = Rect::new(r.x0 + 6.0, mid - 1.5, r.x1 - 6.0, mid + 1.5);
        c.fill_with(RoundedRect::from_rect(track, 1.5), t.lcd);
        let span = f64::from(MAX_SPEED_PERCENT - MIN_SPEED_PERCENT);
        let at = |p: u16| track.x0 + track.width() * (f64::from(p - MIN_SPEED_PERCENT) / span);
        // The mark of full speed.
        sp::line(c, at(100), mid - 5.0, at(100), mid + 5.0, t.text_muted);
        let speed = self.state.speed.clamp(MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
        let colour = if speed == 100 { t.text } else { t.highlight };
        c.fill_with(Circle::new(pt(at(speed), mid), 5.0), colour);
    }
}

/// A button whose text is a number or a sign, never translated.
fn paint_verbatim_button(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    r: Rect,
    text: &str,
    lit: Option<vello::peniko::Color>,
    hover: bool,
) {
    sp::button(c, st, r, "", lit, hover);
    let t = &st.base;
    let col = if lit.is_some() { t.on_accent } else { t.text };
    c.text(
        &TextRun::verbatim(
            text.to_owned(),
            pt(r.center().x, r.center().y + 3.5),
            9.5,
            col,
        )
        .align(Align::Center)
        .weight(700),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1100.0, PRACTICE_BAR_HEIGHT)
    }

    fn center(bar: &PracticeBar, item: PracticeItem) -> Point {
        bar.items(area())
            .into_iter()
            .find(|(_, i)| *i == item)
            .map(|(r, _)| r.center())
            .unwrap_or_else(|| panic!("{item:?} is not laid out"))
    }

    #[test]
    fn a_slider_position_snaps_to_five_percent_inside_the_range() {
        assert_eq!(snap_speed(0.0), 25);
        assert_eq!(snap_speed(52.4), 50);
        assert_eq!(snap_speed(52.6), 55);
        assert_eq!(snap_speed(1000.0), 150);
    }

    #[test]
    fn a_chip_and_the_switches_ask_for_their_commands() {
        let mut bar = PracticeBar::new();
        let mut out = Vec::new();
        assert!(bar.pointer_down(area(), center(&bar, PracticeItem::Chip(75)), &mut out));
        assert!(matches!(
            out.as_slice(),
            [UiAction::RunCommand { id, arg }] if id == "practice.speed" && arg.as_deref() == Some("75")
        ));
        out.clear();
        bar.pointer_down(area(), center(&bar, PracticeItem::Click), &mut out);
        assert!(matches!(
            out.as_slice(),
            [UiAction::Transport(TransportAction::ToggleMetronome)]
        ));
        out.clear();
        bar.pointer_down(area(), center(&bar, PracticeItem::CountIn), &mut out);
        assert!(matches!(
            out.as_slice(),
            [UiAction::RunCommand { id, arg }] if id == "practice.count_in" && arg.as_deref() == Some("1")
        ));
        // Outside the bar nothing is taken.
        out.clear();
        assert!(!bar.pointer_down(area(), Point::new(10.0, 90.0), &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn the_count_in_button_cycles_off_one_two() {
        let mut bar = PracticeBar::new();
        for (now, next) in [(0u8, "1"), (1, "2"), (2, "off")] {
            bar.set_state(PracticeState {
                count_in: now,
                ..PracticeState::default()
            });
            let mut out = Vec::new();
            bar.pointer_down(area(), center(&bar, PracticeItem::CountIn), &mut out);
            assert!(matches!(
                out.as_slice(),
                [UiAction::RunCommand { arg, .. }] if arg.as_deref() == Some(next)
            ));
        }
    }

    #[test]
    fn dragging_the_slider_emits_each_new_value_once() {
        let mut bar = PracticeBar::new();
        let (track, _) = bar
            .items(area())
            .into_iter()
            .find(|(_, i)| *i == PracticeItem::Slider)
            .unwrap();
        let mut out = Vec::new();
        bar.pointer_down(area(), Point::new(track.x0 + 6.0, 13.0), &mut out);
        assert!(bar.is_dragging());
        bar.pointer_move(area(), Point::new(track.x0 + 7.0, 13.0), &mut out);
        bar.pointer_move(area(), Point::new(track.x1 - 6.0, 13.0), &mut out);
        let args: Vec<_> = out
            .iter()
            .map(|a| match a {
                UiAction::RunCommand { arg, .. } => arg.clone().unwrap(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(args, ["25", "150"]);
        assert!(bar.pointer_up());
        assert!(!bar.is_dragging());
    }

    #[test]
    fn the_trainer_values_show_only_while_it_is_on_and_when_they_fit() {
        let mut bar = PracticeBar::new();
        assert!(
            bar.items(area())
                .iter()
                .all(|(_, i)| !matches!(i, PracticeItem::Value(_)))
        );
        bar.set_state(PracticeState {
            trainer: TrainerState {
                enabled: true,
                ..TrainerState::default()
            },
            ..PracticeState::default()
        });
        let wide = bar.items(area());
        assert_eq!(
            wide.iter()
                .filter(|(_, i)| matches!(i, PracticeItem::Value(_)))
                .count(),
            4
        );
        let narrow = bar.items(Rect::new(0.0, 0.0, 700.0, PRACTICE_BAR_HEIGHT));
        assert!(narrow.iter().any(|(_, i)| *i == PracticeItem::Trainer));
        assert!(
            narrow
                .iter()
                .filter(|(_, i)| matches!(i, PracticeItem::Value(_)))
                .count()
                < 4
        );
        for (r, _) in &narrow {
            assert!(r.x1 <= 700.0);
        }
    }

    #[test]
    fn an_arrow_names_the_value_and_the_direction() {
        let mut bar = PracticeBar::new();
        bar.set_state(PracticeState {
            trainer: TrainerState {
                enabled: true,
                ..TrainerState::default()
            },
            ..PracticeState::default()
        });
        let mut out = Vec::new();
        bar.pointer_down(
            area(),
            center(&bar, PracticeItem::Arrow(TrainerValue::After, true)),
            &mut out,
        );
        assert!(matches!(
            out.as_slice(),
            [UiAction::RunCommand { id, arg }] if id == "practice.trainer_after" && arg.as_deref() == Some("up")
        ));
    }
}
