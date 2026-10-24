//! Freeze and bounce in place in the app core: the render runs on a worker, the result freezes the
//! track (the engine restarts without its devices and with a player), unfreeze and undo bring it
//! back, a render of a track that changed is thrown away, and the queue, the cancel and the menu
//! state. The fixtures are procedural projects in a scratch bundle.

use super::*;
use crate::sink::{RecordingSink, SinkCall, assert_new_devices_routed};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ConsoleCommand, CreateOptions, CreateSource, InsertEffect, Note,
    WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn notes(start: i64, length: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content: ClipContent::Notes(vec![
            Note::new(0, PPQ, 64, 0.8),
            Note::new(PPQ, PPQ, 67, 0.8),
        ]),
    }
}

/// The starter project with a synth `lead` (a clip of two bars), `fx` insert effects on its
/// channel (a reverb, a delay), and a synth `bass`.
fn core_with(fx: usize) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    for k in ["lead", "bass"] {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        c.apply_command(Command::Rename {
            key: key(k),
            name: k[..1].to_uppercase() + &k[1..],
        })
        .unwrap();
        c.apply_command(Command::Song(SongCommand::SetTrackFlags {
            device: key(k),
            muted: false,
            solo: false,
            armed: false,
        }))
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: notes(0, 2 * BAR),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("bass"),
        lane: 0,
        clip: notes(BAR, BAR),
    }))
    .unwrap();
    let channel = c.doc().console.channels[0].channel.clone();
    for (slot, t) in ["atomik.reverb", "atomik.stereo-delay"]
        .into_iter()
        .take(fx)
        .enumerate()
    {
        c.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot,
            effect: Some(InsertEffect::New { type_id: t.into() }),
        }))
        .unwrap();
    }
    c.sink_mut().take();
    c
}

fn select_track(c: &mut AppCore<RecordingSink>, k: &str) {
    c.arrangement_mut().set_selected_track(Some(key(k)));
}

fn frozen(c: &AppCore<RecordingSink>, k: &str) -> Option<atomik_model::Frozen> {
    c.doc().song.track(&key(k)).and_then(|t| t.frozen.clone())
}

fn freeze(c: &mut AppCore<RecordingSink>, k: &str, through: FreezeThrough) {
    select_track(c, k);
    c.freeze_tracks(through);
    c.wait_background();
}

fn inserted(calls: &[SinkCall]) -> Vec<&'static str> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Insert { type_id, .. } => Some(*type_id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_freeze_renders_on_a_worker_then_freezes_the_track_and_restarts_the_engine() {
    let mut c = core_with(1);
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.freeze_busy() && c.is_busy());
    assert!(
        c.freeze_progress()
            .is_some_and(|t| t.starts_with("Freezing Lead")),
        "{:?}",
        c.freeze_progress()
    );
    assert!(
        !c.freeze_menu().can_freeze,
        "no second render while one runs"
    );
    assert!(
        frozen(&c, "lead").is_none(),
        "nothing changes before it is done"
    );
    c.wait_background();
    assert!(!c.freeze_busy() && c.freeze_progress().is_none());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Froze Lead")),
        "{:?}",
        c.last_rejection()
    );
    let f = frozen(&c, "lead").expect("frozen");
    assert_eq!(f.start, 0);
    assert_eq!(f.effects.len(), 1, "the whole chain: the reverb");
    assert!(
        f.length >= 2 * BAR,
        "the clips, and a tail past them: {}",
        f.length
    );
    // The render is in the pool, in the project's bundle, and its peaks are loaded.
    let meta = c.doc().assets.get(&f.audio.asset).expect("registered");
    assert_eq!(meta.channels, 2);
    assert!(c.bundle_root().unwrap().join("Audio").is_dir());
    assert!(c.peaks().contains_key(&f.audio.asset));
    assert!(c.song_assets().contains_key(&f.audio.asset));
    // The engine restarted: the instrument and the reverb are not inserted, the player is.
    let calls = c.sink_mut().take();
    assert!(calls.iter().any(|c| matches!(c, SinkCall::Reset { .. })));
    let devices = inserted(&calls);
    let all = c.doc().expand(c.registry());
    assert_eq!(
        devices.len(),
        all.devices.iter().filter(|d| !d.frozen).count()
    );
    assert_eq!(
        devices
            .iter()
            .filter(|t| **t == "atomik.audio-track")
            .count(),
        1
    );
    assert_eq!(devices.iter().filter(|t| **t == "atomik.reverb").count(), 0);
    assert_eq!(
        devices
            .iter()
            .filter(|t| **t == "atomik.poly-synth")
            .count(),
        1,
        "bass"
    );
    // The song the engine plays: the render on the player, no notes for the frozen synth.
    let song = calls.iter().rev().find_map(|call| match call {
        SinkCall::Song(input) => Some(input),
        _ => None,
    });
    let song = song.expect("the song was submitted again");
    assert_eq!(song.audio.len(), 1, "the render plays");
    assert_eq!(song.tracks.len(), 1, "only the bass has notes left");
}

