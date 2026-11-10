//! The Themes tab of the settings dialog: the list of themes, and what to do with them.
//!
//! ```text
//! Dark          [swatches]                                  In use
//! Light         [swatches]                                  Built-in
//! Synthwave     [swatches]                                  Built-in
//! Night drive   [swatches]
//!
//! [Duplicate] [Rename] [Delete] [Edit...]
//! [Import...] [Export...] [Open Themes Folder]
//! ```
//!
//! A click on a theme uses it at once, in the whole interface. The buttons work on the theme
//! last clicked: **Duplicate** makes a user theme of any theme, **Rename**, **Delete** (after a
//! question) and **Edit...** (the theme editor) only work on a user theme, **Import...** and
//! **Export...** go through the file dialogs, **Open Themes Folder** shows the folder in the file
//! manager. The tab owns no files: it asks the shell with a [`ThemeAction`] and the shell gives it
//! the new list ([`ThemesTab::set_rows`]) and a message ([`ThemesTab::set_message`]).

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::themes::ThemeEntry;
use crate::video_tab::wrap;
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::paint::fit_label_raw;
use atomik_ui::peniko::Color;
use atomik_ui::theme_doc::MAX_NAME_CHARS;
use atomik_ui::{
    Align, Canvas, CanvasExt, FieldColors, Key, PointerButton, TextField, TextRun, Theme, UiEvent,
};

const ROW: f64 = 34.0;
const BUTTON_H: f64 = 28.0;
const GAP: f64 = 8.0;
const SWATCHES: usize = 6;
const SWATCH: f64 = 14.0;

/// The words the tab says under the buttons.
const NOTES: [&str; 2] = [
    "A click on a theme uses it at once. A built-in theme cannot be changed: duplicate it first.",
    "Your themes are files in the themes folder next to settings.json; Edit... changes one.",
];

/// What the tab asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeAction {
    /// Use this theme (by id) now.
    Use(String),
    /// Make a user theme as a copy of this one.
    Duplicate(String),
    /// Rename this user theme.
    Rename(String, String),
    /// Delete this user theme (the question was answered).
    Delete(String),
    /// Edit this user theme.
    Edit(String),
    /// Ask for a theme file to import.
    Import,
    /// Ask for a file to export this theme to.
    Export(String),
    /// Show the themes folder.
    OpenFolder,
}

/// One theme of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeRow {
    /// The id (see [`crate::themes`]).
    pub id: String,
    /// The name shown.
    pub name: String,
    /// Built in: read-only, and its name is translated.
    pub builtin: bool,
    /// A few of its colours, drawn as the preview.
    pub swatches: [Color; SWATCHES],
}

impl ThemeRow {
    /// The row of a library entry.
    pub fn of(entry: &ThemeEntry) -> Self {
        let t: Theme = entry.theme();
        Self {
            id: entry.id.clone(),
            name: entry.name().to_string(),
            builtin: entry.builtin,
            swatches: [
                t.background,
                t.panel_top,
                t.highlight,
                t.focus,
                t.warning,
                t.text,
            ],
        }
    }
}

/// The buttons, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Duplicate.
    Duplicate,
    /// Rename.
    Rename,
    /// Delete.
    Delete,
    /// Edit...
    Edit,
    /// Import...
    Import,
    /// Export...
    Export,
    /// Open Themes Folder.
    OpenFolder,
}

impl Button {
    /// The label.
    pub fn label(self) -> &'static str {
        match self {
            Button::Duplicate => "Duplicate",
            Button::Rename => "Rename",
            Button::Delete => "Delete",
            Button::Edit => "Edit...",
            Button::Import => "Import...",
            Button::Export => "Export...",
            Button::OpenFolder => "Open Themes Folder",
        }
    }

    /// The button works on a user theme only.
    fn user_only(self) -> bool {
        matches!(self, Button::Rename | Button::Delete | Button::Edit)
    }
}

/// What the buttons are doing.
#[derive(Debug, Clone)]
enum Mode {
    Idle,
    /// The name of the selected theme is being typed.
    Rename(Box<TextField>),
    /// Waiting for the answer to "Delete this theme?".
    ConfirmDelete,
}

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemesLayout {
    /// The rows of the list.
    pub rows: Vec<Rect>,
    /// The buttons; while a deletion is asked about, only [`Button::Delete`] (its Yes, labelled
    /// Delete) and [`Button::Duplicate`] (its No, labelled Keep) are laid out, in the first row.
    pub buttons: Vec<(Button, Rect)>,
    /// The question under the list, while a deletion is asked about.
    pub question: Option<(Point, String)>,
    /// The notes under the buttons, line by line.
    pub notes: Vec<(Point, String)>,
    /// The message of the last action (an error), line by line.
    pub message: Vec<(Point, String)>,
}

