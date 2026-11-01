//! The song information: `SetSongInfo` as one undo step, the optional `"info"` field of project
//! format 38 (written only when set, byte-identical save, load and save), the limits with their
//! JSON paths, and the way old projects load.

mod common;

use atomik_model::{
    Command, Document, MAX_INFO_AUTHOR_CHARS, MAX_INFO_EMAIL_CHARS, MAX_INFO_NOTES_CHARS,
    MAX_INFO_TITLE_CHARS, MAX_INFO_URL_CHARS, ModelError, PROJECT_VERSION, SongInfo, SyncOp,
    empty_doc, load_project, new_project_doc, save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn info() -> SongInfo {
    SongInfo {
        title: "Example Song".into(),
        author: "Example Author".into(),
        notes: "First line\nSecond \"line\"\twith a tab".into(),
        url: "https://example.org/song".into(),
        email: "author@example.org".into(),
    }
}

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

#[test]
fn a_new_project_has_no_information_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().info.is_empty());
    assert!(!text(&d).contains("\"info\""));
    assert_eq!(PROJECT_VERSION, 39);
}

#[test]
fn setting_the_information_is_one_undo_step_that_reports_a_view_change() {
    let (mut d, r) = doc();
    let none = text(&d);
    let ops = one_step(&mut d, &r, Command::SetSongInfo(info()));
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.doc().info, info());
    assert_eq!(d.undo_label(), Some("Edit Song Information"));
    d.undo(&r).unwrap();
    assert_eq!(text(&d), none);
    assert!(d.doc().info.is_empty());
    d.redo(&r).unwrap();
    assert_eq!(d.doc().info, info());
}

#[test]
fn clearing_every_text_removes_the_field() {
    let (mut d, r) = doc();
    let none = text(&d);
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    one_step(&mut d, &r, Command::SetSongInfo(SongInfo::default()));
    assert_eq!(text(&d), none);
}

#[test]
fn the_save_is_canonical_and_loads_back_byte_for_byte() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    let saved = text(&d);
    assert!(saved.contains("\"info\": {"), "{saved}");
    assert!(saved.contains("\"notes\": \"First line\\nSecond \\\"line\\\"\\twith a tab\""));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.info, info());
    assert_eq!(save_project(&back), saved, "save, load, save");
}

#[test]
fn only_the_texts_that_are_set_are_written() {
    let (mut d, r) = doc();
    one_step(
        &mut d,
        &r,
        Command::SetSongInfo(SongInfo {
            title: "Only a title".into(),
            ..SongInfo::default()
        }),
    );
    let saved = text(&d);
    assert!(saved.contains("\"title\": \"Only a title\""));
    for other in ["author", "email", "url", "notes"] {
        assert!(!saved.contains(&format!("\"{other}\"")), "{other}");
    }
    assert_eq!(save_project(&load_project(&saved, &r).unwrap()), saved);
}

#[test]
fn the_texts_are_stored_as_typed_without_trimming() {
    let (mut d, r) = doc();
    let padded = SongInfo {
        title: "  padded  ".into(),
        ..SongInfo::default()
    };
    one_step(&mut d, &r, Command::SetSongInfo(padded.clone()));
    let back = load_project(&text(&d), &r).unwrap();
    assert_eq!(back.info, padded);
}

#[test]
fn an_unchanged_information_is_rejected() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    rejected(&mut d, &r, Command::SetSongInfo(info()));
    rejected(&mut d, &r, Command::SetSongInfo(info()));
    let (mut empty, r) = doc();
    rejected(&mut empty, &r, Command::SetSongInfo(SongInfo::default()));
}

