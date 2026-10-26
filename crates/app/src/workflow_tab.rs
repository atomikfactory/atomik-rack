//! The Workflow tab of the settings dialog: the workflow
//! preferences that live in `workflow.json`. Each is a labelled switch; nothing is saved until
//! the dialog's **Apply**.

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::keymap_store::WorkflowPrefs;
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};
use std::path::PathBuf;

const ROW: f64 = 48.0;
const GAP: f64 = 8.0;
const BUTTON_W: f64 = 84.0;

/// `text` with its head cut when longer than [`PATH_CHARS`] characters.
fn shorten(text: &str) -> String {
    let count = text.chars().count();
    if count <= PATH_CHARS {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - (PATH_CHARS - 1)).collect();
    format!("…{tail}")
}

/// Which preference a switch edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pref {
    /// Auto-route new devices.
    AutoRoute,
    /// Create keeps the browser open.
    CreateKeepsBrowserOpen,
    /// Follow song while playing.
    FollowSong,
    /// Automation follows the selection.
    FollowSelection,
}

/// One switch.
#[derive(Debug, Clone, PartialEq)]
pub struct Toggle {
    /// The whole clickable line.
    pub rect: Rect,
    /// The preference.
    pub pref: Pref,
    /// Its label.
    pub label: &'static str,
    /// One line saying what it does.
    pub description: &'static str,
    /// The switch is on.
    pub on: bool,
}

/// The library folder row: the folder in use and its two buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryRow {
    /// The whole row.
    pub rect: Rect,
    /// The folder as shown (shortened from the left), or the note that the default is used.
    pub path: String,
    /// **Choose...**: asks the platform for a folder.
    pub choose: Rect,
    /// **Default**: goes back to the default folder.
    pub default: Rect,
}

/// Longest folder text shown, in characters; a longer path keeps its tail.
const PATH_CHARS: usize = 44;

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowLayout {
    /// The switches, top to bottom.
    pub toggles: Vec<Toggle>,
    /// The library folder row under them.
    pub library: LibraryRow,
    /// The note under them.
    pub note: (Point, String),
}

/// The Workflow tab state.
#[derive(Debug, Clone)]
pub struct WorkflowTab {
    prefs: WorkflowPrefs,
    hover: Option<Point>,
}

impl WorkflowTab {
    /// A tab editing `prefs`.
    pub fn new(prefs: WorkflowPrefs) -> Self {
        Self { prefs, hover: None }
    }

    /// The edited preferences.
    pub fn prefs(&self) -> &WorkflowPrefs {
        &self.prefs
    }

    /// Sets the library folder draft (`None`: the default folder).
    pub(crate) fn set_library_folder(&mut self, folder: Option<PathBuf>) {
        self.prefs.library_folder = folder;
    }

    fn get(&self, pref: Pref) -> bool {
        match pref {
            Pref::AutoRoute => self.prefs.auto_route,
            Pref::CreateKeepsBrowserOpen => self.prefs.create_keeps_browser_open,
            Pref::FollowSong => self.prefs.follow_song,
            Pref::FollowSelection => self.prefs.follow_selection,
        }
    }

