//! Help > About: a small box with the name, the version, the licence and where to find the
//! source and the notices. It names the project and its public repository only.

use atomik_ui::kurbo::{Affine, Point, Rect, RoundedRect, Size};
use atomik_ui::peniko::{
    Blob, Brush, Color, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality,
};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};
use std::sync::{Arc, OnceLock};

/// The project's public repository.
pub const REPOSITORY_URL: &str = "https://github.com/atomikfactory/atomik-rack";
/// The project's issue forms.
pub const ISSUES_URL: &str = "https://github.com/atomikfactory/atomik-rack/issues/new/choose";
/// The licence of the program, as an SPDX expression.
pub const LICENCE: &str = "GPL-3.0-or-later";
/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The logo (512 pixels, transparent background), embedded in the program.
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo/monogram.png");
/// The logo's side in logical units.
const LOGO: f64 = 128.0;
/// The room the logo takes at the top of the box: its side plus the gaps around it.
const LOGO_BLOCK: f64 = 136.0;
/// The side of the decoded logo in pixels (half of the file, so a 1x screen shows it at a
/// clean 2:1 and a 2x screen at 1:1).
const LOGO_PIXELS: u32 = 256;

const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 330.0 + LOGO_BLOCK;
const PAD: f64 = 24.0;
const BUTTON: (f64, f64) = (96.0, 28.0);

/// What the box asks of the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AboutOutcome {
    /// The box stays open.
    Open,
    /// The box is closed.
    Close,
    /// Open this address in the browser (the box stays open).
    OpenUrl(&'static str),
}

/// The About box.
#[derive(Debug, Clone)]
pub struct AboutBox {
    version: String,
    hover: Option<Point>,
}

impl AboutBox {
    /// A box for `version`.
    pub fn new(version: &str) -> Self {
        Self {
            version: version.to_string(),
            hover: None,
        }
    }

    /// The text lines of the box, top to bottom (the title is the first).
    pub fn lines(&self) -> Vec<String> {
        vec![
            "Atomik Rack".to_string(),
            format!("Version {}", self.version),
            "A free, open-source, rack-based digital audio workstation.".to_string(),
            String::new(),
            format!("Licence: GNU General Public License, version 3 or later ({LICENCE})."),
            "This program comes with ABSOLUTELY NO WARRANTY. It is free software, and you"
                .to_string(),
            "are welcome to redistribute it under the conditions of the licence; the full"
                .to_string(),
            "text is the LICENSE file that comes with the program.".to_string(),
            String::new(),
            "Third-party notices: the licences of the Rust libraries this build uses are"
                .to_string(),
            "collected for each release with cargo about and attached to the release page."
                .to_string(),
            String::new(),
            REPOSITORY_URL.to_string(),
        ]
    }

