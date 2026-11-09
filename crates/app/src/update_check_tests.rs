use super::*;
use atomik_update::{DAY_SECS, FakeHttp};
use std::time::{Duration, Instant};

const LIST: &str = r#"[
    {"tag_name":"v0.6.0-rc.1","prerelease":true,"draft":false},
    {"tag_name":"v0.5.0","prerelease":false,"draft":false},
    {"tag_name":"v0.4.0","prerelease":false,"draft":false}
]"#;

/// A temporary folder that removes itself.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-app-update-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join(atomik_update::FILE)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn updater_with(http: &Arc<FakeHttp>, running: &str, path: Option<PathBuf>) -> Updater {
    let mut updater = Updater::new(path);
    updater.set_http(Arc::clone(http) as Arc<dyn HttpGet>);
    updater.pretend(running);
    updater
}

/// Waits for the worker's answer (it is instant with a fake; the limit only ends a hung test).
fn finished(updater: &mut Updater) -> Finished {
    let start = Instant::now();
    loop {
        if let Some(done) = updater.poll() {
            return done;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the check never finished"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn tag(done: &Finished) -> Option<String> {
    done.result
        .as_ref()
        .expect("an answer")
        .as_ref()
        .map(|r| r.tag.clone())
}

#[test]
fn nothing_is_asked_before_the_answer_is_yes() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    for consent in [Consent::Unasked, Consent::No] {
        assert!(!updater.start_automatic(consent, 1_000_000));
        assert!(!updater.busy());
    }
    assert_eq!(http.count(), 0);
    assert!(updater.poll().is_none());
}

#[test]
fn a_yes_asks_once_and_finds_the_newer_release() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    assert!(updater.start_automatic(Consent::Yes, 1_000_000));
    assert!(updater.busy());
    let done = finished(&mut updater);
    assert!(!done.manual);
    assert_eq!(tag(&done).as_deref(), Some("v0.5.0"));
    assert!(!updater.busy());
    assert_eq!(http.count(), 1);
}

#[test]
fn the_request_is_the_generic_one_and_nothing_else() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    updater.start_automatic(Consent::Yes, 1);
    finished(&mut updater);
    let seen = http.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://api.github.com/repos/atomikfactory/atomik-rack/releases"
    );
    assert_eq!(seen[0].user_agent, "atomik-rack/0.4.0");
}

#[test]
fn the_daily_check_waits_a_day() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    let t = 5_000_000;
    assert!(updater.start_automatic(Consent::Yes, t));
    finished(&mut updater);
    assert!(!updater.start_automatic(Consent::Yes, t + 3600));
    assert!(!updater.start_automatic(Consent::Yes, t + DAY_SECS - 1));
    assert_eq!(http.count(), 1);
    assert!(updater.start_automatic(Consent::Yes, t + DAY_SECS));
    finished(&mut updater);
    assert_eq!(http.count(), 2);
}

#[test]
fn a_failed_request_counts_for_the_day_and_is_silent() {
    let http = Arc::new(FakeHttp::failing());
    let mut updater = updater_with(&http, "0.4.0", None);
    assert!(updater.start_automatic(Consent::Yes, 100));
    let done = finished(&mut updater);
    assert!(done.result.is_err());
    assert!(!updater.start_automatic(Consent::Yes, 200));
    assert_eq!(http.count(), 1);
}

#[test]
fn the_day_survives_a_restart() {
    let dir = Dir::new("restart");
    let http = Arc::new(FakeHttp::ok(LIST));
    {
        let mut first = updater_with(&http, "0.4.0", Some(dir.file()));
        assert!(first.start_automatic(Consent::Yes, 9_000_000));
        finished(&mut first);
    }
    let mut second = updater_with(&http, "0.4.0", Some(dir.file()));
    assert_eq!(second.state().last_check, Some(9_000_000));
    assert!(!second.start_automatic(Consent::Yes, 9_000_000 + 60));
    assert!(second.start_automatic(Consent::Yes, 9_000_000 + DAY_SECS));
    finished(&mut second);
    assert_eq!(http.count(), 2);
}

