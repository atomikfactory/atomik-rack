//! A session killed from outside is recovered by the next one: the operating system ends a
//! child process that holds a recovery folder in the middle of its work (no clean exit, no
//! destructor), and a new session on the same folder offers the last snapshot, opens it with
//! the edits the child made, and sees the dead process's session marker as gone.
//!
//! The child is this very test executable, started again with an environment variable that
//! selects [`child_session`]; the other tests of the file do nothing in it. The child never
//! opens an audio device or a window: it is an `AppCore` over the null backend whose clock
//! the test advances by hand, so the autosave period costs no waiting.
// Justification: the test waits for another process against the wall clock; nothing here is in
// a signal path.
#![allow(clippy::disallowed_methods)]

use atomik_app::AppCore;
use atomik_app::app_core::{PluginConfig, PluginPaths};
use atomik_app::recovery::session_alive;
use atomik_app::session::{LiveSession, NullBackend};
use atomik_app::settings::AudioSettings;
use atomik_model::{Command, DeviceKey};
use atomik_plugin_host::RecoveryDir;
use atomik_ui::plugin_data::PluginBannerAction;
use atomik_ui::{BannerKind, UiAction};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// The environment variable that turns this executable into the child: the folder it works in.
const CHILD_DIR: &str = "ATOMIK_KILL_TEST_DIR";
/// The file the child writes (inside its folder) once its snapshot is on disk.
const READY: &str = "ready";
/// How long the test waits for the child and for the operating system: generous, since the wait
/// ends on the condition.
const BOUND: Duration = Duration::from_secs(60);
/// The longest an abandoned child lives (a test that failed before it killed the child).
const CHILD_LIFE: Duration = Duration::from_secs(120);

/// The edits both sides know: what the child does before it is killed, and what the recovered
/// document must hold.
const GAIN_DB: f32 = -7.5;
const PAN: f32 = 0.25;
const TEMPO: f64 = 133.0;

fn core(recovery: &Path) -> AppCore<LiveSession<NullBackend>> {
    let mut core = AppCore::new(LiveSession::new(
        NullBackend,
        AudioSettings::default(),
        false,
    ));
    core.configure_plugins(PluginConfig {
        paths: PluginPaths {
            recovery: Some(recovery.to_path_buf()),
            ..PluginPaths::default()
        },
        ..PluginConfig::default()
    });
    core
}

fn edits(core: &mut AppCore<LiveSession<NullBackend>>) {
    let key = DeviceKey("tool".to_string());
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.gain-tool".to_string(),
        key: Some(key.clone()),
    })
    .unwrap();
    for (param, value) in [("gain", GAIN_DB), ("pan", PAN)] {
        core.apply_command(Command::SetParam {
            key: key.clone(),
            param: param.to_string(),
            value,
            merge: None,
        })
        .unwrap();
    }
    core.apply_command(Command::SetTempo(TEMPO)).unwrap();
}

/// The child: edits a document, gets its snapshot written by the autosave (the clock is
/// advanced past the period by hand), tells the test, and then waits to be killed.
#[test]
fn child_session() {
    let Some(dir) = std::env::var_os(CHILD_DIR).map(PathBuf::from) else {
        return;
    };
    let mut core = core(&dir.join("recovery"));
    edits(&mut core);
    core.plugin_tick(0.0);
    core.plugin_tick(1_000.0);
    core.wait_recovery_writes();
    std::fs::write(dir.join(READY), b"ready").unwrap();
    // Killed from outside long before this; the limit only keeps a stray child from living on.
    let end = Instant::now() + CHILD_LIFE;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Kills the child when the test ends by a failure before it did.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_child(dir: &Path) -> Guard {
    let exe = std::env::current_exe().unwrap();
    let child = std::process::Command::new(exe)
        .args(["--exact", "child_session", "--test-threads=1"])
        .env(CHILD_DIR, dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the child starts");
    Guard(child)
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + BOUND;
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-kill-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn param(doc: &atomik_model::RackDoc, name: &str) -> Option<f32> {
    doc.resolve(&DeviceKey("tool".to_string()))
        .and_then(|d| d.params.get(name).copied())
}

#[test]
fn a_session_killed_in_the_middle_of_its_work_is_recovered_by_the_next_one() {
    let dir = folder("session");
    let recovery = dir.join("recovery");
    let mut child = start_child(&dir);
    let pid = child.0.id();
    wait_for("the child's snapshot", || dir.join(READY).exists());
    assert!(
        RecoveryDir::new(&recovery).newest().is_some(),
        "the autosave wrote a snapshot"
    );
    assert!(
        session_alive(&recovery, pid),
        "the child holds its session marker while it lives"
    );

    // The operating system ends it: no clean exit, no destructor.
    child.0.kill().expect("the child is killed");
    child.0.wait().expect("the child is reaped");
    wait_for("the marker of the dead child to be free", || {
        !session_alive(&recovery, pid)
    });

    // The next session on the same folder: the dead marker is recognised, and the offer is
    // there.
    let mut next = core(&recovery);
    let banner = next.take_plugin_banner_change();
    assert!(
        matches!(
            banner,
            Some(Some(BannerKind::UncleanExit { ref plugins, has_recovery: true })) if plugins.is_empty()
        ),
        "{banner:?}"
    );
    // Recovering opens the edited document as an unsaved project.
    next.handle_action(UiAction::PluginBanner(PluginBannerAction::OpenRecovery));
    next.wait_background();
    let doc = next.doc();
    assert!(doc.resolve(&DeviceKey("tool".to_string())).is_some());
    assert_eq!(param(doc, "gain"), Some(GAIN_DB));
    assert_eq!(param(doc, "pan"), Some(PAN));
    assert!((doc.tempo_bpm - TEMPO).abs() < 1e-9, "{}", doc.tempo_bpm);
    assert!(
        next.is_dirty() && next.file().is_none(),
        "not the user's file"
    );
    // The same document as the edits give in this process.
    let mut reference = core(&folder("reference").join("recovery"));
    edits(&mut reference);
    assert_eq!(doc.devices, reference.doc().devices);
    assert_eq!(doc.tempo_bpm, reference.doc().tempo_bpm);
    next.shutdown_plugins();

    // The dead marker was taken away: a clean quit of the new session leaves the folder quiet
    // for the one after it.
    let mut third = core(&recovery);
    assert_eq!(third.take_plugin_banner_change(), None);
    third.shutdown_plugins();
    let _ = std::fs::remove_dir_all(&dir);
}
