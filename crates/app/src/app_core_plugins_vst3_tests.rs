//! Wiring (across formats, as far as a
//! test can run them): the app core with the CLAP **and** the VST3 test plugins. Both formats in
//! the catalog, a VST3 device from stand-in to live, its editor window protocol, the live twin of
//! the `plugin-demo-vst3` bounce, engine restarts and saves with both formats, and a project
//! whose VST3 plugins are missing.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_core::PPQ;
use atomik_device_sdk::{PluginFormat, RunMode};
use atomik_model::{Clip, ClipContent, ClipId, Note, PluginCommand, SongCommand, empty_doc};
use atomik_plugin_host::{NativeParent, PluginMessage};
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings};
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_with};
use atomik_ui::BannerKind;
use atomik_ui::plugin_data::PluginBannerAction;

const LATENCY: usize = atomik_core::BATCH;
const VST3_GAIN: &str = "Atomik Test Gain";
const VST3_SYNTH: &str = "Atomik Test Synth";

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo-vst3/plugin-demo-vst3.atomik")
}

fn both() -> PluginConfig {
    config(true, &[PKG, PKG_VST3])
}

fn both_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(both());
    core
}

fn demo_core() -> AppCore<OfflineSink> {
    let mut core = both_core();
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    assert_eq!(
        core.plugin_instance_count(),
        7,
        "lead, echo, arp, pad, vox, wash and box/syn are live"
    );
    core
}

fn add_vst3(core: &mut AppCore<OfflineSink>, name: &str, k: &str) {
    let id = vst3_id(core, name);
    add_plugin(core, &id, k);
}

fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
    // The bounce settles the plugins' restarts before the song (a latency a param gives a plugin
    // reaches delay compensation through one); a project that has been open for a moment has
    // settled live too.
    settle_steps(core);
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

fn bounce_of(doc: &atomik_model::RackDoc, mode: RunMode) -> Vec<Vec<f32>> {
    let cache = cache_of(&[PKG, PKG_VST3]);
    let mut registry = default_registry();
    let mut host = PluginHost::new();
    host.set_catalog(&cache, &bounce_settings(), &mut registry);
    register_stand_ins(doc, &mut registry).unwrap();
    let mut plugins = PluginBounce {
        host: Some(&mut host),
        mode,
        missing: MissingPlugins::Error,
    };
    render_doc_with(
        doc,
        &registry,
        &ProjectOptions::default(),
        None,
        &mut plugins,
    )
    .unwrap()
    .channels
}

// ------------------------------------------------------------------ the catalog and the devices

#[test]
fn both_formats_are_in_the_catalog_and_registered_without_loading_a_file() {
    let core = both_core();
    let entries = core.plugin_catalog().entries();
    for format in [PluginFormat::Clap, PluginFormat::Vst3] {
        assert!(
            entries.iter().filter(|e| e.plugin.format == format).count() >= 4,
            "{format:?}: the four test plugins"
        );
    }
    for e in entries {
        assert!(core.registry().is_plugin(&e.plugin.type_id()));
    }
    let gain = vst3_id(&core, VST3_GAIN);
    assert!(gain.starts_with("vst3.") && gain.len() == 5 + 32, "{gain}");
    assert_eq!(core.plugin_instance_count(), 0, "nothing was created");
}

#[test]
fn a_vst3_plugin_is_a_stand_in_until_a_frame_makes_it_live_and_it_has_an_editor() {
    let mut core = both_core();
    let id = vst3_id(&core, VST3_GAIN);
    let add = add_command(&core, &id, "echo");
    core.apply_command(add).unwrap();
    assert_eq!(core.plugin_instance_count(), 0, "the stand-in first");
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 1);
    assert!(core.plugin_failure(&key("echo")).is_none());
    core.frame();
    assert!(
        core.view().plugin_status(&key("echo")).has_editor,
        "the VST3 gain has an editor"
    );
    add_vst3(&mut core, VST3_SYNTH, "lead");
    core.frame();
    assert!(
        !core.view().plugin_status(&key("lead")).has_editor,
        "the VST3 synth has none"
    );
    assert_eq!(core.plugin_instance_count(), 2);
}

