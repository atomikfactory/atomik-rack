//! The webcam controller's two views: the **consent dialog** of the one-time model download and
//! the small **camera preview** box that floats over the main window.
//!
//! Both are pure views drawn by the app: the host hands them a state ([`CameraDialogState`],
//! [`CameraPreviewState`]) and acts on the [`UiAction`]s they return. They never touch a
//! camera, a file or the network, so a test renders them from a fake picture.
//!
//! # The dialog
//!
//! Turning the camera on with the hand models missing opens it. It says what will be downloaded,
//! from where and how big, and offers **Download** and **Cancel**; nothing is fetched before
//! Download. While it downloads it shows the progress with **Cancel**; a failure shows a plain
//! message with **Retry** and **Close**.
//!
//! # The preview
//!
//! A box of [`PREVIEW_WIDTH`] by [`PREVIEW_HEIGHT`] logical pixels: the state of the hand (NO
//! HAND, HAND or PINCH), the downscaled camera picture shown like a mirror with the landmark dots
//! and lines on it, and under it the target's name and value while pinched, or a message. The
//! frames per second and the milliseconds the models took show as its tooltip. The picture lives
//! in the state only: it is never written anywhere.
//!
//! While the camera sends MIDI (its MIDI controller mode, or MIDI Learn armed) the box grows by a
//! panel of six small bars, the live values of the controls: twist, height, left and right,
//! closeness, pinch and open palm, under the name of the hand and its channel
//! ([`CameraControlsState`]). With MIDI Learn armed in a mode without MIDI it says that the
//! camera is sending for Learn.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::event::{Key, PointerButton, UiEvent};
use crate::i18n::tf;
use crate::paint::fit_label_raw;
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect};
use vello::peniko::{
    Blob, Brush, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality, ImageSampler,
};

/// The width of the preview box, in logical pixels.
pub const PREVIEW_WIDTH: f64 = 176.0;
/// The height of the preview box, in logical pixels.
pub const PREVIEW_HEIGHT: f64 = 188.0;
const PAD: f64 = 8.0;
const HEADER: f64 = 22.0;
const PICTURE_H: f64 = 120.0;
const FONT: f32 = 10.5;
/// The height of the panel of bars in the MIDI controller mode.
const CONTROLS_H: f64 = 66.0;
const BAR_ROW: f64 = 17.0;

/// What the camera sees of the user's hand, as the indicator shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraLamp {
    /// The camera is off.
    #[default]
    Off,
    /// The camera is on and sees no hand.
    NoHand,
    /// A hand is in view.
    Hand,
    /// The thumb and index finger are pinched.
    Pinch,
}

impl CameraLamp {
    /// The word the preview shows.
    pub fn word(self) -> &'static str {
        match self {
            CameraLamp::Off => "OFF",
            CameraLamp::NoHand => "NO HAND",
            CameraLamp::Hand => "HAND",
            CameraLamp::Pinch => "PINCH",
        }
    }
}

/// A small camera picture, RGB8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraPicture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

/// One control of the camera's MIDI: a small bar.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraBar {
    /// What the control is called (`Twist`, `Height`).
    pub label: &'static str,
    /// The value: 0 to 1, or -1 to 1 when `centred`.
    pub value: f32,
    /// The bar grows from its middle (an encoder: the last turn).
    pub centred: bool,
}

/// The live values of the camera's MIDI controls.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraControlsState {
    /// The hand shown and its channel (`Right hand, channel 1`).
    pub hand: &'static str,
    /// One bar for each control, in the order they show (two columns).
    pub bars: Vec<CameraBar>,
    /// The camera sends only because MIDI Learn is armed.
    pub learning: bool,
}

