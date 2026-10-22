//! Workflow device commands (model half): remove, duplicate, paste,
//! move, duplicate track and disconnect, as one undo step each, with fresh keys, kept inner
//! cables, dropped outer cables, container and plugin devices, the console rule for pasted
//! instruments, and rejections that change nothing.

mod common;

use atomik_device_sdk::PluginStateBlob;
use atomik_model::{
    Command, DeviceSnapshot, Document, ModelError, PluginCommand, SongCommand, SyncOp,
    WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::plugin::*;
use common::song::*;
use common::workflow::*;
use common::*;

fn snapshot(d: &Document, list: &[&str], tracks: bool) -> DeviceSnapshot {
    DeviceSnapshot::capture(d.doc(), &keys_of(list), tracks).unwrap()
}

fn duplicate(list: &[&str]) -> Command {
    wf(WorkflowCommand::DuplicateDevices {
        keys: keys_of(list),
    })
}

// ---------------------------------------------------------------- snapshots

#[test]
fn a_snapshot_holds_the_devices_in_rack_order_and_only_the_cables_inside_the_set() {
    let r = registry();
    let d = chain(&r);
    // Selection order does not matter: the snapshot is in rack order.
    let s = snapshot(&d, &["delay", "synth"], false);
    let ks: Vec<&str> = s.devices.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(ks, ["synth", "delay"]);
    assert_eq!(s.cables.len(), 2, "synth -> delay left and right");
    assert!(
        s.cables
            .iter()
            .all(|c| c.from.device.as_str() == "synth" && c.to.device.as_str() == "delay")
    );
    assert!(s.tracks.is_empty());
}

#[test]
fn a_snapshot_rejects_nothing_selected_unknown_keys_and_mixed_scopes() {
    let r = registry();
    let d = container_doc(&r);
    let doc = d.doc();
    assert!(matches!(
        DeviceSnapshot::capture(doc, &[], false),
        Err(ModelError::BadScope(_))
    ));
    assert!(matches!(
        DeviceSnapshot::capture(doc, &keys_of(&["nope"]), false),
        Err(ModelError::UnknownDevice(_))
    ));
    assert!(matches!(
        DeviceSnapshot::capture(doc, &keys_of(&["c", "c/synth"]), false),
        Err(ModelError::BadScope(_))
    ));
    // Tracks live at the top level only.
    assert!(matches!(
        DeviceSnapshot::capture(doc, &keys_of(&["c/synth"]), true),
        Err(ModelError::BadScope(_))
    ));
}

#[test]
fn a_snapshot_leaves_the_console_devices_out() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    // synth, its channel and the master.
    assert_eq!(d.doc().devices.len(), 3);
    let all: Vec<String> = keys(&d);
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let s = snapshot(&d, &all, false);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].key.as_str(), "synth");
    // Only console devices selected: nothing to copy.
    let channel = d.doc().console.channels[0].channel.clone();
    assert!(matches!(
        DeviceSnapshot::capture(d.doc(), &[channel], false),
        Err(ModelError::BadConsole(_))
    ));
}

// ---------------------------------------------------------------- duplicate

