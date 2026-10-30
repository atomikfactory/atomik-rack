//! Freeze: the frozen state of a track (commands, undo, the guard, the expansion the engine
//! builds from it) and project format 26.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, FREEZE_SUFFIX, Frozen, InsertEffect, ModelError,
    PROJECT_VERSION, SongCommand, SyncOp, clips_span, freeze_key, freeze_tap, frozen_devices,
    insert_chain, load_project, save_project, track_chain,
};
use common::audio::*;
use common::song::*;
use common::*;
use serde_json::Value;

const FRAMES: u64 = 480_000;

/// A synth with its console channel and master, a track with a note clip, and `fx` insert
/// effects (a reverb, then a delay), with assets 1 and 2 in the pool.
fn rack(reg: &Registry, fx: usize) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(register(1, FRAMES), reg).unwrap();
    d.apply(register(2, 96_000), reg).unwrap();
    d.apply(add(0, SYNTH, Some("synth")), reg).unwrap();
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(0, 2 * BAR, vec![note(0, 480, 60, 0.8)]),
        ),
        reg,
    )
    .unwrap();
    for (slot, t) in ["atomik.reverb", "atomik.stereo-delay"]
        .into_iter()
        .take(fx)
        .enumerate()
    {
        d.apply(
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel_of(&d),
                slot,
                effect: Some(InsertEffect::New { type_id: t.into() }),
            }),
            reg,
        )
        .unwrap();
    }
    d
}

fn channel_of(d: &Document) -> atomik_model::DeviceKey {
    d.doc().console.channels[0].channel.clone()
}

fn chain_keys(d: &Document, reg: &Registry) -> Vec<atomik_model::DeviceKey> {
    insert_chain(d.doc(), reg, &channel_of(d))
}

fn render(effects: Vec<atomik_model::DeviceKey>) -> Frozen {
    Frozen {
        start: 0,
        length: 2 * BAR,
        audio: audio(1),
        effects,
    }
}

fn freeze(d: &mut Document, reg: &Registry, effects: Vec<atomik_model::DeviceKey>) {
    d.apply(
        sc(SongCommand::FreezeTrack {
            device: key("synth"),
            frozen: render(effects),
        }),
        reg,
    )
    .unwrap();
}

fn unfreeze(d: &mut Document, reg: &Registry, keep_audio: bool) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(
        sc(SongCommand::UnfreezeTrack {
            device: key("synth"),
            keep_audio,
        }),
        reg,
    )
}

fn frozen_of(d: &Document) -> Option<&Frozen> {
    d.doc().song.track(&key("synth")).unwrap().frozen.as_ref()
}

#[test]
fn the_chain_of_a_track_is_its_channels_insert_effects() {
    let reg = audio_registry();
    let d = rack(&reg, 2);
    let chain = track_chain(d.doc(), &reg, &key("synth")).unwrap();
    assert_eq!(chain.channel, channel_of(&d));
    assert_eq!(chain.effects, chain_keys(&d, &reg));
    assert_eq!(chain.effects.len(), 2);
    let bare = rack(&reg, 0);
    assert!(
        track_chain(bare.doc(), &reg, &key("synth"))
            .unwrap()
            .effects
            .is_empty()
    );
}

