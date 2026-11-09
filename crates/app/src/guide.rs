//! The guided first song: a small floating panel that walks a new user through seven steps (an
//! instrument, some notes, a loop, a drum pattern, a mix, save, export).
//!
//! The panel is not a modal. It never takes the keyboard (every key goes to the song as if it was
//! not there), it closes at any time and the song works normally under it: the shell gives it only
//! the pointer events that land inside its own rectangle. What it shows is not saved in the song
//! or anywhere else.
//!
//! Each step says what to do and watches for the result. [`Progress::read`] looks at the song (and
//! at two facts the app keeps: whether the song has a file and whether an export has finished)
//! and a step is done when every one of its parts is ([`Step::parts`]). [`Guide::update`] moves on
//! when a step *becomes* done while it is on screen; a step that was already done when it came up
//! (the user went ahead, or came back with Back) waits for the Next button instead of racing
//! past. Back, Skip and Close are always there, and so is a button that moves the panel to the
//! other side of the window.
//!
//! Where the interface has a control a step talks about, [`Step::spot`] names it and the shell
//! finds its rectangle ([`paint_outline`] draws it).

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, RackDoc};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Help, PointerButton, TextRun, Theme, UiEvent};

/// The number of steps.
pub const STEP_COUNT: usize = 7;

const WIDTH: f64 = 300.0;
const HEIGHT: f64 = 212.0;
/// The gap between the panel and the edges of the area it floats in.
const MARGIN: f64 = 14.0;
const PAD: f64 = 16.0;
const BUTTON: (f64, f64) = (56.0, 26.0);
const BUTTON_GAP: f64 = 6.0;
const TEXT_SIZE: f32 = 11.5;
const TEXT_LINE: f64 = 15.5;
const PART_LINE: f64 = 16.0;
/// The most lines a step's text may take.
pub const TEXT_LINES: usize = 4;
/// The most parts a step may have.
pub const MAX_PARTS: usize = 2;

/// The type id of the drum machine (a drum pattern lives in one).
const DRUM_MACHINE: &str = "atomik.drum-machine";

/// One step of the tour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Add an instrument: a track with a synth on it.
    Instrument,
    /// Play or draw some notes into a clip.
    Notes,
    /// Turn the loop on.
    Loop,
    /// Add a drum machine.
    Drums,
    /// Move a channel level and add an effect.
    Mix,
    /// Save the song.
    Save,
    /// Export it as an audio file.
    Export,
}

/// Where the interface has the control a step is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// The title of this menu of the menu bar.
    Menu(&'static str),
    /// The Loop button of the transport bar.
    Loop,
    /// The song area (the arrangement).
    Song,
    /// The rack.
    Rack,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; STEP_COUNT] = [
        Step::Instrument,
        Step::Notes,
        Step::Loop,
        Step::Drums,
        Step::Mix,
        Step::Save,
        Step::Export,
    ];

    /// The heading of the step.
    pub fn title(self) -> &'static str {
        match self {
            Step::Instrument => "Add an instrument",
            Step::Notes => "Play or draw some notes",
            Step::Loop => "Loop them",
            Step::Drums => "Add a drum pattern",
            Step::Mix => "Mix it",
            Step::Save => "Save your song",
            Step::Export => "Export it",
        }
    }

    /// What to do, in a few lines.
    pub fn text(self) -> &'static str {
        match self {
            Step::Instrument => {
                "Press Ctrl+I (Cmd+I on a Mac) or choose Create > Create Instrument, then pick a \
                 synth from the list."
            }
            Step::Notes => {
                "Draw a clip on the track with the Pencil tool and double-click it to put notes \
                 in the piano roll. Or arm the track, press Record and play."
            }
            Step::Loop => {
                "Set the loop around your clip in the ruler, then press L or click Loop in the \
                 transport bar."
            }
            Step::Drums => {
                "Create a second instrument and pick the Drum Machine, then fill a pattern on \
                 its step buttons."
            }
            Step::Mix => {
                "Move a channel fader to set a level, then add an effect with Ctrl+E (Cmd+E on a \
                 Mac), a reverb or a delay for example."
            }
            Step::Save => {
                "Press Ctrl+S (Cmd+S on a Mac) and choose a place for the song. It keeps its \
                 audio beside it."
            }
            Step::Export => {
                "Choose File > Export > Export Song as Audio File, pick a format and press Export."
            }
        }
    }

    /// Where the control the step is about is.
    pub fn spot(self) -> Spot {
        match self {
            Step::Instrument | Step::Drums => Spot::Menu("Create"),
            Step::Notes => Spot::Song,
            Step::Loop => Spot::Loop,
            Step::Mix => Spot::Rack,
            Step::Save | Step::Export => Spot::Menu("File"),
        }
    }

    /// What the step waits for, each with whether it is done (at most [`MAX_PARTS`]).
    pub fn parts(self, p: &Progress) -> Vec<(&'static str, bool)> {
        match self {
            Step::Instrument => vec![("a track with an instrument", p.instrument)],
            Step::Notes => vec![("a clip with notes", p.notes)],
            Step::Loop => vec![("the loop is on", p.looping)],
            Step::Drums => vec![("a drum machine track", p.drums)],
            Step::Mix => vec![("a channel level moved", p.level), ("an effect", p.effect)],
            Step::Save => vec![("the song has a file", p.saved)],
            Step::Export => vec![("an export finished", p.exported)],
        }
    }

    /// Whether every part of the step is done.
    pub fn done(self, p: &Progress) -> bool {
        self.parts(p).iter().all(|(_, done)| *done)
    }
}

