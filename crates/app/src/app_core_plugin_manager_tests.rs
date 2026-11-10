//! The plugin manager in the app core: the rows it lists, the favourites, tags, disabled plugins
//! and quarantine kept by plugin id (they survive a rescan that moved the file), the counter of
//! failures in a row, a project that uses a disabled plugin, and the rescan of chosen files on the
//! scan worker. The plugins are the in-repo test plugins; no plugin of this machine is scanned.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use crate::plugins_user::{PluginsUser, QUARANTINE_AFTER};
use atomik_model::empty_doc;
use atomik_plugin_host::{
    CachedFile, FailureKind, ManagerEvent, PluginCache, PluginSettings, ScanFailure, ScanResult,
    ScanRunner, ScannedPlugin,
};
use atomik_ui::plugin_data::PluginEntryStatus;
use atomik_ui::{PluginManagerAction as Act, PluginManagerRow};
use std::sync::{Arc, Mutex};

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo/plugin-demo.atomik")
}

fn row_of(core: &AppCore<OfflineSink>, type_id: &str) -> PluginManagerRow {
    core.plugin_manager_data()
        .rows
        .into_iter()
        .find(|r| r.type_id.as_deref() == Some(type_id))
        .unwrap_or_else(|| panic!("{type_id} has a row"))
}

fn crash(core: &mut AppCore<OfflineSink>, device: &str, reason: &str) {
    let instance = core.plugin_instance(&key(device)).expect("a live instance");
    core.plugin_event(ManagerEvent::Crashed {
        instance,
        reason: reason.to_owned(),
    });
}

#[test]
fn the_manager_lists_every_plugin_with_what_the_plugin_says_about_itself() {
    let core = plugin_core(OfflineSink::new());
    let data = core.plugin_manager_data();
    assert!(data.plugins_on && !data.scanner_available && data.scan.is_none());
    let synth = row_of(&core, SYNTH);
    assert_eq!(synth.category, "Instrument");
    assert_eq!(synth.format, atomik_device_sdk::PluginFormat::Clap);
    assert_eq!(synth.status, PluginEntryStatus::Ok);
    assert!(!synth.name.is_empty() && !synth.path.is_empty());
    assert_eq!(row_of(&core, GAIN).category, "Effect");
    assert_eq!(row_of(&core, SHIFT).category, "Note effect");
    assert!(
        data.rows.iter().all(|r| !r.favourite && r.tags.is_empty()),
        "nothing is marked yet"
    );
}

#[test]
fn favourites_and_tags_are_marks_on_the_rows_saved_once_and_listed_for_the_browser() {
    let mut core = plugin_core(OfflineSink::new());
    assert!(core.take_plugins_user_change().is_none(), "nothing changed");
    core.plugin_manager_action(&Act::Favourite {
        type_id: SYNTH.into(),
        on: true,
    });
    core.plugin_manager_action(&Act::AddTag {
        type_ids: vec![SYNTH.into(), GAIN.into()],
        tag: "  Warm ".into(),
    });
    core.plugin_manager_action(&Act::AddTag {
        type_ids: vec![GAIN.into()],
        tag: "warm".into(),
    });
    let synth = row_of(&core, SYNTH);
    assert!(synth.favourite);
    assert_eq!(synth.tags, ["Warm"]);
    assert_eq!(
        row_of(&core, GAIN).tags,
        ["Warm"],
        "the same tag is added once"
    );
    let saved = core.take_plugins_user_change().expect("one change to save");
    assert!(saved.is_favourite(SYNTH) && saved.has_tag(GAIN, "WARM"));
    assert!(core.take_plugins_user_change().is_none(), "reported once");
    // A tag that is not valid is refused with a message and adds nothing.
    core.plugin_manager_action(&Act::AddTag {
        type_ids: vec![SHIFT.into()],
        tag: "a#b".into(),
    });
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("cannot contain"))
    );
    assert!(row_of(&core, SHIFT).tags.is_empty());
    core.plugin_manager_action(&Act::RemoveTag {
        type_id: SYNTH.into(),
        tag: "warm".into(),
    });
    assert!(row_of(&core, SYNTH).tags.is_empty());
    core.plugin_manager_action(&Act::Favourite {
        type_id: SYNTH.into(),
        on: false,
    });
    assert!(!row_of(&core, SYNTH).favourite);
}

