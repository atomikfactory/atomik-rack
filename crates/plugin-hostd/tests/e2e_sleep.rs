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

/// Renders the script (the link waits for every answer): a burst of input at the start, a long
/// silence in which the plugin falls asleep, a second burst that wakes it, a short bypass inside
/// the park time, a long bypass that parks the effect, and the bypass off again.
fn script(manager: &mut PluginManager, plugin: &PluginRef) -> Render {
    let handle = manager
        .instantiate(&request(plugin, RunMode::Offline))
        .expect("the plugin starts");
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    let (again, short_on, short_off, long_on, long_off, blocks) =
        (4_000, 5_000, 5_200, 6_500, 14_200, 14_600);
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
        if n == short_on || n == long_on {
            h.set_param("bypass", 1.0);
        }
        if n == short_off || n == long_off {
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
fn a_plugin_that_sleeps_is_woken_and_bypassed_renders_the_same_bits_in_the_sandbox_and_in_process()
{
    for format in FORMATS {
        let file = format.install(&format!("atomik-test-{}-variants", format.name()));
        let list = ringing(format);
        let refs: Vec<PluginRef> = list.iter().map(|(_, p)| p.clone()).collect();
        let mut sb = Sandbox::new(format.plugin_format(), config(), &[(file.clone(), refs)]);
        let mut local = in_process(format, &[file]);
        for (name, plugin) in &list {
            let want = script(&mut local, plugin);
            let got = script(&mut sb.manager, plugin);
            assert!(
                got == want,
                "{} {name}: the same bits and states, block for block",
                format.name()
            );
            // A VST3 plugin with an infinite tail is never put to sleep; the others are asleep
            // before the second burst (state 1) and awake after it (0).
            let never = format == Format::Vst3 && *name == "infinite";
            let asleep = if never { 0.0f32 } else { 1.0 };
            assert!(
                got.state[3_600] == asleep.to_bits(),
                "{} {name}: asleep before the second burst",
                format.name()
            );
            assert!(
                got.state[4_100] == 0.0f32.to_bits(),
                "{} {name}: awake after it",
                format.name()
            );
            // Inside the park time the bypassed plugin is not stopped; after it, it is.
            assert!(
                got.state[5_100] != 2.0f32.to_bits(),
                "{} {name}",
                format.name()
            );
            assert!(
                got.state[14_100] == 2.0f32.to_bits(),
                "{} {name}: parked by the long bypass",
                format.name()
            );
        }
        drop(local);
        sb.finish();
    }
}

/// The state captured from the real gain effect after its gain was set to -6 dB, with the effect
/// bypassed for `bypass_blocks` blocks first.
fn captured_after_set(
    manager: &mut PluginManager,
    format: Format,
    bypass_blocks: usize,
) -> Vec<u8> {
    let handle = manager
        .instantiate(&request(
            &format.plugin(sandbox_support::Plug::Gain),
            RunMode::Offline,
        ))
        .expect("the plugin starts");
    let id = handle.id;
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    h.input_mut("in_l").fill(0.25);
    h.input_mut("in_r").fill(0.25);
    if bypass_blocks > 0 {
        h.set_param("bypass", 1.0);
    }
    for _ in 0..bypass_blocks {
        h.process();
    }
    h.set_param("p1", format.gain_value(-6.0));
    for _ in 0..5 {
        h.process();
    }
    let blob = manager.capture_state(id).expect("the state is captured");
    drop(h);
    manager.poll();
    blob.0
}

#[test]
fn a_value_set_while_the_effect_is_parked_is_in_the_saved_state_in_the_sandbox_too() {
    for format in FORMATS {
        let file = format.main_file();
        let gain = format.plugin(sandbox_support::Plug::Gain);
        let mut sb = Sandbox::new(
            format.plugin_format(),
            config(),
            &[(file.clone(), vec![gain])],
        );
        let mut local = in_process(format, &[file]);
        let running = captured_after_set(&mut local, format, 0);
        let parked_local = captured_after_set(&mut local, format, 7_700);
        let parked_sandbox = captured_after_set(&mut sb.manager, format, 7_700);
        assert_eq!(parked_local, running, "{}: in process", format.name());
        assert_eq!(parked_sandbox, running, "{}: in the sandbox", format.name());
        drop(local);
        sb.finish();
    }
}
