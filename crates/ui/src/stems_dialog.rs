//! The **Separate Stems** dialog: which stems to make, then (the first time) the question to
//! download the models, the download's progress, the separation's progress and a failure.
//!
//! It is a pure view drawn by the app: the host hands it a [`StemsDialogState`] and acts on the
//! [`UiAction::StemsDialog`] choices it returns. It never reads a file or touches the network, so
//! a test renders it from a plain state. Nothing is fetched before **Download** is pressed.
//!
//! * Options: four check boxes (vocals, drums, bass, other), "Mute the original clip" and the
//!   buttons **Separate** and **Cancel**.
//! * Consent: what is downloaded, from where and how big, with **Download** and **Cancel**.
//! * Downloading, Working: a bar with **Cancel**.
//! * Failed: a plain message with **Close** (and **Retry** after a failed download).

use crate::action::UiAction;
use crate::camera_panel::megabytes;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::event::{Key, PointerButton, UiEvent};
use crate::i18n::{tf, tr};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};

/// How many stems there are (vocals, drums, bass, other).
pub const STEM_COUNT: usize = 4;

/// The names of the stems, in the order of the check boxes.
pub const STEM_LABELS: [&str; STEM_COUNT] = ["Vocals", "Drums", "Bass", "Other"];

/// What the separation is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StemsPhase {
    /// Reading the clip's file.
    Reading,
    /// Running the networks.
    Separating,
    /// Writing the stems.
    Storing,
}

/// What the dialog says.
#[derive(Debug, Clone, PartialEq)]
pub enum StemsDialogState {
    /// The choices: which stems, and whether the original is muted.
    Options {
        /// The name of the clip.
        clip: String,
        /// Which stems are made.
        stems: [bool; STEM_COUNT],
        /// Mute the original clip.
        mute_original: bool,
    },
    /// The question: download the models?
    Consent {
        /// Where they come from (a host name).
        source: String,
        /// How many bytes the download is.
        bytes: u64,
    },
    /// The download is running.
    Downloading {
        /// Bytes received so far.
        received: u64,
        /// Bytes expected.
        total: u64,
    },
    /// The separation is running.
    Working {
        /// Which part of the work.
        phase: StemsPhase,
        /// How far the separation is, `0.0..=1.0`.
        progress: f32,
    },
    /// It did not finish.
    Failed {
        /// Why, in plain words.
        message: String,
        /// Offer to try the download again.
        retry: bool,
    },
}

/// A button or box of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StemsDialogChoice {
    /// Switch a stem on or off.
    Toggle(usize),
    /// Switch "Mute the original clip".
    ToggleMute,
    /// Start (asks to download first when the models are missing).
    Separate,
    /// Start the download.
    Download,
    /// Close the dialog, or stop what runs.
    Cancel,
    /// Try the download again.
    Retry,
    /// Close the message.
    Close,
}

impl StemsDialogChoice {
    /// The button label (check boxes have their own text).
    pub fn label(self) -> &'static str {
        match self {
            StemsDialogChoice::Separate => "Separate",
            StemsDialogChoice::Download => "Download",
            StemsDialogChoice::Cancel => "Cancel",
            StemsDialogChoice::Retry => "Retry",
            StemsDialogChoice::Close => "Close",
            StemsDialogChoice::Toggle(i) => STEM_LABELS[i.min(STEM_COUNT - 1)],
            StemsDialogChoice::ToggleMute => "Mute the original clip",
        }
    }
}

const DIALOG_W: f64 = 520.0;
const OPTIONS_H: f64 = 292.0;
const OTHER_H: f64 = 190.0;
const BUTTON_W: f64 = 96.0;
const BUTTON_H: f64 = 28.0;
const PAD: f64 = 8.0;
const ROW_H: f64 = 24.0;
const BOX: f64 = 14.0;

/// The dialog. See the module documentation.
#[derive(Debug)]
pub struct StemsDialogView {
    scale: f64,
    size: (f64, f64),
    state: StemsDialogState,
    hover: Option<Point>,
    repaint: bool,
}

impl StemsDialogView {
    /// A dialog showing `state` in a window of `size` logical pixels.
    pub fn new(size: (f64, f64), state: StemsDialogState) -> Self {
        Self {
            scale: 1.0,
            size,
            state,
            hover: None,
            repaint: true,
        }
    }

