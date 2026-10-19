//! Platform-free input events, in logical units.
//!
//! The app translates its window-system events (e.g. winit) into [`UiEvent`]s. Positions are
//! logical pixels relative to the top-left of the view (physical pixels divided by the scale
//! factor). No time source is involved: double-clicks arrive as a click count computed by the
//! app.

use std::path::PathBuf;
use vello::kurbo::{Point, Size, Vec2};

/// A pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    /// Primary (left) button.
    Primary,
    /// Secondary (right) button.
    Secondary,
    /// Middle button.
    Middle,
}

/// Keyboard modifier state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift (fine adjustment on knobs and faders).
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt / Option.
    pub alt: bool,
    /// Meta / Command / Windows key.
    pub meta: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };
    /// Shift only.
    pub const SHIFT: Self = Self {
        shift: true,
        ctrl: false,
        alt: false,
        meta: false,
    };
    /// Ctrl only.
    pub const CTRL: Self = Self {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };

    /// The shortcut modifier: Ctrl, or Command (meta) on macOS. Either one is accepted.
    pub fn command(self) -> bool {
        self.ctrl || self.meta
    }
}

/// A key, independent of the window system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A printable character (already case-mapped by the app: `'z'` or `'Z'`).
    Char(char),
    /// Escape.
    Escape,
    /// Enter / Return.
    Enter,
    /// Space bar.
    Space,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Tab.
    Tab,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// A function key: `F(5)` is F5 (1..=24).
    F(u8),
    /// Insert.
    Insert,
    /// A numpad key, distinct from the same character on the main row (NumLock on or off).
    Numpad(NumpadKey),
    /// A main-row digit by its **physical** position (`Digit(1)` is the key left of `2`,
    /// whatever it types). The app sends it instead of [`Key::Char`] only while Alt is held
    /// (on macOS Option changes the typed character), so chords can bind it (`Digit1`); plain
    /// digit typing stays [`Key::Char`].
    Digit(u8),
    /// Any other key (ignored).
    Other,
}

/// A numpad key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NumpadKey {
    /// A digit key, `0..=9`.
    Digit(u8),
    /// Numpad Enter.
    Enter,
    /// Numpad `+`.
    Add,
    /// Numpad `-`.
    Subtract,
    /// Numpad `*`.
    Multiply,
    /// Numpad `/`.
    Divide,
    /// Numpad `.`.
    Decimal,
}

/// An input or window event, in logical units.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// The pointer moved.
    PointerMove {
        /// Position in logical pixels.
        pos: Point,
        /// Modifiers held during the move (Shift = fine drag).
        modifiers: Modifiers,
    },
    /// A button was pressed.
    PointerDown {
        /// Position in logical pixels.
        pos: Point,
        /// Button.
        button: PointerButton,
        /// Modifiers.
        modifiers: Modifiers,
        /// Click count computed by the app: 1 for a single click, 2 for a double-click, …
        clicks: u8,
    },
    /// A button was released.
    PointerUp {
        /// Position in logical pixels.
        pos: Point,
        /// Button.
        button: PointerButton,
        /// Modifiers.
        modifiers: Modifiers,
    },
    /// The pointer left the view.
    PointerLeave,
    /// Mouse wheel or touchpad scroll.
    Wheel {
        /// Scroll amount in wheel notches (lines). Positive `y` = away from the user
        /// (scroll up / increase). Apps convert pixel deltas (e.g. divide by 40).
        delta: Vec2,
        /// Pointer position in logical pixels.
        pos: Point,
        /// Modifiers (Ctrl = zoom, Shift = fine).
        modifiers: Modifiers,
    },
    /// A key was pressed or released.
    Key {
        /// The key.
        key: Key,
        /// Modifiers.
        modifiers: Modifiers,
        /// `true` on press (and auto-repeat), `false` on release. Releases are delivered for every
        /// key, modifiers included (which the app maps to [`Key::Other`]); the shell derives key
        /// repeat from the keys it sees held.
        pressed: bool,
    },
    /// The modifier state changed (sent on every real change, before any event that carries the
    /// new state). Lets hold-style behaviour (the alternate-tool role) react without a pointer
    /// move or a key of its own.
    ModifiersChanged {
        /// The new modifiers.
        modifiers: Modifiers,
    },
    /// The display scale factor changed (e.g. moving to another monitor).
    ScaleChanged(f64),
    /// The view was resized; size in logical pixels.
    Resize(Size),
    /// Files are being dragged over the window (drag and drop from the OS). Sent for every
    /// pointer movement while hovering; the paths are opaque to the UI.
    FileHover {
        /// Position in logical pixels.
        pos: Point,
        /// The files being dragged.
        paths: Vec<PathBuf>,
    },
    /// The file drag left the window or was cancelled.
    FileHoverCancel,
    /// Files were dropped.
    FileDrop {
        /// Position in logical pixels.
        pos: Point,
        /// The dropped files.
        paths: Vec<PathBuf>,
    },
}
