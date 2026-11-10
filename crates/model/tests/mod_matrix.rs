//! The modulation matrix of a device: `SetModSlot` as one undo step, the optional `"mods"` field
//! of project format 41, the engine params the expansion makes of it, and the way patches, copies
//! and older projects treat it. The device is a stand-in with a matrix of its own, so these
//! tests do not depend on any built-in instrument.

mod common;

use atomik_core::ParamId;
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, MOD_SLOT_PARAMS, MOD_SLOTS, ModSource,
    ModTables, ModTarget, PrepareInfo, ProcessContext, Registry, join_params, mod_slot_params,
};
use atomik_model::{
    Command, DeviceSnapshot, Document, ModSlot, ModelError, PROJECT_VERSION, SyncOp,
    WorkflowCommand, amount_param, empty_doc, engine_params, load_project, save_project,
};
use common::workflow::{one_step, rejected, text, wf};
use common::{add, key, run, set_param};

const TYPE: &str = "test.modulated";

static PARAMS: [atomik_core::ParamDescriptor; 1 + MOD_SLOT_PARAMS] = join_params(
    [atomik_core::ParamDescriptor {
        id: ParamId(0),
        key: "level",
        name: "Level",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: atomik_core::Taper::Linear,
        unit: atomik_core::Unit::None,
        discrete: false,
    }],
    mod_slot_params(1),
);
static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: TYPE,
    name: "Modulated",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &[],
    stages: &[],
};
static TABLES: ModTables = ModTables {
    sources: &[
        ModSource {
            id: 0,
            key: "lfo",
            name: "LFO",
            bipolar: true,
        },
        ModSource {
            id: 1,
            key: "env",
            name: "Envelope",
            bipolar: false,
        },
        ModSource {
            id: 2,
            key: "wheel",
            name: "Wheel",
            bipolar: false,
        },
    ],
    targets: &[
        ModTarget {
            id: 0,
            key: "level",
            name: "Level",
            param: ParamId(0),
        },
        ModTarget {
            id: 1,
            key: "level2",
            name: "Level 2",
            param: ParamId(0),
        },
    ],
    first_param: 1,
};

struct Null;
impl Device for Null {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESC
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {}
}

/// The built-in devices plus the modulated stand-in.
fn registry() -> Registry {
    let mut r = common::registry();
    r.register_with_mods(
        DeviceFactory {
            descriptor: &DESC,
            create: || Box::new(Null),
        },
        &TABLES,
    )
    .unwrap();
    r
}

fn set(device: &str, slot: u8, value: Option<ModSlot>) -> Command {
    wf(WorkflowCommand::SetModSlot {
        device: key(device),
        slot,
        value,
    })
}

fn with_device() -> (Document, Registry) {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, TYPE, Some("m")),
            add(1, common::DELAY, Some("plain")),
        ],
    );
    (d, r)
}

fn slots(d: &Document, k: &str) -> Vec<Option<ModSlot>> {
    d.doc().resolve(&key(k)).unwrap().mods.clone()
}

#[test]
fn a_new_device_has_no_slots() {
    let (d, r) = with_device();
    assert!(slots(&d, "m").is_empty());
    assert!(r.mod_tables(TYPE).is_some());
    assert!(r.mod_tables(common::DELAY).is_none());
}