#[test]
fn the_marks_follow_the_plugin_id_when_a_rescan_finds_the_file_somewhere_else() {
    let dir = temp("moved");
    let mut core = plugin_core(OfflineSink::new());
    core.plugin_manager_action(&Act::Favourite {
        type_id: SYNTH.into(),
        on: true,
    });
    core.plugin_manager_action(&Act::AddTag {
        type_ids: vec![SYNTH.into()],
        tag: "pads".into(),
    });
    core.plugin_manager_action(&Act::Disable {
        type_ids: vec![GAIN.into()],
        disabled: true,
    });
    let before = row_of(&core, SYNTH).path;
    // The same plugins, scanned from other paths.
    let mut moved = PluginCache::default();
    for file in cache_of(&[PKG]).files() {
        let mut file: CachedFile = file.clone();
        file.path = dir.join("elsewhere").join(file.path.file_name().unwrap());
        moved.insert_file(file);
    }
    let mut cfg = config(true, &[]);
    cfg.cache = moved;
    core.configure_plugins(cfg);
    let synth = row_of(&core, SYNTH);
    assert_ne!(synth.path, before, "the file moved");
    assert!(synth.favourite, "the star stays");
    assert_eq!(synth.tags, ["pads"], "and the tags");
    assert_eq!(row_of(&core, GAIN).status, PluginEntryStatus::Disabled);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_disabled_plugin_is_hidden_but_a_project_that_uses_it_still_loads_it() {
    let mut core = plugin_core(OfflineSink::new());
    let all: Vec<String> = [SYNTH, GAIN, SHIFT].map(String::from).to_vec();
    core.plugin_manager_action(&Act::Disable {
        type_ids: all.clone(),
        disabled: true,
    });
    for id in [SYNTH, GAIN, SHIFT] {
        assert_eq!(row_of(&core, id).status, PluginEntryStatus::Disabled);
        // Still in the catalog and the registry: only the lists hide it.
        assert!(core.registry().is_plugin(id), "{id}");
    }
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
        "the project's plugins are live although they are hidden"
    );
    let status = core.view().plugin_status(&key("lead"));
    assert!(status.missing.is_none(), "{status:?}");
    // ENABLE shows them again.
    core.plugin_manager_action(&Act::Disable {
        type_ids: all,
        disabled: false,
    });
    assert_eq!(row_of(&core, SYNTH).status, PluginEntryStatus::Ok);
}

#[test]
fn enable_also_lifts_the_older_disable_switch_of_the_plugin_settings() {
    let mut core = plugin_core(OfflineSink::new());
    core.handle_action(UiAction::PluginSettings(
        atomik_ui::plugin_data::PluginSettingsAction::SetPluginDisabled {
            type_id: GAIN.into(),
            disabled: true,
        },
    ));
    assert_eq!(row_of(&core, GAIN).status, PluginEntryStatus::Disabled);
    core.plugin_manager_action(&Act::Disable {
        type_ids: vec![GAIN.into()],
        disabled: false,
    });
    assert_eq!(row_of(&core, GAIN).status, PluginEntryStatus::Ok);
}

