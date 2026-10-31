//! Tempo ramps and meter changes in the app core: the bar keys follow the meter map, the tempo
//! lane's "Set Tempo..." reaches the transport bar's entry, and the live path plays a ramped
//! project exactly like the bounce (the twin tests of the other devices, with a tempo map that
//! ramps and a meter that changes).

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_model::{MeterEvent, TempoEvent, empty_doc};

const BAR: i64 = 4 * PPQ;

fn core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::default(), empty_doc(), None)
}

fn apply<S: crate::sink::EngineSink>(core: &mut AppCore<S>, c: SongCommand) {
    core.apply_command(Command::Song(c)).unwrap();
}

#[test]
fn the_bar_keys_follow_the_meter_map() {
    let mut c = core();
    apply(
        &mut c,
        SongCommand::SetMeterMap(vec![
            MeterEvent::new(1, 4, 4),
            MeterEvent::new(3, 3, 4),
            MeterEvent::new(5, 6, 8),
        ]),
    );
    // Bars: 0, 4 PPQ, 8 PPQ, 11 PPQ, 14 PPQ (then 6/8 bars of 3 PPQ).
    let lines = [0, BAR, 2 * BAR, 2 * BAR + 3 * PPQ, 2 * BAR + 6 * PPQ];
    let mut seen = vec![c.position()];
    for _ in 0..4 {
        c.locate_bars(1);
        seen.push(c.position());
    }
    assert_eq!(seen, lines);
    c.locate_bars(1);
    assert_eq!(c.position(), 2 * BAR + 9 * PPQ, "6/8 bars are 3 PPQ long");
    // Back: from a bar line to the previous one, from the middle of a bar to its start.
    c.locate_bars(-1);
    assert_eq!(c.position(), 2 * BAR + 6 * PPQ);
    c.transport_action(atomik_ui::TransportAction::Locate(2 * BAR + 4 * PPQ));
    c.locate_bars(-1);
    assert_eq!(c.position(), 2 * BAR + 3 * PPQ);
    c.locate_bars(-3);
    assert_eq!(c.position(), 0);
    c.locate_bars(-1);
    assert_eq!(c.position(), 0, "never before the start");
}

#[test]
fn set_tempo_on_a_point_starts_the_transport_entry_for_that_event() {
    let mut c = core();
    apply(
        &mut c,
        SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 100.0),
            TempoEvent::step(2 * BAR, 150.0),
        ]),
    );
    c.handle_action(UiAction::EditTempoPoint(1));
    assert!(c.transport_bar().is_editing_tempo());
    assert_eq!(c.transport_bar().tempo_text(), Some("150"));
}

#[test]
fn the_engine_song_carries_the_ramps_and_the_meter_map() {
    let mut c = core();
    c.sink_mut().take();
    apply(
        &mut c,
        SongCommand::SetTempoMap(vec![
            TempoEvent {
                tick: 0,
                bpm: 90.0,
                ramp: true,
            },
            TempoEvent::step(2 * BAR, 150.0),
        ]),
    );
    apply(
        &mut c,
        SongCommand::SetMeterMap(vec![MeterEvent::new(1, 4, 4), MeterEvent::new(2, 7, 8)]),
    );
    let calls = c.sink_mut().take();
    let input = calls
        .iter()
        .rev()
        .find_map(|c| match c {
            SinkCall::Song(s) => Some(s.clone()),
            _ => None,
        })
        .expect("a song was sent");
    assert_eq!(input.tempo.len(), 2);
    assert!(input.tempo[0].ramp && !input.tempo[1].ramp);
    assert_eq!(
        input.meter,
        vec![MeterEvent::new(1, 4, 4), MeterEvent::new(2, 7, 8)]
    );
}

// ------------------------------------------------------------------ live equals bounce

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/drum-demo/drum-demo.atomik")
}

/// The engine's command FIFO delays the live transport by one batch.
const LATENCY: usize = atomik_core::BATCH;

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    let root = demo_file().parent().unwrap().to_path_buf();
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        Some(&root),
    )
    .unwrap()
    .channels
}

fn live_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    core
}

fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
    live.iter().zip(bounce).all(|(a, b)| {
        a[LATENCY..]
            .iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits())
            && a[..LATENCY].iter().all(|x| *x == 0.0)
    })
}

/// The drum demo under a tempo map that ramps (up, then down) and a changing meter, played
/// live through the app's path, is the bounce bit for bit at every host block, and the ramp
/// changes the sound (it is not the render of the constant tempo).
#[test]
fn the_live_path_plays_a_ramped_project_exactly_like_the_bounce() {
    let constant = bounce_of(live_core().doc());
    let map = vec![
        TempoEvent {
            tick: 0,
            bpm: 90.0,
            ramp: true,
        },
        TempoEvent::step(2 * BAR, 150.0),
        TempoEvent {
            tick: 3 * BAR + 100,
            bpm: 150.0,
            ramp: true,
        },
        TempoEvent::step(5 * BAR, 70.0),
    ];
    let meter = vec![MeterEvent::new(1, 4, 4), MeterEvent::new(3, 3, 4)];
    let mut doc = live_core().doc().clone();
    doc.song_mut().tempo = map.clone();
    doc.tempo_bpm = map[0].bpm;
    doc.song_mut().meter = meter.clone();
    let reference = bounce_of(&doc);
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    assert_ne!(
        reference[0].len(),
        constant[0].len(),
        "the ramp moves the song end"
    );
    let frames = reference[0].len();
    for block in [64, 480, 512] {
        let mut core = live_core();
        apply(&mut core, SongCommand::SetTempoMap(map.clone()));
        apply(&mut core, SongCommand::SetMeterMap(meter.clone()));
        core.transport_action(atomik_ui::TransportAction::Play);
        let live = core.sink_mut().render_blocks(frames + LATENCY, block);
        assert!(same_bits(&live, &reference), "block {block}");
    }
}