    /// Asks for a repaint (the theme changed).
    pub fn invalidate(&mut self) {
        self.repaint = true;
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The window size in logical pixels.
    pub fn set_size(&mut self, size: (f64, f64)) {
        if size != self.size {
            self.size = size;
            self.repaint = true;
        }
    }

    /// Shows `state`; a repaint only when it changed.
    pub fn set_state(&mut self, state: StemsDialogState) {
        if state != self.state {
            self.state = state;
            self.repaint = true;
        }
    }

    /// What the dialog shows.
    pub fn state(&self) -> &StemsDialogState {
        &self.state
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn height(&self) -> f64 {
        if matches!(self.state, StemsDialogState::Options { .. }) {
            OPTIONS_H
        } else {
            OTHER_H
        }
    }

    /// The card, centred in the window.
    pub fn card(&self) -> Rect {
        let h = self.height();
        let x0 = ((self.size.0 - DIALOG_W) / 2.0).max(0.0);
        let y0 = ((self.size.1 - h) / 2.0).max(0.0);
        Rect::new(x0, y0, x0 + DIALOG_W, y0 + h)
    }

    /// The check boxes (with their text) of the options, top to bottom: the four stems, then
    /// the mute box. Empty in the other states.
    pub fn boxes(&self) -> Vec<(StemsDialogChoice, Rect)> {
        if !matches!(self.state, StemsDialogState::Options { .. }) {
            return Vec::new();
        }
        let card = self.card();
        let mut out = Vec::new();
        for i in 0..STEM_COUNT {
            let y = card.y0 + 82.0 + i as f64 * ROW_H;
            out.push((
                StemsDialogChoice::Toggle(i),
                Rect::new(card.x0 + 18.0, y, card.x1 - 18.0, y + ROW_H),
            ));
        }
        let y = card.y0 + 82.0 + STEM_COUNT as f64 * ROW_H + 10.0;
        out.push((
            StemsDialogChoice::ToggleMute,
            Rect::new(card.x0 + 18.0, y, card.x1 - 18.0, y + ROW_H),
        ));
        out
    }

    /// The buttons the state offers, right to left from the corner of the card.
    pub fn choices(&self) -> Vec<(StemsDialogChoice, Rect)> {
        let card = self.card();
        let list: Vec<StemsDialogChoice> = match &self.state {
            StemsDialogState::Options { stems, .. } => {
                let _ = stems;
                vec![StemsDialogChoice::Separate, StemsDialogChoice::Cancel]
            }
            StemsDialogState::Consent { .. } => {
                vec![StemsDialogChoice::Download, StemsDialogChoice::Cancel]
            }
            StemsDialogState::Downloading { .. } | StemsDialogState::Working { .. } => {
                vec![StemsDialogChoice::Cancel]
            }
            StemsDialogState::Failed { retry, .. } => {
                if *retry {
                    vec![StemsDialogChoice::Retry, StemsDialogChoice::Close]
                } else {
                    vec![StemsDialogChoice::Close]
                }
            }
        };
        let y = card.y1 - PAD * 2.0 - BUTTON_H;
        list.iter()
            .enumerate()
            .map(|(n, choice)| {
                let x1 = card.x1 - 2.0 * PAD - n as f64 * (BUTTON_W + PAD);
                (*choice, Rect::new(x1 - BUTTON_W, y, x1, y + BUTTON_H))
            })
            .collect()
    }

    /// The title and the lines of text of the state, in the interface language.
    pub fn lines(&self) -> (String, Vec<String>) {
        match &self.state {
            StemsDialogState::Options { clip, .. } => (
                tr("Separate Stems").into_owned(),
                vec![
                    tf("Clip: {name}", &[("name", clip)]),
                    tr("Each stem becomes a new track under the clip.").into_owned(),
                ],
            ),
            StemsDialogState::Consent { source, bytes } => (
                tr("Download the stem models?").into_owned(),
                vec![
                    tf(
                        "Stem separation needs four model files ({size}) from {source}.",
                        &[("size", &megabytes(*bytes)), ("source", source)],
                    ),
                    tr("Nothing about you or this computer is sent.").into_owned(),
                    tr("The models are open source (MIT licence) and are downloaded once.")
                        .into_owned(),
                ],
            ),
            StemsDialogState::Downloading { received, total } => (
                tr("Downloading the stem models").into_owned(),
                vec![tf(
                    "{received} of {total}",
                    &[
                        ("received", &megabytes(*received)),
                        ("total", &megabytes(*total)),
                    ],
                )],
            ),
            StemsDialogState::Working { phase, progress } => (
                tr("Separating stems").into_owned(),
                vec![
                    match phase {
                        StemsPhase::Reading => tr("Reading the clip's audio").into_owned(),
                        StemsPhase::Separating => tf(
                            "Separating: {percent}%",
                            &[("percent", &format!("{:.0}", progress * 100.0))],
                        ),
                        StemsPhase::Storing => tr("Writing the stems").into_owned(),
                    },
                    tr("The song keeps playing. This can take a few minutes.").into_owned(),
                ],
            ),
            StemsDialogState::Failed { message, .. } => (
                tr("Stem separation did not finish").into_owned(),
                wrap(message, 62),
            ),
        }
    }

    /// Handles one event. The dialog is modal: it takes every event while it shows.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let ask = |choice| UiAction::StemsDialog(choice);
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                if self.hover != Some(*pos) {
                    self.hover = Some(*pos);
                    self.repaint = true;
                }
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
                if let Some((choice, _)) = self
                    .choices()
                    .into_iter()
                    .chain(self.boxes())
                    .find(|(_, r)| r.contains(*pos))
                {
                    out.push(ask(choice));
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                let first = self.choices().first().map(|(c, _)| *c);
                let last = self.choices().last().map(|(c, _)| *c);
                match key {
                    Key::Escape => out.extend(last.map(ask)),
                    Key::Enter => out.extend(first.map(ask)),
                    _ => {}
                }
            }
            _ => {}
        }
        out
    }

