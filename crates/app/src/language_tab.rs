//! The "Language" section of the Settings > Video tab: the language of the interface.
//!
//! One row with a button per choice: **System** (the language of the operating system, English
//! when the program has no text in it), **English** and **Français**. The answer is a
//! workflow preference ([`WorkflowPrefs::language`](crate::keymap_store::WorkflowPrefs)) edited
//! in a draft and saved with the other preferences when the dialog is applied; the shell then
//! changes the language at once. (The Interface and Workflow tabs are full and the tab strip has
//! no room for another tab, so the section sits under the update question, as that one sits under
//! the FFmpeg rows.) The names of the languages are the languages' own words and are never
//! translated.

use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{
    Align, Canvas, CanvasExt, Lang, LanguageChoice, PointerButton, TextRun, Theme, UiEvent,
};

const ROW: f64 = 44.0;
const BUTTON_W: f64 = 96.0;
const GAP: f64 = 6.0;
/// The room the section heading takes.
const HEADING: f64 = 26.0;

/// Where the section's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageLayout {
    /// Where the heading is drawn.
    pub heading: Point,
    /// The whole row.
    pub rect: Rect,
    /// One button per choice, in the order of [`LanguageChoice::ALL`].
    pub buttons: Vec<(Rect, LanguageChoice)>,
}

/// The section's draft.
#[derive(Debug, Clone)]
pub struct LanguageSection {
    choice: LanguageChoice,
}

/// The text of a button: the choice's own words, or "System" in the language in use.
fn button_label(choice: LanguageChoice) -> &'static str {
    match choice {
        LanguageChoice::System => "System",
        LanguageChoice::Is(lang) => lang.name(),
    }
}

impl LanguageSection {
    /// A section editing `choice`.
    pub fn new(choice: LanguageChoice) -> Self {
        Self { choice }
    }

    /// The edited choice.
    pub fn choice(&self) -> LanguageChoice {
        self.choice
    }

    /// Lays the section out with its left edge at `x0`, `inner` wide, starting at `y`; returns the
    /// layout and the `y` below it.
    pub fn layout_at(x0: f64, inner: f64, y: f64) -> (LanguageLayout, f64) {
        let heading = Point::new(x0, y + 12.0);
        let top = y + HEADING;
        let rect = Rect::new(x0, top, x0 + inner, top + ROW);
        let count = LanguageChoice::ALL.len();
        let buttons = LanguageChoice::ALL
            .into_iter()
            .enumerate()
            .map(|(i, choice)| {
                let right = rect.x1 - (count - 1 - i) as f64 * (BUTTON_W + GAP);
                (
                    Rect::new(right - BUTTON_W, top + 8.0, right, top + 36.0),
                    choice,
                )
            })
            .collect();
        (
            LanguageLayout {
                heading,
                rect,
                buttons,
            },
            top + ROW + 8.0,
        )
    }

    /// The height of the section.
    pub fn height() -> f64 {
        Self::layout_at(0.0, 600.0, 0.0).1
    }

    /// A press at `pos`: a button sets the draft. Returns whether it was one of them.
    pub fn press(&mut self, layout: &LanguageLayout, event: &UiEvent) -> bool {
        let UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            ..
        } = event
        else {
            return false;
        };
        match layout.buttons.iter().find(|(r, _)| r.contains(*pos)) {
            Some((_, choice)) => {
                self.choice = *choice;
                true
            }
            None => false,
        }
    }

    /// Paints the section; `hover` is the pointer, when it is over the dialog.
    pub fn paint(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        l: &LanguageLayout,
        hover: Option<Point>,
    ) {
        canvas.text(&TextRun::new("Language", l.heading, 13.0, theme.text).weight(600));
        canvas.text(&TextRun::new(
            "Language of the interface",
            Point::new(l.rect.x0 + 36.0, l.rect.y0 + 20.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            match self.choice {
                LanguageChoice::System => {
                    "System: the language of the computer, English if unknown"
                }
                LanguageChoice::Is(Lang::English) => "English",
                LanguageChoice::Is(Lang::French) => "French: the whole interface, not the manual",
            },
            Point::new(l.rect.x0 + 36.0, l.rect.y0 + 37.0),
            11.0,
            theme.text_muted,
        ));
        for (r, choice) in &l.buttons {
            let on = self.choice == *choice;
            let lit = hover.is_some_and(|p| r.contains(p));
            canvas.fill_with(
                RoundedRect::from_rect(*r, 6.0),
                if on || lit {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            // The names of the languages are verbatim: each is written in its own language.
            let label = button_label(*choice);
            let run = match choice {
                LanguageChoice::System => TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 15.5),
                    12.0,
                    theme.text,
                ),
                LanguageChoice::Is(_) => TextRun::verbatim(
                    label,
                    Point::new(r.center().x, r.y0 + 15.5),
                    12.0,
                    theme.text,
                ),
            };
            canvas.text(&run.weight(if on { 700 } else { 400 }).align(Align::Center));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::Modifiers;

    fn press_at(section: &mut LanguageSection, l: &LanguageLayout, at: Point) -> bool {
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
        let mut section = LanguageSection::new(LanguageChoice::System);
        let (l, _) = LanguageSection::layout_at(10.0, 600.0, 100.0);
        assert_eq!(l.buttons.len(), LanguageChoice::ALL.len());
        for (rect, choice) in l.buttons.clone() {
            assert!(press_at(&mut section, &l, rect.center()));
            assert_eq!(section.choice(), choice);
        }
        assert!(!press_at(&mut section, &l, Point::new(0.0, 0.0)));
        assert_eq!(section.choice(), LanguageChoice::Is(Lang::French));
    }

    #[test]
    fn the_buttons_are_inside_the_row_and_apart() {
        let (l, end) = LanguageSection::layout_at(0.0, 600.0, 0.0);
        for pair in l.buttons.windows(2) {
            assert!(pair[0].0.x1 < pair[1].0.x0);
        }
        for (r, _) in &l.buttons {
            assert!(r.x0 >= l.rect.x0 && r.x1 <= l.rect.x1);
            assert!(r.y0 >= l.rect.y0 && r.y1 <= l.rect.y1);
        }
        assert_eq!(LanguageSection::height(), end);
    }
}
