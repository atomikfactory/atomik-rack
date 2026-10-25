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

/// The in-window menu bar state.
#[derive(Debug, Clone, Default)]
pub struct MenuBar {
    open: Option<usize>,
    hover: Option<usize>,
    pointer: Point,
}

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
        let width = menu
            .items()
            .map(|it| {
                text_width(it.label)
                    + it.shortcut.as_deref().map_or(0.0, |s| text_width(s) + 32.0)
                    + 48.0
            })
            .fold(DROPDOWN_MIN_WIDTH, f64::max);
        let mut y = MENU_BAR_HEIGHT + 4.0;
        let rows: Vec<Rect> = menu
            .entries
            .iter()
            .map(|e| {
                let h = match e {
                    MenuEntry::Item(_) => ITEM_HEIGHT,
                    MenuEntry::Separator => SEPARATOR_HEIGHT,
                };
                let r = Rect::new(title.x0 + 4.0, y, title.x0 + 4.0 + width - 8.0, y + h);
                y += h;
                r
            })
            .collect();
        let panel = Rect::new(title.x0, MENU_BAR_HEIGHT, title.x0 + width, y + 4.0);
        Some((panel, rows))
    }

    fn title_at(menus: &[Menu], p: Point) -> Option<usize> {
        Self::title_rects(menus).iter().position(|r| r.contains(p))
    }

    fn item_at(&self, menus: &[Menu], p: Point) -> Option<usize> {
        let (_, rows) = Self::dropdown(menus, self.open?)?;
        let i = rows.iter().position(|r| r.contains(p))?;
        matches!(menus[self.open?].entries[i], MenuEntry::Item(_)).then_some(i)
    }

    fn in_dropdown(&self, menus: &[Menu], p: Point) -> bool {
        self.open
            .and_then(|i| Self::dropdown(menus, i))
            .is_some_and(|(panel, _)| panel.contains(p))
    }

    /// Handles an event (logical window coordinates).
    pub fn handle(&mut self, event: &UiEvent, menus: &[Menu]) -> MenuBarOutcome {
        let in_bar = |p: Point| p.y >= 0.0 && p.y < MENU_BAR_HEIGHT;
        match event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = *pos;
                if self.open.is_some() {
                    if let Some(t) = Self::title_at(menus, *pos) {
                        self.open = Some(t);
                    }
                    self.hover = self.item_at(menus, *pos);
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
                match self.item_at(menus, *pos) {
                    Some(i) => match &menus[open].entries[i] {
                        MenuEntry::Item(item) if item.enabled => {
                            self.close();
                            MenuBarOutcome::Command(item.command)
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
                    match key {
                        Key::Escape => self.close(),
                        Key::Left | Key::Right => {
                            let n = menus.len().max(1);
                            let i = self.open.unwrap_or(0);
                            self.open = Some(if *key == Key::Left {
                                (i + n - 1) % n
                            } else {
                                (i + 1) % n
                            });
                            self.hover = None;
                        }
                        _ => {}
                    }
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
        canvas.fill_with(
            RoundedRect::from_rect(panel + atomik_ui::kurbo::Vec2::new(2.0, 3.0), 6.0),
            Color::from_rgba8(0, 0, 0, 90),
        );
        canvas.fill_with(RoundedRect::from_rect(panel, 6.0), theme.browser_bg);
        canvas.stroke_with(RoundedRect::from_rect(panel, 6.0), 1.0, theme.section_line);
        for (i, (entry, r)) in menus[open].entries.iter().zip(&rows).enumerate() {
            match entry {
                MenuEntry::Separator => {
                    let y = r.center().y;
                    canvas.fill_with(
                        Rect::new(r.x0 + 6.0, y - 0.5, r.x1 - 6.0, y + 0.5),
                        theme.section_line,
                    );
                }
                MenuEntry::Item(item) => {
                    if self.hover == Some(i) && item.enabled {
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
                        item.label,
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
            quantize: crate::record::InputQuantize::Off,
            console: false,
            console_maximised: false,
            compensation: true,
            console_devices: false,
            console_links: false,
            tempo_lane: false,
            marker_strip: false,
            marker_list: false,
            rack_navigator: true,
            rack: true,
            sequencer_only: false,
            editor: false,
            missing_audio: false,
            midi_clip: false,
            midi_track: false,
            midi_song: false,
            org: atomik_ui::arrangement::OrgMenu::default(),
            audio: atomik_ui::arrangement::AudioMenu::default(),
            vca: crate::commands::VcaMenu::default(),
            strip: crate::commands::StripMenu::default(),
            snapshots: crate::commands::SnapshotMenu::default(),
            freeze: crate::commands::FreezeMenu::default(),
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
}
