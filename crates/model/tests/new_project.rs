//! The starter document (model half): `new_project_doc()` is the empty
//! document plus the console master directly under Audio I/O, with `main` cabled to outputs 1
//! and 2. `empty_doc()` stays empty. The starter saves byte for byte like any document, loads
//! back to itself, and the console commands build on it without a second master.

mod common;

use atomik_model::{
    Command, ConsoleCommand, Document, empty_doc, load_project, new_project_doc, save_project,
};
use common::audio::audio_registry;
use common::*;

const GOLDEN: &str = "tests/golden/new_project.json";

fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN)
}

#[test]
fn the_starter_holds_exactly_the_master() {
    let doc = new_project_doc();
    let keys: Vec<&str> = doc.devices.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["master"]);
    let master = &doc.devices[0];
    assert_eq!(master.type_id, MASTER);
    assert_eq!(master.name, "Master");
    assert!(!master.folded, "the master is created unfolded");
    assert!(master.params.is_empty(), "defaults are not stored");
    assert_eq!(master.color, None);
    assert!(master.zones.is_empty());
    assert!(master.steps.is_none() && master.inner.is_none() && master.plugin.is_none());
}

#[test]
fn the_starter_cables_main_to_outputs_one_and_two_and_nothing_else() {
    let doc = new_project_doc();
    assert_eq!(
        doc.cables,
        [
            cable("master:main_l", "interface:out_1"),
            cable("master:main_r", "interface:out_2"),
        ]
    );
}

#[test]
fn the_starter_console_has_a_master_and_nothing_else() {
    let doc = new_project_doc();
    assert_eq!(doc.console.master, Some(key("master")));
    assert!(doc.console.channels.is_empty());
    assert!(doc.console.returns.iter().all(Option::is_none));
    assert!(doc.console.compensation, "compensation is on");
}

#[test]
fn the_starter_keeps_the_other_fields_of_the_empty_document() {
    let (start, empty) = (new_project_doc(), empty_doc());
    assert_eq!(start.sample_rate, empty.sample_rate);
    assert_eq!(start.tempo_bpm, empty.tempo_bpm);
    assert_eq!(start.seed, empty.seed);
    assert_eq!(start.output_channels, empty.output_channels);
    assert_eq!(start.input_channels, empty.input_channels);
    assert_eq!(start.assets, empty.assets);
    assert_eq!(start.song, empty.song, "one transport track as before");
}

#[test]
fn the_empty_document_is_still_empty() {
    let doc = empty_doc();
    assert!(doc.devices.is_empty());
    assert!(doc.cables.is_empty());
    assert_eq!(doc.console.master, None);
    assert!(doc.console.channels.is_empty());
}

#[test]
fn the_starter_is_deterministic_and_saves_byte_identically() {
    assert_eq!(new_project_doc(), new_project_doc());
    assert_eq!(
        save_project(&new_project_doc()),
        save_project(&new_project_doc())
    );
}

#[test]
fn the_saved_starter_is_the_committed_golden() {
    let text = save_project(&new_project_doc());
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_path().parent().unwrap()).unwrap();
        std::fs::write(golden_path(), &text).unwrap();
    }
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    assert_eq!(
        golden.replace("\r\n", "\n"),
        text,
        "regenerate with ATOMIK_UPDATE_GOLDEN=1"
    );
}

#[test]
fn loading_then_saving_the_starter_is_the_identity() {
    for registry in [console_registry(), self::real_registry()] {
        let doc = new_project_doc();
        let text = save_project(&doc);
        let back = load_project(&text, &registry).unwrap();
        assert_eq!(back, doc);
        assert_eq!(save_project(&back), text);
    }
}

fn real_registry() -> atomik_device_sdk::Registry {
    let mut r = atomik_device_sdk::Registry::new();
    atomik_devices::register_all(&mut r).unwrap();
    r
}

#[test]
fn a_fresh_new_project_has_an_empty_undo_stack() {
    let d = Document::new(new_project_doc());
    assert!(!d.can_undo());
    assert!(!d.can_redo());
}

#[test]
fn the_first_instrument_reuses_the_master_and_its_cables() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [add(usize::MAX, SYNTH, Some("synth"))]);
    let masters = d
        .doc()
        .devices
        .iter()
        .filter(|x| x.type_id == MASTER)
        .count();
    assert_eq!(masters, 1, "no second master");
    assert_eq!(d.doc().devices[0].key, key("master"));
    assert_eq!(d.doc().console.master, Some(key("master")));
    assert_eq!(d.doc().console.channels.len(), 1);
    for c in [
        cable("master:main_l", "interface:out_1"),
        cable("master:main_r", "interface:out_2"),
    ] {
        assert!(d.doc().cables.contains(&c), "{c:?} is kept");
    }
    // The channel feeds the existing master.
    assert!(
        d.doc()
            .cables
            .contains(&cable("channel:out_l", "master:bus_l"))
    );
    // Undo takes the instrument and its channel and leaves the starter exactly as it was.
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &new_project_doc());
}

#[test]
fn an_explicit_channel_command_reuses_the_master_too() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("fx"))]);
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::AddChannel {
            source: key("fx"),
        })],
    );
    assert_eq!(
        d.doc()
            .devices
            .iter()
            .filter(|x| x.type_id == MASTER)
            .count(),
        1
    );
}

#[test]
fn an_audio_track_on_the_starter_gets_its_channel_and_the_master_stays_put() {
    let r = audio_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [common::audio::add_audio_track()]);
    assert_eq!(d.doc().devices[0].key, key("master"));
    assert_eq!(d.doc().console.channels.len(), 1);
}

#[test]
fn the_starter_saves_and_loads_after_edits_and_undo_restores_the_text() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    let before = save_project(d.doc());
    run(&mut d, &r, [add(usize::MAX, SYNTH, Some("synth"))]);
    let after = save_project(d.doc());
    assert_eq!(load_project(&after, &r).unwrap(), *d.doc());
    d.undo(&r).unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo(&r).unwrap();
    assert_eq!(save_project(d.doc()), after);
}

#[test]
fn the_starter_master_is_the_master_the_first_channel_creates() {
    // Same device (key, params, layout flags) and same console entry as makes; the name
    // is the stub descriptor's here, which is also "Master".
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("synth"))]);
    let made = d
        .doc()
        .device(&key("master"))
        .expect("the first channel made a master");
    assert_eq!(made, &new_project_doc().devices[0]);
    assert_eq!(d.doc().console.master, new_project_doc().console.master);
    for c in new_project_doc().cables {
        assert!(d.doc().cables.contains(&c));
    }
}
