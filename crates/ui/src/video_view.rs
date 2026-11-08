//! The Video window: the picture of the song's film at the song position.
//!
//! The view paints what the app core hands it ([`VideoFrame`]): the decoded picture, black (the
//! song position is before or after the film), a progress bar (the proxy is being built) or a
//! message (no film, the file is missing, FFmpeg is not installed). It decodes nothing and reads
//! no file: the core does that on a worker thread and passes in the finished pixels.
//!
//! * **Picture.** An RGBA image, drawn with its aspect ratio kept inside the window above the
//!   bar. The size control sets the scale: Fit (as large as the window allows), or 50, 100 or
//!   200 % of the film's own size (a 1920 by 1080 film at 50 % is 960 by 540 logical pixels; the
//!   preview is a smaller copy, so a large size is a magnified preview). What does not fit is
//!   cut off at the edge of the window.
//! * **Overlay.** The timecode (`HH:MM:SS:FF`, drop frame with `;`) and the frame number on a
//!   plate in a corner of the picture; each can be switched off. They show on black too.
//! * **Bar.** Size buttons (FIT, 50%, 100%, 200%) on the left, the two overlay switches on the
//!   right. The choices are view state, kept by the shell in `ui-state.json`
//!   ([`VideoView::prefs`]); they are no undo step.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{PointerButton, UiEvent};
use crate::peniko::{
    Blob, Brush, Color, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality,
    ImageSampler,
};
use crate::seq_paint::{SeqTheme, button};
use crate::theme::Theme;
use crate::tooltip::Help;
use std::sync::Arc;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};

/// The size the window opens with when no geometry was saved.
pub const DEFAULT_SIZE: (f64, f64) = (640.0, 410.0);

/// The height of the bar at the bottom, in logical pixels.
const BAR_H: f64 = 30.0;
/// The overlay's text, on its plate: fixed colours, since they sit on a picture, not on the
/// theme's panels.
const OVERLAY_TEXT: Color = Color::from_rgba8(244, 244, 244, 255);
const OVERLAY_PLATE: Color = Color::from_rgba8(0, 0, 0, 170);
/// The area around the film's rectangle when the film is smaller than the window.
const SURROUND: Color = Color::from_rgba8(22, 22, 24, 255);
const FILM_BLACK: Color = Color::from_rgba8(0, 0, 0, 255);

/// How large the picture is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoSize {
    /// As large as the window allows.
    #[default]
    Fit,
    /// Half the film's size.
    Half,
    /// The film's size.
    Full,
    /// Twice the film's size.
    Double,
}

impl VideoSize {
    /// Every size, in the order of the buttons.
    pub const ALL: [VideoSize; 4] = [
        VideoSize::Fit,
        VideoSize::Half,
        VideoSize::Full,
        VideoSize::Double,
    ];

    /// The button's label.
    pub fn label(self) -> &'static str {
        match self {
            VideoSize::Fit => "FIT",
            VideoSize::Half => "50%",
            VideoSize::Full => "100%",
            VideoSize::Double => "200%",
        }
    }

    /// The scale of the film's own size, `None` for Fit.
    pub fn factor(self) -> Option<f64> {
        match self {
            VideoSize::Fit => None,
            VideoSize::Half => Some(0.5),
            VideoSize::Full => Some(1.0),
            VideoSize::Double => Some(2.0),
        }
    }

    fn help(self) -> &'static str {
        match self {
            VideoSize::Fit => "Show the picture as large as the window allows",
            VideoSize::Half => "Show the picture at half the film's size",
            VideoSize::Full => "Show the picture at the film's size",
            VideoSize::Double => "Show the picture at twice the film's size",
        }
    }
}

/// What the overlay shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoOverlay {
    /// The timecode.
    pub timecode: bool,
    /// The frame number.
    pub frame_number: bool,
}

impl Default for VideoOverlay {
    fn default() -> Self {
        Self {
            timecode: true,
            frame_number: false,
        }
    }
}

impl VideoOverlay {
    /// Whether anything shows.
    pub fn any(self) -> bool {
        self.timecode || self.frame_number
    }
}

/// The view's saved choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoPrefs {
    /// The size control.
    pub size: VideoSize,
    /// The overlay switches.
    pub overlay: VideoOverlay,
}

