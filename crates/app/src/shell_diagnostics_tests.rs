//! `Shell` tests for Help > Export Diagnostics: the command asks for a save dialog, and the
//! report written to the picked file holds the session and hides the home folder.

use super::*;
use crate::session::NullBackend;

fn shell(settings: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: settings,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    )
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-shell-diag-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_help_menu_asks_for_a_save_dialog() {
    let mut s = shell(None);
    let requests = s.execute(AppCommand::ExportDiagnostics);
    assert!(matches!(
        requests.as_slice(),
        [ShellRequest::PickExportDiagnostics]
    ));
    let menus = crate::commands::menus(&s.menu_state(), s.keymap());
    let help = menus.iter().find(|m| m.title == "Help").unwrap();
    let labels: Vec<_> = help.items().map(|i| i.label.as_ref()).collect();
    assert!(labels.contains(&"Export Diagnostics\u{2026}"), "{labels:?}");
}

#[test]
fn the_exported_report_has_the_session_and_the_log_of_the_config_folder() {
    let dir = scratch("export");
    let logs = crate::logging::log_dir(&dir);
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join(crate::logging::LOG_FILE), "a line from the log\n").unwrap();
    let settings = dir.join("settings.json");
    std::fs::write(&settings, "{}").unwrap();
    let mut s = shell(Some(settings));
    assert_eq!(s.log_folder(), logs);
    let out = dir.join("report.txt");
    s.export_diagnostics_to(&out);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("Atomik Rack diagnostics"), "{text}");
    assert!(text.contains("a line from the log"), "{text}");
    assert!(text.contains("overload counters:"), "{text}");
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Diagnostics written"),
        "{:?}",
        s.core().last_rejection()
    );
    // A path that cannot be written is a message, not a crash.
    s.export_diagnostics_to(&dir.join("missing").join("report.txt"));
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Could not write"),
        "{:?}",
        s.core().last_rejection()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_overload_counters_start_clear_and_follow_the_status_poll() {
    let mut s = shell(None);
    s.poll_status();
    assert!(s.overload().is_clear());
}

#[test]
fn growing_counters_are_logged_at_once_and_then_at_most_every_twenty_polls() {
    let mut s = shell(None);
    s.overload.dropped_events = 3;
    s.log_overload();
    assert_eq!(
        s.overload_logged.dropped_events, 3,
        "the first growth is logged"
    );
    s.overload.dropped_events = 9;
    for _ in 0..10 {
        s.log_overload();
    }
    assert_eq!(s.overload_logged.dropped_events, 3, "held back");
    for _ in 0..10 {
        s.log_overload();
    }
    assert_eq!(
        s.overload_logged.dropped_events, 9,
        "logged on the 20th poll"
    );
    // A rebuilt engine starts its counters again: the new baseline, no record.
    s.overload = crate::status::OverloadStats::default();
    s.log_overload();
    assert!(s.overload_logged.is_clear());
}