#[test]
fn a_frozen_project_bounces_like_the_unfrozen_one() {
    let mut c = core_with(0);
    let plain = atomik_render::project::render_doc(
        c.doc(),
        c.registry(),
        &atomik_render::project::ProjectOptions::default(),
    )
    .unwrap();
    freeze(&mut c, "lead", FreezeThrough::Track);
    assert!(frozen(&c, "lead").is_some(), "{:?}", c.last_rejection());
    let after = atomik_render::project::render_doc_in(
        c.doc(),
        c.registry(),
        &atomik_render::project::ProjectOptions::default(),
        c.bundle_root(),
    )
    .unwrap();
    assert!(plain.channels[0].iter().any(|x| x.abs() > 0.01));
    assert_eq!(
        plain.channels, after.channels,
        "bit for bit, no tail to lose"
    );
}

#[test]
fn unfreeze_restores_the_track_exactly_and_undo_and_redo_move_between_the_states() {
    let mut c = core_with(1);
    let before = c.doc().song.clone();
    freeze(&mut c, "lead", FreezeThrough::Track);
    let frozen_state = c.doc().clone();
    assert!(frozen(&c, "lead").is_some());
    assert!(c.freeze_menu().can_unfreeze);

    c.unfreeze_tracks(false);
    assert!(frozen(&c, "lead").is_none());
    assert_eq!(c.doc().song, before, "exactly the song it was");
    let calls = c.sink_mut().take();
    assert!(calls.iter().any(|c| matches!(c, SinkCall::Reset { .. })));
    assert!(
        inserted(&calls).contains(&"atomik.reverb"),
        "the effect is back"
    );

    c.undo();
    assert_eq!(c.doc(), &frozen_state, "undo of the unfreeze freezes again");
    c.undo();
    assert_eq!(c.doc().song, before, "undo of the freeze");
    c.redo();
    assert!(frozen(&c, "lead").is_some());

    // Nothing selected that is frozen: a message.
    select_track(&mut c, "bass");
    c.unfreeze_tracks(false);
    assert!(c.last_rejection().unwrap().contains("frozen track"));
}

#[test]
fn unfreeze_and_keep_the_audio_leaves_the_render_on_a_new_track() {
    let mut c = core_with(0);
    freeze(&mut c, "lead", FreezeThrough::Track);
    let asset = frozen(&c, "lead").unwrap().audio.asset;
    let tracks = c.doc().song.tracks.len();
    c.unfreeze_tracks(true);
    assert!(frozen(&c, "lead").is_none());
    assert_eq!(c.doc().song.tracks.len(), tracks + 1);
    let kept = &c.doc().song.tracks[1];
    assert!(kept.is_audio(), "right after the unfrozen track");
    assert!(matches!(&kept.lanes[0].clips[0].content, ClipContent::Audio(a) if a.asset == asset));
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), tracks);
    assert!(frozen(&c, "lead").is_some(), "one step undoes both");
}