/// A decoded picture handed to the view. `serial` names the picture (the same serial is the same
/// pixels), so the view uploads a picture once, not on every repaint.
#[derive(Debug, Clone)]
pub struct VideoImage {
    /// Identifies the picture.
    pub serial: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes of RGBA, opaque.
    pub rgba: Arc<Vec<u8>>,
}

/// What the window shows in its picture area.
#[derive(Debug, Clone)]
pub enum VideoScreen {
    /// Nothing to show: the message says why (no film, missing file, no FFmpeg...).
    Message(String),
    /// The proxy is being built: the fraction done and the line to show.
    Progress(f32, String),
    /// The film is not on screen at this position: black.
    Black,
    /// The film's frame.
    Picture(VideoImage),
}

/// Everything the window shows, set by the app core.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// The picture area.
    pub screen: VideoScreen,
    /// The film's picture size as it shows (rotation applied): sets the size of 100 %.
    pub film: (u32, u32),
    /// The timecode text of the frame (empty: none).
    pub timecode: String,
    /// The frame number counted from the first frame of the film.
    pub frame_number: Option<i64>,
}

impl Default for VideoFrame {
    fn default() -> Self {
        Self {
            screen: VideoScreen::Message("The song has no video".to_owned()),
            film: (16, 9),
            timecode: String::new(),
            frame_number: None,
        }
    }
}

/// The rectangle the picture is drawn in: the film's size scaled by `size`, centred in `area`
/// with its aspect ratio kept (the rectangle may be larger than `area`: the view clips it).
pub fn picture_rect(area: Rect, film: (u32, u32), size: VideoSize) -> Rect {
    let (fw, fh) = (f64::from(film.0.max(1)), f64::from(film.1.max(1)));
    let scale = size
        .factor()
        .unwrap_or_else(|| (area.width() / fw).min(area.height() / fh).max(0.0));
    let (w, h) = (fw * scale, fh * scale);
    let c = area.center();
    Rect::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Size(VideoSize),
    Timecode,
    FrameNumber,
}

/// The Video view (see the module docs).
#[derive(Debug, Clone)]
pub struct VideoView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    frame: VideoFrame,
    prefs: VideoPrefs,
    hover: Option<Hit>,
    brush: Option<(u64, Brush)>,
    repaint: bool,
}

impl VideoView {
    /// A view with nothing to show.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            scale: 1.0,
            frame: VideoFrame::default(),
            prefs: VideoPrefs::default(),
            hover: None,
            brush: None,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Switches to `theme`.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Where the view is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Where the view is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The display scale factor (the view paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The saved choices (size, overlay).
    pub fn prefs(&self) -> VideoPrefs {
        self.prefs
    }

    /// Sets the saved choices.
    pub fn set_prefs(&mut self, prefs: VideoPrefs) {
        if prefs != self.prefs {
            self.prefs = prefs;
            self.repaint = true;
        }
    }

    /// Switches the whole overlay off, or back on (the timecode when both were off).
    pub fn toggle_overlay(&mut self) {
        let o = &mut self.prefs.overlay;
        if o.any() {
            o.timecode = false;
            o.frame_number = false;
        } else {
            o.timecode = true;
        }
        self.repaint = true;
    }

    /// Sets what the window shows. The view repaints only when it differs from what it had.
    pub fn set_frame(&mut self, frame: VideoFrame) {
        if !same_frame(&self.frame, &frame) {
            self.frame = frame;
            self.repaint = true;
        }
    }

    /// What the window shows now.
    pub fn frame(&self) -> &VideoFrame {
        &self.frame
    }

    /// Whether the view has something to paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ layout

    /// The picture area: the window above the bar.
    pub fn area(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x0, b.y0, b.x1, (b.y1 - BAR_H).max(b.y0))
    }

    /// The rectangle the film is drawn in (see [`picture_rect`]).
    pub fn film_rect(&self) -> Rect {
        picture_rect(self.area(), self.frame.film, self.prefs.size)
    }

