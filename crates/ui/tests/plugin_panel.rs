//! The plugin device's rack panel: layout, pins, display-list
//! goldens of both faces and of the MISSING and FAILED states, every button (POWER, BYPASS,
//! AUDIO THR, OPEN GUI, PARAMS, TO CH, RELOAD, RELINK), the pinned knobs as `SetParam`s, pin
//! edits, the same gestures inside a container (path keys), and no platform code.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plugin_panel`.

mod support;

use atomik_model::{Command, ConsoleCommand, Document, PluginCommand, RackDoc};
use atomik_ui::autolayout::PanelSpec;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::plugin_data::{PluginDeviceStatus, STATE_FAILED};
use atomik_ui::plugin_panel::{
    MAX_FEATURED, PLUGIN_UNITS, PluginButton, featured_pins, plugin_layout,
};
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use proptest::prelude::*;
use support::container::*;
use support::plugin::*;
use support::*;

/// The rack with console devices hidden (this file is not about them).
fn view(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    plain_view(doc, reg)
}

fn setup() -> (Document, RackView) {
    let d = plugin_rack();
    let v = view(d.doc(), &registry());
    (d, v)
}

fn with_editor() -> PluginDeviceStatus {
    PluginDeviceStatus {
        has_editor: true,
        ..PluginDeviceStatus::default()
    }
}