#[test]
fn setting_and_clearing_a_slot_are_undo_steps_that_report_the_slots_changed() {
    let (mut d, r) = with_device();
    let a = ModSlot::new(0, 1).via(2);
    let ops = one_step(&mut d, &r, set("m", 2, Some(a)));
    assert_eq!(ops, vec![SyncOp::ModSlotsChanged { device: key("m") }]);
    assert_eq!(slots(&d, "m"), vec![None, None, Some(a)]);
    let b = ModSlot::new(1, 0);
    one_step(&mut d, &r, set("m", 0, Some(b)));
    assert_eq!(slots(&d, "m"), vec![Some(b), None, Some(a)]);
    // Changing a slot in place.
    one_step(&mut d, &r, set("m", 2, Some(ModSlot::new(2, 0))));
    // Clearing the last slot trims the list, so it never ends in an empty slot.
    one_step(&mut d, &r, set("m", 2, None));
    assert_eq!(slots(&d, "m"), vec![Some(b)]);
    one_step(&mut d, &r, set("m", 0, None));
    assert!(slots(&d, "m").is_empty());
    // Undo walks back through every step.
    d.undo().unwrap();
    assert_eq!(slots(&d, "m"), vec![Some(b)]);
    d.undo().unwrap();
    assert_eq!(
        slots(&d, "m"),
        vec![Some(b), None, Some(ModSlot::new(2, 0))]
    );
    d.redo().unwrap();
    assert_eq!(slots(&d, "m"), vec![Some(b)]);
}

#[test]
fn clearing_a_slot_puts_its_amount_back_and_undo_restores_both() {
    let (mut d, r) = with_device();
    run(
        &mut d,
        &r,
        [
            set("m", 1, Some(ModSlot::new(0, 0))),
            set_param("m", &amount_param(1), 0.75),
        ],
    );
    let before = text(&d);
    assert!(before.contains("mod2_amount"));
    let ops = one_step(&mut d, &r, set("m", 1, None));
    assert!(ops.contains(&SyncOp::ModSlotsChanged { device: key("m") }));
    assert!(
        ops.iter().any(|o| matches!(
            o,
            SyncOp::ParamChanged { param, value, .. } if param == "mod2_amount" && *value == 0.0
        )),
        "{ops:?}"
    );
    assert_eq!(d.doc().resolve(&key("m")).unwrap().mod_amount(1), 0.0);
    d.undo().unwrap();
    assert_eq!(text(&d), before);
    assert_eq!(d.doc().resolve(&key("m")).unwrap().mod_amount(1), 0.75);
}

#[test]
fn the_amount_is_an_ordinary_automatable_param() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("m", 0, Some(ModSlot::new(0, 0)))]);
    let ops = one_step(&mut d, &r, set_param("m", "mod1_amount", -0.5));
    assert!(matches!(ops[0], SyncOp::ParamChanged { .. }));
    let desc = r.get(TYPE).unwrap().descriptor;
    let amount = desc.param_by_key("mod1_amount").unwrap();
    assert_eq!((amount.min, amount.max, amount.default), (-1.0, 1.0, 0.0));
    // The model clamps it like any param.
    run(&mut d, &r, [set_param("m", "mod1_amount", 9.0)]);
    assert_eq!(d.doc().resolve(&key("m")).unwrap().mod_amount(0), 1.0);
}

#[test]
fn the_command_refuses_what_is_not_a_slot_edit() {
    let (mut d, r) = with_device();
    let ok = ModSlot::new(0, 0);
    // A device without a matrix, an unknown device.
    let e = rejected(&mut d, &r, set("plain", 0, Some(ok)));
    assert!(e.to_string().contains("no modulation matrix"), "{e}");
    rejected(&mut d, &r, set("nope", 0, Some(ok)));
    // A slot outside the block.
    let e = rejected(&mut d, &r, set("m", MOD_SLOTS as u8, Some(ok)));
    assert!(matches!(e, ModelError::BadScope(_)), "{e}");
    // Ids outside the tables.
    rejected(&mut d, &r, set("m", 0, Some(ModSlot::new(3, 0))));
    rejected(&mut d, &r, set("m", 0, Some(ModSlot::new(0, 2))));
    rejected(&mut d, &r, set("m", 0, Some(ModSlot::new(0, 0).via(3))));
    // Clearing an empty slot and setting the same value again.
    rejected(&mut d, &r, set("m", 0, None));
    run(&mut d, &r, [set("m", 0, Some(ok))]);
    rejected(&mut d, &r, set("m", 0, Some(ok)));
    // Every slot can be used.
    for n in 1..MOD_SLOTS as u8 {
        run(&mut d, &r, [set("m", n, Some(ok))]);
    }
    assert_eq!(slots(&d, "m").len(), MOD_SLOTS);
}

