//! The update check in the shell: the question of the first start, the notice of a newer
//! version and its buttons, Help > Check for Updates, and that nothing is asked before a Yes.
//! Every request goes to a fake client: no test reaches the network.

use super::*;
use crate::keymap_store::{WORKFLOW_FILE, load_workflow};
use crate::session::NullBackend;
use crate::update_notice::{Choice, Notice};
use atomik_ui::{DisplayList, HELP_DELAY, Modifiers, PointerButton};
use atomik_update::{Consent, DAY_SECS, FakeHttp, HttpGet, UpdateState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const LIST: &str = r#"[
    {"tag_name":"v0.6.0","prerelease":false,"draft":false},
    {"tag_name":"v0.5.0","prerelease":false,"draft":false}
]"#;

/// A config folder that removes itself.
struct Config(PathBuf);

impl Config {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-shell-update-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn workflow(&self) -> crate::keymap_store::WorkflowPrefs {
        load_workflow(&self.0.join(WORKFLOW_FILE)).value
    }

    fn state(&self) -> UpdateState {
        UpdateState::load(&self.0.join(atomik_update::FILE))
    }

    /// A shell on this folder, running as `version`, asking `http`.
    fn shell(&self, http: &Arc<FakeHttp>, version: &str) -> Shell<NullBackend> {
        let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
        let mut s = Shell::new(
            AppCore::new(session),
            ShellOptions {
                os: Some(atomik_ui::keymap::Os::Windows),
                profile: None,
                in_window_menu: true,
                settings_path: Some(self.0.join("settings.json")),
                ui_state_path: None,
                library_path: None,
                library_root: Some(self.0.join("Library")),
            },
        );
        s.handle(&UiEvent::ScaleChanged(1.0));
        s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
        s.set_update_http(Arc::clone(http) as Arc<dyn HttpGet>);
        s.pretend_version(version);
        s
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs frames until the request in flight (if any) is answered (the limit only ends a hung
/// test: with a fake the answer is there at once).
fn settle(s: &mut Shell<NullBackend>) {
    for _ in 0..10_000 {
        s.frame();
        if !s.updater.busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("no answer");
}

fn click(s: &mut Shell<NullBackend>, choice: Choice) -> Vec<ShellRequest> {
    let size = s.size();
    let at = s
        .update_notice()
        .expect("a notice")
        .buttons(size)
        .into_iter()
        .find(|(_, c)| *c == choice)
        .map(|(r, _)| r.center())
        .expect("that button");
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

fn is_ask(s: &Shell<NullBackend>) -> bool {
    matches!(
        s.update_notice().map(UpdateNotice::notice),
        Some(Notice::Ask { .. })
    )
}

fn is_news(s: &Shell<NullBackend>, version: &str) -> bool {
    matches!(
        s.update_notice().map(UpdateNotice::notice),
        Some(Notice::News { version: v, .. }) if v == version
    )
}

#[test]
fn a_shell_nobody_started_by_hand_asks_nothing_and_shows_nothing() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    for _ in 0..5 {
        s.frame();
    }
    assert!(s.update_notice().is_none());
    assert_eq!(http.count(), 0);
}

#[test]
fn a_shell_without_a_client_cannot_reach_the_network() {
    let cfg = Config::new();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(cfg.0.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.workflow.update_check = Some(true);
    s.start_update_check();
    settle(&mut s);
    assert!(
        s.update_notice().is_none(),
        "the offline client finds nothing"
    );
}

#[test]
fn the_first_start_asks_the_question_and_sends_nothing() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    assert_eq!(s.update_consent(), Consent::Unasked);
    s.start_update_check();
    assert!(is_ask(&s));
    for _ in 0..5 {
        s.frame();
    }
    assert_eq!(http.count(), 0, "no request before an answer");
    assert_eq!(cfg.workflow().update_check, None, "nothing is saved either");
}

#[test]
fn yes_is_saved_and_asks_once() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    click(&mut s, Choice::Yes);
    assert!(s.update_notice().is_none());
    assert_eq!(s.update_consent(), Consent::Yes);
    assert_eq!(cfg.workflow().update_check, Some(true));
    settle(&mut s);
    assert_eq!(http.count(), 1);
    assert!(is_news(&s, "0.6.0"));
    assert!(cfg.state().last_check.is_some(), "the day is counted");
}

#[test]
fn no_is_saved_and_nothing_is_ever_asked_by_itself() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    click(&mut s, Choice::No);
    assert!(s.update_notice().is_none());
    assert_eq!(cfg.workflow().update_check, Some(false));
    for _ in 0..3 {
        s.start_update_check();
        settle(&mut s);
    }
    assert!(s.update_notice().is_none());
    assert_eq!(http.count(), 0);
    // The answer survives a restart: no question again.
    let mut again = cfg.shell(&http, "0.5.0");
    again.start_update_check();
    assert!(again.update_notice().is_none());
    assert_eq!(http.count(), 0);
}

#[test]
fn a_yes_checks_at_each_start_but_only_once_a_day() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.6.0");
    s.start_update_check();
    settle(&mut s);
    assert_eq!(http.count(), 1);
    assert!(s.update_notice().is_none(), "this build is the newest");
    s.start_update_check();
    settle(&mut s);
    let mut restarted = cfg.shell(&http, "0.6.0");
    restarted.start_update_check();
    settle(&mut restarted);
    assert_eq!(http.count(), 1, "the same day: no second request");
    // A day later the state file says it is due again.
    let mut state = cfg.state();
    state.last_check = Some(state.last_check.unwrap() - DAY_SECS);
    state.save(&cfg.0.join(atomik_update::FILE)).unwrap();
    let mut next_day = cfg.shell(&http, "0.6.0");
    next_day.start_update_check();
    settle(&mut next_day);
    assert_eq!(http.count(), 2);
}

#[test]
fn the_daily_check_says_nothing_when_nothing_is_newer_or_the_request_fails() {
    for http in [
        Arc::new(FakeHttp::ok(LIST)),
        Arc::new(FakeHttp::ok("garbage")),
        Arc::new(FakeHttp::status(503, "")),
        Arc::new(FakeHttp::failing()),
    ] {
        let cfg = Config::new();
        std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
        let mut s = cfg.shell(&http, "0.6.0");
        let before = s.core().last_rejection().map(str::to_string);
        s.start_update_check();
        settle(&mut s);
        assert_eq!(http.count(), 1);
        assert!(s.update_notice().is_none());
        assert_eq!(
            s.core().last_rejection().map(str::to_string),
            before,
            "no message either"
        );
    }
}

#[test]
fn whats_new_opens_the_release_page_and_closes_the_notice() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    settle(&mut s);
    assert!(is_news(&s, "0.6.0"));
    let requests = click(&mut s, Choice::WhatsNew);
    assert_eq!(
        requests,
        [ShellRequest::OpenRelease(
            "https://github.com/atomikfactory/atomik-rack/releases/tag/v0.6.0".to_string()
        )]
    );
    assert!(s.update_notice().is_none());
    assert_eq!(cfg.state().skipped, None);
}