#[test]
fn the_span_of_a_track_is_its_clips() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(3 * BAR, BAR, vec![note(0, 480, 62, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    let track = d.doc().song.track(&key("synth")).unwrap();
    assert_eq!(clips_span(track), Some((0, 4 * BAR)));
    let empty = Document::new(atomik_model::empty_doc());
    assert!(empty.doc().song.tracks.is_empty());
}

#[test]
fn freeze_and_unfreeze_are_one_undo_step_each_and_restore_the_document() {
    let reg = audio_registry();
    let mut d = rack(&reg, 1);
    let before = d.doc().clone();
    let fx = chain_keys(&d, &reg);
    let ops = d
        .apply(
            sc(SongCommand::FreezeTrack {
                device: key("synth"),
                frozen: render(fx.clone()),
            }),
            &reg,
        )
        .unwrap();
    assert!(ops.contains(&SyncOp::SongChanged) && ops.contains(&SyncOp::FrozenChanged));
    assert_eq!(frozen_of(&d).unwrap().effects, fx);
    // Nothing else of the track changed.
    let mut thawed = d.doc().clone();
    thawed.song.tracks[0].frozen = None;
    assert_eq!(thawed.song, before.song);
    assert_eq!(thawed.devices, before.devices);
    assert_eq!(thawed.cables, before.cables);

    let ops = unfreeze(&mut d, &reg, false).unwrap();
    assert!(ops.contains(&SyncOp::FrozenChanged));
    assert!(frozen_of(&d).is_none());
    assert_eq!(d.doc().song, before.song, "the original state, exactly");

    d.undo(&reg).unwrap();
    assert!(frozen_of(&d).is_some(), "undo of the unfreeze refreezes");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song, before.song, "undo of the freeze");
    d.redo(&reg).unwrap();
    assert!(frozen_of(&d).is_some());
    d.redo(&reg).unwrap();
    assert!(frozen_of(&d).is_none());
}

#[test]
fn a_freeze_that_cannot_hold_changes_nothing() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    let rejects = |d: &mut Document, command: Command| -> ModelError {
        let before = d.doc().clone();
        let e = d.apply(command, &reg).expect_err("must be rejected");
        assert_eq!(d.doc(), &before);
        e
    };
    let freeze_with = |frozen: Frozen| {
        sc(SongCommand::FreezeTrack {
            device: key("synth"),
            frozen,
        })
    };
    // Not the first effects of the chain.
    rejects(&mut d, freeze_with(render(vec![fx[1].clone()])));
    rejects(
        &mut d,
        freeze_with(render(vec![fx[0].clone(), fx[0].clone()])),
    );
    // An asset that is not in the pool, a bad range, reverse and warp.
    let mut bad = render(vec![]);
    bad.audio = audio(9);
    rejects(&mut d, freeze_with(bad));
    let mut bad = render(vec![]);
    bad.length = 0;
    rejects(&mut d, freeze_with(bad));
    let mut bad = render(vec![]);
    bad.audio.reverse = true;
    rejects(&mut d, freeze_with(bad));
    // A device with no track.
    let e = rejects(
        &mut d,
        sc(SongCommand::FreezeTrack {
            device: key("nope"),
            frozen: render(vec![]),
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)));
    // Twice.
    freeze(&mut d, &reg, vec![]);
    rejects(&mut d, freeze_with(render(vec![])));
    // Not frozen: nothing to unfreeze.
    unfreeze(&mut d, &reg, false).unwrap();
    let e = rejects(
        &mut d,
        sc(SongCommand::UnfreezeTrack {
            device: key("synth"),
            keep_audio: false,
        }),
    );
    assert!(e.to_string().contains("not frozen"));
}

#[test]
fn a_track_without_a_console_channel_cannot_be_frozen() {
    let reg = registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(register(1, FRAMES), &reg).unwrap();
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    let e = d
        .apply(
            sc(SongCommand::FreezeTrack {
                device: key("synth"),
                frozen: render(vec![]),
            }),
            &reg,
        )
        .expect_err("no channel");
    assert!(e.to_string().contains("console channel"), "{e}");
}

