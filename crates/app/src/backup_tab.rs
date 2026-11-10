//! The Backup tab of the settings dialog: a second folder every saved project is copied into.
//!
//! The tab holds the **Backup folder** (empty: no backup copies) with **Choose...** and **None**,
//! **Keep** (how many copies of a project stay, 1 to 50), and **Include audio** (the audio files
//! the project uses are copied too, once each). The values are workflow preferences
//! ([`WorkflowPrefs::backup_folder`], `backup_keep`, `backup_audio`) edited in a draft and merged
//! back when the dialog is applied. What the copies are, and how they are pruned, is in
//! [`crate::AppCore::set_backup`].

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::keymap_store::MAX_BACKUP_KEEP;
use crate::video_tab::wrap;
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};
use std::path::PathBuf;

const ROW: f64 = 40.0;
const GAP: f64 = 2.0;
const BUTTON_W: f64 = 84.0;
const STEP_W: f64 = 28.0;
/// Longest folder text shown, in characters; a longer path keeps its tail.
const PATH_CHARS: usize = 44;
/// How much Shift moves the count.
const BIG_STEP: u32 = 10;

/// The words the tab says under the rows.
const NOTES: [&str; 3] = [
    "Every save also keeps a copy of the project here, for example on another drive.",
    "File > Restore from Backup lists the copies of the open project and opens one as a new song.",
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

/// The folder row: the folder in use and its two buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupFolderRow {
    /// The whole row.
    pub rect: Rect,
    /// The folder as shown (shortened from the left), or the note that there is none.
    pub path: String,
    /// **Choose...**: asks the platform for a folder.
    pub choose: Rect,
    /// **None**: no backup copies.
    pub none: Rect,
}

/// The Keep row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeepRow {
    /// The whole row.
    pub rect: Rect,
    /// The number of copies as shown.
    pub value: String,
    /// The button that takes one off.
    pub less: Rect,
    /// The box the number is in.
    pub field: Rect,
    /// The button that adds one.
    pub more: Rect,
}

/// The Include audio row.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRow {
    /// The whole row.
    pub rect: Rect,
    /// The check box.
    pub check: Rect,
    /// The box is ticked.
    pub on: bool,
}

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupLayout {
    /// The backup folder row.
    pub folder: BackupFolderRow,
    /// The Keep row.
    pub keep: KeepRow,
    /// The Include audio row.
    pub audio: AudioRow,
    /// The notes under the rows, line by line.
    pub notes: Vec<(Point, String)>,
}

/// The Backup tab state.
#[derive(Debug, Clone)]
pub struct BackupTab {
    folder: Option<PathBuf>,
    keep: u32,
    audio: bool,
    hover: Option<Point>,
}

impl BackupTab {
    /// A tab editing the folder, the count and the audio switch.
    pub fn new(folder: Option<PathBuf>, keep: u32, audio: bool) -> Self {
        Self {
            folder: folder.filter(|f| !f.as_os_str().is_empty()),
            keep: keep.clamp(1, MAX_BACKUP_KEEP),
            audio,
            hover: None,
        }
    }

    /// The edited folder (`None`: no backups).
    pub fn folder(&self) -> Option<&PathBuf> {
        self.folder.as_ref()
    }

    /// The edited number of copies.
    pub fn keep(&self) -> u32 {
        self.keep
    }

    /// The edited audio switch.
    pub fn audio(&self) -> bool {
        self.audio
    }