/// The Themes tab state.
#[derive(Debug, Clone)]
pub struct ThemesTab {
    rows: Vec<ThemeRow>,
    active: String,
    selected: usize,
    mode: Mode,
    message: String,
    hover: Option<Point>,
}

impl ThemesTab {
    /// A tab listing `rows`, with the theme `active` in use.
    pub fn new(rows: Vec<ThemeRow>, active: String) -> Self {
        let selected = rows.iter().position(|r| r.id == active).unwrap_or(0);
        Self {
            rows,
            active,
            selected,
            mode: Mode::Idle,
            message: String::new(),
            hover: None,
        }
    }

    /// Replaces the list (after an action); the selection follows `select` when it is listed.
    pub fn set_rows(&mut self, rows: Vec<ThemeRow>, active: String, select: Option<&str>) {
        let keep = select
            .map(str::to_owned)
            .or_else(|| self.rows.get(self.selected).map(|r| r.id.clone()));
        self.rows = rows;
        self.active = active;
        self.selected = keep
            .and_then(|id| self.rows.iter().position(|r| r.id == id))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.mode = Mode::Idle;
    }

    /// Says what the theme in use is (a theme chosen elsewhere).
    pub fn set_active(&mut self, active: String) {
        self.active = active;
    }

    /// The message under the buttons (empty: none).
    pub fn set_message(&mut self, message: String) {
        self.message = message;
    }

    /// The id of the theme in use.
    pub fn active(&self) -> &str {
        &self.active
    }

    /// The list.
    pub fn rows(&self) -> &[ThemeRow] {
        &self.rows
    }

    /// The theme the buttons work on.
    pub fn selected(&self) -> Option<&ThemeRow> {
        self.rows.get(self.selected)
    }

    /// The name is being typed.
    pub fn is_renaming(&self) -> bool {
        matches!(self.mode, Mode::Rename(_))
    }

    /// A deletion is being asked about.
    pub fn is_confirming(&self) -> bool {
        matches!(self.mode, Mode::ConfirmDelete)
    }

    /// The text typed so far while renaming.
    pub fn rename_text(&self) -> Option<&str> {
        match &self.mode {
            Mode::Rename(f) => Some(f.text()),
            _ => None,
        }
    }

