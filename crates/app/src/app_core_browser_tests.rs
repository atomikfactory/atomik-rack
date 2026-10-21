//! File browser in the app core: sink preview calls, library persistence, listings, tree,
//! favourites and tags, probing, previews and the no-write guarantee. Fixtures are generated
//! WAVs in temporary folders; the engine is the recording sink (or an offline real engine for
//! the audible test).

use super::browser::ViewCall;
use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_engine::preview_channel;
use atomik_library::{AudioFormat, FileKind, FileRow, PathKey};
use atomik_model::empty_doc;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{BrowseAction, Modifiers, Place, PlacesData, PointerButton, UiEvent};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ------------------------------------------------------------------------------- fixtures

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-browser-{tag}-{}-{n}",
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

/// A sine WAV (48 kHz mono) of `frames` frames at `path`; folders are created.
fn wav(path: &Path, frames: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    write_wav_f32(path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
}

/// `root/{kick 2.wav, kick 10.wav, notes.txt, .hidden.wav, sub/{snare.wav, deep/hat.wav}, empty/}`.
fn library(tag: &str) -> Tmp {
    let t = Tmp::new(tag);
    wav(&t.path("kick 2.wav"), 4_800);
    wav(&t.path("kick 10.wav"), 4_800);
    wav(&t.path(".hidden.wav"), 480);
    std::fs::write(t.path("notes.txt"), "hello").unwrap();
    wav(&t.path("sub/snare.wav"), 4_800);
    wav(&t.path("sub/deep/hat.wav"), 4_800);
    std::fs::create_dir_all(t.path("empty")).unwrap();
    t
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.file_browser_mut()
        .set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    c
}

/// Frames until nothing is in flight (bounded), then one more to apply the last answers.
fn wait_browser<S: EngineSink>(c: &mut AppCore<S>) {
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

/// Frames until `done` holds (bounded).
fn wait_until<S: EngineSink>(c: &mut AppCore<S>, what: &str, done: impl Fn(&AppCore<S>) -> bool) {
    for _ in 0..3_000 {
        c.frame();
        if done(c) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

fn show(c: &mut AppCore<RecordingSink>, dir: &Path) {
    c.browse(BrowseAction::Show(Place::Folder(dir.to_path_buf())));
    wait_browser(c);
}

/// The preview calls the sink saw (not the engine set-up around them).
fn preview_calls(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
    c.sink()
        .calls
        .iter()
        .filter(|call| matches!(call, SinkCall::PreviewStart { .. } | SinkCall::PreviewStop))
        .cloned()
        .collect()
}

fn trace(c: &AppCore<impl EngineSink>) -> &[ViewCall] {
    &c.browser.trace
}

/// The rows the panel holds: the last `SetRows` plus the `AppendRows` after it.
fn rows(c: &AppCore<impl EngineSink>) -> Vec<FileRow> {
    let mut out = Vec::new();
    for call in trace(c) {
        match call {
            ViewCall::SetRows(r) => out.clone_from(r),
            ViewCall::AppendRows(r) => out.extend(r.iter().cloned()),
            _ => {}
        }
    }
    out
}

fn names(rows: &[FileRow]) -> Vec<String> {
    let mut n: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    n.sort();
    n
}

fn set_rows_count(c: &AppCore<impl EngineSink>) -> usize {
    trace(c)
        .iter()
        .filter(|t| matches!(t, ViewCall::SetRows(_)))
        .count()
}

fn last_places(c: &AppCore<impl EngineSink>) -> PlacesData {
    trace(c)
        .iter()
        .rev()
        .find_map(|t| match t {
            ViewCall::SetPlaces(p) => Some(p.clone()),
            _ => None,
        })
        .expect("places were set")
}

fn marks_calls(c: &AppCore<impl EngineSink>) -> Vec<Vec<(PathBuf, bool, Vec<String>)>> {
    trace(c)
        .iter()
        .filter_map(|t| match t {
            ViewCall::PatchMarks(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn same_file(a: &Path, b: &Path) -> bool {
    PathKey::new(a) == PathKey::new(b)
}

fn click(c: &mut AppCore<RecordingSink>, x: f64, y: f64) {
    let pos = Point::new(x, y);
    for event in [
        UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    ] {
        c.handle_pane_event(Pane::FileBrowser, &event);
    }
}

// ----------------------------------------------------------------------------- sink calls

#[test]
fn recording_sink_records_preview_calls_and_holds_the_stream() {
    let mut sink = RecordingSink::new();
    let (feed, stream) = preview_channel();
    sink.preview_start(stream, 1.0).unwrap();
    assert!(sink.preview_active());
    assert!(!feed.is_abandoned());
    sink.preview_stop().unwrap();
    assert!(!sink.preview_active());
    assert!(feed.is_abandoned());
    assert_eq!(
        sink.calls,
        vec![SinkCall::PreviewStart { gain: 1.0 }, SinkCall::PreviewStop]
    );
}

#[test]
fn a_new_preview_replaces_the_held_stream() {
    let mut sink = RecordingSink::new();
    let (first, s1) = preview_channel();
    let (second, s2) = preview_channel();
    sink.preview_start(s1, 1.0).unwrap();
    sink.preview_start(s2, 1.0).unwrap();
    assert!(first.is_abandoned());
    assert!(!second.is_abandoned());
}

// ------------------------------------------------------------------------------- listing

#[test]
fn show_switches_the_location_at_once_and_lists_folders_and_audio() {
    let lib = library("list");
    let mut c = core();
    c.browse(BrowseAction::Show(Place::Folder(lib.0.clone())));
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Folder(lib.0.clone()))
    );
    assert!(c.browser_busy());
    wait_browser(&mut c);
    assert!(!c.browser_busy());
    let listed = rows(&c);
    assert_eq!(
        names(&listed),
        ["empty", "kick 10.wav", "kick 2.wav", "sub"]
    );
    let kick = listed.iter().find(|r| r.name == "kick 2.wav").unwrap();
    assert_eq!(kick.kind, FileKind::Audio(AudioFormat::Wav));
    assert!(kick.size > 0);
    assert!(kick.info.is_none());
    let sub = listed.iter().find(|r| r.name == "sub").unwrap();
    assert_eq!(sub.kind, FileKind::Folder);
    assert_eq!(c.file_browser_folder(), Some(lib.0.clone()));
}

#[test]
fn listing_a_missing_folder_reports_and_empties_the_list() {
    let lib = library("missing");
    let mut c = core();
    show(&mut c, &lib.0);
    show(&mut c, &lib.path("nope"));
    assert!(rows(&c).is_empty());
    assert!(c.last_rejection().is_some());
}

#[test]
fn rows_carry_the_library_marks() {
    let lib = library("marks");
    let mut c = core();
    let kick = lib.path("kick 2.wav");
    c.browse(BrowseAction::ToggleFavorite(vec![kick.clone()]));
    c.browse(BrowseAction::AddTag {
        paths: vec![kick.clone()],
        tag: "808".into(),
    });
    show(&mut c, &lib.0);
    let listed = rows(&c);
    let row = listed.iter().find(|r| r.path == kick).unwrap();
    assert!(row.favorite);
    assert_eq!(row.tags, ["808"]);
    let other = listed.iter().find(|r| r.name == "kick 10.wav").unwrap();
    assert!(!other.favorite && other.tags.is_empty());
}

#[test]
fn parent_goes_up_and_does_nothing_at_a_root() {
    let lib = library("parent");
    let mut c = core();
    show(&mut c, &lib.path("sub"));
    c.browse(BrowseAction::Parent);
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Folder(lib.0.clone()))
    );
    wait_browser(&mut c);
    assert!(names(&rows(&c)).contains(&"sub".to_string()));

    let root = if cfg!(windows) { "C:\\" } else { "/" };
    c.browse(BrowseAction::Show(Place::Folder(PathBuf::from(root))));
    wait_browser(&mut c);
    let before = set_rows_count(&c);
    c.browse(BrowseAction::Parent);
    wait_browser(&mut c);
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Folder(PathBuf::from(root)))
    );
    assert_eq!(set_rows_count(&c), before);
}

#[test]
fn parent_does_nothing_on_a_collection() {
    let mut c = core();
    c.browse(BrowseAction::Show(Place::Favorites));
    wait_browser(&mut c);
    c.browse(BrowseAction::Parent);
    assert_eq!(c.file_browser().location(), Some(&Place::Favorites));
}

#[test]
fn refresh_lists_the_folder_again() {
    let lib = library("refresh");
    let mut c = core();
    show(&mut c, &lib.0);
    wav(&lib.path("new.wav"), 480);
    c.browse(BrowseAction::Refresh);
    wait_browser(&mut c);
    assert!(names(&rows(&c)).contains(&"new.wav".to_string()));
}

#[test]
fn a_stale_listing_is_ignored() {
    let a = library("stale-a");
    let b = Tmp::new("stale-b");
    wav(&b.path("only.wav"), 480);
    let mut c = core();
    c.browse(BrowseAction::Show(Place::Folder(a.0.clone())));
    c.browse(BrowseAction::Show(Place::Folder(b.0.clone())));
    wait_browser(&mut c);
    assert_eq!(set_rows_count(&c), 1);
    assert_eq!(names(&rows(&c)), ["only.wav"]);
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Folder(b.0.clone()))
    );
}

