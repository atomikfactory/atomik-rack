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
    AutomationPoint, ClipId, ClipSnapshot, Command, CreateOptions, CreateTarget, Curve, DeviceKey,
    DeviceSnapshot, Document, ModSlot, ModelError, NoteSnapshot, NoteTool, ParamLane,
    QuantizeParams, RackDoc, RackSort, SongCommand, WorkflowCommand, empty_doc, load_project,
    save_project, save_project_checked,
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
    /// Names and colours clips (the flags: a name, a colour; the number picks which).
    ClipMeta(Vec<usize>, u8, u8),
    /// A loop made a loop of its own: a looped clip is set on a clip, a loop converted, or a
    /// repetition made independent (the kind), with the clip and a length or an index.
    Loops(u8, usize, i64),
    /// Several tracks duplicated, with a place (the second value: the place among the tracks,
    /// the folder picked by the third) when the flag is set.
    DuplicateTracks(Vec<usize>, Option<(usize, usize)>),
    /// Several tracks moved to a place (the place, and the folder picked by the third value;
    /// 0 is the top level).
    MoveTracks(Vec<usize>, usize, usize),
    /// A folder (the n-th, wrapping) muted or soloed, or moved to a place and folder.
    FolderFlags(usize, bool, bool),
    /// Tracks gathered in a new folder.
    Folder(Vec<usize>),
    MoveFolder(usize, usize, usize),
    PasteDevices(Vec<usize>, usize, bool),
    /// A device is replaced by one of five types (the second value); kinds that do not match
    /// are rejected, which the sequence tolerates.
    Replace(usize, u8),
    /// Folds (`true`) or unfolds several devices.
    FoldMany(Vec<usize>, bool),
    /// Split Sub on a Mono Bass of the rack (the first value picks it) at one of four
    /// frequencies, one of them outside the range.
    SplitSub(usize, u8),
    /// The reverb send wizard on a top-level device (the first value picks it), with one of four
    /// option sets: plain, a vocal, no duck and pre-fader, and one with an option out of range.
    ReverbSend(usize, u8),
    /// Quick sidechain: a track (the first value) ducks from another top-level device (the
    /// second), pre-fader or not, or (the kind 2) gets a gentle pump, or (the kind 3) loses its
    /// ducking.
    Duck(usize, usize, u8),
    /// A rumble track after a track (the second value: the start in beats and a choice of options).
    Rumble(usize, u8),
    /// A transition (the kind 0 filter down, 1 silence, 2 fill) at a boundary of bars 1 to 5 on a
    /// top-level device (the second value picks it: the filter's strip, the fill's track).
    Transition(u8, u8, usize),
    /// A device takes the patch of another one of its type (the second index; the same device
    /// when none else is of its type), or, for `true`, goes back to a new device's sound.
    ApplyPatch(usize, usize, bool),
    PasteClips(Vec<usize>, i64, Option<usize>),
    DuplicateClips(Vec<usize>),
    Join(Vec<usize>),
    /// Parameter lanes of note clips: an automation clip moved into a note clip (the kind 0), or
    /// a clip given a bent lane on a device of the rack (1) or cleared of its lanes (2).
    ClipLanes(u8, usize, usize),
    /// Trim (0), delete outside (1), split (2) or cut out (3, 4: copy) of the clips at a tick,
    /// insert (5) or remove (6) a bar of time there, reverse (7), scale to 200 % (8) or chop to
    /// half bars (9), stretch (11) or move the crossfade of the first clip (12).
    Gesture(u8, Vec<usize>, i64),
    PasteNotes(usize, Vec<usize>, i64),
    DuplicateNotes(usize, Vec<usize>),
    AddDevice(usize),
    Connect(usize, usize),
    Quantize(usize, u8, bool),
    /// The groove family (kind 0..5): add a groove, give a track a live groove (a built-in or the
    /// added one, a strength), clear it, rename the added groove, delete it.
    Groove(u8, usize, u8),
    /// A note track's own controller lanes: a pitch bend lane (kind 0), a wheel lane (1), both (2)
    /// or none (3), on a track, with a tick step that moves the points.
    TrackCtl(u8, usize, u8),
    /// A note tool (kind 0..7) on a clip with a seed.
    NoteTool(usize, u8, u32),
    /// A note-lane workflow (kind 0..5): move or copy a lane (tracks a, b; lanes c, d), merge a
    /// track's lanes, explode a clip, or extract notes of a clip (copy or move).
    Lanes(u8, usize, usize, usize, usize),
    Player(usize, usize),
    NotesCable(usize, usize),
    /// Warp markers of a clip from tick and frame gaps (0), or a split at them (1), or the name of an asset (2).
    Warp(u8, usize, Vec<(i64, u64)>),
    /// The port of a MIDI Out device (the second value picks none or one of two names).
    MidiPort(usize, u8),
    /// A modulation slot of a poly synth: the device, the slot, and what goes in (0 clears it).
    ModSlot(usize, u8, u8),
    /// The song information: which of its texts changes and to which of three values.
    SongInfo(u8, u8),
    Tune(u8),
    Sort(u8),
    References(u8),
    /// The delivery target: none or one of the table's entries.
    Delivery(u8),
    /// The song's video: set, remove, move, trim or relink (the first value picks which).
    Video(u8, u8),
    Take(TakeOp),
    /// Combine devices into a container, dissolve one, or move a device into or out of one.
    Combine(Vec<usize>),
    Uncombine(usize),
    MoveIn(usize, usize, usize),
    MoveOut(usize, usize),
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
    Duplicate(usize, usize),
    Move(usize, usize, usize),
    Unused(usize),
    Silence(usize, i64, i64),
    /// A comp over every group at once.
    Linked(usize, i64, i64),
    /// Two or three groups (picked by index) linked, or one taken out of its link.
    Link(Vec<usize>),
    Unlink(usize),
    /// A clip command on a take (what a clip editor open on it sends).
    EditTake(usize, usize, u8),
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
        (0usize..4, 0usize..4).prop_map(|(g, t)| TakeOp::Duplicate(g, t)),
        (0usize..4, 0usize..4, 0usize..4).prop_map(|(g, t, to)| TakeOp::Move(g, t, to)),
        (0usize..4).prop_map(TakeOp::Unused),
        (0usize..4, span()).prop_map(|(g, (s, l))| TakeOp::Silence(g, s, s + l)),
        (0usize..4, span()).prop_map(|(t, (s, l))| TakeOp::Linked(t, s, s + l)),
        proptest::collection::vec(0usize..4, 2..4).prop_map(TakeOp::Link),
        (0usize..4).prop_map(TakeOp::Unlink),
        (0usize..4, 0usize..4, 0u8..100).prop_map(|(g, t, k)| TakeOp::EditTake(g, t, k)),
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
        TakeOp::Duplicate(g, t) => {
            let (group, takes) = pick(&all, *g)?;
            SongCommand::DuplicateTake {
                group,
                take: t % takes.max(1),
            }
        }
        TakeOp::Move(g, t, to) => {
            let (group, takes) = pick(&all, *g)?;
            SongCommand::MoveTake {
                group,
                take: t % takes.max(1),
                to: to % takes.max(1),
            }
        }
        TakeOp::Unused(g) => SongCommand::DeleteUnusedTakes {
            group: pick(&all, *g)?.0,
        },
        TakeOp::Linked(t, start, end) => SongCommand::SetLinkedComp {
            groups: all.iter().map(|g| g.0).collect(),
            take: *t,
            start: *start,
            end: *end,
        },
        TakeOp::Link(picks) => SongCommand::LinkTakeGroups {
            groups: picks
                .iter()
                .filter_map(|g| pick(&all, *g).map(|p| p.0))
                .collect(),
        },
        TakeOp::Unlink(g) => SongCommand::UnlinkTakeGroup {
            group: pick(&all, *g)?.0,
        },
        TakeOp::Silence(g, start, end) => SongCommand::SetCompSilence {
            group: pick(&all, *g)?.0,
            start: *start,
            end: *end,
        },
        TakeOp::EditTake(g, t, key) => {
            let (group, takes) = pick(&all, *g)?;
            let (_, _, g) = doc.song.take_group(group)?;
            SongCommand::SetClipContent {
                id: g.takes[t % takes.max(1)].id,
                content: atomik_model::ClipContent::Notes(vec![note(0, PPQ / 4, *key, 0.5)].into()),
                merge: None,
            }
        }
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
        (sel(), 0u8..4, 0u8..4).prop_map(|(s, n, c)| WfOp::ClipMeta(s, n, c)),
        (0u8..4, 0usize..12, 1i64..8).prop_map(|(k, c, n)| WfOp::Loops(k, c, n * BAR / 4)),
        (sel(), proptest::option::of((0usize..12, 0usize..4)))
            .prop_map(|(s, p)| WfOp::DuplicateTracks(s, p)),
        (sel(), 0usize..12, 0usize..4).prop_map(|(s, a, g)| WfOp::MoveTracks(s, a, g)),
        (0usize..4, any::<bool>(), any::<bool>()).prop_map(|(g, m, s)| WfOp::FolderFlags(g, m, s)),
        sel().prop_map(WfOp::Folder),
        (0usize..4, 0usize..12, 0usize..4).prop_map(|(g, a, i)| WfOp::MoveFolder(g, a, i)),
        (sel(), 0usize..12, any::<bool>()).prop_map(|(s, a, i)| WfOp::PasteDevices(s, a, i)),
        (0usize..12, 0u8..5).prop_map(|(d, t)| WfOp::Replace(d, t)),
        (sel(), any::<bool>()).prop_map(|(s, f)| WfOp::FoldMany(s, f)),
        (0usize..12, 0u8..4).prop_map(|(d, f)| WfOp::SplitSub(d, f)),
        (0usize..12, 0u8..4).prop_map(|(d, o)| WfOp::ReverbSend(d, o)),
        (0usize..12, 0usize..12, 0u8..4).prop_map(|(t, s, k)| WfOp::Duck(t, s, k)),
        (0usize..12, 0u8..8).prop_map(|(d, o)| WfOp::Rumble(d, o)),
        (0u8..3, 1u8..6, 0usize..12).prop_map(|(k, b, d)| WfOp::Transition(k, b, d)),
        (0usize..12, 0usize..12, any::<bool>()).prop_map(|(a, b, r)| WfOp::ApplyPatch(a, b, r)),
        (sel(), 0i64..12, proptest::option::of(0usize..3)).prop_map(|(s, b, t)| WfOp::PasteClips(
            s,
            b * BAR / 2,
            t
        )),
        sel().prop_map(WfOp::DuplicateClips),
        sel().prop_map(WfOp::Join),
        (0u8..5, 0usize..12, 0usize..12).prop_map(|(k, a, b)| WfOp::ClipLanes(k, a, b)),
        (0u8..15, sel(), -4i64..12).prop_map(|(k, s, t)| WfOp::Gesture(k, s, t * BAR / 4)),
        (0usize..6, sel(), 0i64..16).prop_map(|(c, n, t)| WfOp::PasteNotes(c, n, t * PPQ)),
        (0usize..6, sel()).prop_map(|(c, n)| WfOp::DuplicateNotes(c, n)),
        (0usize..9).prop_map(WfOp::AddDevice),
        (0usize..12, 0u8..3).prop_map(|(d, p)| WfOp::MidiPort(d, p)),
        (0usize..12, 0u8..8, 0u8..40).prop_map(|(d, s, v)| WfOp::ModSlot(d, s, v)),
        (0u8..5, 0u8..3).prop_map(|(f, v)| WfOp::SongInfo(f, v)),
        (0u8..5).prop_map(WfOp::Tune),
        (0u8..3).prop_map(WfOp::Sort),
        (0u8..4).prop_map(WfOp::References),
        (0u8..5).prop_map(WfOp::Delivery),
        (0u8..6, 0u8..4).prop_map(|(k, v)| WfOp::Video(k, v)),
        (0usize..12, 0usize..12).prop_map(|(a, b)| WfOp::Connect(a, b)),
        (0usize..6, 0u8..5, any::<bool>()).prop_map(|(c, g, e)| WfOp::Quantize(c, g, e)),
        (0u8..8, 0usize..12, 0u8..=100).prop_map(|(k, t, s)| WfOp::Groove(k, t, s)),
        (0usize..6, 0u8..7, any::<u32>()).prop_map(|(c, k, s)| WfOp::NoteTool(c, k, s)),
        (0u8..4, 0usize..12, 0u8..8).prop_map(|(k, t, v)| WfOp::TrackCtl(k, t, v)),
        (0u8..10, 0usize..12, 0usize..12, 0usize..6, 0usize..6)
            .prop_map(|(k, a, b, c, d)| WfOp::Lanes(k, a, b, c, d)),
        (0usize..2, 0usize..12).prop_map(|(t, s)| WfOp::Player(t, s)),
        (0usize..12, 0usize..12).prop_map(|(a, b)| WfOp::NotesCable(a, b)),
        (
            0u8..3,
            0usize..12,
            proptest::collection::vec((1i64..4000, 1u64..200_000), 0..5)
        )
            .prop_map(|(k, c, g)| WfOp::Warp(k, c, g)),
        take_op().prop_map(WfOp::Take),
        sel().prop_map(WfOp::Combine),
        (0usize..4).prop_map(WfOp::Uncombine),
        (0usize..12, 0usize..4, 0usize..4).prop_map(|(d, c, i)| WfOp::MoveIn(d, c, i)),
        (0usize..4, 0usize..4).prop_map(|(c, i)| WfOp::MoveOut(c, i)),
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

