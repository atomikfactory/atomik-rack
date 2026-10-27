//! The audio export panel: the options of File > Export Song / Export Loop as Audio File, and the
//! progress of the render that follows.
//!
//! ```text
//! EXPORT SONG AS AUDIO FILE
//! my song.wav
//!   BIT DEPTH   [ 16-BIT ] [ 24-BIT ] [ 32-BIT FLOAT ]
//!   DITHER      [ NONE ] [ TPDF ] [ SHAPED ]
//!   SAMPLE RATE [ PROJECT ] [ 44.1 kHz ] [ 48 kHz ] [ 88.2 kHz ] [ 96 kHz ]
//!   TAIL        [ KEEP EFFECT TAIL ]
//!   Renders to the End marker, then keeps the tail while it is audible.
//!                                                          [ EXPORT ] [ CANCEL ]
//! ```
//!
//! A modal view the app places over the window like the relink panel. It has two modes. In the
//! **options** mode ([`ExportView::open`]) the buttons change the [`ExportValues`] the view
//! holds; EXPORT emits [`ExportAction::Start`] with them and CANCEL (or Esc) emits
//! [`ExportAction::Cancel`]. The dither applies to 16-bit files only, so its row is greyed at the
//! other depths; the tail applies to a song only. In the **progress** mode
//! ([`ExportView::show_progress`]) a bar shows the fraction the app feeds with
//! [`ExportView::set_progress`], and CANCEL (or Esc) emits [`ExportAction::Cancel`] to stop the
//! render. The view never closes itself: the app drops it when the export is over.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FILE_H: f64 = 26.0;
const ROW_H: f64 = 32.0;
const HINT_H: f64 = 30.0;
const FOOTER_H: f64 = 44.0;
const BAR_H: f64 = 14.0;
const PROGRESS_H: f64 = 74.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 540.0;
const BTN_H: f64 = 22.0;
const BTN_GAP: f64 = 6.0;
const LABEL_W: f64 = 104.0;

/// What is exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// From the start to the End marker, plus the tail.
    Song,
    /// From the left to the right locator.
    Loop,
}

/// The bit depth of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDepth {
    /// 16-bit PCM.
    Pcm16,
    /// 24-bit PCM.
    Pcm24,
    /// 32-bit float.
    Float32,
}

/// The dither of a 16-bit file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDither {
    /// None: plain rounding.
    Off,
    /// Triangular dither.
    Tpdf,
    /// Triangular dither with the noise shaped towards the high frequencies.
    Shaped,
}

/// The sample rates the panel offers besides the project's, in Hz.
pub const EXPORT_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

/// What the panel sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportValues {
    /// The bit depth.
    pub depth: ExportDepth,
    /// The dither (used at 16-bit only).
    pub dither: ExportDither,
    /// The sample rate in Hz; `None` is the project's.
    pub rate: Option<u32>,
    /// Keep the tail of the song after the End while it is audible.
    pub tail: bool,
}

impl Default for ExportValues {
    fn default() -> Self {
        Self {
            depth: ExportDepth::Pcm24,
            dither: ExportDither::Tpdf,
            rate: None,
            tail: true,
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportAction {
    /// Start the export with these values.
    Start(ExportValues),
    /// Close the options, or stop the render that runs.
    Cancel,
}

/// A button of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Depth(ExportDepth),
    Dither(ExportDither),
    Rate(Option<u32>),
    Tail,
    Start,
    Cancel,
}

/// The panel mode.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Options,
    Progress { fraction: f32, text: String },
}

/// The export panel. See the module documentation.
#[derive(Debug)]
pub struct ExportView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    scope: ExportScope,
    file: String,
    project_rate: u32,
    values: ExportValues,
    mode: Mode,
    hover: Option<Point>,
    repaint: bool,
}

