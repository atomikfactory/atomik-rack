//! The file browser panel: a folder tree over a file list, a search field and instant preview.
//!
//! Same hosting contract as the sequencer views (`new`, `set_bounds`, `handle`, `paint`,
//! `needs_repaint`, `is_dragging`, `set_focused`, `set_scale`), with no document: the app
//! pushes everything the panel shows (`set_places`, `set_children`, `set_location`,
//! `set_rows`, `append_rows`, `patch_info`, `patch_item_info`, `patch_marks`, `set_preview`,
//! `set_library_folder`) and answers the [`BrowseAction`]s it emits. The view owns the query text, sort, selection, scroll, tree
//! expansion and the filtered index list; filtering uses [`atomik_library::Query`] and
//! ordering [`atomik_library::sort_rows`]. Painting costs O(visible rows).
//!
//! # Routing (for the shell)
//!
//! Send pointer events to the panel while the pointer is inside its bounds, and keep sending
//! pointer events (also outside) while [`FileBrowserView::is_dragging`] is true. Send key
//! events while the panel has focus; while [`FileBrowserView::is_text_owner`] is true every
//! key must reach the panel (do not treat letters or Space as shortcuts). After a
//! [`BrowseAction::DragFiles`] the panel has dropped the gesture and the shell owns the drag.
//! When a drag of the panel's edge or splitter ends the panel emits
//! [`BrowseAction::LayoutChanged`]: read [`FileBrowserView::width`] and
//! [`FileBrowserView::tree_fraction`] and lay out again.
//!
//! # The library
//!
//! The tree starts with the library sections ([`atomik_library::LibrarySection`]); choosing one
//! emits [`BrowseAction::Show`] with [`Place::Library`]. The host answers with
//! `set_location` and `set_rows`: for `Devices` the rows of [`crate::device_rows`] (virtual
//! rows whose path is a device URI, never a file: the panel asks for no info and no preview of
//! them), for the other sections the items found below `<library folder>/<Section>` (a missing
//! folder is an empty list, for which the panel shows a hint). Items appear in plain folder
//! listings too. Enter or a double-click on an item row emits [`BrowseAction::Load`], the list
//! menu adds "Open as Project" and, for files below [`FileBrowserView::set_library_folder`],
//! "Rename..." (also `F2`), which ends in [`BrowseAction::RenameItem`]. Dragging an item (or a
//! device row) emits [`BrowseAction::DragFiles`] with its path or URI like an audio file.
//!
//! # Info requests
//!
//! [`BrowseAction::RequestInfo`] is emitted from [`FileBrowserView::handle`] (any event,
//! pointer moves and resizes included) after rows were set, scrolled, filtered or sorted: a
//! paint cannot return actions. Feed the panel one event (for example
//! `UiEvent::Resize(size)`) after pushing rows or patches so the request goes out without
//! waiting for the pointer.

mod field;
mod help;
mod input;
mod layout;
mod menu;
mod paint;
mod tree;

use crate::action::{BrowseAction, Place};
use crate::event::UiEvent;
use crate::theme::Theme;
use atomik_library::{AudioInfo, FileKind, FileRow, ItemInfo, Query, SortKey, sort_rows};
use field::TextField;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use vello::kurbo::{Point, Rect};

use crate::action::UiAction;
use crate::canvas::Canvas;

/// How many places the back button remembers.
const MAX_HISTORY: usize = 64;

/// The roots of the tree, refreshed by the app when the library or the drives change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlacesData {
    /// Number of favourites (shown on the Favourites entry).
    pub favorites: usize,
    /// Every tag in use with its file count, in display order.
    pub tags: Vec<(String, usize)>,
    /// Named favourite lists with their file counts, in the order they were made.
    pub lists: Vec<(String, usize)>,
    /// Bookmarked folders, in order (expandable).
    pub bookmarks: Vec<PathBuf>,
    /// Drives / file-system roots and the home folder (expandable).
    pub roots: Vec<PathBuf>,
}

/// A sub-folder in the tree (see [`FileBrowserView::set_children`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChild {
    /// Display name.
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
}

/// Which part of the panel has the keyboard (besides the text fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Tree,
    List,
}

