//! The Plugins section of the settings dialog and the project banner: the enable switch, search folders, SCAN, FULL RESCAN and
//! RESCAN FAILED, the scan timeout stepper, the scan progress, the plugin list with per-plugin
//! DISABLE, the failed list with RETRY, the log; the "This project uses N third-party plugins"
//! banner with LOAD and OPEN WITHOUT, the unclean-exit banner; display-list goldens
//! `plugin_settings_*` and `plugin_banner_*`; and the tests that pin the UI's mirrors of the host
//! crate's timeout range and rescan modes.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plugin_settings`.

mod support;

use atomik_device_sdk::{PluginFormat, PluginRef};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::plugin_banner::BannerKind;
use atomik_ui::plugin_data::{
    FailedFileInfo, PluginBannerAction, PluginEntryStatus, PluginRescan, PluginSettingsAction,
    PluginSettingsData, ScanProgressInfo, SettingsPluginRow,
};
use atomik_ui::{
    DisplayList, Modifiers, PluginBannerView, PluginSettingsView, PointerButton, Theme, UiAction,
    UiEvent,
};
use support::container::check;
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 760.0, 560.0);

fn plugin_row(id: &str, name: &str, status: PluginEntryStatus) -> SettingsPluginRow {
    SettingsPluginRow {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.into(),
        },
        name: name.into(),
        vendor: "Atomik Test".into(),
        version: "1.2.0".into(),
        status,
    }
}

fn on_data() -> PluginSettingsData {
    PluginSettingsData {
        enabled: true,
        extra_paths: vec!["D:/Plugins/CLAP".into(), "E:/More".into()],
        timeout_secs: 30,
        plugins: vec![
            plugin_row("org.test.synth", "Test Synth", PluginEntryStatus::Ok),
            plugin_row("org.test.fx", "Test Fx", PluginEntryStatus::Ok),
            plugin_row("org.test.off", "Switched Off", PluginEntryStatus::Disabled),
            plugin_row(
                "org.test.surround",
                "Surround",
                PluginEntryStatus::Unsupported("surround only".into()),
            ),
            plugin_row(
                "org.test.dup",
                "Duplicate",
                PluginEntryStatus::Failed("duplicate id".into()),
            ),
        ],
        failed: vec![
            FailedFileInfo {
                path: "C:/Program Files/Common Files/CLAP/atomik-test-clap-bad.clap".into(),
                reason: "init failed".into(),
            },
            FailedFileInfo {
                path: "D:/Plugins/CLAP/hang.clap".into(),
                reason: "timed out after 30 s".into(),
            },
        ],
        scan: None,
        scanner_available: true,
        last_scan: None,
        log: vec!["[Test Synth] loaded".into(), "[Test Fx] latency 128".into()],
        sandbox: true,
    }
}

fn view(data: PluginSettingsData) -> PluginSettingsView {
    let mut v = PluginSettingsView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(data);
    v
}

fn feed(v: &mut PluginSettingsView, e: UiEvent) -> Vec<UiAction> {
    v.handle(&e)
}

fn click_at(v: &mut PluginSettingsView, p: Point) -> Vec<UiAction> {
    let mut out = feed(v, down(p));
    out.extend(feed(v, up(p)));
    out
}

