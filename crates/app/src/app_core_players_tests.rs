//! Pattern data reaching the engine as one swap per
//! edit, the sequencer grid editor pane, the player meter feed, the shared pattern clipboard,
//! auditions through a player, and the live twin of the `player-demo` bounce.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_device_sdk::PlayerData;
use atomik_model::{
    Cable, Command, DeviceKey, PatternDoc, SeqResolution, SocketRef, StepDoc, empty_doc,
};
use atomik_ui::UiAction;

const SEQ: &str = "atomik.step-sequencer";
const ARP: &str = "atomik.arpeggiator";
const SYNTH: &str = "atomik.poly-synth";
const LATENCY: usize = atomik_core::BATCH;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

fn busy(first_key: u8) -> PatternDoc {
    let mut p = PatternDoc::new("Busy");
    p.length = 8;
    p.resolution = SeqResolution::Eighth;
    p.steps[0] = StepDoc {
        on: true,
        key: first_key,
        ..StepDoc::default()
    };
    p
}

fn set_pattern<S: EngineSink>(c: &mut AppCore<S>, device: &str, index: u8, p: PatternDoc) {
    c.apply_command(Command::SetPattern {
        device: key(device),
        index,
        pattern: p,
        merge: None,
    })
    .unwrap();
}

/// The `PlayerData` calls of the recording, as `(engine id, first step key of pattern 0)`.
fn data_calls(calls: &[SinkCall]) -> Vec<(u32, Option<u8>)> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::PlayerData { id, data } => Some((
                id.0,
                data.as_ref().map(|d| match &**d {
                    PlayerData::StepSeq(s) => s.patterns[0].steps[0].key,
                    PlayerData::CvSeq(s) => s.notes.patterns[0].steps[0].key,
                    PlayerData::Drums(d) => d.patterns[0].lanes[0].key,
                }),
            )),
            _ => None,
        })
        .collect()
}

fn cable(from: (&str, &str), to: (&str, &str)) -> Command {
    let r = |(d, s): (&str, &str)| SocketRef {
        device: key(d),
        socket: s.to_string(),
    };
    Command::Connect(Cable {
        from: r(from),
        to: r(to),
    })
}

// ------------------------------------------------------------------ pattern data to the engine

#[test]
fn a_new_sequencer_gets_its_data_right_after_its_insert_and_others_get_none() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    add(&mut c, ARP, "arp");
    add(&mut c, SYNTH, "synth");
    let calls = c.sink_mut().take();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { type_id, .. } if *type_id == SEQ))
        .unwrap();
    assert!(
        matches!(
            &calls[insert + 1],
            SinkCall::PlayerData { data: Some(_), .. }
        ),
        "insert, then data: {:?}",
        calls[insert + 1]
    );
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::PlayerData { .. }))
            .count(),
        1,
        "only the sequencer takes pattern data"
    );
}

#[test]
fn every_pattern_edit_undo_and_redo_is_one_swap() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.sink_mut().take();
    set_pattern(&mut c, "seq", 0, busy(50));
    assert_eq!(data_calls(&c.sink_mut().take()), vec![(1, Some(50))]);
    assert!(c.undo());
    assert_eq!(data_calls(&c.sink_mut().take()), vec![(1, Some(60))]);
    assert!(c.redo());
    assert_eq!(data_calls(&c.sink_mut().take()), vec![(1, Some(50))]);
    // The other pattern commands reach the engine too.
    c.apply_command(Command::AddPattern { device: key("seq") })
        .unwrap();
    assert_eq!(data_calls(&c.sink_mut().take()).len(), 1);
    c.apply_command(Command::DuplicatePattern {
        device: key("seq"),
        index: 0,
    })
    .unwrap();
    assert_eq!(data_calls(&c.sink_mut().take()).len(), 1);
    c.apply_command(Command::RemovePattern {
        device: key("seq"),
        index: 2,
    })
    .unwrap();
    assert_eq!(data_calls(&c.sink_mut().take()).len(), 1);
}