    fn bar(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x0, (b.y1 - BAR_H).max(b.y0), b.x1, b.y1)
    }

    fn size_rects(&self) -> Vec<(VideoSize, Rect)> {
        let bar = self.bar();
        let mut x = bar.x0 + 8.0;
        VideoSize::ALL
            .iter()
            .map(|s| {
                let w = 14.0 + s.label().len() as f64 * 6.6;
                let r = Rect::new(x, bar.y0 + 4.0, x + w, bar.y1 - 4.0);
                x += w + 5.0;
                (*s, r)
            })
            .collect()
    }

    fn overlay_rects(&self) -> [(Hit, &'static str, Rect); 2] {
        let bar = self.bar();
        let frame = Rect::new(
            bar.x1 - 8.0 - 28.0,
            bar.y0 + 4.0,
            bar.x1 - 8.0,
            bar.y1 - 4.0,
        );
        let tc = Rect::new(
            frame.x0 - 5.0 - 36.0,
            bar.y0 + 4.0,
            frame.x0 - 5.0,
            bar.y1 - 4.0,
        );
        [(Hit::Timecode, "TC", tc), (Hit::FrameNumber, "#", frame)]
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        if let Some((s, _)) = self.size_rects().into_iter().find(|(_, r)| r.contains(pos)) {
            return Some(Hit::Size(s));
        }
        self.overlay_rects()
            .into_iter()
            .find(|(_, _, r)| r.contains(pos))
            .map(|(h, _, _)| h)
    }

    /// The tooltip of what is under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let hit = self.hit(pos)?;
        Some(match hit {
            Hit::Size(s) => Help::new(
                self.size_rects().into_iter().find(|(k, _)| *k == s)?.1,
                s.help(),
            ),
            Hit::Timecode => Help::new(
                self.overlay_rects()[0].2,
                "Show the timecode of the picture over it",
            ),
            Hit::FrameNumber => Help::new(
                self.overlay_rects()[1].2,
                "Show the frame number of the picture over it",
            ),
        })
    }

    // ------------------------------------------------------------------ input

    /// Handles an event in the window's logical coordinates.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                match self.hit(*pos) {
                    Some(Hit::Size(s)) => self.prefs.size = s,
                    Some(Hit::Timecode) => self.prefs.overlay.timecode ^= true,
                    Some(Hit::FrameNumber) => self.prefs.overlay.frame_number ^= true,
                    None => return Vec::new(),
                }
                self.repaint = true;
            }
            UiEvent::PointerMove { pos, .. } => {
                let hover = self.hit(*pos);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint = true;
                }
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint = true;
                }
            }
            UiEvent::Resize(size) => {
                self.set_bounds(Rect::new(0.0, 0.0, size.width, size.height));
            }
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            _ => {}
        }
        Vec::new()
    }

    // ------------------------------------------------------------------ paint

    /// Paints the view.
    pub fn paint(&mut self, c: &mut dyn Canvas) {
        self.refresh_brush();
        c.push_transform(Affine::scale(self.scale));
        self.paint_logical(c);
        c.pop_transform();
        self.repaint = false;
    }

    /// Builds the image brush of the current picture when it is a new one.
    fn refresh_brush(&mut self) {
        let VideoScreen::Picture(image) = &self.frame.screen else {
            self.brush = None;
            return;
        };
        if self.brush.as_ref().is_some_and(|(s, _)| *s == image.serial) {
            return;
        }
        let data = ImageData {
            data: Blob::new(image.rgba.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: image.width,
            height: image.height,
        };
        let sampler = ImageSampler::new().with_quality(ImageQuality::Medium);
        self.brush = Some((
            image.serial,
            Brush::Image(ImageBrush {
                image: data,
                sampler,
            }),
        ));
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let area = self.area();
        c.fill_with(area, SURROUND);
        c.push_clip(&area.into());
        let film = self.film_rect();
        match &self.frame.screen {
            VideoScreen::Picture(image) => {
                if let Some((_, brush)) = &self.brush {
                    let sx = film.width() / f64::from(image.width.max(1));
                    let sy = film.height() / f64::from(image.height.max(1));
                    c.push_transform(
                        Affine::translate((film.x0, film.y0)) * Affine::scale_non_uniform(sx, sy),
                    );
                    c.fill_with(
                        Rect::new(0.0, 0.0, f64::from(image.width), f64::from(image.height)),
                        brush.clone(),
                    );
                    c.pop_transform();
                }
            }
            VideoScreen::Black => c.fill_with(film, FILM_BLACK),
            VideoScreen::Message(text) => {
                c.fill_with(area, FILM_BLACK);
                let lines: Vec<&str> = text.split('\n').collect();
                let top = area.center().y - 8.0 * lines.len() as f64 + 5.0;
                for (i, line) in lines.iter().enumerate() {
                    c.text(
                        &TextRun::new(
                            *line,
                            pt(area.center().x, top + 17.0 * i as f64),
                            12.0,
                            t.text_muted,
                        )
                        .align(Align::Center),
                    );
                }
            }
            VideoScreen::Progress(fraction, text) => {
                c.fill_with(area, FILM_BLACK);
                let bar = Rect::new(
                    area.center().x - 120.0,
                    area.center().y + 6.0,
                    area.center().x + 120.0,
                    area.center().y + 12.0,
                );
                c.fill_with(RoundedRect::from_rect(bar, 3.0), t.lcd);
                let filled = Rect::new(
                    bar.x0,
                    bar.y0,
                    bar.x0 + bar.width() * f64::from(fraction.clamp(0.0, 1.0)),
                    bar.y1,
                );
                c.fill_with(RoundedRect::from_rect(filled, 3.0), t.highlight);
                c.text(
                    &TextRun::new(
                        text.as_str(),
                        pt(area.center().x, area.center().y - 8.0),
                        12.0,
                        t.text,
                    )
                    .align(Align::Center),
                );
            }
        }
        if matches!(
            self.frame.screen,
            VideoScreen::Picture(_) | VideoScreen::Black
        ) {
            self.paint_overlay(c, film.intersect(area));
        }
        c.pop_clip();
        self.paint_bar(c);
    }

    /// The overlay lines: the timecode on the left, the frame number on the right.
    pub fn overlay_texts(&self) -> (Option<String>, Option<String>) {
        let o = self.prefs.overlay;
        let tc =
            (o.timecode && !self.frame.timecode.is_empty()).then(|| self.frame.timecode.clone());
        let n = o
            .frame_number
            .then_some(self.frame.frame_number)
            .flatten()
            .map(|n| format!("F {n}"));
        (tc, n)
    }

    fn paint_overlay(&self, c: &mut dyn Canvas, within: Rect) {
        let (tc, n) = self.overlay_texts();
        let y = within.y1 - 12.0;
        if let Some(text) = tc {
            self.plate(c, &text, pt(within.x0 + 10.0, y), Align::Start);
        }
        if let Some(text) = n {
            self.plate(c, &text, pt(within.x1 - 10.0, y), Align::End);
        }
    }

    /// `text` on a dark plate whose bottom corner is `at` (left or right edge per `align`).
    fn plate(&self, c: &mut dyn Canvas, text: &str, at: Point, align: Align) {
        let run = TextRun::new(text, pt(at.x, at.y), 15.0, OVERLAY_TEXT)
            .align(align)
            .weight(600);
        let w = c.text_width(&run);
        let (x0, x1) = if align == Align::End {
            (at.x - w, at.x)
        } else {
            (at.x, at.x + w)
        };
        let plate = Rect::new(x0 - 6.0, at.y - 15.0, x1 + 6.0, at.y + 6.0);
        c.fill_with(RoundedRect::from_rect(plate, 4.0), OVERLAY_PLATE);
        c.text(&run);
    }

    fn paint_bar(&self, c: &mut dyn Canvas) {
        let bar = self.bar();
        c.fill_with(bar, self.st.header);
        c.fill_with(
            Rect::new(bar.x0, bar.y0, bar.x1, bar.y0 + 1.0),
            self.st.separator,
        );
        for (s, r) in self.size_rects() {
            let lit = (self.prefs.size == s).then_some(self.st.base.highlight);
            button(
                c,
                &self.st,
                r,
                s.label(),
                lit,
                self.hover == Some(Hit::Size(s)),
            );
        }
        let o = self.prefs.overlay;
        for (hit, text, r) in self.overlay_rects() {
            let on = match hit {
                Hit::Timecode => o.timecode,
                _ => o.frame_number,
            };
            button(
                c,
                &self.st,
                r,
                text,
                on.then_some(self.st.base.highlight),
                self.hover == Some(hit),
            );
        }
    }
}

/// Whether two frames show the same thing (the picture by its serial).
fn same_frame(a: &VideoFrame, b: &VideoFrame) -> bool {
    let screen = match (&a.screen, &b.screen) {
        (VideoScreen::Message(x), VideoScreen::Message(y)) => x == y,
        (VideoScreen::Progress(f, x), VideoScreen::Progress(g, y)) => {
            f.to_bits() == g.to_bits() && x == y
        }
        (VideoScreen::Black, VideoScreen::Black) => true,
        (VideoScreen::Picture(x), VideoScreen::Picture(y)) => x.serial == y.serial,
        _ => false,
    };
    screen && a.film == b.film && a.timecode == b.timecode && a.frame_number == b.frame_number
}