#[test]
fn duplicating_two_cabled_devices_gives_fresh_keys_inner_cables_and_no_outer_cables() {
    let r = registry();
    let mut d = chain(&r);
    d.apply(set_param("delay", "mix", 0.5), &r).unwrap();
    d.apply(
        Command::SetFolded {
            key: key("delay"),
            folded: true,
        },
        &r,
    )
    .unwrap();
    let before_cables = d.doc().cables.len();
    let ops = one_step(&mut d, &r, duplicate(&["synth", "delay"]));
    // Applied: the copies sit right after the last selected device.
    assert_eq!(
        keys(&d),
        ["synth", "delay", "poly-synth", "stereo-delay", "mixer"]
    );
    let doc = d.doc();
    // Inner cables kept (left and right), outer cables (delay -> mixer, synth in) dropped.
    assert_eq!(doc.cables.len(), before_cables + 2);
    assert!(
        doc.cables
            .contains(&cable("poly-synth:out_l", "stereo-delay:in_l"))
    );
    assert!(
        doc.cables
            .contains(&cable("poly-synth:out_r", "stereo-delay:in_r"))
    );
    assert!(
        !doc.cables
            .iter()
            .any(|c| c.from.device.as_str() == "stereo-delay"
                || c.to.device.as_str() == "poly-synth")
    );
    // Params, fold state and type travel with the copy; names stay readable.
    let copy = doc.device(&key("stereo-delay")).unwrap();
    assert_eq!(copy.params.get("mix"), Some(&0.5));
    assert!(copy.folded);
    assert_eq!(copy.type_id, DELAY);
    assert_ne!(copy.name, doc.device(&key("delay")).unwrap().name);
    // The sync ops: the two devices, then one routing update.
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("poly-synth")),
            SyncOp::DeviceAdded(key("stereo-delay")),
            SyncOp::ParamChanged {
                key: key("stereo-delay"),
                param: "mix".into(),
                value: 0.5
            },
            routing()
        ]
    );
}

#[test]
fn keys_of_the_copies_are_generated_in_rack_order_from_the_next_free_key() {
    let r = registry();
    let mut d = chain(&r);
    run(&mut d, &r, [duplicate(&["synth"]), duplicate(&["synth"])]);
    assert_eq!(
        keys(&d),
        ["synth", "poly-synth-2", "poly-synth", "delay", "mixer"]
    );
    // A hole is reused the way `AddDevice` reuses it.
    d.apply(
        Command::RemoveDevice {
            key: key("poly-synth"),
        },
        &r,
    )
    .unwrap();
    d.apply(duplicate(&["delay"]), &r).unwrap();
    assert!(keys(&d).contains(&"stereo-delay".to_owned()));
    d.apply(duplicate(&["synth"]), &r).unwrap();
    assert!(keys(&d).contains(&"poly-synth".to_owned()));
}

#[test]
fn duplicating_the_same_document_twice_gives_identical_text() {
    let r = registry();
    let mut a = chain(&r);
    let mut b = chain(&r);
    a.apply(duplicate(&["delay", "synth"]), &r).unwrap();
    b.apply(duplicate(&["synth", "delay"]), &r).unwrap();
    assert_eq!(text(&a), text(&b));
}

#[test]
fn a_duplicated_instrument_gets_its_console_channel_in_the_same_step() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    assert_eq!(d.doc().console.channels.len(), 1);
    one_step(&mut d, &r, duplicate(&["synth"]));
    let doc = d.doc();
    assert_eq!(doc.console.channels.len(), 2);
    let channel = doc.console.channel_of(&key("poly-synth")).unwrap();
    // The channel sits right after its source in the rack and feeds the master.
    let at = keys(&d).iter().position(|k| k == "poly-synth").unwrap();
    assert_eq!(doc.devices[at + 1].key, channel.channel);
    assert!(
        doc.cables
            .iter()
            .any(|c| c.from.device == key("poly-synth") && c.to.device == channel.channel)
    );
    // One channel strip each: nothing is shared with the original.
    assert_ne!(
        doc.console.channel_of(&key("synth")).unwrap().channel,
        channel.channel
    );
}

#[test]
fn a_duplicate_lands_after_the_channel_of_the_last_selected_instrument() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    d.apply(duplicate(&["synth"]), &r).unwrap();
    let ks = keys(&d);
    let synth = ks.iter().position(|k| k == "synth").unwrap();
    // synth, its channel, then the copy.
    assert_eq!(ks[synth + 2], "poly-synth");
    assert_eq!(
        d.doc().devices[synth + 1].key,
        d.doc().console.channel_of(&key("synth")).unwrap().channel
    );
}

