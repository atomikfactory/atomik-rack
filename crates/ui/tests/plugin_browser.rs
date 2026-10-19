//! The browser's Plugins group: sub-groups, `name (vendor)` entries
//! with a format badge, the filter on name and vendor, unsupported and disabled plugins hidden
//! with a hint, the failed list, keyboard and mouse adding as `PluginCommand::Add` (lifted into
//! the container inside a scope), and a golden of the group.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plugin_browser`.

mod support;

use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, Document, PluginCommand, WorkflowCommand,
};
use atomik_ui::browser::BrowserRow;
use atomik_ui::plugin_data::{FailedFileInfo, PluginEntryStatus};
use atomik_ui::{Key, RackView, UiAction};
use support::container::*;
use support::plugin::*;
use support::*;

/// The rack with console devices hidden (this file is not about them).
fn view(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    plain_view(doc, reg)
}

fn setup() -> (Document, RackView) {
    let d = plugin_rack();
    let mut v = view(d.doc(), &registry());
    v.set_plugin_catalog(catalog());
    v.set_browser_visible(true);
    (d, v)
}

fn labels(v: &RackView) -> Vec<String> {
    v.browser_list(&registry())
        .into_iter()
        .skip_while(|r| !matches!(r, BrowserRow::Heading(_)))
        .map(|r| match r {
            BrowserRow::Device(d) => d.type_id.to_string(),
            BrowserRow::Heading(h) => format!("# {h}"),
            BrowserRow::Plugin(n) => format!("plugin {n}"),
            BrowserRow::Hint(h) => format!("hint {h}"),
            BrowserRow::Failed { title, reason } => format!("failed {title}: {reason}"),
        })
        .collect()
}

#[test]
fn the_plugins_group_follows_the_devices_with_instruments_effects_and_note_effects() {
    let (_, v) = setup();
    let rows = v.browser_list(&registry());
    let devices = rows
        .iter()
        .take_while(|r| matches!(r, BrowserRow::Device(_)))
        .count();
    assert!(devices >= 15, "the device rows come first, unchanged");
    assert_eq!(
        v.browser_rows(&registry()).len(),
        devices,
        "browser_rows still lists the device types only"
    );
    assert_eq!(
        labels(&v),
        [
            "# PLUGINS",
            "# Instruments",
            "plugin 0",
            "# Effects",
            "plugin 1",
            "# Note effects",
            "plugin 2",
            "hint 1 unsupported plugin hidden",
            "# Failed",
            "failed Duplicate (Other Co): duplicate id",
        ]
    );
}

#[test]
fn without_plugins_the_list_is_the_device_list() {
    let d = plugin_rack();
    let v = view(d.doc(), &registry());
    assert!(
        v.browser_list(&registry())
            .iter()
            .all(|r| matches!(r, BrowserRow::Device(_)))
    );
}

#[test]
fn the_filter_matches_name_and_vendor() {
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "other");
    let l = labels(&v);
    // Only the vendor "Other Co" entries: the arp, the unsupported one (hint) and the failed one.
    assert_eq!(
        l,
        [
            "# PLUGINS",
            "# Note effects",
            "plugin 2",
            "hint 1 unsupported plugin hidden",
            "# Failed",
            "failed Duplicate (Other Co): duplicate id",
        ]
    );
    for _ in 0..5 {
        let _ = send_key(&mut v, d.doc(), Key::Backspace);
    }
    type_keys(&mut v, d.doc(), "SYNTH");
    assert_eq!(
        labels(&v),
        ["# PLUGINS", "# Instruments", "plugin 0"],
        "a name filter hides the rest (the unsupported plugin is named Surround)"
    );
}

#[test]
fn a_query_that_matches_nothing_hides_the_whole_group() {
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "zzzz");
    assert!(v.browser_list(&registry()).is_empty());
}

#[test]
fn disabled_and_unsupported_plugins_are_never_offered() {
    let (_, v) = setup();
    let plugin_rows: Vec<usize> = v
        .browser_list(&registry())
        .into_iter()
        .filter_map(|r| match r {
            BrowserRow::Plugin(n) => Some(n),
            _ => None,
        })
        .collect();
    let cat = catalog();
    for n in &plugin_rows {
        assert_eq!(cat[*n].status, PluginEntryStatus::Ok);
    }
    assert_eq!(plugin_rows, [0, 1, 2]);
}

#[test]
fn failed_scan_files_are_listed_greyed_with_their_reason() {
    let (_, mut v) = setup();
    v.set_plugin_failed_files(vec![FailedFileInfo {
        path: "C:\\Plugins\\bad.clap".into(),
        reason: "crashed while scanning".into(),
    }]);
    let l = labels(&v);
    assert_eq!(l.last().unwrap(), "failed bad.clap: crashed while scanning");
    // A failed file alone still shows the group.
    let d = plugin_rack();
    let mut v2 = view(d.doc(), &registry());
    v2.set_plugin_failed_files(vec![FailedFileInfo {
        path: "/usr/lib/clap/bad.clap".into(),
        reason: "timed out".into(),
    }]);
    assert_eq!(
        labels(&v2),
        ["# PLUGINS", "# Failed", "failed bad.clap: timed out"]
    );
}

fn plugin_row_center(v: &RackView, n: usize) -> atomik_ui::kurbo::Point {
    let i = v
        .browser_list(&registry())
        .iter()
        .position(|r| *r == BrowserRow::Plugin(n))
        .unwrap();
    v.browser_row_center(i).unwrap()
}

