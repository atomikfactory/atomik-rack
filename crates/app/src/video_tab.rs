//! The Video tab of the settings dialog: where the user's FFmpeg is.
//!
//! Video is decoded by an FFmpeg the user installs (nothing is bundled). The tab holds the
//! **FFmpeg folder** (empty: the system path is searched) with **Choose...** and **Default**, and
//! shows what the last check found: `FFmpeg 7.1 found at ...`, or why video is off and what to
//! do. The folder is a workflow preference ([`WorkflowPrefs::ffmpeg_folder`]) edited in a draft
//! and merged back when the dialog is applied; the status line is the core's
//! ([`crate::AppCore::video_status`]) and describes the folder that is applied, so after a
//! change it is looked for again once Apply is pressed. File > Video > Check FFmpeg looks again
//! on request.

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};
use std::path::PathBuf;

const ROW: f64 = 40.0;
const GAP: f64 = 2.0;
const BUTTON_W: f64 = 84.0;
/// Longest folder text shown, in characters; a longer path keeps its tail.
const PATH_CHARS: usize = 44;
/// The characters of a wrapped line of the status and the notes.
pub(crate) const WRAP: usize = 84;

/// The words the tab says under the folder row.
const NOTES: [&str; 3] = [
    "Video uses the FFmpeg you installed: nothing is bundled and no codec ships with Atomik Rack.",
    "File > Video > Check FFmpeg looks again; Prepare Video makes the small copy of a film that the viewer reads.",
    "Saved in workflow.json next to settings.json when you press Apply.",
];

/// `text` with its head cut when longer than [`PATH_CHARS`] characters.
fn shorten(text: &str) -> String {
    let count = text.chars().count();
    if count <= PATH_CHARS {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - (PATH_CHARS - 1)).collect();
    format!("\u{2026}{tail}")
}

/// `text` broken at spaces into lines of at most `width` characters (a longer word has a line
/// of its own).
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
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

/// The folder row: the folder in use and its two buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    /// The whole row.
    pub rect: Rect,
    /// The folder as shown (shortened from the left), or the note that the system path is used.
    pub path: String,
    /// **Choose...**: asks the platform for a folder.
    pub choose: Rect,
    /// **Default**: goes back to searching the system path.
    pub default: Rect,
}

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoLayout {
    /// The FFmpeg folder row.
    pub folder: FolderRow,
    /// The title of the status and its lines, from the top line down.
    pub status: (Point, Vec<String>),
    /// The notes under the status, line by line.
    pub notes: Vec<(Point, String)>,
}

/// The Video tab state.
#[derive(Debug, Clone)]
pub struct VideoTab {
    folder: Option<PathBuf>,
    status: String,
    hover: Option<Point>,
}

impl VideoTab {
    /// A tab editing `folder`, showing `status`.
    pub fn new(folder: Option<PathBuf>, status: String) -> Self {
        Self {
            folder,
            status,
            hover: None,
        }
    }

    /// The edited folder (`None`: the system path).
    pub fn folder(&self) -> Option<&PathBuf> {
        self.folder.as_ref()
    }

    /// Sets the folder draft.
    pub(crate) fn set_folder(&mut self, folder: Option<PathBuf>) {
        self.folder = folder.filter(|f| !f.as_os_str().is_empty());
    }

    /// Replaces the status line the tab shows.
    pub(crate) fn set_status(&mut self, status: String) {
        self.status = status;
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> VideoLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let folder = FolderRow {
            rect: Rect::new(x0, y, x0 + inner, y + ROW),
            path: self.folder.as_ref().map_or_else(
                || "empty: the system path is searched".to_string(),
                |p| shorten(&p.to_string_lossy()),
            ),
            default: Rect::new(x0 + inner - BUTTON_W, y + 6.0, x0 + inner, y + 34.0),
            choose: Rect::new(
                x0 + inner - 2.0 * BUTTON_W - GAP,
                y + 6.0,
                x0 + inner - BUTTON_W - GAP,
                y + 34.0,
            ),
        };
        y += ROW + 14.0;
        let title = Point::new(x0 + 36.0, y + 12.0);
        let lines = wrap(&self.status, WRAP);
        y += 22.0 + 16.0 * lines.len() as f64 + 14.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(note, WRAP + 8) {
                notes.push((Point::new(x0, y + 12.0), line));
                y += 16.0;
            }
            y += 6.0;
        }
        VideoLayout {
            folder,
            status: (title, lines),
            notes,
        }
    }

    /// The height the panel needs for the status and the notes of this tab.
    pub fn content_height(&self) -> f64 {
        let lines = wrap(&self.status, WRAP).len() as f64;
        let notes: f64 = NOTES
            .iter()
            .map(|n| 16.0 * wrap(n, WRAP + 8).len() as f64 + 6.0)
            .sum();
        ROW + 14.0 + 22.0 + 16.0 * lines + 14.0 + notes
    }

    /// Handles an event inside `panel`.
    pub(crate) fn handle(&mut self, event: &UiEvent, panel: Rect) -> TabOutcome {
        match event {
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => return TabOutcome::Cancel,
                Key::Enter => return TabOutcome::Apply,
                _ => {}
            },
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let layout = self.layout(panel);
                if layout.folder.choose.contains(*pos) {
                    return TabOutcome::PickFfmpegFolder;
                }
                if layout.folder.default.contains(*pos) {
                    self.folder = None;
                }
            }
            _ => {}
        }
        TabOutcome::Open
    }

    /// Paints the tab inside `panel`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        let row = &l.folder;
        canvas.text(&TextRun::new(
            "FFmpeg folder",
            Point::new(row.rect.x0 + 36.0, row.rect.y0 + 18.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            row.path.clone(),
            Point::new(row.rect.x0 + 36.0, row.rect.y0 + 36.0),
            11.0,
            theme.text_muted,
        ));
        for (r, label) in [(row.choose, "Choose..."), (row.default, "Default")] {
            let lit = self.hover.is_some_and(|p| r.contains(p));
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if lit {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 15.5),
                    12.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
        canvas.text(&TextRun::new("FFmpeg status", l.status.0, 13.0, theme.text));
        for (i, line) in l.status.1.iter().enumerate() {
            canvas.text(&TextRun::new(
                line.clone(),
                Point::new(l.status.0.x, l.status.0.y + 20.0 + 16.0 * i as f64),
                12.0,
                theme.text,
            ));
        }
        for (at, line) in &l.notes {
            canvas.text(&TextRun::new(line.clone(), *at, 11.5, theme.text_muted));
        }
    }
}