fn texts_of(v: &mut RackView, doc: &atomik_model::RackDoc) -> Vec<String> {
    paint_rack(v, doc)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn spec_of(type_id: &str, d: &Document, device: &str) -> PanelSpec {
    let reg = registry();
    let desc = reg.get(type_id).unwrap().descriptor;
    let doc = d.doc();
    plugin_layout(
        desc,
        doc.device(&key(device)).unwrap().plugin.as_ref().unwrap(),
    )
}

// ------------------------------------------------------------------ layout and pins

#[test]
fn the_panel_is_two_units_with_the_first_eight_params_when_nothing_is_pinned() {
    let d = plugin_rack();
    let s = spec_of(SYNTH_TYPE, &d, "synth");
    assert_eq!(s.height_units, PLUGIN_UNITS);
    assert_eq!(s.height_units, 2);
    let panel = s.plugin.expect("plugin panel");
    let reg = registry();
    let desc = reg.get(SYNTH_TYPE).unwrap().descriptor;
    let keys: Vec<&str> = s.front.iter().map(|pc| desc.params[pc.param].key).collect();
    assert_eq!(
        keys,
        ["p10", "p11", "p12", "p13", "p14", "p15", "p16", "p17"],
        "the first eight visible params, never bypass"
    );
    for pc in &s.front {
        assert!(
            pc.rect.x0 >= panel.info.x1,
            "knobs sit right of the left block"
        );
        assert!(pc.rect.x1 <= 736.0 && pc.rect.y1 <= s.height());
    }
    let buttons = [
        PluginButton::Power,
        PluginButton::Bypass,
        PluginButton::AudioThread,
        PluginButton::OpenGui,
        PluginButton::Params,
        PluginButton::ToChannel,
        PluginButton::Reload,
        PluginButton::Relink,
    ];
    for (i, a) in buttons.iter().enumerate() {
        let r = panel.button(*a);
        assert!(
            r.width() > 20.0 && r.y0 >= 0.0 && r.y1 <= s.height(),
            "{a:?}"
        );
        assert!(
            !r.overlaps(panel.info),
            "{a:?} stays out of the info window"
        );
        for b in &buttons[i + 1..] {
            assert!(!r.overlaps(panel.button(*b)), "{a:?} and {b:?} overlap");
        }
        for pc in &s.front {
            assert!(!r.overlaps(pc.rect), "{a:?} overlaps a knob");
        }
    }
}

#[test]
fn pins_choose_and_order_the_knobs_up_to_sixteen_in_two_rows() {
    let mut d = plugin_rack();
    set_pins(&mut d, "synth", &[21, 10, 18]);
    let s = spec_of(SYNTH_TYPE, &d, "synth");
    let reg = registry();
    let desc = reg.get(SYNTH_TYPE).unwrap().descriptor;
    let keys: Vec<&str> = s.front.iter().map(|pc| desc.params[pc.param].key).collect();
    assert_eq!(keys, ["p21", "p10", "p18"]);
    // Sixteen pins fill two rows of eight; a toggle (p18) is a toggle.
    let all: Vec<u32> = (10..=21).collect();
    set_pins(&mut d, "synth", &all);
    let s = spec_of(SYNTH_TYPE, &d, "synth");
    assert_eq!(s.front.len(), 12);
    assert_eq!(s.front[0].rect.y0, s.front[1].rect.y0);
    assert!(
        s.front[8].rect.y0 > s.front[0].rect.y0,
        "the ninth knob starts the second row"
    );
    assert!(s.front[8].rect.y1 <= s.height());
    assert_eq!(MAX_FEATURED, 16);
}

#[test]
fn a_pin_the_plugin_no_longer_has_is_skipped_not_an_error() {
    let mut d = plugin_rack();
    set_pins(&mut d, "fx", &[2, 1]);
    let doc = d.doc();
    let pins = featured_pins(doc.device(&key("fx")).unwrap().plugin.as_ref().unwrap());
    assert_eq!(pins, [2, 1]);
    // Defaults: the first eight rows, or fewer when the plugin has fewer.
    let d2 = plugin_rack();
    let pins = featured_pins(
        d2.doc()
            .device(&key("fx"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap(),
    );
    assert_eq!(pins, [1, 2, 3]);
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_matches_golden() {
    let (d, mut v) = setup();
    v.set_plugin_status(&key("synth"), with_editor());
    v.set_device_meter_values(&key("synth"), &[0.2, 0.3, 0.0, 128.0, 0.0]);
    check(
        "plugin_panel_front_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_back_matches_golden() {
    let (d, mut v) = setup();
    v.set_flip(Flip::Back);
    check(
        "plugin_panel_back_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn a_missing_plugin_matches_golden_and_says_why() {
    let (d, mut v) = setup();
    v.set_plugin_status(
        &key("synth"),
        PluginDeviceStatus {
            missing: Some("not installed".into()),
            ..PluginDeviceStatus::default()
        },
    );
    let texts = texts_of(&mut v, d.doc());
    for t in ["MISSING", "not installed", "RELOAD", "RELINK"] {
        assert!(
            texts.iter().any(|x| x.contains(t)),
            "missing {t:?}: {texts:?}"
        );
    }
    check(
        "plugin_panel_missing_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn a_failed_plugin_matches_golden_and_says_why() {
    let (d, mut v) = setup();
    v.set_plugin_status(
        &key("synth"),
        PluginDeviceStatus {
            failed: Some("state restore failed".into()),
            ..with_editor()
        },
    );
    v.set_device_meter_values(&key("synth"), &[0.0, 0.0, STATE_FAILED, 0.0, 0.0]);
    let texts = texts_of(&mut v, d.doc());
    for t in ["FAILED", "state restore failed", "RELOAD"] {
        assert!(
            texts.iter().any(|x| x.contains(t)),
            "missing {t:?}: {texts:?}"
        );
    }
    check(
        "plugin_panel_failed_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_panel_shows_name_vendor_format_and_the_status_strip() {
    let (d, mut v) = setup();
    v.set_plugin_status(&key("synth"), with_editor());
    v.set_device_meter_values(&key("synth"), &[0.0, 0.0, 1.0, 256.0, 3.0]);
    let texts = texts_of(&mut v, d.doc());
    for t in [
        "Test Synth",
        "Atomik Test",
        "CLAP",
        "POWER",
        "BYPASS",
        "OPEN GUI",
        "PARAMS",
        "TO CH",
        "AUDIO THR",
        "SLEEPING",
        "256 fr",
        "3 dropped",
        "Osc 1",
    ] {
        assert!(
            texts.iter().any(|x| x.contains(t)),
            "missing {t:?}: {texts:?}"
        );
    }
}

// ------------------------------------------------------------------ buttons

fn press_button(v: &mut RackView, d: &Document, device: &str, b: PluginButton) -> Vec<UiAction> {
    // A status set by the app takes effect at the next sync (every frame in the app).
    v.sync(d.doc(), &registry());
    let p = v
        .plugin_button_center(&key(device), b)
        .unwrap_or_else(|| panic!("{b:?} is not on screen"));
    click(v, d.doc(), p)
}

#[test]
fn power_bypass_and_audio_thread_are_one_command_each() {
    let (d, mut v) = setup();
    let acts = press_button(&mut v, &d, "synth", PluginButton::Power);
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPower {
            device: key("synth"),
            powered: false
        })]
    );
    let acts = press_button(&mut v, &d, "synth", PluginButton::Bypass);
    assert_eq!(
        commands(&acts),
        vec![Command::SetParam {
            key: key("synth"),
            param: "bypass".into(),
            value: 1.0,
            merge: None
        }]
    );
    let acts = press_button(&mut v, &d, "synth", PluginButton::AudioThread);
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetRunMode {
            device: key("synth"),
            run_on_audio_thread: true
        })]
    );
}

#[test]
fn the_buttons_follow_the_document_state_after_a_sync() {
    let (mut d, mut v) = setup();
    for c in [
        Command::Plugin(PluginCommand::SetPower {
            device: key("synth"),
            powered: false,
        }),
        Command::SetParam {
            key: key("synth"),
            param: "bypass".into(),
            value: 1.0,
            merge: None,
        },
        Command::Plugin(PluginCommand::SetRunMode {
            device: key("synth"),
            run_on_audio_thread: true,
        }),
    ] {
        d.apply(c, &registry()).unwrap();
    }
    v.sync(d.doc(), &registry());
    // Now the same buttons switch back.
    let acts = press_button(&mut v, &d, "synth", PluginButton::Power);
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPower {
            device: key("synth"),
            powered: true
        })]
    );
    let acts = press_button(&mut v, &d, "synth", PluginButton::Bypass);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::SetParam { value, .. }] if value.abs() < 1e-6
    ));
    let acts = press_button(&mut v, &d, "synth", PluginButton::AudioThread);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Plugin(PluginCommand::SetRunMode {
            run_on_audio_thread: false,
            ..
        })]
    ));
}

