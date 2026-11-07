//! The Delivery panel: what a playback chain that normalises to a common target would do to the
//! master (the **Targets** page), and the Peak Surgeon that trims the shortest peaks of an audio
//! clip for headroom (the **Peaks** page).
//!
//! ```text
//! DELIVERY    [TARGETS] [PEAKS]                       [ MEASURE ] [ SET CEILING ] [ CLOSE ]
//! Master  -16.2 LUFS   -0.3 dBTP   range 6.1 LU
//! Streaming loud -11 LUFS / -1 dBTP        turned up 5.2 dB into its limiter (5.4 dB past ...)
//! Streaming -14 LUFS / -1 dBTP             turned up 2.2 dB into its limiter (...)
//! ...                                      (the common targets)
//! A quiet master with peaks near 0 dBFS is turned up into a limiter: ...
//! ```
//!
//! A modal view the app places over the window like the mix check. The app fills it with a
//! [`DeliveryData`] ([`DeliveryView::set_data`]); the view only draws and reports what was pressed
//! as a [`UiAction::Delivery`]:
//!
//! * **Targets.** A row chooses the target ([`DeliveryAction::Choose`]; also Up and Down).
//!   `MEASURE` renders the master and measures it ([`DeliveryAction::Measure`]; also Enter);
//!   while it runs it reads `STOP`. `SET CEILING` ([`DeliveryAction::SetCeiling`]) sets the
//!   ceiling of the master's limiter and the export report's target in one undo step.
//! * **Peaks.** A row chooses a peak ([`DeliveryAction::PeakChoose`]). `TRIM` dips the chosen peak
//!   by one step ([`DeliveryAction::Trim`]; also Enter); `APPLY ALL` dips every peak above the
//!   threshold ([`DeliveryAction::ApplyAll`]). The threshold moves in 0.1 dB steps
//!   ([`DeliveryAction::Threshold`]); `STEP` and `WIDTH` cycle the dip's step and half-width.
//!   `RESCAN` reads the clip again.
//! * `TARGETS` and `PEAKS` switch the page ([`DeliveryAction::Page`]; also Left and Right);
//!   `CLOSE` emits [`DeliveryAction::Close`] (the app also closes the panel on Esc).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 22.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 700.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.5;
/// The rows each list shows.
pub const ROWS: usize = 8;

/// The two pages of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeliveryPage {
    /// The targets and the master.
    #[default]
    Targets,
    /// The Peak Surgeon.
    Peaks,
}

/// How a target treats the master, for the colour of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMark {
    /// Nothing is lost: it is turned down or left alone.
    Fine,
    /// It is turned up and its peaks stay under the ceiling.
    Up,
    /// It is turned up into a limiter.
    Limiter,
    /// Not measured.
    None,
}

/// One target of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetRow {
    /// The target's name with its numbers.
    pub name: String,
    /// What it does to the master, one phrase.
    pub effect: String,
    /// The colour class.
    pub mark: TargetMark,
    /// The project works to this target.
    pub current: bool,
}

/// What the Targets page shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TargetsData {
    /// How far a running measurement is (`0.0..=1.0`); `None` when none runs.
    pub running: Option<f32>,
    /// The measured numbers as one line; `None` before the first measurement.
    pub master: Option<String>,
    /// The song changed after the measurement.
    pub stale: bool,
    /// The targets.
    pub rows: Vec<TargetRow>,
    /// The chosen target.
    pub chosen: usize,
    /// The warning of the chosen target (empty for none).
    pub warning: String,
    /// What SET CEILING will set, one line.
    pub ceiling_note: String,
}

/// One peak of the Peaks list.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakRow {
    /// Where it is in the clip, as `0:12.345`.
    pub time: String,
    /// Its level in dBFS.
    pub db: f32,
    /// The headroom won if every peak above it were trimmed to its level, in dB.
    pub headroom: f32,
}

