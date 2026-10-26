//! The file browser: answers the panel's [`BrowseAction`]s with folder listings, header
//! probes and previews run on worker threads, and keeps the user's library database
//! (favourites, tags, bookmarked folders).
//!
//! The UI thread never touches the disk here: listings, probes, preview decoding and library
//! saves run on workers whose results are applied in [`AppCore::frame`]. The one exception is
//! the library file, read once in [`AppCore::set_library_path`] (a small file, at start-up) and
//! written synchronously by [`AppCore::flush_library`] on exit.
//!
//! # How the pieces fit
//!
//! * Every listing, walk and file-set lookup carries a generation; a newer `Show` makes older
//!   answers stale and they are dropped (and a running walk stops).
//! * A preview starts on the UI thread: the ring is created, its stream goes to the engine at
//!   once and the feed goes to the decoder thread, so the sound begins as soon as the first
//!   chunk is decoded. The panel shows progress from the stream's monitor.
//! * The library sections list from the library folder: **Devices** is built from the registry on
//!   the UI thread (virtual rows whose path is a device URI), the other sections are a walk over
//!   `<library>/<Section>` that keeps the items of the section. A device URI is never handed to
//!   a worker as a path: collections split them off ([`split_devices`]) and the probe skips them.
//! * Library changes mark the database dirty; the next frame hands a JSON snapshot to the saver
//!   thread (one write in flight, the newest snapshot wins).

use super::{AppCore, LoadTarget};
use crate::browser_workers::{
    BrowseJob, BrowseMsg, CopyAssets, PreviewJob, WalkScope, Workers, home_dir, split_devices,
};
use crate::library_store::{LibrarySaver, Loaded, load};
use crate::sink::EngineSink;
use atomik_engine::{PreviewMonitor, preview_channel};
#[cfg(test)]
use atomik_library::{AudioInfo, ItemInfo};
use atomik_library::{FileRow, LibraryDb, LibrarySection, PathKey, Tag, classify, device_type_of};
#[cfg(test)]
use atomik_ui::TreeChild;
use atomik_ui::{BrowseAction, FileBrowserView, Place, PlacesData, device_rows};
use std::path::PathBuf;

/// The preview that is playing (or about to).
struct ActivePreview {
    path: PathBuf,
    monitor: PreviewMonitor,
    /// Source frames in the file, once the decoder has opened it and the header says.
    total: Option<u64>,
    /// The decoder opened the file (until then a failure means no sound at all).
    opened: bool,
}

/// What the core pushed into the panel, recorded in tests (the view has no row getters).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewCall {
    SetRows(Vec<FileRow>),
    AppendRows(Vec<FileRow>),
    PatchMarks(Vec<(PathBuf, bool, Vec<String>)>),
    PatchInfo(Vec<(PathBuf, AudioInfo)>),
    PatchItemInfo(Vec<(PathBuf, ItemInfo)>),
    SetPlaces(PlacesData),
    SetChildren(PathBuf, Vec<TreeChild>),
    Select(PathBuf),
}

