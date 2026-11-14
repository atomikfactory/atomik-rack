//! The view state a project keeps for the meters and the rack: the folded insert areas and the
//! second curve of each Spectrum device. Each is one undo step that reports a view change, an
//! optional ignorable root field of format 45 written only when set, and old files load.

mod common;

use atomik_model::{
    Command, CurveFrom, DeviceKey, Document, PROJECT_VERSION, SpectrumCurve, SyncOp, load_project,
    save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn doc() -> (Document, atomik_device_sdk::Registry) {
    let r = registry();
    (three_devices(&r), r)
}

fn pair(_: &Document) -> (DeviceKey, DeviceKey) {
    (key("synth"), key("delay"))
}

#[test]
fn a_new_project_has_neither_list_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().folded_inserts.is_empty() && d.doc().spectrum_curves.is_empty());
    let t = text(&d);
    assert!(!t.contains("inserts_folded\": ["), "{t}");
    assert!(!t.contains("spectrum_curves\": ["), "{t}");
    assert_eq!(PROJECT_VERSION, 45, "the format does not change");
}

#[test]
fn the_fold_state_is_one_undo_step_and_comes_back_on_load() {
    let (mut d, r) = doc();
    let (a, b) = pair(&d);
    let none = text(&d);
    let ops = one_step(
        &mut d,
        &r,
        Command::SetInsertsFolded(vec![b.clone(), a.clone(), a.clone()]),
    );
    assert!(ops.contains(&SyncOp::ViewChanged), "{ops:?}");
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(d.doc().folded_inserts, want, "sorted, without repeats");
    let saved = text(&d);
    assert!(saved.contains("\"inserts_folded\": ["), "{saved}");
    assert!(
        saved.contains("\"inserts_folded\": 45"),
        "the ignorable header"
    );
    let loaded = load_project(&saved, &r).expect("loads");
    assert_eq!(loaded.folded_inserts, want);
    assert_eq!(save_project(&loaded), saved, "byte for byte");
    assert!(d.undo().is_some());
    assert!(d.doc().folded_inserts.is_empty());
    assert_eq!(text(&d), none);
    assert!(d.redo().is_some());
    rejected(&mut d, &r, Command::SetInsertsFolded(want));
}

#[test]
fn the_spectrum_curve_is_one_undo_step_and_comes_back_on_load() {
    let (mut d, r) = doc();
    let (a, b) = pair(&d);
    let row = SpectrumCurve {
        device: a.clone(),
        from: CurveFrom::Strip(b),
    };
    let reference = SpectrumCurve {
        device: DeviceKey("x".into()),
        from: CurveFrom::Reference,
    };
    let none = text(&d);
    let ops = one_step(&mut d, &r, Command::SetSpectrumCurves(vec![row.clone()]));
    assert!(ops.contains(&SyncOp::ViewChanged), "{ops:?}");
    let saved = text(&d);
    assert!(saved.contains("\"spectrum_curves\": ["), "{saved}");
    assert!(
        saved.contains("\"spectrum_curves\": 45"),
        "the ignorable header"
    );
    let loaded = load_project(&saved, &r).expect("loads");
    assert_eq!(loaded.spectrum_curves, vec![row.clone()]);
    assert_eq!(save_project(&loaded), saved, "byte for byte");
    // A device twice is refused; a repeat is refused.
    rejected(
        &mut d,
        &r,
        Command::SetSpectrumCurves(vec![
            row.clone(),
            SpectrumCurve {
                device: a,
                ..row.clone()
            },
        ]),
    );
    rejected(&mut d, &r, Command::SetSpectrumCurves(vec![row.clone()]));
    // A row for a device the project does not have is kept in memory but never written.
    one_step(&mut d, &r, Command::SetSpectrumCurves(vec![reference]));
    assert_eq!(text(&d), none, "nothing is saved for a device that is gone");
    assert!(d.undo().is_some() && d.undo().is_some());
    assert!(d.doc().spectrum_curves.is_empty());
}

#[test]
fn an_older_file_without_the_fields_loads_with_empty_lists() {
    let (d, r) = doc();
    let loaded = load_project(&text(&d), &r).expect("loads");
    assert!(loaded.folded_inserts.is_empty() && loaded.spectrum_curves.is_empty());
}

#[test]
fn the_fields_are_unknown_in_a_version_44_file_without_the_header() {
    let (mut d, r) = doc();
    let (a, _) = pair(&d);
    one_step(&mut d, &r, Command::SetInsertsFolded(vec![a]));
    let old = text(&d).replacen("\"version\": 45", "\"version\": 44", 1);
    assert!(load_project(&old, &r).is_err());
}

#[test]
fn the_limit_holds_in_the_command() {
    let (mut d, r) = doc();
    let many: Vec<DeviceKey> = (0..=atomik_model::MAX_VIEW_ENTRIES)
        .map(|i| DeviceKey(format!("k{i}")))
        .collect();
    rejected(&mut d, &r, Command::SetInsertsFolded(many));
}
