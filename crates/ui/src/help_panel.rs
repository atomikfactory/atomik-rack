//! The Help panel of Help Mode: the long form of what a tooltip says.
//!
//! While Help Mode is on the shell docks this strip above the status line. It shows the card of
//! the control under the pointer: its title, a few sentences, the shortcut of the command it
//! runs and "More: <section>", a link into the manual. The panel owns no state: the app hands
//! [`paint`] the card and the rectangle, and asks [`link_rect`] where a click opens the manual.
//! The text is wrapped by the measured text widget ([`crate::paint::measure`]), so the panel
//! follows the glyphs actually drawn.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::paint::{estimate_width, fit_label_weight, measure};
use crate::theme::Theme;
use vello::kurbo::{Point, Rect};

/// The height of the docked panel in logical pixels.
pub const HELP_PANEL_HEIGHT: f64 = 92.0;
/// The room kept at both sides of the text.
const PAD: f64 = 14.0;
/// Type size of the title.
const TITLE_SIZE: f32 = 12.5;
/// Type size of the text.
const BODY_SIZE: f32 = 11.5;
/// Type size of the link line.
const LINK_SIZE: f32 = 11.0;
/// Distance between two text lines.
const LINE: f64 = 14.5;
/// The most text lines the panel shows (the last one is cut with a dot when the text is longer).
pub const MAX_LINES: usize = 3;

/// What the panel says about one control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpCard {
    /// The name of the control.
    pub title: String,
    /// What it does, one to four sentences (may be empty).
    pub text: String,
    /// The shortcut of the command it runs, as the keyboard sheet writes it.
    pub shortcut: Option<String>,
    /// The manual section that explains it more (the text after "More: ").
    pub more: Option<String>,
}

impl HelpCard {
    /// A card with a `title` and a `text`.
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            shortcut: None,
            more: None,
        }
    }

    /// The same, with the `shortcut` of its command.
    #[must_use]
    pub fn with_shortcut(mut self, shortcut: Option<String>) -> Self {
        self.shortcut = shortcut;
        self
    }

    /// The same, with the manual section it links to.
    #[must_use]
    pub fn with_more(mut self, section: impl Into<String>) -> Self {
        self.more = Some(section.into());
        self
    }

    /// The text of the link line.
    pub fn link_text(&self) -> Option<String> {
        self.more.as_ref().map(|s| format!("More: {s}"))
    }
}

/// Splits `text` into lines no wider than `width` at `size` and `weight`, measured on the
/// canvas, breaking at spaces (a word wider than the line is cut). At most `max` lines come
/// back; when the text is longer the last line ends in a dot.
pub fn wrap(
    c: &mut dyn Canvas,
    text: &str,
    width: f64,
    size: f32,
    weight: u16,
    max: usize,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let tried = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if measure(c, &tried, size, weight) <= width || line.is_empty() {
            line = tried;
        } else {
            lines.push(std::mem::take(&mut line));
            if lines.len() == max {
                // Out of lines with words left: the last one says so.
                let last = lines.pop().unwrap_or_default();
                lines.push(fit_label_weight(
                    c,
                    &format!("{last} {word}"),
                    width,
                    size,
                    weight,
                ));
                return lines;
            }
            line = word.to_owned();
        }
        if words.peek().is_none() && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
    }
    for l in &mut lines {
        if measure(c, l, size, weight) > width {
            *l = fit_label_weight(c, l, width, size, weight);
        }
    }
    lines
}

/// Where a click opens the manual: the "More: ..." line of `card` inside the panel `rect`
/// (`None` for a card without a link). The width is estimated, so the rectangle does not
/// depend on a text engine.
pub fn link_rect(rect: Rect, card: &HelpCard) -> Option<Rect> {
    let text = card.link_text()?;
    let width = estimate_width(&text, LINK_SIZE) + 4.0;
    let y1 = rect.y1 - 6.0;
    Some(Rect::new(
        rect.x0 + PAD - 2.0,
        y1 - 16.0,
        (rect.x0 + PAD + width).min(rect.x1 - PAD),
        y1,
    ))
}

/// Paints the panel into `rect` (logical pixels, the canvas scaled) with `card`.
pub fn paint(c: &mut dyn Canvas, theme: &Theme, rect: Rect, card: &HelpCard) {
    c.fill_with(rect, theme.browser_bg);
    c.fill_with(
        Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + 1.0),
        theme.section_line,
    );
    let x = rect.x0 + PAD;
    let room = (rect.width() - 2.0 * PAD).max(40.0);
    let mut title_room = room;
    if let Some(shortcut) = &card.shortcut {
        let shown = format!("Shortcut: {shortcut}");
        let w = measure(c, &shown, BODY_SIZE, 600);
        c.text(
            &TextRun::new(
                shown,
                Point::new(rect.x1 - PAD, rect.y0 + 21.0),
                BODY_SIZE,
                theme.text_muted,
            )
            .weight(600)
            .align(Align::End),
        );
        title_room = (room - w - 16.0).max(40.0);
    }
    let title = fit_label_weight(c, &card.title, title_room, TITLE_SIZE, 700);
    c.text(&TextRun::new(title, Point::new(x, rect.y0 + 21.0), TITLE_SIZE, theme.text).weight(700));
    let link = link_rect(rect, card);
    // The text keeps clear of the link line.
    let lines = if link.is_some() {
        MAX_LINES - 1
    } else {
        MAX_LINES
    };
    for (i, line) in wrap(c, &card.text, room, BODY_SIZE, 400, lines)
        .into_iter()
        .enumerate()
    {
        c.text(&TextRun::new(
            line,
            Point::new(x, rect.y0 + 40.0 + LINE * i as f64),
            BODY_SIZE,
            theme.text,
        ));
    }
    if let (Some(r), Some(text)) = (link, card.link_text()) {
        c.text(&TextRun::new(text, Point::new(x, r.y1 - 3.0), LINK_SIZE, theme.focus).weight(600));
        c.fill_with(
            Rect::new(r.x0 + 2.0, r.y1 - 1.0, r.x1 - 2.0, r.y1),
            theme.focus,
        );
    }
}
