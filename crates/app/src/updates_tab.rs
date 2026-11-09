//! The "New versions" section of the Settings > Video tab: the answer to "Check for new versions
//! once a day?".
//!
//! One row with **Yes** and **No**, and the words about the one network request the program
//! makes: what it asks, what it sends, and that nothing is downloaded or installed. While the
//! question is unanswered neither button is lit and the program asks nothing. The answer is a
//! workflow preference ([`WorkflowPrefs::update_check`](crate::keymap_store::WorkflowPrefs))
//! edited in a draft and saved with the other preferences when the dialog is applied. Help >
//! Check for Updates checks on request. (The Interface and Workflow tabs are full and the tab
//! strip has no room for another tab, so the section sits under the FFmpeg rows.)

use crate::video_tab::wrap;
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, PointerButton, TextRun, Theme, UiEvent};

const ROW: f64 = 44.0;
const BUTTON_W: f64 = 72.0;
const GAP: f64 = 6.0;
/// The room the section heading takes.
const HEADING: f64 = 26.0;
/// The characters of a wrapped line of the notes.
const WRAP: usize = 92;

/// The words under the row.
const NOTES: [&str; 3] = [
    "Atomik Rack asks github.com for the list of releases once a day, and only after you say Yes. It is the only network request the program makes.",
    "It sends the address and the User-Agent atomik-rack/<version>: no identifier, no cookie, nothing about you or this computer. A newer version is only announced; nothing is downloaded or installed.",
    "Help > Check for Updates asks now, whatever the answer here.",
];

/// Where the section's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdatesLayout {
    /// Where the heading is drawn.
    pub heading: Point,
    /// The whole row.
    pub rect: Rect,
    /// **Yes**.
    pub yes: Rect,
    /// **No**.
    pub no: Rect,
    /// The notes, line by line.
    pub notes: Vec<(Point, String)>,
}

/// The section's draft.
#[derive(Debug, Clone)]
pub struct UpdatesSection {
    answer: Option<bool>,
}

impl UpdatesSection {
    /// A section editing `answer` (`None`: the question is not answered).
    pub fn new(answer: Option<bool>) -> Self {
        Self { answer }
    }

    /// The edited answer.
    pub fn answer(&self) -> Option<bool> {
        self.answer
    }

    /// Lays the section out with its left edge at `x0`, `inner` wide, starting at `y`; returns the
    /// layout and the `y` below it.
    pub fn layout_at(x0: f64, inner: f64, y: f64) -> (UpdatesLayout, f64) {
        let mut y = y;
        let heading = Point::new(x0, y + 12.0);
        y += HEADING;
        let rect = Rect::new(x0, y, x0 + inner, y + ROW);
        let no = Rect::new(x0 + inner - BUTTON_W, y + 8.0, x0 + inner, y + 36.0);
        let yes = Rect::new(no.x0 - GAP - BUTTON_W, y + 8.0, no.x0 - GAP, y + 36.0);
        y += ROW + 8.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(note, WRAP) {
                notes.push((Point::new(x0, y + 12.0), line));
                y += 16.0;
            }
            y += 6.0;
        }
        (
            UpdatesLayout {
                heading,
                rect,
                yes,
                no,
                notes,
            },
            y,
        )
    }

    /// The height of the section.
    pub fn height() -> f64 {
        Self::layout_at(0.0, 600.0, 0.0).1
    }

    /// A press at `pos`: Yes or No sets the draft. Returns whether it was one of them.
    pub fn press(&mut self, layout: &UpdatesLayout, event: &UiEvent) -> bool {
        let UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            ..
        } = event
        else {
            return false;
        };
        if layout.yes.contains(*pos) {
            self.answer = Some(true);
            true
        } else if layout.no.contains(*pos) {
            self.answer = Some(false);
            true
        } else {
            false
        }
    }

    /// Paints the section; `hover` is the pointer, when it is over the dialog.
    pub fn paint(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        l: &UpdatesLayout,
        hover: Option<Point>,
    ) {
        canvas.text(&TextRun::new("New versions", l.heading, 13.0, theme.text).weight(600));
        canvas.text(&TextRun::new(
            "Check for new versions once a day",
            Point::new(l.rect.x0 + 36.0, l.rect.y0 + 20.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            match self.answer {
                None => "Not answered yet: nothing is asked",
                Some(true) => "Yes: once a day, when Atomik Rack starts",
                Some(false) => "No: only when you choose Help > Check for Updates",
            },
            Point::new(l.rect.x0 + 36.0, l.rect.y0 + 37.0),
            11.0,
            theme.text_muted,
        ));
        for (r, label, on) in [
            (l.yes, "Yes", self.answer == Some(true)),
            (l.no, "No", self.answer == Some(false)),
        ] {
            let lit = hover.is_some_and(|p| r.contains(p));
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if on || lit {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 15.5),
                    12.0,
                    theme.text,
                )
                .weight(if on { 700 } else { 400 })
                .align(Align::Center),
            );
        }
        for (at, line) in &l.notes {
            canvas.text(&TextRun::new(line.clone(), *at, 11.5, theme.text_muted));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::Modifiers;

    fn press_at(section: &mut UpdatesSection, l: &UpdatesLayout, at: Point) -> bool {
        section.press(
            l,
            &UiEvent::PointerDown {
                pos: at,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            },
        )
    }

    #[test]
    fn the_buttons_set_the_draft() {
        let mut section = UpdatesSection::new(None);
        assert_eq!(section.answer(), None);
        let (l, _) = UpdatesSection::layout_at(10.0, 600.0, 100.0);
        assert!(press_at(&mut section, &l, l.yes.center()));
        assert_eq!(section.answer(), Some(true));
        assert!(press_at(&mut section, &l, l.no.center()));
        assert_eq!(section.answer(), Some(false));
        assert!(press_at(&mut section, &l, l.yes.center()));
        assert_eq!(section.answer(), Some(true));
        // A click elsewhere changes nothing, and nothing un-answers the question.
        assert!(!press_at(&mut section, &l, Point::new(0.0, 0.0)));
        assert_eq!(section.answer(), Some(true));
    }

    #[test]
    fn a_secondary_click_changes_nothing() {
        let mut section = UpdatesSection::new(None);
        let (l, _) = UpdatesSection::layout_at(0.0, 600.0, 0.0);
        let event = UiEvent::PointerDown {
            pos: l.yes.center(),
            button: PointerButton::Secondary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        };
        assert!(!section.press(&l, &event));
        assert_eq!(section.answer(), None);
    }

    #[test]
    fn the_buttons_are_inside_the_row_and_apart() {
        let (l, end) = UpdatesSection::layout_at(0.0, 600.0, 0.0);
        assert!(l.yes.x1 < l.no.x0);
        assert!(l.no.x1 <= l.rect.x1 && l.yes.y0 >= l.rect.y0 && l.yes.y1 <= l.rect.y1);
        assert!(l.notes.last().unwrap().0.y < end);
        assert_eq!(UpdatesSection::height(), end);
    }

    #[test]
    fn the_notes_say_what_is_sent() {
        let (l, _) = UpdatesSection::layout_at(0.0, 600.0, 0.0);
        let text: String = l
            .notes
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for word in [
            "only network request",
            "User-Agent",
            "no identifier",
            "nothing is downloaded",
            "Check for Updates",
        ] {
            assert!(text.contains(word), "{word}");
        }
    }
}
