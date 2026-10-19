//! Plugin fixtures: two plugin layouts (a CLAP synth with module-grouped params,
//! one of them stepped, and a CLAP effect with a sidechain), their registration, a rack with
//! both plus a container holding the effect, and the helpers the plugin tests share.

use super::*;
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    descriptor_for,
};
use atomik_model::{Command, Document, PluginCommand};
use atomik_ui::plugin_data::{PluginEntryInfo, PluginEntryStatus};

pub const SYNTH_ID: &str = "org.test.synth";
pub const FX_ID: &str = "org.test.fx";
pub const SYNTH_TYPE: &str = "clap.org.test.synth";
pub const FX_TYPE: &str = "clap.org.test.fx";
pub const BIG_ID: &str = "org.test.big";
pub const BIG_TYPE: &str = "clap.org.test.big";

pub fn big_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: BIG_ID.to_string(),
    }
}

pub fn param(
    id: u32,
    name: &str,
    module: &str,
    min: f64,
    max: f64,
    stepped: bool,
) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: name.to_string(),
        module: module.to_string(),
        min,
        max,
        default: min,
        stepped,
        steps: 0,
    }
}

pub fn synth_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: SYNTH_ID.to_string(),
    }
}

pub fn fx_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: FX_ID.to_string(),
    }
}

/// A synth with 12 params (ids 10..=21, modules `Osc`, `Filter`, `Env`), `p17` stepped (0..=3)
/// and `p18` a 0/1 toggle.
pub fn synth_layout() -> PluginLayout {
    let mut params = Vec::new();
    for i in 0..4u32 {
        params.push(param(
            10 + i,
            &format!("Osc {}", i + 1),
            "Osc",
            0.0,
            1.0,
            false,
        ));
    }
    for i in 0..4u32 {
        params.push(param(
            14 + i,
            &format!("Filter {}", i + 1),
            "Filter",
            20.0,
            20_000.0,
            false,
        ));
    }
    params[7] = param(17, "Filter Mode", "Filter", 0.0, 3.0, true);
    params.push(param(18, "Sync", "Env", 0.0, 1.0, true));
    for i in 0..3u32 {
        params.push(param(
            19 + i,
            &format!("Env {}", i + 1),
            "Env",
            0.0,
            10.0,
            false,
        ));
    }
    PluginLayout {
        name: "Test Synth".into(),
        vendor: "Atomik Test".into(),
        version: "1.2.0".into(),
        kind_hint: PluginKindHint::Instrument,
        main_in: None,
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: true,
        notes_out: false,
        params,
    }
}

/// A stereo effect with a sidechain and three params (ids 1, 2, 3).
pub fn fx_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Fx".into(),
        vendor: "Atomik Test".into(),
        version: "0.9".into(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: true,
        notes_in: false,
        notes_out: false,
        params: vec![
            param(1, "Gain", "", -24.0, 24.0, false),
            param(2, "Mute", "", 0.0, 1.0, true),
            param(3, "Latency", "", 0.0, 512.0, true),
        ],
    }
}

/// A plugin with `n` params (ids 0..n, modules `Group 0..`) for the large-table tests.
pub fn big_layout(n: u32) -> PluginLayout {
    PluginLayout {
        name: "Big".into(),
        vendor: "Atomik Test".into(),
        version: "1".into(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: false,
        notes_out: false,
        params: (0..n)
            .map(|i| {
                param(
                    i,
                    &format!("Param {i}"),
                    &format!("Group {}", i / 64),
                    0.0,
                    1.0,
                    i % 5 == 0,
                )
            })
            .collect(),
    }
}

/// The note effect the catalog lists as "Test Arp" (usable, so the browser can add it).
pub fn arp_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: "org.test.arp".to_string(),
    }
}

/// The layout of [`arp_ref`] (the effect fixture renamed, a note effect).
pub fn arp_layout() -> PluginLayout {
    let mut l = fx_layout();
    l.name = "Test Arp".into();
    l.vendor = "Other Co".into();
    l.kind_hint = PluginKindHint::NoteEffect;
    l
}

