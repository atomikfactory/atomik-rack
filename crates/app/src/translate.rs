//! winit → `atomik-ui` event translation (the hosting contract in the `atomik-ui` crate docs).

use crate::cursor_art::CursorArt;
use atomik_ui::keymap::Os;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{CursorHint, Key, Modifiers, NumpadKey, PointerButton, UiEvent};
use std::path::PathBuf;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::CursorIcon;

/// Pixels of a touchpad (pixel) scroll per wheel notch.
pub const PIXELS_PER_NOTCH: f64 = 40.0;

/// Modifier state.
pub fn modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// The drawn cursor for a view's cursor hint, for the hints the window system has no icon for.
pub fn cursor_art(hint: CursorHint) -> Option<CursorArt> {
    match hint {
        CursorHint::Pen => Some(CursorArt::Pen),
        CursorHint::Eraser => Some(CursorArt::Eraser),
        CursorHint::Razor => Some(CursorArt::Razor),
        _ => None,
    }
}

/// The winit cursor icon for a view's cursor hint (the stand-in for a drawn cursor).
pub fn cursor_icon(hint: CursorHint) -> CursorIcon {
    match hint {
        CursorHint::Default => CursorIcon::Default,
        CursorHint::ResizeHorizontal => CursorIcon::EwResize,
        CursorHint::Move => CursorIcon::Move,
        CursorHint::Crosshair | CursorHint::Pen | CursorHint::Eraser | CursorHint::Razor => {
            CursorIcon::Crosshair
        }
    }
}

/// Pointer buttons the view knows.
pub fn button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

/// Maps pointer buttons, turning Ctrl+click on macOS into a secondary (right) click: the press decides, and the matching release follows it even if Ctrl was let go in
/// between. The event's modifiers lose the Ctrl that made the click secondary, so the views do
/// not also see a "command" click.
#[derive(Debug, Default)]
pub struct PointerMap {
    ctrl_click: bool,
}

impl PointerMap {
    /// A press of `mb` with `modifiers` held, on `os`.
    pub fn press(
        &mut self,
        mb: MouseButton,
        modifiers: Modifiers,
        os: Os,
    ) -> Option<(PointerButton, Modifiers)> {
        let b = button(mb)?;
        if os == Os::Mac && b == PointerButton::Primary && modifiers.ctrl && !modifiers.meta {
            self.ctrl_click = true;
            return Some((
                PointerButton::Secondary,
                Modifiers {
                    ctrl: false,
                    ..modifiers
                },
            ));
        }
        Some((b, modifiers))
    }

    /// A release of `mb`.
    pub fn release(
        &mut self,
        mb: MouseButton,
        modifiers: Modifiers,
    ) -> Option<(PointerButton, Modifiers)> {
        let b = button(mb)?;
        if b == PointerButton::Primary && self.ctrl_click {
            self.ctrl_click = false;
            return Some((
                PointerButton::Secondary,
                Modifiers {
                    ctrl: false,
                    ..modifiers
                },
            ));
        }
        Some((b, modifiers))
    }
}

/// Turns winit's modifier state into [`UiEvent::ModifiersChanged`], once per real change.
#[derive(Debug, Default)]
pub struct ModifierTracker {
    last: Modifiers,
}

impl ModifierTracker {
    /// The new state; the event only when it differs from the last one.
    pub fn update(&mut self, state: ModifiersState) -> Option<UiEvent> {
        let modifiers = modifiers(state);
        if modifiers == self.last {
            return None;
        }
        self.last = modifiers;
        Some(UiEvent::ModifiersChanged { modifiers })
    }

    /// The current modifiers.
    pub fn current(&self) -> Modifiers {
        self.last
    }

    /// Forgets the state (the window lost focus).
    pub fn reset(&mut self) -> Option<UiEvent> {
        self.update(ModifiersState::empty())
    }
}

/// The key of a keyboard event: a numpad key is [`Key::Numpad`] by its physical code (the same
/// with NumLock on or off), a main-row digit is [`Key::Digit`] by position while Command and
/// Option are both held on macOS (Option changes the typed character, so a chord such as
/// Cmd+Opt+3 binds the physical key), anything else is [`key`] of the logical key.
pub fn key_event(logical: &WinitKey, physical: &PhysicalKey, modifiers: Modifiers, os: Os) -> Key {
    if let PhysicalKey::Code(code) = physical {
        if let Some(n) = numpad_key(*code) {
            return Key::Numpad(n);
        }
        if os == Os::Mac
            && modifiers.meta
            && modifiers.alt
            && let Some(d) = main_row_digit(*code)
        {
            return Key::Digit(d);
        }
    }
    key(logical)
}

