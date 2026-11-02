//! The master tune: `SetMasterTune` as one undo step, the optional `"tune_cents"` field of project
//! format 40 (written only when not 0, byte-identical save, load and save), its limits and the
//! way old projects load.

mod common;

use atomik_model::{
    Command, Document, MASTER_TUNE_MAX_CENTS, ModelError, SyncOp, empty_doc, load_project,
    new_project_doc, save_project, tune_ratio,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

#[test]
fn a_new_project_is_in_tune_and_saves_no_field() {
    let (d, _) = doc();
    assert_eq!(d.doc().tune_cents, 0.0);
    assert!(!text(&d).contains("tune_cents"));
}

#[test]
fn setting_the_tune_is_one_undo_step_that_reports_a_tune_change() {
    let (mut d, r) = doc();
    let none = text(&d);
    let ops = one_step(&mut d, &r, Command::SetMasterTune(-12.5));
    assert_eq!(ops, vec![SyncOp::TuneChanged(-12.5)]);
    assert_eq!(d.doc().tune_cents, -12.5);
    assert_eq!(d.undo_label(), Some("Change Master Tune"));
    d.undo(&r).unwrap();
    assert_eq!(text(&d), none);
    assert_eq!(d.doc().tune_cents, 0.0);
    d.redo(&r).unwrap();
    assert_eq!(d.doc().tune_cents, -12.5);
}

#[test]
fn setting_the_tune_back_to_zero_removes_the_field() {
    let (mut d, r) = doc();
    let none = text(&d);
    one_step(&mut d, &r, Command::SetMasterTune(30.0));
    assert!(text(&d).contains("\"tune_cents\": 30"));
    one_step(&mut d, &r, Command::SetMasterTune(0.0));
    assert_eq!(text(&d), none);
}

#[test]
fn the_current_value_and_values_out_of_range_are_rejected() {
    let (mut d, r) = doc();
    rejected(&mut d, &r, Command::SetMasterTune(0.0));
    rejected(&mut d, &r, Command::SetMasterTune(-0.0));
    for bad in [100.5, -100.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let e = rejected(&mut d, &r, Command::SetMasterTune(bad));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{bad}: {e:?}");
    }
    one_step(&mut d, &r, Command::SetMasterTune(MASTER_TUNE_MAX_CENTS));
    one_step(&mut d, &r, Command::SetMasterTune(-MASTER_TUNE_MAX_CENTS));
    rejected(&mut d, &r, Command::SetMasterTune(-MASTER_TUNE_MAX_CENTS));
}

#[test]
fn the_save_is_canonical_and_loads_back_byte_for_byte() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetMasterTune(-31.25));
    let saved = text(&d);
    assert!(saved.contains("\"tune_cents\": -31.25,"), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.tune_cents, -31.25);
    assert_eq!(save_project(&back), saved, "save, load, save");
}

#[test]
fn a_file_without_the_field_loads_in_tune_and_saves_the_same_bytes() {
    let r = registry();
    let current = save_project(&empty_doc());
    let loaded = load_project(&current, &r).unwrap();
    assert_eq!(loaded.tune_cents, 0.0);
    assert_eq!(save_project(&loaded), current);
    // An explicit zero is accepted and written back without the field.
    let explicit = current.replacen("  \"seed\"", "  \"tune_cents\": 0,\n  \"seed\"", 1);
    let loaded = load_project(&explicit, &r).unwrap();
    assert_eq!(save_project(&loaded), current);
}

#[test]
fn the_loader_names_the_path_of_a_bad_value() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetMasterTune(10.0));
    let saved = text(&d);
    let high = saved.replacen("\"tune_cents\": 10", "\"tune_cents\": 150", 1);
    let e = load_project(&high, &r).unwrap_err().to_string();
    assert!(e.contains("tune_cents") && e.contains("-100"), "{e}");
    let wrong_type = saved.replacen("\"tune_cents\": 10", "\"tune_cents\": \"ten\"", 1);
    let e = load_project(&wrong_type, &r).unwrap_err().to_string();
    assert!(e.contains("tune_cents"), "{e}");
}

#[test]
fn the_field_is_unknown_in_a_version_39_file() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetMasterTune(10.0));
    let old = text(&d).replacen("\"version\": 40", "\"version\": 39", 1);
    let e = load_project(&old, &r).unwrap_err().to_string();
    assert!(
        e.contains("tune_cents") && e.contains("unknown field"),
        "{e}"
    );
}

#[test]
fn the_ratio_is_two_to_the_cents_over_twelve_hundred() {
    assert_eq!(tune_ratio(0.0), 1.0);
    assert!((tune_ratio(1200.0) - 2.0).abs() < 1e-12);
    assert!((tune_ratio(50.0) - 2f64.powf(50.0 / 1200.0)).abs() < 1e-12);
    assert!((tune_ratio(-100.0) * tune_ratio(100.0) - 1.0).abs() < 1e-12);
}

#[test]
fn a_copy_of_the_document_keeps_its_tune() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetMasterTune(7.0));
    let copy = d.doc().clone();
    assert_eq!(copy.tune_cents, 7.0);
    assert_eq!(copy, *d.doc());
}
