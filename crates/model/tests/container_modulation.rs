//! Container boundary CV, sixteen macros with switches, and per-split pass flags: the expansion
//! theorem (a container expands to the flat graph one would have patched by hand), the format
//! (written only when set, so an untouched project saves as before) and the rules.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, Cable, Command, ContainerCommand, Curve, Document, MacroKind, NoteSplit,
    RackDoc, SplitPass, empty_doc, expand_macro_lane, load_project, save_project,
};
use common::*;
use proptest::prelude::*;
use serde_json::Value;

const MOD: &str = "atomik.mod-source";

fn reg() -> Registry {
    common::registry()
}

fn doc_of(r: &Registry, commands: impl IntoIterator<Item = Command>) -> Document {
    let mut d = Document::new(empty_doc());
    run(&mut d, r, commands);
    d
}

/// The flat cable list as text, sorted.
fn cables(doc: &RackDoc, r: &Registry) -> Vec<String> {
    doc.expand(r)
        .cables
        .iter()
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect()
}

/// A mod source and a synth, the source's LFO cabled to the synth's cutoff CV: flat.
fn flat_twin(r: &Registry) -> RackDoc {
    doc_of(
        r,
        [
            add(0, MOD, Some("lfo")),
            add(1, SYNTH, Some("synth")),
            connect("lfo:lfo1_cv_out", "synth:cutoff_cv_in"),
        ],
    )
    .doc()
    .clone()
}

/// The same patch with the synth inside a container: `lfo -> box:cv1`, `boundary:cv1 -> synth`.
fn container_cv(r: &Registry) -> Document {
    doc_of(
        r,
        [
            add(0, MOD, Some("lfo")),
            add(1, CONTAINER, Some("box")),
            add_inner("box", 0, SYNTH, "synth"),
            connect("box/boundary:cv1", "box/synth:cutoff_cv_in"),
            connect("lfo:lfo1_cv_out", "box:cv1"),
        ],
    )
}

fn rename_paths(text: &str) -> String {
    text.replace("box/synth", "synth")
}

#[test]
fn a_cv_cable_to_the_container_is_the_flat_cable_of_the_twin() {
    let r = reg();
    let d = container_cv(&r);
    let flat = d.doc().expand(&r);
    let twin = flat_twin(&r).expand(&r);
    let have: Vec<String> = flat
        .cables
        .iter()
        .map(|c| rename_paths(&format!("{} -> {}", c.from, c.to)))
        .collect();
    let want: Vec<String> = twin
        .cables
        .iter()
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect();
    assert_eq!(have, want);
    assert_eq!(have, ["lfo:lfo1_cv_out -> synth:cutoff_cv_in"]);
}

#[test]
fn one_cv_input_fans_out_to_every_inner_target() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, MOD, Some("lfo")),
            add(1, CONTAINER, Some("box")),
            add_inner("box", 0, SYNTH, "a"),
            add_inner("box", 1, SYNTH, "b"),
            connect("box/boundary:cv2", "box/a:cutoff_cv_in"),
            connect("box/boundary:cv2", "box/b:pitch_cv_in"),
            connect("lfo:env_cv_out", "box:cv2"),
        ],
    );
    assert_eq!(
        cables(d.doc(), &r),
        [
            "lfo:env_cv_out -> box/a:cutoff_cv_in",
            "lfo:env_cv_out -> box/b:pitch_cv_in"
        ]
    );
}

#[test]
fn cv_passes_through_nested_containers() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, MOD, Some("lfo")),
            add(1, CONTAINER, Some("outer")),
            add_inner("outer", 0, CONTAINER, "inner"),
            add_inner("outer/inner", 0, SYNTH, "synth"),
            connect(
                "outer/inner/boundary:cv3",
                "outer/inner/synth:resonance_cv_in",
            ),
            connect("outer/boundary:cv4", "outer/inner:cv3"),
            connect("lfo:lfo2_cv_out", "outer:cv4"),
        ],
    );
    assert_eq!(
        cables(d.doc(), &r),
        ["lfo:lfo2_cv_out -> outer/inner/synth:resonance_cv_in"]
    );
}