/// The folder a track-move op picks: 0 is the top level, else the n-th folder (wrapping).
fn folder_pick(doc: &RackDoc, n: usize) -> Option<u32> {
    let ids: Vec<u32> = doc.song.org.groups.iter().map(|g| g.id).collect();
    if n == 0 || ids.is_empty() {
        None
    } else {
        Some(ids[(n - 1) % ids.len()])
    }
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
            wf(WorkflowCommand::DuplicateDevices {
                keys,
                route: s.len() % 2 == 0,
            })
        }
        WfOp::Remove(s) => wf(WorkflowCommand::RemoveDevices {
            keys: chosen(&top, s),
        }),
        WfOp::Move(s, to) => wf(WorkflowCommand::MoveDevices {
            route: *to % 2 == 0,
            keys: chosen(&top, s),
            to: *to,
        }),
        WfOp::Disconnect(i) => wf(WorkflowCommand::DisconnectDevice {
            key: pick(&top, *i)?,
        }),
        WfOp::DuplicateTrack(i) => wf(WorkflowCommand::DuplicateTrack {
            device: pick(&tracks, *i)?,
        }),
        WfOp::DuplicateTracks(s, place) => {
            let devices = chosen(&tracks, s);
            let mut seen = Vec::new();
            let devices: Vec<DeviceKey> = devices
                .into_iter()
                .filter(|d| {
                    let fresh = !seen.contains(d);
                    seen.push(d.clone());
                    fresh
                })
                .collect();
            wf(WorkflowCommand::DuplicateTracks {
                devices,
                place: place.map(|(to_index, g)| atomik_model::TrackPlace {
                    to_index,
                    into: folder_pick(doc, g),
                }),
            })
        }
        WfOp::MoveTracks(s, to_index, g) => {
            let mut seen = Vec::new();
            let devices: Vec<DeviceKey> = chosen(&tracks, s)
                .into_iter()
                .filter(|d| {
                    let fresh = !seen.contains(d);
                    seen.push(d.clone());
                    fresh
                })
                .collect();
            sc(SongCommand::MoveTracks {
                devices,
                place: atomik_model::TrackPlace {
                    to_index: *to_index,
                    into: folder_pick(doc, *g),
                },
            })
        }
        WfOp::Folder(s) => {
            let mut seen = Vec::new();
            let devices: Vec<DeviceKey> = chosen(&tracks, s)
                .into_iter()
                .filter(|d| {
                    let fresh = !seen.contains(d);
                    seen.push(d.clone());
                    fresh
                })
                .collect();
            sc(SongCommand::GroupTracks {
                devices,
                name: "F".into(),
            })
        }
        WfOp::FolderFlags(g, muted, solo) => sc(SongCommand::SetGroupFlags {
            group: folder_pick(doc, g + 1)?,
            muted: *muted,
            solo: *solo,
        }),
        WfOp::MoveFolder(g, to_index, i) => sc(SongCommand::MoveGroupTo {
            group: folder_pick(doc, g + 1)?,
            place: atomik_model::TrackPlace {
                to_index: *to_index,
                into: folder_pick(doc, *i),
            },
        }),
        WfOp::PasteDevices(s, at, inner) => {
            let keys = chosen(&copyable_keys(doc, *inner), s);
            let snapshot = DeviceSnapshot::capture(doc, &keys, !*inner && at % 2 == 0).ok()?;
            wf(WorkflowCommand::PasteDevices {
                route: at % 3 != 0,
                snapshot,
                container: (*inner).then(|| key("c")),
                at: *at,
            })
        }
        WfOp::Replace(i, t) => {
            let types = [
                SYNTH,
                "atomik.wavetable-synth",
                "atomik.fm-synth",
                DELAY,
                "atomik.reverb",
            ];
            wf(WorkflowCommand::ReplaceDevice {
                target: pick(&top, *i)?,
                source: atomik_model::CreateSource::Type(
                    types[usize::from(*t) % types.len()].into(),
                ),
            })
        }
        WfOp::SplitSub(i, f) => {
            let basses: Vec<DeviceKey> = top
                .iter()
                .filter(|k| {
                    doc.device(k)
                        .is_some_and(|d| d.type_id == "atomik.mono-bass")
                })
                .cloned()
                .collect();
            wf(WorkflowCommand::SplitSub {
                device: pick(&basses, *i)?,
                freq: [30.0, 60.0, 120.0, 250.0][usize::from(*f) % 4],
            })
        }
        WfOp::ReverbSend(i, o) => {
            let defaults = atomik_model::reverb_send::ReverbSendOptions::default();
            let options = match o % 4 {
                0 => defaults,
                1 => atomik_model::reverb_send::ReverbSendOptions {
                    vocal: true,
                    ..defaults
                },
                2 => atomik_model::reverb_send::ReverbSendOptions {
                    duck: false,
                    pre_fader: true,
                    decay: Some(4.0),
                    ..defaults
                },
                _ => atomik_model::reverb_send::ReverbSendOptions {
                    high_pass: 5.0,
                    ..defaults
                },
            };
            wf(WorkflowCommand::AddReverbSend {
                track: pick(&top, *i)?,
                options,
            })
        }
        WfOp::Duck(t, s, kind) => {
            let target = pick(&top, *t)?;
            match kind {
                2 => wf(WorkflowCommand::GentlePump { target }),
                3 => wf(WorkflowCommand::RemoveDucking { target }),
                kind => wf(WorkflowCommand::DuckFrom {
                    target,
                    source: pick(&top, *s)?,
                    pre_fader: *kind == 1,
                }),
            }
        }
        WfOp::Rumble(i, o) => {
            let defaults = atomik_model::rumble::RumbleTrackOptions::default();
            let options = match o % 4 {
                0 => defaults,
                1 => atomik_model::rumble::RumbleTrackOptions {
                    low_pass: 90.0,
                    depth: 1.0,
                    ..defaults
                },
                2 => atomik_model::rumble::RumbleTrackOptions {
                    release_ms: 600.0,
                    ..defaults
                },
                _ => atomik_model::rumble::RumbleTrackOptions {
                    high_pass: 5.0,
                    ..defaults
                },
            };
            wf(WorkflowCommand::AddRumble {
                kick: pick(&top, *i)?,
                start: i64::from(*o) * PPQ,
                length: BAR,
                audio: audio(2),
                name: (o % 2 == 0).then(|| "Rumble".to_owned()),
                options,
            })
        }
        WfOp::Transition(kind, bar, i) => {
            let edit = match kind % 3 {
                0 => atomik_model::transition::TransitionEdit::FilterDown {
                    target: doc.console.master.clone()?,
                    bars: bar % 5,
                },
                1 => atomik_model::transition::TransitionEdit::Silence,
                _ => atomik_model::transition::TransitionEdit::Fill {
                    track: pick(&top, *i)?,
                },
            };
            wf(WorkflowCommand::AddTransition {
                at: i64::from(*bar) * BAR,
                edit,
            })
        }
        WfOp::FoldMany(s, folded) => wf(WorkflowCommand::SetFoldedMany {
            keys: chosen(&top, s),
            folded: *folded,
        }),
        WfOp::ApplyPatch(target, source, reset) => {
            let device = pick(&top, *target)?;
            if *reset {
                return Some(wf(WorkflowCommand::ResetDevice { device }));
            }
            let type_id = &doc.device(&device)?.type_id;
            let same: Vec<DeviceKey> = top
                .iter()
                .filter(|k| doc.device(k).is_some_and(|d| &d.type_id == type_id))
                .cloned()
                .collect();
            let from = pick(&same, *source)?;
            let snapshot = DeviceSnapshot::capture(doc, &[from], false).ok()?;
            wf(WorkflowCommand::ApplyPatch { device, snapshot })
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
        WfOp::ClipMeta(s, n, c) => {
            let ids: Vec<ClipId> = s.iter().filter_map(|i| pick(&ids, *i)).collect();
            sc(SongCommand::SetClipMeta {
                ids,
                name: match n {
                    0 => atomik_model::MetaChange::Keep,
                    1 => atomik_model::MetaChange::Clear,
                    2 => atomik_model::MetaChange::Set("Verse".into()),
                    _ => atomik_model::MetaChange::Set(" ".into()),
                },
                color: match c {
                    0 => atomik_model::MetaChange::Keep,
                    1 => atomik_model::MetaChange::Clear,
                    _ => atomik_model::MetaChange::Set([*c, 30, 90]),
                },
            })
        }
        WfOp::Loops(kind, c, n) => {
            let id = pick(&ids, *c)?;
            let (_, _, clip) = doc.song.clip(id)?;
            match kind {
                // A loop of `n` ticks (an audio clip loops too).
                0 => sc(SongCommand::SetClipGeometry {
                    id,
                    start: clip.start,
                    length: clip.length,
                    offset: if clip.content.is_audio() {
                        0
                    } else {
                        clip.offset
                    },
                    loop_length: Some(*n),
                    muted: clip.muted,
                    merge: None,
                }),
                1 => wf(WorkflowCommand::ConvertLoop {
                    ids: vec![id],
                    mode: atomik_model::LoopConvert::Separate,
                }),
                2 => wf(WorkflowCommand::ConvertLoop {
                    ids: vec![id],
                    mode: atomik_model::LoopConvert::Single,
                }),
                _ => wf(WorkflowCommand::DuplicateLoopInstance {
                    id,
                    index: (*n / (BAR / 4)) as usize,
                }),
            }
        }
        WfOp::ClipLanes(kind, a, b) => match (*kind, pick(&ids, *a), pick(&ids, *b)) {
            (0, Some(automation), Some(into)) => {
                wf(WorkflowCommand::AutomationIntoClip { automation, into })
            }
            (1, Some(clip), _) => {
                let device = pick(&top, *b)?;
                Command::Song(SongCommand::SetClipParamLanes {
                    clip,
                    lanes: vec![ParamLane::new(
                        device,
                        "filter.cutoff",
                        vec![
                            AutomationPoint {
                                tick: 0,
                                value: 300.0,
                                curve: Curve::Bend(24),
                            },
                            AutomationPoint {
                                tick: PPQ,
                                value: 2000.0,
                                curve: Curve::Step,
                            },
                        ],
                    )],
                    merge: None,
                })
            }
            (2, Some(clip), _) => Command::Song(SongCommand::SetClipParamLanes {
                clip,
                lanes: Vec::new(),
                merge: None,
            }),
            (3, Some(clip), _) => Command::Song(SongCommand::SetClipMidi {
                clip,
                midi: atomik_model::MidiData {
                    programs: vec![atomik_model::ProgramChange {
                        bank_msb: Some(1),
                        bank_lsb: None,
                        ..atomik_model::ProgramChange::new(PPQ / 2, 9)
                    }],
                    sysex: vec![atomik_model::SysexEvent {
                        tick: 0,
                        data: vec![0x7E, 0x7F, 0x09, 0x01],
                    }],
                    ..Default::default()
                },
                merge: None,
            }),
            (4, Some(clip), _) => Command::Song(SongCommand::SetClipMidi {
                clip,
                midi: atomik_model::MidiData::default(),
                merge: None,
            }),
            _ => return None,
        },
        WfOp::Join(s) => wf(WorkflowCommand::JoinClips {
            ids: s.iter().filter_map(|i| pick(&ids, *i)).collect(),
        }),
        WfOp::Gesture(kind, s, t) => {
            let ids = s.iter().filter_map(|i| pick(&ids, *i)).collect();
            wf(match kind {
                0 => WorkflowCommand::TrimClips {
                    ids,
                    start_delta: *t,
                    merge: None,
                },
                1 => WorkflowCommand::DeleteOutsideEvents { ids },
                2 => WorkflowCommand::SplitClips { ids, at: *t },
                5 => WorkflowCommand::InsertTime {
                    at: *t,
                    length: BAR,
                },
                6 => WorkflowCommand::RemoveTime {
                    at: *t,
                    length: BAR,
                },
                7 => WorkflowCommand::ReverseClips {
                    ids,
                    graphical: *t % 2 == 0,
                },
                8 => WorkflowCommand::ScaleTempo {
                    ids,
                    percent: 200.0,
                    from_end: *t % 3 == 0,
                },
                9 => WorkflowCommand::ChopClips {
                    ids,
                    length: BAR / 2,
                },
                10 => WorkflowCommand::CleanupAutomation {
                    ids,
                    level: atomik_model::ThinLevel::Heavy,
                },
                11 => WorkflowCommand::StretchClips {
                    ids,
                    percent: if *t % 2 == 0 { 50.0 } else { 200.0 },
                    from_start: *t % 3 == 0,
                },
                12 if (*t / (BAR / 4)) % 2 != 0 => WorkflowCommand::ResizeCrossfade {
                    later: ids.first().copied().unwrap_or(ClipId(1)),
                    delta: *t,
                },
                12 => WorkflowCommand::MoveCrossfade {
                    later: ids.first().copied().unwrap_or(ClipId(1)),
                    delta: *t,
                },
                13 | 14 => WorkflowCommand::ExtendClips {
                    ids,
                    next: *kind == 13,
                },
                k => WorkflowCommand::CutOut {
                    ids,
                    start: *t,
                    end: *t + BAR,
                    copy: *k == 4,
                },
            })
        }
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
        WfOp::AddDevice(t) => add(
            usize::MAX,
            [
                SYNTH,
                DELAY,
                CONTAINER,
                SAMPLER,
                "atomik.midi-out",
                // The only device a loop may run through.
                "atomik.feedback",
                "atomik.rotary-speaker",
                "atomik.organ",
                "atomik.mono-bass",
            ][*t],
            None,
        ),
        WfOp::ModSlot(i, slot, v) => {
            let synths: Vec<DeviceKey> = top
                .iter()
                .filter(|k| doc.device(k).is_some_and(|d| d.type_id == SYNTH))
                .cloned()
                .collect();
            let value = (*v > 0).then(|| {
                let (source, target) = (u16::from(*v) % 9, u16::from(*v / 9) % 3);
                let slot = ModSlot::new(source, target);
                if v % 3 == 0 {
                    slot.via((source + 1) % 9)
                } else {
                    slot
                }
            });
            wf(WorkflowCommand::SetModSlot {
                device: pick(&synths, *i)?,
                slot: *slot,
                value,
            })
        }
        WfOp::MidiPort(i, p) => {
            let outs: Vec<DeviceKey> = top
                .iter()
                .filter(|k| {
                    doc.device(k)
                        .is_some_and(|d| d.type_id == "atomik.midi-out")
                })
                .cloned()
                .collect();
            wf(WorkflowCommand::SetMidiOutPort {
                device: pick(&outs, *i)?,
                port: [None, Some("Port A"), Some("Port B")][usize::from(*p)].map(str::to_owned),
            })
        }
        WfOp::SongInfo(field, value) => {
            let mut info = doc.info.clone();
            let text = ["", "Example", "two\nlines\tand a tab"][usize::from(*value)];
            let slot = match field {
                0 => &mut info.title,
                1 => &mut info.author,
                2 => &mut info.email,
                3 => &mut info.url,
                _ => &mut info.notes,
            };
            // A one-line field keeps to one line.
            *slot = if *field == 4 {
                text.to_owned()
            } else {
                text.replace(['\n', '\t'], " ")
            };
            Command::SetSongInfo(info)
        }
        WfOp::Tune(step) => {
            Command::SetMasterTune([0.0, -50.0, 12.5, 100.0, -100.0][usize::from(*step)])
        }
        WfOp::References(step) => Command::SetReferences(match step {
            0 => Vec::new(),
            1 => vec![atomik_model::Reference::new("one.wav")],
            2 => vec![
                atomik_model::Reference::new("one.wav"),
                atomik_model::Reference {
                    path: "two.wav".into(),
                    cue_ms: Some(1500),
                },
            ],
            _ => vec![atomik_model::Reference::new("three.wav")],
        }),
        WfOp::Delivery(step) => Command::SetDelivery(
            atomik_model::TARGETS
                .get(usize::from(*step))
                .map(atomik_model::Delivery::of),
        ),
        WfOp::Video(kind, v) => {
            let facts = |secs: u64| atomik_model::VideoFacts {
                duration_us: secs * 1_000_000,
                fps_num: 25,
                fps_den: 1,
                width: 640,
                height: 360,
                timecode: None,
                has_audio: false,
            };
            let at = i64::from(*v) * 1_500_000 - 2_000_000;
            Command::Video(match kind {
                0 => atomik_model::VideoCommand::Set(atomik_model::VideoClip::new(
                    format!("film-{v}.mov"),
                    facts(30 + u64::from(*v)),
                )),
                1 => atomik_model::VideoCommand::Remove,
                2 => atomik_model::VideoCommand::Move {
                    start_us: at,
                    merge: None,
                },
                3 => atomik_model::VideoCommand::Trim {
                    in_us: u64::from(*v) * 1_000_000,
                    out_us: 20_000_000 + u64::from(*v) * 1_000_000,
                    merge: None,
                },
                4 => atomik_model::VideoCommand::Relink {
                    path: format!("copy-{v}.mov"),
                    facts: facts(25 + u64::from(*v)),
                },
                _ => atomik_model::VideoCommand::Move {
                    start_us: at,
                    merge: Some(1),
                },
            })
        }
        WfOp::Sort(mode) => Command::SetRackSort(
            [RackSort::Free, RackSort::PerTrack, RackSort::AutoGroup][usize::from(*mode)],
        ),
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
        WfOp::Combine(s) => Command::Container(atomik_model::ContainerCommand::Combine {
            keys: chosen(&top, s),
            name: String::new(),
        }),
        WfOp::Uncombine(i) => {
            let containers: Vec<DeviceKey> = doc
                .devices
                .iter()
                .filter(|x| x.inner.is_some())
                .map(|x| x.key.clone())
                .collect();
            Command::Container(atomik_model::ContainerCommand::Uncombine {
                container: pick(&containers, *i)?,
            })
        }
        WfOp::MoveIn(d, c, at) => {
            let containers: Vec<DeviceKey> = doc
                .devices
                .iter()
                .filter(|x| x.inner.is_some())
                .map(|x| x.key.clone())
                .collect();
            Command::Container(atomik_model::ContainerCommand::MoveIntoContainer {
                key: pick(&top, *d)?,
                container: pick(&containers, *c)?,
                index: *at,
            })
        }
        WfOp::MoveOut(c, at) => {
            let inside: Vec<DeviceKey> = doc
                .devices
                .iter()
                .filter_map(|x| x.inner.as_ref().map(|i| (&x.key, i)))
                .flat_map(|(k, i)| {
                    i.devices
                        .iter()
                        .map(move |d| key(&format!("{k}/{}", d.key)))
                })
                .collect();
            Command::Container(atomik_model::ContainerCommand::MoveOutOfContainer {
                key: pick(&inside, *c)?,
                index: *at,
            })
        }
        WfOp::Warp(kind, c, gaps) => {
            let id = pick(&ids, *c)?;
            let (_, _, clip) = doc.song.clip(id)?;
            let atomik_model::ClipContent::Audio(a) = &clip.content else {
                return None;
            };
            match kind {
                0 => {
                    let (mut tick, mut source) = (0i64, 0u64);
                    let mut markers = Vec::new();
                    for (dt, ds) in gaps {
                        tick += *dt;
                        source += *ds;
                        markers.push(atomik_model::WarpMarker { tick, source });
                    }
                    sc(SongCommand::SetWarpMarkers {
                        id,
                        markers,
                        merge: None,
                    })
                }
                1 => {
                    // One Batch of splits at the markers, each on the right piece of the last.
                    let next = doc.song.next_clip_id;
                    let mut commands = Vec::new();
                    let mut current = id;
                    for (k, tick) in atomik_model::slice_edit::split_ticks(&a.warp, clip.length)
                        .into_iter()
                        .enumerate()
                    {
                        commands.push(sc(SongCommand::SplitClip {
                            id: current,
                            at: clip.start + tick,
                        }));
                        current = ClipId(next + k as u64);
                    }
                    if commands.is_empty() {
                        return None;
                    }
                    Command::Batch(commands)
                }
                _ => Command::RenameAsset {
                    asset: a.asset.clone(),
                    name: format!("take {}", gaps.len()),
                },
            }
        }
        WfOp::NoteTool(c, kind, seed) => sc(SongCommand::ApplyNoteTool {
            clips: vec![pick(&ids, *c)?],
            selection: None,
            tool: match kind {
                0 => NoteTool::SetVelocity(0.3),
                1 => NoteTool::ScaleVelocity(1.5),
                2 => NoteTool::RampVelocity {
                    start: None,
                    end: Some(0.2),
                },
                3 => NoteTool::RandomizeVelocity {
                    amount: 0.6,
                    seed: *seed,
                },
                4 => NoteTool::AlterNotes {
                    chance: 0.7,
                    seed: *seed,
                },
                5 => NoteTool::RandomizePitch {
                    range: 12,
                    seed: *seed,
                },
                _ => NoteTool::SetPitch((*seed % 128) as u8),
            },
        }),
        WfOp::Lanes(kind, a, b, c, d) => match kind {
            0 | 1 => sc(SongCommand::MoveNoteLane {
                device: pick(&tracks, *a)?,
                lane: *c,
                to_device: pick(&tracks, *b)?,
                to_index: *d,
                copy: *kind == 1,
            }),
            2 => sc(SongCommand::MergeNoteLanes {
                devices: vec![pick(&tracks, *a)?, pick(&tracks, *b)?],
            }),
            3 => Command::Workflow(atomik_model::WorkflowCommand::ExplodeNotes {
                clip: pick(&ids, *a)?,
            }),
            5 => sc(SongCommand::SetLaneSolo {
                device: pick(&tracks, *a)?,
                lane: *c,
                solo: *d % 2 == 0,
            }),
            6 => sc(SongCommand::SetLaneColor {
                device: pick(&tracks, *a)?,
                lane: *c,
                color: (*d % 3 != 0).then_some([10 * (*d as u8), 40, 90]),
            }),
            7 | 8 => sc(SongCommand::SetTakeLoop {
                group: (*b % 3) as u32 + 1,
                take: *c,
                repeats: (*d as u32 % 4) + 1,
            }),
            _ => Command::Workflow(atomik_model::WorkflowCommand::ExtractNotes {
                clip: pick(&ids, *a)?,
                selection: vec![*c, *d],
                mode: if *kind == 4 {
                    atomik_model::ExtractMode::Copy
                } else {
                    atomik_model::ExtractMode::Move
                },
                to: (*b % 3 == 0).then_some(*d),
            }),
        },
        WfOp::TrackCtl(kind, t, step) => {
            let at = i64::from(*step) * PPQ;
            let lane = |kind, a: f32, b: f32| {
                atomik_model::controller::ControllerLane::new(
                    kind,
                    vec![point(at, a, Curve::Linear), point(at + PPQ, b, Curve::Step)],
                )
            };
            let bend = lane(
                atomik_model::controller::ControllerKind::PitchBend,
                -0.5,
                1.0,
            );
            let wheel = lane(atomik_model::controller::ControllerKind::Cc(1), 0.25, 0.75);
            sc(SongCommand::SetTrackControllers {
                device: tracks.get(*t % tracks.len().max(1))?.clone(),
                lanes: match kind {
                    0 => vec![bend],
                    1 => vec![wheel],
                    2 => vec![bend, wheel],
                    _ => Vec::new(),
                },
                merge: None,
            })
        }
        WfOp::Groove(kind, t, strength) => {
            let song = |c| Command::Song(c);
            let custom = |name: &str| atomik_model::Groove {
                name: name.to_owned(),
                step: atomik_model::StepResolution::Sixteenth,
                slots: vec![
                    atomik_model::GrooveSlot::STRAIGHT,
                    atomik_model::GrooveSlot {
                        offset: 0.25,
                        velocity: 0.8,
                    },
                ],
            };
            match kind {
                0 => song(SongCommand::SetGroove(custom("Custom"))),
                1 | 2 => song(SongCommand::SetTrackGroove {
                    device: tracks.get(*t % tracks.len().max(1))?.clone(),
                    groove: Some(atomik_model::TrackGroove {
                        name: if *kind == 1 { "Swing 55%" } else { "Custom" }.to_owned(),
                        strength: *strength,
                    }),
                }),
                3 => song(SongCommand::SetTrackGroove {
                    device: tracks.get(*t % tracks.len().max(1))?.clone(),
                    groove: None,
                }),
                4 => song(SongCommand::RenameGroove {
                    from: "Custom".to_owned(),
                    to: "Renamed".to_owned(),
                }),
                6 | 7 => song(SongCommand::SetClipGroove {
                    clips: vec![pick(&ids, *t)?],
                    groove: (*kind == 6).then(|| atomik_model::TrackGroove {
                        name: "Swing 55%".to_owned(),
                        strength: *strength,
                    }),
                }),
                _ => song(SongCommand::DeleteGroove {
                    name: if *t % 2 == 0 { "Custom" } else { "Renamed" }.to_owned(),
                }),
            }
        }
        WfOp::Quantize(c, grid, ends) => sc(SongCommand::ApplyQuantize {
            clips: vec![pick(&ids, *c)?],
            quantize: QuantizeParams {
                grid: [1, 7, PPQ / 4, PPQ, 1 << 20][usize::from(*grid) % 5],
                snap_ends: *ends,
                legato: *ends,
                humanize_length: if *grid % 2 == 0 { 0.5 } else { 0.0 },
                grid_by_length: *grid % 3 == 0,
                ..QuantizeParams::default()
            },
        }),
    })
}

