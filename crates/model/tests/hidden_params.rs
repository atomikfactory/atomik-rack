//! Hidden params (`hidden.*`): stored, undoable and saved like any other param, but no list of
//! targets offers them. The Sidechain Tool keeps the points of its drawn curve in them.

mod common;

use atomik_model::{
    Command, Document, ParamIndex, SearchTarget, SongCommand, load_project, new_project_doc,
    save_project,
};
use common::*;

const DUCK: &str = "atomik.duck";
const COUNT: &str = "hidden.curve_n";
const X2: &str = "hidden.curve_p2_x";

fn project() -> (Document, atomik_device_sdk::Registry) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    d.apply(add(usize::MAX, DUCK, Some("duck")), &r).unwrap();
    (d, r)
}

fn set(d: &mut Document, r: &atomik_device_sdk::Registry, param: &str, value: f32) {
    d.apply(
        Command::SetParam {
            key: key("duck"),
            param: param.into(),
            value,
            merge: None,
        },
        r,
    )
    .unwrap_or_else(|e| panic!("{param}: {e}"));
}

#[test]
fn a_hidden_param_is_stored_undoable_and_saved_with_the_project() {
    let (mut d, r) = project();
    set(&mut d, &r, COUNT, 3.0);
    set(&mut d, &r, X2, 0.25);
    let device = d.doc().device(&key("duck")).unwrap();
    assert!((device.params[COUNT] - 3.0).abs() < f32::EPSILON);
    assert!((device.params[X2] - 0.25).abs() < f32::EPSILON);
    // The value is clamped by the descriptor, like any param.
    set(&mut d, &r, X2, 7.0);
    assert!((d.doc().device(&key("duck")).unwrap().params[X2] - 1.0).abs() < f32::EPSILON);
    // One undo per edit.
    d.undo().unwrap();
    assert!((d.doc().device(&key("duck")).unwrap().params[X2] - 0.25).abs() < f32::EPSILON);
    // The project text keeps them and the load gives the same document back.
    let saved = save_project(d.doc());
    assert!(saved.contains(COUNT), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert!(&back == d.doc());
    assert_eq!(save_project(&back), saved, "the save is canonical");
}

#[test]
fn the_internal_params_stay_unstorable() {
    let (mut d, r) = project();
    let err = d.apply(
        Command::SetParam {
            key: key("duck"),
            param: "internal.match".into(),
            value: 1.0,
            merge: None,
        },
        &r,
    );
    assert!(err.is_err());
}

#[test]
fn no_list_of_targets_offers_a_hidden_param() {
    let (mut d, r) = project();
    // The search index.
    let index = ParamIndex::build(d.doc(), &r);
    assert!(index.search("curve", 200).iter().all(|e| match &e.target {
        SearchTarget::Param { param, .. } => !param.starts_with("hidden."),
        _ => true,
    }));
    // An automation lane.
    let err = d.apply(
        Command::Song(SongCommand::AddAutomationLane {
            device: key("duck"),
            param: COUNT.into(),
        }),
        &r,
    );
    assert!(err.is_err(), "no lane on a hidden param");
}