// --------------------------------------------------------------------------------- tree

#[test]
fn list_children_answers_with_sorted_sub_folders() {
    let t = Tmp::new("children");
    for n in ["pad 10", "pad 2", ".git"] {
        std::fs::create_dir_all(t.path(n)).unwrap();
    }
    wav(&t.path("x.wav"), 480);
    let mut c = core();
    c.browse(BrowseAction::ListChildren(t.0.clone()));
    wait_browser(&mut c);
    let found = trace(&c).iter().find_map(|v| match v {
        ViewCall::SetChildren(p, ch) if *p == t.0 => Some(ch.clone()),
        _ => None,
    });
    let found: Vec<String> = found.unwrap().into_iter().map(|c| c.name).collect();
    assert_eq!(found, ["pad 2", "pad 10"]);
}

// ------------------------------------------------------------------------ recursive walk

#[test]
fn recursive_walk_appends_deeper_audio_after_the_listing() {
    let lib = library("walk");
    let mut c = core();
    show(&mut c, &lib.0);
    // The "Sub" toggle in the panel header.
    click(&mut c, 240.0, 17.0);
    assert!(c.file_browser().recursive(), "the toggle was not hit");
    wait_browser(&mut c);
    let all = rows(&c);
    assert_eq!(
        names(&all),
        [
            "empty",
            "hat.wav",
            "kick 10.wav",
            "kick 2.wav",
            "snare.wav",
            "sub"
        ]
    );
    assert!(
        trace(&c)
            .iter()
            .any(|t| matches!(t, ViewCall::AppendRows(r) if r.len() == 2))
    );
    // Switching it off lists the folder alone again.
    click(&mut c, 240.0, 17.0);
    assert!(!c.file_browser().recursive());
    wait_browser(&mut c);
    assert_eq!(
        names(&rows(&c)),
        ["empty", "kick 10.wav", "kick 2.wav", "sub"]
    );
}