#[test]
fn a_check_by_hand_does_not_wait_and_does_not_run_twice_at_once() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    assert!(updater.start_automatic(Consent::Yes, 7));
    finished(&mut updater);
    assert!(updater.start_manual(8));
    assert!(!updater.start_manual(9), "one request at a time");
    let done = finished(&mut updater);
    assert!(done.manual);
    assert_eq!(http.count(), 2);
}

#[test]
fn the_answers_of_the_fake_server() {
    // (running build, body or status, what the check says)
    let cases: [(&str, u16, &str, Option<&str>); 6] = [
        ("0.4.0", 200, LIST, Some("v0.5.0")),
        ("0.5.0", 200, LIST, None),
        ("1.0.0", 200, LIST, None),
        ("0.4.0-rc.1", 200, LIST, Some("v0.6.0-rc.1")),
        ("0.6.0-rc.1", 200, LIST, None),
        ("0.5.0", 200, "[]", None),
    ];
    for (running, status, body, expected) in cases {
        let http = Arc::new(FakeHttp::status(status, body));
        let mut updater = updater_with(&http, running, None);
        updater.start_manual(1);
        let done = finished(&mut updater);
        assert_eq!(tag(&done).as_deref(), expected, "running {running}");
    }
    for (status, body) in [
        (200, "not json"),
        (200, "{}"),
        (403, LIST),
        (500, ""),
        (404, "[]"),
    ] {
        let http = Arc::new(FakeHttp::status(status, body));
        let mut updater = updater_with(&http, "0.4.0", None);
        updater.start_manual(1);
        assert!(finished(&mut updater).result.is_err(), "{status} {body:?}");
    }
    let http = Arc::new(FakeHttp::failing());
    let mut updater = updater_with(&http, "0.4.0", None);
    updater.start_manual(1);
    assert!(finished(&mut updater).result.is_err());
}

#[test]
fn a_skipped_version_stays_quiet_until_a_later_one_or_a_check_by_hand() {
    let dir = Dir::new("skip");
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", Some(dir.file()));
    updater.start_manual(1);
    let release = finished(&mut updater).result.unwrap().unwrap();
    assert!(updater.offer(&release, false));
    updater.skip(&release);
    assert!(!updater.offer(&release, false));
    assert!(updater.offer(&release, true), "by hand it shows anyway");

    // The skip is in the file: a new run of the program knows it.
    let mut again = updater_with(&http, "0.4.0", Some(dir.file()));
    assert!(!again.offer(&release, false));

    // A later version is a new question.
    http.set_answer(Ok(atomik_update::Response {
        status: 200,
        body: br#"[{"tag_name":"v0.5.1"}]"#.to_vec(),
    }));
    again.start_manual(2);
    let later = finished(&mut again).result.unwrap().unwrap();
    assert_eq!(later.tag, "v0.5.1");
    assert!(again.offer(&later, false));
}

#[test]
fn two_windows_do_not_undo_each_others_facts() {
    let dir = Dir::new("windows");
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut a = updater_with(&http, "0.4.0", Some(dir.file()));
    let mut b = updater_with(&http, "0.4.0", Some(dir.file()));
    a.start_manual(100);
    let release = finished(&mut a).result.unwrap().unwrap();
    a.skip(&release);
    // B checks later: it must keep A's skip and A's time of the last request.
    b.start_manual(200);
    finished(&mut b);
    let kept = UpdateState::load(&dir.file());
    assert_eq!(kept.last_check, Some(200));
    assert!(kept.skips(&release.version));
    assert!(!b.offer(&release, false));
}

#[test]
fn an_updater_that_was_never_given_a_client_cannot_reach_the_network() {
    let mut updater = Updater::new(None);
    assert!(updater.start_manual(1));
    let done = finished(&mut updater);
    assert_eq!(
        done.result.err(),
        Some(CheckError::Http(HttpError::Offline))
    );
}

#[test]
fn the_running_version_is_this_build() {
    let updater = Updater::new(None);
    assert_eq!(updater.running().to_string(), RUNNING);
    assert!(Version::parse(RUNNING).is_ok());
}

#[test]
fn nothing_is_written_without_a_file() {
    let http = Arc::new(FakeHttp::ok(LIST));
    let mut updater = updater_with(&http, "0.4.0", None);
    updater.start_automatic(Consent::Yes, 55);
    finished(&mut updater);
    // Without a file the facts live in memory only.
    assert_eq!(updater.state().last_check, Some(55));
}
