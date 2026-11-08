//! Copy and paste between two instances: two cores share one in-memory system clipboard (the
//! real one is never touched), one copies, the other pastes.

use super::*;
use crate::clipboard::ClipboardKind;
use crate::clipboard_os::FakeClipboard;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Note, SocketRef,
    SongCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn instance(os: &FakeClipboard) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.set_os_clipboard(Box::new(os.clone()));
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

fn note_clip(notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, k)| Note::new(tick, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

/// Synth `a` with a track holding one note clip (id 1, three notes).
fn with_clip(os: &FakeClipboard) -> AppCore<RecordingSink> {
    let mut c = instance(os);
    add(&mut c, SYNTH, "a");
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(&[(0, 60), (PPQ, 62), (2 * PPQ, 64)]),
    }))
    .unwrap();
    c
}

fn command(c: &mut AppCore<RecordingSink>, pane: Pane, id: &str) {
    let _ = c.pane_command(pane, id, None);
}

fn open_roll(c: &mut AppCore<RecordingSink>) {
    c.open_piano_roll(ClipId(1));
    c.after_view_change();
}

fn notes_of(c: &AppCore<RecordingSink>) -> Vec<(i64, u8)> {
    let (_, _, clip) = c.doc().song.clip(ClipId(1)).unwrap();
    match &clip.content {
        ClipContent::Notes(n) => n.iter().map(|n| (n.tick, n.key)).collect(),
        _ => panic!("a note clip"),
    }
}

fn instruments(c: &AppCore<RecordingSink>) -> Vec<String> {
    c.doc()
        .devices
        .iter()
        .filter(|d| !atomik_model::console::is_console_type(&d.type_id))
        .map(|d| d.key.0.clone())
        .collect()
}

#[test]
fn notes_copied_in_one_instance_paste_in_another() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    let mut b = with_clip(&os);
    open_roll(&mut a);
    open_roll(&mut b);
    a.piano_roll_mut().set_selected_notes(&[0, 1]);
    command(&mut a, Pane::PianoRoll, "edit.copy");
    assert_eq!(os.writes.get(), 1, "the copy reached the system clipboard");
    assert!(
        b.clipboard().is_empty(),
        "the other instance holds nothing itself"
    );
    assert!(
        b.can_paste(Pane::PianoRoll),
        "the menu offers the other instance's copy"
    );
    b.transport_action(atomik_ui::TransportAction::Locate(3 * PPQ));
    command(&mut b, Pane::PianoRoll, "edit.paste");
    let notes = notes_of(&b);
    assert!(notes.contains(&(3 * PPQ, 60)), "{notes:?}");
    assert!(notes.contains(&(4 * PPQ, 62)), "{notes:?}");
    assert_eq!(b.clipboard().kind(), Some(ClipboardKind::Notes));
    assert_eq!(notes_of(&a).len(), 3, "the copying instance is untouched");
}

#[test]
fn clips_copied_in_one_instance_paste_in_another() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    let mut b = with_clip(&os);
    a.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    command(&mut a, Pane::Arrangement, "edit.copy");
    b.transport_action(atomik_ui::TransportAction::Locate(4 * BAR));
    command(&mut b, Pane::Arrangement, "edit.paste");
    let starts: Vec<i64> = b
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter())
        .map(|c| c.start)
        .collect();
    assert_eq!(starts, [0, 4 * BAR], "{starts:?}");
}

#[test]
fn automation_points_copied_in_one_instance_paste_in_another() {
    let os = FakeClipboard::default();
    let build = || {
        let mut c = instance(&os);
        add(&mut c, SYNTH, "a");
        c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
            .unwrap();
        c.apply_command(Command::Song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: "filter.cutoff".into(),
        }))
        .unwrap();
        let clip = Clip {
            content: ClipContent::Automation(vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.2,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: PPQ,
                    value: 0.8,
                    curve: Curve::Step,
                },
            ]),
            ..note_clip(&[])
        };
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip,
        }))
        .unwrap();
        c
    };
    let (mut a, mut b) = (build(), build());
    a.arrangement_mut().set_selected_points(ClipId(1), &[0, 1]);
    command(&mut a, Pane::Arrangement, "edit.copy");
    assert_eq!(a.clipboard().kind(), Some(ClipboardKind::AutomationPoints));
    b.arrangement_mut().set_selected_points(ClipId(1), &[0]);
    b.transport_action(atomik_ui::TransportAction::Locate(2 * PPQ));
    command(&mut b, Pane::Arrangement, "edit.paste");
    assert_eq!(b.clipboard().kind(), Some(ClipboardKind::AutomationPoints));
    let (_, _, clip) = b.doc().song.clip(ClipId(1)).unwrap();
    let ClipContent::Automation(points) = &clip.content else {
        panic!("an automation clip");
    };
    assert!(points.len() > 2, "the points arrived: {points:?}");
}

#[test]
fn devices_copied_in_one_instance_paste_in_another_with_their_cables() {
    let os = FakeClipboard::default();
    let mut a = instance(&os);
    let mut b = instance(&os);
    add(&mut a, SYNTH, "a");
    add(&mut a, DELAY, "d");
    a.apply_command(Command::Connect(Cable {
        from: SocketRef {
            device: key("a"),
            socket: "out_l".into(),
        },
        to: SocketRef {
            device: key("d"),
            socket: "in_l".into(),
        },
    }))
    .unwrap();
    a.after_view_change();
    a.view_mut().set_selection(&[key("a"), key("d")], None);
    command(&mut a, Pane::Rack, "edit.copy");
    let cables_before = b.doc().cables.len();
    command(&mut b, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&b).len(), 2, "{:?}", instruments(&b));
    assert_eq!(
        b.doc().cables.len(),
        cables_before + 1,
        "the cable between them came along"
    );
    // The copies have keys of their own in the target.
    command(&mut b, Pane::Rack, "edit.paste");
    assert_eq!(instruments(&b).len(), 4);
}

