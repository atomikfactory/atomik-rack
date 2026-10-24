//! The creation chain in the app core: creating an
//! instrument makes its channel and its armed track and selects it so the live keyboard plays
//! it, the tracks an earlier auto-route armed are disarmed, the effect rules reach the
//! selected channel, and every operation is one undo step.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{
    CreateOptions, CreateSource, CreateTarget, DeviceKey, RackSlot, SongCommand, WorkflowCommand,
    empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

fn text(c: &AppCore<RecordingSink>) -> String {
    atomik_model::save_project(c.doc())
}

fn instrument(c: &mut AppCore<RecordingSink>, type_id: &str) -> Result<(), ModelError> {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(type_id.into()),
        key: None,
        options: CreateOptions::default(),
    }))
}

fn effect(c: &mut AppCore<RecordingSink>, type_id: &str, target: CreateTarget) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(type_id.into()),
        target,
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
}

fn armed(c: &AppCore<RecordingSink>) -> Vec<String> {
    c.doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.armed)
        .map(|t| t.device.0.clone())
        .collect()
}

fn target_name(c: &AppCore<RecordingSink>) -> Option<String> {
    c.notes_target_device().map(|d| d.key.0.clone())
}

#[test]
fn creating_an_instrument_selects_it_and_the_keyboard_plays_it() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.doc().devices[0].key.clone();
    assert_eq!(c.doc().devices[0].type_id, SYNTH);
    assert!(c.doc().console.channel_of(&synth).is_some());
    assert_eq!(armed(&c), vec![synth.0.clone()]);
    assert_eq!(
        c.view().selected(),
        Some(&synth),
        "the new device is selected"
    );
    assert_eq!(target_name(&c), Some(synth.0.clone()));
    assert!(
        c.last_rejection().is_some_and(|m| m.contains("Created")),
        "a message names what was created: {:?}",
        c.last_rejection()
    );
}

#[test]
fn the_whole_chain_is_one_undo_step_and_redo_is_byte_identical() {
    let mut c = core();
    let start = text(&c);
    instrument(&mut c, SYNTH).unwrap();
    let created = text(&c);
    assert!(c.undo());
    assert_eq!(text(&c), start);
    assert!(!c.undo(), "nothing else to undo");
    assert!(c.redo());
    assert_eq!(text(&c), created);
}

#[test]
fn a_new_instrument_disarms_only_the_tracks_an_earlier_auto_route_armed() {
    let mut c = core();
    // A track the user armed by hand.
    c.apply_command(Command::AddDevice {
        index: 0,
        type_id: SYNTH.into(),
        key: Some(key("hand")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("hand"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("hand"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    instrument(&mut c, SYNTH).unwrap();
    let first = c.view().selected().unwrap().clone();
    instrument(&mut c, SYNTH).unwrap();
    let second = c.view().selected().unwrap().clone();
    assert_ne!(first, second);
    let mut want = vec!["hand".to_string(), second.0.clone()];
    want.sort();
    let mut got = armed(&c);
    got.sort();
    assert_eq!(
        got, want,
        "the auto-armed first track was disarmed, the user's stays"
    );
    assert_eq!(target_name(&c), Some(second.0.clone()));
    // Undo restores the first track's arming (history), and creating again disarms it again.
    assert!(c.undo());
    assert!(armed(&c).contains(&first.0));
    assert!(c.redo());
    assert!(!armed(&c).contains(&first.0));
}

#[test]
fn a_new_document_forgets_which_tracks_were_auto_armed() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    c.new_document();
    // Same key in a fresh document, armed by the user this time.
    c.apply_command(Command::AddDevice {
        index: 0,
        type_id: SYNTH.into(),
        key: Some(key("poly-synth")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("poly-synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("poly-synth"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    instrument(&mut c, SYNTH).unwrap();
    assert!(
        armed(&c).contains(&"poly-synth".to_string()),
        "armed by the user: left alone"
    );
}

#[test]
fn opening_a_project_forgets_which_tracks_were_auto_armed() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let first = c.view().selected().unwrap().clone();
    // Reopen the saved text: the same key and the same armed track, but nobody auto-armed it
    // in this session of the document.
    let saved = text(&c);
    c.open_str(&saved, None).unwrap();
    assert!(armed(&c).contains(&first.0));
    instrument(&mut c, SYNTH).unwrap();
    assert!(
        armed(&c).contains(&first.0),
        "Open clears the session list: the first track keeps its arming"
    );
    assert_eq!(armed(&c).len(), 2);
}

#[test]
fn auto_route_disarms_the_tracks_an_earlier_auto_route_armed() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let first = c.view().selected().unwrap().clone();
    // A second synth added the old way has neither a track nor a selection.
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.into(),
        key: Some(key("plain")),
    })
    .unwrap();
    assert!(armed(&c).contains(&first.0));
    c.apply_command(Command::Workflow(WorkflowCommand::AutoRoute {
        devices: vec![key("plain")],
        options: CreateOptions::default(),
    }))
    .unwrap();
    assert_eq!(armed(&c), vec!["plain".to_string()], "the keyboard moved");
}

#[test]
fn the_status_message_names_what_was_made() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.view().selected().unwrap().clone();
    let name = c.doc().device(&synth).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {name} with a mixer channel and an armed song track").as_str())
    );
    effect(&mut c, DELAY, CreateTarget::Selected(synth.clone()));
    let delay = c.view().selected().unwrap().clone();
    let delay_name = c.doc().device(&delay).unwrap().name.clone();
    let channel = c.doc().console.channel_of(&synth).unwrap().channel.clone();
    let channel_name = c.doc().device(&channel).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {delay_name} on insert 1 of {channel_name}").as_str())
    );
    effect(&mut c, DELAY, CreateTarget::Selected(channel.clone()));
    let second = c.view().selected().unwrap().clone();
    let second_name = c.doc().device(&second).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {second_name} on insert 2 of {channel_name}").as_str())
    );
    // A free device: "after".
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: DELAY.into(),
        key: Some(key("free")),
    })
    .unwrap();
    effect(&mut c, DELAY, CreateTarget::Selected(key("free")));
    let after = c.view().selected().unwrap().clone();
    let after_name = c.doc().device(&after).unwrap().name.clone();
    let free_name = c.doc().device(&key("free")).unwrap().name.clone();
    assert_eq!(
        c.last_rejection(),
        Some(format!("Created {after_name} after {free_name}").as_str())
    );
}

