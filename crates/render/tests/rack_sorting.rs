//! The rack sorting mode is view data: a project bounces to the same bits in every mode, and
//! changing the mode back and forth leaves the bounce alone.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    Note, RackDoc, RackSort, SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// Two synths, each with a note clip, an arpeggiator above the first and a delay insert on the
/// second, in the mode `mode`.
fn project(mode: RackSort) -> RackDoc {
    let mut d = Document::new(new_project_doc());
    for (k, pitch) in [("a", 48u8), ("b", 60)] {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type("atomik.poly-synth".into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length: BAR,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    param_lanes: Vec::new(),
                    controllers: Vec::new(),
                    name: None,
                    color: None,
                    grid: None,
                    content: ClipContent::Notes(
                        vec![
                            Note::new(0, PPQ, pitch, 0.8),
                            Note::new(2 * PPQ, PPQ, pitch + 7, 0.7),
                        ]
                        .into(),
                    ),
                },
            }),
        );
    }
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreatePlayer {
            type_id: "atomik.arpeggiator".into(),
            target: CreateTarget::Selected(key("a")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type("atomik.stereo-delay".into()),
            target: CreateTarget::Selected(key("b")),
            key: None,
            options: CreateOptions::default(),
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(BAR)));
    if mode != RackSort::Free {
        apply(&mut d, Command::SetRackSort(mode));
    }
    d.doc().clone()
}

fn bounce(doc: &RackDoc) -> Vec<Vec<u32>> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(256).unwrap(),
        workers: 1,
        loop_count: None,
    };
    let r = common::render_guarded_in(doc, &default_registry(), &options, None).unwrap();
    r.channels
        .iter()
        .map(|c| c.iter().map(|x| x.to_bits()).collect())
        .collect()
}

#[test]
fn every_sort_mode_bounces_the_same_bits() {
    let free = bounce(&project(RackSort::Free));
    assert!(
        free.iter()
            .flatten()
            .any(|b| f32::from_bits(*b).abs() > 1e-3),
        "the project makes sound"
    );
    for mode in [RackSort::PerTrack, RackSort::AutoGroup] {
        let doc = project(mode);
        assert_eq!(doc.sort_mode, mode);
        assert_eq!(bounce(&doc), free, "{mode:?}");
    }
}
