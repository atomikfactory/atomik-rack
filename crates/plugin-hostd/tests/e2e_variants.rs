//! Every test plugin library through the real sandbox, on every platform: a library whose entry
//! fails, one whose entry never returns, one created and destroyed many times, and the edge-case
//! plugins (mono, split component and controller, refused layouts, plugins that fail at each
//! step or output NaN). A sandboxed plugin must end up where the same plugin ends up in this
//! process, and no child may be left behind.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]
// Justification: the hang test measures how long the sandbox waited.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::{PluginRef, RunMode};
use atomik_plugin_host::{PluginHandle, PluginManager};
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    FORMATS, Format, METER_STATE, SETTLE, SR, Sandbox, bits, config, hex, meters, process_settled,
    request,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn plugin(format: Format, id: String) -> PluginRef {
    PluginRef {
        format: format.plugin_format(),
        id,
    }
}

/// The one plugin of a single-plugin library (`bad`, `hang`, `leak`).
fn single(format: Format, kind: &str) -> (PathBuf, PluginRef) {
    let (package, clap_id, vst3_word) = match kind {
        "bad" => ("bad", "atomik.test.bad", 0xA70F_1C03),
        "hang" => ("hang", "atomik.test.hang", 0xA70F_1C04),
        _ => ("leak", "atomik.test.leak", 0xA70F_1C05),
    };
    let file = format.install(&format!("atomik-test-{}-{package}", format.name()));
    let id = match format {
        Format::Clap => clap_id.to_string(),
        Format::Vst3 => hex([vst3_word, 0x4A5B_4C01, 0x8D2E_1F00, 1]),
    };
    (file, plugin(format, id))
}

/// The edge-case plugins of a format's variants library, with a name for the messages.
fn variants(format: Format) -> (PathBuf, Vec<(&'static str, PluginRef)>) {
    let file = format.install(&format!("atomik-test-{}-variants", format.name()));
    let list: Vec<(&'static str, String)> = match format {
        Format::Clap => [
            ("mono", "atomik.test.mono"),
            ("midi synth", "atomik.test.midisynth"),
            ("multi out", "atomik.test.multiout"),
            ("init fails", "atomik.test.init-fails"),
            ("activate fails", "atomik.test.activate-fails"),
            ("start fails", "atomik.test.start-fails"),
            ("process error", "atomik.test.error"),
            ("nan", "atomik.test.nan"),
        ]
        .into_iter()
        .map(|(n, id)| (n, id.to_string()))
        .collect(),
        Format::Vst3 => [
            ("split gain", 1),
            ("mono", 2),
            ("surround", 3),
            ("init fails", 4),
            ("activate fails", 5),
            ("start fails", 6),
            ("process error", 7),
            ("nan", 8),
            ("picky synth", 9),
        ]
        .into_iter()
        .map(|(n, w)| (n, hex([0xA70F_1C02, 0x4A5B_4C01, 0x8D2E_1F00, w])))
        .collect(),
    };
    (
        file,
        list.into_iter()
            .map(|(n, id)| (n, plugin(format, id)))
            .collect(),
    )
}

/// A harness over a device, with every main input it has at `level`.
fn drive(handle: PluginHandle, level: f32) -> DeviceHarness {
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    for key in ["in_l", "in_r"] {
        if h.device().descriptor().socket_by_key(key).is_some() {
            h.input_mut(key).fill(level);
        }
    }
    h
}

/// The bits of every audio output the device has.
fn outputs(h: &DeviceHarness) -> Vec<Vec<u32>> {
    ["out_l", "out_r"]
        .into_iter()
        .filter(|k| h.device().descriptor().socket_by_key(k).is_some())
        .map(|k| bits(h.output(k)))
        .collect()
}

/// What a plugin ends up as: refused (with the error text), or the device state and output
/// after a few batches.
#[derive(Debug, PartialEq)]
enum Outcome {
    Refused,
    Ran { state: u32, out: Vec<Vec<u32>> },
}

fn outcome(manager: &mut PluginManager, plugin: &PluginRef, settle: bool) -> Outcome {
    match manager.instantiate(&request(plugin, RunMode::Realtime)) {
        Err(_) => Outcome::Refused,
        Ok(handle) => {
            let mut h = drive(handle, 0.25);
            if settle {
                process_settled(&mut h, 20, SETTLE);
            } else {
                for _ in 0..20 {
                    h.process();
                }
            }
            let state = meters(&h)[METER_STATE].to_bits();
            let out = outputs(&h);
            drop(h);
            manager.poll();
            Outcome::Ran { state, out }
        }
    }
}

#[test]
fn a_library_whose_entry_fails_fails_instantiate_and_leaves_no_child() {
    for format in FORMATS {
        let (file, bad) = single(format, "bad");
        let mut sb = Sandbox::new(
            format.plugin_format(),
            config(),
            &[(file, vec![bad.clone()])],
        );
        let r = sb.manager.instantiate(&request(&bad, RunMode::Realtime));
        assert!(
            r.is_err(),
            "{}: a library that fails loads nothing",
            format.name()
        );
        sb.finish();
    }
}

#[test]
fn a_library_whose_entry_never_returns_is_stopped_at_the_load_timeout() {
    for format in FORMATS {
        let (file, hang) = single(format, "hang");
        let mut c = config();
        c.load_timeout = Duration::from_secs(3);
        let mut sb = Sandbox::new(format.plugin_format(), c, &[(file, vec![hang.clone()])]);
        let started = Instant::now();
        let r = sb.manager.instantiate(&request(&hang, RunMode::Realtime));
        assert!(
            r.is_err(),
            "{}: a library that hangs loads nothing",
            format.name()
        );
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{}: the wait ends at the timeout ({:?})",
            format.name(),
            started.elapsed()
        );
        // The child is stopped, not left hanging.
        sb.finish();
    }
}

