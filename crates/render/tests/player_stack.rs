//! Player stacks through the offline render: a track whose notes enter at the top of its players
//! sends its notes there and its automation to the instrument, a bounce and a freeze play through
//! the stack, and a stale entry plays the track's own device.

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, Command, CreateOptions, CreateSource, CreateTarget, Curve,
    DeviceKey, Document, Note, RackDoc, SongCommand, WorkflowCommand, clips_span, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::freeze::{FreezeRequest, render_freeze};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{
    PluginBounce, ProjectOptions, expanded_ids, render_doc_in, song_input,
};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
const FILTER: &str = "atomik.note-filter";

const NOTES: [(i64, i64, u8, f32); 3] = [
    (0, PPQ, 48, 0.8),
    (PPQ, PPQ, 52, 0.6),
    (2 * PPQ, 2 * PPQ, 60, 0.7),
];

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

/// A synth with its channel and a track of `notes` and a cutoff sweep on a second lane. With
/// `stack` a note filter that moves every note up `transpose` semitones sits above it.
fn project(notes: &[(i64, i64, u8, f32)], stack: Option<f32>) -> RackDoc {
    let mut d = Document::new(new_project_doc());
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        }),
    );
    for (p, v) in [("env.amp.release", 40.0), ("filter.cutoff", 900.0)] {
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
    let notes: Vec<Note> = notes
        .iter()
        .map(|&(t, l, k, v)| Note::new(t, l, k, v))
        .collect();
    let clip = |content| Clip {
        id: ClipId(0),
        start: 0,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content,
    };
    apply(
        &mut d,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
    );
    for (lane, content) in [
        (0, ClipContent::Notes(notes)),
        (
            1,
            ClipContent::Automation(vec![
                AutomationPoint {
                    tick: 0,
                    value: 500.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: BAR,
                    value: 6000.0,
                    curve: Curve::Linear,
                },
            ]),
        ),
    ] {
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key("synth"),
                lane,
                clip: clip(content),
            }),
        );
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(BAR)));
    if let Some(transpose) = stack {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreatePlayer {
                type_id: FILTER.into(),
                target: CreateTarget::Selected(key("synth")),
                options: CreateOptions::default(),
            }),
        );
        let filter = d
            .doc()
            .devices
            .iter()
            .find(|x| x.type_id == FILTER)
            .map(|x| x.key.clone())
            .unwrap();
        apply(
            &mut d,
            Command::SetParam {
                key: filter,
                param: "transpose".into(),
                value: transpose,
                merge: None,
            },
        );
    }
    d.doc().clone()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<Vec<u32>> {
    let r = render_doc_in(doc, &default_registry(), &options(block, workers), None).unwrap();
    r.channels
        .iter()
        .map(|c| c.iter().map(|x| x.to_bits()).collect())
        .collect()
}

fn audible(channels: &[Vec<u32>]) -> bool {
    channels
        .iter()
        .flatten()
        .any(|b| f32::from_bits(*b).abs() > 1e-3)
}

fn filter_key(doc: &RackDoc) -> DeviceKey {
    doc.devices
        .iter()
        .find(|d| d.type_id == FILTER)
        .map(|d| d.key.clone())
        .expect("a note filter")
}

#[test]
fn render_sends_track_notes_to_the_entry_and_automation_to_the_instrument() {
    let registry = default_registry();
    let doc = project(&NOTES, Some(12.0));
    let filter = filter_key(&doc);
    assert_eq!(
        doc.song.track(&key("synth")).unwrap().notes_to,
        Some(filter.clone())
    );
    let ids = expanded_ids(&doc.expand(&registry));
    let input = song_input(&doc, &registry, |k| ids.get(k).copied());
    let (synth, filter) = (ids[&key("synth")], ids[&filter]);
    let at = |id| input.tracks.iter().filter(move |t| t.device == id);
    // The notes go into the filter, the automation stays with the synth.
    let entry: Vec<_> = at(filter).collect();
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0].notes.len(), NOTES.len());
    assert!(entry[0].automation.is_empty());
    let instrument: Vec<_> = at(synth).collect();
    assert_eq!(instrument.len(), 1);
    assert!(instrument[0].notes.is_empty());
    assert_eq!(instrument[0].automation.len(), 1);
    // A track without a stack is one engine track, as before.
    let plain = project(&NOTES, None);
    let ids = expanded_ids(&plain.expand(&registry));
    let input = song_input(&plain, &registry, |k| ids.get(k).copied());
    assert_eq!(input.tracks.len(), 1);
    assert_eq!(input.tracks[0].notes.len(), NOTES.len());
    assert_eq!(input.tracks[0].automation.len(), 1);
}

