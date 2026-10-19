//! Expansion of containers into the flat graph the engine understands: path keys, macro mapping and automation helpers.

mod common;

use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, Cable, Command, Curve, DeviceKey, Document, ExpandedRack, RackDoc, empty_doc,
    expand_macro_lane, macro_value,
};
use common::*;
use std::collections::BTreeMap;

fn reg() -> Registry {
    common::console_registry()
}

fn doc_of(r: &Registry, commands: impl IntoIterator<Item = Command>) -> RackDoc {
    let mut d = Document::new(empty_doc());
    run(&mut d, r, commands);
    d.doc().clone()
}

fn flat_keys(x: &ExpandedRack) -> Vec<String> {
    x.devices.iter().map(|d| d.key.0.clone()).collect()
}

fn flat_cables(x: &ExpandedRack) -> Vec<String> {
    x.cables
        .iter()
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// `container` with `boundary -> fx -> boundary` for the left channel.
fn effect(c: &str, fx: &str) -> Vec<Command> {
    vec![
        add_inner(c, 0, DELAY, fx),
        connect(&format!("{c}/boundary:in_l"), &format!("{c}/{fx}:in_l")),
        connect(&format!("{c}/{fx}:out_l"), &format!("{c}/boundary:out_l")),
    ]
}

#[test]
fn a_document_without_containers_expands_to_itself() {
    let r = reg();
    let mut d = chain(&common::registry());
    // `chain` uses the plain registry; add a console on top with the console registry.
    let doc = {
        let mut x = Document::new(empty_doc());
        run(
            &mut x,
            &r,
            [
                add(0, DELAY, Some("s")),
                add(1, DELAY, Some("d")),
                connect("s:out_l", "d:in_l"),
                set_param("d", "mix", 0.3),
            ],
        );
        x.doc().clone()
    };
    let _ = &mut d;
    let flat = doc.expand(&r);
    assert_eq!(
        flat_keys(&flat),
        doc.devices
            .iter()
            .map(|d| d.key.0.clone())
            .collect::<Vec<_>>()
    );
    for (f, d) in flat.devices.iter().zip(&doc.devices) {
        assert_eq!(f.type_id, d.type_id);
        assert_eq!(f.params, d.params);
        assert_eq!(f.zones, d.zones);
        assert_eq!(f.steps, d.steps);
    }
    assert_eq!(flat.cables, doc.cables);
    assert!(flat.alias.is_empty() && flat.macro_targets.is_empty());
    // The console devices of a real project too.
    let mut x = Document::new(empty_doc());
    run(&mut x, &r, [add(0, DELAY, Some("s"))]);
    let doc = x.doc().clone();
    let flat = doc.expand(&r);
    assert_eq!(flat_keys(&flat).len(), doc.devices.len());
    assert_eq!(flat.cables, doc.cables);
}

#[test]
fn an_effect_container_is_spliced_into_the_outer_cables() {
    let r = reg();
    let mut cmds = vec![
        add(0, DELAY, Some("s")),
        add(1, CONTAINER, Some("c")),
        add(2, MASTER, Some("master")),
    ];
    cmds.extend(effect("c", "fx"));
    cmds.extend([
        connect("s:out_l", "c:in_l"),
        connect("c:out_l", "master:bus_l"),
    ]);
    let doc = doc_of(&r, cmds);
    let flat = doc.expand(&r);
    // The container itself vanishes: its split and its inside take its place.
    assert_eq!(flat_keys(&flat), ["s", "c/~split", "c/fx", "master"]);
    assert_eq!(
        flat_cables(&flat),
        ["c/fx:out_l -> master:bus_l", "s:out_l -> c/fx:in_l",]
    );
    assert_eq!(flat.devices[1].type_id, SPLIT);
    assert_eq!(flat.devices[2].type_id, DELAY);
    assert_eq!(flat.alias[&key("c")], key("c/~split"));
}

#[test]
fn fan_out_inside_gives_one_cable_per_inner_target() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "a"),
            add_inner("c", 1, DELAY, "b"),
            connect("c/boundary:in_l", "c/a:in_l"),
            connect("c/boundary:in_l", "c/b:in_l"),
            connect("s:out_l", "c:in_l"),
        ],
    );
    assert_eq!(
        flat_cables(&doc.expand(&r)),
        ["s:out_l -> c/a:in_l", "s:out_l -> c/b:in_l"]
    );
}

