//! The screen of the audio and MIDI setup: layout, events and painting of the three pages.
//!
//! Everything that decides something is in [`crate::audio_setup`]; this file lays the page out
//! as a list of widgets (so the painting and the hit test read the same rectangles), turns
//! events into [`SetupOutcome`]s for the shell, and draws with the colours of the theme only.
//! The panel is 600 by 520 logical pixels, shrunk to fit a small window, so the same layout
//! serves a 1x and a 2x screen.

use crate::audio_setup::{AudioSetup, Page};
use crate::settings_dialog::Choice;
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::paint::{estimate_width, fit_label_in};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 520.0;
const PAD: f64 = 24.0;
const ROW: f64 = 24.0;
const CHIP: f64 = 26.0;
const BUTTON: (f64, f64) = (104.0, 28.0);
const GAP: f64 = 8.0;
/// The most rows a device list shows before it scrolls.
const VISIBLE_ROWS: usize = 5;
const METER_WIDTH: f64 = 260.0;
const METER_HEIGHT: f64 = 12.0;

/// What a widget does when it is pressed.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Picks a host, a device, a buffer or switches a MIDI input.
    Pick(Choice),
    /// Plays the test tone.
    Test,
    /// Gives the project input channels so the input meter has something to read.
    TurnOnInput,
    /// Opens the controller wizard.
    SetUpController,
    /// Goes back a page.
    Back,
    /// Closes the panel without going through the rest of the pages.
    Skip,
    /// Goes to the next page.
    Next,
    /// Closes the panel after the last page.
    Done,
}

/// How a widget is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Chip,
    Radio,
    Check,
    Button,
    Primary,
}

/// One thing to press.
#[derive(Debug, Clone)]
pub struct Widget {
    /// What it does.
    pub kind: Kind,
    /// Where it is.
    pub rect: Rect,
    /// Its text.
    pub label: String,
    look: Look,
}

/// One line of text.
#[derive(Debug, Clone)]
struct Line {
    text: String,
    at: Point,
    size: f32,
    /// A heading or a plain line (`true` draws it in the muted colour).
    muted: bool,
    bold: bool,
    /// Drawn as a warning.
    warn: bool,
}

/// A meter bar: where and how far it is filled.
#[derive(Debug, Clone, Copy)]
struct Bar {
    rect: Rect,
    fraction: f32,
}

/// The screen laid out for one window size.
struct Layout {
    panel: Rect,
    widgets: Vec<Widget>,
    lines: Vec<Line>,
    bars: Vec<Bar>,
    /// The "note received" lamp: where and whether it is lit.
    lamp: Option<(Point, bool)>,
}

/// What the panel asks of the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum SetupOutcome {
    /// Still open (a page change or a scroll happened inside).
    Open,
    /// The user picked this option: make it the draft and apply the settings.
    Pick(Choice),
    /// Play the test tone.
    Test,
    /// Turn the project input on.
    TurnOnInput,
    /// Open the controller wizard.
    SetUpController,
    /// Close; setup counts as done.
    Skip,
    /// Close after the last page; setup counts as done.
    Done,
}

impl Layout {
    fn line(&mut self, text: impl Into<String>, at: Point, size: f32, muted: bool) {
        self.lines.push(Line {
            text: text.into(),
            at,
            size,
            muted,
            bold: false,
            warn: false,
        });
    }

    fn heading(&mut self, text: &str, at: Point) {
        self.lines.push(Line {
            text: text.to_owned(),
            at,
            size: 12.0,
            muted: true,
            bold: true,
            warn: false,
        });
    }

    fn widget(&mut self, kind: Kind, rect: Rect, label: impl Into<String>, look: Look) {
        self.widgets.push(Widget {
            kind,
            rect,
            label: label.into(),
            look,
        });
    }
}

impl AudioSetup {
    fn panel(&self, view: Size) -> Rect {
        let size = Size::new(WIDTH.min(view.width - 20.0), HEIGHT.min(view.height - 20.0));
        Rect::from_center_size(Point::new(view.width / 2.0, view.height / 2.0), size)
    }