#[test]
fn a_walk_of_an_older_folder_never_reaches_the_list() {
    let lib = library("walk-stale");
    let other = Tmp::new("walk-other");
    wav(&other.path("o.wav"), 480);
    let mut c = core();
    show(&mut c, &lib.0);
    click(&mut c, 240.0, 17.0);
    c.browse(BrowseAction::Show(Place::Folder(other.0.clone())));
    wait_browser(&mut c);
    assert!(
        names(&rows(&c))
            .iter()
            .all(|n| n != "hat.wav" && n != "snare.wav")
    );
}

// ------------------------------------------------------------------ collections and marks

#[test]
fn favorites_place_lists_existing_favourites_only() {
    let lib = library("fav");
    let mut c = core();
    let gone = lib.path("gone.wav");
    c.browse(BrowseAction::ToggleFavorite(vec![
        lib.path("kick 2.wav"),
        lib.path("sub/snare.wav"),
        gone,
    ]));
    c.browse(BrowseAction::Show(Place::Favorites));
    wait_browser(&mut c);
    let listed = rows(&c);
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|r| r.favorite));
    assert!(
        listed
            .iter()
            .any(|r| same_file(&r.path, &lib.path("kick 2.wav")))
    );
    assert_eq!(c.file_browser().location(), Some(&Place::Favorites));
}

#[cfg(windows)]
#[test]
fn favourite_rows_keep_the_on_disk_spelling() {
    let t = Tmp::new("spelling");
    wav(&t.path("Kick 01.WAV"), 480);
    let mut c = core();
    c.browse(BrowseAction::ToggleFavorite(vec![t.path("Kick 01.WAV")]));
    c.browse(BrowseAction::Show(Place::Favorites));
    wait_browser(&mut c);
    let listed = rows(&c);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Kick 01.WAV");
    assert_eq!(listed[0].path.file_name().unwrap(), "Kick 01.WAV");
    assert!(listed[0].favorite);
}

#[test]
fn tag_place_lists_the_files_with_that_tag() {
    let lib = library("tagplace");
    let mut c = core();
    c.browse(BrowseAction::AddTag {
        paths: vec![lib.path("kick 2.wav"), lib.path("sub/snare.wav")],
        tag: "808".into(),
    });
    c.browse(BrowseAction::AddTag {
        paths: vec![lib.path("kick 10.wav")],
        tag: "other".into(),
    });
    c.browse(BrowseAction::Show(Place::Tag("808".into())));
    wait_browser(&mut c);
    assert_eq!(rows(&c).len(), 2);
    assert!(rows(&c).iter().all(|r| r.tags == ["808"]));
    // A tag name that cannot exist lists nothing.
    c.browse(BrowseAction::Show(Place::Tag("a,b".into())));
    wait_browser(&mut c);
    assert!(rows(&c).is_empty());
}

#[test]
fn toggling_a_favourite_patches_the_row_and_refreshes_places() {
    let lib = library("toggle");
    let mut c = core();
    show(&mut c, &lib.0);
    let kick = lib.path("kick 2.wav");
    c.browse(BrowseAction::ToggleFavorite(vec![kick.clone()]));
    assert_eq!(
        marks_calls(&c).last().unwrap(),
        &[(kick.clone(), true, vec![])]
    );
    assert_eq!(last_places(&c).favorites, 1);
    c.browse(BrowseAction::ToggleFavorite(vec![kick.clone()]));
    assert_eq!(marks_calls(&c).last().unwrap(), &[(kick, false, vec![])]);
    assert_eq!(last_places(&c).favorites, 0);
}