/// A launcher command in abstract form: indices wrap around what the document holds.
#[derive(Debug, Clone)]
enum LauncherOp {
    New(usize, usize),
    Put(usize, usize, usize),
    Move(usize, usize, usize, usize, bool),
    Clear(usize, usize),
    Edit(usize, usize, u8),
    Scene(u8, usize),
    Settings(u8),
    ToArrangement(usize, usize, i64),
    EditClip(usize, usize, u8),
}

fn launcher_op() -> impl Strategy<Value = LauncherOp> {
    prop_oneof![
        (0usize..4, 0usize..5).prop_map(|(t, r)| LauncherOp::New(t, r)),
        (0usize..4, 0usize..5, 0usize..4).prop_map(|(t, r, c)| LauncherOp::Put(t, r, c)),
        (0usize..4, 0usize..5, 0usize..4, 0usize..5, any::<bool>())
            .prop_map(|(a, b, c, d, copy)| LauncherOp::Move(a, b, c, d, copy)),
        (0usize..4, 0usize..5).prop_map(|(t, r)| LauncherOp::Clear(t, r)),
        (0usize..4, 0usize..5, 0u8..8).prop_map(|(t, r, k)| LauncherOp::Edit(t, r, k)),
        (0u8..5, 0usize..6).prop_map(|(k, at)| LauncherOp::Scene(k, at)),
        (0u8..4).prop_map(LauncherOp::Settings),
        (0usize..4, 0usize..5, 0i64..8).prop_map(|(t, r, b)| LauncherOp::ToArrangement(
            t,
            r,
            b * BAR
        )),
        (0usize..4, 0usize..5, 0u8..4).prop_map(|(t, r, k)| LauncherOp::EditClip(t, r, k)),
    ]
}

