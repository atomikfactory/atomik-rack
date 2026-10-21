//! Rack columns (model half): `RackDevice::column`, `RackDoc::columns`, slot resolution, the
//! slot commands (move, create, paste), `AddRackColumn` / `RemoveRackColumn`, the sort
//! invariant, undo and the sync ops. Every column edit is one undo step and reports
//! `ViewChanged` only (no graph rebuild).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, DeviceSnapshot, Document, MAX_RACK_COLUMNS,
    ModelError, RackSlot, SyncOp, WorkflowCommand, empty_doc, new_project_doc,
};
use common::plugin::plugin_console_registry;
use common::workflow::*;
use common::*;

fn slot(column: u16, index: usize) -> RackSlot {
    RackSlot { column, index }
}

/// Test racks use the plain effect so no console devices come along.
fn reg() -> Registry {
    registry()
}

/// A document with `columns` columns holding `spec` (key, column), each key appended to the
/// end of its column.
fn rack(r: &Registry, columns: u16, spec: &[(&str, u16)]) -> Document {
    let mut d = Document::new(empty_doc());
    for _ in 1..columns {
        d.apply(Command::AddRackColumn, r).unwrap();
    }
    for (k, c) in spec {
        d.apply(
            Command::AddDeviceAt {
                slot: slot(*c, usize::MAX),
                type_id: DELAY.into(),
                key: Some(key(k)),
            },
            r,
        )
        .unwrap();
    }
    valid(&d);
    d
}

/// `"a0 b0 c1"`: every device with its column, in rack order.
fn layout(d: &Document) -> String {
    d.doc()
        .devices
        .iter()
        .map(|x| format!("{}{}", x.key, x.column))
        .collect::<Vec<_>>()
        .join(" ")
}

fn valid(d: &Document) {
    d.doc().check_columns().unwrap();
}

fn mv(keys: &[&str], to: RackSlot) -> Command {
    wf(WorkflowCommand::MoveDevicesToSlot {
        keys: keys_of(keys),
        slot: to,
    })
}

fn view_only(ops: &[SyncOp]) {
    assert_eq!(
        ops,
        [SyncOp::ViewChanged],
        "a column edit never touches the graph"
    );
}

// ---- the document and the validator ---------------------------------------------------

#[test]
fn a_new_document_has_one_column_and_new_devices_are_in_column_zero() {
    let r = reg();
    let d = three_devices(&r);
    assert_eq!(d.doc().columns, 1);
    assert_eq!(layout(&d), "synth0 delay0 mixer0");
    valid(&d);
}

#[test]
fn check_columns_names_the_first_fault() {
    let r = reg();
    let mut doc = rack(&r, 2, &[("a", 0), ("b", 1)]).doc().clone();
    assert!(doc.check_columns().is_ok());
    doc.devices.swap(0, 1);
    let e = doc.check_columns().unwrap_err();
    assert!(
        e.contains("devices[1]") && e.contains("column 0 after column 1"),
        "{e}"
    );
    doc.devices.swap(0, 1);
    doc.devices[1].column = 2;
    assert!(doc.check_columns().unwrap_err().contains("column 2 of 2"));
    doc.devices[1].column = 1;
    doc.columns = 0;
    assert!(doc.check_columns().is_err());
    doc.columns = MAX_RACK_COLUMNS + 1;
    assert!(doc.check_columns().is_err());
}

#[test]
fn check_columns_rejects_a_container_device_outside_column_zero() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, CONTAINER, Some("c")), add_inner("c", 0, DELAY, "x")],
    );
    let mut doc = d.doc().clone();
    assert!(doc.check_columns().is_ok());
    doc.devices[0].inner.as_mut().unwrap().devices[0].column = 1;
    assert!(doc.check_columns().unwrap_err().contains("c/x"));
}

// ---- slots ------------------------------------------------------------------------------