#[test]
fn fan_out_outside_gives_one_cable_per_outer_target() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add(1, DELAY, Some("x")),
            add(2, DELAY, Some("y")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/fx:out_l", "c/boundary:out_l"),
            connect("c:out_l", "x:in_l"),
            connect("c:out_l", "y:in_l"),
        ],
    );
    assert_eq!(
        flat_cables(&doc.expand(&r)),
        ["c/fx:out_l -> x:in_l", "c/fx:out_l -> y:in_l"]
    );
}

#[test]
fn boundary_to_boundary_passes_through() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c")),
            add(2, MASTER, Some("master")),
            connect("c/boundary:in_l", "c/boundary:out_l"),
            connect("s:out_l", "c:in_l"),
            connect("c:out_l", "master:bus_l"),
        ],
    );
    assert_eq!(flat_cables(&doc.expand(&r)), ["s:out_l -> master:bus_l"]);
}

#[test]
fn a_one_sided_boundary_is_a_silent_edge_not_an_error() {
    let r = reg();
    // Nothing outside feeds the container, nothing outside takes its output.
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/fx:out_l", "c/boundary:out_l"),
        ],
    );
    let flat = doc.expand(&r);
    assert_eq!(flat_keys(&flat), ["c/~split", "c/fx"]);
    assert!(flat.cables.is_empty());
    // Fed but unread, and read but unfed.
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c")),
            add(2, DELAY, Some("d")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/fx:out_l", "c/boundary:out_l"),
            connect("s:out_l", "c:in_l"),
            connect("c:out_r", "d:in_r"),
        ],
    );
    assert_eq!(flat_cables(&doc.expand(&r)), ["s:out_l -> c/fx:in_l"]);
}

#[test]
fn summing_inputs_take_one_cable_per_container() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s1")),
            add(1, DELAY, Some("s2")),
            add(2, CONTAINER, Some("a")),
            add(3, CONTAINER, Some("b")),
            add(4, MASTER, Some("master")),
            add_inner("a", 0, DELAY, "fx"),
            add_inner("b", 0, DELAY, "fx"),
            connect("a/boundary:in_l", "a/fx:in_l"),
            connect("a/fx:out_l", "a/boundary:out_l"),
            connect("b/boundary:in_l", "b/fx:in_l"),
            connect("b/fx:out_l", "b/boundary:out_l"),
            connect("s1:out_l", "a:in_l"),
            connect("s2:out_l", "b:in_l"),
            connect("a:out_l", "master:bus_l"),
            connect("b:out_l", "master:bus_l"),
            connect("s1:out_r", "master:bus_r"),
        ],
    );
    assert_eq!(
        sorted(flat_cables(&doc.expand(&r))),
        [
            "a/fx:out_l -> master:bus_l",
            "b/fx:out_l -> master:bus_l",
            "s1:out_l -> a/fx:in_l",
            "s1:out_r -> master:bus_r",
            "s2:out_l -> b/fx:in_l",
        ]
    );
}

#[test]
fn a_notes_cable_to_a_container_goes_through_its_split() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, SEQ, Some("seq")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "lo"),
            add_inner("c", 1, SYNTH, "hi"),
            set_splits("c", vec![split("lo", 0, 59, 0), split("hi", 60, 127, 12)]),
            connect("seq:notes_out", "c:notes_in"),
        ],
    );
    let flat = doc.expand(&r);
    assert_eq!(flat_keys(&flat), ["seq", "c/~split", "c/lo", "c/hi"]);
    assert_eq!(
        sorted(flat_cables(&flat)),
        [
            "c/~split:out_1 -> c/lo:notes_in",
            "c/~split:out_2 -> c/hi:notes_in",
            "seq:notes_out -> c/~split:notes_in",
        ]
    );
    // Without a notes cable the splits still route to their targets, and nothing arrives.
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "lo"),
            set_splits("c", vec![split("lo", 0, 127, 0)]),
        ],
    );
    assert_eq!(
        flat_cables(&doc.expand(&r)),
        ["c/~split:out_1 -> c/lo:notes_in"]
    );
}

