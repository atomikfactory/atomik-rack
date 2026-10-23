//! The real CLAP and VST3 backends behind the child: the plugin gets exactly the bits the parent
//! sent, so the result equals the same plugin run in this process (HI-2, AC-2 child side), and a
//! crashing plugin ends the child and not the test (AC-5).

mod support;

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin};
use atomik_plugin_host::{
    ActivateConfig, AudioThreadGuard, EventList, HostHandle, InstanceId, PluginBackend,
    PluginEvent, PluginEventKind, PluginIo, PluginTransport, ProcessStatus, mark_main_thread,
    plugin_channel,
};
use atomik_plugin_ipc::{AudioOp, FromChild, ToChild, WireLayout};
use atomik_plugin_vst3::Vst3Backend;
use std::path::{Path, PathBuf};
use support::{PATIENCE, SR, Spawn, TestParent, note_off, note_on, param};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Clap,
    Vst3,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
        }
    }

    fn file(self, package: &str) -> PathBuf {
        match self {
            Format::Clap => clap_test_plugin(package).expect("the test plugin builds"),
            Format::Vst3 => vst3_test_plugin(package).expect("the test plugin builds"),
        }
    }

    fn id(self, kind: Kind) -> String {
        let n = match kind {
            Kind::Gain => 1,
            Kind::Synth => 2,
        };
        match (self, kind) {
            (Format::Clap, Kind::Gain) => "atomik.test.gain".to_string(),
            (Format::Clap, Kind::Synth) => "atomik.test.synth".to_string(),
            (Format::Vst3, _) => hex([0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, n]),
        }
    }

    fn package(self) -> &'static str {
        match self {
            Format::Clap => "atomik-test-clap",
            Format::Vst3 => "atomik-test-vst3",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Gain,
    Synth,
}

fn hex(words: [u32; 4]) -> String {
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        words[0], words[1], words[2], words[3]
    )
}

/// One block of a script: inputs and events.
struct Step {
    main_in: Vec<Batch>,
    events: Vec<PluginEvent>,
}