#[test]
fn slot_of_and_column_range_describe_each_column() {
    let r = reg();
    let d = rack(&r, 3, &[("a", 0), ("b", 0), ("c", 2), ("d", 2), ("e", 2)]);
    let doc = d.doc();
    assert_eq!(doc.column_range(0), 0..2);
    assert_eq!(
        doc.column_range(1),
        2..2,
        "an empty column sits where it would start"
    );
    assert_eq!(doc.column_range(2), 2..5);
    assert_eq!(doc.slot_of(&key("d")), Some(slot(2, 1)));
    assert_eq!(doc.slot_of(&key("a")), Some(slot(0, 0)));
    assert_eq!(doc.slot_of(&key("nope")), None);
}

#[test]
fn resolve_slot_gives_flat_indexes_and_handles_empty_columns() {
    let r = reg();
    let d = rack(&r, 3, &[("a", 0), ("b", 0), ("c", 2), ("d", 2)]);
    let doc = d.doc();
    let at = |c, i| {
        let s = doc.resolve_slot(slot(c, i)).unwrap();
        (s.index, s.column, s.adds_column)
    };
    assert_eq!(at(0, 0), (0, 0, false));
    assert_eq!(at(0, 1), (1, 0, false));
    assert_eq!(
        at(0, 99),
        (2, 0, false),
        "past the end means the end of the column"
    );
    assert_eq!(
        at(1, 0),
        (2, 1, false),
        "the empty column starts where column 2 does"
    );
    assert_eq!(at(1, 5), (2, 1, false));
    assert_eq!(at(2, 1), (3, 2, false));
    assert_eq!(
        at(3, 7),
        (4, 3, true),
        "one past the last column appends one"
    );
}

#[test]
fn resolve_slot_refuses_a_column_beyond_the_next_one() {
    let r = reg();
    let d = rack(&r, 2, &[("a", 0)]);
    let e = d.doc().resolve_slot(slot(3, 0)).unwrap_err();
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
}

#[test]
fn column_zero_never_goes_above_the_master_section() {
    let r = plugin_console_registry();
    let mut d = Document::new(new_project_doc());
    d.apply(add(1, DELAY, Some("fx")), &r).unwrap();
    let at = d.doc().resolve_slot(slot(0, 0)).unwrap();
    assert_eq!(at.index, 1, "below the master");
    assert_eq!(d.doc().resolve_slot(slot(0, 1)).unwrap().index, 1);
}

#[test]
fn the_master_cannot_be_moved_to_another_column() {
    let r = plugin_console_registry();
    let mut d = Document::new(new_project_doc());
    d.apply(Command::AddRackColumn, &r).unwrap();
    let e = rejected(&mut d, &r, mv(&["master"], slot(1, 0)));
    assert!(
        matches!(e, ModelError::Fixed { what: "master", .. }),
        "{e:?}"
    );
}

#[test]
fn a_device_moved_to_the_top_of_column_zero_lands_below_the_master() {
    let r = plugin_console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            Command::AddRackColumn,
            Command::AddDeviceAt {
                slot: slot(1, 0),
                type_id: DELAY.into(),
                key: Some(key("fx")),
            },
        ],
    );
    d.apply(mv(&["fx"], slot(0, 0)), &r).unwrap();
    assert_eq!(layout(&d), "master0 fx0");
    valid(&d);
}

// ---- AddRackColumn / RemoveRackColumn -----------------------------------------------

#[test]
fn add_rack_column_appends_an_empty_column_as_one_undo_step() {
    let r = reg();
    let mut d = three_devices(&r);
    let ops = one_step(&mut d, &r, Command::AddRackColumn);
    view_only(&ops);
    assert_eq!(d.doc().columns, 2);
}

#[test]
fn add_rack_column_counts_empty_columns_and_stops_at_the_maximum() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    for n in 2..=MAX_RACK_COLUMNS {
        d.apply(Command::AddRackColumn, &r).unwrap();
        assert_eq!(d.doc().columns, n);
    }
    let e = rejected(&mut d, &r, Command::AddRackColumn);
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
    valid(&d);
}

#[test]
fn remove_rack_column_refuses_the_last_column_and_unknown_columns() {
    let r = reg();
    let mut d = three_devices(&r);
    let e = rejected(&mut d, &r, Command::RemoveRackColumn { column: 0 });
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
    d.apply(Command::AddRackColumn, &r).unwrap();
    rejected(&mut d, &r, Command::RemoveRackColumn { column: 2 });
}

