//! Help > About: a small box with the name, the version, the licence and where to find the
//! source and the notices. It names the project and its public repository only.

use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

/// The project's public repository.
pub const REPOSITORY_URL: &str = "https://github.com/atomikfactory/atomik-rack";
/// The project's issue forms.
pub const ISSUES_URL: &str = "https://github.com/atomikfactory/atomik-rack/issues/new/choose";
/// The licence of the program, as an SPDX expression.
pub const LICENCE: &str = "GPL-3.0-or-later";
/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 330.0;
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
        let y = r.y0 + 40.0 + 12.0 * LINE;
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
        for (i, line) in self.lines().iter().enumerate() {
            let y = r.y0 + 40.0 + LINE * i as f64;
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