#[test]
fn inner_note_devices_may_feed_instruments_and_be_split_targets() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SEQ, "seq"),
            add_inner("c", 1, SYNTH, "synth"),
            connect("c/seq:notes_out", "c/synth:notes_in"),
            set_splits("c", vec![split("seq", 0, 127, 0)]),
        ],
    );
    assert_eq!(
        sorted(flat_cables(&doc.expand(&r))),
        [
            "c/seq:notes_out -> c/synth:notes_in",
            "c/~split:out_1 -> c/seq:notes_in",
        ]
    );
}

#[test]
fn nested_containers_flatten_depth_first() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c")),
            add(2, MASTER, Some("master")),
            add_inner("c", 0, DELAY, "a"),
            add_inner("c", 1, CONTAINER, "n"),
            add_inner("c", 2, DELAY, "b"),
            add_inner("c/n", 0, DELAY, "z"),
            // outside -> a -> n -> b -> outside
            connect("c/boundary:in_l", "c/a:in_l"),
            connect("c/a:out_l", "c/n:in_l"),
            connect("c/n/boundary:in_l", "c/n/z:in_l"),
            connect("c/n/z:out_l", "c/n/boundary:out_l"),
            connect("c/n:out_l", "c/b:in_l"),
            connect("c/b:out_l", "c/boundary:out_l"),
            connect("s:out_l", "c:in_l"),
            connect("c:out_l", "master:bus_l"),
        ],
    );
    let flat = doc.expand(&r);
    assert_eq!(
        flat_keys(&flat),
        [
            "s",
            "c/~split",
            "c/a",
            "c/n/~split",
            "c/n/z",
            "c/b",
            "master"
        ]
    );
    assert_eq!(
        sorted(flat_cables(&flat)),
        [
            "c/a:out_l -> c/n/z:in_l",
            "c/b:out_l -> master:bus_l",
            "c/n/z:out_l -> c/b:in_l",
            "s:out_l -> c/a:in_l",
        ]
    );
    assert_eq!(flat.alias[&key("c")], key("c/~split"));
    assert_eq!(flat.alias[&key("c/n")], key("c/n/~split"));
}

#[test]
fn nested_pass_through_and_notes_chain() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, SEQ, Some("seq")),
            add(1, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, SYNTH, "synth"),
            set_splits("o/i", vec![split("synth", 0, 127, 0)]),
            set_splits("o", vec![split("i", 0, 127, 0)]),
            connect("seq:notes_out", "o:notes_in"),
        ],
    );
    assert_eq!(
        sorted(flat_cables(&doc.expand(&r))),
        [
            "o/i/~split:out_1 -> o/i/synth:notes_in",
            "o/~split:out_1 -> o/i/~split:notes_in",
            "seq:notes_out -> o/~split:notes_in",
        ]
    );
}

#[test]
fn two_containers_in_series() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c1")),
            add(2, CONTAINER, Some("c2")),
            add(3, MASTER, Some("master")),
            add_inner("c1", 0, DELAY, "d1"),
            add_inner("c2", 0, DELAY, "d2"),
            connect("c1/boundary:in_l", "c1/d1:in_l"),
            connect("c1/d1:out_l", "c1/boundary:out_l"),
            connect("c2/boundary:in_l", "c2/d2:in_l"),
            connect("c2/d2:out_l", "c2/boundary:out_l"),
            connect("s:out_l", "c1:in_l"),
            connect("c1:out_l", "c2:in_l"),
            connect("c2:out_l", "master:bus_l"),
        ],
    );
    let flat = doc.expand(&r);
    assert_eq!(
        flat_keys(&flat),
        ["s", "c1/~split", "c1/d1", "c2/~split", "c2/d2", "master"]
    );
    assert_eq!(
        sorted(flat_cables(&flat)),
        [
            "c1/d1:out_l -> c2/d2:in_l",
            "c2/d2:out_l -> master:bus_l",
            "s:out_l -> c1/d1:in_l",
        ]
    );
}

