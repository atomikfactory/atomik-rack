//! The clipboard and the multi-device and clip commands in the app core: copy, cut, paste and duplicate for devices, tracks, clips, notes and
//! automation points, the paste position rules and the repeated-paste advance, the clipboard
//! surviving New and Open, selection of what a paste created, and every operation as one undo
//! step.

use super::*;
use crate::clipboard::ClipboardKind;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Note, SocketRef,
    SongCommand, WorkflowCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    // The rack as it looked with the console devices hidden.
    c.view_mut().set_show_console_devices(false);
    c
}

fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn song(c: &mut AppCore<RecordingSink>, s: SongCommand) {
    c.apply_command(Command::Song(s)).unwrap();
}

fn cable(from: (&str, &str), to: (&str, &str)) -> Command {
    Command::Connect(Cable {
        from: SocketRef {
            device: key(from.0),
            socket: from.1.into(),
        },
        to: SocketRef {
            device: key(to.0),
            socket: to.1.into(),
        },
    })
}

fn keys(c: &AppCore<RecordingSink>) -> Vec<String> {
    c.doc().devices.iter().map(|d| d.key.0.clone()).collect()
}

fn instruments(c: &AppCore<RecordingSink>) -> Vec<String> {
    c.doc()
        .devices
        .iter()
        .filter(|d| !atomik_model::console::is_console_type(&d.type_id))
        .map(|d| d.key.0.clone())
        .collect()
}

fn select_devices(c: &mut AppCore<RecordingSink>, ks: &[&str]) {
    let ks: Vec<DeviceKey> = ks.iter().map(|k| key(k)).collect();
    c.after_view_change();
    c.view_mut().set_selection(&ks, None);
}

fn note_clip(start: i64, length: i64, notes: &[(i64, u8)]) -> Clip {
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
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, k)| Note::new(tick, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

/// Synth `a` with a track holding one note clip (id 1, bar 1 to 2, three notes).
fn with_clip() -> AppCore<RecordingSink> {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    song(&mut c, SongCommand::AddTrack { device: key("a") });
    song(
        &mut c,
        SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, BAR, &[(0, 60), (PPQ, 62), (2 * PPQ, 64)]),
        },
    );
    c
}

fn command(c: &mut AppCore<RecordingSink>, pane: Pane, id: &str) {
    let _ = c.pane_command(pane, id, None);
}

fn clip_starts(c: &AppCore<RecordingSink>) -> Vec<(u64, i64)> {
    let mut v: Vec<(u64, i64)> = c
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter())
        .map(|k| (k.id.0, k.start))
        .collect();
    v.sort();
    v
}

fn saved(c: &AppCore<RecordingSink>) -> String {
    atomik_model::save_project(c.doc())
}

// ------------------------------------------------------------------ devices

#[test]
fn copy_and_paste_devices_keeps_inner_cables_with_fresh_keys_and_selects_the_copies() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    c.apply_command(cable(("a", "out_l"), ("d", "in_l")))
        .unwrap();
    let cables_before = c.doc().cables.len();
    select_devices(&mut c, &["a", "d"]);
    command(&mut c, Pane::Rack, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Devices));
    let before = saved(&c);
    command(&mut c, Pane::Rack, "edit.paste");
    let now = instruments(&c);
    assert_eq!(now.len(), 4, "{now:?}");
    // The pasted pair is cabled to each other and to nothing else outside.
    assert_eq!(c.doc().cables.len(), cables_before + 1, "one inner cable");
    // The copies are the selection, the originals are not.
    let sel = c.view().selection();
    assert_eq!(sel.len(), 2);
    assert!(sel.iter().all(|k| k.0 != "a" && k.0 != "d"), "{sel:?}");
    // One undo step takes the whole paste back, byte for byte.
    assert!(c.undo());
    assert_eq!(saved(&c), before);
    assert!(c.redo());
    assert_eq!(instruments(&c).len(), 4);
}

#[test]
fn paste_lands_after_the_primary_device() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    add(&mut c, DELAY, "e");
    select_devices(&mut c, &["a"]);
    command(&mut c, Pane::Rack, "edit.copy");
    select_devices(&mut c, &["d"]);
    command(&mut c, Pane::Rack, "edit.paste");
    let order = instruments(&c);
    let at = order.iter().position(|k| k == "d").unwrap();
    assert_eq!(order[at + 1], "poly-synth", "{order:?}");
}