/// Registers the fixture plugins: the synth, the effect, the large one and every other catalog
/// entry the browser can add (the app registers a plugin's type before its `Add` is applied).
pub fn register_test_plugins(r: &mut Registry) {
    for (p, l) in [
        (synth_ref(), synth_layout()),
        (fx_ref(), fx_layout()),
        (big_ref(), big_layout(2048)),
        (arp_ref(), arp_layout()),
    ] {
        let d = descriptor_for(&p, &l).expect("descriptor");
        r.register_plugin(p.type_id(), d).expect("register plugin");
    }
}

pub fn entry(p: PluginRef, layout: PluginLayout, status: PluginEntryStatus) -> PluginEntryInfo {
    PluginEntryInfo {
        plugin: p,
        name: layout.name.clone(),
        vendor: layout.vendor.clone(),
        version: layout.version.clone(),
        kind: layout.kind_hint,
        status,
        path: Some("C:/Plugins/test.clap".into()),
        layout,
    }
}

/// The catalog of the fixtures: the synth, the effect, an unsupported surround plugin, a
/// disabled one and a failed one.
pub fn catalog() -> Vec<PluginEntryInfo> {
    let other = |id: &str, name: &str, vendor: &str, kind, status| {
        let mut l = fx_layout();
        l.name = name.into();
        l.vendor = vendor.into();
        l.kind_hint = kind;
        entry(
            PluginRef {
                format: PluginFormat::Clap,
                id: id.into(),
            },
            l,
            status,
        )
    };
    vec![
        entry(synth_ref(), synth_layout(), PluginEntryStatus::Ok),
        entry(fx_ref(), fx_layout(), PluginEntryStatus::Ok),
        entry(arp_ref(), arp_layout(), PluginEntryStatus::Ok),
        other(
            "org.test.surround",
            "Surround",
            "Other Co",
            PluginKindHint::Effect,
            PluginEntryStatus::Unsupported("surround only".into()),
        ),
        other(
            "org.test.off",
            "Switched Off",
            "Other Co",
            PluginKindHint::Effect,
            PluginEntryStatus::Disabled,
        ),
        other(
            "org.test.dup",
            "Duplicate",
            "Other Co",
            PluginKindHint::Effect,
            PluginEntryStatus::Failed("duplicate id".into()),
        ),
    ]
}

pub fn plugin_add(index: usize, p: &PluginRef, l: PluginLayout, k: &str) -> Command {
    Command::Plugin(PluginCommand::Add {
        index,
        container: None,
        plugin: p.clone(),
        layout: l,
        path_hint: Some("C:/Plugins/test.clap".into()),
        key: Some(key(k)),
        auto_split: false,
    })
}

/// An empty rack with the synth `synth` and the effect `fx` (plugin devices), no pins set.
pub fn plugin_rack() -> Document {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = Document::new(doc);
    let reg = registry();
    d.apply(plugin_add(0, &synth_ref(), synth_layout(), "synth"), &reg)
        .expect("add synth");
    d.apply(plugin_add(1, &fx_ref(), fx_layout(), "fx"), &reg)
        .expect("add fx");
    d
}

/// `plugin_rack` plus a container `layer` holding a plugin effect `inner` (and a synth `s2`).
pub fn plugin_in_container() -> Document {
    let mut d = plugin_rack();
    let reg = registry();
    d.apply(
        Command::AddDevice {
            index: 2,
            type_id: "atomik.container".into(),
            key: Some(key("layer")),
        },
        &reg,
    )
    .expect("container");
    for (k, p, l, split) in [
        ("inner", fx_ref(), fx_layout(), false),
        ("s2", synth_ref(), synth_layout(), true),
    ] {
        d.apply(
            Command::Plugin(PluginCommand::Add {
                index: 0,
                container: Some(key("layer")),
                plugin: p,
                layout: l,
                path_hint: None,
                key: Some(key(k)),
                auto_split: split,
            }),
            &reg,
        )
        .expect("inner plugin");
    }
    d
}

/// An empty rack with one plugin device `big` of 2 048 params.
pub fn big_rack() -> Document {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = Document::new(doc);
    d.apply(
        plugin_add(0, &big_ref(), big_layout(2048), "big"),
        &registry(),
    )
    .expect("add big");
    d
}

pub fn set_pins(d: &mut Document, device: &str, pins: &[u32]) {
    d.apply(
        Command::Plugin(PluginCommand::SetPinned {
            device: key(device),
            pinned: pins.to_vec(),
        }),
        &registry(),
    )
    .expect("pins");
}
