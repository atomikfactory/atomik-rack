//! The auto-routing creation chain: `CreateInstrument`,
//! `CreateEffect`, `AutoRoute`, `DisconnectDevices` and `DeleteDevices` (delete bridging). Each
//! command is one undo step, a rejected one changes nothing, and the same inputs give the same
//! saved text.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    ModelError, SongCommand, SyncOp, WorkflowCommand, empty_doc, insert_chain, load_project,
};
use common::console::AUDIO_ONLY_SYNTH;
use common::plugin::*;
use common::song::add_track;
use common::workflow::*;
use common::*;

fn reg() -> Registry {
    plugin_console_registry()
}

fn empty() -> Document {
    Document::new(empty_doc())
}

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

fn instrument_with(t: &str, options: CreateOptions) -> Command {
    wf(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: typ(t),
        key: None,
        options,
    })
}

fn instrument(t: &str) -> Command {
    instrument_with(t, CreateOptions::default())
}

fn effect_with(t: &str, target: CreateTarget, options: CreateOptions) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: typ(t),
        target,
        key: None,
        options,
    })
}

fn effect_on(t: &str, selected: &str) -> Command {
    effect_with(
        t,
        CreateTarget::Selected(key(selected)),
        CreateOptions::default(),
    )
}

fn effect(t: &str) -> Command {
    effect_with(t, CreateTarget::Nothing, CreateOptions::default())
}

fn no_route() -> CreateOptions {
    CreateOptions {
        route: false,
        ..CreateOptions::default()
    }
}

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn channel_of(d: &Document, source: &str) -> DeviceKey {
    d.doc()
        .console
        .channel_of(&key(source))
        .unwrap_or_else(|| panic!("{source} has a channel"))
        .channel
        .clone()
}

fn names(d: &Document, r: &Registry, device: &DeviceKey) -> Vec<String> {
    insert_chain(d.doc(), r, device)
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn has_cable(d: &Document, from: &str, to: &str) -> bool {
    d.doc().cables.contains(&cable(from, to))
}

fn armed(d: &Document, k: &str) -> bool {
    d.doc().song.track(&key(k)).is_some_and(|t| t.armed)
}

fn rules_index(d: &Document, k: &str) -> usize {
    d.doc().devices.iter().position(|x| x.key.0 == k).unwrap()
}

fn cables_of(d: &Document, k: &str) -> usize {
    d.doc()
        .cables
        .iter()
        .filter(|c| c.from.device.0 == k || c.to.device.0 == k)
        .count()
}

// ----------------------------------------------------------------------- CreateInstrument

#[test]
fn an_instrument_gets_its_channel_its_track_and_is_armed_in_one_undo_step() {
    let r = reg();
    let mut d = empty();
    let ops = one_step(&mut d, &r, instrument(SYNTH));
    // one_step undid and redid; the state is the created one.
    let doc = d.doc();
    assert_eq!(doc.devices.len(), 3, "synth, channel, master");
    assert_eq!(doc.devices[0].type_id, SYNTH);
    let synth = doc.devices[0].key.clone();
    assert_eq!(doc.console.channels.len(), 1);
    assert_eq!(doc.console.channels[0].source.as_ref(), Some(&synth));
    assert!(doc.console.master.is_some());
    let track = doc.song.track(&synth).expect("a song track");
    assert!(track.armed, "the new track is armed");
    assert_eq!(doc.song.tracks.len(), 1);
    // The sync ops of the compound step name every part.
    assert!(ops.contains(&SyncOp::SongChanged));
    assert!(ops.contains(&SyncOp::ConsoleChanged));
    assert!(ops.contains(&SyncOp::RoutingChanged));
}

#[test]
fn route_false_is_exactly_add_device() {
    let r = reg();
    let mut a = empty();
    a.apply(instrument_with(SYNTH, no_route()), &r).unwrap();
    let mut b = empty();
    b.apply(add(usize::MAX, SYNTH, None), &r).unwrap();
    assert_eq!(text(&a), text(&b));
    assert!(a.doc().song.tracks.is_empty(), "no track without routing");
    assert_eq!(
        a.doc().console.channels.len(),
        1,
        "the console rule still adds the channel"
    );
}

#[test]
fn alt_skips_the_track_but_keeps_the_channel() {
    let r = reg();
    let mut d = empty();
    d.apply(
        instrument_with(
            SYNTH,
            CreateOptions {
                skip_track: true,
                ..CreateOptions::default()
            },
        ),
        &r,
    )
    .unwrap();
    assert!(d.doc().song.tracks.is_empty());
    assert_eq!(d.doc().console.channels.len(), 1);
}

#[test]
fn an_instrument_without_a_notes_input_gets_a_channel_and_no_track() {
    let r = reg();
    let mut d = empty();
    one_step(&mut d, &r, instrument(AUDIO_ONLY_SYNTH.type_id));
    assert_eq!(d.doc().console.channels.len(), 1);
    assert!(d.doc().song.tracks.is_empty());
}

#[test]
fn an_instrument_without_audio_outputs_gets_a_track_and_no_channel() {
    let r = reg();
    let mut d = empty();
    one_step(&mut d, &r, instrument("test.silent-synth"));
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(d.doc().song.tracks.len(), 1);
    assert!(d.doc().song.tracks[0].armed);
}

#[test]
fn only_the_tracks_named_by_the_caller_are_disarmed() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add(2, SYNTH, Some("c")),
            add_track("a"),
            add_track("b"),
            add_track("c"),
        ],
    );
    for k in ["a", "b", "c"] {
        d.apply(sc_flags(k, true), &r).unwrap();
    }
    let options = CreateOptions {
        // `a` was armed by an earlier auto-route; `b` by the user; `gone` no longer exists.
        disarm: vec![key("a"), key("gone")],
        ..CreateOptions::default()
    };
    one_step(&mut d, &r, instrument_with(SYNTH, options));
    assert!(!armed(&d, "a"), "the earlier auto-armed track is disarmed");
    assert!(armed(&d, "b"), "a track armed by the user stays armed");
    assert!(armed(&d, "c"));
    let new = d.doc().song.tracks.last().unwrap();
    assert!(new.armed);
}

