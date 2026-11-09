//! The small notice of the update check, in the lower left corner of the window.
//!
//! It is not a modal: it sits above the status line, takes only the clicks that land on it and
//! leaves every other event to the program. It has two forms:
//!
//! - the **question** the first time ("Check for new versions once a day?" with Yes and No), which
//!   is also what Help > Check for Updates shows while the question is still open, so nothing is
//!   asked of the network before the answer;
//! - the **news** when a newer version exists: "What's new" opens its release page in the
//!   browser, "Skip this version" keeps the daily check quiet about exactly this version, "Later"
//!   closes the notice. Nothing is downloaded or installed.

use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::{Align, Canvas, CanvasExt, Help, PointerButton, TextRun, Theme, UiEvent};

/// The width of the card.
const WIDTH: f64 = 470.0;
/// The height of the card.
const HEIGHT: f64 = 82.0;
/// The gap to the window edge and above the status line.
const MARGIN: f64 = 14.0;
const BUTTON_H: f64 = 26.0;

/// What the notice says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The question. `then_check` is set when it was opened by Help > Check for Updates: a Yes
    /// also makes the request at once.
    Ask {
        /// Make the request right after a Yes.
        then_check: bool,
    },
    /// A newer version exists.
    News {
        /// The version found, as text (`0.5.0-rc.1`).
        version: String,
        /// The version running.
        running: String,
    },
}

/// A button of the notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Yes: ask once a day.
    Yes,
    /// No: never ask by itself.
    No,
    /// Open the release page.
    WhatsNew,
    /// Skip this version.
    Skip,
    /// Close the notice for now.
    Later,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Self::Yes => "Yes",
            Self::No => "No",
            Self::WhatsNew => "What's new",
            Self::Skip => "Skip this version",
            Self::Later => "Later",
        }
    }

    fn width(self) -> f64 {
        match self {
            Self::Yes | Self::No => 72.0,
            Self::Later => 76.0,
            Self::WhatsNew => 100.0,
            Self::Skip => 138.0,
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Yes => "Ask the project page once a day whether a newer version exists",
            Self::No => {
                "Never ask by itself; Help > Check for Updates still works when you choose it"
            }
            Self::WhatsNew => "Open the page of the new version in the browser",
            Self::Skip => "Do not show this version again; a later version still shows",
            Self::Later => "Close this notice; the daily check may show it again",
        }
    }
}

/// What a click on the notice asks of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeOutcome {
    /// The event is not the notice's: the program handles it.
    Pass,
    /// The notice took the event and stays.
    Taken,
    /// A button was pressed.
    Chose(Choice),
}

/// The notice and the pointer over it.
#[derive(Debug, Clone)]
pub struct UpdateNotice {
    notice: Notice,
    hover: Option<Point>,
}

impl UpdateNotice {
    /// A notice saying `notice`.
    pub fn new(notice: Notice) -> Self {
        Self {
            notice,
            hover: None,
        }
    }

    /// What it says.
    pub fn notice(&self) -> &Notice {
        &self.notice
    }

    /// The two text lines of the card.
    pub fn lines(&self) -> [String; 2] {
        match &self.notice {
            Notice::Ask { .. } => [
                "Check for new versions once a day?".to_string(),
                "One request to the project page; nothing about you or this machine.".to_string(),
            ],
            Notice::News { version, running } => [
                format!("Atomik Rack {version} is available"),
                format!("You have {running}. Nothing is downloaded or installed."),
            ],
        }
    }