#[test]
fn an_uncabled_boundary_cv_is_a_silent_edge() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("box")),
            add_inner("box", 0, SYNTH, "synth"),
            connect("box/boundary:cv1", "box/synth:cutoff_cv_in"),
        ],
    );
    assert!(cables(d.doc(), &r).is_empty());
}

#[test]
fn a_bypassed_container_has_no_cv_cable() {
    let r = reg();
    let mut d = container_cv(&r);
    run(&mut d, &r, [set_param("box", "bypass", 1.0)]);
    assert!(cables(d.doc(), &r).is_empty());
}

#[test]
fn the_boundary_cv_sockets_follow_the_cable_rules() {
    let r = reg();
    let mut d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("box")),
            add_inner("box", 0, SYNTH, "synth"),
            add_inner("box", 1, DELAY, "delay"),
        ],
    );
    let refuses = |d: &mut Document, from: &str, to: &str| {
        let c = Command::Connect(Cable {
            from: sock(from),
            to: sock(to),
        });
        assert!(d.apply(c, &r).is_err(), "{from} -> {to} should be refused");
    };
    // CV into an audio input, audio into a CV output, an input as a source.
    refuses(&mut d, "box/boundary:cv1", "box/delay:in_l");
    refuses(&mut d, "box/synth:out_l", "box/boundary:cv1");
    refuses(&mut d, "box/boundary:out_l", "box/synth:cutoff_cv_in");
    // There are four and only four.
    refuses(&mut d, "box/boundary:cv5", "box/synth:cutoff_cv_in");
    // One cable per CV input, on the container as well.
    run(
        &mut d,
        &r,
        [connect("box/boundary:cv1", "box/synth:cutoff_cv_in")],
    );
    refuses(&mut d, "box/boundary:cv2", "box/synth:cutoff_cv_in");
}

#[test]
fn container_cv_cables_round_trip_and_undo() {
    let r = reg();
    let mut d = container_cv(&r);
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text);
    // The outer cable and the inner one each undo as one step.
    d.undo().unwrap();
    assert!(cables(d.doc(), &r).is_empty());
    d.redo().unwrap();
    assert_eq!(cables(d.doc(), &r).len(), 1);
}

// ------------------------------------------------------------------------- macros

fn kind(container: &str, index: u8, kind: MacroKind) -> Command {
    Command::Container(ContainerCommand::SetMacroKind {
        container: key(container),
        index,
        kind,
    })
}

#[test]
fn there_are_sixteen_macros_and_the_last_eight_drive_params() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 15, target("delay", "mix", 0.0, 1.0)),
            set_param("c", "macro16", 0.25),
        ],
    );
    let flat = d.doc().expand(&r);
    assert_eq!(flat.devices[1].params["mix"], 0.25);
    assert_eq!(flat.macro_targets.len(), 1);
    assert_eq!(flat.macro_targets[0].1, 15);
    let mut bad = Document::new(empty_doc());
    run(&mut bad, &r, [add(0, CONTAINER, Some("c"))]);
    let past = Command::Container(ContainerCommand::SetMacro {
        container: key("c"),
        index: 16,
        target: target("delay", "mix", 0.0, 1.0),
        merge: None,
    });
    assert!(bad.apply(past, &r).is_err());
}

#[test]
fn a_param_is_still_driven_by_one_macro_across_all_sixteen() {
    let r = reg();
    let mut d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 2, target("delay", "mix", 0.0, 1.0)),
        ],
    );
    let again = set_macro("c", 12, target("delay", "mix", 0.0, 1.0));
    assert!(d.apply(again, &r).is_err());
}

#[test]
fn a_switch_macro_gives_min_below_half_and_max_from_half_up() {
    let r = reg();
    let mut d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 9, target("delay", "mix", 0.2, 0.9)),
            kind("c", 9, MacroKind::Switch),
        ],
    );
    let mix = |d: &Document| d.doc().expand(&r).devices[1].params["mix"];
    for (value, want) in [(0.0, 0.2), (0.49, 0.2), (0.5, 0.9), (1.0, 0.9)] {
        run(&mut d, &r, [set_param("c", "macro10", value)]);
        assert_eq!(mix(&d), want, "macro value {value}");
    }
    // A knob in the same place is linear again.
    run(&mut d, &r, [kind("c", 9, MacroKind::Knob)]);
    run(&mut d, &r, [set_param("c", "macro10", 0.5)]);
    assert_eq!(mix(&d), 0.2 + (0.9 - 0.2) * 0.5);
    // The kind is one undo step.
    d.undo().unwrap();
    assert_eq!(
        d.doc().inner(&key("c")).unwrap().macros[9].kind,
        MacroKind::Knob
    );
}

