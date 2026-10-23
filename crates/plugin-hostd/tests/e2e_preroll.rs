//! A bounce's pre-roll polls the manager after every batch and ends after a few quiet ones, so
//! what a plugin posts (a latency change, a restart request) must reach the manager at the same
//! batch whether the plugin is in process or in a child: never later because of a timer (HI-2).
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{ManagerEvent, PluginManager};
use sandbox_support::{
    FORMATS, Format, Plug, Sandbox, bits, effect, gain_state_with_latency, in_process, out_l,
    request, request_with_state,
};

/// What one poll saw, without the instance id.
fn kind(e: &ManagerEvent) -> String {
    match e {
        ManagerEvent::LatencyChanged { frames, .. } => format!("latency {frames}"),
        ManagerEvent::Restarted(_) => "restarted".to_string(),
        ManagerEvent::RestartFailed { .. } => "restart failed".to_string(),
        other => format!("{other:?}"),
    }
}

/// Drives the gain effect like the bounce's pre-roll: a batch, then a poll. Returns, per batch,
/// the events that poll returned and the output.
fn pre_roll(
    format: Format,
    manager: &mut PluginManager,
    state: Option<atomik_device_sdk::PluginStateBlob>,
    latency_param_at: Option<usize>,
) -> Vec<(Vec<String>, Vec<u32>)> {
    let plugin = format.plugin(Plug::Gain);
    let handle = match state {
        Some(s) => manager.instantiate(&request_with_state(&plugin, RunMode::Offline, s)),
        None => manager.instantiate(&request(&plugin, RunMode::Offline)),
    }
    .unwrap();
    let mut h = effect(handle, 0.25);
    let mut seen = Vec::new();
    for i in 0..40 {
        if latency_param_at == Some(i) {
            h.set_param("p3", 100.0);
        }
        h.process();
        let events = manager.poll().iter().map(kind).collect();
        seen.push((events, bits(&out_l(&h))));
    }
    drop(h);
    manager.poll();
    seen
}

fn compare(format: Format, state: Option<atomik_device_sdk::PluginStateBlob>, at: Option<usize>) {
    let mut local = in_process(format, &[format.main_file()]);
    let want = pre_roll(format, &mut local, state.clone(), at);
    let mut sb = Sandbox::real(format);
    let got = pre_roll(format, &mut sb.manager, state, at);
    sb.finish();
    let events = |v: &[(Vec<String>, Vec<u32>)]| -> Vec<(usize, Vec<String>)> {
        v.iter()
            .enumerate()
            .filter(|(_, (e, _))| !e.is_empty())
            .map(|(i, (e, _))| (i, e.clone()))
            .collect()
    };
    assert_eq!(
        events(&got),
        events(&want),
        "{}: the same events at the same batches",
        format.name()
    );
    assert!(
        got.iter().zip(&want).all(|(g, w)| g.1 == w.1),
        "{}: the same output in every batch",
        format.name()
    );
}

#[test]
fn a_latency_change_after_a_state_restore_arrives_at_the_same_batch_in_the_sandbox_as_in_process() {
    for format in FORMATS {
        compare(format, Some(gain_state_with_latency(format, 100)), None);
    }
}

#[test]
fn a_latency_change_made_by_a_param_arrives_at_the_same_batch_in_the_sandbox_as_in_process() {
    for format in FORMATS {
        compare(format, None, Some(3));
    }
}

#[test]
fn the_in_process_reference_really_posts_something_to_compare() {
    for format in FORMATS {
        let mut local = in_process(format, &[format.main_file()]);
        let a = pre_roll(
            format,
            &mut local,
            Some(gain_state_with_latency(format, 100)),
            None,
        );
        let b = pre_roll(format, &mut local, None, Some(3));
        assert!(
            b.iter()
                .any(|(e, _)| e.iter().any(|k| k.starts_with("latency"))),
            "{}: a param makes the latency change",
            format.name()
        );
        println!(
            "{} state restore events: {:?}",
            format.name(),
            a.iter()
                .enumerate()
                .filter(|(_, (e, _))| !e.is_empty())
                .map(|(i, (e, _))| (i, e.clone()))
                .collect::<Vec<_>>()
        );
    }
}
