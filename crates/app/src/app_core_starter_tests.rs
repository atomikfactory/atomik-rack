//! App wiring: File, New makes the starter project and Open does not, the status text after
//! a create with the master selected, the Audio I/O host channel counts and meter feed, the
//! audio track input cables, and the live twin of the bounce for a starter project with an
//! instrument and a send effect.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::RecordingSink;
use atomik_engine::MeterReading;
use atomik_model::{
    Cable, CreateOptions, CreateSource, CreateTarget, DeviceKey, SocketRef, SongCommand,
    WorkflowCommand, empty_doc, new_project_doc, save_project,
};
use atomik_ui::IoSocketState;

const SYNTH: &str = "atomik.poly-synth";
const REVERB: &str = "atomik.reverb";
const DELAY: &str = "atomik.stereo-delay";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

fn starter<S: EngineSink>(sink: S) -> AppCore<S> {
    AppCore::with_doc(sink, new_project_doc(), None)
}

fn instrument<S: EngineSink>(c: &mut AppCore<S>, name: &str) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key(name)),
        options: CreateOptions::default(),
    }))
    .unwrap();
}

fn effect<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, target: CreateTarget) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(type_id.into()),
        target,
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
}

fn endpoint<S: EngineSink>(c: &AppCore<S>, device: &str, socket: &str) -> Endpoint {
    let socket = SocketRef {
        device: key(device),
        socket: socket.to_string(),
    };
    c.endpoint(c.topology(), &socket).expect("endpoint")
}

// ------------------------------------------------------------------ File, New

#[test]
fn new_document_makes_the_starter_project_with_a_clean_history() {
    let mut c = AppCore::new(RecordingSink::new());
    instrument(&mut c, "lead");
    c.new_document();
    assert_eq!(save_project(c.doc()), save_project(&new_project_doc()));
    assert!(!c.is_dirty(), "a fresh New is not dirty");
    assert!(!c.undo(), "the undo stack of a fresh New is empty");
    let master = c.doc().console.master.clone().expect("a master");
    assert_eq!(master, key("master"));
    assert_eq!(c.doc().devices.first().map(|d| d.key.clone()), Some(master));
    assert_eq!(c.doc().cables.len(), 2, "main to outputs 1 and 2");
    // The engine plays what the document holds: the master exists as an engine device.
    assert!(c.device_id(&key("master")).is_some());
}

#[test]
fn open_never_creates_a_master_old_and_foreign_files_load_as_they_are() {
    let mut c = AppCore::new(RecordingSink::new());
    c.new_document();
    // An older project: no master at all.
    let old = save_project(&empty_doc());
    c.open_str(&old, None).unwrap();
    assert!(c.doc().console.master.is_none());
    assert_eq!(c.doc().devices.len(), 0);
    assert_eq!(save_project(c.doc()), old, "load then save is the identity");
    // Opening a document built the old way keeps its master where it is.
    let mut doc = AppCore::new(RecordingSink::new());
    instrument(&mut doc, "a");
    let text = save_project(doc.doc());
    c.open_str(&text, None).unwrap();
    assert_eq!(save_project(c.doc()), text);
}

#[test]
fn the_first_channel_still_creates_a_master_in_a_document_without_one() {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    assert!(c.doc().console.master.is_none());
    instrument(&mut c, "lead");
    assert!(c.doc().console.master.is_some());
}

// ------------------------------------------------------------------ the status text

#[test]
fn creating_an_effect_with_the_master_selected_says_where_it_was_routed() {
    let mut c = starter(RecordingSink::new());
    effect(&mut c, REVERB, CreateTarget::Selected(key("master")));
    assert_eq!(c.last_rejection(), Some("Effect routed to send/return 1"));
    effect(&mut c, DELAY, CreateTarget::Selected(key("master")));
    assert_eq!(c.last_rejection(), Some("Effect routed to send/return 2"));
    // The routed effect is selected like every created device.
    assert!(c.view().selected().is_some());
    assert!(c.doc().console.returns[0].is_some() && c.doc().console.returns[1].is_some());
}