fn sc_flags(k: &str, armed: bool) -> Command {
    Command::Song(SongCommand::SetTrackFlags {
        device: key(k),
        muted: false,
        solo: false,
        armed,
    })
}

#[test]
fn an_explicit_key_and_index_are_used() {
    let r = reg();
    let mut d = empty();
    run(&mut d, &r, [add(0, DELAY, Some("delay"))]);
    d.apply(
        wf(WorkflowCommand::CreateInstrument {
            index: 0,
            source: typ(SYNTH),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().devices[0].key, key("lead"));
    assert!(d.doc().song.track(&key("lead")).is_some());
    // A key that is taken is a rejection and nothing changes.
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateInstrument {
            index: 0,
            source: typ(SYNTH),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    assert!(matches!(e, ModelError::BadKey(_)), "{e:?}");
}

#[test]
fn create_instrument_rejects_what_is_not_an_instrument_and_changes_nothing() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [instrument_with(SYNTH, CreateOptions::default())],
    );
    for t in [DELAY, "atomik.channel", "atomik.master", "nope.nothing"] {
        rejected(&mut d, &r, instrument(t));
    }
}

#[test]
fn the_same_creation_gives_the_same_document_and_it_loads_back() {
    let r = reg();
    let mut a = empty();
    let mut b = empty();
    for d in [&mut a, &mut b] {
        run(
            &mut *d,
            &r,
            [instrument(SYNTH), instrument(SYNTH), effect(DELAY)],
        );
    }
    assert_eq!(text(&a), text(&b));
    let loaded = load_project(&text(&a), &r).expect("loads");
    assert_eq!(atomik_model::save_project(&loaded), text(&a));
}

#[test]
fn a_plugin_instrument_is_routed_like_a_built_in_one() {
    let r = reg();
    let mut d = empty();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Plugin {
                plugin: synth_ref(),
                layout: synth_layout(),
                path_hint: None,
            },
            key: None,
            options: CreateOptions::default(),
        }),
    );
    let doc = d.doc();
    let dev = doc
        .devices
        .iter()
        .find(|x| x.plugin.is_some())
        .expect("the plugin device");
    assert!(doc.console.channel_of(&dev.key).is_some());
    assert!(doc.song.track(&dev.key).is_some_and(|t| t.armed));
}

// ------------------------------------------------------------------------ CreateEffect

/// A document with one synth (with channel and armed track).
fn with_synth(r: &Registry) -> Document {
    let mut d = empty();
    run(
        &mut d,
        r,
        [wf(WorkflowCommand::CreateInstrument {
            index: 0,
            source: typ(SYNTH),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        })],
    );
    d
}

#[test]
fn an_effect_goes_to_the_first_free_insert_of_the_selected_channel() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    one_step(&mut d, &r, effect_on(DELAY, &ch.0));
    assert_eq!(names(&d, &r, &ch).len(), 1);
    one_step(&mut d, &r, effect_on("atomik.reverb", &ch.0));
    let c = names(&d, &r, &ch);
    assert_eq!(c.len(), 2, "the second effect takes the next slot");
    // The chain order is the creation order and the devices follow the channel in the rack.
    let order: Vec<String> = d.doc().devices.iter().map(|x| x.key.0.clone()).collect();
    let pos = |k: &str| order.iter().position(|x| x == k).unwrap();
    assert!(pos(&ch.0) < pos(&c[0]) && pos(&c[0]) < pos(&c[1]));
}

#[test]
fn a_full_insert_chain_leaves_the_new_effect_added_only() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    for _ in 0..atomik_model::INSERT_SLOTS {
        run(&mut d, &r, [effect_on(DELAY, &ch.0)]);
    }
    let before: Vec<DeviceKey> = d.doc().devices.iter().map(|x| x.key.clone()).collect();
    one_step(&mut d, &r, effect_on(DELAY, &ch.0));
    assert_eq!(d.doc().devices.len(), before.len() + 1);
    assert_eq!(names(&d, &r, &ch).len(), atomik_model::INSERT_SLOTS);
    let added = d
        .doc()
        .devices
        .iter()
        .find(|x| !before.contains(&x.key))
        .unwrap()
        .key
        .clone();
    assert_eq!(
        cables_of(&d, &added.0),
        0,
        "an unrouted effect has no cables"
    );
    assert_eq!(
        d.doc().devices[rules_index(&d, &ch.0) + 1].key,
        added,
        "it sits right after the selection"
    );
}