#[test]
fn a_vst3_effect_and_a_clap_effect_sound_the_same_on_a_console_insert() {
    // The same synth and effect in each format: the same bits (in the app).
    let reference = both_core();
    let (vsynth, vgain) = (
        vst3_id(&reference, VST3_SYNTH),
        vst3_id(&reference, VST3_GAIN),
    );
    let render = |synth: &str, effect: &str, gain: f32| {
        let mut core = both_core();
        add_plugin(&mut core, synth, "lead");
        add_plugin(&mut core, effect, "fx");
        let channel = core.doc().console.channels[0].channel.clone();
        apply(
            &mut core,
            Command::Console(atomik_model::ConsoleCommand::SetInsert {
                channel,
                slot: 0,
                effect: Some(atomik_model::InsertEffect::Existing(key("fx"))),
            }),
        );
        set_param(&mut core, "fx", "p1", gain);
        apply(
            &mut core,
            Command::Song(SongCommand::AddTrack {
                device: key("lead"),
            }),
        );
        apply(
            &mut core,
            Command::Song(SongCommand::AddClip {
                device: key("lead"),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length: 4 * PPQ,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    grid: None,
                    content: ClipContent::Notes(vec![Note {
                        tick: 0,
                        length: 2 * PPQ,
                        key: 60,
                        velocity: 0.8,
                    }]),
                },
            }),
        );
        settle(&mut core);
        core.transport_action(atomik_ui::TransportAction::Play);
        core.sink_mut().render(8_192)
    };
    let clap = render(SYNTH, GAIN, -6.0);
    let vst3 = render(&vsynth, &vgain, (-6.0 + 64.0) / 128.0);
    assert!(clap[0].iter().any(|x| x.abs() > 1e-4), "it sounds");
    assert!(
        clap.iter()
            .flatten()
            .map(|x| x.to_bits())
            .eq(vst3.iter().flatten().map(|x| x.to_bits())),
        "the two formats give the same bits"
    );
}

// ------------------------------------------------------------------ the editor window

fn fake_parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xA11C_E000)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xA11C_E000)
    } else {
        NativeParent::X11(0x00A1_1CE0)
    }
}

fn window_info() -> PluginWindowInfo {
    PluginWindowInfo {
        parent: fake_parent(),
        scale: 1.5,
    }
}

fn vst3_gain_core() -> AppCore<OfflineSink> {
    let mut core = both_core();
    add_vst3(&mut core, VST3_GAIN, "echo");
    core.frame();
    core
}

fn open_editor(core: &mut AppCore<OfflineSink>, device: &DeviceKey) {
    core.handle_action(UiAction::OpenPluginGui {
        device: device.clone(),
    });
    let first = core.take_plugin_window_requests();
    assert!(
        matches!(first.as_slice(), [PluginWindowRequest::Open { .. }]),
        "{first:?}"
    );
    core.plugin_window_created(device, window_info());
}

#[test]
fn the_vst3_editor_opens_in_a_window_is_sized_by_the_plugin_and_shown() {
    let mut core = vst3_gain_core();
    let dev = key("echo");
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    let asked = core.take_plugin_window_requests();
    let [
        PluginWindowRequest::Open {
            device,
            title,
            remembered,
        },
    ] = asked.as_slice()
    else {
        panic!("{asked:?}");
    };
    assert_eq!(device, &dev);
    assert!(title.starts_with("Atomik Test Gain - "), "{title}");
    assert_eq!(*remembered, None);
    assert!(core.plugin_gui_open(&dev), "opening");
    core.plugin_window_created(&dev, window_info());
    let after = core.take_plugin_window_requests();
    let [
        PluginWindowRequest::Resize { w, h, .. },
        PluginWindowRequest::Show { .. },
    ] = after.as_slice()
    else {
        panic!("size then show: {after:?}");
    };
    assert!(*w > 0 && *h > 0, "the plugin's own size: {w}x{h}");
}

