//! The in-window menu bar (Windows and Linux).
//!
//! muda's Windows attach (`Menu::init_for_hwnd`) is `unsafe`, which the workspace denies, and
//! its Linux menus need a GTK window that winit does not create. So the app draws the same
//! [`Menu`] model itself on those platforms: one bar of titles, one dropdown at a time, drawn
//! through the [`Canvas`] in logical units. macOS uses native muda menus instead.
//!
//! A list taller than the window is cut to it and scrolls (mouse wheel, the strips at its two
//! ends, the arrow keys); a submenu opens beside its row, on the other side when the window ends
//! there, and may hold submenus of its own. The geometry is in [`atomik_ui::menu_layout`].

use crate::commands::{AppCommand, Menu, MenuEntry};
use crate::menu_layout::{
    self as geo, ARROW_SPEED, EDGE_MARGIN, ListLayout, WHEEL_STEP, natural_height,
};
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

/// Height of the bar, logical pixels.
pub const MENU_BAR_HEIGHT: f64 = 26.0;
pub use atomik_ui::menu_layout::{ITEM_HEIGHT, SEPARATOR_HEIGHT};
/// Approximate advance of one character of UI text at 13 px (layout estimate).
pub const CHAR_WIDTH: f64 = 7.2;
const TITLE_PAD: f64 = 12.0;
const DROPDOWN_MIN_WIDTH: f64 = 220.0;
const FONT: f32 = 13.0;

/// What the bar did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarOutcome {
    /// Not for the bar: pass it on.
    Ignored,
    /// Used by the bar (menu opened, closed, hovered…).
    Consumed,
    /// An item was chosen.
    Command(AppCommand),
}

/// Estimated width of a text run.
pub fn text_width(text: &str) -> f64 {
    text.chars().count() as f64 * CHAR_WIDTH
}

/// The width of a list of `entries`: the widest text with its shortcut and the room for the
/// marks, never less than the minimum.
fn list_width(entries: &[MenuEntry]) -> f64 {
    entries
        .iter()
        .map(|e| match e {
            MenuEntry::Item(it) => {
                text_width(&it.label)
                    + it.shortcut.as_deref().map_or(0.0, |s| text_width(s) + 32.0)
                    + 48.0
            }
            MenuEntry::Submenu(sub) => text_width(sub.label) + 48.0,
            MenuEntry::Separator => 0.0,
        })
        .fold(DROPDOWN_MIN_WIDTH, f64::max)
}

/// The height of each of `entries`.
fn row_heights(entries: &[MenuEntry]) -> Vec<f64> {
    entries
        .iter()
        .map(|e| geo::row_height(matches!(e, MenuEntry::Separator)))
        .collect()
}

/// Whether `entry` can be highlighted and chosen.
fn is_selectable(entry: &MenuEntry) -> bool {
    match entry {
        MenuEntry::Item(it) => it.enabled,
        MenuEntry::Submenu(_) => true,
        MenuEntry::Separator => false,
    }
}

/// The entry after (or before) `at` in `entries` that can be chosen, wrapping round. From
/// nothing, Down steps on to the first and Up to the last.
fn step(entries: &[MenuEntry], at: Option<usize>, down: bool) -> Option<usize> {
    let rows = entries.len();
    let mut at = at.unwrap_or(if down { rows.max(1) - 1 } else { 0 });
    for _ in 0..rows {
        at = if down {
            (at + 1) % rows
        } else {
            (at + rows - 1) % rows
        };
        if is_selectable(&entries[at]) {
            return Some(at);
        }
    }
    None
}

/// The first (or last) entry of `entries` that can be chosen.
fn edge(entries: &[MenuEntry], first: bool) -> Option<usize> {
    let mut rows = (0..entries.len()).filter(|r| is_selectable(&entries[*r]));
    if first { rows.next() } else { rows.next_back() }
}

/// A list that opens beside a row of the list before it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flyout {
    /// The row of the list it belongs to that opened it.
    row: usize,
    /// Its highlighted entry.
    hover: Option<usize>,
    /// How far it is scrolled.
    scroll: f64,
}

/// One open list with its place: the dropdown, then the flyouts one beside the other.
struct Open<'a> {
    entries: &'a [MenuEntry],
    layout: ListLayout,
}

/// The in-window menu bar state.
#[derive(Debug, Clone)]
pub struct MenuBar {
    open: Option<usize>,
    /// The highlighted title (nothing open) or the highlighted entry of the open dropdown.
    hover: Option<usize>,
    /// How far the open dropdown is scrolled.
    scroll: f64,
    /// The flyouts that are open, the first beside the dropdown.
    subs: Vec<Flyout>,
    pointer: Point,
    /// The window the menus must stay inside.
    window: Size,
    /// When the strips last scrolled the list under the pointer.
    last_tick: Option<f64>,
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            open: None,
            hover: None,
            scroll: 0.0,
            subs: Vec::new(),
            pointer: Point::ZERO,
            // Until the shell says how big the window is, the menus are not cut.
            window: Size::new(f64::INFINITY, f64::INFINITY),
            last_tick: None,
        }
    }
}

/// What the bar shows of the pointer and the keyboard: the open menu, its highlighted entry,
/// the first open submenu and its highlighted entry.
pub type BarLook = (Option<usize>, Option<usize>, Option<usize>, Option<usize>);

impl MenuBar {
    /// A closed bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tells the bar how big the window is: menus stay inside it.
    pub fn set_window(&mut self, size: Size) {
        self.window = size;
    }

    /// The open menu, if any.
    pub fn open_menu(&self) -> Option<usize> {
        self.open
    }

    /// Whether a dropdown is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Closes any dropdown.
    pub fn close(&mut self) {
        self.open = None;
        self.hover = None;
        self.scroll = 0.0;
        self.subs.clear();
        self.last_tick = None;
    }

    /// The title rectangles, left to right.
    pub fn title_rects(menus: &[Menu]) -> Vec<Rect> {
        let mut x = 4.0;
        menus
            .iter()
            .map(|m| {
                let w = text_width(m.title) + 2.0 * TITLE_PAD;
                let r = Rect::new(x, 0.0, x + w, MENU_BAR_HEIGHT);
                x += w;
                r
            })
            .collect()
    }

