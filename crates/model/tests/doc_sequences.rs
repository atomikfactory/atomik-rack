//! Whole-document sequences (proptest): random runs of the public command set over a document
//! that holds a rack, a console, tracks and clips, audio, a container and plugins. After every
//! step the document passes `RackDoc::validate`; a command that is accepted is one undo step
//! whose undo gives back the document before and whose redo gives back the one after; a command
//! that is rejected leaves the document and its history as they were and is never an
//! `Invariant` error (that is a planner and the history disagreeing, which must not happen for
//! any command a user can send). Undoing everything gives back the start, redoing everything
//! the end.
//!
//! Documents are compared with the track heights left out: the heights of tracks that are gone
//! are not part of what an undo brings back (they are view state).
//!
//! `PROPTEST_CASES` raises the number of sequences (the acceptance run uses 10000).

mod common;
use atomik_device_sdk::Registry;
use atomik_model::{
    ClipId, ClipSnapshot, Command, CreateOptions, CreateTarget, DeviceKey, DeviceSnapshot,
    Document, ModelError, NoteSnapshot, QuantizeParams, RackDoc, SongCommand, WorkflowCommand,
    empty_doc, load_project, save_project, save_project_checked,
};
use common::audio::*;
use common::console_ops::*;
use common::plugin::*;
use common::song::*;
use common::workflow::wf;
use common::*;
use proptest::prelude::*;

/// A workflow command in abstract form: indices wrap around what the document holds.
#[derive(Debug, Clone)]
enum WfOp {
    Duplicate(Vec<usize>, bool),
    Remove(Vec<usize>),
    Move(Vec<usize>, usize),
    Disconnect(usize),
    DuplicateTrack(usize),
    PasteDevices(Vec<usize>, usize, bool),
    PasteClips(Vec<usize>, i64, Option<usize>),
    DuplicateClips(Vec<usize>),
    Join(Vec<usize>),
    PasteNotes(usize, Vec<usize>, i64),
    DuplicateNotes(usize, Vec<usize>),
    AddDevice(usize),
    Connect(usize, usize),
    Quantize(usize, u8, bool),
    Player(usize, usize),
    NotesCable(usize, usize),
    Take(TakeOp),
}

/// A take-group command in abstract form: indices wrap around the groups and takes held.
#[derive(Debug, Clone)]
enum TakeOp {
    NewGroup(usize, Vec<(i64, i64)>),
    GroupAsTakes(usize, Vec<usize>),
    AddTake(usize, i64, i64),
    Comp(usize, usize, i64, i64),
    Pick(usize, usize),
    Delete(usize, usize),
    Flatten(usize),
}

fn take_op() -> impl Strategy<Value = TakeOp> {
    let half = BAR / 2;
    let span = move || (0i64..6, 1i64..4).prop_map(move |(s, l)| (s * half, l * half));
    prop_oneof![
        (0usize..4, proptest::collection::vec(span(), 1..4))
            .prop_map(|(t, s)| TakeOp::NewGroup(t, s)),
        (0usize..4, sel()).prop_map(|(t, c)| TakeOp::GroupAsTakes(t, c)),
        (0usize..4, span()).prop_map(|(g, (s, l))| TakeOp::AddTake(g, s, l)),
        (0usize..4, 0usize..4, span()).prop_map(|(g, t, (s, l))| TakeOp::Comp(g, t, s, s + l)),
        (0usize..4, 0usize..4).prop_map(|(g, t)| TakeOp::Pick(g, t)),
        (0usize..4, 0usize..4).prop_map(|(g, t)| TakeOp::Delete(g, t)),
        (0usize..4).prop_map(TakeOp::Flatten),
    ]
}

/// The take groups of the document: `(group id, takes held)`.
fn groups(doc: &RackDoc) -> Vec<(u32, usize)> {
    doc.song
        .take_groups()
        .map(|(_, _, g)| (g.id, g.takes.len()))
        .collect()
}