#[test]
fn switch_automation_maps_every_point_through_the_threshold() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
            kind("c", 0, MacroKind::Switch),
        ],
    );
    let point = |tick, value| AutomationPoint {
        tick,
        value,
        curve: Curve::Step,
    };
    let lanes = expand_macro_lane(
        d.doc(),
        &r,
        &key("c"),
        "macro1",
        &[point(0, 0.3), point(100, 0.7)],
    )
    .unwrap();
    let values: Vec<f32> = lanes[0].2.iter().map(|p| p.value).collect();
    assert_eq!(values, [0.0, 1.0]);
}

#[test]
fn macro_kinds_and_sixteen_macros_are_written_only_when_used() {
    let r = reg();
    // An untouched container writes eight macros and no kind, no pass flag.
    let d = doc_of(&r, [add(0, CONTAINER, Some("c"))]);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["devices"][0]["inner"]["macros"].as_array().unwrap().len(),
        8
    );
    assert!(!text.contains("\"kind\""));
    assert!(!text.contains("pass_"));
    // A switch among the first eight adds only its kind.
    let mut d = d;
    run(&mut d, &r, [kind("c", 3, MacroKind::Switch)]);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let macros = v["devices"][0]["inner"]["macros"].as_array().unwrap();
    assert_eq!(macros.len(), 8);
    assert_eq!(macros[3]["kind"], "switch");
    assert!(macros[2].get("kind").is_none());
    assert_eq!(load_project(&text, &r).unwrap(), *d.doc());
    // A used macro among the last eight writes all sixteen.
    run(
        &mut d,
        &r,
        [
            Command::Container(ContainerCommand::RenameMacro {
                container: key("c"),
                index: 11,
                name: "Mode".into(),
            }),
            kind("c", 14, MacroKind::Switch),
        ],
    );
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["devices"][0]["inner"]["macros"].as_array().unwrap().len(),
        16
    );
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_bad_macro_count_or_kind_is_a_load_error_with_a_path() {
    let r = reg();
    let d = doc_of(&r, [add(0, CONTAINER, Some("c"))]);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let mut few = v.clone();
    few["devices"][0]["inner"]["macros"]
        .as_array_mut()
        .unwrap()
        .pop();
    let e = load_project(&few.to_string(), &r).unwrap_err().to_string();
    assert!(e.contains("devices[0].inner.macros"), "{e}");
    assert!(e.contains("8 or 16"), "{e}");
    let mut odd = v;
    odd["devices"][0]["inner"]["macros"][1]["kind"] = "dial".into();
    let e = load_project(&odd.to_string(), &r).unwrap_err().to_string();
    assert!(e.contains("devices[0].inner.macros[1].kind"), "{e}");
}

// ------------------------------------------------------------------------- split flags

fn pass(bend: bool, cc: bool, pressure: bool) -> SplitPass {
    SplitPass { bend, cc, pressure }
}

#[test]
fn split_pass_flags_become_split_device_params_only_when_off() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "a"),
            add_inner("c", 1, SYNTH, "b"),
            set_splits(
                "c",
                vec![
                    NoteSplit {
                        pass: pass(false, true, false),
                        ..NoteSplit::full(key("a"))
                    },
                    NoteSplit::full(key("b")),
                ],
            ),
        ],
    );
    let flat = d.doc().expand(&r);
    let p = &flat.devices[0].params;
    assert_eq!(p["s1.pass_bend"], 0.0);
    assert_eq!(p["s1.pass_pressure"], 0.0);
    assert!(!p.contains_key("s1.pass_cc"));
    assert!(p.keys().all(|k| !k.starts_with("s2.pass_")));
}