    /// The dropdown of menu `i` in a window `window` big, scrolled by `offset`.
    pub fn dropdown_in(menus: &[Menu], i: usize, window: Size, offset: f64) -> Option<ListLayout> {
        let menu = menus.get(i)?;
        let title = *Self::title_rects(menus).get(i)?;
        let width = list_width(&menu.entries);
        let x0 = geo::place_dropdown_x(title.x0, width, window.width);
        let heights = row_heights(&menu.entries);
        let (top, height) = geo::fit_vertical(
            MENU_BAR_HEIGHT,
            natural_height(&heights),
            MENU_BAR_HEIGHT,
            window.height - EDGE_MARGIN,
            false,
        );
        Some(geo::layout(&heights, x0, x0 + width, top, height, offset))
    }

    /// The flyout of the submenu at `row` of the list `parent` (which holds `parent_entries`):
    /// beside the row, moved up or to the other side to stay in the window, scrolled by `offset`.
    pub fn flyout_in<'a>(
        parent: &ListLayout,
        parent_entries: &'a [MenuEntry],
        row: usize,
        window: Size,
        offset: f64,
    ) -> Option<(ListLayout, &'a [MenuEntry])> {
        let MenuEntry::Submenu(sub) = parent_entries.get(row)? else {
            return None;
        };
        let drawn = *parent.drawn.get(row)?;
        let width = list_width(&sub.entries);
        let x0 = geo::place_flyout_x(parent.panel, width, window.width);
        let heights = row_heights(&sub.entries);
        let (top, height) = geo::fit_vertical(
            drawn.y0 - 4.0,
            natural_height(&heights),
            MENU_BAR_HEIGHT,
            window.height - EDGE_MARGIN,
            true,
        );
        let layout = geo::layout(&heights, x0, x0 + width, top, height, offset);
        Some((layout, &sub.entries))
    }

    /// The dropdown of menu `i` and the rectangle of each entry (separators included), in a
    /// window that has room for all of it.
    pub fn dropdown(menus: &[Menu], i: usize) -> Option<(Rect, Vec<Rect>)> {
        let l = Self::dropdown_in(menus, i, Size::new(f64::INFINITY, f64::INFINITY), 0.0)?;
        Some((l.panel, l.rows))
    }

    /// The flyout of the submenu at `row` of menu `menu` and the rectangle of each of its
    /// entries: to the right of the dropdown, level with the row, in a window that has room for
    /// all of it. `None` when that entry is not a submenu.
    pub fn flyout(menus: &[Menu], menu: usize, row: usize) -> Option<(Rect, Vec<Rect>)> {
        let window = Size::new(f64::INFINITY, f64::INFINITY);
        let dd = Self::dropdown_in(menus, menu, window, 0.0)?;
        let (l, _) = Self::flyout_in(&dd, &menus.get(menu)?.entries, row, window, 0.0)?;
        Some((l.panel, l.rows))
    }

    /// The open dropdown and its flyouts, each with its entries and place, for a window
    /// `window` big.
    fn chain<'a>(&self, menus: &'a [Menu], window: Size) -> Vec<Open<'a>> {
        let mut out = Vec::new();
        let Some(open) = self.open else { return out };
        let Some(menu) = menus.get(open) else {
            return out;
        };
        let Some(layout) = Self::dropdown_in(menus, open, window, self.scroll) else {
            return out;
        };
        out.push(Open {
            entries: &menu.entries,
            layout,
        });
        for sub in &self.subs {
            let parent = &out[out.len() - 1];
            let Some((layout, entries)) =
                Self::flyout_in(&parent.layout, parent.entries, sub.row, window, sub.scroll)
            else {
                break;
            };
            out.push(Open { entries, layout });
        }
        out
    }

    /// The highlighted entry of list `level` (0 is the dropdown).
    fn lit(&self, level: usize) -> Option<usize> {
        if level == 0 {
            self.hover
        } else {
            self.subs.get(level - 1).and_then(|s| s.hover)
        }
    }

    fn set_lit(&mut self, level: usize, row: Option<usize>) {
        if level == 0 {
            self.hover = row;
        } else if let Some(s) = self.subs.get_mut(level - 1) {
            s.hover = row;
        }
    }

    fn set_scroll(&mut self, level: usize, offset: f64) {
        if level == 0 {
            self.scroll = offset;
        } else if let Some(s) = self.subs.get_mut(level - 1) {
            s.scroll = offset;
        }
    }

    /// Opens the submenu at `row` of list `level` (closing what was open beside it) unless it is
    /// open already.
    fn open_flyout(&mut self, level: usize, row: usize) {
        if self.subs.get(level).is_some_and(|s| s.row == row) {
            self.subs.truncate(level + 1);
            return;
        }
        self.subs.truncate(level);
        self.subs.push(Flyout {
            row,
            hover: None,
            scroll: 0.0,
        });
    }

    /// Opens the submenu at `row` of list `level` from the keyboard: its first choosable entry
    /// is lit.
    fn open_flyout_keyed(&mut self, menus: &[Menu], level: usize, row: usize) {
        self.subs.truncate(level);
        self.subs.push(Flyout {
            row,
            hover: None,
            scroll: 0.0,
        });
        let first =
            self.chain(menus, self.window)
                .get(level)
                .and_then(|o| match o.entries.get(row) {
                    Some(MenuEntry::Submenu(s)) => edge(&s.entries, true),
                    _ => None,
                });
        self.set_lit(level + 1, first);
    }

    /// The deepest list with a highlighted entry: where the arrow keys move. A flyout the
    /// pointer opened without lighting anything has no say.
    fn focus(&self) -> usize {
        match self.subs.last() {
            Some(s) if s.hover.is_some() => self.subs.len(),
            Some(_) => self.subs.len() - 1,
            None => 0,
        }
    }

    /// What the bar shows of the pointer: the open menu and the highlighted title or entry,
    /// and the same for the first open submenu. A move that leaves it as it was changes nothing
    /// on screen.
    pub fn look(&self) -> BarLook {
        (
            self.open,
            self.hover,
            self.subs.first().map(|s| s.row),
            self.subs.first().and_then(|s| s.hover),
        )
    }

    /// What the bar shows of the submenus beyond the first: the row that opened each and the
    /// entry lit in it (empty when there are none).
    pub fn deep_look(&self) -> Vec<(usize, Option<usize>)> {
        self.subs.iter().skip(1).map(|s| (s.row, s.hover)).collect()
    }

    /// How far the open dropdown is scrolled, pixels (0 when it fits the window).
    pub fn dropdown_scroll(&self) -> f64 {
        self.scroll
    }

    /// How far the flyout `depth` levels in (1 is the first) is scrolled; 0 when there is none.
    pub fn flyout_scroll(&self, depth: usize) -> f64 {
        depth
            .checked_sub(1)
            .and_then(|d| self.subs.get(d))
            .map_or(0.0, |s| s.scroll)
    }

    /// The highlighted entry of the open dropdown: the pointer's and the keyboard's.
    pub fn highlighted_item(&self) -> Option<usize> {
        self.open.and(self.hover)
    }

    fn title_at(menus: &[Menu], p: Point) -> Option<usize> {
        Self::title_rects(menus).iter().position(|r| r.contains(p))
    }

    /// The deepest open list whose panel holds `p`, and the entry under it.
    fn list_at(&self, menus: &[Menu], p: Point) -> Option<(usize, Option<usize>)> {
        let chain = self.chain(menus, self.window);
        let level = chain.iter().rposition(|o| o.layout.panel.contains(p))?;
        let o = &chain[level];
        let row = o
            .layout
            .rows
            .iter()
            .position(|r| r.contains(p))
            .filter(|i| matches!(o.entries[*i], MenuEntry::Item(_) | MenuEntry::Submenu(_)));
        Some((level, row))
    }

    fn in_dropdown(&self, menus: &[Menu], p: Point) -> bool {
        self.list_at(menus, p).is_some()
    }

    /// Lights what the pointer is on, opens the submenu it rests on and closes the ones it left.
    fn hover_at(&mut self, menus: &[Menu], pos: Point) {
        let Some((level, row)) = self.list_at(menus, pos) else {
            self.hover = None;
            return;
        };
        // The rows that opened the lists up to this one stay lit.
        for k in 0..level {
            let opened = self.subs.get(k).map(|s| s.row);
            self.set_lit(k, opened);
        }
        let chain = self.chain(menus, self.window);
        let on_strip = row.is_none()
            && (chain[level]
                .layout
                .up_strip()
                .is_some_and(|s| s.contains(pos))
                || chain[level]
                    .layout
                    .down_strip()
                    .is_some_and(|s| s.contains(pos)));
        if on_strip {
            self.set_lit(level, None);
            self.subs.truncate(level);
            return;
        }
        self.set_lit(level, row);
        match row {
            Some(i) if matches!(chain[level].entries[i], MenuEntry::Submenu(_)) => {
                // Resting on a submenu row opens it.
                self.open_flyout(level, i);
            }
            Some(_) => self.subs.truncate(level),
            None => {}
        }
    }

    /// Scrolls list `level` by `by` pixels (positive moves the list up, showing later rows).
    fn scroll_by(&mut self, menus: &[Menu], level: usize, by: f64) -> bool {
        let chain = self.chain(menus, self.window);
        let Some(o) = chain.get(level) else {
            return false;
        };
        let next = (o.layout.offset + by).clamp(0.0, o.layout.max_offset);
        if (next - o.layout.offset).abs() < 1e-9 {
            return false;
        }
        self.set_scroll(level, next);
        true
    }

    /// Keeps the highlighted entry of list `level` in view.
    fn reveal(&mut self, menus: &[Menu], level: usize) {
        let chain = self.chain(menus, self.window);
        let (Some(o), Some(row)) = (chain.get(level), self.lit(level)) else {
            return;
        };
        if !o.layout.scrolls() || row >= o.layout.drawn.len() {
            return;
        }
        let top = o.layout.content_top(row);
        let offset = geo::offset_to_show(
            o.layout.offset,
            o.layout.view.height(),
            o.layout.max_offset,
            top,
            top + o.layout.drawn[row].height(),
        );
        self.set_scroll(level, offset);
    }

    /// Scrolls the list whose strip the pointer rests on, at `now` seconds (any monotonic
    /// clock); `true` when something moved. The shell calls it every frame.
    pub fn tick(&mut self, now: f64, menus: &[Menu]) -> bool {
        let chain = self.chain(menus, self.window);
        let p = self.pointer;
        let strip = chain
            .iter()
            .rposition(|o| o.layout.panel.contains(p))
            .and_then(|level| {
                let l = &chain[level].layout;
                if l.up_strip().is_some_and(|s| s.contains(p)) {
                    Some((level, -1.0))
                } else if l.down_strip().is_some_and(|s| s.contains(p)) {
                    Some((level, 1.0))
                } else {
                    None
                }
            });
        let Some((level, dir)) = strip else {
            self.last_tick = None;
            return false;
        };
        let dt = self
            .last_tick
            .map_or(1.0 / 60.0, |t| (now - t).clamp(0.0, 0.1));
        self.last_tick = Some(now);
        self.scroll_by(menus, level, dir * ARROW_SPEED * dt)
    }

    /// Handles an event (logical window coordinates).
    pub fn handle(&mut self, event: &UiEvent, menus: &[Menu]) -> MenuBarOutcome {
        let in_bar = |p: Point| p.y >= 0.0 && p.y < MENU_BAR_HEIGHT;
        match event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = *pos;
                if self.open.is_some() {
                    if let Some(t) = Self::title_at(menus, *pos) {
                        if self.open != Some(t) {
                            self.subs.clear();
                            self.scroll = 0.0;
                        }
                        self.open = Some(t);
                    }
                    self.hover_at(menus, *pos);
                    MenuBarOutcome::Consumed
                } else if in_bar(*pos) {
                    let hover = Self::title_at(menus, *pos);
                    self.hover = hover;
                    MenuBarOutcome::Consumed
                } else {
                    if self.hover.take().is_some() {
                        return MenuBarOutcome::Consumed;
                    }
                    MenuBarOutcome::Ignored
                }
            }
            UiEvent::PointerDown { pos, button, .. } => {
                self.pointer = *pos;
                if let Some(t) = Self::title_at(menus, *pos) {
                    if *button == PointerButton::Primary {
                        self.open = if self.open == Some(t) { None } else { Some(t) };
                        self.hover = None;
                        self.scroll = 0.0;
                        self.subs.clear();
                    }
                    return MenuBarOutcome::Consumed;
                }
                if self.open.is_some() {
                    if self.in_dropdown(menus, *pos) {
                        return MenuBarOutcome::Consumed;
                    }
                    // A click outside closes the menu and is swallowed.
                    self.close();
                    return MenuBarOutcome::Consumed;
                }
                if in_bar(*pos) {
                    MenuBarOutcome::Consumed
                } else {
                    MenuBarOutcome::Ignored
                }
            }
            UiEvent::PointerUp { pos, button, .. } => {
                if self.open.is_none() {
                    return if in_bar(*pos) {
                        MenuBarOutcome::Consumed
                    } else {
                        MenuBarOutcome::Ignored
                    };
                };
                if *button != PointerButton::Primary {
                    return MenuBarOutcome::Consumed;
                }
                let Some((level, Some(row))) = self.list_at(menus, *pos) else {
                    return MenuBarOutcome::Consumed;
                };
                let chain = self.chain(menus, self.window);
                match &chain[level].entries[row] {
                    MenuEntry::Item(item) if item.enabled => {
                        let command = item.command;
                        self.close();
                        MenuBarOutcome::Command(command)
                    }
                    MenuEntry::Submenu(_) => {
                        // A click opens the submenu again if the pointer had left it.
                        self.open_flyout(level, row);
                        MenuBarOutcome::Consumed
                    }
                    _ => MenuBarOutcome::Consumed,
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                if self.open.is_some() {
                    self.pointer = *pos;
                    // The list under the pointer scrolls; a wheel notch up (positive) moves
                    // the list down.
                    if let Some((level, _)) = self.list_at(menus, *pos)
                        && self.scroll_by(menus, level, -delta.y * WHEEL_STEP)
                    {
                        self.hover_at(menus, *pos);
                    }
                    MenuBarOutcome::Consumed
                } else if in_bar(*pos) {
                    MenuBarOutcome::Consumed
                } else {
                    MenuBarOutcome::Ignored
                }
            }
            UiEvent::Key { key, pressed, .. } if self.open.is_some() => {
                if *pressed {
                    return self.key(key.clone(), menus);
                }
                MenuBarOutcome::Consumed
            }
            UiEvent::PointerLeave => {
                self.pointer = Point::new(-1.0e6, -1.0e6);
                self.last_tick = None;
                self.hover = if self.open.is_some() {
                    self.hover
                } else {
                    None
                };
                MenuBarOutcome::Ignored
            }
            _ => MenuBarOutcome::Ignored,
        }
    }

    /// A key pressed while a dropdown is open. Up, Down, Home and End move in the deepest list
    /// that has a lit entry; Right opens the submenu of the lit entry and Left closes the
    /// deepest one (Left and Right move between menus when no submenu is involved). The list
    /// scrolls to keep the lit entry in view.
    fn key(&mut self, key: Key, menus: &[Menu]) -> MenuBarOutcome {
        let Some(open) = self.open else {
            return MenuBarOutcome::Consumed;
        };
        let level = self.focus();
        let lit_entry: Option<MenuEntry> = {
            let chain = self.chain(menus, self.window);
            chain
                .get(level)
                .zip(self.lit(level))
                .and_then(|(o, i)| o.entries.get(i).cloned())
        };
        let lit_submenu = match (&lit_entry, self.lit(level)) {
            (Some(MenuEntry::Submenu(_)), Some(row)) => Some(row),
            _ => None,
        };
        match key {
            Key::Escape if !self.subs.is_empty() => {
                self.subs.pop();
            }
            Key::Escape => self.close(),
            Key::Left if !self.subs.is_empty() => {
                self.subs.pop();
            }
            Key::Right if lit_submenu.is_some() => {
                if let Some(row) = lit_submenu {
                    self.open_flyout_keyed(menus, level, row);
                    self.reveal(menus, level + 1);
                }
            }
            Key::Right if level > 0 => {}
            Key::Left | Key::Right => {
                let n = menus.len().max(1);
                let next = if key == Key::Left {
                    (open + n - 1) % n
                } else {
                    (open + 1) % n
                };
                self.open = Some(next);
                self.subs.clear();
                self.scroll = 0.0;
                self.hover = edge(&menus[next].entries, true);
                self.reveal(menus, 0);
            }
            Key::Down | Key::Up | Key::Home | Key::End => {
                let chain = self.chain(menus, self.window);
                let Some(o) = chain.get(level) else {
                    return MenuBarOutcome::Consumed;
                };
                let at = self.lit(level);
                let pick = match key {
                    Key::Down | Key::Up => step(o.entries, at, key == Key::Down),
                    _ => edge(o.entries, key == Key::Home),
                };
                let pick = pick.or(at);
                self.set_lit(level, pick);
                // Moving closes the lists beyond this one.
                self.subs.truncate(level);
                self.reveal(menus, level);
                // At the first entry the leading separators belong in view too.
                if key == Key::Home
                    || (key == Key::Down && pick == self.first_selectable(menus, level))
                {
                    self.set_scroll(level, 0.0);
                }
            }
            Key::Enter | Key::Space => match lit_entry {
                Some(MenuEntry::Item(item)) if item.enabled => {
                    self.close();
                    return MenuBarOutcome::Command(item.command);
                }
                Some(MenuEntry::Submenu(_)) => {
                    if let Some(row) = lit_submenu {
                        self.open_flyout_keyed(menus, level, row);
                        self.reveal(menus, level + 1);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        MenuBarOutcome::Consumed
    }

    /// The first entry of list `level` that can be chosen.
    fn first_selectable(&self, menus: &[Menu], level: usize) -> Option<usize> {
        let chain = self.chain(menus, self.window);
        chain.get(level).and_then(|o| edge(o.entries, true))
    }

    /// Paints the bar across `size.width` and the open dropdown and its flyouts, kept inside
    /// the window of `size` (logical units).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, menus: &[Menu], size: Size) {
        let width = size.width;
        canvas.fill_with(
            Rect::new(0.0, 0.0, width, MENU_BAR_HEIGHT),
            theme.browser_bg,
        );
        canvas.fill_with(
            Rect::new(0.0, MENU_BAR_HEIGHT - 1.0, width, MENU_BAR_HEIGHT),
            theme.section_line,
        );
        let titles = Self::title_rects(menus);
        for (i, (m, r)) in menus.iter().zip(&titles).enumerate() {
            let active = self.open == Some(i) || (self.open.is_none() && self.hover == Some(i));
            if active {
                canvas.fill_with(
                    RoundedRect::from_rect(r.inset(-2.0), 4.0),
                    theme.browser_row,
                );
            }
            canvas.text(
                &TextRun::new(m.title, Point::new(r.center().x, 17.5), FONT, theme.text)
                    .align(Align::Center),
            );
        }
        for (level, o) in self.chain(menus, size).iter().enumerate() {
            paint_list(canvas, theme, o, self.lit(level), self.pointer);
        }
    }
}

/// Paints one open list (a dropdown or a flyout), `lit` highlighted; the scroll strips light
/// when `pointer` is on one.
fn paint_list(
    canvas: &mut dyn Canvas,
    theme: &Theme,
    open: &Open<'_>,
    lit: Option<usize>,
    pointer: Point,
) {
    let panel = open.layout.panel;
    canvas.fill_with(
        RoundedRect::from_rect(panel + atomik_ui::kurbo::Vec2::new(2.0, 3.0), 6.0),
        Color::from_rgba8(0, 0, 0, 90),
    );
    canvas.fill_with(RoundedRect::from_rect(panel, 6.0), theme.browser_bg);
    canvas.stroke_with(RoundedRect::from_rect(panel, 6.0), 1.0, theme.section_line);
    let scrolls = open.layout.scrolls();
    if scrolls {
        canvas.push_clip(&open.layout.view.into());
    }
    for (i, (entry, r)) in open.entries.iter().zip(&open.layout.drawn).enumerate() {
        if scrolls && (r.y1 <= open.layout.view.y0 || r.y0 >= open.layout.view.y1) {
            continue;
        }
        paint_row(canvas, theme, entry, *r, lit == Some(i));
    }
    if scrolls {
        canvas.pop_clip();
        for (strip, glyph, enabled) in [
            (open.layout.up_strip(), "\u{25B4}", open.layout.offset > 0.0),
            (
                open.layout.down_strip(),
                "\u{25BE}",
                open.layout.offset < open.layout.max_offset,
            ),
        ] {
            let Some(strip) = strip else { continue };
            if enabled && strip.contains(pointer) {
                canvas.fill_with(
                    RoundedRect::from_rect(strip.inset(-2.0), 4.0),
                    theme.selection,
                );
            }
            let color = if enabled {
                theme.text
            } else {
                theme.text_muted
            };
            canvas.text(
                &TextRun::new(
                    glyph,
                    Point::new(strip.center().x, strip.y1 - 2.5),
                    11.0,
                    color,
                )
                .align(Align::Center),
            );
        }
    }
}

/// Paints one entry in its row `r`.
fn paint_row(canvas: &mut dyn Canvas, theme: &Theme, entry: &MenuEntry, r: Rect, lit: bool) {
    match entry {
        MenuEntry::Separator => {
            let y = r.center().y;
            canvas.fill_with(
                Rect::new(r.x0 + 6.0, y - 0.5, r.x1 - 6.0, y + 0.5),
                theme.section_line,
            );
        }
        MenuEntry::Item(item) => {
            if lit && item.enabled {
                canvas.fill_with(RoundedRect::from_rect(r, 4.0), theme.selection);
            }
            let color = if item.enabled {
                theme.text
            } else {
                theme.text_muted
            };
            let base = r.y0 + 16.5;
            if item.checked == Some(true) {
                canvas.text(&TextRun::new(
                    "\u{2713}",
                    Point::new(r.x0 + 8.0, base),
                    FONT,
                    color,
                ));
            }
            canvas.text(&TextRun::new(
                item.label.as_ref(),
                Point::new(r.x0 + 28.0, base),
                FONT,
                color,
            ));
            if let Some(s) = item.shortcut.as_deref() {
                canvas.text(
                    &TextRun::new(s, Point::new(r.x1 - 10.0, base), 12.0, theme.text_muted)
                        .align(Align::End),
                );
            }
        }
        MenuEntry::Submenu(sub) => {
            if lit {
                canvas.fill_with(RoundedRect::from_rect(r, 4.0), theme.selection);
            }
            let base = r.y0 + 16.5;
            canvas.text(&TextRun::new(
                sub.label,
                Point::new(r.x0 + 28.0, base),
                FONT,
                theme.text,
            ));
            canvas.text(
                &TextRun::new(
                    "\u{25B8}",
                    Point::new(r.x1 - 10.0, base),
                    12.0,
                    theme.text_muted,
                )
                .align(Align::End),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{MenuState, menus};
    use atomik_ui::{CableVisibility, DisplayList, Flip, Modifiers};

    fn model() -> Vec<Menu> {
        let keymap = atomik_ui::keymap::Keymap::bundled(
            atomik_ui::keymap::Profile::Legacy,
            atomik_ui::keymap::Os::Windows,
        );
        menus(&state(), &keymap)
    }

    pub(super) fn state() -> MenuState {
        MenuState {
            flip: Flip::Front,
            cables: CableVisibility::Show,
            browser: false,
            file_browser: false,
            piano_keys: false,
            track_panel: false,
            history_panel: false,
            launcher: crate::commands::LauncherMenu::default(),
            rack_sort: atomik_model::RackSort::Free,
            single_column: false,
            can_undo: false,
            can_redo: false,
            undo_label: None,
            redo_label: None,
            can_edit: false,
            can_paste: false,
            can_combine: false,
            can_uncombine: false,
            can_split_sub: false,
            can_reverb_send: false,
            lens: Default::default(),
            song_visible: true,
            piano_roll: false,
            sample_editor: false,
            step_editor: false,
            slice_editor: false,
            playing: false,
            recording: false,
            loop_enabled: false,
            metronome: false,
            auto_lanes: false,
            pre_count: false,
            clock_out: false,
            clock_while_stopped: false,
            follow_clock: false,
            sample_rate: 48_000,
            midi_resolution: crate::midi_file::MidiResolution::Native,
            input_channels: 2,
            output_channels: 2,
            quantize: crate::record::InputQuantize::OFF,
            console: false,
            console_maximised: false,
            compensation: true,
            align_returns: true,
            send_follow: true,
            link_faders: false,
            console_links: false,
            cpu_meters: false,
            big_meter: false,
            tempo_lane: false,
            energy_lane: false,
            video_row: false,
            marker_strip: false,
            inspector: false,
            marker_list: false,
            rack_navigator: true,
            rack: true,
            rack_detached: false,
            console_detached: false,
            browser_detached: false,
            spectrum_eq: false,
            video_window: false,
            video_overlay: true,
            sequencer_only: false,
            editor: false,
            missing_audio: false,
            archive_busy: false,
            video_busy: false,
            video_clip: false,
            video_audio: false,
            video_audio_keep: false,
            video_import_audio: false,
            video_missing: false,
            export_film: false,
            collect_busy: false,
            default_song: false,
            midi_clip: false,
            midi_track: false,
            midi_song: false,
            export_song: false,
            export_loop: false,
            export_selection: false,
            export_stems: false,
            org: atomik_ui::arrangement::OrgMenu::default(),
            lane: atomik_ui::arrangement::LaneMenu::default(),
            audio: atomik_ui::arrangement::AudioMenu::default(),
            vca: crate::commands::VcaMenu::default(),
            strip: crate::commands::StripMenu::default(),
            snapshots: crate::commands::SnapshotMenu::default(),
            reference: crate::commands::ReferenceMenu::default(),
            midi: crate::commands::MidiMenu::default(),
            freeze: crate::commands::FreezeMenu::default(),
            follow_selection: false,
            theme: atomik_ui::ThemeChoice::Dark,
            app_zoom: 100,
        }
    }

    pub(super) fn down(p: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        }
    }

    pub(super) fn up(p: Point) -> UiEvent {
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }
    }

    pub(super) fn mv(p: Point) -> UiEvent {
        UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        }
    }

    /// Centre of the item labelled `label` in open menu `menu`.
    fn item_center(m: &[Menu], menu: usize, label: &str) -> Point {
        let (_, rows) = MenuBar::dropdown(m, menu).unwrap();
        let i = m[menu]
            .entries
            .iter()
            .position(|e| matches!(e, MenuEntry::Item(it) if it.label == label))
            .unwrap();
        rows[i].center()
    }

    #[test]
    fn clicking_a_title_opens_and_closes_its_menu() {
        let m = model();
        let mut bar = MenuBar::new();
        let file = MenuBar::title_rects(&m)[0].center();
        assert_eq!(bar.handle(&down(file), &m), MenuBarOutcome::Consumed);
        assert_eq!(bar.open_menu(), Some(0));
        assert_eq!(bar.handle(&up(file), &m), MenuBarOutcome::Consumed);
        assert!(bar.is_open(), "releasing on the title keeps it open");
        bar.handle(&down(file), &m);
        assert!(!bar.is_open());
    }

    #[test]
    fn choosing_an_item_returns_its_command_and_closes() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
        let p = item_center(&m, 2, "Zoom In");
        bar.handle(&mv(p), &m);
        assert_eq!(
            bar.handle(&up(p), &m),
            MenuBarOutcome::Command(AppCommand::ZoomIn)
        );
        assert!(!bar.is_open());
    }

    #[test]
    fn disabled_items_and_separators_do_nothing() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[1].center()), &m);
        let undo = item_center(&m, 1, "Undo");
        assert_eq!(bar.handle(&up(undo), &m), MenuBarOutcome::Consumed);
        assert!(bar.is_open());
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let (_, rows) = MenuBar::dropdown(&m, 0).unwrap();
        let sep = m[0]
            .entries
            .iter()
            .position(|e| *e == MenuEntry::Separator)
            .unwrap();
        assert_eq!(
            bar.handle(&up(rows[sep].center()), &m),
            MenuBarOutcome::Consumed
        );
    }

    #[test]
    fn hovering_another_title_switches_the_open_menu() {
        let m = model();
        let mut bar = MenuBar::new();
        let t = MenuBar::title_rects(&m);
        bar.handle(&down(t[0].center()), &m);
        bar.handle(&mv(t[3].center()), &m);
        assert_eq!(bar.open_menu(), Some(3));
    }

    #[test]
    fn a_click_outside_closes_and_is_swallowed() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let outside = Point::new(700.0, 500.0);
        assert_eq!(bar.handle(&down(outside), &m), MenuBarOutcome::Consumed);
        assert!(!bar.is_open());
        assert_eq!(bar.handle(&down(outside), &m), MenuBarOutcome::Ignored);
        assert_eq!(bar.handle(&mv(outside), &m), MenuBarOutcome::Ignored);
    }

    #[test]
    fn escape_closes_and_arrows_move_between_menus() {
        let m = model();
        let mut bar = MenuBar::new();
        let key = |k| UiEvent::Key {
            key: k,
            modifiers: Modifiers::NONE,
            pressed: true,
        };
        assert_eq!(bar.handle(&key(Key::Escape), &m), MenuBarOutcome::Ignored);
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        bar.handle(&key(Key::Left), &m);
        assert_eq!(bar.open_menu(), Some(m.len() - 1), "wraps to the last menu");
        bar.handle(&key(Key::Right), &m);
        assert_eq!(bar.open_menu(), Some(0));
        assert_eq!(bar.handle(&key(Key::Escape), &m), MenuBarOutcome::Consumed);
        assert!(!bar.is_open());
    }

    #[test]
    fn titles_do_not_overlap_and_dropdowns_sit_below_the_bar() {
        let m = model();
        let t = MenuBar::title_rects(&m);
        for w in t.windows(2) {
            assert!(w[0].x1 <= w[1].x0);
        }
        for i in 0..m.len() {
            let (panel, rows) = MenuBar::dropdown(&m, i).unwrap();
            assert!(panel.y0 >= MENU_BAR_HEIGHT);
            assert_eq!(rows.len(), m[i].entries.len());
            assert!(rows.iter().all(|r| panel.contains(r.center())));
        }
    }

    #[test]
    fn painting_shows_titles_and_the_open_menu_with_checks() {
        let m = model();
        let mut bar = MenuBar::new();
        let mut list = DisplayList::new();
        bar.paint(&mut list, &Theme::dark(), &m, Size::new(800.0, 600.0));
        let texts = list.texts();
        assert!(texts.contains(&"File") && texts.contains(&"Audio"));
        assert!(!texts.contains(&"Zoom In"));
        bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
        let mut list = DisplayList::new();
        bar.paint(&mut list, &Theme::dark(), &m, Size::new(800.0, 600.0));
        let texts = list.texts();
        assert!(texts.contains(&"Zoom In"));
        assert!(texts.contains(&"Cables"), "the Cables submenu is a row");
        assert!(texts.contains(&"Ctrl+="));
        // Resting on the Cables row opens its list, where Show is checked.
        let (_, rows) = MenuBar::dropdown(&m, 2).unwrap();
        let cables = m[2]
            .entries
            .iter()
            .position(|e| matches!(e, MenuEntry::Submenu(s) if s.label == "Cables"))
            .unwrap();
        bar.handle(&mv(rows[cables].center()), &m);
        let mut list = DisplayList::new();
        bar.paint(&mut list, &Theme::dark(), &m, Size::new(800.0, 600.0));
        let texts = list.texts();
        assert!(texts.contains(&"Cables: Hide Auto-routed"));
        assert!(texts.contains(&"\u{2713}"), "Cables: Show is checked");
    }

    pub(super) fn press(bar: &mut MenuBar, m: &[Menu], k: Key) -> MenuBarOutcome {
        bar.handle(
            &UiEvent::Key {
                key: k,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            m,
        )
    }

    /// The entry indices of menu `i` that can be chosen.
    fn selectable(m: &[Menu], i: usize) -> Vec<usize> {
        m[i].entries
            .iter()
            .enumerate()
            .filter(|(_, e)| match e {
                MenuEntry::Item(it) => it.enabled,
                MenuEntry::Submenu(_) => true,
                _ => false,
            })
            .map(|(r, _)| r)
            .collect()
    }

    #[test]
    fn up_and_down_move_the_highlight_wrap_and_skip_separators_and_disabled_items() {
        let m = model();
        // The Edit menu has a disabled Undo and separators.
        let ok = selectable(&m, 1);
        assert!(ok.len() < m[1].entries.len(), "rows to skip");
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[1].center()), &m);
        assert_eq!(bar.highlighted_item(), None);
        let mut seen = Vec::new();
        for _ in 0..=ok.len() {
            press(&mut bar, &m, Key::Down);
            seen.push(bar.highlighted_item().unwrap());
        }
        let mut want = ok.clone();
        want.push(ok[0]);
        assert_eq!(seen, want, "every enabled item once, then wraps");
        press(&mut bar, &m, Key::Up);
        assert_eq!(bar.highlighted_item(), ok.last().copied(), "Up wraps back");
        press(&mut bar, &m, Key::Home);
        assert_eq!(bar.highlighted_item(), ok.first().copied());
        press(&mut bar, &m, Key::End);
        assert_eq!(bar.highlighted_item(), ok.last().copied());
    }

    #[test]
    fn up_with_nothing_highlighted_starts_at_the_last_item() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
        press(&mut bar, &m, Key::Up);
        assert_eq!(bar.highlighted_item(), selectable(&m, 2).last().copied());
    }

    #[test]
    fn enter_and_space_choose_the_highlighted_item_and_close_the_menu() {
        let m = model();
        for k in [Key::Enter, Key::Space] {
            let mut bar = MenuBar::new();
            bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
            assert_eq!(
                press(&mut bar, &m, k.clone()),
                MenuBarOutcome::Consumed,
                "nothing yet"
            );
            press(&mut bar, &m, Key::Down);
            let first = selectable(&m, 2)[0];
            let MenuEntry::Item(item) = &m[2].entries[first] else {
                panic!("an item");
            };
            assert_eq!(
                press(&mut bar, &m, k),
                MenuBarOutcome::Command(item.command)
            );
            assert!(!bar.is_open());
        }
    }

    #[test]
    fn hover_and_keyboard_share_one_highlight() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
        let ok = selectable(&m, 2);
        let (_, rows) = MenuBar::dropdown(&m, 2).unwrap();
        bar.handle(&mv(rows[ok[1]].center()), &m);
        assert_eq!(bar.highlighted_item(), Some(ok[1]));
        press(&mut bar, &m, Key::Down);
        assert_eq!(
            bar.highlighted_item(),
            Some(ok[2]),
            "continues from the hovered item"
        );
    }

    #[test]
    fn left_and_right_move_to_the_neighbour_menu_on_its_first_item_and_right_on_an_item_does_too() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        press(&mut bar, &m, Key::Down);
        assert_eq!(press(&mut bar, &m, Key::Right), MenuBarOutcome::Consumed);
        assert_eq!(
            bar.open_menu(),
            Some(1),
            "Right on an item without a submenu"
        );
        assert_eq!(bar.highlighted_item(), selectable(&m, 1).first().copied());
        press(&mut bar, &m, Key::Left);
        assert_eq!(bar.open_menu(), Some(0));
        assert_eq!(bar.highlighted_item(), selectable(&m, 0).first().copied());
        press(&mut bar, &m, Key::Left);
        assert_eq!(bar.open_menu(), Some(m.len() - 1), "wraps to the last menu");
    }

    #[test]
    fn keys_in_an_open_menu_never_reach_the_panes_below() {
        let m = model();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        for k in [
            Key::Space,
            Key::Char('a'),
            Key::Tab,
            Key::Delete,
            Key::PageUp,
        ] {
            assert_ne!(press(&mut bar, &m, k), MenuBarOutcome::Ignored);
        }
        let release = UiEvent::Key {
            key: Key::Space,
            modifiers: Modifiers::NONE,
            pressed: false,
        };
        assert_eq!(bar.handle(&release, &m), MenuBarOutcome::Consumed);
    }

    fn model_with_recent() -> Vec<Menu> {
        let keymap = atomik_ui::keymap::Keymap::bundled(
            atomik_ui::keymap::Profile::Legacy,
            atomik_ui::keymap::Os::Windows,
        );
        let recent = ["first".to_string(), "second".to_string()];
        crate::commands::menus_with_recent(&state(), &keymap, &recent)
    }

    /// The row of File's Open Recent entry.
    fn recent_row(m: &[Menu]) -> usize {
        m[0].entries
            .iter()
            .position(|e| matches!(e, MenuEntry::Submenu(_)))
            .unwrap()
    }

    /// Centre of the entry `at` of the flyout of File's Open Recent.
    fn flyout_center(m: &[Menu], at: usize) -> Point {
        MenuBar::flyout(m, 0, recent_row(m)).unwrap().1[at].center()
    }

    #[test]
    fn resting_on_open_recent_opens_its_flyout_and_a_click_chooses_a_song() {
        let m = model_with_recent();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let (_, rows) = MenuBar::dropdown(&m, 0).unwrap();
        let row = recent_row(&m);
        bar.handle(&mv(rows[row].center()), &m);
        assert_eq!(bar.look(), (Some(0), Some(row), Some(row), None));
        // Into the flyout: the row stays lit and the entry under the pointer too.
        bar.handle(&mv(flyout_center(&m, 1)), &m);
        assert_eq!(bar.look(), (Some(0), Some(row), Some(row), Some(1)));
        let at = flyout_center(&m, 1);
        assert_eq!(bar.handle(&down(at), &m), MenuBarOutcome::Consumed);
        assert_eq!(
            bar.handle(&up(at), &m),
            MenuBarOutcome::Command(AppCommand::OpenRecent(1))
        );
        assert!(!bar.is_open());
        assert_eq!(bar.look(), (None, None, None, None));
    }

    #[test]
    fn leaving_the_open_recent_row_for_another_closes_the_flyout() {
        let m = model_with_recent();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let (_, rows) = MenuBar::dropdown(&m, 0).unwrap();
        let row = recent_row(&m);
        bar.handle(&mv(rows[row].center()), &m);
        bar.handle(&mv(rows[row - 1].center()), &m);
        assert_eq!(bar.look().2, None);
        // A click on the row opens it too.
        bar.handle(&mv(rows[row].center()), &m);
        bar.handle(&mv(rows[row - 1].center()), &m);
        bar.handle(&up(rows[row].center()), &m);
        assert_eq!(bar.look().2, Some(row));
    }

    #[test]
    fn the_flyout_sits_beside_the_row_and_a_disabled_entry_is_not_chosen() {
        let empty = {
            let keymap = atomik_ui::keymap::Keymap::bundled(
                atomik_ui::keymap::Profile::Legacy,
                atomik_ui::keymap::Os::Windows,
            );
            menus(&state(), &keymap)
        };
        let row = recent_row(&empty);
        let (panel, rows) = MenuBar::dropdown(&empty, 0).unwrap();
        let (fly, entries) = MenuBar::flyout(&empty, 0, row).unwrap();
        assert!(fly.x0 >= panel.x1 - 3.0 && fly.y0 <= rows[row].y0);
        assert!(
            MenuBar::flyout(&empty, 0, row - 1).is_none(),
            "an item has no flyout"
        );
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&empty)[0].center()), &empty);
        bar.handle(&mv(rows[row].center()), &empty);
        // The list is empty: Clear Menu is the last entry and is disabled.
        let clear = entries.last().unwrap().center();
        bar.handle(&mv(clear), &empty);
        bar.handle(&down(clear), &empty);
        assert_eq!(bar.handle(&up(clear), &empty), MenuBarOutcome::Consumed);
        assert!(bar.is_open());
    }

    #[test]
    fn right_opens_the_flyout_and_enter_chooses_inside_it() {
        let m = model_with_recent();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let row = recent_row(&m);
        // Step down to Open Recent.
        for _ in 0..40 {
            if bar.highlighted_item() == Some(row) {
                break;
            }
            press(&mut bar, &m, Key::Down);
        }
        assert_eq!(bar.highlighted_item(), Some(row));
        assert_eq!(
            bar.look().2,
            None,
            "the keyboard does not open it by moving"
        );
        press(&mut bar, &m, Key::Right);
        assert_eq!(bar.look(), (Some(0), Some(row), Some(row), Some(0)));
        assert_eq!(
            bar.open_menu(),
            Some(0),
            "Right opened the list, not the next menu"
        );
        press(&mut bar, &m, Key::Down);
        assert_eq!(bar.look().3, Some(1));
        assert_eq!(
            press(&mut bar, &m, Key::Enter),
            MenuBarOutcome::Command(AppCommand::OpenRecent(1))
        );
        assert!(!bar.is_open());
    }

    #[test]
    fn left_and_escape_close_the_flyout_before_the_menu() {
        let m = model_with_recent();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let row = recent_row(&m);
        while bar.highlighted_item() != Some(row) {
            press(&mut bar, &m, Key::Down);
        }
        press(&mut bar, &m, Key::Right);
        press(&mut bar, &m, Key::Left);
        assert_eq!(bar.look(), (Some(0), Some(row), None, None));
        press(&mut bar, &m, Key::Right);
        press(&mut bar, &m, Key::Escape);
        assert_eq!(bar.look(), (Some(0), Some(row), None, None));
        press(&mut bar, &m, Key::Escape);
        assert!(!bar.is_open());
    }

    #[test]
    fn the_flyout_paints_its_songs() {
        let m = model_with_recent();
        let mut bar = MenuBar::new();
        bar.handle(&down(MenuBar::title_rects(&m)[0].center()), &m);
        let (_, rows) = MenuBar::dropdown(&m, 0).unwrap();
        bar.handle(&mv(rows[recent_row(&m)].center()), &m);
        let mut list = DisplayList::new();
        bar.paint(&mut list, &Theme::dark(), &m, Size::new(800.0, 600.0));
        let json = list.to_json();
        for text in ["Open Recent", "first", "second", "Clear Menu"] {
            assert!(json.contains(&format!("\"text\":\"{text}\"")), "{text}");
        }
    }
}

#[cfg(test)]
#[path = "menu_structure_tests.rs"]
mod structure_tests;

#[cfg(test)]
#[path = "menu_scroll_tests.rs"]
mod scroll_tests;
