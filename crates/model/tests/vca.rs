//! VCA groups (model half): the commands and their undo, the rules they refuse, membership
//! when a strip goes, the solo and mute interaction, the gain math, and the project format 26
//! (round trip, refusals with their JSON path, and the version 25 file that loads unchanged).

mod common;

use atomik_core::{VCA_MAX_DB, VCA_MIN_DB};
use atomik_device_sdk::Registry;
use atomik_model::console::{MAX_VCA_NAME_CHARS, MAX_VCAS, OutputDest, VCA_PALETTE, VcaFeed};
use atomik_model::{
    Command, ConsoleCommand, Document, PROJECT_VERSION, SyncOp, WorkflowCommand, load_project,
    new_project_doc, save_project,
};
use common::console_ops::assert_console_invariants;
use common::workflow::wf;
use common::*;
use serde_json::Value;

/// The starter with four synths: channels `channel`, `channel-2`, `channel-3`, `channel-4`.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = new_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        ["a", "b", "c", "d"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn keys(names: &[&str]) -> Vec<atomik_model::DeviceKey> {
    names.iter().map(|n| key(n)).collect()
}

fn create(name: &str, members: &[&str]) -> Command {
    con(ConsoleCommand::CreateVca {
        name: name.into(),
        members: keys(members),
    })
}

fn assign(vca: Option<u32>, strips: &[&str]) -> Command {
    con(ConsoleCommand::AssignToVca {
        vca,
        strips: keys(strips),
    })
}

fn level(vca: u32, db: f32, merge: Option<u64>) -> Command {
    con(ConsoleCommand::SetVcaLevel { vca, db, merge })
}

fn members(d: &Document, vca: u32) -> Vec<String> {
    d.doc()
        .console
        .vca(vca)
        .unwrap_or_else(|| panic!("no VCA {vca}"))
        .members
        .iter()
        .map(|m| m.0.clone())
        .collect()
}

/// Applies `c` and expects a refusal that leaves the document and the history alone.
fn refused(d: &mut Document, r: &Registry, c: Command) -> String {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let e = d.apply(c.clone(), r).expect_err(&format!("{c:?} accepted"));
    assert_eq!(d.doc(), &before, "{c:?} changed the document");
    assert_eq!(d.can_undo(), can_undo, "{c:?} recorded a step");
    e.to_string()
}

/// Applies `c` and checks it is exactly one undo step.
fn one_step(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    assert_console_invariants(&after);
    d.undo(r).expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

fn silent(d: &Document, r: &Registry) -> Vec<String> {
    d.doc()
        .solo_silence(r)
        .into_iter()
        .filter(|(_, s)| *s)
        .map(|(k, _)| k.0)
        .collect()
}

// ---- The commands -------------------------------------------------------------------------

#[test]
fn creating_a_group_is_one_undo_step_with_the_members_in_console_order() {
    let (mut d, r) = project();
    one_step(&mut d, &r, create("Drums", &["channel-3", "channel"]));
    assert_eq!(members(&d, 1), ["channel", "channel-3"]);
    let g = d.doc().console.vca(1).unwrap();
    assert_eq!(
        (g.name.as_str(), g.level_db, g.mute, g.solo, g.color),
        ("Drums", 0.0, false, false, Some(VCA_PALETTE[0]))
    );
    // The next group counts on; an empty group is allowed.
    d.apply(create("Empty", &[]), &r).unwrap();
    assert_eq!(d.doc().console.vca(2).unwrap().color, Some(VCA_PALETTE[1]));
    assert_eq!(
        d.doc()
            .console
            .vcas
            .iter()
            .map(|v| v.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(d.doc().console.default_vca_name(), "VCA 1");
}

#[test]
fn a_strip_is_in_at_most_one_group_and_moves_when_assigned_again() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [create("A", &["channel", "channel-2"]), create("B", &[])],
    );
    one_step(&mut d, &r, assign(Some(2), &["channel-2", "channel-3"]));
    assert_eq!(members(&d, 1), ["channel"]);
    assert_eq!(members(&d, 2), ["channel-2", "channel-3"]);
    // Creating a group over a member takes it from its group too.
    one_step(&mut d, &r, create("C", &["channel"]));
    assert!(members(&d, 1).is_empty());
    assert_eq!(d.doc().console.vca_of(&key("channel")).unwrap().name, "C");
    // Removing takes strips out of whatever group they are in, one step.
    one_step(&mut d, &r, assign(None, &["channel", "channel-3"]));
    assert!(d.doc().console.vca_of(&key("channel")).is_none());
    assert_eq!(members(&d, 2), ["channel-2"]);
}

#[test]
fn assigning_that_changes_nothing_is_refused() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    assert!(refused(&mut d, &r, assign(Some(1), &["channel"])).contains("already"));
    assert!(refused(&mut d, &r, assign(None, &["channel-2"])).contains("none of those"));
}