fn resolve_launcher(op: &LauncherOp, doc: &RackDoc) -> Option<Command> {
    use atomik_model::{LaunchQuantize, LauncherCommand as L, MetaChange};
    let tracks: Vec<DeviceKey> = doc
        .song
        .tracks
        .iter()
        .filter(|t| !t.is_audio() && !t.is_automation_only())
        .map(|t| t.device.clone())
        .collect();
    let slots: Vec<_> = doc
        .song
        .launcher
        .clips()
        .map(|(d, r, _)| (d.clone(), r))
        .collect();
    let ids = clip_ids(doc);
    Some(sc(SongCommand::Launcher(match op {
        LauncherOp::New(t, r) => L::NewClip {
            device: pick(&tracks, *t)?,
            row: *r,
            length: BAR,
        },
        LauncherOp::Put(t, r, c) => L::PutClip {
            device: pick(&tracks, *t)?,
            row: *r,
            clip: pick(&ids, *c)?,
        },
        LauncherOp::Move(a, b, c, d, copy) => L::MoveSlot {
            from: pick(&slots, a + b)?,
            to: (pick(&tracks, *c)?, *d),
            copy: *copy,
        },
        LauncherOp::Clear(t, r) => L::ClearSlots {
            slots: vec![pick(&slots, t + r)?],
        },
        LauncherOp::Edit(t, r, k) => {
            let (device, row) = pick(&slots, t + r)?;
            L::EditSlot {
                device,
                row,
                name: if *k % 2 == 0 {
                    MetaChange::Set(format!("S{k}"))
                } else {
                    MetaChange::Clear
                },
                color: if *k % 3 == 0 {
                    MetaChange::Set([*k, 2, 3])
                } else {
                    MetaChange::Keep
                },
                length: (*k % 4 == 0).then_some(BAR * 2),
                quantize: (*k % 5 == 0).then_some(Some(LaunchQuantize::Eighth)),
            }
        }
        LauncherOp::Scene(k, at) => match k {
            0 => L::InsertScene { at: *at },
            1 => L::DeleteScene { at: *at },
            2 => L::DuplicateScene { at: *at },
            3 => L::RenameScene {
                at: *at,
                name: Some(format!("Scene {at}")),
            },
            _ => L::SetSceneTempo {
                at: *at,
                tempo: Some(90.0 + *at as f64),
            },
        },
        LauncherOp::Settings(k) => match k {
            0 => L::SetQuantize(LaunchQuantize::TwoBars),
            1 => L::SetQuantize(LaunchQuantize::Bar),
            2 => L::SetStopEmpty(false),
            _ => L::SetStopEmpty(true),
        },
        LauncherOp::ToArrangement(t, r, start) => {
            let (device, row) = pick(&slots, t + r)?;
            L::ToArrangement {
                device,
                row,
                start: *start,
                onto: None,
            }
        }
        LauncherOp::EditClip(t, r, k) => {
            let (device, row) = pick(&slots, t + r)?;
            let id = doc.song.launcher.slot(&device, row)?.clip.id;
            return Some(sc(SongCommand::SetClipContent {
                id,
                content: atomik_model::ClipContent::Notes(
                    vec![note(0, PPQ / 4, 60 + *k, 0.8)].into(),
                ),
                merge: None,
            }));
        }
    })))
}