fn numpad_key(code: KeyCode) -> Option<NumpadKey> {
    Some(match code {
        KeyCode::Numpad0 => NumpadKey::Digit(0),
        KeyCode::Numpad1 => NumpadKey::Digit(1),
        KeyCode::Numpad2 => NumpadKey::Digit(2),
        KeyCode::Numpad3 => NumpadKey::Digit(3),
        KeyCode::Numpad4 => NumpadKey::Digit(4),
        KeyCode::Numpad5 => NumpadKey::Digit(5),
        KeyCode::Numpad6 => NumpadKey::Digit(6),
        KeyCode::Numpad7 => NumpadKey::Digit(7),
        KeyCode::Numpad8 => NumpadKey::Digit(8),
        KeyCode::Numpad9 => NumpadKey::Digit(9),
        KeyCode::NumpadEnter => NumpadKey::Enter,
        KeyCode::NumpadAdd => NumpadKey::Add,
        KeyCode::NumpadSubtract => NumpadKey::Subtract,
        KeyCode::NumpadMultiply => NumpadKey::Multiply,
        KeyCode::NumpadDivide => NumpadKey::Divide,
        KeyCode::NumpadDecimal => NumpadKey::Decimal,
        _ => return None,
    })
}

fn main_row_digit(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Digit0 => 0,
        KeyCode::Digit1 => 1,
        KeyCode::Digit2 => 2,
        KeyCode::Digit3 => 3,
        KeyCode::Digit4 => 4,
        KeyCode::Digit5 => 5,
        KeyCode::Digit6 => 6,
        KeyCode::Digit7 => 7,
        KeyCode::Digit8 => 8,
        KeyCode::Digit9 => 9,
        _ => return None,
    })
}

/// A logical key: characters as typed (one char), named keys to their variant (F1–F24 to
/// [`Key::F`], Insert to [`Key::Insert`]), the rest [`Key::Other`]. Numpad and physical digits
/// need the physical code: see [`key_event`].
pub fn key(key: &WinitKey) -> Key {
    match key {
        WinitKey::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Char(c),
                _ => Key::Other,
            }
        }
        WinitKey::Named(named) => match named {
            NamedKey::Escape => Key::Escape,
            NamedKey::Enter => Key::Enter,
            NamedKey::Space => Key::Space,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Tab => Key::Tab,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Insert => Key::Insert,
            other => function_key(*other).map_or(Key::Other, Key::F),
        },
        _ => Key::Other,
    }
}

/// The number of a function key F1–F24 (F5 toggles the console).
fn function_key(named: NamedKey) -> Option<u8> {
    const KEYS: [NamedKey; 24] = [
        NamedKey::F1,
        NamedKey::F2,
        NamedKey::F3,
        NamedKey::F4,
        NamedKey::F5,
        NamedKey::F6,
        NamedKey::F7,
        NamedKey::F8,
        NamedKey::F9,
        NamedKey::F10,
        NamedKey::F11,
        NamedKey::F12,
        NamedKey::F13,
        NamedKey::F14,
        NamedKey::F15,
        NamedKey::F16,
        NamedKey::F17,
        NamedKey::F18,
        NamedKey::F19,
        NamedKey::F20,
        NamedKey::F21,
        NamedKey::F22,
        NamedKey::F23,
        NamedKey::F24,
    ];
    (1u8..).zip(KEYS).find(|(_, k)| *k == named).map(|(n, _)| n)
}

/// Wheel notches: line deltas as they are, pixel deltas (physical) converted to logical and
/// divided by [`PIXELS_PER_NOTCH`]. Positive `y` = away from the user.
pub fn wheel(delta: MouseScrollDelta, scale: f64) -> Vec2 {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => Vec2::new(f64::from(x), f64::from(y)),
        MouseScrollDelta::PixelDelta(p) => {
            let s = scale.max(f64::MIN_POSITIVE) * PIXELS_PER_NOTCH;
            Vec2::new(p.x / s, p.y / s)
        }
    }
}

/// A physical position in logical pixels.
pub fn logical(pos: PhysicalPosition<f64>, scale: f64) -> Point {
    let s = scale.max(f64::MIN_POSITIVE);
    Point::new(pos.x / s, pos.y / s)
}