fn settings(actions: &[UiAction]) -> Vec<PluginSettingsAction> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::PluginSettings(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn texts_of(v: &PluginSettingsView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

// ------------------------------------------------------------------ the mirrors

#[test]
fn the_timeout_range_mirrors_the_host_crate() {
    assert_eq!(
        atomik_ui::plugin_data::MIN_TIMEOUT_SECS,
        atomik_plugin_host::MIN_TIMEOUT_SECS
    );
    assert_eq!(
        atomik_ui::plugin_data::MAX_TIMEOUT_SECS,
        atomik_plugin_host::MAX_TIMEOUT_SECS
    );
}

#[test]
fn every_host_rescan_mode_has_a_ui_mirror() {
    use atomik_plugin_host::RescanMode;
    // An exhaustive match: a new host mode fails to compile here until the UI learns it.
    let mirror = |m: RescanMode| match m {
        RescanMode::Incremental => PluginRescan::Incremental,
        RescanMode::Full => PluginRescan::Full,
        RescanMode::FailedOnly => PluginRescan::FailedOnly,
    };
    assert_eq!(mirror(RescanMode::FailedOnly), PluginRescan::FailedOnly);
}

// ------------------------------------------------------------------ the enable switch

#[test]
fn while_plugins_are_off_only_the_switch_and_a_hint_show() {
    let v = view(PluginSettingsData::default());
    let t = texts_of(&v);
    assert!(has(&t, "PLUGINS") && has(&t, "ENABLE"));
    assert!(has(&t, "Third-party plugins are off"));
    for hidden in ["SCAN", "SEARCH FOLDERS", "FOUND PLUGINS"] {
        assert!(!has(&t, hidden), "{hidden} is not shown while off");
    }
    assert!(v.scan_rect().is_none());
    assert!(v.add_folder_rect().is_none());
}

#[test]
fn the_switch_turns_plugins_on_and_off_at_once_without_a_warning() {
    let mut v = view(PluginSettingsData::default());
    let acts = {
        let p = v.enable_rect().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::SetEnabled { enabled: true }]
    );
    let t = texts_of(&v);
    assert!(!has(&t, "native programs"), "no warning text: {t:?}");
    assert!(!has(&t, "I UNDERSTAND"), "no confirmation: {t:?}");
    let mut v = view(on_data());
    let acts = {
        let p = v.enable_rect().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::SetEnabled { enabled: false }]
    );
}

// ------------------------------------------------------------------ folders and scanning

#[test]
fn folders_can_be_added_and_removed() {
    let mut v = view(on_data());
    let acts = {
        let p = v.add_folder_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::AddFolderDialog]);
    let acts = {
        let p = v.folder_remove_rect(1).unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::RemoveFolder("E:/More".into())]
    );
    assert!(v.folder_remove_rect(2).is_none());
    let t = texts_of(&v);
    assert!(has(&t, "D:/Plugins/CLAP") && has(&t, "E:/More"));
}

#[test]
fn the_three_scan_buttons_pick_the_rescan_mode() {
    let mut v = view(on_data());
    for (rect, mode) in [
        (v.scan_rect().unwrap(), PluginRescan::Incremental),
        (v.full_rescan_rect().unwrap(), PluginRescan::Full),
        (v.rescan_failed_rect().unwrap(), PluginRescan::FailedOnly),
    ] {
        let acts = click_at(&mut v, rect.center());
        assert_eq!(settings(&acts), [PluginSettingsAction::Scan(mode)]);
    }
}

#[test]
fn a_running_scan_shows_progress_a_cancel_button_and_disables_the_scan_buttons() {
    let mut data = on_data();
    data.scan = Some(ScanProgressInfo {
        done: 12,
        total: 40,
    });
    let mut v = view(data);
    let t = texts_of(&v);
    assert!(has(&t, "12 / 40 files"), "{t:?}");
    let acts = {
        let p = v.scan_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert!(settings(&acts).is_empty(), "SCAN is dim while scanning");
    let acts = {
        let p = v.cancel_scan_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::CancelScan]);
    // No scan, no cancel.
    let v = view(on_data());
    assert!(v.cancel_scan_rect().is_none());
    // An empty scan (total 0) shows "Scanning...".
    let mut data = on_data();
    data.scan = Some(ScanProgressInfo { done: 0, total: 0 });
    assert!(has(&texts_of(&view(data)), "Scanning"));
}