/// A harmony edit in abstract form: it builds the new harmony from the current one with the model's
/// own helpers, so refusals (an overlap) simply skip the step.
#[derive(Debug, Clone)]
enum HarmonyOp {
    Key(u8, usize),
    NoKey,
    Change(i64, u8, bool),
    Chord(i64, i64, u8, usize, Option<u8>),
    RemoveChord(usize),
    MoveChord(usize, i64),
    Drag(usize, i64),
}

fn harmony_op() -> impl Strategy<Value = HarmonyOp> {
    prop_oneof![
        (0u8..12, 0usize..14).prop_map(|(r, s)| HarmonyOp::Key(r, s)),
        Just(HarmonyOp::NoKey),
        (0i64..16, 0u8..12, any::<bool>()).prop_map(|(b, r, rm)| HarmonyOp::Change(b, r, rm)),
        (
            0i64..16,
            1i64..4,
            0u8..12,
            0usize..21,
            proptest::option::of(0u8..12)
        )
            .prop_map(|(b, l, r, q, bass)| HarmonyOp::Chord(b, l, r, q, bass)),
        (0usize..8).prop_map(HarmonyOp::RemoveChord),
        (0usize..8, 0i64..16).prop_map(|(i, b)| HarmonyOp::MoveChord(i, b)),
        (0usize..8, 0i64..16).prop_map(|(i, b)| HarmonyOp::Drag(i, b)),
    ]
}