#[test]
fn an_effect_with_an_instrument_selected_goes_to_its_channel() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    one_step(&mut d, &r, effect_on(DELAY, "synth"));
    assert_eq!(names(&d, &r, &ch).len(), 1);
}

#[test]
fn an_effect_with_an_fx_return_selected_follows_it_on_the_return() {
    let r = reg();
    let mut d = with_synth(&r);
    run(&mut d, &r, [add(usize::MAX, "atomik.reverb", Some("verb"))]);
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("verb")),
        })],
    );
    let master = d.doc().console.master.clone().unwrap();
    assert!(has_cable(&d, "verb:out_l", &format!("{master}:return1_l")));
    one_step(&mut d, &r, effect_on(DELAY, "verb"));
    let new = d
        .doc()
        .devices
        .iter()
        .find(|x| x.key.0.starts_with("stereo-delay"))
        .expect("the new effect")
        .key
        .clone();
    // verb -> new -> return 1 of the master.
    assert!(has_cable(&d, "verb:out_l", &format!("{new}:in_l")));
    assert!(has_cable(&d, "verb:out_r", &format!("{new}:in_r")));
    assert!(has_cable(
        &d,
        &format!("{new}:out_l"),
        &format!("{master}:return1_l")
    ));
    assert!(has_cable(
        &d,
        &format!("{new}:out_r"),
        &format!("{master}:return1_r")
    ));
    assert!(!has_cable(&d, "verb:out_l", &format!("{master}:return1_l")));
    // The device sits right after the return effect in the rack.
    let order: Vec<String> = d.doc().devices.iter().map(|x| x.key.0.clone()).collect();
    let i = order.iter().position(|k| k == "verb").unwrap();
    assert_eq!(order[i + 1], new.0);
}

#[test]
fn an_effect_after_a_free_device_is_cabled_to_it() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [add(0, DELAY, Some("d1")), add(1, MIXER, Some("mix"))],
    );
    one_step(&mut d, &r, effect_on("atomik.reverb", "d1"));
    let new = d.doc().devices[1].key.clone();
    assert_eq!(d.doc().devices[1].type_id, "atomik.reverb");
    assert!(has_cable(&d, "d1:out_l", &format!("{new}:in_l")));
    assert!(has_cable(&d, "d1:out_r", &format!("{new}:in_r")));
    assert_eq!(
        d.doc().devices[2].key,
        key("mix"),
        "right after the selection"
    );
}

#[test]
fn an_effect_after_a_device_whose_output_is_cabled_is_only_added_after_it() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("d1")),
            add(1, MIXER, Some("mix")),
            connect("d1:out_l", "mix:ch1_in_l"),
        ],
    );
    one_step(&mut d, &r, effect_on("atomik.reverb", "d1"));
    let new = d.doc().devices[1].key.clone();
    assert_eq!(cables_of(&d, &new.0), 0);
    assert!(
        has_cable(&d, "d1:out_l", "mix:ch1_in_l"),
        "nothing is re-cabled"
    );
}

#[test]
fn with_nothing_selected_the_effect_is_added_at_the_end() {
    let r = reg();
    let mut d = with_synth(&r);
    let n = d.doc().devices.len();
    one_step(&mut d, &r, effect(DELAY));
    assert_eq!(d.doc().devices.len(), n + 1);
    assert_eq!(d.doc().devices[n].type_id, DELAY);
    assert_eq!(cables_of(&d, &d.doc().devices[n].key.0.clone()), 0);
}

#[test]
fn route_false_adds_the_effect_only_even_with_a_channel_selected() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    one_step(
        &mut d,
        &r,
        effect_with(DELAY, CreateTarget::Selected(ch.clone()), no_route()),
    );
    assert!(names(&d, &r, &ch).is_empty());
}

#[test]
fn the_master_is_a_send_target_and_never_a_device_to_cable_after() {
    // Seam with the master section: the master has audio outputs, but the generic "after the
    // selected device" rule must never cable an effect after `main`. Since the send rule exists
    // the effect goes to send/return 1 instead; `main` stays as it was.
    let r = reg();
    let mut d = with_synth(&r);
    let master = d.doc().console.master.clone().unwrap();
    let n = d.doc().devices.len();
    let main_before: Vec<_> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.from.device == master && c.from.socket.starts_with("main_"))
        .cloned()
        .collect();
    one_step(&mut d, &r, effect_on(DELAY, &master.0));
    assert_eq!(d.doc().devices.len(), n + 1);
    let new = d
        .doc()
        .devices
        .iter()
        .find(|x| x.key.0.starts_with("stereo-delay"))
        .unwrap()
        .key
        .clone();
    assert_eq!(cables_of(&d, &new.0), 4, "fx_send1 -> effect -> return1");
    assert_eq!(d.doc().console.return_slot(&new), Some(0));
    let main_after: Vec<_> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.from.device == master && c.from.socket.starts_with("main_"))
        .cloned()
        .collect();
    assert_eq!(main_before, main_after, "nothing is cabled after main");
}