/// What the preview box shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraPreviewState {
    /// What the camera sees of the hand.
    pub lamp: CameraLamp,
    /// The last picture, when the camera has made one.
    pub picture: Option<CameraPicture>,
    /// The landmarks of every hand found, each 21 points in 0..1 over the picture.
    pub hands: Vec<Vec<(f32, f32)>>,
    /// The name of the control being turned (while pinched).
    pub target: Option<String>,
    /// Its value as text (while pinched).
    pub value: Option<String>,
    /// A message for the user (`Click a knob first`, why the camera stopped).
    pub message: Option<String>,
    /// Frames per second the tracker handles.
    pub fps: f32,
    /// Milliseconds the models took for a frame.
    pub infer_ms: f32,
    /// The camera's MIDI controls, while it sends MIDI.
    pub controls: Option<CameraControlsState>,
}

/// The chains of landmarks that make the lines of a hand.
const BONES: [&[usize]; 6] = [
    &[0, 1, 2, 3, 4],
    &[0, 5, 6, 7, 8],
    &[0, 9, 10, 11, 12],
    &[0, 13, 14, 15, 16],
    &[0, 17, 18, 19, 20],
    &[5, 9, 13, 17],
];

/// The preview box. See the module documentation.
#[derive(Debug)]
pub struct CameraPreviewView {
    origin: Point,
    scale: f64,
    state: CameraPreviewState,
    brush: Option<Brush>,
    hover: Option<Point>,
    repaint: bool,
}

impl Default for CameraPreviewView {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraPreviewView {
    /// A preview at the origin; call [`Self::set_origin`] and [`Self::set_state`].
    pub fn new() -> Self {
        Self {
            origin: Point::ZERO,
            scale: 1.0,
            state: CameraPreviewState::default(),
            brush: None,
            hover: None,
            repaint: true,
        }
    }

    /// Asks for a repaint (the theme changed).
    pub fn invalidate(&mut self) {
        self.repaint = true;
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Where the top left of the box is, in window logical pixels.
    pub fn set_origin(&mut self, origin: Point) {
        if origin != self.origin {
            self.origin = origin;
            self.repaint = true;
        }
    }

    /// The height of the box showing `state`: taller while the MIDI controls show.
    pub fn height_for(state: &CameraPreviewState) -> f64 {
        if state.controls.is_some() {
            PREVIEW_HEIGHT + CONTROLS_H
        } else {
            PREVIEW_HEIGHT
        }
    }

    /// The box, in window logical pixels.
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + PREVIEW_WIDTH,
            self.origin.y + Self::height_for(&self.state),
        )
    }

    /// Shows `state`; a repaint only when it changed.
    pub fn set_state(&mut self, state: CameraPreviewState) {
        if state == self.state {
            return;
        }
        if state.picture != self.state.picture {
            self.brush = state.picture.as_ref().map(brush_of);
        }
        self.state = state;
        self.repaint = true;
    }

