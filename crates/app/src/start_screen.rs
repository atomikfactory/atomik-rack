//! The start screen: a modal box that offers a new song, the templates, the recent songs (with
//! their dates and folders) and Open. It shows when the app starts with no song to open (unless
//! the preference says not to) and from File > Start Screen.
//!
//! The box only draws and picks: what a pick does is an [`AppCommand`] the shell runs, the same
//! command the File menu runs for that row, so the question about unsaved changes, a template
//! that is gone and a recent song that is gone behave exactly as they do there.

use crate::commands::AppCommand;
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::paint::{fit_label_in, fit_label_weight, measure, snap_hairline};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Help, Key, PointerButton, TextRun, Theme, UiEvent};
use std::path::Path;

/// The most templates the box lists (the File menu lists them all).
pub const VISIBLE_TEMPLATES: usize = 8;
/// The most recent songs the box lists (the whole recent list).
pub const VISIBLE_RECENT: usize = crate::recent::MAX_RECENT;

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 470.0;
const PAD: f64 = 24.0;
const LEFT_WIDTH: f64 = 250.0;
const COLUMN_GAP: f64 = 28.0;
/// Where the columns start below the top of the box.
const TOP: f64 = 84.0;
const HEADING: f64 = 20.0;
const ROW: f64 = 26.0;
const RECENT_ROW: f64 = 38.0;
const FOOT: f64 = 56.0;
const BUTTON: (f64, f64) = (96.0, 28.0);
const BOX: f64 = 14.0;

/// One recent song as the box lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRow {
    /// The song's name (its file name without the extension).
    pub name: String,
    /// The folder the song is in.
    pub folder: String,
    /// The day the song was last changed (`2026-10-29`), or empty when the file is gone.
    pub date: String,
    /// Whether the file is still there.
    pub exists: bool,
}

impl RecentRow {
    /// The row for the song at `path`; `modified` is the file's change time in seconds since the
    /// Unix epoch (`None` when the file cannot be read).
    pub fn new(path: &Path, modified: Option<u64>) -> Self {
        Self {
            name: crate::recent::menu_label(path),
            folder: path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            date: modified.map(crate::logging::date_stamp).unwrap_or_default(),
            exists: modified.is_some(),
        }
    }
}

/// What a row of the box does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A new song (File > New).
    New,
    /// The file picker (File > Open).
    Open,
    /// The template at this place of the templates list.
    Template(usize),
    /// The song at this place of the recent list.
    Recent(usize),
    /// The "show at start-up" check box.
    ShowAtStart,
    /// The Close button.
    Close,
}

impl Target {
    /// The command a pick of this row runs (`None` for the check box and Close).
    pub fn command(self) -> Option<AppCommand> {
        match self {
            Target::New => Some(AppCommand::New),
            Target::Open => Some(AppCommand::Open),
            Target::Template(i) => Some(AppCommand::NewFromTemplate(i)),
            Target::Recent(i) => Some(AppCommand::OpenRecent(i)),
            Target::ShowAtStart | Target::Close => None,
        }
    }
}

/// What the box asks of the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    /// The box stays open.
    Open,
    /// The box is closed.
    Close,
    /// Close the box and run this command.
    Run(AppCommand),
    /// The check box was flipped: the preference is now this.
    ShowAtStart(bool),
}

/// The start screen.
#[derive(Debug, Clone)]
pub struct StartScreen {
    templates: Vec<String>,
    recent: Vec<RecentRow>,
    default_song: bool,
    show_at_start: bool,
    focus: Target,
    hover: Option<Target>,
}

impl StartScreen {
    /// A box listing `templates` (names) and `recent` songs. `default_song` says File > New
    /// opens the chosen default song; `show_at_start` is the preference.
    pub fn new(
        templates: Vec<String>,
        recent: Vec<RecentRow>,
        default_song: bool,
        show_at_start: bool,
    ) -> Self {
        let mut templates = templates;
        templates.truncate(VISIBLE_TEMPLATES);
        let mut recent = recent;
        recent.truncate(VISIBLE_RECENT);
        Self {
            templates,
            recent,
            default_song,
            show_at_start,
            focus: Target::New,
            hover: None,
        }
    }

