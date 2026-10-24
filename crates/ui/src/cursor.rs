//! The mouse cursor a view asks the host to show.
//!
//! A hint, not a window-system type: the host (the winit layer of the app) maps it to its own
//! cursor icon after every event, so the views stay free of the window system.

/// The cursor shape for the pointer's current position and gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorHint {
    /// The arrow.
    #[default]
    Default,
    /// Left-right arrows: an edge that resizes horizontally.
    ResizeHorizontal,
    /// The four-way move cursor: a body that can be dragged.
    Move,
    /// A crosshair: the pencil, a rubber band.
    Crosshair,
}