#[test]
fn open_gui_exists_only_when_the_plugin_has_an_editor() {
    let (d, mut v) = setup();
    assert!(
        v.plugin_button_center(&key("synth"), PluginButton::OpenGui)
            .is_none(),
        "no editor known: no button"
    );
    v.set_plugin_status(&key("synth"), with_editor());
    v.sync(d.doc(), &registry());
    let acts = press_button(&mut v, &d, "synth", PluginButton::OpenGui);
    assert!(acts.contains(&UiAction::OpenPluginGui {
        device: key("synth")
    }));
    // A missing plugin has no editor even if the flag is stale.
    v.set_plugin_status(
        &key("synth"),
        PluginDeviceStatus {
            has_editor: true,
            missing: Some("gone".into()),
            ..PluginDeviceStatus::default()
        },
    );
    v.sync(d.doc(), &registry());
    assert!(
        v.plugin_button_center(&key("synth"), PluginButton::OpenGui)
            .is_none()
    );
}

#[test]
fn params_and_reload_emit_their_actions() {
    let (d, mut v) = setup();
    let acts = press_button(&mut v, &d, "fx", PluginButton::Params);
    assert!(acts.contains(&UiAction::OpenPluginParams { device: key("fx") }));
    let acts = press_button(&mut v, &d, "fx", PluginButton::Reload);
    assert!(acts.contains(&UiAction::ReloadPlugin { device: key("fx") }));
    // RELOAD is dim while the instance is being created: the press does nothing.
    v.set_plugin_status(
        &key("fx"),
        PluginDeviceStatus {
            loading: true,
            ..PluginDeviceStatus::default()
        },
    );
    let acts = press_button(&mut v, &d, "fx", PluginButton::Reload);
    assert!(!acts.contains(&UiAction::ReloadPlugin { device: key("fx") }));
    assert!(
        texts_of(&mut v, d.doc())
            .iter()
            .any(|t| t.contains("LOADING"))
    );
}