#[test]
fn a_frozen_track_and_what_it_freezes_cannot_be_edited() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    freeze(&mut d, &reg, vec![fx[0].clone()]);
    let clip = clip_ids(d.doc())[0];
    let refused = [
        Command::SetParam {
            key: key("synth"),
            param: "cutoff".into(),
            value: 0.5,
            merge: None,
        },
        Command::SetParam {
            key: fx[0].clone(),
            param: "mix".into(),
            value: 0.5,
            merge: None,
        },
        add_clip(
            "synth",
            0,
            notes_clip(4 * BAR, BAR, vec![note(0, 480, 64, 0.5)]),
        ),
        sc(SongCommand::RemoveClip { id: clip }),
        sc(SongCommand::SplitClip { id: clip, at: BAR }),
        sc(SongCommand::AddNoteLane {
            device: key("synth"),
            name: None,
        }),
        sc(SongCommand::RemoveNoteLane {
            device: key("synth"),
            lane: 0,
        }),
        sc(SongCommand::RenameLane {
            device: key("synth"),
            lane: 0,
            name: Some("Lead".into()),
        }),
        sc(SongCommand::SetLaneMuted {
            device: key("synth"),
            lane: 0,
            muted: true,
        }),
        Command::RemoveDevice { key: fx[0].clone() },
        Command::SetTempo(100.0),
        sc(SongCommand::SetTempoMap(vec![
            atomik_model::TempoEvent::step(0, 100.0),
        ])),
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel_of(&d),
            slot: 0,
            effect: None,
        }),
        Command::Console(ConsoleCommand::MoveInsert {
            channel: channel_of(&d),
            from: 0,
            to: 1,
        }),
        // A player on a frozen instrument would cable into it and move its track's entry.
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: ARP.into(),
            target: atomik_model::CreateTarget::Selected(key("synth")),
            options: atomik_model::CreateOptions::default(),
        }),
    ];
    for command in refused {
        let before = d.doc().clone();
        let e = d.apply(command.clone(), &reg).expect_err("a frozen edit");
        assert!(matches!(e, ModelError::Frozen(_)), "{command:?}: {e}");
        assert_eq!(d.doc(), &before);
    }
    let message = d
        .apply(sc(SongCommand::RemoveClip { id: clip }), &reg)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("frozen") && message.contains("unfreeze"),
        "{message}"
    );
    // What is not frozen, and what only looks, still works: the effect after the freeze, the
    // track's flags, the colour, a rename.
    let live = fx[1].clone();
    d.apply(
        Command::SetParam {
            key: live,
            param: "mix".into(),
            value: 0.25,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("synth"),
            muted: true,
            solo: false,
            armed: false,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Rename {
            key: key("synth"),
            name: "Lead".into(),
        },
        &reg,
    )
    .unwrap();
    // Unfrozen again, everything works.
    unfreeze(&mut d, &reg, false).unwrap();
    d.apply(sc(SongCommand::RemoveClip { id: clip }), &reg)
        .unwrap();
}

#[test]
fn removing_the_frozen_instrument_removes_its_track_and_undo_brings_the_freeze_back() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    freeze(&mut d, &reg, vec![]);
    d.apply(Command::RemoveDevice { key: key("synth") }, &reg)
        .unwrap();
    assert!(d.doc().song.track(&key("synth")).is_none());
    d.undo(&reg).unwrap();
    assert!(frozen_of(&d).is_some());
}

#[test]
fn the_expansion_of_a_frozen_instrument_feeds_the_channel_from_the_player() {
    let reg = audio_registry();
    let plain = rack(&reg, 0);
    let before = plain.doc().expand(&reg);
    assert!(before.devices.iter().all(|d| !d.frozen));
    let mut d = plain;
    freeze(&mut d, &reg, vec![]);
    let x = d.doc().expand(&reg);
    // Every device keeps its place (and so its engine id); the player comes last.
    let keys: Vec<_> = x.devices.iter().map(|d| d.key.0.as_str()).collect();
    let mut expected: Vec<_> = before.devices.iter().map(|d| d.key.0.clone()).collect();
    expected.push(format!("synth/{FREEZE_SUFFIX}"));
    assert_eq!(keys, expected);
    assert_eq!(
        freeze_key(&key("synth")).0,
        format!("synth/{FREEZE_SUFFIX}")
    );
    for dev in &x.devices {
        assert_eq!(dev.frozen, dev.key.0 == "synth", "{}", dev.key);
    }
    let player = x.devices.last().unwrap();
    assert_eq!(player.type_id, AUDIO_TRACK);
    assert_eq!(player.params.get("monitor"), Some(&0.0));
    // No cable touches the instrument; the channel's inputs come from the player.
    assert!(
        x.cables
            .iter()
            .all(|c| c.from.device.0 != "synth" && c.to.device.0 != "synth")
    );
    let channel = channel_of(&d);
    for (from, input) in [("out_l", "in_l"), ("out_r", "in_r")] {
        assert!(
            x.cables
                .iter()
                .any(|c| c.from.device == freeze_key(&key("synth"))
                    && c.from.socket == from
                    && c.to.device == channel
                    && c.to.socket == input),
            "{from} -> {input}"
        );
    }
    // The other cables are as they were.
    let kept = |cables: &[atomik_model::Cable]| {
        cables
            .iter()
            .filter(|c| c.from.device.0 != "synth" && c.to.device.0 != "synth")
            .cloned()
            .collect::<Vec<_>>()
    };
    let others: Vec<_> = kept(&before.cables);
    assert!(others.iter().all(|c| x.cables.contains(c)));
    assert_eq!(
        frozen_devices(d.doc()),
        [key("synth")].into_iter().collect()
    );
}

