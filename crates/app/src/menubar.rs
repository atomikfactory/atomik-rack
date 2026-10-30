//! The in-window menu bar (Windows and Linux).
//!
//! muda's Windows attach (`Menu::init_for_hwnd`) is `unsafe`, which the workspace denies, and
//! its Linux menus need a GTK window that winit does not create. So the app draws the same
//! [`Menu`] model itself on those platforms: one bar of titles, one dropdown at a time, drawn
//! through the [`Canvas`] in logical units. macOS uses native muda menus instead.

use crate::commands::{AppCommand, Menu, MenuEntry};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

/// Height of the bar, logical pixels.
pub const MENU_BAR_HEIGHT: f64 = 26.0;
/// Height of one dropdown item.
pub const ITEM_HEIGHT: f64 = 24.0;
/// Height of a dropdown separator.
pub const SEPARATOR_HEIGHT: f64 = 9.0;
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

/// The rectangle of each of `entries` stacked from `top`, `x0..x1` wide, and the bottom.
fn list_rows(entries: &[MenuEntry], x0: f64, x1: f64, top: f64) -> (Vec<Rect>, f64) {
    let mut y = top;
    let rows = entries
        .iter()
        .map(|e| {
            let h = match e {
                MenuEntry::Item(_) | MenuEntry::Submenu(_) => ITEM_HEIGHT,
                MenuEntry::Separator => SEPARATOR_HEIGHT,
            };
            let r = Rect::new(x0, y, x1, y + h);
            y += h;
            r
        })
        .collect();
    (rows, y)
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

/// The in-window menu bar state.
#[derive(Debug, Clone, Default)]
pub struct MenuBar {
    open: Option<usize>,
    hover: Option<usize>,
    /// The entry of the open dropdown whose submenu is open.
    sub: Option<usize>,
    /// The highlighted entry of the open submenu.
    sub_hover: Option<usize>,
    pointer: Point,
}

/// What the bar shows of the pointer and the keyboard: the open menu, its highlighted entry,
/// the open submenu and its highlighted entry.
pub type BarLook = (Option<usize>, Option<usize>, Option<usize>, Option<usize>);

impl MenuBar {
    /// A closed bar.
    pub fn new() -> Self {
        Self::default()
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
        self.sub = None;
        self.sub_hover = None;
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

    /// The dropdown of menu `i` and the rectangle of each entry (separators included).
    pub fn dropdown(menus: &[Menu], i: usize) -> Option<(Rect, Vec<Rect>)> {
        let menu = menus.get(i)?;
        let title = *Self::title_rects(menus).get(i)?;
        let width = list_width(&menu.entries);
        let (rows, bottom) = list_rows(
            &menu.entries,
            title.x0 + 4.0,
            title.x0 + width - 4.0,
            MENU_BAR_HEIGHT + 4.0,
        );
        let panel = Rect::new(title.x0, MENU_BAR_HEIGHT, title.x0 + width, bottom + 4.0);
        Some((panel, rows))
    }

    /// The flyout of the submenu at `row` of menu `menu` and the rectangle of each of its
    /// entries: to the right of the dropdown, level with the row. `None` when that entry is
    /// not a submenu.
    pub fn flyout(menus: &[Menu], menu: usize, row: usize) -> Option<(Rect, Vec<Rect>)> {
        let MenuEntry::Submenu(sub) = menus.get(menu)?.entries.get(row)? else {
            return None;
        };
        let (panel, rows) = Self::dropdown(menus, menu)?;
        let width = list_width(&sub.entries);
        let x0 = panel.x1 - 2.0;
        let top = rows.get(row)?.y0;
        let (entries, bottom) = list_rows(&sub.entries, x0 + 4.0, x0 + width - 4.0, top);
        Some((Rect::new(x0, top - 4.0, x0 + width, bottom + 4.0), entries))
    }

    /// The entries of the open submenu.
    fn sub_entries<'a>(&self, menus: &'a [Menu]) -> Option<&'a [MenuEntry]> {
        match menus.get(self.open?)?.entries.get(self.sub?)? {
            MenuEntry::Submenu(s) => Some(&s.entries),
            _ => None,
        }
    }

    /// The item of the open submenu under `p`.
    fn sub_item_at(&self, menus: &[Menu], p: Point) -> Option<usize> {
        let (_, rows) = Self::flyout(menus, self.open?, self.sub?)?;
        let i = rows.iter().position(|r| r.contains(p))?;
        matches!(self.sub_entries(menus)?[i], MenuEntry::Item(_)).then_some(i)
    }

    fn in_flyout(&self, menus: &[Menu], p: Point) -> bool {
        self.open
            .zip(self.sub)
            .and_then(|(m, r)| Self::flyout(menus, m, r))
            .is_some_and(|(panel, _)| panel.contains(p))
    }

    /// Opens the submenu at `row` (the keyboard's Right): its first choosable entry is lit.
    fn open_sub(&mut self, menus: &[Menu], row: usize) {
        self.sub = Some(row);
        self.sub_hover = self
            .sub_entries(menus)
            .and_then(|e| e.iter().position(is_selectable));
    }

    /// What the bar shows of the pointer: the open menu and the highlighted title or entry,
    /// and the same for the open submenu. A move that leaves it as it was changes nothing on
    /// screen.
    pub fn look(&self) -> BarLook {
        (self.open, self.hover, self.sub, self.sub_hover)
    }

    /// The highlighted entry of the open dropdown: the pointer's and the keyboard's.
    pub fn highlighted_item(&self) -> Option<usize> {
        self.open.and(self.hover)
    }

    fn title_at(menus: &[Menu], p: Point) -> Option<usize> {
        Self::title_rects(menus).iter().position(|r| r.contains(p))
    }

    fn item_at(&self, menus: &[Menu], p: Point) -> Option<usize> {
        let (_, rows) = Self::dropdown(menus, self.open?)?;
        let i = rows.iter().position(|r| r.contains(p))?;
        matches!(
            menus[self.open?].entries[i],
            MenuEntry::Item(_) | MenuEntry::Submenu(_)
        )
        .then_some(i)
    }

    fn in_dropdown(&self, menus: &[Menu], p: Point) -> bool {
        self.open
            .and_then(|i| Self::dropdown(menus, i))
            .is_some_and(|(panel, _)| panel.contains(p))
            || self.in_flyout(menus, p)
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
                            self.sub = None;
                            self.sub_hover = None;
                        }
                        self.open = Some(t);
                    }
                    if self.in_flyout(menus, *pos) {
                        // The row of the submenu stays lit while its entries are chosen.
                        self.hover = self.sub;
                        self.sub_hover = self.sub_item_at(menus, *pos);
                    } else {
                        self.hover = self.item_at(menus, *pos);
                        if let (Some(i), Some(open)) = (self.hover, self.open) {
                            // Resting on a submenu row opens it; any other row closes it.
                            let is_sub = matches!(menus[open].entries[i], MenuEntry::Submenu(_));
                            if is_sub && self.sub != Some(i) {
                                self.sub = Some(i);
                                self.sub_hover = None;
                            } else if !is_sub {
                                self.sub = None;
                                self.sub_hover = None;
                            }
                        }
                    }
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
                if let Some(t) = Self::title_at(menus, *pos) {
                    if *button == PointerButton::Primary {
                        self.open = if self.open == Some(t) { None } else { Some(t) };
                        self.hover = None;
                        self.sub = None;
                        self.sub_hover = None;
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
                let Some(open) = self.open else {
                    return if in_bar(*pos) {
                        MenuBarOutcome::Consumed
                    } else {
                        MenuBarOutcome::Ignored
                    };
                };
                if *button != PointerButton::Primary {
                    return MenuBarOutcome::Consumed;
                }
                if self.in_flyout(menus, *pos) {
                    let chosen = self
                        .sub_item_at(menus, *pos)
                        .zip(self.sub_entries(menus))
                        .and_then(|(i, entries)| match &entries[i] {
                            MenuEntry::Item(item) if item.enabled => Some(item.command),
                            _ => None,
                        });
                    return match chosen {
                        Some(command) => {
                            self.close();
                            MenuBarOutcome::Command(command)
                        }
                        None => MenuBarOutcome::Consumed,
                    };
                }
                match self.item_at(menus, *pos) {
                    Some(i) => match &menus[open].entries[i] {
                        MenuEntry::Item(item) if item.enabled => {
                            self.close();
                            MenuBarOutcome::Command(item.command)
                        }
                        MenuEntry::Submenu(_) => {
                            // A click opens the submenu again if the pointer had left it.
                            self.sub = Some(i);
                            MenuBarOutcome::Consumed
                        }
                        _ => MenuBarOutcome::Consumed,
                    },
                    None => MenuBarOutcome::Consumed,
                }
            }
            UiEvent::Wheel { pos, .. } => {
                if self.open.is_some() || in_bar(*pos) {
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

    /// A key pressed while a dropdown is open. Up, Down, Home and End move in the submenu once
    /// it has a lit entry and in the dropdown otherwise; Right opens the submenu of the lit
    /// entry and Left closes it (Left and Right move between menus when no submenu is involved).
    fn key(&mut self, key: Key, menus: &[Menu]) -> MenuBarOutcome {
        let Some(open) = self.open else {
            return MenuBarOutcome::Consumed;
        };
        let in_sub = self.sub.is_some() && self.sub_hover.is_some();
        let lit_submenu = self
            .hover
            .filter(|i| matches!(menus[open].entries[*i], MenuEntry::Submenu(_)));
        match key {
            Key::Escape if self.sub.is_some() => {
                self.sub = None;
                self.sub_hover = None;
            }
            Key::Escape => self.close(),
            Key::Left if self.sub.is_some() => {
                self.sub = None;
                self.sub_hover = None;
            }
            Key::Right if in_sub => {}
            Key::Right if lit_submenu.is_some() => {
                if let Some(row) = lit_submenu {
                    self.open_sub(menus, row);
                }
            }
            Key::Left | Key::Right => {
                let n = menus.len().max(1);
                let next = if key == Key::Left {
                    (open + n - 1) % n
                } else {
                    (open + 1) % n
                };
                self.open = Some(next);
                self.sub = None;
                self.sub_hover = None;
                self.hover = edge(&menus[next].entries, true);
            }
            Key::Down | Key::Up | Key::Home | Key::End => {
                let (entries, at) = match self.sub_entries(menus) {
                    Some(entries) if in_sub => (entries, self.sub_hover),
                    _ => (menus[open].entries.as_slice(), self.hover),
                };
                let pick = match key {
                    Key::Down | Key::Up => step(entries, at, key == Key::Down),
                    _ => edge(entries, key == Key::Home),
                };
                let pick = pick.or(at);
                if in_sub {
                    self.sub_hover = pick;
                } else {
                    self.hover = pick;
                    self.sub = None;
                    self.sub_hover = None;
                }
            }
            Key::Enter | Key::Space => {
                if in_sub {
                    let chosen =
                        self.sub_hover
                            .zip(self.sub_entries(menus))
                            .and_then(|(i, entries)| match &entries[i] {
                                MenuEntry::Item(item) if item.enabled => Some(item.command),
                                _ => None,
                            });
                    if let Some(command) = chosen {
                        self.close();
                        return MenuBarOutcome::Command(command);
                    }
                } else if let Some(row) = lit_submenu {
                    self.open_sub(menus, row);
                } else if let Some(i) = self.hover
                    && let MenuEntry::Item(item) = &menus[open].entries[i]
                    && item.enabled
                {
                    self.close();
                    return MenuBarOutcome::Command(item.command);
                }
            }
            _ => {}
        }
        MenuBarOutcome::Consumed
    }

    /// Paints the bar across `width` and the open dropdown (logical units).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, menus: &[Menu], width: f64) {
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
        let Some(open) = self.open else { return };
        let Some((panel, rows)) = Self::dropdown(menus, open) else {
            return;
        };
        paint_list(
            canvas,
            theme,
            panel,
            &menus[open].entries,
            &rows,
            self.hover,
        );
        if let Some(row) = self.sub
            && let Some((panel, rows)) = Self::flyout(menus, open, row)
            && let Some(entries) = self.sub_entries(menus)
        {
            paint_list(canvas, theme, panel, entries, &rows, self.sub_hover);
        }
    }
}

/// Paints a list of entries (a dropdown or a flyout) in `panel`, `lit` highlighted.
fn paint_list(
    canvas: &mut dyn Canvas,
    theme: &Theme,
    panel: Rect,
    entries: &[MenuEntry],
    rows: &[Rect],
    lit: Option<usize>,
) {
    canvas.fill_with(
        RoundedRect::from_rect(panel + atomik_ui::kurbo::Vec2::new(2.0, 3.0), 6.0),
        Color::from_rgba8(0, 0, 0, 90),
    );
    canvas.fill_with(RoundedRect::from_rect(panel, 6.0), theme.browser_bg);
    canvas.stroke_with(RoundedRect::from_rect(panel, 6.0), 1.0, theme.section_line);
    for (i, (entry, r)) in entries.iter().zip(rows).enumerate() {
        match entry {
            MenuEntry::Separator => {
                let y = r.center().y;
                canvas.fill_with(
                    Rect::new(r.x0 + 6.0, y - 0.5, r.x1 - 6.0, y + 0.5),
                    theme.section_line,
                );
            }
            MenuEntry::Item(item) => {
                if lit == Some(i) && item.enabled {
                    canvas.fill_with(RoundedRect::from_rect(*r, 4.0), theme.selection);
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
                if lit == Some(i) {
                    canvas.fill_with(RoundedRect::from_rect(*r, 4.0), theme.selection);
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

    fn state() -> MenuState {
        MenuState {
            flip: Flip::Front,
            cables: CableVisibility::Show,
            browser: false,
            file_browser: false,
            can_undo: false,
            can_redo: false,
            can_edit: false,
            can_paste: false,
            song_visible: true,
            piano_roll: false,
            sample_editor: false,
            step_editor: false,
            slice_editor: false,
            playing: false,
            recording: false,
            loop_enabled: false,
            metronome: false,
            pre_count: false,
            quantize: crate::record::InputQuantize::Off,
            console: false,
            console_maximised: false,
            compensation: true,
            console_devices: false,
            console_links: false,
            tempo_lane: false,
            marker_strip: false,
            inspector: false,
            marker_list: false,
            rack_navigator: true,
            rack: true,
            rack_detached: false,
            console_detached: false,
            sequencer_only: false,
            editor: false,
            missing_audio: false,
            midi_clip: false,
            midi_track: false,
            midi_song: false,
            export_song: false,
            export_loop: false,
            org: atomik_ui::arrangement::OrgMenu::default(),
            lane: atomik_ui::arrangement::LaneMenu::default(),
            audio: atomik_ui::arrangement::AudioMenu::default(),
            vca: crate::commands::VcaMenu::default(),
            strip: crate::commands::StripMenu::default(),
            snapshots: crate::commands::SnapshotMenu::default(),
            midi: crate::commands::MidiMenu::default(),
            freeze: crate::commands::FreezeMenu::default(),
            follow_selection: false,
        }
    }

    fn down(p: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        }
    }

    fn up(p: Point) -> UiEvent {
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }
    }

    fn mv(p: Point) -> UiEvent {
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
        bar.paint(&mut list, &Theme::dark(), &m, 800.0);
        let texts = list.texts();
        assert!(texts.contains(&"File") && texts.contains(&"Audio"));
        assert!(!texts.contains(&"Zoom In"));
        bar.handle(&down(MenuBar::title_rects(&m)[2].center()), &m);
        let mut list = DisplayList::new();
        bar.paint(&mut list, &Theme::dark(), &m, 800.0);
        let texts = list.texts();
        assert!(texts.contains(&"Zoom In"));
        assert!(texts.contains(&"\u{2713}"), "Cables: Show is checked");
        assert!(texts.contains(&"Ctrl+="));
    }

    fn press(bar: &mut MenuBar, m: &[Menu], k: Key) -> MenuBarOutcome {
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
            .filter(|(_, e)| matches!(e, MenuEntry::Item(it) if it.enabled))
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
        bar.paint(&mut list, &Theme::dark(), &m, 800.0);
        let json = list.to_json();
        for text in ["Open Recent", "first", "second", "Clear Menu"] {
            assert!(json.contains(&format!("\"text\":\"{text}\"")), "{text}");
        }
    }
}
