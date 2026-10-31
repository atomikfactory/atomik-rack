//! Note lanes in the offline render: two lanes bounce like one lane holding every clip, a muted
//! lane is silent (bounce and freeze), and bouncing chosen clips in place renders the clips of a
//! muted lane too.

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::freeze::{FreezeRender, FreezeRequest, render_freeze};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn options(block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn song_cmd(c: SongCommand) -> Command {
    Command::Song(c)
}

fn notes_clip(start: i64, notes: &[(i64, i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, l, k)| Note::new(t, l, k, 0.8))
                .collect(),
        ),
    }
}

/// One synth `lead` on the starter master with `lanes` note lanes.
fn project(lanes: usize) -> Document {
    let mut d = Document::new(new_project_doc());
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        song_cmd(SongCommand::SetTrackFlags {
            device: key("lead"),
            muted: false,
            solo: false,
            armed: false,
        }),
    );
    for _ in 1..lanes {
        apply(
            &mut d,
            song_cmd(SongCommand::AddNoteLane {
                device: key("lead"),
                name: None,
            }),
        );
    }
    apply(&mut d, song_cmd(SongCommand::SetSongEnd(5 * BAR)));
    d
}

fn add_clip(d: &mut Document, lane: usize, clip: Clip) -> ClipId {
    apply(
        d,
        song_cmd(SongCommand::AddClip {
            device: key("lead"),
            lane,
            clip,
        }),
    );
    d.last_clip_id().expect("a clip was added")
}

fn set_muted(d: &mut Document, lane: usize, muted: bool) {
    apply(
        d,
        song_cmd(SongCommand::SetLaneMuted {
            device: key("lead"),
            lane,
            muted,
        }),
    );
}

const FIRST: &[(i64, i64, u8)] = &[(0, PPQ, 60), (PPQ, PPQ, 64)];
const SECOND: &[(i64, i64, u8)] = &[(0, 2 * PPQ, 67), (2 * PPQ, PPQ, 72)];

fn bounce(doc: &RackDoc, block: usize) -> Rendered {
    render_doc(doc, &default_registry(), &options(block)).unwrap()
}

fn peak(r: &Rendered) -> f32 {
    r.channels
        .iter()
        .flatten()
        .fold(0.0_f32, |m, x| m.max(x.abs()))
}

#[test]
fn two_note_lanes_bounce_bit_for_bit_like_one_lane_with_all_clips() {
    let mut two = project(2);
    add_clip(&mut two, 0, notes_clip(0, FIRST));
    add_clip(&mut two, 1, notes_clip(2 * BAR, SECOND));
    let mut one = project(1);
    add_clip(&mut one, 0, notes_clip(0, FIRST));
    add_clip(&mut one, 0, notes_clip(2 * BAR, SECOND));
    let reference = bounce(one.doc(), 512);
    assert!(peak(&reference) > 0.01, "the project sounds");
    assert_eq!(bounce(two.doc(), 512), reference);
    // Independent of the host block size, too.
    assert_eq!(bounce(two.doc(), 64), reference);
}

#[test]
fn a_muted_note_lane_is_silent_in_the_bounce_and_unmuted_it_sounds_again() {
    let mut d = project(2);
    add_clip(&mut d, 1, notes_clip(0, FIRST));
    set_muted(&mut d, 1, true);
    assert_eq!(peak(&bounce(d.doc(), 512)), 0.0);
    set_muted(&mut d, 1, false);
    let mut one = project(1);
    add_clip(&mut one, 0, notes_clip(0, FIRST));
    assert_eq!(bounce(d.doc(), 512), bounce(one.doc(), 512));
}

fn freeze_render_of(doc: &RackDoc, request: &FreezeRequest) -> FreezeRender {
    render_freeze(
        doc,
        &default_registry(),
        (&options(512), None),
        &mut PluginBounce::default(),
        request,
        RenderControl::default(),
    )
    .unwrap()
    .unwrap()
}

fn request(clips: Option<Vec<ClipId>>, span: (i64, i64)) -> FreezeRequest {
    FreezeRequest {
        device: key("lead"),
        effects: 0,
        clips,
        span,
        tail_seconds: 0.0,
    }
}

#[test]
fn freezing_the_whole_track_leaves_a_muted_lane_out() {
    let mut d = project(2);
    add_clip(&mut d, 0, notes_clip(0, FIRST));
    add_clip(&mut d, 1, notes_clip(0, SECOND));
    set_muted(&mut d, 1, true);
    let mut without = project(2);
    add_clip(&mut without, 0, notes_clip(0, FIRST));
    let req = request(None, (0, BAR));
    let frozen = freeze_render_of(d.doc(), &req);
    assert!(frozen.audio.channels[0].iter().any(|x| x.abs() > 0.01));
    assert_eq!(frozen.audio, freeze_render_of(without.doc(), &req).audio);
}

#[test]
fn bouncing_a_clip_of_a_muted_lane_in_place_renders_it() {
    let mut d = project(2);
    add_clip(&mut d, 0, notes_clip(0, FIRST));
    let held = add_clip(&mut d, 1, notes_clip(0, SECOND));
    set_muted(&mut d, 1, true);
    let req = request(Some(vec![held]), (0, BAR));
    let render = freeze_render_of(d.doc(), &req);
    assert!(
        render.audio.channels[0].iter().any(|x| x.abs() > 0.01),
        "the muted lane's clip is rendered"
    );
    // The same as bouncing it from an unmuted lane, and only it (not the first lane's clip).
    set_muted(&mut d, 1, false);
    assert_eq!(render.audio, freeze_render_of(d.doc(), &req).audio);
}