#[test]
fn the_slots_are_saved_only_when_used_and_round_trip() {
    let (mut d, r) = with_device();
    let none = text(&d);
    assert!(!none.contains("\"mods\""), "no field without a slot");
    assert!(none.contains(&format!("\"version\": {PROJECT_VERSION}")));
    run(
        &mut d,
        &r,
        [
            set("m", 1, Some(ModSlot::new(0, 1).via(2))),
            set("m", 4, Some(ModSlot::new(1, 0))),
        ],
    );
    let saved = text(&d);
    assert!(saved.contains("\"mods\": ["), "{saved}");
    assert!(saved.contains("{ \"slot\": 1, \"source\": 0, \"target\": 1, \"via\": 2 }"));
    assert!(saved.contains("{ \"slot\": 4, \"source\": 1, \"target\": 0 }"));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(
        back.resolve(&key("m")).unwrap().mods,
        vec![
            None,
            Some(ModSlot::new(0, 1).via(2)),
            None,
            None,
            Some(ModSlot::new(1, 0))
        ]
    );
    assert_eq!(save_project(&back), saved, "the save is canonical");
    // Undoing both slots gives the text without them back, byte for byte.
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(text(&d), none);
}

#[test]
fn a_project_without_slots_differs_from_its_version_40_file_in_the_version_line_alone() {
    let (d, r) = with_device();
    let current = text(&d);
    let old = current.replacen("\"version\": 45", "\"version\": 40", 1);
    let loaded = load_project(&old, &r).expect("a version 40 file loads");
    assert_eq!(save_project(&loaded), current);
}

#[test]
fn the_field_is_refused_in_a_version_40_file_and_on_other_devices_and_when_malformed() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("m", 0, Some(ModSlot::new(0, 0)))]);
    let saved = text(&d);
    let old = saved.replacen("\"version\": 45", "\"version\": 40", 1);
    let e = load_project(&old, &r).unwrap_err().to_string();
    assert!(e.contains("mods") && e.contains("unknown field"), "{e}");
    // On a device type without a matrix.
    let wrong = saved.replacen(
        &format!("\"type\": \"{TYPE}\""),
        &format!("\"type\": \"{}\"", common::DELAY),
        1,
    );
    assert_ne!(wrong, saved);
    let e = load_project(&wrong, &r).unwrap_err().to_string();
    assert!(e.contains("no modulation matrix"), "{e}");
    // Malformed lists.
    let slot = "{ \"slot\": 0, \"source\": 0, \"target\": 0 }";
    for (from, to, why) in [
        (
            slot,
            "{ \"slot\": 8, \"source\": 0, \"target\": 0 }",
            "outside",
        ),
        (
            slot,
            "{ \"slot\": 0, \"source\": 3, \"target\": 0 }",
            "source 3",
        ),
        (
            slot,
            "{ \"slot\": 0, \"source\": 0, \"target\": 2 }",
            "target 2",
        ),
        (
            slot,
            "{ \"slot\": 0, \"source\": 0, \"target\": 0, \"via\": 3 }",
            "source 3",
        ),
        (slot, "{ \"slot\": 0, \"source\": 0 }", "missing"),
        (
            slot,
            "{ \"slot\": 0, \"source\": 0, \"target\": 0, \"amount\": 1 }",
            "unknown field",
        ),
        (
            slot,
            "{ \"slot\": 0, \"source\": 0, \"target\": 0 }, { \"slot\": 0, \"source\": 1, \"target\": 0 }",
            "ascending",
        ),
        (
            slot,
            "{ \"slot\": 1, \"source\": 0, \"target\": 0 }, { \"slot\": 0, \"source\": 1, \"target\": 0 }",
            "ascending",
        ),
    ] {
        assert!(saved.contains(from), "{saved}");
        let bad = saved.replacen(from, to, 1);
        let e = load_project(&bad, &r).unwrap_err().to_string();
        assert!(e.contains(why), "{to}: {e}");
    }
    let empty = saved.replacen(slot, "", 1);
    let e = load_project(&empty, &r).unwrap_err().to_string();
    assert!(e.contains("empty list"), "{e}");
}