#[test]
fn names_are_trimmed_unique_ignoring_case_and_bounded() {
    let (mut d, r) = project();
    d.apply(create("  Drums ", &[]), &r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().name, "Drums");
    assert!(refused(&mut d, &r, create("drums", &[])).contains("already called"));
    assert!(refused(&mut d, &r, create("   ", &[])).contains("empty"));
    let long = "x".repeat(MAX_VCA_NAME_CHARS + 1);
    assert!(refused(&mut d, &r, create(&long, &[])).contains("at most"));
    d.apply(create("Bass", &[]), &r).unwrap();
    // A rename keeps its own name legal and refuses another group's.
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RenameVca {
            vca: 1,
            name: "DRUMS".into(),
        }),
    );
    assert!(
        refused(
            &mut d,
            &r,
            con(ConsoleCommand::RenameVca {
                vca: 1,
                name: "bass".into()
            })
        )
        .contains("already called")
    );
}

#[test]
fn unknown_groups_and_strips_are_refused() {
    let (mut d, r) = project();
    d.apply(create("A", &[]), &r).unwrap();
    assert!(refused(&mut d, &r, create("B", &["synth-that-is-not"])).contains("not a console"));
    // A device that is no strip (the instrument) is refused too.
    assert!(refused(&mut d, &r, create("B", &["a"])).contains("not a console"));
    for c in [
        level(9, -3.0, None),
        con(ConsoleCommand::SetVcaMute { vca: 9, on: true }),
        con(ConsoleCommand::SetVcaSolo { vca: 9, on: true }),
        con(ConsoleCommand::SetVcaColor {
            vca: 9,
            color: None,
        }),
        con(ConsoleCommand::DeleteVca { vca: 9 }),
        assign(Some(9), &["channel"]),
    ] {
        assert!(refused(&mut d, &r, c).contains("no VCA group 9"));
    }
}

#[test]
fn at_most_the_limit_of_groups() {
    let (mut d, r) = project();
    for i in 0..MAX_VCAS {
        d.apply(create(&format!("G{i}"), &[]), &r).unwrap();
    }
    assert!(refused(&mut d, &r, create("one more", &[])).contains("at most"));
}

#[test]
fn level_mute_solo_and_colour_are_single_steps_and_the_level_clamps() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    one_step(&mut d, &r, level(1, -6.0, None));
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetVcaSolo { vca: 1, on: true }),
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetVcaColor {
            vca: 1,
            color: Some([10, 20, 30]),
        }),
    );
    d.apply(level(1, 99.0, None), &r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, VCA_MAX_DB);
    d.apply(level(1, -99.0, None), &r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, VCA_MIN_DB);
    d.apply(level(1, f32::NAN, None), &r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, 0.0);
    one_step(&mut d, &r, con(ConsoleCommand::DeleteVca { vca: 1 }));
}

#[test]
fn a_level_drag_with_one_merge_id_is_one_undo_step() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    let before = d.doc().clone();
    for db in [-1.0, -2.0, -3.0, -4.0] {
        d.apply(level(1, db, Some(7)), &r).unwrap();
    }
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, -4.0);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before, "the whole drag is one step");
    d.redo(&r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, -4.0);
    // Another gesture is another step.
    d.apply(level(1, -5.0, Some(8)), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, -4.0);
}