    /// The row that has the keys.
    pub fn focus(&self) -> Target {
        self.focus
    }

    /// Whether the check box is ticked.
    pub fn show_at_start(&self) -> bool {
        self.show_at_start
    }

    /// The templates the box lists.
    pub fn templates(&self) -> &[String] {
        &self.templates
    }

    /// The recent songs the box lists.
    pub fn recent(&self) -> &[RecentRow] {
        &self.recent
    }

    /// The box in a window of `view` (logical units).
    pub fn rect(&self, view: Size) -> Rect {
        let w = (view.width - 32.0).clamp(300.0, WIDTH);
        let h = (view.height - 32.0).clamp(240.0, HEIGHT);
        let x0 = ((view.width - w) / 2.0).max(0.0);
        let y0 = ((view.height - h) / 2.0).max(0.0);
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// The Close button.
    pub fn close_button(&self, view: Size) -> Rect {
        let r = self.rect(view);
        Rect::new(
            r.x1 - PAD - BUTTON.0,
            r.y1 - FOOT + 14.0,
            r.x1 - PAD,
            r.y1 - FOOT + 14.0 + BUTTON.1,
        )
    }

    /// The check box and its label.
    pub fn check_row(&self, view: Size) -> Rect {
        let r = self.rect(view);
        Rect::new(
            r.x0 + PAD,
            r.y1 - FOOT + 14.0,
            r.x1 - 2.0 * PAD - BUTTON.0,
            r.y1 - FOOT + 14.0 + BUTTON.1,
        )
    }

    /// Whether the window is wide enough for the recent column beside the left one.
    fn two_columns(&self, view: Size) -> bool {
        self.rect(view).width() >= 560.0
    }

    /// The left column's x range.
    fn left_x(&self, view: Size) -> (f64, f64) {
        let r = self.rect(view);
        let right = if self.two_columns(view) {
            r.x0 + PAD + LEFT_WIDTH
        } else {
            r.x1 - PAD
        };
        (r.x0 + PAD, right)
    }

    /// The recent column's x range.
    fn right_x(&self, view: Size) -> (f64, f64) {
        let r = self.rect(view);
        if self.two_columns(view) {
            (r.x0 + PAD + LEFT_WIDTH + COLUMN_GAP, r.x1 - PAD)
        } else {
            self.left_x(view)
        }
    }

    /// The bottom of the area the lists use.
    fn list_bottom(&self, view: Size) -> f64 {
        self.rect(view).y1 - FOOT
    }

    /// Where the headings of the left column sit: START, then TEMPLATES.
    fn left_headings(&self, view: Size) -> (f64, f64) {
        let top = self.rect(view).y0 + TOP;
        (top, top + HEADING + 2.0 * ROW + 12.0)
    }

    /// The rows of the left column that fit, top to bottom.
    fn left_rows(&self, view: Size) -> Vec<(Target, Rect)> {
        let (x0, x1) = self.left_x(view);
        let (start, templates) = self.left_headings(view);
        let bottom = self.list_bottom(view);
        let row = |y: f64| Rect::new(x0, y, x1, y + ROW);
        let mut rows = vec![
            (Target::New, row(start + HEADING)),
            (Target::Open, row(start + HEADING + ROW)),
        ];
        for i in 0..self.templates.len() {
            rows.push((
                Target::Template(i),
                row(templates + HEADING + ROW * i as f64),
            ));
        }
        rows.retain(|(_, r)| r.y1 <= bottom);
        rows
    }

    /// The rows of the recent column that fit, top to bottom (below the left column when the
    /// window is narrow).
    fn recent_rows(&self, view: Size) -> Vec<(Target, Rect)> {
        let (x0, x1) = self.right_x(view);
        let top = if self.two_columns(view) {
            self.rect(view).y0 + TOP + HEADING
        } else {
            // One column: the recent songs have no room; the File menu lists them.
            return Vec::new();
        };
        let bottom = self.list_bottom(view);
        (0..self.recent.len())
            .map(|i| {
                let y = top + RECENT_ROW * i as f64;
                (Target::Recent(i), Rect::new(x0, y, x1, y + RECENT_ROW))
            })
            .filter(|(_, r)| r.y1 <= bottom)
            .collect()
    }

    /// Every row the box shows with its rectangle: the left column, the recent column, the check
    /// box and Close.
    pub fn rows(&self, view: Size) -> Vec<(Target, Rect)> {
        let mut rows = self.left_rows(view);
        rows.extend(self.recent_rows(view));
        rows.push((Target::ShowAtStart, self.check_row(view)));
        rows.push((Target::Close, self.close_button(view)));
        rows
    }

    /// The row under `pos`.
    pub fn row_at(&self, view: Size, pos: Point) -> Option<Target> {
        self.rows(view)
            .into_iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(t, _)| t)
    }

