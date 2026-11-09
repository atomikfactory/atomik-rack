//! The recent commands file of the command palette. What the palette lists and how a shell runs
//! it is in `shell_palette_tests.rs`.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// A temporary folder that is removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("atomik-palette-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }

    fn file(&self) -> PathBuf {
        self.0.join(FILE)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_file_sits_next_to_the_settings() {
    assert_eq!(
        path(Path::new("cfg").join("settings.json").as_path()),
        Path::new("cfg").join("palette.json")
    );
}

#[test]
fn a_missing_or_corrupt_file_is_an_empty_list() {
    let d = Dir::new();
    assert!(load(&d.file()).is_empty());
    for text in [
        "",
        "not json",
        "[1, 2]",
        "{\"recent\": 4}",
        "{\"recent\": {\"a\": 1}}",
        "{\"other\": [\"a\"]}",
    ] {
        std::fs::write(d.file(), text).unwrap();
        assert!(load(&d.file()).is_empty(), "{text:?}");
    }
}

#[test]
fn a_command_run_goes_first_and_is_kept() {
    let d = Dir::new();
    let (list, saved) = remember(&d.file(), "edit.undo");
    saved.unwrap();
    assert_eq!(list, ["edit.undo"]);
    let (list, saved) = remember(&d.file(), "help.guided_tour");
    saved.unwrap();
    assert_eq!(list, ["help.guided_tour", "edit.undo"]);
    // Running one again moves it to the front instead of listing it twice.
    let (list, saved) = remember(&d.file(), "edit.undo");
    saved.unwrap();
    assert_eq!(list, ["edit.undo", "help.guided_tour"]);
    assert_eq!(load(&d.file()), list);
}

#[test]
fn only_the_last_eight_are_kept() {
    let d = Dir::new();
    for n in 0..12 {
        remember(&d.file(), &format!("cmd.n{n}")).1.unwrap();
    }
    let list = load(&d.file());
    assert_eq!(list.len(), MAX_RECENT);
    assert_eq!(list[0], "cmd.n11");
    assert_eq!(list[MAX_RECENT - 1], "cmd.n4");
}

#[test]
fn a_file_with_odd_entries_keeps_the_usable_ones() {
    let d = Dir::new();
    std::fs::write(
        d.file(),
        "{\"recent\": [\"a.b\", 7, \"\", \"a.b\", null, \"c.d\"]}",
    )
    .unwrap();
    assert_eq!(load(&d.file()), ["a.b", "c.d"]);
    // A write over a corrupt file starts the list again.
    std::fs::write(d.file(), "garbage").unwrap();
    let (list, saved) = remember(&d.file(), "x.y");
    saved.unwrap();
    assert_eq!(list, ["x.y"]);
}

#[test]
fn an_unwritable_place_still_gives_the_list_in_memory() {
    let d = Dir::new();
    // A file where the folder should be: the write fails, the list does not.
    let blocker = d.0.join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let (list, saved) = remember(&blocker.join(FILE), "edit.undo");
    assert!(saved.is_err());
    assert_eq!(list, ["edit.undo"]);
}

#[test]
fn labels_lose_the_ellipsis_of_a_dialog() {
    assert_eq!(plain("Export Patch\u{2026}"), "Export Patch");
    assert_eq!(plain("Find Parameter..."), "Find Parameter");
    assert_eq!(plain("Undo"), "Undo");
}
