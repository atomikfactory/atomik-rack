//! The Learn strip of a plugin editor window: a thin band, drawn by the app, that picks what a
//! touch on a control of the plugin's own interface does.
//!
//! ```text
//! Test Synth  [LEARN AUTOMATION] [LEARN MIDI]  Cutoff: lane shown                        [×]
//! ```
//!
//! **Pick before touching.** At most one of the two toggles is armed. With LEARN AUTOMATION, the
//! next control the user moves in the plugin shows its automation lane in the arrangement (an
//! empty lane is added when it has none); with LEARN MIDI, that control is armed as the MIDI
//! learn target and the next hardware control moved is mapped to it. The mode stays armed for the
//! next touch; a click on the lit button, the `×` or Esc in the main window disarms it.
//!
//! The strip owns nothing: the host gives it the state ([`PluginLearnStrip::set_state`]) and
//! acts on the [`UiAction::PluginLearn`] it returns. The same view is drawn in the window's strip
//! and, as the fallback where a window cannot have one, in the parameter pane.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, TextRun};
use crate::event::{PointerButton, UiEvent};
use crate::i18n::tf;
use crate::paint::fit_label_raw;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_model::DeviceKey;
use vello::kurbo::{Affine, Point, Rect};

/// The height of the strip (logical pixels).
pub const STRIP_HEIGHT: f64 = 28.0;
const PAD: f64 = 8.0;
const NAME_W: f64 = 140.0;
const AUTO_W: f64 = 132.0;
const MIDI_W: f64 = 100.0;
const GAP: f64 = 6.0;
const BTN_H: f64 = 20.0;
const CLOSE_W: f64 = 22.0;
const FONT: f32 = 10.5;

/// What a touch on a control of the plugin's own window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluginLearnMode {
    /// Show the touched param's automation lane (adding an empty one when it has none).
    Automation,
    /// Arm the touched param as the MIDI learn target.
    Midi,
}

/// What the strip last has to say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PluginLearnStatus {
    /// Nothing was touched since the mode was armed.
    #[default]
    Waiting,
    /// The lane of this param is shown.
    LaneShown(String),
    /// This param has no lane to show (a device inside a container, say).
    NoLane(String),
    /// This param waits for a hardware control.
    MoveHardware(String),
    /// This param was mapped to the source described.
    Mapped {
        /// The param's name.
        param: String,
        /// The source (`CC 21 ch 1`).
        source: String,
    },
    /// The control touched is not one of the plugin's automatable params.
    NotAutomatable,
}

/// What the strip shows for one plugin window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLearnState {
    /// The plugin device (path key).
    pub device: DeviceKey,
    /// The plugin's name, drawn as it is.
    pub plugin: String,
    /// The armed mode, if any.
    pub mode: Option<PluginLearnMode>,
    /// What happened last.
    pub status: PluginLearnStatus,
}

/// What the strip asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLearnRequest {
    /// Arm this mode, or disarm it when it is the armed one.
    Toggle(PluginLearnMode),
    /// Disarm whatever is armed.
    Off,
}

impl PluginLearnState {
    /// The status line in the interface language.
    pub fn status_text(&self) -> String {
        match &self.status {
            PluginLearnStatus::Waiting => match self.mode {
                None => crate::i18n::tr(
                    "Pick LEARN AUTOMATION or LEARN MIDI, then touch a control in the plugin",
                )
                .into_owned(),
                Some(PluginLearnMode::Automation) => {
                    crate::i18n::tr("Touch a control in the plugin to show its lane").into_owned()
                }
                Some(PluginLearnMode::Midi) => {
                    crate::i18n::tr("Touch a control in the plugin to map it to MIDI").into_owned()
                }
            },
            PluginLearnStatus::LaneShown(p) => tf("{param}: lane shown", &[("param", p)]),
            PluginLearnStatus::NoLane(p) => tf("{param}: no automation lane here", &[("param", p)]),
            PluginLearnStatus::MoveHardware(p) => {
                tf("{param}: move a hardware control", &[("param", p)])
            }
            PluginLearnStatus::Mapped { param, source } => {
                format!("{param} \u{2190} {source}")
            }
            PluginLearnStatus::NotAutomatable => {
                crate::i18n::tr("This control is not automatable").into_owned()
            }
        }
    }
}

/// The Learn strip. See the module documentation.
#[derive(Debug)]
pub struct PluginLearnStrip {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    state: PluginLearnState,
    hover: Option<Point>,
    repaint: bool,
}

