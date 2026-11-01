//! The mouse cursor of a drag in progress: what the shell shows while files are dragged out of
//! the file browser, and while a drag that started in one pane is over another.

use atomik_ui::CursorHint;

/// The cursor of a file drag over a place. `has_target`: the place takes files (a pane of the
/// shell that a release delivers to; the file browser itself and the bars do not). `library`:
/// every dragged file is a library item (a patch, a container, a device row, a clip), which
/// only the rack and the note lanes take. `library_ok`: the place under the pointer takes such
/// an item right here (the rack anywhere; the arrangement only on a note lane for a clip).
///
/// A place that takes the drop shows the copy cursor (the files are imported or inserted, the
/// originals stay); every other place the not-allowed one.
pub fn file_drop_hint(has_target: bool, library: bool, library_ok: bool) -> CursorHint {
    if has_target && (!library || library_ok) {
        CursorHint::Copy
    } else {
        CursorHint::NotAllowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_that_takes_the_files_copies_and_every_other_place_refuses() {
        // Audio files over a pane that imports them.
        assert_eq!(file_drop_hint(true, false, false), CursorHint::Copy);
        // Library items over the rack, or over a note lane.
        assert_eq!(file_drop_hint(true, true, true), CursorHint::Copy);
        // Library items over a pane that does not take them.
        assert_eq!(file_drop_hint(true, true, false), CursorHint::NotAllowed);
        // The file browser itself, the bars, outside every window.
        assert_eq!(file_drop_hint(false, false, false), CursorHint::NotAllowed);
        assert_eq!(file_drop_hint(false, true, true), CursorHint::NotAllowed);
    }
}