#[test]
fn toggling_in_the_favourites_place_drops_the_row() {
    let lib = library("favdrop");
    let mut c = core();
    let kick = lib.path("kick 2.wav");
    c.browse(BrowseAction::ToggleFavorite(vec![kick.clone()]));
    c.browse(BrowseAction::Show(Place::Favorites));
    wait_browser(&mut c);
    assert_eq!(rows(&c).len(), 1);
    c.browse(BrowseAction::ToggleFavorite(vec![kick]));
    wait_browser(&mut c);
    assert!(rows(&c).is_empty());
}

#[test]
fn multi_selection_toggles_all_together() {
    let lib = library("multi");
    let mut c = core();
    let a = lib.path("kick 2.wav");
    let b = lib.path("kick 10.wav");
    c.browse(BrowseAction::ToggleFavorite(vec![a.clone()]));
    // One is already a favourite: the others become favourites too, nothing is cleared.
    c.browse(BrowseAction::ToggleFavorite(vec![a.clone(), b.clone()]));
    assert!(c.library_db().is_favorite(&a) && c.library_db().is_favorite(&b));
    c.browse(BrowseAction::ToggleFavorite(vec![a.clone(), b.clone()]));
    assert!(!c.library_db().is_favorite(&a) && !c.library_db().is_favorite(&b));
}

#[test]
fn add_and_remove_tag_patch_rows_and_places() {
    let lib = library("tags");
    let mut c = core();
    show(&mut c, &lib.0);
    let kick = lib.path("kick 2.wav");
    c.browse(BrowseAction::AddTag {
        paths: vec![kick.clone()],
        tag: " 808 ".into(),
    });
    assert_eq!(
        marks_calls(&c).last().unwrap(),
        &[(kick.clone(), false, vec!["808".to_string()])]
    );
    assert_eq!(last_places(&c).tags, [("808".to_string(), 1)]);
    // Adding it again (any case) changes nothing.
    let calls = marks_calls(&c).len();
    c.browse(BrowseAction::AddTag {
        paths: vec![kick.clone()],
        tag: "808".into(),
    });
    assert_eq!(marks_calls(&c).len(), calls);
    c.browse(BrowseAction::RemoveTag {
        paths: vec![kick.clone()],
        tag: "808".into(),
    });
    assert_eq!(marks_calls(&c).last().unwrap(), &[(kick, false, vec![])]);
    assert!(last_places(&c).tags.is_empty());
}

#[test]
fn an_invalid_tag_is_reported_and_changes_nothing() {
    let lib = library("badtag");
    let mut c = core();
    for bad in ["", "  ", "a,b", "#x", &"x".repeat(40)] {
        c.browse(BrowseAction::AddTag {
            paths: vec![lib.path("kick 2.wav")],
            tag: bad.to_string(),
        });
        assert!(c.last_rejection().is_some(), "{bad:?}");
    }
    assert!(c.library_db().all_tags().is_empty());
    assert!(marks_calls(&c).is_empty());
    assert!(!c.browser_busy());
}

#[test]
fn rename_tag_updates_shown_rows_places_and_follows_the_open_tag() {
    let lib = library("rename");
    let mut c = core();
    let a = lib.path("kick 2.wav");
    let b = lib.path("kick 10.wav");
    c.browse(BrowseAction::AddTag {
        paths: vec![a.clone(), b.clone()],
        tag: "old".into(),
    });
    show(&mut c, &lib.0);
    c.browse(BrowseAction::RenameTag {
        from: "old".into(),
        to: "new".into(),
    });
    let patched = marks_calls(&c).last().unwrap().clone();
    assert_eq!(patched.len(), 2);
    assert!(patched.iter().all(|(_, _, t)| t == &["new".to_string()]));
    assert_eq!(last_places(&c).tags, [("new".to_string(), 2)]);

    c.browse(BrowseAction::Show(Place::Tag("new".into())));
    wait_browser(&mut c);
    c.browse(BrowseAction::RenameTag {
        from: "new".into(),
        to: "newer".into(),
    });
    wait_browser(&mut c);
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Tag("newer".into()))
    );
    assert_eq!(rows(&c).len(), 2);
    // An invalid target is reported and nothing changes.
    c.browse(BrowseAction::RenameTag {
        from: "newer".into(),
        to: "a,b".into(),
    });
    assert!(c.last_rejection().is_some());
    assert_eq!(last_places(&c).tags, [("newer".to_string(), 2)]);
}

#[test]
fn delete_tag_clears_it_everywhere() {
    let lib = library("deltag");
    let mut c = core();
    c.browse(BrowseAction::AddTag {
        paths: vec![lib.path("kick 2.wav")],
        tag: "gone".into(),
    });
    show(&mut c, &lib.0);
    c.browse(BrowseAction::DeleteTag("gone".into()));
    let last = marks_calls(&c).last().unwrap().clone();
    assert_eq!(last, [(lib.path("kick 2.wav"), false, vec![])]);
    assert!(last_places(&c).tags.is_empty());
    assert!(c.library_db().all_tags().is_empty());
}

