//! A plugin that changes its buses at run time (VST3 `restartComponent` with a bus change): the
//! layout sync drops only the cables on ports that vanished, a project saved after the change
//! loads over a registry that still has the older scan, and a command can be given the freshest
//! layout.

mod common;

use atomik_device_sdk::{PluginLayout, Registry, descriptor_for};
use atomik_model::{
    Command, Document, PluginCommand, cables_off_layout, empty_doc, load_project,
    map_plugin_layouts, register_stand_ins, save_project,
};
use common::plugin::*;
use common::*;

fn grown() -> PluginLayout {
    PluginLayout {
        aux_out: 2,
        ..fx_layout()
    }
}

fn registry_with(layout: &PluginLayout) -> Registry {
    let mut r = registry();
    register_plugin(&mut r, &fx_ref(), layout);
    r
}

/// Two effects of the same plugin: `a` into `b`, plus a cable from `a`'s first aux pair into
/// `b`'s sidechain.
fn cabled(r: &Registry, layout: &PluginLayout) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add_plugin(0, &fx_ref(), layout, Some("a")),
            add_plugin(1, &fx_ref(), layout, Some("b")),
            connect("a:out_l", "b:in_l"),
            connect("a:out_aux_1_l", "b:sc_in_l"),
        ],
    );
    d
}

fn sync(device: &str, layout: PluginLayout) -> Command {
    Command::Plugin(PluginCommand::SyncLayout {
        device: key(device),
        layout,
    })
}

#[test]
fn a_layout_sync_that_loses_a_port_drops_only_the_cables_on_it() {
    let r = registry_with(&grown());
    let mut d = cabled(&r, &grown());
    let desc = descriptor_for(&fx_ref(), &fx_layout()).unwrap();
    let off = cables_off_layout(d.doc(), &fx_ref().type_id(), desc);
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].1, cable("a:out_aux_1_l", "b:sc_in_l"));
    let steps = d.undo_depth();
    d.apply(sync("a", fx_layout()), &r).unwrap();
    assert_eq!(d.doc().cables, vec![cable("a:out_l", "b:in_l")]);
    assert_eq!(d.undo_depth(), steps, "a layout sync is not an undo step");
}

#[test]
fn a_layout_sync_that_only_gains_ports_keeps_every_cable() {
    let r = registry_with(&grown());
    let mut d = cabled(&r, &grown());
    let more = PluginLayout {
        aux_out: 3,
        ..grown()
    };
    d.apply(sync("a", more), &r).unwrap();
    assert_eq!(d.doc().cables.len(), 2);
}

#[test]
fn a_project_saved_after_the_buses_grew_loads_over_the_older_scan() {
    let saved = save_project(cabled(&registry_with(&grown()), &grown()).doc());
    // This run's scan is the older one: no aux outputs.
    let mut older = registry_with(&fx_layout());
    let doc = load_project(&saved, &older).expect("the project loads");
    assert_eq!(doc.cables.len(), 2, "the cable on the aux port is kept");
    // The app registers the types the file needs: the saved ports win until the plugin speaks.
    let added = register_stand_ins(&doc, &mut older).unwrap();
    assert_eq!(added, vec![fx_ref().type_id()]);
    assert!(
        older
            .descriptor(&fx_ref().type_id())
            .unwrap()
            .socket_by_key("out_aux_2_r")
            .is_some()
    );
}

#[test]
fn a_project_whose_scan_has_more_ports_than_the_file_is_left_alone() {
    let older = registry_with(&fx_layout());
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &older,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("a")),
            add_plugin(1, &fx_ref(), &fx_layout(), Some("b")),
            connect("a:out_l", "b:in_l"),
        ],
    );
    let saved = save_project(d.doc());
    let mut newer = registry_with(&grown());
    let doc = load_project(&saved, &newer).expect("the project loads");
    assert!(register_stand_ins(&doc, &mut newer).unwrap().is_empty());
}

#[test]
fn commands_are_given_the_layout_the_caller_knows_now() {
    let mut add = add_plugin(0, &fx_ref(), &fx_layout(), Some("a"));
    let mut batch = Command::Batch(vec![
        add_plugin(0, &fx_ref(), &fx_layout(), Some("b")),
        connect("a:out_l", "b:in_l"),
    ]);
    let fresh = grown();
    for c in [&mut add, &mut batch] {
        map_plugin_layouts(c, &mut |plugin, layout| {
            assert_eq!(plugin, &fx_ref());
            *layout = fresh.clone();
        });
    }
    let Command::Plugin(PluginCommand::Add { layout, .. }) = &add else {
        panic!("an add");
    };
    assert_eq!(layout.aux_out, 2);
    let Command::Batch(inner) = &batch else {
        panic!("a batch");
    };
    let Command::Plugin(PluginCommand::Add { layout, .. }) = &inner[0] else {
        panic!("an add in the batch");
    };
    assert_eq!(layout.aux_out, 2);
}
