//! The mix check panel: the findings of the analysis of the song, each with the track it concerns,
//! the measured number, a line on why it matters and, where there is one, a button that fixes it
//! in one step or opens the tool that works on it.
//!
//! ```text
//! MIX CHECK                                              [ CHECK ]  [ CLOSE ]
//! BASS ROOT  Bass                                             [ FIX BUTTON ]
//! root D1 at 36.7 Hz, outside 40-60 Hz                        [ TOOL BUTTON ]
//! Below 40 Hz small speakers give nothing and ...
//! LOW MIDS   Keys 41%, Pad 22%                                [ CUT 250 Hz ]
//! ...
//! note line
//! ```
//!
//! A modal view the app places over the window like the masking map. The app fills it with a
//! [`MixCheckData`] when a check finishes ([`MixCheckView::set_data`]) and shows progress while one
//! runs ([`MixCheckView::set_progress`]); the view only draws and reports what was pressed as a
//! [`UiAction::MixCheck`]:
//!
//! * `CHECK` ([`MixCheckAction::Run`]); while a check runs the button says `CANCEL` and emits
//!   [`MixCheckAction::Cancel`].
//! * A row chooses the finding (also Up and Down; the wheel scrolls). A finding that happens at a
//!   time ([`MixCheckRow::at`]) emits [`MixCheckAction::Locate`]: the app puts the playhead there.
//! * The fix button emits [`MixCheckAction::Fix`], the tool button [`MixCheckAction::Open`].
//! * `CLOSE` emits [`MixCheckAction::Close`]; the app also closes the panel on Esc.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::masking_panel::clock;
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 62.0;
const FOOT_H: f64 = 30.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 860.0;
const BTN_H: f64 = 22.0;
const BTN_W: f64 = 150.0;
const FONT: f32 = 10.5;
/// The findings the list shows at once.
pub const ROWS: usize = 6;

/// One finding, as the panel draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct MixCheckRow {
    /// What it is about, as a short tag (`BASS ROOT`).
    pub tag: String,
    /// The track or tracks it concerns.
    pub track: String,
    /// The measured number, as a sentence.
    pub measured: String,
    /// Why it matters.
    pub why: String,
    /// Seconds of the song where it happens (the playhead can go there).
    pub at: Option<f64>,
    /// The label of the one-click fix.
    pub fix: Option<String>,
    /// The label of the button that opens the tool that works on it.
    pub tool: Option<String>,
    /// The fix was applied.
    pub done: bool,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MixCheckData {
    /// How many strips were measured.
    pub strips: usize,
    /// The findings.
    pub rows: Vec<MixCheckRow>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixCheckAction {
    /// Check the song (or the time selection) now.
    Run,
    /// Stop the check that runs.
    Cancel,
    /// Put the playhead where finding `n` happens.
    Locate(usize),
    /// Apply the fix of finding `n` (one undo step).
    Fix(usize),
    /// Open the tool of finding `n`.
    Open(usize),
    /// Close the panel.
    Close,
}

/// The mix check panel. See the module documentation.
#[derive(Debug)]
pub struct MixCheckView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: Option<MixCheckData>,
    progress: Option<f32>,
    note: String,
    chosen: usize,
    top: usize,
    hover: Option<Point>,
    repaint: bool,
}

impl MixCheckView {
    /// A panel with nothing checked yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            data: None,
            progress: None,
            note: String::new(),
            chosen: 0,
            top: 0,
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

    /// Shows the result of a check. The chosen finding and the scroll stay when the list has the
    /// same length (a fix changes a row, not the list).
    pub fn set_data(&mut self, data: Option<MixCheckData>) {
        if self.data != data {
            let same =
                matches!((&self.data, &data), (Some(a), Some(b)) if a.rows.len() == b.rows.len());
            if !same {
                self.chosen = 0;
                self.top = 0;
            }
            self.data = data;
            self.repaint = true;
        }
    }

    /// Shows the progress of the check that runs (`None`: none runs).
    pub fn set_progress(&mut self, progress: Option<f32>) {
        if self.progress != progress {
            self.progress = progress;
            self.repaint = true;
        }
    }