#[test]
fn the_vst3_window_and_plugin_resize_each_other_and_closing_captures_the_state() {
    let mut core = vst3_gain_core();
    let dev = key("echo");
    open_editor(&mut core, &dev);
    let _ = core.take_plugin_window_requests();
    // The user drags the window edge: the plugin takes (or adjusts) the size.
    core.plugin_window_resized(&dev, 640, 480);
    let reqs = core.take_plugin_window_requests();
    assert!(
        matches!(reqs.as_slice(), [] | [PluginWindowRequest::Resize { .. }]),
        "{reqs:?}"
    );
    // The plugin asks for another size from another thread (`IPlugFrame::resizeView`): the
    // request reaches the platform from the main thread's poll.
    let id = core.plugin_instance(&dev).unwrap();
    let messenger = core
        .plugin_host()
        .manager_ref()
        .slot(id)
        .unwrap()
        .messenger();
    std::thread::spawn(move || {
        assert!(messenger.post(PluginMessage::GuiResize { w: 800, h: 600 }));
    })
    .join()
    .unwrap();
    assert!(core.take_plugin_window_requests().is_empty());
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Resize {
            device: dev.clone(),
            w: 800,
            h: 600
        }]
    );
    // Closing the window captures the state (the component and the controller sections) and
    // remembers the size.
    core.plugin_window_closed(&dev);
    assert!(!core.plugin_gui_open(&dev));
    let state = core
        .doc()
        .resolve(&dev)
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .state
        .clone();
    assert!(
        state.is_some_and(|s| s.0.starts_with(b"AVS1")),
        "a VST3 state container"
    );
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    let asked = core.take_plugin_window_requests();
    let [PluginWindowRequest::Open { remembered, .. }] = asked.as_slice() else {
        panic!("{asked:?}");
    };
    assert!(remembered.is_some());
    // The plugin closing its own editor asks the platform to destroy the window.
    core.plugin_window_created(&dev, window_info());
    let _ = core.take_plugin_window_requests();
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Close { device: dev }]
    );
}

#[test]
fn removing_a_vst3_device_closes_its_editor() {
    let mut core = vst3_gain_core();
    let dev = key("echo");
    open_editor(&mut core, &dev);
    let _ = core.take_plugin_window_requests();
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    let reqs = core.take_plugin_window_requests();
    assert!(
        reqs.contains(&PluginWindowRequest::Close {
            device: dev.clone()
        }),
        "{reqs:?}"
    );
    assert!(!core.plugin_gui_open(&dev));
}

// ------------------------------------------------------------------ the mixed demo, live and bounced

#[test]
fn the_live_path_plays_the_vst3_demo_exactly_like_the_realtime_bounce() {
    let reference = bounce_of(demo_core().doc(), RunMode::Realtime);
    let frames = reference[0].len();
    assert!(
        reference[0].iter().any(|s| s.abs() > 1e-3),
        "the demo sounds"
    );
    for block in [64, 480, 512] {
        let mut core = demo_core();
        let live = play(&mut core, frames, block);
        assert!(same_bits(&live, &reference), "block {block}");
    }
}

