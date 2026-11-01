//! Event handling of the file browser: pointer, wheel, keyboard, menus and info requests.

use super::field::FieldEvent;
use super::layout::{self as lay, Layout};
use super::menu::{Act, Menu, MenuItem};
use super::{Entry, EntryKind, FileBrowserView, Gesture, Hover, Pane, TextField, tree};
use crate::action::{BrowseAction, Place, UiAction};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use atomik_library::{FileKind, FileRow, Query, SortKey};
use std::path::PathBuf;
use vello::kurbo::{Point, Rect};

/// Pointer travel (logical pixels) after which a press on a file starts a file drag.
const DRAG_THRESHOLD: f64 = 6.0;

/// The most folders the path pop-up lists.
const MAX_POPUP_FOLDERS: usize = 24;

fn emit(out: &mut Vec<UiAction>, action: BrowseAction) {
    out.push(UiAction::Browse(action));
}

fn is_audio(row: &FileRow) -> bool {
    matches!(row.kind, FileKind::Audio(_))
}

/// Whether the row is a file (audio or library item): what a star, a tag and a drag apply to.
fn is_file(row: &FileRow) -> bool {
    row.kind != FileKind::Folder
}

/// Whether the info probe reads the row: audio headers and the item files (a device row
/// carries its detail and is never a file).
fn wants_info(row: &FileRow) -> bool {
    match row.kind {
        FileKind::Audio(_) => row.info.is_none(),
        FileKind::Patch | FileKind::Container | FileKind::Clip | FileKind::Project => {
            row.item.is_none()
        }
        FileKind::Folder | FileKind::Device => false,
    }
}

