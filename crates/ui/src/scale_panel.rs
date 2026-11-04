//! The Scale Tempo panel: one free percentage for the selected clips.
//!
//! ```text
//! SCALE TEMPO                                                            [ CLOSE ]
//! Applies to: 3 clips
//! Tempo    [ 150            ] %      10 to 1000: 200 plays twice as fast
//! [ APPLY ]    Enter applies, Esc cancels; the clips show the result while you type
//! ```
//!
//! A modal view the app places over the window like the song information panel. It has no
//! document: the app tells it what it applies to ([`ScaleView::set_target`]) and reads the
//! percentage back ([`ScaleView::percent`]) when [`UiAction::ScaleTempo`] arrives:
//!
//! * every edit of the text that leaves a usable percentage (or takes it away) emits
//!   [`ScaleAction::Preview`], so the app can show the result as a trial that adds no undo
//!   step;
//! * `APPLY` (and Enter) emits [`ScaleAction::Apply`], not while the text is no usable
//!   percentage or nothing is selected;
//! * `CLOSE` emits [`ScaleAction::Close`]; the app also closes the panel on Esc, which drops
//!   the preview.
//!
//! The text is a [`TextField`] (caret, selection, clipboard, input-method composition); it takes
//! digits, a decimal point or comma and an optional percent sign.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_model::{SCALE_PERCENT_MAX, SCALE_PERCENT_MIN};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TARGET_H: f64 = 24.0;
const ROW_H: f64 = 32.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 480.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 70.0;
const FIELD_W: f64 = 120.0;
const FONT: f32 = 10.5;
const LINE_H: f64 = 14.0;
const MAX_CHARS: usize = 12;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ScaleAction {
    /// Show the clips scaled by this percentage (`None`: the text is no usable percentage,
    /// show them as they were).
    Preview(Option<f64>),
    /// Make the percentage of the panel the new tempo of the clips (one undo step).
    Apply,
    /// Close the panel.
    Close,
}

/// The percentage a text stands for, when it is one the command accepts: a number (a comma
/// counts as the decimal point, a trailing percent sign is ignored) from
/// [`SCALE_PERCENT_MIN`] to [`SCALE_PERCENT_MAX`] that is not 100 (no change).
pub fn parse_percent(text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches('%').trim().replace(',', ".");
    let p: f64 = t.parse().ok()?;
    (p.is_finite()
        && (SCALE_PERCENT_MIN..=SCALE_PERCENT_MAX).contains(&p)
        && (p - 100.0).abs() > 1e-9)
        .then_some(p)
}

/// The Scale Tempo panel. See the module documentation.
#[derive(Debug)]
pub struct ScaleView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    field: TextField,
    dragging: bool,
    target: String,
    can_apply: bool,
    /// The percentage last announced with [`ScaleAction::Preview`].
    announced: Option<f64>,
    hover: Option<Point>,
    repaint: bool,
}

impl ScaleView {
    /// A panel showing `percent` (as the text of the field), with the keys on it; call
    /// [`Self::set_bounds`]. The text starts selected, so typing replaces it.
    pub fn new(theme: Theme, percent: f64) -> Self {
        let mut field = TextField::new(&format_percent(percent), MAX_CHARS, false);
        field.select_all();
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            field,
            dragging: false,
            target: String::new(),
            can_apply: false,
            announced: None,
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

    /// Tells the panel what it applies to (shown under the title) and whether APPLY can work.
    pub fn set_target(&mut self, text: String, can_apply: bool) {
        if text != self.target || can_apply != self.can_apply {
            self.target = text;
            self.can_apply = can_apply;
            self.repaint = true;
        }
    }

    /// The usable percentage in the text, if there is one.
    pub fn percent(&self) -> Option<f64> {
        parse_percent(self.field.text())
    }

    /// The text of the field.
    pub fn text(&self) -> &str {
        self.field.text()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal and every key goes to the field).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// The preview the text calls for when it is not the one last announced.
    fn announce(&mut self, out: &mut Vec<UiAction>) {
        let now = self.percent().filter(|_| self.can_apply);
        if now != self.announced {
            self.announced = now;
            out.push(UiAction::ScaleTempo(ScaleAction::Preview(now)));
        }
    }

    /// The first announcement after the panel opened (or after the target changed): the app
    /// calls this once it has told the panel what it applies to.
    pub fn open_preview(&mut self) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.announce(&mut out);
        out
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        let height = HEADER_H + TARGET_H + ROW_H + FOOT_H + PAD;
        Rect::new(x0, y0, x0 + width, y0 + height.min(room))
    }

