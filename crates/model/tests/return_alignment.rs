//! The return-alignment project flag: on in a new project, off in every older file, saved only
//! when on, refused in a file older than the format that introduced it, and one undo step.

mod common;

use atomik_model::{
    Command, ConsoleCommand, Document, PROJECT_VERSION, load_project, new_project_doc, save_project,
};
use common::{console_registry, legacy_project_doc};

const FIELD: &str = "\"align_returns\": true";

#[test]
fn a_new_project_has_the_flag_on_and_saves_it() {
    let doc = new_project_doc();
    assert!(doc.console.align_returns);
    let text = save_project(&doc);
    assert!(text.contains(FIELD), "{text}");
    let loaded = load_project(&text, &console_registry()).unwrap();
    assert!(loaded.console.align_returns);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_file_without_the_field_loads_with_the_flag_off_and_saves_without_it() {
    let doc = legacy_project_doc();
    assert!(!doc.console.align_returns);
    let text = save_project(&doc);
    assert!(!text.contains("align_returns"), "{text}");
    for version in [PROJECT_VERSION, 36, 29] {
        let old = text.replace(
            &format!("\"version\": {PROJECT_VERSION}"),
            &format!("\"version\": {version}"),
        );
        let loaded = load_project(&old, &console_registry()).unwrap();
        assert!(!loaded.console.align_returns, "version {version}");
    }
}

#[test]
fn the_flag_is_refused_in_a_file_older_than_the_format_that_added_it() {
    let text = save_project(&new_project_doc());
    let old = text.replace(
        &format!("\"version\": {PROJECT_VERSION}"),
        &format!("\"version\": {}", PROJECT_VERSION - 1),
    );
    let e = load_project(&old, &console_registry())
        .unwrap_err()
        .to_string();
    assert!(e.contains("console.align_returns"), "{e}");
}

#[test]
fn switching_the_flag_is_one_undo_step() {
    let reg = console_registry();
    let mut d = Document::new(legacy_project_doc());
    d.apply(
        Command::Console(ConsoleCommand::SetAlignReturns(true)),
        &reg,
    )
    .unwrap();
    assert!(d.doc().console.align_returns);
    assert!(save_project(d.doc()).contains(FIELD));
    d.undo(&reg).unwrap();
    assert!(!d.doc().console.align_returns);
    d.redo(&reg).unwrap();
    assert!(d.doc().console.align_returns);
}
