//! Plugin test fixtures: layouts for a CLAP synth, a CLAP effect with a sidechain, a VST3
//! effect and a note effect, their registration, and the command helpers.
#![allow(dead_code)]

use super::*;
use atomik_device_sdk::Registry;
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    descriptor_for,
};
use atomik_model::{Command, PluginCommand};

pub const CLAP_SYNTH: &str = "clap.test.synth";
pub const CLAP_FX: &str = "clap.test.fx";
pub const CLAP_NOTES: &str = "clap.test.noteshift";
pub const VST3_ID: &str = "00112233445566778899aabbccddeeff";
pub const VST3_FX: &str = "vst3.00112233445566778899aabbccddeeff";

pub fn param(id: u32, name: &str, min: f64, max: f64, default: f64) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: name.into(),
        module: String::new(),
        min,
        max,
        default,
        stepped: false,
        steps: 0,
    }
}

pub fn stepped(id: u32, name: &str, min: f64, max: f64, default: f64) -> PluginParamInfo {
    PluginParamInfo {
        stepped: true,
        ..param(id, name, min, max, default)
    }
}

/// An instrument: notes in, stereo out, three params (10 cutoff, 11 resonance, 42 voices).
pub fn synth_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Synth".into(),
        vendor: "Atomik Test".into(),
        version: "1.2.3".into(),
        kind_hint: PluginKindHint::Instrument,
        main_in: None,
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: true,
        notes_out: false,
        params: vec![
            PluginParamInfo {
                module: "Filter".into(),
                ..param(10, "Cutoff", 0.0, 1.0, 0.5)
            },
            PluginParamInfo {
                module: "Filter".into(),
                ..param(11, "Resonance", 0.0, 1.0, 0.0)
            },
            stepped(42, "Voices", 1.0, 16.0, 8.0),
        ],
    }
}

/// A stereo effect with a sidechain: params 1 (gain dB), 2 (mute, stepped) and 3 (mix).
pub fn fx_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Fx".into(),
        vendor: "Atomik Test".into(),
        version: "2.0.0".into(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: true,
        notes_in: false,
        notes_out: false,
        params: vec![
            param(1, "Gain", -60.0, 12.0, 0.0),
            stepped(2, "Mute", 0.0, 1.0, 0.0),
            param(3, "Mix", 0.0, 1.0, 1.0),
        ],
    }
}

/// The same effect without a sidechain and without the `Mix` param (an "older version").
pub fn fx_layout_old() -> PluginLayout {
    PluginLayout {
        sidechain: false,
        params: fx_layout().params.into_iter().take(2).collect(),
        ..fx_layout()
    }
}

/// A VST3 stereo effect (normalised params; param 7 is stepped with 4 steps).
pub fn vst3_layout() -> PluginLayout {
    PluginLayout {
        name: "Vee Fx".into(),
        vendor: "Vee".into(),
        version: "1.0".into(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: false,
        notes_out: false,
        params: vec![
            param(5, "Drive", 0.0, 1.0, 0.25),
            PluginParamInfo {
                stepped: true,
                steps: 4,
                ..param(7, "Mode", 0.0, 1.0, 0.5)
            },
        ],
    }
}

/// A note effect: notes in and out, no audio.
pub fn notes_layout() -> PluginLayout {
    PluginLayout {
        name: "Note Shift".into(),
        vendor: "Atomik Test".into(),
        version: "1.0.0".into(),
        kind_hint: PluginKindHint::NoteEffect,
        main_in: None,
        main_out: None,
        sidechain: false,
        notes_in: true,
        notes_out: true,
        params: vec![stepped(1, "Semitones", -24.0, 24.0, 0.0)],
    }
}

pub fn clap_ref(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.into(),
    }
}

pub fn synth_ref() -> PluginRef {
    clap_ref("test.synth")
}

pub fn fx_ref() -> PluginRef {
    clap_ref("test.fx")
}

pub fn vst3_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Vst3,
        id: VST3_ID.into(),
    }
}

pub fn notes_ref() -> PluginRef {
    clap_ref("test.noteshift")
}

/// Registers one plugin type from its layout.
pub fn register_plugin(r: &mut Registry, plugin: &PluginRef, layout: &PluginLayout) {
    let desc = descriptor_for(plugin, layout).unwrap();
    r.register_plugin(plugin.type_id(), desc).unwrap();
}

/// Registers the four test plugins.
pub fn register_plugins(r: &mut Registry) {
    register_plugin(r, &synth_ref(), &synth_layout());
    register_plugin(r, &fx_ref(), &fx_layout());
    register_plugin(r, &vst3_ref(), &vst3_layout());
    register_plugin(r, &notes_ref(), &notes_layout());
}

/// [`registry`] plus the test plugins (no console devices).
pub fn plugin_registry() -> Registry {
    let mut r = registry();
    register_plugins(&mut r);
    r
}

/// [`console_registry`] plus the test plugins.
pub fn plugin_console_registry() -> Registry {
    let mut r = console_registry();
    register_plugins(&mut r);
    r
}

/// `PluginCommand::Add` at the top level.
pub fn add_plugin(
    index: usize,
    plugin: &PluginRef,
    layout: &PluginLayout,
    k: Option<&str>,
) -> Command {
    Command::Plugin(PluginCommand::Add {
        index,
        container: None,
        plugin: plugin.clone(),
        layout: layout.clone(),
        path_hint: None,
        key: k.map(key),
        auto_split: false,
    })
}

/// `PluginCommand::Add` inside a container (path key), with a local key.
pub fn add_plugin_inner(
    container: &str,
    index: usize,
    plugin: &PluginRef,
    layout: &PluginLayout,
    k: Option<&str>,
    auto_split: bool,
) -> Command {
    Command::Plugin(PluginCommand::Add {
        index,
        container: Some(key(container)),
        plugin: plugin.clone(),
        layout: layout.clone(),
        path_hint: None,
        key: k.map(key),
        auto_split,
    })
}

pub fn sync_state(device: &str, bytes: &[u8]) -> Command {
    Command::Plugin(PluginCommand::SyncState {
        device: key(device),
        state: atomik_device_sdk::PluginStateBlob(bytes.to_vec()),
    })
}

pub fn set_pinned(device: &str, ids: &[u32]) -> Command {
    Command::Plugin(PluginCommand::SetPinned {
        device: key(device),
        pinned: ids.to_vec(),
    })
}

pub fn relink(
    device: &str,
    plugin: &PluginRef,
    layout: &PluginLayout,
    hint: Option<&str>,
) -> Command {
    Command::Plugin(PluginCommand::Relink {
        device: key(device),
        plugin: plugin.clone(),
        layout: layout.clone(),
        path_hint: hint.map(str::to_owned),
    })
}

/// A document with a plugin synth `synth` and a plugin effect `fx` cabled synth -> fx.
pub fn plugin_chain(r: &Registry) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("synth")),
            add_plugin(1, &fx_ref(), &fx_layout(), Some("fx")),
            connect("synth:out_l", "fx:in_l"),
            connect("synth:out_r", "fx:in_r"),
        ],
    );
    d
}

/// The plugin object of device `k` (top level) of a document.
pub fn plugin_of<'d>(doc: &'d atomik_model::RackDoc, k: &str) -> &'d atomik_model::PluginDoc {
    doc.device(&key(k))
        .unwrap_or_else(|| panic!("no device {k}"))
        .plugin
        .as_deref()
        .unwrap_or_else(|| panic!("{k} has no plugin doc"))
}