#[test]
fn the_ninth_effect_is_added_unrouted_and_the_status_says_all_slots_are_in_use() {
    let mut c = starter(RecordingSink::new());
    for _ in 0..8 {
        effect(&mut c, REVERB, CreateTarget::Selected(key("master")));
    }
    assert!(c.doc().console.returns.iter().all(Option::is_some));
    let devices = c.doc().devices.len();
    effect(&mut c, REVERB, CreateTarget::Selected(key("master")));
    assert_eq!(
        c.last_rejection(),
        Some("All 8 send and return slots are in use")
    );
    assert_eq!(c.doc().devices.len(), devices + 1, "the effect is added");
    assert!(
        c.doc().console.returns.iter().flatten().count() == 8,
        "and routed nowhere"
    );
}

#[test]
fn an_effect_on_a_channel_keeps_its_insert_message_and_the_master_message_is_not_used() {
    let mut c = starter(RecordingSink::new());
    instrument(&mut c, "lead");
    let channel = c
        .doc()
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    effect(&mut c, DELAY, CreateTarget::Selected(channel));
    let message = c.last_rejection().unwrap_or_default().to_string();
    assert!(
        message.starts_with("Created ") && message.contains("insert 1"),
        "{message}"
    );
}

// ------------------------------------------------------------------ Audio I/O host channels

fn io_states<S: EngineSink>(c: &AppCore<S>) -> Vec<IoSocketState> {
    c.view().io_states().to_vec()
}

#[test]
fn the_view_gets_the_host_channel_counts_and_follows_every_stream_change() {
    let mut c = starter(RecordingSink::new());
    let n_in = usize::from(c.doc().input_channels);
    // Offline sink: every channel exists, the starter cables are connected.
    c.frame();
    assert_eq!(
        c.view().io_socket_count(),
        n_in + usize::from(c.doc().output_channels)
    );
    assert_eq!(io_states(&c)[n_in], IoSocketState::Connected);
    assert_eq!(io_states(&c)[n_in + 1], IoSocketState::Connected);
    // A device with one output: out_2 is cabled and missing.
    c.sink_mut().host_channels = Some((2, 1));
    c.frame();
    assert_eq!(io_states(&c)[n_in], IoSocketState::Connected);
    assert_eq!(io_states(&c)[n_in + 1], IoSocketState::Unavailable);
    // Back to a stereo device.
    c.sink_mut().host_channels = Some((2, 2));
    c.frame();
    assert_eq!(io_states(&c)[n_in + 1], IoSocketState::Connected);
}

#[test]
fn a_new_engine_restart_tells_the_view_the_host_again() {
    let mut c = starter(RecordingSink::new());
    let n_in = usize::from(c.doc().input_channels);
    c.sink_mut().host_channels = Some((0, 1));
    c.rebuild_engine();
    assert_eq!(io_states(&c)[n_in + 1], IoSocketState::Unavailable);
}

// ------------------------------------------------------------------ Audio I/O meters

#[test]
fn input_sockets_are_metered_at_their_interface_endpoint() {
    let mut c = starter(RecordingSink::new());
    let ep = endpoint(&c, "interface", "in_1");
    c.sink_mut().meters.insert(
        ep,
        MeterReading {
            peak: 0.5,
            rms: 0.2,
        },
    );
    c.feed_io_meters();
    let levels = c.view().io_levels().to_vec();
    assert!(levels[0] > -7.0 && levels[0] < -5.0, "{levels:?}");
    assert!(levels[1] <= atomik_ui::io_panel::LEVEL_FLOOR_DB + 0.01);
    assert!(!c.view().io_clips()[0]);
}

#[test]
fn an_output_socket_shows_the_peak_of_the_source_of_its_cable() {
    let mut c = starter(RecordingSink::new());
    let n_in = usize::from(c.doc().input_channels);
    let main_l = endpoint(&c, "master", "main_l");
    c.sink_mut().meters.insert(
        main_l,
        MeterReading {
            peak: 0.25,
            rms: 0.1,
        },
    );
    c.feed_io_meters();
    let levels = c.view().io_levels().to_vec();
    assert!(
        levels[n_in] > -13.0 && levels[n_in] < -11.0,
        "out_1: {levels:?}"
    );
    assert!(
        levels[n_in + 1] <= atomik_ui::io_panel::LEVEL_FLOOR_DB + 0.01,
        "out_2 is dark"
    );
}