#[test]
fn removing_a_column_moves_its_devices_to_the_end_of_the_left_neighbour() {
    let r = reg();
    let mut d = rack(&r, 3, &[("a", 0), ("b", 1), ("c", 1), ("d", 2)]);
    let ops = one_step(&mut d, &r, Command::RemoveRackColumn { column: 1 });
    view_only(&ops);
    assert_eq!(d.doc().columns, 2);
    assert_eq!(
        layout(&d),
        "a0 b0 c0 d1",
        "d shifts down; the order never changes"
    );
    valid(&d);
    d.undo(&r).unwrap();
    assert_eq!(layout(&d), "a0 b1 c1 d2");
    assert_eq!(d.doc().columns, 3);
}

#[test]
fn removing_column_zero_moves_its_devices_to_the_start_of_the_next_column() {
    let r = reg();
    let mut d = rack(&r, 3, &[("a", 0), ("b", 0), ("c", 1), ("d", 2)]);
    one_step(&mut d, &r, Command::RemoveRackColumn { column: 0 });
    assert_eq!(layout(&d), "a0 b0 c0 d1");
    assert_eq!(d.doc().columns, 2);
    valid(&d);
}

#[test]
fn removing_the_last_column_joins_the_one_to_its_left() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1), ("c", 1)]);
    one_step(&mut d, &r, Command::RemoveRackColumn { column: 1 });
    assert_eq!(layout(&d), "a0 b0 c0");
}

#[test]
fn removing_an_empty_column_only_shifts_the_higher_ones() {
    let r = reg();
    let mut d = rack(&r, 3, &[("a", 0), ("b", 2)]);
    one_step(&mut d, &r, Command::RemoveRackColumn { column: 1 });
    assert_eq!(layout(&d), "a0 b1");
    assert_eq!(d.doc().columns, 2);
}

#[test]
fn an_empty_column_persists_after_its_last_device_is_removed() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1)]);
    d.apply(Command::RemoveDevice { key: key("b") }, &r)
        .unwrap();
    assert_eq!(d.doc().columns, 2);
    assert_eq!(d.doc().column_range(1), 1..1);
}

// ---- moving -----------------------------------------------------------------------------

#[test]
fn moving_a_device_to_another_column_is_one_view_only_step() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1)]);
    let ops = one_step(&mut d, &r, mv(&["a"], slot(1, 1)));
    view_only(&ops);
    assert_eq!(layout(&d), "b0 c1 a1");
    valid(&d);
}

#[test]
fn moving_into_an_empty_column_uses_its_flat_position() {
    let r = reg();
    let mut d = rack(&r, 3, &[("a", 0), ("b", 2)]);
    one_step(&mut d, &r, mv(&["b"], slot(1, 0)));
    assert_eq!(layout(&d), "a0 b1");
}

#[test]
fn a_group_keeps_its_relative_order_and_moves_in_one_step() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 0), ("x", 1), ("y", 1)]);
    // The selection order does not matter: the rack order does.
    let ops = one_step(&mut d, &r, mv(&["c", "a"], slot(1, 1)));
    view_only(&ops);
    assert_eq!(layout(&d), "b0 x1 a1 c1 y1");
    valid(&d);
    // one_step proved that a single undo restores the rack.
    d.undo(&r).unwrap();
    assert_eq!(layout(&d), "a0 b0 c0 x1 y1");
}

#[test]
fn the_index_counts_the_devices_that_stay_in_the_column() {
    let r = reg();
    let mut d = rack(&r, 1, &[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
    d.apply(mv(&["a"], slot(0, 2)), &r).unwrap();
    assert_eq!(layout(&d), "b0 c0 a0 d0");
}

#[test]
fn a_move_to_a_new_column_slot_creates_the_column_in_the_same_step() {
    let r = reg();
    let mut d = rack(&r, 1, &[("a", 0), ("b", 0)]);
    let ops = one_step(&mut d, &r, mv(&["a"], slot(1, 0)));
    view_only(&ops);
    assert_eq!(layout(&d), "b0 a1");
    assert_eq!(d.doc().columns, 2);
    d.undo(&r).unwrap();
    assert_eq!((layout(&d), d.doc().columns), ("a0 b0".to_owned(), 1));
}

#[test]
fn a_move_that_changes_nothing_or_names_a_missing_column_is_rejected() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1)]);
    rejected(&mut d, &r, mv(&["a"], slot(0, 0)));
    let e = rejected(&mut d, &r, mv(&["a"], slot(5, 0)));
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
    rejected(&mut d, &r, mv(&["ghost"], slot(0, 0)));
    rejected(&mut d, &r, mv(&[], slot(0, 0)));
}

