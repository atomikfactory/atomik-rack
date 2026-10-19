//! VST3 next to CLAP in the UI: the format badge on the panel
//! and in the browser, the browser filter that also matches the format, the settings list, a
//! VST3 device added from the browser and its stepped params (a step index as the value).

mod support;

use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    descriptor_for,
};
use atomik_model::{Command, Document, PluginCommand};
use atomik_ui::browser::BrowserRow;
use atomik_ui::plugin_data::{
    PluginEntryInfo, PluginEntryStatus, PluginSettingsData, SettingsPluginRow,
};
use atomik_ui::{PluginSettingsView, RackView, Theme};
use support::container::*;
use support::plugin::*;
use support::*;

const VST3_ID: &str = "a70f1c014a5b4c018d2e1f0000000001";

fn vst3_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Vst3,
        id: VST3_ID.to_string(),
    }
}

/// A VST3 effect: normalised params, one stepped (steps 4: host values 0..=4).
fn vst3_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Vst Gain".into(),
        vendor: "Atomik Test".into(),
        version: "1.0".into(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: true,
        notes_in: false,
        notes_out: false,
        params: vec![
            PluginParamInfo {
                id: 7,
                name: "Gain".into(),
                module: "".into(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                stepped: false,
                steps: 0,
            },
            PluginParamInfo {
                id: 8,
                name: "Mode".into(),
                module: "".into(),
                min: 0.0,
                max: 4.0,
                default: 0.0,
                stepped: true,
                steps: 4,
            },
        ],
    }
}

fn registry_with_vst3() -> atomik_device_sdk::Registry {
    let mut r = registry();
    let d = descriptor_for(&vst3_ref(), &vst3_layout()).expect("descriptor");
    r.register_plugin(vst3_ref().type_id(), d)
        .expect("register");
    r
}

fn catalog_with_vst3() -> Vec<PluginEntryInfo> {
    let mut c = catalog();
    let mut e = entry(vst3_ref(), vst3_layout(), PluginEntryStatus::Ok);
    e.path = Some("C:/Plugins/test.vst3".into());
    c.push(e);
    c
}

fn setup() -> (Document, RackView) {
    let mut d = plugin_rack();
    let reg = registry_with_vst3();
    d.apply(
        Command::Plugin(PluginCommand::Add {
            index: usize::MAX,
            container: None,
            plugin: vst3_ref(),
            layout: vst3_layout(),
            path_hint: None,
            key: Some(key("vfx")),
            auto_split: false,
        }),
        &reg,
    )
    .expect("a VST3 device is added like a CLAP one");
    let mut v = view(d.doc(), &reg);
    v.set_plugin_catalog(catalog_with_vst3());
    v.set_browser_visible(true);
    (d, v)
}

fn plugin_rows(v: &RackView) -> Vec<usize> {
    v.browser_list(&registry_with_vst3())
        .into_iter()
        .filter_map(|r| match r {
            BrowserRow::Plugin(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn the_panel_of_a_vst3_device_carries_the_vst3_badge_and_a_clap_one_the_clap_badge() {
    let (d, mut v) = setup();
    let texts: Vec<String> = paint_rack(&mut v, d.doc())
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(texts.iter().any(|t| t == "VST3"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "CLAP"), "{texts:?}");
}

#[test]
fn the_browser_lists_vst3_plugins_with_the_badge_and_filters_on_the_format() {
    let (d, mut v) = setup();
    // Both formats are offered, grouped by kind (instruments, effects, note effects).
    assert_eq!(plugin_rows(&v), [0, 1, 6, 2]);
    type_keys(&mut v, d.doc(), "vst3");
    assert_eq!(plugin_rows(&v), [6], "the format name filters");
    let texts: Vec<String> = paint_rack(&mut v, d.doc())
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(texts.iter().any(|t| t == "Test Vst Gain (Atomik Test)"));
    assert!(texts.iter().any(|t| t == "VST3"));
}

#[test]
fn the_clap_filter_hides_the_vst3_entries() {
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "CLAP");
    assert_eq!(plugin_rows(&v), [0, 1, 2]);
}

#[test]
fn the_settings_list_names_the_format_of_a_vst3_plugin() {
    let data = PluginSettingsData {
        enabled: true,
        timeout_secs: 30,
        plugins: vec![
            SettingsPluginRow {
                plugin: vst3_ref(),
                name: "Test Vst Gain".into(),
                vendor: "Atomik Test".into(),
                version: "1.0".into(),
                status: PluginEntryStatus::Ok,
            },
            SettingsPluginRow {
                plugin: synth_ref(),
                name: "Test Synth".into(),
                vendor: "Atomik Test".into(),
                version: "1.2.0".into(),
                status: PluginEntryStatus::Ok,
            },
        ],
        ..PluginSettingsData::default()
    };
    let mut view = PluginSettingsView::new(Theme::dark());
    view.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 760.0, 560.0));
    view.set_data(data);
    let mut dl = atomik_ui::DisplayList::new();
    view.paint(&mut dl);
    let texts = dl.texts();
    assert!(texts.contains(&"VST3"), "{texts:?}");
    assert!(texts.contains(&"CLAP"), "{texts:?}");
}

#[test]
fn a_stepped_vst3_param_is_a_discrete_param_driven_by_its_step_index() {
    let reg = registry_with_vst3();
    let desc = reg.get(&vst3_ref().type_id()).unwrap().descriptor;
    let mode = desc.params.iter().find(|p| p.key == "p8").unwrap();
    assert!(mode.discrete, "stepped means discrete");
    assert!(
        (mode.max - 4.0).abs() < f32::EPSILON,
        "max is the step count"
    );
    let (mut d, _) = setup();
    d.apply(
        Command::SetParam {
            key: key("vfx"),
            param: "p8".into(),
            value: 3.0,
            merge: None,
        },
        &reg,
    )
    .expect("a step index is a valid value");
    let doc = d.doc();
    let dev = doc.device(&key("vfx")).unwrap();
    assert!((dev.params["p8"] - 3.0).abs() < f32::EPSILON);
}