#[test]
fn the_auto_route_message_counts_the_devices() {
    let mut c = core();
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
        c.apply_command(Command::Workflow(WorkflowCommand::AutoRoute {
            devices: vec![key(k)],
            options: CreateOptions::default(),
        }))
        .unwrap();
        assert_eq!(c.last_rejection(), Some("Auto-routed 1 device"));
    }
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: DELAY.into(),
        key: Some(key("fx1")),
    })
    .unwrap();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: DELAY.into(),
        key: Some(key("fx2")),
    })
    .unwrap();
    c.apply_command(Command::Workflow(WorkflowCommand::AutoRoute {
        devices: vec![key("fx1"), key("fx2")],
        options: CreateOptions::default(),
    }))
    .unwrap();
    assert_eq!(c.last_rejection(), Some("Auto-routed 2 devices"));
}

#[test]
fn route_false_adds_the_device_only_and_leaves_the_selection_alone() {
    let mut c = core();
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions {
            route: false,
            ..CreateOptions::default()
        },
    }))
    .unwrap();
    assert!(c.doc().song.tracks.is_empty());
    assert!(armed(&c).is_empty());
    assert_eq!(c.view().selected(), None, "the plain add does not select");
    assert_eq!(c.last_rejection(), None, "and says nothing");
}