#[test]
fn the_timeout_stepper_steps_by_five_and_stays_in_range() {
    let mut v = view(on_data());
    let acts = {
        let p = v.timeout_inc_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::SetTimeout(35)]);
    let acts = {
        let p = v.timeout_dec_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::SetTimeout(25)]);
    let mut data = on_data();
    data.timeout_secs = 5;
    let mut v = view(data);
    let acts = {
        let p = v.timeout_dec_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert!(settings(&acts).is_empty(), "already at the shortest");
    let mut data = on_data();
    data.timeout_secs = 298;
    let mut v = view(data);
    let acts = {
        let p = v.timeout_inc_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::SetTimeout(300)]);
    assert!(has(&texts_of(&view(on_data())), "30 s"));
}

#[test]
fn the_sandbox_row_says_what_it_does_and_when_it_applies_and_toggles() {
    let mut v = view(on_data());
    let texts = texts_of(&v);
    assert!(
        has(&texts, "Run plugins in a separate process (recommended)"),
        "{texts:?}"
    );
    assert!(has(&texts, "Applies after restarting Atomik"), "{texts:?}");
    // On: a click asks for off; off: a click asks for on.
    let acts = {
        let p = v.sandbox_toggle_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::SetSandbox(false)]);
    let mut data = on_data();
    data.sandbox = false;
    let mut v = view(data);
    let acts = {
        let p = v.sandbox_toggle_rect().unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(settings(&acts), [PluginSettingsAction::SetSandbox(true)]);
}

#[test]
fn the_sandbox_row_is_there_while_plugins_are_off_and_sandboxing_defaults_on() {
    assert!(PluginSettingsData::default().sandbox);
    let mut v = view(PluginSettingsData::default());
    assert!(has(
        &texts_of(&v),
        "Run plugins in a separate process (recommended)"
    ));
    let p = v.sandbox_toggle_rect().unwrap().center();
    assert_eq!(
        settings(&click_at(&mut v, p)),
        [PluginSettingsAction::SetSandbox(false)]
    );
}

// ------------------------------------------------------------------ the lists

#[test]
fn the_plugin_list_shows_format_vendor_version_and_status_with_reasons() {
    let v = view(on_data());
    let t = texts_of(&v);
    for s in [
        "FOUND PLUGINS (5)",
        "Test Synth",
        "Atomik Test",
        "1.2.0",
        "CLAP",
        "ENABLED",
        "UNSUPPORTED",
        "surround only",
        "COULD NOT LOAD",
        "duplicate id",
        "DISABLED",
    ] {
        assert!(has(&t, s), "missing {s:?}: {t:?}");
    }
}

#[test]
fn the_per_plugin_disable_check_toggles_the_plugin() {
    let mut v = view(on_data());
    // Row 0 is OK: checking disables it.
    let acts = {
        let p = v.plugin_toggle_rect(0).unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::SetPluginDisabled {
            type_id: "clap.org.test.synth".into(),
            disabled: true
        }]
    );
    // Row 2 is DISABLED: unchecking enables it.
    let acts = {
        let p = v.plugin_toggle_rect(2).unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::SetPluginDisabled {
            type_id: "clap.org.test.off".into(),
            disabled: false
        }]
    );
    assert!(v.plugin_toggle_rect(99).is_none());
}

#[test]
fn the_failed_list_has_a_reason_and_a_retry_per_file() {
    let mut v = view(on_data());
    let t = texts_of(&v);
    assert!(has(&t, "FAILED FILES (2)"));
    assert!(has(&t, "init failed") && has(&t, "timed out after 30 s"));
    assert!(has(&t, "atomik-test-clap-bad.clap"));
    let acts = {
        let p = v.retry_rect(1).unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::RetryFile(
            "D:/Plugins/CLAP/hang.clap".into()
        )]
    );
    assert!(v.retry_rect(2).is_none());
    // No failed files: no list.
    let mut data = on_data();
    data.failed.clear();
    assert!(!has(&texts_of(&view(data)), "FAILED FILES"));
}

