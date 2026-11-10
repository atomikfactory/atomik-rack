//! The card that suggests freezing the heaviest track, in the lower left corner of the window.
//!
//! It shows when the engine load stayed above 70 % for five seconds while playing (the decision
//! is [`crate::cpu_gauge::HintWatch`]): "Track X uses the most CPU (Y%). Freeze it?" with a
//! Freeze button that runs Freeze Track on that track, and a Not now button. It is not a modal:
//! it sits above the status line, takes only the clicks that land on it and leaves every other
//! event to the program.

use atomik_model::DeviceKey;
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::{Align, Canvas, CanvasExt, Help, PointerButton, TextRun, Theme, UiEvent};

/// The width of the card.
const WIDTH: f64 = 430.0;
/// The height of the card.
const HEIGHT: f64 = 72.0;
/// The gap to the window edge and above the status line.
const MARGIN: f64 = 14.0;
const BUTTON_H: f64 = 24.0;

/// A button of the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Freeze the track.
    Freeze,
    /// Close the card.
    NotNow,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Self::Freeze => "Freeze",
            Self::NotNow => "Not now",
        }
    }

    fn width(self) -> f64 {
        match self {
            Self::Freeze => 80.0,
            Self::NotNow => 84.0,
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Freeze => "Render the track to audio so it stops using CPU; Unfreeze undoes it",
            Self::NotNow => "Close this hint; it shows once per track in a song",
        }
    }
}

/// What an event asks of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The event is not the card's: the program handles it.
    Pass,
    /// The card took the event and stays.
    Taken,
    /// A button was pressed.
    Chose(Choice),
}

/// The card and the pointer over it.
#[derive(Debug, Clone)]
pub struct CpuHintCard {
    track: DeviceKey,
    text: String,
    hover: Option<Point>,
}

impl CpuHintCard {
    /// A card suggesting to freeze `track` (called `name`), which uses `percent` of the audio
    /// time.
    pub fn new(track: DeviceKey, name: &str, percent: f32) -> Self {
        Self {
            track,
            text: format!(
                "Track {name} uses the most CPU ({}). Freeze it?",
                atomik_ui::paint::cpu_text(percent)
            ),
            hover: None,
        }
    }

    /// The track it suggests freezing.
    pub fn track(&self) -> &DeviceKey {
        &self.track
    }

    /// What it says.
    pub fn text(&self) -> &str {
        &self.text
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
        let y0 = card.y1 - 10.0 - BUTTON_H;
        let mut x = card.x0 + 14.0;
        [Choice::Freeze, Choice::NotNow]
            .into_iter()
            .map(|choice| {
                let r = Rect::new(x, y0, x + choice.width(), y0 + BUTTON_H);
                x += choice.width() + 8.0;
                (r, choice)
            })
            .collect()
    }

    /// Handles an event of the window of size `view`. A click on a button chooses it; a click
    /// elsewhere on the card and a move over it are taken; every other event is not the card's.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> Outcome {
        let card = self.rect(view);
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let over = card.contains(*pos);
                self.hover = over.then_some(*pos);
                if over { Outcome::Taken } else { Outcome::Pass }
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                Outcome::Pass
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
                    return Outcome::Chose(choice);
                }
                Outcome::Taken
            }
            _ => Outcome::Pass,
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
        canvas.stroke_with(shape, 1.0, theme.warning);
        canvas.text(
            &TextRun::new(
                atomik_ui::paint::fit_label(&self.text, card.width() - 28.0, 12.0),
                Point::new(card.x0 + 14.0, card.y0 + 22.0),
                12.0,
                theme.text,
            )
            .weight(600),
        );
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
                    Point::new(r.center().x, r.y0 + 16.0),
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

    fn card() -> CpuHintCard {
        CpuHintCard::new(DeviceKey("bass".into()), "Bass", 41.3)
    }

    fn press(pos: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
            clicks: 1,
        }
    }

    fn button(c: &CpuHintCard, choice: Choice) -> Rect {
        c.buttons(VIEW)
            .into_iter()
            .find(|(_, x)| *x == choice)
            .map(|(r, _)| r)
            .unwrap()
    }

    #[test]
    fn it_names_the_track_and_its_share() {
        assert_eq!(
            card().text(),
            "Track Bass uses the most CPU (41%). Freeze it?"
        );
        let small = CpuHintCard::new(DeviceKey("x".into()), "Pad", 3.24);
        assert_eq!(
            small.text(),
            "Track Pad uses the most CPU (3.2%). Freeze it?"
        );
    }

    #[test]
    fn the_buttons_sit_inside_the_card_above_the_status_line() {
        let c = card();
        let rect = c.rect(VIEW);
        assert!(rect.y1 <= VIEW.height - crate::shell::STATUS_BAR_HEIGHT);
        let buttons = c.buttons(VIEW);
        assert_eq!(buttons.len(), 2);
        for (r, _) in &buttons {
            assert!(r.x0 >= rect.x0 && r.x1 <= rect.x1 && r.y0 >= rect.y0 && r.y1 <= rect.y1);
        }
        assert!(buttons[0].0.x1 < buttons[1].0.x0);
    }

    #[test]
    fn a_click_chooses_a_button_and_the_rest_of_the_card_is_taken() {
        let mut c = card();
        for choice in [Choice::Freeze, Choice::NotNow] {
            let at = button(&c, choice).center();
            assert_eq!(c.handle(&press(at), VIEW), Outcome::Chose(choice));
        }
        let rect = c.rect(VIEW);
        assert_eq!(
            c.handle(&press(Point::new(rect.x1 - 2.0, rect.y0 + 2.0)), VIEW),
            Outcome::Taken
        );
        assert_eq!(
            c.handle(&press(Point::new(500.0, 100.0)), VIEW),
            Outcome::Pass
        );
    }

    #[test]
    fn every_button_has_a_short_help_text() {
        let c = card();
        for (r, _) in c.buttons(VIEW) {
            let help = c.help_at(VIEW, r.center()).unwrap();
            assert!(
                !help.text.is_empty() && help.text.len() <= 90,
                "{}",
                help.text
            );
        }
        assert!(c.help_at(VIEW, Point::new(500.0, 100.0)).is_none());
    }
}