/// A physical size in logical pixels.
pub fn logical_size(size: PhysicalSize<u32>, scale: f64) -> atomik_ui::kurbo::Size {
    let s = scale.max(f64::MIN_POSITIVE);
    atomik_ui::kurbo::Size::new(f64::from(size.width) / s, f64::from(size.height) / s)
}

/// Collects winit's per-file drag and drop events into the UI's whole-drop events.
///
/// winit sends one `HoveredFile` and one `DroppedFile` per file, and `HoveredFileCancelled`
/// once. The platform layer feeds each into this struct and calls [`Self::flush`] before it
/// handles any other window event and at the end of every event-loop iteration
/// (`about_to_wait`), so all files of one drop arrive as **one** [`UiEvent::FileDrop`] and
/// all files of one hover as one [`UiEvent::FileHover`]. Per flush at most one event comes
/// out, by priority: a drop (which also ends the hover), else a cancel, else a hover. The
/// position is the last known cursor position (winit sends none with file events, and on
/// Windows no cursor movement during an OS drag).
#[derive(Debug, Default)]
pub struct FileDrag {
    hovered: Vec<PathBuf>,
    dropped: Vec<PathBuf>,
    hover_changed: bool,
    cancelled: bool,
}

impl FileDrag {
    /// An empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// `WindowEvent::HoveredFile`.
    pub fn hovered(&mut self, path: PathBuf) {
        self.hovered.push(path);
        self.hover_changed = true;
        self.cancelled = false;
    }

    /// `WindowEvent::HoveredFileCancelled`.
    pub fn hover_cancelled(&mut self) {
        self.hovered.clear();
        self.hover_changed = false;
        self.cancelled = true;
    }

    /// `WindowEvent::DroppedFile`.
    pub fn dropped(&mut self, path: PathBuf) {
        self.dropped.push(path);
    }

    /// True if [`Self::flush`] would produce an event.
    pub fn is_pending(&self) -> bool {
        !self.dropped.is_empty() || self.cancelled || self.hover_changed
    }

