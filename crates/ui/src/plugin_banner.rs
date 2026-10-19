//! The project banner of the plugin consent flow: a strip the
//! app shows above the rack.
//!
//! ```text
//! This project uses 3 third-party plugins                       [LOAD] [OPEN WITHOUT]
//! ```
//!
//! * [`BannerKind::ProjectUsesPlugins`]: a project that uses plugins was opened while plugins
//!   are off or some are missing, so it opened with stand-ins. `LOAD` turns plugins on for this
//!   session after the same native-code warning as the settings switch (`needs_consent`; when
//!   plugins were already enabled there is nothing to warn about) and emits
//!   [`PluginBannerAction::Load`]; `OPEN WITHOUT` keeps the stand-ins
//!   ([`PluginBannerAction::OpenWithout`]). Either hides the banner.
//! * [`BannerKind::UncleanExit`]: the crash journal says Atomik ended unexpectedly while these
//!   plugins were loaded. `DISABLE` blacklists them ([`PluginBannerAction::DisableCrashed`]),
//!   `KEEP` leaves them ([`PluginBannerAction::KeepCrashed`]), both hide the banner;
//!   `OPEN RECOVERY` (when a recovery snapshot exists) emits
//!   [`PluginBannerAction::OpenRecovery`] and leaves the question open.
//!
//! Hosting: `set_bounds` (only the width and the top-left matter), `show`, `handle`, `paint`,
//! `height` (0 while hidden; it grows while the warning is open, so lay the rack out below it).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::plugin_data::{ENABLE_WARNING, PluginBannerAction};
use crate::plugin_settings::wrap_words;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use vello::kurbo::{Affine, Point, Rect};

/// Height of the banner strip.
pub const BAR_HEIGHT: f64 = 40.0;
/// Extra height while the warning is open.
pub const WARNING_HEIGHT: f64 = 84.0;
const PAD: f64 = 14.0;
const BTN_H: f64 = 24.0;
const GAP: f64 = 8.0;
const WRAP_CHARS: usize = 110;

/// What the banner says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerKind {
    /// The project uses `count` third-party plugins and opened with stand-ins.
    ProjectUsesPlugins {
        /// How many plugins the project uses.
        count: usize,
        /// Plugins are off, so LOAD must show the warning first.
        needs_consent: bool,
    },
    /// The last session ended unexpectedly while these plugins were loaded.
    UncleanExit {
        /// Their names.
        plugins: Vec<String>,
        /// A recovery snapshot exists.
        has_recovery: bool,
    },
}

/// The banner. See the module documentation.
#[derive(Debug)]
pub struct PluginBannerView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    kind: Option<BannerKind>,
    warning: bool,
    hover: Option<Point>,
    repaint: bool,
}