    /// The chips of `options`, wrapped to the panel width from `(x, y)`; returns the y below.
    fn chips(l: &mut Layout, panel: Rect, options: &[(String, Choice)], y: f64) -> f64 {
        let (mut x, mut y) = (panel.x0 + PAD, y);
        for (label, choice) in options {
            let w = (estimate_width(label, 13.0) + 22.0).max(44.0);
            if x + w > panel.x1 - PAD {
                x = panel.x0 + PAD;
                y += CHIP + 4.0;
            }
            l.widget(
                Kind::Pick(choice.clone()),
                Rect::new(x, y, x + w, y + CHIP),
                label.clone(),
                Look::Chip,
            );
            x += w + 6.0;
        }
        y + CHIP
    }

    /// The rows of `options` from the page's first visible row, `rows` at most; returns the y
    /// below. `check` draws boxes (several can be on) instead of radio dots.
    fn rows(
        &self,
        l: &mut Layout,
        panel: Rect,
        options: &[(String, Choice)],
        y: f64,
        rows: usize,
        check: bool,
    ) -> f64 {
        let first = self.first[self.page.index()].min(options.len().saturating_sub(rows));
        for (i, (label, choice)) in options.iter().skip(first).take(rows).enumerate() {
            let top = y + i as f64 * ROW;
            l.widget(
                Kind::Pick(choice.clone()),
                Rect::new(panel.x0 + PAD, top, panel.x1 - PAD, top + ROW - 2.0),
                label.clone(),
                if check { Look::Check } else { Look::Radio },
            );
        }
        let shown = options.len().min(rows);
        if options.len() > rows {
            l.line(
                format!(
                    "{}-{} of {} (scroll for more)",
                    first + 1,
                    first + shown,
                    options.len()
                ),
                Point::new(panel.x0 + PAD, y + shown as f64 * ROW + 12.0),
                11.0,
                true,
            );
            return y + shown as f64 * ROW + 18.0;
        }
        y + shown as f64 * ROW
    }

    fn layout(&self, view: Size) -> Layout {
        let panel = self.panel(view);
        let mut l = Layout {
            panel,
            widgets: Vec::new(),
            lines: Vec::new(),
            bars: Vec::new(),
            lamp: None,
        };
        let x = panel.x0 + PAD;
        l.lines.push(Line {
            text: "Audio and MIDI Setup".to_owned(),
            at: Point::new(x, panel.y0 + 30.0),
            size: 18.0,
            muted: false,
            bold: true,
            warn: false,
        });
        l.line(
            format!(
                "Step {} of {}: {}",
                self.page.index() + 1,
                Page::ALL.len(),
                self.page.title()
            ),
            Point::new(x, panel.y0 + 52.0),
            12.5,
            true,
        );
        let options = self.options();
        let top = panel.y0 + 72.0;
        match self.page {
            Page::Output => self.output_page(&mut l, panel, &options, top),
            Page::Input => self.input_page(&mut l, panel, &options, top),
            Page::Midi => self.midi_page(&mut l, panel, &options, top),
        }
        let by = panel.y1 - 20.0 - BUTTON.1;
        let button = |x: f64| Rect::new(x, by, x + BUTTON.0, by + BUTTON.1);
        l.widget(Kind::Skip, button(panel.x0 + PAD), "Skip", Look::Button);
        let next_x = panel.x1 - PAD - BUTTON.0;
        if self.is_last() {
            l.widget(Kind::Done, button(next_x), "Done", Look::Primary);
        } else {
            l.widget(Kind::Next, button(next_x), "Next", Look::Primary);
        }
        if self.page != Page::Output {
            l.widget(
                Kind::Back,
                button(next_x - BUTTON.0 - GAP),
                "Back",
                Look::Button,
            );
        }
        l
    }