#[test]
fn a_pasted_instrument_that_feeds_a_pasted_device_gets_no_channel_of_its_own() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, DELAY, Some("delay")),
            connect("synth:out_l", "delay:in_l"),
            connect("synth:out_r", "delay:in_r"),
        ],
    );
    let channels = d.doc().console.channels.len();
    d.apply(duplicate(&["synth", "delay"]), &r).unwrap();
    assert_eq!(
        d.doc().console.channels.len(),
        channels,
        "synth feeds delay"
    );
    assert!(
        d.doc()
            .cables
            .contains(&cable("poly-synth:out_l", "stereo-delay:in_l"))
    );
}

#[test]
fn duplicating_a_container_copies_the_inside_deep() {
    let r = registry();
    let mut d = container_doc(&r);
    run(
        &mut d,
        &r,
        [
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
            set_splits("c", vec![split("synth", 0, 127, 0)]),
        ],
    );
    one_step(&mut d, &r, duplicate(&["c"]));
    assert_eq!(keys(&d), ["c", "container"]);
    let (a, b) = (
        d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap(),
        d.doc()
            .device(&key("container"))
            .unwrap()
            .inner
            .as_ref()
            .unwrap(),
    );
    // Inner keys are local to the container, so the inside is the same data.
    assert_eq!(a, b);
    assert_eq!(b.cables.len(), 4);
}

#[test]
fn a_container_copy_beyond_the_inner_device_limit_is_rejected() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    for i in 0..30 {
        d.apply(add_inner("c", i, DELAY, &format!("d{i}")), &r)
            .unwrap();
    }
    // Container c holds 30; a copy holds 30 more: 60 of the project's 256 is fine.
    d.apply(duplicate(&["c"]), &r).unwrap();
    assert_eq!(d.doc().inner_device_count(), 60);
    // Copying inside the container beyond 32 devices is refused.
    let s = snapshot(&d, &["c/d0", "c/d1", "c/d2", "c/d3"], false);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: Some(key("c")),
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e:?}");
}

#[test]
fn a_container_copy_beyond_the_project_limit_is_rejected() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    for i in 0..32 {
        d.apply(add_inner("c", i, DELAY, &format!("d{i}")), &r)
            .unwrap();
    }
    // 32 per copy: seven copies reach 256, the eighth is refused.
    for _ in 0..7 {
        d.apply(duplicate(&["c"]), &r).unwrap();
    }
    assert_eq!(d.doc().inner_device_count(), 256);
    let e = rejected(&mut d, &r, duplicate(&["c"]));
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e:?}");
}

#[test]
fn containers_nest_at_most_three_levels_when_pasting() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("a")),
            add_inner("a", 0, CONTAINER, "b"),
            add_inner("a/b", 0, CONTAINER, "c"),
            add(1, CONTAINER, Some("x")),
        ],
    );
    // A container pasted into the innermost container would be a fourth level.
    let s = snapshot(&d, &["x"], false);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: Some(key("a/b/c")),
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::ContainerTooDeep(_)), "{e:?}");
}

#[test]
fn devices_paste_into_a_container_and_keep_their_inner_cables() {
    let r = registry();
    let mut d = container_doc(&r);
    d.apply(add(1, DELAY, Some("outer-delay")), &r).unwrap();
    d.apply(add(2, SYNTH, Some("outer-synth")), &r).unwrap();
    d.apply(connect("outer-synth:out_l", "outer-delay:in_l"), &r)
        .unwrap();
    let s = snapshot(&d, &["outer-synth", "outer-delay"], false);
    assert_eq!(s.cables.len(), 1);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: Some(key("c")),
            at: 2,
        }),
    );
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    let ks: Vec<&str> = inner.devices.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(ks, ["synth", "delay", "stereo-delay", "poly-synth"]);
    assert!(
        inner
            .cables
            .contains(&cable("poly-synth:out_l", "stereo-delay:in_l"))
    );
}

#[test]
fn a_pasted_device_that_a_container_cannot_hold_is_rejected() {
    // The audio track device cannot live inside a container.
    let r = audio_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, CONTAINER, Some("c")), add_audio_track()],
    );
    let s = snapshot(&d, &["audio-track"], false);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: Some(key("c")),
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::NotAllowedInContainer(_)), "{e:?}");
}

