//! Shared fixtures: the reference rack (3 devices, 4 cables) built directly as a `RackDoc`.
#![allow(dead_code, reason = "each test binary uses a subset")]

pub mod audio;
pub mod console;
pub mod container;
pub mod cpu;
pub mod drum;
pub mod fx;
pub mod plugin;
pub mod sampler;
pub mod seq;
pub mod slicer;
pub mod song;
pub mod starter;

use atomik_device_sdk::Registry;
use atomik_model::{Cable, DeviceKey, RackDevice, RackDoc, SocketRef};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent};
use std::collections::BTreeMap;

pub fn registry() -> Registry {
    let mut r = Registry::new();
    atomik_devices::register_all(&mut r).expect("register");
    plugin::register_test_plugins(&mut r);
    r
}

pub fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

pub fn sref(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

pub fn cable(from: (&str, &str), to: (&str, &str)) -> Cable {
    Cable {
        from: sref(from.0, from.1),
        to: sref(to.0, to.1),
    }
}

pub fn device(k: &str, type_id: &str, name: &str) -> RackDevice {
    RackDevice {
        key: key(k),
        type_id: type_id.to_string(),
        name: name.to_string(),
        params: BTreeMap::new(),
        folded: false,
        color: None,
        zones: Vec::new(),
        steps: None,
        curves: None,
        drums: None,
        inner: None,
        plugin: None,
        column: 0,
        patch_name: String::new(),
        midi_out: None,
    }
}

/// Synth, delay and mixer with 4 cables (audio, CV, audio, audio to the interface).
pub fn reference_doc() -> RackDoc {
    let mut synth = device("synth", "atomik.poly-synth", "Poly Synth");
    synth.params.insert("filter.cutoff".into(), 2400.0);
    synth.params.insert("osc2.wave".into(), 1.0);
    let mut delay = device("delay", "atomik.stereo-delay", "Stereo Delay");
    delay.params.insert("feedback".into(), 0.55);
    delay.params.insert("ping_pong".into(), 1.0);
    let mut mixer = device("mixer", "atomik.line-mixer", "Line Mixer");
    mixer.params.insert("ch1.level".into(), -6.0);
    mixer.params.insert("ch2.mute".into(), 1.0);
    let mut cables = vec![
        cable(("synth", "out_l"), ("delay", "in_l")),
        cable(("synth", "lfo_cv_out"), ("delay", "time_cv_in")),
        cable(("delay", "out_l"), ("mixer", "ch1_in_l")),
        cable(("mixer", "master_l"), ("interface", "out_1")),
    ];
    cables.sort();
    RackDoc {
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        seed: 0,
        output_channels: 2,
        input_channels: 0,
        assets: BTreeMap::new(),
        devices: vec![synth, delay, mixer],
        columns: 1,
        cables,
        song: atomik_model::empty_song(120.0).into(),
        console: atomik_model::Console::default(),
        midi_maps: Vec::new(),
        macros: Default::default(),
        info: Default::default(),
        tune_cents: 0.0,
    }
}

pub const VIEW: Size = Size::new(1000.0, 1200.0);

/// A synced view of the reference rack at 1x, sized to show everything.
pub fn view(doc: &RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(VIEW), doc, reg);
    v.sync(doc, reg);
    v
}

/// [`view`] with the console devices hidden: the rack with the console devices hidden (they show by
/// default now), for tests that are not about them.
pub fn plain_view(doc: &RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    v.set_show_console_devices(false);
    let _ = v.handle(&UiEvent::Resize(VIEW), doc, reg);
    v.sync(doc, reg);
    v
}

pub fn down(pos: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

pub fn down_with(pos: Point, button: PointerButton, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button,
        modifiers: Modifiers::NONE,
        clicks,
    }
}

pub fn mv(pos: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos,
        modifiers: Modifiers::NONE,
    }
}

pub fn mv_with(pos: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerMove { pos, modifiers }
}

pub fn up(pos: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

/// Only the commands among `actions`.
pub fn commands(actions: &[UiAction]) -> Vec<atomik_model::Command> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}