#[test]
fn to_ch_is_for_instruments_dim_while_the_channel_exists_and_adds_it_otherwise() {
    let (d, mut v) = setup();
    assert!(
        v.plugin_button_center(&key("fx"), PluginButton::ToChannel)
            .is_none(),
        "an effect has no TO CH"
    );
    // The synth already has its auto channel from `PluginCommand::Add`: the button is dim.
    assert!(d.doc().console.channel_of(&key("synth")).is_some());
    let acts = press_button(&mut v, &d, "synth", PluginButton::ToChannel);
    assert!(commands(&acts).is_empty(), "{acts:?}");
    // Without the channel the button adds it.
    let mut doc: RackDoc = d.doc().clone();
    doc.console.channels.clear();
    v.sync(&doc, &registry());
    let p = v
        .plugin_button_center(&key("synth"), PluginButton::ToChannel)
        .unwrap();
    let acts = click(&mut v, &doc, p);
    assert_eq!(
        commands(&acts),
        vec![Command::Console(ConsoleCommand::AddChannel {
            source: key("synth")
        })]
    );
}

#[test]
fn relink_only_exists_for_a_stand_in_and_offers_compatible_installed_plugins() {
    let (d, mut v) = setup();
    v.set_plugin_catalog(catalog());
    assert!(
        v.plugin_button_center(&key("fx"), PluginButton::Relink)
            .is_none(),
        "an installed plugin is not relinked"
    );
    v.set_plugin_status(
        &key("fx"),
        PluginDeviceStatus {
            missing: Some("not installed".into()),
            ..PluginDeviceStatus::default()
        },
    );
    v.sync(d.doc(), &registry());
    let acts = press_button(&mut v, &d, "fx", PluginButton::Relink);
    assert!(commands(&acts).is_empty());
    let rows = v.plugin_menu_rows();
    let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
    // Effects of the same kind that are installed and usable: the fx itself and nothing else
    // (the surround, disabled and failed ones are not offered; the synth is not an effect).
    assert_eq!(labels, ["Test Fx (Atomik Test)"], "{labels:?}");
    let acts = click(&mut v, d.doc(), rows[0].1);
    match commands(&acts).as_slice() {
        [
            Command::Plugin(PluginCommand::Relink {
                device,
                plugin,
                layout,
                path_hint,
            }),
        ] => {
            assert_eq!(device, &key("fx"));
            assert_eq!(plugin, &fx_ref());
            assert_eq!(layout, &fx_layout());
            assert_eq!(path_hint.as_deref(), Some("C:/Plugins/test.clap"));
        }
        other => panic!("{other:?}"),
    }
    assert!(v.plugin_menu_rows().is_empty(), "the menu closed");
}