#[test]
fn live_equals_bounce_after_edits_to_clap_and_vst3_params_undo_and_redo() {
    let mut core = demo_core();
    // A VST3 gain (normalised), a VST3 stepped param (a step index) and a CLAP param.
    set_param(&mut core, "echo", "p1", (-12.0 + 64.0) / 128.0);
    set_param(&mut core, "arp", "p1", 24.0 + 12.0);
    set_param(&mut core, "pad", "p2", 0.9);
    let edited = bounce_of(&core.doc().clone(), RunMode::Realtime);
    let original = bounce_of(demo_core().doc(), RunMode::Realtime);
    assert!(edited != original, "the edits are audible");
    let live = play(&mut core, edited[0].len(), 512);
    assert!(same_bits(&live, &edited), "edited");
    let mut core = demo_core();
    set_param(&mut core, "echo", "p1", (-12.0 + 64.0) / 128.0);
    assert!(core.undo());
    let live = play(&mut core, original[0].len(), 64);
    assert!(same_bits(&live, &original), "undone");
}

#[test]
fn an_engine_restart_keeps_clap_and_vst3_instances_and_their_params() {
    let mut core = demo_core();
    let devices: Vec<DeviceKey> = core
        .doc()
        .plugin_devices()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let before: Vec<_> = devices
        .iter()
        .map(|k| {
            let id = core.plugin_instance(k).expect("live");
            let native = core.plugin_host().manager_ref().native_handle(id).unwrap();
            (k.clone(), id, native)
        })
        .collect();
    let params: Vec<_> = core
        .doc()
        .plugin_devices()
        .into_iter()
        .map(|(k, d)| (k, d.params.clone()))
        .collect();
    core.rebuild_engine();
    assert!(core.plugins_ready(), "reattached at once");
    settle(&mut core);
    assert_eq!(core.plugin_instance_count(), 7, "none created, none lost");
    for (k, id, native) in before {
        assert_eq!(core.plugin_instance(&k), Some(id), "{k}");
        assert_eq!(
            core.plugin_host().manager_ref().native_handle(id),
            Some(native),
            "{k} is the same plugin instance"
        );
    }
    let after: Vec<_> = core
        .doc()
        .plugin_devices()
        .into_iter()
        .map(|(k, d)| (k, d.params.clone()))
        .collect();
    assert_eq!(params, after);
    core.transport_action(atomik_ui::TransportAction::Play);
    let out = core.sink_mut().render(48_000);
    assert!(out[0].iter().any(|x| x.abs() > 1e-3), "it plays after");
}

fn states(core: &AppCore<OfflineSink>) -> Vec<(String, Option<Vec<u8>>)> {
    core.doc()
        .plugin_devices()
        .into_iter()
        .map(|(k, d)| {
            (
                k.0,
                d.plugin
                    .as_ref()
                    .unwrap()
                    .state
                    .as_ref()
                    .map(|s| s.0.clone()),
            )
        })
        .collect()
}