#[test]
fn an_effect_lands_in_the_insert_of_the_selected_device_channel_and_is_selected() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.view().selected().unwrap().clone();
    effect(&mut c, DELAY, CreateTarget::Selected(synth.clone()));
    let channel = c.doc().console.channel_of(&synth).unwrap().channel.clone();
    let chain = atomik_model::insert_chain(c.doc(), &c.registry, &channel);
    assert_eq!(chain.len(), 1);
    assert_eq!(
        c.view().selected(),
        Some(&chain[0]),
        "the effect is selected"
    );
    // The keyboard still goes to the armed instrument track.
    assert_eq!(
        target_name(&c),
        Some(synth.0.clone()),
        "the armed track still plays"
    );
    assert!(
        c.last_rejection().is_some_and(|m| m.contains("insert")),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn an_effect_with_nothing_selected_is_added_and_selected() {
    let mut c = core();
    effect(&mut c, DELAY, CreateTarget::Nothing);
    assert_eq!(c.doc().devices.len(), 1);
    assert_eq!(c.view().selected(), Some(&c.doc().devices[0].key));
}

#[test]
fn auto_route_on_a_selected_instrument_routes_it_and_arms_its_track() {
    let mut c = core();
    // An instrument added the old way has a channel but no track.
    c.apply_command(Command::AddDevice {
        index: 0,
        type_id: SYNTH.into(),
        key: Some(key("lead")),
    })
    .unwrap();
    c.select(Some(key("lead")));
    assert!(
        c.pane_command(Pane::Rack, "rack.auto_route", None)
            .is_some()
    );
    assert_eq!(armed(&c), vec!["lead".to_string()]);
    assert!(c.undo(), "one undo step");
    assert!(c.doc().song.tracks.is_empty());
}

#[test]
fn a_rejected_creation_changes_nothing_and_says_why() {
    let mut c = core();
    let before = text(&c);
    assert!(
        instrument(&mut c, DELAY).is_err(),
        "an effect is not an instrument"
    );
    assert_eq!(text(&c), before);
    assert!(c.last_rejection().is_some());
}

#[test]
fn deleting_through_the_rack_command_bridges_the_chain() {
    let mut c = core();
    for (i, (t, k)) in [(DELAY, "d1"), (DELAY, "d2"), (DELAY, "d3")]
        .into_iter()
        .enumerate()
    {
        c.apply_command(Command::AddDevice {
            index: i,
            type_id: t.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    for (a, b) in [("d1", "d2"), ("d2", "d3")] {
        for side in ["l", "r"] {
            c.apply_command(Command::Connect(atomik_model::Cable {
                from: atomik_model::SocketRef {
                    device: key(a),
                    socket: format!("out_{side}"),
                },
                to: atomik_model::SocketRef {
                    device: key(b),
                    socket: format!("in_{side}"),
                },
            }))
            .unwrap();
        }
    }
    c.select(Some(key("d2")));
    assert!(c.pane_command(Pane::Rack, "edit.delete", None).is_some());
    assert!(c.doc().device(&key("d2")).is_none());
    assert!(
        c.doc()
            .cables
            .iter()
            .any(|x| x.from.device == key("d1") && x.to.device == key("d3"))
    );
}

// ------------------------------------------------------------------ live equals bounce

mod live {
    use super::*;
    use crate::offline_sink::OfflineSink;
    use atomik_core::PPQ;
    use atomik_model::{Clip, ClipContent, ClipId, Note};

    const LATENCY: usize = atomik_core::BATCH;

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

    fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
        live.iter().zip(bounce).all(|(a, b)| {
            a[LATENCY..]
                .iter()
                .zip(b)
                .all(|(x, y)| x.to_bits() == y.to_bits())
                && a[..LATENCY].iter().all(|x| *x == 0.0)
        })
    }

    fn song_clip() -> Clip {
        Clip {
            id: ClipId(0),
            start: 0,
            length: 2 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: None,
            content: ClipContent::Notes(
                [(0, 60u8), (PPQ / 2, 64), (PPQ, 67)]
                    .into_iter()
                    .map(|(tick, k)| Note::new(tick, PPQ / 2, k, 0.8))
                    .collect(),
            ),
        }
    }

    /// A project made only by the creation chain: an instrument (device, channel, track), a
    /// clip, and a delay in the channel's first insert slot.
    fn created() -> AppCore<OfflineSink> {
        let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: 0,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }))
        .unwrap();
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: song_clip(),
        }))
        .unwrap();
        c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type(DELAY.into()),
            target: CreateTarget::Selected(key("lead")),
            key: None,
            options: CreateOptions::default(),
        }))
        .unwrap();
        c
    }

    fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
        core.transport_action(atomik_ui::TransportAction::Play);
        core.sink_mut().render_blocks(frames + LATENCY, block)
    }

    #[test]
    fn a_created_project_plays_live_exactly_like_the_bounce_and_after_undo_and_redo() {
        let reference = bounce_of(created().doc());
        assert!(
            reference[0].iter().any(|s| s.abs() > 1e-3),
            "it makes sound"
        );
        let frames = reference[0].len();
        for block in [64, 480, 512] {
            let mut core = created();
            let live = play(&mut core, frames, block);
            assert!(same_bits(&live, &reference), "block {block}");
        }
        // Undo of the effect and redo: the live engine follows the document.
        let mut core = created();
        assert!(core.undo());
        let without = bounce_of(&core.doc().clone());
        assert!(without != reference, "the delay is audible");
        let live = play(&mut core, without[0].len(), 512);
        assert!(same_bits(&live, &without), "after undo");
        let mut core = created();
        assert!(core.undo());
        assert!(core.redo());
        let live = play(&mut core, frames, 480);
        assert!(same_bits(&live, &reference), "after redo");
        // An engine restart after undoing the whole creation chain and redoing it.
        let mut core = created();
        for _ in 0..3 {
            assert!(core.undo());
        }
        for _ in 0..3 {
            assert!(core.redo());
        }
        core.rebuild_engine();
        let live = play(&mut core, frames, 512);
        assert!(same_bits(&live, &reference), "after a restart");
    }
}