    /// The box of the percentage text (the area that takes a click).
    pub fn field_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + TARGET_H + 2.0;
        Rect::new(
            p.x0 + PAD + LABEL_W,
            y,
            p.x0 + PAD + LABEL_W + FIELD_W,
            y + ROW_H - 8.0,
        )
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

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + TARGET_H + ROW_H + (FOOT_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the panel and put the clips back as they were",
            );
        }
        if self.apply_rect().contains(pos) {
            return help(
                self.apply_rect(),
                "Scale the selected clips to this tempo (one undo step)",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if self.dragging {
                    self.field.drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging = false,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::ScaleTempo(ScaleAction::Close));
                } else if self.apply_rect().contains(*pos) {
                    self.apply(&mut out);
                } else if self.field_rect().contains(*pos) {
                    self.dragging = true;
                    self.field.press(*pos, *clicks, modifiers.shift);
                }
            }
            UiEvent::Ime(ime) => {
                if self.field.handle_ime(ime) {
                    self.repaint = true;
                    self.announce(&mut out);
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let used = self.field.handle_key(key, *modifiers);
                if used.handled() {
                    self.repaint = true;
                    self.announce(&mut out);
                } else if *key == Key::Enter {
                    self.apply(&mut out);
                }
            }
            _ => {}
        }
        out
    }

    fn apply(&mut self, out: &mut Vec<UiAction>) {
        if self.can_apply && self.percent().is_some() {
            out.push(UiAction::ScaleTempo(ScaleAction::Apply));
        }
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
            "SCALE TEMPO",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let target = if self.target.is_empty() {
            "Select the clips to scale".to_owned()
        } else {
            format!("Applies to: {}", self.target)
        };
        sp::label(
            c,
            &fit_label(&target, p.width() - 2.0 * PAD, FONT),
            p.x0 + PAD,
            p.y0 + HEADER_H + 16.0,
            FONT,
            t.text_muted,
            Align::Start,
        );
        let r = self.field_rect();
        sp::label(
            c,
            "Tempo",
            p.x0 + PAD,
            r.y0 + 14.0,
            FONT,
            t.text,
            Align::Start,
        );
        sp::lcd(c, st, r, Some(t.focus));
        let colors = FieldColors {
            text: t.text,
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        };
        self.field.paint(c, r, FONT, LINE_H, colors, true);
        sp::label(c, "%", r.x1 + 6.0, r.y0 + 14.0, FONT, t.text, Align::Start);
        let hint = if self.percent().is_some() {
            "200 plays twice as fast, 50 half as fast".to_owned()
        } else {
            format!(
                "A number from {} to {}, not 100",
                SCALE_PERCENT_MIN as u32, SCALE_PERCENT_MAX as u32
            )
        };
        sp::label(
            c,
            &fit_label(&hint, p.x1 - PAD - (r.x1 + 24.0), 9.5),
            r.x1 + 24.0,
            r.y0 + 14.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        let apply = self.apply_rect();
        let ready = self.can_apply && self.percent().is_some();
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            ready.then_some(st.loop_on),
            ready && hovered(apply),
        );
        sp::label(
            c,
            &fit_label(
                "Enter applies, Esc cancels; the clips show the result as you type",
                p.x1 - PAD - apply.x1 - 12.0,
                9.5,
            ),
            apply.x1 + 12.0,
            apply.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}

/// A percentage as the text of the field: no trailing zeros.
fn format_percent(percent: f64) -> String {
    let text = format!("{percent:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_parse_with_a_comma_or_a_sign_and_stay_in_range() {
        assert_eq!(parse_percent("150"), Some(150.0));
        assert_eq!(parse_percent(" 87,5 % "), Some(87.5));
        assert_eq!(parse_percent("10"), Some(10.0));
        assert_eq!(parse_percent("1000"), Some(1000.0));
        for bad in [
            "", "abc", "9.9", "1000.1", "100", "100.0", "NaN", "inf", "-50", "1e2e",
        ] {
            assert_eq!(parse_percent(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn percentages_show_without_trailing_zeros() {
        assert_eq!(format_percent(150.0), "150");
        assert_eq!(format_percent(87.5), "87.5");
        assert_eq!(format_percent(33.333), "33.33");
    }
}