#[test]
fn save_captures_both_formats_state_and_reopening_restores_it() {
    let dir = temp("vst3-save");
    let file = dir.join("mixed.atomik");
    let mut core = demo_core();
    core.save_as(&file).unwrap();
    let saved = states(&core);
    assert_eq!(saved.len(), 7);
    for (k, state) in &saved {
        assert!(state.as_ref().is_some_and(|s| !s.is_empty()), "{k}");
    }
    let vst3_containers = core
        .doc()
        .plugin_devices()
        .into_iter()
        .filter(|(_, d)| d.plugin.as_ref().unwrap().format == PluginFormat::Vst3)
        .all(|(_, d)| {
            d.plugin
                .as_ref()
                .unwrap()
                .state
                .as_ref()
                .unwrap()
                .0
                .starts_with(b"AVS1")
        });
    assert!(vst3_containers, "the VST3 blobs are VST3 containers");
    let mut again = both_core();
    again.open(&file).unwrap();
    again.wait_background();
    assert_eq!(again.plugin_instance_count(), 7);
    assert!(
        again
            .doc()
            .plugin_devices()
            .iter()
            .all(|(k, _)| again.plugin_failure(k).is_none())
    );
    assert_eq!(states(&again), saved, "the states came back");
    assert!(!again.is_dirty(), "loading changes nothing");
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ missing and disabled plugins

#[test]
fn a_project_with_vst3_plugins_opens_with_stand_ins_when_only_clap_is_installed() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    core.open(&demo_file()).unwrap();
    core.wait_background();
    core.wait_plugins();
    assert_eq!(
        core.plugin_instance_count(),
        3,
        "the three CLAP plugins are live, the four VST3 ones are stand-ins"
    );
    for k in ["echo", "arp", "vox"] {
        let status = core.view().plugin_status(&key(k));
        assert!(status.missing.is_some(), "{k} is MISSING: {status:?}");
    }
    assert!(core.view().plugin_status(&key("lead")).missing.is_none());
    assert_eq!(
        core.plugin_banner(),
        Some(&BannerKind::ProjectUsesPlugins {
            count: 7,
            needs_consent: false
        })
    );
    // Nothing is lost: the stand-ins keep everything and save back unchanged (the live CLAP
    // devices add the state they captured, so the VST3 devices are compared one by one).
    let dir = temp("vst3-missing");
    let out = dir.join("again.atomik");
    core.save_as(&out).unwrap();
    let original = atomik_model::load_project(
        &std::fs::read_to_string(demo_file()).unwrap(),
        core.registry(),
    )
    .unwrap();
    let saved =
        atomik_model::load_project(&std::fs::read_to_string(&out).unwrap(), core.registry())
            .unwrap();
    for k in ["echo", "arp", "vox", "box/syn"] {
        assert_eq!(
            saved.resolve(&key(k)),
            original.resolve(&key(k)),
            "{k} is unchanged"
        );
    }
    assert_eq!(saved.cables, original.cables);
    assert_eq!(saved.song, original.song);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rescan_that_finds_the_vst3_plugins_gives_the_stand_ins_their_instances() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    core.open(&demo_file()).unwrap();
    core.wait_background();
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 3);
    // The user installs the VST3 plugins and scans: the cache grows and the stand-ins go live.
    core.configure_plugins(both());
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(core.plugin_instance_count(), 7);
    assert!(core.view().plugin_status(&key("echo")).missing.is_none());
}

#[test]
fn opening_the_vst3_demo_while_plugins_are_off_gives_stand_ins_and_load_enables_them() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(false, &[PKG, PKG_VST3]));
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert_eq!(core.plugin_instance_count(), 0);
    assert_eq!(
        core.plugin_banner(),
        Some(&BannerKind::ProjectUsesPlugins {
            count: 7,
            needs_consent: true
        })
    );
    core.handle_action(UiAction::PluginBanner(PluginBannerAction::Load {
        warning_acknowledged: true,
    }));
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 7);
}

#[test]
fn the_crash_journal_names_a_vst3_plugin() {
    let dir = temp("vst3-journal");
    let journal = dir.join("plugins-in-use.json");
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    let mut c = both();
    c.paths = PluginPaths::in_dir(&dir);
    core.configure_plugins(c);
    add_vst3(&mut core, VST3_GAIN, "echo");
    let text = std::fs::read_to_string(&journal).expect("written at the first instance");
    assert!(text.contains("\"vst3\""), "{text}");
    core.shutdown_plugins();
    assert!(!journal.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_vst3_instrument_can_be_added_inside_a_container() {
    let mut core = both_core();
    apply(
        &mut core,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.container".into(),
            key: Some(key("box")),
        },
    );
    let id = vst3_id(&core, VST3_SYNTH);
    let plugin = atomik_device_sdk::PluginRef::from_type_id(&id).unwrap();
    let layout = core.plugin_catalog().entry(&plugin).unwrap().layout.clone();
    apply(
        &mut core,
        Command::Plugin(PluginCommand::Add {
            index: usize::MAX,
            container: Some(key("box")),
            plugin,
            layout,
            path_hint: None,
            key: Some(key("syn")),
            auto_split: true,
        }),
    );
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 1);
    assert!(core.plugin_failure(&key("box/syn")).is_none());
}
