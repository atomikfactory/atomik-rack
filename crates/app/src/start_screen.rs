//! The start screen: a modal box that offers a new song, the templates, the recent songs (with
//! their dates and folders) and Open. It shows when the app starts with no song to open (unless
//! the preference says not to) and from File > Start Screen.
//!
//! A template under the pointer or the keys shows a card with what is in it: its tracks, tempo and
//! meter ([`TemplatePreview`], read by the shell on demand). A recent song has a pin: a pinned
//! song stays at the top of the list (click the pin, or press P on the row).
//!
//! Below the title a strip of cards offers more: the factory example songs (each opens as a new
//! unsaved song, with a line saying what it is for) and "Make your first song", which starts the
//! guided first song (the same command as Help > Guided Tour). A second check box at the foot
//! hides the guide card; the strip is then all examples.
//!
//! The box only draws and picks: what a pick does is an [`AppCommand`] the shell runs, the same
//! command the File menu runs for that row, so the question about unsaved changes, a template
//! that is gone and a recent song that is gone behave exactly as they do there.

use crate::app_core::TemplatePreview;
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
/// The pin button at the right end of a recent row.
const PIN: f64 = 22.0;
/// The width of the template card.
const CARD_WIDTH: f64 = 232.0;
/// The most tracks the template card lists.
const CARD_TRACKS: usize = 8;
const CARD_LINE: f64 = 16.0;
/// The height of a card of the strip, and the space the strip takes (the card and a gap).
const STRIP_CARD: f64 = 88.0;
const STRIP: f64 = STRIP_CARD + 14.0;
const STRIP_GAP: f64 = 12.0;
/// The widest a card of the strip gets (one card alone does not stretch over the box).
const STRIP_CARD_WIDTH: f64 = 240.0;
/// The most example cards the strip shows.
pub const VISIBLE_EXAMPLES: usize = 3;
/// A box shorter than this has no room for the strip: the lists come first.
const STRIP_MIN_HEIGHT: f64 = 540.0;
/// The width of the "show at start-up" check box when a second one sits beside it.
const CHECK_SPLIT: f64 = 332.0;

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
    /// Whether the song is pinned to the top of the list.
    pub pinned: bool,
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
            pinned: false,
        }
    }

    /// The same row, pinned or not.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }
}

/// One example song as the strip shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleCard {
    /// The song name.
    pub name: String,
    /// What it is for, in a line.
    pub line: String,
}