#[test]
fn freezing_through_an_effect_leaves_the_later_effects_live() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    let before = d.doc().expand(&reg);
    // The tap of the unfrozen rack: the first effect's outputs.
    let tap = freeze_tap(&before, &key("synth"), &fx[..1]).unwrap();
    assert_eq!(
        (tap[0].device.clone(), tap[0].socket.as_str()),
        (fx[0].clone(), "out_l")
    );
    assert_eq!(tap[1].socket, "out_r");
    // With no effect the tap is where the instrument feeds the channel.
    let bare = freeze_tap(&before, &key("synth"), &[]).unwrap();
    assert_eq!(bare[0].device, key("synth"));
    assert_eq!(
        (bare[0].socket.as_str(), bare[1].socket.as_str()),
        ("out_l", "out_r")
    );

    freeze(&mut d, &reg, vec![fx[0].clone()]);
    let x = d.doc().expand(&reg);
    for dev in &x.devices {
        assert_eq!(
            dev.frozen,
            dev.key.0 == "synth" || dev.key == fx[0],
            "{}",
            dev.key
        );
    }
    let player = freeze_key(&key("synth"));
    // The second effect is fed by the player; nothing touches the frozen devices.
    for (from, input) in [("out_l", "in_l"), ("out_r", "in_r")] {
        assert!(
            x.cables.iter().any(|c| c.from.device == player
                && c.from.socket == from
                && c.to.device == fx[1]
                && c.to.socket == input),
            "{from} -> {}:{input}",
            fx[1]
        );
    }
    assert!(
        x.cables
            .iter()
            .all(|c| c.from.device != fx[0] && c.to.device != fx[0] && c.to.device.0 != "synth")
    );
    // The effect after the freeze still returns into the channel.
    assert!(
        x.cables
            .iter()
            .any(|c| c.from.device == fx[1] && c.to.device == channel_of(&d))
    );
}

#[test]
fn the_asset_of_a_render_is_referenced_while_frozen_and_while_undo_needs_it() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    assert!(!d.referenced_assets().contains(&hash(1)));
    freeze(&mut d, &reg, vec![]);
    assert!(d.doc().referenced_assets().contains(&hash(1)));
    unfreeze(&mut d, &reg, false).unwrap();
    assert!(
        !d.doc().referenced_assets().contains(&hash(1)),
        "a save would drop it"
    );
    assert!(
        d.referenced_assets().contains(&hash(1)),
        "undo can bring it back, so the file stays until then"
    );
    d.prune_unreferenced_assets();
    assert!(d.doc().assets.contains_key(&hash(1)));
}

#[test]
fn unfreeze_and_keep_the_audio_adds_a_track_with_the_render_in_one_step() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    let before = d.doc().clone();
    freeze(&mut d, &reg, vec![]);
    let frozen_doc = d.doc().clone();
    unfreeze(&mut d, &reg, true).unwrap();
    let doc = d.doc();
    assert!(frozen_of(&d).is_none());
    assert_eq!(doc.song.tracks.len(), before.song.tracks.len() + 1);
    let kept = doc.song.tracks.last().unwrap();
    assert!(kept.is_audio());
    let clip = &kept.lanes[0].clips[0];
    assert_eq!((clip.start, clip.length), (0, 2 * BAR));
    assert!(matches!(&clip.content, atomik_model::ClipContent::Audio(a) if a.asset == hash(1)));
    assert!(doc.device(&kept.device).unwrap().name.contains("(frozen)"));
    // One step undoes both.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &frozen_doc);
}