#[test]
fn a_device_that_is_not_a_stereo_effect_is_only_added() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    one_step(&mut d, &r, effect_on(MIXER, &ch.0));
    assert!(names(&d, &r, &ch).is_empty());
    assert!(d.doc().devices.iter().any(|x| x.type_id == MIXER));
}

#[test]
fn create_effect_rejects_instruments_and_console_devices() {
    let r = reg();
    let mut d = with_synth(&r);
    for t in [SYNTH, "atomik.channel", "atomik.master", "nope.nothing"] {
        rejected(&mut d, &r, effect(t));
    }
    // An unknown selected key is not an error: nothing suitable is selected.
    one_step(&mut d, &r, effect_on(DELAY, "ghost"));
}

#[test]
fn a_selection_inside_a_container_counts_as_nothing_selected() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "inner"),
        ],
    );
    one_step(&mut d, &r, effect_on(DELAY, "c/inner"));
    assert_eq!(d.doc().devices.len(), 2);
}

#[test]
fn a_plugin_effect_goes_to_the_channel_insert_too() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Plugin {
                plugin: fx_ref(),
                layout: fx_layout(),
                path_hint: None,
            },
            target: CreateTarget::Selected(ch.clone()),
            key: None,
            options: CreateOptions::default(),
        }),
    );
    let c = names(&d, &r, &ch);
    assert_eq!(c.len(), 1);
    assert!(d.doc().device(&key(&c[0])).unwrap().plugin.is_some());
}

// -------------------------------------------------------------------------- AutoRoute

fn route(devices: &[&str]) -> Command {
    wf(WorkflowCommand::AutoRoute {
        devices: devices.iter().map(|k| key(k)).collect(),
        options: CreateOptions::default(),
    })
}

#[test]
fn auto_route_gives_an_instrument_its_channel_and_track() {
    let r = reg();
    let mut d = with_synth(&r);
    // Take the channel and the track away again.
    let ch = channel_of(&d, "synth");
    run(
        &mut d,
        &r,
        [
            cc(ConsoleCommand::RemoveChannel { channel: ch }),
            Command::Song(SongCommand::RemoveTrack {
                device: key("synth"),
            }),
        ],
    );
    assert!(d.doc().console.channels.is_empty());
    one_step(&mut d, &r, route(&["synth"]));
    assert!(d.doc().console.channel_of(&key("synth")).is_some());
    assert!(armed(&d, "synth"));
}

#[test]
fn auto_route_adds_only_what_is_missing() {
    let r = reg();
    let mut d = with_synth(&r);
    d.apply(
        Command::Song(SongCommand::RemoveTrack {
            device: key("synth"),
        }),
        &r,
    )
    .unwrap();
    let channels = d.doc().devices.len();
    one_step(&mut d, &r, route(&["synth"]));
    assert_eq!(
        d.doc().devices.len(),
        channels,
        "the channel was there already"
    );
    assert!(armed(&d, "synth"));
}

#[test]
fn auto_route_of_a_routed_instrument_is_a_rejection() {
    let r = reg();
    let mut d = with_synth(&r);
    let e = rejected(&mut d, &r, route(&["synth"]));
    assert!(e.to_string().contains("already"), "{e}");
}

#[test]
fn auto_route_disarms_the_named_tracks() {
    let r = reg();
    let mut d = empty();
    run(&mut d, &r, [instrument(SYNTH)]);
    let first = d.doc().devices[0].key.clone();
    run(&mut d, &r, [add(usize::MAX, SYNTH, Some("b"))]);
    // `b` got a channel through; remove its channel so there is something to route.
    let ch = channel_of(&d, "b");
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::RemoveChannel { channel: ch })],
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::AutoRoute {
            devices: vec![key("b")],
            options: CreateOptions {
                disarm: vec![first.clone()],
                ..CreateOptions::default()
            },
        }),
    );
    assert!(!armed(&d, &first.0));
    assert!(armed(&d, "b"));
}

#[test]
fn auto_route_puts_a_loose_effect_into_the_insert_of_the_channel_before_it() {
    let r = reg();
    let mut d = with_synth(&r);
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("fx"))]);
    let ch = channel_of(&d, "synth");
    one_step(&mut d, &r, route(&["fx"]));
    assert_eq!(names(&d, &r, &ch), vec!["fx".to_string()]);
}

#[test]
fn auto_route_cables_a_loose_effect_after_a_free_device() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [add(0, DELAY, Some("a")), add(1, DELAY, Some("b"))],
    );
    one_step(&mut d, &r, route(&["b"]));
    assert!(has_cable(&d, "a:out_l", "b:in_l"));
    assert!(has_cable(&d, "a:out_r", "b:in_r"));
}

#[test]
fn auto_route_refuses_an_effect_that_is_connected_or_has_nowhere_to_go() {
    let r = reg();
    let mut d = empty();
    run(&mut d, &r, [add(0, DELAY, Some("a"))]);
    rejected(&mut d, &r, route(&["a"])); // nothing before it
    run(
        &mut d,
        &r,
        [add(1, DELAY, Some("b")), connect("a:out_l", "b:in_l")],
    );
    rejected(&mut d, &r, route(&["b"])); // already cabled
    rejected(&mut d, &r, route(&["ghost"]));
    rejected(&mut d, &r, route(&[]));
}