#[test]
fn places_are_bookmarked_and_removed() {
    let lib = library("places");
    let mut c = core();
    c.browse(BrowseAction::AddPlace(lib.0.clone()));
    assert_eq!(last_places(&c).bookmarks, std::slice::from_ref(&lib.0));
    c.browse(BrowseAction::AddPlace(lib.0.clone()));
    assert_eq!(c.library_db().places().len(), 1);
    c.browse(BrowseAction::RemovePlace(lib.0.clone()));
    assert!(last_places(&c).bookmarks.is_empty());
}

#[test]
fn shell_owned_actions_are_ignored() {
    let mut c = core();
    for action in [
        BrowseAction::DragFiles(vec![PathBuf::from("x.wav")]),
        BrowseAction::LayoutChanged,
        BrowseAction::SetAutoPreview(false),
    ] {
        c.browse(action);
    }
    assert!(!c.browser_busy());
    assert!(trace(&c).is_empty());
}

// ------------------------------------------------------------------------------- persistence

fn library_file(dir: &Tmp) -> PathBuf {
    dir.path("config/library.json")
}

#[test]
fn a_change_is_saved_off_thread_and_loads_back() {
    let lib = library("save");
    let cfg = Tmp::new("save-cfg");
    let file = library_file(&cfg);
    let mut c = core();
    c.set_library_path(Some(file.clone()));
    assert!(!file.exists());
    c.browse(BrowseAction::ToggleFavorite(vec![lib.path("kick 2.wav")]));
    c.browse(BrowseAction::AddPlace(lib.0.clone()));
    assert!(c.browser_busy());
    wait_browser(&mut c);
    assert_eq!(c.library_writes(), 1);
    assert!(file.exists());
    assert!(!cfg.path("config/library.json.tmp").exists());

    let mut again = core();
    again.set_library_path(Some(file));
    assert!(again.library_db().is_favorite(&lib.path("kick 2.wav")));
    assert_eq!(again.library_db().places(), std::slice::from_ref(&lib.0));
}

#[test]
fn many_changes_in_one_frame_make_one_snapshot() {
    let lib = library("throttle");
    let cfg = Tmp::new("throttle-cfg");
    let mut c = core();
    c.set_library_path(Some(library_file(&cfg)));
    for i in 0..20 {
        c.browse(BrowseAction::AddTag {
            paths: vec![lib.path("kick 2.wav")],
            tag: format!("t{i}"),
        });
    }
    wait_browser(&mut c);
    assert_eq!(c.library_writes(), 1);
    let mut again = core();
    again.set_library_path(Some(library_file(&cfg)));
    assert_eq!(again.library_db().all_tags().len(), 20);
}

#[test]
fn no_change_writes_nothing() {
    let cfg = Tmp::new("nochange-cfg");
    let mut c = core();
    c.set_library_path(Some(library_file(&cfg)));
    c.browse(BrowseAction::RemovePlace(PathBuf::from("/nothing")));
    wait_browser(&mut c);
    c.flush_library();
    assert_eq!(c.library_writes(), 0);
    assert!(!library_file(&cfg).exists());
}

#[test]
fn flush_writes_a_pending_change_synchronously() {
    let lib = library("flush");
    let cfg = Tmp::new("flush-cfg");
    let mut c = core();
    c.set_library_path(Some(library_file(&cfg)));
    c.browse(BrowseAction::ToggleFavorite(vec![lib.path("kick 2.wav")]));
    // No frame ran: the snapshot is still pending.
    c.flush_library();
    assert!(library_file(&cfg).exists());
    assert!(!c.browser_busy());
    let text = std::fs::read_to_string(library_file(&cfg)).unwrap();
    assert!(text.contains("\"favorite\": true"));
}

#[test]
fn without_a_library_path_nothing_is_written() {
    let lib = library("memory");
    let mut c = core();
    c.browse(BrowseAction::ToggleFavorite(vec![lib.path("kick 2.wav")]));
    c.flush_library();
    wait_browser(&mut c);
    assert_eq!(c.library_writes(), 0);
}

#[test]
fn a_damaged_library_moves_to_bad_and_a_fresh_one_starts() {
    let cfg = Tmp::new("corrupt-cfg");
    let file = library_file(&cfg);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ definitely not json").unwrap();
    let mut c = core();
    c.set_library_path(Some(file.clone()));
    let bad = cfg.path("config/library.json.bad");
    assert_eq!(
        std::fs::read_to_string(&bad).unwrap(),
        "{ definitely not json"
    );
    assert!(!file.exists());
    assert!(c.last_rejection().unwrap().contains("damaged"));
    assert!(c.library_db().favorites().is_empty());
    // The fresh library is saved normally.
    c.browse(BrowseAction::AddPlace(PathBuf::from("/music")));
    c.flush_library();
    assert!(file.exists());
}

