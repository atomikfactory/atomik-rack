//! The masking analysis of a song: each console channel is rendered as a stem and turned into a
//! band map, a pair that plays the same notes at different levels ranks first with the quieter
//! one as the masked track, a track in another register is not part of the worst pair, a selected
//! range is analysed on its own, and a stopped render returns nothing.

use atomik_core::PPQ;
use atomik_dsp::masking::{band_centre, rank_pairs};
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, StemKind, WorkflowCommand, new_project_doc, stem_sources,
};
use atomik_render::default_registry;
use atomik_render::export::ExportSpan;
use atomik_render::masking::{MaskRequest, TrackMap, render_maps};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::sync::atomic::AtomicBool;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn sustained(notes: &[(i64, i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, l, k)| Note::new(t, l, k, 0.9))
                .collect(),
        ),
    }
}

/// Three synths on the starter master, all with a sustained sound: `loud` and `quiet` play the
/// same two notes (a fifth apart, in the low-mid register) at 0 dB and -18 dB, `high` plays two
/// notes three octaves above.
fn song() -> RackDoc {
    let mut d = Document::new(new_project_doc());
    let tracks: [(&str, f32, &[u8]); 3] = [
        ("loud", 0.0, &[52, 59]),
        ("quiet", -18.0, &[52, 59]),
        ("high", -6.0, &[88, 95]),
    ];
    for (k, level, keys) in tracks {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        for (p, v) in [
            ("osc1.wave", 0.0f32),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
            ("env.amp.release", 100.0),
        ] {
            apply(
                &mut d,
                Command::SetParam {
                    key: key(k),
                    param: p.into(),
                    value: v,
                    merge: None,
                },
            );
        }
        let notes: Vec<(i64, i64, u8)> = keys.iter().map(|&n| (0, 2 * BAR, n)).collect();
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: sustained(&notes),
            }),
        );
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    d.doc().clone()
}

fn options(block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: std::num::NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn maps(doc: &RackDoc, span: ExportSpan, block: usize) -> Vec<TrackMap> {
    let registry = default_registry();
    let sources = stem_sources(doc, &registry)
        .into_iter()
        .filter(|s| s.kind == StemKind::Channel)
        .collect();
    render_maps(
        doc,
        &registry,
        (&options(block), None),
        &mut PluginBounce::default(),
        &MaskRequest { span, sources },
        RenderControl::default(),
    )
    .expect("renders")
    .expect("not stopped")
}

#[test]
fn the_pair_that_shares_a_register_ranks_first_and_the_quiet_one_is_masked() {
    let doc = song();
    let tracks = maps(&doc, ExportSpan::Song, 512);
    assert_eq!(tracks.len(), 3);
    let refs: Vec<_> = tracks.iter().map(|t| &t.map).collect();
    let ranked = rank_pairs(&refs, 5);
    let top = ranked.first().expect("a pair clashes");
    // Console order is the order the synths were made in: loud, quiet, high.
    assert_eq!((top.masker, top.masked), (0, 1));
    assert!(top.score > 30.0, "score {}", top.score);
    let hz = band_centre(top.worst);
    assert!((100.0..4000.0).contains(&hz), "the clash is at {hz} Hz");
    assert!(top.zone()[top.worst]);
    assert!(top.masker_db[top.worst] > top.masked_db[top.worst] + 6.0);
    // Every map has the same length: the song to its End.
    assert!(
        tracks
            .windows(2)
            .all(|w| w[0].map.levels.len() == w[1].map.levels.len())
    );
}

#[test]
fn a_selected_range_is_analysed_on_its_own_and_the_maps_are_the_same_at_every_block_size() {
    let doc = song();
    let whole = maps(&doc, ExportSpan::Song, 512);
    let part = maps(&doc, ExportSpan::Range(0, BAR), 128);
    assert!(part[0].map.levels.len() < whole[0].map.levels.len());
    let again = maps(&doc, ExportSpan::Song, 64);
    for (a, b) in whole.iter().zip(&again) {
        assert_eq!(a.map, b.map, "{}", a.source.name);
    }
}

#[test]
fn a_stopped_render_returns_nothing() {
    let doc = song();
    let registry = default_registry();
    let stop = AtomicBool::new(true);
    let sources = stem_sources(&doc, &registry);
    let result = render_maps(
        &doc,
        &registry,
        (&options(512), None),
        &mut PluginBounce::default(),
        &MaskRequest {
            span: ExportSpan::Song,
            sources,
        },
        RenderControl {
            cancel: Some(&stop),
            progress: None,
        },
    )
    .expect("no error");
    assert!(result.is_none());
}