#[test]
fn cables_to_the_interface_are_spliced_too() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "s"),
            connect("c/s:out_l", "c/boundary:out_l"),
            connect("c:out_l", "interface:out_1"),
            connect("interface:in_1", "c:in_l"),
            connect("c/boundary:in_l", "c/boundary:out_r"),
            connect("c:out_r", "interface:out_2"),
        ],
    );
    assert_eq!(
        sorted(flat_cables(&doc.expand(&r))),
        [
            "c/s:out_l -> interface:out_1",
            "interface:in_1 -> interface:out_2"
        ]
    );
}

#[test]
fn the_expanded_order_is_rack_order_then_split_then_inner_depth_first() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("first")),
            add(1, CONTAINER, Some("c1")),
            add(2, DELAY, Some("mid")),
            add(3, CONTAINER, Some("c2")),
            add(4, DELAY, Some("last")),
            add_inner("c1", 0, SYNTH, "x"),
            add_inner("c1", 1, SYNTH, "y"),
            add_inner("c2", 0, CONTAINER, "n"),
            add_inner("c2", 1, SYNTH, "w"),
            add_inner("c2/n", 0, SYNTH, "v"),
        ],
    );
    assert_eq!(
        flat_keys(&doc.expand(&r)),
        [
            "first",
            "c1/~split",
            "c1/x",
            "c1/y",
            "mid",
            "c2/~split",
            "c2/n/~split",
            "c2/n/v",
            "c2/w",
            "last"
        ]
    );
    // Moving a device inside changes only that order.
    let mut d = Document::new(doc);
    d.apply(
        Command::MoveDevice {
            key: key("c1/y"),
            to: 0,
        },
        &r,
    )
    .unwrap();
    let keys = flat_keys(&d.doc().expand(&r));
    assert_eq!(&keys[..4], ["first", "c1/~split", "c1/y", "c1/x"]);
}

#[test]
fn the_split_device_gets_its_params_from_the_splits() {
    let r = reg();
    let mut s1 = split("lo", 0, 59, -12);
    s1.vel_lo = 20;
    s1.vel_hi = 100;
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "lo"),
            add_inner("c", 1, SYNTH, "hi"),
            set_splits("c", vec![s1, split("hi", 60, 127, 48)]),
        ],
    );
    let flat = doc.expand(&r);
    let p = &flat.devices[0].params;
    assert_eq!(flat.devices[0].key, key("c/~split"));
    let expect: BTreeMap<String, f32> = [
        ("s1.on", 1.0),
        ("s1.key_lo", 0.0),
        ("s1.key_hi", 59.0),
        ("s1.vel_lo", 20.0),
        ("s1.vel_hi", 100.0),
        ("s1.transpose", -12.0),
        ("s2.on", 1.0),
        ("s2.key_lo", 60.0),
        ("s2.key_hi", 127.0),
        ("s2.vel_lo", 1.0),
        ("s2.vel_hi", 127.0),
        ("s2.transpose", 48.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(p, &expect);
    // Every param is a real param of the split device.
    let desc = r.get(SPLIT).unwrap().descriptor;
    assert!(p.keys().all(|k| desc.param_by_key(k).is_some()));
    // No splits: all slots stay off (the default).
    let doc = doc_of(&r, [add(0, CONTAINER, Some("c"))]);
    assert!(doc.expand(&r).devices[0].params.is_empty());
}

#[test]
fn expansion_is_pure_and_edits_change_only_what_they_touch() {
    let r = reg();
    let mut cmds = vec![
        add(0, DELAY, Some("s")),
        add(1, CONTAINER, Some("c")),
        add(2, MASTER, Some("master")),
    ];
    cmds.extend(effect("c", "fx"));
    cmds.extend([connect("s:out_l", "c:in_l")]);
    let mut d = Document::new(doc_of(&r, cmds));
    let a = d.doc().expand(&r);
    assert_eq!(a, d.doc().expand(&r));
    d.apply(set_param("c/fx", "mix", 0.77), &r).unwrap();
    let b = d.doc().expand(&r);
    assert_eq!(a.cables, b.cables);
    assert_eq!(flat_keys(&a), flat_keys(&b));
    let changed: Vec<_> = a
        .devices
        .iter()
        .zip(&b.devices)
        .filter(|(x, y)| x != y)
        .map(|(x, _)| x.key.0.clone())
        .collect();
    assert_eq!(changed, ["c/fx"]);
    assert_eq!(b.devices[2].params["mix"], 0.77);
}

#[test]
fn inner_zones_and_steps_travel_with_the_flat_device() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SEQ, "q"),
            add_inner("c", 1, SAMPLER, "sm"),
        ],
    );
    let flat = doc.expand(&r);
    let q = flat.devices.iter().find(|d| d.key == key("c/q")).unwrap();
    assert!(q.steps.is_some());
    let sm = flat.devices.iter().find(|d| d.key == key("c/sm")).unwrap();
    assert!(sm.zones.is_empty());
}