fn resolve_take(op: &TakeOp, doc: &RackDoc) -> Option<SongCommand> {
    let note_tracks: Vec<DeviceKey> = doc
        .song
        .tracks
        .iter()
        .filter(|t| !t.is_audio())
        .map(|t| t.device.clone())
        .collect();
    let all = groups(doc);
    let clip = |s: i64, l: i64| notes_clip(s, l, vec![note(0, PPQ / 4, 60, 0.8)]);
    Some(match op {
        TakeOp::NewGroup(t, spans) => SongCommand::NewTakeGroup {
            device: pick(&note_tracks, *t)?,
            lane: 0,
            takes: spans.iter().map(|(s, l)| clip(*s, *l)).collect(),
        },
        TakeOp::GroupAsTakes(t, sel) => {
            let device = pick(&note_tracks, *t)?;
            let on_lane: Vec<ClipId> = doc
                .song
                .track(&device)?
                .lanes
                .first()?
                .clips
                .iter()
                .map(|c| c.id)
                .collect();
            SongCommand::GroupAsTakes {
                device,
                lane: 0,
                clips: sel.iter().filter_map(|i| pick(&on_lane, *i)).collect(),
                extra: vec![clip(0, BAR)],
            }
        }
        TakeOp::AddTake(g, s, l) => SongCommand::AddTake {
            group: pick(&all, *g)?.0,
            clip: clip(*s, *l),
        },
        TakeOp::Comp(g, t, start, end) => {
            let (group, takes) = pick(&all, *g)?;
            SongCommand::SetCompSegment {
                group,
                take: t % takes.max(1),
                start: *start,
                end: *end,
            }
        }
        TakeOp::Pick(g, t) => {
            let (group, takes) = pick(&all, *g)?;
            SongCommand::PickTake {
                group,
                take: t % takes.max(1),
            }
        }
        TakeOp::Delete(g, t) => {
            let (group, takes) = pick(&all, *g)?;
            SongCommand::DeleteTake {
                group,
                take: t % takes.max(1),
            }
        }
        TakeOp::Flatten(g) => SongCommand::FlattenTakes {
            group: pick(&all, *g)?.0,
        },
    })
}

fn sel() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..12, 1..4)
}

fn wf_op() -> impl Strategy<Value = WfOp> {
    prop_oneof![
        (sel(), any::<bool>()).prop_map(|(s, i)| WfOp::Duplicate(s, i)),
        sel().prop_map(WfOp::Remove),
        (sel(), 0usize..12).prop_map(|(s, t)| WfOp::Move(s, t)),
        (0usize..12).prop_map(WfOp::Disconnect),
        (0usize..12).prop_map(WfOp::DuplicateTrack),
        (sel(), 0usize..12, any::<bool>()).prop_map(|(s, a, i)| WfOp::PasteDevices(s, a, i)),
        (sel(), 0i64..12, proptest::option::of(0usize..3)).prop_map(|(s, b, t)| WfOp::PasteClips(
            s,
            b * BAR / 2,
            t
        )),
        sel().prop_map(WfOp::DuplicateClips),
        sel().prop_map(WfOp::Join),
        (0usize..6, sel(), 0i64..16).prop_map(|(c, n, t)| WfOp::PasteNotes(c, n, t * PPQ)),
        (0usize..6, sel()).prop_map(|(c, n)| WfOp::DuplicateNotes(c, n)),
        (0usize..4).prop_map(WfOp::AddDevice),
        (0usize..12, 0usize..12).prop_map(|(a, b)| WfOp::Connect(a, b)),
        (0usize..6, 0u8..5, any::<bool>()).prop_map(|(c, g, e)| WfOp::Quantize(c, g, e)),
        (0usize..2, 0usize..12).prop_map(|(t, s)| WfOp::Player(t, s)),
        (0usize..12, 0usize..12).prop_map(|(a, b)| WfOp::NotesCable(a, b)),
        take_op().prop_map(WfOp::Take),
    ]
}

fn pick<T: Clone>(list: &[T], i: usize) -> Option<T> {
    (!list.is_empty()).then(|| list[i % list.len()].clone())
}

fn top_keys(doc: &RackDoc) -> Vec<DeviceKey> {
    doc.devices.iter().map(|x| x.key.clone()).collect()
}

