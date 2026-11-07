//! The blind A/B panel: compare an effect with and without it without knowing which is which.
//!
//! ```text
//! BLIND A/B  Reverb                                                      [ CLOSE ]
//! Trial 3 of 10
//! Switch between X and Y as often as you like, then pick the one you prefer.
//! [ LISTEN X ]  [ LISTEN Y ]
//! [ PREFER X ]  [ PREFER Y ]
//! [ MATCH LEVEL ]
//! ```
//!
//! After the last trial the same area shows the result lines and an `AGAIN` button.
//!
//! A modal view the app places over the window like the strip silence panel. It holds no
//! test state: the app tells it what to show ([`BlindView::set_info`]) and hears what was
//! clicked ([`UiAction::Blind`]). Which of X and Y is the processed sound is decided by the app
//! and never shown here.
//!
//! * `LISTEN X` / `LISTEN Y` emit [`BlindAction::Listen`] with the slot (0 or 1).
//! * `PREFER X` / `PREFER Y` emit [`BlindAction::Pick`]; they are dead until both were heard.
//! * `MATCH LEVEL` emits [`BlindAction::ToggleMatch`] (the loudness match for the test only).
//! * `AGAIN` emits [`BlindAction::Again`] and `CLOSE` emits [`BlindAction::Close`].

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TEXT_H: f64 = 52.0;
const ROW_H: f64 = 40.0;
const MATCH_H: f64 = 34.0;
const LINE_H: f64 = 20.0;
const BTN_H: f64 = 26.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 460.0;

/// What the app shows in the panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlindInfo {
    /// The name of the device under test.
    pub device: String,
    /// The trial being judged, counting from 1.
    pub trial: usize,
    /// How many trials the test has.
    pub trials: usize,
    /// The slot that plays now (0 = X, 1 = Y).
    pub listening: Option<u8>,
    /// Both sounds were heard in this trial, so a pick is allowed.
    pub can_pick: bool,
    /// The loudness match is on for the test.
    pub match_on: bool,
    /// The result lines once the test is over (`None` while it runs).
    pub result: Option<Vec<String>>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlindAction {
    /// Play slot 0 (X) or 1 (Y).
    Listen(u8),
    /// Prefer slot 0 (X) or 1 (Y) in this trial.
    Pick(u8),
    /// Switch the loudness match of the test.
    ToggleMatch,
    /// Start a new test.
    Again,
    /// Close the panel (the test ends and the device is as it was).
    Close,
}

/// The names of the two slots.
pub const SLOT_NAMES: [&str; 2] = ["X", "Y"];

/// The blind A/B panel. See the module documentation.
#[derive(Debug)]
pub struct BlindView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    info: BlindInfo,
    hover: Option<Point>,
    repaint: bool,
}

impl BlindView {
    /// A panel; call [`Self::set_bounds`] and [`Self::set_info`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            info: BlindInfo::default(),
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

    /// Shows `info`.
    pub fn set_info(&mut self, info: BlindInfo) {
        if info != self.info {
            self.info = info;
            self.repaint = true;
        }
    }

    /// What the panel shows.
    pub fn info(&self) -> &BlindInfo {
        &self.info
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    fn body_h(&self) -> f64 {
        match &self.info.result {
            None => TEXT_H + 2.0 * ROW_H + MATCH_H,
            Some(lines) => lines.len() as f64 * LINE_H + 16.0 + ROW_H,
        }
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        let height = HEADER_H + self.body_h() + PAD;
        Rect::new(x0, y0, x0 + width, y0 + height.min(room))
    }

    fn body_top(&self) -> f64 {
        self.panel().y0 + HEADER_H
    }

    fn half_buttons(&self, y: f64) -> [Rect; 2] {
        let p = self.panel();
        let w = (p.width() - 2.0 * PAD - 12.0) / 2.0;
        let y0 = y + (ROW_H - BTN_H) / 2.0;
        [
            Rect::new(p.x0 + PAD, y0, p.x0 + PAD + w, y0 + BTN_H),
            Rect::new(p.x1 - PAD - w, y0, p.x1 - PAD, y0 + BTN_H),
        ]
    }

    /// The `LISTEN` button of slot `slot`.
    pub fn listen_rect(&self, slot: u8) -> Rect {
        self.half_buttons(self.body_top() + TEXT_H)[usize::from(slot.min(1))]
    }

    /// The `PREFER` button of slot `slot`.
    pub fn pick_rect(&self, slot: u8) -> Rect {
        self.half_buttons(self.body_top() + TEXT_H + ROW_H)[usize::from(slot.min(1))]
    }

    /// The `MATCH LEVEL` button.
    pub fn match_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.body_top() + TEXT_H + 2.0 * ROW_H + (MATCH_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 150.0, y + BTN_H)
    }

    /// The `AGAIN` button of the result.
    pub fn again_rect(&self) -> Rect {
        let p = self.panel();
        let lines = self.info.result.as_ref().map_or(0, Vec::len);
        let y = self.body_top() + 8.0 + lines as f64 * LINE_H + (ROW_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 120.0, y + BTN_H)
    }

