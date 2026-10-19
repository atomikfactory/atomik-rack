//! The Workflow tab of the settings dialog: the workflow
//! preferences that live in `workflow.json`. Each is a labelled switch; nothing is saved until
//! the dialog's **Apply**.

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::keymap_store::WorkflowPrefs;
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

const ROW: f64 = 48.0;
const GAP: f64 = 8.0;

/// Which preference a switch edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pref {
    /// Auto-route new devices.
    AutoRoute,
    /// Create keeps the browser open.
    CreateKeepsBrowserOpen,
    /// Follow song while playing.
    FollowSong,
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

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowLayout {
    /// The switches, top to bottom.
    pub toggles: Vec<Toggle>,
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

    fn get(&self, pref: Pref) -> bool {
        match pref {
            Pref::AutoRoute => self.prefs.auto_route,
            Pref::CreateKeepsBrowserOpen => self.prefs.create_keeps_browser_open,
            Pref::FollowSong => self.prefs.follow_song,
        }
    }

    fn flip(&mut self, pref: Pref) {
        match pref {
            Pref::AutoRoute => self.prefs.auto_route = !self.prefs.auto_route,
            Pref::CreateKeepsBrowserOpen => {
                self.prefs.create_keeps_browser_open = !self.prefs.create_keeps_browser_open;
            }
            Pref::FollowSong => self.prefs.follow_song = !self.prefs.follow_song,
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
        WorkflowLayout {
            toggles,
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
                let hit = self
                    .layout(panel)
                    .toggles
                    .into_iter()
                    .find(|t| t.rect.contains(*pos));
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
        canvas.text(&TextRun::new(
            l.note.1.clone(),
            l.note.0,
            11.5,
            theme.text_muted,
        ));
    }
}