#[test]
fn cut_removes_the_selection_in_one_undo_step_and_paste_brings_it_back() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    let before = saved(&c);
    select_devices(&mut c, &["a", "d"]);
    command(&mut c, Pane::Rack, "edit.cut");
    assert!(instruments(&c).is_empty(), "{:?}", keys(&c));
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Devices));
    command(&mut c, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&c).len(), 2);
    // Undo the paste, then the cut: the starting document is back.
    assert!(c.undo());
    assert!(c.undo());
    assert_eq!(saved(&c), before);
}

#[test]
fn duplicate_copies_devices_after_the_selection_and_selects_the_copies() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    select_devices(&mut c, &["a"]);
    command(&mut c, Pane::Rack, "edit.duplicate");
    assert_eq!(instruments(&c).len(), 3);
    let sel = c.view().selection();
    assert_eq!(sel.len(), 1);
    assert_ne!(sel[0].0, "a");
    assert!(c.undo());
    assert_eq!(instruments(&c).len(), 2);
}

#[test]
fn delete_removes_several_devices_in_one_undo_step() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    add(&mut c, DELAY, "e");
    let before = saved(&c);
    select_devices(&mut c, &["a", "e"]);
    command(&mut c, Pane::Rack, "edit.delete");
    assert_eq!(instruments(&c), ["d"]);
    assert!(c.undo());
    assert_eq!(saved(&c), before);
}

#[test]
fn the_clipboard_survives_new_and_open() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    select_devices(&mut c, &["a"]);
    command(&mut c, Pane::Rack, "edit.copy");
    c.new_document();
    assert!(instruments(&c).is_empty());
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Devices));
    command(&mut c, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&c).len(), 1, "a cross-song paste");
    // Open another project: still there.
    let json = atomik_model::save_project(c.doc());
    c.new_document();
    c.open_str(&json, None).unwrap();
    command(&mut c, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&c).len(), 2);
}

#[test]
fn a_cross_song_paste_registers_the_assets_it_needs() {
    use atomik_model::{AssetMeta, ZoneDoc};
    let hash = "ab".repeat(32);
    let mut c = core();
    add(&mut c, "atomik.sampler", "s");
    c.apply_command(Command::RegisterAsset {
        hash: hash.clone(),
        meta: AssetMeta {
            channels: 1,
            sample_rate: 48_000,
            frames: 4800,
            original_name: "kick.wav".into(),
        },
    })
    .unwrap();
    c.apply_command(Command::SetZones {
        device: key("s"),
        zones: vec![ZoneDoc::new(hash.clone(), 4800)],
        merge: None,
    })
    .unwrap();
    select_devices(&mut c, &["s"]);
    command(&mut c, Pane::Rack, "edit.copy");
    c.new_document();
    assert!(c.doc().assets.is_empty());
    command(&mut c, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&c).len(), 1);
    assert!(c.doc().assets.contains_key(&hash));
}

#[test]
fn paste_into_the_open_container_scope_uses_its_path() {
    let mut c = core();
    add(&mut c, "atomik.container", "box");
    c.apply_command(Command::Container(
        atomik_model::ContainerCommand::AddInner {
            container: key("box"),
            index: 0,
            type_id: DELAY.into(),
            key: Some(key("d")),
            auto_split: false,
        },
    ))
    .unwrap();
    c.view_mut().set_scope(&[key("box")]);
    c.after_view_change();
    c.view_mut().set_selection(&[key("box/d")], None);
    command(&mut c, Pane::Rack, "edit.copy");
    command(&mut c, Pane::Rack, "edit.paste");
    let inner = c.doc().inner(&key("box")).unwrap();
    assert_eq!(inner.devices.len(), 2, "pasted inside the container");
}

// ------------------------------------------------------------------ clips

#[test]
fn copy_and_paste_clips_at_the_playhead_and_repeated_pastes_lay_copies_end_to_end() {
    let mut c = with_clip();
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Clips));
    c.transport_action(TransportAction::Locate(4 * BAR));
    command(&mut c, Pane::Arrangement, "edit.paste");
    command(&mut c, Pane::Arrangement, "edit.paste");
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert_eq!(
        clip_starts(&c),
        [(1, 0), (2, 4 * BAR), (3, 5 * BAR), (4, 6 * BAR)]
    );
    assert_eq!(
        c.position(),
        7 * BAR,
        "the playhead advanced by the span each time"
    );
    // The last paste is the selection.
    assert_eq!(c.arrangement().selected_clips(), vec![ClipId(4)]);
    // Three pastes are three undo steps.
    assert!(c.undo() && c.undo() && c.undo());
    assert_eq!(clip_starts(&c), [(1, 0)]);
}