    /// What the box shows.
    pub fn state(&self) -> &CameraPreviewState {
        &self.state
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The box where the picture is drawn.
    pub fn picture_rect(&self) -> Rect {
        let b = self.bounds();
        Rect::new(
            b.x0 + PAD,
            b.y0 + HEADER,
            b.x1 - PAD,
            b.y0 + HEADER + PICTURE_H,
        )
    }

    /// The `×` that hides the box.
    pub fn close_rect(&self) -> Rect {
        let b = self.bounds();
        Rect::new(b.x1 - 20.0, b.y0 + 3.0, b.x1 - 4.0, b.y0 + 19.0)
    }

    /// The tooltip of the part under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.close_rect().contains(pos) {
            return Some(
                Help::new(self.close_rect(), "Hide the camera preview")
                    .with_command("camera.preview"),
            );
        }
        if self.bounds().contains(pos) {
            let text = tf(
                "{fps} fps, models {ms} ms per frame",
                &[
                    ("fps", &format!("{:.0}", self.state.fps)),
                    ("ms", &format!("{:.0}", self.state.infer_ms)),
                ],
            );
            return Some(Help::new(self.bounds(), text));
        }
        None
    }

    /// Handles one event (window logical pixels). A press on the `×` asks for the preview to be
    /// hidden; a press on the box is taken (nothing under it gets it).
    pub fn handle(&mut self, event: &UiEvent) -> (bool, Vec<UiAction>) {
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                let inside = self.bounds().contains(*pos).then_some(*pos);
                if inside != self.hover {
                    self.hover = inside;
                    self.repaint = true;
                }
                return (inside.is_some(), Vec::new());
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } if self.bounds().contains(*pos) => {
                let out = if self.close_rect().contains(*pos) {
                    vec![UiAction::RunCommand {
                        id: "camera.preview".into(),
                        arg: None,
                    }]
                } else {
                    Vec::new()
                };
                return (true, out);
            }
            _ => {}
        }
        (false, Vec::new())
    }

    /// Paints the box (scaled by the scale factor).
    pub fn paint(&mut self, c: &mut dyn Canvas, t: &Theme) {
        let b = self.bounds();
        c.push_transform(Affine::scale(self.scale));
        let card = RoundedRect::from_rect(b, 8.0);
        c.fill_with(card, t.browser_bg);
        c.stroke_with(card, 1.0, t.section_line);
        // The state of the hand, with a lamp.
        let (word, colour) = match self.state.lamp {
            CameraLamp::Pinch => (CameraLamp::Pinch.word(), t.focus),
            CameraLamp::Hand => (CameraLamp::Hand.word(), t.highlight),
            lamp => (lamp.word(), t.text_muted),
        };
        c.fill_with(
            Circle::new(Point::new(b.x0 + PAD + 3.0, b.y0 + 12.0), 3.5),
            colour,
        );
        c.text(
            &TextRun::new(
                word,
                Point::new(b.x0 + PAD + 12.0, b.y0 + 16.0),
                FONT,
                t.text,
            )
            .weight(700),
        );
        let close = self.close_rect();
        c.text(
            &TextRun::new(
                "\u{d7}",
                Point::new(close.center().x, close.y0 + 13.0),
                13.0,
                if self.hover.is_some_and(|p| close.contains(p)) {
                    t.text
                } else {
                    t.text_muted
                },
            )
            .align(Align::Center),
        );
        // The picture, mirrored like a selfie, with the hands on it.
        let area = self.picture_rect();
        c.fill_with(RoundedRect::from_rect(area, 4.0), t.background);
        c.push_clip(&area.into());
        let mirror = Affine::translate((area.x1, area.y0)) * Affine::scale_non_uniform(-1.0, 1.0);
        if let (Some(pic), Some(brush)) = (&self.state.picture, &self.brush) {
            let sx = area.width() / f64::from(pic.width.max(1));
            let sy = area.height() / f64::from(pic.height.max(1));
            c.push_transform(mirror * Affine::scale_non_uniform(sx, sy));
            c.fill_with(
                Rect::new(0.0, 0.0, f64::from(pic.width), f64::from(pic.height)),
                brush.clone(),
            );
            c.pop_transform();
        }
        c.push_transform(mirror);
        for hand in &self.state.hands {
            let at = |i: usize| {
                hand.get(i).map_or(Point::ZERO, |p| {
                    Point::new(
                        f64::from(p.0) * area.width(),
                        f64::from(p.1) * area.height(),
                    )
                })
            };
            let mut path = BezPath::new();
            for chain in BONES {
                for (n, i) in chain.iter().enumerate() {
                    if n == 0 {
                        path.move_to(at(*i));
                    } else {
                        path.line_to(at(*i));
                    }
                }
            }
            c.stroke_with(path, 1.4, t.highlight);
            for i in 0..hand.len() {
                c.fill_with(Circle::new(at(i), 2.2), t.text);
            }
        }
        c.pop_transform();
        c.pop_clip();
        // The target while pinched, else the message.
        let text_w = PREVIEW_WIDTH - 2.0 * PAD;
        let y = b.y0 + HEADER + PICTURE_H + 16.0;
        if let Some(controls) = &self.state.controls {
            self.paint_controls(c, t, controls, y);
            c.pop_transform();
            self.repaint = false;
            return;
        }
        match (&self.state.target, &self.state.value, &self.state.message) {
            (Some(target), value, _) => {
                c.text(&TextRun::verbatim(
                    fit_label_raw(target, text_w, FONT),
                    Point::new(b.x0 + PAD, y),
                    FONT,
                    t.text,
                ));
                if let Some(value) = value {
                    c.text(
                        &TextRun::verbatim(
                            fit_label_raw(value, text_w, FONT),
                            Point::new(b.x0 + PAD, y + 14.0),
                            FONT,
                            t.focus,
                        )
                        .weight(700),
                    );
                }
            }
            (None, _, Some(message)) => {
                c.text(&TextRun::verbatim(
                    fit_label_raw(&crate::i18n::tr(message), text_w, FONT),
                    Point::new(b.x0 + PAD, y),
                    FONT,
                    t.warning,
                ));
            }
            (None, _, None) => {
                c.text(&TextRun::new(
                    "Click a knob, pinch, twist",
                    Point::new(b.x0 + PAD, y),
                    FONT,
                    t.text_muted,
                ));
            }
        }
        c.pop_transform();
        self.repaint = false;
    }
}