    /// The box in a window of `view` (logical units).
    pub fn rect(&self, view: Size) -> Rect {
        let w = (view.width - 32.0).clamp(240.0, WIDTH);
        let h = (view.height - 32.0).clamp(200.0, HEIGHT);
        let x0 = ((view.width - w) / 2.0).max(0.0);
        let y0 = ((view.height - h) / 2.0).max(0.0);
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// The Close button.
    pub fn close_button(&self, view: Size) -> Rect {
        let r = self.rect(view);
        Rect::new(
            r.x1 - PAD - BUTTON.0,
            r.y1 - PAD - BUTTON.1 + 8.0,
            r.x1 - PAD,
            r.y1 - PAD + 8.0,
        )
    }

    /// The line that shows the repository address, where a click opens it.
    pub fn link(&self, view: Size) -> Rect {
        let r = self.rect(view);
        let y = r.y0 + 40.0 + LOGO_BLOCK + 12.0 * LINE;
        Rect::new(r.x0 + PAD, y, r.x0 + PAD + 330.0, y + LINE)
    }

    /// Handles an event of the window of size `view`.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> AboutOutcome {
        match event {
            UiEvent::Key {
                key: Key::Escape | Key::Enter,
                pressed: true,
                ..
            } => return AboutOutcome::Close,
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if self.close_button(view).contains(*pos) || !self.rect(view).contains(*pos) {
                    return AboutOutcome::Close;
                }
                if self.link(view).contains(*pos) {
                    return AboutOutcome::OpenUrl(REPOSITORY_URL);
                }
            }
            _ => {}
        }
        AboutOutcome::Open
    }

    /// Paints the dimmed backdrop and the box (logical units).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        canvas.fill_with(
            Rect::from_origin_size(Point::ZERO, view),
            Color::from_rgba8(0, 0, 0, 140),
        );
        let r = self.rect(view);
        let panel = RoundedRect::from_rect(r, 10.0);
        canvas.fill_with(panel, theme.browser_bg);
        canvas.stroke_with(panel, 1.0, theme.section_line);
        if let Some(brush) = logo_brush() {
            let x = r.center().x - LOGO / 2.0;
            let scale = LOGO / f64::from(LOGO_PIXELS);
            canvas.push_transform(Affine::translate((x, r.y0 + 16.0)) * Affine::scale(scale));
            let side = f64::from(LOGO_PIXELS);
            canvas.fill_with(Rect::new(0.0, 0.0, side, side), brush.clone());
            canvas.pop_transform();
        }
        for (i, line) in self.lines().iter().enumerate() {
            let y = r.y0 + 40.0 + LOGO_BLOCK + LINE * i as f64;
            if i == 0 {
                canvas.text(
                    &TextRun::new(line.clone(), Point::new(r.x0 + PAD, y), 22.0, theme.text)
                        .weight(700),
                );
            } else if line == REPOSITORY_URL {
                let lit = self.hover.is_some_and(|p| self.link(view).contains(p));
                canvas.text(&TextRun::new(
                    line.clone(),
                    Point::new(r.x0 + PAD, y + 12.0),
                    12.5,
                    if lit { theme.highlight } else { theme.focus },
                ));
            } else {
                canvas.text(&TextRun::new(
                    line.clone(),
                    Point::new(r.x0 + PAD, y + 12.0),
                    12.5,
                    if i == 1 { theme.text } else { theme.text_muted },
                ));
            }
        }
        let b = self.close_button(view);
        let lit = self.hover.is_some_and(|p| b.contains(p));
        canvas.fill_with(
            RoundedRect::from_rect(b, 6.0),
            if lit {
                theme.selection
            } else {
                theme.browser_row
            },
        );
        canvas.text(
            &TextRun::new(
                "Close",
                Point::new(b.center().x, b.y0 + 18.5),
                12.5,
                theme.text,
            )
            .align(Align::Center),
        );
    }
}

/// The height of a text line of the box.
const LINE: f64 = 18.0;

/// The logo as an image brush, decoded and reduced once on first use (never on the audio path).
/// `None` if the embedded file cannot be decoded; the box then shows no logo.
fn logo_brush() -> Option<&'static Brush> {
    static LOGO_BRUSH: OnceLock<Option<Brush>> = OnceLock::new();
    LOGO_BRUSH
        .get_or_init(|| {
            let (side, rgba) = decode_logo(LOGO_PNG, LOGO_PIXELS)?;
            let image = ImageData {
                data: Blob::new(Arc::new(rgba)),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::AlphaPremultiplied,
                width: side,
                height: side,
            };
            let sampler = atomik_ui::peniko::ImageSampler::new().with_quality(ImageQuality::High);
            Some(Brush::Image(ImageBrush { image, sampler }))
        })
        .as_ref()
}