#[test]
fn a_batch_of_pattern_edits_is_one_swap_per_op() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.sink_mut().take();
    set_pattern(&mut c, "seq", 0, busy(52));
    let calls = c.sink_mut().take();
    assert_eq!(calls.len(), 1, "{calls:?}");
}

#[test]
fn removing_and_undoing_the_removal_sends_the_data_again_after_the_insert() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    set_pattern(&mut c, "seq", 0, busy(55));
    c.apply_command(Command::RemoveDevice { key: key("seq") })
        .unwrap();
    c.sink_mut().take();
    assert!(c.undo());
    let calls = c.sink_mut().take();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { .. }))
        .unwrap();
    assert!(matches!(
        &calls[insert + 1],
        SinkCall::PlayerData { data: Some(_), .. }
    ));
    assert_eq!(data_calls(&calls)[0].1, Some(55), "the edited pattern");
}

#[test]
fn opening_a_project_and_restarting_send_every_sequencers_data_in_restart_order() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "a");
    add(&mut c, SEQ, "b");
    set_pattern(&mut c, "b", 0, busy(61));
    c.sink_mut().take();
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    assert_eq!(
        data_calls(&calls),
        vec![(1, Some(60)), (2, Some(61))],
        "{calls:?}"
    );
    for id in [1, 2] {
        let insert = calls
            .iter()
            .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if i.0 == id))
            .unwrap();
        assert!(matches!(&calls[insert + 1], SinkCall::PlayerData { .. }));
    }
}

#[test]
fn the_default_pattern_is_sent_for_a_sequencer_without_edits() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    let calls = c.sink_mut().take();
    let data = calls
        .iter()
        .find_map(|x| match x {
            SinkCall::PlayerData { data, .. } => data.clone(),
            _ => None,
        })
        .unwrap();
    let PlayerData::StepSeq(s) = &*data else {
        panic!("a step sequencer gets StepSeq data")
    };
    assert_eq!(s.count, 1);
    assert_eq!(s.patterns[0].length, 16);
}

#[test]
fn a_cv_sequencer_gets_cv_data_and_every_curve_edit_swaps_it() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "atomik.cv-sequencer", "cv");
    let first = c.sink_mut().take();
    let data = first
        .iter()
        .find_map(|x| match x {
            SinkCall::PlayerData { data, .. } => data.clone(),
            _ => None,
        })
        .unwrap();
    assert!(matches!(&*data, PlayerData::CvSeq(_)));
    let mut rows = atomik_model::CurvePatternDoc::default();
    rows.rows[0][3].value = 70;
    c.apply_command(Command::SetCurves {
        device: key("cv"),
        index: 0,
        curves: rows,
        merge: None,
    })
    .unwrap();
    let calls = c.sink_mut().take();
    let PlayerData::CvSeq(d) = calls
        .iter()
        .find_map(|x| match x {
            SinkCall::PlayerData { data, .. } => data.clone(),
            _ => None,
        })
        .expect("the curve edit re-sends the data")
        .as_ref()
        .clone()
    else {
        panic!("CvSeq data")
    };
    assert_eq!(d.curves[0][0][3].value, 70);
    // The grid editor opens on it.
    c.handle_action(UiAction::OpenSeqEditor { device: key("cv") });
    assert_eq!(c.seq_editor_device(), Some(&key("cv")));
    assert!(c.seq_editor().has_curve_lanes());
}

// ------------------------------------------------------------------ the editor pane

#[test]
fn open_seq_editor_opens_on_a_sequencer_only_and_mirrors_the_panel_selection() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    add(&mut c, SYNTH, "synth");
    c.apply_command(Command::AddPattern { device: key("seq") })
        .unwrap();
    c.handle_action(UiAction::OpenSeqEditor {
        device: key("synth"),
    });
    assert!(c.seq_editor_device().is_none(), "a synth has no editor");
    c.view_mut().set_edited_pattern(&key("seq"), 1);
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    assert_eq!(c.seq_editor_device(), Some(&key("seq")));
    assert_eq!(
        c.seq_editor().pattern_index(),
        1,
        "starts on the panel's pattern"
    );
    c.handle_action(UiAction::CloseSeqEditor);
    assert!(c.seq_editor_device().is_none());
}