impl PluginBannerView {
    /// A hidden banner.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, BAR_HEIGHT),
            scale: 1.0,
            kind: None,
            warning: false,
            hover: None,
            repaint: true,
        }
    }

    /// Places the banner: its width and top-left corner (logical pixels).
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

    /// Shows the banner (closing any open warning).
    pub fn show(&mut self, kind: BannerKind) {
        self.kind = Some(kind);
        self.warning = false;
        self.repaint = true;
    }

    /// Hides the banner.
    pub fn hide(&mut self) {
        if self.kind.take().is_some() {
            self.warning = false;
            self.repaint = true;
        }
    }

    /// Whether the banner is shown.
    pub fn is_visible(&self) -> bool {
        self.kind.is_some()
    }

    /// What the banner says.
    pub fn kind(&self) -> Option<&BannerKind> {
        self.kind.as_ref()
    }

    /// Whether the native-code warning is open.
    pub fn is_warning_open(&self) -> bool {
        self.warning
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The banner's height: 0 while hidden, the strip, plus the warning while it is open.
    pub fn height(&self) -> f64 {
        match (&self.kind, self.warning) {
            (None, _) => 0.0,
            (Some(_), false) => BAR_HEIGHT,
            (Some(_), true) => BAR_HEIGHT + WARNING_HEIGHT,
        }
    }

    fn bar(&self) -> Rect {
        Rect::new(
            self.bounds.x0,
            self.bounds.y0,
            self.bounds.x1,
            self.bounds.y0 + BAR_HEIGHT,
        )
    }

    /// Buttons from the right: each is (width, index in the order they are listed).
    fn right_buttons(&self, widths: &[f64]) -> Vec<Rect> {
        let bar = self.bar();
        let mut x = bar.x1 - PAD;
        let y = bar.y0 + (BAR_HEIGHT - BTN_H) / 2.0;
        let mut rects: Vec<Rect> = widths
            .iter()
            .rev()
            .map(|w| {
                let r = Rect::new(x - w, y, x, y + BTN_H);
                x -= w + GAP;
                r
            })
            .collect();
        rects.reverse();
        rects
    }

    fn project_buttons(&self) -> Option<[Rect; 2]> {
        matches!(self.kind, Some(BannerKind::ProjectUsesPlugins { .. })).then(|| {
            let r = self.right_buttons(&[70.0, 120.0]);
            [r[0], r[1]]
        })
    }

    fn crash_buttons(&self) -> Option<(Rect, Rect, Option<Rect>)> {
        let Some(BannerKind::UncleanExit { has_recovery, .. }) = &self.kind else {
            return None;
        };
        if *has_recovery {
            let r = self.right_buttons(&[80.0, 70.0, 130.0]);
            Some((r[0], r[1], Some(r[2])))
        } else {
            let r = self.right_buttons(&[80.0, 70.0]);
            Some((r[0], r[1], None))
        }
    }

    /// The LOAD button.
    pub fn load_rect(&self) -> Option<Rect> {
        self.project_buttons().map(|b| b[0])
    }

    /// The OPEN WITHOUT button.
    pub fn open_without_rect(&self) -> Option<Rect> {
        self.project_buttons().map(|b| b[1])
    }

    /// The DISABLE button of the unclean-exit banner.
    pub fn disable_rect(&self) -> Option<Rect> {
        self.crash_buttons().map(|b| b.0)
    }

    /// The KEEP button of the unclean-exit banner.
    pub fn keep_rect(&self) -> Option<Rect> {
        self.crash_buttons().map(|b| b.1)
    }

    /// The OPEN RECOVERY button of the unclean-exit banner (when a snapshot exists).
    pub fn recovery_rect(&self) -> Option<Rect> {
        self.crash_buttons().and_then(|b| b.2)
    }

    fn warning_area(&self) -> Option<Rect> {
        self.warning.then(|| {
            Rect::new(
                self.bounds.x0,
                self.bounds.y0 + BAR_HEIGHT,
                self.bounds.x1,
                self.bounds.y0 + BAR_HEIGHT + WARNING_HEIGHT,
            )
        })
    }

    fn warning_buttons(&self) -> Option<(Rect, Rect)> {
        let a = self.warning_area()?;
        let y = a.y1 - 32.0;
        Some((
            Rect::new(a.x0 + PAD, y, a.x0 + PAD + 150.0, y + BTN_H),
            Rect::new(a.x0 + PAD + 158.0, y, a.x0 + PAD + 158.0 + 80.0, y + BTN_H),
        ))
    }

    /// The accept button of the warning (`I UNDERSTAND - LOAD`).
    pub fn accept_rect(&self) -> Option<Rect> {
        self.warning_buttons().map(|b| b.0)
    }

    /// The CANCEL button of the warning.
    pub fn cancel_rect(&self) -> Option<Rect> {
        self.warning_buttons().map(|b| b.1)
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
            } if self.kind.is_some() => self.pointer_down(*pos, &mut out),
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } if self.warning => {
                self.warning = false;
                self.repaint = true;
            }
            _ => {}
        }
        out
    }

    fn pointer_down(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        self.repaint = true;
        let emit = |out: &mut Vec<UiAction>, a: PluginBannerAction| {
            out.push(UiAction::PluginBanner(a));
        };
        if self.warning {
            if self.accept_rect().is_some_and(|r| r.contains(pos)) {
                emit(
                    out,
                    PluginBannerAction::Load {
                        warning_acknowledged: true,
                    },
                );
                self.hide();
            } else if self.cancel_rect().is_some_and(|r| r.contains(pos)) {
                self.warning = false;
            }
            return;
        }
        if self.load_rect().is_some_and(|r| r.contains(pos)) {
            let needs = matches!(
                self.kind,
                Some(BannerKind::ProjectUsesPlugins {
                    needs_consent: true,
                    ..
                })
            );
            if needs {
                self.warning = true;
            } else {
                emit(
                    out,
                    PluginBannerAction::Load {
                        warning_acknowledged: true,
                    },
                );
                self.hide();
            }
        } else if self.open_without_rect().is_some_and(|r| r.contains(pos)) {
            emit(out, PluginBannerAction::OpenWithout);
            self.hide();
        } else if self.disable_rect().is_some_and(|r| r.contains(pos)) {
            emit(out, PluginBannerAction::DisableCrashed);
            self.hide();
        } else if self.keep_rect().is_some_and(|r| r.contains(pos)) {
            emit(out, PluginBannerAction::KeepCrashed);
            self.hide();
        } else if self.recovery_rect().is_some_and(|r| r.contains(pos)) {
            emit(out, PluginBannerAction::OpenRecovery);
        }
    }

    /// Paints the banner (nothing while hidden).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let Some(kind) = &self.kind else { return };
        let st = &self.st;
        let t = &st.base;
        let accent = t.highlight;
        c.push_transform(Affine::scale(self.scale));
        let all = Rect::new(
            self.bounds.x0,
            self.bounds.y0,
            self.bounds.x1,
            self.bounds.y0 + self.height(),
        );
        c.push_clip(&all.into());
        let bar = self.bar();
        sp::rect(c, bar, st.header);
        sp::line(c, bar.x0, bar.y0 + 0.5, bar.x1, bar.y0 + 0.5, accent);
        sp::line(c, bar.x0, bar.y1 - 0.5, bar.x1, bar.y1 - 0.5, st.separator);
        let hovered = |r: Rect| self.hover.is_some_and(|p| r.contains(p));
        let text = match kind {
            BannerKind::ProjectUsesPlugins { count, .. } => format!(
                "This project uses {count} third-party plugin{}",
                if *count == 1 { "" } else { "s" }
            ),
            BannerKind::UncleanExit { plugins, .. } => format!(
                "Atomik ended unexpectedly while these plugins were loaded: {}",
                plugins.join(", ")
            ),
        };
        let leftmost = self
            .button_list()
            .iter()
            .map(|b| b.0.x0)
            .fold(bar.x1, f64::min);
        let width = leftmost - bar.x0 - PAD * 2.0;
        sp::label(
            c,
            &fit_label(&text, width.max(100.0), 10.0),
            bar.x0 + PAD,
            bar.y0 + 24.0,
            10.0,
            t.text,
            Align::Start,
        );
        for (rect, label, lit) in self.button_list() {
            sp::button(c, st, rect, label, lit.then_some(accent), hovered(rect));
        }
        if let Some(area) = self.warning_area() {
            sp::rect(c, area, t.lcd);
            for (i, line) in wrap_words(ENABLE_WARNING, WRAP_CHARS).iter().enumerate() {
                sp::label(
                    c,
                    line,
                    area.x0 + PAD,
                    area.y0 + 18.0 + i as f64 * 14.0,
                    9.5,
                    t.text,
                    Align::Start,
                );
            }
            if let Some((a, cancel)) = self.warning_buttons() {
                sp::button(c, st, a, "I UNDERSTAND - LOAD", Some(accent), hovered(a));
                sp::button(c, st, cancel, "CANCEL", None, hovered(cancel));
            }
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn button_list(&self) -> Vec<(Rect, &'static str, bool)> {
        let mut v = Vec::new();
        if let (Some(a), Some(b)) = (self.load_rect(), self.open_without_rect()) {
            v.push((a, "LOAD", false));
            v.push((b, "OPEN WITHOUT", false));
        }
        if let Some((a, b, r)) = self.crash_buttons() {
            v.push((a, "DISABLE", false));
            v.push((b, "KEEP", false));
            if let Some(r) = r {
                v.push((r, "OPEN RECOVERY", false));
            }
        }
        v
    }
}