#[test]
fn a_rendered_track_adds_one_clip_and_mutes_the_originals_in_one_step() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    let original = clip_ids(d.doc())[0];
    let before = d.doc().clone();
    d.apply(
        sc(SongCommand::AddRenderedTrack {
            name: Some("Bounce".into()),
            after: None,
            start: BAR,
            length: BAR,
            audio: audio(2),
            mute: vec![original],
        }),
        &reg,
    )
    .unwrap();
    let doc = d.doc();
    let track = doc.song.tracks.last().unwrap();
    assert!(track.is_audio() && track.frozen.is_none());
    assert_eq!(doc.device(&track.device).unwrap().name, "Bounce");
    assert_eq!(track.lanes[0].clips[0].start, BAR);
    assert!(doc.song.clip(original).unwrap().2.muted);
    assert!(d.last_clip_id().is_some());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
    // Bad content changes nothing.
    let e = d
        .apply(
            sc(SongCommand::AddRenderedTrack {
                name: None,
                after: None,
                start: 0,
                length: BAR,
                audio: audio(9),
                mute: vec![],
            }),
            &reg,
        )
        .expect_err("asset not in the pool");
    assert!(matches!(e, ModelError::MissingAsset(_)));
    assert_eq!(d.doc(), &before);
}

/// The engine gets a device before the routing that names it: every `DeviceAdded` comes before
/// the one `RoutingChanged`, and the song and console follow.
fn assert_new_track_synced(ops: &[SyncOp], added: usize) {
    let at = |f: &dyn Fn(&SyncOp) -> bool| ops.iter().position(f);
    let routing = at(&|o| matches!(o, SyncOp::RoutingChanged)).expect("the routing is rebuilt");
    let last_added = ops
        .iter()
        .rposition(|o| matches!(o, SyncOp::DeviceAdded(_)))
        .expect("the track and its channel are inserted");
    assert_eq!(
        ops.iter()
            .filter(|o| matches!(o, SyncOp::DeviceAdded(_)))
            .count(),
        added,
        "{ops:?}"
    );
    assert!(
        last_added < routing,
        "the routing names devices the engine does not have yet: {ops:?}"
    );
    assert_eq!(
        ops.iter()
            .filter(|o| matches!(o, SyncOp::RoutingChanged))
            .count(),
        1,
        "{ops:?}"
    );
    assert!(ops.contains(&SyncOp::SongChanged) && ops.contains(&SyncOp::ConsoleChanged));
}

#[test]
fn a_rendered_track_is_routed_like_an_added_audio_track() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    let ops = d
        .apply(
            sc(SongCommand::AddRenderedTrack {
                name: Some("Bounce".into()),
                after: None,
                start: 0,
                length: BAR,
                audio: audio(2),
                mute: vec![],
            }),
            &reg,
        )
        .unwrap();
    assert_new_track_synced(&ops, 2);
    let doc = d.doc();
    let track = doc.song.tracks.last().unwrap();
    let channel = doc.console.channel_of(&track.device).expect("a channel");
    // The same wiring as a track that was added the usual way.
    let mut plain = rack(&reg, 0);
    plain
        .apply(sc(SongCommand::AddAudioTrack { name: None }), &reg)
        .unwrap();
    let usual = plain.doc().song.tracks.last().unwrap();
    let usual_channel = plain.doc().console.channel_of(&usual.device).unwrap();
    let wires = |doc: &atomik_model::RackDoc,
                 source: &atomik_model::DeviceKey,
                 ch: &atomik_model::DeviceKey| {
        let mut w: Vec<(String, String)> = doc
            .cables
            .iter()
            .filter(|c| &c.from.device == source || &c.to.device == source || &c.from.device == ch)
            .map(|c| {
                let name = |k: &atomik_model::DeviceKey| {
                    if k == source {
                        "track".to_owned()
                    } else if k == ch {
                        "channel".to_owned()
                    } else {
                        k.0.clone()
                    }
                };
                (
                    format!("{}:{}", name(&c.from.device), c.from.socket),
                    format!("{}:{}", name(&c.to.device), c.to.socket),
                )
            })
            .collect();
        w.sort();
        w
    };
    assert_eq!(
        wires(doc, &track.device, &channel.channel),
        wires(plain.doc(), &usual.device, &usual_channel.channel)
    );
}