impl PluginLearnStrip {
    /// A strip with nothing armed; call [`Self::set_bounds`] and [`Self::set_state`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 640.0, STRIP_HEIGHT),
            scale: 1.0,
            state: PluginLearnState {
                device: DeviceKey(String::new()),
                plugin: String::new(),
                mode: None,
                status: PluginLearnStatus::Waiting,
            },
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// The area of the strip (logical pixels; the height is [`STRIP_HEIGHT`]).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// The area of the strip.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Shows `state`; a repaint only when it changed.
    pub fn set_state(&mut self, state: PluginLearnState) {
        if state != self.state {
            self.state = state;
            self.repaint = true;
        }
    }

    /// Where the pointer is over the strip, if it is.
    pub fn hover(&self) -> Option<Point> {
        self.hover
    }

    /// What the strip shows.
    pub fn state(&self) -> &PluginLearnState {
        &self.state
    }

    /// True when something changed since the last [`Self::paint`] (call [`Self::painted`]).
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The host painted the strip.
    pub fn painted(&mut self) {
        self.repaint = false;
    }

    // ------------------------------------------------------------------ geometry

    fn button_top(&self) -> f64 {
        self.bounds.y0 + (STRIP_HEIGHT - BTN_H) / 2.0
    }

    /// The LEARN AUTOMATION toggle.
    pub fn automation_rect(&self) -> Rect {
        let x = self.bounds.x0 + PAD + NAME_W + GAP;
        let y = self.button_top();
        Rect::new(x, y, x + AUTO_W, y + BTN_H)
    }

    /// The LEARN MIDI toggle.
    pub fn midi_rect(&self) -> Rect {
        let x = self.automation_rect().x1 + GAP;
        let y = self.button_top();
        Rect::new(x, y, x + MIDI_W, y + BTN_H)
    }

    /// The `×` that disarms.
    pub fn close_rect(&self) -> Rect {
        let x1 = self.bounds.x1 - PAD;
        let y = self.button_top();
        Rect::new(x1 - CLOSE_W, y, x1, y + BTN_H)
    }

    fn status_left(&self) -> f64 {
        self.midi_rect().x1 + 2.0 * GAP
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.automation_rect().contains(pos) {
            return Some(Help::new(
                self.automation_rect(),
                "Touch a control in the plugin to show its automation lane",
            ));
        }
        if self.midi_rect().contains(pos) {
            return Some(Help::new(
                self.midi_rect(),
                "Touch a control in the plugin, then move a hardware control to map it",
            ));
        }
        if self.close_rect().contains(pos) {
            return Some(Help::new(self.close_rect(), "Stop learning (Esc)"));
        }
        None
    }

    fn request(&self, request: PluginLearnRequest) -> UiAction {
        UiAction::PluginLearn {
            device: self.state.device.clone(),
            request,
        }
    }

    /// Handles one event (in the strip's logical coordinates) and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
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
                if self.automation_rect().contains(*pos) {
                    out.push(self.request(PluginLearnRequest::Toggle(PluginLearnMode::Automation)));
                } else if self.midi_rect().contains(*pos) {
                    out.push(self.request(PluginLearnRequest::Toggle(PluginLearnMode::Midi)));
                } else if self.close_rect().contains(*pos) {
                    out.push(self.request(PluginLearnRequest::Off));
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    /// Paints the strip (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let b = self.bounds;
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&b.into());
        sp::rect(c, b, st.header);
        sp::line(c, b.x0, b.y1 - 0.5, b.x1, b.y1 - 0.5, st.separator);
        let baseline = b.y0 + STRIP_HEIGHT / 2.0 + 4.0;
        c.text(
            &TextRun::verbatim(
                fit_label_raw(&self.state.plugin, NAME_W, FONT),
                Point::new(b.x0 + PAD, baseline),
                FONT,
                t.text,
            )
            .align(Align::Start)
            .weight(700),
        );
        let (auto, midi) = (self.automation_rect(), self.midi_rect());
        let lit = |mode| self.state.mode == Some(mode);
        sp::button(
            c,
            st,
            auto,
            "LEARN AUTOMATION",
            lit(PluginLearnMode::Automation).then_some(st.loop_on),
            hovered(auto),
        );
        sp::button(
            c,
            st,
            midi,
            "LEARN MIDI",
            lit(PluginLearnMode::Midi).then_some(t.warning),
            hovered(midi),
        );
        let close = self.close_rect();
        sp::button(c, st, close, "\u{d7}", None, hovered(close));
        let room = (close.x0 - GAP - self.status_left()).max(1.0);
        let colour = match (&self.state.status, self.state.mode) {
            (PluginLearnStatus::NotAutomatable | PluginLearnStatus::NoLane(_), _) => t.warning,
            (_, None) => t.text_muted,
            _ => t.text,
        };
        c.text(
            &TextRun::verbatim(
                fit_label_raw(&self.state.status_text(), room, FONT),
                Point::new(self.status_left(), baseline),
                FONT,
                colour,
            )
            .align(Align::Start),
        );
        c.pop_clip();
        c.pop_transform();
    }
}