#[test]
fn split_pass_flags_round_trip_and_default_to_passing() {
    let r = reg();
    let mut d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "a"),
            add_inner("c", 1, SYNTH, "b"),
            set_splits(
                "c",
                vec![
                    NoteSplit {
                        pass: pass(false, false, true),
                        ..NoteSplit::full(key("a"))
                    },
                    NoteSplit::full(key("b")),
                ],
            ),
        ],
    );
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let splits = &v["devices"][0]["inner"]["splits"];
    assert_eq!(splits[0]["pass_bend"], false);
    assert_eq!(splits[0]["pass_cc"], false);
    assert!(splits[0].get("pass_pressure").is_none());
    assert!(splits[1].get("pass_bend").is_none());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text);
    // One undo step restores what was there.
    run(
        &mut d,
        &r,
        [set_splits("c", vec![NoteSplit::full(key("a"))])],
    );
    d.undo().unwrap();
    assert_eq!(d.doc().inner(&key("c")).unwrap().splits.len(), 2);
}

#[test]
fn a_non_boolean_pass_flag_is_a_load_error_with_a_path() {
    let r = reg();
    let d = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "a"),
            set_splits("c", vec![NoteSplit::full(key("a"))]),
        ],
    );
    let mut v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    v["devices"][0]["inner"]["splits"][0]["pass_cc"] = "no".into();
    let e = load_project(&v.to_string(), &r).unwrap_err().to_string();
    assert!(e.contains("devices[0].inner.splits[0].pass_cc"), "{e}");
}

// ------------------------------------------------------------------------- property

/// Random CV patches: the container version expands to exactly the flat cables of the same
/// patch written without a container.
fn cv_inputs() -> [&'static str; 6] {
    [
        "cutoff_cv_in",
        "pitch_cv_in",
        "resonance_cv_in",
        "cutoff_cv_in",
        "pitch_cv_in",
        "resonance_cv_in",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_container_with_cv_expands_to_its_flat_twin(
        // For each of up to four synths: which of the four CV inputs feeds which of its
        // three CV sockets (or none).
        wiring in proptest::collection::vec(
            proptest::collection::vec(proptest::option::of(0u8..4), 3),
            1..4,
        ),
        sources in proptest::collection::vec(0u8..3, 4),
    ) {
        let r = reg();
        const OUTS: [&str; 3] = ["lfo1_cv_out", "lfo2_cv_out", "env_cv_out"];
        let ins = cv_inputs();
        let mut boxed = vec![add(0, MOD, Some("lfo")), add(1, CONTAINER, Some("box"))];
        let mut flat = vec![add(0, MOD, Some("lfo"))];
        let mut used = [false; 4];
        for (i, synth) in wiring.iter().enumerate() {
            let k = format!("s{i}");
            boxed.push(add_inner("box", i, SYNTH, &k));
            flat.push(add(i + 1, SYNTH, Some(&k)));
            for (j, cv) in synth.iter().enumerate() {
                if let Some(n) = cv {
                    let socket = ins[j];
                    boxed.push(connect(
                        &format!("box/boundary:cv{}", n + 1),
                        &format!("box/{k}:{socket}"),
                    ));
                    flat.push(connect(
                        &format!("lfo:{}", OUTS[usize::from(sources[usize::from(*n)])]),
                        &format!("{k}:{socket}"),
                    ));
                    used[usize::from(*n)] = true;
                }
            }
        }
        for (n, on) in used.iter().enumerate() {
            if *on {
                boxed.push(connect(
                    &format!("lfo:{}", OUTS[usize::from(sources[n])]),
                    &format!("box:cv{}", n + 1),
                ));
            }
        }
        let have = doc_of(&r, boxed);
        let want = doc_of(&r, flat);
        let strip = |s: String| s.replace("box/", "");
        let mut a: Vec<String> = cables(have.doc(), &r).into_iter().map(strip).collect();
        let mut b = cables(want.doc(), &r);
        a.sort();
        b.sort();
        prop_assert_eq!(a, b);
        // The devices match too, in order, apart from the container's split.
        let keys = |doc: &RackDoc| -> Vec<String> {
            doc.expand(&r)
                .devices
                .iter()
                .filter(|d| d.type_id != SPLIT)
                .map(|d| d.key.0.replace("box/", ""))
                .collect()
        };
        prop_assert_eq!(keys(have.doc()), keys(want.doc()));
        // And the whole thing saves and loads.
        let text = save_project(have.doc());
        prop_assert_eq!(&load_project(&text, &r).unwrap(), have.doc());
    }
}