#[test]
fn freezing_up_to_the_selected_device_leaves_the_later_effects_live() {
    let mut c = core_with(2);
    let fx = track_chain_of(&c, "lead");
    // The rack's selection says where to stop.
    c.view_mut().set_selected(Some(fx[0].clone()));
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::SelectedDevice);
    c.wait_background();
    let f = frozen(&c, "lead").expect("frozen through the reverb");
    assert_eq!(f.effects, vec![fx[0].clone()]);
    let devices = inserted(&c.sink_mut().take());
    assert!(
        devices.contains(&"atomik.stereo-delay"),
        "the delay stays live"
    );
    assert!(!devices.contains(&"atomik.reverb"));

    // The instrument itself: nothing but it is frozen.
    let mut c = core_with(2);
    c.view_mut().set_selected(Some(key("lead")));
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::SelectedDevice);
    c.wait_background();
    assert!(frozen(&c, "lead").unwrap().effects.is_empty());

    // A device that is not on the track's chain, and no selection, say so.
    let mut c = core_with(1);
    select_track(&mut c, "lead");
    c.view_mut().set_selected(Some(key("bass")));
    c.freeze_tracks(FreezeThrough::SelectedDevice);
    assert!(c.last_rejection().unwrap().contains("not an insert effect"));
    c.view_mut().set_selected(None);
    c.freeze_tracks(FreezeThrough::SelectedDevice);
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("Select the instrument")
    );
    assert!(!c.freeze_busy());
}

fn track_chain_of(c: &AppCore<RecordingSink>, k: &str) -> Vec<DeviceKey> {
    atomik_model::track_chain(c.doc(), c.registry(), &key(k))
        .unwrap()
        .effects
}

#[test]
fn a_render_of_a_track_that_changed_meanwhile_is_thrown_away() {
    let mut c = core_with(1);
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.freeze_busy());
    // An edit of the track before the render is applied.
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: notes(3 * BAR, BAR),
    }))
    .unwrap();
    c.wait_background();
    assert!(frozen(&c, "lead").is_none());
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("changed while it was rendered"),
        "{:?}",
        c.last_rejection()
    );
    // An edit of another track does not matter.
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("bass"),
        muted: true,
        solo: false,
        armed: false,
    }))
    .unwrap();
    c.wait_background();
    assert!(frozen(&c, "lead").is_some());
}

#[test]
fn a_render_can_be_cancelled_and_the_queue_runs_in_order() {
    let mut c = core_with(0);
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    c.cancel_freeze();
    c.wait_background();
    assert!(frozen(&c, "lead").is_none());
    assert!(c.last_rejection().unwrap().contains("cancelled"));
    assert!(!c.freeze_busy());
    c.cancel_freeze();
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("Nothing is being frozen")
    );

    // Two tracks: one render after the other, each one undo step.
    c.arrangement_mut().set_selected_clips(&[]);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    c.arrangement_mut().toggle_selected_track(&key("bass"));
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.freeze_progress().unwrap().contains("(+1 waiting)"));
    c.wait_background();
    assert!(frozen(&c, "lead").is_some() && frozen(&c, "bass").is_some());
    c.undo();
    assert!(frozen(&c, "lead").is_some() && frozen(&c, "bass").is_none());
}

#[test]
fn what_cannot_be_frozen_is_said_and_changes_nothing() {
    let mut c = core_with(0);
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.last_rejection().unwrap().contains("Select a track"));
    // A track without clips.
    let empty = {
        let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }))
        .unwrap();
        c
    };
    let mut empty = empty;
    select_track(&mut empty, "lead");
    empty.freeze_tracks(FreezeThrough::Track);
    assert!(empty.last_rejection().unwrap().contains("no clips"));
    assert!(!empty.freeze_busy());
    // Already frozen.
    freeze(&mut c, "lead", FreezeThrough::Track);
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.last_rejection().unwrap().contains("frozen already"));
}

#[test]
fn a_frozen_track_refuses_edits_with_a_message_that_says_to_unfreeze() {
    let mut c = core_with(0);
    freeze(&mut c, "lead", FreezeThrough::Track);
    let before = c.doc().clone();
    let e = c
        .apply_command(Command::SetParam {
            key: key("lead"),
            param: "filter.cutoff".into(),
            value: 500.0,
            merge: None,
        })
        .unwrap_err();
    assert!(e.to_string().contains("unfreeze"), "{e}");
    assert!(c.last_rejection().unwrap().contains("frozen"));
    assert_eq!(c.doc(), &before);
}

