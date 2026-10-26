//! Help tooltips: one line of text under a button after the pointer has rested on it.
//!
//! Every view that has buttons answers `help_at(pos)` with a [`Help`] (the button's rectangle,
//! a short text and, when the button runs a command that has a shortcut, that command's id).
//! The app asks the view under the pointer on each pointer move and feeds the answer to a
//! [`TipTracker`], which is the whole behaviour: a tooltip shows once the pointer has rested on
//! the same button for [`HELP_DELAY`] seconds of the app's frame clock (nothing here reads a
//! clock), and hides on a press, a wheel turn, a key, or when the pointer leaves the button.
//! The tracker reports whether what is on screen changed, so a window repaints only then.
//!
//! The shortcut is looked up in the keymap when the tooltip shows ([`Help::text_with`]), so it
//! stays right after the user rebinds the command.

use crate::canvas::{Canvas, CanvasExt, TextRun};
use crate::keymap::Keymap;
use crate::kurbo::{Point, Rect, RoundedRect};
use crate::paint::estimate_width;
use crate::theme::Theme;
use std::borrow::Cow;

/// Seconds the pointer rests on a button before its tooltip shows.
pub const HELP_DELAY: f64 = 0.6;

/// Type size of a help tooltip.
const SIZE: f32 = 11.0;
/// Height of the box.
const HEIGHT: f64 = 22.0;
/// Gap between the button and the box.
const GAP: f64 = 6.0;
/// Margin kept between the box and the window edge.
const MARGIN: f64 = 4.0;

/// What a button says about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Help {
    /// The button, in the coordinates the view is hit-tested in (window logical pixels).
    pub rect: Rect,
    /// What the button does, one short line.
    pub text: Cow<'static, str>,
    /// The id of the keymap command the button runs, when it has one (the shortcut is added).
    pub command: Option<&'static str>,
    /// The argument of that command (`view.cables` + `show`).
    pub arg: Option<&'static str>,
}

impl Help {
    /// A button with a fixed `text` and no shortcut.
    pub fn new(rect: Rect, text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            rect,
            text: text.into(),
            command: None,
            arg: None,
        }
    }

    /// The same, saying that the button runs keymap command `command`.
    #[must_use]
    pub fn with_command(mut self, command: &'static str) -> Self {
        self.command = Some(command);
        self
    }

    /// The same, with the command's argument.
    #[must_use]
    pub fn with_arg(mut self, command: &'static str, arg: &'static str) -> Self {
        self.command = Some(command);
        self.arg = Some(arg);
        self
    }

    /// The text to show: the help line and, in brackets, the first chord `keymap` has for the
    /// button's command (nothing added when it is unbound).
    pub fn text_with(&self, keymap: &Keymap) -> String {
        let shortcut = self.command.and_then(|c| keymap.shortcut_text(c, self.arg));
        match shortcut {
            Some(s) => format!("{} ({s})", self.text),
            None => self.text.to_string(),
        }
    }
}

/// The hover-delay state machine behind the tooltips. It is told what is under the pointer
/// ([`Self::pointer`]), when the pointer does something that dismisses a tooltip
/// ([`Self::dismiss`]) and what time it is ([`Self::tick`]); it answers with what to show.
#[derive(Debug, Clone, Default)]
pub struct TipTracker {
    /// The button the pointer is on.
    target: Option<Help>,
    /// When the pointer arrived on it (the tick clock); set by the first tick after it did.
    since: Option<f64>,
    /// The tooltip is on screen.
    shown: bool,
    /// A press, a wheel turn or a key hid it: it stays hidden until the pointer is on another
    /// button.
    dismissed: bool,
}

impl TipTracker {
    /// An idle tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// The pointer is over `help` (`None`: over no button). Resting on the same button keeps
    /// the timer and the tooltip; another button, or none, starts over. Returns whether the
    /// tooltip on screen changed (a tooltip that was shown went away).
    pub fn pointer(&mut self, help: Option<Help>) -> bool {
        if self.target == help {
            return false;
        }
        let was_shown = self.shown;
        self.target = help;
        self.since = None;
        self.shown = false;
        self.dismissed = false;
        was_shown
    }

    /// A press, a wheel turn or a key: the tooltip hides and stays hidden while the pointer
    /// stays on this button. Returns whether one was on screen.
    pub fn dismiss(&mut self) -> bool {
        self.dismissed = true;
        std::mem::take(&mut self.shown)
    }

    /// Forgets everything (the pointer left the window, a menu or dialog opened, tooltips were
    /// switched off). Returns whether one was on screen.
    pub fn clear(&mut self) -> bool {
        let was = self.shown;
        *self = Self::default();
        was
    }