/// What the song and the app show of the steps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// A note track exists.
    pub instrument: bool,
    /// A note clip holds at least one note.
    pub notes: bool,
    /// The loop is on.
    pub looping: bool,
    /// A track plays a drum machine.
    pub drums: bool,
    /// A channel fader is somewhere else than its default.
    pub level: bool,
    /// An effect is in the rack.
    pub effect: bool,
    /// The song has a file.
    pub saved: bool,
    /// An export finished.
    pub exported: bool,
}

impl Progress {
    /// What `doc` shows. `saved`: the song has a file; `exports`: how many exports finished.
    pub fn read(doc: &RackDoc, registry: &Registry, saved: bool, exports: u32) -> Self {
        let note_track =
            |t: &atomik_model::Track| t.lanes.first().is_some_and(|l| l.kind.is_notes());
        let instrument = doc.song.tracks.iter().any(note_track);
        let notes = doc.song.tracks.iter().filter(|t| note_track(t)).any(|t| {
            t.note_lanes().any(|lane| {
                lane.clips
                    .iter()
                    .any(|c| matches!(&c.content, ClipContent::Notes(n) if !n.is_empty()))
            })
        });
        let drums = doc.song.tracks.iter().any(|t| {
            doc.device(&t.device)
                .is_some_and(|d| d.type_id == DRUM_MACHINE)
        });
        let level = doc.console.channels.iter().any(|c| {
            let Some(device) = doc.device(&c.channel) else {
                return false;
            };
            let default = registry
                .get(&device.type_id)
                .and_then(|e| e.descriptor.param_by_key("fader").map(|p| p.default));
            match (device.params.get("fader"), default) {
                (Some(set), Some(default)) => (set - default).abs() > 1e-4,
                _ => false,
            }
        });
        let effect = doc.devices.iter().any(|d| {
            registry
                .get(&d.type_id)
                .is_some_and(|e| e.descriptor.kind == atomik_device_sdk::DeviceKind::Effect)
        });
        Self {
            instrument,
            notes,
            looping: doc.song.loop_enabled,
            drums,
            level,
            effect,
            saved,
            exported: exports > 0,
        }
    }
}

/// A button of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The step before.
    Back,
    /// The step after (Skip while the step is not done, Next when it is).
    Skip,
    /// The panel to the other side of the window.
    Move,
    /// Closes the panel.
    Close,
}

impl Button {
    const ALL: [Button; 4] = [Button::Back, Button::Skip, Button::Move, Button::Close];
}

/// What the panel asks of the shell after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideOutcome {
    /// The event is not the panel's (the pointer is outside it): the song gets it.
    Ignored,
    /// The panel took the event; it stays.
    Consumed,
    /// The panel took the event and wants to close.
    Close,
}

/// The tour: which step it is on and what it saw last.
#[derive(Debug, Clone)]
pub struct Guide {
    /// The step on show; [`STEP_COUNT`] is the closing page.
    step: usize,
    /// The step was seen not done since it came up (so its completion moves on).
    armed: bool,
    /// The panel is on the left side of the window.
    left: bool,
    hover: Option<Button>,
    progress: Progress,
}

impl Default for Guide {
    fn default() -> Self {
        Self::new()
    }
}

impl Guide {
    /// A tour at its first step.
    pub fn new() -> Self {
        Self {
            step: 0,
            armed: false,
            left: false,
            hover: None,
            progress: Progress::default(),
        }
    }

    /// The index of the page on show (0-based; [`STEP_COUNT`] is the closing page).
    pub fn index(&self) -> usize {
        self.step
    }

    /// The step on show (`None` on the closing page).
    pub fn current(&self) -> Option<Step> {
        Step::ALL.get(self.step).copied()
    }

    /// Whether the closing page is on show.
    pub fn finished(&self) -> bool {
        self.step >= STEP_COUNT
    }