#[test]
fn a_direct_cable_to_an_output_is_metered_at_its_source_and_a_clip_latches() {
    let mut c = starter(RecordingSink::new());
    instrument(&mut c, "lead");
    let n_in = usize::from(c.doc().input_channels);
    // The synth goes straight to out_1 instead of the master's main_l.
    c.apply_command(Command::Disconnect {
        to: sref("interface", "out_1"),
    })
    .unwrap();
    c.apply_command(Command::Connect(Cable {
        from: sref("lead", "out_l"),
        to: sref("interface", "out_1"),
    }))
    .unwrap();
    let main_l = endpoint(&c, "master", "main_l");
    let lead = endpoint(&c, "lead", "out_l");
    c.sink_mut().meters.insert(
        main_l,
        MeterReading {
            peak: 0.1,
            rms: 0.0,
        },
    );
    c.sink_mut().meters.insert(
        lead,
        MeterReading {
            peak: 1.5,
            rms: 0.5,
        },
    );
    c.feed_io_meters();
    assert!(c.view().io_clips()[n_in], "1.5 clips out_1");
    assert!(
        !c.view().io_clips()[n_in + 1],
        "out_2 still follows main_r (silent)"
    );
    // The lamp stays latched until it is reset.
    c.sink_mut().meters.clear();
    c.feed_io_meters();
    assert!(c.view().io_clips()[n_in]);
    c.view_mut().reset_io_clips(None);
    assert!(!c.view().io_clips()[n_in]);
}

#[test]
fn an_uncabled_output_stays_dark_and_a_missing_reading_is_silence() {
    let mut c = starter(RecordingSink::new());
    c.feed_io_meters();
    assert!(
        c.view()
            .io_levels()
            .iter()
            .all(|l| *l <= atomik_ui::io_panel::LEVEL_FLOOR_DB + 0.01)
    );
    assert!(c.view().io_clips().iter().all(|c| !c));
}

fn lead_with_a_clip<S: EngineSink>(c: &mut AppCore<S>) {
    use atomik_core::PPQ;
    use atomik_model::{Clip, ClipContent, ClipId, Note};
    instrument(c, "lead");
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 2 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            content: ClipContent::Notes(
                [(0, 60u8), (PPQ / 2, 64), (PPQ, 67)]
                    .into_iter()
                    .map(|(tick, k)| Note::new(tick, PPQ / 2, k, 0.9))
                    .collect(),
            ),
        },
    }))
    .unwrap();
}

#[test]
fn a_synth_through_the_starter_lights_outputs_one_and_two_on_a_real_engine() {
    let mut c = starter(OfflineSink::new());
    lead_with_a_clip(&mut c);
    let n_in = usize::from(c.doc().input_channels);
    c.transport_action(atomik_ui::TransportAction::Play);
    let out = c.sink_mut().render_blocks(24_000, 512);
    assert!(out[0].iter().any(|s| s.abs() > 1e-3), "out_1 sounds");
    assert!(out[1].iter().any(|s| s.abs() > 1e-3), "out_2 sounds");
    c.frame();
    let levels = c.view().io_levels().to_vec();
    assert!(
        levels[n_in] > atomik_ui::io_panel::LEVEL_FLOOR_DB + 3.0,
        "{levels:?}"
    );
    assert!(
        levels[n_in + 1] > atomik_ui::io_panel::LEVEL_FLOOR_DB + 3.0,
        "{levels:?}"
    );
}

#[test]
fn the_io_feed_does_not_steal_the_peaks_the_console_reads() {
    let mut c = starter(OfflineSink::new());
    lead_with_a_clip(&mut c);
    c.transport_action(atomik_ui::TransportAction::Play);
    c.sink_mut().render_blocks(24_000, 512);
    // One frame as the shell runs it: the core frame (the I/O feed reads the master outputs),
    // then the console feed. Both see the peak of the same blocks.
    c.frame();
    c.update_console_meters(0.0);
    let master = c.doc().console.master.clone().unwrap();
    let meter = c.console().meter(&master).expect("master meter");
    assert!(
        meter.peak[0].level_db > -60.0,
        "the console saw the master peak: {meter:?}"
    );
}

// ------------------------------------------------------------------ audio track inputs

fn input_cables<S: EngineSink>(c: &AppCore<S>, track: &DeviceKey) -> Vec<(String, String)> {
    c.doc()
        .cables
        .iter()
        .filter(|x| x.from.device.0 == "interface" && &x.to.device == track)
        .map(|x| (x.from.socket.clone(), x.to.socket.clone()))
        .collect()
}