    fn flip(&mut self, pref: Pref) {
        match pref {
            Pref::AutoRoute => self.prefs.auto_route = !self.prefs.auto_route,
            Pref::CreateKeepsBrowserOpen => {
                self.prefs.create_keeps_browser_open = !self.prefs.create_keeps_browser_open;
            }
            Pref::FollowSong => self.prefs.follow_song = !self.prefs.follow_song,
            Pref::FollowSelection => self.prefs.follow_selection = !self.prefs.follow_selection,
        }
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> WorkflowLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let mut toggles = Vec::new();
        for (pref, label, description) in [
            (
                Pref::AutoRoute,
                "Auto-route new devices",
                "A new instrument gets its mixer channel and song track, armed. Shift at create inverts it.",
            ),
            (
                Pref::CreateKeepsBrowserOpen,
                "Create keeps the browser open",
                "After creating a device from the browser, the browser stays open.",
            ),
            (
                Pref::FollowSong,
                "Follow song while playing",
                "The arrangement and piano roll scroll to keep the playhead visible while playing.",
            ),
            (
                Pref::FollowSelection,
                "Automation follows the selection",
                "Touching a control shows its lane in the arrangement (an empty lane is added; no data is written).",
            ),
        ] {
            toggles.push(Toggle {
                rect: Rect::new(x0, y, x0 + inner, y + ROW),
                pref,
                label,
                description,
                on: self.get(pref),
            });
            y += ROW + GAP;
        }
        let library = LibraryRow {
            rect: Rect::new(x0, y, x0 + inner, y + ROW),
            path: self.prefs.library_folder.as_ref().map_or_else(
                || "the default folder (Documents/Atomik Rack/Library)".to_string(),
                |p| shorten(&p.to_string_lossy()),
            ),
            default: Rect::new(x0 + inner - BUTTON_W, y + 10.0, x0 + inner, y + 38.0),
            choose: Rect::new(
                x0 + inner - 2.0 * BUTTON_W - GAP,
                y + 10.0,
                x0 + inner - BUTTON_W - GAP,
                y + 38.0,
            ),
        };
        y += ROW + GAP;
        WorkflowLayout {
            toggles,
            library,
            note: (
                Point::new(x0, y + 12.0),
                "Saved in workflow.json next to settings.json when you press Apply.".to_string(),
            ),
        }
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
                if layout.library.choose.contains(*pos) {
                    return TabOutcome::PickLibraryFolder;
                }
                if layout.library.default.contains(*pos) {
                    self.prefs.library_folder = None;
                }
                let hit = layout.toggles.into_iter().find(|t| t.rect.contains(*pos));
                if let Some(t) = hit {
                    self.flip(t.pref);
                }
            }
            _ => {}
        }
        TabOutcome::Open
    }

    /// Paints the tab inside `panel`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        for t in &l.toggles {
            if self.hover.is_some_and(|p| t.rect.contains(p)) {
                canvas.fill_with(RoundedRect::from_rect(t.rect, 6.0), theme.browser_row);
            }
            let boxr = Rect::new(
                t.rect.x0 + 8.0,
                t.rect.y0 + 8.0,
                t.rect.x0 + 24.0,
                t.rect.y0 + 24.0,
            );
            let shape = RoundedRect::from_rect(boxr, 3.0);
            if t.on {
                canvas.fill_with(shape, theme.focus);
            }
            canvas.stroke_with(shape, 1.2, theme.text_muted);
            canvas.text(&TextRun::new(
                t.label,
                Point::new(t.rect.x0 + 36.0, t.rect.y0 + 20.0),
                13.0,
                theme.text,
            ));
            canvas.text(&TextRun::new(
                t.description,
                Point::new(t.rect.x0 + 36.0, t.rect.y0 + 38.0),
                11.0,
                theme.text_muted,
            ));
        }
        let lib = &l.library;
        canvas.text(&TextRun::new(
            "Library folder",
            Point::new(lib.rect.x0 + 36.0, lib.rect.y0 + 20.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            lib.path.clone(),
            Point::new(lib.rect.x0 + 36.0, lib.rect.y0 + 38.0),
            11.0,
            theme.text_muted,
        ));
        for (r, label) in [(lib.choose, "Choose..."), (lib.default, "Default")] {
            let shape = RoundedRect::from_rect(r, 6.0);
            let lit = self.hover.is_some_and(|p| r.contains(p));
            canvas.fill_with(
                shape,
                if lit {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 18.5),
                    12.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
        canvas.text(&TextRun::new(
            l.note.1.clone(),
            l.note.0,
            11.5,
            theme.text_muted,
        ));
    }
}