#[test]
fn duplicating_a_plugin_device_keeps_its_state_and_keys_it_after_its_name() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("fx")),
            sync_state("fx", &[1, 2, 3, 4]),
            set_pinned("fx", &[1, 3]),
        ],
    );
    one_step(&mut d, &r, duplicate(&["fx"]));
    assert_eq!(keys(&d), ["fx", "test-fx"]);
    let a = plugin_of(d.doc(), "fx");
    let b = plugin_of(d.doc(), "test-fx");
    assert_eq!(a, b, "the plugin document (state, pins, layout) is copied");
    assert_eq!(b.state.as_deref(), Some(&PluginStateBlob(vec![1, 2, 3, 4])));
}

#[test]
fn pasting_plugin_state_beyond_the_project_limit_is_rejected() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("fx")),
            Command::Plugin(PluginCommand::SyncState {
                device: key("fx"),
                state: PluginStateBlob(vec![7; 16 * 1024 * 1024]),
            }),
        ],
    );
    // The blob is shared, but each copy counts: 128 MiB holds eight of 16 MiB.
    for n in 1..=7 {
        d.apply(duplicate(&["fx"]), &r)
            .unwrap_or_else(|e| panic!("copy {n}: {e}"));
    }
    let e = rejected(&mut d, &r, duplicate(&["fx"]));
    assert!(matches!(e, ModelError::BadPlugin(_)), "{e:?}");
}

#[test]
fn a_plugin_that_the_registry_does_not_know_cannot_be_pasted() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add_plugin(0, &fx_ref(), &fx_layout(), Some("fx")), &r)
        .unwrap();
    let s = snapshot(&d, &["fx"], false);
    // Another song, another registry: the plugin is not known there.
    let r2 = registry();
    let mut other = Document::new(empty_doc());
    let e = rejected(
        &mut other,
        &r2,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::UnknownType(_)), "{e:?}");
}

#[test]
fn a_sampler_with_zones_pastes_where_the_pool_has_the_assets_and_not_elsewhere() {
    let r = audio_registry();
    let mut d = with_assets(&r);
    run(
        &mut d,
        &r,
        [
            add(0, SAMPLER, Some("sampler")),
            Command::SetZones {
                device: key("sampler"),
                zones: vec![atomik_model::ZoneDoc::new(hash(1), 480_000)],
                merge: None,
            },
        ],
    );
    let s = snapshot(&d, &["sampler"], false);
    assert!(s.assets.contains_key(&hash(1)), "the pool entries it needs");
    assert!(s.required_assets().contains(&hash(1)));
    // A song without the asset refuses it and changes nothing.
    let mut empty = Document::new(empty_doc());
    let paste = wf(WorkflowCommand::PasteDevices {
        snapshot: s.clone(),
        container: None,
        at: 0,
    });
    let e = rejected(&mut empty, &r, paste.clone());
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    // The app registers the asset first (RegisterAsset is not an undo step); then it pastes.
    empty
        .apply(
            Command::RegisterAsset {
                hash: hash(1),
                meta: s.assets[&hash(1)].clone(),
            },
            &r,
        )
        .unwrap();
    one_step(&mut empty, &r, paste);
    assert_eq!(empty.doc().device(&key("sampler")).unwrap().zones.len(), 1);
}

// ---------------------------------------------------------------- paste

#[test]
fn paste_inserts_at_the_given_rack_index_and_clamps_it() {
    let r = registry();
    let mut d = chain(&r);
    let s = snapshot(&d, &["delay"], false);
    let paste = |at| {
        wf(WorkflowCommand::PasteDevices {
            snapshot: s.clone(),
            container: None,
            at,
        })
    };
    one_step(&mut d, &r, paste(0));
    assert_eq!(keys(&d), ["stereo-delay", "synth", "delay", "mixer"]);
    one_step(&mut d, &r, paste(99));
    assert_eq!(
        keys(&d),
        ["stereo-delay", "synth", "delay", "mixer", "stereo-delay-2"]
    );
}

