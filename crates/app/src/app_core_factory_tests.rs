//! The factory library in the app core: its own root in the browser, the marks of its manifest,
//! read-only items, and (the content test) every shipped patch loads, renders and stays bounded.

use super::browser::ViewCall;
use super::*;
use crate::factory::{Factory, MANIFEST};
use crate::sink::RecordingSink;
use atomik_library::{FileRow, LibrarySection};
use atomik_model::empty_doc;
use atomik_ui::kurbo::Rect;
use atomik_ui::{BrowseAction, Place};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-factory-core-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `root/Patches/{Warm.atomikpatch, Cold.atomikpatch}` and a manifest that marks `Warm`.
fn factory_folder(tmp: &Tmp) -> PathBuf {
    let root = tmp.path("Factory");
    write(&root.join("Patches/Warm.atomikpatch"), "{}");
    write(&root.join("Patches/Cold.atomikpatch"), "{}");
    write(
        &root.join(MANIFEST),
        r#"{ "version": 1, "items": { "Patches/Warm.atomikpatch":
            { "author": "Atomik", "categories": ["Pad"], "tags": ["warm"] } } }"#,
    );
    root
}

fn core(tmp: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.file_browser_mut()
        .set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    c.set_library_root(Some(tmp.path("Library")));
    c
}

/// Shows the panel (the first show lists an empty folder, never the home folder).
fn shown(tmp: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core(tmp);
    let empty = tmp.path("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    c.set_file_browser_folder(Some(empty));
    c.set_file_browser_visible(true);
    wait(&mut c);
    c
}

fn wait<S: EngineSink>(c: &mut AppCore<S>) {
    for _ in 0..3_000 {
        c.frame();
        if !c.browser_busy() {
            c.frame();
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the browser stayed busy");
}

/// The rows the panel holds: the last `SetRows` plus the `AppendRows` after it.
fn rows(c: &AppCore<impl EngineSink>) -> Vec<FileRow> {
    let mut out = Vec::new();
    for call in &c.browser.trace {
        match call {
            ViewCall::SetRows(r) => out.clone_from(r),
            ViewCall::AppendRows(r) => out.extend(r.iter().cloned()),
            _ => {}
        }
    }
    out
}

fn row_named(rows: &[FileRow], name: &str) -> FileRow {
    rows.iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed: {rows:?}"))
        .clone()
}

fn says(c: &AppCore<RecordingSink>, text: &str) {
    let message = c.last_rejection().unwrap_or_default().to_lowercase();
    assert!(message.contains(text), "{text:?}: {message}");
}

#[test]
fn the_factory_is_a_root_of_its_own_with_the_sections_it_has() {
    let tmp = Tmp::new("places");
    let mut c = shown(&tmp);
    assert!(c.factory_root().is_none());
    c.set_factory_root(Some(factory_folder(&tmp)));
    let places = c
        .browser
        .trace
        .iter()
        .rev()
        .find_map(|t| match t {
            ViewCall::SetPlaces(p) => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(places.factory, [LibrarySection::Patches]);
    assert_eq!(c.factory_root(), Some(tmp.path("Factory")));
    assert!(c.in_factory(&tmp.path("Factory/Patches/Warm.atomikpatch")));
    assert!(!c.in_factory(&tmp.path("Library/Patches/Warm.atomikpatch")));
    c.set_factory_root(None);
    assert!(c.factory_root().is_none());
}

#[test]
fn a_factory_section_lists_its_items_with_the_marks_of_the_manifest() {
    let tmp = Tmp::new("list");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    // The user library has a patch of the same name: only the factory one is listed here.
    write(&tmp.path("Library/Patches/Other.atomikpatch"), "{}");
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let listed = rows(&c);
    let names: Vec<&str> = listed.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let warm = row_named(&listed, "Warm.atomikpatch");
    assert_eq!(warm.tags, ["warm"]);
    assert_eq!(warm.categories, ["Pad"]);
    assert_eq!(warm.author, "Atomik");
    let cold = row_named(&listed, "Cold.atomikpatch");
    assert!(cold.tags.is_empty() && cold.categories.is_empty() && cold.author.is_empty());
    // A section the factory has no folder for lists nothing.
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Clips)));
    wait(&mut c);
    assert!(rows(&c).is_empty());
}

#[test]
fn favourites_and_user_tags_work_on_factory_items_and_keep_the_factory_marks() {
    let tmp = Tmp::new("marks");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let warm = tmp.path("Factory/Patches/Warm.atomikpatch");
    c.browse(BrowseAction::ToggleFavorite(vec![warm.clone()]));
    let marks = c
        .browser
        .trace
        .iter()
        .rev()
        .find_map(|t| match t {
            ViewCall::PatchMarks(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(marks, [(warm.clone(), true, vec!["warm".to_string()])]);
    assert!(c.library_mark(&warm).favorite);
    // The user's own database holds only what the user did.
    assert!(c.library_mark(&warm).tags.is_empty());
    // A fresh listing shows both: the star and the factory tag.
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let row = row_named(&rows(&c), "Warm.atomikpatch");
    assert!(row.favorite);
    assert_eq!(row.tags, ["warm"]);
}

#[test]
fn factory_items_cannot_be_deleted_or_renamed_and_the_files_stay() {
    let tmp = Tmp::new("readonly");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    let warm = tmp.path("Factory/Patches/Warm.atomikpatch");
    c.browse(BrowseAction::DeleteItem(warm.clone()));
    says(&c, "factory content");
    assert!(warm.is_file());
    c.browse(BrowseAction::RenameItem {
        path: warm.clone(),
        to: "Hot".into(),
    });
    says(&c, "factory content");
    assert!(warm.is_file());
    assert!(!tmp.path("Factory/Patches/Hot.atomikpatch").exists());
}

#[test]
fn losing_the_factory_while_it_is_shown_falls_back_to_the_library_section() {
    let tmp = Tmp::new("lost");
    let mut c = shown(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    c.set_factory_root(None);
    wait(&mut c);
    assert_eq!(
        c.file_browser_mut().location(),
        Some(&Place::Library(LibrarySection::Patches))
    );
}

#[test]
fn an_unreadable_manifest_still_opens_the_folder_without_marks() {
    let tmp = Tmp::new("badmanifest");
    let root = factory_folder(&tmp);
    write(&root.join(MANIFEST), "{ nope");
    assert!(Factory::open(&root).is_err());
    let mut c = core(&tmp);
    c.set_factory_root(Some(root));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let warm = row_named(&rows(&c), "Warm.atomikpatch");
    assert!(warm.tags.is_empty() && warm.author.is_empty());
}