#[test]
fn auto_route_of_several_devices_is_one_step() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("s1")), add(1, SYNTH, Some("s2"))],
    );
    for k in ["s1", "s2"] {
        let ch = channel_of(&d, k);
        run(
            &mut d,
            &r,
            [cc(ConsoleCommand::RemoveChannel { channel: ch })],
        );
    }
    one_step(&mut d, &r, route(&["s1", "s2"]));
    assert_eq!(d.doc().console.channels.len(), 2);
    assert_eq!(d.doc().song.tracks.len(), 2);
}

#[test]
fn auto_route_ignores_a_channel_and_a_master() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    rejected(&mut d, &r, route(&[&ch.0]));
}

// ------------------------------------------------------------------ DisconnectDevices

#[test]
fn disconnect_devices_cuts_every_cable_of_every_device_in_one_step() {
    let r = registry();
    let mut d = chain_doc(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices {
            keys: vec![key("synth"), key("delay")],
        }),
    );
    assert_eq!(cables_of(&d, "synth"), 0);
    assert_eq!(cables_of(&d, "delay"), 0);
    assert!(
        has_cable(&d, "mixer:master_l", "interface:out_1"),
        "others stay"
    );
}

fn chain_doc(r: &Registry) -> Document {
    chain(r)
}

#[test]
fn disconnect_devices_rejects_nothing_to_cut_and_unknown_keys() {
    let r = registry();
    let mut d = three_devices(&r);
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices {
            keys: vec![key("synth")],
        }),
    );
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices { keys: vec![] }),
    );
    let mut d = chain_doc(&r);
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices {
            keys: vec![key("ghost")],
        }),
    );
    // A device without cables next to one with cables is fine.
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("loose"))]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices {
            keys: vec![key("loose"), key("delay")],
        }),
    );
}

// ------------------------------------------------------------------------ DeleteDevices

fn delete(keys: &[&str]) -> Command {
    wf(WorkflowCommand::DeleteDevices {
        keys: keys.iter().map(|k| key(k)).collect(),
    })
}

/// synth -> delay -> reverb -> mixer ch1.
fn serial(r: &Registry) -> Document {
    let mut d = empty();
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, DELAY, Some("delay")),
            add(2, "atomik.reverb", Some("verb")),
            add(3, MIXER, Some("mixer")),
        ],
    );
    // The synth's automatic channel sits after it; remove it to keep the chain by hand.
    let ch = channel_of(&d, "synth");
    run(
        &mut d,
        r,
        [cc(ConsoleCommand::RemoveChannel { channel: ch })],
    );
    run(
        &mut d,
        r,
        [
            connect("synth:out_l", "delay:in_l"),
            connect("synth:out_r", "delay:in_r"),
            connect("delay:out_l", "verb:in_l"),
            connect("delay:out_r", "verb:in_r"),
            connect("verb:out_l", "mixer:ch1_in_l"),
            connect("verb:out_r", "mixer:ch1_in_r"),
        ],
    );
    d
}

#[test]
fn deleting_a_middle_effect_reconnects_its_neighbours() {
    let r = reg();
    let mut d = serial(&r);
    one_step(&mut d, &r, delete(&["delay"]));
    assert!(d.doc().device(&key("delay")).is_none());
    assert!(has_cable(&d, "synth:out_l", "verb:in_l"));
    assert!(has_cable(&d, "synth:out_r", "verb:in_r"));
    assert!(has_cable(&d, "verb:out_l", "mixer:ch1_in_l"));
}

#[test]
fn deleting_two_neighbours_bridges_end_to_end() {
    let r = reg();
    let mut d = serial(&r);
    one_step(&mut d, &r, delete(&["delay", "verb"]));
    assert!(has_cable(&d, "synth:out_l", "mixer:ch1_in_l"));
    assert!(has_cable(&d, "synth:out_r", "mixer:ch1_in_r"));
}

#[test]
fn deleting_with_no_audio_neighbours_is_a_plain_removal() {
    let r = reg();
    let mut a = serial(&r);
    let mut b = Document::new(a.doc().clone());
    a.apply(delete(&["synth"]), &r).unwrap();
    b.apply(
        wf(WorkflowCommand::RemoveDevices {
            keys: vec![key("synth")],
        }),
        &r,
    )
    .unwrap();
    assert_eq!(text(&a), text(&b), "an instrument has no input to bridge");
}

#[test]
fn a_side_without_an_input_is_not_bridged_and_a_mixer_is_removed_plainly() {
    let r = reg();
    // delay: only the left input is cabled, both outputs are.
    let mut d = serial(&r);
    run(
        &mut d,
        &r,
        [Command::Disconnect {
            to: sock("delay:in_r"),
        }],
    );
    one_step(&mut d, &r, delete(&["delay"]));
    assert!(
        has_cable(&d, "synth:out_l", "verb:in_l"),
        "the left side is bridged"
    );
    assert!(
        !d.doc().cables.iter().any(|c| c.to == sock("verb:in_r")),
        "the right side has no input to bridge"
    );
    // A mixer passes nothing through: deleting it leaves no joined cable.
    let mut d = serial(&r);
    one_step(&mut d, &r, delete(&["mixer"]));
    assert!(!d.doc().cables.iter().any(|c| c.from.device == key("verb")));
}

