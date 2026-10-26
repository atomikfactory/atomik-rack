//! The tooltips of the file browser: what the button under the pointer does.

use super::layout as lay;
use super::{FileBrowserView, Gesture};
use crate::action::Place;
use crate::tooltip::Help;
use atomik_library::SortKey;
use vello::kurbo::{Point, Rect};

impl FileBrowserView {
    /// What the button under `pos` does, for its tooltip (`None` during a drag, while a menu or
    /// an entry field is open, and away from a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.gesture != Gesture::None
            || self.menu.is_some()
            || self.entry.is_some()
            || !self.bounds.contains(pos)
        {
            return None;
        }
        let l = self.lay();
        if l.recursive_btn.contains(pos) {
            return Some(Help::new(
                l.recursive_btn,
                "Also list the files of the sub-folders",
            ));
        }
        if l.preview_btn.contains(pos) {
            return Some(Help::new(
                l.preview_btn,
                "Play an audio file when you select it",
            ));
        }
        if l.colhdr.contains(pos) {
            let cols = lay::columns(l.colhdr.x0, l.colhdr.x1, self.bounds.width());
            let col = |c: (f64, f64)| Rect::new(c.0, l.colhdr.y0, c.1, l.colhdr.y1);
            let sorts = [
                Some((cols.name, SortKey::Name)),
                Some((cols.duration, SortKey::Duration)),
                cols.size.map(|c| (c, SortKey::Size)),
            ];
            for (c, key) in sorts.into_iter().flatten() {
                if col(c).contains(pos) {
                    return Some(Help::new(
                        col(c),
                        match key {
                            SortKey::Name => "Sort by name: click again to reverse",
                            SortKey::Duration => "Sort by length: click again to reverse",
                            _ => "Sort by size: click again to reverse",
                        },
                    ));
                }
            }
        }
        if l.path.contains(pos)
            && let Some(Place::Folder(path)) = &self.location
        {
            let bar = lay::path_segments(path, l.path);
            if let Some(seg) = bar.segments.iter().find(|s| s.rect.contains(pos)) {
                return Some(Help::new(seg.rect, "Go to this folder"));
            }
        }
        None
    }
}
