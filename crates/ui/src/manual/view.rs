//! The Manual panel: a table of contents on the left, the page on the right (or the search
//! results while a query is typed), a search field and Back and Forward buttons.
//!
//! ```text
//!  [<] [>]  [ Search the manual                                     3 results ]
//!  Documentation  | The main window
//!    The main window | - The transport bar. Play, Stop, Record ...
//!    Help Mode    |
//!    Launcher     |
//! ```
//!
//! A large floating panel the app places over the window, centred. It takes the keyboard while
//! it is open (every key is text for its field or a key of the page, so a typed letter never
//! reaches a shortcut), and a click outside it closes it. Esc clears the query, then closes.
//!
//! The page is laid out once per page and width ([`super::layout`]) with the canvas it is painted
//! on and cached; scrolling only moves the window over that list. A link inside the manual
//! moves there and is kept in the history (Back and Forward, or Alt+Left and Alt+Right); a link
//! to anything else is passed to the app as [`ManualAction::Open`].
//!
//! Colours come from the theme tokens; there is no italic face, so emphasis is drawn in a
//! softer accent tone, and code spans sit on the LCD colour.

use super::layout::{self, Draw, PageLayout, Tone};
use super::{Hit, Link, Location, Manual};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::display_list::DisplayList;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::paint::{fit_label_weight, measure};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use std::cell::RefCell;
use std::sync::Arc;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};
use vello::peniko::Color;

const PAD: f64 = 14.0;
const BAR_H: f64 = 44.0;
const FOOT_H: f64 = 22.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 1120.0;
const MAX_HEIGHT: f64 = 820.0;
const TOC_ROW: f64 = 22.0;
const RESULT_ROW: f64 = 54.0;
const COLUMN_PAD: f64 = 22.0;
const BAR_W: f64 = 8.0;
const BUTTON_W: f64 = 30.0;
const FONT: f32 = 11.0;
const SMALL: f32 = 9.5;
/// Most letters of a query.
const MAX_QUERY_CHARS: usize = 80;
/// How far the wheel scrolls the page for one notch.
const NOTCH: f64 = 51.0;
/// Space kept above a heading the view scrolls to.
const HEAD_ROOM: f64 = 8.0;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualAction {
    /// Close the panel.
    Close,
    /// Open something outside the manual: the target of a link as written in the text.
    Open(String),
}

/// One stop of the history: a place and the scroll it was left at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stop {
    at: Location,
    scroll: f64,
}

/// The Manual panel. See the module documentation.
#[derive(Debug)]
pub struct ManualView {
    manual: Arc<Manual>,
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    field: TextField,
    hits: Vec<Hit>,
    selected: usize,
    /// First result row shown.
    first_hit: usize,
    page: usize,
    scroll: f64,
    /// A heading (block of the page) the view keeps at the top until the user scrolls.
    want: Option<usize>,
    toc_scroll: f64,
    history: Vec<Stop>,
    cursor: usize,
    cache: RefCell<Option<(Arc<PageLayout>, bool)>>,
    dragging_field: bool,
    /// Where on the thumb the scroll bar was grabbed.
    dragging_bar: Option<f64>,
    hover: Option<Point>,
    repaint: bool,
}