    /// The line at the foot (what the last action did).
    pub fn set_note(&mut self, note: &str) {
        if self.note != note {
            note.clone_into(&mut self.note);
            self.repaint = true;
        }
    }

    /// The result that is shown.
    pub fn data(&self) -> Option<&MixCheckData> {
        self.data.as_ref()
    }

    /// The finding that is chosen.
    pub fn chosen(&self) -> usize {
        self.chosen
    }

    /// The first finding the list shows.
    pub fn top(&self) -> usize {
        self.top
    }

    /// Whether a check runs.
    pub fn busy(&self) -> bool {
        self.progress.is_some()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn count(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.rows.len())
    }

    fn row(&self, n: usize) -> Option<&MixCheckRow> {
        self.data.as_ref()?.rows.get(n)
    }

    fn choose(&mut self, n: usize) {
        let count = self.count();
        if count > 0 {
            self.chosen = n.min(count - 1);
            if self.chosen < self.top {
                self.top = self.chosen;
            } else if self.chosen >= self.top + ROWS {
                self.top = self.chosen + 1 - ROWS;
            }
        }
        self.repaint = true;
    }

    fn scroll(&mut self, by: isize) {
        let most = self.count().saturating_sub(ROWS);
        let top = (self.top as isize + by).clamp(0, most as isize) as usize;
        if top != self.top {
            self.top = top;
            self.repaint = true;
        }
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
        HEADER_H + ROWS as f64 * ROW_H + FOOT_H + 3.0 * PAD
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

    /// The CHECK (or CANCEL) button.
    pub fn run_rect(&self) -> Rect {
        let close = self.close_rect();
        Rect::new(close.x0 - 10.0 - 110.0, close.y0, close.x0 - 10.0, close.y1)
    }

    /// The list of findings.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = p.y0 + HEADER_H + PAD;
        Rect::new(p.x0 + PAD, y0, p.x1 - PAD, y0 + ROWS as f64 * ROW_H)
    }

    /// The row in slot `i` of the list (the finding `top() + i`).
    pub fn row_rect(&self, i: usize) -> Rect {
        let l = self.list_rect();
        Rect::new(
            l.x0,
            l.y0 + i as f64 * ROW_H,
            l.x1,
            l.y0 + (i + 1) as f64 * ROW_H,
        )
    }