impl ExampleCard {
    /// A card for the example `name` with its `line`.
    pub fn new(name: impl Into<String>, line: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            line: line.into(),
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
    /// The pin button of the song at this place of the recent list. It is hit by the pointer only:
    /// the keys pin the song of the row they are on with P.
    Pin(usize),
    /// The "show at start-up" check box.
    ShowAtStart,
    /// The "Make your first song" card: the guided first song.
    Guide,
    /// The card of the example at this place of the examples list.
    Example(usize),
    /// The check box that shows or hides the guide card.
    ShowGuide,
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
            Target::Guide => Some(AppCommand::GuidedTour),
            Target::Example(i) => Some(AppCommand::OpenExample(i)),
            Target::Pin(_) | Target::ShowAtStart | Target::ShowGuide | Target::Close => None,
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
    /// The guide card check box was flipped: the preference is now this.
    ShowGuide(bool),
    /// Pin or unpin the song at this place of the recent list; the shell changes the list and
    /// gives the box the new rows ([`StartScreen::set_recent`]).
    TogglePin(usize),
}

/// The start screen.
#[derive(Debug, Clone)]
pub struct StartScreen {
    templates: Vec<String>,
    /// What each template holds: `None` until the shell has looked (see
    /// [`StartScreen::preview_wanted`]), then the answer.
    previews: Vec<Option<Option<TemplatePreview>>>,
    recent: Vec<RecentRow>,
    default_song: bool,
    show_at_start: bool,
    /// The example songs of the strip.
    examples: Vec<ExampleCard>,
    /// The strip and the second check box exist (the shell asks for them; a bare box has none).
    guide_offered: bool,
    /// The guide card shows in the strip.
    show_guide: bool,
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
            previews: vec![None; templates.len()],
            templates,
            recent,
            default_song,
            show_at_start,
            examples: Vec::new(),
            guide_offered: false,
            show_guide: true,
            focus: Target::New,
            hover: None,
        }
    }

    /// The same box with the strip of cards: the `examples` (at most [`VISIBLE_EXAMPLES`]) and
    /// the guide card, shown while `show_guide` is on (the second check box flips it).
    pub fn with_cards(mut self, examples: Vec<ExampleCard>, show_guide: bool) -> Self {
        self.examples = examples;
        self.examples.truncate(VISIBLE_EXAMPLES);
        self.guide_offered = true;
        self.show_guide = show_guide;
        self
    }

    /// Whether the guide card is on (the second check box is ticked).
    pub fn show_guide(&self) -> bool {
        self.show_guide
    }

    /// The example cards.
    pub fn examples(&self) -> &[ExampleCard] {
        &self.examples
    }

    /// Whether there is a card to show at all.
    fn has_cards(&self) -> bool {
        (self.guide_offered && self.show_guide) || !self.examples.is_empty()
    }

    /// Whether the strip of cards is drawn: there are cards and the box is big enough.
    pub fn cards_shown(&self, view: Size) -> bool {
        self.has_cards() && self.two_columns(view) && self.rect(view).height() >= STRIP_MIN_HEIGHT
    }

    /// Where the lists start: lower when the strip is above them.
    fn top(&self, view: Size) -> f64 {
        let strip = if self.cards_shown(view) { STRIP } else { 0.0 };
        self.rect(view).y0 + TOP + strip
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

    /// Replaces the recent rows (a song was pinned or unpinned) and puts the keys on the row
    /// `focus` of the new list.
    pub fn set_recent(&mut self, recent: Vec<RecentRow>, focus: Option<usize>) {
        let mut recent = recent;
        recent.truncate(VISIBLE_RECENT);
        self.recent = recent;
        if let Some(i) = focus.filter(|i| *i < self.recent.len()) {
            self.focus = Target::Recent(i);
        } else if matches!(self.focus, Target::Recent(_) | Target::Pin(_)) {
            self.focus = Target::New;
        }
    }

    /// The template the card describes: the one under the pointer, else the one the keys are on.
    pub fn shown_template(&self) -> Option<usize> {
        let of = |t: Option<Target>| match t {
            Some(Target::Template(i)) if i < self.templates.len() => Some(i),
            _ => None,
        };
        of(self.hover).or_else(|| of(Some(self.focus)))
    }

    /// The template the shell should read the preview of: the one the card is about, when it has
    /// not been looked at yet.
    pub fn preview_wanted(&self) -> Option<usize> {
        self.shown_template()
            .filter(|i| self.previews.get(*i).is_some_and(Option::is_none))
    }

    /// Gives the preview of template `i` (`None`: it cannot be read, so the card says so).
    pub fn set_preview(&mut self, i: usize, preview: Option<TemplatePreview>) {
        if let Some(slot) = self.previews.get_mut(i) {
            *slot = Some(preview);
        }
    }

    /// The preview of template `i`, when it was read and could be.
    pub fn preview(&self, i: usize) -> Option<&TemplatePreview> {
        self.previews.get(i)?.as_ref()?.as_ref()
    }

    /// The box in a window of `view` (logical units).
    pub fn rect(&self, view: Size) -> Rect {
        let w = (view.width - 32.0).clamp(300.0, WIDTH);
        let strip = if self.has_cards() { STRIP } else { 0.0 };
        let h = (view.height - 32.0).clamp(240.0, HEIGHT + strip);
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

    /// Whether the second check box (the guide card) is drawn.
    fn guide_check_shown(&self, view: Size) -> bool {
        self.guide_offered && self.two_columns(view)
    }

    /// The check box and its label.
    pub fn check_row(&self, view: Size) -> Rect {
        let r = self.rect(view);
        let right = if self.guide_check_shown(view) {
            r.x0 + PAD + CHECK_SPLIT
        } else {
            r.x1 - 2.0 * PAD - BUTTON.0
        };
        Rect::new(
            r.x0 + PAD,
            r.y1 - FOOT + 14.0,
            right,
            r.y1 - FOOT + 14.0 + BUTTON.1,
        )
    }

    /// The check box of the guide card, beside the first one.
    pub fn guide_check_row(&self, view: Size) -> Rect {
        let r = self.rect(view);
        Rect::new(
            r.x0 + PAD + CHECK_SPLIT,
            r.y1 - FOOT + 14.0,
            r.x1 - 2.0 * PAD - BUTTON.0,
            r.y1 - FOOT + 14.0 + BUTTON.1,
        )
    }

    /// The cards of the strip, in order: the guide card, then the examples.
    fn card_targets(&self) -> Vec<Target> {
        let mut targets = Vec::new();
        if self.guide_offered && self.show_guide {
            targets.push(Target::Guide);
        }
        targets.extend((0..self.examples.len()).map(Target::Example));
        targets
    }

    /// The cards of the strip that are drawn, left to right.
    fn card_rows(&self, view: Size) -> Vec<(Target, Rect)> {
        if !self.cards_shown(view) {
            return Vec::new();
        }
        let targets = self.card_targets();
        let r = self.rect(view);
        let n = targets.len() as f64;
        let inner = r.width() - 2.0 * PAD;
        let width = ((inner - STRIP_GAP * (n - 1.0)) / n).min(STRIP_CARD_WIDTH);
        let y = r.y0 + TOP;
        targets
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let x = r.x0 + PAD + (width + STRIP_GAP) * i as f64;
                (t, Rect::new(x, y, x + width, y + STRIP_CARD))
            })
            .collect()
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
        let top = self.top(view);
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
            self.top(view) + HEADING
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

    /// The pin buttons of the recent rows that fit: at the lower right of each row.
    fn pin_rows(&self, view: Size) -> Vec<(Target, Rect)> {
        self.recent_rows(view)
            .into_iter()
            .filter_map(|(t, r)| match t {
                Target::Recent(i) => Some((Target::Pin(i), pin_rect(r))),
                _ => None,
            })
            .collect()
    }

    /// Every row the box shows with its rectangle: the left column, the recent column, the check
    /// box and Close. The pin buttons are not rows ([`Self::pin_rows`]).
    pub fn rows(&self, view: Size) -> Vec<(Target, Rect)> {
        let mut rows = self.left_rows(view);
        rows.extend(self.recent_rows(view));
        rows.extend(self.card_rows(view));
        rows.push((Target::ShowAtStart, self.check_row(view)));
        if self.guide_check_shown(view) {
            rows.push((Target::ShowGuide, self.guide_check_row(view)));
        }
        rows.push((Target::Close, self.close_button(view)));
        rows
    }

    /// The row under `pos`; a pin button wins over the row it is on.
    pub fn row_at(&self, view: Size, pos: Point) -> Option<Target> {
        self.pin_rows(view)
            .into_iter()
            .chain(self.rows(view))
            .find(|(_, r)| r.contains(pos))
            .map(|(t, _)| t)
    }

    /// What the row under `pos` does, for its tooltip.
    pub fn help_at(&self, view: Size, pos: Point) -> Option<Help> {
        let rows = self.pin_rows(view).into_iter().chain(self.rows(view));
        let (target, rect) = rows.into_iter().find(|(_, r)| r.contains(pos))?;
        let text = match target {
            Target::New if self.default_song => "A new song from the default song",
            Target::New => "A new, empty song",
            Target::Open => "Choose a song file to open",
            Target::Template(_) => "A new song made from this template",
            Target::Recent(_) => "Open this song",
            Target::Pin(i) if self.recent.get(i).is_some_and(|r| r.pinned) => {
                "Unpin this song: it goes back to its place in the list by date"
            }
            Target::Pin(_) => "Pin this song: it stays at the top of the list",
            Target::ShowAtStart => "Show this screen whenever Atomik Rack starts without a song",
            Target::Guide => "A new song and seven short steps that make a first song",
            Target::Example(_) => "Open this example as a new, unsaved song",
            Target::ShowGuide => {
                "Show the first-song card here (Help > Guided Tour always starts it)"
            }
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
        let cards: Vec<Target> = self.card_rows(view).into_iter().map(|(t, _)| t).collect();
        if cards.contains(&self.focus) {
            // Down leaves the strip for the first row of the lists; Up has nowhere to go.
            if delta > 0
                && let Some(first) = left.first()
            {
                self.focus = *first;
            }
            return;
        }
        // Up from the top of a list goes to the strip.
        if delta < 0
            && let Some(card) = cards.first()
            && (left.first() == Some(&self.focus) || right.first() == Some(&self.focus))
        {
            self.focus = *card;
            return;
        }
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

    /// Whether the keys are on a card of the strip.
    fn focus_on_card(&self, view: Size) -> bool {
        self.card_rows(view).iter().any(|(t, _)| *t == self.focus)
    }

    /// Moves the keys to the next or previous card of the strip.
    fn move_in_strip(&mut self, view: Size, delta: isize) {
        let cards: Vec<Target> = self.card_rows(view).into_iter().map(|(t, _)| t).collect();
        let at = cards.iter().position(|t| *t == self.focus).unwrap_or(0) as isize;
        let to = (at + delta).clamp(0, cards.len() as isize - 1) as usize;
        self.focus = cards[to];
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
            Target::ShowGuide => {
                self.show_guide = !self.show_guide;
                StartOutcome::ShowGuide(self.show_guide)
            }
            Target::Pin(i) => StartOutcome::TogglePin(i),
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
                Key::Char('p' | 'P') if !modifiers.ctrl && !modifiers.alt && !modifiers.meta => {
                    if let Target::Recent(i) = self.focus {
                        return StartOutcome::TogglePin(i);
                    }
                }
                Key::Up => self.move_in_column(view, -1),
                Key::Down => self.move_in_column(view, 1),
                Key::Left if self.focus_on_card(view) => self.move_in_strip(view, -1),
                Key::Right if self.focus_on_card(view) => self.move_in_strip(view, 1),
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
                    // The keys stay on the song whose pin was clicked.
                    self.focus = match target {
                        Target::Pin(i) => Target::Recent(i),
                        other => other,
                    };
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
        ];
        if self.guide_offered && self.show_guide {
            lines.push("Make your first song".to_string());
        }
        for e in &self.examples {
            lines.push(e.name.clone());
        }
        lines.extend([
            "START".to_string(),
            self.new_label().to_string(),
            "Open\u{2026}".to_string(),
            "TEMPLATES".to_string(),
        ]);
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
        if self.guide_offered {
            lines.push("Show the first-song card".to_string());
        }
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
            heading(canvas, "RECENT", rx, self.top(view));
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
                    Point::new(rx + 10.0, self.top(view) + HEADING + 18.0),
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
                    rect.width() - 20.0 - PIN,
                    11.0,
                    400,
                );
                canvas.text(&TextRun::new(
                    folder,
                    Point::new(rect.x0 + 10.0, rect.y0 + 31.0),
                    11.0,
                    theme.text_muted,
                ));
                // A pinned song always shows its pin; the others show it under the pointer or
                // the keys.
                let lit = self.hover == Some(Target::Pin(i));
                if song.pinned || lit || self.hover == Some(target) || self.focus == target {
                    let ink = if song.pinned {
                        theme.highlight
                    } else if lit {
                        theme.text
                    } else {
                        theme.text_muted
                    };
                    paint_pin(canvas, pin_rect(rect), ink, song.pinned);
                }
            }
        }
        self.paint_strip(canvas, theme, view);
        self.paint_card(canvas, theme, view);
        let first = self.check_row(view);
        self.paint_check(
            canvas,
            theme,
            Target::ShowAtStart,
            first,
            "Show this screen when Atomik Rack starts",
            self.show_at_start,
        );
        if self.guide_check_shown(view) {
            let second = self.guide_check_row(view);
            self.paint_check(
                canvas,
                theme,
                Target::ShowGuide,
                second,
                "Show the first-song card",
                self.show_guide,
            );
        }
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

    /// The card that says what the template under the pointer or the keys holds.
    fn paint_card(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        let Some(i) = self.shown_template().filter(|_| self.two_columns(view)) else {
            return;
        };
        let Some((_, row)) = self
            .left_rows(view)
            .into_iter()
            .find(|(t, _)| *t == Target::Template(i))
        else {
            return;
        };
        let (rx, rx1) = self.right_x(view);
        let width = CARD_WIDTH.min(rx1 - rx);
        let lines = self.card_lines(i);
        let height = 16.0 + CARD_LINE * (lines.len() as f64 + 1.0) + 6.0;
        let top = self.top(view);
        let bottom = self.list_bottom(view);
        let y0 = (row.y0 - 8.0).clamp(top, (bottom - height).max(top));
        let card = Rect::new(rx, y0, rx + width, y0 + height);
        let shape = RoundedRect::from_rect(card, 8.0);
        canvas.fill_with(shape, theme.browser_row);
        canvas.stroke_with(shape, 1.0, theme.section_line);
        let title = fit_label_weight(canvas, &self.templates[i], width - 24.0, 13.0, 700);
        canvas.text(&TextRun::new(
            title,
            Point::new(card.x0 + 12.0, card.y0 + 20.0),
            13.0,
            theme.text,
        ));
        for (n, line) in lines.iter().enumerate() {
            let text = fit_label_in(canvas, line, width - 24.0, 11.5);
            canvas.text(&TextRun::new(
                text,
                Point::new(
                    card.x0 + 12.0,
                    card.y0 + 20.0 + CARD_LINE * (n as f64 + 1.0),
                ),
                11.5,
                theme.text_muted,
            ));
        }
    }

    /// The lines of the card of template `i`: tempo and meter, then the tracks.
    pub fn card_lines(&self, i: usize) -> Vec<String> {
        let Some(preview) = self.preview(i) else {
            return vec![if self.previews.get(i).is_some_and(Option::is_some) {
                "No preview for this template".to_string()
            } else {
                "Reading the template...".to_string()
            }];
        };
        let tempo = if preview.tempo_bpm.fract().abs() < 0.005 {
            format!("{:.0}", preview.tempo_bpm)
        } else {
            format!("{:.2}", preview.tempo_bpm)
        };
        let mut lines = vec![format!(
            "{tempo} BPM, {}/{}, {}",
            preview.meter.0,
            preview.meter.1,
            match preview.tracks.len() {
                0 => "no tracks".to_string(),
                1 => "1 track".to_string(),
                n => format!("{n} tracks"),
            }
        )];
        lines.extend(preview.tracks.iter().take(CARD_TRACKS).cloned());
        let more = preview.tracks.len().saturating_sub(CARD_TRACKS);
        if more > 0 {
            lines.push(format!("and {more} more"));
        }
        lines
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

    /// The strip of cards under the title: the guide card, then the examples.
    fn paint_strip(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        for (target, rect) in self.card_rows(view) {
            let (title, line) = match target {
                Target::Guide => (
                    "Make your first song",
                    "Seven short steps, from nothing to an export.",
                ),
                Target::Example(i) => (
                    self.examples[i].name.as_str(),
                    self.examples[i].line.as_str(),
                ),
                _ => continue,
            };
            let shape = RoundedRect::from_rect(rect, 8.0);
            canvas.fill_with(shape, theme.browser_row);
            if self.hover == Some(target) {
                canvas.fill_with(shape, theme.selection);
            }
            let edge = if target == Target::Guide {
                theme.highlight
            } else {
                theme.section_line
            };
            canvas.stroke_with(shape, 1.0, edge);
            if self.focus == target {
                canvas.stroke_with(shape, 1.5, theme.focus);
            }
            // A long title takes a second line and leaves two for the description.
            let heading = atomik_ui::paint::wrap_name(title, rect.width() - 24.0, 13.0);
            for (n, text) in heading.iter().enumerate() {
                canvas.text(
                    &TextRun::new(
                        text.clone(),
                        Point::new(rect.x0 + 12.0, rect.y0 + 21.0 + 15.0 * n as f64),
                        13.0,
                        theme.text,
                    )
                    .weight(700),
                );
            }
            let first = 21.0 + 15.0 * (heading.len() as f64 - 1.0) + 20.0;
            let room = if heading.len() > 1 { 2 } else { 3 };
            for (n, text) in crate::guide::wrap(line, rect.width() - 24.0, 11.0)
                .iter()
                .take(room)
                .enumerate()
            {
                canvas.text(&TextRun::new(
                    text.clone(),
                    Point::new(rect.x0 + 12.0, rect.y0 + first + 14.0 * n as f64),
                    11.0,
                    theme.text_muted,
                ));
            }
        }
    }

    fn paint_check(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        target: Target,
        row: Rect,
        text: &str,
        on: bool,
    ) {
        self.paint_row_back(canvas, theme, target, row);
        let cy = row.center().y;
        let square = Rect::new(
            row.x0 + 8.0,
            cy - BOX / 2.0,
            row.x0 + 8.0 + BOX,
            cy + BOX / 2.0,
        );
        let shape = RoundedRect::from_rect(square, 3.0);
        canvas.stroke_with(shape, 1.0, theme.text_muted);
        if on {
            canvas.fill_with(
                RoundedRect::from_rect(square.inset(-3.0), 1.5),
                theme.highlight,
            );
        }
        let label = fit_label_in(canvas, text, row.width() - 40.0, 12.5);
        canvas.text(&TextRun::new(
            label,
            Point::new(square.x1 + 8.0, cy + 4.5),
            12.5,
            theme.text,
        ));
    }
}

/// The pin button of the recent row `row`: at its lower right.
pub(crate) fn pin_rect(row: Rect) -> Rect {
    Rect::new(
        row.x1 - 6.0 - PIN,
        row.y0 + 15.0,
        row.x1 - 6.0,
        row.y0 + 15.0 + PIN,
    )
}

/// A push pin in `button`: filled when the song is pinned, an outline when it is not.
fn paint_pin(canvas: &mut dyn Canvas, button: Rect, ink: Color, filled: bool) {
    let c = button.center();
    let parts = [
        Rect::new(c.x - 4.0, c.y - 8.0, c.x + 4.0, c.y - 5.0),
        Rect::new(c.x - 2.5, c.y - 5.0, c.x + 2.5, c.y + 1.0),
        Rect::new(c.x - 5.0, c.y + 1.0, c.x + 5.0, c.y + 2.5),
        Rect::new(c.x - 0.75, c.y + 2.5, c.x + 0.75, c.y + 7.0),
    ];
    for part in parts {
        if filled {
            canvas.fill_with(part, ink);
        } else {
            canvas.stroke_with(part, 1.0, ink);
        }
    }
}

#[cfg(test)]
#[path = "start_screen_tests.rs"]
mod tests;
