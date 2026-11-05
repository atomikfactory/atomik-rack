//! Feedback routing in the model: a loop is legal only when it passes through a Feedback Route
//! (a device whose outputs and inputs are different processing stages), whether it is cabled by
//! hand, written to a project and read back, or checked after the containers are flattened.

mod common;

use atomik_model::{Command, ModelError, load_project, save_project};
use common::*;

const GAIN: &str = "atomik.gain-tool";
const FEEDBACK: &str = "atomik.feedback";

/// A gain tool fed by a synth, and a feedback route beside it; the loop is not cabled yet.
fn rack(reg: &atomik_device_sdk::Registry) -> atomik_model::Document {
    let mut d = three_devices(reg);
    d.apply(add(3, GAIN, Some("gain")), reg).unwrap();
    d.apply(add(4, FEEDBACK, Some("fb")), reg).unwrap();
    for (f, t) in [
        ("synth:out_l", "gain:in_l"),
        ("synth:out_r", "gain:in_r"),
        ("gain:out_l", "mixer:ch1_in_l"),
        ("gain:out_r", "mixer:ch1_in_r"),
    ] {
        d.apply(connect(f, t), reg).unwrap();
    }
    d
}

fn refused(
    d: &mut atomik_model::Document,
    reg: &atomik_device_sdk::Registry,
    c: Command,
) -> String {
    match d.apply(c, reg) {
        Err(ModelError::BadCable(why)) => why,
        other => panic!("expected a refused cable, got {other:?}"),
    }
}

#[test]
fn a_loop_through_a_feedback_delay_is_accepted_and_any_other_loop_is_refused() {
    let reg = registry();
    let mut d = rack(&reg);
    // The same loop without the delay stage is a loop.
    let why = refused(&mut d, &reg, connect("gain:out_l", "gain:aux_l"));
    assert!(why.contains("cycle"), "{why}");
    // Through the feedback route it is not.
    for (f, t) in [
        ("gain:out_l", "fb:in_l"),
        ("fb:out_l", "gain:aux_l"),
        ("gain:out_r", "fb:in_r"),
        ("fb:out_r", "gain:aux_r"),
    ] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    assert!(d.doc().validate().is_ok());
    // Another loop that does not run through it is still refused, with the feedback cabled.
    refused(&mut d, &reg, connect("mixer:master_l", "gain:in_l"));
    // An undo takes a feedback cable out like any other.
    assert!(d.undo().is_some());
    assert!(d.doc().validate().is_ok());
}

#[test]
fn the_outputs_of_a_feedback_delay_may_feed_its_own_inputs() {
    let reg = registry();
    let mut d = rack(&reg);
    d.apply(connect("gain:out_l", "fb:in_l"), &reg).unwrap();
    d.apply(connect("fb:out_l", "fb:in_r"), &reg).unwrap();
    d.apply(connect("fb:out_r", "gain:aux_l"), &reg).unwrap();
    assert!(d.doc().validate().is_ok());
    // A loop that closes without any stage boundary in it is still a loop: two gain tools.
    d.apply(add(5, GAIN, Some("other")), &reg).unwrap();
    d.apply(connect("gain:out_r", "other:in_l"), &reg).unwrap();
    refused(&mut d, &reg, connect("other:out_l", "gain:in_r"));
}

#[test]
fn a_chain_of_feedback_delays_in_one_loop_is_accepted() {
    let reg = registry();
    let mut d = rack(&reg);
    d.apply(add(5, FEEDBACK, Some("fb2")), &reg).unwrap();
    for (f, t) in [
        ("gain:out_l", "fb:in_l"),
        ("fb:out_l", "fb2:in_l"),
        ("fb2:out_l", "gain:aux_l"),
    ] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    assert!(d.doc().validate().is_ok());
}

#[test]
fn the_check_after_flattening_containers_judges_feedback_the_same_way() {
    let reg = registry();
    let mut d = rack(&reg);
    // A container in the rack makes every cable go through the flattened-graph check.
    d.apply(add(5, "atomik.container", Some("box")), &reg)
        .unwrap();
    assert!(d.doc().has_containers());
    refused(&mut d, &reg, connect("gain:out_l", "gain:aux_l"));
    d.apply(connect("gain:out_l", "fb:in_l"), &reg).unwrap();
    d.apply(connect("fb:out_l", "gain:aux_l"), &reg).unwrap();
    assert!(d.doc().validate().is_ok());
}

#[test]
fn a_project_with_a_feedback_loop_saves_and_loads_and_a_file_with_a_bare_loop_does_not() {
    let reg = registry();
    let mut d = rack(&reg);
    for (f, t) in [("gain:out_l", "fb:in_l"), ("fb:out_l", "gain:aux_l")] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).expect("a loop through the delay loads");
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "and saves the same text");

    // The delay's type swapped for a gain tool (same socket names): now the loop has no stage
    // boundary in it and the reader refuses the file.
    let bare = text.replace(FEEDBACK, GAIN);
    assert_ne!(bare, text);
    match load_project(&bare, &reg) {
        Err(e) => assert!(e.to_string().contains("cycle"), "{e}"),
        Ok(_) => panic!("a loop without a delay stage must not load"),
    }
}

#[test]
fn a_loop_inside_a_container_follows_the_same_rule() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(add(3, "atomik.container", Some("box")), &reg)
        .unwrap();
    d.apply(add_inner("box", 0, GAIN, "gain"), &reg).unwrap();
    d.apply(add_inner("box", 1, FEEDBACK, "fb"), &reg).unwrap();
    refused(&mut d, &reg, connect("box/gain:out_l", "box/gain:aux_l"));
    d.apply(connect("box/gain:out_l", "box/fb:in_l"), &reg)
        .unwrap();
    d.apply(connect("box/fb:out_l", "box/gain:aux_l"), &reg)
        .unwrap();
    assert!(d.doc().validate().is_ok());
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).expect("a loop inside a container loads");
    assert_eq!(save_project(&loaded), text);
}