impl CameraPreviewView {
    /// The panel of bars under the picture: the hand and its channel (or that the camera sends
    /// for MIDI Learn), then the six controls in two columns.
    fn paint_controls(
        &self,
        c: &mut dyn Canvas,
        t: &Theme,
        controls: &CameraControlsState,
        y: f64,
    ) {
        let b = self.bounds();
        let text_w = PREVIEW_WIDTH - 2.0 * PAD;
        let (head, colour) = if controls.learning {
            ("Sending for MIDI Learn", t.warning)
        } else {
            (controls.hand, t.text_muted)
        };
        c.text(&TextRun::new(head, Point::new(b.x0 + PAD, y), FONT, colour));
        let col_w = (text_w - 8.0) / 2.0;
        for (i, bar) in controls.bars.iter().enumerate() {
            let (col, row) = (i % 2, i / 2);
            let x0 = b.x0 + PAD + col as f64 * (col_w + 8.0);
            let top = y + 6.0 + row as f64 * BAR_ROW;
            c.text(&TextRun::new(
                bar.label,
                Point::new(x0, top + 8.0),
                9.0,
                t.text_muted,
            ));
            let track = Rect::new(x0, top + 11.0, x0 + col_w, top + 15.0);
            c.fill_with(RoundedRect::from_rect(track, 2.0), t.background);
            let v = f64::from(bar.value.clamp(-1.0, 1.0));
            let fill = if bar.centred {
                let mid = track.center().x;
                let end = mid + v * col_w / 2.0;
                Rect::new(mid.min(end), track.y0, mid.max(end), track.y1)
            } else {
                Rect::new(track.x0, track.y0, track.x0 + v.max(0.0) * col_w, track.y1)
            };
            if fill.width() > 0.0 {
                c.fill_with(RoundedRect::from_rect(fill, 2.0), t.focus);
            }
        }
    }
}

fn brush_of(pic: &CameraPicture) -> Brush {
    let mut rgba = Vec::with_capacity(pic.rgb.len() / 3 * 4);
    for px in pic.rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Brush::Image(ImageBrush {
        image: ImageData {
            data: Blob::new(std::sync::Arc::new(rgba)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: pic.width,
            height: pic.height,
        },
        sampler: ImageSampler::new().with_quality(ImageQuality::Medium),
    })
}

// ---------------------------------------------------------------------- the dialog

/// What the dialog says.
#[derive(Debug, Clone, PartialEq)]
pub enum CameraDialogState {
    /// The question: download the models?
    Consent {
        /// Where they come from (a host name).
        source: String,
        /// How many bytes the download is.
        bytes: u64,
    },
    /// The download is running.
    Downloading {
        /// Bytes received so far.
        received: u64,
        /// Bytes expected.
        total: u64,
    },
    /// The download did not finish.
    Failed {
        /// Why, in plain words.
        message: String,
    },
}

/// A button of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraDialogChoice {
    /// Start the download.
    Download,
    /// Close the question, or stop the download.
    Cancel,
    /// Try the download again.
    Retry,
    /// Close the message.
    Close,
}