#[test]
fn three_crashes_in_a_row_quarantine_a_plugin_and_say_so_once() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    for n in 1..QUARANTINE_AFTER {
        crash(&mut core, "lead", "the plugin process exited with code 3");
        assert_eq!(core.plugins_user().failures(SYNTH), n);
        assert!(row_of(&core, SYNTH).status.reason().is_some());
        assert!(
            !core
                .last_rejection()
                .is_some_and(|m| m.contains("quarantined")),
            "not yet"
        );
    }
    crash(&mut core, "lead", "the plugin process exited with code 3");
    assert!(core.plugins_user().is_quarantined(SYNTH));
    assert_eq!(row_of(&core, SYNTH).status, PluginEntryStatus::Quarantined);
    let message = core.last_rejection().expect("a message").to_owned();
    assert!(message.contains("quarantined"), "{message}");
    assert!(message.contains("3 failures"), "{message}");
    assert!(core.take_plugins_user_change().is_some(), "it is saved");
    // Nothing is said twice: another crash does not repeat it.
    core.report("something else");
    crash(&mut core, "lead", "again");
    assert_eq!(core.last_rejection(), Some("something else"));
    // The plugin is still registered and its device still has its instance.
    assert!(core.registry().is_plugin(SYNTH));
    assert!(core.plugin_instance(&key("lead")).is_some());
}

#[test]
fn a_clean_end_starts_the_count_again_and_try_again_clears_the_quarantine() {
    let mut core = plugin_core(OfflineSink::new());
    core.plugin_failed_in_a_row(SYNTH);
    core.plugin_failed_in_a_row(SYNTH);
    assert_eq!(core.plugins_user().failures(SYNTH), 2);
    // An instance that lives and ends well is a success.
    add_plugin(&mut core, SYNTH, "lead");
    apply(&mut core, Command::RemoveDevice { key: key("lead") });
    for _ in 0..8 {
        core.frame();
        core.sink_mut().render(128);
    }
    assert_eq!(core.plugins_user().failures(SYNTH), 0, "reset by the end");
    // Two more are not three in a row.
    core.plugin_failed_in_a_row(SYNTH);
    core.plugin_failed_in_a_row(SYNTH);
    assert!(!core.plugins_user().is_quarantined(SYNTH));
    core.plugin_worked(SYNTH);
    assert_eq!(core.plugins_user().failures(SYNTH), 0);
    for _ in 0..QUARANTINE_AFTER {
        core.plugin_failed_in_a_row(SYNTH);
    }
    assert!(core.plugins_user().is_quarantined(SYNTH));
    // TRY AGAIN: out of quarantine, the CRASHED mark of the settings gone too.
    core.plugin_manager_action(&Act::TryAgain(vec![SYNTH.into()]));
    assert!(!core.plugins_user().is_quarantined(SYNTH));
    assert_eq!(core.plugins_user().failures(SYNTH), 0);
    assert_eq!(row_of(&core, SYNTH).status, PluginEntryStatus::Ok);
}

#[test]
fn a_crash_is_a_crashed_row_until_try_again_and_the_quarantine_wins_over_it() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    crash(&mut core, "lead", "exit code 3");
    assert!(matches!(
        row_of(&core, SYNTH).status,
        PluginEntryStatus::Crashed(ref r) if r.contains("exit code 3")
    ));
    core.plugin_manager_action(&Act::TryAgain(vec![SYNTH.into()]));
    assert_eq!(row_of(&core, SYNTH).status, PluginEntryStatus::Ok);
}

#[test]
fn the_choices_are_restored_from_the_store_the_shell_read() {
    let mut core = plugin_core(OfflineSink::new());
    let mut user = PluginsUser::default();
    user.set_favourite(SHIFT, true);
    user.set_disabled(GAIN, true);
    core.set_plugins_user(user);
    assert!(
        core.take_plugins_user_change().is_none(),
        "a load is not a change"
    );
    assert!(row_of(&core, SHIFT).favourite);
    assert_eq!(row_of(&core, GAIN).status, PluginEntryStatus::Disabled);
}

/// Calls every file good and remembers what it was asked to scan.
struct Recording {
    seen: Mutex<Vec<String>>,
    plugin: ScannedPlugin,
}