/// Noise between -1 and 1 that depends only on `seed`.
fn noise(seed: u32) -> Batch {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
    let mut b = [0.0; BATCH];
    for s in &mut b {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s = (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
    }
    b
}

fn gain_script(format: Format) -> Vec<Step> {
    // CLAP takes the gain in dB, VST3 a normalised value; both run the same way in both worlds.
    let gain = if format == Format::Clap { -6.0 } else { 0.4 };
    let events: Vec<Vec<PluginEvent>> = vec![
        vec![],
        vec![param(10, 1, gain)],
        vec![],
        vec![param(0, 2, 1.0)],
        vec![param(33, 2, 0.0), param(33, 1, gain * 0.5)],
        vec![],
        vec![],
        vec![param(63, 1, gain)],
    ];
    events
        .into_iter()
        .enumerate()
        .map(|(i, events)| Step {
            main_in: vec![noise(i as u32 * 2 + 1), noise(i as u32 * 2 + 2)],
            events,
        })
        .collect()
}

fn synth_script() -> Vec<Step> {
    let events: Vec<Vec<PluginEvent>> = vec![
        vec![],
        vec![note_on(5, 60, 0.8, 1)],
        vec![],
        vec![
            note_on(20, 64, 0.5, 2),
            PluginEvent {
                offset: 30,
                kind: PluginEventKind::PitchBend { value: 0.25 },
            },
        ],
        vec![note_off(3, 60, 1)],
        vec![],
        vec![note_off(60, 64, 2)],
        vec![],
    ];
    events
        .into_iter()
        .map(|events| Step {
            main_in: Vec::new(),
            events,
        })
        .collect()
}

type Outputs = Vec<(ProcessStatus, [Batch; 2])>;

fn backend_for(format: Format, file: &Path, id: &str) -> Box<dyn PluginBackend> {
    match format {
        Format::Clap => {
            let b = ClapBackend::new();
            b.register_plugin_file(id, file);
            Box::new(b)
        }
        Format::Vst3 => {
            let b = Vst3Backend::new();
            b.register_plugin_file(id, file);
            Box::new(b)
        }
    }
}

fn plugin_ref(format: Format, id: &str) -> PluginRef {
    PluginRef {
        format: match format {
            Format::Clap => PluginFormat::Clap,
            Format::Vst3 => PluginFormat::Vst3,
        },
        id: id.to_string(),
    }
}

/// The reference: the same plugin, the same script, in this process through the backend.
fn in_process(
    format: Format,
    file: &Path,
    id: &str,
    script: &[Step],
) -> (Outputs, WireLayout, bool) {
    mark_main_thread();
    let backend = backend_for(format, file, id);
    let (_device_end, manager) = plugin_channel(InstanceId(1));
    let mut instance = backend
        .instantiate(
            &plugin_ref(format, id),
            HostHandle::new(manager.messenger()),
        )
        .expect("the plugin is created");
    let layout = WireLayout::from(instance.layout());
    let has_gui = instance.has_gui();
    let mut processor = instance
        .activate(&ActivateConfig {
            sample_rate: SR,
            frames: BATCH as u32,
            mode: RunMode::Offline,
        })
        .expect("the plugin activates");
    let guard = AudioThreadGuard::enter();
    assert!(processor.start_processing());
    let mut results = Vec::new();
    for step in script {
        let mut main_out = [[0.0; BATCH]; 2];
        let mut events_out = EventList::with_capacity(512);
        let status = {
            let mut io = PluginIo {
                frames: BATCH,
                main_in: &step.main_in,
                main_in_silent: false,
                sidechain: &[],
                sidechain_silent: true,
                main_out: &mut main_out,
                events_in: &step.events,
                events_out: &mut events_out,
                transport: PluginTransport::default(),
                dropped: 0,
            };
            processor.process(&mut io)
        };
        results.push((status, main_out));
    }
    processor.stop_processing();
    drop(guard);
    instance.deactivate(processor);
    (results, layout, has_gui)
}

/// The same script through the child.
fn through_child(spec: &Spawn, script: &[Step]) -> (Outputs, WireLayout, bool) {
    let mut p = TestParent::spawn(spec);
    p.hello();
    let (layout, has_gui, _) = p.instantiate();
    p.activate(true);
    assert!(p.op(AudioOp::Start).ok);
    let mut results = Vec::new();
    for step in script {
        let out = p.process(&step.main_in, &step.events);
        results.push((out.status, out.main_out));
    }
    p.op(AudioOp::Stop);
    p.deactivate();
    (results, layout, has_gui)
}

fn bits(b: &Batch) -> Vec<u32> {
    b.iter().map(|s| s.to_bits()).collect()
}

fn same_bits(format: Format, kind: Kind, script: &[Step]) {
    let file = format.file(format.package());
    let id = format.id(kind);
    let spec = Spawn::real(format.name(), &file, &id);
    let (want, want_layout, want_gui) = in_process(format, &file, &id, script);
    let (got, got_layout, got_gui) = through_child(&spec, script);
    assert_eq!(got_layout, want_layout, "the same layout");
    assert_eq!(got_gui, want_gui);
    assert_eq!(got.len(), want.len());
    for (i, ((gs, go), (ws, wo))) in got.iter().zip(&want).enumerate() {
        assert_eq!(gs, ws, "status of block {i}");
        for ch in 0..2 {
            assert_eq!(
                bits(&go[ch]),
                bits(&wo[ch]),
                "{format:?} {kind:?} block {i} channel {ch}"
            );
        }
    }
    let heard = got.iter().any(|(_, o)| o[0].iter().any(|s| s.abs() > 1e-6));
    assert!(
        heard,
        "the script makes sound, so the comparison means something"
    );
}

#[test]
fn the_clap_effect_gives_the_bits_it_gives_in_process() {
    same_bits(Format::Clap, Kind::Gain, &gain_script(Format::Clap));
}

#[test]
fn the_vst3_effect_gives_the_bits_it_gives_in_process() {
    same_bits(Format::Vst3, Kind::Gain, &gain_script(Format::Vst3));
}

#[test]
fn the_clap_instrument_gives_the_bits_it_gives_in_process() {
    same_bits(Format::Clap, Kind::Synth, &synth_script());
}

#[test]
fn the_vst3_instrument_gives_the_bits_it_gives_in_process() {
    same_bits(Format::Vst3, Kind::Synth, &synth_script());
}

#[test]
fn the_clap_state_round_trips_through_a_new_child() {
    let file = Format::Clap.file("atomik-test-clap");
    let id = Format::Clap.id(Kind::Gain);
    let spec = Spawn::real("clap", &file, &id);
    let mut a = TestParent::spawn(&spec);
    a.hello();
    a.instantiate();
    a.activate(false);
    a.process(
        &[noise(1), noise(2)],
        &[param(0, 1, -12.0), param(0, 2, 1.0)],
    );
    let (reply, blob) = a.call(|id| ToChild::CaptureState { id }, None);
    assert!(matches!(reply, FromChild::State { .. }), "{reply:?}");
    let blob = blob.expect("a blob");
    assert!(!blob.is_empty());

    let mut b = TestParent::spawn(&spec);
    b.hello();
    b.instantiate();
    let (reply, _) = b.call(|id| ToChild::RestoreState { id }, Some(&blob));
    assert!(matches!(reply, FromChild::Ok { .. }), "{reply:?}");
    let (_, again) = b.call(|id| ToChild::CaptureState { id }, None);
    assert_eq!(
        again.expect("a blob"),
        blob,
        "the restored plugin saves what it was given"
    );
}

#[test]
fn the_clap_param_text_is_the_plugins_own() {
    let file = Format::Clap.file("atomik-test-clap");
    let id = Format::Clap.id(Kind::Gain);
    let mut p = TestParent::spawn(&Spawn::real("clap", &file, &id));
    p.hello();
    p.instantiate();
    let child = match p
        .call(
            |id| ToChild::ParamText {
                id,
                param: 1,
                plain: -6.0,
            },
            None,
        )
        .0
    {
        FromChild::Text { text, .. } => text,
        other => panic!("{other:?}"),
    };
    mark_main_thread();
    let backend = backend_for(Format::Clap, &file, &id);
    let (_d, manager) = plugin_channel(InstanceId(7));
    let mut instance = backend
        .instantiate(
            &plugin_ref(Format::Clap, &id),
            HostHandle::new(manager.messenger()),
        )
        .unwrap();
    assert_eq!(child, instance.param_text(1, -6.0));
    assert!(child.is_some_and(|t| !t.is_empty()));
}

#[test]
fn a_file_that_is_not_a_plugin_fails_instantiate_with_an_error_and_the_child_lives() {
    let mut p = TestParent::spawn(&Spawn::real(
        "clap",
        Path::new("definitely-not-here.clap"),
        "x",
    ));
    p.hello();
    let (reply, _) = p.call(|id| ToChild::Instantiate { id }, None);
    assert!(matches!(reply, FromChild::Err { .. }), "{reply:?}");
    assert!(p.wait_exit(std::time::Duration::from_millis(100)).is_none());
}

fn crash_through(format: Format, package: &str, id: &str) {
    let file = format.file(package);
    let mut p =
        TestParent::spawn(&Spawn::real(format.name(), &file, id).env("ATOMIK_TEST_CRASH", "1"));
    p.hello();
    p.instantiate();
    p.activate(false);
    assert!(p.op(AudioOp::Start).ok);
    let t = p.submit_process(&[noise(1), noise(2)]);
    let status = p.wait_exit(PATIENCE).expect("the crash ends the child");
    assert!(!status.success(), "{status:?}");
    p.port.mark_dead();
    assert_eq!(
        p.port
            .wait_done(t, atomik_plugin_ipc::Deadline::after(PATIENCE)),
        atomik_plugin_ipc::Poll::Dead
    );
}

#[test]
fn clap_crash_crashes_the_child_and_not_the_test() {
    crash_through(Format::Clap, "atomik-test-clap-crash", "atomik.test.crash");
}

#[test]
fn vst3_crash_crashes_the_child_and_not_the_test() {
    crash_through(
        Format::Vst3,
        "atomik-test-vst3-crash",
        &hex([0xA70F_1C06, 0x4A5B_4C01, 0x8D2E_1F00, 1]),
    );
}

/// Opens the editor of the CLAP test effect in a real window of the child. Needs a desktop, so it
/// is not part of the default run: `cargo test -p atomik-plugin-hostd -- --ignored`.
#[test]
#[ignore = "opens a real window; run by hand"]
fn the_clap_editor_opens_in_a_window_of_the_child_and_closes_again() {
    let file = Format::Clap.file("atomik-test-clap");
    let id = Format::Clap.id(Kind::Gain);
    let mut p = TestParent::spawn(&Spawn::real("clap", &file, &id));
    p.hello();
    let (_, has_gui, _) = p.instantiate();
    assert!(has_gui);
    let open = |p: &mut TestParent| match p
        .call(
            |id| ToChild::OpenGui {
                id,
                owner: None,
                scale: 1.0,
            },
            None,
        )
        .0
    {
        FromChild::GuiOpened { w, h, .. } => (w, h),
        other => panic!("{other:?}"),
    };
    let (w, h) = open(&mut p);
    assert!(w > 0 && h > 0);
    assert_eq!(open(&mut p), (w, h), "a second open raises the same window");
    assert!(matches!(
        p.call(|id| ToChild::CloseGui { id }, None).0,
        FromChild::Ok { .. }
    ));
}