#[test]
fn the_open_relink_menu_is_painted_and_follows_the_pointer() {
    let (d, mut v) = setup();
    v.set_plugin_catalog(catalog());
    v.set_plugin_status(
        &key("fx"),
        PluginDeviceStatus {
            missing: Some("not installed".into()),
            ..PluginDeviceStatus::default()
        },
    );
    press_button(&mut v, &d, "fx", PluginButton::Relink);
    let rows = v.plugin_menu_rows();
    assert_eq!(rows.len(), 1);
    // Closed menus paint nothing of it; an open one paints its labels and a hover highlight.
    let plain = paint_rack(&mut v, d.doc());
    assert!(
        plain.texts().iter().any(|t| t.contains("Test Fx")),
        "the entry is painted: {:?}",
        plain.texts()
    );
    let before = plain.to_json();
    v.handle(
        &UiEvent::PointerMove {
            pos: rows[0].1,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &registry(),
    );
    let hovered = paint_rack(&mut v, d.doc()).to_json();
    assert_ne!(before, hovered, "the row under the pointer is highlighted");
    // Moving off the menu takes the highlight away again.
    v.handle(
        &UiEvent::PointerMove {
            pos: Point::new(1.0, 1.0),
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &registry(),
    );
    assert_eq!(paint_rack(&mut v, d.doc()).to_json(), before);
}

#[test]
fn a_press_elsewhere_closes_the_relink_menu_without_a_command() {
    let (d, mut v) = setup();
    v.set_plugin_catalog(catalog());
    v.set_plugin_status(
        &key("fx"),
        PluginDeviceStatus {
            missing: Some("x".into()),
            ..PluginDeviceStatus::default()
        },
    );
    let _ = press_button(&mut v, &d, "fx", PluginButton::Relink);
    assert!(!v.plugin_menu_rows().is_empty());
    let acts = click(&mut v, d.doc(), Point::new(5.0, 5.0));
    assert!(commands(&acts).is_empty());
    assert!(v.plugin_menu_rows().is_empty());
}

// ------------------------------------------------------------------ knobs and pins

#[test]
fn a_pinned_knob_drag_is_a_set_param_of_the_plugins_own_key() {
    let (d, mut v) = setup();
    let c = v
        .control_center(&key("synth"), "p10")
        .expect("p10 is a default pin");
    let mut out = v.handle(&down(c), d.doc(), &registry());
    out.extend(v.handle(&mv(Point::new(c.x, c.y - 30.0)), d.doc(), &registry()));
    out.extend(v.handle(&up(Point::new(c.x, c.y - 30.0)), d.doc(), &registry()));
    let cmds = commands(&out);
    assert!(!cmds.is_empty());
    for cmd in &cmds {
        match cmd {
            Command::SetParam {
                key: k,
                param,
                value,
                ..
            } => {
                assert_eq!(k, &key("synth"));
                assert_eq!(param, "p10");
                assert!((0.0..=1.0).contains(value));
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(out.contains(&UiAction::EndMerge));
}

#[test]
fn a_shift_right_click_on_a_knob_unpins_it_and_the_panel_follows() {
    let mut d = plugin_rack();
    set_pins(&mut d, "synth", &[10, 11, 12]);
    let mut v = view(d.doc(), &registry());
    let c = v.control_center(&key("synth"), "p11").unwrap();
    let acts = press_with(
        &mut v,
        d.doc(),
        c,
        PointerButton::Secondary,
        1,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPinned {
            device: key("synth"),
            pinned: vec![10, 12]
        })]
    );
    d.apply(commands(&acts).remove(0), &registry()).unwrap();
    v.sync(d.doc(), &registry());
    assert!(
        v.control_center(&key("synth"), "p11").is_none(),
        "its knob is gone"
    );
    assert!(v.control_center(&key("synth"), "p12").is_some());
}

#[test]
fn changing_the_pins_rebuilds_the_panel_without_a_structure_change() {
    let mut d = plugin_rack();
    let mut v = view(d.doc(), &registry());
    assert!(v.control_center(&key("synth"), "p21").is_none());
    set_pins(&mut d, "synth", &[21]);
    v.sync(d.doc(), &registry());
    assert!(v.control_center(&key("synth"), "p21").is_some());
    assert!(v.control_center(&key("synth"), "p10").is_none());
}

#[test]
fn a_stepped_pin_is_a_toggle_or_stepper_widget_not_a_knob_drag() {
    let mut d = plugin_rack();
    set_pins(&mut d, "synth", &[18]);
    let mut v = view(d.doc(), &registry());
    let c = v.control_center(&key("synth"), "p18").unwrap();
    let acts = click(&mut v, d.doc(), c);
    let cmds = commands(&acts);
    assert!(
        matches!(cmds.as_slice(), [Command::SetParam { param, value, .. }] if param == "p18" && (*value - 1.0).abs() < 1e-6),
        "{cmds:?}"
    );
}

// ------------------------------------------------------------------ inside a container

#[test]
fn inside_a_container_the_panel_emits_path_keys() {
    let d = plugin_in_container();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_plugin_status(&key("layer/s2"), with_editor());
    let acts = press_button(&mut v, &d, "inner", PluginButton::Power);
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPower {
            device: key("layer/inner"),
            powered: false
        })]
    );
    let acts = press_button(&mut v, &d, "s2", PluginButton::OpenGui);
    assert!(acts.contains(&UiAction::OpenPluginGui {
        device: key("layer/s2")
    }));
    let acts = press_button(&mut v, &d, "inner", PluginButton::Params);
    assert!(acts.contains(&UiAction::OpenPluginParams {
        device: key("layer/inner")
    }));
    let acts = press_button(&mut v, &d, "inner", PluginButton::Bypass);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::SetParam { key: k, param, .. }] if k == &key("layer/inner") && param == "bypass"
    ));
    // TO CH is refused inside a scope (no command), like the container's.
    let acts = press_button(&mut v, &d, "s2", PluginButton::ToChannel);
    assert!(commands(&acts).is_empty());
}