impl ScanRunner for Recording {
    fn scan(
        &self,
        format: atomik_device_sdk::PluginFormat,
        path: &Path,
        _timeout: std::time::Duration,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        self.seen.lock().unwrap().push(name.clone());
        if name.starts_with("bad") {
            return Err(ScanFailure {
                kind: FailureKind::Crash,
                reason: "the scanner exited with code 3".to_owned(),
            });
        }
        let mut plugin = self.plugin.clone();
        plugin.plugin.id = format!("test.{name}");
        plugin.plugin.format = format;
        Ok(ScanResult {
            path: path.to_path_buf(),
            format,
            plugins: vec![plugin],
            error: None,
        })
    }
}

fn scanning_core(dir: &Path) -> (AppCore<OfflineSink>, Arc<Recording>) {
    let folder = dir.join("plugins");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["one.clap", "two.clap", "bad.clap"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let plugin = cache_of(&[PKG])
        .files()
        .flat_map(|f| f.plugins.clone())
        .next()
        .unwrap();
    let runner = Arc::new(Recording {
        seen: Mutex::new(Vec::new()),
        plugin,
    });
    let cfg = PluginConfig {
        settings: PluginSettings {
            enabled: true,
            extra_paths: vec![folder],
            ..PluginSettings::default()
        },
        cache: PluginCache::default(),
        paths: PluginPaths::default(),
        runner: Some(runner.clone()),
        previous_crash: None,
        isolation: None,
    };
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(cfg);
    // Only the folder the settings list: not this machine's plugin folders.
    core.set_plugin_scan_env(Arc::new(|_: &str| None));
    (core, runner)
}

#[test]
fn rescan_all_and_rescan_selected_run_on_the_scan_worker_and_visit_only_what_was_asked() {
    let dir = temp("rescan");
    let (mut core, runner) = scanning_core(&dir);
    core.plugin_manager_action(&Act::RescanAll);
    assert!(core.plugin_scan_running() || core.plugin_scan_note().is_some());
    core.wait_scan();
    let first = runner.seen.lock().unwrap().len();
    assert_eq!(first, 3, "every file");
    let data = core.plugin_manager_data();
    assert_eq!(data.rows.len(), 3, "two plugins and the failed file");
    assert!(data.scanner_available && data.scan.is_none());
    let two = data
        .rows
        .iter()
        .find(|r| r.path.ends_with("two.clap"))
        .expect("the plugin of two.clap");
    assert!(two.scanned_secs > 0, "the scan time is shown");
    assert!(
        two.scanned.len() >= 16,
        "as a date and time: {}",
        two.scanned
    );
    let failed = data.rows.iter().find(|r| r.type_id.is_none()).unwrap();
    assert!(matches!(&failed.status, PluginEntryStatus::Failed(r) if r.contains("code 3")));
    // Only the chosen file is scanned again.
    runner.seen.lock().unwrap().clear();
    core.plugin_manager_action(&Act::RescanFiles(vec![two.path.clone()]));
    core.wait_scan();
    assert_eq!(*runner.seen.lock().unwrap(), ["two.clap"]);
    // A failed file asked again is scanned again (it fails again here).
    runner.seen.lock().unwrap().clear();
    core.plugin_manager_action(&Act::RescanFiles(vec![failed.path.clone()]));
    core.wait_scan();
    assert_eq!(*runner.seen.lock().unwrap(), ["bad.clap"]);
    assert_eq!(core.plugin_manager_data().rows.len(), 3);
    // Cancel with nothing running does nothing.
    core.plugin_manager_action(&Act::CancelScan);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rescan_that_cannot_run_forgets_nothing() {
    // Plugins are on but no scanner can be started: the files stay as they were.
    let mut core = plugin_core(OfflineSink::new());
    let before = core.plugin_manager_data();
    let paths: Vec<String> = before.rows.iter().map(|r| r.path.clone()).collect();
    core.plugin_manager_action(&Act::RescanFiles(paths));
    assert_eq!(core.plugin_manager_data().rows.len(), before.rows.len());
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("no plugin scanner")),
        "{:?}",
        core.last_rejection()
    );
    assert!(
        core.plugin_cache().files().count() > 0,
        "the cache is whole"
    );
}
