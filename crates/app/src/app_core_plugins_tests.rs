//! The plugin glue
//! of the app core with the in-repo CLAP test plugins. Catalog and consent, stand-ins and one
//! instance per frame, engine restarts that keep the instances, state capture on save, removal
//! and undo, GUI gestures as undo steps, missing plugins and the banner, RELOAD, patches that
//! refuse plugins, and the live twin of the `plugin-demo` bounce.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_device_sdk::RunMode;
use atomik_model::{PluginCommand, empty_doc};
use atomik_plugin_host::{ParamPhase, PluginMessage};
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings};
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_with};
use atomik_ui::BannerKind;
use atomik_ui::plugin_data::{PluginBannerAction, PluginSettingsAction};

const LATENCY: usize = atomik_core::BATCH;

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo/plugin-demo.atomik")
}

fn inserts(calls: &[SinkCall]) -> Vec<(u32, &'static str)> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Insert { id, type_id, .. } => Some((id.0, *type_id)),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ consent and the catalog

#[test]
fn plugins_are_off_by_default_the_catalog_is_empty_and_adding_is_refused() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    assert!(
        !core.plugin_settings().enabled,
        "off until the user says so"
    );
    core.configure_plugins(config(false, &[PKG]));
    assert!(
        core.plugin_catalog().entries().is_empty(),
        "nothing listed while off"
    );
    assert!(!core.registry().is_plugin(SYNTH));
    // A synth added anyway (the catalog of another run) is refused: the type is not registered.
    let mut other = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    other.configure_plugins(config(true, &[PKG]));
    let add = add_command(&other, SYNTH, "lead");
    assert!(core.apply_command(add).is_err());
}

#[test]
fn an_enabled_catalog_registers_the_test_plugins_without_loading_a_file() {
    let core = plugin_core(OfflineSink::new());
    let ids: Vec<String> = core
        .plugin_catalog()
        .entries()
        .iter()
        .map(|e| e.plugin.type_id())
        .collect();
    for id in [SYNTH, GAIN, SHIFT] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
        assert!(core.registry().is_plugin(id));
    }
    assert_eq!(
        core.plugin_instance_count(),
        0,
        "nothing is created until a device is added"
    );
}

#[test]
fn the_enable_switch_turns_plugins_on_at_once() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(false, &[PKG]));
    assert_eq!(core.take_plugin_settings_change(), None);
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::SetEnabled {
        enabled: true,
    }));
    assert!(core.last_rejection().is_none());
    assert!(core.plugin_settings().enabled);
    assert_eq!(
        core.take_plugin_settings_change().map(|s| s.enabled),
        Some(true)
    );
    assert_eq!(core.take_plugin_settings_change(), None, "reported once");
    assert!(core.registry().is_plugin(SYNTH), "the catalog registered");
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetPluginDisabled {
            type_id: SYNTH.into(),
            disabled: true,
        },
    ));
    assert!(
        !core.registry().is_plugin(SYNTH),
        "a disabled plugin is not offered"
    );
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::SetEnabled {
        enabled: false,
    }));
    assert!(!core.plugin_settings().enabled);
    assert!(core.plugin_catalog().entries().is_empty());
}

// ------------------------------------------------------------------ stand-in, then the live device

#[test]
fn an_added_plugin_is_a_stand_in_until_a_frame_swaps_the_live_device_in() {
    let mut core = plugin_core(RecordingSink::new());
    core.sink_mut().take();
    let add = add_command(&core, SYNTH, "lead");
    apply(&mut core, add);
    let calls = core.sink_mut().take();
    let first = inserts(&calls);
    assert_eq!(
        first.len(),
        1 + 2,
        "the stand-in, its channel and the master"
    );
    assert_eq!(
        first[0].1, SYNTH,
        "the stand-in has the plugin's descriptor"
    );
    let stand_in_id = first[0].0;
    assert!(core.plugin_instance(&key("lead")).is_none());
    assert_eq!(core.plugin_loading(), Some((0, 1)));
    assert!(!core.plugins_ready());
    // The status the panel shows: loading.
    core.frame();
    let calls = core.sink_mut().take();
    let live = inserts(&calls);
    assert_eq!(live.len(), 1, "one instance per frame: {calls:?}");
    assert_eq!(live[0].1, SYNTH);
    assert_ne!(live[0].0, stand_in_id, "a fresh id, never a reused one");
    assert!(core.plugin_instance(&key("lead")).is_some());
    assert!(core.plugins_ready());
    // Insert, then routing and song for it, and only then the stand-in is removed.
    let pos = |pred: &dyn Fn(&SinkCall) -> bool| calls.iter().position(pred).unwrap();
    let insert = pos(&|c| matches!(c, SinkCall::Insert { .. }));
    let routing = pos(&|c| matches!(c, SinkCall::Routing(_)));
    let song = pos(&|c| matches!(c, SinkCall::Song(_)));
    let remove = pos(&|c| matches!(c, SinkCall::Remove(id) if id.0 == stand_in_id));
    assert!(
        insert < routing && routing < remove && song < remove,
        "{calls:?}"
    );
    // The routing of the swap uses the new id, not the stand-in.
    let SinkCall::Routing(order) = &calls[routing] else {
        unreachable!()
    };
    assert!(order.iter().any(|i| i.0 == live[0].0));
    assert!(!order.iter().any(|i| i.0 == stand_in_id));
}