    fn choices(&self) -> &'static [Choice] {
        match self.notice {
            Notice::Ask { .. } => &[Choice::Yes, Choice::No],
            Notice::News { .. } => &[Choice::WhatsNew, Choice::Skip, Choice::Later],
        }
    }

    /// The card in a window of `view` (logical units).
    pub fn rect(&self, view: Size) -> Rect {
        let w = (view.width - 2.0 * MARGIN).clamp(120.0, WIDTH);
        let y1 = view.height - crate::shell::STATUS_BAR_HEIGHT - MARGIN;
        Rect::new(MARGIN, y1 - HEIGHT, MARGIN + w, y1)
    }

    /// The buttons, left to right.
    pub fn buttons(&self, view: Size) -> Vec<(Rect, Choice)> {
        let card = self.rect(view);
        let y0 = card.y1 - 12.0 - BUTTON_H;
        let mut x = card.x0 + 14.0;
        self.choices()
            .iter()
            .map(|&choice| {
                let r = Rect::new(x, y0, x + choice.width(), y0 + BUTTON_H);
                x += choice.width() + 8.0;
                (r, choice)
            })
            .collect()
    }

    /// Handles an event of the window of size `view`. A click on a button chooses it; a click
    /// elsewhere on the card and a move over it are taken (nothing happens behind the card); every
    /// other event is not the notice's.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> NoticeOutcome {
        let card = self.rect(view);
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let over = card.contains(*pos);
                self.hover = over.then_some(*pos);
                if over {
                    NoticeOutcome::Taken
                } else {
                    NoticeOutcome::Pass
                }
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                NoticeOutcome::Pass
            }
            UiEvent::PointerDown { pos, button, .. } | UiEvent::PointerUp { pos, button, .. }
                if card.contains(*pos) =>
            {
                let pressed = matches!(event, UiEvent::PointerDown { .. });
                if pressed
                    && *button == PointerButton::Primary
                    && let Some((_, choice)) = self
                        .buttons(view)
                        .into_iter()
                        .find(|(r, _)| r.contains(*pos))
                {
                    return NoticeOutcome::Chose(choice);
                }
                NoticeOutcome::Taken
            }
            _ => NoticeOutcome::Pass,
        }
    }

    /// What the button under `pos` does.
    pub fn help_at(&self, view: Size, pos: Point) -> Option<Help> {
        self.buttons(view)
            .into_iter()
            .find(|(r, _)| r.contains(pos))
            .map(|(r, choice)| Help::new(r, choice.help()))
    }

    /// Paints the card (logical units).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        let card = self.rect(view);
        let shape = RoundedRect::from_rect(card, 8.0);
        canvas.fill_with(shape, theme.browser_bg);
        canvas.stroke_with(shape, 1.0, theme.focus);
        let [title, detail] = self.lines();
        canvas.text(
            &TextRun::new(
                title,
                Point::new(card.x0 + 14.0, card.y0 + 22.0),
                13.0,
                theme.text,
            )
            .weight(600),
        );
        canvas.text(&TextRun::new(
            detail,
            Point::new(card.x0 + 14.0, card.y0 + 40.0),
            11.5,
            theme.text_muted,
        ));
        for (r, choice) in self.buttons(view) {
            let lit = self.hover.is_some_and(|p| r.contains(p));
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if lit {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    choice.label(),
                    Point::new(r.center().x, r.y0 + 17.0),
                    12.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::Modifiers;

    const VIEW: Size = Size::new(1000.0, 700.0);

    fn ask() -> UpdateNotice {
        UpdateNotice::new(Notice::Ask { then_check: false })
    }

    fn news() -> UpdateNotice {
        UpdateNotice::new(Notice::News {
            version: "0.5.0".into(),
            running: "0.4.0".into(),
        })
    }

    fn press(pos: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
            clicks: 1,
        }
    }

    fn button(n: &UpdateNotice, choice: Choice) -> Rect {
        n.buttons(VIEW)
            .into_iter()
            .find(|(_, c)| *c == choice)
            .map(|(r, _)| r)
            .unwrap()
    }

    #[test]
    fn the_question_has_yes_and_no_and_the_news_has_three_buttons() {
        let choices = |n: &UpdateNotice| -> Vec<Choice> {
            n.buttons(VIEW).into_iter().map(|(_, c)| c).collect()
        };
        assert_eq!(choices(&ask()), [Choice::Yes, Choice::No]);
        assert_eq!(
            choices(&news()),
            [Choice::WhatsNew, Choice::Skip, Choice::Later]
        );
    }

    #[test]
    fn the_buttons_sit_inside_the_card_above_the_status_line() {
        for n in [ask(), news()] {
            let card = n.rect(VIEW);
            assert!(card.y1 <= VIEW.height - crate::shell::STATUS_BAR_HEIGHT);
            assert!(card.x0 >= 0.0 && card.x1 <= VIEW.width);
            let buttons = n.buttons(VIEW);
            for (r, _) in &buttons {
                assert!(r.x0 >= card.x0 && r.x1 <= card.x1 && r.y0 >= card.y0 && r.y1 <= card.y1);
            }
            for pair in buttons.windows(2) {
                assert!(pair[0].0.x1 < pair[1].0.x0, "buttons do not overlap");
            }
        }
    }

    #[test]
    fn a_click_on_a_button_chooses_it() {
        let mut n = news();
        for choice in [Choice::WhatsNew, Choice::Skip, Choice::Later] {
            let at = button(&n, choice).center();
            assert_eq!(n.handle(&press(at), VIEW), NoticeOutcome::Chose(choice));
        }
        let mut q = ask();
        let yes = button(&q, Choice::Yes).center();
        assert_eq!(
            q.handle(&press(yes), VIEW),
            NoticeOutcome::Chose(Choice::Yes)
        );
    }

    #[test]
    fn the_card_takes_its_own_clicks_and_lets_the_rest_through() {
        let mut n = news();
        let card = n.rect(VIEW);
        let on_card = Point::new(card.x0 + 4.0, card.y0 + 4.0);
        assert_eq!(n.handle(&press(on_card), VIEW), NoticeOutcome::Taken);
        let away = Point::new(card.x1 + 50.0, card.y0 - 50.0);
        assert_eq!(n.handle(&press(away), VIEW), NoticeOutcome::Pass);
        assert_eq!(
            n.handle(
                &UiEvent::PointerMove {
                    pos: away,
                    modifiers: Modifiers::default()
                },
                VIEW
            ),
            NoticeOutcome::Pass
        );
        assert_eq!(n.handle(&UiEvent::PointerLeave, VIEW), NoticeOutcome::Pass);
    }

    #[test]
    fn a_secondary_click_on_a_button_chooses_nothing() {
        let mut n = news();
        let at = button(&n, Choice::Skip).center();
        let event = UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Secondary,
            modifiers: Modifiers::default(),
            clicks: 1,
        };
        assert_eq!(n.handle(&event, VIEW), NoticeOutcome::Taken);
    }

    #[test]
    fn keys_are_never_the_notices() {
        let mut n = ask();
        let key = UiEvent::Key {
            key: atomik_ui::Key::Escape,
            modifiers: Modifiers::default(),
            pressed: true,
        };
        assert_eq!(n.handle(&key, VIEW), NoticeOutcome::Pass);
    }

    #[test]
    fn the_words_say_what_is_sent_and_that_nothing_is_installed() {
        let [title, detail] = ask().lines();
        assert_eq!(title, "Check for new versions once a day?");
        assert!(detail.contains("nothing about you"));
        let [title, detail] = news().lines();
        assert!(title.contains("0.5.0"));
        assert!(detail.contains("0.4.0") && detail.contains("Nothing is downloaded"));
    }

    #[test]
    fn the_tooltips_are_short_and_each_button_has_one() {
        for n in [ask(), news()] {
            for (r, choice) in n.buttons(VIEW) {
                let help = n.help_at(VIEW, r.center()).expect("a tooltip");
                assert!(help.text.chars().count() <= 90, "{}", help.text);
                assert_eq!(help.rect, r);
                assert!(!choice.label().is_empty());
            }
            assert!(n.help_at(VIEW, Point::new(1.0, 1.0)).is_none());
        }
    }

    #[test]
    fn a_narrow_window_keeps_the_card_inside() {
        let n = news();
        let narrow = Size::new(300.0, 400.0);
        let card = n.rect(narrow);
        assert!(card.x1 <= narrow.width);
        assert!(card.width() > 100.0);
    }
}
