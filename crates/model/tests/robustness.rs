//! Truncated, corrupted and hostile documents (project, patch, clip and library item files) are
//! refused or loaded; they never panic and never nest without bound.

mod common;

use atomik_model::{load_clip_file, load_device_item, load_patch, load_project};
use common::console_registry;
use proptest::prelude::*;
use std::path::PathBuf;

/// Real documents: the newest committed project, a container project and the patch examples.
fn documents() -> Vec<String> {
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = vec![
        model.join("tests/fixtures/demo-v36.atomik"),
        model.join("tests/fixtures/container-v8.atomik"),
    ];
    let patches = model.join("../../examples/patches");
    let mut examples: Vec<_> = std::fs::read_dir(patches)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    examples.sort();
    paths.extend(examples);
    paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap().replace("\r\n", "\n"))
        .collect()
}

/// Every loader on `text`: none may panic.
fn load_everywhere(text: &str) {
    let registry = console_registry();
    let _ = load_project(text, &registry);
    let _ = load_patch(text, &registry);
    let _ = load_device_item(text, &registry);
    let _ = load_clip_file(text);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn truncated_documents_never_panic(index in 0usize..16, frac in 0.0f64..1.0) {
        let docs = documents();
        let text = &docs[index % docs.len()];
        let mut cut = (text.len() as f64 * frac) as usize;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        load_everywhere(&text[..cut]);
    }

    #[test]
    fn corrupted_documents_never_panic(
        index in 0usize..16,
        edits in proptest::collection::vec((0.0f64..1.0, any::<u8>()), 1..16),
    ) {
        let docs = documents();
        let mut bytes = docs[index % docs.len()].clone().into_bytes();
        for (pos, value) in edits {
            let i = ((bytes.len() - 1) as f64 * pos) as usize;
            bytes[i] = value;
        }
        load_everywhere(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn the_documents_themselves_load_somewhere() {
    // Guards the proptests above against reading nothing useful.
    let registry = console_registry();
    let docs = documents();
    assert!(docs.len() >= 3);
    assert!(load_project(&docs[0], &registry).is_ok());
}

#[test]
fn deeply_nested_json_is_an_error_not_a_stack_overflow() {
    for depth in [200usize, 10_000, 200_000] {
        let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        load_everywhere(&nested);
        let objects = format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
        load_everywhere(&objects);
    }
}

#[test]
fn absurd_numbers_and_lengths_are_refused() {
    let registry = console_registry();
    let huge = [
        r#"{"format":"atomik-project","version":36,"sample_rate":4294967295,"devices":[]}"#,
        r#"{"format":"atomik-project","version":36,"sample_rate":48000,"tempo":1e308,"devices":[]}"#,
        r#"{"format":"atomik-project","version":99999999999999999999,"devices":[]}"#,
        r#"{"format":"atomik-project","version":36,"song":{"tracks":null},"devices":[]}"#,
    ];
    for text in huge {
        assert!(load_project(text, &registry).is_err(), "{text}");
    }
}