#[test]
fn the_track_input_selector_makes_exactly_the_cables_it_made_before() {
    use atomik_ui::audio::{InputSel, input_change};
    // The same steps on the starter and on the old empty document give the same input
    // cables: a new audio track listens to in 1+2, the selector re-cables it by hand.
    let run = |mut c: AppCore<RecordingSink>| {
        c.add_audio_track();
        let track = c.doc().song.tracks.last().unwrap().device.clone();
        let mut seen = vec![input_cables(&c, &track)];
        let mut current = InputSel::Pair(1, 2);
        for target in [InputSel::Mono(2), InputSel::Off, InputSel::Pair(1, 2)] {
            let batch = input_change(&track, current, target);
            c.apply_command(Command::Batch(batch)).unwrap();
            current = target;
            seen.push(input_cables(&c, &track));
        }
        seen
    };
    let new = run(starter(RecordingSink::new()));
    let old = run(AppCore::with_doc(RecordingSink::new(), empty_doc(), None));
    assert_eq!(new, old);
    assert_eq!(
        new[0],
        vec![
            ("in_1".to_string(), "in_l".to_string()),
            ("in_2".to_string(), "in_r".to_string())
        ]
    );
    assert_eq!(new[2], Vec::new());
}

// ------------------------------------------------------------------ live equals bounce

mod live {
    use super::*;

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

    /// The starter, an instrument made by the chain, and a send effect made with the master
    /// selected; the channel sends to bus 1.
    fn project() -> AppCore<OfflineSink> {
        let mut c = starter(OfflineSink::new());
        lead_with_a_clip(&mut c);
        effect(&mut c, REVERB, CreateTarget::Selected(key("master")));
        let channel = c
            .doc()
            .console
            .channel_of(&key("lead"))
            .unwrap()
            .channel
            .clone();
        for (param, value) in [("send1.on", 1.0), ("send1.level", 0.8)] {
            c.apply_command(Command::SetParam {
                key: channel.clone(),
                param: param.into(),
                value,
                merge: None,
            })
            .unwrap();
        }
        c.rebuild_engine();
        c
    }

    /// The same chain without the channel's send switches: the engine got every step live.
    fn chain_only() -> AppCore<OfflineSink> {
        let mut c = starter(OfflineSink::new());
        lead_with_a_clip(&mut c);
        effect(&mut c, REVERB, CreateTarget::Selected(key("master")));
        c
    }

    /// Plays from the start. A parameter edited while the engine runs ramps for a few
    /// milliseconds (the engine's smoothers) and the bounce starts from the stored value, so
    /// a project edited live is compared after an engine restart (what Open does).
    fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
        core.transport_action(atomik_ui::TransportAction::Play);
        core.sink_mut().render_blocks(frames + LATENCY, block)
    }

    #[test]
    fn the_chain_made_live_plays_like_the_bounce_of_its_document() {
        // Nothing but the creation chain (the instrument made with the starter, the effect
        // made with the master selected), each step applied to the running engine.
        let reference = bounce_of(chain_only().doc());
        let frames = reference[0].len();
        for block in [64, 512] {
            let mut core = chain_only();
            let live = play(&mut core, frames, block);
            assert!(same_bits(&live, &reference), "block {block}");
        }
    }

    #[test]
    fn a_starter_project_with_an_instrument_and_a_send_effect_plays_live_like_the_bounce() {
        let reference = bounce_of(project().doc());
        assert!(
            reference[0].iter().any(|s| s.abs() > 1e-3),
            "it makes sound"
        );
        assert!(
            reference[1].iter().any(|s| s.abs() > 1e-3),
            "on both outputs"
        );
        let frames = reference[0].len();
        for block in [64, 480, 512] {
            let mut core = project();
            let live = play(&mut core, frames, block);
            assert!(same_bits(&live, &reference), "block {block}");
        }
        // The send makes a difference: without it the bounce is another one.
        let mut dry = project();
        dry.apply_command(Command::SetParam {
            key: dry
                .doc()
                .console
                .channel_of(&key("lead"))
                .unwrap()
                .channel
                .clone(),
            param: "send1.on".into(),
            value: 0.0,
            merge: None,
        })
        .unwrap();
        assert!(
            bounce_of(dry.doc()) != reference,
            "the send effect is heard"
        );
        // Undo of the send settings and of the effect, redo, and an engine restart.
        let mut core = project();
        for _ in 0..3 {
            assert!(core.undo());
        }
        let without = bounce_of(&core.doc().clone());
        assert!(without != reference);
        let live = play(&mut core, without[0].len(), 512);
        assert!(same_bits(&live, &without), "after undo");
        let mut core = project();
        for _ in 0..3 {
            assert!(core.undo());
        }
        for _ in 0..3 {
            assert!(core.redo());
        }
        core.rebuild_engine();
        let live = play(&mut core, frames, 512);
        assert!(same_bits(&live, &reference), "after redo and a restart");
    }

    #[test]
    fn the_io_meter_taps_do_not_change_a_single_sample() {
        // The same project rendered with the meters being read every block and never read.
        let reference = bounce_of(project().doc());
        let frames = reference[0].len();
        let mut core = project();
        core.transport_action(atomik_ui::TransportAction::Play);
        let mut live: Vec<Vec<f32>> = vec![Vec::new(); 2];
        for _ in 0..(frames + LATENCY).div_ceil(256) {
            let block = core.sink_mut().render_blocks(256, 256);
            core.frame();
            core.update_console_meters(0.0);
            for (a, b) in live.iter_mut().zip(block) {
                a.extend(b);
            }
        }
        let live: Vec<Vec<f32>> = live
            .into_iter()
            .map(|c| c[..frames + LATENCY].to_vec())
            .collect();
        assert!(same_bits(&live, &reference));
    }
}

