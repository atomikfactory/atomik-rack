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
}

impl CursorHint {
    /// The cursor of an edit tool over the editing area: the arrow for Select.
    pub fn of_tool(tool: crate::Tool) -> CursorHint {
        match tool {
            crate::Tool::Select => CursorHint::Default,
            crate::Tool::Pencil => CursorHint::Pen,
            crate::Tool::Eraser => CursorHint::Eraser,
            crate::Tool::Razor => CursorHint::Razor,
        }
    }
}