#[test]
fn skip_this_version_keeps_the_daily_check_quiet_about_it() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    settle(&mut s);
    click(&mut s, Choice::Skip);
    assert!(s.update_notice().is_none());
    assert_eq!(cfg.state().skipped.as_deref(), Some("0.6.0"));
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("0.6.0"))
    );

    // The next day the daily check finds the same version and says nothing.
    let mut state = cfg.state();
    state.last_check = Some(state.last_check.unwrap() - DAY_SECS);
    state.save(&cfg.0.join(atomik_update::FILE)).unwrap();
    let mut next = cfg.shell(&http, "0.5.0");
    next.start_update_check();
    settle(&mut next);
    assert_eq!(http.count(), 2);
    assert!(next.update_notice().is_none());

    // Help > Check for Updates shows it anyway.
    next.execute(AppCommand::CheckForUpdates);
    settle(&mut next);
    assert!(is_news(&next, "0.6.0"));

    // A later version is not skipped.
    http.set_answer(Ok(atomik_update::Response {
        status: 200,
        body: br#"[{"tag_name":"v0.6.1"}]"#.to_vec(),
    }));
    let mut state = cfg.state();
    state.last_check = Some(state.last_check.unwrap() - DAY_SECS);
    state.save(&cfg.0.join(atomik_update::FILE)).unwrap();
    let mut later = cfg.shell(&http, "0.5.0");
    later.start_update_check();
    settle(&mut later);
    assert!(is_news(&later, "0.6.1"));
}