#[test]
fn the_tail_option_decides_how_much_of_the_decay_is_kept() {
    // A clip that ends with the notes, so the reverb has a tail to keep.
    let short_clip = |c: &mut AppCore<RecordingSink>| {
        let id = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
        c.apply_command(Command::Song(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 2 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }))
        .unwrap();
    };
    let mut with_tail = core_with(1);
    short_clip(&mut with_tail);
    assert!(with_tail.freeze_tail() && with_tail.freeze_menu().tail);
    freeze(&mut with_tail, "lead", FreezeThrough::Track);
    let long = frozen(&with_tail, "lead").unwrap();

    let mut without = core_with(1);
    short_clip(&mut without);
    without.toggle_freeze_tail();
    assert!(!without.freeze_tail() && !without.freeze_menu().tail);
    freeze(&mut without, "lead", FreezeThrough::Track);
    let short = frozen(&without, "lead").unwrap();
    assert_eq!(short.length, 2 * PPQ, "exactly the clip");
    assert!(
        long.length > short.length + PPQ,
        "{} > {}",
        long.length,
        short.length
    );
    assert!(
        with_tail.last_rejection().unwrap().contains("s of tail"),
        "{:?}",
        with_tail.last_rejection()
    );
}

#[test]
fn bounce_in_place_adds_a_new_track_and_leaves_the_originals() {
    let mut c = core_with(1);
    let lead_clip = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    let tracks = c.doc().song.tracks.len();
    c.arrangement_mut().set_selected_clips(&[lead_clip]);
    assert!(c.freeze_menu().can_bounce);
    c.bounce_in_place(false);
    assert!(
        c.freeze_progress()
            .is_some_and(|t| t.starts_with("Bouncing Lead")),
        "{:?}",
        c.freeze_progress()
    );
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), tracks + 1);
    let new = &c.doc().song.tracks[1];
    assert!(new.is_audio() && new.frozen.is_none(), "right after Lead");
    assert_eq!(c.doc().device(&new.device).unwrap().name, "Lead (bounce)");
    let clip = &new.lanes[0].clips[0];
    assert_eq!(clip.start, 0);
    assert!(
        c.doc()
            .song
            .clip(lead_clip)
            .is_some_and(|(_, _, c)| !c.muted),
        "originals stay"
    );
    assert!(frozen(&c, "lead").is_none(), "nothing is frozen");
    assert_eq!(
        c.arrangement().selected_clips(),
        vec![clip.id],
        "the new clip is selected"
    );
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), tracks, "one undo step");
}

#[test]
fn bounce_in_place_and_mute_originals_mutes_them_in_the_same_step() {
    let mut c = core_with(0);
    let lead_clip = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[lead_clip]);
    c.bounce_in_place(true);
    c.wait_background();
    assert!(c.doc().song.clip(lead_clip).unwrap().2.muted);
    c.undo();
    assert!(!c.doc().song.clip(lead_clip).unwrap().2.muted);
}

#[test]
fn bounce_in_place_takes_the_clips_of_each_track_into_one_clip_per_track() {
    let mut c = core_with(0);
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: notes(3 * BAR, BAR),
    }))
    .unwrap();
    let lead: Vec<ClipId> = c.doc().song.track(&key("lead")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    let bass = c.doc().song.track(&key("bass")).unwrap().lanes[0].clips[0].id;
    let mut all = lead.clone();
    all.push(bass);
    let tracks = c.doc().song.tracks.len();
    c.arrangement_mut().set_selected_clips(&all);
    c.bounce_in_place(false);
    c.wait_background();
    assert_eq!(
        c.doc().song.tracks.len(),
        tracks + 2,
        "a new track for each source track"
    );
    let first = c.doc().song.tracks.iter().find(|t| t.is_audio()).unwrap();
    let clip = &first.lanes[0].clips[0];
    assert_eq!(clip.start, 0, "from the first clip of the track");
    assert!(
        clip.length >= 4 * BAR,
        "to the end of the last one: {}",
        clip.length
    );
}