    /// Paints the dimmed window and the card.
    pub fn paint(&mut self, c: &mut dyn Canvas, t: &Theme) {
        c.push_transform(Affine::scale(self.scale));
        c.fill_with(
            Rect::new(0.0, 0.0, self.size.0, self.size.1),
            t.background.with_alpha(0.6),
        );
        let card = self.card();
        let shape = RoundedRect::from_rect(card, 10.0);
        c.fill_with(shape, t.browser_bg);
        c.stroke_with(shape, 1.0, t.section_line);
        let (title, lines) = self.lines();
        c.text(
            &TextRun::verbatim(
                title,
                Point::new(card.x0 + 18.0, card.y0 + 30.0),
                15.0,
                t.text,
            )
            .weight(600),
        );
        for (n, line) in lines.iter().enumerate() {
            c.text(&TextRun::verbatim(
                line.clone(),
                Point::new(card.x0 + 18.0, card.y0 + 56.0 + n as f64 * 18.0),
                11.5,
                if n == 0 { t.text } else { t.text_muted },
            ));
        }
        if let StemsDialogState::Options {
            stems,
            mute_original,
            ..
        } = &self.state
        {
            for (choice, r) in self.boxes() {
                let on = match choice {
                    StemsDialogChoice::Toggle(i) => stems[i.min(STEM_COUNT - 1)],
                    _ => *mute_original,
                };
                let lit = self.hover.is_some_and(|p| r.contains(p));
                if lit {
                    c.fill_with(RoundedRect::from_rect(r, 4.0), t.browser_row);
                }
                let bx = Rect::new(
                    r.x0 + 6.0,
                    r.y0 + (ROW_H - BOX) / 2.0,
                    r.x0 + 6.0 + BOX,
                    r.y0 + (ROW_H + BOX) / 2.0,
                );
                c.stroke_with(RoundedRect::from_rect(bx, 3.0), 1.0, t.section_line);
                if on {
                    c.fill_with(RoundedRect::from_rect(bx.inset(-3.0), 2.0), t.focus);
                }
                c.text(&TextRun::new(
                    choice.label(),
                    Point::new(bx.x1 + 10.0, r.y0 + 16.5),
                    12.0,
                    t.text,
                ));
            }
        }
        let bar = match &self.state {
            StemsDialogState::Downloading { received, total } => Some(if *total == 0 {
                0.0
            } else {
                (*received as f64 / *total as f64).clamp(0.0, 1.0)
            }),
            StemsDialogState::Working { phase, progress } => Some(match phase {
                StemsPhase::Reading => 0.0,
                StemsPhase::Separating => f64::from(progress.clamp(0.0, 1.0)),
                StemsPhase::Storing => 1.0,
            }),
            _ => None,
        };
        if let Some(part) = bar {
            let r = Rect::new(
                card.x0 + 18.0,
                card.y0 + 100.0,
                card.x1 - 18.0,
                card.y0 + 110.0,
            );
            c.fill_with(RoundedRect::from_rect(r, 4.0), t.browser_row);
            c.fill_with(
                RoundedRect::from_rect(Rect::new(r.x0, r.y0, r.x0 + r.width() * part, r.y1), 4.0),
                t.focus,
            );
        }
        for (choice, r) in self.choices() {
            let lit = self.hover.is_some_and(|p| r.contains(p));
            c.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if lit { t.selection } else { t.browser_row },
            );
            c.text(
                &TextRun::new(
                    choice.label(),
                    Point::new(r.center().x, r.y0 + 18.0),
                    12.0,
                    t.text,
                )
                .align(Align::Center),
            );
        }
        c.pop_transform();
        self.repaint = false;
    }

    /// The tooltip of a button or box.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if let Some((choice, r)) = self.boxes().into_iter().find(|(_, r)| r.contains(pos)) {
            let text = match choice {
                StemsDialogChoice::ToggleMute => {
                    "Mute the clip that is separated, so only the stems play"
                }
                _ => "Make this stem as a new track",
            };
            return Some(Help::new(r, text));
        }
        let (choice, r) = self.choices().into_iter().find(|(_, r)| r.contains(pos))?;
        let text = match choice {
            StemsDialogChoice::Separate => "Make a track for each stem that is checked",
            StemsDialogChoice::Download => {
                "Download the four model files once and separate the clip"
            }
            StemsDialogChoice::Cancel => "Close this, or stop what is running",
            StemsDialogChoice::Retry => "Try the download again",
            StemsDialogChoice::Close => "Close this message",
            StemsDialogChoice::Toggle(_) | StemsDialogChoice::ToggleMute => return None,
        };
        Some(Help::new(r, text))
    }
}

/// `text` broken at spaces into lines of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}