#[test]
fn open_seq_editor_default_takes_the_selected_sequencer_else_the_first() {
    let mut c = AppCore::new(RecordingSink::new());
    assert!(!c.open_seq_editor_default(), "an empty rack has none");
    add(&mut c, SYNTH, "synth");
    assert!(!c.open_seq_editor_default(), "a synth is not a sequencer");
    add(&mut c, SEQ, "first");
    add(&mut c, SEQ, "second");
    // A selected synth does not count: the first sequencer of the rack opens.
    c.select(Some(key("synth")));
    assert!(c.open_seq_editor_default());
    assert_eq!(c.seq_editor_device(), Some(&key("first")));
    c.close_seq_editor();
    // A selected sequencer wins.
    c.select(Some(key("second")));
    assert!(c.open_seq_editor_default());
    assert_eq!(c.seq_editor_device(), Some(&key("second")));
}

#[test]
fn select_pattern_is_mirrored_between_the_panel_and_the_editor() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    for _ in 0..2 {
        c.apply_command(Command::AddPattern { device: key("seq") })
            .unwrap();
    }
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    c.handle_action(UiAction::SelectPattern {
        device: key("seq"),
        index: 2,
    });
    assert_eq!(c.seq_editor().pattern_index(), 2);
    assert_eq!(c.view().edited_pattern(&key("seq")), Some(2));
    // A selection on a device that is not the one open in the editor leaves the editor alone.
    add(&mut c, SEQ, "other");
    c.handle_action(UiAction::SelectPattern {
        device: key("other"),
        index: 0,
    });
    assert_eq!(c.seq_editor().pattern_index(), 2);
}

#[test]
fn the_editor_closes_when_its_device_goes_away_and_on_a_new_document() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    c.apply_command(Command::RemoveDevice { key: key("seq") })
        .unwrap();
    assert!(c.seq_editor_device().is_none());
    add(&mut c, SEQ, "seq2");
    c.handle_action(UiAction::OpenSeqEditor {
        device: key("seq2"),
    });
    c.new_document();
    assert!(c.seq_editor_device().is_none());
}

#[test]
fn the_editors_replace_each_other() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    add(&mut c, "atomik.sampler", "smp");
    add(&mut c, "atomik.drum-machine", "drm");
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    c.handle_action(UiAction::OpenSampleEditor(key("smp")));
    assert!(c.seq_editor_device().is_none());
    assert!(c.sample_editor_device().is_some());
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    assert!(
        c.sample_editor_device().is_none(),
        "the sequencer editor took the pane"
    );
    c.handle_action(UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    });
    assert!(c.seq_editor_device().is_none());
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    assert!(c.step_editor_device().is_none());
}

#[test]
fn editor_events_edit_the_document_and_reach_the_engine() {
    use atomik_ui::{Modifiers, PointerButton, UiEvent};
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    c.seq_editor_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1100.0, 640.0));
    c.sink_mut().take();
    let at = c.seq_editor().note_cell_center(3, 64);
    for event in [
        UiEvent::PointerMove {
            pos: at,
            modifiers: Modifiers::NONE,
        },
        UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    ] {
        c.handle_pane_event(Pane::SeqEditor, &event);
    }
    let step = c
        .doc()
        .device(&key("seq"))
        .unwrap()
        .steps
        .as_ref()
        .unwrap()
        .patterns[0]
        .steps[3];
    assert!(step.on && step.key == 64, "{step:?}");
    assert_eq!(
        data_calls(&c.sink_mut().take()).len(),
        1,
        "one swap for the click"
    );
}

// ------------------------------------------------------------------ meters

#[test]
fn the_sequencers_published_cursor_reaches_the_panel_and_the_editor() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    let id = c.device_id(&key("seq")).unwrap();
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.0, 5.0, 1.0], 1));
    c.frame();
    assert_eq!(c.view().step_cursor(&key("seq")), Some(5));
    assert_eq!(c.seq_editor().cursor(), Some(5));
    // The same sequence number is not applied twice.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.0, 9.0, 1.0], 1));
    c.frame();
    assert_eq!(c.view().step_cursor(&key("seq")), Some(5));
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.0, 9.0, 1.0], 2));
    c.frame();
    assert_eq!(c.view().step_cursor(&key("seq")), Some(9));
    assert_eq!(c.seq_editor().cursor(), Some(9));
}