    fn output_page(&self, l: &mut Layout, panel: Rect, options: &[(String, Choice)], top: f64) {
        let x = panel.x0 + PAD;
        let hosts: Vec<(String, Choice)> = options
            .iter()
            .filter(|(_, c)| matches!(c, Choice::Host(_)))
            .cloned()
            .collect();
        let devices: Vec<(String, Choice)> = options
            .iter()
            .filter(|(_, c)| matches!(c, Choice::Device(_)))
            .cloned()
            .collect();
        let buffers: Vec<(String, Choice)> = options
            .iter()
            .filter(|(_, c)| matches!(c, Choice::Buffer(_)))
            .cloned()
            .collect();
        l.heading("Audio host", Point::new(x, top + 12.0));
        let mut y = Self::chips(l, panel, &hosts, top + 18.0) + 10.0;
        l.heading("Output device", Point::new(x, y + 12.0));
        y = self.rows(l, panel, &devices, y + 18.0, VISIBLE_ROWS, false) + 8.0;
        l.heading("Buffer size (frames)", Point::new(x, y + 12.0));
        y = Self::chips(l, panel, &buffers, y + 18.0);
        l.line(
            "smaller = less delay, more CPU",
            Point::new(x, y + 16.0),
            12.0,
            true,
        );
        l.line(self.latency.clone(), Point::new(x, y + 38.0), 12.5, false);
        let test_top = y + 50.0;
        l.widget(
            Kind::Test,
            Rect::new(x, test_top, x + BUTTON.0, test_top + BUTTON.1),
            "Test",
            Look::Button,
        );
        let meter_x = x + BUTTON.0 + 16.0;
        let meter_y = test_top + (BUTTON.1 - METER_HEIGHT) / 2.0;
        l.bars.push(Bar {
            rect: Rect::new(
                meter_x,
                meter_y,
                (meter_x + METER_WIDTH).min(panel.x1 - PAD),
                meter_y + METER_HEIGHT,
            ),
            fraction: self.out_meter.fraction(),
        });
        let hint = if self.tested {
            "The meter moves when sound reaches the device."
        } else {
            "Press Test: a soft chord plays and the meter moves."
        };
        l.line(hint, Point::new(x, test_top + 46.0), 12.0, true);
        if let Some(problem) = &self.problem {
            l.lines.push(Line {
                text: problem.clone(),
                at: Point::new(x, test_top + 66.0),
                size: 12.5,
                muted: false,
                bold: false,
                warn: true,
            });
        }
    }

    fn input_page(&self, l: &mut Layout, panel: Rect, options: &[(String, Choice)], top: f64) {
        let x = panel.x0 + PAD;
        l.line(
            "Optional: only needed to record sound from a microphone or an instrument.",
            Point::new(x, top + 12.0),
            12.5,
            true,
        );
        l.heading("Input device", Point::new(x, top + 40.0));
        let y = self.rows(l, panel, options, top + 46.0, VISIBLE_ROWS + 1, false) + 14.0;
        l.line(
            self.input_line.clone(),
            Point::new(x, y + 12.0),
            12.5,
            false,
        );
        let mut y = y + 24.0;
        if self.input_unwanted {
            l.widget(
                Kind::TurnOnInput,
                Rect::new(x, y, x + 150.0, y + BUTTON.1),
                "Turn On Input",
                Look::Button,
            );
            y += BUTTON.1 + 10.0;
        }
        l.heading("Input level", Point::new(x, y + 12.0));
        let meter_y = y + 22.0;
        l.bars.push(Bar {
            rect: Rect::new(
                x,
                meter_y,
                (x + METER_WIDTH).min(panel.x1 - PAD),
                meter_y + METER_HEIGHT,
            ),
            fraction: self.in_meter.fraction(),
        });
        let hint = if self.input_open {
            "Speak or play: the meter moves."
        } else {
            "The meter moves once an input is open."
        };
        l.line(hint, Point::new(x, meter_y + 34.0), 12.0, true);
    }

