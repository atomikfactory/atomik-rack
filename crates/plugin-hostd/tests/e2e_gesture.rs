//! An edit made in a sandboxed plugin's own editor reaches the app as a whole gesture: begin,
//! change and end, in that order, for each format. The app records only the changes inside a
//! gesture, so a lost begin or end would drop every edit the user makes in the plugin's window.

// Justification: the test waits for the child's notifications against the wall clock.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{ManagerEvent, ParamPhase};
use atomik_test_plugin_core::GUI_EDIT_ON_RESTORE_ENV;
use sandbox_support::{
    FORMATS, Format, Plug, Sandbox, config_with_env, effect, gain_state_with_latency,
    request_with_state,
};
use std::time::{Duration, Instant};

/// How long the test waits for the gesture: it ends on the event, so this only bounds a failure.
const PATIENCE: Duration = Duration::from_secs(30);

/// The gain param of the test effect.
const GAIN: u32 = 1;

fn gesture_reaches_the_parent(format: Format) {
    let value = f64::from(format.gain_value(-6.0));
    let mut sb = Sandbox::real_with(
        format,
        config_with_env(GUI_EDIT_ON_RESTORE_ENV, &format!("{GAIN}:{value}")),
    );
    // Loading a state makes the test plugin report an edit of its gain from its editor.
    let handle = sb
        .manager
        .instantiate(&request_with_state(
            &format.plugin(Plug::Gain),
            RunMode::Realtime,
            gain_state_with_latency(format, 0),
        ))
        .expect("the plugin starts in a child");
    assert!(handle.restore_error.is_none());
    let id = handle.id;
    // A CLAP plugin reports the edit from its next `process`.
    let mut h = effect(handle, 0.25);
    let limit = Instant::now() + PATIENCE;
    let mut seen: Vec<(ParamPhase, f64)> = Vec::new();
    while !seen.iter().any(|(p, _)| *p == ParamPhase::End) {
        assert!(
            Instant::now() < limit,
            "{}: no whole gesture within {PATIENCE:?}: {seen:?}",
            format.name()
        );
        h.process();
        for e in sb.manager.poll() {
            if let ManagerEvent::ParamEdit {
                instance,
                param,
                value,
                phase,
            } = e
                && instance == id
                && param == GAIN
            {
                seen.push((phase, value));
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // The host may load the state more than once (each load is one whole gesture).
    assert_eq!(seen.len() % 3, 0, "{}: {seen:?}", format.name());
    for gesture in seen.chunks(3) {
        let phases: Vec<ParamPhase> = gesture.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            phases,
            [ParamPhase::Begin, ParamPhase::Change, ParamPhase::End],
            "{}: {seen:?}",
            format.name()
        );
        assert!(
            (gesture[1].1 - value).abs() < 1e-9,
            "{}: the change carries the edited value: {seen:?}",
            format.name()
        );
    }
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_gesture_in_a_sandboxed_plugins_editor_reaches_the_app_whole() {
    for format in FORMATS {
        gesture_reaches_the_parent(format);
    }
}