/// The pointer gesture in progress.
#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    None,
    /// Dragging the right edge.
    Resize,
    /// Dragging the tree/list splitter.
    Splitter,
    /// Dragging a scroll bar thumb; `grab` is the pointer offset inside the thumb.
    Thumb {
        pane: Pane,
        grab: f64,
    },
    /// A press on a file row: a move past the threshold starts a file drag.
    RowPress {
        start: Point,
        /// Collapse the multi-selection to this row on release when no drag happened.
        collapse: Option<PathBuf>,
    },
}

/// The device patch browsing applies to (see [`FileBrowserView::set_patch_target`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchTarget {
    /// The device's name, shown in the band above the header.
    label: String,
    /// The device's type id: only patches for it are listed.
    type_id: String,
}

/// What the inline entry creates.
#[derive(Debug, Clone, PartialEq)]
enum EntryKind {
    NewTag,
    RenameTag(String),
    /// Rename the library item in `Entry::targets`.
    RenameItem,
    /// Set the author of the files in `Entry::targets`.
    Author,
    /// Set the categories (comma-separated) of the files in `Entry::targets`.
    Categories,
    /// Make a favourite list; with `Entry::targets` the files go into it.
    NewList,
    /// Rename the favourite list of this name.
    RenameList(String),
}

/// The inline text entry for "New tag..." and "Rename...".
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    kind: EntryKind,
    field: TextField,
    targets: Vec<PathBuf>,
}

/// What the pointer is over (for hover highlights).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Hover {
    #[default]
    None,
    Row(usize),
    Tree(usize),
}

/// The file browser panel.
pub struct FileBrowserView {
    theme: Theme,
    bounds: Rect,
    scale: f64,
    focused: bool,
    pane: Pane,
    dirty: bool,
    width: f64,
    tree_fraction: f64,
    auto_preview: bool,
    recursive: bool,
    // Tree.
    places: PlacesData,
    children: BTreeMap<PathBuf, Vec<TreeChild>>,
    requested_children: BTreeSet<PathBuf>,
    expanded: BTreeSet<PathBuf>,
    tags_open: bool,
    tree_items: Vec<tree::Item>,
    tree_cursor: Option<tree::Key>,
    tree_scroll: f64,
    location: Option<Place>,
    /// The places shown so far, oldest first (see `Self::set_location`).
    history: Vec<Place>,
    /// The position in `history` of the place shown.
    history_at: usize,
    /// The history position a back or forward step is going to (so the answer does not
    /// start a new branch).
    history_jump: Option<usize>,
    // List.
    rows: Vec<FileRow>,
    view: Vec<u32>,
    sort_key: SortKey,
    descending: bool,
    query: Query,
    query_empty: bool,
    selection: BTreeSet<PathBuf>,
    cursor: Option<u32>,
    anchor: Option<u32>,
    list_scroll: f64,
    preview: Option<(PathBuf, f32)>,
    requested_info: BTreeSet<PathBuf>,
    info_dirty: bool,
    library_folder: Option<PathBuf>,
    /// The device the panel applies patches to (see [`FileBrowserView::set_patch_target`]).
    target: Option<PatchTarget>,
    // Text fields and pointer state.
    search: TextField,
    search_focused: bool,
    entry: Option<Entry>,
    menu: Option<menu::Menu>,
    gesture: Gesture,
    hover: Hover,
}