#[test]
fn the_arpeggiators_held_keys_reach_its_panel() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, ARP, "arp");
    let id = c.device_id(&key("arp")).unwrap();
    c.sink_mut().device_values.insert(
        id,
        (vec![3.0, 60.0, 64.0, 67.0, 72.0, 0.0, 0.0, 0.0, 0.0], 1),
    );
    c.frame();
    let (held, next) = c.view().arp_display(&key("arp")).unwrap();
    assert_eq!(held, 3);
    assert_eq!(&next[..4], &[60, 64, 67, 72]);
}

// ------------------------------------------------------------------ clipboard

#[test]
fn the_pattern_clipboard_is_shared_between_the_panel_and_the_editor() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    c.handle_action(UiAction::OpenSeqEditor { device: key("seq") });
    c.view_mut().set_pattern_clipboard(Some(busy(70)));
    c.after_view_change();
    assert_eq!(c.seq_editor().clipboard().map(|p| p.steps[0].key), Some(70));
    c.seq_editor_mut().set_clipboard(Some(busy(71)));
    c.after_view_change();
    assert_eq!(
        c.view().pattern_clipboard().map(|p| p.steps[0].key),
        Some(71)
    );
}

// ------------------------------------------------------------------ audition

#[test]
fn auditioning_in_the_editor_plays_the_device_the_sequencer_is_cabled_to() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SEQ, "seq");
    add(&mut c, SYNTH, "synth");
    c.apply_command(cable(("seq", "notes_out"), ("synth", "notes_in")))
        .unwrap();
    c.sink_mut().take();
    c.handle_action(UiAction::AuditionVelocity {
        device: key("seq"),
        key: 64,
        velocity: 90,
        on: true,
    });
    c.handle_action(UiAction::AuditionVelocity {
        device: key("seq"),
        key: 64,
        velocity: 90,
        on: false,
    });
    let calls = c.sink_mut().take();
    let synth = c.device_id(&key("synth")).unwrap();
    let notes: Vec<_> = calls
        .iter()
        .filter_map(|x| match x {
            SinkCall::LiveNote { target, kind } => Some((target.0, *kind)),
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 2, "{calls:?}");
    assert!(notes.iter().all(|(id, _)| *id == synth));
    // An uncabled sequencer has nothing to audition on.
    add(&mut c, SEQ, "lonely");
    c.sink_mut().take();
    c.handle_action(UiAction::Audition {
        device: key("lonely"),
        key: 60,
        on: true,
    });
    assert!(
        !c.sink_mut()
            .take()
            .iter()
            .any(|x| matches!(x, SinkCall::LiveNote { .. })),
        "a sequencer ignores live notes; nothing is downstream"
    );
}

// ------------------------------------------------------------------ live equals bounce

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/player-demo/player-demo.atomik")
}

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
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

fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
    core.transport_action(atomik_ui::TransportAction::Play);
    core.sink_mut().render_blocks(frames + LATENCY, block)
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

/// The player demo (sequencer bass line with swing, ties and seeded probability steps; an
/// arpeggiated chord; the pitch CV on a filter) played through the app's live path gives the
/// bounce's samples, bit for bit, delayed by the engine batch, whatever the host block.
#[test]
fn the_live_path_plays_the_player_demo_exactly_like_the_bounce() {
    let reference = bounce_of(live_core().doc());
    let frames = reference[0].len();
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    for block in [64, 480, 512] {
        let mut core = live_core();
        let live = play(&mut core, frames, block);
        assert!(same_bits(&live, &reference), "block {block}");
    }
}