    /// What the row under `pos` does, for its tooltip.
    pub fn help_at(&self, view: Size, pos: Point) -> Option<Help> {
        let rows = self.rows(view);
        let (target, rect) = rows.into_iter().find(|(_, r)| r.contains(pos))?;
        let text = match target {
            Target::New if self.default_song => "A new song from the default song",
            Target::New => "A new, empty song",
            Target::Open => "Choose a song file to open",
            Target::Template(_) => "A new song made from this template",
            Target::Recent(_) => "Open this song",
            Target::ShowAtStart => "Show this screen whenever Atomik Rack starts without a song",
            Target::Close => "Close the start screen",
        };
        Some(Help::new(rect, text))
    }

    /// Moves the keys to the next or previous row of the whole box (Tab).
    fn step(&mut self, view: Size, back: bool) {
        let rows: Vec<Target> = self.rows(view).into_iter().map(|(t, _)| t).collect();
        let at = rows.iter().position(|t| *t == self.focus).unwrap_or(0);
        let n = rows.len();
        self.focus = rows[if back { (at + n - 1) % n } else { (at + 1) % n }];
    }

    /// Moves the keys up or down inside the column that has them.
    fn move_in_column(&mut self, view: Size, delta: isize) {
        let left: Vec<Target> = self.left_rows(view).into_iter().map(|(t, _)| t).collect();
        let right: Vec<Target> = self.recent_rows(view).into_iter().map(|(t, _)| t).collect();
        let column = if right.contains(&self.focus) {
            &right
        } else if left.contains(&self.focus) {
            &left
        } else {
            // On the check box or Close: Up goes back to the lists, Down stays.
            if delta < 0
                && let Some(last) = left.last()
            {
                self.focus = *last;
            }
            return;
        };
        let at = column.iter().position(|t| *t == self.focus).unwrap_or(0) as isize;
        let to = (at + delta).clamp(0, column.len() as isize - 1) as usize;
        self.focus = column[to];
    }

    /// Moves the keys to the other column, to the row at the same place (or the nearest).
    fn switch_column(&mut self, view: Size, to_recent: bool) {
        let left = self.left_rows(view);
        let right = self.recent_rows(view);
        let (from, into) = if to_recent {
            (&left, &right)
        } else {
            (&right, &left)
        };
        let Some(at) = from.iter().position(|(t, _)| *t == self.focus) else {
            return;
        };
        if into.is_empty() {
            return;
        }
        // Rows of the two columns have different heights: match them by their vertical place.
        let y = from[at].1.y0;
        let nearest = into
            .iter()
            .min_by(|a, b| (a.1.y0 - y).abs().total_cmp(&(b.1.y0 - y).abs()));
        if let Some((t, _)) = nearest {
            self.focus = *t;
        }
    }