#[test]
fn deleting_an_insert_keeps_the_chain_closed_and_the_last_one_leaves_it_empty() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    run(
        &mut d,
        &r,
        [effect_on(DELAY, &ch.0), effect_on("atomik.reverb", &ch.0)],
    );
    let both = names(&d, &r, &ch);
    assert_eq!(both.len(), 2);
    one_step(&mut d, &r, delete(&[&both[0]]));
    assert_eq!(names(&d, &r, &ch), vec![both[1].clone()]);
    // The last insert goes: nothing is cabled between send and return (an empty loop).
    one_step(&mut d, &r, delete(&[&both[1]]));
    assert!(names(&d, &r, &ch).is_empty());
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| c.from.device == ch && c.from.socket.starts_with("insert_send"))
    );
}

#[test]
fn deleting_a_return_effect_clears_the_slot_without_a_loop() {
    let r = reg();
    let mut d = with_synth(&r);
    run(&mut d, &r, [add(usize::MAX, "atomik.reverb", Some("verb"))]);
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("verb")),
        })],
    );
    let master = d.doc().console.master.clone().unwrap();
    one_step(&mut d, &r, delete(&["verb"]));
    assert!(d.doc().console.returns[0].is_none());
    assert!(!has_cable(
        &d,
        &format!("{master}:fx_send1_l"),
        &format!("{master}:return1_l")
    ));
}

#[test]
fn deleting_a_channel_or_an_instrument_behaves_as_remove_devices() {
    let r = reg();
    let mut a = with_synth(&r);
    let mut b = Document::new(a.doc().clone());
    a.apply(delete(&["synth"]), &r).unwrap();
    b.apply(
        wf(WorkflowCommand::RemoveDevices {
            keys: vec![key("synth")],
        }),
        &r,
    )
    .unwrap();
    assert_eq!(text(&a), text(&b));
}

#[test]
fn delete_devices_rejects_nothing_and_unknown_keys_and_changes_nothing() {
    let r = reg();
    let mut d = serial(&r);
    rejected(&mut d, &r, delete(&[]));
    rejected(&mut d, &r, delete(&["ghost"]));
}

#[test]
fn deleted_and_bridged_documents_load_back() {
    let r = reg();
    let mut d = serial(&r);
    run(&mut d, &r, [delete(&["delay"])]);
    let t = text(&d);
    let loaded = load_project(&t, &r).expect("loads");
    assert_eq!(atomik_model::save_project(&loaded), t);
}

// ---------------------------------------------------------- bridging rules in detail

#[test]
fn remove_devices_never_bridges_while_delete_devices_does() {
    let r = reg();
    let mut plain = serial(&r);
    plain
        .apply(
            wf(WorkflowCommand::RemoveDevices {
                keys: vec![key("delay")],
            }),
            &r,
        )
        .unwrap();
    assert!(
        !plain.doc().cables.iter().any(|c| c.to == sock("verb:in_l")),
        "RemoveDevices leaves the neighbours unconnected"
    );
    let mut bridged = serial(&r);
    bridged.apply(delete(&["delay"]), &r).unwrap();
    assert!(has_cable(&bridged, "synth:out_l", "verb:in_l"));
    assert_ne!(text(&plain), text(&bridged));
}

#[test]
fn a_container_at_the_top_level_is_bridged_like_an_effect() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, CONTAINER, Some("box")),
            add(2, DELAY, Some("delay")),
        ],
    );
    let ch = channel_of(&d, "synth");
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::RemoveChannel { channel: ch })],
    );
    run(
        &mut d,
        &r,
        [
            connect("synth:out_l", "box:in_l"),
            connect("synth:out_r", "box:in_r"),
            connect("box:out_l", "delay:in_l"),
            connect("box:out_r", "delay:in_r"),
        ],
    );
    one_step(&mut d, &r, delete(&["box"]));
    assert!(d.doc().device(&key("box")).is_none());
    assert!(has_cable(&d, "synth:out_l", "delay:in_l"));
    assert!(has_cable(&d, "synth:out_r", "delay:in_r"));
}

#[test]
fn every_destination_of_a_fanned_out_output_is_bridged() {
    let r = reg();
    let mut d = serial(&r);
    // The delay's left output also feeds the mixer's second channel.
    run(&mut d, &r, [connect("delay:out_l", "mixer:ch2_in_l")]);
    one_step(&mut d, &r, delete(&["delay"]));
    assert!(has_cable(&d, "synth:out_l", "verb:in_l"));
    assert!(has_cable(&d, "synth:out_l", "mixer:ch2_in_l"));
    assert!(has_cable(&d, "synth:out_r", "verb:in_r"));
}

#[test]
fn a_single_connected_pair_is_bridged_whatever_the_socket_names() {
    let r = reg();
    // Only the left side is cabled through the delay (in_l -> out_l): one input, one output.
    let mut d = serial(&r);
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("delay:in_r"),
            },
            Command::Disconnect {
                to: sock("verb:in_r"),
            },
        ],
    );
    one_step(&mut d, &r, delete(&["delay"]));
    assert!(has_cable(&d, "synth:out_l", "verb:in_l"));
    assert_eq!(cables_of(&d, "synth"), 1, "nothing on the right side");
}

