//! The parent and the child together: a plugin's request for a restart is acted on only when it
//! changes something, a burst of requests is cut off, and renamed params reach the app without a
//! restart. The decision is made where the plugin lives, so the sandbox and a plugin in this
//! process agree (their in-process twin is `atomik-plugin-vst3/tests/lifecycle.rs`).

// Justification: the waits for what the child does by itself are bounded by wall time.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::{PluginRef, RunMode};
use atomik_plugin_host::{
    DeviceState, InstanceId, LogLevel, ManagerEvent, PluginHandle, RESTART_BURST,
};
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    FORMATS, Format, METER_STATE, SETTLE, Sandbox, config, effect, hex, meters, process_settled,
    request,
};
use std::time::{Duration, Instant};

/// How long a test waits for something the child does by itself.
const WAIT: Duration = Duration::from_secs(60);

/// The three plugins of the variants libraries that ask for restarts.
#[derive(Clone, Copy)]
enum Kind {
    Chatty,
    Storm,
    Retitle,
}

fn variant(format: Format, kind: Kind) -> PluginRef {
    let id = match (format, kind) {
        (Format::Clap, Kind::Chatty) => "atomik.test.chatty-activation".to_string(),
        (Format::Clap, Kind::Storm) => "atomik.test.restart-storm".to_string(),
        (Format::Clap, Kind::Retitle) => "atomik.test.retitle".to_string(),
        (Format::Vst3, k) => {
            let n = match k {
                Kind::Chatty => 14,
                Kind::Storm => 15,
                Kind::Retitle => 16,
            };
            hex([0xA70F_1C02, 0x4A5B_4C01, 0x8D2E_1F00, n])
        }
    };
    PluginRef {
        format: format.plugin_format(),
        id,
    }
}

/// A sandbox over the variants library of `format` and the plugin of `kind` in it, running.
fn start(format: Format, kind: Kind) -> (Sandbox, DeviceHarness, InstanceId) {
    let file = format.install(&format!("atomik-test-{}-variants", format.name()));
    let plugin = variant(format, kind);
    let mut sb = Sandbox::new(
        format.plugin_format(),
        config(),
        &[(file, vec![plugin.clone()])],
    );
    let handle: PluginHandle = sb
        .manager
        .instantiate(&request(&plugin, RunMode::Realtime))
        .expect("the plugin starts in a child");
    let id = handle.id;
    let mut h = effect(handle, 0.25);
    process_settled(&mut h, 5, SETTLE);
    (sb, h, id)
}

fn restarts(events: &[ManagerEvent], id: InstanceId) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ManagerEvent::Restarted(i) if *i == id))
        .count()
}

/// Drives the device and the manager until `done` is true of the events so far (or `WAIT` is up).
fn drive_until(
    sb: &mut Sandbox,
    h: &mut DeviceHarness,
    events: &mut Vec<ManagerEvent>,
    done: impl Fn(&[ManagerEvent]) -> bool,
) -> bool {
    let limit = Instant::now() + WAIT;
    while Instant::now() < limit {
        h.process();
        events.extend(sb.manager.poll());
        if done(events) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn a_plugin_that_announces_from_inside_its_activation_is_activated_once_in_the_sandbox() {
    for format in FORMATS {
        let (mut sb, mut h, id) = start(format, Kind::Chatty);
        let mut events = Vec::new();
        // Without the rule every activation asks for the next one. Give it time to loop.
        for _ in 0..400 {
            h.process();
            events.extend(sb.manager.poll());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(restarts(&events, id), 0, "{}: {events:?}", format.name());
        assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
        drop(h);
        sb.manager.poll();
        sb.finish();
    }
}

#[test]
fn a_plugin_that_asks_for_a_restart_after_every_block_is_cut_off_in_the_sandbox() {
    for format in FORMATS {
        let (mut sb, mut h, id) = start(format, Kind::Storm);
        let mut events = Vec::new();
        let warned = |events: &[ManagerEvent]| {
            events.iter().any(|e| {
                matches!(e, ManagerEvent::Log { instance, level: LogLevel::Warning, text }
                    if *instance == id && text.contains("keeps asking"))
            })
        };
        let name = format.name();
        assert!(
            drive_until(&mut sb, &mut h, &mut events, warned),
            "{name}: no warning in {events:?}"
        );
        // Let it keep asking for a while: nothing more is honoured, nothing is said twice.
        for _ in 0..300 {
            h.process();
            events.extend(sb.manager.poll());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(restarts(&events, id), RESTART_BURST as usize, "{name}");
        let said = events
            .iter()
            .filter(|e| {
                matches!(e, ManagerEvent::Log { level: LogLevel::Warning, text, .. }
                if text.contains("keeps asking"))
            })
            .count();
        assert_eq!(said, 1, "{name}: said once");
        process_settled(&mut h, 10, SETTLE);
        assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
        drop(h);
        sb.manager.poll();
        sb.finish();
    }
}

#[test]
fn renamed_params_of_a_sandboxed_plugin_reach_the_app_without_a_restart() {
    for format in FORMATS {
        let (mut sb, mut h, id) = start(format, Kind::Retitle);
        let name = format.name();
        assert_eq!(sb.manager.layout(id).unwrap().params[0].name, "Level");
        // The plugin renames its param when it is set to half or more.
        h.set_param("p1", 1.0);
        let mut events = Vec::new();
        let got = drive_until(&mut sb, &mut h, &mut events, |events| {
            events
                .iter()
                .any(|e| matches!(e, ManagerEvent::ParamTitlesChanged { .. }))
        });
        assert!(got, "{name}: no ParamTitlesChanged in {events:?}");
        let titles = events.iter().find_map(|e| match e {
            ManagerEvent::ParamTitlesChanged { instance, layout } if *instance == id => {
                Some(layout)
            }
            _ => None,
        });
        assert_eq!(titles.unwrap().params[0].name, "Drive", "{name}");
        assert_eq!(sb.manager.layout(id).unwrap().params[0].name, "Drive");
        // Give a restart time to show up if there were one.
        for _ in 0..200 {
            h.process();
            events.extend(sb.manager.poll());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(restarts(&events, id), 0, "{name}: {events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ManagerEvent::LayoutChanged { .. })),
            "{name}: {events:?}"
        );
        drop(h);
        sb.manager.poll();
        sb.finish();
    }
}