    /// Whether the step on show is done.
    pub fn current_done(&self) -> bool {
        self.current().is_some_and(|s| s.done(&self.progress))
    }

    /// The button under the pointer, if any.
    pub fn hovered(&self) -> Option<Button> {
        self.hover
    }

    /// What the panel saw last.
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// Whether the panel is on the left side of the window.
    pub fn on_left(&self) -> bool {
        self.left
    }

    /// Takes a new look at the song. Moves on when the step on show became done since it came up.
    /// True when what the panel shows changed.
    pub fn update(&mut self, progress: Progress) -> bool {
        let before = (self.step, self.progress);
        self.progress = progress;
        if let Some(step) = self.current() {
            if !step.done(&progress) {
                self.armed = true;
            } else if self.armed {
                self.step += 1;
                self.armed = false;
            }
        }
        before != (self.step, self.progress)
    }

    /// The step before (the first stays).
    pub fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
            self.armed = false;
        }
    }

    /// The step after (the closing page follows the last).
    pub fn skip(&mut self) {
        if self.step < STEP_COUNT {
            self.step += 1;
            self.armed = false;
        }
    }

    /// Puts the panel on the other side of the window.
    pub fn flip_side(&mut self) {
        self.left = !self.left;
    }

    /// The panel in the area `bounds` (the window between the menu bar and the status bar).
    pub fn rect(&self, bounds: Rect) -> Rect {
        let w = WIDTH.min((bounds.width() - 2.0 * MARGIN).max(120.0));
        let h = HEIGHT.min((bounds.height() - 2.0 * MARGIN).max(80.0));
        let x0 = if self.left {
            bounds.x0 + MARGIN
        } else {
            bounds.x1 - MARGIN - w
        };
        let y0 = bounds.y1 - MARGIN - h;
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// The rectangle of `button`.
    pub fn button_rect(&self, bounds: Rect, button: Button) -> Rect {
        let r = self.rect(bounds);
        let top = r.y1 - 14.0 - BUTTON.1;
        let x = match button {
            Button::Back => r.x0 + PAD,
            Button::Skip => r.x0 + PAD + BUTTON.0 + BUTTON_GAP,
            Button::Move => r.x1 - PAD - 2.0 * BUTTON.0 - BUTTON_GAP,
            Button::Close => r.x1 - PAD - BUTTON.0,
        };
        Rect::new(x, top, x + BUTTON.0, top + BUTTON.1)
    }

    /// Whether `button` does anything now.
    pub fn enabled(&self, button: Button) -> bool {
        match button {
            Button::Back => self.step > 0,
            Button::Skip => !self.finished(),
            Button::Move | Button::Close => true,
        }
    }

    /// The label of `button`.
    fn label(&self, button: Button) -> &'static str {
        match button {
            Button::Back => "Back",
            Button::Skip if self.current_done() => "Next",
            Button::Skip => "Skip",
            Button::Move => "Move",
            Button::Close => "Close",
        }
    }

    fn button_at(&self, bounds: Rect, pos: Point) -> Option<Button> {
        Button::ALL
            .into_iter()
            .find(|b| self.button_rect(bounds, *b).contains(pos))
    }

    /// Handles an event of the window; `bounds` is the area the panel floats in. Only pointer
    /// events inside the panel are taken.
    pub fn handle(&mut self, event: &UiEvent, bounds: Rect) -> GuideOutcome {
        let area = self.rect(bounds);
        let inside = |pos: &Point| area.contains(*pos);
        match event {
            UiEvent::PointerMove { pos, .. } => {
                if inside(pos) {
                    self.hover = self.button_at(bounds, *pos);
                    GuideOutcome::Consumed
                } else {
                    self.hover = None;
                    GuideOutcome::Ignored
                }
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                GuideOutcome::Ignored
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } if inside(pos) => match self.button_at(bounds, *pos) {
                Some(button) if self.enabled(button) => self.press(button),
                _ => GuideOutcome::Consumed,
            },
            UiEvent::PointerDown { pos, .. }
            | UiEvent::PointerUp { pos, .. }
            | UiEvent::Wheel { pos, .. }
                if inside(pos) =>
            {
                GuideOutcome::Consumed
            }
            _ => GuideOutcome::Ignored,
        }
    }

    /// Presses `button`.
    pub fn press(&mut self, button: Button) -> GuideOutcome {
        match button {
            Button::Back => self.back(),
            Button::Skip => self.skip(),
            Button::Move => self.flip_side(),
            Button::Close => return GuideOutcome::Close,
        }
        GuideOutcome::Consumed
    }

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, bounds: Rect, pos: Point) -> Option<Help> {
        let button = self.button_at(bounds, pos)?;
        let text = match button {
            Button::Back => "Go back to the step before",
            Button::Skip if self.current_done() => "Go on to the next step",
            Button::Skip => "Skip this step without doing it",
            Button::Move => "Move this panel to the other side of the window",
            Button::Close => "Close the guide; the song stays as it is",
        };
        Some(Help::new(self.button_rect(bounds, button), text))
    }

    /// The header, the heading, the text and the parts on show, in drawing order (for tests).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.header()];
        match self.current() {
            Some(step) => {
                lines.push(step.title().to_string());
                lines.extend(wrap(step.text(), WIDTH - 2.0 * PAD, TEXT_SIZE));
                for (what, _) in step.parts(&self.progress) {
                    lines.push(what.to_string());
                }
            }
            None => {
                lines.push(CLOSING_TITLE.to_string());
                lines.extend(wrap(CLOSING_TEXT, WIDTH - 2.0 * PAD, TEXT_SIZE));
            }
        }
        lines
    }

    fn header(&self) -> String {
        if self.finished() {
            "MAKE YOUR FIRST SONG".to_string()
        } else {
            format!(
                "MAKE YOUR FIRST SONG   STEP {} OF {STEP_COUNT}",
                self.step + 1
            )
        }
    }

    /// Paints the panel in the area `bounds`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, bounds: Rect) {
        let r = self.rect(bounds);
        let shape = RoundedRect::from_rect(r, 10.0);
        canvas.fill_with(shape, theme.browser_bg);
        canvas.stroke_with(shape, 1.0, theme.section_line);
        canvas.text(
            &TextRun::new(
                self.header(),
                Point::new(r.x0 + PAD, r.y0 + 24.0),
                10.5,
                theme.text_muted,
            )
            .weight(700),
        );
        let (title, text, parts) = match self.current() {
            Some(step) => (step.title(), step.text(), step.parts(&self.progress)),
            None => (CLOSING_TITLE, CLOSING_TEXT, Vec::new()),
        };
        canvas.text(
            &TextRun::new(title, Point::new(r.x0 + PAD, r.y0 + 46.0), 14.5, theme.text).weight(700),
        );
        for (n, line) in wrap(text, r.width() - 2.0 * PAD, TEXT_SIZE)
            .iter()
            .take(TEXT_LINES)
            .enumerate()
        {
            canvas.text(&TextRun::new(
                line.clone(),
                Point::new(r.x0 + PAD, r.y0 + 68.0 + TEXT_LINE * n as f64),
                TEXT_SIZE,
                theme.text_muted,
            ));
        }
        for (n, (what, done)) in parts.iter().take(MAX_PARTS).enumerate() {
            let y = r.y0 + 140.0 + PART_LINE * n as f64;
            let square = Rect::new(r.x0 + PAD, y - 9.0, r.x0 + PAD + 11.0, y + 2.0);
            let mark = RoundedRect::from_rect(square, 2.5);
            canvas.stroke_with(mark, 1.0, theme.text_muted);
            if *done {
                canvas.fill_with(
                    RoundedRect::from_rect(square.inset(-2.5), 1.0),
                    theme.highlight,
                );
            }
            canvas.text(&TextRun::new(
                (*what).to_string(),
                Point::new(square.x1 + 8.0, y),
                TEXT_SIZE,
                if *done { theme.text } else { theme.text_muted },
            ));
        }
        for button in Button::ALL {
            let rect = self.button_rect(bounds, button);
            let shape = RoundedRect::from_rect(rect, 6.0);
            let enabled = self.enabled(button);
            if enabled && self.hover == Some(button) {
                canvas.fill_with(shape, theme.selection);
            } else {
                canvas.fill_with(shape, theme.browser_row);
            }
            canvas.text(
                &TextRun::new(
                    self.label(button),
                    Point::new(rect.center().x, rect.y0 + 17.5),
                    12.0,
                    if enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                )
                .align(Align::Center),
            );
        }
    }
}

const CLOSING_TITLE: &str = "That is a first song";
const CLOSING_TEXT: &str = "It has an instrument, notes, a loop, drums, a mix, a file and an \
                            export. Everything you did can still be undone. Close this panel \
                            when you like.";

/// Outlines `rect`, the control a step talks about.
pub fn paint_outline(canvas: &mut dyn Canvas, theme: &Theme, rect: Rect) {
    let shape = RoundedRect::from_rect(rect.inset(2.0), 5.0);
    canvas.stroke_with(shape, 2.0, theme.highlight);
}

/// Splits `text` into lines of at most `width` pixels at `size` (words are not broken). The width
/// of a character is estimated, so the lines do not depend on the font.
pub fn wrap(text: &str, width: f64, size: f32) -> Vec<String> {
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
    lines
}

#[cfg(test)]
#[path = "guide_tests.rs"]
mod tests;
