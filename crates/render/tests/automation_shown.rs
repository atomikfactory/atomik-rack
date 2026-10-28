//! The value a control shows for an automated param is the value the song player applies:
//! the model's overlay ([`RackDoc::automation_values_at`]) against the engine's lane rule
//! ([`atomik_engine::lane_value`]) over the lanes `song_input` hands the engine, for a lane
//! with a clip offset and a clip loop.

use atomik_core::PPQ;
use atomik_engine::DeviceInstanceId;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Document, SongCommand,
    empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::song_input;

#[test]
fn the_overlay_equals_the_lane_value_the_engine_applies() {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let synth = DeviceKey("synth".into());
    let p = |tick, value, curve| AutomationPoint { tick, value, curve };
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
        Command::Song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
        Command::Song(SongCommand::AddAutomationLane {
            device: synth.clone(),
            param: "filter.cutoff".into(),
        }),
        Command::Song(SongCommand::AddClip {
            device: synth.clone(),
            lane: 1,
            clip: Clip {
                id: ClipId(0),
                start: PPQ,
                length: 6 * PPQ,
                offset: PPQ / 2,
                loop_length: Some(2 * PPQ),
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Automation(vec![
                    p(0, 300.0, Curve::Linear),
                    p(PPQ, 3000.0, Curve::Step),
                    p(PPQ + 100, 800.0, Curve::Linear),
                    p(2 * PPQ - 1, 200.0, Curve::Step),
                ]),
            },
        }),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    let doc = d.doc();
    let input = song_input(doc, &registry, |k| {
        (k == &synth).then_some(DeviceInstanceId(1))
    });
    let lane = &input.tracks[0].automation[0];
    assert!(!lane.points.is_empty());
    let shown =
        |tick: f64| doc.automation_values_at(tick)[&(synth.clone(), "filter.cutoff".to_string())];
    let mut tick = 0.0;
    while tick < (9 * PPQ) as f64 {
        let engine = atomik_engine::lane_value(&lane.points, tick);
        assert_eq!(shown(tick).to_bits(), engine.to_bits(), "tick {tick}");
        tick += 37.5;
    }
}