#[test]
fn later_closes_the_notice_and_keeps_nothing() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    settle(&mut s);
    assert!(click(&mut s, Choice::Later).is_empty());
    assert!(s.update_notice().is_none());
    assert_eq!(cfg.state().skipped, None);
    assert_eq!(cfg.workflow().update_check, Some(true));
}

#[test]
fn check_for_updates_asks_the_question_first_while_it_is_open() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.execute(AppCommand::CheckForUpdates);
    settle(&mut s);
    assert!(is_ask(&s));
    assert_eq!(http.count(), 0, "the command did not go to the network");
    click(&mut s, Choice::Yes);
    settle(&mut s);
    assert_eq!(
        http.count(),
        1,
        "the yes makes the request the command wanted"
    );
    assert!(is_news(&s, "0.6.0"));
    assert_eq!(s.update_consent(), Consent::Yes);
}

#[test]
fn a_no_to_the_question_of_the_command_makes_no_request() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.execute(AppCommand::CheckForUpdates);
    click(&mut s, Choice::No);
    settle(&mut s);
    assert_eq!(http.count(), 0);
    assert_eq!(s.update_consent(), Consent::No);
}

#[test]
fn check_for_updates_after_an_answer_checks_now_and_says_the_result() {
    for (answer, body, running, expected) in [
        (true, LIST, "0.6.0", "is the newest version"),
        (false, LIST, "0.7.0", "is the newest version"),
        (true, "garbage", "0.5.0", "Could not check"),
    ] {
        let cfg = Config::new();
        let http = Arc::new(FakeHttp::ok(body));
        std::fs::write(
            cfg.0.join(WORKFLOW_FILE),
            format!("{{\"update_check\": {answer}}}"),
        )
        .unwrap();
        let mut s = cfg.shell(&http, running);
        s.execute(AppCommand::CheckForUpdates);
        assert_eq!(
            s.core().last_rejection(),
            Some("Checking for new versions...")
        );
        settle(&mut s);
        assert_eq!(http.count(), 1);
        assert!(
            s.core()
                .last_rejection()
                .is_some_and(|m| m.contains(expected)),
            "{:?}",
            s.core().last_rejection()
        );
        assert!(s.update_notice().is_none());
    }
}

#[test]
fn check_for_updates_does_not_wait_for_the_day() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    settle(&mut s);
    click(&mut s, Choice::Later);
    s.execute(AppCommand::CheckForUpdates);
    settle(&mut s);
    assert_eq!(http.count(), 2);
    assert!(is_news(&s, "0.6.0"));
}

#[test]
fn a_second_command_while_the_first_request_runs_is_refused() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.execute(AppCommand::CheckForUpdates);
    if s.updater.busy() {
        s.execute(AppCommand::CheckForUpdates);
        assert_eq!(
            s.core().last_rejection(),
            Some("Already checking for new versions")
        );
    }
    settle(&mut s);
    assert!(http.count() <= 2);
}

#[test]
fn the_request_carries_the_generic_user_agent_and_nothing_else() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    std::fs::write(cfg.0.join(WORKFLOW_FILE), "{\"update_check\": true}").unwrap();
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    settle(&mut s);
    let seen = http.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://api.github.com/repos/atomikfactory/atomik-rack/releases"
    );
    assert_eq!(seen[0].user_agent, "atomik-rack/0.5.0");
}

#[test]
fn the_question_closes_when_the_answer_is_given_in_settings() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    assert!(is_ask(&s));
    s.workflow.update_check = Some(false);
    s.frame();
    assert!(s.update_notice().is_none());
    assert_eq!(http.count(), 0);
}