#[test]
fn paste_works_across_documents_and_the_snapshot_owns_its_data() {
    let r = registry();
    let a = chain(&r);
    let s = snapshot(&a, &["synth", "delay"], false);
    drop(a);
    let mut b = Document::new(empty_doc());
    one_step(
        &mut b,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
    assert_eq!(keys(&b), ["poly-synth", "stereo-delay"]);
    assert_eq!(b.doc().cables.len(), 2);
}

#[test]
fn an_empty_snapshot_and_a_console_device_in_a_snapshot_are_rejected() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    let s = DeviceSnapshot {
        devices: Vec::new(),
        cables: Vec::new(),
        tracks: Vec::new(),
        assets: Default::default(),
    };
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    let channel = d.doc().devices[1].clone();
    assert_eq!(channel.type_id, CHANNEL);
    let s = DeviceSnapshot {
        devices: vec![channel],
        cables: Vec::new(),
        tracks: Vec::new(),
        assets: Default::default(),
    };
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
}

#[test]
fn a_snapshot_with_tracks_cannot_paste_into_a_container() {
    let r = registry();
    let mut d = container_doc(&r);
    run(&mut d, &r, [add(1, SYNTH, Some("top")), add_track("top")]);
    let s = snapshot(&d, &["top"], true);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: Some(key("c")),
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
}

#[test]
fn a_pasted_cable_that_would_close_a_loop_is_never_produced() {
    // The copies of a valid chain are valid; a snapshot built by hand with a loop is refused.
    let r = registry();
    let mut d = chain(&r);
    let mut s = snapshot(&d, &["delay"], false);
    s.devices.push({
        let mut x = d.doc().device(&key("delay")).unwrap().clone();
        x.key = key("delay-b");
        x
    });
    s.cables = vec![
        cable("delay:out_l", "delay-b:in_l"),
        cable("delay-b:out_l", "delay:in_l"),
    ];
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

// ---------------------------------------------------------------- remove

#[test]
fn removing_devices_is_one_undo_step_that_restores_everything() {
    let r = registry();
    let mut d = chain(&r);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["delay", "synth"]),
        }),
    );
    assert_eq!(keys(&d), ["mixer"]);
    // Routing first, then the removals, in the order of the sync table.
    assert_eq!(ops[0], routing());
    assert!(ops.contains(&SyncOp::DeviceRemoved(key("synth"))));
    assert!(ops.contains(&SyncOp::DeviceRemoved(key("delay"))));
    assert!(d.doc().cables.iter().all(|c| {
        !matches!(c.from.device.as_str(), "synth" | "delay")
            && !matches!(c.to.device.as_str(), "synth" | "delay")
    }));
}

#[test]
fn removing_an_instrument_and_its_channel_together_works() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    let channel = d.doc().console.channels[0].channel.clone();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: vec![channel.clone(), key("synth")],
        }),
    );
    assert!(d.doc().console.channels.is_empty());
    assert!(d.doc().device(&key("synth")).is_none());
    assert!(d.doc().device(&channel).is_none());
}

#[test]
fn removing_a_device_takes_its_song_track_and_its_channel_like_remove_device() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        ],
    );
    let mut single = Document::new(d.doc().clone());
    single
        .apply(Command::RemoveDevice { key: key("synth") }, &r)
        .unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["synth"]),
        }),
    );
    // The same document as the single-device command leaves.
    assert_eq!(text(&d), text(&single));
    assert!(d.doc().song.tracks.is_empty());
}

#[test]
fn removing_a_container_and_one_of_its_devices_removes_the_container_once() {
    let r = registry();
    let mut d = container_doc(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["c/synth", "c"]),
        }),
    );
    assert!(d.doc().devices.is_empty());
}

#[test]
fn removing_inner_devices_works_in_a_container_scope() {
    let r = registry();
    let mut d = container_doc(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["c/synth", "c/delay"]),
        }),
    );
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert!(inner.devices.is_empty() && inner.cables.is_empty());
}