/// The keys a copy may take: the top level, or the inside of the first container.
fn copyable_keys(doc: &RackDoc, inner: bool) -> Vec<DeviceKey> {
    let keys: Vec<DeviceKey> = if inner {
        doc.devices
            .iter()
            .find(|x| x.inner.is_some())
            .map(|c| {
                c.inner
                    .iter()
                    .flat_map(|i| i.devices.iter())
                    .map(|x| key(&format!("{}/{}", c.key, x.key)))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        top_keys(doc)
    };
    keys.into_iter()
        .filter(|k| {
            doc.device(k)
                .is_none_or(|x| x.type_id != CHANNEL && x.type_id != MASTER)
        })
        .collect()
}

fn chosen(list: &[DeviceKey], sel: &[usize]) -> Vec<DeviceKey> {
    sel.iter().filter_map(|i| pick(list, *i)).collect()
}

fn resolve_wf(op: &WfOp, doc: &RackDoc) -> Option<Command> {
    let top = top_keys(doc);
    let ids = clip_ids(doc);
    let tracks: Vec<DeviceKey> = doc.song.tracks.iter().map(|t| t.device.clone()).collect();
    Some(match op {
        WfOp::Duplicate(s, inner) => {
            let keys = chosen(&copyable_keys(doc, *inner), s);
            if keys.is_empty() {
                return None;
            }
            wf(WorkflowCommand::DuplicateDevices { keys })
        }
        WfOp::Remove(s) => wf(WorkflowCommand::RemoveDevices {
            keys: chosen(&top, s),
        }),
        WfOp::Move(s, to) => wf(WorkflowCommand::MoveDevices {
            keys: chosen(&top, s),
            to: *to,
        }),
        WfOp::Disconnect(i) => wf(WorkflowCommand::DisconnectDevice {
            key: pick(&top, *i)?,
        }),
        WfOp::DuplicateTrack(i) => wf(WorkflowCommand::DuplicateTrack {
            device: pick(&tracks, *i)?,
        }),
        WfOp::PasteDevices(s, at, inner) => {
            let keys = chosen(&copyable_keys(doc, *inner), s);
            let snapshot = DeviceSnapshot::capture(doc, &keys, !*inner && at % 2 == 0).ok()?;
            wf(WorkflowCommand::PasteDevices {
                snapshot,
                container: (*inner).then(|| key("c")),
                at: *at,
            })
        }
        WfOp::PasteClips(s, at, track) => {
            let picked: Vec<ClipId> = s.iter().filter_map(|i| pick(&ids, *i)).collect();
            wf(WorkflowCommand::PasteClips {
                track: track.and_then(|i| pick(&tracks, i)),
                lane: None,
                at_tick: *at,
                clips: ClipSnapshot::capture(doc, &picked).ok()?,
            })
        }
        WfOp::DuplicateClips(s) => wf(WorkflowCommand::DuplicateClips {
            ids: s.iter().filter_map(|i| pick(&ids, *i)).collect(),
        }),
        WfOp::Join(s) => wf(WorkflowCommand::JoinClips {
            ids: s.iter().filter_map(|i| pick(&ids, *i)).collect(),
        }),
        WfOp::PasteNotes(c, n, at) => {
            let id = pick(&ids, *c)?;
            let (_, _, clip) = doc.song.clip(id)?;
            wf(WorkflowCommand::PasteNotes {
                clip: id,
                at_tick: *at,
                notes: NoteSnapshot::capture(clip, n).ok()?,
            })
        }
        WfOp::DuplicateNotes(c, n) => wf(WorkflowCommand::DuplicateNotes {
            clip: pick(&ids, *c)?,
            indices: n.clone(),
        }),
        WfOp::AddDevice(t) => add(usize::MAX, [SYNTH, DELAY, CONTAINER, SAMPLER][*t], None),
        WfOp::Connect(a, b) => {
            let (from, to) = (pick(&top, *a)?, pick(&top, *b)?);
            Command::Connect(cable(&format!("{from}:out_l"), &format!("{to}:in_l")))
        }
        WfOp::Player(t, s) => wf(WorkflowCommand::CreatePlayer {
            type_id: [ARP, "atomik.note-filter"][*t].into(),
            target: CreateTarget::Selected(pick(&top, *s)?),
            options: CreateOptions::default(),
        }),
        WfOp::NotesCable(a, b) => {
            let (from, to) = (pick(&top, *a)?, pick(&top, *b)?);
            Command::Connect(cable(
                &format!("{from}:notes_out"),
                &format!("{to}:notes_in"),
            ))
        }
        WfOp::Take(op) => sc(resolve_take(op, doc)?),
        WfOp::Quantize(c, grid, ends) => sc(SongCommand::ApplyQuantize {
            clips: vec![pick(&ids, *c)?],
            quantize: QuantizeParams {
                grid: [1, 7, PPQ / 4, PPQ, 1 << 20][usize::from(*grid) % 5],
                snap_ends: *ends,
                ..QuantizeParams::default()
            },
        }),
    })
}

/// One step of a run.
#[derive(Debug, Clone)]
enum Step {
    Song(SongOp),
    Console(ConsoleOp),
    Audio(AudioOp),
    Wf(WfOp),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => song_op().prop_map(Step::Song),
        1 => console_op().prop_map(Step::Console),
        2 => audio_op().prop_map(Step::Audio),
        4 => wf_op().prop_map(Step::Wf),
    ]
}

/// A rich starting document: a rack with a synth and a delay, a container with two devices, a
/// plugin, tracks with note clips, the console, and two pooled assets.
fn start(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        reg,
        [
            register(1, 480_000),
            register(2, 96_000),
            add(0, SYNTH, Some("synth")),
            add(1, DELAY, Some("delay")),
            connect("synth:out_l", "delay:in_l"),
            connect("synth:out_r", "delay:in_r"),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(
                    BAR,
                    BAR,
                    vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.6)],
                ),
            ),
            add(2, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "isynth"),
            add_inner("c", 1, DELAY, "idelay"),
            connect("c/isynth:out_l", "c/idelay:in_l"),
            add_plugin(3, &synth_ref(), &synth_layout(), Some("ps")),
            add_track("ps"),
            add_clip("ps", 0, notes_clip(0, 2 * BAR, vec![note(0, PPQ, 50, 1.0)])),
            sync_state("ps", &[9, 8, 7]),
        ],
    );
    Document::new(d.doc().clone())
}