#[test]
fn instances_created_and_destroyed_in_a_row_leave_no_child_behind() {
    for format in FORMATS {
        let (file, leak) = single(format, "leak");
        let mut sb = Sandbox::new(
            format.plugin_format(),
            config(),
            &[(file, vec![leak.clone()])],
        );
        for round in 0..8 {
            let handle = sb
                .manager
                .instantiate(&request(&leak, RunMode::Realtime))
                .unwrap_or_else(|e| panic!("{} round {round}: {e}", format.name()));
            let mut h = drive(handle, 0.25);
            process_settled(&mut h, 5, SETTLE);
            assert!(
                h.output("out_l").iter().all(|s| *s == 0.25),
                "{}: the pass-through passes",
                format.name()
            );
            drop(h);
            sb.manager.poll();
            assert!(
                sb.wait_children(0, Duration::from_secs(10)),
                "{} round {round}: the child of a dropped instance ends",
                format.name()
            );
        }
        sb.finish();
    }
}

#[test]
fn every_edge_case_plugin_ends_up_in_the_sandbox_where_it_ends_up_in_process() {
    for format in FORMATS {
        let (file, plugins) = variants(format);
        let refs: Vec<PluginRef> = plugins.iter().map(|(_, p)| p.clone()).collect();
        let mut sb = Sandbox::new(format.plugin_format(), config(), &[(file.clone(), refs)]);
        let mut local = sandbox_support::in_process(format, &[file]);
        let mut ran = 0;
        for (name, p) in &plugins {
            let want = outcome(&mut local, p, false);
            let got = outcome(&mut sb.manager, p, true);
            assert_eq!(got, want, "{} {name}", format.name());
            ran += usize::from(matches!(got, Outcome::Ran { .. }));
        }
        assert!(ran >= 2, "{}: some of them run at all", format.name());
        drop(local);
        sb.finish();
    }
}