impl CameraDialogChoice {
    /// The button label.
    pub fn label(self) -> &'static str {
        match self {
            CameraDialogChoice::Download => "Download",
            CameraDialogChoice::Cancel => "Cancel",
            CameraDialogChoice::Retry => "Retry",
            CameraDialogChoice::Close => "Close",
        }
    }
}

const DIALOG_W: f64 = 520.0;
const DIALOG_H: f64 = 190.0;
const BUTTON_W: f64 = 96.0;
const BUTTON_H: f64 = 28.0;

/// A size in bytes as megabytes with one decimal (`7.5 MB`), in the interface language's digits.
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The consent, progress and failure dialog. See the module documentation.
#[derive(Debug)]
pub struct CameraDialogView {
    scale: f64,
    size: (f64, f64),
    state: CameraDialogState,
    hover: Option<Point>,
    repaint: bool,
}

impl CameraDialogView {
    /// A dialog showing `state` in a window of `size` logical pixels.
    pub fn new(size: (f64, f64), state: CameraDialogState) -> Self {
        Self {
            scale: 1.0,
            size,
            state,
            hover: None,
            repaint: true,
        }
    }

    /// Asks for a repaint (the theme changed).
    pub fn invalidate(&mut self) {
        self.repaint = true;
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The window size in logical pixels.
    pub fn set_size(&mut self, size: (f64, f64)) {
        if size != self.size {
            self.size = size;
            self.repaint = true;
        }
    }

    /// Shows `state`; a repaint only when it changed.
    pub fn set_state(&mut self, state: CameraDialogState) {
        if state != self.state {
            self.state = state;
            self.repaint = true;
        }
    }

    /// What the dialog shows.
    pub fn state(&self) -> &CameraDialogState {
        &self.state
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The card, centred in the window.
    pub fn card(&self) -> Rect {
        let x0 = ((self.size.0 - DIALOG_W) / 2.0).max(0.0);
        let y0 = ((self.size.1 - DIALOG_H) / 2.0).max(0.0);
        Rect::new(x0, y0, x0 + DIALOG_W, y0 + DIALOG_H)
    }

    /// The buttons the state offers, right to left from the corner of the card.
    pub fn choices(&self) -> Vec<(CameraDialogChoice, Rect)> {
        let card = self.card();
        let list: &[CameraDialogChoice] = match self.state {
            CameraDialogState::Consent { .. } => {
                &[CameraDialogChoice::Download, CameraDialogChoice::Cancel]
            }
            CameraDialogState::Downloading { .. } => &[CameraDialogChoice::Cancel],
            CameraDialogState::Failed { .. } => {
                &[CameraDialogChoice::Retry, CameraDialogChoice::Close]
            }
        };
        let y = card.y1 - PAD * 2.0 - BUTTON_H;
        list.iter()
            .enumerate()
            .map(|(n, choice)| {
                let x1 = card.x1 - 2.0 * PAD - n as f64 * (BUTTON_W + PAD);
                (*choice, Rect::new(x1 - BUTTON_W, y, x1, y + BUTTON_H))
            })
            .collect()
    }

    /// The title and the lines of text of the state, in the interface language.
    pub fn lines(&self) -> (String, Vec<String>) {
        use crate::i18n::tr;
        match &self.state {
            CameraDialogState::Consent { source, bytes } => (
                tr("Download the hand models?").into_owned(),
                vec![
                    tf(
                        "Camera control needs two model files ({size}) from {source}.",
                        &[("size", &megabytes(*bytes)), ("source", source)],
                    ),
                    tr("Nothing about you or this computer is sent.").into_owned(),
                    tr("Camera pictures stay in memory and are never saved.").into_owned(),
                    tr("You can delete the models in Settings > Camera.").into_owned(),
                ],
            ),
            CameraDialogState::Downloading { received, total } => (
                tr("Downloading the hand models").into_owned(),
                vec![tf(
                    "{received} of {total}",
                    &[
                        ("received", &megabytes(*received)),
                        ("total", &megabytes(*total)),
                    ],
                )],
            ),
            CameraDialogState::Failed { message } => (
                tr("The download did not finish").into_owned(),
                wrap(message, 62),
            ),
        }
    }

    /// Handles one event. The dialog is modal: it takes every event while it shows.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let ask = |choice| UiAction::CameraDialog(choice);
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
                if let Some((choice, _)) =
                    self.choices().into_iter().find(|(_, r)| r.contains(*pos))
                {
                    out.push(ask(choice));
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                let first = self.choices().first().map(|(c, _)| *c);
                let last = self.choices().last().map(|(c, _)| *c);
                match key {
                    Key::Escape => out.extend(last.map(ask)),
                    Key::Enter => out.extend(first.map(ask)),
                    _ => {}
                }
            }
            _ => {}
        }
        out
    }