    /// The event for what was collected since the last flush (module docs), if any.
    pub fn flush(&mut self, pos: Point) -> Option<UiEvent> {
        let event = if !self.dropped.is_empty() {
            self.hovered.clear();
            Some(UiEvent::FileDrop {
                pos,
                paths: std::mem::take(&mut self.dropped),
            })
        } else if self.cancelled {
            Some(UiEvent::FileHoverCancel)
        } else if self.hover_changed {
            Some(UiEvent::FileHover {
                pos,
                paths: self.hovered.clone(),
            })
        } else {
            None
        };
        self.hover_changed = false;
        self.cancelled = false;
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_map_one_to_one() {
        let m = modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert_eq!(
            m,
            Modifiers {
                shift: true,
                ctrl: true,
                alt: false,
                meta: false
            }
        );
        assert!(modifiers(ModifiersState::SUPER).command());
        assert!(modifiers(ModifiersState::ALT).alt);
        assert_eq!(modifiers(ModifiersState::empty()), Modifiers::NONE);
    }

    #[test]
    fn cursor_hints_map_to_icons() {
        assert_eq!(cursor_icon(CursorHint::Default), CursorIcon::Default);
        assert_eq!(
            cursor_icon(CursorHint::ResizeHorizontal),
            CursorIcon::EwResize
        );
        assert_eq!(cursor_icon(CursorHint::Move), CursorIcon::Move);
        assert_eq!(cursor_icon(CursorHint::Crosshair), CursorIcon::Crosshair);
        assert_eq!(cursor_art(CursorHint::Default), None);
        assert_eq!(cursor_art(CursorHint::Pen), Some(CursorArt::Pen));
        assert_eq!(cursor_art(CursorHint::Eraser), Some(CursorArt::Eraser));
        assert_eq!(cursor_art(CursorHint::Razor), Some(CursorArt::Razor));
    }

    #[test]
    fn buttons_map_and_extra_buttons_are_ignored() {
        assert_eq!(button(MouseButton::Left), Some(PointerButton::Primary));
        assert_eq!(button(MouseButton::Right), Some(PointerButton::Secondary));
        assert_eq!(button(MouseButton::Middle), Some(PointerButton::Middle));
        assert_eq!(button(MouseButton::Back), None);
    }

    #[test]
    fn keys_map_to_ui_keys() {
        assert_eq!(key(&WinitKey::Character("z".into())), Key::Char('z'));
        assert_eq!(key(&WinitKey::Character("Z".into())), Key::Char('Z'));
        assert_eq!(key(&WinitKey::Character("ab".into())), Key::Other);
        for (named, expected) in [
            (NamedKey::Escape, Key::Escape),
            (NamedKey::Enter, Key::Enter),
            (NamedKey::Space, Key::Space),
            (NamedKey::Backspace, Key::Backspace),
            (NamedKey::Delete, Key::Delete),
            (NamedKey::Tab, Key::Tab),
            (NamedKey::ArrowUp, Key::Up),
            (NamedKey::ArrowDown, Key::Down),
            (NamedKey::ArrowLeft, Key::Left),
            (NamedKey::ArrowRight, Key::Right),
            (NamedKey::PageUp, Key::PageUp),
            (NamedKey::PageDown, Key::PageDown),
            (NamedKey::Home, Key::Home),
            (NamedKey::End, Key::End),
            (NamedKey::F1, Key::F(1)),
            (NamedKey::F5, Key::F(5)),
            (NamedKey::F12, Key::F(12)),
            (NamedKey::F24, Key::F(24)),
            (NamedKey::F25, Key::Other),
            (NamedKey::Insert, Key::Insert),
            (NamedKey::Shift, Key::Other),
            (NamedKey::Control, Key::Other),
        ] {
            assert_eq!(key(&WinitKey::Named(named)), expected);
        }
    }

    fn press_key(code: KeyCode, logical: WinitKey, m: Modifiers, os: Os) -> Key {
        key_event(&logical, &PhysicalKey::Code(code), m, os)
    }

    #[test]
    fn numpad_keys_are_distinct_from_the_main_row_with_numlock_on_or_off() {
        use NumpadKey as N;
        let none = Modifiers::NONE;
        for (code, expected) in [
            (KeyCode::Numpad0, N::Digit(0)),
            (KeyCode::Numpad1, N::Digit(1)),
            (KeyCode::Numpad4, N::Digit(4)),
            (KeyCode::Numpad9, N::Digit(9)),
            (KeyCode::NumpadEnter, N::Enter),
            (KeyCode::NumpadAdd, N::Add),
            (KeyCode::NumpadSubtract, N::Subtract),
            (KeyCode::NumpadMultiply, N::Multiply),
            (KeyCode::NumpadDivide, N::Divide),
            (KeyCode::NumpadDecimal, N::Decimal),
        ] {
            // NumLock on: the logical key is the character; off: a navigation key. The
            // physical code decides either way.
            for logical in [
                WinitKey::Character("5".into()),
                WinitKey::Named(NamedKey::Home),
                WinitKey::Named(NamedKey::Enter),
            ] {
                assert_eq!(
                    press_key(code, logical, none, Os::Windows),
                    Key::Numpad(expected),
                    "{code:?}"
                );
            }
        }
        assert_eq!(
            press_key(
                KeyCode::Digit1,
                WinitKey::Character("1".into()),
                none,
                Os::Windows
            ),
            Key::Char('1'),
            "the main row stays a character"
        );
        assert_eq!(
            press_key(
                KeyCode::Enter,
                WinitKey::Named(NamedKey::Enter),
                none,
                Os::Windows
            ),
            Key::Enter
        );
    }

    #[test]
    fn insert_and_function_keys_come_from_the_logical_key() {
        let none = Modifiers::NONE;
        assert_eq!(
            press_key(
                KeyCode::Insert,
                WinitKey::Named(NamedKey::Insert),
                none,
                Os::Windows
            ),
            Key::Insert
        );
        assert_eq!(
            press_key(KeyCode::F8, WinitKey::Named(NamedKey::F8), none, Os::Linux),
            Key::F(8)
        );
        assert_eq!(
            press_key(KeyCode::F3, WinitKey::Named(NamedKey::F3), none, Os::Mac),
            Key::F(3)
        );
        assert_eq!(
            press_key(
                KeyCode::ShiftLeft,
                WinitKey::Named(NamedKey::Shift),
                none,
                Os::Windows
            ),
            Key::Other,
            "modifier keys are delivered (as Other) so their releases reach the shell"
        );
    }

    #[test]
    fn main_row_digits_are_physical_only_with_command_and_option_on_macos() {
        let cmd_opt = Modifiers {
            meta: true,
            alt: true,
            ..Modifiers::NONE
        };
        // Option+3 types a different character on a Mac; the physical key still says 3.
        let logical = WinitKey::Character("\u{a3}".into());
        assert_eq!(
            press_key(KeyCode::Digit3, logical.clone(), cmd_opt, Os::Mac),
            Key::Digit(3)
        );
        assert_eq!(
            press_key(KeyCode::Digit0, logical.clone(), cmd_opt, Os::Mac),
            Key::Digit(0)
        );
        // Not on other systems (AltGr types characters there), not without Command.
        assert_ne!(
            press_key(KeyCode::Digit3, logical.clone(), cmd_opt, Os::Windows),
            Key::Digit(3)
        );
        let alt_only = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            press_key(KeyCode::Digit3, logical, alt_only, Os::Mac),
            Key::Char('\u{a3}')
        );
    }