impl FileBrowserView {
    pub(super) fn dispatch(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) {
        if self.menu.is_some() && self.menu_event(event, out) {
            return;
        }
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::Resize(_) => {
                self.dirty = true;
                self.info_dirty = true;
            }
            UiEvent::PointerLeave => {
                if self.hover != Hover::None {
                    self.hover = Hover::None;
                    self.dirty = true;
                }
            }
            UiEvent::PointerMove { pos, .. } => self.pointer_move(*pos, out),
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => match button {
                PointerButton::Primary => self.primary_down(*pos, *modifiers, *clicks, out),
                PointerButton::Secondary => self.secondary_down(*pos),
                PointerButton::Middle => {}
            },
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.pointer_up(out),
            UiEvent::Wheel { delta, pos, .. } => self.wheel(delta.y, *pos),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key_down(key, *modifiers, out),
            _ => {}
        }
    }

    // ----- scrolling ----------------------------------------------------------------------

    fn set_list_scroll(&mut self, scroll: f64) {
        let s = scroll.clamp(0.0, self.max_list_scroll());
        if (s - self.list_scroll).abs() > f64::EPSILON {
            self.list_scroll = s;
            self.dirty = true;
            self.info_dirty = true;
        }
    }

    fn set_tree_scroll(&mut self, scroll: f64) {
        let s = scroll.clamp(0.0, self.max_tree_scroll());
        if (s - self.tree_scroll).abs() > f64::EPSILON {
            self.tree_scroll = s;
            self.dirty = true;
        }
    }

    fn ensure_row_visible(&mut self, pos: usize) {
        let view_h = self.lay().list.height();
        let top = pos as f64 * lay::ROW_H;
        let bottom = top + lay::ROW_H;
        if top < self.list_scroll {
            self.set_list_scroll(top);
        } else if bottom > self.list_scroll + view_h {
            self.set_list_scroll(bottom - view_h);
        }
    }

    fn ensure_tree_visible(&mut self, idx: usize) {
        let view_h = self.lay().tree.height();
        let top = idx as f64 * lay::TREE_ROW_H;
        let bottom = top + lay::TREE_ROW_H;
        if top < self.tree_scroll {
            self.set_tree_scroll(top);
        } else if bottom > self.tree_scroll + view_h {
            self.set_tree_scroll(bottom - view_h);
        }
    }

    fn wheel(&mut self, dy: f64, pos: Point) {
        if !self.bounds.contains(pos) {
            return;
        }
        let lay = self.lay();
        if pos.y < lay.tree.y1 {
            self.set_tree_scroll(self.tree_scroll - dy * lay::TREE_ROW_H * 3.0);
        } else {
            self.set_list_scroll(self.list_scroll - dy * lay::ROW_H * 3.0);
        }
    }

    // ----- hit testing --------------------------------------------------------------------

    fn list_pos_at(&self, pos: Point, lay: &Layout) -> Option<usize> {
        if !lay.list.contains(pos) {
            return None;
        }
        let i = ((pos.y - lay.list.y0 + self.list_scroll) / lay::ROW_H).floor();
        (i >= 0.0 && (i as usize) < self.view.len()).then_some(i as usize)
    }

    fn tree_idx_at(&self, pos: Point, lay: &Layout) -> Option<usize> {
        if !lay.tree.contains(pos) {
            return None;
        }
        let i = ((pos.y - lay.tree.y0 + self.tree_scroll) / lay::TREE_ROW_H).floor();
        (i >= 0.0 && (i as usize) < self.tree_items.len()).then_some(i as usize)
    }

    fn list_track(&self, lay: &Layout) -> Rect {
        Rect::new(
            lay.list.x1 - lay::SB_W,
            lay.list.y0,
            lay.list.x1,
            lay.list.y1,
        )
    }

    fn tree_track(&self, lay: &Layout) -> Rect {
        Rect::new(
            lay.tree.x1 - lay::SB_W,
            lay.tree.y0,
            lay.tree.x1,
            lay.tree.y1,
        )
    }

    // ----- pointer ------------------------------------------------------------------------

    fn pointer_move(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        let lay = self.lay();
        match self.gesture.clone() {
            Gesture::Resize => {
                self.set_width(pos.x - self.bounds.x0);
                self.bounds.x1 = self.bounds.x0 + self.width;
                self.clamp_scrolls();
                self.info_dirty = true;
            }
            Gesture::Splitter => {
                let avail = (self.bounds.y1 - lay.header.y1).max(1.0);
                self.set_tree_fraction((pos.y - lay::SPLIT_H / 2.0 - lay.header.y1) / avail);
            }
            Gesture::Thumb { pane, grab } => match pane {
                Pane::List => {
                    let s = lay::scroll_from_thumb(
                        self.list_track(&lay),
                        self.content_h(),
                        pos.y - grab,
                    );
                    self.set_list_scroll(s);
                }
                Pane::Tree => {
                    let content = self.tree_items.len() as f64 * lay::TREE_ROW_H;
                    let s = lay::scroll_from_thumb(self.tree_track(&lay), content, pos.y - grab);
                    self.set_tree_scroll(s);
                }
            },
            Gesture::RowPress { start, .. } => {
                if (pos - start).hypot() > DRAG_THRESHOLD {
                    self.gesture = Gesture::None;
                    let files = self.selected_entries();
                    if !files.is_empty() {
                        emit(out, BrowseAction::DragFiles(files));
                    }
                    self.dirty = true;
                }
            }
            Gesture::None => {
                let hover = if !self.bounds.contains(pos) {
                    Hover::None
                } else if let Some(i) = self.tree_idx_at(pos, &lay) {
                    Hover::Tree(i)
                } else if let Some(i) = self.list_pos_at(pos, &lay) {
                    Hover::Row(i)
                } else {
                    Hover::None
                };
                if hover != self.hover {
                    self.hover = hover;
                    self.dirty = true;
                }
            }
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        let gesture = std::mem::replace(&mut self.gesture, Gesture::None);
        match gesture {
            Gesture::Resize | Gesture::Splitter => emit(out, BrowseAction::LayoutChanged),
            Gesture::RowPress {
                collapse: Some(path),
                ..
            } => {
                self.cursor = self.row_of_path(&path);
                self.anchor = self.cursor;
                self.selection.clear();
                self.selection.insert(path.clone());
                if self.auto_preview {
                    emit(out, BrowseAction::Preview(path));
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn primary_down(&mut self, pos: Point, mods: Modifiers, clicks: u8, out: &mut Vec<UiAction>) {
        let lay = self.lay();
        self.dirty = true;
        if !self.bounds.contains(pos) {
            self.search_focused = false;
            self.entry = None;
            return;
        }
        self.focused = true;
        if !lay.search.contains(pos) {
            self.search_focused = false;
        }
        if self.entry.is_some() && !lay.path.contains(pos) {
            self.entry = None;
        }
        if lay.handle.contains(pos) {
            self.gesture = Gesture::Resize;
        } else if lay.search.contains(pos) {
            self.search_focused = true;
        } else if lay.recursive_btn.contains(pos) {
            self.recursive = !self.recursive;
            emit(out, BrowseAction::SetRecursive(self.recursive));
        } else if lay.preview_btn.contains(pos) {
            self.auto_preview = !self.auto_preview;
            emit(out, BrowseAction::SetAutoPreview(self.auto_preview));
        } else if lay.splitter.contains(pos) {
            self.gesture = Gesture::Splitter;
        } else if lay.tree.contains(pos) {
            self.pane = Pane::Tree;
            self.tree_down(pos, &lay, out);
        } else {
            self.pane = Pane::List;
            if lay.path.contains(pos) {
                self.path_down(pos, &lay, out);
            } else if lay.colhdr.contains(pos) {
                self.header_down(pos, &lay);
            } else if lay.list.contains(pos) {
                self.list_down(pos, mods, clicks, &lay, out);
            }
        }
    }

    fn path_down(&mut self, pos: Point, lay: &Layout, out: &mut Vec<UiAction>) {
        if self.entry.is_some() {
            return;
        }
        if lay.back_btn.contains(pos) {
            self.step_history(true, out);
        } else if lay.fwd_btn.contains(pos) {
            self.step_history(false, out);
        } else if let Some(Place::Folder(path)) = &self.location {
            let bar = lay::path_segments(path, lay.path_text);
            if let Some(seg) = bar.segments.iter().find(|s| s.rect.contains(pos)) {
                emit(out, BrowseAction::Show(Place::Folder(seg.path.clone())));
            } else if bar.ellipsis && bar.segments.first().is_some_and(|s| pos.x < s.rect.x0) {
                // The shortened start of the path: offer every folder above.
                self.open_path_menu(pos);
            }
        }
    }

    /// Goes one place back or forward in the history (the answer to the `Show` this emits
    /// is not a new branch).
    fn step_history(&mut self, back: bool, out: &mut Vec<UiAction>) {
        let target = if back {
            self.history_at.checked_sub(1)
        } else {
            (self.history_at + 1 < self.history.len()).then_some(self.history_at + 1)
        };
        if let Some(at) = target {
            self.history_jump = Some(at);
            emit(out, BrowseAction::Show(self.history[at].clone()));
        }
    }

    /// The path pop-up: the folder shown and every folder above it, nearest first.
    fn open_path_menu(&mut self, at: Point) {
        let Some(Place::Folder(path)) = &self.location else {
            return;
        };
        let items: Vec<MenuItem> = path
            .ancestors()
            .take(MAX_POPUP_FOLDERS)
            .filter(|a| !a.as_os_str().is_empty())
            .map(|a| {
                let label = a.file_name().map_or_else(
                    || a.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                MenuItem::new(&label, Act::Go(a.to_path_buf()))
            })
            .collect();
        if !items.is_empty() {
            self.menu = Some(Menu::new(items, Vec::new(), at, self.bounds));
        }
    }

    fn header_down(&mut self, pos: Point, lay: &Layout) {
        let cols = lay::columns(lay.colhdr.x0, lay.colhdr.x1, self.bounds.width());
        let within = |c: (f64, f64)| pos.x >= c.0 && pos.x < c.1;
        let key = if within(cols.name) {
            Some(SortKey::Name)
        } else if within(cols.duration) {
            Some(SortKey::Duration)
        } else if cols.size.is_some_and(within) {
            Some(SortKey::Size)
        } else {
            None
        };
        if let Some(key) = key {
            if key == self.sort_key {
                self.descending = !self.descending;
            } else {
                self.sort_key = key;
                self.descending = false;
            }
            self.resort();
        }
    }

    fn list_down(
        &mut self,
        pos: Point,
        mods: Modifiers,
        clicks: u8,
        lay: &Layout,
        out: &mut Vec<UiAction>,
    ) {
        let track = self.list_track(lay);
        if track.contains(pos)
            && let Some(thumb) = lay::scroll_thumb(track, self.content_h(), self.list_scroll)
        {
            let grab = if thumb.contains(pos) {
                pos.y - thumb.y0
            } else {
                let g = thumb.height() / 2.0;
                let s = lay::scroll_from_thumb(track, self.content_h(), pos.y - g);
                self.set_list_scroll(s);
                g
            };
            self.gesture = Gesture::Thumb {
                pane: Pane::List,
                grab,
            };
            return;
        }
        let Some(vp) = self.list_pos_at(pos, lay) else {
            return;
        };
        let row_idx = self.view[vp];
        let (path, audio, file, item, patch) = {
            let r = &self.rows[row_idx as usize];
            (
                r.path.clone(),
                is_audio(r),
                is_file(r),
                r.kind.is_item(),
                r.kind == FileKind::Patch,
            )
        };
        let cols = lay::columns(lay.list.x0, lay.list.x1, self.bounds.width());
        if file && pos.x >= cols.star.0 && pos.x < cols.star.1 {
            emit(out, BrowseAction::ToggleFavorite(vec![path]));
            return;
        }
        if !file && clicks >= 2 {
            emit(out, BrowseAction::Show(Place::Folder(path)));
            return;
        }
        if item && clicks >= 2 {
            emit(out, BrowseAction::Load(path));
            return;
        }
        let mut collapse = None;
        if mods.shift {
            let from = self.anchor.and_then(|a| self.pos_of(a)).unwrap_or(vp);
            self.select_range(from, vp);
            self.cursor = Some(row_idx);
            self.anchor.get_or_insert(row_idx);
        } else if mods.command() {
            if !self.selection.remove(&path) {
                self.selection.insert(path.clone());
            }
            self.cursor = Some(row_idx);
            self.anchor = Some(row_idx);
        } else if self.selection.len() > 1 && self.selection.contains(&path) {
            // Keep the group so it can be dragged; collapse on release if it was a click.
            collapse = Some(path.clone());
            self.cursor = Some(row_idx);
        } else {
            self.selection.clear();
            self.selection.insert(path.clone());
            self.cursor = Some(row_idx);
            self.anchor = Some(row_idx);
            if self.target.is_some() && patch && clicks <= 1 {
                emit(out, BrowseAction::Audition(path));
            } else if audio && clicks <= 1 && self.auto_preview {
                emit(out, BrowseAction::Preview(path));
            }
        }
        if file {
            self.gesture = Gesture::RowPress {
                start: pos,
                collapse,
            };
        }
    }

    fn select_range(&mut self, a: usize, b: usize) {
        let (lo, hi) = (a.min(b), a.max(b));
        self.selection = self.view[lo..=hi]
            .iter()
            .map(|&i| self.rows[i as usize].path.clone())
            .collect();
    }

    fn tree_down(&mut self, pos: Point, lay: &Layout, out: &mut Vec<UiAction>) {
        let track = self.tree_track(lay);
        let content = self.tree_items.len() as f64 * lay::TREE_ROW_H;
        if track.contains(pos)
            && let Some(thumb) = lay::scroll_thumb(track, content, self.tree_scroll)
        {
            let grab = if thumb.contains(pos) {
                pos.y - thumb.y0
            } else {
                let g = thumb.height() / 2.0;
                self.set_tree_scroll(lay::scroll_from_thumb(track, content, pos.y - g));
                g
            };
            self.gesture = Gesture::Thumb {
                pane: Pane::Tree,
                grab,
            };
            return;
        }
        let Some(idx) = self.tree_idx_at(pos, lay) else {
            return;
        };
        let item = self.tree_items[idx].clone();
        if !item.key.selectable() {
            return;
        }
        self.tree_cursor = Some(item.key.clone());
        let arrow_x = lay.tree.x0 + lay::PAD + f64::from(item.depth) * lay::INDENT;
        if item.expandable && (pos.x < arrow_x + lay::ARROW_W || item.key == tree::Key::TagsHeader)
        {
            self.toggle_item(&item.key, out);
        } else if let Some(place) = item.key.place() {
            emit(out, BrowseAction::Show(place));
        }
    }

    fn toggle_item(&mut self, key: &tree::Key, out: &mut Vec<UiAction>) {
        match key {
            tree::Key::TagsHeader => self.tags_open = !self.tags_open,
            tree::Key::Folder { path, .. } => {
                if !self.expanded.remove(path) {
                    self.expanded.insert(path.clone());
                    if !self.children.contains_key(path)
                        && self.requested_children.insert(path.clone())
                    {
                        emit(out, BrowseAction::ListChildren(path.clone()));
                    }
                }
            }
            _ => return,
        }
        self.rebuild_tree();
    }

    fn secondary_down(&mut self, pos: Point) {
        let lay = self.lay();
        if !self.bounds.contains(pos) {
            return;
        }
        self.focused = true;
        self.search_focused = false;
        self.entry = None;
        self.dirty = true;
        if let Some(idx) = self.tree_idx_at(pos, &lay) {
            self.pane = Pane::Tree;
            self.open_tree_menu(idx, pos);
        } else if let Some(vp) = self.list_pos_at(pos, &lay) {
            self.pane = Pane::List;
            self.open_list_menu(vp, pos);
        } else if lay.path.contains(pos) {
            self.pane = Pane::List;
            self.open_path_menu(pos);
        }
    }

    fn open_tree_menu(&mut self, idx: usize, at: Point) {
        let item = self.tree_items[idx].clone();
        let items = match &item.key {
            tree::Key::Tag(name) => vec![
                MenuItem::new("Rename...", Act::RenameTag(name.clone())),
                MenuItem::new("Delete", Act::DeleteTag(name.clone())),
            ],
            tree::Key::List(name) => vec![
                MenuItem::new("Rename...", Act::RenameList(name.clone())),
                MenuItem::new("Delete", Act::DeleteList(name.clone())),
            ],
            tree::Key::Favorites => vec![MenuItem::new("New list...", Act::NewList)],
            tree::Key::Folder { path, .. } => {
                if self.places.bookmarks.contains(path) {
                    vec![MenuItem::new(
                        "Remove bookmark",
                        Act::RemoveBookmark(path.clone()),
                    )]
                } else {
                    vec![MenuItem::new("Bookmark", Act::Bookmark(path.clone()))]
                }
            }
            _ => return,
        };
        self.tree_cursor = Some(item.key);
        self.menu = Some(Menu::new(items, Vec::new(), at, self.bounds));
    }

    fn open_list_menu(&mut self, vp: usize, at: Point) {
        let row_idx = self.view[vp];
        let path = self.rows[row_idx as usize].path.clone();
        if !self.selection.contains(&path) {
            self.selection.clear();
            self.selection.insert(path.clone());
            self.cursor = Some(row_idx);
            self.anchor = Some(row_idx);
        }
        let targets = self.selected_entries();
        let folder = (self.rows[row_idx as usize].kind == FileKind::Folder).then(|| path.clone());
        // How many of the targets carry each tag.
        let mut counts = vec![0usize; self.places.tags.len()];
        if !targets.is_empty() {
            for &i in &self.view {
                let r = &self.rows[i as usize];
                if is_file(r) && self.selection.contains(&r.path) {
                    for (k, (name, _)) in self.places.tags.iter().enumerate() {
                        if r.tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
                            counts[k] += 1;
                        }
                    }
                }
            }
        }
        let have = !targets.is_empty();
        let mut items: Vec<MenuItem> = Vec::new();
        let kind = self.rows[row_idx as usize].kind;
        if kind.is_item() {
            items.push(MenuItem::new("Load", Act::Load(path.clone())));
            if matches!(
                kind,
                FileKind::Patch | FileKind::Container | FileKind::Project
            ) {
                items.push(MenuItem::new(
                    "Open as Project",
                    Act::OpenProject(path.clone()),
                ));
            }
            if kind != FileKind::Device && self.inside_library(&path) {
                items.push(MenuItem::new("Rename...", Act::RenameItem(path.clone())));
            }
            items.push(MenuItem::separator());
        }
        for (k, (name, _)) in self.places.tags.iter().enumerate() {
            let checked = have && counts[k] == targets.len();
            items.push(MenuItem {
                label: name.clone(),
                checked: Some(checked),
                enabled: have,
                act: Some(Act::ToggleTag {
                    tag: name.clone(),
                    add: !checked,
                }),
            });
        }
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.push(MenuItem {
            enabled: have,
            ..MenuItem::new("New tag...", Act::NewTag)
        });
        items.push(MenuItem {
            enabled: have,
            ..MenuItem::new("Toggle favourite", Act::ToggleFavorite)
        });
        items.push(MenuItem::separator());
        items.push(MenuItem {
            enabled: have,
            ..MenuItem::new("Edit Author...", Act::EditAuthor)
        });
        items.push(MenuItem {
            enabled: have,
            ..MenuItem::new("Edit Categories...", Act::EditCategories)
        });
        items.push(MenuItem::separator());
        for (name, _) in &self.places.lists {
            items.push(MenuItem {
                enabled: have,
                ..MenuItem::new(&format!("Add to {name}"), Act::AddToList(name.clone()))
            });
        }
        items.push(MenuItem {
            enabled: have,
            ..MenuItem::new("New list...", Act::NewList)
        });
        if let Some(Place::List(name)) = &self.location {
            items.push(MenuItem {
                enabled: have,
                ..MenuItem::new(
                    &format!("Remove from {name}"),
                    Act::RemoveFromList(name.clone()),
                )
            });
        }
        if kind != FileKind::Device {
            items.push(MenuItem::separator());
            items.push(MenuItem::new("Show in Folder", Act::Reveal(path.clone())));
        }
        if let Some(folder) = folder {
            items.push(MenuItem::separator());
            if self.places.bookmarks.contains(&folder) {
                items.push(MenuItem::new(
                    "Remove bookmark",
                    Act::RemoveBookmark(folder),
                ));
            } else {
                items.push(MenuItem::new("Bookmark folder", Act::Bookmark(folder)));
            }
        }
        self.menu = Some(Menu::new(items, targets, at, self.bounds));
    }

    // ----- menu ---------------------------------------------------------------------------

    fn menu_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let hover = menu.item_at(*pos).filter(|&i| menu.chosen(i).is_some());
                if hover != menu.hover {
                    menu.hover = hover;
                    self.dirty = true;
                }
            }
            UiEvent::PointerDown { pos, .. } => {
                let hit = menu.item_at(*pos);
                let act = hit.and_then(|i| menu.chosen(i));
                if hit.is_some() && act.is_none() {
                    return true;
                }
                self.close_menu_and_run(act, out);
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => self.close_menu_and_run(None, out),
                Key::Up => menu.step(false),
                Key::Down => menu.step(true),
                Key::Enter => {
                    let act = menu.hover.and_then(|i| menu.chosen(i));
                    self.close_menu_and_run(act, out);
                }
                _ => {}
            },
            UiEvent::PointerUp { .. } | UiEvent::Wheel { .. } | UiEvent::Key { .. } => {}
            _ => return false,
        }
        self.dirty = true;
        true
    }

    fn close_menu_and_run(&mut self, act: Option<Act>, out: &mut Vec<UiAction>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.dirty = true;
        let targets = menu.targets;
        match act {
            None => {}
            Some(Act::ToggleTag { tag, add }) => {
                let action = if add {
                    BrowseAction::AddTag {
                        paths: targets,
                        tag,
                    }
                } else {
                    BrowseAction::RemoveTag {
                        paths: targets,
                        tag,
                    }
                };
                emit(out, action);
            }
            Some(Act::NewTag) => self.begin_entry(EntryKind::NewTag, "", targets),
            Some(Act::ToggleFavorite) => emit(out, BrowseAction::ToggleFavorite(targets)),
            Some(Act::Bookmark(p)) => emit(out, BrowseAction::AddPlace(p)),
            Some(Act::RemoveBookmark(p)) => emit(out, BrowseAction::RemovePlace(p)),
            Some(Act::RenameTag(t)) => {
                let initial = t.clone();
                self.begin_entry(EntryKind::RenameTag(t), &initial, Vec::new());
            }
            Some(Act::DeleteTag(t)) => emit(out, BrowseAction::DeleteTag(t)),
            Some(Act::Load(p)) => emit(out, BrowseAction::Load(p)),
            Some(Act::OpenProject(p)) => emit(out, BrowseAction::OpenProject(p)),
            Some(Act::RenameItem(p)) => self.begin_rename_item(p),
            Some(Act::Go(p)) => emit(out, BrowseAction::Show(Place::Folder(p))),
            Some(Act::Reveal(p)) => emit(out, BrowseAction::Reveal(p)),
            Some(Act::EditAuthor) => {
                let initial = self.first_target_row(&targets).map(|r| r.author.clone());
                self.begin_entry(EntryKind::Author, &initial.unwrap_or_default(), targets);
            }
            Some(Act::EditCategories) => {
                let initial = self
                    .first_target_row(&targets)
                    .map(|r| r.categories.join(", "));
                self.begin_entry(EntryKind::Categories, &initial.unwrap_or_default(), targets);
            }
            Some(Act::AddToList(list)) => emit(
                out,
                BrowseAction::AddToList {
                    paths: targets,
                    list,
                },
            ),
            Some(Act::RemoveFromList(list)) => emit(
                out,
                BrowseAction::RemoveFromList {
                    paths: targets,
                    list,
                },
            ),
            Some(Act::NewList) => self.begin_entry(EntryKind::NewList, "", targets),
            Some(Act::RenameList(l)) => {
                let initial = l.clone();
                self.begin_entry(EntryKind::RenameList(l), &initial, Vec::new());
            }
            Some(Act::DeleteList(l)) => emit(out, BrowseAction::DeleteList(l)),
        }
    }

    /// The row of the first of `targets` (the one an entry is prefilled from).
    fn first_target_row(&self, targets: &[PathBuf]) -> Option<&FileRow> {
        let first = targets.first()?;
        self.rows.iter().find(|r| &r.path == first)
    }

    /// Starts the rename entry of a library item with its file stem selected.
    pub(super) fn begin_rename_item(&mut self, path: PathBuf) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.begin_entry(EntryKind::RenameItem, &stem, vec![path]);
    }

    fn begin_entry(&mut self, kind: EntryKind, initial: &str, targets: Vec<PathBuf>) {
        self.search_focused = false;
        self.focused = true;
        self.entry = Some(Entry {
            kind,
            field: TextField::with_text(initial, true),
            targets,
        });
    }

    // ----- keyboard -----------------------------------------------------------------------

    fn key_down(&mut self, key: &Key, mods: Modifiers, out: &mut Vec<UiAction>) {
        if !self.focused {
            return;
        }
        self.dirty = true;
        if self.entry.is_some() {
            self.entry_key(key, mods, out);
        } else if self.search_focused {
            self.search_key(key, mods, out);
        } else if mods.command() && matches!(key, Key::Char(c) if c.eq_ignore_ascii_case(&'f')) {
            self.focus_search();
        } else {
            match self.pane {
                Pane::Tree => self.tree_key(key, out),
                Pane::List => self.list_key(key, mods, out),
            }
        }
    }

    fn entry_key(&mut self, key: &Key, mods: Modifiers, out: &mut Vec<UiAction>) {
        let Some(entry) = self.entry.as_mut() else {
            return;
        };
        match entry.field.key(key, mods) {
            FieldEvent::Escape => self.entry = None,
            FieldEvent::Enter => {
                let Some(entry) = self.entry.take() else {
                    return;
                };
                let text = entry.field.text().trim().to_string();
                // An empty author or category list clears it; every other entry needs a name.
                match entry.kind {
                    EntryKind::Author if !entry.targets.is_empty() => {
                        emit(
                            out,
                            BrowseAction::SetAuthor {
                                paths: entry.targets,
                                author: text,
                            },
                        );
                        return;
                    }
                    EntryKind::Categories if !entry.targets.is_empty() => {
                        emit(
                            out,
                            BrowseAction::SetCategories {
                                paths: entry.targets,
                                categories: text,
                            },
                        );
                        return;
                    }
                    _ => {}
                }
                if text.is_empty() {
                    return;
                }
                match entry.kind {
                    EntryKind::NewList if entry.targets.is_empty() => {
                        emit(out, BrowseAction::NewList(text));
                    }
                    EntryKind::NewList => emit(
                        out,
                        BrowseAction::AddToList {
                            paths: entry.targets,
                            list: text,
                        },
                    ),
                    EntryKind::RenameList(from) if from != text => {
                        emit(out, BrowseAction::RenameList { from, to: text });
                    }
                    EntryKind::NewTag if !entry.targets.is_empty() => emit(
                        out,
                        BrowseAction::AddTag {
                            paths: entry.targets,
                            tag: text,
                        },
                    ),
                    EntryKind::RenameTag(from) if from != text => {
                        emit(out, BrowseAction::RenameTag { from, to: text });
                    }
                    EntryKind::RenameItem => {
                        if let Some(path) = entry.targets.into_iter().next()
                            && path.file_stem().is_none_or(|s| s.to_string_lossy() != text)
                        {
                            emit(out, BrowseAction::RenameItem { path, to: text });
                        }
                    }
                    _ => {}
                }
            }
            FieldEvent::Edited | FieldEvent::Ignored => {}
        }
    }

    fn search_key(&mut self, key: &Key, mods: Modifiers, out: &mut Vec<UiAction>) {
        match self.search.key(key, mods) {
            FieldEvent::Escape => {
                if self.search.clear() {
                    self.query_changed();
                }
                self.search_focused = false;
            }
            FieldEvent::Enter => self.search_focused = false,
            FieldEvent::Edited => self.query_changed(),
            FieldEvent::Ignored => {
                if matches!(key, Key::Up | Key::Down) {
                    self.search_focused = false;
                    self.pane = Pane::List;
                    self.list_key(key, mods, out);
                }
            }
        }
    }

    /// The query text changed: parse it once, filter, and drop the selection.
    fn query_changed(&mut self) {
        self.query = Query::parse(self.search.text());
        self.query_empty = self.search.text().trim().is_empty();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.list_scroll = 0.0;
        self.refilter();
    }

    fn list_key(&mut self, key: &Key, mods: Modifiers, out: &mut Vec<UiAction>) {
        let page = ((self.lay().list.height() / lay::ROW_H).floor() as usize).max(1);
        let n = self.view.len();
        let cur = self.cursor.and_then(|c| self.pos_of(c));
        let step = |delta: isize| -> usize {
            match cur {
                None => 0,
                Some(c) => c.saturating_add_signed(delta).min(n.saturating_sub(1)),
            }
        };
        match key {
            Key::Up if mods.alt => emit(out, BrowseAction::Parent),
            Key::Up if n > 0 => self.select_to(step(-1), mods.shift, out),
            Key::Down if n > 0 => self.select_to(step(1), mods.shift, out),
            Key::PageUp if n > 0 => self.select_to(step(-(page as isize)), mods.shift, out),
            Key::PageDown if n > 0 => self.select_to(step(page as isize), mods.shift, out),
            Key::Home if n > 0 => self.select_to(0, mods.shift, out),
            Key::End if n > 0 => self.select_to(n - 1, mods.shift, out),
            Key::Enter => {
                if let Some(r) = cur.and_then(|c| self.row_at_pos(c)) {
                    let action = match r.kind {
                        FileKind::Folder => BrowseAction::Show(Place::Folder(r.path.clone())),
                        FileKind::Audio(_) => BrowseAction::Preview(r.path.clone()),
                        FileKind::Patch
                        | FileKind::Container
                        | FileKind::Clip
                        | FileKind::Project
                        | FileKind::Device => BrowseAction::Load(r.path.clone()),
                    };
                    emit(out, action);
                }
            }
            Key::Space => {
                let target = cur
                    .and_then(|c| self.row_at_pos(c))
                    .filter(|r| is_audio(r))
                    .map(|r| r.path.clone())
                    .or_else(|| self.selected_files().into_iter().next());
                if let Some(path) = target {
                    let playing = self.preview.as_ref().is_some_and(|(p, _)| *p == path);
                    emit(
                        out,
                        if playing {
                            BrowseAction::StopPreview
                        } else {
                            BrowseAction::Preview(path)
                        },
                    );
                }
            }
            Key::Char(c) if c.eq_ignore_ascii_case(&'a') && mods.command() && n > 0 => {
                self.select_range(0, n - 1);
                self.dirty = true;
            }
            Key::Escape => emit(out, self.escape_action()),
            Key::Backspace => emit(out, BrowseAction::Parent),
            Key::F(2) => {
                let target = cur
                    .and_then(|c| self.row_at_pos(c))
                    .filter(|r| r.kind.is_item() && r.kind != FileKind::Device)
                    .map(|r| r.path.clone())
                    .filter(|p| self.inside_library(p));
                if let Some(path) = target {
                    self.begin_rename_item(path);
                }
            }
            Key::Char(c) if c.eq_ignore_ascii_case(&'f') && !mods.command() && !mods.alt => {
                let files = self.selected_entries();
                if !files.is_empty() {
                    emit(out, BrowseAction::ToggleFavorite(files));
                }
            }
            _ => {}
        }
    }

    /// Moves the cursor to list position `pos`, selecting it (or extending the selection).
    fn select_to(&mut self, pos: usize, extend: bool, out: &mut Vec<UiAction>) {
        let row_idx = self.view[pos];
        if extend {
            let from = self.anchor.and_then(|a| self.pos_of(a)).unwrap_or(pos);
            self.select_range(from, pos);
            self.anchor.get_or_insert(row_idx);
        } else {
            self.selection.clear();
            self.selection
                .insert(self.rows[row_idx as usize].path.clone());
            self.anchor = Some(row_idx);
        }
        self.cursor = Some(row_idx);
        self.ensure_row_visible(pos);
        let r = &self.rows[row_idx as usize];
        if !extend && self.target.is_some() && r.kind == FileKind::Patch {
            emit(out, BrowseAction::Audition(r.path.clone()));
        } else if !extend && self.auto_preview && is_audio(r) {
            emit(out, BrowseAction::Preview(r.path.clone()));
        }
    }

    fn tree_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let n = self.tree_items.len();
        let cur = self
            .tree_cursor
            .as_ref()
            .and_then(|k| self.tree_items.iter().position(|i| &i.key == k));
        let next_sel = |from: Option<usize>, down: bool| -> Option<usize> {
            let range: Vec<usize> = match (from, down) {
                (None, true) => (0..n).collect(),
                (None, false) => (0..n).rev().collect(),
                (Some(c), true) => (c + 1..n).collect(),
                (Some(c), false) => (0..c).rev().collect(),
            };
            range
                .into_iter()
                .find(|&i| self.tree_items[i].key.selectable())
        };
        match key {
            Key::Down => {
                if let Some(i) = next_sel(cur, true) {
                    self.set_tree_cursor(i);
                }
            }
            Key::Up => {
                if let Some(i) = next_sel(cur, false) {
                    self.set_tree_cursor(i);
                }
            }
            Key::Right => {
                if let Some(c) = cur {
                    let item = self.tree_items[c].clone();
                    if item.expandable && !item.expanded {
                        self.toggle_item(&item.key, out);
                    } else if item.expanded
                        && let Some(i) = next_sel(Some(c), true)
                    {
                        self.set_tree_cursor(i);
                    }
                }
            }
            Key::Left => {
                if let Some(c) = cur {
                    let item = self.tree_items[c].clone();
                    if item.expanded {
                        self.toggle_item(&item.key, out);
                    } else if let Some(i) = (0..c).rev().find(|&i| {
                        self.tree_items[i].depth < item.depth && self.tree_items[i].key.selectable()
                    }) {
                        self.set_tree_cursor(i);
                    }
                }
            }
            Key::Enter => {
                if let Some(c) = cur {
                    let item = self.tree_items[c].clone();
                    match item.key.place() {
                        Some(place) => emit(out, BrowseAction::Show(place)),
                        None => self.toggle_item(&item.key, out),
                    }
                }
            }
            Key::Escape => emit(out, self.escape_action()),
            _ => {}
        }
    }

    fn set_tree_cursor(&mut self, idx: usize) {
        self.tree_cursor = Some(self.tree_items[idx].key.clone());
        self.ensure_tree_visible(idx);
    }

    // ----- info requests --------------------------------------------------------------------

    pub(super) fn collect_info_requests(&mut self, out: &mut Vec<UiAction>) {
        if !self.info_dirty {
            return;
        }
        self.info_dirty = false;
        let range = lay::visible_range(
            self.list_scroll,
            self.lay().list.height(),
            lay::ROW_H,
            self.view.len(),
        );
        let mut want: Vec<PathBuf> = Vec::new();
        for pos in range {
            let r = &self.rows[self.view[pos] as usize];
            if wants_info(r) && !self.requested_info.contains(&r.path) {
                want.push(r.path.clone());
            }
        }
        if !want.is_empty() {
            self.requested_info.extend(want.iter().cloned());
            emit(out, BrowseAction::RequestInfo(want));
        }
    }
}

impl FileBrowserView {
    /// What Escape does in the lists: end patch browsing, else stop the preview.
    fn escape_action(&self) -> BrowseAction {
        if self.target.is_some() {
            BrowseAction::Cancel
        } else {
            BrowseAction::StopPreview
        }
    }
}