/// The browser's worker channels, generations, preview and library state.
pub(crate) struct BrowserState {
    visible: bool,
    /// The first show has listed the roots and the first folder.
    shown: bool,
    /// The folder the list shows (or showed last, while a collection is shown).
    folder: Option<PathBuf>,
    /// The folder to open on the first show.
    restore: Option<PathBuf>,
    /// A folder to fall back to when `restore` cannot be listed.
    fallback: Option<(PathBuf, PathBuf)>,
    roots: Vec<PathBuf>,
    db: LibraryDb,
    lib_path: Option<PathBuf>,
    /// The library folder (`<root>/Patches`, `Containers`, `Clips`, `Projects`); `None` while
    /// unknown.
    library_root: Option<PathBuf>,
    /// The database changed since the last snapshot was handed to the saver.
    dirty: bool,
    /// The file must not be overwritten this session (newer version, unreadable).
    locked: bool,
    saver: Option<LibrarySaver>,
    workers: Option<Workers>,
    /// Jobs submitted whose `Done` has not arrived.
    jobs: usize,
    /// A saved library item to select once the list that shows it arrives.
    select_after: Option<PathBuf>,
    /// The row of `select_after` is a new item: its rename entry opens when it is selected.
    offer_name: bool,
    /// A project the user asked to open from the library: the shell asks about unsaved
    /// changes, then opens it.
    open_request: Option<PathBuf>,
    /// The item file the last Save to Library wrote.
    last_saved: Option<PathBuf>,
    /// The panel started a rename entry the user should type into.
    focus_request: bool,
    /// The device rows of the favourites or tag place being listed (built from the registry,
    /// shown with the answer of the stat job).
    collection_devices: Vec<FileRow>,
    /// Paths of the audio rows the list holds (to patch marks after a library change).
    shown_paths: Vec<PathBuf>,
    preview: Option<ActivePreview>,
    /// Every push into the panel (tests).
    #[cfg(test)]
    pub(crate) trace: Vec<ViewCall>,
    /// The paths of every stat job submitted (tests).
    #[cfg(test)]
    pub(crate) stat_paths: Vec<Vec<PathBuf>>,
    /// Every path queued for the probe worker (tests).
    #[cfg(test)]
    pub(crate) probed: Vec<PathBuf>,
    /// The last `set_preview` (tests).
    #[cfg(test)]
    pub(crate) last_preview: Option<(Option<PathBuf>, f32)>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            visible: false,
            shown: false,
            folder: None,
            restore: None,
            fallback: None,
            roots: Vec::new(),
            db: LibraryDb::new(),
            lib_path: None,
            library_root: None,
            dirty: false,
            locked: false,
            saver: None,
            workers: None,
            jobs: 0,
            select_after: None,
            offer_name: false,
            open_request: None,
            last_saved: None,
            focus_request: false,
            collection_devices: Vec::new(),
            shown_paths: Vec::new(),
            preview: None,
            #[cfg(test)]
            trace: Vec::new(),
            #[cfg(test)]
            stat_paths: Vec::new(),
            #[cfg(test)]
            probed: Vec::new(),
            #[cfg(test)]
            last_preview: None,
        }
    }
}

impl BrowserState {
    fn workers(&mut self) -> &Workers {
        self.workers.get_or_insert_with(Workers::spawn)
    }

    fn try_next(&self) -> Option<BrowseMsg> {
        self.workers.as_ref()?.rx.try_recv().ok()
    }

    /// The library file is saved this session.
    fn writable(&self) -> bool {
        self.lib_path.is_some() && !self.locked
    }

    fn saver(&mut self) -> Option<&mut LibrarySaver> {
        let path = self.lib_path.clone()?;
        Some(self.saver.get_or_insert_with(|| LibrarySaver::spawn(path)))
    }

    fn saving(&self) -> bool {
        self.saver.as_ref().is_some_and(LibrarySaver::in_flight)
    }
}

fn path_of(key: &PathKey) -> PathBuf {
    PathBuf::from(key.as_str())
}

impl<S: EngineSink> AppCore<S> {
    /// The file browser panel.
    pub fn file_browser(&self) -> &FileBrowserView {
        &self.file_browser
    }

    /// The file browser panel, mutably (bounds, scale, focus, width).
    pub fn file_browser_mut(&mut self) -> &mut FileBrowserView {
        &mut self.file_browser
    }

    /// Where the library database lives (`<config_dir>/library.json`); `None` keeps it in
    /// memory only (tests). Loads it now: a damaged file is renamed `library.json.bad`, a fresh
    /// database is used and a status message says so. A file from a newer version is never
    /// overwritten this session.
    pub fn set_library_path(&mut self, path: Option<PathBuf>) {
        let b = &mut self.browser;
        b.saver = None;
        b.dirty = false;
        b.locked = false;
        b.db = LibraryDb::new();
        let mut message = None;
        if let Some(p) = &path {
            match load(p) {
                Loaded::Db(db) => b.db = db,
                Loaded::Missing => {}
                Loaded::Corrupt { moved_to, detail } => {
                    message = Some(match moved_to {
                        Some(bad) => format!(
                            "the library file was damaged ({detail}); it was moved to {} and a new library started",
                            bad.display()
                        ),
                        None => format!(
                            "the library file was damaged ({detail}); a new library started"
                        ),
                    });
                }
                Loaded::Newer(version) => {
                    b.locked = true;
                    message = Some(format!(
                        "the library file was written by a newer version ({version}); changes to favourites and tags will not be saved"
                    ));
                }
                Loaded::Unreadable(why) => {
                    b.locked = true;
                    message = Some(format!(
                        "the library file cannot be read ({why}); changes to favourites and tags will not be saved"
                    ));
                }
            }
        }
        b.lib_path = path;
        if let Some(m) = message {
            self.report(m);
        }
        if self.browser.shown {
            self.refresh_places();
            self.reshow_collection();
        }
    }