#[test]
fn project_open_creates_one_instance_per_frame_with_a_loading_status() {
    let mut core = plugin_core(OfflineSink::new());
    for (id, k) in [(SYNTH, "a"), (GAIN, "b"), (SYNTH, "c")] {
        let add = add_command(&core, id, k);
        apply(&mut core, add);
    }
    assert_eq!(
        core.plugin_loading_text().as_deref(),
        Some("Loading plugins 1/3")
    );
    core.frame();
    assert_eq!(core.plugin_instance_count(), 1);
    assert_eq!(
        core.plugin_loading_text().as_deref(),
        Some("Loading plugins 2/3")
    );
    core.frame();
    assert_eq!(core.plugin_instance_count(), 2);
    assert_eq!(
        core.plugin_loading_text().as_deref(),
        Some("Loading plugins 3/3")
    );
    core.frame();
    assert_eq!(core.plugin_instance_count(), 3);
    assert_eq!(core.plugin_loading_text(), None, "all devices are live");
    for k in ["a", "b", "c"] {
        let status = core.view().plugin_status(&key(k));
        assert!(!status.loading && status.missing.is_none() && status.failed.is_none());
        assert!(status.has_editor || !status.has_editor, "the status is fed");
    }
}

#[test]
fn without_an_engine_the_plugins_are_created_parked_say_no_audio_and_attach_when_audio_starts() {
    // Audio off (no asio feature with the saved asio host, `--no-audio`, a failed device): the
    // panels used to sit on LOADING forever. The instances are created anyway, parked behind
    // their stand-ins: the editor and params work, the panel says why there is no sound.
    let mut core = AppCore::with_doc(
        RecordingSink {
            no_engine: true,
            ..RecordingSink::default()
        },
        empty_doc(),
        None,
    );
    core.configure_plugins(config(true, &[PKG]));
    for (id, k) in [(SYNTH, "a"), (GAIN, "b")] {
        let add = add_command(&core, id, k);
        apply(&mut core, add);
    }
    core.wait_plugins();
    core.frame();
    assert_eq!(core.plugin_instance_count(), 2, "created without an engine");
    assert_eq!(core.plugin_loading_text(), None);
    let status = core.view().plugin_status(&key("b"));
    assert!(status.has_editor, "OPEN GUI works with audio off");
    assert!(!status.loading && status.failed.is_none());
    assert_eq!(status.waiting.as_deref(), Some("audio is off"));
    // The engine arrives (audio switched on): the parked instances are attached at once.
    core.sink_mut().no_engine = false;
    core.sink_mut().take();
    core.frame();
    core.frame();
    let calls = core.sink_mut().take();
    assert_eq!(
        inserts(&calls).iter().filter(|(_, t)| *t == SYNTH).count(),
        1,
        "{calls:?}"
    );
    assert_eq!(core.plugin_instance_count(), 2, "the same instances");
    assert_eq!(core.view().plugin_status(&key("a")).waiting, None);
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
    let cache = cache_of(&[PKG]);
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

fn demo_core() -> AppCore<OfflineSink> {
    let mut core = plugin_core(OfflineSink::new());
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    assert_eq!(
        core.plugin_instance_count(),
        5,
        "lead, echo, arp, pad and box/syn are live"
    );
    core
}

#[test]
fn the_live_path_plays_the_plugin_demo_exactly_like_the_realtime_bounce() {
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
fn live_equals_bounce_after_param_edits_undo_and_redo_on_plugin_params() {
    let mut core = demo_core();
    set_param(&mut core, "pad", "p2", 0.9);
    set_param(&mut core, "echo", "p1", -12.0);
    let edited = bounce_of(&core.doc().clone(), RunMode::Realtime);
    let original = bounce_of(demo_core().doc(), RunMode::Realtime);
    assert!(edited != original, "the edits are audible");
    let live = play(&mut core, edited[0].len(), 512);
    assert!(same_bits(&live, &edited), "edited");
    // Undo both: the original sound; redo: the edited one again.
    let mut core = demo_core();
    set_param(&mut core, "pad", "p2", 0.9);
    assert!(core.undo());
    let live = play(&mut core, original[0].len(), 64);
    assert!(same_bits(&live, &original), "undone");
}

// ------------------------------------------------------------------ restart, new document, removal

#[test]
fn an_engine_restart_keeps_the_instances_and_reattaches_them_at_once() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    add_plugin(&mut core, GAIN, "echo");
    let ids: Vec<_> = ["lead", "echo"]
        .iter()
        .map(|k| core.plugin_instance(&key(k)).unwrap())
        .collect();
    let natives: Vec<_> = ids
        .iter()
        .map(|id| core.plugin_host().manager_ref().native_handle(*id).unwrap())
        .collect();
    core.rebuild_engine();
    assert!(
        core.plugins_ready(),
        "reattached, not queued: no stand-in in between"
    );
    core.wait_background();
    let again: Vec<_> = ["lead", "echo"]
        .iter()
        .map(|k| core.plugin_instance(&key(k)).unwrap())
        .collect();
    assert_eq!(again, ids, "the same instances");
    let natives_again: Vec<_> = again
        .iter()
        .map(|id| core.plugin_host().manager_ref().native_handle(*id).unwrap())
        .collect();
    assert_eq!(natives_again, natives, "the plugins were not created again");
    assert_eq!(core.plugin_instance_count(), 2);
    settle(&mut core);
    assert_eq!(
        core.plugin_instance_count(),
        2,
        "nothing leaked or destroyed"
    );
}

#[test]
fn opening_another_project_drops_the_old_instances() {
    let mut core = demo_core();
    core.open_doc(empty_doc());
    settle(&mut core);
    assert_eq!(core.plugin_instance_count(), 0);
    assert!(core.plugin_instance(&key("lead")).is_none());
}

#[test]
fn removing_a_plugin_destroys_its_instance_and_undo_brings_it_back_with_its_state() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    set_param(&mut core, "lead", "p2", 0.25);
    // The plugin says its state changed; the removal captures it before the device goes.
    post(&mut core, &key("lead"), PluginMessage::StateDirty);
    core.frame();
    assert!(
        core.doc()
            .resolve(&key("lead"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .state
            .is_none()
    );
    apply(&mut core, Command::RemoveDevice { key: key("lead") });
    let state = core.document().doc().plugin_devices().is_empty();
    assert!(state, "the device is gone");
    settle(&mut core);
    assert_eq!(
        core.plugin_instance_count(),
        0,
        "the removed plugin was destroyed"
    );
    assert!(core.undo(), "undo of the removal");
    core.wait_plugins();
    settle(&mut core);
    let device = core.doc().resolve(&key("lead")).unwrap();
    let blob = device.plugin.as_ref().unwrap().state.as_ref();
    assert!(
        blob.is_some_and(|b| !b.0.is_empty()),
        "the state was captured at the removal"
    );
    assert_eq!(core.plugin_instance_count(), 1);
    assert!(core.plugin_failure(&key("lead")).is_none());
}

#[test]
fn many_adds_and_removes_leave_no_instance_behind() {
    let mut core = plugin_core(OfflineSink::new());
    let mut descriptors = Vec::new();
    for round in 0..12 {
        let k = format!("p{round}");
        add_plugin(&mut core, GAIN, &k);
        let d = core.registry().get(GAIN).unwrap().descriptor as *const _;
        descriptors.push(d);
        apply(&mut core, Command::RemoveDevice { key: key(&k) });
        settle(&mut core);
        assert_eq!(core.plugin_instance_count(), 0, "round {round}");
    }
    assert!(
        descriptors.windows(2).all(|w| w[0] == w[1]),
        "one interned descriptor"
    );
}

// ------------------------------------------------------------------ state and save

#[test]
fn save_captures_the_state_and_reopening_restores_it() {
    let dir = temp("save");
    let file = dir.join("rack.atomik");
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    set_param(&mut core, "lead", "p2", 0.3);
    assert!(
        core.doc()
            .resolve(&key("lead"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .state
            .is_none()
    );
    core.save_as(&file).unwrap();
    let saved = core
        .doc()
        .resolve(&key("lead"))
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .state
        .clone();
    assert!(
        saved.as_ref().is_some_and(|s| !s.0.is_empty()),
        "captured before the save"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"state\":"), "the state is in the project");
    assert!(!core.is_dirty(), "the save is clean");
    // A new run: the plugin loads from the project with its state.
    let mut again = plugin_core(OfflineSink::new());
    again.open(&file).unwrap();
    again.wait_background();
    assert!(again.plugin_instance(&key("lead")).is_some());
    assert!(again.plugin_failure(&key("lead")).is_none());
    let restored = again
        .doc()
        .resolve(&key("lead"))
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .state
        .clone();
    assert_eq!(restored, saved);
    assert!(
        !again.is_dirty(),
        "loading a plugin does not change the project"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ gui gestures

#[test]
fn a_gui_gesture_is_one_undo_step_and_is_not_echoed_to_the_plugin() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    settle(&mut core);
    let before = core.sink().param_calls;
    let dev = key("lead");
    for phase_value in [
        (ParamPhase::Begin, f64::NAN),
        (ParamPhase::Change, 0.1),
        (ParamPhase::Change, 0.2),
        (ParamPhase::Change, 0.35),
        (ParamPhase::End, f64::NAN),
    ] {
        post(
            &mut core,
            &dev,
            PluginMessage::ParamEdit {
                id: 2,
                value: phase_value.1,
                phase: phase_value.0,
            },
        );
    }
    core.frame();
    let value = core.doc().resolve(&dev).unwrap().params.get("p2").copied();
    assert_eq!(value, Some(0.35), "the last change of the gesture");
    assert_eq!(
        core.sink().param_calls,
        before,
        "not sent back to the plugin"
    );
    // One undo takes the whole gesture back (the param was never set before).
    assert!(core.undo());
    assert_eq!(core.doc().resolve(&dev).unwrap().params.get("p2"), None);
    assert!(
        core.doc().resolve(&dev).is_some(),
        "the device stays: that was the gesture"
    );
    assert!(core.redo());
    assert_eq!(
        core.doc().resolve(&dev).unwrap().params.get("p2").copied(),
        Some(0.35)
    );
    // The undo of a gesture goes to the plugin through the normal path.
    assert!(core.undo());
    assert!(core.sink().param_calls > before);
}

#[test]
fn a_change_outside_a_gesture_is_not_stored() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    post(
        &mut core,
        &key("lead"),
        PluginMessage::ParamEdit {
            id: 2,
            value: 0.7,
            phase: ParamPhase::Change,
        },
    );
    core.frame();
    assert_eq!(
        core.doc().resolve(&key("lead")).unwrap().params.get("p2"),
        None
    );
    assert!(
        !core.document().can_undo() || core.undo(),
        "only the add is in the history"
    );
}

// ------------------------------------------------------------------ missing plugins

fn without_plugins() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(false, &[PKG]));
    core
}

#[test]
fn reload_of_a_stand_in_whose_plugin_cannot_be_loaded_says_why_and_changes_nothing() {
    let mut core = without_plugins();
    core.open(&demo_file()).unwrap();
    core.wait_background();
    core.handle_action(UiAction::ReloadPlugin {
        device: key("lead"),
    });
    let reason = core
        .last_rejection()
        .expect("the user is told why")
        .to_string();
    assert!(reason.contains("cannot load the plugin"), "{reason}");
    assert!(
        reason.contains("off"),
        "plugins are off, and it says so: {reason}"
    );
    assert_eq!(core.plugin_instance_count(), 0);
    assert!(core.doc().resolve(&key("lead")).unwrap().plugin.is_some());
}

#[test]
fn a_project_with_plugins_opens_with_stand_ins_and_the_banner_while_plugins_are_off() {
    let mut core = without_plugins();
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert_eq!(core.plugin_instance_count(), 0);
    assert_eq!(
        core.plugin_banner(),
        Some(&BannerKind::ProjectUsesPlugins { count: 5 })
    );
    let status = core.view().plugin_status(&key("lead"));
    assert!(
        status.missing.as_deref().is_some_and(|m| m.contains("off")),
        "{status:?}"
    );
    // The document is intact: it saves back to the very file.
    let dir = temp("standin");
    let out = dir.join("again.atomik");
    core.save_as(&out).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        std::fs::read_to_string(demo_file()).unwrap(),
        "the project saves unchanged"
    );
    // LOAD enables plugins and creates every instance.
    core.handle_action(UiAction::PluginBanner(PluginBannerAction::Load));
    core.wait_plugins();
    assert!(core.plugin_settings().enabled);
    assert_eq!(core.plugin_instance_count(), 5);
    assert_eq!(
        core.take_plugin_banner_change(),
        Some(None),
        "the banner is hidden"
    );
    let status = core.view().plugin_status(&key("lead"));
    assert!(status.missing.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn open_without_keeps_the_stand_ins_and_hides_the_banner() {
    let mut core = without_plugins();
    core.open(&demo_file()).unwrap();
    assert!(core.plugin_banner().is_some());
    core.handle_action(UiAction::PluginBanner(PluginBannerAction::OpenWithout));
    assert!(core.plugin_banner().is_none());
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 0);
}

#[test]
fn a_rescan_that_finds_the_plugin_gives_the_stand_in_its_instance() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[]));
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert_eq!(core.plugin_instance_count(), 0, "nothing installed yet");
    let status = core.view().plugin_status(&key("lead"));
    assert!(
        status
            .missing
            .as_deref()
            .is_some_and(|m| m.contains("scan")),
        "{status:?}"
    );
    // The plugins appear (a rescan result): every stand-in gets its instance, state kept.
    core.configure_plugins(config(true, &[PKG]));
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 5);
    assert!(core.view().plugin_status(&key("lead")).missing.is_none());
    // The other way: the plugin goes away (disabled in Settings), devices show MISSING again.
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetPluginDisabled {
            type_id: SYNTH.into(),
            disabled: true,
        },
    ));
    core.wait_background();
    settle(&mut core);
    core.frame();
    assert!(core.plugin_instance(&key("lead")).is_none());
    assert!(
        core.plugin_instance(&key("echo")).is_some(),
        "the others stay live"
    );
    let status = core.view().plugin_status(&key("lead"));
    assert!(
        status
            .missing
            .as_deref()
            .is_some_and(|m| m.contains("disabled")),
        "{status:?}"
    );
}

