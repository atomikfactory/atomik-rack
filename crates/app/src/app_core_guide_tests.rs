//! The factory examples the start screen lists, and what the guided first song reads from the
//! song (one test per step).

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, CreateOptions, CreateSource, CreateTarget, DeviceKey, Note,
    SongCommand, WorkflowCommand, new_project_doc,
};
use std::sync::atomic::{AtomicU64, Ordering};

const SYNTH: &str = "atomik.poly-synth";
const DRUMS: &str = "atomik.drum-machine";
const REVERB: &str = "atomik.reverb";

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-guide-core-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn starter() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::new(), new_project_doc(), None)
}

fn instrument(c: &mut AppCore<RecordingSink>, type_id: &str, name: &str) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(type_id.into()),
        key: Some(key(name)),
        options: CreateOptions::default(),
    }))
    .unwrap();
}

fn note_clip(notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, k)| Note::new(tick, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../factory")
}

// ---------------------------------------------------------------------------- the examples

#[test]
fn the_examples_are_the_projects_of_the_factory_folder_by_name() {
    let t = Tmp::new();
    let projects = t.0.join("Projects");
    for name in ["Zed", "Basalt Beat"] {
        std::fs::create_dir_all(projects.join(name)).unwrap();
        std::fs::write(projects.join(name).join(format!("{name}.atomik")), "{}").unwrap();
    }
    // A folder without its project file and a loose file are not examples.
    std::fs::create_dir_all(projects.join("Empty")).unwrap();
    std::fs::write(projects.join("Loose.atomik"), "{}").unwrap();
    let mut c = starter();
    assert!(c.examples().is_empty(), "no factory folder, no examples");
    c.set_factory_root(Some(t.0.clone()));
    let found = c.examples();
    let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Basalt Beat", "Zed"]);
    assert_eq!(
        found[0].path,
        projects.join("Basalt Beat").join("Basalt Beat.atomik")
    );
    assert_eq!(Some(found[0].line.as_str()), known_line("Basalt Beat"));
    assert!(found[1].line.is_empty(), "an unreadable one has no line");
}

#[test]
fn every_shipped_example_has_a_line_and_opens_as_an_untitled_song() {
    let mut c = starter();
    c.set_factory_root(Some(shipped()));
    let found = c.examples();
    assert_eq!(found.len(), MAX_EXAMPLES);
    for e in &found {
        assert!(
            known_line(&e.name).is_some(),
            "{} needs a line in the table",
            e.name
        );
        assert!(e.line.len() < 80, "{}: a line fits a card", e.line);
        c.open_template(&e.path).unwrap();
        assert!(c.file().is_none(), "{} opens untitled", e.name);
        assert!(!c.doc().song.tracks.is_empty());
    }
}

#[test]
fn an_example_the_table_does_not_know_says_its_tempo_and_tracks() {
    let p = TemplatePreview {
        tracks: vec!["Bass".into(), "Keys".into()],
        tempo_bpm: 96.0,
        meter: (4, 4),
    };
    assert_eq!(preview_line(&p), "96 BPM, 2 tracks");
    let one = TemplatePreview {
        tracks: vec!["Bass".into()],
        tempo_bpm: 87.5,
        meter: (4, 4),
    };
    assert_eq!(preview_line(&one), "87.50 BPM, 1 track");
}

// ---------------------------------------------------------------------------- the steps

#[test]
fn a_fresh_starter_song_has_done_none_of_the_steps() {
    let c = starter();
    assert_eq!(c.guide_progress(), Progress::default());
    assert_eq!(c.exports_finished(), 0);
}

#[test]
fn the_instrument_step_sees_a_note_track() {
    let mut c = starter();
    instrument(&mut c, SYNTH, "lead");
    let p = c.guide_progress();
    assert!(p.instrument && !p.notes && !p.drums);
}

#[test]
fn the_notes_step_needs_a_clip_with_a_note_in_it() {
    let mut c = starter();
    instrument(&mut c, SYNTH, "lead");
    let empty = note_clip(&[]);
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: empty,
    }))
    .unwrap();
    assert!(!c.guide_progress().notes, "an empty clip is not notes");
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: note_clip(&[(0, 60), (PPQ, 64)]),
    }))
    .unwrap();
    assert!(c.guide_progress().notes);
}

#[test]
fn the_loop_step_follows_the_loop_switch() {
    let mut c = starter();
    assert!(!c.guide_progress().looping);
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, 4 * PPQ),
        enabled: true,
    }))
    .unwrap();
    assert!(c.guide_progress().looping);
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, 4 * PPQ),
        enabled: false,
    }))
    .unwrap();
    assert!(!c.guide_progress().looping);
}

#[test]
fn the_drum_step_sees_a_drum_machine_track_and_not_a_synth() {
    let mut c = starter();
    instrument(&mut c, SYNTH, "lead");
    assert!(!c.guide_progress().drums);
    instrument(&mut c, DRUMS, "kit");
    assert!(c.guide_progress().drums);
}

#[test]
fn the_mix_step_wants_a_moved_level_and_an_effect() {
    let mut c = starter();
    instrument(&mut c, SYNTH, "lead");
    let p = c.guide_progress();
    assert!(
        !p.level && !p.effect,
        "a new channel is at its defaults: {p:?}"
    );
    let channel = c
        .doc()
        .console
        .channel_of(&key("lead"))
        .expect("a channel")
        .channel
        .clone();
    // The default value is not a moved level.
    let device = c.doc().device(&channel).unwrap().clone();
    let default = c
        .registry()
        .get(&device.type_id)
        .unwrap()
        .descriptor
        .param_by_key("fader")
        .unwrap()
        .default;
    c.apply_command(Command::SetParam {
        key: channel.clone(),
        param: "fader".into(),
        value: default,
        merge: None,
    })
    .unwrap();
    assert!(!c.guide_progress().level);
    c.apply_command(Command::SetParam {
        key: channel,
        param: "fader".into(),
        value: (default - 0.25).max(0.0),
        merge: None,
    })
    .unwrap();
    let p = c.guide_progress();
    assert!(p.level && !p.effect);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(REVERB.into()),
        target: CreateTarget::Master,
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    let p = c.guide_progress();
    assert!(p.level && p.effect);
    assert!(crate::guide::Step::Mix.done(&p));
}

#[test]
fn the_save_step_sees_a_file() {
    assert!(!starter().guide_progress().saved);
    let c = AppCore::with_doc(
        RecordingSink::new(),
        new_project_doc(),
        Some(PathBuf::from("song.atomik")),
    );
    assert!(c.guide_progress().saved);
}
