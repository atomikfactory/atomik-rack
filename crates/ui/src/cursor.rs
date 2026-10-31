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
    /// A crosshair: a rubber band.
    Crosshair,
    /// A pen: the pencil tool.
    Pen,
    /// An eraser: the eraser tool.
    Eraser,
    /// A blade: the razor tool.
    Razor,
    /// A speaker: the speaker tool.
    Speaker,
    /// A magnifier: the magnifier tool.
    Magnifier,
    /// An open hand: the hand tool.
    Hand,
    /// A closed hand: the view is being dragged along.
    Grabbing,
}

impl CursorHint {
    /// The cursor of an edit tool over the editing area: the arrow for Select and Mute.
    pub fn of_tool(tool: crate::Tool) -> CursorHint {
        match tool {
            crate::Tool::Select | crate::Tool::Mute => CursorHint::Default,
            crate::Tool::Magnifier => CursorHint::Magnifier,
            crate::Tool::Hand => CursorHint::Hand,
            crate::Tool::Pencil => CursorHint::Pen,
            crate::Tool::Eraser => CursorHint::Eraser,
            crate::Tool::Razor => CursorHint::Razor,
            crate::Tool::Speaker => CursorHint::Speaker,
        }
    }
}