    /// Sets the folder draft.
    pub(crate) fn set_folder(&mut self, folder: Option<PathBuf>) {
        self.folder = folder.filter(|f| !f.as_os_str().is_empty());
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> BackupLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let folder = BackupFolderRow {
            rect: Rect::new(x0, y, x0 + inner, y + ROW),
            path: self.folder.as_ref().map_or_else(
                || "None: no backup copies are made".to_string(),
                |p| shorten(&p.to_string_lossy()),
            ),
            none: Rect::new(x0 + inner - BUTTON_W, y + 6.0, x0 + inner, y + 34.0),
            choose: Rect::new(
                x0 + inner - 2.0 * BUTTON_W - GAP,
                y + 6.0,
                x0 + inner - BUTTON_W - GAP,
                y + 34.0,
            ),
        };
        y += ROW + GAP;
        let rect = Rect::new(x0, y, x0 + inner, y + ROW);
        let more = Rect::new(rect.x1 - STEP_W, y + 6.0, rect.x1, y + 34.0);
        let field = Rect::new(more.x0 - 48.0 - GAP, y + 6.0, more.x0 - GAP, y + 34.0);
        let less = Rect::new(field.x0 - STEP_W - GAP, y + 6.0, field.x0 - GAP, y + 34.0);
        let keep = KeepRow {
            rect,
            value: self.keep.to_string(),
            less,
            field,
            more,
        };
        y += ROW + GAP;
        let rect = Rect::new(x0, y, x0 + inner, y + ROW);
        let audio = AudioRow {
            rect,
            check: Rect::new(rect.x0 + 8.0, rect.y0 + 6.0, rect.x0 + 24.0, rect.y0 + 22.0),
            on: self.audio,
        };
        y += ROW + 14.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(note, crate::video_tab::WRAP + 8) {
                notes.push((Point::new(x0, y + 12.0), line));
                y += 16.0;
            }
            y += 6.0;
        }
        BackupLayout {
            folder,
            keep,
            audio,
            notes,
        }
    }

    /// The height the panel needs for the rows and the notes of this tab.
    pub fn content_height(&self) -> f64 {
        let notes: f64 = NOTES
            .iter()
            .map(|n| 16.0 * wrap(n, crate::video_tab::WRAP + 8).len() as f64 + 6.0)
            .sum();
        3.0 * ROW + 2.0 * GAP + 14.0 + notes
    }

    /// Moves the count by `delta`, kept between 1 and the most.
    fn move_keep(&mut self, delta: i64) {
        let next = i64::from(self.keep) + delta;
        self.keep = next.clamp(1, i64::from(MAX_BACKUP_KEEP)) as u32;
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
                modifiers,
                ..
            } => {
                let l = self.layout(panel);
                let step = i64::from(if modifiers.shift { BIG_STEP } else { 1 });
                if l.folder.choose.contains(*pos) {
                    return TabOutcome::PickBackupFolder;
                }
                if l.folder.none.contains(*pos) {
                    self.folder = None;
                } else if l.keep.less.contains(*pos) {
                    self.move_keep(-step);
                } else if l.keep.more.contains(*pos) {
                    self.move_keep(step);
                } else if l.audio.rect.contains(*pos) {
                    self.audio = !self.audio;
                }
            }
            _ => {}
        }
        TabOutcome::Open
    }

    /// Paints the tab inside `panel`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        let lit = |r: Rect| self.hover.is_some_and(|p| r.contains(p));
        let button = |canvas: &mut dyn Canvas, r: Rect, label: &str| {
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if lit(r) {
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
        };
        let row = &l.folder;
        canvas.text(&TextRun::new(
            "Backup folder",
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
        button(canvas, row.choose, "Choose...");
        button(canvas, row.none, "None");

        let keep = &l.keep;
        canvas.text(&TextRun::new(
            "Keep",
            Point::new(keep.rect.x0 + 36.0, keep.rect.y0 + 18.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            "Copies of each project that stay; Shift moves by ten",
            Point::new(keep.rect.x0 + 36.0, keep.rect.y0 + 36.0),
            11.0,
            theme.text_muted,
        ));
        button(canvas, keep.less, "-");
        button(canvas, keep.more, "+");
        canvas.fill_with(RoundedRect::from_rect(keep.field, 6.0), theme.browser_row);
        canvas.text(
            &TextRun::new(
                keep.value.clone(),
                Point::new(keep.field.center().x, keep.field.y0 + 16.5),
                12.0,
                theme.text,
            )
            .align(Align::Center),
        );

        let audio = &l.audio;
        if lit(audio.rect) {
            canvas.fill_with(RoundedRect::from_rect(audio.rect, 6.0), theme.browser_row);
        }
        let shape = RoundedRect::from_rect(audio.check, 3.0);
        if audio.on {
            canvas.fill_with(shape, theme.focus);
        }
        canvas.stroke_with(shape, 1.2, theme.text_muted);
        canvas.text(&TextRun::new(
            "Include audio",
            Point::new(audio.rect.x0 + 36.0, audio.rect.y0 + 18.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::new(
            "Copy the audio files the project uses too, once each",
            Point::new(audio.rect.x0 + 36.0, audio.rect.y0 + 36.0),
            11.0,
            theme.text_muted,
        ));
        for (at, line) in &l.notes {
            canvas.text(&TextRun::new(line.clone(), *at, 11.5, theme.text_muted));
        }
    }
}