#[test]
fn every_vca_command_reports_a_console_change() {
    let (mut d, r) = project();
    let ops = d.apply(create("A", &["channel"]), &r).unwrap();
    assert!(ops.contains(&SyncOp::ConsoleChanged), "{ops:?}");
    let ops = d.apply(level(1, -3.0, None), &r).unwrap();
    assert!(ops.contains(&SyncOp::ConsoleChanged), "{ops:?}");
    let ops = d.undo(&r).unwrap();
    assert!(ops.contains(&SyncOp::ConsoleChanged), "{ops:?}");
}

// ---- Membership follows the strips --------------------------------------------------------

#[test]
fn removing_a_channel_takes_it_out_of_its_group_and_undo_puts_it_back() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel", "channel-2"]), &r).unwrap();
    let before = d.doc().clone();
    d.apply(
        wf(WorkflowCommand::RemoveDevices { keys: keys(&["a"]) }),
        &r,
    )
    .unwrap();
    assert_eq!(members(&d, 1), ["channel-2"]);
    assert_console_invariants(d.doc());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    assert_eq!(members(&d, 1), ["channel", "channel-2"]);
}

#[test]
fn deleting_a_bus_takes_it_out_of_its_group() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::CreateBus {
            name: "Sub".into(),
            dest: OutputDest::Master,
        })],
    );
    d.apply(create("A", &["channel", "bus"]), &r).unwrap();
    let before = d.doc().clone();
    d.apply(con(ConsoleCommand::DeleteBus { bus: key("bus") }), &r)
        .unwrap();
    assert_eq!(members(&d, 1), ["channel"]);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---- Solo and mute ------------------------------------------------------------------------

#[test]
fn a_group_without_solo_changes_nothing_in_the_solo_silence() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel", "channel-2"]), &r).unwrap();
    d.apply(con(ConsoleCommand::SetVcaMute { vca: 1, on: true }), &r)
        .unwrap();
    assert!(silent(&d, &r).is_empty(), "a mute is not a solo");
}

#[test]
fn soloing_a_group_soloes_its_members_and_silences_the_rest() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel", "channel-2"]), &r).unwrap();
    d.apply(con(ConsoleCommand::SetVcaSolo { vca: 1, on: true }), &r)
        .unwrap();
    assert_eq!(silent(&d, &r), ["channel-3", "channel-4"]);
    // A member's own solo adds to it; another strip's own solo joins the audible set.
    d.apply(set_param("channel-3", "solo", 1.0), &r).unwrap();
    assert_eq!(silent(&d, &r), ["channel-4"]);
    // Taking a member out of the group silences it again.
    d.apply(assign(None, &["channel-2"]), &r).unwrap();
    assert_eq!(silent(&d, &r), ["channel-2", "channel-4"]);
}

#[test]
fn a_soloed_group_with_no_members_soloes_nothing() {
    let (mut d, r) = project();
    d.apply(create("A", &[]), &r).unwrap();
    d.apply(con(ConsoleCommand::SetVcaSolo { vca: 1, on: true }), &r)
        .unwrap();
    assert!(silent(&d, &r).is_empty());
}

/// What the strip's own rule plays: solo beats mute, then the solo silence.
fn plays(mute: bool, solo: bool, feed: VcaFeed, silenced: bool) -> bool {
    let soloed = solo || feed.solo;
    let muted = (mute || feed.mute) && !soloed;
    !(muted || silenced)
}