#[test]
fn text_that_is_not_an_envelope_is_ignored_and_the_in_process_copy_pastes() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    open_roll(&mut a);
    a.piano_roll_mut().set_selected_notes(&[0]);
    command(&mut a, Pane::PianoRoll, "edit.copy");
    *os.text.borrow_mut() = Some("some words copied from a text field".into());
    assert!(!a.can_paste(Pane::Rack));
    a.transport_action(atomik_ui::TransportAction::Locate(3 * PPQ));
    command(&mut a, Pane::PianoRoll, "edit.paste");
    assert_eq!(notes_of(&a).len(), 4, "the in-process copy pasted");
    // A core that copied nothing has nothing to paste from plain text.
    let mut b = with_clip(&os);
    open_roll(&mut b);
    assert!(!b.can_paste(Pane::PianoRoll));
    command(&mut b, Pane::PianoRoll, "edit.paste");
    assert_eq!(notes_of(&b).len(), 3);
}

#[test]
fn your_own_copy_takes_the_fast_path_and_is_not_decoded() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    open_roll(&mut a);
    a.piano_roll_mut().set_selected_notes(&[0]);
    command(&mut a, Pane::PianoRoll, "edit.copy");
    // Replace the text by one that would be refused if it were decoded, and say it is the text
    // this instance already stands for: the paste must not look at it.
    let trap = "atomik-clipboard/9 notes\n{}\n{}".to_owned();
    *os.text.borrow_mut() = Some(trap.clone());
    a.clipboard_mut().set_known(Some(trap));
    a.transport_action(atomik_ui::TransportAction::Locate(3 * PPQ));
    command(&mut a, Pane::PianoRoll, "edit.paste");
    assert_eq!(notes_of(&a).len(), 4);
    assert_eq!(a.last_rejection(), None);
}

#[test]
fn a_newer_or_damaged_envelope_is_refused_with_a_message_and_changes_nothing() {
    let os = FakeClipboard::default();
    let mut b = with_clip(&os);
    open_roll(&mut b);
    for (text, wanted) in [
        ("atomik-clipboard/2 notes\n{}\n{}", "newer"),
        ("atomik-clipboard/1 notes\nnot json\n{}", "damaged"),
        (
            "atomik-clipboard/1 notes\n{}\n{\"format\": \"x\"}",
            "cannot be pasted",
        ),
    ] {
        *os.text.borrow_mut() = Some(text.into());
        b.rejection = None;
        command(&mut b, Pane::PianoRoll, "edit.paste");
        let message = b.last_rejection().unwrap_or_default().to_owned();
        assert!(message.contains(wanted), "{text:?} gave {message:?}");
        assert_eq!(notes_of(&b).len(), 3, "nothing changed");
    }
}

#[test]
fn without_a_system_clipboard_copy_and_paste_still_work_inside_one_instance() {
    let os = FakeClipboard {
        broken: true,
        ..FakeClipboard::default()
    };
    let mut a = with_clip(&os);
    open_roll(&mut a);
    a.piano_roll_mut().set_selected_notes(&[0, 1]);
    command(&mut a, Pane::PianoRoll, "edit.copy");
    assert_eq!(os.writes.get(), 0);
    assert!(a.can_paste(Pane::PianoRoll));
    a.transport_action(atomik_ui::TransportAction::Locate(3 * PPQ));
    command(&mut a, Pane::PianoRoll, "edit.paste");
    assert_eq!(notes_of(&a).len(), 5);
}

#[test]
fn kinds_that_stay_in_one_instance_do_not_resurrect_an_older_foreign_copy() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    let mut b = with_clip(&os);
    open_roll(&mut a);
    open_roll(&mut b);
    // A copies notes; B pastes them (adopting the text) and then copies strips locally.
    a.piano_roll_mut().set_selected_notes(&[0]);
    command(&mut a, Pane::PianoRoll, "edit.copy");
    command(&mut b, Pane::PianoRoll, "edit.paste");
    b.set_clipboard(crate::clipboard::ClipboardData::Strips(
        crate::clipboard::StripSnapshot {
            devices: atomik_model::DeviceSnapshot {
                devices: Vec::new(),
                cables: Vec::new(),
                tracks: Vec::new(),
                assets: Default::default(),
            },
            settings: Vec::new(),
            loose: Vec::new(),
        },
    ));
    assert_eq!(b.clipboard().kind(), Some(ClipboardKind::Strips));
    assert!(
        !b.can_paste(Pane::PianoRoll),
        "the older notes on the system clipboard are not a newer copy"
    );
}

#[test]
fn a_copy_over_the_size_cap_stays_in_the_instance_and_says_so() {
    let os = FakeClipboard::default();
    let mut a = with_clip(&os);
    open_roll(&mut a);
    let big = crate::clipboard::ClipboardData::Notes(atomik_model::NoteSnapshot {
        notes: (0..400_000)
            .map(|i| Note::new(i * 10, 5, (i % 100) as u8, 0.5))
            .collect(),
        span: 4_000_000,
    });
    a.set_clipboard(big);
    assert_eq!(os.writes.get(), 0);
    assert!(a.last_rejection().unwrap_or_default().contains("too much"));
    assert_eq!(a.clipboard().kind(), Some(ClipboardKind::Notes));
}