    #[test]
    fn ctrl_click_on_macos_is_a_secondary_click_and_the_release_follows() {
        let ctrl = Modifiers::CTRL;
        let mut map = PointerMap::default();
        assert_eq!(
            map.press(MouseButton::Left, ctrl, Os::Mac),
            Some((PointerButton::Secondary, Modifiers::NONE)),
            "the Ctrl that made it secondary is not also a command"
        );
        assert_eq!(
            map.release(MouseButton::Left, Modifiers::NONE),
            Some((PointerButton::Secondary, Modifiers::NONE)),
            "released after Ctrl was let go: still the secondary"
        );
        assert_eq!(
            map.press(MouseButton::Left, Modifiers::NONE, Os::Mac),
            Some((PointerButton::Primary, Modifiers::NONE))
        );
        assert_eq!(
            map.release(MouseButton::Left, Modifiers::NONE),
            Some((PointerButton::Primary, Modifiers::NONE))
        );
    }

    #[test]
    fn ctrl_click_is_a_plain_click_with_ctrl_elsewhere_and_cmd_click_stays_primary() {
        let mut map = PointerMap::default();
        for os in [Os::Windows, Os::Linux] {
            assert_eq!(
                map.press(MouseButton::Left, Modifiers::CTRL, os),
                Some((PointerButton::Primary, Modifiers::CTRL))
            );
            map.release(MouseButton::Left, Modifiers::CTRL);
        }
        let cmd = Modifiers {
            meta: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            map.press(MouseButton::Left, cmd, Os::Mac),
            Some((PointerButton::Primary, cmd))
        );
        map.release(MouseButton::Left, cmd);
        assert_eq!(
            map.press(MouseButton::Right, Modifiers::CTRL, Os::Mac),
            Some((PointerButton::Secondary, Modifiers::CTRL)),
            "a real right click is untouched"
        );
        assert_eq!(map.press(MouseButton::Back, Modifiers::NONE, Os::Mac), None);
    }

    #[test]
    fn modifier_changes_are_emitted_once_per_real_change() {
        let mut t = ModifierTracker::default();
        assert_eq!(t.update(ModifiersState::empty()), None, "nothing changed");
        assert_eq!(
            t.update(ModifiersState::ALT),
            Some(UiEvent::ModifiersChanged {
                modifiers: Modifiers {
                    alt: true,
                    ..Modifiers::NONE
                }
            })
        );
        assert_eq!(t.update(ModifiersState::ALT), None, "same state again");
        assert!(
            t.update(ModifiersState::ALT | ModifiersState::CONTROL)
                .is_some()
        );
        assert!(t.current().alt && t.current().ctrl);
        assert_eq!(
            t.reset(),
            Some(UiEvent::ModifiersChanged {
                modifiers: Modifiers::NONE
            }),
            "focus loss clears the state"
        );
        assert_eq!(t.reset(), None);
    }