#[test]
fn the_mute_and_solo_matrix_follows_the_rule_for_a_member_and_a_bystander() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    for bits in 0..16u32 {
        let (own_mute, own_solo, vca_mute, vca_solo) =
            (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
        d.apply(
            set_param("channel", "mute", f32::from(u8::from(own_mute))),
            &r,
        )
        .unwrap();
        d.apply(
            set_param("channel", "solo", f32::from(u8::from(own_solo))),
            &r,
        )
        .unwrap();
        d.apply(
            con(ConsoleCommand::SetVcaMute {
                vca: 1,
                on: vca_mute,
            }),
            &r,
        )
        .unwrap();
        d.apply(
            con(ConsoleCommand::SetVcaSolo {
                vca: 1,
                on: vca_solo,
            }),
            &r,
        )
        .unwrap();
        let feeds: std::collections::BTreeMap<String, VcaFeed> = d
            .doc()
            .vca_feeds()
            .into_iter()
            .map(|(k, f)| (k.0, f))
            .collect();
        let silenced = silent(&d, &r);
        let member = plays(
            own_mute,
            own_solo,
            feeds["channel"],
            silenced.contains(&"channel".to_string()),
        );
        let bystander = plays(
            false,
            false,
            feeds["channel-2"],
            silenced.contains(&"channel-2".to_string()),
        );
        let any_solo = own_solo || vca_solo;
        let expected_member = if any_solo {
            true
        } else {
            !(own_mute || vca_mute)
        };
        assert_eq!(member, expected_member, "member, bits {bits:04b}");
        assert_eq!(bystander, !any_solo, "bystander, bits {bits:04b}");
        assert_eq!(feeds["channel-2"], VcaFeed::NONE);
    }
}

#[test]
fn clearing_the_solos_and_mutes_clears_the_groups_too_in_one_step() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::SetVcaSolo { vca: 1, on: true }),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
            set_param("channel-2", "solo", 1.0),
        ],
    );
    one_step(&mut d, &r, con(ConsoleCommand::ClearSolos));
    assert!(!d.doc().console.vca(1).unwrap().solo);
    assert!(d.doc().console.vca(1).unwrap().mute, "the mute stays");
    one_step(&mut d, &r, con(ConsoleCommand::ClearMutes));
    assert!(!d.doc().console.vca(1).unwrap().mute);
}

// ---- Gain ---------------------------------------------------------------------------------

#[test]
fn the_effective_level_is_the_members_own_fader_plus_the_offset() {
    let (mut d, r) = project();
    d.apply(create("A", &["channel"]), &r).unwrap();
    let doc = d.doc();
    let unity = doc.effective_fader_db(&key("channel"), &r).unwrap();
    assert!(unity.abs() < 1e-4, "{unity}");
    d.apply(level(1, -6.0, None), &r).unwrap();
    let e = d.doc().effective_fader_db(&key("channel"), &r).unwrap();
    assert!((e + 6.0).abs() < 1e-4, "{e}");
    // The member's own fader keeps its position: the sum is in dB.
    d.apply(set_param("channel", "fader", 0.5), &r).unwrap();
    let own = 60.0 * (0.5f32 / 0.75).log10();
    let e = d.doc().effective_fader_db(&key("channel"), &r).unwrap();
    assert!((e - (own - 6.0)).abs() < 1e-3, "{e}");
    d.apply(level(1, VCA_MIN_DB, None), &r).unwrap();
    assert_eq!(
        d.doc().effective_fader_db(&key("channel"), &r),
        Some(f32::NEG_INFINITY)
    );
    // A strip outside a group plays its own level.
    let e = d.doc().effective_fader_db(&key("channel-2"), &r).unwrap();
    assert!(e.abs() < 1e-4);
    assert_eq!(d.doc().effective_fader_db(&key("a"), &r), None);
}

#[test]
fn the_feed_of_a_strip_is_its_groups_state_and_none_outside() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", &["channel-2"]),
            level(1, -3.0, None),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
        ],
    );
    let feeds = d.doc().vca_feeds();
    let names: Vec<&str> = feeds.iter().map(|(k, _)| &*k.0).collect();
    assert_eq!(names, ["channel", "channel-2", "channel-3", "channel-4"]);
    assert_eq!(feeds[0].1, VcaFeed::NONE);
    assert_eq!(
        feeds[1].1,
        VcaFeed {
            db: -3.0,
            mute: true,
            solo: false
        }
    );
    assert!(!feeds[1].1.is_none() && feeds[2].1.is_none());
}

// ---- The file -----------------------------------------------------------------------------