#[test]
fn the_playhead_does_not_jump_while_the_transport_runs() {
    let mut c = with_clip();
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    c.transport_action(TransportAction::Locate(2 * BAR));
    c.transport_action(TransportAction::Play);
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert!(c.is_playing());
    assert_eq!(clip_starts(&c).len(), 2);
    assert_eq!(
        c.position(),
        2 * BAR,
        "the playhead stays where the music is"
    );
}

#[test]
fn paste_lands_on_the_selected_track() {
    let mut c = with_clip();
    add(&mut c, SYNTH, "b");
    song(&mut c, SongCommand::AddTrack { device: key("b") });
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    c.arrangement_mut().set_selected_track(Some(key("b")));
    c.transport_action(TransportAction::Locate(BAR));
    command(&mut c, Pane::Arrangement, "edit.paste");
    let b = c.doc().song.track(&key("b")).unwrap();
    assert_eq!(b.lanes[0].clips.len(), 1);
    assert_eq!(b.lanes[0].clips[0].start, BAR);
    // The track stays selected, so the next paste goes there too.
    assert_eq!(c.arrangement().selected_track(), Some(&key("b")));
}

#[test]
fn paste_lands_on_the_selected_lane_of_the_selected_track() {
    let mut c = with_clip();
    add(&mut c, SYNTH, "b");
    song(&mut c, SongCommand::AddTrack { device: key("b") });
    song(
        &mut c,
        SongCommand::AddNoteLane {
            device: key("b"),
            name: None,
        },
    );
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    c.arrangement_mut().set_selected_track(Some(key("b")));
    c.arrangement_mut().set_selected_lane(&key("b"), 1);
    c.transport_action(TransportAction::Locate(BAR));
    command(&mut c, Pane::Arrangement, "edit.paste");
    let b = c.doc().song.track(&key("b")).unwrap();
    assert!(b.lanes[0].clips.is_empty());
    assert_eq!(b.lanes[1].clips.len(), 1);
    assert_eq!(b.lanes[1].clips[0].start, BAR);
}

#[test]
fn cut_clips_removes_them_in_one_step_and_duplicate_selects_the_copies() {
    let mut c = with_clip();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(BAR, BAR, &[(0, 67)]),
    }))
    .unwrap();
    c.arrangement_mut()
        .set_selected_clips(&[ClipId(1), ClipId(2)]);
    command(&mut c, Pane::Arrangement, "edit.duplicate");
    assert_eq!(clip_starts(&c).len(), 4);
    assert_eq!(c.arrangement().selected_clips(), vec![ClipId(3), ClipId(4)]);
    assert!(c.undo());
    assert_eq!(clip_starts(&c).len(), 2);
    c.arrangement_mut()
        .set_selected_clips(&[ClipId(1), ClipId(2)]);
    command(&mut c, Pane::Arrangement, "edit.cut");
    assert!(
        clip_starts(&c).is_empty(),
        "{:?} {:?}",
        clip_starts(&c),
        c.last_rejection()
    );
    assert!(c.undo(), "one undo step brings both clips back");
    assert_eq!(clip_starts(&c).len(), 2);
}

#[test]
fn join_makes_one_clip_that_keeps_the_first_id_and_selects_it() {
    let mut c = with_clip();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(BAR, BAR, &[(0, 67)]),
    }))
    .unwrap();
    c.arrangement_mut()
        .set_selected_clips(&[ClipId(2), ClipId(1)]);
    command(&mut c, Pane::Arrangement, "sequencer.join");
    assert_eq!(clip_starts(&c), [(1, 0)]);
    assert_eq!(c.arrangement().selected_clips(), vec![ClipId(1)]);
    assert!(c.undo());
    assert_eq!(clip_starts(&c).len(), 2);
}

#[test]
fn a_clip_from_another_song_needs_its_track_and_says_so() {
    let mut c = with_clip();
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    c.new_document();
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert!(
        c.last_rejection().is_some_and(|m| m.contains("no track")),
        "{:?}",
        c.last_rejection()
    );
    assert!(clip_starts(&c).is_empty());
}

// ------------------------------------------------------------------ tracks

#[test]
fn duplicate_track_copies_the_device_the_track_and_its_clips() {
    let mut c = with_clip();
    c.arrangement_mut().set_selected_track(Some(key("a")));
    let before = saved(&c);
    command(&mut c, Pane::Arrangement, "edit.duplicate");
    assert_eq!(c.doc().song.tracks.len(), 2);
    assert_eq!(clip_starts(&c).len(), 2);
    assert!(c.undo());
    assert_eq!(saved(&c), before);
}