#[test]
fn a_device_that_loops_back_into_its_source_gets_no_bridge() {
    let r = reg();
    // delay -> verb and verb -> delay is refused as a loop by the cable rules, so the closest
    // real case is the insert loop of a channel: deleting the lone insert leaves the loop empty
    // (tested above) and never cables the send to itself.
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    run(&mut d, &r, [effect_on(DELAY, &ch.0)]);
    let before = d.doc().cables.len();
    let fx = names(&d, &r, &ch).remove(0);
    one_step(&mut d, &r, delete(&[&fx]));
    assert!(d.doc().cables.len() < before);
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| !(c.from.device == ch && c.to.device == ch))
    );
}

#[test]
fn a_note_only_utility_is_removed_without_audio_bridging() {
    let r = reg();
    let mut a = serial(&r);
    run(&mut a, &r, [add(usize::MAX, ARP, Some("arp"))]);
    let mut b = Document::new(a.doc().clone());
    a.apply(delete(&["arp"]), &r).unwrap();
    b.apply(
        wf(WorkflowCommand::RemoveDevices {
            keys: vec![key("arp")],
        }),
        &r,
    )
    .unwrap();
    assert_eq!(text(&a), text(&b));
}

#[test]
fn bridging_a_removal_the_cable_rules_refuse_leaves_the_removal_plain() {
    let r = reg();
    // The delay also feeds a second mixer channel; deleting both effects joins the synth to
    // every destination, and the removal succeeds whatever the joins.
    let mut d = serial(&r);
    run(&mut d, &r, [connect("delay:out_l", "mixer:ch2_in_l")]);
    let n = d.doc().devices.len();
    d.apply(delete(&["delay", "verb"]), &r).unwrap();
    assert_eq!(d.doc().devices.len(), n - 2);
    assert!(has_cable(&d, "synth:out_l", "mixer:ch1_in_l"));
    assert!(has_cable(&d, "synth:out_l", "mixer:ch2_in_l"));
}

// ----------------------------------------------- arming details killed by mutation

#[test]
fn the_new_track_is_appended_after_the_existing_tracks_and_armed() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add_track("a"),
            add_track("b"),
        ],
    );
    let before: Vec<String> = d
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect();
    one_step(&mut d, &r, instrument(SYNTH));
    let after: Vec<String> = d
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect();
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(
        &after[..before.len()],
        &before[..],
        "existing tracks keep their order"
    );
    let new = d.doc().song.tracks.last().unwrap();
    assert!(new.armed);
    assert!(
        !before.contains(&new.device.0),
        "the last track is the new one"
    );
}

#[test]
fn disarming_keeps_the_mute_and_solo_flags_of_the_track() {
    let r = reg();
    let mut d = empty();
    run(&mut d, &r, [add(0, SYNTH, Some("a")), add_track("a")]);
    d.apply(
        Command::Song(SongCommand::SetTrackFlags {
            device: key("a"),
            muted: true,
            solo: true,
            armed: true,
        }),
        &r,
    )
    .unwrap();
    let options = CreateOptions {
        disarm: vec![key("a")],
        ..CreateOptions::default()
    };
    one_step(&mut d, &r, instrument_with(SYNTH, options));
    let t = d.doc().song.track(&key("a")).unwrap();
    assert!(t.muted && t.solo && !t.armed, "only the arming changed");
}

#[test]
fn an_effect_on_a_return_whose_cable_was_cut_is_only_added() {
    let r = reg();
    let mut d = with_synth(&r);
    run(&mut d, &r, [add(usize::MAX, "atomik.reverb", Some("verb"))]);
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("verb")),
        })],
    );
    let master = d.doc().console.master.clone().unwrap();
    run(
        &mut d,
        &r,
        [Command::Disconnect {
            to: sock(&format!("{master}:return1_l")),
        }],
    );
    let cables = d.doc().cables.len();
    one_step(&mut d, &r, effect_on(DELAY, "verb"));
    let new = d
        .doc()
        .devices
        .iter()
        .find(|x| x.key.0.starts_with("stereo-delay"))
        .expect("the new effect")
        .key
        .clone();
    assert_eq!(cables_of(&d, &new.0), 0, "an unrouted add: no cable");
    assert_eq!(d.doc().cables.len(), cables, "no cable changed");
    let order: Vec<String> = d.doc().devices.iter().map(|x| x.key.0.clone()).collect();
    let i = order.iter().position(|k| k == "verb").unwrap();
    assert_eq!(order[i + 1], new.0, "still right after the selection");
}

// ----------------------------------------------- auto-route details killed by mutation

fn without_channel_and_track(r: &Registry, k: &str) -> Document {
    let mut d = empty();
    run(&mut d, r, [add(0, SYNTH, Some(k))]);
    let ch = channel_of(&d, k);
    run(
        &mut d,
        r,
        [cc(ConsoleCommand::RemoveChannel { channel: ch })],
    );
    d
}

#[test]
fn auto_route_with_skip_track_gives_the_channel_and_no_track() {
    let r = reg();
    let mut d = without_channel_and_track(&r, "s");
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::AutoRoute {
            devices: vec![key("s")],
            options: CreateOptions {
                skip_track: true,
                ..CreateOptions::default()
            },
        }),
    );
    assert!(d.doc().console.channel_of(&key("s")).is_some());
    assert!(d.doc().song.tracks.is_empty(), "Alt: no track");
}