    fn midi_page(&self, l: &mut Layout, panel: Rect, options: &[(String, Choice)], top: f64) {
        let x = panel.x0 + PAD;
        l.heading("MIDI inputs", Point::new(x, top + 12.0));
        let mut y = top + 18.0;
        if options.is_empty() {
            l.line(
                "No MIDI input found. Plug in a keyboard, then open this setup again.",
                Point::new(x, y + 16.0),
                12.5,
                true,
            );
            y += 30.0;
        } else {
            y = self.rows(l, panel, options, y, VISIBLE_ROWS + 1, true) + 4.0;
        }
        y += 16.0;
        l.lamp = Some((Point::new(x + 8.0, y + 8.0), self.lamp_lit()));
        let text = match self.last_key {
            Some(key) if self.lamp_lit() => {
                format!("Note received: {}", atomik_model::note_name(key))
            }
            Some(key) => format!("Last note: {}", atomik_model::note_name(key)),
            None => "Play a key on your keyboard.".to_owned(),
        };
        l.line(text, Point::new(x + 26.0, y + 13.0), 13.0, false);
        l.line(
            "Each key you play also sounds a soft tone, so you know it works.",
            Point::new(x, y + 38.0),
            12.0,
            true,
        );
        let by = y + 54.0;
        l.widget(
            Kind::SetUpController,
            Rect::new(x, by, x + 170.0, by + BUTTON.1),
            "Set Up Controller...",
            Look::Button,
        );
        l.line(
            "For a controller with knobs and faders.",
            Point::new(x + 184.0, by + 18.0),
            12.0,
            true,
        );
    }

    /// The middle of the widget that does `kind`, for a window of size `view`, when it is on
    /// the page (tests and the headless session click it).
    pub fn center_of(&self, view: Size, kind: &Kind) -> Option<Point> {
        self.layout(view)
            .widgets
            .iter()
            .find(|w| w.kind == *kind)
            .map(|w| w.rect.center())
    }

    /// The widgets of the page now (tests).
    pub fn widgets(&self, view: Size) -> Vec<Widget> {
        self.layout(view).widgets
    }

    /// The text of the panel in drawing order (tests and the headless session).
    pub fn lines(&self, view: Size) -> Vec<String> {
        let l = self.layout(view);
        l.lines
            .into_iter()
            .map(|x| x.text)
            .chain(l.widgets.into_iter().map(|w| w.label))
            .collect()
    }

    /// The page the panel takes `kind` to, or the outcome it needs the shell for.
    fn press(&mut self, kind: Kind) -> SetupOutcome {
        match kind {
            Kind::Pick(choice) => SetupOutcome::Pick(choice),
            Kind::Test => {
                self.tested = true;
                SetupOutcome::Test
            }
            Kind::TurnOnInput => SetupOutcome::TurnOnInput,
            Kind::SetUpController => SetupOutcome::SetUpController,
            Kind::Back => {
                self.back();
                SetupOutcome::Open
            }
            Kind::Next => {
                self.next();
                SetupOutcome::Open
            }
            Kind::Skip => SetupOutcome::Skip,
            Kind::Done => SetupOutcome::Done,
        }
    }