/// What the Peaks page shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeaksData {
    /// The clip's name; empty when no clip can be trimmed (then `message` says why).
    pub clip: String,
    /// Why there is nothing to show (no clip, a warped clip...).
    pub message: String,
    /// The highest peaks, first the highest.
    pub rows: Vec<PeakRow>,
    /// How many peaks the scan kept.
    pub total: usize,
    /// The chosen row.
    pub chosen: usize,
    /// The level above which APPLY ALL trims, in dBFS.
    pub threshold_db: f32,
    /// The dip step in dB.
    pub step_db: f32,
    /// The dip's half-width in frames.
    pub width: u32,
    /// What APPLY ALL would do, one line.
    pub preview: String,
    /// APPLY ALL has something to do.
    pub can_apply: bool,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeliveryData {
    /// The page shown.
    pub page: DeliveryPage,
    /// The Targets page.
    pub targets: TargetsData,
    /// The Peaks page.
    pub peaks: PeaksData,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryAction {
    /// Show a page.
    Page(DeliveryPage),
    /// Choose target `n`.
    Choose(usize),
    /// Render the master and measure it.
    Measure,
    /// Stop the measurement.
    Stop,
    /// Set the limiter's ceiling and the report target to the chosen target.
    SetCeiling,
    /// Choose peak `n` of the list.
    PeakChoose(usize),
    /// Move the threshold by this many dB.
    Threshold(f32),
    /// Cycle the dip step.
    CycleStep,
    /// Cycle the dip width.
    CycleWidth,
    /// Dip the chosen peak by one step.
    Trim,
    /// Dip every peak above the threshold.
    ApplyAll,
    /// Read the clip again.
    Rescan,
    /// Close the panel.
    Close,
}

/// The Delivery panel. See the module documentation.
#[derive(Debug)]
pub struct DeliveryView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: DeliveryData,
    hover: Option<Point>,
    repaint: bool,
}

impl DeliveryView {
    /// A panel with nothing measured yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 960.0, 600.0),
            scale: 1.0,
            data: DeliveryData::default(),
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

    /// Shows `data`.
    pub fn set_data(&mut self, data: DeliveryData) {
        if self.data != data {
            self.data = data;
            self.repaint = true;
        }
    }

    /// What the panel shows.
    pub fn data(&self) -> &DeliveryData {
        &self.data
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn page(&self) -> DeliveryPage {
        self.data.page
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H + 34.0 + ROWS as f64 * ROW_H + 100.0 + 2.0 * PAD
    }

    fn button_rect(&self, from_right: usize, width: f64) -> Rect {
        let p = self.panel();
        let mut x1 = p.x1 - PAD;
        for w in [80.0, 110.0, 100.0].iter().take(from_right) {
            x1 -= w + 10.0;
        }
        Rect::new(x1 - width, p.y0 + 6.0, x1, p.y0 + 6.0 + BTN_H)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        self.button_rect(0, 80.0)
    }

    /// The SET CEILING button (Targets) or the APPLY ALL button (Peaks).
    pub fn action_rect(&self) -> Rect {
        self.button_rect(1, 110.0)
    }

    /// The MEASURE (or STOP) button (Targets) or the TRIM button (Peaks).
    pub fn main_rect(&self) -> Rect {
        self.button_rect(2, 100.0)
    }

    /// The tab button of `page`.
    pub fn tab_rect(&self, page: DeliveryPage) -> Rect {
        let p = self.panel();
        let (x, w) = match page {
            DeliveryPage::Targets => (p.x0 + PAD + 82.0, 74.0),
            DeliveryPage::Peaks => (p.x0 + PAD + 82.0 + 78.0, 60.0),
        };
        Rect::new(x, p.y0 + 6.0, x + w, p.y0 + 6.0 + BTN_H)
    }

    /// The line under the header (the master numbers, or the clip).
    pub fn info_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = p.y0 + HEADER_H + 4.0;
        Rect::new(p.x0 + PAD, y0, p.x1 - PAD, y0 + 26.0)
    }