// ---------------------------------------------------------------- macros

fn p(min: f32, max: f32, default: f32, discrete: bool, unit: Unit) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(0),
        key: "x",
        name: "x",
        min,
        max,
        default,
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

#[test]
fn macro_value_maps_linearly_inverts_clamps_and_rounds() {
    let c = p(0.0, 100.0, 50.0, false, Unit::None);
    assert_eq!(macro_value(0.0, 100.0, 0.0, &c), 0.0);
    assert_eq!(macro_value(0.0, 100.0, 1.0, &c), 100.0);
    assert_eq!(macro_value(0.0, 100.0, 0.25, &c), 25.0);
    // Inverted: min > max.
    assert_eq!(macro_value(100.0, 0.0, 0.25, &c), 75.0);
    assert_eq!(macro_value(100.0, 0.0, 0.0, &c), 100.0);
    // Clamped to the descriptor range.
    assert_eq!(macro_value(-50.0, 500.0, 0.0, &c), 0.0);
    assert_eq!(macro_value(-50.0, 500.0, 1.0, &c), 100.0);
    // Discrete and choice rounding: nearest, ties up.
    let d = p(0.0, 4.0, 0.0, true, Unit::None);
    assert_eq!(macro_value(0.0, 4.0, 0.3, &d), 1.0);
    assert_eq!(macro_value(0.0, 4.0, 0.125, &d), 1.0, "0.5 rounds up");
    assert_eq!(macro_value(0.0, 4.0, 0.1, &d), 0.0);
    assert_eq!(macro_value(4.0, 0.0, 0.1, &d), 4.0);
    let ch = p(0.0, 2.0, 0.0, true, Unit::Choice(&["a", "b", "c"]));
    assert_eq!(macro_value(0.0, 2.0, 0.5, &ch), 1.0);
    assert_eq!(macro_value(0.0, 2.0, 0.74, &ch), 1.0);
    assert_eq!(macro_value(0.0, 2.0, 0.76, &ch), 2.0);
    // Negative ranges round the same way.
    let n = p(-48.0, 48.0, 0.0, true, Unit::Semitones);
    assert_eq!(macro_value(-48.0, 48.0, 0.5, &n), 0.0);
    assert_eq!(macro_value(-48.0, 48.0, 0.5 + 0.7 / 96.0, &n), 1.0);
    // NaN macro value: the descriptor default.
    assert_eq!(macro_value(0.0, 1.0, f32::NAN, &c), 50.0);
}

#[test]
fn macro_value_uses_only_plus_minus_times() {
    // Bit-exact against the plain formula, for awkward values.
    let c = p(-1000.0, 1000.0, 0.0, false, Unit::None);
    for &(lo, hi, m) in &[
        (0.1f32, 0.7f32, 0.3f32),
        (-3.3, 9.9, 0.123_456_7),
        (5.0, -5.0, 0.9),
    ] {
        let want = (lo + (hi - lo) * m).clamp(-1000.0, 1000.0);
        assert_eq!(macro_value(lo, hi, m, &c).to_bits(), want.to_bits());
    }
}

