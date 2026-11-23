//! A VST3 plugin in its own process that writes 64 output parameters in every block, like a
//! metering plugin: the app is told of the ones the layout shows and of nothing else, far less
//! often than once per block, and a gesture made in the plugin's window while it floods still
//! arrives whole (a lost begin or end would drop every edit the user makes there).

// Justification: the test waits for the child's notifications against the wall clock.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{ManagerEvent, ParamPhase};
use atomik_test_plugin_core::{GUI_EDIT_ON_RESTORE_ENV, OUT_PARAMS_ENV};
use sandbox_support::{
    Format, Plug, Sandbox, config, effect, gain_state_with_latency, request_with_state,
};
use std::ffi::OsString;
use std::time::{Duration, Instant};

/// How long the test waits: it ends on the events, so this only bounds a failure.
const PATIENCE: Duration = Duration::from_secs(60);

/// The id the plugin's editor edits while the flood runs: outside the ids the flood writes.
const EDITED: u32 = 900;

#[test]
fn a_plugin_process_that_floods_output_params_reports_the_layout_only_and_keeps_its_gestures() {
    let format = Format::Vst3;
    let mut cfg = config();
    for (key, value) in [
        (OUT_PARAMS_ENV, "64:1".to_string()),
        (GUI_EDIT_ON_RESTORE_ENV, format!("{EDITED}:0.25")),
    ] {
        cfg.command
            .envs
            .push((OsString::from(key), OsString::from(value)));
    }
    let mut sb = Sandbox::real_with(format, cfg);
    let handle = sb
        .manager
        .instantiate(&request_with_state(
            &format.plugin(Plug::Gain),
            RunMode::Realtime,
            gain_state_with_latency(format, 0),
        ))
        .expect("the plugin starts in a child");
    let id = handle.id;
    let shown: Vec<u32> = handle
        .layout
        .params
        .iter()
        .map(|p| p.id)
        .filter(|p| (1..65).contains(p))
        .collect();
    assert!(!shown.is_empty() && shown.len() < 64, "{shown:?}");
    let mut h = effect(handle, 0.25);

    let limit = Instant::now() + PATIENCE;
    let mut batches = 0usize;
    let mut gesture: Vec<ParamPhase> = Vec::new();
    let mut changes: Vec<u32> = Vec::new();
    loop {
        h.process();
        batches += 1;
        for e in sb.manager.poll() {
            if let ManagerEvent::ParamEdit {
                instance,
                param,
                phase,
                ..
            } = e
                && instance == id
            {
                if param == EDITED {
                    gesture.push(phase);
                } else if phase == ParamPhase::Change {
                    changes.push(param);
                }
            }
        }
        let all_shown_seen = shown.iter().all(|p| changes.contains(p));
        if batches >= 600 && all_shown_seen && gesture.len() >= 3 {
            break;
        }
        assert!(
            Instant::now() < limit,
            "{batches} batches: shown ids seen {all_shown_seen}, gesture {gesture:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        changes.iter().all(|p| shown.contains(p)),
        "an id the layout does not show was reported: {changes:?}"
    );
    // At most one report per parameter every ten blocks (the rule is one every fifteen), where
    // the plugin wrote new values in every one of them.
    assert!(
        changes.len() <= (batches / 10 + 2) * shown.len(),
        "{} reports in {batches} blocks for {} params",
        changes.len(),
        shown.len()
    );
    assert_eq!(gesture.len() % 3, 0, "{gesture:?}");
    for whole in gesture.chunks(3) {
        assert_eq!(
            whole,
            [ParamPhase::Begin, ParamPhase::Change, ParamPhase::End],
            "{gesture:?}"
        );
    }
    drop(h);
    sb.manager.poll();
    sb.finish();
}