fn resolve_harmony(op: &HarmonyOp, doc: &RackDoc) -> Option<Command> {
    use atomik_model::theory::{ChordKind, ScaleKind};
    use atomik_model::{ChordEvent, Key};
    let h = &doc.song.harmony;
    let scales: Vec<ScaleKind> = ScaleKind::ALL[1..].to_vec();
    let (harmony, merge) = match op {
        HarmonyOp::Key(r, s) => (
            h.with_key(Some(Key::new(*r, scales[*s % scales.len()]))),
            None,
        ),
        HarmonyOp::NoKey => (h.with_key(None), None),
        HarmonyOp::Change(b, r, rm) => (
            h.with_key_change(*b * BAR, (!rm).then(|| Key::new(*r, ScaleKind::Major))),
            None,
        ),
        HarmonyOp::Chord(b, l, r, q, bass) => {
            let mut c = ChordEvent::new(*b * BAR, *l * BAR, *r, ChordKind::ALL[*q % 21]);
            c.bass = *bass;
            (h.with_chord(c), None)
        }
        HarmonyOp::RemoveChord(i) => {
            let at = h.chords.get(*i % h.chords.len().max(1))?.tick;
            (Ok(h.without_chord(at)), None)
        }
        HarmonyOp::MoveChord(i, b) | HarmonyOp::Drag(i, b) => {
            let old = *h.chords.get(*i % h.chords.len().max(1))?;
            let mut c = old;
            c.tick = *b * BAR;
            let merge = matches!(op, HarmonyOp::Drag(..)).then_some(5);
            (h.with_chord_replaced(old.tick, c), merge)
        }
    };
    let harmony = harmony.ok()?;
    Some(sc(match merge {
        Some(merge) => SongCommand::DragHarmony { harmony, merge },
        None => SongCommand::SetHarmony(harmony),
    }))
}