#[test]
fn macros_are_applied_to_the_flat_devices() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, DELAY, "delay"),
            set_macro("c", 0, target("synth", "filter.cutoff", 200.0, 8000.0)),
            set_macro("c", 0, target("synth", "filter.mode", 0.0, 2.0)),
            set_macro("c", 0, target("delay", "mix", 1.0, 0.0)),
            set_param("c", "macro1", 0.5),
        ],
    );
    let flat = doc.expand(&r);
    let get = |k: &str, p: &str| {
        flat.devices
            .iter()
            .find(|d| d.key == key(k))
            .unwrap()
            .params[p]
    };
    assert_eq!(
        get("c/synth", "filter.cutoff"),
        200.0 + (8000.0 - 200.0) * 0.5
    );
    assert_eq!(get("c/synth", "filter.mode"), 1.0);
    assert_eq!(get("c/delay", "mix"), 0.5);
    // The list of targets for the app.
    assert_eq!(flat.macro_targets.len(), 1);
    let (container, index, targets) = &flat.macro_targets[0];
    assert_eq!((container, *index), (&key("c"), 0));
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].device, key("c/synth"));
    assert_eq!(targets[0].param, "filter.cutoff");
    assert_eq!((targets[0].min, targets[0].max), (200.0, 8000.0));
    assert_eq!(targets[0].value, 4100.0);
    // A mapped param ignores its stored value; with m = 0 the target sits at `min`.
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_param("c/delay", "mix", 0.9),
            set_macro("c", 3, target("delay", "mix", 0.25, 0.75)),
        ],
    );
    let flat = doc.expand(&r);
    assert_eq!(flat.devices[1].params["mix"], 0.25);
    assert_eq!(
        doc.resolve(&key("c/delay")).unwrap().params["mix"],
        0.9,
        "kept"
    );
}

#[test]
fn nested_macros_evaluate_outermost_first() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
            // outer macro 2 drives inner macro 1 over 0.2..0.6, the inner one drives the mix
            // over 0..1.
            set_macro("o", 1, target("i", "macro1", 0.2, 0.6)),
            set_macro("o/i", 0, target("fx", "mix", 0.0, 1.0)),
            set_param("o", "macro2", 0.5),
        ],
    );
    let flat = doc.expand(&r);
    let mix = flat
        .devices
        .iter()
        .find(|d| d.key == key("o/i/fx"))
        .unwrap()
        .params["mix"];
    let inner_macro = 0.2 + (0.6 - 0.2) * 0.5;
    assert_eq!(mix, 0.0 + (1.0 - 0.0) * inner_macro);
    // The expanded macro list has both stages, outer first.
    let order: Vec<_> = flat
        .macro_targets
        .iter()
        .map(|(c, i, _)| (c.0.clone(), *i))
        .collect();
    assert_eq!(order, [("o".to_owned(), 1), ("o/i".to_owned(), 0)]);
}

#[test]
fn macro_updates_lists_one_set_param_per_engine_target() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
            add_inner("o", 1, DELAY, "d"),
            set_macro("o", 0, target("i", "macro1", 0.0, 1.0)),
            set_macro("o", 0, target("d", "mix", 0.0, 1.0)),
            set_macro("o/i", 0, target("fx", "mix", 0.0, 1.0)),
            set_param("o", "macro1", 0.5),
        ],
    );
    let flat = doc.expand(&r);
    let updates = flat.macro_updates(&key("o"));
    // Container macro params are not engine devices: only `d` and `fx` get a set_param.
    assert_eq!(
        updates,
        [
            (key("o/d"), "mix".to_owned(), 0.5),
            (key("o/i/fx"), "mix".to_owned(), 0.5)
        ]
    );
    // Only the inner container's own targets.
    assert_eq!(
        flat.macro_updates(&key("o/i")),
        [(key("o/i/fx"), "mix".to_owned(), 0.5)]
    );
}