#[test]
fn bounce_in_place_refuses_what_it_cannot_bounce() {
    let mut c = core_with(0);
    c.bounce_in_place(false);
    assert!(c.last_rejection().unwrap().contains("Select the clips"));
    // A frozen track's clips are hidden: unfreeze first.
    let clip = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    freeze(&mut c, "lead", FreezeThrough::Track);
    c.arrangement_mut().set_selected_clips(&[clip]);
    c.bounce_in_place(false);
    assert!(c.last_rejection().unwrap().contains("frozen"));
    assert!(!c.freeze_busy());
}

#[test]
fn the_menu_state_follows_the_selection_and_the_render() {
    let mut c = core_with(1);
    let none = c.freeze_menu();
    assert!(!none.can_freeze && !none.can_unfreeze && !none.can_bounce && !none.busy);
    select_track(&mut c, "lead");
    assert!(c.freeze_menu().can_freeze);
    assert!(!c.freeze_menu().can_unfreeze);
    c.freeze_tracks(FreezeThrough::Track);
    let busy = c.freeze_menu();
    assert!(busy.busy && !busy.can_freeze);
    c.wait_background();
    select_track(&mut c, "lead");
    let done = c.freeze_menu();
    assert!(!done.busy && !done.can_freeze && done.can_unfreeze);
}

#[test]
fn opening_another_project_drops_the_render() {
    let mut c = core_with(0);
    select_track(&mut c, "lead");
    c.freeze_tracks(FreezeThrough::Track);
    assert!(c.freeze_busy());
    c.new_document();
    c.wait_background();
    assert!(!c.freeze_busy());
    assert!(c.doc().song.tracks.iter().all(|t| t.frozen.is_none()));
}

#[test]
fn a_normally_added_audio_track_is_routed_after_its_devices_arrive() {
    let mut c = core_with(0);
    c.sink_mut().take();
    c.add_audio_track();
    assert_new_devices_routed(&c.sink_mut().take());
}

#[test]
fn a_bounced_track_is_connected_to_the_engine_and_heard_alone() {
    let mut c = core_with(1);
    let lead_clip = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[lead_clip]);
    c.sink_mut().take();
    c.bounce_in_place(true);
    c.wait_background();
    assert_new_devices_routed(&c.sink_mut().take());
    // Right after its source, named and routed like any audio track.
    let order: Vec<_> = c
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.clone())
        .collect();
    assert_eq!(order[0], key("lead"));
    let new = order[1].clone();
    assert!(c.doc().song.track(&new).unwrap().is_audio());
    assert!(c.doc().console.channel_of(&new).is_some());
    assert_eq!(c.doc().device(&new).unwrap().name, "Lead (bounce)");
    // The original is muted: the first bar is the bounce alone, and it is not silent.
    let out = atomik_render::project::render_doc_in(
        c.doc(),
        c.registry(),
        &atomik_render::project::ProjectOptions::default(),
        c.bundle_root(),
    )
    .unwrap();
    let bar = c.doc().sample_rate as usize * 2;
    let peak = out.channels[0][..bar]
        .iter()
        .fold(0.0_f32, |a, x| a.max(x.abs()));
    assert!(peak > 0.05, "the bounce is audible in the project: {peak}");
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), 2);
    assert!(
        c.doc().console.channel_of(&new).is_none(),
        "undo removes the channel too"
    );
}

#[test]
fn unfreeze_and_keep_the_audio_connects_the_new_track_to_the_engine() {
    let mut c = core_with(0);
    freeze(&mut c, "lead", FreezeThrough::Track);
    c.sink_mut().take();
    c.unfreeze_tracks(true);
    assert_new_devices_routed(&c.sink_mut().take());
    let order: Vec<_> = c
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.clone())
        .collect();
    assert_eq!(order[0], key("lead"));
    assert!(c.doc().song.track(&order[1]).unwrap().is_audio());
}
