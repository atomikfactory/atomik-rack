//! A CLAP library of four classes for the scanner's robustness tests (`atomik-test-clap-mix`).
//! Every class is a pass-through effect that scans cleanly unless an environment variable of
//! the scanning process says otherwise:
//!
//! * `atomik.test.mix.a` and `atomik.test.mix.b` always scan cleanly.
//! * `atomik.test.mix.helper` starts a helper process when it is created, if
//!   `ATOMIK_TEST_MIX_HELPER_PID_FILE` names a file: the helper is `ping` (Windows) or `sleep`
//!   (elsewhere), runs for five minutes, shares the creator's output pipes, and its process id is
//!   written to that file. A scan that leaves nothing behind has ended it again.
//! * `atomik.test.mix.stuck` never returns from `create` when `ATOMIK_TEST_MIX_STUCK` is set and
//!   aborts the process there when `ATOMIK_TEST_MIX_CRASH` is set.
//! * `ATOMIK_TEST_MIX_SLOW_MS` makes every class take that many milliseconds to create (a slow
//!   but healthy file).

use atomik_test_clap_shell::{
    Info, LibraryDef, Passthrough, PluginDef, clap_library, sys::plugin_features as feat,
};

static INFO_A: Info = Info {
    id: c"atomik.test.mix.a",
    name: c"Atomik Test Mix A",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static INFO_HELPER: Info = Info {
    id: c"atomik.test.mix.helper",
    name: c"Atomik Test Mix Helper",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static INFO_STUCK: Info = Info {
    id: c"atomik.test.mix.stuck",
    name: c"Atomik Test Mix Stuck",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static INFO_B: Info = Info {
    id: c"atomik.test.mix.b",
    name: c"Atomik Test Mix B",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

/// Waits as long as `ATOMIK_TEST_MIX_SLOW_MS` says (a slow but healthy plugin).
fn pause() {
    let ms = std::env::var("ATOMIK_TEST_MIX_SLOW_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(ms) = ms {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Starts the helper process and records its process id, when the environment asks for it.
fn start_helper() {
    let Some(pid_file) = std::env::var_os("ATOMIK_TEST_MIX_HELPER_PID_FILE") else {
        return;
    };
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "300", "127.0.0.1"]);
        c
    } else {
        let mut c = std::process::Command::new("sleep");
        c.arg("300");
        c
    };
    // The helper keeps this process's output pipes open, as a plugin's helper typically does.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    if let Ok(child) = cmd.spawn() {
        let _ = std::fs::write(pid_file, child.id().to_string());
        // The child handle is dropped without waiting: the helper runs on by itself.
    }
}

fn stuck_or_crash() {
    pause();
    if std::env::var_os("ATOMIK_TEST_MIX_CRASH").is_some() {
        // An abort ends the process abnormally on every system (a signal on Unix, a non-zero
        // status on Windows), which is what this test plugin exists to do; it needs no `unsafe`.
        std::process::abort();
    }
    if std::env::var_os("ATOMIK_TEST_MIX_STUCK").is_some() {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

static PLUGINS: [PluginDef; 4] = [
    PluginDef {
        info: &INFO_A,
        create: || {
            pause();
            Box::new(Passthrough)
        },
    },
    PluginDef {
        info: &INFO_HELPER,
        create: || {
            pause();
            start_helper();
            Box::new(Passthrough)
        },
    },
    PluginDef {
        info: &INFO_STUCK,
        create: || {
            stuck_or_crash();
            Box::new(Passthrough)
        },
    },
    PluginDef {
        info: &INFO_B,
        create: || {
            pause();
            Box::new(Passthrough)
        },
    },
];

clap_library!(LibraryDef::new(&PLUGINS, || true));