#[test]
fn the_command_enforces_the_limits() {
    let (mut d, r) = doc();
    let long = |n: usize| "x".repeat(n);
    for (name, bad) in [
        (
            "title",
            SongInfo {
                title: long(MAX_INFO_TITLE_CHARS + 1),
                ..SongInfo::default()
            },
        ),
        (
            "author",
            SongInfo {
                author: long(MAX_INFO_AUTHOR_CHARS + 1),
                ..SongInfo::default()
            },
        ),
        (
            "email",
            SongInfo {
                email: long(MAX_INFO_EMAIL_CHARS + 1),
                ..SongInfo::default()
            },
        ),
        (
            "url",
            SongInfo {
                url: long(MAX_INFO_URL_CHARS + 1),
                ..SongInfo::default()
            },
        ),
        (
            "notes",
            SongInfo {
                notes: long(MAX_INFO_NOTES_CHARS + 1),
                ..SongInfo::default()
            },
        ),
        (
            "title",
            SongInfo {
                title: "two\nlines".into(),
                ..SongInfo::default()
            },
        ),
        (
            "notes",
            SongInfo {
                notes: "bell\u{7}".into(),
                ..SongInfo::default()
            },
        ),
    ] {
        let e = rejected(&mut d, &r, Command::SetSongInfo(bad));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
        assert!(e.to_string().contains(name), "{name}: {e}");
    }
    // The limits themselves are allowed (in characters, not bytes).
    let at_limit = SongInfo {
        title: "\u{e9}".repeat(MAX_INFO_TITLE_CHARS),
        notes: "n\n".repeat(MAX_INFO_NOTES_CHARS / 2),
        ..SongInfo::default()
    };
    one_step(&mut d, &r, Command::SetSongInfo(at_limit.clone()));
    let back = load_project(&text(&d), &r).unwrap();
    assert_eq!(back.info, at_limit);
}

#[test]
fn the_loader_names_the_json_path_of_a_text_that_breaks_a_limit() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    let saved = text(&d);
    let too_long = saved.replacen(
        "\"Example Song\"",
        &format!("\"{}\"", "x".repeat(MAX_INFO_TITLE_CHARS + 1)),
        1,
    );
    let e = load_project(&too_long, &r).unwrap_err().to_string();
    assert!(e.contains("info.title"), "{e}");
    let multiline = saved.replacen("\"Example Author\"", "\"a\\nb\"", 1);
    let e = load_project(&multiline, &r).unwrap_err().to_string();
    assert!(e.contains("info.author") && e.contains("control"), "{e}");
    let unknown = saved.replacen("\"title\"", "\"subtitle\"", 1);
    let e = load_project(&unknown, &r).unwrap_err().to_string();
    assert!(
        e.contains("info.subtitle") && e.contains("unknown field"),
        "{e}"
    );
    let wrong_type = saved.replacen("\"Example Song\"", "5", 1);
    let e = load_project(&wrong_type, &r).unwrap_err().to_string();
    assert!(e.contains("info.title"), "{e}");
    let not_object = saved.replacen("\"info\": {", "\"info\": [], \"x\": {", 1);
    assert!(load_project(&not_object, &r).is_err());
}

#[test]
fn the_field_is_unknown_in_a_version_37_file() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    let old = text(&d).replacen("\"version\": 39", "\"version\": 37", 1);
    let e = load_project(&old, &r).unwrap_err().to_string();
    assert!(e.contains("info") && e.contains("unknown field"), "{e}");
}

#[test]
fn an_old_project_loads_without_the_field_and_differs_in_the_version_line_alone() {
    let r = registry();
    let current = save_project(&empty_doc());
    let old = current.replacen("\"version\": 39", "\"version\": 37", 1);
    let loaded = load_project(&old, &r).expect("a version 37 file loads");
    assert!(loaded.info.is_empty());
    assert_eq!(save_project(&loaded), current);
}

#[test]
fn a_copy_of_the_document_keeps_its_information() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    let copy = d.doc().clone();
    assert_eq!(copy.info, info());
    assert_eq!(copy, *d.doc());
}

#[test]
fn the_corpus_file_with_information_is_what_the_writer_makes() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetSongInfo(info()));
    let saved = text(&d);
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/song-info-v39.atomik");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &saved).unwrap();
    }
    let file = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(file, saved, "regenerate with ATOMIK_UPDATE_GOLDEN=1");
}