#[test]
fn copy_and_paste_a_track() {
    let mut c = with_clip();
    c.arrangement_mut().set_selected_track(Some(key("a")));
    command(&mut c, Pane::Arrangement, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Tracks));
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert_eq!(c.doc().song.tracks.len(), 2);
    // Devices pasted in the rack bring the track too.
    c.new_document();
    command(&mut c, Pane::Rack, "edit.paste");
    assert_eq!(c.doc().song.tracks.len(), 1);
}

// ------------------------------------------------------------------ notes

fn open_roll(c: &mut AppCore<RecordingSink>) {
    c.open_piano_roll(ClipId(1));
    c.after_view_change();
}

fn notes_of(c: &AppCore<RecordingSink>) -> Vec<(i64, u8)> {
    let (_, _, clip) = c.doc().song.clip(ClipId(1)).unwrap();
    match &clip.content {
        ClipContent::Notes(n) => n.iter().map(|n| (n.tick, n.key)).collect(),
        _ => panic!(),
    }
}

#[test]
fn copy_and_paste_notes_at_the_playhead_and_repeat_to_advance() {
    let mut c = with_clip();
    open_roll(&mut c);
    c.piano_roll_mut().set_selected_notes(&[0, 1]);
    command(&mut c, Pane::PianoRoll, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Notes));
    c.transport_action(TransportAction::Locate(2 * PPQ));
    command(&mut c, Pane::PianoRoll, "edit.paste");
    let notes = notes_of(&c);
    assert!(notes.contains(&(2 * PPQ, 60)), "{notes:?}");
    assert!(notes.contains(&(3 * PPQ, 62)), "{notes:?}");
    // The pasted notes are selected.
    let sel = c.piano_roll().selected_notes();
    assert_eq!(sel.len(), 2);
    // The playhead advanced by the span of the copied notes (a beat and a half... the span).
    let pos = c.position();
    assert!(pos > 2 * PPQ, "{pos}");
    command(&mut c, Pane::PianoRoll, "edit.paste");
    assert!(notes_of(&c).len() >= 5);
    assert!(c.undo() && c.undo());
    assert_eq!(notes_of(&c).len(), 3);
}

#[test]
fn duplicate_notes_copies_them_after_the_selection() {
    let mut c = with_clip();
    open_roll(&mut c);
    c.piano_roll_mut().set_selected_notes(&[0]);
    command(&mut c, Pane::PianoRoll, "edit.duplicate");
    assert_eq!(notes_of(&c).len(), 4);
    assert_eq!(c.piano_roll().selected_notes().len(), 1);
    assert!(c.undo());
    assert_eq!(notes_of(&c).len(), 3);
}

#[test]
fn a_paste_with_the_playhead_outside_the_clip_starts_at_the_clip() {
    let mut c = with_clip();
    open_roll(&mut c);
    c.piano_roll_mut().set_selected_notes(&[0]);
    command(&mut c, Pane::PianoRoll, "edit.copy");
    c.transport_action(TransportAction::Locate(9 * BAR));
    command(&mut c, Pane::PianoRoll, "edit.paste");
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert!(notes_of(&c).iter().all(|(t, _)| *t < BAR));
}

// ------------------------------------------------------------------ points

fn with_automation() -> AppCore<RecordingSink> {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    song(&mut c, SongCommand::AddTrack { device: key("a") });
    song(
        &mut c,
        SongCommand::AddAutomationLane {
            device: key("a"),
            param: "filter.cutoff".into(),
        },
    );
    let points = vec![
        AutomationPoint {
            tick: 0,
            value: 300.0,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: PPQ,
            value: 600.0,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 2 * PPQ,
            value: 900.0,
            curve: Curve::Linear,
        },
    ];
    song(
        &mut c,
        SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Automation(points),
            },
        },
    );
    c
}

fn points_of(c: &AppCore<RecordingSink>) -> Vec<(i64, f32)> {
    let (_, _, clip) = c.doc().song.clip(ClipId(1)).unwrap();
    match &clip.content {
        ClipContent::Automation(p) => p.iter().map(|p| (p.tick, p.value)).collect(),
        _ => panic!(),
    }
}