#[test]
fn the_notice_is_out_of_the_way_of_a_modal_dialog_and_shows_over_the_start_screen() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    assert!(s.update_notice_reachable());
    s.execute(AppCommand::AudioSettings);
    assert!(!s.update_notice_reachable(), "the dialog is on top");
    // Pressing where the card would be reaches the dialog, not the notice.
    let size = s.size();
    let at = s.update_notice().unwrap().rect(size).center();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert!(is_ask(&s));
}

#[test]
fn a_press_on_the_card_is_not_passed_on_and_a_press_elsewhere_is() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    let size = s.size();
    let card = s.update_notice().unwrap().rect(size);
    let press = |pos| UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let on_card = Point::new(card.x0 + 3.0, card.y0 + 3.0);
    assert!(
        s.update_notice_event(WindowKind::Main, &press(on_card))
            .is_some()
    );
    assert!(is_ask(&s), "a click beside the buttons only keeps the card");
    let elsewhere = Point::new(card.x1 + 200.0, card.y0 - 200.0);
    assert!(
        s.update_notice_event(WindowKind::Main, &press(elsewhere))
            .is_none()
    );
    assert!(
        s.update_notice_event(WindowKind::Rack, &press(on_card))
            .is_none(),
        "only the main window has the card"
    );
}

#[test]
fn the_notice_has_tooltips_on_its_buttons() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_update_check();
    let size = s.size();
    let yes = s
        .update_notice()
        .unwrap()
        .buttons(size)
        .into_iter()
        .find(|(_, c)| *c == Choice::Yes)
        .unwrap()
        .0;
    s.handle(&UiEvent::PointerMove {
        pos: yes.center(),
        modifiers: Modifiers::NONE,
    });
    s.frame_at(1.0);
    s.frame_at(1.0 + HELP_DELAY + 0.05);
    let tip = s.tooltip().expect("a tooltip after the delay");
    assert!(tip.text.contains("once a day"), "{}", tip.text);
    assert!(tip.rect.contains(yes.center()));
}

#[test]
fn the_help_menu_has_the_command() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let s = cfg.shell(&http, "0.5.0");
    let menus = s.menu_model();
    let help = menus.iter().find(|m| m.title == "Help").expect("Help menu");
    let labels: Vec<String> = help
        .entries
        .iter()
        .filter_map(|e| match e {
            crate::commands::MenuEntry::Item(i) => Some(i.label.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        labels.contains(&"Check for Updates".to_string()),
        "{labels:?}"
    );
}

#[test]
fn the_command_is_reachable_by_its_registry_id() {
    assert_eq!(
        AppCommand::CheckForUpdates.command_id(),
        Some(("help.check_for_updates", None))
    );
    assert_eq!(
        AppCommand::from_command("help.check_for_updates", None),
        Some(AppCommand::CheckForUpdates)
    );
}

#[test]
fn the_notice_is_painted_with_its_words_and_buttons() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(!list.texts().contains(&"Check for new versions once a day?"));
    s.start_update_check();
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let texts = list.texts();
    for t in ["Check for new versions once a day?", "Yes", "No"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    click(&mut s, Choice::Yes);
    settle(&mut s);
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let texts = list.texts();
    for t in [
        "Atomik Rack 0.6.0 is available",
        "What's new",
        "Skip this version",
        "Later",
    ] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
}

#[test]
fn the_question_shows_on_the_start_screen_and_leaves_it_open() {
    let cfg = Config::new();
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut s = cfg.shell(&http, "0.5.0");
    s.start_without_project();
    assert!(s.start_screen_open());
    s.start_update_check();
    assert!(is_ask(&s));
    assert!(s.update_notice_reachable());
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(list.texts().contains(&"Check for new versions once a day?"));
    click(&mut s, Choice::No);
    assert!(
        s.start_screen_open(),
        "answering does not close the start screen"
    );
    assert_eq!(s.update_consent(), Consent::No);
    assert_eq!(http.count(), 0);
}