#[test]
fn a_column_only_move_keeps_the_flat_order() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1)]);
    // `b` goes to the end of column 0: same flat place, other column.
    one_step(&mut d, &r, mv(&["b"], slot(0, 1)));
    assert_eq!(layout(&d), "a0 b0");
}

#[test]
fn a_flat_move_takes_the_column_of_the_device_before_the_new_position() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1), ("d", 1)]);
    // `a` goes to flat index 2: after `c` (column 1).
    d.apply(
        Command::MoveDevice {
            key: key("a"),
            to: 2,
        },
        &r,
    )
    .unwrap();
    assert_eq!(layout(&d), "b0 c1 a1 d1");
    d.apply(
        Command::MoveDevice {
            key: key("d"),
            to: 0,
        },
        &r,
    )
    .unwrap();
    assert_eq!(layout(&d), "d0 b0 c1 a1", "the top is column 0");
    valid(&d);
}

#[test]
fn a_flat_group_move_joins_the_column_before_it_and_reports_the_view_only() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1), ("d", 1)]);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevices {
            keys: keys_of(&["a", "b"]),
            to: 2,
        }),
    );
    view_only(&ops);
    assert_eq!(layout(&d), "c1 d1 a1 b1");
}

// ---- creating ---------------------------------------------------------------------------

#[test]
fn a_plain_add_joins_the_column_of_the_device_before_it() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1), ("d", 1)]);
    d.apply(add(3, DELAY, Some("m")), &r).unwrap();
    assert_eq!(layout(&d), "a0 b0 c1 m1 d1");
    d.apply(add(2, DELAY, Some("edge")), &r).unwrap();
    assert_eq!(
        layout(&d),
        "a0 b0 edge0 c1 m1 d1",
        "the end of column 0 stays column 0"
    );
    d.apply(add(0, DELAY, Some("top")), &r).unwrap();
    assert_eq!(layout(&d), "top0 a0 b0 edge0 c1 m1 d1");
    d.apply(add(usize::MAX, DELAY, Some("last")), &r).unwrap();
    assert_eq!(layout(&d), "top0 a0 b0 edge0 c1 m1 d1 last1");
    valid(&d);
    // undo and redo reproduce each state
    for expect in [
        "top0 a0 b0 edge0 c1 m1 d1",
        "a0 b0 edge0 c1 m1 d1",
        "a0 b0 c1 m1 d1",
        "a0 b0 c1 d1",
    ] {
        d.undo(&r).unwrap();
        assert_eq!(layout(&d), expect);
    }
}

#[test]
fn add_device_at_places_the_device_in_the_slot() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1), ("d", 1)]);
    let ops = one_step(
        &mut d,
        &r,
        Command::AddDeviceAt {
            slot: slot(1, 1),
            type_id: DELAY.into(),
            key: Some(key("n")),
        },
    );
    assert!(ops.contains(&SyncOp::DeviceAdded(key("n"))));
    assert_eq!(layout(&d), "a0 b0 c1 n1 d1");
    d.apply(
        Command::AddDeviceAt {
            slot: slot(0, 0),
            type_id: DELAY.into(),
            key: Some(key("t")),
        },
        &r,
    )
    .unwrap();
    assert_eq!(layout(&d), "t0 a0 b0 c1 n1 d1");
    valid(&d);
}

#[test]
fn add_device_at_the_new_column_slot_creates_the_column_in_the_same_step() {
    let r = reg();
    let mut d = rack(&r, 1, &[("a", 0)]);
    one_step(
        &mut d,
        &r,
        Command::AddDeviceAt {
            slot: slot(1, 0),
            type_id: DELAY.into(),
            key: Some(key("n")),
        },
    );
    assert_eq!((layout(&d), d.doc().columns), ("a0 n1".to_owned(), 2));
    d.undo(&r).unwrap();
    assert_eq!((layout(&d), d.doc().columns), ("a0".to_owned(), 1));
}