    /// Handles one event; the panel is modal and swallows them all.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> SetupOutcome {
        match event {
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                if modifiers.ctrl || modifiers.alt || modifiers.meta {
                    return SetupOutcome::Open;
                }
                match key {
                    Key::Escape => return self.press(Kind::Skip),
                    Key::Enter => {
                        return self.press(if self.is_last() {
                            Kind::Done
                        } else {
                            Kind::Next
                        });
                    }
                    _ => {}
                }
            }
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Wheel { delta, .. } => {
                let rows = self
                    .options()
                    .iter()
                    .filter(|(_, c)| match self.page {
                        Page::Output => matches!(c, Choice::Device(_)),
                        Page::Input => matches!(c, Choice::Input(_)),
                        Page::Midi => matches!(c, Choice::MidiInput(_)),
                    })
                    .count();
                let shown = if self.page == Page::Output {
                    VISIBLE_ROWS
                } else {
                    VISIBLE_ROWS + 1
                };
                let at = self.page.index();
                let step = if delta.y > 0.0 { -1 } else { 1 };
                let max = rows.saturating_sub(shown) as isize;
                self.first[at] = (self.first[at] as isize + step).clamp(0, max) as usize;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let hit = self
                    .layout(view)
                    .widgets
                    .into_iter()
                    .find(|w| w.rect.contains(*pos));
                if let Some(widget) = hit {
                    return self.press(widget.kind);
                }
            }
            _ => {}
        }
        SetupOutcome::Open
    }

    /// Paints the dimmed backdrop and the panel.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        canvas.fill_with(Rect::from_origin_size(Point::ZERO, view), theme.veil);
        let l = self.layout(view);
        let shape = RoundedRect::from_rect(l.panel, 10.0);
        canvas.fill_with(shape, theme.browser_bg);
        canvas.stroke_with(shape, 1.0, theme.section_line);
        for line in &l.lines {
            let width = (l.panel.x1 - PAD - line.at.x).max(40.0);
            let text = fit_label_in(canvas, &line.text, width, line.size);
            let color = if line.warn {
                theme.warning
            } else if line.muted {
                theme.text_muted
            } else {
                theme.text
            };
            let run = TextRun::new(text, line.at, line.size, color);
            canvas.text(&if line.bold { run.weight(700) } else { run });
        }
        for bar in &l.bars {
            let track = RoundedRect::from_rect(bar.rect, 3.0);
            canvas.fill_with(track, theme.knob_track);
            if bar.fraction > 0.0 {
                let fill = Rect::new(
                    bar.rect.x0,
                    bar.rect.y0,
                    bar.rect.x0 + bar.rect.width() * f64::from(bar.fraction),
                    bar.rect.y1,
                );
                let hot = bar.fraction > 0.95;
                canvas.fill_with(
                    RoundedRect::from_rect(fill, 3.0),
                    if hot { theme.clip } else { theme.io_connected },
                );
            }
            canvas.stroke_with(track, 1.0, theme.section_line);
        }
        if let Some((at, lit)) = l.lamp {
            let lamp = atomik_ui::kurbo::Circle::new(at, 6.0);
            canvas.fill_with(
                lamp,
                if lit {
                    theme.io_connected
                } else {
                    theme.led_off
                },
            );
            canvas.stroke_with(lamp, 1.0, theme.section_line);
        }
        for w in &l.widgets {
            self.paint_widget(canvas, theme, w);
        }
    }

    fn paint_widget(&self, canvas: &mut dyn Canvas, theme: &Theme, w: &Widget) {
        let r = w.rect;
        let hovered = self.hover.is_some_and(|p| r.contains(p));
        let selected = matches!(&w.kind, Kind::Pick(c) if self.is_selected(c));
        let shape = RoundedRect::from_rect(r, if w.look == Look::Chip { 5.0 } else { 6.0 });
        match w.look {
            Look::Chip | Look::Radio | Look::Check => {
                if selected {
                    canvas.fill_with(shape, theme.selection);
                } else if hovered {
                    canvas.fill_with(shape, theme.browser_row);
                }
                if w.look == Look::Chip {
                    canvas.stroke_with(shape, 1.0, theme.section_line);
                    canvas.text(
                        &TextRun::new(
                            w.label.clone(),
                            Point::new(r.center().x, r.y0 + 17.5),
                            13.0,
                            theme.text,
                        )
                        .align(Align::Center),
                    );
                    return;
                }
                let c = Point::new(r.x0 + 12.0, r.center().y);
                if w.look == Look::Check {
                    let b = Rect::new(c.x - 5.0, c.y - 5.0, c.x + 5.0, c.y + 5.0);
                    canvas.stroke_with(RoundedRect::from_rect(b, 1.5), 1.2, theme.text_muted);
                    if selected {
                        canvas.fill_with(b.inset(-2.5), theme.text);
                    }
                } else {
                    let dot = atomik_ui::kurbo::Circle::new(c, 5.0);
                    canvas.stroke_with(dot, 1.2, theme.text_muted);
                    if selected {
                        canvas.fill_with(atomik_ui::kurbo::Circle::new(c, 2.8), theme.text);
                    }
                }
                let label = fit_label_in(canvas, &w.label, r.width() - 34.0, 13.0);
                canvas.text(&TextRun::new(
                    label,
                    Point::new(r.x0 + 26.0, r.y0 + 16.5),
                    13.0,
                    theme.text,
                ));
            }
            Look::Button | Look::Primary => {
                let primary = w.look == Look::Primary;
                let fill = if primary {
                    theme.focus
                } else if hovered {
                    theme.selection
                } else {
                    theme.browser_row
                };
                canvas.fill_with(shape, fill);
                canvas.text(
                    &TextRun::new(
                        w.label.clone(),
                        Point::new(r.center().x, r.y0 + 18.5),
                        13.0,
                        if primary { theme.on_accent } else { theme.text },
                    )
                    .align(Align::Center)
                    .weight(600),
                );
            }
        }
    }
}