// ------------------------------------------------------------------ the example project

#[test]
fn the_master_demo_opens_as_it_is_and_plays_through_audio_io_outputs_one_and_two() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/master-demo/master-demo.atomik");
    let mut c = AppCore::new(OfflineSink::new());
    c.open(&path).expect("the demo opens");
    // Open does not touch the document: the file is what is shown.
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(save_project(c.doc()), text);
    assert!(!c.is_dirty());
    let n_in = usize::from(c.doc().input_channels);
    c.transport_action(atomik_ui::TransportAction::Play);
    let out = c.sink_mut().render_blocks(48_000, 512);
    assert!(out[0].iter().any(|s| s.abs() > 1e-3) && out[1].iter().any(|s| s.abs() > 1e-3));
    c.frame();
    assert_eq!(io_states(&c)[n_in], IoSocketState::Connected);
    let levels = c.view().io_levels().to_vec();
    assert!(
        levels[n_in] > atomik_ui::io_panel::LEVEL_FLOOR_DB + 3.0,
        "{levels:?}"
    );
    assert_eq!(c.doc().console.returns[0], Some(key("hall")));
}

// ------------------------------------------------------------------ CV activity

fn cv_pair(c: &mut AppCore<RecordingSink>) -> Cable {
    instrument(c, "lead");
    instrument(c, "pad");
    let cable = Cable {
        from: sref("lead", "lfo_cv_out"),
        to: sref("pad", "cutoff_cv_in"),
    };
    c.apply_command(Command::Connect(cable.clone())).unwrap();
    cable
}

#[test]
fn a_cv_cable_is_metered_at_its_source_and_its_peak_reaches_the_view() {
    let mut c = starter(RecordingSink::new());
    let cable = cv_pair(&mut c);
    let src = endpoint(&c, "lead", "lfo_cv_out");
    assert!(
        c.topology().meters().any(|e| e == src),
        "the source is metered"
    );
    let audio = endpoint(&c, "lead", "out_l");
    assert!(
        !c.topology().meters().any(|e| e == audio),
        "audio cables are not metered by the CV feed"
    );
    c.sink_mut().meters.insert(
        src,
        MeterReading {
            peak: 0.5,
            rms: 0.3,
        },
    );
    c.feed_cv_activity(1.0);
    assert!(c.view().cv_activity_step(&cable) > 0);
    // Silence decays to dark over the following frames.
    c.sink_mut().meters.clear();
    for i in 0..400 {
        c.feed_cv_activity(1.0 + f64::from(i) * 0.016);
    }
    assert_eq!(c.view().cv_activity_step(&cable), 0);
}

#[test]
fn a_removed_cv_cable_stops_being_metered_and_drawn() {
    let mut c = starter(RecordingSink::new());
    let cable = cv_pair(&mut c);
    let src = endpoint(&c, "lead", "lfo_cv_out");
    c.sink_mut().meters.insert(
        src,
        MeterReading {
            peak: 1.0,
            rms: 1.0,
        },
    );
    c.feed_cv_activity(1.0);
    assert_eq!(c.view().cv_activity_step(&cable), 8);
    c.apply_command(Command::Disconnect {
        to: cable.to.clone(),
    })
    .unwrap();
    assert!(!c.topology().meters().any(|e| e == src));
    c.feed_cv_activity(1.016);
    assert_eq!(c.view().cv_activity_step(&cable), 0);
}