#[test]
fn a_rendered_track_sits_after_its_source_in_its_folder_and_colour() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    d.apply(add(0, SYNTH, Some("bass")), &reg).unwrap();
    d.apply(add_track("bass"), &reg).unwrap();
    d.apply(
        sc(SongCommand::GroupTracks {
            devices: vec![key("synth")],
            name: "Leads".into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetTrackColor {
            device: key("synth"),
            color: Some([10, 20, 30]),
        }),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    d.apply(
        sc(SongCommand::AddRenderedTrack {
            name: None,
            after: Some(key("synth")),
            start: 0,
            length: BAR,
            audio: audio(2),
            mute: vec![],
        }),
        &reg,
    )
    .unwrap();
    let doc = d.doc();
    let order: Vec<_> = doc
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.as_str())
        .collect();
    assert_eq!(order[0], "synth");
    assert!(order[1].starts_with("audio-track"), "{order:?}");
    assert_eq!(order[2], "bass");
    let new = key(order[1]);
    let org = doc.song.normalised_org();
    assert_eq!(org.members.get(&new), org.members.get(&key("synth")));
    assert!(org.members.contains_key(&new));
    assert_eq!(org.colors.get(&new), Some(&[10, 20, 30]));
    assert!(doc.console.channel_of(&new).is_some());
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "one undo step removes the track, its channel and its folder entry"
    );
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks.len(), before.song.tracks.len() + 1);
}

#[test]
fn unfreeze_and_keep_the_audio_syncs_the_new_track_after_its_devices() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    freeze(&mut d, &reg, vec![]);
    let ops = unfreeze(&mut d, &reg, true).unwrap();
    let added = ops
        .iter()
        .filter(|o| matches!(o, SyncOp::DeviceAdded(_)))
        .count();
    assert_eq!(added, 2, "{ops:?}");
    let routing = ops
        .iter()
        .position(|o| matches!(o, SyncOp::RoutingChanged))
        .expect("the routing is rebuilt");
    assert!(
        ops.iter()
            .rposition(|o| matches!(o, SyncOp::DeviceAdded(_)))
            .unwrap()
            < routing,
        "{ops:?}"
    );
    let order: Vec<_> = d
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.as_str())
        .collect();
    assert_eq!(order[0], "synth");
    assert!(
        order[1].starts_with("audio-track"),
        "right after the unfrozen track: {order:?}"
    );
}

#[test]
fn a_relink_swaps_the_asset_of_a_render() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    freeze(&mut d, &reg, vec![]);
    d.apply(register(3, FRAMES), &reg).unwrap();
    d.apply(
        Command::RelinkAsset {
            from: hash(1),
            to: hash(3),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(frozen_of(&d).unwrap().audio.asset, hash(3));
    d.undo(&reg).unwrap();
    assert_eq!(frozen_of(&d).unwrap().audio.asset, hash(1));
}

// ---- project format 25 --------------------------------------------------------------------

fn saved(d: &Document) -> String {
    save_project(d.doc())
}

#[test]
fn a_frozen_track_round_trips_canonically() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    freeze(&mut d, &reg, fx.clone());
    assert_eq!(PROJECT_VERSION, 36);
    let text = saved(&d);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 36);
    let track = &v["song"]["tracks"][0];
    assert_eq!(track["frozen"]["start"], 0);
    assert_eq!(track["frozen"]["length"], 2 * BAR);
    assert_eq!(track["frozen"]["effects"][0], fx[0].0);
    assert_eq!(track["frozen"]["audio"]["asset"], hash(1));
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "save, load, save is stable");
    // A project that is not frozen has no field.
    let thawed = rack(&reg, 2);
    assert!(!saved(&thawed).contains("frozen"));
}