#[test]
fn a_newer_library_is_never_overwritten() {
    let cfg = Tmp::new("newer-cfg");
    let file = library_file(&cfg);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = r#"{"version":99,"future":true}"#;
    std::fs::write(&file, original).unwrap();
    let mut c = core();
    c.set_library_path(Some(file.clone()));
    assert!(c.last_rejection().unwrap().contains("newer"));
    c.browse(BrowseAction::AddPlace(PathBuf::from("/music")));
    // The change lives in memory for this session.
    assert_eq!(c.library_db().places().len(), 1);
    wait_browser(&mut c);
    c.flush_library();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert!(!cfg.path("config/library.json.bad").exists());
    assert!(!cfg.path("config/library.json.tmp").exists());
    assert_eq!(c.library_writes(), 0);
}

// ------------------------------------------------------------------------------- probing

#[test]
fn request_info_patches_rows_and_skips_unreadable_files() {
    let lib = library("probe");
    std::fs::write(lib.path("broken.wav"), b"not a wav").unwrap();
    let mut c = core();
    show(&mut c, &lib.0);
    c.browse(BrowseAction::RequestInfo(vec![
        lib.path("kick 2.wav"),
        lib.path("broken.wav"),
        lib.path("kick 2.wav"),
    ]));
    assert!(c.browser_busy());
    wait_browser(&mut c);
    let patched: Vec<(PathBuf, atomik_library::AudioInfo)> = trace(&c)
        .iter()
        .filter_map(|t| match t {
            ViewCall::PatchInfo(i) => Some(i.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    // The broken file gives no result; the readable one is described correctly (the panel
    // also asks for its visible rows, so it may be probed more than once).
    assert!(patched.iter().all(|(p, _)| p != &lib.path("broken.wav")));
    let kick: Vec<_> = patched
        .iter()
        .filter(|(p, _)| p == &lib.path("kick 2.wav"))
        .collect();
    assert!(!kick.is_empty());
    for (_, info) in kick {
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 1);
        assert_eq!(info.frames, Some(4_800));
    }
}

#[test]
fn visible_rows_are_probed_without_any_request_action() {
    let lib = library("autoprobe");
    let mut c = core();
    show(&mut c, &lib.0);
    // The view's own requests are polled every frame.
    wait_until(&mut c, "probe results", |c| {
        trace(c)
            .iter()
            .any(|t| matches!(t, ViewCall::PatchInfo(i) if !i.is_empty()))
    });
    wait_browser(&mut c);
    let probed: Vec<PathBuf> = trace(&c)
        .iter()
        .filter_map(|t| match t {
            ViewCall::PatchInfo(i) => Some(i.iter().map(|(p, _)| p.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(probed.contains(&lib.path("kick 2.wav")));
    assert!(probed.contains(&lib.path("kick 10.wav")));
}

// -------------------------------------------------------------------------------- preview

#[test]
fn preview_starts_the_stream_at_once_and_the_decoder_opens_the_file() {
    let lib = library("preview");
    let mut c = core();
    let file = lib.path("kick 2.wav");
    c.browse(BrowseAction::Preview(file.clone()));
    // Zero latency path: the engine had the stream before any worker answered.
    assert_eq!(preview_calls(&c), [SinkCall::PreviewStart { gain: 1.0 }]);
    assert_eq!(c.browser.last_preview, Some((Some(file.clone()), 0.0)));
    let monitor = c.preview_monitor().unwrap();
    wait_until(&mut c, "the decoder to open the file", |_| {
        monitor.source_rate() == 48_000
    });
    // Still playing (nothing consumes the recording sink's stream).
    assert!(c.browser_busy());
    assert_eq!(
        c.browser.last_preview.as_ref().map(|p| p.0.clone()),
        Some(Some(file))
    );
}

#[test]
fn stop_preview_stops_the_engine_and_clears_the_panel() {
    let lib = library("stop");
    let mut c = core();
    c.browse(BrowseAction::Preview(lib.path("kick 2.wav")));
    let monitor = c.preview_monitor().unwrap();
    c.browse(BrowseAction::StopPreview);
    assert_eq!(
        preview_calls(&c),
        [SinkCall::PreviewStart { gain: 1.0 }, SinkCall::PreviewStop]
    );
    assert_eq!(c.browser.last_preview, Some((None, 0.0)));
    assert!(c.preview_monitor().is_none());
    assert!(monitor.is_abandoned());
    wait_browser(&mut c);
}

#[test]
fn a_second_preview_replaces_the_first_and_the_old_decode_ends() {
    let lib = library("switch");
    let mut c = core();
    c.browse(BrowseAction::Preview(lib.path("kick 2.wav")));
    let first = c.preview_monitor().unwrap();
    c.browse(BrowseAction::Preview(lib.path("kick 10.wav")));
    assert!(first.is_abandoned());
    let second = c.preview_monitor().unwrap();
    wait_until(&mut c, "the second file to open", |_| {
        second.source_rate() == 48_000
    });
    assert_eq!(c.preview_path(), Some(lib.path("kick 10.wav").as_path()));
    let starts = c
        .sink()
        .calls
        .iter()
        .filter(|call| matches!(call, SinkCall::PreviewStart { .. }))
        .count();
    assert_eq!(starts, 2);
    // Only one decode is still running.
    c.browse(BrowseAction::StopPreview);
    wait_browser(&mut c);
}

#[test]
fn an_unreadable_file_reports_and_stops_the_preview() {
    let lib = library("badpreview");
    std::fs::write(lib.path("broken.wav"), b"not a wav at all").unwrap();
    let mut c = core();
    c.browse(BrowseAction::Preview(lib.path("broken.wav")));
    wait_until(&mut c, "the failure", |c| c.last_rejection().is_some());
    assert!(c.last_rejection().unwrap().contains("broken.wav"));
    assert!(c.preview_monitor().is_none());
    assert_eq!(c.sink().calls.last(), Some(&SinkCall::PreviewStop));
    wait_browser(&mut c);
}

#[test]
fn hiding_the_panel_stops_the_preview() {
    let lib = library("hide");
    let mut c = core();
    c.set_file_browser_folder(Some(lib.0.clone()));
    c.set_file_browser_visible(true);
    c.browse(BrowseAction::Preview(lib.path("kick 2.wav")));
    c.set_file_browser_visible(false);
    assert_eq!(c.sink().calls.last(), Some(&SinkCall::PreviewStop));
    assert!(c.preview_monitor().is_none());
    wait_browser(&mut c);
}

#[test]
fn a_stream_the_engine_dropped_ends_the_preview() {
    let lib = library("dropped");
    let mut c = core();
    c.browse(BrowseAction::Preview(lib.path("kick 2.wav")));
    // The engine lets go of the stream (a replacement, or no engine at all).
    c.sink_mut().previews.clear();
    c.frame();
    assert!(c.preview_monitor().is_none());
    assert_eq!(c.browser.last_preview, Some((None, 0.0)));
    wait_browser(&mut c);
}

#[test]
fn a_preview_without_an_engine_never_sticks() {
    let lib = library("noengine");
    let mut c = core();
    c.sink_mut().fail_next = Some(EngineError::QueueFull);
    c.browse(BrowseAction::Preview(lib.path("kick 2.wav")));
    assert!(c.last_rejection().is_some());
    assert!(c.preview_monitor().is_none());
    assert!(!c.browser_busy());
}

// --------------------------------------------------------------- preview through a real engine

fn peak(blocks: &[Vec<f32>]) -> f32 {
    blocks
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn a_preview_plays_audibly_through_a_real_engine_and_ends_by_itself() {
    let lib = Tmp::new("audible");
    wav(&lib.path("tone.wav"), 24_000);
    let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    // Silence before.
    assert_eq!(peak(&c.sink_mut().render(512)), 0.0);

    c.browse(BrowseAction::Preview(lib.path("tone.wav")));
    let mut heard = 0.0f32;
    let mut played = 0usize;
    for _ in 0..2_000 {
        let out = c.sink_mut().render(64);
        heard = heard.max(peak(&out));
        played += 64;
        c.frame();
        if heard > 0.05 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        heard > 0.05,
        "the preview stayed silent after {played} frames"
    );
    assert!(c.sink().preview_active());

    // Progress follows playback (24 000 frames at 48 kHz source and engine rate).
    for _ in 0..40 {
        c.sink_mut().render(64);
    }
    c.frame();
    let (path, progress) = c.browser.last_preview.clone().unwrap();
    assert_eq!(path, Some(lib.path("tone.wav")));
    assert!(progress > 0.0 && progress < 1.0, "progress {progress}");

    // Play it out: the panel clears by itself.
    for _ in 0..2_000 {
        c.sink_mut().render(256);
        c.frame();
        if c.preview_monitor().is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(c.preview_monitor().is_none(), "the preview never finished");
    assert_eq!(c.browser.last_preview, Some((None, 0.0)));
    wait_browser(&mut c);
}

#[test]
fn switching_previews_on_a_real_engine_keeps_sounding() {
    let lib = Tmp::new("switch-audible");
    wav(&lib.path("a.wav"), 96_000);
    wav(&lib.path("b.wav"), 96_000);
    let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    c.browse(BrowseAction::Preview(lib.path("a.wav")));
    let mut heard = false;
    for _ in 0..2_000 {
        heard |= peak(&c.sink_mut().render(64)) > 0.05;
        c.frame();
        if heard {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(heard);
    c.browse(BrowseAction::Preview(lib.path("b.wav")));
    let mut heard_b = false;
    for _ in 0..2_000 {
        heard_b |= peak(&c.sink_mut().render(64)) > 0.05;
        c.frame();
        if heard_b {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(heard_b);
    c.browse(BrowseAction::StopPreview);
    for _ in 0..20 {
        c.sink_mut().render(64);
    }
    assert_eq!(peak(&c.sink_mut().render(256)), 0.0);
}

// ------------------------------------------------------------------- visibility and busy

#[test]
fn first_show_lists_roots_and_the_restored_folder() {
    let lib = library("restore");
    let mut c = core();
    c.set_file_browser_folder(Some(lib.path("sub")));
    assert_eq!(c.file_browser_folder(), Some(lib.path("sub")));
    assert!(!c.file_browser_visible());
    c.set_file_browser_visible(true);
    assert!(c.file_browser_visible());
    assert!(c.browser_busy());
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Folder(lib.path("sub")))
    );
    wait_browser(&mut c);
    assert_eq!(names(&rows(&c)), ["deep", "snare.wav"]);
    let roots = last_places(&c).roots;
    assert!(!roots.is_empty());
    assert!(roots.iter().all(|r| r.exists()));
    assert_eq!(c.file_browser_folder(), Some(lib.path("sub")));
}

#[test]
fn a_second_show_does_not_list_again() {
    let lib = library("again");
    let mut c = core();
    c.set_file_browser_folder(Some(lib.0.clone()));
    c.set_file_browser_visible(true);
    wait_browser(&mut c);
    let lists = set_rows_count(&c);
    c.set_file_browser_visible(false);
    c.set_file_browser_visible(true);
    wait_browser(&mut c);
    assert_eq!(set_rows_count(&c), lists);
}

#[test]
fn first_show_without_a_folder_opens_home() {
    let mut c = core();
    c.set_file_browser_visible(true);
    wait_browser(&mut c);
    if let Some(home) = crate::browser_workers::home_dir() {
        assert_eq!(c.file_browser().location(), Some(&Place::Folder(home)));
    }
}

#[test]
fn a_restored_folder_that_is_gone_falls_back_to_home() {
    let lib = library("fallback");
    let Some(home) = crate::browser_workers::home_dir() else {
        return;
    };
    let mut c = core();
    c.set_file_browser_folder(Some(lib.path("deleted")));
    c.set_file_browser_visible(true);
    wait_browser(&mut c);
    assert_eq!(c.file_browser().location(), Some(&Place::Folder(home)));
    assert!(c.last_rejection().is_none());
}

#[test]
fn the_core_is_busy_while_the_browser_works() {
    let lib = library("busy");
    let mut c = core();
    assert!(!c.is_busy() && !c.browser_busy());
    c.browse(BrowseAction::Show(Place::Folder(lib.0.clone())));
    assert!(c.is_busy());
    wait_browser(&mut c);
    assert!(!c.is_busy());
}

// ----------------------------------------------------------------------------------- AC-12

/// Every path below `dir` with its length and modification time.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>)> {
    fn visit(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            // Not `entry.metadata()`: NTFS serves directory times from a lagging cache there.
            let meta = std::fs::metadata(entry.path()).unwrap();
            out.insert(entry.path(), (meta.len(), meta.modified().ok()));
            if meta.is_dir() {
                visit(&entry.path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(dir, &mut out);
    out
}

#[test]
fn a_full_browse_session_leaves_the_user_folder_untouched() {
    let lib = library("ac12");
    let cfg = Tmp::new("ac12-cfg");
    let before = snapshot(&lib.0);
    let mut c = core();
    c.set_library_path(Some(library_file(&cfg)));
    c.set_file_browser_folder(Some(lib.0.clone()));
    c.set_file_browser_visible(true);
    wait_browser(&mut c);
    c.browse(BrowseAction::ListChildren(lib.0.clone()));
    click(&mut c, 240.0, 17.0);
    wait_browser(&mut c);
    let kick = lib.path("kick 2.wav");
    c.browse(BrowseAction::RequestInfo(vec![kick.clone()]));
    c.browse(BrowseAction::Preview(kick.clone()));
    wait_until(&mut c, "the decoder", |c| {
        c.preview_monitor()
            .is_some_and(|m| m.source_rate() == 48_000)
    });
    c.browse(BrowseAction::StopPreview);
    c.browse(BrowseAction::ToggleFavorite(vec![kick.clone()]));
    c.browse(BrowseAction::AddTag {
        paths: vec![kick.clone()],
        tag: "808".into(),
    });
    c.browse(BrowseAction::RenameTag {
        from: "808".into(),
        to: "909".into(),
    });
    c.browse(BrowseAction::AddPlace(lib.0.clone()));
    c.browse(BrowseAction::Show(Place::Favorites));
    wait_browser(&mut c);
    c.browse(BrowseAction::Refresh);
    c.flush_library();
    wait_browser(&mut c);
    assert_eq!(snapshot(&lib.0), before);
    // The only file the browser wrote is the library, in its own folder.
    let written: Vec<String> = std::fs::read_dir(cfg.path("config"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written, ["library.json"]);
}