    #[test]
    fn wheel_deltas_become_notches() {
        assert_eq!(
            wheel(MouseScrollDelta::LineDelta(0.0, 1.0), 2.0),
            Vec2::new(0.0, 1.0)
        );
        let px = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 160.0));
        assert_eq!(wheel(px, 2.0), Vec2::new(0.0, 2.0));
        assert_eq!(wheel(px, 1.0), Vec2::new(0.0, 4.0));
    }

    #[test]
    fn positions_and_sizes_become_logical() {
        assert_eq!(
            logical(PhysicalPosition::new(300.0, 150.0), 1.5),
            Point::new(200.0, 100.0)
        );
        assert_eq!(
            logical_size(PhysicalSize::new(2000, 1400), 2.0),
            atomik_ui::kurbo::Size::new(1000.0, 700.0)
        );
    }

    #[test]
    fn the_files_of_one_drop_become_one_event() {
        let mut d = FileDrag::new();
        let at = Point::new(10.0, 20.0);
        assert!(!d.is_pending());
        assert_eq!(d.flush(at), None);
        for name in ["a.wav", "b.wav", "c.wav"] {
            d.hovered(name.into());
        }
        d.dropped("a.wav".into());
        d.dropped("b.wav".into());
        d.dropped("c.wav".into());
        assert!(d.is_pending());
        assert_eq!(
            d.flush(at),
            Some(UiEvent::FileDrop {
                pos: at,
                paths: vec!["a.wav".into(), "b.wav".into(), "c.wav".into()]
            }),
            "a drop wins over the hover that led to it, in drop order"
        );
        assert_eq!(d.flush(at), None, "nothing is repeated");
        assert!(!d.is_pending());
    }

    #[test]
    fn hovered_files_group_into_one_hover_and_cancel_clears_them() {
        let mut d = FileDrag::new();
        let at = Point::new(1.0, 2.0);
        d.hovered("a.wav".into());
        d.hovered("b.wav".into());
        assert_eq!(
            d.flush(at),
            Some(UiEvent::FileHover {
                pos: at,
                paths: vec!["a.wav".into(), "b.wav".into()]
            })
        );
        assert_eq!(d.flush(at), None);
        d.hover_cancelled();
        assert_eq!(d.flush(at), Some(UiEvent::FileHoverCancel));
        // A hover then a cancel in one iteration is just the cancel; a cancel then a new
        // hover is just the new hover.
        d.hovered("c.wav".into());
        d.hover_cancelled();
        assert_eq!(d.flush(at), Some(UiEvent::FileHoverCancel));
        d.hover_cancelled();
        d.hovered("d.wav".into());
        assert_eq!(
            d.flush(at),
            Some(UiEvent::FileHover {
                pos: at,
                paths: vec!["d.wav".into()]
            })
        );
    }

    #[test]
    fn a_later_drop_does_not_inherit_earlier_hovers() {
        let mut d = FileDrag::new();
        let at = Point::ZERO;
        d.hovered("a.wav".into());
        d.flush(at);
        d.dropped("a.wav".into());
        d.flush(at);
        d.dropped("z.wav".into());
        assert_eq!(
            d.flush(at),
            Some(UiEvent::FileDrop {
                pos: at,
                paths: vec!["z.wav".into()]
            })
        );
    }

    #[test]
    fn every_numpad_digit_and_every_main_row_digit_has_its_own_key() {
        let digits = [
            (KeyCode::Numpad0, KeyCode::Digit0),
            (KeyCode::Numpad1, KeyCode::Digit1),
            (KeyCode::Numpad2, KeyCode::Digit2),
            (KeyCode::Numpad3, KeyCode::Digit3),
            (KeyCode::Numpad4, KeyCode::Digit4),
            (KeyCode::Numpad5, KeyCode::Digit5),
            (KeyCode::Numpad6, KeyCode::Digit6),
            (KeyCode::Numpad7, KeyCode::Digit7),
            (KeyCode::Numpad8, KeyCode::Digit8),
            (KeyCode::Numpad9, KeyCode::Digit9),
        ];
        let cmd_opt = Modifiers {
            meta: true,
            alt: true,
            ..Modifiers::NONE
        };
        for (n, (pad, row)) in digits.into_iter().enumerate() {
            let n = u8::try_from(n).unwrap();
            let logical = WinitKey::Character(n.to_string().into());
            for os in [Os::Windows, Os::Mac, Os::Linux] {
                assert_eq!(
                    press_key(pad, logical.clone(), Modifiers::NONE, os),
                    Key::Numpad(NumpadKey::Digit(n)),
                    "{os:?} {pad:?}"
                );
            }
            assert_eq!(
                press_key(row, logical.clone(), cmd_opt, Os::Mac),
                Key::Digit(n),
                "{row:?}"
            );
            assert_eq!(
                press_key(row, logical, Modifiers::NONE, Os::Mac),
                Key::Char(char::from(b'0' + n)),
                "{row:?}"
            );
        }
    }
}