#[test]
fn a_project_before_freezing_loads_and_saves_with_only_the_version_changed() {
    let reg = audio_registry();
    let d = rack(&reg, 1);
    let text = saved(&d);
    let old = text.replace("\"version\": 36", "\"version\": 24");
    let loaded = load_project(&old, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    assert_eq!(
        text.replace("\"version\": 36", "\"version\": 24"),
        old,
        "the files differ in the version line alone"
    );
    // A version 24 file knows no frozen state.
    let mut d = rack(&reg, 0);
    freeze(&mut d, &reg, vec![]);
    let old = saved(&d).replace("\"version\": 36", "\"version\": 24");
    let e = load_project(&old, &reg).expect_err("frozen before 25");
    assert!(e.to_string().contains("frozen"), "{e}");
    // And a newer file than this reader is refused.
    let newer = saved(&d).replace("\"version\": 36", "\"version\": 37");
    assert!(load_project(&newer, &reg).is_err());
}

#[test]
fn a_bad_frozen_object_is_refused_with_its_path() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    freeze(&mut d, &reg, vec![fx[0].clone()]);
    let good = saved(&d);
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        f(&mut v["song"]["tracks"][0]["frozen"]);
        load_project(&serde_json::to_string(&v).unwrap(), &reg)
            .expect_err("must be refused")
            .to_string()
    };
    let e = edit(&|f| f["effects"] = serde_json::json!([fx[1].0]));
    assert!(e.contains("song.tracks[0].frozen.effects"), "{e}");
    let e = edit(&|f| f["effects"] = serde_json::json!(["nope"]));
    assert!(e.contains("frozen.effects[0]") && e.contains("nope"), "{e}");
    let e = edit(&|f| f["effects"] = serde_json::json!([fx[0].0, fx[0].0]));
    assert!(e.contains("listed twice"), "{e}");
    let e = edit(&|f| f["length"] = serde_json::json!(0));
    assert!(e.contains("frozen.length"), "{e}");
    let e = edit(&|f| f["audio"]["asset"] = serde_json::json!(hash(9)));
    assert!(e.contains("frozen.audio.asset"), "{e}");
    let e = edit(&|f| f["audio"]["reverse"] = serde_json::json!(true));
    assert!(e.contains("plain audio"), "{e}");
    let e = edit(&|f| f["extra"] = serde_json::json!(1));
    assert!(e.contains("extra"), "{e}");
    let e = edit(&|f| {
        f.as_object_mut().unwrap().remove("audio");
    });
    assert!(e.contains("audio"), "{e}");
}

#[test]
fn the_frozen_playback_of_a_track_is_one_audio_lane_with_the_render() {
    let reg = audio_registry();
    let mut d = rack(&reg, 0);
    assert!(d.doc().song.tracks[0].frozen_playback().is_none());
    freeze(&mut d, &reg, vec![]);
    let t = d.doc().song.tracks[0].frozen_playback().unwrap();
    assert!(t.is_audio() && t.frozen.is_none());
    assert_eq!(t.device, key("synth"));
    let regions = atomik_model::song::flatten_audio(&t, &d.doc().song, d.doc().sample_rate);
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].asset, hash(1));
    assert_eq!(regions[0].song_start_frame, 0);
    // A muted track plays nothing, as any track.
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("synth"),
            muted: true,
            solo: false,
            armed: false,
        }),
        &reg,
    )
    .unwrap();
    let t = d.doc().song.tracks[0].frozen_playback().unwrap();
    assert!(atomik_model::song::flatten_audio(&t, &d.doc().song, d.doc().sample_rate).is_empty());
}

#[test]
fn the_channel_of_a_frozen_track_and_the_frozen_effects_stay_where_they_are() {
    let reg = audio_registry();
    let mut d = rack(&reg, 2);
    let fx = chain_keys(&d, &reg);
    let channel = channel_of(&d);
    freeze(&mut d, &reg, vec![fx[0].clone()]);
    let before = d.doc().clone();
    // Taking the channel away would leave a freeze the file could not be reloaded with.
    for command in [
        Command::Console(ConsoleCommand::RemoveChannel {
            channel: channel.clone(),
        }),
        Command::RemoveDevice {
            key: channel.clone(),
        },
        Command::Workflow(atomik_model::WorkflowCommand::RemoveDevices {
            keys: vec![channel.clone()],
        }),
        // A frozen effect cannot move to another insert loop.
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot: 3,
            effect: Some(InsertEffect::Existing(fx[0].clone())),
        }),
    ] {
        let e = d.apply(command.clone(), &reg).expect_err("refused");
        assert!(matches!(e, ModelError::Frozen(_)), "{command:?}: {e}");
        assert_eq!(d.doc(), &before);
    }
    // Every state a document can reach reloads.
    let text = save_project(d.doc());
    assert_eq!(&load_project(&text, &reg).unwrap(), d.doc());
    // Unfrozen, the channel can go.
    unfreeze(&mut d, &reg, false).unwrap();
    d.apply(
        Command::Console(ConsoleCommand::RemoveChannel { channel }),
        &reg,
    )
    .unwrap();
}