fn cv_cable(from: &str, to: &str) -> Cable {
    Cable {
        from: sref(from, "lfo_cv_out"),
        to: sref(to, "cutoff_cv_in"),
    }
}

fn container_with_two_synths(c: &mut AppCore<RecordingSink>) {
    let inner = |type_id: &str, k: &str| {
        Command::Container(atomik_model::ContainerCommand::AddInner {
            container: key("layer"),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split: false,
        })
    };
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.container".into(),
        key: Some(key("layer")),
    })
    .unwrap();
    c.apply_command(inner(SYNTH, "lead")).unwrap();
    c.apply_command(inner(SYNTH, "pad")).unwrap();
}

fn spend(c: &mut AppCore<RecordingSink>, e: Endpoint, peak: f32) {
    c.sink_mut()
        .meters
        .insert(e, MeterReading { peak, rms: peak });
    c.sink_mut().spent_meters.lock().unwrap().clear();
}

#[test]
fn a_cv_cable_inside_a_container_lights_in_the_containers_view_by_its_local_key() {
    let mut c = starter(RecordingSink::new());
    container_with_two_synths(&mut c);
    let flat = Cable {
        from: sref("layer/lead", "lfo_cv_out"),
        to: sref("layer/pad", "cutoff_cv_in"),
    };
    c.apply_command(Command::Connect(flat.clone())).unwrap();
    let view_cable = cv_cable("lead", "pad");
    let src = endpoint(&c, "layer/lead", "lfo_cv_out");
    assert!(
        c.topology().meters().any(|e| e == src),
        "the source is metered"
    );
    spend(&mut c, src, 0.8);
    // Shown from inside: the view's own (local) key lights.
    c.view_mut().set_scope(&[key("layer")]);
    c.feed_cv_activity(1.0);
    assert!(c.view().cv_activity_step(&view_cable) > 0);
    assert_eq!(
        c.view().cv_activity_step(&flat),
        0,
        "the path key is not a view key"
    );
}

#[test]
fn a_cable_is_fed_only_to_the_scope_that_shows_it() {
    let mut c = starter(RecordingSink::new());
    container_with_two_synths(&mut c);
    instrument(&mut c, "lead");
    instrument(&mut c, "pad");
    c.apply_command(Command::Connect(cv_cable("layer/lead", "layer/pad")))
        .unwrap();
    c.apply_command(Command::Connect(cv_cable("lead", "pad")))
        .unwrap();
    let inner_src = endpoint(&c, "layer/lead", "lfo_cv_out");
    let top_src = endpoint(&c, "lead", "lfo_cv_out");
    spend(&mut c, inner_src, 1.0);
    c.sink_mut().meters.insert(
        top_src,
        MeterReading {
            peak: 0.0,
            rms: 0.0,
        },
    );
    // At the top level the inner cable (same local keys) must not light the top cable.
    c.feed_cv_activity(1.0);
    assert_eq!(c.view().cv_activity_step(&cv_cable("lead", "pad")), 0);
    c.view_mut().set_scope(&[key("layer")]);
    c.feed_cv_activity(1.016);
    assert!(c.view().cv_activity_step(&cv_cable("lead", "pad")) > 0);
}

#[test]
fn inner_cv_cables_follow_edits_inside_the_container() {
    let mut c = starter(RecordingSink::new());
    container_with_two_synths(&mut c);
    let src = endpoint(&c, "layer/lead", "lfo_cv_out");
    assert!(!c.topology().meters().any(|e| e == src), "no cable, no tap");
    let flat = Cable {
        from: sref("layer/lead", "lfo_cv_out"),
        to: sref("layer/pad", "cutoff_cv_in"),
    };
    c.apply_command(Command::Connect(flat.clone())).unwrap();
    let src = endpoint(&c, "layer/lead", "lfo_cv_out");
    assert!(c.topology().meters().any(|e| e == src));
    c.apply_command(Command::Disconnect {
        to: flat.to.clone(),
    })
    .unwrap();
    let src = endpoint(&c, "layer/lead", "lfo_cv_out");
    assert!(
        !c.topology().meters().any(|e| e == src),
        "the tap goes with the cable"
    );
    c.apply_command(Command::Connect(flat)).unwrap();
    let src = endpoint(&c, "layer/lead", "lfo_cv_out");
    assert!(c.topology().meters().any(|e| e == src), "and comes back");
}