#[test]
fn copy_paste_and_duplicate_automation_points() {
    let mut c = with_automation();
    c.arrangement_mut().set_selected_points(ClipId(1), &[0, 1]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::AutomationPoints));
    c.transport_action(TransportAction::Locate(2 * PPQ + PPQ / 2));
    command(&mut c, Pane::Arrangement, "edit.paste");
    let pts = points_of(&c);
    assert!(pts.contains(&(2 * PPQ + PPQ / 2, 300.0)), "{pts:?}");
    assert!(pts.contains(&(3 * PPQ + PPQ / 2, 600.0)), "{pts:?}");
    assert_eq!(c.arrangement().selected_points().unwrap().1.len(), 2);
    assert!(c.undo());
    assert_eq!(points_of(&c).len(), 3);
    c.arrangement_mut().set_selected_points(ClipId(1), &[1, 2]);
    command(&mut c, Pane::Arrangement, "edit.duplicate");
    assert_eq!(points_of(&c).len(), 4);
}

// ------------------------------------------------------------------ messages

#[test]
fn refused_commands_say_why_and_change_nothing() {
    let mut c = with_clip();
    let before = saved(&c);
    command(&mut c, Pane::Rack, "edit.paste");
    assert!(c.last_rejection().unwrap().contains("clipboard is empty"));
    command(&mut c, Pane::Rack, "edit.copy");
    assert!(c.last_rejection().unwrap().contains("Nothing is selected"));
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut c, Pane::Arrangement, "edit.copy");
    command(&mut c, Pane::Rack, "edit.paste");
    assert!(
        c.last_rejection().unwrap().contains("clips"),
        "{:?}",
        c.last_rejection()
    );
    // A foreign clipboard in the piano roll.
    open_roll(&mut c);
    command(&mut c, Pane::PianoRoll, "edit.paste");
    assert!(c.last_rejection().unwrap().contains("clips"));
    assert_eq!(saved(&c), before);
}

#[test]
fn the_menus_know_what_can_be_copied_and_pasted() {
    let mut c = with_clip();
    assert!(!c.has_selection(Pane::Arrangement));
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    assert!(c.has_selection(Pane::Arrangement));
    assert!(!c.can_paste(Pane::Arrangement));
    command(&mut c, Pane::Arrangement, "edit.copy");
    assert!(c.can_paste(Pane::Arrangement));
    assert!(!c.can_paste(Pane::Rack));
    assert!(!c.can_paste(Pane::PianoRoll));
    // A holder with nothing in it offers no paste, whatever its kind says.
    c.clipboard_mut()
        .set(crate::clipboard::ClipboardData::Notes(
            atomik_model::NoteSnapshot {
                notes: Vec::new(),
                span: 0,
            },
        ));
    open_roll(&mut c);
    assert!(!c.can_paste(Pane::PianoRoll));
    assert!(!c.can_paste(Pane::Arrangement));
}

#[test]
fn go_to_track_selects_the_track_of_the_selected_device() {
    let mut c = with_clip();
    add(&mut c, DELAY, "fx");
    select_devices(&mut c, &["fx"]);
    assert!(!c.go_to_track(), "an effect has no track");
    select_devices(&mut c, &["a"]);
    assert!(c.go_to_track());
    assert_eq!(c.arrangement().selected_track(), Some(&key("a")));
}

#[test]
fn disconnect_removes_the_cables_of_one_device_in_one_step() {
    let mut c = core();
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "d");
    c.apply_command(cable(("a", "out_l"), ("d", "in_l")))
        .unwrap();
    c.apply_command(cable(("a", "out_r"), ("d", "in_r")))
        .unwrap();
    let before = saved(&c);
    select_devices(&mut c, &["d"]);
    command(&mut c, Pane::Rack, "rack.disconnect");
    assert!(
        c.doc()
            .cables
            .iter()
            .all(|k| k.from.device != key("d") && k.to.device != key("d"))
    );
    assert!(c.undo());
    assert_eq!(saved(&c), before);
}

#[test]
fn the_workflow_commands_are_the_only_way_in() {
    // The clipboard commands build `WorkflowCommand`s: the model sees one command per step.
    let mut c = core();
    add(&mut c, SYNTH, "a");
    select_devices(&mut c, &["a"]);
    command(&mut c, Pane::Rack, "edit.copy");
    let snapshot = match c.clipboard().get() {
        Some(crate::clipboard::ClipboardData::Devices(s)) => s.clone(),
        other => panic!("{other:?}"),
    };
    c.apply_command(Command::Workflow(WorkflowCommand::PasteDevices {
        snapshot,
        container: None,
        at: usize::MAX,
    }))
    .unwrap();
    assert_eq!(instruments(&c).len(), 2);
}