#[test]
fn removing_nothing_or_an_unknown_device_is_rejected_and_changes_nothing() {
    let r = registry();
    let mut d = chain(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices { keys: Vec::new() }),
    );
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    // One unknown key spoils the whole command: the known ones stay.
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["synth", "ghost"]),
        }),
    );
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
}

#[test]
fn duplicate_keys_in_a_selection_count_once() {
    let r = registry();
    let mut d = chain(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["delay", "delay", "delay"]),
        }),
    );
    assert_eq!(keys(&d), ["synth", "mixer"]);
    one_step(&mut d, &r, duplicate(&["synth", "synth"]));
    assert_eq!(keys(&d), ["synth", "poly-synth", "mixer"]);
}

// ---------------------------------------------------------------- move

fn mv(list: &[&str], to: usize) -> Command {
    wf(WorkflowCommand::MoveDevices {
        keys: keys_of(list),
        to,
    })
}

#[test]
fn moving_a_group_keeps_its_relative_order_and_puts_the_first_at_the_target() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, DELAY, Some("b")),
            add(2, DELAY, Some("c")),
            add(3, MIXER, Some("d")),
            add(4, COMP, Some("e")),
        ],
    );
    // Selection order is irrelevant; the group keeps rack order a, c.
    one_step(&mut d, &r, mv(&["c", "a"], 2));
    assert_eq!(keys(&d), ["b", "d", "a", "c", "e"]);
    let ops = one_step(&mut d, &r, mv(&["e"], 0));
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert_eq!(keys(&d), ["e", "b", "d", "a", "c"]);
    // The target clamps so the whole group still fits.
    one_step(&mut d, &r, mv(&["e", "b"], 99));
    assert_eq!(keys(&d), ["d", "a", "c", "e", "b"]);
}

#[test]
fn moving_inner_devices_stays_in_their_container() {
    let r = registry();
    let mut d = container_doc(&r);
    one_step(&mut d, &r, mv(&["c/synth"], 1));
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    let ks: Vec<&str> = inner.devices.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(ks, ["delay", "synth"]);
}

#[test]
fn a_move_that_changes_nothing_or_mixes_scopes_is_rejected() {
    let r = registry();
    let mut d = container_doc(&r);
    d.apply(add(1, DELAY, Some("top")), &r).unwrap();
    let e = rejected(&mut d, &r, mv(&["c"], 0));
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    let e = rejected(&mut d, &r, mv(&["c", "c/synth"], 0));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    let e = rejected(&mut d, &r, mv(&["ghost"], 0));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
    let e = rejected(&mut d, &r, mv(&[], 0));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
}

// ---------------------------------------------------------------- disconnect

#[test]
fn disconnecting_a_device_removes_every_cable_it_has_in_one_step() {
    let r = registry();
    let mut d = chain(&r);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevice { key: key("delay") }),
    );
    assert_eq!(ops, [routing()]);
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device != key("delay") && c.to.device != key("delay"))
    );
    // The neighbours keep their other cables.
    assert!(
        d.doc()
            .cables
            .contains(&cable("mixer:master_l", "interface:out_1"))
    );
    // A device without cables is a rejection, like `Disconnect` of an unplugged input.
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevice { key: key("delay") }),
    );
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn disconnecting_an_inner_device_clears_its_inner_cables_including_the_boundary() {
    let r = registry();
    let mut d = container_doc(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevice {
            key: key("c/delay"),
        }),
    );
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert!(inner.cables.is_empty(), "all four cables touched the delay");
}

#[test]
fn disconnecting_unknown_devices_is_rejected() {
    let r = registry();
    let mut d = chain(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevice { key: key("ghost") }),
    );
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevice {
            key: key("c/ghost"),
        }),
    );
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadScope(_)),
        "{e:?}"
    );
}

// ---------------------------------------------------------------- duplicate track