    /// Picks `target`: the outcome the shell acts on.
    fn pick(&mut self, target: Target) -> StartOutcome {
        match target {
            Target::Close => StartOutcome::Close,
            Target::ShowAtStart => {
                self.show_at_start = !self.show_at_start;
                StartOutcome::ShowAtStart(self.show_at_start)
            }
            other => other
                .command()
                .map_or(StartOutcome::Open, StartOutcome::Run),
        }
    }

    /// Handles an event of the window of size `view`.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> StartOutcome {
        match event {
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => match key {
                Key::Escape => return StartOutcome::Close,
                Key::Enter | Key::Space => {
                    let target = self.focus;
                    return self.pick(target);
                }
                Key::Up => self.move_in_column(view, -1),
                Key::Down => self.move_in_column(view, 1),
                Key::Left => self.switch_column(view, false),
                Key::Right => self.switch_column(view, true),
                Key::Tab => self.step(view, modifiers.shift),
                Key::Home => {
                    if let Some((t, _)) = self.left_rows(view).first() {
                        self.focus = *t;
                    }
                }
                Key::End => {
                    if let Some((t, _)) = self.left_rows(view).last() {
                        self.focus = *t;
                    }
                }
                _ => {}
            },
            UiEvent::PointerMove { pos, .. } => self.hover = self.row_at(view, *pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(target) = self.row_at(view, *pos) {
                    self.focus = target;
                    return self.pick(target);
                }
                if !self.rect(view).contains(*pos) {
                    return StartOutcome::Close;
                }
            }
            _ => {}
        }
        StartOutcome::Open
    }

    /// The label of the New row.
    fn new_label(&self) -> &'static str {
        if self.default_song {
            "New Song (default song)"
        } else {
            "New Song"
        }
    }

    /// Every text the box draws, in drawing order (for tests and tooltips of the whole box).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Atomik Rack".to_string(),
            "Start a song, or pick up where you left off.".to_string(),
            "START".to_string(),
            self.new_label().to_string(),
            "Open\u{2026}".to_string(),
            "TEMPLATES".to_string(),
        ];
        if self.templates.is_empty() {
            lines.push("No templates yet".to_string());
        }
        lines.extend(self.templates.iter().cloned());
        lines.push("RECENT".to_string());
        if self.recent.is_empty() {
            lines.push("No songs opened yet".to_string());
        }
        for r in &self.recent {
            lines.push(r.name.clone());
        }
        lines.push("Show this screen when Atomik Rack starts".to_string());
        lines.push("Close".to_string());
        lines
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
        canvas.text(
            &TextRun::new(
                "Atomik Rack",
                Point::new(r.x0 + PAD, r.y0 + 22.0),
                22.0,
                theme.text,
            )
            .weight(700),
        );
        canvas.text(&TextRun::new(
            "Start a song, or pick up where you left off.",
            Point::new(r.x0 + PAD, r.y0 + 54.0),
            12.5,
            theme.text_muted,
        ));
        let heading = |canvas: &mut dyn Canvas, text: &str, x: f64, y: f64| {
            canvas.text(
                &TextRun::new(text, Point::new(x, y + 3.0), 11.0, theme.text_muted).weight(700),
            );
        };
        let (lx, _) = self.left_x(view);
        let (start, templates) = self.left_headings(view);
        heading(canvas, "START", lx, start);
        heading(canvas, "TEMPLATES", lx, templates);
        if self.two_columns(view) {
            let (rx, _) = self.right_x(view);
            heading(canvas, "RECENT", rx, r.y0 + TOP);
        }
        let rule = snap_hairline(r.y1 - FOOT, 1.0);
        canvas.fill_with(
            Rect::new(r.x0 + PAD, rule, r.x1 - PAD, rule + 1.0),
            theme.section_line,
        );

        for (target, rect) in self.left_rows(view) {
            self.paint_row_back(canvas, theme, target, rect);
            let label = match target {
                Target::New => self.new_label().to_string(),
                Target::Open => "Open\u{2026}".to_string(),
                Target::Template(i) => self.templates[i].clone(),
                _ => String::new(),
            };
            let text = fit_label_in(canvas, &label, rect.width() - 20.0, 13.0);
            canvas.text(&TextRun::new(
                text,
                Point::new(rect.x0 + 10.0, rect.y0 + 18.0),
                13.0,
                theme.text,
            ));
        }
        if self.templates.is_empty() && self.left_rows(view).len() >= 2 {
            canvas.text(&TextRun::new(
                "No templates yet",
                Point::new(lx + 10.0, templates + HEADING + 18.0),
                12.5,
                theme.text_muted,
            ));
        }
        if self.two_columns(view) {
            if self.recent.is_empty() {
                let (rx, _) = self.right_x(view);
                canvas.text(&TextRun::new(
                    "No songs opened yet",
                    Point::new(rx + 10.0, r.y0 + TOP + HEADING + 18.0),
                    12.5,
                    theme.text_muted,
                ));
            }
            for (target, rect) in self.recent_rows(view) {
                let Target::Recent(i) = target else { continue };
                let song = &self.recent[i];
                self.paint_row_back(canvas, theme, target, rect);
                let ink = if song.exists {
                    theme.text
                } else {
                    theme.text_muted
                };
                let date = if song.exists { &song.date } else { "missing" };
                let date_w = measure(canvas, date, 11.0, 400);
                canvas.text(
                    &TextRun::new(
                        date,
                        Point::new(rect.x1 - 10.0, rect.y0 + 14.5),
                        11.0,
                        theme.text_muted,
                    )
                    .align(Align::End),
                );
                let name =
                    fit_label_weight(canvas, &song.name, rect.width() - 28.0 - date_w, 13.0, 400);
                canvas.text(&TextRun::new(
                    name,
                    Point::new(rect.x0 + 10.0, rect.y0 + 15.0),
                    13.0,
                    ink,
                ));
                let folder = atomik_ui::paint::tail_fit(
                    canvas,
                    &song.folder,
                    rect.width() - 20.0,
                    11.0,
                    400,
                );
                canvas.text(&TextRun::new(
                    folder,
                    Point::new(rect.x0 + 10.0, rect.y0 + 31.0),
                    11.0,
                    theme.text_muted,
                ));
            }
        }
        self.paint_check(canvas, theme, view);
        let b = self.close_button(view);
        self.paint_row_back(canvas, theme, Target::Close, b);
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

    /// The background of a row: the keys' row has a ring, the pointer's row is lit.
    fn paint_row_back(&self, canvas: &mut dyn Canvas, theme: &Theme, target: Target, rect: Rect) {
        let shape = RoundedRect::from_rect(rect, 6.0);
        if self.hover == Some(target) {
            canvas.fill_with(shape, theme.selection);
        } else if target == Target::Close {
            canvas.fill_with(shape, theme.browser_row);
        }
        if self.focus == target {
            canvas.stroke_with(shape, 1.5, theme.focus);
        }
    }

    fn paint_check(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        let row = self.check_row(view);
        self.paint_row_back(canvas, theme, Target::ShowAtStart, row);
        let cy = row.center().y;
        let square = Rect::new(
            row.x0 + 8.0,
            cy - BOX / 2.0,
            row.x0 + 8.0 + BOX,
            cy + BOX / 2.0,
        );
        let shape = RoundedRect::from_rect(square, 3.0);
        canvas.stroke_with(shape, 1.0, theme.text_muted);
        if self.show_at_start {
            canvas.fill_with(
                RoundedRect::from_rect(square.inset(-3.0), 1.5),
                theme.highlight,
            );
        }
        let label = fit_label_in(
            canvas,
            "Show this screen when Atomik Rack starts",
            row.width() - 40.0,
            12.5,
        );
        canvas.text(&TextRun::new(
            label,
            Point::new(square.x1 + 8.0, cy + 4.5),
            12.5,
            theme.text,
        ));
    }
}

#[cfg(test)]
#[path = "start_screen_tests.rs"]
mod tests;
