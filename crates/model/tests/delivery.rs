//! The delivery target: `SetDelivery` as one undo step (alone and batched with a parameter), the
//! optional `"delivery"` field of the project format (written only when set, byte-identical save,
//! load and save) and its limits.

mod common;

use atomik_model::{
    Command, Delivery, Document, ModelError, SyncOp, TARGETS, load_project, new_project_doc,
    save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

fn streaming() -> Delivery {
    Delivery::of(&TARGETS[1])
}

#[test]
fn a_new_project_has_none_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().delivery.is_none());
    assert!(!text(&d).contains("delivery"));
}

#[test]
fn setting_and_clearing_the_target_are_one_undo_step_each() {
    let (mut d, r) = doc();
    let ops = one_step(&mut d, &r, Command::SetDelivery(Some(streaming())));
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.doc().delivery, Some(streaming()));
    assert_eq!(d.undo_label(), Some("Set Delivery Target"));
    rejected(&mut d, &r, Command::SetDelivery(Some(streaming())));
    one_step(&mut d, &r, Command::SetDelivery(None));
    assert!(d.doc().delivery.is_none());
    d.undo().unwrap();
    assert_eq!(d.doc().delivery, Some(streaming()));
    d.undo().unwrap();
    assert!(d.doc().delivery.is_none());
}

#[test]
fn the_limits_are_enforced() {
    let (mut d, r) = doc();
    let mut bad = streaming();
    bad.name.clear();
    for d2 in [
        bad.clone(),
        Delivery {
            name: "a\nb".into(),
            ..streaming()
        },
        Delivery {
            lufs: 4.0,
            ..streaming()
        },
        Delivery {
            ceiling_dbtp: -40.0,
            ..streaming()
        },
        Delivery {
            lufs: f32::NAN,
            ..streaming()
        },
    ] {
        let e = rejected(&mut d, &r, Command::SetDelivery(Some(d2)));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    }
}

#[test]
fn the_save_is_canonical_and_loads_back_byte_for_byte() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetDelivery(Some(streaming())));
    let saved = text(&d);
    assert!(saved.contains("\"delivery\": {"), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.delivery, Some(streaming()));
    assert_eq!(save_project(&back), saved, "save, load, save");
}

#[test]
fn the_loader_names_the_field_of_a_bad_target() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetDelivery(Some(streaming())));
    let saved = text(&d);
    let bad = saved.replacen("\"lufs\": -14", "\"lufs\": \"loud\"", 1);
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(e.contains("delivery.lufs"), "{e}");
    let far = saved.replacen("\"ceiling\": -1", "\"ceiling\": 6", 1);
    let e = load_project(&far, &r).unwrap_err().to_string();
    assert!(e.contains("delivery"), "{e}");
}

#[test]
fn the_target_and_a_parameter_batch_into_one_undo_step() {
    let (mut d, r) = doc();
    let master = d
        .doc()
        .console
        .master
        .clone()
        .expect("a new project has a master");
    let level = |d: &Document| d.doc().param_value(&master, "comp.on", &r);
    let before = level(&d);
    one_step(
        &mut d,
        &r,
        Command::Batch(vec![
            Command::SetParam {
                key: master.clone(),
                param: "comp.on".into(),
                value: 1.0,
                merge: None,
            },
            Command::SetDelivery(Some(streaming())),
        ]),
    );
    assert_eq!(d.doc().delivery, Some(streaming()));
    assert_eq!(level(&d), Some(1.0));
    d.undo().unwrap();
    assert!(d.doc().delivery.is_none());
    assert_eq!(level(&d), before);
    d.redo().unwrap();
    assert_eq!(d.doc().delivery, Some(streaming()));
}