#[test]
fn add_device_at_a_bad_slot_or_type_is_rejected() {
    let r = reg();
    let mut d = rack(&r, 1, &[("a", 0)]);
    rejected(
        &mut d,
        &r,
        Command::AddDeviceAt {
            slot: slot(4, 0),
            type_id: DELAY.into(),
            key: None,
        },
    );
    rejected(
        &mut d,
        &r,
        Command::AddDeviceAt {
            slot: slot(0, 0),
            type_id: "no.such".into(),
            key: None,
        },
    );
}

#[test]
fn create_instrument_at_places_the_instrument_and_wires_it_like_the_flat_command() {
    let r = plugin_console_registry();
    let mut base = Document::new(new_project_doc());
    run(
        &mut base,
        &r,
        [
            Command::AddRackColumn,
            Command::AddDeviceAt {
                slot: slot(1, 0),
                type_id: DELAY.into(),
                key: Some(key("fx")),
            },
        ],
    );
    let create = |c| {
        wf(WorkflowCommand::CreateInstrumentAt {
            slot: c,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        })
    };
    let mut at = base.clone();
    let ops = one_step(&mut at, &r, create(slot(1, 0)));
    assert!(ops.contains(&SyncOp::DeviceAdded(key("lead"))));
    assert_eq!(at.doc().device(&key("lead")).unwrap().column, 1);
    assert_eq!(at.doc().slot_of(&key("lead")), Some(slot(1, 0)));
    assert!(
        at.doc().console.channel_of(&key("lead")).is_some(),
        "it still gets its channel"
    );
    valid(&at);

    // The same wiring as the flat command.
    let mut flat = base.clone();
    flat.apply(
        wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(at.doc().cables, flat.doc().cables);
    assert_eq!(at.doc().console, flat.doc().console);
    assert_eq!(at.doc().song, flat.doc().song);

    // The new-column slot.
    let mut fresh = base.clone();
    one_step(&mut fresh, &r, create(slot(2, 0)));
    assert_eq!(fresh.doc().columns, 3);
    assert_eq!(fresh.doc().device(&key("lead")).unwrap().column, 2);
    valid(&fresh);
}

/// A rack with the master, instrument `lead` (and its channel) in column 0 and an empty
/// column 1.
fn with_channel(r: &Registry) -> Document {
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        r,
        [
            wf(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key("lead")),
                options: CreateOptions::default(),
            }),
            Command::AddRackColumn,
        ],
    );
    d
}

fn effect_at(c: RackSlot, k: &str) -> Command {
    wf(WorkflowCommand::CreateEffectAt {
        slot: c,
        source: CreateSource::Type(DELAY.into()),
        key: Some(key(k)),
        options: CreateOptions::default(),
    })
}

#[test]
fn create_effect_at_routes_as_if_the_device_above_the_slot_were_selected() {
    let r = plugin_console_registry();
    let base = with_channel(&r);
    let channel = base
        .doc()
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    let after_channel = base.doc().slot_of(&channel).unwrap();
    assert_eq!(after_channel.column, 0);

    // Dropped right below the channel: routed like `CreateEffect` with the channel selected,
    // and placed exactly at the slot.
    let mut at = base.clone();
    let target = slot(0, after_channel.index + 1);
    let ops = one_step(&mut at, &r, effect_at(target, "fx"));
    assert!(ops.contains(&SyncOp::DeviceAdded(key("fx"))));
    assert_eq!(at.doc().slot_of(&key("fx")), Some(target));
    let mut selected = base.clone();
    selected
        .apply(
            wf(WorkflowCommand::CreateEffect {
                source: CreateSource::Type(DELAY.into()),
                target: CreateTarget::Selected(channel.clone()),
                key: Some(key("fx")),
                options: CreateOptions::default(),
            }),
            &r,
        )
        .unwrap();
    assert_eq!(at.doc().cables, selected.doc().cables);
    assert_eq!(at.doc().console, selected.doc().console);
    assert!(
        at.doc().cables.iter().any(|c| c.to.device == key("fx")),
        "the effect is wired into the channel's insert chain"
    );
    valid(&at);

    // Dropped into another column, below a device that is not part of the chain: placed
    // there, and routed by the rule of the device above it.
    let mut other = base.clone();
    other
        .apply(
            Command::AddDeviceAt {
                slot: slot(1, 0),
                type_id: DELAY.into(),
                key: Some(key("above")),
            },
            &r,
        )
        .unwrap();
    other.apply(effect_at(slot(1, 1), "fx"), &r).unwrap();
    assert_eq!(other.doc().slot_of(&key("fx")), Some(slot(1, 1)));
    valid(&other);
}