/// An edit, an undo and a redo made through the app reach the live engine like the bounce of
/// the same document: the data swap is applied before the notes it changes.
#[test]
fn live_equals_bounce_after_a_pattern_edit_an_undo_and_a_redo() {
    let mut core = live_core();
    let mut edited = core
        .doc()
        .devices
        .iter()
        .find(|d| d.key == key("seq"))
        .unwrap()
        .steps
        .clone()
        .unwrap()
        .patterns[0]
        .clone();
    edited.steps[0].key = 52;
    edited.swing = 40;
    set_pattern(&mut core, "seq", 0, edited);
    let after_edit = core.doc().clone();
    let reference = bounce_of(&after_edit);
    let frames = reference[0].len();
    let live = play(&mut core, frames, 512);
    assert!(same_bits(&live, &reference), "edit");
    let mut core = live_core();
    let original = bounce_of(core.doc());
    let mut p = core
        .doc()
        .devices
        .iter()
        .find(|d| d.key == key("seq"))
        .unwrap()
        .steps
        .clone()
        .unwrap()
        .patterns[0]
        .clone();
    p.steps[0].key = 52;
    set_pattern(&mut core, "seq", 0, p);
    assert!(core.undo());
    let live = play(&mut core, original[0].len(), 480);
    assert!(
        same_bits(&live, &original),
        "undo returns to the original sound"
    );
    let mut core = live_core();
    let mut p = core
        .doc()
        .devices
        .iter()
        .find(|d| d.key == key("seq"))
        .unwrap()
        .steps
        .clone()
        .unwrap()
        .patterns[0]
        .clone();
    p.steps[0].key = 52;
    set_pattern(&mut core, "seq", 0, p);
    assert!(core.undo());
    assert!(core.redo());
    let reference = bounce_of(&core.doc().clone());
    let live = play(&mut core, reference[0].len(), 64);
    assert!(same_bits(&live, &reference), "redo");
}

/// A pattern switch by the `pattern` param plays the same live and in the bounce.
#[test]
fn live_equals_bounce_with_the_second_pattern_playing() {
    let mut core = live_core();
    core.apply_command(Command::SetParam {
        key: key("seq"),
        param: "pattern".into(),
        value: 2.0,
        merge: None,
    })
    .unwrap();
    let reference = bounce_of(&core.doc().clone());
    assert!(
        reference != bounce_of(live_core().doc()),
        "the second pattern sounds different"
    );
    let live = play(&mut core, reference[0].len(), 512);
    assert!(same_bits(&live, &reference));
}

#[test]
fn a_sequencer_cabled_into_an_arpeggiator_plays_live_like_the_bounce() {
    let mut core = live_core();
    core.apply_command(Command::Disconnect {
        to: SocketRef {
            device: key("bass"),
            socket: "notes_in".into(),
        },
    })
    .unwrap();
    // The sequencer's notes go through the arpeggiator (which also holds the song's chord)
    // to the second synth.
    core.apply_command(cable(("seq", "notes_out"), ("arp", "notes_in")))
        .unwrap_or_else(|e| panic!("{e}"));
    core.apply_command(Command::SetParam {
        key: key("arp"),
        param: "run".into(),
        value: 1.0,
        merge: None,
    })
    .unwrap();
    let reference = bounce_of(&core.doc().clone());
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    let live = play(&mut core, reference[0].len(), 512);
    assert!(same_bits(&live, &reference));
}

// ------------------------------------------------------------------ player stacks