impl ExportView {
    /// A panel in the options mode for a song; call [`Self::open`] to set it up.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            scope: ExportScope::Song,
            file: String::new(),
            project_rate: 48_000,
            values: ExportValues::default(),
            mode: Mode::Options,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels). It is centred in it.
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

    /// Shows the options for exporting `scope` to the file called `file`, with the last `values`;
    /// `project_rate` is the project's sample rate in Hz (named on its button).
    pub fn open(
        &mut self,
        scope: ExportScope,
        file: &str,
        project_rate: u32,
        values: ExportValues,
    ) {
        self.scope = scope;
        self.file = file.to_owned();
        self.project_rate = project_rate;
        self.values = values;
        self.mode = Mode::Options;
        self.repaint = true;
    }

    /// Switches to the progress bar (the render has started).
    pub fn show_progress(&mut self) {
        self.mode = Mode::Progress {
            fraction: 0.0,
            text: String::new(),
        };
        self.repaint = true;
    }

    /// Sets the fraction done, `0.0..=1.0`, and the text beside the bar. Cheap when nothing
    /// changed.
    pub fn set_progress(&mut self, fraction: f32, text: &str) {
        let fraction = fraction.clamp(0.0, 1.0);
        if let Mode::Progress {
            fraction: f,
            text: t,
        } = &mut self.mode
            && (f.to_bits() != fraction.to_bits() || t != text)
        {
            *f = fraction;
            text.clone_into(t);
            self.repaint = true;
        }
    }

    /// True while the progress bar is shown.
    pub fn in_progress(&self) -> bool {
        matches!(self.mode, Mode::Progress { .. })
    }

    /// The values the buttons have set.
    pub fn values(&self) -> ExportValues {
        self.values
    }

    /// What is being exported.
    pub fn scope(&self) -> ExportScope {
        self.scope
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    fn rows(&self) -> usize {
        match self.scope {
            ExportScope::Song => 4,
            ExportScope::Loop => 3,
        }
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let height = match self.mode {
            Mode::Options => HEADER_H + FILE_H + self.rows() as f64 * ROW_H + HINT_H + FOOTER_H,
            Mode::Progress { .. } => HEADER_H + PROGRESS_H + FOOTER_H,
        };
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + height)
    }

    fn row_top(&self, row: usize) -> f64 {
        self.panel().y0 + HEADER_H + FILE_H + row as f64 * ROW_H
    }

    fn button(&self, row: usize, index: usize, width: f64) -> Rect {
        let p = self.panel();
        let x = p.x0 + PAD + LABEL_W + index as f64 * (width + BTN_GAP);
        let y = self.row_top(row) + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + width, y + BTN_H)
    }

    fn footer_button(&self, from_right: usize) -> Rect {
        let p = self.panel();
        let x1 = p.x1 - PAD - from_right as f64 * (96.0 + BTN_GAP);
        Rect::new(
            x1 - 96.0,
            p.y1 - FOOTER_H + (FOOTER_H - BTN_H) / 2.0,
            x1,
            p.y1 - FOOTER_H + (FOOTER_H - BTN_H) / 2.0 + BTN_H,
        )
    }

    /// The progress bar.
    pub fn bar_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + 34.0;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + BAR_H)
    }

    /// Every button with its label, whether it is lit and whether it can be pressed.
    fn buttons(&self) -> Vec<(Hit, Rect, String, bool, bool)> {
        let v = self.values;
        let mut out = Vec::new();
        if self.in_progress() {
            out.push((
                Hit::Cancel,
                self.footer_button(0),
                "CANCEL".to_owned(),
                false,
                true,
            ));
            return out;
        }
        let w = 92.0;
        for (i, (depth, label)) in [
            (ExportDepth::Pcm16, "16-BIT"),
            (ExportDepth::Pcm24, "24-BIT"),
            (ExportDepth::Float32, "32-BIT FLOAT"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Depth(depth),
                self.button(0, i, w),
                label.to_owned(),
                v.depth == depth,
                true,
            ));
        }
        let dither_on = v.depth == ExportDepth::Pcm16;
        for (i, (dither, label)) in [
            (ExportDither::Off, "NONE"),
            (ExportDither::Tpdf, "TPDF"),
            (ExportDither::Shaped, "SHAPED"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Dither(dither),
                self.button(1, i, w),
                label.to_owned(),
                dither_on && v.dither == dither,
                dither_on,
            ));
        }
        let rw = 76.0;
        out.push((
            Hit::Rate(None),
            self.button(2, 0, rw),
            "PROJECT".to_owned(),
            v.rate.is_none(),
            true,
        ));
        for (i, rate) in EXPORT_RATES.into_iter().enumerate() {
            out.push((
                Hit::Rate(Some(rate)),
                self.button(2, i + 1, rw),
                rate_label(rate),
                v.rate == Some(rate),
                true,
            ));
        }
        if self.scope == ExportScope::Song {
            out.push((
                Hit::Tail,
                self.button(3, 0, 150.0),
                "KEEP EFFECT TAIL".to_owned(),
                v.tail,
                true,
            ));
        }
        out.push((
            Hit::Start,
            self.footer_button(1),
            "EXPORT".to_owned(),
            false,
            true,
        ));
        out.push((
            Hit::Cancel,
            self.footer_button(0),
            "CANCEL".to_owned(),
            false,
            true,
        ));
        out
    }

    /// The button labelled `label` (`"EXPORT"`, `"24-BIT"`, `"44.1 kHz"`), for tests and for the
    /// headless session to press.
    pub fn button_rect(&self, label: &str) -> Option<Rect> {
        self.buttons()
            .into_iter()
            .find(|b| b.2 == label)
            .map(|b| b.1)
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        self.buttons()
            .into_iter()
            .find(|(_, r, _, _, enabled)| *enabled && r.contains(pos))
            .map(|(h, ..)| h)
    }

    // ------------------------------------------------------------------ events

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
                if let Some(hit) = self.hit(*pos) {
                    out.extend(self.press(hit).map(UiAction::Export));
                }
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => out.push(UiAction::Export(ExportAction::Cancel)),
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if !self.in_progress() => {
                out.push(UiAction::Export(ExportAction::Start(self.values)));
            }
            _ => {}
        }
        out
    }

    fn press(&mut self, hit: Hit) -> Option<ExportAction> {
        match hit {
            Hit::Depth(d) => self.values.depth = d,
            Hit::Dither(d) => self.values.dither = d,
            Hit::Rate(r) => self.values.rate = r,
            Hit::Tail => self.values.tail = !self.values.tail,
            Hit::Start => return Some(ExportAction::Start(self.values)),
            Hit::Cancel => return Some(ExportAction::Cancel),
        }
        None
    }

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let hit = self.hit(pos)?;
        let rect = self.buttons().into_iter().find(|b| b.0 == hit)?.1;
        let text = match hit {
            Hit::Depth(ExportDepth::Pcm16) => {
                "16-bit integer file, as a compact disc: dither it to keep quiet detail"
            }
            Hit::Depth(ExportDepth::Pcm24) => "24-bit integer file: no dither is needed",
            Hit::Depth(ExportDepth::Float32) => "32-bit float file: the mix is written untouched",
            Hit::Dither(ExportDither::Off) => "Round to 16 bits without dither",
            Hit::Dither(ExportDither::Tpdf) => {
                "Add triangular dither before rounding to 16 bits (the usual choice)"
            }
            Hit::Dither(ExportDither::Shaped) => {
                "Triangular dither with the noise moved towards the high frequencies"
            }
            Hit::Rate(None) => "Write the file at the sample rate of the project",
            Hit::Rate(Some(_)) => "Convert the render to this sample rate",
            Hit::Tail => {
                "Keep what rings out after the End marker (reverb, delay) while it is audible"
            }
            Hit::Start => "Render and write the file",
            Hit::Cancel if self.in_progress() => "Stop the render and write nothing",
            Hit::Cancel => "Close without exporting",
        };
        Some(Help::new(rect, text))
    }

    // ------------------------------------------------------------------ painting

    fn title(&self) -> &'static str {
        match (self.scope, self.in_progress()) {
            (ExportScope::Song, false) => "EXPORT SONG AS AUDIO FILE",
            (ExportScope::Loop, false) => "EXPORT LOOP AS AUDIO FILE",
            (ExportScope::Song, true) => "EXPORTING SONG",
            (ExportScope::Loop, true) => "EXPORTING LOOP",
        }
    }

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
            self.title(),
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let width = p.width() - 2.0 * PAD;
        sp::label(
            c,
            &fit_label(&self.file, width, 11.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 17.0,
            11.0,
            t.text,
            Align::Start,
        );
        match &self.mode {
            Mode::Options => self.paint_options(c, p),
            Mode::Progress { fraction, text } => {
                sp::label(
                    c,
                    &fit_label(text, width, 10.0),
                    p.x0 + PAD,
                    p.y0 + HEADER_H + 30.0,
                    10.0,
                    t.text_muted,
                    Align::Start,
                );
                let bar = self.bar_rect();
                sp::rect(c, bar, st.lane);
                let filled = Rect::new(
                    bar.x0,
                    bar.y0,
                    bar.x0 + bar.width() * f64::from(*fraction),
                    bar.y1,
                );
                sp::rect(c, filled, t.highlight);
                let percent = (fraction * 100.0).round() as u32;
                sp::label(
                    c,
                    &format!("{percent}%"),
                    bar.x1,
                    bar.y1 + 14.0,
                    10.0,
                    t.text_muted,
                    Align::End,
                );
            }
        }
        for (_, rect, text, lit, enabled) in self.buttons() {
            let color = (lit && enabled).then_some(t.highlight);
            if enabled {
                sp::button(c, st, rect, &text, color, hovered(rect));
            } else {
                sp::button(c, st, rect, "", None, false);
                sp::label(
                    c,
                    &text,
                    rect.center().x,
                    rect.center().y + 3.5,
                    9.5,
                    t.text_muted,
                    Align::Center,
                );
            }
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_options(&self, c: &mut dyn Canvas, p: Rect) {
        let t = &self.st.base;
        let mut rows = vec!["BIT DEPTH", "DITHER", "SAMPLE RATE"];
        if self.scope == ExportScope::Song {
            rows.push("TAIL");
        }
        for (i, name) in rows.iter().enumerate() {
            sp::label(
                c,
                name,
                p.x0 + PAD,
                self.row_top(i) + ROW_H / 2.0 + 3.5,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        let hint = match self.scope {
            ExportScope::Song => {
                "From the start to the End marker; the tail is kept while audible."
            }
            ExportScope::Loop => "Exactly from the left to the right locator, with no tail.",
        };
        let note = if self.values.depth == ExportDepth::Pcm16 {
            hint.to_owned()
        } else if self.values.depth == ExportDepth::Pcm24 {
            format!("{hint} 24-bit needs no dither.")
        } else {
            format!("{hint} Float is written as it is.")
        };
        sp::label(
            c,
            &fit_label(&note, p.width() - 2.0 * PAD, 10.0),
            p.x0 + PAD,
            self.row_top(self.rows()) + 14.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        sp::label(
            c,
            &format!("Project rate {}", rate_label(self.project_rate)),
            p.x0 + PAD,
            p.y1 - FOOTER_H / 2.0 + 3.5,
            10.0,
            t.text_muted,
            Align::Start,
        );
    }
}

/// A rate as a button label: `44.1 kHz`, `48 kHz`.
pub fn rate_label(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}