#[test]
fn one_cv_output_feeding_two_cables_lights_both() {
    let mut c = starter(RecordingSink::new());
    instrument(&mut c, "lead");
    instrument(&mut c, "pad");
    instrument(&mut c, "bass");
    let (a, b) = (cv_cable("lead", "pad"), cv_cable("lead", "bass"));
    c.apply_command(Command::Connect(a.clone())).unwrap();
    c.apply_command(Command::Connect(b.clone())).unwrap();
    let src = endpoint(&c, "lead", "lfo_cv_out");
    spend(&mut c, src, 0.9);
    c.feed_cv_activity(1.0);
    assert!(c.view().cv_activity_step(&a) > 0);
    assert!(c.view().cv_activity_step(&b) > 0);
}

#[test]
fn a_cv_output_the_console_meters_lights_its_cable_and_the_console_reads_the_same() {
    let mut c = starter(RecordingSink::new());
    instrument(&mut c, "lead");
    instrument(&mut c, "pad");
    let channel = c
        .doc()
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    let cable = Cable {
        from: SocketRef {
            device: channel.clone(),
            socket: "gr_cv_out".into(),
        },
        to: sref("pad", "cutoff_cv_in"),
    };
    c.apply_command(Command::Connect(cable.clone())).unwrap();
    let gr = endpoint(&c, &channel.0, "gr_cv_out");
    // The console alone: what it reads with nothing else around.
    spend(&mut c, gr, 0.5);
    let alone = c.console_meter_readings();
    // The real meter bank resets on read: the frame reads once, both consumers share it.
    c.sink_mut().spend_meters_on_read = true;
    spend(&mut c, gr, 0.5);
    c.frame();
    c.update_console_meters(0.0);
    c.feed_cv_activity(1.0);
    let ballistics = |c: &AppCore<RecordingSink>| c.console().meter(&channel).cloned();
    assert!(c.view().cv_activity_step(&cable) > 0, "the cable lights");
    let mut reference = starter(RecordingSink::new());
    instrument(&mut reference, "lead");
    let ref_channel = reference
        .doc()
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    let ref_gr = endpoint(&reference, &ref_channel.0, "gr_cv_out");
    reference.sink_mut().meters.insert(
        ref_gr,
        MeterReading {
            peak: 0.5,
            rms: 0.5,
        },
    );
    reference.update_console_meters(0.0);
    assert_eq!(
        ballistics(&c),
        reference.console().meter(&ref_channel).cloned()
    );
    let got = |r: &[(DeviceKey, atomik_ui::ChannelMeters)]| {
        r.iter().find(|(k, _)| *k == channel).unwrap().1
    };
    assert!((got(&alone).gr - 0.5).abs() < 1e-6);
}

#[test]
fn more_cv_taps_than_the_meter_bank_holds_degrade_to_unlit_cables() {
    let mut c = starter(RecordingSink::new());
    instrument(&mut c, "lead");
    instrument(&mut c, "pad");
    instrument(&mut c, "bass");
    let metered = c.topology().meters().count();
    // Room for exactly one more meter: the second CV source gets none.
    c.meter_capacity = metered + 1;
    let (a, b) = (cv_cable("lead", "pad"), cv_cable("bass", "pad"));
    let b = Cable {
        to: sref("pad", "resonance_cv_in"),
        ..b
    };
    c.apply_command(Command::Connect(a.clone())).unwrap();
    c.apply_command(Command::Connect(b.clone())).unwrap();
    assert_eq!(c.topology().meters().count(), metered + 1, "capped");
    let (sa, sb) = (
        endpoint(&c, "lead", "lfo_cv_out"),
        endpoint(&c, "bass", "lfo_cv_out"),
    );
    spend(&mut c, sa, 1.0);
    c.sink_mut().meters.insert(
        sb,
        MeterReading {
            peak: 1.0,
            rms: 1.0,
        },
    );
    c.feed_cv_activity(1.0);
    let lit = |cable: &Cable| c.view().cv_activity_step(cable) > 0;
    assert!(
        lit(&a) != lit(&b),
        "one cable got the last slot, the other shows nothing"
    );
}
