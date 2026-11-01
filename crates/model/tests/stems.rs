//! The strips of the console that can be taken as stems: their order, names and taps.

mod common;

use atomik_model::console::OutputDest;
use atomik_model::{
    Command, ConsoleCommand, Document, StemKind, new_project_doc, stem_file_names, stem_sources,
};
use common::*;

fn project() -> (Document, atomik_device_sdk::Registry) {
    let r = console_registry();
    let mut doc = new_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("lead")),
            add(usize::MAX, SYNTH, Some("pad")),
        ],
    );
    (d, r)
}

#[test]
fn the_strips_come_in_console_order_with_their_taps() {
    let (mut d, r) = project();
    d.apply(
        Command::Console(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        }),
        &r,
    )
    .unwrap();
    let sources = stem_sources(d.doc(), &r);
    let kinds: Vec<StemKind> = sources.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            StemKind::Channel,
            StemKind::Channel,
            StemKind::Bus,
            StemKind::Master
        ]
    );
    // A channel is named after its source and has a dry tap: the source's own outputs.
    assert_eq!(sources[0].name, d.doc().device(&key("lead")).unwrap().name);
    assert_eq!(sources[0].tap[0].socket, "out_l");
    assert_eq!(sources[0].tap[1].socket, "out_r");
    let dry = sources[0].dry.as_ref().expect("a dry tap");
    assert_eq!(dry[0].device, key("lead"));
    assert_eq!(sources[2].name, "Drums");
    assert!(sources[2].dry.is_none());
    assert_eq!(sources[3].tap[0].socket, "main_l");
    let names = stem_file_names(&sources);
    assert_eq!(names.len(), 4);
    assert!(names[3].ends_with("Master"), "{names:?}");
}

#[test]
fn a_project_without_a_console_has_no_stems() {
    let r = console_registry();
    let d = Document::new(new_project_doc());
    let _ = &r;
    // The starter has no channel until a device is added.
    let sources = stem_sources(d.doc(), &r);
    assert!(sources.iter().all(|s| s.kind != StemKind::Channel));
}