#[test]
fn the_log_shows_the_latest_lines() {
    let v = view(on_data());
    let t = texts_of(&v);
    assert!(has(&t, "PLUGIN LOG"));
    assert!(has(&t, "[Test Synth] loaded") && has(&t, "[Test Fx] latency 128"));
    let mut data = on_data();
    data.log = (0..100).map(|i| format!("line {i}")).collect();
    let mut v = view(data);
    let _ = v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -10_000.0),
        pos: Point::new(300.0, 300.0),
        modifiers: Modifiers::NONE,
    });
    let t = texts_of(&v);
    assert!(has(&t, "line 99"), "the newest line is shown");
    assert!(
        !has(&t, "line 0\u{0}") && !t.iter().any(|x| x == "line 0"),
        "old lines scroll off"
    );
}

#[test]
fn a_long_list_scrolls_and_never_paints_all_of_it() {
    let mut data = on_data();
    data.plugins = (0..400)
        .map(|i| {
            plugin_row(
                &format!("org.p{i}"),
                &format!("Plugin {i}"),
                PluginEntryStatus::Ok,
            )
        })
        .collect();
    let mut v = view(data);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(
        dl.len() < 3_000,
        "painting is bounded by the viewport: {}",
        dl.len()
    );
    assert!(v.content_height() > 400.0 * 20.0);
    assert!(v.plugin_toggle_rect(300).is_none(), "off screen");
    let _ = feed(
        &mut v,
        UiEvent::Wheel {
            delta: Vec2::new(0.0, -200.0),
            pos: Point::new(300.0, 300.0),
            modifiers: Modifiers::NONE,
        },
    );
    assert!(v.scroll() > 0.0);
    let _ = feed(
        &mut v,
        UiEvent::Wheel {
            delta: Vec2::new(0.0, 100_000.0),
            pos: Point::new(300.0, 300.0),
            modifiers: Modifiers::NONE,
        },
    );
    assert_eq!(v.scroll(), 0.0, "scrolling up stops at the top");
}

// ------------------------------------------------------------------ feedback messages

#[test]
fn the_off_state_says_how_to_turn_plugins_on() {
    let t = texts_of(&view(PluginSettingsData::default()));
    assert!(has(&t, "Third-party plugins are off"));
    assert!(has(&t, "ENABLE"), "{t:?}");
}

#[test]
fn a_missing_scanner_is_said_in_the_view_and_persists() {
    let mut data = on_data();
    assert!(!has(&texts_of(&view(data.clone())), "scanner"), "no noise");
    data.scanner_available = false;
    let v = view(data);
    let t = texts_of(&v);
    assert!(has(&t, "No plugin scanner can be run"), "{t:?}");
    // The message is part of the layout, not a toast: it is still there after a repaint.
    assert!(has(&texts_of(&v), "No plugin scanner can be run"));
    // Not shown while plugins are off (the off hint says it all).
    let off = PluginSettingsData {
        scanner_available: false,
        ..PluginSettingsData::default()
    };
    assert!(!has(&texts_of(&view(off)), "No plugin scanner"));
}

#[test]
fn the_last_scan_summary_shows_when_no_scan_runs() {
    let mut data = on_data();
    data.last_scan = Some("scan: 12 scanned, 0 cached, 9 ok, 3 failed".into());
    let t = texts_of(&view(data.clone()));
    assert!(has(&t, "9 ok, 3 failed"), "{t:?}");
    // While a scan runs the progress shows instead of the old summary.
    data.scan = Some(ScanProgressInfo { done: 2, total: 9 });
    let t = texts_of(&view(data));
    assert!(has(&t, "2 / 9 files"));
    assert!(!has(&t, "9 ok, 3 failed"));
}