#[test]
fn macro_automation_expands_to_one_lane_per_target() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, DELAY, "delay"),
            set_macro("c", 0, target("synth", "filter.cutoff", 200.0, 8000.0)),
            set_macro("c", 0, target("synth", "filter.mode", 2.0, 0.0)),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
        ],
    );
    let points = vec![
        AutomationPoint {
            tick: 0,
            value: 0.0,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 100,
            value: 0.3,
            curve: Curve::Step,
        },
        AutomationPoint {
            tick: 200,
            value: 1.0,
            curve: Curve::Linear,
        },
    ];
    let lanes = expand_macro_lane(&doc, &r, &key("c"), "macro1", &points).unwrap();
    assert_eq!(lanes.len(), 3);
    let (dev, param, pts) = &lanes[0];
    assert_eq!((dev, param.as_str()), (&key("c/synth"), "filter.cutoff"));
    assert_eq!(
        pts.iter().map(|p| p.tick).collect::<Vec<_>>(),
        [0, 100, 200]
    );
    assert_eq!(pts[0].value, 200.0);
    assert_eq!(pts[1].value, 200.0 + (8000.0 - 200.0) * 0.3);
    assert_eq!(pts[2].value, 8000.0);
    assert_eq!(pts[1].curve, Curve::Step);
    // The discrete target rounds each point.
    let (dev, param, pts) = &lanes[1];
    assert_eq!((dev, param.as_str()), (&key("c/synth"), "filter.mode"));
    assert_eq!(
        pts.iter().map(|p| p.value).collect::<Vec<_>>(),
        [2.0, 1.0, 0.0]
    );
    // Anything that is not a macro lane of a container gives None.
    assert!(expand_macro_lane(&doc, &r, &key("c"), "nope", &points).is_none());
    assert!(expand_macro_lane(&doc, &r, &key("c/synth"), "macro1", &points).is_none());
    // An unmapped macro expands to no lanes.
    assert!(
        expand_macro_lane(&doc, &r, &key("c"), "macro2", &points)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn nested_macro_automation_chains_through_the_inner_macro() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
            set_macro("o", 0, target("i", "macro1", 0.0, 0.5)),
            set_macro("o/i", 0, target("fx", "mix", 0.0, 1.0)),
        ],
    );
    let points = vec![
        AutomationPoint {
            tick: 0,
            value: 0.0,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 10,
            value: 1.0,
            curve: Curve::Linear,
        },
    ];
    let lanes = expand_macro_lane(&doc, &r, &key("o"), "macro1", &points).unwrap();
    // Only engine devices get lanes (the inner container's macro is not an engine param).
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].0, key("o/i/fx"));
    assert_eq!(lanes[0].2[1].value, 0.5);
}

#[test]
fn the_helper_agrees_with_expand_for_a_constant_macro() {
    // A hand-made lane on the target equals the mapped macro lane: one point at tick 0.
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 0, target("delay", "mix", 0.1, 0.9)),
            set_param("c", "macro1", 0.375),
        ],
    );
    let flat = doc.expand(&r);
    let pts = vec![AutomationPoint {
        tick: 0,
        value: 0.375,
        curve: Curve::Linear,
    }];
    let lanes = expand_macro_lane(&doc, &r, &key("c"), "macro1", &pts).unwrap();
    assert_eq!(
        lanes[0].2[0].value.to_bits(),
        flat.devices[1].params["mix"].to_bits()
    );
}

#[test]
fn cable_type_is_ordered_for_canonical_output() {
    // Flat cables are sorted like document cables.
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "b"),
            add_inner("c", 1, DELAY, "a"),
            connect("c/boundary:in_l", "c/b:in_l"),
            connect("c/boundary:in_l", "c/a:in_l"),
            connect("c/boundary:in_r", "c/a:in_r"),
            connect("s:out_l", "c:in_l"),
            connect("s:out_r", "c:in_r"),
        ],
    );
    let cables: Vec<Cable> = doc.expand(&r).cables;
    let mut s = cables.clone();
    s.sort();
    assert_eq!(cables, s);
    let _: &[DeviceKey] = &[];
}