    /// Paints the dimmed window and the card.
    pub fn paint(&mut self, c: &mut dyn Canvas, t: &Theme) {
        c.push_transform(Affine::scale(self.scale));
        c.fill_with(
            Rect::new(0.0, 0.0, self.size.0, self.size.1),
            t.background.with_alpha(0.6),
        );
        let card = self.card();
        let shape = RoundedRect::from_rect(card, 10.0);
        c.fill_with(shape, t.browser_bg);
        c.stroke_with(shape, 1.0, t.section_line);
        let (title, lines) = self.lines();
        c.text(
            &TextRun::verbatim(
                title,
                Point::new(card.x0 + 18.0, card.y0 + 30.0),
                15.0,
                t.text,
            )
            .weight(600),
        );
        for (n, line) in lines.iter().enumerate() {
            c.text(&TextRun::verbatim(
                line.clone(),
                Point::new(card.x0 + 18.0, card.y0 + 56.0 + n as f64 * 18.0),
                11.5,
                if n == 0 { t.text } else { t.text_muted },
            ));
        }
        if let CameraDialogState::Downloading { received, total } = &self.state {
            let bar = Rect::new(
                card.x0 + 18.0,
                card.y0 + 84.0,
                card.x1 - 18.0,
                card.y0 + 94.0,
            );
            c.fill_with(RoundedRect::from_rect(bar, 4.0), t.browser_row);
            let part = if *total == 0 {
                0.0
            } else {
                (*received as f64 / *total as f64).clamp(0.0, 1.0)
            };
            c.fill_with(
                RoundedRect::from_rect(
                    Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * part, bar.y1),
                    4.0,
                ),
                t.focus,
            );
        }
        for (choice, r) in self.choices() {
            let lit = self.hover.is_some_and(|p| r.contains(p));
            c.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if lit { t.selection } else { t.browser_row },
            );
            c.text(
                &TextRun::new(
                    choice.label(),
                    Point::new(r.center().x, r.y0 + 18.0),
                    12.0,
                    t.text,
                )
                .align(Align::Center),
            );
        }
        c.pop_transform();
        self.repaint = false;
    }

    /// The tooltip of a button.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let (choice, r) = self.choices().into_iter().find(|(_, r)| r.contains(pos))?;
        let text = match choice {
            CameraDialogChoice::Download => {
                "Download the two model files once and turn the camera on"
            }
            CameraDialogChoice::Cancel => "Close this without downloading anything",
            CameraDialogChoice::Retry => "Try the download again",
            CameraDialogChoice::Close => "Close this message",
        };
        Some(Help::new(r, text))
    }
}

/// `text` broken at spaces into lines of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}