/// Decodes a square 8-bit RGBA PNG and reduces it by whole 2x2 box averages until it is no
/// larger than `max_side`. The result is premultiplied RGBA, so no colour bleeds out of the
/// transparent pixels. `None` for anything that is not a square 8-bit RGBA PNG.
fn decode_logo(png_bytes: &[u8], max_side: u32) -> Option<(u32, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
        || info.width != info.height
        || info.width == 0
    {
        return None;
    }
    buf.truncate(info.buffer_size());
    let mut side = info.width as usize;
    // Premultiply in place.
    for px in buf.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
    while side > max_side as usize && side.is_multiple_of(2) {
        let half = side / 2;
        let mut out = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let at = |dx: usize, dy: usize| {
                        u32::from(buf[((2 * y + dy) * side + 2 * x + dx) * 4 + c])
                    };
                    out[(y * half + x) * 4 + c] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        buf = out;
        side = half;
    }
    Some((u32::try_from(side).ok()?, buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::{DisplayList, Modifiers};

    const VIEW: Size = Size::new(1000.0, 800.0);

    fn down(p: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        }
    }

    #[test]
    fn the_text_has_the_version_the_licence_and_the_public_repository_only() {
        let text = AboutBox::new("1.2.3").lines().join("\n");
        assert!(text.contains("Atomik Rack"));
        assert!(text.contains("Version 1.2.3"));
        assert!(text.contains("GPL-3.0-or-later"));
        assert!(text.contains("ABSOLUTELY NO WARRANTY"));
        assert!(
            text.contains("cargo about"),
            "where the third-party notices come from"
        );
        assert!(text.contains(REPOSITORY_URL));
        // No person's name or address anywhere: one address, and it is the repository's.
        assert!(!text.contains('@'), "{text}");
        assert_eq!(text.matches("https://").count(), 1, "{text}");
        assert_eq!(LICENCE, env!("CARGO_PKG_LICENSE"));
    }

    #[test]
    fn the_version_of_the_build_is_the_cargo_one() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(
            AboutBox::new(VERSION)
                .lines()
                .iter()
                .any(|l| l.contains(VERSION))
        );
    }

    #[test]
    fn the_box_sits_inside_the_window_even_a_small_one() {
        for size in [VIEW, Size::new(400.0, 300.0), Size::new(260.0, 220.0)] {
            let b = AboutBox::new("1");
            let r = b.rect(size);
            assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= size.width && r.y1 <= size.height);
            assert!(r.contains(b.close_button(size).center()));
        }
    }

    #[test]
    fn escape_enter_the_button_and_a_click_outside_close_it_and_the_link_opens() {
        let mut b = AboutBox::new("1");
        for key in [Key::Escape, Key::Enter] {
            assert_eq!(
                b.handle(
                    &UiEvent::Key {
                        key,
                        modifiers: Modifiers::NONE,
                        pressed: true
                    },
                    VIEW
                ),
                AboutOutcome::Close
            );
        }
        assert_eq!(
            b.handle(&down(b.close_button(VIEW).center()), VIEW),
            AboutOutcome::Close
        );
        assert_eq!(
            b.handle(&down(Point::new(1.0, 1.0)), VIEW),
            AboutOutcome::Close
        );
        assert_eq!(
            b.handle(&down(b.link(VIEW).center()), VIEW),
            AboutOutcome::OpenUrl(REPOSITORY_URL)
        );
        let inside = b.rect(VIEW).center();
        assert_eq!(b.handle(&down(inside), VIEW), AboutOutcome::Open);
    }

    #[test]
    fn the_logo_decodes_reduced_and_premultiplied_and_the_box_draws_it_first() {
        let (side, rgba) = decode_logo(LOGO_PNG, LOGO_PIXELS).expect("the embedded logo decodes");
        assert_eq!(side, LOGO_PIXELS);
        assert_eq!(rgba.len(), (side * side * 4) as usize);
        // Transparent corners, visible artwork, and no colour outside the alpha (premultiplied).
        assert_eq!(&rgba[..4], &[0, 0, 0, 0]);
        assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 200));
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[..3].iter().all(|&c| c <= p[3]))
        );
        assert_eq!(decode_logo(b"not a png", LOGO_PIXELS), None);

        let mut list = DisplayList::new();
        AboutBox::new("1").paint(&mut list, &Theme::dark(), VIEW);
        assert!(list.to_json().contains("\"image\""), "the logo is painted");
        // The logo sits inside the box, centred, above the title.
        let b = AboutBox::new("1");
        let r = b.rect(VIEW);
        assert!(r.height() >= LOGO_BLOCK + 40.0 + 13.0 * LINE);
    }

    fn golden(name: &str, theme: &Theme) {
        let mut list = DisplayList::new();
        AboutBox::new("0.0.0-test").paint(&mut list, theme, VIEW);
        let actual = list.to_json();
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(format!("{name}_1x.json"));
        if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
            std::fs::write(&path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| {
                panic!(
                    "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                    path.display()
                )
            })
            .replace("\r\n", "\n");
        assert!(
            expected == actual,
            "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
            path.display()
        );
    }

    #[test]
    fn golden_about_box_dark() {
        golden("about_box", &Theme::dark());
    }

    #[test]
    fn golden_about_box_light() {
        golden("about_box_light", &Theme::light());
    }
}