    /// The `CLOSE` button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H - 4.0,
        )
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "End the test; the device is left as it was",
            );
        }
        if self.info.result.is_some() {
            return self
                .again_rect()
                .contains(pos)
                .then(|| Help::new(self.again_rect(), "Run the test again with a new order"));
        }
        for slot in 0..2 {
            if self.listen_rect(slot).contains(pos) {
                return help(
                    self.listen_rect(slot),
                    "Play this sound; which one it is stays hidden",
                );
            }
            if self.pick_rect(slot).contains(pos) {
                return help(
                    self.pick_rect(slot),
                    "Choose this sound for the trial (hear both first)",
                );
            }
        }
        if self.match_rect().contains(pos) {
            return help(
                self.match_rect(),
                "Level both sounds to the same loudness for the test",
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
                self.repaint = true;
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
                self.repaint = true;
                if let Some(action) = self.click(*pos) {
                    out.push(UiAction::Blind(action));
                }
            }
            _ => {}
        }
        out
    }

    fn click(&self, pos: Point) -> Option<BlindAction> {
        if self.close_rect().contains(pos) {
            return Some(BlindAction::Close);
        }
        if self.info.result.is_some() {
            return self
                .again_rect()
                .contains(pos)
                .then_some(BlindAction::Again);
        }
        for slot in 0..2 {
            if self.listen_rect(slot).contains(pos) {
                return Some(BlindAction::Listen(slot));
            }
            if self.pick_rect(slot).contains(pos) {
                return self.info.can_pick.then_some(BlindAction::Pick(slot));
            }
        }
        self.match_rect()
            .contains(pos)
            .then_some(BlindAction::ToggleMatch)
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
        let title = format!("BLIND A/B  {}", self.info.device);
        sp::label(
            c,
            &fit_label(&title, p.width() - 2.0 * PAD - 90.0, 12.0),
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let top = self.body_top();
        let width = p.width() - 2.0 * PAD;
        if let Some(lines) = &self.info.result {
            for (i, line) in lines.iter().enumerate() {
                sp::label(
                    c,
                    &fit_label(line, width, 10.5),
                    p.x0 + PAD,
                    top + 18.0 + i as f64 * LINE_H,
                    10.5,
                    t.text,
                    Align::Start,
                );
            }
            let again = self.again_rect();
            sp::button(c, st, again, "AGAIN", Some(st.loop_on), hovered(again));
        } else {
            sp::label(
                c,
                &format!("Trial {} of {}", self.info.trial, self.info.trials),
                p.x0 + PAD,
                top + 18.0,
                11.0,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(
                    "Switch between X and Y as often as you like, then pick the one you prefer.",
                    width,
                    9.5,
                ),
                p.x0 + PAD,
                top + 38.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
            for slot in 0..2u8 {
                let name = SLOT_NAMES[usize::from(slot)];
                let listen = self.listen_rect(slot);
                let lit = (self.info.listening == Some(slot)).then_some(st.loop_on);
                sp::button(
                    c,
                    st,
                    listen,
                    &format!("LISTEN {name}"),
                    lit,
                    hovered(listen),
                );
                let pick = self.pick_rect(slot);
                let live = self.info.can_pick;
                sp::button(
                    c,
                    st,
                    pick,
                    &format!("PREFER {name}"),
                    None,
                    live && hovered(pick),
                );
            }
            let m = self.match_rect();
            sp::button(
                c,
                st,
                m,
                "MATCH LEVEL",
                self.info.match_on.then_some(st.loop_on),
                hovered(m),
            );
        }
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(info: BlindInfo) -> BlindView {
        let mut v = BlindView::new(Theme::dark());
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 700.0));
        v.set_info(info);
        v
    }

    fn click(v: &mut BlindView, at: Rect) -> Vec<UiAction> {
        v.handle(&UiEvent::PointerDown {
            pos: at.center(),
            button: PointerButton::Primary,
            modifiers: Default::default(),
            clicks: 1,
        })
    }

    fn running(can_pick: bool) -> BlindInfo {
        BlindInfo {
            device: "Reverb".into(),
            trial: 2,
            trials: 10,
            can_pick,
            ..BlindInfo::default()
        }
    }

    #[test]
    fn listen_buttons_name_their_slot_and_a_pick_waits_for_both_to_be_heard() {
        let mut v = view(running(false));
        let l1 = v.listen_rect(1);
        assert_eq!(
            click(&mut v, l1),
            vec![UiAction::Blind(BlindAction::Listen(1))]
        );
        let p0 = v.pick_rect(0);
        assert!(
            click(&mut v, p0).is_empty(),
            "no pick before both were heard"
        );
        v.set_info(running(true));
        let p0 = v.pick_rect(0);
        assert_eq!(
            click(&mut v, p0),
            vec![UiAction::Blind(BlindAction::Pick(0))]
        );
    }

    #[test]
    fn the_result_replaces_the_trial_buttons_with_again() {
        let mut info = running(true);
        info.result = Some(vec!["one".into(), "two".into(), "three".into()]);
        let mut v = view(info);
        let again = v.again_rect();
        assert_eq!(
            click(&mut v, again),
            vec![UiAction::Blind(BlindAction::Again)]
        );
        let close = v.close_rect();
        assert_eq!(
            click(&mut v, close),
            vec![UiAction::Blind(BlindAction::Close)]
        );
        assert!(v.panel().contains(again.center()));
    }

    #[test]
    fn the_match_button_toggles_and_the_buttons_do_not_overlap() {
        let mut v = view(running(true));
        let m = v.match_rect();
        assert_eq!(
            click(&mut v, m),
            vec![UiAction::Blind(BlindAction::ToggleMatch)]
        );
        let rects = [
            v.listen_rect(0),
            v.listen_rect(1),
            v.pick_rect(0),
            v.pick_rect(1),
            v.match_rect(),
            v.close_rect(),
        ];
        for (i, a) in rects.iter().enumerate() {
            assert!(v.panel().contains(a.center()));
            for b in &rects[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{a:?} {b:?}");
            }
        }
    }
}
