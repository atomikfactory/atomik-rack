//! The modulation matrix through a bounce: a poly synth with slots renders the same bits for every
//! block size and worker count, a project saved and loaded again bounces the same, a project whose
//! slots were all cleared bounces like one that never had any, and a slot is audible.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, ModSlot, Note, RackDoc, SongCommand,
    WorkflowCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const NOTES: [(i64, i64, u8, f32); 3] = [
    (0, PPQ, 48, 0.9),
    (PPQ, PPQ, 52, 0.5),
    (2 * PPQ, 2 * PPQ, 60, 0.8),
];

// Source and target ids of the poly synth's tables.
const FILTER_ENV: u16 = 1;
const LFO1: u16 = 3;
const VELOCITY: u16 = 5;
const MOD_WHEEL: u16 = 7;
const CUTOFF: u16 = 0;
const PITCH: u16 = 1;
const LEVEL: u16 = 2;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn set_slot(d: &mut Document, slot: u8, value: Option<ModSlot>) {
    apply(
        d,
        Command::Workflow(WorkflowCommand::SetModSlot {
            device: key("synth"),
            slot,
            value,
        }),
    );
}

fn amount(d: &mut Document, slot: usize, value: f32) {
    apply(
        d,
        Command::SetParam {
            key: key("synth"),
            param: format!("mod{}_amount", slot + 1),
            value,
            merge: None,
        },
    );
}

fn base() -> Document {
    let mut d = Document::new(empty_doc());
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("synth")),
        },
    );
    for (p, v) in [("filter.cutoff", 300.0), ("filter.env_amount", 0.0)] {
        apply(
            &mut d,
            Command::SetParam {
                key: key("synth"),
                param: p.into(),
                value: v,
                merge: None,
            },
        );
    }
    apply(
        &mut d,
        Command::Song(SongCommand::AddTrack {
            device: key("synth"),
        }),
    );
    let notes = NOTES
        .iter()
        .map(|&(s, l, k, v)| Note::new(s, l, k, v))
        .collect();
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
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
                midi: Default::default(),
                groove: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(BAR)));
    d
}

fn modulated() -> Document {
    let mut d = base();
    set_slot(&mut d, 0, Some(ModSlot::new(FILTER_ENV, CUTOFF)));
    amount(&mut d, 0, 0.8);
    set_slot(&mut d, 1, Some(ModSlot::new(LFO1, PITCH).via(MOD_WHEEL)));
    amount(&mut d, 1, 0.05);
    set_slot(&mut d, 3, Some(ModSlot::new(VELOCITY, LEVEL)));
    amount(&mut d, 3, -0.3);
    d
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = common::render_guarded_in(doc, &default_registry(), &options, None).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn audible(bits: &[u32]) -> bool {
    bits.iter().any(|b| f32::from_bits(*b).abs() > 1e-3)
}

#[test]
fn a_modulated_synth_bounces_to_the_same_bits_for_every_block_size_and_worker_count() {
    let doc = modulated().doc().clone();
    let first = bounce(&doc, 512, 0);
    assert!(audible(&first));
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3), (128, 4)] {
        assert!(
            bounce(&doc, block, workers) == first,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn the_slots_change_the_sound_and_survive_a_save_and_load() {
    let plain = bounce(base().doc(), 512, 0);
    let doc = modulated().doc().clone();
    let with = bounce(&doc, 512, 0);
    assert!(with != plain, "the slots are audible");
    let text = save_project(&doc);
    assert!(text.contains("\"mods\""), "{text}");
    let back = load_project(&text, &default_registry()).unwrap();
    assert_eq!(save_project(&back), text, "canonical");
    assert!(
        bounce(&back, 512, 0) == with,
        "the loaded project sounds the same"
    );
}

#[test]
fn a_project_whose_slots_were_cleared_bounces_like_one_that_never_had_any() {
    let plain = bounce(base().doc(), 512, 0);
    let mut d = modulated();
    for slot in [0, 1, 3] {
        set_slot(&mut d, slot, None);
    }
    assert!(d.doc().devices[0].mods.is_empty());
    assert!(bounce(d.doc(), 512, 0) == plain);
}