#[test]
fn create_effect_at_the_top_of_a_column_is_only_added_and_not_routed() {
    let r = plugin_console_registry();
    let base = with_channel(&r);
    let mut d = base.clone();
    let ops = one_step(&mut d, &r, effect_at(slot(1, 0), "fx"));
    assert!(ops.contains(&SyncOp::DeviceAdded(key("fx"))));
    assert_eq!(d.doc().slot_of(&key("fx")), Some(slot(1, 0)));
    assert_eq!(
        d.doc().cables,
        base.doc().cables,
        "no auto-route without a device above"
    );
    assert_eq!(d.doc().console, base.doc().console);

    // The new-column slot has no device above it either.
    let mut n = base.clone();
    one_step(&mut n, &r, effect_at(slot(2, 0), "fx"));
    assert_eq!(
        (n.doc().columns, n.doc().device(&key("fx")).unwrap().column),
        (3, 2)
    );
    assert_eq!(n.doc().cables, base.doc().cables);
}

#[test]
fn create_effect_at_below_the_master_uses_the_send_return_rule() {
    let r = plugin_console_registry();
    let mut d = Document::new(new_project_doc());
    d.apply(effect_at(slot(0, 1), "fx"), &r).unwrap();
    assert_eq!(d.doc().slot_of(&key("fx")), Some(slot(0, 1)));
    assert!(
        d.doc().console.return_slot(&key("fx")).is_some(),
        "the master was the device above"
    );
}

#[test]
fn create_effect_at_rejects_instruments_and_bad_slots() {
    let r = plugin_console_registry();
    let mut d = with_channel(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateEffectAt {
            slot: slot(0, 1),
            source: CreateSource::Type(SYNTH.into()),
            key: None,
            options: CreateOptions::default(),
        }),
    );
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    let e = rejected(&mut d, &r, effect_at(slot(9, 0), "fx"));
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
}

// ---- paste ------------------------------------------------------------------------------

#[test]
fn paste_at_a_slot_puts_the_copies_in_that_column() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 0), ("c", 1), ("d", 1)]);
    let snap = DeviceSnapshot::capture(d.doc(), &keys_of(&["a", "b"]), false).unwrap();
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevicesAt {
            snapshot: snap.clone(),
            container: None,
            slot: slot(1, 1),
        }),
    );
    assert!(
        ops.iter()
            .filter(|o| matches!(o, SyncOp::DeviceAdded(_)))
            .count()
            == 2
    );
    assert_eq!(d.doc().devices.len(), 6);
    assert_eq!(
        d.doc().column_range(1).len(),
        4,
        "both copies are in column 1"
    );
    assert_eq!(d.doc().devices[3].column, 1);
    assert_eq!(d.doc().devices[4].column, 1);
    valid(&d);
    // Into a new column.
    let mut n = d.clone();
    one_step(
        &mut n,
        &r,
        wf(WorkflowCommand::PasteDevicesAt {
            snapshot: snap,
            container: None,
            slot: slot(2, 0),
        }),
    );
    assert_eq!(n.doc().columns, 3);
    assert_eq!(n.doc().column_range(2).len(), 2);
    valid(&n);
}