#[test]
fn duplicating_a_track_copies_device_track_lanes_and_clips_with_new_ids() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip(
                "synth",
                0,
                notes_clip(BAR, BAR, vec![note(PPQ, PPQ, 64, 0.5)]),
            ),
            sc(SongCommand::SetTrackFlags {
                device: key("synth"),
                muted: true,
                solo: false,
                armed: true,
            }),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
        ],
    );
    let next = d.doc().song.next_clip_id;
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("synth"),
        }),
    );
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), 2);
    let copy = doc.song.track(&key("poly-synth")).unwrap();
    let orig = doc.song.track(&key("synth")).unwrap();
    assert_eq!(copy.lanes.len(), orig.lanes.len());
    assert_eq!(copy.lanes[1].kind, orig.lanes[1].kind);
    // Clips keep geometry and content, with fresh ids from the next free id upward.
    let ids: Vec<u64> = copy.lanes[0].clips.iter().map(|c| c.id.0).collect();
    assert_eq!(ids, [next, next + 1]);
    assert_eq!(copy.lanes[0].clips[1].start, BAR);
    assert_eq!(doc.song.next_clip_id, next + 2);
    // The copy never takes over the keyboard: it is not armed, but mute carries over.
    assert!(!copy.armed && orig.armed && copy.muted);
    // The channel by the console rule, right after the copy.
    assert!(doc.console.channel_of(&key("poly-synth")).is_some());
    assert!(ops.contains(&SyncOp::SongChanged) && ops.contains(&SyncOp::ConsoleChanged));
    // The copy sits after the original and its channel.
    let ks = keys(&d);
    assert!(
        ks.iter().position(|k| k == "poly-synth").unwrap()
            > ks.iter().position(|k| k == "synth").unwrap() + 1
    );
}

#[test]
fn duplicating_a_device_without_a_track_as_a_track_is_rejected() {
    let r = registry();
    let mut d = chain(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("synth"),
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("ghost"),
        }),
    );
    assert!(matches!(
        e,
        ModelError::UnknownDevice(_) | ModelError::BadSong(_)
    ));
}

#[test]
fn duplicating_an_audio_track_copies_the_clips_and_wires_the_inputs() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &r)
        .unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("audio-track"),
        }),
    );
    let doc = d.doc();
    let copy = doc.song.track(&key("audio-track-2")).unwrap();
    assert!(copy.is_audio());
    assert_eq!(copy.lanes[0].clips.len(), 1);
    // Like a new audio track: inputs 1 and 2 feed it.
    assert!(
        doc.cables
            .contains(&cable("interface:in_1", "audio-track-2:in_l"))
    );
    assert!(
        doc.cables
            .contains(&cable("interface:in_2", "audio-track-2:in_r"))
    );
    assert!(doc.console.channel_of(&key("audio-track-2")).is_some());
}

// ---------------------------------------------------------------- history

#[test]
fn a_rejected_workflow_command_leaves_history_and_redo_alone() {
    let r = registry();
    let mut d = chain(&r);
    d.apply(duplicate(&["delay"]), &r).unwrap();
    d.undo(&r).unwrap();
    assert!(d.can_redo());
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDevices {
            keys: keys_of(&["ghost"]),
        }),
    );
    assert!(matches!(e, ModelError::UnknownDevice(_)));
    assert!(d.can_redo(), "the redo step survived the rejection");
}

#[test]
fn a_workflow_command_cannot_be_batched() {
    let r = registry();
    let mut d = chain(&r);
    let e = rejected(&mut d, &r, Command::Batch(vec![duplicate(&["delay"])]));
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

// ---------------------------------------------------------------- more edges

#[test]
fn the_sync_ops_of_a_track_duplicate_follow_the_compound_order() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("synth")), add_track("synth")],
    );
    let ops = d
        .apply(
            wf(WorkflowCommand::DuplicateTrack {
                device: key("synth"),
            }),
            &r,
        )
        .unwrap();
    let at = |op: &SyncOp| ops.iter().position(|o| o == op).unwrap();
    let added = at(&SyncOp::DeviceAdded(key("poly-synth")));
    // Devices arrive first, then routing, the console, and the song last.
    assert!(added < at(&routing()));
    assert!(at(&routing()) < at(&SyncOp::ConsoleChanged));
    assert!(at(&SyncOp::ConsoleChanged) < at(&SyncOp::SongChanged));
    assert_eq!(ops.iter().filter(|o| **o == routing()).count(), 1);
}