    /// The list.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = self.info_rect().y1 + 4.0;
        Rect::new(p.x0 + PAD, y0, p.x1 - PAD, y0 + ROWS as f64 * ROW_H)
    }

    /// The row in slot `i`.
    pub fn row_rect(&self, i: usize) -> Rect {
        let l = self.list_rect();
        Rect::new(
            l.x0,
            l.y0 + i as f64 * ROW_H,
            l.x1,
            l.y0 + (i + 1) as f64 * ROW_H,
        )
    }

    /// The `-` button of the threshold (Peaks).
    pub fn threshold_down_rect(&self) -> Rect {
        let b = self.list_rect().y1 + 10.0;
        let x = self.panel().x0 + PAD + 110.0;
        Rect::new(x, b, x + 26.0, b + BTN_H)
    }

    /// The `+` button of the threshold (Peaks).
    pub fn threshold_up_rect(&self) -> Rect {
        let d = self.threshold_down_rect();
        Rect::new(d.x1 + 70.0, d.y0, d.x1 + 96.0, d.y1)
    }

    /// The STEP button (Peaks).
    pub fn step_rect(&self) -> Rect {
        let u = self.threshold_up_rect();
        Rect::new(u.x1 + 24.0, u.y0, u.x1 + 24.0 + 96.0, u.y1)
    }

    /// The WIDTH button (Peaks).
    pub fn width_rect(&self) -> Rect {
        let s = self.step_rect();
        Rect::new(s.x1 + 10.0, s.y0, s.x1 + 10.0 + 96.0, s.y1)
    }

    /// The RESCAN button (Peaks).
    pub fn rescan_rect(&self) -> Rect {
        let w = self.width_rect();
        Rect::new(w.x1 + 10.0, w.y0, w.x1 + 10.0 + 80.0, w.y1)
    }

    fn count(&self) -> usize {
        match self.page() {
            DeliveryPage::Targets => self.data.targets.rows.len(),
            DeliveryPage::Peaks => self.data.peaks.rows.len(),
        }
        .min(ROWS)
    }

    fn running(&self) -> bool {
        self.data.targets.running.is_some()
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        for (page, text) in [
            (
                DeliveryPage::Targets,
                "What common loudness targets would do to the master",
            ),
            (
                DeliveryPage::Peaks,
                "Trim the shortest peaks of the selected audio clip for headroom",
            ),
        ] {
            if self.tab_rect(page).contains(pos) {
                return help(self.tab_rect(page), text);
            }
        }
        match self.page() {
            DeliveryPage::Targets => {
                if self.main_rect().contains(pos) {
                    return if self.running() {
                        help(self.main_rect(), "Stop the measurement")
                    } else {
                        help(
                            self.main_rect(),
                            "Render the master and measure its loudness and true peak",
                        )
                    };
                }
                if self.action_rect().contains(pos) {
                    return help(
                        self.action_rect(),
                        "Set the master limiter's ceiling and the export report's target",
                    );
                }
                if self.list_rect().contains(pos) {
                    return help(
                        self.list_rect(),
                        "Choose a target: the gain it would apply to the measured master",
                    );
                }
            }
            DeliveryPage::Peaks => {
                if self.main_rect().contains(pos) {
                    return help(self.main_rect(), "Dip the chosen peak by one step");
                }
                if self.action_rect().contains(pos) {
                    return help(self.action_rect(), "Dip every peak above the threshold");
                }
                if self.threshold_down_rect().contains(pos)
                    || self.threshold_up_rect().contains(pos)
                {
                    return help(self.threshold_down_rect(), "Move the threshold by 0.1 dB");
                }
                if self.step_rect().contains(pos) {
                    return help(
                        self.step_rect(),
                        "How much one dip takes: 0.2, 0.3 or 0.4 dB",
                    );
                }
                if self.width_rect().contains(pos) {
                    return help(
                        self.width_rect(),
                        "Frames each side of a dip: a few samples",
                    );
                }
                if self.rescan_rect().contains(pos) {
                    return help(self.rescan_rect(), "Read the selected clip again");
                }
                if self.list_rect().contains(pos) {
                    return help(
                        self.list_rect(),
                        "The highest peaks of the clip; headroom if trimmed down to each",
                    );
                }
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = UiAction::Delivery;
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if let Some(a) = self.press(*pos) {
                    out.push(act(a));
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                if let Some(a) = self.key(key) {
                    out.push(act(a));
                }
            }
            _ => {}
        }
        out
    }

    fn press(&self, pos: Point) -> Option<DeliveryAction> {
        use DeliveryAction as A;
        if self.close_rect().contains(pos) {
            return Some(A::Close);
        }
        for page in [DeliveryPage::Targets, DeliveryPage::Peaks] {
            if self.tab_rect(page).contains(pos) {
                return Some(A::Page(page));
            }
        }
        let row = (0..self.count()).find(|i| self.row_rect(*i).contains(pos));
        match self.page() {
            DeliveryPage::Targets => {
                if self.main_rect().contains(pos) {
                    Some(if self.running() { A::Stop } else { A::Measure })
                } else if self.action_rect().contains(pos) {
                    Some(A::SetCeiling)
                } else {
                    row.map(A::Choose)
                }
            }
            DeliveryPage::Peaks => {
                if self.main_rect().contains(pos) {
                    Some(A::Trim)
                } else if self.action_rect().contains(pos) {
                    Some(A::ApplyAll)
                } else if self.threshold_down_rect().contains(pos) {
                    Some(A::Threshold(-0.1))
                } else if self.threshold_up_rect().contains(pos) {
                    Some(A::Threshold(0.1))
                } else if self.step_rect().contains(pos) {
                    Some(A::CycleStep)
                } else if self.width_rect().contains(pos) {
                    Some(A::CycleWidth)
                } else if self.rescan_rect().contains(pos) {
                    Some(A::Rescan)
                } else {
                    row.map(A::PeakChoose)
                }
            }
        }
    }

    fn key(&self, key: &Key) -> Option<DeliveryAction> {
        use DeliveryAction as A;
        let (chosen, count) = match self.page() {
            DeliveryPage::Targets => (self.data.targets.chosen, self.count()),
            DeliveryPage::Peaks => (self.data.peaks.chosen, self.count()),
        };
        let pick = |n: usize| match self.page() {
            DeliveryPage::Targets => A::Choose(n),
            DeliveryPage::Peaks => A::PeakChoose(n),
        };
        match key {
            Key::Up if chosen > 0 => Some(pick(chosen - 1)),
            Key::Down if chosen + 1 < count => Some(pick(chosen + 1)),
            Key::Left if self.page() == DeliveryPage::Peaks => Some(A::Page(DeliveryPage::Targets)),
            Key::Right if self.page() == DeliveryPage::Targets => {
                Some(A::Page(DeliveryPage::Peaks))
            }
            Key::Enter => match self.page() {
                DeliveryPage::Targets if !self.running() => Some(A::Measure),
                DeliveryPage::Peaks => Some(A::Trim),
                DeliveryPage::Targets => None,
            },
            _ => None,
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
            "DELIVERY",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        for (page, name) in [
            (DeliveryPage::Targets, "TARGETS"),
            (DeliveryPage::Peaks, "PEAKS"),
        ] {
            let r = self.tab_rect(page);
            let lit = (self.page() == page).then_some(t.focus);
            sp::button(c, st, r, name, lit, hovered(r));
        }
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        match self.page() {
            DeliveryPage::Targets => self.paint_targets(c, hovered),
            DeliveryPage::Peaks => self.paint_peaks(c, hovered),
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_targets(&self, c: &mut dyn Canvas, hovered: impl Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let d = &self.data.targets;
        let p = self.panel();
        let inner = p.width() - 2.0 * PAD;
        let main = self.main_rect();
        sp::button(
            c,
            st,
            main,
            if self.running() { "STOP" } else { "MEASURE" },
            None,
            hovered(main),
        );
        let set = self.action_rect();
        sp::button(c, st, set, "SET CEILING", None, hovered(set));
        // The measured master.
        let info = self.info_rect();
        let line = match (&d.master, d.running) {
            (_, Some(f)) => format!("Rendering the master: {:.0} %", f.clamp(0.0, 1.0) * 100.0),
            (Some(m), None) => format!("Master  {m}"),
            (None, None) => "Master  not measured yet".to_owned(),
        };
        let muted = d.stale && d.running.is_none();
        sp::label(
            c,
            &fit_label(&line, info.width(), 11.5),
            info.x0,
            info.y0 + 16.0,
            11.5,
            if muted { t.text_muted } else { t.text },
            Align::Start,
        );
        // The list.
        let list = self.list_rect();
        sp::lcd(c, st, list.inflate(2.0, 2.0), None);
        for (i, row) in d.rows.iter().take(ROWS).enumerate() {
            let r = self.row_rect(i);
            if i == d.chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            let colour = match row.mark {
                TargetMark::Fine => t.io_connected,
                TargetMark::Up => t.warning,
                TargetMark::Limiter => t.clip,
                TargetMark::None => t.text_muted,
            };
            sp::rect(
                c,
                Rect::new(r.x0 + 4.0, r.y0 + 6.0, r.x0 + 10.0, r.y0 + 16.0),
                if muted {
                    mix(t.lcd, colour, 0.4)
                } else {
                    colour
                },
            );
            let name = if row.current {
                format!("{}  (project target)", row.name)
            } else {
                row.name.clone()
            };
            let half = (r.width() - 24.0) * 0.5;
            sp::label(
                c,
                &fit_label(&name, half, FONT),
                r.x0 + 16.0,
                r.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(&row.effect, half, FONT),
                r.x1 - 8.0,
                r.y0 + 15.0,
                FONT,
                if muted { t.text_muted } else { t.text },
                Align::End,
            );
        }
        let mut y = list.y1 + 22.0;
        if !d.warning.is_empty() {
            for part in wrap(&d.warning, inner, 10.5) {
                sp::label(c, &part, list.x0, y, 10.5, t.warning, Align::Start);
                y += 15.0;
            }
        }
        sp::label(
            c,
            &fit_label(&d.ceiling_note, inner, 9.5),
            list.x0,
            p.y1 - PAD - 14.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        let foot = if d.running.is_some() {
            "Rendering the master: the song keeps playing meanwhile"
        } else if d.stale {
            "The song changed since this measurement: measure again"
        } else {
            "A quiet master with hot peaks is turned up into a limiter you do not control"
        };
        sp::label(
            c,
            &fit_label(foot, inner, 9.5),
            list.x0,
            p.y1 - PAD,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }

    fn paint_peaks(&self, c: &mut dyn Canvas, hovered: impl Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let d = &self.data.peaks;
        let p = self.panel();
        let inner = p.width() - 2.0 * PAD;
        let ready = !d.clip.is_empty();
        let main = self.main_rect();
        sp::button(c, st, main, "TRIM", None, ready && hovered(main));
        let all = self.action_rect();
        sp::button(c, st, all, "APPLY ALL", None, d.can_apply && hovered(all));
        let info = self.info_rect();
        let line = if ready {
            format!("Clip  {}  ({} peaks kept)", d.clip, d.total)
        } else {
            d.message.clone()
        };
        sp::label(
            c,
            &fit_label(&line, info.width(), 11.5),
            info.x0,
            info.y0 + 16.0,
            11.5,
            t.text,
            Align::Start,
        );
        let list = self.list_rect();
        sp::lcd(c, st, list.inflate(2.0, 2.0), None);
        for (i, row) in d.rows.iter().take(ROWS).enumerate() {
            let r = self.row_rect(i);
            if i == d.chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            let above = row.db > d.threshold_db;
            sp::label(
                c,
                &format!("#{}", i + 1),
                r.x0 + 8.0,
                r.y0 + 15.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
            sp::label(
                c,
                &row.time,
                r.x0 + 48.0,
                r.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &format!("{:.2} dBFS", row.db),
                r.x0 + 200.0,
                r.y0 + 15.0,
                FONT,
                if above { t.warning } else { t.text },
                Align::Start,
            );
            sp::label(
                c,
                &format!("{:.2} dB if trimmed to here", row.headroom),
                r.x1 - 8.0,
                r.y0 + 15.0,
                FONT,
                t.text_muted,
                Align::End,
            );
        }
        if !ready {
            sp::label(
                c,
                "Select an audio clip, then press RESCAN",
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
        }
        // The controls under the list.
        let down = self.threshold_down_rect();
        let up = self.threshold_up_rect();
        sp::label(
            c,
            "Trim above",
            p.x0 + PAD,
            down.y0 + 15.0,
            FONT,
            t.text,
            Align::Start,
        );
        sp::button(c, st, down, "-", None, hovered(down));
        sp::label(
            c,
            &format!("{:.1} dBFS", d.threshold_db),
            (down.x1 + up.x0) / 2.0,
            down.y0 + 15.0,
            FONT,
            t.text,
            Align::Center,
        );
        sp::button(c, st, up, "+", None, hovered(up));
        let step = self.step_rect();
        sp::button(
            c,
            st,
            step,
            &format!("STEP {:.1} dB", d.step_db),
            None,
            hovered(step),
        );
        let width = self.width_rect();
        sp::button(
            c,
            st,
            width,
            &format!("WIDTH {}", d.width),
            None,
            hovered(width),
        );
        let rescan = self.rescan_rect();
        sp::button(c, st, rescan, "RESCAN", None, hovered(rescan));
        let mut y = down.y1 + 22.0;
        for part in wrap(&d.preview, inner, 10.5) {
            sp::label(c, &part, p.x0 + PAD, y, 10.5, t.text, Align::Start);
            y += 15.0;
        }
        sp::label(
            c,
            "One undo step: the clip's gain envelope takes the dips, the file is not touched",
            p.x0 + PAD,
            p.y1 - PAD,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }
}

/// Splits `text` into lines that fit `width` at `size` (words are not broken; at most 3 lines).
fn wrap(text: &str, width: f64, size: f32) -> Vec<String> {
    let per_line = ((width / (f64::from(size) * 0.52)) as usize).max(10);
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= per_line => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines.truncate(3);
    lines
}
