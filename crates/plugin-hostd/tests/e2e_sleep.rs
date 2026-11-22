//! Plugins that sleep and a bypassed effect that is not called, through the real sandbox: the
//! rules look only at batch content and at what the plugin returned, so a plugin in its own
//! process renders the same bits, block for block, as the same plugin in this process, while it
//! is put to sleep, woken by its input and bypassed.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::{PluginRef, RunMode};
use atomik_plugin_host::PluginManager;
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    FORMATS, Format, METER_STATE, SR, Sandbox, bits, config, hex, in_process, meters, request,
};

/// The ringing effects of a format's variants library: tail of two seconds (rings two seconds),
/// no tail (rings one), a tail without end (rings one).
fn ringing(format: Format) -> Vec<(&'static str, PluginRef)> {
    let ids: [(&'static str, String); 3] = match format {
        Format::Clap => [
            ("two seconds", "atomik.test.tail-2s".to_string()),
            (
                "under-reported",
                "atomik.test.tail-underreported".to_string(),
            ),
            ("infinite", "atomik.test.tail-infinite".to_string()),
        ],
        Format::Vst3 => [
            ("two seconds", 17),
            ("under-reported", 18),
            ("infinite", 19),
        ]
        .map(|(n, w)| (n, hex([0xA70F_1C02, 0x4A5B_4C01, 0x8D2E_1F00, w]))),
    };
    ids.into_iter()
        .map(|(n, id)| {
            (
                n,
                PluginRef {
                    format: format.plugin_format(),
                    id,
                },
            )
        })
        .collect()
}

/// What a script leaves behind: the bits of the output of every block and the state meter at
/// every block.
#[derive(Debug, PartialEq)]
struct Render {
    out: Vec<Vec<u32>>,
    state: Vec<u32>,
}

/// Renders the script offline-activated (the link waits for every answer), with the device in
/// `mode`: a burst of input at the start, a long silence in which the plugin falls asleep, a
/// second burst that wakes it, a bypass switched on after the ring and off again.
fn script(manager: &mut PluginManager, plugin: &PluginRef, mode: RunMode) -> Render {
    let handle = manager
        .instantiate(&request(plugin, RunMode::Offline))
        .expect("the plugin starts");
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    h.device_mut().set_run_mode(mode);
    let (again, bypass_on, bypass_off, blocks) = (4_000, 6_500, 7_000, 7_400);
    let mut out = Vec::new();
    let mut state = Vec::new();
    for n in 0..blocks {
        let x = if n < 3 || (again..again + 3).contains(&n) {
            0.5
        } else {
            0.0
        };
        h.input_mut("in_l").fill(x);
        h.input_mut("in_r").fill(x);
        if n == bypass_on {
            h.set_param("bypass", 1.0);
        }
        if n == bypass_off {
            h.set_param("bypass", 0.0);
        }
        h.process();
        out.push(bits(h.output("out_l")));
        state.push(meters(&h)[METER_STATE].to_bits());
    }
    drop(h);
    manager.poll();
    Render { out, state }
}

#[test]
fn a_plugin_that_sleeps_and_is_woken_renders_the_same_bits_in_the_sandbox_and_in_process() {
    for format in FORMATS {
        let file = format.install(&format!("atomik-test-{}-variants", format.name()));
        let list = ringing(format);
        let refs: Vec<PluginRef> = list.iter().map(|(_, p)| p.clone()).collect();
        let mut sb = Sandbox::new(format.plugin_format(), config(), &[(file.clone(), refs)]);
        let mut local = in_process(format, &[file]);
        for (name, plugin) in &list {
            for mode in [RunMode::Realtime, RunMode::Offline] {
                let want = script(&mut local, plugin, mode);
                let got = script(&mut sb.manager, plugin, mode);
                assert!(
                    got == want,
                    "{} {name} {mode:?}: the same bits and states, block for block",
                    format.name()
                );
                // The plugin did sleep (state 1) between its ring and the second burst, and
                // while it was bypassed live it was stopped (state 2); offline it never is.
                assert!(
                    got.state[3_000] == 1.0f32.to_bits(),
                    "{} {name}: asleep before the second burst",
                    format.name()
                );
                assert!(
                    got.state[4_100] == 0.0f32.to_bits(),
                    "{} {name}: awake after it",
                    format.name()
                );
                let bypassed = got.state[6_900];
                assert_eq!(
                    bypassed == 2.0f32.to_bits(),
                    mode == RunMode::Realtime,
                    "{} {name} {mode:?}: parked only live",
                    format.name()
                );
            }
        }
        drop(local);
        sb.finish();
    }
}
