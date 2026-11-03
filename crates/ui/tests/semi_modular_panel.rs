//! The semi-modular synth's rack panel is the generated one: every param but the matrix slots is a
//! control placed once, the sections follow the slots and the sequencer rows, the panel fits in
//! the rack and the matrix is reachable through its panel.

mod support;

use atomik_device_sdk::is_mod_slot_key;
use atomik_ui::autolayout::{MAX_UNITS, auto_layout, group_prefix, section_label};
use support::*;

#[test]
fn every_param_but_the_matrix_slots_is_placed_once() {
    let reg = registry();
    let d = reg.get("atomik.semi-modular").unwrap().descriptor;
    let spec = auto_layout(d);
    assert!(spec.height_units >= 1 && spec.height_units <= 14.min(MAX_UNITS));
    let mut placed: Vec<usize> = spec.front.iter().map(|c| c.param).collect();
    placed.sort_unstable();
    let want: Vec<usize> = (0..d.params.len())
        .filter(|i| !is_mod_slot_key(d.params[*i].key))
        .collect();
    assert_eq!(placed, want);
    // The back panel has every socket.
    assert_eq!(spec.back.len(), d.sockets.len());
}

#[test]
fn the_sections_follow_the_slots_and_the_sequencer_rows() {
    let reg = registry();
    let d = reg.get("atomik.semi-modular").unwrap().descriptor;
    let labels: Vec<String> = spec_labels(d);
    for want in [
        "OSC 1",
        "OSC 3",
        "FILTER 2",
        "SHAPER",
        "ENV AMP",
        "LFO 1",
        "SEQ",
        "SEQ ON",
        "SEQ PITCH",
        "SEQ VEL",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
}

fn spec_labels(d: &atomik_device_sdk::DeviceDescriptor) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in d.params {
        if is_mod_slot_key(p.key) {
            continue;
        }
        let label = section_label(group_prefix(p.key));
        if !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

#[test]
fn the_device_has_a_modulation_matrix() {
    let reg = registry();
    assert!(reg.mod_tables("atomik.semi-modular").is_some());
    assert_eq!(
        reg.mod_tables("atomik.semi-modular").unwrap().targets.len(),
        15
    );
}