#[test]
fn the_expansion_hands_the_engine_the_slot_structure_as_params() {
    let (mut d, r) = with_device();
    run(
        &mut d,
        &r,
        [
            set("m", 0, Some(ModSlot::new(1, 0))),
            set("m", 3, Some(ModSlot::new(0, 1).via(2))),
            set_param("m", "mod4_amount", 0.5),
        ],
    );
    let exp = d.doc().expand(&r);
    let dev = exp.devices.iter().find(|x| x.key == key("m")).unwrap();
    let get = |k: &str| dev.params.get(k).copied();
    assert_eq!(get("internal.mod1_source"), Some(2.0));
    assert_eq!(get("internal.mod1_target"), Some(1.0));
    assert_eq!(get("internal.mod1_via"), Some(0.0));
    assert_eq!(get("internal.mod4_source"), Some(1.0));
    assert_eq!(get("internal.mod4_target"), Some(2.0));
    assert_eq!(get("internal.mod4_via"), Some(3.0));
    assert_eq!(get("mod4_amount"), Some(0.5));
    assert_eq!(
        get("internal.mod2_source"),
        None,
        "unused slots stay default"
    );
    // Every key it makes is a param of the descriptor.
    let desc = r.get(TYPE).unwrap().descriptor;
    for (k, _) in engine_params(&slots(&d, "m"), true) {
        assert!(desc.param_by_key(&k).is_some(), "{k}");
    }
    // A device without slots gets none.
    let plain = exp.devices.iter().find(|x| x.key == key("plain")).unwrap();
    assert!(plain.params.keys().all(|k| !k.starts_with("internal.mod")));
}

#[test]
fn a_patch_carries_the_slots_and_applying_one_replaces_them() {
    let (mut d, r) = with_device();
    run(
        &mut d,
        &r,
        [
            set("m", 1, Some(ModSlot::new(0, 1))),
            set_param("m", "mod2_amount", 0.25),
            add(2, TYPE, Some("n")),
        ],
    );
    let patch = DeviceSnapshot::capture(d.doc(), &[key("m")], false).unwrap();
    assert_eq!(
        patch.single_device().unwrap().mods,
        vec![None, Some(ModSlot::new(0, 1))]
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyPatch {
            device: key("n"),
            snapshot: patch,
        }),
    );
    assert_eq!(slots(&d, "n"), vec![None, Some(ModSlot::new(0, 1))]);
    assert_eq!(d.doc().resolve(&key("n")).unwrap().mod_amount(1), 0.25);
    // Resetting the device empties the slots again, as one undo step.
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ResetDevice { device: key("n") }),
    );
    assert!(
        ops.contains(&SyncOp::ModSlotsChanged { device: key("n") }),
        "{ops:?}"
    );
    assert!(slots(&d, "n").is_empty());
    assert_eq!(d.doc().resolve(&key("n")).unwrap().mod_amount(1), 0.0);
}

#[test]
fn duplicating_a_device_keeps_its_slots() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("m", 0, Some(ModSlot::new(2, 1)))]);
    let n = d.doc().devices.len();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::DuplicateDevices {
            keys: vec![key("m")],
            route: false,
        })],
    );
    assert_eq!(d.doc().devices.len(), n + 1);
    let copy = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == TYPE && x.key != key("m"))
        .unwrap();
    assert_eq!(copy.mods, vec![Some(ModSlot::new(2, 1))]);
}