    /// The fix button of the row in slot `i`.
    pub fn fix_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(
            r.x1 - 6.0 - BTN_W,
            r.y0 + 8.0,
            r.x1 - 6.0,
            r.y0 + 8.0 + BTN_H,
        )
    }

    /// The tool button of the row in slot `i`.
    pub fn tool_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(
            r.x1 - 6.0 - BTN_W,
            r.y0 + 8.0 + BTN_H + 6.0,
            r.x1 - 6.0,
            r.y0 + 8.0 + 2.0 * BTN_H + 6.0,
        )
    }

    fn slots(&self) -> usize {
        self.count().saturating_sub(self.top).min(ROWS)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.run_rect().contains(pos) {
            return help(
                self.run_rect(),
                if self.busy() {
                    "Stop the check"
                } else {
                    "Render the song and list what the mix needs"
                },
            );
        }
        for i in 0..self.slots() {
            let Some(row) = self.row(self.top + i) else {
                continue;
            };
            if row.fix.is_some() && !row.done && self.fix_rect(i).contains(pos) {
                return help(self.fix_rect(i), "Apply this fix (one undo step)");
            }
            if row.tool.is_some() && self.tool_rect(i).contains(pos) {
                return help(
                    self.tool_rect(i),
                    "Open the tool that works on this finding",
                );
            }
        }
        if self.list_rect().contains(pos) && self.count() > 0 {
            return help(
                self.list_rect(),
                "Click a finding to choose it; a finding with a time moves the playhead there",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = UiAction::MixCheck;
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
            UiEvent::Wheel { delta, pos, .. } => {
                if self.list_rect().contains(*pos) {
                    self.scroll(if delta.y > 0.0 { -1 } else { 1 });
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(act(MixCheckAction::Close));
                } else if self.run_rect().contains(*pos) {
                    out.push(act(if self.busy() {
                        MixCheckAction::Cancel
                    } else {
                        MixCheckAction::Run
                    }));
                } else if let Some(i) = (0..self.slots()).find(|i| self.row_rect(*i).contains(*pos))
                {
                    let n = self.top + i;
                    let row = self.row(n).cloned();
                    self.choose(n);
                    if let Some(row) = row {
                        if row.fix.is_some() && !row.done && self.fix_rect(i).contains(*pos) {
                            out.push(act(MixCheckAction::Fix(n)));
                        } else if row.tool.is_some() && self.tool_rect(i).contains(*pos) {
                            out.push(act(MixCheckAction::Open(n)));
                        } else if row.at.is_some() {
                            out.push(act(MixCheckAction::Locate(n)));
                        }
                    }
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Up => self.choose(self.chosen.saturating_sub(1)),
                Key::Down => self.choose(self.chosen + 1),
                _ => {}
            },
            _ => {}
        }
        out
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
            "MIX CHECK",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let run = self.run_rect();
        let text = match self.progress {
            Some(f) => format!("CANCEL {:.0}%", f * 100.0),
            None => "CHECK".to_owned(),
        };
        sp::button(
            c,
            st,
            run,
            &text,
            self.busy().then_some(st.record),
            hovered(run),
        );
        self.paint_list(c);
        let foot = if self.note.is_empty() {
            match &self.data {
                None => {
                    "Check renders the channels, the returns and the master of the song (or the selected time range)"
                }
                Some(d) if d.rows.is_empty() => {
                    "Nothing to report: the measured numbers are inside the limits"
                }
                Some(_) => "Fix a finding in one step, or open the tool that works on it",
            }
        } else {
            self.note.as_str()
        };
        sp::label(
            c,
            &fit_label(foot, p.width() - 2.0 * PAD, 9.5),
            p.x0 + PAD,
            p.y1 - PAD,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_list(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let list = self.list_rect();
        sp::lcd(c, st, list.inflate(2.0, 2.0), None);
        let Some(data) = &self.data else {
            sp::label(
                c,
                "No check yet",
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        };
        if data.rows.is_empty() {
            sp::label(
                c,
                &format!("No findings in {} strips", data.strips),
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        }
        let text_w = list.width() - BTN_W - 24.0;
        for i in 0..self.slots() {
            let n = self.top + i;
            let Some(row) = data.rows.get(n) else {
                continue;
            };
            let r = self.row_rect(i);
            if n == self.chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            if i > 0 {
                sp::line(c, r.x0, r.y0 + 0.5, r.x1, r.y0 + 0.5, st.separator);
            }
            let head = format!("{}   {}", row.tag, row.track);
            sp::label(
                c,
                &fit_label(&head, text_w, FONT),
                r.x0 + 8.0,
                r.y0 + 17.0,
                FONT,
                t.text,
                Align::Start,
            );
            let measured = match row.at {
                Some(at) => format!("{} (at {})", row.measured, clock(at)),
                None => row.measured.clone(),
            };
            sp::label(
                c,
                &fit_label(&measured, text_w, FONT),
                r.x0 + 8.0,
                r.y0 + 33.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(&row.why, text_w, 9.5),
                r.x0 + 8.0,
                r.y0 + 50.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
            if let Some(fix) = &row.fix {
                let b = self.fix_rect(i);
                if row.done {
                    sp::button(c, st, b, "DONE", None, false);
                } else {
                    sp::button(
                        c,
                        st,
                        b,
                        fix,
                        Some(st.loop_on),
                        self.hover.is_some_and(|h| b.contains(h)),
                    );
                }
            }
            if let Some(tool) = &row.tool {
                let b = self.tool_rect(i);
                sp::button(
                    c,
                    st,
                    b,
                    tool,
                    None,
                    self.hover.is_some_and(|h| b.contains(h)),
                );
            }
        }
        if data.rows.len() > ROWS {
            sp::label(
                c,
                &format!(
                    "{}-{} of {}",
                    self.top + 1,
                    self.top + self.slots(),
                    data.rows.len()
                ),
                list.x1 - 4.0,
                list.y1 + 12.0,
                9.0,
                t.text_muted,
                Align::End,
            );
        }
    }
}