#[test]
fn a_paste_into_something_that_is_not_a_container_is_rejected() {
    let r = registry();
    let mut d = chain(&r);
    let s = snapshot(&d, &["delay"], false);
    for container in ["synth", "ghost"] {
        let e = rejected(
            &mut d,
            &r,
            wf(WorkflowCommand::PasteDevices {
                snapshot: s.clone(),
                container: Some(key(container)),
                at: 0,
            }),
        );
        assert!(
            matches!(e, ModelError::BadScope(_) | ModelError::UnknownDevice(_)),
            "{e:?}"
        );
    }
}

#[test]
fn a_copied_audio_track_in_a_song_without_inputs_is_not_wired() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    d.apply(Command::SetInputChannels(0), &r).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("audio-track"),
        }),
    );
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device.as_str() != "interface" || c.to.device != key("audio-track-2"))
    );
}

#[test]
fn a_copied_audio_device_without_its_track_is_not_wired_to_the_inputs() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    one_step(&mut d, &r, duplicate(&["audio-track"]));
    assert!(d.doc().song.track(&key("audio-track-2")).is_none());
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| c.to.device == key("audio-track-2") && c.from.device.as_str() == "interface")
    );
}

#[test]
fn a_hand_made_snapshot_of_an_instrument_without_audio_outputs_stays_a_plain_device() {
    // The instrument that cannot be wired to a channel is pasted as it is, like AddDevice.
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SEQ, Some("seq")), &r).unwrap();
    let before = d.doc().console.channels.len();
    one_step(&mut d, &r, duplicate(&["seq"]));
    assert_eq!(d.doc().console.channels.len(), before);
    assert_eq!(keys(&d).len(), 2);
}

// ---------------------------------------------------------------- additions

/// The snapshot reuses the document types, so every device type the registry knows
/// copies as one undo step and the copy equals the source apart from its key and name.
#[test]
fn every_device_type_of_the_registry_duplicates_as_one_step_with_its_own_data() {
    let r = console_registry();
    let types: Vec<String> = r.type_ids().map(str::to_owned).collect();
    let mut tested = 0;
    for t in types {
        if [CHANNEL, BUS, MASTER, SPLIT].contains(&t.as_str()) {
            continue;
        }
        let mut d = Document::new(empty_doc());
        if d.apply(add(0, &t, Some("src")), &r).is_err() {
            continue;
        }
        let src = d
            .doc()
            .devices
            .iter()
            .find(|x| x.key.0 == "src")
            .unwrap()
            .clone();
        one_step(&mut d, &r, duplicate(&["src"]));
        let mut copy = d
            .doc()
            .devices
            .iter()
            .find(|x| x.key.0 != "src" && x.type_id == t)
            .unwrap_or_else(|| panic!("{t} left no copy"))
            .clone();
        assert_ne!(copy.name, src.name, "{t}: the name is numbered");
        copy.key = src.key.clone();
        copy.name = src.name.clone();
        assert_eq!(
            copy, src,
            "{t}: params, zones, steps and the rest are copied"
        );
        tested += 1;
    }
    assert!(tested >= 8, "only {tested} types were exercised");
}

#[test]
fn a_copied_audio_track_on_a_mono_interface_gets_input_one_on_both_sides() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    d.apply(Command::SetInputChannels(1), &r).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateTrack {
            device: key("audio-track"),
        }),
    );
    let doc = d.doc();
    assert!(
        doc.cables
            .contains(&cable("interface:in_1", "audio-track-2:in_l"))
    );
    assert!(
        doc.cables
            .contains(&cable("interface:in_1", "audio-track-2:in_r"))
    );
}