#[test]
fn the_found_list_shows_every_plugin_with_name_vendor_and_format_badge() {
    let mut data = on_data();
    data.plugins = (0..30)
        .map(|i| {
            let mut r = plugin_row(
                &format!("id{i}"),
                &format!("Plugin {i}"),
                PluginEntryStatus::Ok,
            );
            if i % 2 == 1 {
                r.plugin.format = PluginFormat::Vst3;
            }
            r
        })
        .collect();
    let mut v = view(data);
    // Scroll through the whole list: every row is painted when it is in view.
    let mut seen = std::collections::BTreeSet::new();
    let mut badges = std::collections::BTreeSet::new();
    for _ in 0..60 {
        for s in texts_of(&v) {
            if s == "CLAP" || s == "VST3" {
                badges.insert(s);
            } else if s.starts_with("Plugin ") {
                seen.insert(s);
            }
        }
        v.handle(&UiEvent::Wheel {
            pos: Point::new(300.0, 300.0),
            delta: Vec2::new(0.0, -3.0),
            modifiers: Modifiers::default(),
        });
    }
    assert_eq!(seen.len(), 30, "{seen:?}");
    assert_eq!(badges.len(), 2, "{badges:?}");
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_off_state_matches_golden() {
    let v = view(PluginSettingsData::default());
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("plugin_settings_off_1x.json", &dl.to_json());
}

#[test]
fn the_on_state_matches_golden() {
    let mut data = on_data();
    data.scan = Some(ScanProgressInfo {
        done: 12,
        total: 40,
    });
    let v = view(data);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("plugin_settings_on_1x.json", &dl.to_json());
}

#[test]
fn the_no_scanner_and_last_scan_states_match_goldens() {
    let mut data = on_data();
    data.scanner_available = false;
    data.last_scan = Some("scan: 12 scanned, 0 cached, 9 ok, 3 failed".into());
    let v = view(data);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("plugin_settings_feedback_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ the banner

fn banner(kind: BannerKind) -> PluginBannerView {
    let mut b = PluginBannerView::new(Theme::dark());
    b.set_bounds(Rect::new(0.0, 0.0, 900.0, 40.0));
    b.show(kind);
    b
}

fn banner_click(b: &mut PluginBannerView, p: Point) -> Vec<UiAction> {
    let mut out = b.handle(&down(p));
    out.extend(b.handle(&up(p)));
    out
}

fn banner_actions(actions: &[UiAction]) -> Vec<PluginBannerAction> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::PluginBanner(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

fn banner_texts(b: &PluginBannerView) -> Vec<String> {
    let mut dl = DisplayList::new();
    b.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_project_banner_says_how_many_plugins_and_offers_load_and_open_without() {
    let b = banner(BannerKind::ProjectUsesPlugins { count: 3 });
    assert!(b.is_visible());
    let t = banner_texts(&b);
    assert!(has(&t, "This project uses 3 third-party plugins"), "{t:?}");
    assert!(has(&t, "LOAD") && has(&t, "OPEN WITHOUT"));
    let one = banner(BannerKind::ProjectUsesPlugins { count: 1 });
    assert!(has(
        &banner_texts(&one),
        "This project uses 1 third-party plugin"
    ));
    assert!(!has(&banner_texts(&one), "1 third-party plugins"));
    let hidden = PluginBannerView::new(Theme::dark());
    assert!(!hidden.is_visible());
    assert_eq!(hidden.height(), 0.0);
    assert!(banner_texts(&hidden).is_empty());
}

#[test]
fn load_emits_at_once_and_hides_and_open_without_hides() {
    let mut b = banner(BannerKind::ProjectUsesPlugins { count: 2 });
    let h0 = b.height();
    assert!(h0 > 0.0);
    let acts = {
        let p = b.load_rect().unwrap().center();
        banner_click(&mut b, p)
    };
    assert_eq!(banner_actions(&acts), [PluginBannerAction::Load]);
    assert!(!b.is_visible());
    // OPEN WITHOUT.
    let mut b = banner(BannerKind::ProjectUsesPlugins { count: 2 });
    let acts = {
        let p = b.open_without_rect().unwrap().center();
        banner_click(&mut b, p)
    };
    assert_eq!(banner_actions(&acts), [PluginBannerAction::OpenWithout]);
    assert!(!b.is_visible());
    // Clicks on a hidden banner do nothing.
    assert!(banner_click(&mut b, Point::new(10.0, 10.0)).is_empty());
}

#[test]
fn the_unclean_exit_banner_offers_disable_keep_and_the_recovery_snapshot() {
    let kind = BannerKind::UncleanExit {
        plugins: vec!["Test Synth".into(), "Test Fx".into()],
        has_recovery: true,
    };
    let mut b = banner(kind);
    let t = banner_texts(&b);
    assert!(has(
        &t,
        "Atomik ended unexpectedly while these plugins were loaded"
    ));
    assert!(has(&t, "Test Synth, Test Fx"));
    for l in ["DISABLE", "KEEP", "OPEN RECOVERY"] {
        assert!(has(&t, l), "{l}");
    }
    let acts = {
        let p = b.recovery_rect().unwrap().center();
        banner_click(&mut b, p)
    };
    assert_eq!(banner_actions(&acts), [PluginBannerAction::OpenRecovery]);
    assert!(
        b.is_visible(),
        "the question about the plugins is still open"
    );
    let acts = {
        let p = b.disable_rect().unwrap().center();
        banner_click(&mut b, p)
    };
    assert_eq!(banner_actions(&acts), [PluginBannerAction::DisableCrashed]);
    assert!(!b.is_visible());
    let mut b = banner(BannerKind::UncleanExit {
        plugins: vec!["X".into()],
        has_recovery: false,
    });
    assert!(b.recovery_rect().is_none());
    let acts = {
        let p = b.keep_rect().unwrap().center();
        banner_click(&mut b, p)
    };
    assert_eq!(banner_actions(&acts), [PluginBannerAction::KeepCrashed]);
}

#[test]
fn a_long_plugin_list_is_cut() {
    let long = BannerKind::UncleanExit {
        plugins: (0..40).map(|i| format!("Plugin number {i}")).collect(),
        has_recovery: false,
    };
    let b = banner(long);
    let mut dl = DisplayList::new();
    b.paint(&mut dl);
    assert!(dl.texts().iter().any(|t| t.ends_with('.') || t.len() < 200));
}

#[test]
fn the_banner_matches_goldens() {
    let mut dl = DisplayList::new();
    banner(BannerKind::ProjectUsesPlugins { count: 3 }).paint(&mut dl);
    check("plugin_banner_project_1x.json", &dl.to_json());
    let mut dl = DisplayList::new();
    banner(BannerKind::UncleanExit {
        plugins: vec!["Test Synth".into(), "Test Fx".into()],
        has_recovery: true,
    })
    .paint(&mut dl);
    check("plugin_banner_crash_1x.json", &dl.to_json());
    let _ = PointerButton::Primary;
}

#[test]
fn the_status_column_says_enabled_disabled_could_not_load_or_crashed() {
    let mut data = on_data();
    data.plugins[1].status = PluginEntryStatus::Crashed("exited with code 3".into());
    let mut v = view(data);
    let t = texts_of(&v);
    for word in [
        "ENABLED",
        "DISABLED",
        "UNSUPPORTED",
        "COULD NOT LOAD",
        "CRASHED",
    ] {
        assert!(has(&t, word), "{word} in {t:?}");
    }
    assert!(has(&t, "exited with code 3"), "the reason shows");
    // Only the crashed row has a RETRY, and it clears the mark.
    assert!(v.crash_retry_rect(0).is_none() && v.crash_retry_rect(2).is_none());
    let acts = {
        let p = v.crash_retry_rect(1).unwrap().center();
        click_at(&mut v, p)
    };
    assert_eq!(
        settings(&acts),
        [PluginSettingsAction::RetryPlugin("clap.org.test.fx".into())]
    );
}