#[test]
fn a_double_click_on_a_plugin_adds_it_below_the_selection() {
    let (d, mut v) = setup();
    // Select the synth (index 0) by clicking its plate.
    let plate = v.plate_center(&key("synth")).unwrap();
    let _ = click(&mut v, d.doc(), plate);
    let p = plugin_row_center(&v, 1);
    let acts = double_click(&mut v, d.doc(), p);
    // A top-level plugin effect is created through the creation chain, routed
    // from the selected device.
    match commands(&acts).as_slice() {
        [
            Command::Workflow(WorkflowCommand::CreateEffect {
                source:
                    CreateSource::Plugin {
                        plugin,
                        layout,
                        path_hint,
                    },
                target,
                key: k,
                options,
            }),
        ] => {
            assert_eq!(target, &CreateTarget::Selected(key("synth")));
            assert_eq!(plugin, &fx_ref());
            assert_eq!(layout, &fx_layout());
            assert_eq!(path_hint.as_deref(), Some("C:/Plugins/test.clap"));
            assert_eq!(k, &None);
            assert_eq!(options, &CreateOptions::default());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_single_click_on_a_heading_or_hint_does_nothing_and_a_double_click_adds_nothing() {
    let (d, mut v) = setup();
    let rows = v.browser_list(&registry());
    for (i, r) in rows.iter().enumerate() {
        if matches!(
            r,
            BrowserRow::Heading(_) | BrowserRow::Hint(_) | BrowserRow::Failed { .. }
        ) {
            let p = v.browser_row_center(i).unwrap();
            assert!(
                commands(&double_click(&mut v, d.doc(), p)).is_empty(),
                "row {i}"
            );
        }
    }
}

#[test]
fn the_keyboard_skips_headings_and_enter_adds_the_highlighted_plugin() {
    let (d, mut v) = setup();
    // Narrow the list to the plugins, then walk it.
    type_keys(&mut v, d.doc(), "atomik test");
    // "Atomik Test" is the vendor of the synth and the effect.
    let l = labels(&v);
    assert_eq!(
        &l[..5],
        [
            "# PLUGINS",
            "# Instruments",
            "plugin 0",
            "# Effects",
            "plugin 1"
        ]
    );
    // Typing put the highlight on the first selectable row (the synth).
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Workflow(WorkflowCommand::CreateInstrument {
            source: CreateSource::Plugin { plugin, .. }, ..
        })] if plugin == &synth_ref()
    ));
    // Down skips the "Effects" heading and lands on the effect.
    let _ = send_key(&mut v, d.doc(), Key::Down);
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Plugin { plugin, .. }, ..
        })] if plugin == &fx_ref()
    ));
    // Up goes back to the synth, and further up stays there.
    let _ = send_key(&mut v, d.doc(), Key::Up);
    let _ = send_key(&mut v, d.doc(), Key::Up);
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Workflow(WorkflowCommand::CreateInstrument {
            source: CreateSource::Plugin { plugin, .. }, ..
        })] if plugin == &synth_ref()
    ));
}

#[test]
fn inside_a_container_an_added_plugin_goes_into_the_container() {
    let d = plugin_in_container();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_plugin_catalog(catalog());
    v.set_browser_visible(true);
    type_keys(&mut v, d.doc(), "atomik test");
    // First selectable: the synth: an instrument gets its split.
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    match commands(&acts).as_slice() {
        [
            Command::Plugin(PluginCommand::Add {
                container,
                plugin,
                auto_split,
                ..
            }),
        ] => {
            assert_eq!(container, &Some(key("layer")));
            assert_eq!(plugin, &synth_ref());
            assert!(*auto_split);
        }
        other => panic!("{other:?}"),
    }
    let _ = send_key(&mut v, d.doc(), Key::Down);
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Plugin(PluginCommand::Add { container: Some(c), auto_split: false, .. })] if c == &key("layer")
    ));
}

#[test]
fn an_added_plugin_command_is_accepted_by_the_model_in_both_places() {
    let mut d = plugin_in_container();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_plugin_catalog(catalog());
    v.set_browser_visible(true);
    type_keys(&mut v, d.doc(), "atomik test");
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    for a in acts {
        if let UiAction::Command(c) = a {
            d.apply(c, &registry()).expect("the lifted add applies");
        }
    }
    assert!(d.doc().resolve(&key("layer/test-synth")).is_some());
    let mut v = view(d.doc(), &registry());
    v.set_plugin_catalog(catalog());
    v.set_browser_visible(true);
    type_keys(&mut v, d.doc(), "atomik test");
    let acts = send_key(&mut v, d.doc(), Key::Enter);
    for a in acts {
        if let UiAction::Command(c) = a {
            d.apply(c, &registry()).expect("the top-level add applies");
        }
    }
    assert!(
        d.doc()
            .devices
            .iter()
            .filter(|x| x.type_id == SYNTH_TYPE)
            .count()
            >= 2
    );
}

#[test]
fn the_group_matches_golden() {
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "atomik test");
    check(
        "plugin_browser_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "other");
    check(
        "plugin_browser_hidden_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_painted_group_shows_titles_badges_the_hint_and_the_reason() {
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "atomik test");
    let dl = paint_rack(&mut v, d.doc());
    let texts = dl.texts();
    for t in [
        "PLUGINS",
        "Instruments",
        "Effects",
        "Test Synth (Atomik Test)",
        "Test Fx (Atomik Test)",
        "CLAP",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
    let (d, mut v) = setup();
    type_keys(&mut v, d.doc(), "other");
    let dl = paint_rack(&mut v, d.doc());
    let texts = dl.texts();
    for t in [
        "Note effects",
        "1 unsupported plugin hidden",
        "Failed",
        "duplicate id",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
    assert!(
        !texts.contains(&"Surround (Other Co)"),
        "unsupported plugins are hidden"
    );
    assert!(
        !texts.contains(&"Switched Off (Other Co)"),
        "disabled plugins are hidden"
    );
}
