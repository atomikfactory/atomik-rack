//! Old projects open unchanged after the master section was added:
//! the format is version 10, every committed example project (all of them but the new
//! `master-demo` were written without a master section) loads and saves back byte for byte, a master appears
//! only in a project that already had one, and the loader never reorders the rack or edits a
//! param (the master lock is command level only).

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Document, Note,
    RackDoc, SongCommand, WorkflowCommand, load_project, new_project_doc, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

fn projects_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects")
}

/// Every `.atomik` file under `dir`, sorted.
fn project_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            project_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "atomik") {
            out.push(path);
        }
    }
    out.sort();
}

fn all_projects() -> Vec<PathBuf> {
    let mut files = Vec::new();
    project_files(&projects_dir(), &mut files);
    files
}

#[test]
fn every_committed_project_is_format_10_and_saves_back_byte_for_byte() {
    let registry = default_registry();
    let files = all_projects();
    assert!(files.len() >= 10, "the examples are there: {files:?}");
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(
            text.contains("\"version\": 26"),
            "{}: still format 10",
            path.display()
        );
        let doc =
            load_project(&text, &registry).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            save_project(&doc),
            text,
            "{}: load then save changes bytes",
            path.display()
        );
    }
}

#[test]
fn a_master_appears_only_in_a_project_that_had_one_and_the_rack_order_is_kept() {
    let registry = default_registry();
    for path in all_projects() {
        let text = std::fs::read_to_string(&path).unwrap();
        let doc = load_project(&text, &registry).unwrap();
        let in_file = text.contains("\"atomik.master\"");
        assert_eq!(
            doc.console.master.is_some(),
            in_file,
            "{}: Open never creates a master",
            path.display()
        );
        // The file lists the devices in rack order; the loader keeps that order.
        let saved = save_project(&doc);
        assert_eq!(saved, text, "{}", path.display());
    }
}

#[test]
fn the_starter_is_not_what_the_old_examples_look_like() {
    // The one document with the master first is the starter (and the master demo built from it);
    // every older example that has a master keeps it where its file put it, so the lock is
    // not imposed by loading.
    let registry = default_registry();
    let starter = new_project_doc();
    assert_eq!(starter.devices.len(), 1);
    let old_with_master_not_first = all_projects().into_iter().any(|path| {
        let text = std::fs::read_to_string(&path).unwrap();
        let doc = load_project(&text, &registry).unwrap();
        doc.console
            .master
            .as_ref()
            .zip(doc.devices.first())
            .is_some_and(|(m, first)| &first.key != m)
    });
    assert!(
        old_with_master_not_first,
        "an example with a late master keeps loading as it was written"
    );
}

fn bounce(doc: &RackDoc) -> atomik_render::Rendered {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    };
    render_doc_in(doc, &default_registry(), &options, None).expect("the project bounces")
}

fn peak(r: &atomik_render::Rendered, channel: usize) -> f32 {
    r.channels[channel].iter().fold(0.0, |m, x| m.max(x.abs()))
}

fn with_song_end(mut doc: Document, bars: i64) -> Document {
    doc.apply(
        Command::Song(SongCommand::SetSongEnd(bars * 4 * PPQ)),
        &default_registry(),
    )
    .unwrap();
    doc
}

/// The starter alone bounces silently without a panic, and with one synth and one note
/// clip the sound is on `out_1` and `out_2` (the two `main` cables) and nowhere else.
#[test]
fn the_starter_bounces_silently_and_one_synth_sounds_on_outputs_one_and_two() {
    let registry = default_registry();
    let silent = bounce(with_song_end(Document::new(new_project_doc()), 1).doc());
    assert!(silent.channels.len() >= 2);
    assert!(
        silent.channels.iter().flatten().all(|x| *x == 0.0),
        "the starter alone is silent"
    );

    let mut d = with_song_end(Document::new(new_project_doc()), 1);
    d.apply(
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(DeviceKey("lead".into())),
            options: CreateOptions::default(),
        }),
        &registry,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::AddClip {
            device: DeviceKey("lead".into()),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 2 * PPQ, 60, 0.8)]),
            },
        }),
        &registry,
    )
    .unwrap();
    let r = bounce(d.doc());
    assert!(peak(&r, 0) > 1e-3, "out_1 sounds: {}", peak(&r, 0));
    assert!(peak(&r, 1) > 1e-3, "out_2 sounds: {}", peak(&r, 1));
    for c in 2..r.channels.len() {
        assert_eq!(peak(&r, c), 0.0, "output {} stays silent", c + 1);
    }
}