    /// The library folder, or `None` while unknown.
    pub fn library_root(&self) -> Option<PathBuf> {
        self.browser.library_root.clone()
    }

    /// Sets the library folder (the shell resolves it from the run's option, the Workflow
    /// setting and the default). The panel is told, so it renames only below it, and a library
    /// section on show is listed again.
    pub fn set_library_root(&mut self, root: Option<PathBuf>) {
        if self.browser.library_root == root {
            return;
        }
        self.browser.library_root.clone_from(&root);
        self.file_browser.set_library_folder(root);
        if self.browser.shown
            && let Some(place @ Place::Library(_)) = self.file_browser.location().cloned()
        {
            self.show(place);
        }
    }

    /// Shows the library `section` and selects the row of `path` once it is listed, when the
    /// panel shows. Returns whether it did (otherwise the caller says so in the status line).
    pub(super) fn show_saved(&mut self, section: LibrarySection, path: PathBuf) -> bool {
        self.browser.last_saved = Some(path.clone());
        if !self.browser.visible {
            return false;
        }
        self.show(Place::Library(section));
        self.browser.select_after = Some(path);
        self.browser.offer_name = true;
        true
    }

    /// Lists the shown place again and selects the row of the renamed item `path` once it is
    /// there (without opening another rename entry).
    pub(super) fn show_renamed(&mut self, path: PathBuf) {
        if let Some(place) = self.file_browser.location().cloned() {
            self.show(place);
            self.browser.select_after = Some(path);
            self.browser.offer_name = false;
        }
    }

    /// The favourite flag and tags of `path` in the library database (tests).
    #[cfg(test)]
    pub(crate) fn library_mark(&self, path: &std::path::Path) -> atomik_library::Mark {
        self.browser.db.mark(path)
    }

    /// Moves the favourite flag and tags of a renamed item to its new path and tells the panel.
    pub(super) fn library_moved(&mut self, from: &std::path::Path, to: &std::path::Path) {
        if self.browser.db.move_path(from, to) {
            self.library_changed(&[from.to_path_buf(), to.to_path_buf()]);
        }
    }

    /// A project (or an item to open as one) was picked in the library: the shell takes it
    /// once, asks about unsaved changes and opens it.
    pub fn take_open_request(&mut self) -> Option<PathBuf> {
        self.browser.open_request.take()
    }