#[test]
fn a_param_a_macro_drives_is_locked_on_the_plugin_panel_too() {
    use atomik_model::{ContainerCommand, MacroTarget};
    let mut d = plugin_in_container();
    d.apply(
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: MacroTarget {
                device: key("inner"),
                param: "p1".into(),
                min: -24.0,
                max: 24.0,
            },
            merge: None,
        }),
        &registry(),
    )
    .unwrap();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let c = v.control_center(&key("layer/inner"), "p1").unwrap();
    let mut out = v.handle(&down(c), d.doc(), &registry());
    out.extend(v.handle(&mv(Point::new(c.x, c.y - 30.0)), d.doc(), &registry()));
    out.extend(v.handle(&up(Point::new(c.x, c.y - 30.0)), d.doc(), &registry()));
    assert!(commands(&out).is_empty(), "driven knob is locked: {out:?}");
}

// ------------------------------------------------------------------ misc

#[test]
fn an_unknown_event_sequence_never_panics_on_a_plugin_rack() {
    let (d, mut v) = setup();
    for p in [
        Point::new(300.0, 100.0),
        Point::new(300.0, 250.0),
        Point::new(500.0, 400.0),
    ] {
        let _ = v.handle(
            &UiEvent::PointerMove {
                pos: p,
                modifiers: Modifiers::NONE,
            },
            d.doc(),
            &registry(),
        );
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    assert!(!dl.is_empty());
}

// found as a rare failure of the property test below in a full run: the browser
// lists "Test Arp" as a usable plugin, so a gesture sequence can emit its `Plugin(Add)`; the
// fixture registry must know every usable catalog entry, as the app registers a plugin's type
// before the command is applied.
#[test]
fn every_usable_catalog_entry_can_be_added_with_the_fixture_registry() {
    let reg = registry();
    for e in catalog()
        .into_iter()
        .filter(|e| matches!(e.status, atomik_ui::plugin_data::PluginEntryStatus::Ok))
    {
        let mut d = plugin_rack();
        let cmd = plugin_add(0, &e.plugin, e.layout.clone(), "added");
        d.apply(cmd, &reg)
            .unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert!(d.doc().device(&key("added")).is_some(), "{}", e.name);
    }
}

// ------------------------------------------------------------------ property test

#[derive(Debug, Clone)]
enum Gesture {
    Down(f64, f64, u8, u8, bool),
    Move(f64, f64, bool),
    Up(f64, f64),
    Wheel(f64, f64, f64),
    Char(char),
    Key(u8),
    StatusMissing(bool),
    StatusEditor(bool),
}

fn gesture() -> impl Strategy<Value = Gesture> {
    prop_oneof![
        4 => (0.0..1000.0f64, 0.0..700.0f64, 1u8..=2, 0u8..3, any::<bool>())
            .prop_map(|(x, y, c, b, s)| Gesture::Down(x, y, c, b, s)),
        4 => (0.0..1000.0f64, 0.0..700.0f64, any::<bool>()).prop_map(|(x, y, s)| Gesture::Move(x, y, s)),
        4 => (0.0..1000.0f64, 0.0..700.0f64).prop_map(|(x, y)| Gesture::Up(x, y)),
        1 => (0.0..1000.0f64, 0.0..700.0f64, -3.0..3.0f64).prop_map(|(x, y, d)| Gesture::Wheel(x, y, d)),
        1 => proptest::char::range('a', 'z').prop_map(Gesture::Char),
        1 => (0u8..6).prop_map(Gesture::Key),
        1 => any::<bool>().prop_map(Gesture::StatusMissing),
        1 => any::<bool>().prop_map(Gesture::StatusEditor),
    ]
}

fn run_gestures(steps: &[Gesture], doc: Document, scope: &[&str]) {
    let reg = registry();
    let mut d = doc;
    let mut v = scoped_view(d.doc(), scope);
    v.set_plugin_catalog(catalog());
    v.set_browser_visible(true);
    let names: Vec<&str> = if scope.is_empty() {
        vec!["synth", "fx"]
    } else {
        vec!["layer/inner", "layer/s2"]
    };
    for s in steps {
        let modifiers = |shift: bool| Modifiers {
            shift,
            ..Modifiers::NONE
        };
        let event = match s {
            Gesture::Down(x, y, clicks, b, shift) => Some(UiEvent::PointerDown {
                pos: Point::new(*x, *y),
                button: [
                    PointerButton::Primary,
                    PointerButton::Secondary,
                    PointerButton::Middle,
                ][usize::from(*b)],
                modifiers: modifiers(*shift),
                clicks: *clicks,
            }),
            Gesture::Move(x, y, shift) => Some(UiEvent::PointerMove {
                pos: Point::new(*x, *y),
                modifiers: modifiers(*shift),
            }),
            Gesture::Up(x, y) => Some(UiEvent::PointerUp {
                pos: Point::new(*x, *y),
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            }),
            Gesture::Wheel(x, y, dy) => Some(UiEvent::Wheel {
                delta: Vec2::new(0.0, *dy),
                pos: Point::new(*x, *y),
                modifiers: Modifiers::NONE,
            }),
            Gesture::Char(c) => Some(key_event(atomik_ui::Key::Char(*c))),
            Gesture::Key(k) => Some(key_event(
                [
                    atomik_ui::Key::Escape,
                    atomik_ui::Key::Enter,
                    atomik_ui::Key::Down,
                    atomik_ui::Key::Up,
                    atomik_ui::Key::Backspace,
                    atomik_ui::Key::Tab,
                ][usize::from(*k)]
                .clone(),
            )),
            Gesture::StatusMissing(on) => {
                for n in &names {
                    v.set_plugin_status(
                        &key(n),
                        PluginDeviceStatus {
                            missing: on.then(|| "gone".to_string()),
                            has_editor: true,
                            ..PluginDeviceStatus::default()
                        },
                    );
                }
                None
            }
            Gesture::StatusEditor(on) => {
                for n in &names {
                    v.set_plugin_status(
                        &key(n),
                        PluginDeviceStatus {
                            has_editor: *on,
                            ..PluginDeviceStatus::default()
                        },
                    );
                }
                None
            }
        };
        let Some(event) = event else { continue };
        let actions = v.handle(&event, d.doc(), &reg);
        for a in actions {
            match a {
                UiAction::Command(c) => {
                    d.apply(c.clone(), &reg)
                        .unwrap_or_else(|e| panic!("the model refused {c:?}: {e}"));
                }
                UiAction::EndMerge => d.end_merge(),
                UiAction::Undo => {
                    let _ = d.undo(&reg);
                }
                UiAction::Redo => {
                    let _ = d.redo(&reg);
                }
                _ => {}
            }
        }
        v.sync(d.doc(), &reg);
        let _ = paint_rack(&mut v, d.doc());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn no_gesture_sequence_on_a_plugin_rack_makes_the_model_refuse_a_command(
        steps in proptest::collection::vec(gesture(), 1..40)
    ) {
        run_gestures(&steps, plugin_rack(), &[]);
        run_gestures(&steps, plugin_in_container(), &["layer"]);
    }
}