/// One step of a run.
#[derive(Debug, Clone)]
enum Step {
    Song(SongOp),
    Console(ConsoleOp),
    Audio(AudioOp),
    Wf(WfOp),
    Launcher(LauncherOp),
    Harmony(HarmonyOp),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => song_op().prop_map(Step::Song),
        1 => console_op().prop_map(Step::Console),
        2 => audio_op().prop_map(Step::Audio),
        4 => wf_op().prop_map(Step::Wf),
        2 => launcher_op().prop_map(Step::Launcher),
        2 => harmony_op().prop_map(Step::Harmony),
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
        Step::Launcher(o) => resolve_launcher(o, doc),
        Step::Harmony(o) => resolve_harmony(o, doc),
    }
}

/// The document as compared across undo and redo (see the module docs).
fn norm(doc: &RackDoc) -> RackDoc {
    let mut d = doc.clone();
    d.song_mut().track_heights.clear();
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
                    let undone = d.undo();
                    prop_assert!(undone.is_some(), "{:?}", cmd);
                    prop_assert!(norm(d.doc()) == before, "undo of {:?}", cmd);
                    prop_assert!(d.doc().validate().is_ok(), "undo of {:?}", cmd);
                    let redone = d.redo();
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
    while d.undo().is_some() {
        prop_assert!(d.doc().validate().is_ok());
    }
    prop_assert!(norm(d.doc()) == original, "undo of everything");
    while d.redo().is_some() {
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

#[test]
fn moving_a_channel_after_a_folder_with_a_following_group_keeps_the_invariants() {
    run_sequence(&[
        Step::Wf(WfOp::Folder(vec![5, 2])),
        Step::Console(ConsoleOp::VcaForFolder(0)),
        Step::Console(ConsoleOp::FollowFolder(0, None)),
        Step::Console(ConsoleOp::MoveChannel(30, 1)),
    ])
    .expect("the sequence keeps the invariants");
}