fn stack_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "synth");
    c.apply_command(Command::Song(atomik_model::SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(atomik_model::SongCommand::SetTrackFlags {
        device: key("synth"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    c.select(Some(key("synth")));
    c
}

fn create_player<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, selected: &str) -> DeviceKey {
    let before: Vec<DeviceKey> = c.doc().devices.iter().map(|d| d.key.clone()).collect();
    c.apply_command(Command::Workflow(
        atomik_model::WorkflowCommand::CreatePlayer {
            type_id: type_id.into(),
            target: atomik_model::CreateTarget::Selected(key(selected)),
            options: atomik_model::CreateOptions::default(),
        },
    ))
    .unwrap();
    c.doc()
        .devices
        .iter()
        .map(|d| d.key.clone())
        .find(|k| !before.contains(k))
        .expect("a new device")
}

#[test]
fn live_keyboard_on_an_armed_track_plays_through_the_stack() {
    let mut c = stack_core();
    let synth = c.device_id(&key("synth")).unwrap();
    assert_eq!(c.notes_target().unwrap().0, synth);
    let arp = create_player(&mut c, ARP, "synth");
    let arp_id = c.device_id(&arp).unwrap();
    // The new player is selected, the track is still armed: the keys reach the player.
    assert_eq!(c.view().selected(), Some(&arp));
    let (id, socket) = c.notes_target().unwrap();
    assert_eq!(id, arp_id);
    assert_eq!(
        socket,
        atomik_render::project::notes_input(c.registry.get(ARP).unwrap().descriptor).unwrap()
    );
    // Whatever is selected, an armed track plays through its stack.
    c.select(Some(key("synth")));
    assert_eq!(c.notes_target().unwrap().0, arp_id);
    // Auditioning the instrument goes through the stack too.
    c.sink_mut().take();
    c.handle_action(UiAction::Audition {
        device: key("synth"),
        key: 60,
        on: true,
    });
    let targets: Vec<_> = c
        .sink_mut()
        .take()
        .iter()
        .filter_map(|x| match x {
            SinkCall::LiveNote { target, .. } => Some(target.0),
            _ => None,
        })
        .collect();
    assert_eq!(targets, vec![arp_id]);
}

#[test]
fn a_stale_entry_plays_the_track_device_live() {
    let mut c = stack_core();
    let arp = create_player(&mut c, ARP, "synth");
    let mut doc = c.doc().clone();
    doc.devices.retain(|d| d.key != arp);
    doc.cables.clear();
    // A hand-edited project: the entry names a device that is not there.
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    assert_eq!(
        c.doc().song.track(&key("synth")).unwrap().notes_to.as_ref(),
        Some(&arp)
    );
    c.select(Some(key("synth")));
    let synth = c.device_id(&key("synth")).unwrap();
    assert_eq!(c.notes_target().unwrap().0, synth);
    // An entry that is there but takes no notes (an effect) plays the track's device too.
    let mut doc = c.doc().clone();
    add(&mut c, "atomik.stereo-delay", "delay");
    doc.devices
        .push(c.doc().device(&key("delay")).unwrap().clone());
    doc.song
        .tracks
        .iter_mut()
        .find(|t| t.device == key("synth"))
        .unwrap()
        .notes_to = Some(key("delay"));
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    c.select(Some(key("synth")));
    let synth = c.device_id(&key("synth")).unwrap();
    assert_eq!(c.notes_target().unwrap().0, synth);
}

#[test]
fn the_song_sent_to_the_engine_enters_at_the_player() {
    let mut c = stack_core();
    c.apply_command(Command::Song(atomik_model::SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: 4 * atomik_core::PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: None,
            content: atomik_model::ClipContent::Notes(vec![atomik_model::Note::new(
                0,
                atomik_core::PPQ,
                60,
                0.8,
            )]),
        },
    }))
    .unwrap();
    c.sink_mut().take();
    let arp = create_player(&mut c, ARP, "synth");
    let arp_id = c.device_id(&arp).unwrap();
    let calls = c.sink_mut().take();
    let song = calls
        .iter()
        .rev()
        .find_map(|x| match x {
            SinkCall::Song(s) => Some(s),
            _ => None,
        })
        .expect("the song is sent again when the entry moves");
    let entry: Vec<_> = song.tracks.iter().filter(|t| t.device == arp_id).collect();
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0].notes.len(), 1);
    let synth = c.device_id(&key("synth")).unwrap();
    assert!(
        song.tracks
            .iter()
            .filter(|t| t.device == synth)
            .all(|t| t.notes.is_empty())
    );
}

#[test]
fn creating_a_player_selects_it_and_names_the_device_it_sits_above() {
    let mut c = stack_core();
    let arp = create_player(&mut c, ARP, "synth");
    let name = c.doc().device(&arp).unwrap().name.clone();
    let synth_name = c.doc().device(&key("synth")).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {name} above {synth_name}").as_str())
    );
    // The next one goes on the same stack, above the first.
    let second = create_player(&mut c, SEQ, &arp.0);
    let second_name = c.doc().device(&second).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {second_name} above {name}").as_str())
    );
    assert_eq!(c.view().selected(), Some(&second));
}