fn instrument_at(c: &mut AppCore<RecordingSink>, slot: RackSlot) -> Result<(), ModelError> {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrumentAt {
        slot,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions::default(),
    }))
}

#[test]
fn creating_at_a_slot_takes_the_same_path_as_creating_at_an_index() {
    let mut c = core();
    instrument_at(
        &mut c,
        RackSlot {
            column: 0,
            index: 0,
        },
    )
    .unwrap();
    let first = c.view().selected().unwrap().clone();
    // A new column: the instrument lands in it, is selected, and its track is armed.
    instrument_at(
        &mut c,
        RackSlot {
            column: 1,
            index: 0,
        },
    )
    .unwrap();
    let second = c.view().selected().unwrap().clone();
    assert_ne!(first, second, "the created device is selected");
    assert_eq!(c.doc().columns, 2);
    assert_eq!(c.doc().device(&second).unwrap().column, 1);
    assert_eq!(target_name(&c), Some(second.0.clone()));
    // The track the first creation armed was disarmed, as for the index variant.
    assert_eq!(armed(&c), vec![second.0.clone()]);
    assert!(
        c.last_rejection().is_some_and(|m| m.starts_with("Created")),
        "the status names what was made: {:?}",
        c.last_rejection()
    );
    // One undo step takes both the device and the column away.
    assert!(c.undo());
    assert_eq!(c.doc().columns, 1);
}

#[test]
fn an_effect_dropped_at_a_slot_is_selected_and_routed_from_the_device_above() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.view().selected().unwrap().clone();
    let at = c.doc().slot_of(&synth).unwrap();
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffectAt {
        slot: RackSlot {
            column: at.column,
            index: at.index + 1,
        },
        source: CreateSource::Type(DELAY.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    let fx = c.view().selected().unwrap().clone();
    assert_ne!(fx, synth);
    assert_eq!(c.doc().device(&fx).unwrap().type_id, DELAY);
}

const MOD_SOURCE: &str = "atomik.mod-source";

#[test]
fn a_mod_source_created_over_a_selected_instrument_is_added_without_cables_and_selected() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.view().selected().unwrap().clone();
    let cables = c.doc().cables.clone();
    let channels = c.doc().console.channels.len();
    effect(&mut c, MOD_SOURCE, CreateTarget::Selected(synth.clone()));
    let created = c.view().selected().unwrap().clone();
    assert_ne!(created, synth, "the new device is selected");
    assert_eq!(c.doc().device(&created).unwrap().type_id, MOD_SOURCE);
    assert_eq!(c.doc().cables, cables, "no cable was made");
    assert_eq!(
        c.doc().console.channels.len(),
        channels,
        "no channel either"
    );
    assert!(c.doc().console.channel_of(&created).is_none());
    let channel = c.doc().console.channel_of(&synth).unwrap().channel.clone();
    assert!(
        atomik_model::insert_chain(c.doc(), &c.registry, &channel).is_empty(),
        "it is not an insert of the instrument channel"
    );
    // One undo step takes it away.
    assert!(c.undo());
    assert!(c.doc().device(&created).is_none());
}

#[test]
fn a_mod_source_created_with_nothing_selected_is_added_alone() {
    let mut c = core();
    effect(&mut c, MOD_SOURCE, CreateTarget::Nothing);
    assert_eq!(c.doc().devices.len(), 1);
    assert!(c.doc().cables.is_empty());
    assert_eq!(c.view().selected(), Some(&c.doc().devices[0].key));
}

#[test]
fn a_mod_source_dropped_at_a_slot_is_placed_there_without_cables_and_selected() {
    let mut c = core();
    instrument(&mut c, SYNTH).unwrap();
    let synth = c.view().selected().unwrap().clone();
    let cables = c.doc().cables.clone();
    let at = c.doc().slot_of(&synth).unwrap();
    // To the right of the last column: a new column.
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffectAt {
        slot: RackSlot {
            column: at.column + 1,
            index: 0,
        },
        source: CreateSource::Type(MOD_SOURCE.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    let created = c.view().selected().unwrap().clone();
    assert_eq!(c.doc().device(&created).unwrap().type_id, MOD_SOURCE);
    assert_eq!(c.doc().device(&created).unwrap().column, at.column + 1);
    assert_eq!(c.doc().cables, cables, "no cable was made");
    assert!(c.doc().console.channel_of(&created).is_none());
    // And dropped below the instrument in its own column.
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffectAt {
        slot: RackSlot {
            column: at.column,
            index: at.index + 1,
        },
        source: CreateSource::Type(MOD_SOURCE.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    let second = c.view().selected().unwrap().clone();
    assert_ne!(second, created);
    assert_eq!(c.doc().cables, cables, "still no cable");
}