impl ManualView {
    /// A panel over `manual`, open at its start. Call [`Self::set_bounds`].
    pub fn new(theme: Theme, manual: Arc<Manual>) -> Self {
        let start = manual.start();
        let mut view = Self {
            manual,
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 700.0),
            scale: 1.0,
            field: TextField::new("", MAX_QUERY_CHARS, false),
            hits: Vec::new(),
            selected: 0,
            first_hit: 0,
            page: start.page,
            scroll: 0.0,
            want: None,
            toc_scroll: 0.0,
            history: vec![Stop {
                at: start,
                scroll: 0.0,
            }],
            cursor: 0,
            cache: RefCell::new(None),
            dragging_field: false,
            dragging_bar: None,
            hover: None,
            repaint: true,
        };
        view.show(start);
        view
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). It is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// The manual it shows.
    pub fn manual(&self) -> &Manual {
        &self.manual
    }

    // ------------------------------------------------------------------ state

    /// The text typed in the search field.
    pub fn query(&self) -> &str {
        self.field.text()
    }

    /// Types `text` in place of the query (the app's tests and the headless driver).
    pub fn set_query(&mut self, text: &str) {
        self.field.set_text(text);
        self.requery();
    }

    /// Whether the search results are showing (a query is typed).
    pub fn searching(&self) -> bool {
        !self.field.text().trim().is_empty()
    }

    /// The search results, best first.
    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// The highlighted result.
    pub fn selected(&self) -> Option<usize> {
        (!self.hits.is_empty()).then_some(self.selected)
    }

    /// The page shown.
    pub fn page(&self) -> usize {
        self.page
    }

    /// Where the history stands: the page and the heading the view was sent to.
    pub fn current(&self) -> Location {
        self.history[self.cursor].at
    }

    /// The title of the section shown.
    pub fn title(&self) -> String {
        self.manual
            .heading_at(self.current())
            .map(|h| h.1)
            .or_else(|| self.manual.pages().get(self.page).map(|p| p.title.clone()))
            .unwrap_or_default()
    }

    /// Whether Back has somewhere to go.
    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    /// Whether Forward has somewhere to go.
    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    /// The scroll of the page, in pixels from its top.
    pub fn scroll(&self) -> f64 {
        let layout = self.layout(None);
        self.top(&layout)
    }

    /// The height of the page laid out for the current width.
    pub fn page_height(&self) -> f64 {
        self.layout(None).height
    }

    fn requery(&mut self) {
        let q = self.field.text().trim().to_owned();
        self.hits = self.manual.search(&q);
        self.selected = 0;
        self.first_hit = 0;
        self.repaint = true;
    }

    /// Goes to `at`, as a link does: the place is added to the history.
    pub fn goto(&mut self, at: Location) {
        let scroll_now = self.scroll();
        if let Some(stop) = self.history.get_mut(self.cursor) {
            stop.scroll = scroll_now;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(Stop { at, scroll: 0.0 });
        self.cursor = self.history.len() - 1;
        self.show(at);
        self.field.set_text("");
        self.requery();
    }

    /// Goes to the section called `title` (see [`Manual::find_heading`]); false when there is
    /// none.
    pub fn goto_section(&mut self, title: &str) -> bool {
        match self.manual.find_heading(title) {
            Some(at) => {
                self.goto(at);
                true
            }
            None => false,
        }
    }

    /// Goes back one stop of the history.
    pub fn back(&mut self) {
        self.step(false);
    }

    /// Goes forward one stop of the history.
    pub fn forward(&mut self) {
        self.step(true);
    }

    fn step(&mut self, forward: bool) {
        let target = if forward {
            (self.cursor + 1 < self.history.len()).then_some(self.cursor + 1)
        } else {
            self.cursor.checked_sub(1)
        };
        let Some(target) = target else { return };
        let scroll_now = self.scroll();
        self.history[self.cursor].scroll = scroll_now;
        self.cursor = target;
        let stop = self.history[target];
        self.show(stop.at);
        // The stop remembers where the page was left; a stop never left starts at its heading.
        if stop.scroll > 0.0 {
            self.want = None;
            self.scroll = stop.scroll;
            self.clamp_scroll();
        }
        self.field.set_text("");
        self.requery();
    }

    /// Shows the page of `at`, with its heading at the top.
    fn show(&mut self, at: Location) {
        let last = self.manual.pages().len().saturating_sub(1);
        self.page = at.page.min(last);
        self.scroll = 0.0;
        self.want = (at.block > 0).then_some(at.block);
        *self.cache.borrow_mut() = None;
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let w = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let h = (self.bounds.height() - 2.0 * MARGIN).clamp(1.0, MAX_HEIGHT);
        let x0 = self.bounds.x0 + (self.bounds.width() - w) / 2.0;
        let y0 = self.bounds.y0 + (self.bounds.height() - h) / 2.0;
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// The Back button.
    pub fn back_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x0 + PAD,
            p.y0 + 9.0,
            p.x0 + PAD + BUTTON_W,
            p.y0 + BAR_H - 9.0,
        )
    }

    /// The Forward button.
    pub fn forward_rect(&self) -> Rect {
        let b = self.back_rect();
        Rect::new(b.x1 + 6.0, b.y0, b.x1 + 6.0 + BUTTON_W, b.y1)
    }

    /// The search field.
    pub fn field_rect(&self) -> Rect {
        let p = self.panel();
        let f = self.forward_rect();
        Rect::new(f.x1 + 12.0, p.y0 + 7.0, p.x1 - PAD, p.y0 + BAR_H - 7.0)
    }

    /// The table of contents column.
    pub fn toc_rect(&self) -> Rect {
        let p = self.panel();
        let w = (p.width() * 0.25).clamp(140.0, 250.0).min(p.width() / 2.0);
        Rect::new(p.x0, p.y0 + BAR_H, p.x0 + w, p.y1 - FOOT_H)
    }

    /// The page (or results) area.
    pub fn page_rect(&self) -> Rect {
        let p = self.panel();
        let t = self.toc_rect();
        Rect::new(t.x1, t.y0, p.x1, t.y1)
    }

    /// The left edge and the width of the text column.
    fn column(&self) -> (f64, f64) {
        let a = self.page_rect();
        let x0 = a.x0 + COLUMN_PAD;
        (x0, (a.x1 - BAR_W - COLUMN_PAD * 0.5 - x0).max(40.0))
    }

    fn bar_track(&self) -> Rect {
        let a = self.page_rect();
        Rect::new(a.x1 - BAR_W - 2.0, a.y0 + 4.0, a.x1 - 2.0, a.y1 - 4.0)
    }

    // ------------------------------------------------------------------ layout and scroll

    fn layout(&self, canvas: Option<&mut dyn Canvas>) -> Arc<PageLayout> {
        let width = self.column().1.round().max(80.0);
        if let Some((l, real)) = &*self.cache.borrow()
            && l.page == self.page
            && (l.width - width).abs() < 0.5
            && (*real || canvas.is_none())
        {
            return l.clone();
        }
        let blocks = self
            .manual
            .pages()
            .get(self.page)
            .map_or(&[][..], |p| p.blocks.as_slice());
        let real = canvas.is_some();
        let layout = match canvas {
            Some(c) => layout::layout_page(c, self.page, blocks, width),
            None => layout::layout_page(&mut DisplayList::new(), self.page, blocks, width),
        };
        let layout = Arc::new(layout);
        *self.cache.borrow_mut() = Some((layout.clone(), real));
        layout
    }

    fn max_scroll(&self, layout: &PageLayout) -> f64 {
        (layout.height - self.page_rect().height()).max(0.0)
    }

    /// The scroll of the page: the heading asked for, or where the user scrolled to.
    fn top(&self, layout: &PageLayout) -> f64 {
        let raw = match self.want.and_then(|b| layout.headings.get(&b)) {
            Some(y) => y - HEAD_ROOM,
            None => self.scroll,
        };
        raw.clamp(0.0, self.max_scroll(layout))
    }

    fn scroll_to(&mut self, value: f64) {
        let layout = self.layout(None);
        self.want = None;
        self.scroll = value.clamp(0.0, self.max_scroll(&layout));
        self.repaint = true;
    }

    fn scroll_by(&mut self, delta: f64) {
        let now = self.scroll();
        self.scroll_to(now + delta);
    }

    fn clamp_scroll(&mut self) {
        let now = self.scroll;
        self.scroll_to(now);
    }

    /// The box of the scroll bar thumb (`None` when the page fits).
    pub fn thumb_rect(&self) -> Option<Rect> {
        let layout = self.layout(None);
        let max = self.max_scroll(&layout);
        if max <= 0.0 {
            return None;
        }
        let track = self.bar_track();
        let h = (track.height() * self.page_rect().height() / layout.height).max(24.0);
        let y = track.y0 + (track.height() - h) * (self.top(&layout) / max);
        Some(Rect::new(track.x0, y, track.x1, y + h))
    }

    fn toc_max(&self) -> f64 {
        (self.manual.toc().len() as f64 * TOC_ROW - self.toc_rect().height()).max(0.0)
    }

    /// The box of table of contents row `i`, `None` when it is scrolled out of view.
    pub fn toc_row_rect(&self, i: usize) -> Option<Rect> {
        if i >= self.manual.toc().len() {
            return None;
        }
        let area = self.toc_rect();
        let y = area.y0 + 4.0 + i as f64 * TOC_ROW - self.toc_scroll;
        (y >= area.y0 - 0.5 && y + TOC_ROW <= area.y1 + 0.5)
            .then(|| Rect::new(area.x0, y, area.x1, y + TOC_ROW))
    }

    /// The centre of table of contents row `i`, for tests and the headless driver.
    pub fn toc_row_center(&self, i: usize) -> Option<Point> {
        self.toc_row_rect(i).map(|r| r.center())
    }

    fn toc_at(&self, pos: Point) -> Option<usize> {
        (0..self.manual.toc().len())
            .find(|i| self.toc_row_rect(*i).is_some_and(|r| r.contains(pos)))
    }

    fn visible_hits(&self) -> usize {
        ((self.page_rect().height() / RESULT_ROW).floor() as usize).max(1)
    }

    /// The box of result row `row`, `None` when it is scrolled out of view.
    pub fn result_rect(&self, row: usize) -> Option<Rect> {
        if row < self.first_hit || row >= self.hits.len() {
            return None;
        }
        let a = self.page_rect();
        let y = a.y0 + (row - self.first_hit) as f64 * RESULT_ROW;
        (y + RESULT_ROW <= a.y1 + 0.5).then(|| Rect::new(a.x0, y, a.x1, y + RESULT_ROW))
    }

    /// The centre of result row `row`, for tests and the headless driver.
    pub fn result_center(&self, row: usize) -> Option<Point> {
        self.result_rect(row).map(|r| r.center())
    }

    fn result_at(&self, pos: Point) -> Option<usize> {
        (self.first_hit..self.hits.len())
            .find(|i| self.result_rect(*i).is_some_and(|r| r.contains(pos)))
    }

    fn show_selected(&mut self) {
        let visible = self.visible_hits();
        if self.selected < self.first_hit {
            self.first_hit = self.selected;
        } else if self.selected >= self.first_hit + visible {
            self.first_hit = self.selected + 1 - visible;
        }
        self.first_hit = self.first_hit.min(self.hits.len().saturating_sub(visible));
    }

    /// The panel position of the first link of the page whose target is `target`, for tests and
    /// the headless driver (`None` when it is not in view).
    pub fn link_center(&self, target: &str) -> Option<Point> {
        let layout = self.layout(None);
        let top = self.top(&layout);
        let (x0, _) = self.column();
        let a = self.page_rect();
        layout
            .links
            .iter()
            .filter(|l| l.target == target)
            .map(|l| {
                let c = l.rect.center();
                Point::new(x0 + c.x, a.y0 + c.y - top)
            })
            .find(|p| a.contains(*p))
    }

    /// The targets of the links of the page, in reading order.
    pub fn link_targets(&self) -> Vec<String> {
        self.layout(None)
            .links
            .iter()
            .map(|l| l.target.clone())
            .collect()
    }

    fn link_at(&self, pos: Point) -> Option<String> {
        if !self.page_rect().contains(pos) {
            return None;
        }
        let layout = self.layout(None);
        let top = self.top(&layout);
        let (x0, _) = self.column();
        let local = Point::new(pos.x - x0, pos.y - self.page_rect().y0 + top);
        layout
            .links
            .iter()
            .find(|l| l.rect.contains(local))
            .map(|l| l.target.clone())
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if self.dragging_field {
                    self.field.drag(*pos);
                }
                if let Some(grab) = self.dragging_bar {
                    self.drag_bar(pos.y, grab);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                self.dragging_field = false;
                self.dragging_bar = None;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                out.extend(self.press(*pos, *clicks, modifiers.shift));
            }
            UiEvent::PointerDown { pos, .. } => {
                // Any other button outside the panel also gives way.
                if !self.panel().contains(*pos) {
                    out.push(act(ManualAction::Close));
                }
            }
            UiEvent::Wheel { delta, pos, .. } => self.wheel(delta.y, *pos),
            UiEvent::Ime(ime) => {
                if self.field.handle_ime(ime) {
                    self.requery();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => out.extend(self.key(key, *modifiers)),
            _ => {}
        }
        out
    }

    fn press(&mut self, pos: Point, clicks: u8, shift: bool) -> Vec<UiAction> {
        if !self.panel().contains(pos) {
            return vec![act(ManualAction::Close)];
        }
        if self.field_rect().contains(pos) {
            self.dragging_field = true;
            self.field.press(pos, clicks, shift);
            return Vec::new();
        }
        if self.back_rect().contains(pos) {
            self.back();
            return Vec::new();
        }
        if self.forward_rect().contains(pos) {
            self.forward();
            return Vec::new();
        }
        if let Some(i) = self.toc_at(pos) {
            let at = self.manual.toc()[i].at;
            self.goto(at);
            return Vec::new();
        }
        if self.searching() {
            if let Some(row) = self.result_at(pos) {
                self.selected = row;
                self.open_selected();
            }
            return Vec::new();
        }
        if let Some(thumb) = self.thumb_rect()
            && self.page_rect().contains(pos)
            && pos.x >= self.bar_track().x0 - 2.0
        {
            if thumb.contains(pos) {
                self.dragging_bar = Some(pos.y - thumb.y0);
            } else {
                let page = self.page_rect().height() - 40.0;
                self.scroll_by(if pos.y < thumb.y0 { -page } else { page });
            }
            return Vec::new();
        }
        match self.link_at(pos) {
            Some(target) => self.follow(&target),
            None => Vec::new(),
        }
    }

    fn follow(&mut self, target: &str) -> Vec<UiAction> {
        match self.manual.resolve(self.page, target) {
            Link::Here(at) => {
                self.goto(at);
                Vec::new()
            }
            Link::Outside(t) => vec![act(ManualAction::Open(t))],
        }
    }

    /// Follows the link `target` of the page as a click on it does.
    pub fn open_link(&mut self, target: &str) -> Vec<UiAction> {
        self.follow(target)
    }

    fn open_selected(&mut self) {
        if let Some(hit) = self.hits.get(self.selected) {
            let at = hit.at;
            self.goto(at);
        }
    }

    fn drag_bar(&mut self, y: f64, grab: f64) {
        let layout = self.layout(None);
        let max = self.max_scroll(&layout);
        let track = self.bar_track();
        let h = (track.height() * self.page_rect().height() / layout.height).max(24.0);
        let room = (track.height() - h).max(1.0);
        let frac = ((y - grab - track.y0) / room).clamp(0.0, 1.0);
        self.scroll_to(frac * max);
    }

    fn wheel(&mut self, dy: f64, pos: Point) {
        if dy == 0.0 {
            return;
        }
        if self.toc_rect().contains(pos) {
            self.toc_scroll = (self.toc_scroll - dy * TOC_ROW * 2.0).clamp(0.0, self.toc_max());
        } else if self.page_rect().contains(pos) {
            if self.searching() {
                let rows = (dy.abs().ceil() as usize).max(1);
                self.first_hit = if dy > 0.0 {
                    self.first_hit.saturating_sub(rows)
                } else {
                    self.first_hit + rows
                };
                self.first_hit = self
                    .first_hit
                    .min(self.hits.len().saturating_sub(self.visible_hits()));
            } else {
                self.scroll_by(-dy * NOTCH);
            }
        }
        self.repaint = true;
    }

    fn key(&mut self, key: &Key, m: Modifiers) -> Vec<UiAction> {
        self.repaint = true;
        let searching = self.searching();
        let page = (self.page_rect().height() - 40.0).max(40.0);
        match key {
            Key::Escape => {
                if self.field.text().is_empty() {
                    return vec![act(ManualAction::Close)];
                }
                self.field.set_text("");
                self.requery();
            }
            Key::Left if m.alt && !m.command() => self.back(),
            Key::Right if m.alt && !m.command() => self.forward(),
            Key::Enter if searching => self.open_selected(),
            Key::Up if searching && !m.command() => {
                self.selected = self.selected.saturating_sub(1);
                self.show_selected();
            }
            Key::Down if searching && !m.command() => {
                self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
                self.show_selected();
            }
            Key::PageUp if searching => {
                self.selected = self.selected.saturating_sub(self.visible_hits());
                self.show_selected();
            }
            Key::PageDown if searching => {
                self.selected =
                    (self.selected + self.visible_hits()).min(self.hits.len().saturating_sub(1));
                self.show_selected();
            }
            Key::Up if !m.command() => self.scroll_by(-34.0),
            Key::Down if !m.command() => self.scroll_by(34.0),
            Key::PageUp => self.scroll_by(-page),
            Key::PageDown => self.scroll_by(page),
            Key::Home if !searching && !m.command() && self.field.text().is_empty() => {
                self.scroll_to(0.0);
            }
            Key::End if !searching && !m.command() && self.field.text().is_empty() => {
                self.scroll_to(f64::MAX);
            }
            _ => {
                let before = self.field.text().to_owned();
                if self.field.handle_key(key, m).handled() && self.field.text() != before {
                    self.requery();
                }
            }
        }
        Vec::new()
    }

    // ------------------------------------------------------------------ painting

    fn tone(&self, tone: Tone) -> Color {
        let st = &self.st;
        let t = &st.base;
        match tone {
            Tone::Ink => t.text,
            Tone::Muted | Tone::Marker => t.text_muted,
            Tone::Heading => t.highlight,
            Tone::Link => t.focus,
            Tone::Code => t.text,
            Tone::Italic => mix(t.text, t.highlight, 0.5),
            Tone::CodeBack => t.lcd,
            Tone::HeadBack => t.plate,
            Tone::Rule => st.separator,
        }
    }

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        self.paint_bar(c);
        self.paint_toc(c);
        let area = self.page_rect();
        c.push_clip(&area.into());
        if self.searching() {
            self.paint_results(c);
        } else {
            self.paint_page(c);
        }
        c.pop_clip();
        sp::line(
            c,
            p.x0,
            p.y1 - FOOT_H + 0.5,
            p.x1,
            p.y1 - FOOT_H + 0.5,
            st.separator,
        );
        sp::label(
            c,
            "Up and Down scroll, Alt+Left and Alt+Right go back and forward, Esc closes",
            p.x0 + PAD,
            p.y1 - FOOT_H + 15.0,
            SMALL,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_bar(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hover = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        let back = self.back_rect();
        let fwd = self.forward_rect();
        sp::button(c, st, back, "<", None, self.can_back() && hover(back));
        sp::button(c, st, fwd, ">", None, self.can_forward() && hover(fwd));
        if !self.can_back() {
            sp::rect(c, back, st.background.with_alpha(0.55));
        }
        if !self.can_forward() {
            sp::rect(c, fwd, st.background.with_alpha(0.55));
        }
        let field = self.field_rect();
        sp::lcd(c, st, field, Some(t.focus));
        if self.field.is_empty() && self.field.preedit().is_none() {
            sp::label(
                c,
                "Search the manual",
                field.x0 + 8.0,
                field.y0 + 17.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        let colors = FieldColors {
            text: t.text,
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        };
        self.field
            .paint(c, field, FONT, field.height() - 6.0, colors, true);
        if self.searching() {
            let n = self.hits.len();
            let count = if n == 1 {
                "1 result".to_owned()
            } else {
                format!("{n} results")
            };
            sp::label(
                c,
                &count,
                field.x1 - 8.0,
                field.y0 + 17.0,
                SMALL,
                t.text_muted,
                Align::End,
            );
        }
        sp::line(
            c,
            p.x0,
            p.y0 + BAR_H - 0.5,
            p.x1,
            p.y0 + BAR_H - 0.5,
            st.separator,
        );
    }

    fn paint_toc(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let area = self.toc_rect();
        sp::rect(c, area, st.header);
        sp::line(
            c,
            area.x1 - 0.5,
            area.y0,
            area.x1 - 0.5,
            area.y1,
            st.separator,
        );
        c.push_clip(&area.into());
        let current = self.manual.toc_index_of(self.current());
        for (i, entry) in self.manual.toc().iter().enumerate() {
            let Some(r) = self.toc_row_rect(i) else {
                continue;
            };
            if Some(i) == current {
                sp::rect(c, r, t.selection.with_alpha(0.3));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, t.selection.with_alpha(0.15));
            }
            let indent = f64::from(entry.level.saturating_sub(1)) * 12.0;
            let weight = if entry.level == 1 { 700 } else { 400 };
            let ink = if entry.level == 1 {
                t.text
            } else {
                mix(t.text, t.text_muted, 0.25)
            };
            let width = r.width() - 2.0 * 10.0 - indent;
            let shown = fit_label_weight(c, &entry.title, width, FONT, weight);
            c.text(
                &TextRun::new(
                    shown,
                    Point::new(r.x0 + 10.0 + indent, r.y0 + 15.0),
                    FONT,
                    ink,
                )
                .weight(weight),
            );
        }
        c.pop_clip();
    }

    fn paint_page(&self, c: &mut dyn Canvas) {
        let area = self.page_rect();
        let layout = self.layout(Some(&mut *c));
        let top = self.top(&layout);
        let (x0, _) = self.column();
        c.push_clip(&area.into());
        c.push_transform(Affine::translate((x0, area.y0 - top)));
        let (lo, hi) = (top - 24.0, top + area.height() + 24.0);
        for draw in &layout.draws {
            let (a, b) = draw.span();
            if b < lo || a > hi {
                continue;
            }
            self.paint_draw(c, draw);
        }
        c.pop_transform();
        c.pop_clip();
        if let Some(thumb) = self.thumb_rect() {
            let track = self.bar_track();
            pill(c, track, self.st.separator.with_alpha(0.35));
            let lit = self.dragging_bar.is_some() || self.hover.is_some_and(|h| thumb.contains(h));
            let col = self
                .st
                .base
                .text_muted
                .with_alpha(if lit { 0.9 } else { 0.55 });
            pill(c, thumb, col);
        }
    }

    fn paint_draw(&self, c: &mut dyn Canvas, draw: &Draw) {
        match draw {
            Draw::Text {
                x,
                y,
                text,
                size,
                weight,
                tone,
            } => c.text(
                &TextRun::new(text.as_str(), Point::new(*x, *y), *size, self.tone(*tone))
                    .weight(*weight),
            ),
            Draw::Fill { rect, tone } => {
                let colour = self.tone(*tone);
                if matches!(tone, Tone::CodeBack) {
                    c.fill(&RoundedRect::from_rect(*rect, 2.0).into(), &colour.into());
                } else {
                    sp::rect(c, *rect, colour);
                }
            }
            Draw::Line {
                x0,
                y0,
                x1,
                y1,
                tone,
            } => sp::line(c, *x0, *y0, *x1, *y1, self.tone(*tone)),
        }
    }

    fn paint_results(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let a = self.page_rect();
        if self.hits.is_empty() {
            sp::label(
                c,
                "Nothing in the manual matches.",
                a.x0 + COLUMN_PAD,
                a.y0 + 28.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
            return;
        }
        for row in self.first_hit..self.hits.len() {
            let Some(r) = self.result_rect(row) else {
                break;
            };
            let hit = &self.hits[row];
            if row == self.selected {
                sp::rect(c, r, t.selection.with_alpha(0.35));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, t.selection.with_alpha(0.15));
            }
            let x = r.x0 + COLUMN_PAD;
            let doc_w = measure(c, &hit.doc, SMALL, 400);
            sp::label(
                c,
                &hit.doc,
                r.x1 - COLUMN_PAD,
                r.y0 + 20.0,
                SMALL,
                t.text_muted,
                Align::End,
            );
            marked(
                c,
                &hit.title,
                &hit.title_hits,
                (x, r.y0 + 20.0),
                r.width() - 2.0 * COLUMN_PAD - doc_w - 12.0,
                (FONT + 1.0, 700),
                (t.text, t.highlight),
            );
            marked(
                c,
                &hit.snippet,
                &hit.snippet_hits,
                (x, r.y0 + 40.0),
                r.width() - 2.0 * COLUMN_PAD,
                (FONT - 0.5, 400),
                (t.text_muted, t.highlight),
            );
            sp::line(
                c,
                r.x0,
                r.y1 - 0.5,
                r.x1,
                r.y1 - 0.5,
                st.separator.with_alpha(0.5),
            );
        }
    }
}

/// A filled pill (a scroll bar part).
fn pill(c: &mut dyn Canvas, r: Rect, colour: Color) {
    c.fill_with(RoundedRect::from_rect(r, r.width() / 2.0), colour);
}

fn act(a: ManualAction) -> UiAction {
    UiAction::Manual(a)
}

/// Draws `text` at `at` (left edge and baseline) cut to `width`, with the characters inside
/// `hits` (character ranges) bold in the second colour.
fn marked(
    c: &mut dyn Canvas,
    text: &str,
    hits: &[(usize, usize)],
    at: (f64, f64),
    width: f64,
    font: (f32, u16),
    colours: (Color, Color),
) {
    let (size, weight) = font;
    let shown = fit_label_weight(c, text, width, size, weight);
    let mut x = at.0;
    let mut run = String::new();
    let mut run_hit = false;
    let flush = |c: &mut dyn Canvas, run: &mut String, hit: bool, x: &mut f64| {
        if run.is_empty() {
            return;
        }
        let (colour, w) = if hit {
            (colours.1, 700)
        } else {
            (colours.0, weight)
        };
        c.text(&TextRun::new(run.as_str(), Point::new(*x, at.1), size, colour).weight(w));
        *x += measure(c, run, size, w);
        run.clear();
    };
    for (i, ch) in shown.chars().enumerate() {
        let hit = hits.iter().any(|(a, b)| i >= *a && i < *b);
        if hit != run_hit {
            flush(c, &mut run, run_hit, &mut x);
            run_hit = hit;
        }
        run.push(ch);
    }
    flush(c, &mut run, run_hit, &mut x);
}