#[test]
fn auto_route_cables_a_loose_effect_after_an_instrument_that_has_no_channel() {
    // Decision (2026-10-19): `AutoRoute` and `CreateEffect` agree. `CreateEffect` with
    // a channel-less instrument selected cables the effect after the instrument (the generic
    // device rule); auto-route of a loose effect does the same instead of refusing it as before
    // first built it (the pinned reason then: "nowhere to be routed to").
    let r = reg();
    let mut d = without_channel_and_track(&r, "s");
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("fx"))]);
    one_step(&mut d, &r, route(&["fx"]));
    assert!(has_cable(&d, "s:out_l", "fx:in_l"));
    assert!(has_cable(&d, "s:out_r", "fx:in_r"));
    assert!(
        d.doc().console.channel_of(&key("s")).is_none(),
        "the effect does not give the instrument a channel"
    );
}

#[test]
fn auto_route_and_create_effect_agree_for_an_instrument_that_has_no_channel() {
    let r = reg();
    // CreateEffect with the channel-less instrument selected.
    let mut created = without_channel_and_track(&r, "s");
    run(&mut created, &r, [effect_on(DELAY, "s")]);
    let fx = created
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == DELAY)
        .expect("the effect")
        .key
        .clone();
    // AutoRoute of the same effect added loose.
    let mut routed = without_channel_and_track(&r, "s");
    run(&mut routed, &r, [add(usize::MAX, DELAY, Some(fx.as_str()))]);
    run(&mut routed, &r, [route(&[fx.as_str()])]);
    assert_eq!(created.doc().cables, routed.doc().cables);
}

#[test]
fn auto_route_does_not_cable_a_loose_effect_to_an_instrument_with_cabled_outputs() {
    // The agreement keeps the free-outputs rule: an instrument whose outputs go somewhere
    // already is not a source for a loose effect.
    let r = reg();
    let mut d = without_channel_and_track(&r, "s");
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, MIXER, Some("m")),
            connect("s:out_l", "m:ch1_in_l"),
            add(1, DELAY, Some("fx")),
        ],
    );
    let e = rejected(&mut d, &r, route(&["fx"]));
    assert!(e.to_string().contains("nowhere"), "{e}");
}

#[test]
fn auto_route_skips_a_device_with_one_cabled_output_and_uses_the_one_before_it() {
    let r = reg();
    let mut d = empty();
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("a")),
            add(1, DELAY, Some("b")),
            add(2, DELAY, Some("c")),
            add(3, MIXER, Some("m")),
            connect("b:out_l", "m:ch1_in_l"),
        ],
    );
    one_step(&mut d, &r, route(&["c"]));
    assert!(has_cable(&d, "a:out_l", "c:in_l"));
    assert!(has_cable(&d, "a:out_r", "c:in_r"));
    assert!(!has_cable(&d, "b:out_l", "c:in_l"), "b has a cabled output");
    assert!(!has_cable(&d, "b:out_r", "c:in_r"));
}

#[test]
fn auto_route_picks_the_nearest_channel_before_the_effect() {
    let r = reg();
    let mut d = empty();
    run(&mut d, &r, [instrument(SYNTH), instrument(SYNTH)]);
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("fx"))]);
    let synths: Vec<DeviceKey> = d
        .doc()
        .devices
        .iter()
        .filter(|x| x.type_id == SYNTH)
        .map(|x| x.key.clone())
        .collect();
    assert_eq!(synths.len(), 2);
    one_step(&mut d, &r, route(&["fx"]));
    let first = channel_of(&d, &synths[0].0);
    let second = channel_of(&d, &synths[1].0);
    assert!(names(&d, &r, &first).is_empty());
    assert_eq!(names(&d, &r, &second), vec!["fx".to_string()]);
}

#[test]
fn auto_route_says_why_a_device_is_not_routable() {
    let r = reg();
    let mut d = with_synth(&r);
    let ch = channel_of(&d, "synth");
    let e = rejected(&mut d, &r, route(&[&ch.0]));
    assert!(e.to_string().contains("console"), "{e}");
    run(&mut d, &r, [add(usize::MAX, ARP, Some("arp"))]);
    let e = rejected(&mut d, &r, route(&["arp"]));
    assert!(e.to_string().contains("nothing to route"), "{e}");
}

#[test]
fn disconnect_devices_of_two_scopes_is_rejected_for_the_scope() {
    let r = reg();
    let mut d = container_doc(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DisconnectDevices {
            keys: vec![key("c"), key("c/delay")],
        }),
    );
    assert!(e.to_string().contains("same scope"), "{e}");
}

#[test]
fn bridging_pairs_the_connected_side_with_its_own_output_side() {
    let r = reg();
    // Only the right input of the delay is cabled; both outputs are.
    let mut d = serial(&r);
    run(
        &mut d,
        &r,
        [Command::Disconnect {
            to: sock("delay:in_l"),
        }],
    );
    one_step(&mut d, &r, delete(&["delay"]));
    assert!(has_cable(&d, "synth:out_r", "verb:in_r"));
    assert!(
        !has_cable(&d, "synth:out_r", "verb:in_l"),
        "the right source never feeds the left side"
    );
}