#[test]
fn every_split_list_maps_to_its_slots() {
    let r = reg();
    let mut cmds = vec![add(0, CONTAINER, Some("c"))];
    for i in 0..8 {
        cmds.push(add_inner("c", i, SYNTH, &format!("s{i}")));
    }
    let base = doc_of(&r, cmds);
    let desc = r.get(SPLIT).unwrap().descriptor;
    // 0 to 8 splits, each with its own range so a mix-up of slots shows.
    for n in 0..=8usize {
        let list: Vec<_> = (0..n)
            .map(|i| {
                let mut s = split(
                    &format!("s{}", 7 - i),
                    i as u8 * 10,
                    i as u8 * 10 + 9,
                    i as i8 - 4,
                );
                s.vel_lo = 1 + i as u8;
                s.vel_hi = 120 + i as u8;
                s
            })
            .collect();
        let mut d = Document::new(base.clone());
        d.apply(set_splits("c", list.clone()), &r).unwrap();
        let flat = d.doc().expand(&r);
        let sp = &flat.devices[0];
        assert_eq!(sp.key, key("c/~split"));
        assert_eq!(sp.params.len(), n * 6, "n = {n}");
        for (i, s) in list.iter().enumerate() {
            let k = i + 1;
            let get = |p: &str| sp.params[&format!("s{k}.{p}")];
            assert_eq!(get("on"), 1.0);
            assert_eq!(get("key_lo"), f32::from(s.key_lo));
            assert_eq!(get("key_hi"), f32::from(s.key_hi));
            assert_eq!(get("vel_lo"), f32::from(s.vel_lo));
            assert_eq!(get("vel_hi"), f32::from(s.vel_hi));
            assert_eq!(get("transpose"), f32::from(s.transpose));
            // The param ids are the contract's: 6 (k - 1) + 0..6.
            let id = |p: &str| desc.param_by_key(&format!("s{k}.{p}")).unwrap().id.0;
            assert_eq!(id("on"), 6 * (k as u32 - 1));
            assert_eq!(id("transpose"), 6 * (k as u32 - 1) + 5);
            // Slot k goes to out_k and the target of split k.
            let cable = format!("c/~split:out_{k} -> c/{}:notes_in", s.target);
            assert!(flat_cables(&flat).contains(&cable), "{cable}");
        }
        assert_eq!(
            flat_cables(&flat)
                .iter()
                .filter(|c| c.starts_with("c/~split:out_"))
                .count(),
            n
        );
    }
}

#[test]
fn boundary_sockets_and_macro_names() {
    let s = atomik_model::boundary_sockets();
    assert_eq!(s.map(|x| x.0), ["in_l", "in_r", "out_l", "out_r"]);
    let m = atomik_model::MacroDoc::default();
    assert_eq!(m.display_name(2), "Macro 3");
    let m = atomik_model::MacroDoc {
        name: "Tone".into(),
        targets: vec![],
    };
    assert_eq!(m.display_name(0), "Tone");
}

/// QA (mutation M2): the left and right boundary sockets are traced one by one, in and out,
/// even when the two channels come from (and go to) different devices.
#[test]
fn boundary_sockets_are_traced_per_channel_in_and_out() {
    let r = reg();
    let doc = doc_of(
        &r,
        [
            add(0, DELAY, Some("s")),
            add(1, DELAY, Some("t")),
            add(2, CONTAINER, Some("c")),
            add(3, MASTER, Some("master")),
            add_inner("c", 0, DELAY, "a"),
            add_inner("c", 1, DELAY, "b"),
            connect("c/boundary:in_l", "c/a:in_l"),
            connect("c/boundary:in_r", "c/b:in_l"),
            connect("c/a:out_l", "c/boundary:out_r"),
            connect("c/b:out_l", "c/boundary:out_l"),
            connect("s:out_l", "c:in_l"),
            connect("t:out_l", "c:in_r"),
            connect("c:out_l", "master:bus_l"),
            connect("c:out_r", "master:bus_r"),
        ],
    );
    assert_eq!(
        sorted(flat_cables(&doc.expand(&r))),
        [
            "c/a:out_l -> master:bus_r",
            "c/b:out_l -> master:bus_l",
            "s:out_l -> c/a:in_l",
            "t:out_l -> c/b:in_l",
        ]
    );
}