#[test]
fn a_bounce_through_a_stack_equals_the_edited_clip_and_is_bit_identical_across_blocks() {
    let up: Vec<_> = NOTES
        .iter()
        .map(|&(t, l, k, v)| (t, l, k + 12, v))
        .collect();
    let direct = bounce(&project(&up, None), 512, 0);
    assert!(audible(&direct));
    let stacked = project(&NOTES, Some(12.0));
    assert!(
        bounce(&stacked, 512, 0) == direct,
        "the notes play +12, the sweep still moves the synth"
    );
    // Not a no-op: without the transpose the stack sounds like the plain clip.
    assert!(bounce(&project(&NOTES, Some(0.0)), 512, 0) != direct);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&stacked, block, workers) == direct,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn a_stale_entry_plays_the_track_device_in_a_bounce() {
    let plain = bounce(&project(&NOTES, None), 512, 0);
    let mut doc = project(&NOTES, Some(12.0));
    doc.song
        .tracks
        .iter_mut()
        .find(|t| t.device == key("synth"))
        .unwrap()
        .notes_to = Some(key("gone"));
    // The filter is still cabled into the synth but nothing feeds it: the synth plays the clip
    // itself, as if the stack were not there.
    assert!(audible(&plain));
    assert!(bounce(&doc, 512, 0) == plain);
    // The same when the device the entry names has been taken out of the rack.
    let mut doc = project(&NOTES, Some(12.0));
    let filter = filter_key(&doc);
    doc.devices.retain(|d| d.key != filter);
    doc.cables
        .retain(|c| c.from.device != filter && c.to.device != filter);
    assert!(bounce(&doc, 512, 0) == plain);
}

fn freeze_request(doc: &RackDoc) -> FreezeRequest {
    FreezeRequest {
        device: key("synth"),
        effects: 0,
        clips: None,
        span: clips_span(doc.song.track(&key("synth")).unwrap()).unwrap(),
        tail_seconds: 0.0,
    }
}

fn freeze(doc: &RackDoc, block: usize) -> atomik_audio_file::AudioData {
    render_freeze(
        doc,
        &default_registry(),
        (&options(block, 0), None::<&std::path::Path>),
        &mut PluginBounce::default(),
        &freeze_request(doc),
        RenderControl::default(),
    )
    .unwrap()
    .unwrap()
    .audio
}

#[test]
fn freeze_renders_through_the_stack() {
    let up: Vec<_> = NOTES
        .iter()
        .map(|&(t, l, k, v)| (t, l, k + 12, v))
        .collect();
    let direct = freeze(&project(&up, None), 512);
    assert!(direct.channels[0].iter().any(|x| x.abs() > 1e-3));
    let stacked = project(&NOTES, Some(12.0));
    assert_eq!(freeze(&stacked, 512), direct, "frozen through the filter");
    assert_eq!(freeze(&stacked, 64), direct, "whatever the block size");
    assert_ne!(freeze(&project(&NOTES, Some(0.0)), 512), direct);
}

#[test]
fn two_tracks_that_enter_at_one_player_share_its_engine_track() {
    // The player has a track of its own and the synth's track enters at it too (what deleting
    // the player in between leaves): the engine takes one track per device.
    let registry = default_registry();
    let mut d = Document::new(project(&NOTES, Some(12.0)));
    let filter = filter_key(d.doc());
    apply(
        &mut d,
        Command::Song(SongCommand::AddTrack {
            device: filter.clone(),
        }),
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: filter.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note::new(BAR / 2, PPQ, 64, 0.5)]),
            },
        }),
    );
    let doc = d.doc();
    let ids = expanded_ids(&doc.expand(&registry));
    let input = song_input(doc, &registry, |k| ids.get(k).copied());
    let on_filter: Vec<_> = input
        .tracks
        .iter()
        .filter(|t| t.device == ids[&filter])
        .collect();
    assert_eq!(on_filter.len(), 1);
    assert_eq!(on_filter[0].notes.len(), NOTES.len() + 1);
    assert!(atomik_engine::compile_song(&input).is_ok());
}