    /// True once after the panel opened a rename entry (a saved item's name): the shell gives
    /// the panel the keys.
    pub fn take_browser_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.browser.focus_request)
    }

    /// The item file the last Save to Library wrote (its audio may still be copying).
    pub fn last_saved_item(&self) -> Option<&std::path::Path> {
        self.browser.last_saved.as_deref()
    }

    pub(super) fn request_open(&mut self, path: PathBuf) {
        self.browser.open_request = Some(path);
    }

    /// Hands the audio of a saved library item to the copy worker.
    pub(super) fn copy_item_assets(&mut self, job: CopyAssets) {
        if self.browser.workers().copy_assets(job) {
            self.browser.jobs += 1;
        }
    }

    /// Tells the core whether the panel shows. The first show lists the tree roots and the
    /// last (or home) folder; hiding stops a preview.
    pub fn set_file_browser_visible(&mut self, visible: bool) {
        self.browser.visible = visible;
        if !visible {
            self.stop_preview();
            return;
        }
        if self.browser.shown {
            return;
        }
        self.browser.shown = true;
        self.submit(BrowseJob::Roots);
        self.refresh_places();
        let home = home_dir();
        let first = match self.browser.restore.take() {
            Some(folder) => {
                if let Some(home) = home {
                    self.browser.fallback = Some((folder.clone(), home));
                }
                Some(folder)
            }
            None => home,
        };
        if let Some(folder) = first {
            self.show(Place::Folder(folder));
        }
    }

    /// Whether the panel shows.
    pub fn file_browser_visible(&self) -> bool {
        self.browser.visible
    }

    /// The folder the list shows, for persisting across runs.
    pub fn file_browser_folder(&self) -> Option<PathBuf> {
        self.browser
            .folder
            .clone()
            .or_else(|| self.browser.restore.clone())
    }

    /// Restores the folder shown on the first show.
    pub fn set_file_browser_folder(&mut self, folder: Option<PathBuf>) {
        self.browser.restore = folder;
    }

    /// Handles one panel request. [`BrowseAction::DragFiles`] and
    /// [`BrowseAction::LayoutChanged`] belong to the shell and are ignored here.
    pub fn browse(&mut self, action: BrowseAction) {
        match action {
            BrowseAction::Show(place) => self.show(place),
            BrowseAction::Parent => {
                if let Some(Place::Folder(path)) = self.file_browser.location()
                    && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
                {
                    let parent = parent.to_path_buf();
                    self.show(Place::Folder(parent));
                }
            }
            BrowseAction::Refresh | BrowseAction::SetRecursive(_) => {
                if let Some(place) = self.file_browser.location().cloned() {
                    self.show(place);
                }
            }
            BrowseAction::ListChildren(path) => self.submit(BrowseJob::Children(path)),
            BrowseAction::Preview(path) => {
                // Only audio is previewed: not a device row, not a patch or a clip.
                let audio = device_type_of(&path).is_none()
                    && !classify(&path, false).is_some_and(atomik_library::FileKind::is_item);
                if audio {
                    self.start_preview(path);
                }
            }
            BrowseAction::StopPreview => self.stop_preview(),
            BrowseAction::ToggleFavorite(paths) => {
                if self.browser.db.toggle_favorite(&paths) {
                    self.library_changed(&paths);
                }
            }
            BrowseAction::AddTag { paths, tag } => {
                if let Some(tag) = self.valid_tag(&tag) {
                    let mut changed = false;
                    for path in &paths {
                        changed |= self.browser.db.add_tag(path, &tag);
                    }
                    if changed {
                        self.library_changed(&paths);
                    }
                }
            }
            BrowseAction::RemoveTag { paths, tag } => {
                if let Ok(tag) = Tag::new(&tag) {
                    let mut changed = false;
                    for path in &paths {
                        changed |= self.browser.db.remove_tag(path, &tag);
                    }
                    if changed {
                        self.library_changed(&paths);
                    }
                }
            }
            BrowseAction::RenameTag { from, to } => self.rename_tag(&from, &to),
            BrowseAction::DeleteTag(name) => {
                if let Ok(tag) = Tag::new(&name) {
                    let affected = self.tagged_shown(&tag);
                    if self.browser.db.delete_tag(&tag) {
                        self.library_changed(&affected);
                    }
                }
            }
            BrowseAction::AddPlace(path) => {
                if self.browser.db.add_place(&path) {
                    self.library_changed(&[]);
                }
            }
            BrowseAction::RemovePlace(path) => {
                if self.browser.db.remove_place(&path) {
                    self.library_changed(&[]);
                }
            }
            BrowseAction::RequestInfo(paths) => self.queue_probe(paths),
            BrowseAction::SetAutoPreview(_)
            | BrowseAction::DragFiles(_)
            | BrowseAction::LayoutChanged => {}
            // Loading, opening and renaming library items is the library work of the app.
            BrowseAction::Load(path) => self.load_library_item(&path, LoadTarget::Default),
            BrowseAction::OpenProject(path) => self.open_library_project(&path),
            BrowseAction::RenameItem { path, to } => self.rename_library_item(&path, &to),
        }
    }

    /// True while a listing, walk, probe or preview is in flight: the shell keeps ticking
    /// frames (see `Shell::is_busy`) so results are applied without waiting for an event.
    pub fn browser_busy(&self) -> bool {
        let b = &self.browser;
        b.jobs > 0 || b.preview.is_some() || b.saving() || (b.dirty && b.writable())
    }

    /// Writes the library database now if it changed (call on exit).
    pub fn flush_library(&mut self) {
        if !self.browser.writable() {
            return;
        }
        if self.browser.dirty {
            self.hand_over_snapshot();
        }
        if let Some(saver) = &self.browser.saver {
            saver.wait_idle();
        }
    }

    /// Per-frame work: apply worker results, feed the preview progress, poll info requests.
    pub(super) fn browser_frame(&mut self) {
        while let Some(msg) = self.browser.try_next() {
            self.apply(msg);
        }
        let wanted = self.file_browser.take_info_requests();
        self.queue_probe(wanted);
        self.preview_progress();
        if let Some(error) = self
            .browser
            .saver
            .as_ref()
            .and_then(LibrarySaver::take_error)
        {
            self.report(format!("the library file could not be saved: {error}"));
        }
        if self.browser.dirty && self.browser.writable() && !self.browser.saving() {
            self.hand_over_snapshot();
        }
    }

    // ------------------------------------------------------------------ listings

    fn submit(&mut self, job: BrowseJob) {
        #[cfg(test)]
        if let BrowseJob::Stat { paths, .. } = &job {
            self.browser.stat_paths.push(paths.clone());
        }
        if self.browser.workers().browse(job) {
            self.browser.jobs += 1;
        }
    }

    fn queue_probe(&mut self, paths: Vec<PathBuf>) {
        // A device URI is not a file: nothing is read for it.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| device_type_of(p).is_none())
            .collect();
        #[cfg(test)]
        self.browser.probed.extend(paths.iter().cloned());
        if !paths.is_empty() && self.browser.workers().probe(paths) {
            self.browser.jobs += 1;
        }
    }

    /// Shows `place`: the view switches at once, the rows follow from a worker.
    fn show(&mut self, place: Place) {
        self.browser.select_after = None;
        let generation = self.browser.workers().bump();
        self.file_browser.set_location(place.clone());
        match place {
            Place::Folder(path) => {
                self.browser.folder = Some(path.clone());
                self.submit(BrowseJob::List {
                    generation,
                    path: path.clone(),
                });
                if self.file_browser.recursive() {
                    self.submit(BrowseJob::Walk {
                        generation,
                        root: path,
                        scope: WalkScope::Below,
                    });
                }
            }
            Place::Favorites => {
                let paths = self.browser.db.favorites().iter().map(path_of).collect();
                self.show_collection(generation, paths);
            }
            Place::Tag(name) => {
                let paths: Vec<PathBuf> = Tag::new(&name)
                    .map(|tag| self.browser.db.tagged(&tag).iter().map(path_of).collect())
                    .unwrap_or_default();
                self.show_collection(generation, paths);
            }
            Place::Library(section) => self.show_library(generation, section),
        }
    }

    /// Lists a favourites or tag place: the files from a stat job, the device rows from the
    /// registry (a device that no longer exists is left out like a missing file).
    fn show_collection(&mut self, generation: u64, paths: Vec<PathBuf>) {
        let (files, devices) = split_devices(&paths);
        self.browser.collection_devices = device_rows(&self.registry)
            .into_iter()
            .filter(|r| device_type_of(&r.path).is_some_and(|id| devices.iter().any(|d| d == id)))
            .collect();
        self.submit(BrowseJob::Stat {
            generation,
            paths: files,
        });
    }

    /// Lists a library section. Devices come from the registry at once; the other sections
    /// are empty until the walk over `<library>/<Section>` delivers (no folder, no rows).
    fn show_library(&mut self, generation: u64, section: LibrarySection) {
        let Some(folder) = section.folder_name() else {
            self.set_rows(device_rows(&self.registry));
            return;
        };
        self.set_rows(Vec::new());
        if let Some(root) = &self.browser.library_root {
            let root = root.join(folder);
            self.submit(BrowseJob::Walk {
                generation,
                root,
                scope: WalkScope::Section(section),
            });
        }
    }

    /// Lists a collection again after the library changed under it.
    fn reshow_collection(&mut self) {
        if let Some(place @ (Place::Favorites | Place::Tag(_))) =
            self.file_browser.location().cloned()
        {
            self.show(place);
        }
    }

    fn view_rows(&mut self, rows: Vec<FileRow>, append: bool) {
        #[cfg(test)]
        self.browser.trace.push(if append {
            ViewCall::AppendRows(rows.clone())
        } else {
            ViewCall::SetRows(rows.clone())
        });
        if append {
            self.file_browser.append_rows(rows);
        } else {
            self.file_browser.set_rows(rows);
        }
    }

    fn view_preview(&mut self, path: Option<PathBuf>, progress: f32) {
        #[cfg(test)]
        {
            self.browser.last_preview = Some((path.clone(), progress));
        }
        self.file_browser.set_preview(path, progress);
    }

    fn decorated(&self, mut rows: Vec<FileRow>) -> Vec<FileRow> {
        for row in &mut rows {
            self.browser.db.decorate(row);
        }
        rows
    }

    fn apply(&mut self, msg: BrowseMsg) {
        let current = self.browser.workers.as_ref().map_or(0, Workers::current);
        match msg {
            BrowseMsg::Listed {
                generation,
                path,
                result,
            } if generation == current => match result {
                Ok(rows) => self.set_rows(rows),
                Err(error) => {
                    if let Some((_, home)) = self.browser.fallback.take_if(|(f, _)| *f == path) {
                        self.show(Place::Folder(home));
                    } else {
                        self.set_rows(Vec::new());
                        self.report(error);
                    }
                }
            },
            BrowseMsg::Stats {
                generation,
                mut rows,
            } if generation == current => {
                let mut all = std::mem::take(&mut self.browser.collection_devices);
                all.append(&mut rows);
                self.set_rows(all);
            }
            BrowseMsg::Walked { generation, rows } if generation == current => {
                let rows = self.decorated(rows);
                self.browser
                    .shown_paths
                    .extend(rows.iter().map(|r| r.path.clone()));
                self.view_rows(rows, true);
                self.select_pending();
            }
            BrowseMsg::Listed { .. } | BrowseMsg::Stats { .. } | BrowseMsg::Walked { .. } => {}
            BrowseMsg::Children { path, children } => {
                #[cfg(test)]
                self.browser
                    .trace
                    .push(ViewCall::SetChildren(path.clone(), children.clone()));
                self.file_browser.set_children(&path, children);
            }
            BrowseMsg::Roots(roots) => {
                self.browser.roots = roots;
                self.refresh_places();
            }
            BrowseMsg::Info(info) => {
                #[cfg(test)]
                self.browser.trace.push(ViewCall::PatchInfo(info.clone()));
                self.file_browser.patch_info(&info);
            }
            BrowseMsg::ItemInfo(info) => {
                #[cfg(test)]
                self.browser
                    .trace
                    .push(ViewCall::PatchItemInfo(info.clone()));
                self.file_browser.patch_item_info(&info);
            }
            BrowseMsg::AssetsCopied {
                name,
                section,
                path,
                report,
            } => self.library_audio_copied(&name, section, path, &report),
            BrowseMsg::PreviewTotal { path, frames } => {
                if let Some(active) = self.browser.preview.as_mut().filter(|a| a.path == path) {
                    active.total = frames;
                    active.opened = true;
                }
            }
            BrowseMsg::PreviewFailed { path, error } => {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.report(format!("cannot preview {name}: {error}"));
                if self
                    .browser
                    .preview
                    .as_ref()
                    .is_some_and(|a| a.path == path && !a.opened)
                {
                    self.stop_preview();
                }
            }
            BrowseMsg::Done => self.browser.jobs = self.browser.jobs.saturating_sub(1),
        }
    }

    /// Selects the saved item the list was waiting for, once its row is there.
    fn select_pending(&mut self) {
        let Some(path) = self.browser.select_after.clone() else {
            return;
        };
        if self.file_browser.select_path(&path) {
            // A saved item is offered a name: the rename entry opens on its row.
            if self.browser.offer_name && self.file_browser.begin_rename(&path) {
                self.browser.focus_request = true;
            }
            #[cfg(test)]
            self.browser.trace.push(ViewCall::Select(path));
            self.browser.select_after = None;
        }
    }

    fn set_rows(&mut self, rows: Vec<FileRow>) {
        let rows = self.decorated(rows);
        self.browser.shown_paths = rows.iter().map(|r| r.path.clone()).collect();
        self.view_rows(rows, false);
    }

    // ------------------------------------------------------------------ library

    fn valid_tag(&mut self, name: &str) -> Option<Tag> {
        match Tag::new(name) {
            Ok(tag) => Some(tag),
            Err(e) => {
                self.report(format!("invalid tag: {e}"));
                None
            }
        }
    }

    fn rename_tag(&mut self, from: &str, to: &str) {
        let Ok(from_tag) = Tag::new(from) else {
            return;
        };
        let Some(to_tag) = self.valid_tag(to) else {
            return;
        };
        let affected = self.tagged_shown(&from_tag);
        if !self.browser.db.rename_tag(&from_tag, &to_tag) {
            return;
        }
        if self.file_browser.location() == Some(&Place::Tag(from.to_owned())) {
            // Follow the tag: the list shows it under its new name.
            self.file_browser
                .set_location(Place::Tag(to_tag.as_str().to_owned()));
        }
        self.library_changed(&affected);
    }

    /// The shown rows that carry `tag` now.
    fn tagged_shown(&self, tag: &Tag) -> Vec<PathBuf> {
        let keys: std::collections::BTreeSet<PathKey> =
            self.browser.db.tagged(tag).into_iter().collect();
        self.browser
            .shown_paths
            .iter()
            .filter(|p| keys.contains(&PathKey::new(p)))
            .cloned()
            .collect()
    }

    /// The database changed: save later, repaint the marks of `paths`, refresh Places and the
    /// list when it shows a collection.
    fn library_changed(&mut self, paths: &[PathBuf]) {
        self.browser.dirty = true;
        let marks: Vec<(PathBuf, bool, Vec<String>)> = paths
            .iter()
            .map(|p| {
                let mark = self.browser.db.mark(p);
                (
                    p.clone(),
                    mark.favorite,
                    mark.tags.iter().map(|t| t.as_str().to_owned()).collect(),
                )
            })
            .collect();
        if !marks.is_empty() {
            #[cfg(test)]
            self.browser.trace.push(ViewCall::PatchMarks(marks.clone()));
            self.file_browser.patch_marks(&marks);
        }
        self.refresh_places();
        self.reshow_collection();
    }

    fn refresh_places(&mut self) {
        let db = &self.browser.db;
        let places = PlacesData {
            favorites: db.favorites().len(),
            tags: db
                .all_tags()
                .into_iter()
                .map(|(tag, count)| (tag.as_str().to_owned(), count))
                .collect(),
            bookmarks: db.places().to_vec(),
            roots: self.browser.roots.clone(),
        };
        #[cfg(test)]
        self.browser.trace.push(ViewCall::SetPlaces(places.clone()));
        self.file_browser.set_places(places);
    }

    fn hand_over_snapshot(&mut self) {
        let json = self.browser.db.to_json();
        self.browser.dirty = false;
        if let Some(saver) = self.browser.saver() {
            saver.save(json);
        }
    }

    // ------------------------------------------------------------------ preview

    /// Starts a preview: the stream reaches the engine now, the decoder thread fills it.
    fn start_preview(&mut self, path: PathBuf) {
        let (feed, stream) = preview_channel();
        let monitor = stream.monitor();
        // Gain is fixed at 0 dB.
        if let Err(e) = self.sink.preview_start(stream, 1.0) {
            self.report(format!("preview: {e}"));
            return;
        }
        if self.browser.workers().preview(PreviewJob {
            path: path.clone(),
            feed,
        }) {
            self.browser.jobs += 1;
        }
        self.view_preview(Some(path.clone()), 0.0);
        self.browser.preview = Some(ActivePreview {
            path,
            monitor,
            total: None,
            opened: false,
        });
    }

    fn stop_preview(&mut self) {
        if self.browser.preview.take().is_some() || self.sink.preview_active() {
            let _ = self.sink.preview_stop();
        }
        self.view_preview(None, 0.0);
    }

    fn preview_progress(&mut self) {
        let Some(active) = &self.browser.preview else {
            return;
        };
        if active.monitor.is_done() || active.monitor.is_abandoned() {
            self.browser.preview = None;
            self.view_preview(None, 0.0);
            return;
        }
        #[allow(clippy::cast_precision_loss)] // a progress fraction needs no exactness
        let progress = match active.total {
            Some(total) if total > 0 => active.monitor.frames_played() as f32 / total as f32,
            _ => 0.0,
        };
        let path = active.path.clone();
        self.view_preview(Some(path), progress);
    }

    // ------------------------------------------------------------------ test support

    /// The monitor of the active preview.
    #[cfg(test)]
    pub(crate) fn preview_monitor(&self) -> Option<PreviewMonitor> {
        self.browser.preview.as_ref().map(|a| a.monitor.clone())
    }

    /// The path being previewed.
    #[cfg(test)]
    pub(crate) fn preview_path(&self) -> Option<&std::path::Path> {
        self.browser.preview.as_ref().map(|a| a.path.as_path())
    }

    /// Library files the saver thread has written.
    #[cfg(test)]
    pub(crate) fn library_writes(&self) -> u64 {
        self.browser.saver.as_ref().map_or(0, LibrarySaver::writes)
    }

    /// The database (tests).
    #[cfg(test)]
    pub(crate) fn library_db(&self) -> &LibraryDb {
        &self.browser.db
    }
}