    /// The height the panel needs.
    pub fn content_height(&self) -> f64 {
        let notes: f64 = NOTES
            .iter()
            .map(|n| {
                16.0 * wrap(atomik_ui::i18n::t(n), crate::video_tab::WRAP + 8).len() as f64 + 4.0
            })
            .sum();
        self.rows.len() as f64 * ROW + 2.0 * (BUTTON_H + GAP) + 16.0 + notes + 40.0
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> ThemesLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let rows: Vec<Rect> = (0..self.rows.len())
            .map(|i| {
                let top = y + i as f64 * ROW;
                Rect::new(x0, top, x0 + inner, top + ROW)
            })
            .collect();
        y += self.rows.len() as f64 * ROW + 14.0;
        let mut buttons = Vec::new();
        let mut question = None;
        let place = |buttons: &mut Vec<(Button, Rect)>, list: &[(Button, f64)], y: f64| {
            let mut x = x0;
            for (b, w) in list {
                buttons.push((*b, Rect::new(x, y, x + w, y + BUTTON_H)));
                x += w + GAP;
            }
        };
        if self.is_confirming() {
            let name = self
                .selected()
                .map(|r| fit_label_raw(&r.name, inner - 220.0, 12.0))
                .unwrap_or_default();
            question = Some((Point::new(x0, y + 18.0), name));
            // Delete (yes) and Keep (no) side by side, then the rest hidden.
            place(
                &mut buttons,
                &[(Button::Delete, 110.0), (Button::Duplicate, 110.0)],
                y + 28.0,
            );
            y += 2.0 * (BUTTON_H + GAP) + 6.0;
        } else {
            place(
                &mut buttons,
                &[
                    (Button::Duplicate, 130.0),
                    (Button::Rename, 130.0),
                    (Button::Delete, 130.0),
                    (Button::Edit, 130.0),
                ],
                y,
            );
            y += BUTTON_H + GAP;
            place(
                &mut buttons,
                &[
                    (Button::Import, 130.0),
                    (Button::Export, 130.0),
                    (Button::OpenFolder, 190.0),
                ],
                y,
            );
            y += BUTTON_H + GAP;
        }
        y += 8.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(atomik_ui::i18n::t(note), crate::video_tab::WRAP + 8) {
                notes.push((Point::new(x0, y + 12.0), line));
                y += 16.0;
            }
            y += 4.0;
        }
        let mut message = Vec::new();
        if !self.message.is_empty() {
            y += 6.0;
            for line in wrap(
                atomik_ui::i18n::t(&self.message),
                crate::video_tab::WRAP + 8,
            ) {
                message.push((Point::new(x0, y + 12.0), line));
                y += 16.0;
            }
        }
        ThemesLayout {
            rows,
            buttons,
            question,
            notes,
            message,
        }
    }

    /// Whether the button works on the theme selected now.
    pub fn enabled(&self, button: Button) -> bool {
        let Some(row) = self.selected() else {
            return matches!(button, Button::Import | Button::OpenFolder);
        };
        !(button.user_only() && row.builtin)
    }

    fn press(&mut self, button: Button) -> TabOutcome {
        let Some(row) = self.selected().cloned() else {
            return TabOutcome::Open;
        };
        if !self.enabled(button) {
            self.message = "A built-in theme cannot be changed: duplicate it first.".into();
            return TabOutcome::Open;
        }
        self.message.clear();
        let action = match button {
            Button::Duplicate => ThemeAction::Duplicate(row.id),
            Button::Rename => {
                let mut field = TextField::new(&row.name, MAX_NAME_CHARS, false);
                field.select_all();
                self.mode = Mode::Rename(Box::new(field));
                return TabOutcome::Open;
            }
            Button::Delete => {
                self.mode = Mode::ConfirmDelete;
                return TabOutcome::Open;
            }
            Button::Edit => ThemeAction::Edit(row.id),
            Button::Import => ThemeAction::Import,
            Button::Export => ThemeAction::Export(row.id),
            Button::OpenFolder => ThemeAction::OpenFolder,
        };
        TabOutcome::Theme(action)
    }

    /// Handles an event inside `panel`.
    pub(crate) fn handle(&mut self, event: &UiEvent, panel: Rect) -> TabOutcome {
        match event {
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                if let Mode::Rename(field) = &mut self.mode {
                    match key {
                        Key::Escape => self.mode = Mode::Idle,
                        Key::Enter => {
                            let name = field.text().trim().to_string();
                            let id = self.rows.get(self.selected).map(|r| r.id.clone());
                            self.mode = Mode::Idle;
                            if let Some(id) = id {
                                return TabOutcome::Theme(ThemeAction::Rename(id, name));
                            }
                        }
                        _ => {
                            field.handle_key(key, *modifiers);
                        }
                    }
                    return TabOutcome::Open;
                }
                if self.is_confirming() {
                    match key {
                        Key::Escape => self.mode = Mode::Idle,
                        Key::Enter => {
                            self.mode = Mode::Idle;
                            if let Some(r) = self.selected() {
                                return TabOutcome::Theme(ThemeAction::Delete(r.id.clone()));
                            }
                        }
                        _ => {}
                    }
                    return TabOutcome::Open;
                }
                match key {
                    Key::Escape => return TabOutcome::Cancel,
                    Key::Up | Key::Down => {
                        let n = self.rows.len();
                        if n > 0 {
                            self.selected = if *key == Key::Up {
                                (self.selected + n - 1) % n
                            } else {
                                (self.selected + 1) % n
                            };
                            return TabOutcome::Theme(ThemeAction::Use(
                                self.rows[self.selected].id.clone(),
                            ));
                        }
                    }
                    Key::Enter => return TabOutcome::Apply,
                    _ => {}
                }
            }
            UiEvent::Ime(ime) => {
                if let Mode::Rename(field) = &mut self.mode {
                    field.handle_ime(ime);
                }
            }
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                let l = self.layout(panel);
                if let Some((which, _)) = l.buttons.iter().find(|(_, r)| r.contains(*pos)) {
                    if self.is_confirming() {
                        let delete = *which == Button::Delete;
                        let id = self.selected().map(|r| r.id.clone());
                        self.mode = Mode::Idle;
                        if delete && let Some(id) = id {
                            return TabOutcome::Theme(ThemeAction::Delete(id));
                        }
                        return TabOutcome::Open;
                    }
                    self.mode = Mode::Idle;
                    return self.press(*which);
                }
                if let Mode::Rename(field) = &mut self.mode {
                    let row = l.rows.get(self.selected).copied();
                    if let Some(row) = row
                        && row.contains(*pos)
                    {
                        field.press(*pos, *clicks, modifiers.shift);
                        return TabOutcome::Open;
                    }
                    self.mode = Mode::Idle;
                }
                if self.is_confirming() {
                    return TabOutcome::Open;
                }
                if let Some(i) = l.rows.iter().position(|r| r.contains(*pos)) {
                    self.selected = i;
                    self.message.clear();
                    return TabOutcome::Theme(ThemeAction::Use(self.rows[i].id.clone()));
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
        for (i, (row, rect)) in self.rows.iter().zip(&l.rows).enumerate() {
            let shape = RoundedRect::from_rect(*rect, 6.0);
            if i == self.selected {
                canvas.fill_with(shape, theme.browser_row);
            } else if lit(*rect) {
                canvas.fill_with(shape, theme.browser_row.with_alpha(0.5));
            }
            for (k, color) in row.swatches.iter().enumerate() {
                let x = rect.x0 + 10.0 + k as f64 * (SWATCH + 3.0);
                let s = Rect::new(x, rect.y0 + 10.0, x + SWATCH, rect.y0 + 10.0 + SWATCH);
                canvas.fill_with(RoundedRect::from_rect(s, 3.0), *color);
                canvas.stroke_with(
                    RoundedRect::from_rect(s.inset(-0.5), 3.5),
                    1.0,
                    theme.section_line,
                );
            }
            let name_x = rect.x0 + 10.0 + SWATCHES as f64 * (SWATCH + 3.0) + 12.0;
            let renaming = match &self.mode {
                Mode::Rename(field) if i == self.selected => Some(field),
                _ => None,
            };
            if let Some(field) = renaming {
                let box_ = Rect::new(name_x - 4.0, rect.y0 + 4.0, name_x + 230.0, rect.y1 - 4.0);
                canvas.fill_with(RoundedRect::from_rect(box_, 4.0), theme.lcd);
                canvas.stroke_with(RoundedRect::from_rect(box_, 4.0), 1.0, theme.focus);
                let colors = FieldColors {
                    text: theme.text,
                    caret: theme.text,
                    selection: theme.focus.with_alpha(0.35),
                };
                field.paint(canvas, box_.inset(-4.0), 12.5, 17.0, colors, true);
            } else if row.builtin {
                canvas.text(&TextRun::new(
                    row.name.clone(),
                    Point::new(name_x, rect.y0 + 21.0),
                    13.0,
                    theme.text,
                ));
            } else {
                canvas.text(&TextRun::verbatim(
                    fit_label_raw(&row.name, 240.0, 13.0),
                    Point::new(name_x, rect.y0 + 21.0),
                    13.0,
                    theme.text,
                ));
            }
            let (word, color) = if row.id == self.active {
                ("In use", theme.text)
            } else if row.builtin {
                ("Built-in", theme.text_muted)
            } else {
                ("", theme.text_muted)
            };
            if !word.is_empty() {
                canvas.text(
                    &TextRun::new(
                        word,
                        Point::new(rect.x1 - 12.0, rect.y0 + 21.0),
                        11.5,
                        color,
                    )
                    .align(Align::End),
                );
            }
        }
        if let Some((at, name)) = &l.question {
            canvas.text(&TextRun::new(
                "Delete this theme? It cannot be brought back.",
                *at,
                12.0,
                theme.text,
            ));
            canvas.text(&TextRun::verbatim(
                name.clone(),
                Point::new(at.x, at.y + 16.0),
                11.5,
                theme.text_muted,
            ));
        }
        for (b, r) in &l.buttons {
            let enabled = self.enabled(*b) || self.is_confirming();
            let label = if self.is_confirming() {
                if *b == Button::Delete {
                    "Delete"
                } else {
                    "Keep"
                }
            } else {
                b.label()
            };
            canvas.fill_with(
                RoundedRect::from_rect(*r, 6.0),
                if enabled && lit(*r) {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 18.0),
                    12.0,
                    if enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                )
                .align(Align::Center),
            );
        }
        for (at, line) in &l.notes {
            canvas.text(&TextRun::new(line.clone(), *at, 11.5, theme.text_muted));
        }
        for (at, line) in &l.message {
            canvas.text(&TextRun::verbatim(line.clone(), *at, 11.5, theme.warning));
        }
    }
}