#[test]
fn a_flat_paste_and_a_duplicate_take_the_column_of_the_device_before_them() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1), ("c", 1)]);
    // `b` lives in column 1; its copy goes right after it, in column 1, whatever column the
    // clipboard says.
    d.apply(
        wf(WorkflowCommand::DuplicateDevices {
            keys: keys_of(&["b"]),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().column_range(1).len(), 3);
    let snap = DeviceSnapshot::capture(d.doc(), &keys_of(&["b"]), false).unwrap();
    d.apply(
        wf(WorkflowCommand::PasteDevices {
            snapshot: snap,
            container: None,
            at: 1,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        layout(&d).split(' ').filter(|x| x.ends_with('0')).count(),
        2,
        "{}",
        layout(&d)
    );
    valid(&d);
}

#[test]
fn paste_at_a_slot_is_rejected_for_a_missing_column() {
    let r = reg();
    let mut d = rack(&r, 1, &[("a", 0)]);
    let snap = DeviceSnapshot::capture(d.doc(), &keys_of(&["a"]), false).unwrap();
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevicesAt {
            snapshot: snap,
            container: None,
            slot: slot(3, 0),
        }),
    );
    assert!(matches!(e, ModelError::BadColumn(_)), "{e:?}");
}

// ---- containers -------------------------------------------------------------------------

#[test]
fn inside_a_container_only_the_slot_index_counts_and_columns_stay_zero() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "x"),
            add_inner("c", 1, DELAY, "y"),
            add_inner("c", 2, DELAY, "z"),
        ],
    );
    let ops = one_step(&mut d, &r, mv(&["c/x"], slot(7, 2)));
    assert!(!ops.is_empty());
    let inner: Vec<&str> = d
        .doc()
        .inner(&key("c"))
        .unwrap()
        .devices
        .iter()
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(
        inner,
        ["y", "z", "x"],
        "the column is ignored; the index counts"
    );
    valid(&d);
    assert_eq!(d.doc().columns, 1, "no column was added inside a container");

    // Paste at a slot inside the container: copies in column 0.
    let snap = DeviceSnapshot::capture(d.doc(), &keys_of(&["c/x"]), false).unwrap();
    d.apply(
        wf(WorkflowCommand::PasteDevicesAt {
            snapshot: snap,
            container: Some(key("c")),
            slot: slot(4, 0),
        }),
        &r,
    )
    .unwrap();
    let c = d.doc().inner(&key("c")).unwrap();
    assert_eq!(c.devices.len(), 4);
    assert!(c.devices.iter().all(|x| x.column == 0));
    valid(&d);
}

#[test]
fn pasting_a_device_copied_from_another_column_into_a_container_resets_the_column() {
    let r = reg();
    let mut d = rack(&r, 2, &[("a", 0), ("b", 1)]);
    d.apply(add(2, CONTAINER, Some("c")), &r).unwrap();
    let snap = DeviceSnapshot::capture(d.doc(), &keys_of(&["b"]), false).unwrap();
    d.apply(
        wf(WorkflowCommand::PasteDevices {
            snapshot: snap,
            container: Some(key("c")),
            at: 0,
        }),
        &r,
    )
    .unwrap();
    assert!(
        d.doc()
            .inner(&key("c"))
            .unwrap()
            .devices
            .iter()
            .all(|x| x.column == 0)
    );
    valid(&d);
}

// ---- undo and redo of a whole session ---------------------------------------------------

#[test]
fn undoing_everything_restores_the_columns_and_redo_replays_them() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    let mut states = vec![text(&d)];
    let steps = [
        Command::AddRackColumn,
        add(0, DELAY, Some("a")),
        Command::AddDeviceAt {
            slot: slot(1, 0),
            type_id: DELAY.into(),
            key: Some(key("b")),
        },
        mv(&["a"], slot(2, 0)),
        Command::AddRackColumn,
        Command::RemoveRackColumn { column: 1 },
        Command::RemoveDevice { key: key("b") },
    ];
    for c in steps {
        d.apply(c, &r).unwrap();
        valid(&d);
        states.push(text(&d));
    }
    for want in states.iter().rev().skip(1) {
        d.undo(&r).unwrap();
        valid(&d);
        assert_eq!(&text(&d), want);
    }
    for want in states.iter().skip(1) {
        d.redo(&r).unwrap();
        valid(&d);
        assert_eq!(&text(&d), want);
    }
}