impl FileBrowserView {
    /// Default panel width in logical pixels.
    pub const DEFAULT_WIDTH: f64 = 300.0;
    /// Narrowest panel width.
    pub const MIN_WIDTH: f64 = 200.0;

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.dirty = true;
    }

    /// An empty panel; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        let mut v = Self {
            theme,
            bounds: Rect::new(0.0, 0.0, Self::DEFAULT_WIDTH, 600.0),
            scale: 1.0,
            focused: false,
            pane: Pane::List,
            dirty: true,
            width: Self::DEFAULT_WIDTH,
            tree_fraction: 0.4,
            auto_preview: true,
            recursive: false,
            places: PlacesData::default(),
            children: BTreeMap::new(),
            requested_children: BTreeSet::new(),
            expanded: BTreeSet::new(),
            tags_open: true,
            tree_items: Vec::new(),
            tree_cursor: None,
            tree_scroll: 0.0,
            location: None,
            history: Vec::new(),
            history_at: 0,
            history_jump: None,
            rows: Vec::new(),
            view: Vec::new(),
            sort_key: SortKey::Name,
            descending: false,
            query: Query::default(),
            query_empty: true,
            selection: BTreeSet::new(),
            cursor: None,
            anchor: None,
            list_scroll: 0.0,
            preview: None,
            requested_info: BTreeSet::new(),
            info_dirty: false,
            library_folder: None,
            target: None,
            search: TextField::default(),
            search_focused: false,
            entry: None,
            menu: None,
            gesture: Gesture::None,
            hover: Hover::None,
        };
        v.rebuild_tree();
        v
    }

    /// Places the panel (logical pixels, window coordinates).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scrolls();
            self.dirty = true;
            self.info_dirty = true;
        }
    }

    /// Display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        let scale = scale.max(0.1);
        if (scale - self.scale).abs() > f64::EPSILON {
            self.scale = scale;
            self.dirty = true;
        }
    }

    /// Keyboard focus (the last clicked pane).
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            if !focused {
                self.search_focused = false;
                self.entry = None;
            }
            self.dirty = true;
        }
    }

    /// The width the user chose by dragging the panel's right edge (the app lays out with it).
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Restores a persisted width (clamped to at least [`Self::MIN_WIDTH`]).
    pub fn set_width(&mut self, width: f64) {
        if width.is_finite() {
            self.width = width.max(Self::MIN_WIDTH);
            self.dirty = true;
        }
    }

    /// Fraction of the panel height given to the tree (0.1..=0.9).
    pub fn tree_fraction(&self) -> f64 {
        self.tree_fraction
    }

    /// Restores a persisted tree/list split.
    pub fn set_tree_fraction(&mut self, fraction: f64) {
        if fraction.is_finite() {
            self.tree_fraction = fraction.clamp(0.1, 0.9);
            self.clamp_scrolls();
            self.dirty = true;
            self.info_dirty = true;
        }
    }

    /// Whether selecting a file previews it.
    pub fn auto_preview(&self) -> bool {
        self.auto_preview
    }

    /// Restores the persisted auto-preview flag (default on).
    pub fn set_auto_preview(&mut self, on: bool) {
        self.auto_preview = on;
        self.dirty = true;
    }

    /// Whether the search covers sub-folders.
    pub fn recursive(&self) -> bool {
        self.recursive
    }

    /// Replaces the tree roots.
    pub fn set_places(&mut self, places: PlacesData) {
        self.places = places;
        self.rebuild_tree();
    }

    /// Answers [`crate::action::BrowseAction::ListChildren`]: the sub-folders of `parent`
    /// (empty = leaf).
    pub fn set_children(&mut self, parent: &Path, children: Vec<TreeChild>) {
        self.children.insert(parent.to_path_buf(), children);
        self.rebuild_tree();
    }

    /// The place the list shows (highlighted in the tree, shown in the path bar). Every new
    /// place joins the history the back and forward buttons walk; going somewhere new after
    /// stepping back drops the places that were ahead.
    pub fn set_location(&mut self, place: Place) {
        if self.location.as_ref() == Some(&place) {
            return;
        }
        match self.history_jump.take() {
            Some(at) if self.history.get(at) == Some(&place) => self.history_at = at,
            _ => {
                if !self.history.is_empty() {
                    self.history.truncate(self.history_at + 1);
                }
                self.history.push(place.clone());
                if self.history.len() > MAX_HISTORY {
                    self.history.remove(0);
                }
                self.history_at = self.history.len() - 1;
            }
        }
        self.location = Some(place);
        self.dirty = true;
    }

    /// The place the back button goes to, if any.
    pub fn back_place(&self) -> Option<&Place> {
        self.history_at
            .checked_sub(1)
            .and_then(|at| self.history.get(at))
    }

    /// The place the forward button goes to, if any.
    pub fn forward_place(&self) -> Option<&Place> {
        self.history.get(self.history_at + 1)
    }

    /// The place the list shows.
    pub fn location(&self) -> Option<&Place> {
        self.location.as_ref()
    }

    /// Replaces the list (unsorted; the view sorts and filters). Resets scroll and selection.
    pub fn set_rows(&mut self, rows: Vec<FileRow>) {
        self.rows = rows;
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.list_scroll = 0.0;
        self.requested_info.clear();
        self.resort();
    }

    /// Adds rows (recursive search results streaming in), keeping scroll and selection.
    pub fn append_rows(&mut self, rows: Vec<FileRow>) {
        self.rows.extend(rows);
        self.resort();
    }

    /// Selects the row of `path` alone, moves the cursor to it and scrolls it into view (a
    /// saved library item is shown this way). `false` when the list has no such row (it may be
    /// filtered out).
    pub fn select_path(&mut self, path: &Path) -> bool {
        let Some(row) = self.row_of_path(path) else {
            return false;
        };
        let Some(pos) = self.pos_of(row) else {
            return false;
        };
        self.selection.clear();
        self.selection.insert(path.to_path_buf());
        self.cursor = Some(row);
        self.anchor = Some(row);
        let top = pos as f64 * layout::ROW_H;
        let height = self.lay().list.height();
        if top < self.list_scroll {
            self.list_scroll = top;
        } else if top + layout::ROW_H > self.list_scroll + height {
            self.list_scroll = top + layout::ROW_H - height;
        }
        self.clamp_scrolls();
        self.info_dirty = true;
        self.dirty = true;
        true
    }

    /// Selects the row of the library item `path` and starts its rename entry (a saved item is
    /// offered a name this way). `false` when the list has no such row or the item is not below
    /// the library folder.
    pub fn begin_rename(&mut self, path: &Path) -> bool {
        if !self.inside_library(path) || !self.select_path(path) {
            return false;
        }
        self.begin_rename_item(path.to_path_buf());
        true
    }

    /// Fills in probed header data.
    pub fn patch_info(&mut self, info: &[(PathBuf, AudioInfo)]) {
        let wanted: BTreeMap<&Path, usize> = info
            .iter()
            .enumerate()
            .map(|(i, (p, _))| (p.as_path(), i))
            .collect();
        for (row, k) in self.find_rows(wanted) {
            self.rows[row].info = Some(info[k].1);
        }
        if self.sort_key == SortKey::Duration {
            self.resort();
        }
        self.dirty = true;
    }

    /// Fills in the probed detail of library items (the answer to a
    /// [`BrowseAction::RequestInfo`] for item rows).
    pub fn patch_item_info(&mut self, info: &[(PathBuf, ItemInfo)]) {
        let wanted: BTreeMap<&Path, usize> = info
            .iter()
            .enumerate()
            .map(|(i, (p, _))| (p.as_path(), i))
            .collect();
        for (row, k) in self.find_rows(wanted) {
            self.rows[row].item = Some(info[k].1.clone());
        }
        if !self.query_empty || self.target.is_some() {
            self.refilter();
        }
        self.dirty = true;
    }

    /// Aims the panel at one device (`Some((device name, device type id))`) or back at nothing.
    /// With a target the panel shows an orange band naming the device and lists only the
    /// patches for its type (folders stay, to move around); selecting a patch (a click, the
    /// arrow keys) emits [`BrowseAction::Audition`], Enter or a double-click emits
    /// [`BrowseAction::Load`] (the app applies it to the device and accepts it), and Escape
    /// emits [`BrowseAction::Cancel`].
    pub fn set_patch_target(&mut self, target: Option<(String, String)>) {
        let next = target.map(|(label, type_id)| PatchTarget { label, type_id });
        if self.target != next {
            self.target = next;
            self.refilter();
        }
    }

    /// The type id of the device the panel applies patches to.
    pub fn patch_target(&self) -> Option<&str> {
        self.target.as_ref().map(|t| t.type_id.as_str())
    }

    /// The library folder (`None` while unknown). Only items below it can be renamed; an
    /// open rename entry that is no longer allowed is dropped.
    pub fn set_library_folder(&mut self, folder: Option<PathBuf>) {
        if self.library_folder != folder {
            self.library_folder = folder;
            if self.entry.as_ref().is_some_and(|e| {
                e.kind == EntryKind::RenameItem
                    && !e.targets.first().is_some_and(|p| self.inside_library(p))
            }) {
                self.entry = None;
            }
            self.dirty = true;
        }
    }

    /// Whether `path` is a file below the library folder.
    fn inside_library(&self, path: &Path) -> bool {
        self.library_folder
            .as_deref()
            .is_some_and(|lib| path != lib && path.starts_with(lib))
    }

    /// Updates favourite flags and tags after a library change.
    pub fn patch_marks(&mut self, marks: &[(PathBuf, bool, Vec<String>)]) {
        let wanted: BTreeMap<&Path, usize> = marks
            .iter()
            .enumerate()
            .map(|(i, (p, _, _))| (p.as_path(), i))
            .collect();
        for (row, k) in self.find_rows(wanted) {
            self.rows[row].favorite = marks[k].1;
            self.rows[row].tags.clone_from(&marks[k].2);
        }
        if !self.query_empty {
            self.refilter();
        }
        self.dirty = true;
    }

    /// Updates the author and categories of rows after a library change (`(path, author,
    /// categories)`).
    pub fn patch_meta(&mut self, meta: &[(PathBuf, String, Vec<String>)]) {
        let wanted: BTreeMap<&Path, usize> = meta
            .iter()
            .enumerate()
            .map(|(i, (p, _, _))| (p.as_path(), i))
            .collect();
        for (row, k) in self.find_rows(wanted) {
            self.rows[row].author.clone_from(&meta[k].1);
            self.rows[row].categories.clone_from(&meta[k].2);
        }
        if !self.query_empty {
            self.refilter();
        }
        self.dirty = true;
    }

    /// The file being previewed and its progress (0..=1), or `None`. Call every frame while
    /// a preview plays.
    pub fn set_preview(&mut self, path: Option<PathBuf>, progress: f32) {
        let next = path.map(|p| (p, progress.clamp(0.0, 1.0)));
        let same = match (&self.preview, &next) {
            (None, None) => true,
            (Some((a, x)), Some((b, y))) => a == b && (x - y).abs() <= f32::EPSILON,
            _ => false,
        };
        if !same {
            self.preview = next;
            self.dirty = true;
        }
    }

    /// Focuses the search field (`view.focus_file_search`).
    pub fn focus_search(&mut self) {
        self.focused = true;
        self.search_focused = true;
        self.entry = None;
        self.menu = None;
        self.search.select_all();
        self.dirty = true;
    }

    /// True while a text field (search, new tag, rename tag) owns the keyboard.
    pub fn is_text_owner(&self) -> bool {
        self.focused && (self.search_focused || self.entry.is_some())
    }

    /// True while a pointer drag (resize, splitter, scroll bar, file drag before handover) is
    /// in progress: keep routing pointer events here until release.
    pub fn is_dragging(&self) -> bool {
        self.gesture != Gesture::None
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.dirty
    }

    /// The visible audio rows still lacking [`AudioInfo`] that were not requested yet (each
    /// path once until [`Self::set_rows`]). The app polls this every frame, so rows that
    /// appear without an event (a listing arriving, a search narrowing) are probed promptly;
    /// the same paths are also emitted as [`BrowseAction::RequestInfo`] from `handle`.
    pub fn take_info_requests(&mut self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.collect_info_requests(&mut out);
        out.into_iter()
            .flat_map(|a| match a {
                UiAction::Browse(BrowseAction::RequestInfo(paths)) => paths,
                _ => Vec::new(),
            })
            .collect()
    }

    /// Handles one event (window-logical coordinates). See the module docs for routing and
    /// for when [`crate::action::BrowseAction::RequestInfo`] is emitted.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.dispatch(event, &mut out);
        self.collect_info_requests(&mut out);
        out
    }

    /// Paints the panel inside its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas) {
        self.paint_panel(canvas);
        self.dirty = false;
    }

    // ----- shared helpers ------------------------------------------------------------------

    fn lay(&self) -> layout::Layout {
        layout::layout(self.bounds, self.tree_fraction, self.target.is_some())
    }

    fn rebuild_tree(&mut self) {
        self.tree_items = tree::build(&tree::Source {
            places: &self.places,
            children: &self.children,
            expanded: &self.expanded,
            tags_open: self.tags_open,
        });
        if let Some(k) = &self.tree_cursor
            && !self.tree_items.iter().any(|i| &i.key == k)
        {
            self.tree_cursor = None;
        }
        self.clamp_scrolls();
        self.dirty = true;
    }

    fn content_h(&self) -> f64 {
        self.view.len() as f64 * layout::ROW_H
    }

    fn max_list_scroll(&self) -> f64 {
        (self.content_h() - self.lay().list.height()).max(0.0)
    }

    fn max_tree_scroll(&self) -> f64 {
        (self.tree_items.len() as f64 * layout::TREE_ROW_H - self.lay().tree.height()).max(0.0)
    }

    fn clamp_scrolls(&mut self) {
        self.list_scroll = self.list_scroll.clamp(0.0, self.max_list_scroll());
        self.tree_scroll = self.tree_scroll.clamp(0.0, self.max_tree_scroll());
    }

    /// The row index behind list position `pos`.
    fn row_at_pos(&self, pos: usize) -> Option<&FileRow> {
        self.view.get(pos).map(|&i| &self.rows[i as usize])
    }

    /// The position in the filtered list of row index `row`.
    fn pos_of(&self, row: u32) -> Option<usize> {
        self.view.iter().position(|&i| i == row)
    }

    /// The row index of the row with this path.
    fn row_of_path(&self, path: &Path) -> Option<u32> {
        self.rows
            .iter()
            .position(|r| r.path == path)
            .map(|i| i as u32)
    }

    /// Sorts the rows, remaps the cursor and anchor, and filters again.
    fn resort(&mut self) {
        let cursor = self.cursor.map(|i| self.rows[i as usize].path.clone());
        let anchor = self.anchor.map(|i| self.rows[i as usize].path.clone());
        sort_rows(&mut self.rows, self.sort_key, self.descending);
        self.cursor = cursor.and_then(|p| self.row_of_path(&p));
        self.anchor = anchor.and_then(|p| self.row_of_path(&p));
        self.refilter();
    }

    /// Whether the patch target lets `row` through: folders (to move around) and the patches
    /// that are not known to be for another device type. Without a target every row passes.
    fn target_allows(target: Option<&PatchTarget>, row: &FileRow) -> bool {
        let Some(target) = target else {
            return true;
        };
        match row.kind {
            FileKind::Folder => true,
            FileKind::Patch => row
                .item
                .as_ref()
                .is_none_or(|i| i.device_type == target.type_id),
            _ => false,
        }
    }

    /// Rebuilds the filtered index list from the current query and patch target.
    fn refilter(&mut self) {
        self.view.clear();
        if self.query_empty && self.target.is_none() {
            self.view.extend(0..self.rows.len() as u32);
        } else {
            let q = &self.query;
            let all = self.query_empty;
            let target = self.target.as_ref();
            self.view.extend(
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| (all || q.matches(r)) && Self::target_allows(target, r))
                    .map(|(i, _)| i as u32),
            );
        }
        self.clamp_scrolls();
        self.dirty = true;
        self.info_dirty = true;
    }

    /// Finds the rows of `wanted` paths (path to caller index): the visible neighbourhood
    /// first, then everything. Returns `(row index, caller index)`.
    fn find_rows(&self, mut wanted: BTreeMap<&Path, usize>) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        if wanted.is_empty() {
            return found;
        }
        let page = (self.lay().list.height() / layout::ROW_H).ceil() as usize + 1;
        let first = (self.list_scroll / layout::ROW_H) as usize;
        let lo = first.saturating_sub(page);
        let hi = (first + 2 * page).min(self.view.len());
        if lo < hi {
            for &i in &self.view[lo..hi] {
                if let Some(k) = wanted.remove(self.rows[i as usize].path.as_path()) {
                    found.push((i as usize, k));
                }
            }
        }
        if !wanted.is_empty() {
            for (i, r) in self.rows.iter().enumerate() {
                if let Some(k) = wanted.remove(r.path.as_path()) {
                    found.push((i, k));
                    if wanted.is_empty() {
                        break;
                    }
                }
            }
        }
        found
    }

    /// The selected audio files and library items (what a tag, a favourite or a drag acts on),
    /// in list order.
    fn selected_entries(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.view
            .iter()
            .map(|&i| &self.rows[i as usize])
            .filter(|r| r.kind != FileKind::Folder && self.selection.contains(&r.path))
            .map(|r| r.path.clone())
            .collect()
    }

    /// The selected audio files, in list order.
    fn selected_files(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.view
            .iter()
            .map(|&i| &self.rows[i as usize])
            .filter(|r| matches!(r.kind, FileKind::Audio(_)) && self.selection.contains(&r.path))
            .map(|r| r.path.clone())
            .collect()
    }
}