    /// Advances to `now` (seconds, any monotonic clock). Returns whether a tooltip just
    /// appeared.
    pub fn tick(&mut self, now: f64) -> bool {
        if self.target.is_none() || self.shown || self.dismissed {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now - since >= HELP_DELAY {
            self.shown = true;
            return true;
        }
        false
    }

    /// Whether the tracker waits for time to pass (the app keeps its frame loop ticking).
    pub fn pending(&self) -> bool {
        self.target.is_some() && !self.shown && !self.dismissed
    }

    /// The tooltip on screen, if any.
    pub fn shown(&self) -> Option<&Help> {
        self.target.as_ref().filter(|_| self.shown)
    }

    /// The button the pointer is on, whether or not its tooltip shows yet.
    pub fn target(&self) -> Option<&Help> {
        self.target.as_ref()
    }
}

/// The help of the stepper row of a panel at `pos`: the `-` and `+` buttons and the value window
/// between them. `numeric` rows take a drag and the wheel; the others (a switch, a choice) flip
/// on a click.
pub fn stepper_help(
    pos: Point,
    title: &str,
    (minus, value, plus): (Rect, Rect, Rect),
    numeric: bool,
) -> Option<Help> {
    if minus.contains(pos) {
        Some(Help::new(
            minus,
            format!("Decrease {}", title.to_lowercase()),
        ))
    } else if plus.contains(pos) {
        Some(Help::new(
            plus,
            format!("Increase {}", title.to_lowercase()),
        ))
    } else if value.contains(pos) {
        let how = if numeric {
            "drag or scroll to change"
        } else {
            "click to switch"
        };
        Some(Help::new(value, format!("{title}: {how}")))
    } else {
        None
    }
}
/// Where the box for a tooltip of `width` goes: under `rect`, its left edge on the button's,
/// moved to stay inside `view`; above the button when there is no room below.
pub fn place(rect: Rect, width: f64, view: Rect) -> Rect {
    let max_x = (view.x1 - MARGIN - width).max(view.x0 + MARGIN);
    let x = rect.x0.clamp(view.x0 + MARGIN, max_x);
    let below = rect.y1 + GAP;
    let y = if below + HEIGHT + MARGIN <= view.y1 {
        below
    } else {
        (rect.y0 - GAP - HEIGHT).max(view.y0 + MARGIN)
    };
    Rect::new(x, y, x + width, y + HEIGHT)
}

/// The box a tooltip of `text` for the button `rect` takes in `view`.
pub fn box_for(rect: Rect, text: &str, view: Rect) -> Rect {
    place(rect, estimate_width(text, SIZE) + 16.0, view)
}

/// Paints the tooltip `text` of the button `rect` inside `view` (logical pixels).
pub fn paint<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, rect: Rect, text: &str, view: Rect) {
    let r = box_for(rect, text, view);
    let shape = RoundedRect::from_rect(r, 4.0);
    c.fill_with(shape, theme.tooltip_bg);
    c.stroke_with(shape, 1.0, theme.section_line);
    c.text(&TextRun::new(text, Point::new(r.x0 + 8.0, r.y0 + 15.0), SIZE, theme.text).weight(600));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(x: f64) -> Help {
        Help::new(Rect::new(x, 10.0, x + 30.0, 30.0), "Play")
    }

    #[test]
    fn a_tooltip_shows_only_after_the_delay() {
        let mut t = TipTracker::new();
        assert!(!t.pointer(Some(help(0.0))));
        assert!(t.pending());
        assert!(!t.tick(1.0), "the first tick starts the clock");
        assert!(!t.tick(1.0 + HELP_DELAY - 0.01));
        assert!(t.shown().is_none());
        assert!(t.tick(1.0 + HELP_DELAY + 0.01));
        assert!(t.shown().is_some());
        assert!(!t.pending());
        assert!(!t.tick(9.0), "it shows once");
    }

    #[test]
    fn moving_within_a_button_keeps_the_timer_and_to_another_restarts_it() {
        let mut t = TipTracker::new();
        t.pointer(Some(help(0.0)));
        t.tick(1.0);
        assert!(!t.pointer(Some(help(0.0))));
        assert!(
            t.tick(1.0 + HELP_DELAY + 0.01),
            "same button: the timer kept running"
        );
        assert!(t.pointer(Some(help(100.0))), "a shown tooltip goes away");
        assert!(t.shown().is_none());
        assert!(!t.tick(5.0));
        assert!(!t.tick(5.0 + HELP_DELAY - 0.01));
        assert!(t.tick(5.0 + HELP_DELAY + 0.01));
    }

    #[test]
    fn leaving_the_button_hides_without_a_repaint_before_it_shows() {
        let mut t = TipTracker::new();
        t.pointer(Some(help(0.0)));
        t.tick(0.0);
        assert!(!t.pointer(None), "nothing was on screen");
        assert!(!t.pending());
        assert!(!t.tick(10.0));
    }

    #[test]
    fn a_press_hides_it_until_the_pointer_is_on_another_button() {
        let mut t = TipTracker::new();
        t.pointer(Some(help(0.0)));
        t.tick(0.0);
        t.tick(HELP_DELAY);
        assert!(t.dismiss(), "one was on screen");
        assert!(t.shown().is_none());
        assert!(!t.pending());
        assert!(!t.pointer(Some(help(0.0))), "same button: still hidden");
        assert!(!t.tick(50.0));
        t.pointer(Some(help(100.0)));
        t.tick(60.0);
        assert!(t.tick(60.0 + HELP_DELAY + 0.01));
    }

    #[test]
    fn clear_forgets_the_target() {
        let mut t = TipTracker::new();
        t.pointer(Some(help(0.0)));
        t.tick(0.0);
        t.tick(HELP_DELAY);
        assert!(t.clear());
        assert!(t.target().is_none());
        assert!(!t.clear());
    }

    #[test]
    fn the_box_stays_inside_the_window() {
        let view = Rect::new(0.0, 0.0, 200.0, 100.0);
        let b = place(Rect::new(180.0, 10.0, 200.0, 30.0), 120.0, view);
        assert!(b.x1 <= view.x1 - MARGIN && b.x0 >= view.x0);
        assert!(b.y0 >= 30.0, "under the button");
        let b = place(Rect::new(10.0, 80.0, 40.0, 98.0), 60.0, view);
        assert!(b.y1 <= 80.0, "above the button when the bottom is full");
        let b = place(Rect::new(0.0, 0.0, 10.0, 10.0), 60.0, view);
        assert!(b.x0 >= MARGIN);
    }
}
