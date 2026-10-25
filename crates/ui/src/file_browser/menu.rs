//! The painted context menu of the file browser (tags, favourite, bookmarks).

use std::path::PathBuf;
use vello::kurbo::{Point, Rect};

/// Height of one item.
pub(super) const ITEM_H: f64 = 22.0;
/// Height of a separator.
pub(super) const SEP_H: f64 = 8.0;
/// Estimated advance per character at the menu font size.
pub(super) const CHAR_W: f64 = 6.6;
/// Font size of the items.
pub(super) const FONT: f32 = 11.0;
const MIN_W: f64 = 130.0;

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Act {
    /// Add (`add`) or remove a tag on the targets.
    ToggleTag {
        /// Tag name.
        tag: String,
        /// True to add, false to remove.
        add: bool,
    },
    /// Start the "New tag" entry for the targets.
    NewTag,
    /// Toggle the favourite flag of the targets.
    ToggleFavorite,
    /// Bookmark a folder.
    Bookmark(PathBuf),
    /// Remove a bookmark.
    RemoveBookmark(PathBuf),
    /// Start the rename entry of a tag.
    RenameTag(String),
    /// Delete a tag everywhere.
    DeleteTag(String),
    /// Load a library item or device.
    Load(PathBuf),
    /// Open a library item as a project.
    OpenProject(PathBuf),
    /// Start the rename entry of a library item.
    RenameItem(PathBuf),
}

/// One row of a menu.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MenuItem {
    /// Shown text (empty for a separator).
    pub label: String,
    /// `Some(checked)` for a check item.
    pub checked: Option<bool>,
    /// Whether it can be chosen.
    pub enabled: bool,
    /// What it does; `None` marks a separator.
    pub act: Option<Act>,
}

impl MenuItem {
    /// An enabled plain item.
    pub fn new(label: &str, act: Act) -> Self {
        Self {
            label: label.to_string(),
            checked: None,
            enabled: true,
            act: Some(act),
        }
    }

    /// A separator.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            checked: None,
            enabled: false,
            act: None,
        }
    }
}

/// An open menu.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Menu {
    /// The rows.
    pub items: Vec<MenuItem>,
    /// Its rectangle (inside the panel).
    pub rect: Rect,
    /// The highlighted row.
    pub hover: Option<usize>,
    /// The files the menu acts on (the selected audio files).
    pub targets: Vec<PathBuf>,
}

impl Menu {
    /// Builds a menu at `at`, moved to stay inside `bounds`.
    pub fn new(items: Vec<MenuItem>, targets: Vec<PathBuf>, at: Point, bounds: Rect) -> Self {
        let chars = items
            .iter()
            .map(|i| i.label.chars().count())
            .max()
            .unwrap_or(0);
        let w = (chars as f64 * CHAR_W + 44.0).max(MIN_W);
        let h: f64 = items
            .iter()
            .map(|i| if i.act.is_some() { ITEM_H } else { SEP_H })
            .sum::<f64>()
            + 8.0;
        let x = at.x.min(bounds.x1 - w).max(bounds.x0);
        let y = at.y.min(bounds.y1 - h).max(bounds.y0);
        Self {
            items,
            rect: Rect::new(x, y, x + w, y + h),
            hover: None,
            targets,
        }
    }

    /// The rectangle of row `i`.
    pub fn item_rect(&self, i: usize) -> Rect {
        let mut y = self.rect.y0 + 4.0;
        for (k, item) in self.items.iter().enumerate() {
            let h = if item.act.is_some() { ITEM_H } else { SEP_H };
            if k == i {
                return Rect::new(self.rect.x0 + 3.0, y, self.rect.x1 - 3.0, y + h);
            }
            y += h;
        }
        Rect::ZERO
    }

    /// The row under `p`.
    pub fn item_at(&self, p: Point) -> Option<usize> {
        if !self.rect.contains(p) {
            return None;
        }
        (0..self.items.len()).find(|&i| self.item_rect(i).contains(p))
    }

    fn selectable(&self, i: usize) -> bool {
        self.items[i].act.is_some() && self.items[i].enabled
    }

    /// Moves the highlight by one selectable row (wrapping).
    pub fn step(&mut self, down: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut i = self.hover.unwrap_or(if down { n - 1 } else { 0 });
        for _ in 0..n {
            i = if down { (i + 1) % n } else { (i + n - 1) % n };
            if self.selectable(i) {
                self.hover = Some(i);
                return;
            }
        }
    }

    /// The action of the highlighted row, when it can be chosen.
    pub fn chosen(&self, i: usize) -> Option<Act> {
        self.selectable(i)
            .then(|| self.items[i].act.clone())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu::new(
            vec![
                MenuItem::new("808", Act::NewTag),
                MenuItem::separator(),
                MenuItem {
                    enabled: false,
                    ..MenuItem::new("Off", Act::ToggleFavorite)
                },
                MenuItem::new("New tag...", Act::NewTag),
            ],
            Vec::new(),
            Point::new(190.0, 390.0),
            Rect::new(0.0, 0.0, 200.0, 400.0),
        )
    }

    #[test]
    fn menu_is_kept_inside_the_bounds() {
        let m = menu();
        assert!(m.rect.x1 <= 200.0 && m.rect.y1 <= 400.0);
        assert!(m.rect.x0 >= 0.0 && m.rect.y0 >= 0.0);
    }

    #[test]
    fn rows_are_hit_tested_and_stepping_skips_separators_and_disabled() {
        let mut m = menu();
        let r = m.item_rect(3);
        assert_eq!(m.item_at(r.center()), Some(3));
        assert_eq!(m.item_at(Point::new(-5.0, -5.0)), None);
        m.step(true);
        assert_eq!(m.hover, Some(0));
        m.step(true);
        assert_eq!(m.hover, Some(3));
        m.step(true);
        assert_eq!(m.hover, Some(0));
        m.step(false);
        assert_eq!(m.hover, Some(3));
        assert!(m.chosen(2).is_none());
        assert!(m.chosen(3).is_some());
    }
}