fn resolve(op: &Step, doc: &RackDoc, reg: &Registry) -> Option<Command> {
    match op {
        Step::Song(o) => resolve_song(o, doc, reg),
        Step::Console(o) => resolve_console(o, doc),
        Step::Audio(o) => resolve_audio(o, doc),
        Step::Wf(o) => resolve_wf(o, doc),
    }
}

/// The document as compared across undo and redo (see the module docs).
fn norm(doc: &RackDoc) -> RackDoc {
    let mut d = doc.clone();
    d.song.track_heights.clear();
    d
}

/// Runs `steps` over a fresh document, checking everything after each one.
fn run_sequence(steps: &[Step]) -> Result<(), TestCaseError> {
    let reg = plugin_console_registry();
    let mut d = start(&reg);
    let original = norm(d.doc());
    prop_assert!(d.doc().validate().is_ok());
    for s in steps {
        let Some(cmd) = resolve(s, d.doc(), &reg) else {
            if matches!(s, Step::Song(SongOp::EndMerge)) {
                d.end_merge();
            }
            continue;
        };
        d.end_merge();
        let before = norm(d.doc());
        let depth = d.undo_depth();
        match d.apply(cmd.clone(), &reg) {
            Ok(_) => {
                if let Err(v) = d.doc().validate() {
                    return Err(TestCaseError::fail(format!(
                        "{cmd:?} left an invalid document: {v:?}"
                    )));
                }
                let after = norm(d.doc());
                // Every reachable document saves, and what it saves loads back to the same text.
                let text = match save_project_checked(d.doc()) {
                    Ok(text) => text,
                    Err(e) => return Err(TestCaseError::fail(format!("{cmd:?}: {e}"))),
                };
                match load_project(&text, &reg) {
                    Ok(loaded) => prop_assert!(save_project(&loaded) == text, "{:?}", cmd),
                    Err(e) => {
                        return Err(TestCaseError::fail(format!(
                            "{cmd:?} saved a project that does not load: {e}"
                        )));
                    }
                }
                if d.undo_depth() == depth + 1 {
                    let undone = d.undo(&reg);
                    prop_assert!(undone.is_some(), "{:?}", cmd);
                    prop_assert!(norm(d.doc()) == before, "undo of {:?}", cmd);
                    prop_assert!(d.doc().validate().is_ok(), "undo of {:?}", cmd);
                    let redone = d.redo(&reg);
                    prop_assert!(redone.is_some(), "{:?}", cmd);
                    prop_assert!(norm(d.doc()) == after, "redo of {:?}", cmd);
                } else {
                    prop_assert_eq!(d.undo_depth(), depth, "{:?} is no step or one step", cmd);
                }
            }
            Err(e) => {
                prop_assert!(
                    !matches!(e, ModelError::Invariant(_)),
                    "{:?} was planned for a document it does not fit: {}",
                    cmd,
                    e
                );
                prop_assert!(
                    norm(d.doc()) == before,
                    "{:?} was rejected but changed",
                    cmd
                );
                prop_assert_eq!(d.undo_depth(), depth);
            }
        }
    }
    let last = norm(d.doc());
    while d.undo(&reg).is_some() {
        prop_assert!(d.doc().validate().is_ok());
    }
    prop_assert!(norm(d.doc()) == original, "undo of everything");
    while d.redo(&reg).is_some() {
        prop_assert!(d.doc().validate().is_ok());
    }
    prop_assert!(norm(d.doc()) == last, "redo of everything");
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn whole_document_sequences_keep_the_invariants(steps in prop::collection::vec(step(), 0..50)) {
        run_sequence(&steps)?;
    }
}