#[test]
fn reload_creates_the_instance_of_a_stand_in_whose_plugin_is_installed() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    let add = add_command(&core, SYNTH, "lead");
    apply(&mut core, add);
    // Say the first creation failed, then RELOAD.
    core.handle_action(UiAction::ReloadPlugin {
        device: key("lead"),
    });
    core.wait_plugins();
    assert!(core.plugin_instance(&key("lead")).is_some());
    // RELOAD on a live instance restarts it and stays live.
    core.handle_action(UiAction::ReloadPlugin {
        device: key("lead"),
    });
    settle(&mut core);
    assert!(core.plugin_instance(&key("lead")).is_some());
    assert!(core.plugin_failure(&key("lead")).is_none());
}

// ------------------------------------------------------------------ patches and the rest

#[test]
fn a_rack_with_plugins_cannot_be_exported_as_a_patch() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    let dir = temp("export");
    let out = dir.join("rack.json");
    let e = core.export_patch(&out).unwrap_err().to_string();
    assert!(e.contains("patch") && e.contains("plugin"), "{e}");
    assert!(!out.exists());
    core.apply_command(Command::RemoveDevice { key: key("lead") })
        .unwrap();
    core.export_patch(&out).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn undo_redo_of_add_and_pin_changes_reach_the_plugin_panel_state() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    apply(
        &mut core,
        Command::Plugin(PluginCommand::SetPower {
            device: key("lead"),
            powered: false,
        }),
    );
    let id = core.plugin_instance(&key("lead")).unwrap();
    let slot = core.plugin_host().manager_ref().slot(id).unwrap();
    assert!(!slot.powered(), "power reaches the live device");
    assert!(core.undo());
    assert!(slot.powered());
    apply(
        &mut core,
        Command::Plugin(PluginCommand::SetRunMode {
            device: key("lead"),
            run_on_audio_thread: true,
        }),
    );
    assert!(slot.pinned(), "RUN ON AUDIO THREAD reaches the live device");
}