fn grouped() -> (Document, Registry) {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::CreateBus {
                name: "Sub".into(),
                dest: OutputDest::Master,
            }),
            create("Drums", &["channel-2", "channel", "bus"]),
            create("Keys", &["channel-4"]),
            level(1, -4.5, None),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
            con(ConsoleCommand::SetVcaSolo { vca: 2, on: true }),
            con(ConsoleCommand::SetVcaColor {
                vca: 2,
                color: Some([0x12, 0xab, 0xef]),
            }),
        ],
    );
    (d, r)
}

#[test]
fn groups_round_trip_in_format_26_in_a_stable_text() {
    let (d, r) = grouped();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 30);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 30);
    assert_eq!(v["console"]["vcas"][0]["name"], "Drums");
    assert_eq!(v["console"]["vcas"][0]["level_db"], -4.5);
    assert_eq!(v["console"]["vcas"][0]["mute"], true);
    assert!(v["console"]["vcas"][0].get("solo").is_none());
    assert_eq!(
        v["console"]["vcas"][0]["members"],
        serde_json::json!(["channel", "channel-2", "bus"])
    );
    assert_eq!(v["console"]["vcas"][1]["color"], "#12abef");
    assert!(v["console"]["vcas"][1].get("level_db").is_none());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console, d.doc().console);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_project_without_groups_differs_from_its_version_25_file_in_the_version_line() {
    let (d, r) = project();
    let text = save_project(d.doc());
    assert!(!text.contains("vcas"));
    let v25 = text.replace("\"version\": 30", "\"version\": 25");
    let loaded = load_project(&v25, &r).unwrap();
    assert!(loaded.console.vcas.is_empty());
    assert_eq!(save_project(&loaded), text);
    // A file before the format has no field for them.
    let with = save_project(grouped().0.doc()).replace("\"version\": 30", "\"version\": 25");
    let e = load_project(&with, &r).unwrap_err().to_string();
    assert!(e.contains("vcas"), "{e}");
}

fn edited(from: &str, to: &str) -> String {
    let (d, _) = grouped();
    let text = save_project(d.doc());
    assert!(text.contains(from), "{from} not in {text}");
    text.replacen(from, to, 1)
}

#[test]
fn a_bad_group_is_refused_with_its_path() {
    let r = console_registry();
    let cases: [(&str, &str, &str); 9] = [
        ("\"id\": 2", "\"id\": 1", "used twice"),
        ("\"id\": 1", "\"id\": 0", "from 1"),
        ("\"name\": \"Keys\"", "\"name\": \"drums\"", "used twice"),
        (
            "\"name\": \"Keys\"",
            "\"name\": \" Keys\"",
            "valid VCA name",
        ),
        ("\"level_db\": -4.5", "\"level_db\": -61", "outside"),
        ("\"level_db\": -4.5", "\"level_db\": 13", "outside"),
        (
            "\"members\": [\"channel-4\"]",
            "\"members\": [\"channel-2\"]",
            "more than one",
        ),
        (
            "\"members\": [\"channel-4\"]",
            "\"members\": [\"a\"]",
            "not a console",
        ),
        ("\"color\": \"#12abef\"", "\"color\": \"red\"", "colour"),
    ];
    for (from, to, want) in cases {
        let e = load_project(&edited(from, to), &r).unwrap_err().to_string();
        assert!(e.contains(want) && e.contains("console"), "{from} -> {e}");
    }
    let e = load_project(
        &edited(
            "\"members\": [\"channel-4\"]",
            "\"members\": [\"channel-4\"], \"x\": 1",
        ),
        &r,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("unknown field"), "{e}");
}

#[test]
fn members_in_any_order_load_in_console_order() {
    let r = console_registry();
    let text = edited(
        "\"members\": [\"channel\", \"channel-2\", \"bus\"]",
        "\"members\": [\"bus\", \"channel-2\", \"channel\"]",
    );
    let loaded = load_project(&text, &r).unwrap();
    let names: Vec<&str> = loaded
        .console
        .vca(1)
        .unwrap()
        .members
        .iter()
        .map(|m| &*m.0)
        .collect();
    assert_eq!(names, ["channel", "channel-2", "bus"]);
}
