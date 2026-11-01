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
    /// A closed hand over a drag that carries something to a drop place (a device, a clip, a
    /// track, files): the item moves where it is let go.
    Dragging,
    /// The copy cursor: letting go here copies or inserts what is dragged.
    Copy,
    /// The not-allowed cursor: letting go here does nothing.
    NotAllowed,
}

impl CursorHint {
    /// The hint of a drag that carries something while the pointer is not over the pane the
    /// drag started in: a drop there is refused, so the drag shows [`CursorHint::NotAllowed`].
    /// Every other hint (a pan, a resize, a tool) is the pane's own business and stays.
    pub fn outside_own_pane(self) -> CursorHint {
        match self {
            CursorHint::Dragging | CursorHint::Copy => CursorHint::NotAllowed,
            other => other,
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_carrying_drag_outside_its_pane_is_not_allowed_and_other_hints_stay() {
        assert_eq!(
            CursorHint::Dragging.outside_own_pane(),
            CursorHint::NotAllowed
        );
        assert_eq!(CursorHint::Copy.outside_own_pane(), CursorHint::NotAllowed);
        for stay in [
            CursorHint::Default,
            CursorHint::Grabbing,
            CursorHint::ResizeHorizontal,
            CursorHint::Pen,
            CursorHint::NotAllowed,
        ] {
            assert_eq!(stay.outside_own_pane(), stay);
        }
    }
}
