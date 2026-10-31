//! Workflow commands: multi-device edits and the clipboard pastes, each
//! planned on a scratch copy of the document and committed as one undo step.
//!
//! Every command builds its edits on a [`Planner`], which performs each edit on a scratch copy
//! as it is planned, so each cable, key and limit is validated against the state it joins.
//! A command that fails anywhere is rejected whole and the document is untouched. Keys
//! of pasted devices come from the existing generators (`generate_key_in`, the plugin key base)
//! in rack order, clip ids from the song's next free id upward in snapshot order, and the
//! console channel of a pasted instrument from the rule (`add_channel`), all inside the
//! same step, so the same inputs always give the same document.

use crate::console::{MASTER_TYPE, is_console_type};
use crate::console_cmd::{Planner, add_channel, console_available, is_instrument};
use crate::container::{SPLIT_TYPE, split_path};
use crate::controller::{ControllerKind, ControllerLane, canonical, window_points};
use crate::history::Edit;
use crate::plugin::{MAX_PROJECT_PLUGIN_STATE_BYTES, key_base_of_name};
use crate::rack_columns::{self, Placement};
use crate::song::{AUDIO_TRACK_TYPE, AutomationPoint, Clip, ClipContent, ClipId, LaneKind, Note};
use crate::song_cmd::{
    check_audio, check_content, check_geometry, has_notes_input, is_audio_track_type,
};
use crate::workflow::{ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot};
use crate::workflow_create::{self, CreateOptions, CreateSource, CreateTarget};
use crate::{
    Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, RackSlot, SocketRef,
    console_cmd, container_cmd, rules, zones,
};
use atomik_device_sdk::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// A workflow edit (wrapped in [`crate::Command::Workflow`]). Every command is one undo step; a
/// rejected command changes nothing. Keys are path keys (`container/device` inside a
/// container); a command that takes several keys needs them all in one scope.
///
/// Sync ops are those of the compound steps: `DeviceAdded` for each pasted device (with its
/// stored params), `RoutingChanged`, `ViewChanged` for a move, `ConsoleChanged` and
/// `SongChanged` when channels and tracks come along, and `DeviceRemoved` after
/// `RoutingChanged` for a removal. The commands open no drag-merge group and use no merge id.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowCommand {
    /// Removes the devices, each as [`crate::Command::RemoveDevice`] does it: its cables, its
    /// song track, its console channel and the rest of the console bookkeeping. Devices inside
    /// a removed container are not removed twice; a key that goes away with another (a channel
    /// with its instrument) is fine. One unknown key rejects the command.
    RemoveDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
    /// Copies the devices (with the cables between them) and pastes the copies right after the
    /// last selected one (after its console channel). With `route` a copied effect or note
    /// player that has no cable is routed as if created at its place (see
    /// [`WorkflowCommand::MoveDevices`]).
    DuplicateDevices {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
        /// Route the copies by the creation rules (the Workflow setting `auto_route`; Shift
        /// inverts it). Without it the copies are only placed.
        route: bool,
    },
    /// Pastes a snapshot into the top level (or into `container`, a path key) at rack index `at`
    /// (clamped), keeping the snapshot's order. The copies get fresh keys and names, keep their
    /// params, zones, patterns, inner containers and plugin documents, keep the cables between
    /// them and drop every other cable. A copied instrument that feeds no other copied device
    /// gets its console channel and the master when missing; the tracks of the snapshot
    /// come back with fresh clip ids and are not armed; a copied audio track is wired to the
    /// interface inputs like a new one. The container, plugin and inner-device limits apply and
    /// every asset the snapshot needs must be in the document's pool. With `route` a copied
    /// effect or note player that ends up without a cable is routed as if created at its place
    /// (the effect rules of `CreateEffectAt`; a player feeds the device below it).
    PasteDevices {
        /// What to paste.
        snapshot: DeviceSnapshot,
        /// The container to paste into, or `None` for the top level.
        container: Option<DeviceKey>,
        /// Rack index of the first pasted device.
        at: usize,
        /// Route the copies (the Workflow setting `auto_route`; Shift inverts it).
        route: bool,
    },
    /// Moves the devices together: they keep their relative order and end up contiguous with the
    /// first at index `to` among the other devices of the scope (clamped so the group fits). A
    /// move that changes nothing is rejected.
    ///
    /// With `route`, each top-level effect and note player whose cables are the ones the
    /// creation rules make is re-routed as if created at its new place, in the same undo step:
    /// its path is bridged where it was (an insert chain closes up, a send/return slot is
    /// freed, the tracks that entered at a player enter at its successor), then the rules of
    /// `CreateEffectAt` run at the new place (a player feeds the device below it). A route the
    /// rules refuse leaves the move plain, and devices with other cables are only moved.
    MoveDevices {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
        /// Target index.
        to: usize,
        /// Re-route the devices (the Workflow setting `auto_route`; Shift keeps the cables).
        route: bool,
    },
    /// Moves the devices together to a rack column slot ([`RackSlot`]): they keep their
    /// relative rack order and end up contiguous at `slot.index` among the devices that stay
    /// in `slot.column` (all take that column). `slot.column == columns` appends a new column in
    /// the same undo step; column 0 never goes above the Master Section. Inside a container only
    /// `slot.index` counts (like [`WorkflowCommand::MoveDevices`]). A move that changes nothing
    /// is rejected. One undo step; reports `ViewChanged` only.
    MoveDevicesToSlot {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
        /// Target slot.
        slot: RackSlot,
        /// Re-route the devices, as for [`WorkflowCommand::MoveDevices`].
        route: bool,
    },
    /// [`WorkflowCommand::PasteDevices`] at a rack column slot: the copies land at
    /// `slot.index` of `slot.column` (a column past the last appends one). Inside a
    /// `container` only `slot.index` counts.
    PasteDevicesAt {
        /// What to paste.
        snapshot: DeviceSnapshot,
        /// The container to paste into, or `None` for the top level.
        container: Option<DeviceKey>,
        /// Where the first copy goes.
        slot: RackSlot,
        /// Route the copies, as for [`WorkflowCommand::PasteDevices`].
        route: bool,
    },
    /// [`WorkflowCommand::CreateInstrument`] at a rack column slot (see [`crate::Command::AddDeviceAt`]
    /// for how the slot resolves). The instrument's console channel lands by the console rules.
    CreateInstrumentAt {
        /// Where the instrument goes.
        slot: RackSlot,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options.
        options: CreateOptions,
    },
    /// [`WorkflowCommand::CreateEffect`] at a rack column slot: the effect is inserted exactly
    /// there and the routing rules run as if the device directly above the slot (the previous
    /// device of that column) were selected. A slot at the top of its column has no such
    /// device: the effect is only added, without auto-route. Instruments and console devices
    /// are rejected.
    CreateEffectAt {
        /// Where the effect goes.
        slot: RackSlot,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options (`skip_track` and `disarm` do not apply to effects).
        options: CreateOptions,
    },
    /// Replaces a top-level instrument, effect or note player by a device of the same kind that
    /// takes its place, its key and what hangs on it: the song track (clips, lanes, armed
    /// state), the console channel with its sends, the note entry of every track, the cables
    /// whose sockets the new device has, the insert-chain or send/return place and the column.
    /// The params and the sample, pattern and plugin data are the new device's own defaults,
    /// the cables on sockets it lacks go, and so do the automation lanes of params it lacks
    /// (see [`crate::replace_lost_lanes`]). A default-named device and channel take the new
    /// name. Console devices, containers, devices inside a container, devices of a frozen track
    /// and a replacement of another kind are rejected. One undo step.
    ReplaceDevice {
        /// The device to replace.
        target: DeviceKey,
        /// What takes its place.
        source: CreateSource,
    },
    /// Duplicates a device together with its song track, lanes and clips (fresh clip ids, not
    /// armed, a console channel by the instrument rule), right after the device and its
    /// channel.
    DuplicateTrack {
        /// The device that has the track.
        device: DeviceKey,
    },
    /// [`WorkflowCommand::DuplicateTrack`] for several devices, in one undo step: each copy
    /// comes right after its device (and the device's channel) in the rack and the copies are
    /// not armed. In the track list the copies stay together: right after the last duplicated
    /// track, in its folder, or, with `place`, where [`crate::SongCommand::MoveTracks`] would
    /// put them (the drop of a Ctrl-drag). A `place` the copies cannot take is an error.
    DuplicateTracks {
        /// The devices that have the tracks.
        devices: Vec<DeviceKey>,
        /// Where the copies go, or `None` to leave them next to their originals.
        place: Option<crate::TrackPlace>,
    },
    /// Folds or unfolds every listed device (any scope) in one undo step; the ones that are in
    /// that state already are left alone, and a command that changes none is rejected.
    SetFoldedMany {
        /// The devices.
        keys: Vec<DeviceKey>,
        /// The new state.
        folded: bool,
    },
    /// Removes every cable of the device (a device without cables is rejected).
    DisconnectDevice {
        /// The device.
        key: DeviceKey,
    },
    /// Pastes clips with their track-relative layout: the earliest clip starts at `at_tick`.
    /// Each clip goes to the lane of its kind on its source track, or on `track` when given
    /// (a lane of the same kind must exist there). A note clip goes to note lane `lane` when
    /// given (it must be a note lane of the target track), else to the note lane with the
    /// ordinal it had when copied (the last one if the track has fewer). New ids are allocated from the song's next
    /// free id in snapshot order (the last is [`crate::Document::last_clip_id`]).
    PasteClips {
        /// The track that takes every clip, or `None` for the source tracks.
        track: Option<DeviceKey>,
        /// The note lane that takes every note clip, or `None` to keep each clip's lane.
        lane: Option<usize>,
        /// Song tick of the earliest pasted clip.
        at_tick: i64,
        /// What to paste.
        clips: ClipSnapshot,
    },
    /// Copies the clips right after the selection, keeping their layout: the earliest copy
    /// starts where the selection ends.
    DuplicateClips {
        /// The clips.
        ids: Vec<ClipId>,
    },
    /// Puts clips on other tracks and ticks, or copies of them (a drag across lanes, one undo
    /// step). Each clip goes to the lane of its own kind on its target track (a note clip to a
    /// note lane, an audio clip to the audio lane, an automation clip to the lane of its
    /// param); a track without such a lane rejects the whole command. A note clip goes to
    /// [`ClipMove::lane`] when given, else stays on its note lane when the track does not
    /// change, else goes to the note lane with the same ordinal (the last one if the target
    /// has fewer). With `copy` the clips stay
    /// and the copies get new ids from the song's next free id in `moves` order (the last is
    /// [`crate::Document::last_clip_id`]). `merge` joins the move to the drag-merge group of the
    /// `SetClipGeometry` commands of the same drag. Unknown clips and an empty list reject it.
    MoveClips {
        /// Where each clip goes.
        moves: Vec<ClipMove>,
        /// Copy instead of move.
        copy: bool,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Removes the clips (one undo step; `SongCommand::RemoveClip` cannot be batched, so a
    /// selection of several clips goes through this command). Unknown clips reject it.
    RemoveClips {
        /// The clips.
        ids: Vec<ClipId>,
    },
    /// Sets `muted` on every listed clip (note, automation and audio clips) in one undo step.
    /// An unknown clip rejects the command, and so does one that would change nothing (every
    /// clip already has that state) or an empty list.
    SetClipsMuted {
        /// The clips.
        ids: Vec<ClipId>,
        /// The new mute state.
        muted: bool,
    },
    /// Joins note or automation clips of one lane into the first of them (it keeps
    /// its id), which then spans all of them. Each clip contributes what it shows (its content
    /// window, notes cut at the window end); where two notes or points coincide the later
    /// clip wins. A looped clip is unrolled (each repetition contributes what it plays). Audio
    /// clips and clips of different lanes are rejected. Muted
    /// clips of a mixed selection are left alone (their notes must not become audible); muted
    /// clips alone join into a muted clip.
    JoinClips {
        /// At least two clips.
        ids: Vec<ClipId>,
    },
    /// Moves the start of every listed note or automation clip by `start_delta` ticks (negative:
    /// earlier) and gives the length back or takes it, so the clip ends where it did (one undo
    /// step). The content stays where it is in song time: the offset moves by the delta. A
    /// clip that grows left past content tick 0 shifts its notes, controller points or
    /// automation points right to make room (a looped clip wraps its loop instead). Audio clips,
    /// a start below 0 and a length that would fall to 0 reject the command. `merge` joins the
    /// trim to the drag-merge group of the `SetClipGeometry` commands of the same drag.
    TrimClips {
        /// The clips.
        ids: Vec<ClipId>,
        /// Ticks the start moves by.
        start_delta: i64,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Removes what the listed note and automation clips do not play: notes that start outside
    /// the clip's content window (`[offset, offset + length)`, or the loop window of a looped
    /// clip) and automation or controller points outside it, keeping the nearest point on each
    /// side so the curve at the window edges stays what it was. Audio clips are skipped. One
    /// undo step; a command that would change nothing rejects.
    DeleteOutsideEvents {
        /// The clips.
        ids: Vec<ClipId>,
    },
    /// Splits every listed clip that `at` falls strictly inside at that tick, in one undo step.
    /// The new right halves take ids from the song's next free id in song order (clips sorted
    /// by start, then id; the last is [`crate::Document::last_clip_id`]). A command that
    /// splits nothing rejects.
    SplitClips {
        /// The clips.
        ids: Vec<ClipId>,
        /// Song tick.
        at: i64,
    },
    /// Cuts the song range `[start, end)` out of every listed clip it touches (a razor drag):
    /// the part inside goes, the parts outside stay (a clip the range lies inside leaves two
    /// clips, the right one with a new id). With `copy` the clips stay as they are and a copy
    /// of each clip's part inside the range is placed so the range starts at `end`. One undo
    /// step; new ids come from the song's next free id in song order.
    CutOut {
        /// The clips.
        ids: Vec<ClipId>,
        /// Start of the range, in song ticks.
        start: i64,
        /// End of the range (greater than `start`).
        end: i64,
        /// Copy the range instead of removing it.
        copy: bool,
    },
    /// Reverses the listed clips in time, in one undo step. A note clip mirrors the notes that
    /// start inside what it shows (the loop of a looped clip), and its controller points; with
    /// `graphical` each note is mirrored as a bar (its end becomes its start), otherwise only
    /// the starts are mirrored and every note keeps its length. An automation clip mirrors its
    /// points. Audio clips play backwards, or forwards again when they all already do. Two
    /// reverses give the clip back. A selection that would not change rejects.
    ReverseClips {
        /// The clips (any mix of notes, automation and audio).
        ids: Vec<ClipId>,
        /// Mirror notes as bars instead of mirroring their starts.
        graphical: bool,
    },
    /// Changes the tempo of the listed clips to `percent` per cent of what it was: 200 plays them
    /// twice as fast (half as long), 50 half as fast. Clip starts scale relative to the
    /// earliest start of the selection, lengths, loops, notes and controller and automation
    /// points scale with them (positions round to the nearest tick), and audio clips are warped
    /// to follow (an audio clip that plays backwards rejects the command). The percentage is
    /// `SCALE_PERCENT_MIN..=SCALE_PERCENT_MAX` and not 100. One undo step.
    ScaleTempo {
        /// The clips.
        ids: Vec<ClipId>,
        /// The new tempo as a percentage of the old one.
        percent: f64,
    },
    /// Cuts every listed clip longer than `length` into pieces of `length` ticks from its own
    /// start (the last piece takes what is left). The new pieces take ids from the song's next
    /// free id in song order. One undo step; a selection with nothing to cut rejects.
    ChopClips {
        /// The clips.
        ids: Vec<ClipId>,
        /// Piece length in ticks (above 0).
        length: i64,
    },
    /// Opens `length` ticks of empty time at `at`: a clip that straddles `at` is split there,
    /// and everything from `at` on moves `length` later (clips, take groups, markers, ranges,
    /// tempo and meter changes, the loop and the song End). One undo step; rejected while a
    /// track is frozen. The exact rules are in the `time_edit` module docs.
    InsertTime {
        /// Song tick where the time opens.
        at: i64,
        /// Ticks to open (above 0).
        length: i64,
    },
    /// Cuts `[at, at + length)` out of the whole arrangement and closes the gap: the parts of
    /// clips inside the range go, the rest moves `length` earlier, and the tempo and meter in
    /// effect at the end of the range carry on from `at`. The inverse of
    /// [`WorkflowCommand::InsertTime`] for a range that holds nothing. One undo step; rejected
    /// while a track is frozen.
    RemoveTime {
        /// Start of the range, in song ticks.
        at: i64,
        /// Length of the range in ticks (above 0).
        length: i64,
    },
    /// Merges a recorded take into the note clip under its start (a *Layer* recording): the
    /// clip keeps its id, start and offset, grows to hold the take, keeps every old note, takes
    /// the take's notes (a new note replaces an old one with the same start and key) and, for
    /// each controller kind the take carries, its points over the take's span. The clip must be
    /// an unlooped note clip and the take (a note clip, content offset 0 as a recording makes
    /// it) must start inside it. One undo step; the song End follows a longer clip. See
    /// [`crate::record_over`] for the rules.
    LayerRecording {
        /// The clip that takes the notes.
        clip: ClipId,
        /// The recorded clip (its id is ignored; it is not added).
        take: Clip,
    },
    /// Adds a recorded take as a new note clip and makes it the clip that plays over its span (an
    /// *Engulf*): an unmuted clip that starts inside the take and ends within it is removed, one
    /// that starts inside it and ends after it has its start trimmed to the take's end (content
    /// offset advanced, a loop keeps its phase). Clips that start before the take are not
    /// touched. One undo step; the new id is reported like `AddClip`'s.
    EngulfRecording {
        /// Device (track).
        device: DeviceKey,
        /// The note lane.
        lane: usize,
        /// The recorded clip (id ignored).
        clip: Clip,
    },
    /// Pastes notes into a note clip so the earliest starts at song tick `at_tick`. A pasted
    /// note replaces a note with the same start and key. The clip keeps its length.
    PasteNotes {
        /// The note clip.
        clip: ClipId,
        /// Song tick of the earliest pasted note.
        at_tick: i64,
        /// What to paste.
        notes: NoteSnapshot,
    },
    /// Copies the notes at `indices` (positions in the clip's note list) right after the
    /// selection.
    DuplicateNotes {
        /// The note clip.
        clip: ClipId,
        /// Note positions.
        indices: Vec<usize>,
    },
    /// Pastes automation points into an automation clip so the earliest is at song tick
    /// `at_tick`; a pasted point replaces a point at the same tick.
    PasteAutomationPoints {
        /// The automation clip.
        clip: ClipId,
        /// Song tick of the earliest pasted point.
        at_tick: i64,
        /// What to paste.
        points: PointSnapshot,
    },
    /// Copies the points at `indices` right after the selection (by the selection's span).
    DuplicatePoints {
        /// The automation clip.
        clip: ClipId,
        /// Point positions.
        indices: Vec<usize>,
    },
    /// Creates an instrument that wires itself, in one undo step: the
    /// device (with its console channel and the master), its song track when it has a
    /// Notes input, the track armed, and the tracks in `options.disarm` (the ones an earlier
    /// auto-route armed) disarmed. `options.route == false` is exactly `AddDevice` (or
    /// `PluginCommand::Add`); `options.skip_track` leaves the track out. `index` is clamped and
    /// `key` generated when `None`. Anything that is not an instrument is rejected.
    CreateInstrument {
        /// Rack position of the device (clamped).
        index: usize,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options.
        options: CreateOptions,
    },
    /// Creates an effect and routes it by the rules of
    /// the `workflow_create` module (first free insert slot of the selected channel, on the
    /// selected FX return, after the selected device, else only added), in one undo step.
    /// Instruments and console devices are rejected.
    CreateEffect {
        /// What to add.
        source: CreateSource,
        /// The selection the effect is routed from.
        target: CreateTarget,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options (`skip_track` and `disarm` do not apply to effects).
        options: CreateOptions,
    },
    /// Creates a note player (any type of `atomik_devices::PLAYER_TYPE_IDS`) on top of a stack:
    /// with an instrument, or a player of its chain, selected, the new player goes **above the
    /// head of the chain** (the device its track's notes enter: the top player, else the
    /// instrument) in the same rack column, is cabled `new.notes_out -> head.notes_in` and takes
    /// over the cable that fed the head (`old_source -> new.notes_in`), and every track whose
    /// entry was the head gets `notes_to = new` (one undo step). The chain is found through the
    /// note cables: from the selection down to the instrument, and without a track up through
    /// the players that feed it. A selection with no Notes input (or `options.route == false`,
    /// or anything that cannot be cabled) only adds the player after the selection, as
    /// `AddDevice` does. A device that is not a note player is rejected.
    CreatePlayer {
        /// The player's type.
        type_id: String,
        /// The selection the stack is found from.
        target: CreateTarget,
        /// `route` off adds the player only.
        options: CreateOptions,
    },
    /// Applies the creation rules to existing devices that lack them, in
    /// one step: an instrument gets its console channel and its armed song track, a loose
    /// stereo effect goes to the next free insert of the nearest channel before it (or is cabled
    /// after the nearest device with free outputs). Devices that are routed already are skipped;
    /// the command is rejected, with the reason, when nothing changes. `options.route` is
    /// ignored.
    AutoRoute {
        /// The devices (top level).
        devices: Vec<DeviceKey>,
        /// `skip_track` and `disarm` apply.
        options: CreateOptions,
    },
    /// "Use as Send Effect": puts an existing, unrouted stereo effect on the
    /// lowest free send/return slot (`SetReturn`), in one undo step. Rejected, with the reason and
    /// no change, for an effect that is on a return, in an insert chain or cabled already, for a
    /// console device, an instrument or a device that is not a stereo effect, and when all eight
    /// slots are in use.
    AssignSend {
        /// The effect (top level).
        effect: DeviceKey,
    },
    /// Removes every cable of every device, in one step. Devices in one scope;
    /// devices without cables are fine when some other has any.
    DisconnectDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
    /// Removes the devices like [`WorkflowCommand::RemoveDevices`] and keeps
    /// the audio path of each removed top-level device where it is unambiguous: its single
    /// source is cabled to its destinations (an effect in a chain, a middle insert). Console
    /// devices, instruments, paths that fan in from several sources, outputs that match no
    /// input, the insert loop of a channel and devices inside containers are removed plainly.
    DeleteDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
    /// Makes one device take the sound of a device patch (`snapshot` holds exactly that one
    /// device, as a `.atomikpatch` of one device does; see [`crate::DeviceSnapshot::single_device`]):
    /// every param (the ones the patch does not store go back to their default), zones, step
    /// patterns, curves, drum patterns, a plugin's state, a container's inside and the patch name.
    /// The device keeps its key, name, column, colour, cables, track and console channel. The
    /// patch's device type must be the device's; every asset the patch needs must be in the
    /// document's pool; params driven by a macro are left alone; a patch that changes nothing is
    /// rejected. One undo step. Reports `ParamChanged` per changed param, `ZonesChanged`,
    /// `PatternsChanged`, `PluginStateChanged`, `ViewChanged` (the name) and, for a container,
    /// the removal and re-adding of the whole inside.
    ApplyPatch {
        /// The device that takes the patch (path key).
        device: DeviceKey,
        /// The patch.
        snapshot: DeviceSnapshot,
    },
    /// Puts one device back to the sound of a new device of its type: default params, no zones,
    /// default patterns, no plugin state, an empty container, no patch name. A device that is
    /// already there is rejected. One undo step; reports like [`WorkflowCommand::ApplyPatch`].
    ResetDevice {
        /// The device (path key).
        device: DeviceKey,
    },
    /// Sets the patch name the device shows (empty clears it). Reports `ViewChanged`.
    SetPatchName {
        /// The device (path key).
        device: DeviceKey,
        /// The name: trimmed, at most [`crate::MAX_PATCH_NAME_CHARS`] characters, no control
        /// characters; empty for none.
        name: String,
    },
    /// Sets the output port of a MIDI Out device (`None` goes back to the fallback port of the app
    /// settings). Reports `ViewChanged`; the app re-routes the device. Refused on any other device
    /// type.
    SetMidiOutPort {
        /// The device (path key).
        device: DeviceKey,
        /// The port name: trimmed, at most [`crate::MAX_MIDI_OUT_PORT_CHARS`] characters, no control
        /// characters.
        port: Option<String>,
    },
    /// Adds the tracks of a MIDI file, and optionally its tempo map, meter map and markers, as
    /// one undo step (see [`crate::MidiImport`]): per track a new instrument with its console
    /// channel, an unarmed song track and a note clip. Reports the compound sync ops of the
    /// pieces (`DeviceAdded`, `RoutingChanged`, `TempoChanged`, `ConsoleChanged`, `SongChanged`,
    /// `ViewChanged` for markers).
    ImportMidi(crate::MidiImport),
}

/// Where [`WorkflowCommand::MoveClips`] puts one clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipMove {
    /// The clip.
    pub id: ClipId,
    /// The track that takes it.
    pub track: DeviceKey,
    /// The target lane (an index into the track's lanes; it must have the clip's lane kind),
    /// or `None` to let the rule of [`WorkflowCommand::MoveClips`] choose.
    pub lane: Option<usize>,
    /// Its new start tick.
    pub start: i64,
}

fn bad_song(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

fn bad_scope(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// Plans a workflow command: the edit and the last clip id it allocated or reports, if any.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: WorkflowCommand,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let plain = |edit: Result<Edit, ModelError>| edit.map(|e| (e, None));
    match command {
        WorkflowCommand::RemoveDevices { keys } => {
            plain(remove_devices(doc, registry, &keys, false))
        }
        WorkflowCommand::DeleteDevices { keys } => {
            plain(remove_devices(doc, registry, &keys, true))
        }
        WorkflowCommand::CreateInstrument {
            index,
            source,
            key,
            options,
        } => plain(workflow_create::create_instrument(
            doc, registry, index, &source, key, &options,
        )),
        WorkflowCommand::CreateEffect {
            source,
            target,
            key,
            options,
        } => plain(workflow_create::create_effect(
            doc, registry, &source, &target, key, &options,
        )),
        WorkflowCommand::CreateInstrumentAt {
            slot,
            source,
            key,
            options,
        } => {
            let at = doc.resolve_slot(slot)?;
            let edit = workflow_create::create_instrument(
                doc, registry, at.index, &source, key, &options,
            )?;
            Ok((rack_columns::place_new_device(doc, at, edit), None))
        }
        WorkflowCommand::CreatePlayer {
            type_id,
            target,
            options,
        } => plain(workflow_create::create_player(
            doc, registry, &type_id, &target, &options,
        )),
        WorkflowCommand::CreateEffectAt {
            slot,
            source,
            key,
            options,
        } => plain(workflow_create::create_effect_at(
            doc, registry, slot, &source, key, &options,
        )),
        WorkflowCommand::MoveDevicesToSlot { keys, slot, route } => plain(move_routed(
            doc,
            registry,
            &keys,
            Placement::Slot(slot),
            route,
        )),
        WorkflowCommand::PasteDevicesAt {
            snapshot,
            container,
            slot,
            route,
        } => {
            let mut p = Planner::new(doc, registry);
            let (at, column) = match &container {
                None => {
                    let r = doc.resolve_slot(slot)?;
                    (r.index, Some(r.column))
                }
                Some(_) => (slot.index, None),
            };
            let new = paste_devices(&mut p, &snapshot, container.as_ref(), at, column)?;
            if route && container.is_none() {
                route_pasted(&mut p, &new);
            }
            Ok((p.finish(), None))
        }
        WorkflowCommand::ReplaceDevice { target, source } => {
            plain(crate::replace_device::plan(doc, registry, &target, &source))
        }
        WorkflowCommand::SetFoldedMany { keys, folded } => {
            let mut edits = Vec::new();
            for key in keys {
                if rules::resolve(doc, &key)?.folded != folded {
                    edits.push(Edit::Folded {
                        key,
                        old: !folded,
                        new: folded,
                    });
                }
            }
            if edits.is_empty() {
                return Err(bad_scope("the devices are already that way"));
            }
            plain(Ok(Edit::Compound(edits)))
        }
        WorkflowCommand::AutoRoute { devices, options } => plain(workflow_create::auto_route(
            doc, registry, &devices, &options,
        )),
        WorkflowCommand::AssignSend { effect } => {
            plain(workflow_create::assign_send(doc, registry, &effect))
        }
        WorkflowCommand::DisconnectDevices { keys } => plain(
            workflow_create::disconnect_devices(doc, &keys)
                .map(|e| workflow_create::with_entries(doc, registry, e)),
        ),
        WorkflowCommand::DuplicateDevices { keys, route } => {
            plain(duplicate_devices(doc, registry, &keys, route))
        }
        WorkflowCommand::PasteDevices {
            snapshot,
            container,
            at,
            route,
        } => {
            let mut p = Planner::new(doc, registry);
            let new = paste_devices(&mut p, &snapshot, container.as_ref(), at, None)?;
            if route && container.is_none() {
                route_pasted(&mut p, &new);
            }
            Ok((p.finish(), None))
        }
        WorkflowCommand::MoveDevices { keys, to, route } => plain(move_routed(
            doc,
            registry,
            &keys,
            Placement::Flat(to),
            route,
        )),
        WorkflowCommand::ApplyPatch { device, snapshot } => plain(crate::device_patch::plan_apply(
            doc, registry, &device, &snapshot,
        )),
        WorkflowCommand::ResetDevice { device } => {
            plain(crate::device_patch::plan_reset(doc, registry, &device))
        }
        WorkflowCommand::SetPatchName { device, name } => {
            plain(crate::device_patch::plan_set_name(doc, &device, &name))
        }
        WorkflowCommand::SetMidiOutPort { device, port } => plain(crate::midi_out::plan_set_port(
            doc,
            &device,
            port.as_deref(),
        )),
        WorkflowCommand::ImportMidi(import) => {
            plain(crate::midi_import::plan(doc, registry, import))
        }
        WorkflowCommand::DuplicateTrack { device } => {
            plain(duplicate_track(doc, registry, &device))
        }
        WorkflowCommand::DuplicateTracks { devices, place } => {
            plain(duplicate_tracks(doc, registry, &devices, place))
        }
        WorkflowCommand::DisconnectDevice { key } => plain(
            disconnect_device(doc, &key).map(|e| workflow_create::with_entries(doc, registry, e)),
        ),
        WorkflowCommand::PasteClips {
            track,
            lane,
            at_tick,
            clips,
        } => paste_clips(doc, registry, (track.as_ref(), lane), at_tick, &clips),
        WorkflowCommand::DuplicateClips { ids } => {
            let s = ClipSnapshot::capture(doc, &ids)?;
            paste_clips(
                doc,
                registry,
                (None, None),
                s.origin.saturating_add(s.span),
                &s,
            )
        }
        WorkflowCommand::MoveClips { moves, copy, .. } => move_clips(doc, registry, &moves, copy),
        WorkflowCommand::JoinClips { ids } => join_clips(doc, &ids),
        WorkflowCommand::TrimClips {
            ids, start_delta, ..
        } => plain(trim_clips(doc, registry, &ids, start_delta)),
        WorkflowCommand::DeleteOutsideEvents { ids } => plain(delete_outside_events(doc, &ids)),
        WorkflowCommand::SplitClips { ids, at } => split_clips(doc, &ids, at),
        WorkflowCommand::CutOut {
            ids,
            start,
            end,
            copy,
        } => cut_out(doc, &ids, (start, end), copy),
        WorkflowCommand::ReverseClips { ids, graphical } => {
            crate::clip_transform::reverse_clips(doc, &ids, graphical)
        }
        WorkflowCommand::ScaleTempo { ids, percent } => {
            crate::clip_transform::scale_clips(doc, &ids, percent)
        }
        WorkflowCommand::ChopClips { ids, length } => {
            crate::clip_transform::chop_clips(doc, &ids, length)
        }
        WorkflowCommand::InsertTime { at, length } => {
            crate::time_edit::insert_time(doc, at, length)
        }
        WorkflowCommand::RemoveTime { at, length } => {
            crate::time_edit::remove_time(doc, at, length)
        }
        WorkflowCommand::LayerRecording { clip, take } => {
            crate::record_over::plan_layer(doc, registry, clip, &take)
        }
        WorkflowCommand::EngulfRecording { device, lane, clip } => {
            crate::record_over::plan_engulf(doc, registry, &device, lane, clip)
        }
        WorkflowCommand::RemoveClips { ids } => plain(remove_clips(doc, &ids)),
        WorkflowCommand::SetClipsMuted { ids, muted } => plain(set_clips_muted(doc, &ids, muted)),
        WorkflowCommand::PasteNotes {
            clip,
            at_tick,
            notes,
        } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
            plain(notes_edit(doc, registry, clip, base, &notes))
        }
        WorkflowCommand::DuplicateNotes { clip, indices } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let s = NoteSnapshot::capture(c, &indices)?;
            let ClipContent::Notes(all) = &c.content else {
                unreachable!("capture checked the kind")
            };
            let first = all[indices.iter().copied().min().unwrap_or(0)].tick;
            plain(notes_edit(doc, registry, clip, first + s.span, &s))
        }
        WorkflowCommand::PasteAutomationPoints {
            clip,
            at_tick,
            points,
        } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
            plain(points_edit(doc, registry, clip, base, &points))
        }
        WorkflowCommand::DuplicatePoints { clip, indices } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let s = PointSnapshot::capture(c, &indices)?;
            let ClipContent::Automation(all) = &c.content else {
                unreachable!("capture checked the kind")
            };
            let first = all[indices.iter().copied().min().unwrap_or(0)].tick;
            plain(points_edit(doc, registry, clip, first + s.span, &s))
        }
    }
}

// ------------------------------------------------------------------------------ devices

/// The ancestors of a path key: `a/b/c` has `a` and `a/b`.
fn ancestors(key: &DeviceKey) -> impl Iterator<Item = DeviceKey> + '_ {
    key.0
        .match_indices('/')
        .map(|(i, _)| DeviceKey(key.0[..i].to_owned()))
}

fn remove_devices(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    bridge: bool,
) -> Result<Edit, ModelError> {
    if keys.is_empty() {
        return Err(bad_scope("nothing is selected"));
    }
    let set: BTreeSet<&DeviceKey> = keys.iter().collect();
    for k in &set {
        rules::resolve(doc, k)?;
    }
    // A device inside a removed container goes with it.
    let tops: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| set.contains(&d.key))
        .map(|d| d.key.clone())
        .collect();
    let inners: Vec<DeviceKey> = set
        .iter()
        .filter(|k| k.0.contains('/') && !ancestors(k).any(|a| set.contains(&a)))
        .map(|k| (*k).clone())
        .collect();
    let mut p = Planner::new(doc, registry);
    for k in tops {
        // An instrument's channel can be in the set too and is gone by the time its turn comes.
        if rules::index_of(p.doc(), &k).is_ok() {
            let joins = if bridge {
                workflow_create::bridge_cables(p.doc(), registry, &k)
            } else {
                Vec::new()
            };
            console_cmd::remove_device(&mut p, &k)?;
            workflow_create::connect_bridge(&mut p, &joins);
        }
    }
    for k in inners {
        if p.doc().resolve(&k).is_some() {
            let edit = container_cmd::remove_inner(p.doc(), &k)?;
            p.push(edit);
        }
    }
    Ok(p.finish())
}

/// The rack index right after `source` (a local key of `parent`'s scope) and, at the top level,
/// after the console channels it feeds.
fn after_with_channels(doc: &RackDoc, parent: Option<&DeviceKey>, source: &DeviceKey) -> usize {
    let scope = doc.scope_devices(parent).unwrap_or(&[]);
    let mut at = scope
        .iter()
        .position(|d| &d.key == source)
        .map_or(scope.len(), |i| i + 1);
    if parent.is_none() {
        while at < scope.len()
            && doc
                .console
                .channels
                .iter()
                .any(|c| c.channel == scope[at].key && c.source.as_ref() == Some(source))
        {
            at += 1;
        }
    }
    at
}

fn duplicate_devices(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    route: bool,
) -> Result<Edit, ModelError> {
    let snapshot = DeviceSnapshot::capture(doc, keys, false)?;
    let parent = keys.first().and_then(|k| split_path(k).0);
    let last = &snapshot.devices[snapshot.devices.len() - 1].key;
    let at = after_with_channels(doc, parent.as_ref(), last);
    let mut p = Planner::new(doc, registry);
    let new = paste_devices(&mut p, &snapshot, parent.as_ref(), at, None)?;
    if route && parent.is_none() {
        route_pasted(&mut p, &new);
    }
    Ok(p.finish())
}

/// The plain move of `keys`, or with `route` the move with the effects and note players in it
/// re-routed at their new place (see [`WorkflowCommand::MoveDevices`]). The routed plan is
/// tried on a scratch copy; one the document refuses leaves the plain move.
fn move_routed(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    place: Placement,
    route: bool,
) -> Result<Edit, ModelError> {
    let plain = move_devices(doc, keys, place)?;
    if !route {
        return Ok(plain);
    }
    Ok(rerouted_move(doc, registry, keys, &plain).unwrap_or(plain))
}

fn rerouted_move(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    plain: &Edit,
) -> Option<Edit> {
    if keys.iter().any(|k| k.0.contains('/')) {
        return None;
    }
    let movers: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| keys.contains(&d.key) && workflow_create::reroutable(doc, registry, &d.key))
        .map(|d| d.key.clone())
        .collect();
    if movers.is_empty() {
        return None;
    }
    let mut p = Planner::new(doc, registry);
    for k in &movers {
        workflow_create::unroute(&mut p, k);
    }
    p.push(plain.clone());
    for k in &movers {
        route_in_place(&mut p, k);
    }
    let edit = p.finish();
    let mut check = doc.clone();
    edit.perform(&mut check).ok()?;
    Some(edit)
}

/// Routes the effect or note player `key`, at its place on `p`, by the creation rules; nothing
/// happens when no rule applies or the document refuses the route.
fn route_in_place(p: &mut Planner<'_>, key: &DeviceKey) {
    let (doc, registry) = (p.doc(), p.registry);
    let player = doc
        .device(key)
        .and_then(|d| registry.descriptor(&d.type_id))
        .is_some_and(workflow_create::is_note_player);
    let edit = if player {
        workflow_create::route_player_in_place(doc, registry, key)
    } else {
        workflow_create::route_effect_in_place(doc, registry, key)
    };
    if let Some(edit) = edit {
        let mut check = doc.clone();
        if edit.perform(&mut check).is_ok() {
            p.push(edit);
        }
    }
}

/// Routes the pasted copies `keys` that came without a cable (an effect or a note player of the
/// paste that is not cabled to another copy), as creating them at their places does.
fn route_pasted(p: &mut Planner<'_>, keys: &[DeviceKey]) {
    for k in keys {
        let doc = p.doc();
        let loose = !doc
            .cables
            .iter()
            .any(|c| &c.from.device == k || &c.to.device == k);
        if loose && workflow_create::reroutable(doc, p.registry, k) {
            route_in_place(p, k);
        }
    }
}

fn duplicate_track(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Result<Edit, ModelError> {
    let mut p = Planner::new(doc, registry);
    duplicate_onto(&mut p, device)?;
    Ok(p.finish())
}

/// Plans a copy of the track of `device` on `p`; returns the copy's device.
fn duplicate_onto(p: &mut Planner<'_>, device: &DeviceKey) -> Result<DeviceKey, ModelError> {
    rules::device(p.doc(), device)?;
    if p.doc().song.track(device).is_none() {
        return Err(bad_song(format!("{device} has no track")));
    }
    let snapshot = DeviceSnapshot::capture(p.doc(), std::slice::from_ref(device), true)?;
    let at = after_with_channels(p.doc(), None, device);
    let new = paste_devices(p, &snapshot, None, at, None)?;
    new.into_iter()
        .next()
        .ok_or_else(|| bad_song(format!("{device} could not be copied")))
}

/// [`WorkflowCommand::DuplicateTracks`].
fn duplicate_tracks(
    doc: &RackDoc,
    registry: &Registry,
    devices: &[DeviceKey],
    place: Option<crate::TrackPlace>,
) -> Result<Edit, ModelError> {
    if devices.is_empty() {
        return Err(bad_song("there are no tracks to duplicate"));
    }
    for (i, d) in devices.iter().enumerate() {
        if devices[..i].contains(d) {
            return Err(bad_song(format!("{d} is listed twice")));
        }
    }
    // In track order, so the copies come out in the order of their originals.
    let sources: Vec<&DeviceKey> = doc
        .song
        .tracks
        .iter()
        .map(|t| &t.device)
        .filter(|d| devices.contains(d))
        .collect();
    if sources.len() != devices.len() {
        return Err(bad_song("a device has no track"));
    }
    let sources_last = sources.last().map(|k| (*k).clone());
    let mut p = Planner::new(doc, registry);
    let mut copies = Vec::with_capacity(sources.len());
    for source in sources {
        copies.push(duplicate_onto(&mut p, source)?);
    }
    // The copies were appended to the track list: they go where asked, else right after the
    // last original, in its folder.
    let place = place.unwrap_or_else(|| {
        let at = sources_last.as_ref().map_or(0, |k| {
            doc.song
                .tracks
                .iter()
                .position(|t| &t.device == k)
                .map_or(0, |i| i + 1)
        });
        crate::TrackPlace {
            to_index: at,
            into: sources_last
                .as_ref()
                .and_then(|k| p.doc().song.track_group(k)),
        }
    });
    let moved = crate::organise_cmd::moved_layout(&p.doc().song, &copies, place)?;
    if moved.changed {
        p.push(crate::organise_cmd::layout_edit(
            &p.doc().song,
            moved.order,
            &moved.org,
        ));
    }
    Ok(p.finish())
}

/// `base` without a trailing ` <digits>`: `Poly Synth 2` -> `Poly Synth`.
fn name_base(name: &str) -> &str {
    match name.rsplit_once(' ') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => name,
    }
}

/// A name for a pasted device: its own when free in the scope, else the next free numbered one.
fn pasted_name(scope: &[RackDevice], name: &str) -> String {
    if scope.iter().all(|d| d.name != name) {
        name.to_owned()
    } else {
        rules::unique_name_in(scope, name_base(name))
    }
}

/// The zone ranges of `device` and of everything inside it, against the pool of `doc`.
fn check_device_zones(doc: &RackDoc, device: &RackDevice) -> Result<(), ModelError> {
    zones::check_zones(&doc.assets, &device.zones)
        .map_err(|(i, m)| ModelError::BadZones(format!("{} zone {i}: {m}", device.name)))?;
    if let Some(inner) = &device.inner {
        for d in &inner.devices {
            check_device_zones(doc, d)?;
        }
    }
    Ok(())
}

/// Adds the channel of an instrument as `AddDevice` does; an instrument that cannot be wired
/// (no audio output) stays a plain device.
fn try_channel(p: &mut Planner<'_>, source: &DeviceKey) {
    let mut sub = Planner::new(p.doc(), p.registry);
    if add_channel(&mut sub, source).is_ok() {
        p.push(sub.finish());
    }
}

/// Plans the paste of `snapshot` (see [`WorkflowCommand::PasteDevices`]) on `p`; returns the new
/// keys in snapshot order. At the top level the copies all go to `column` (a column equal to
/// the rack's column count is appended first), by default the column of the device before the
/// first copy; inside a container they are in column 0.
fn paste_devices(
    p: &mut Planner<'_>,
    snapshot: &DeviceSnapshot,
    container: Option<&DeviceKey>,
    at: usize,
    column: Option<u16>,
) -> Result<Vec<DeviceKey>, ModelError> {
    if snapshot.devices.is_empty() {
        return Err(bad_scope("the clipboard holds no devices"));
    }
    if let Some(c) = container {
        rules::resolve(p.doc(), c)?;
        if p.doc().inner(c).is_none() {
            return Err(bad_scope(format!("{c} is not a container")));
        }
        if !snapshot.tracks.is_empty() {
            return Err(bad_scope("a track cannot live inside a container"));
        }
    }
    if let Some(h) = snapshot
        .required_assets()
        .into_iter()
        .find(|h| !p.doc().assets.contains_key(h))
    {
        return Err(ModelError::UnknownKey(format!(
            "asset {h} is not in this project's sample pool"
        )));
    }
    let first = match container {
        // Nothing goes above the master: a paste at the top lands below it.
        None => rules::top_index(p.doc(), at),
        Some(_) => at.min(p.doc().scope_devices(container).map_or(0, <[_]>::len)),
    };
    let column = match container {
        None => column.unwrap_or_else(|| p.doc().column_at(first)),
        Some(_) => 0,
    };
    if container.is_none() && column >= p.doc().columns {
        let old = p.doc().columns;
        p.push(Edit::Columns {
            old,
            new: column + 1,
        });
    }
    let mut map: BTreeMap<DeviceKey, DeviceKey> = BTreeMap::new();
    let mut new_keys = Vec::with_capacity(snapshot.devices.len());
    for (i, d) in snapshot.devices.iter().enumerate() {
        rules::descriptor(p.registry, &d.type_id)?;
        if d.type_id == MASTER_TYPE {
            return Err(rules::fixed_master(rules::MASTER_COPY));
        }
        if is_console_type(&d.type_id) {
            return Err(ModelError::BadConsole(format!(
                "{} is a console device; it cannot be pasted",
                d.name
            )));
        }
        if d.type_id == SPLIT_TYPE {
            return Err(ModelError::NotAllowedInContainer(format!(
                "{SPLIT_TYPE} is internal and cannot be pasted"
            )));
        }
        check_device_zones(p.doc(), d)?;
        let scope = p.doc().scope_devices(container).unwrap_or(&[]);
        let new_key = match &d.plugin {
            Some(plugin) => {
                rules::generate_key_with_base_in(scope, &key_base_of_name(&plugin.layout.name))
            }
            None => rules::generate_key_in(scope, &d.type_id),
        };
        let mut copy = d.clone();
        copy.key = new_key.clone();
        copy.name = pasted_name(scope, &d.name);
        copy.column = column;
        let index = (first + i).min(scope.len());
        p.push(match container {
            None => Edit::Insert {
                index,
                device: copy,
                cables: Vec::new(),
            },
            Some(c) => Edit::InnerInsert {
                container: c.clone(),
                index,
                device: copy,
                cables: Vec::new(),
            },
        });
        map.insert(d.key.clone(), new_key.clone());
        new_keys.push(new_key);
    }
    for c in &snapshot.cables {
        let (Some(from), Some(to)) = (map.get(&c.from.device), map.get(&c.to.device)) else {
            continue;
        };
        let cable = Cable {
            from: SocketRef {
                device: from.clone(),
                socket: c.from.socket.clone(),
            },
            to: SocketRef {
                device: to.clone(),
                socket: c.to.socket.clone(),
            },
        };
        match container {
            None => p.connect(cable)?,
            Some(cont) => {
                rules::check_inner_cable(p.doc(), p.registry, cont, &cable)?;
                p.push(Edit::InnerConnect {
                    container: cont.clone(),
                    cable,
                });
            }
        }
    }
    if container.is_none() {
        paste_tracks(p, snapshot, &map)?;
        let feeds: BTreeSet<&DeviceKey> = snapshot.cables.iter().map(|c| &c.from.device).collect();
        for d in &snapshot.devices {
            if is_instrument(p.registry, &d.type_id)
                && console_available(p.registry)
                && !feeds.contains(&d.key)
            {
                try_channel(p, &map[&d.key]);
            }
        }
        for t in &snapshot.tracks {
            let Some(new) = map.get(&t.device) else {
                continue;
            };
            if p.type_of(new)? == AUDIO_TRACK_TYPE {
                wire_inputs(p, new)?;
            }
        }
    }
    p.doc().check_containers(p.registry)?;
    if p.doc().plugin_state_bytes(None) > MAX_PROJECT_PLUGIN_STATE_BYTES {
        return Err(ModelError::BadPlugin(format!(
            "the plugin states of the project would total more than {MAX_PROJECT_PLUGIN_STATE_BYTES} bytes"
        )));
    }
    Ok(new_keys)
}

/// The song tracks of a snapshot for the pasted devices: fresh clip ids, not armed.
fn paste_tracks(
    p: &mut Planner<'_>,
    snapshot: &DeviceSnapshot,
    map: &BTreeMap<DeviceKey, DeviceKey>,
) -> Result<(), ModelError> {
    let start = p.doc().song.next_clip_id;
    let mut next = start;
    for t in &snapshot.tracks {
        let Some(new) = map.get(&t.device) else {
            continue;
        };
        let desc = rules::descriptor(p.registry, p.type_of(new)?)?;
        if !is_audio_track_type(desc) && !has_notes_input(desc) && !t.is_automation_only() {
            return Err(bad_song(format!("{new} has no Notes input")));
        }
        if t.is_automation_only() && has_notes_input(desc) {
            return Err(bad_song(format!("{new} has a Notes input")));
        }
        let mut track = t.clone();
        track.device = new.clone();
        track.armed = false;
        // The entry follows a copied player; one that was not copied is left behind.
        track.notes_to = t.notes_to.as_ref().and_then(|k| map.get(k).cloned());
        let mut next_group = p.doc().song.next_take_group_id();
        for lane in &mut track.lanes {
            let mut renumbered: BTreeMap<ClipId, ClipId> = BTreeMap::new();
            for c in &mut lane.clips {
                renumbered.insert(c.id, ClipId(next));
                c.id = ClipId(next);
                next = next
                    .checked_add(1)
                    .ok_or_else(|| bad_song("clip ids are exhausted"))?;
                if let ClipContent::Audio(a) = &c.content {
                    check_audio(p.doc(), a)?;
                }
            }
            // A take group is copied whole: its own id, the ids of its takes (clips of the song
            // like any other) and the lane clips it compiled all become the copy's.
            for g in &mut lane.take_groups {
                g.id = next_group;
                next_group += 1;
                for take in &mut g.takes {
                    take.id = ClipId(next);
                    next = next
                        .checked_add(1)
                        .ok_or_else(|| bad_song("clip ids are exhausted"))?;
                    if let ClipContent::Audio(a) = &take.content {
                        check_audio(p.doc(), a)?;
                    }
                }
                g.clips = g
                    .clips
                    .iter()
                    .filter_map(|id| renumbered.get(id).copied())
                    .collect();
            }
        }
        let index = p.doc().song.tracks.len();
        p.push(Edit::Track {
            index,
            old: None,
            new: Some(track),
        });
    }
    if next != start {
        p.push(Edit::NextClipId {
            old: start,
            new: next,
        });
    }
    Ok(())
}

/// The interface inputs of a new audio track, as `AddAudioTrack` wires them.
fn wire_inputs(p: &mut Planner<'_>, device: &DeviceKey) -> Result<(), ModelError> {
    let inputs = p.doc().input_channels;
    for (input, socket) in [(1u16, "in_l"), (2, "in_r")] {
        if inputs == 0 {
            break;
        }
        p.connect(Cable {
            from: SocketRef {
                device: DeviceKey(INTERFACE_KEY.to_owned()),
                socket: format!("in_{}", input.min(inputs)),
            },
            to: SocketRef {
                device: device.clone(),
                socket: socket.to_owned(),
            },
        })?;
    }
    Ok(())
}

fn move_devices(doc: &RackDoc, keys: &[DeviceKey], place: Placement) -> Result<Edit, ModelError> {
    let first = keys
        .first()
        .ok_or_else(|| bad_scope("nothing is selected"))?;
    let parent = split_path(first).0;
    let mut wanted = BTreeSet::new();
    for k in keys {
        let (p, local) = split_path(k);
        if p != parent {
            return Err(bad_scope(format!(
                "{first} and {k} are not in the same scope"
            )));
        }
        rules::resolve(doc, k)?;
        wanted.insert(local);
    }
    if parent.is_none() {
        let edit = rack_columns::plan_move(doc, keys, place)?;
        if rack_columns::is_noop(&edit) {
            return Err(ModelError::OutOfRange(
                "the devices are already there".into(),
            ));
        }
        return Ok(edit);
    }
    // Inside a container: one column, only the index counts.
    let to = match place {
        Placement::Flat(to) => to,
        Placement::Slot(slot) => slot.index,
    };
    let order: Vec<DeviceKey> = doc
        .scope_devices(parent.as_ref())
        .ok_or_else(|| ModelError::UnknownDevice(first.0.clone()))?
        .iter()
        .map(|d| d.key.clone())
        .collect();
    let group: Vec<DeviceKey> = order
        .iter()
        .filter(|k| wanted.contains(k))
        .cloned()
        .collect();
    let rest: Vec<DeviceKey> = order
        .iter()
        .filter(|k| !wanted.contains(k))
        .cloned()
        .collect();
    let at = to.min(rest.len());
    let mut target = rest[..at].to_vec();
    target.extend(group.iter().cloned());
    target.extend_from_slice(&rest[at..]);
    if target == order {
        return Err(ModelError::OutOfRange(
            "the devices are already there".into(),
        ));
    }
    let mut current = order;
    let mut edits = Vec::new();
    for (i, k) in target.iter().enumerate() {
        let from = current
            .iter()
            .position(|c| c == k)
            .ok_or_else(|| bad_song("the target order is not a permutation"))?;
        if from != i {
            let k = current.remove(from);
            current.insert(i, k);
            edits.push(Edit::InnerMove {
                container: parent
                    .clone()
                    .ok_or_else(|| bad_song("a container scope is needed"))?,
                from,
                to: i,
            });
        }
    }
    Ok(Edit::Compound(edits))
}

fn disconnect_device(doc: &RackDoc, key: &DeviceKey) -> Result<Edit, ModelError> {
    rules::resolve(doc, key)?;
    let (parent, local) = split_path(key);
    let cables: Vec<Cable> = doc
        .scope_cables(parent.as_ref())
        .unwrap_or(&[])
        .iter()
        .filter(|c| c.from.device == local || c.to.device == local)
        .cloned()
        .collect();
    if cables.is_empty() {
        return Err(ModelError::BadCable(format!("{key} has no cables")));
    }
    Ok(Edit::Compound(
        cables
            .into_iter()
            .map(|cable| match &parent {
                None => Edit::Disconnect(cable),
                Some(c) => Edit::InnerDisconnect {
                    container: c.clone(),
                    cable,
                },
            })
            .collect(),
    ))
}

// ------------------------------------------------------------------------------ clips

fn clip_of(doc: &RackDoc, id: ClipId) -> Result<(DeviceKey, usize, &Clip), ModelError> {
    doc.song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))
}

/// The kind of lane `lane` of the track of `device`.
fn lane_kind<'d>(
    doc: &'d RackDoc,
    device: &DeviceKey,
    lane: usize,
) -> Result<&'d LaneKind, ModelError> {
    doc.song
        .track(device)
        .and_then(|t| t.lanes.get(lane))
        .map(|l| &l.kind)
        .ok_or_else(|| bad_song(format!("{device} has no lane {lane}")))
}

fn lane_name(kind: &LaneKind) -> String {
    match kind {
        LaneKind::Notes => "note".into(),
        LaneKind::Audio => "audio".into(),
        LaneKind::Automation(p) => format!("{p} automation"),
    }
}

fn paste_clips(
    doc: &RackDoc,
    registry: &Registry,
    (track, note_lane): (Option<&DeviceKey>, Option<usize>),
    at_tick: i64,
    snapshot: &ClipSnapshot,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if snapshot.entries.is_empty() {
        return Err(bad_song("the clipboard holds no clips"));
    }
    if let Some(h) = snapshot
        .required_assets()
        .into_iter()
        .find(|h| !doc.assets.contains_key(h))
    {
        return Err(ModelError::UnknownKey(format!(
            "asset {h} is not in this project's sample pool"
        )));
    }
    let mut p = Planner::new(doc, registry);
    let start = doc.song.next_clip_id;
    let mut next = start;
    // Audio clips whose track is not in this project go on a new audio track (one per source
    // track), made inside the same step.
    let mut made: BTreeMap<DeviceKey, DeviceKey> = BTreeMap::new();
    for e in &snapshot.entries {
        let mut device = track.unwrap_or(&e.device).clone();
        if track.is_none() && e.lane.is_audio() && p.doc().song.track(&device).is_none() {
            if let Some(new) = made.get(&e.device) {
                device = new.clone();
            } else {
                let new = rules::generate_key(p.doc(), AUDIO_TRACK_TYPE);
                let edit = crate::song_cmd::add_audio_track(p.doc(), registry, None, None)?;
                p.push(edit);
                made.insert(e.device.clone(), new.clone());
                device = new;
            }
        }
        let device = &device;
        let doc = p.doc();
        let t = doc
            .song
            .track(device)
            .ok_or_else(|| bad_song(format!("{device} has no track")))?;
        let lane = if e.lane.is_notes() {
            match note_lane {
                Some(l) if t.is_note_lane(l) => l,
                Some(l) => return Err(bad_song(format!("{device} has no note lane {l}"))),
                None => e.note_lane.min(t.note_lane_count().saturating_sub(1)),
            }
        } else {
            t.lanes
                .iter()
                .position(|l| l.kind == e.lane)
                .ok_or_else(|| bad_song(format!("{device} has no {} lane", lane_name(&e.lane))))?
        };
        if !t.lanes.get(lane).is_some_and(|l| l.kind == e.lane) {
            return Err(bad_song(format!(
                "{device} has no {} lane",
                lane_name(&e.lane)
            )));
        }
        let desc = rules::descriptor(registry, &rules::device(doc, device)?.type_id)?;
        let mut clip = e.clip.clone();
        clip.id = ClipId(next);
        clip.start = at_tick.saturating_add(e.clip.start);
        next = next
            .checked_add(1)
            .ok_or_else(|| bad_song("clip ids are exhausted"))?;
        check_geometry(clip.start, clip.length, clip.offset, clip.loop_length)
            .map_err(ModelError::OutOfRange)?;
        clip.content = check_content(doc, clip.content, &t.lanes[lane].kind, desc)?;
        p.push(Edit::Clip {
            device: device.clone(),
            lane,
            old: None,
            new: Some(clip),
        });
    }
    p.push(Edit::NextClipId {
        old: start,
        new: next,
    });
    Ok((p.finish(), Some(ClipId(next - 1))))
}

/// Moves or copies clips to other tracks and ticks in one step (see
/// [`WorkflowCommand::MoveClips`]).
fn move_clips(
    doc: &RackDoc,
    registry: &Registry,
    moves: &[ClipMove],
    copy: bool,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if moves.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut seen = BTreeSet::new();
    let mut p = Planner::new(doc, registry);
    let first = doc.song.next_clip_id;
    let mut next = first;
    for m in moves {
        if !seen.insert(m.id) {
            return Err(bad_song(format!("clip {} is listed twice", m.id.0)));
        }
        let (device, lane, c) = clip_of(doc, m.id)?;
        let t = doc
            .song
            .track(&m.track)
            .ok_or_else(|| bad_song(format!("{} has no track", m.track)))?;
        let kind = lane_kind(doc, &device, lane)?;
        let to = match m.lane {
            Some(l) => {
                let target = t
                    .lanes
                    .get(l)
                    .ok_or_else(|| bad_song(format!("{} has no lane {l}", m.track)))?;
                if &target.kind != kind {
                    return Err(bad_song(format!(
                        "lane {l} of {} is not a {} lane",
                        m.track,
                        lane_name(kind)
                    )));
                }
                l
            }
            None if device == m.track => lane,
            None if kind.is_notes() && t.note_lane_count() > 0 => lane.min(t.note_lane_count() - 1),
            None => t
                .lanes
                .iter()
                .position(|l| &l.kind == kind)
                .ok_or_else(|| bad_song(format!("{} has no {} lane", m.track, lane_name(kind))))?,
        };
        check_geometry(m.start, c.length, c.offset, c.loop_length)
            .map_err(ModelError::OutOfRange)?;
        let placed = Clip {
            start: m.start,
            ..c.clone()
        };
        if copy {
            let id = ClipId(next);
            next = next
                .checked_add(1)
                .ok_or_else(|| bad_song("clip ids are exhausted"))?;
            p.push(Edit::Clip {
                device: m.track.clone(),
                lane: to,
                old: None,
                new: Some(Clip { id, ..placed }),
            });
        } else if device == m.track && lane == to {
            p.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(placed),
            });
        } else {
            p.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: None,
            });
            p.push(Edit::Clip {
                device: m.track.clone(),
                lane: to,
                old: None,
                new: Some(placed),
            });
        }
    }
    if !copy {
        return Ok((p.finish(), None));
    }
    p.push(Edit::NextClipId {
        old: first,
        new: next,
    });
    Ok((p.finish(), Some(ClipId(next - 1))))
}

/// Removes the clips in one step (`SongCommand::RemoveClip` cannot be batched, so a selection
/// of several clips needs this one). An unknown clip rejects the command.
fn remove_clips(doc: &RackDoc, ids: &[ClipId]) -> Result<Edit, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut edits = Vec::new();
    for id in &wanted {
        let (device, lane, c) = clip_of(doc, *id)?;
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: None,
        });
    }
    Ok(Edit::Compound(edits))
}

fn set_clips_muted(doc: &RackDoc, ids: &[ClipId], muted: bool) -> Result<Edit, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut edits = Vec::new();
    for id in &wanted {
        let (device, lane, c) = clip_of(doc, *id)?;
        if c.muted != muted {
            edits.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip { muted, ..c.clone() }),
            });
        }
    }
    if edits.is_empty() {
        return Err(bad_song(if muted {
            "the clips are already muted"
        } else {
            "the clips are not muted"
        }));
    }
    Ok(Edit::Compound(edits))
}

/// What a clip shows, as pieces `(song start, song end, content start)`: one piece, or with a
/// loop one per repetition (the content range `[0, loop)` repeats from `offset mod loop`).
fn content_pieces(c: &Clip) -> Vec<(i64, i64, i64)> {
    let end = c.end();
    match c.loop_length {
        Some(n) if n > 0 => {
            let mut out = Vec::new();
            let mut t = c.start;
            let mut cs = c.offset.rem_euclid(n);
            while t < end {
                let e = (t + n - cs).min(end);
                out.push((t, e, cs));
                t = e;
                cs = 0;
            }
            out
        }
        _ => vec![(c.start, end, c.offset)],
    }
}

/// The controller lanes of the clips of a join: each clip's window of every lane, later clips
/// hiding the earlier points they overlap, relative to the joined clip's `start`.
fn joined_controllers(clips: &[(DeviceKey, usize, &Clip)], start: i64) -> Vec<ControllerLane> {
    let mut by_kind: BTreeMap<ControllerKind, Vec<AutomationPoint>> = BTreeMap::new();
    for (_, _, c) in clips {
        // A looped clip is unrolled: each repetition is a window of its own.
        for (from, to, cs) in content_pieces(c) {
            let shift = from - start;
            for lane in &c.controllers {
                let pts = window_points(lane, cs, cs + (to - from), shift);
                let all = by_kind.entry(lane.kind).or_default();
                all.retain(|p| p.tick < shift);
                all.extend(pts);
            }
        }
    }
    canonical(
        by_kind
            .into_iter()
            .map(|(kind, points)| ControllerLane::new(kind, points))
            .collect(),
    )
}

fn join_clips(doc: &RackDoc, ids: &[ClipId]) -> Result<(Edit, Option<ClipId>), ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    let mut clips = Vec::new();
    for id in &wanted {
        clips.push(clip_of(doc, *id)?);
    }
    if clips.len() < 2 {
        return Err(bad_song("joining needs at least two clips"));
    }
    clips.sort_by_key(|(_, _, c)| (c.start, c.id));
    let (device, lane, _) = clips[0].clone();
    let kind = lane_kind(doc, &device, lane)?;
    if clips.iter().any(|(d, l, _)| *d != device || *l != lane) {
        return Err(bad_song("clips of different lanes cannot be joined"));
    }
    if kind.is_audio() {
        return Err(bad_song("audio clips cannot be joined"));
    }
    // A muted clip is silent: folding its content into an audible clip would make it heard, so
    // a mixed selection joins only the audible clips and leaves the muted ones where they are.
    // Muted clips alone join into a muted clip.
    if clips.iter().any(|(_, _, c)| !c.muted) {
        clips.retain(|(_, _, c)| !c.muted);
        if clips.len() < 2 {
            return Err(bad_song(
                "joining needs at least two clips that are not muted",
            ));
        }
    }
    let (device, lane, _) = clips[0].clone();
    let start = clips[0].2.start;
    let end = clips.iter().map(|(_, _, c)| c.end()).max().unwrap_or(start);
    let content = match kind {
        LaneKind::Notes => {
            let mut notes: BTreeMap<(i64, u8), Note> = BTreeMap::new();
            for (_, _, c) in &clips {
                let ClipContent::Notes(all) = &c.content else {
                    return Err(bad_song("note lane without notes"));
                };
                for (from, to, cs) in content_pieces(c) {
                    let window = cs..cs + (to - from);
                    for n in all.iter().filter(|n| window.contains(&n.tick)) {
                        let tick = from + (n.tick - cs) - start;
                        notes.insert(
                            (tick, n.key),
                            Note {
                                tick,
                                length: n.length.min(window.end - n.tick),
                                ..*n
                            },
                        );
                    }
                }
            }
            ClipContent::Notes(notes.into_values().collect())
        }
        _ => {
            let mut points: BTreeMap<i64, AutomationPoint> = BTreeMap::new();
            for (_, _, c) in &clips {
                let ClipContent::Automation(all) = &c.content else {
                    return Err(bad_song("automation lane without points"));
                };
                for (from, to, cs) in content_pieces(c) {
                    let window = cs..cs + (to - from);
                    for pt in all.iter().filter(|pt| window.contains(&pt.tick)) {
                        let tick = from + (pt.tick - cs) - start;
                        points.insert(tick, AutomationPoint { tick, ..*pt });
                    }
                }
            }
            ClipContent::Automation(points.into_values().collect())
        }
    };
    check_geometry(start, end - start, 0, None).map_err(ModelError::OutOfRange)?;
    let controllers = joined_controllers(&clips, start);
    let first = clips[0].2;
    let joined = Clip {
        id: first.id,
        start,
        length: end - start,
        offset: 0,
        loop_length: None,
        muted: clips.iter().all(|(_, _, c)| c.muted),
        grid: first.grid,
        scale: first.scale,
        controllers,
        content,
    };
    let mut edits: Vec<Edit> = clips[1..]
        .iter()
        .map(|(device, lane, c)| Edit::Clip {
            device: device.clone(),
            lane: *lane,
            old: Some((*c).clone()),
            new: None,
        })
        .collect();
    edits.push(Edit::Clip {
        device,
        lane,
        old: Some(first.clone()),
        new: Some(joined),
    });
    Ok((Edit::Compound(edits), Some(first.id)))
}

/// Replaces the content of clip `id` with the result of `merge`, checked as `SetClipContent`
/// checks it.
fn content_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    merge: impl FnOnce(&Clip) -> Result<ClipContent, ModelError>,
) -> Result<Edit, ModelError> {
    let (device, lane, c) = clip_of(doc, id)?;
    let desc = rules::descriptor(registry, &rules::device(doc, &device)?.type_id)?;
    let kind = lane_kind(doc, &device, lane)?;
    let content = check_content(doc, merge(c)?, kind, desc)?;
    Ok(Edit::Clip {
        device,
        lane,
        old: Some(c.clone()),
        new: Some(Clip {
            content,
            ..c.clone()
        }),
    })
}

fn notes_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    base: i64,
    snapshot: &NoteSnapshot,
) -> Result<Edit, ModelError> {
    content_edit(doc, registry, id, |c| {
        let ClipContent::Notes(existing) = &c.content else {
            return Err(bad_song(format!("clip {} is not a note clip", id.0)));
        };
        let mut notes: BTreeMap<(i64, u8), Note> =
            existing.iter().map(|n| ((n.tick, n.key), *n)).collect();
        for n in &snapshot.notes {
            let tick = base.saturating_add(n.tick);
            notes.insert((tick, n.key), Note { tick, ..*n });
        }
        Ok(ClipContent::Notes(notes.into_values().collect()))
    })
}

fn points_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    base: i64,
    snapshot: &PointSnapshot,
) -> Result<Edit, ModelError> {
    content_edit(doc, registry, id, |c| {
        let ClipContent::Automation(existing) = &c.content else {
            return Err(bad_song(format!("clip {} is not an automation clip", id.0)));
        };
        let mut points: BTreeMap<i64, AutomationPoint> =
            existing.iter().map(|p| (p.tick, *p)).collect();
        for pt in &snapshot.points {
            let tick = base.saturating_add(pt.tick);
            points.insert(tick, AutomationPoint { tick, ..*pt });
        }
        Ok(ClipContent::Automation(points.into_values().collect()))
    })
}

/// The unique clips of `ids` in song order (by start, then id), each with its device and lane.
fn clips_in_song_order<'d>(
    doc: &'d RackDoc,
    ids: &[ClipId],
) -> Result<Vec<(DeviceKey, usize, &'d Clip)>, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut clips = Vec::new();
    for id in &wanted {
        clips.push(clip_of(doc, *id)?);
    }
    clips.sort_by_key(|(_, _, c)| (c.start, c.id));
    Ok(clips)
}

/// Plans `TrimClips`: see the command docs.
fn trim_clips(
    doc: &RackDoc,
    registry: &Registry,
    ids: &[ClipId],
    delta: i64,
) -> Result<Edit, ModelError> {
    if delta == 0 {
        return Err(bad_song("the start does not move"));
    }
    let mut edits = Vec::new();
    for (device, lane, c) in clips_in_song_order(doc, ids)? {
        if c.content.is_audio() {
            return Err(bad_song(format!(
                "clip {} is an audio clip: trim it with its own handle",
                c.id.0
            )));
        }
        let start = c.start.saturating_add(delta);
        let length = c.length.saturating_sub(delta);
        let (offset, shift) = match c.loop_length {
            Some(n) => ((c.offset + delta).rem_euclid(n), 0),
            None if c.offset + delta < 0 => (0, -(c.offset + delta)),
            None => (c.offset + delta, 0),
        };
        check_geometry(start, length, offset, c.loop_length).map_err(ModelError::OutOfRange)?;
        let mut new = Clip {
            start,
            length,
            offset,
            ..c.clone()
        };
        if shift > 0 {
            let desc = rules::descriptor(registry, &rules::device(doc, &device)?.type_id)?;
            let kind = lane_kind(doc, &device, lane)?;
            let moved = |p: &AutomationPoint| AutomationPoint {
                tick: p.tick.saturating_add(shift),
                ..*p
            };
            new.content = check_content(
                doc,
                match &c.content {
                    ClipContent::Notes(notes) => ClipContent::Notes(
                        notes
                            .iter()
                            .map(|n| Note {
                                tick: n.tick.saturating_add(shift),
                                ..*n
                            })
                            .collect(),
                    ),
                    ClipContent::Automation(points) => {
                        ClipContent::Automation(points.iter().map(moved).collect())
                    }
                    audio @ ClipContent::Audio(_) => audio.clone(),
                },
                kind,
                desc,
            )?;
            new.controllers = c
                .controllers
                .iter()
                .map(|l| ControllerLane::new(l.kind, l.points.iter().map(moved).collect()))
                .collect();
            crate::controller::check_lanes(&new.controllers, crate::song::MAX_TICK)
                .map_err(ModelError::OutOfRange)?;
        }
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: Some(new),
        });
    }
    Ok(Edit::Compound(edits))
}

/// The points of `points` (sorted by tick) that start inside `[lo, hi)`, with the last one
/// before `lo` (unless a point sits at `lo`) and the first one from `hi` on, so the curve inside
/// the window is unchanged.
fn points_around(points: &[AutomationPoint], lo: i64, hi: i64) -> Vec<AutomationPoint> {
    let first = points.partition_point(|p| p.tick < lo);
    let last = points.partition_point(|p| p.tick < hi);
    let from = if points.get(first).is_some_and(|p| p.tick == lo) {
        first
    } else {
        first.saturating_sub(1)
    };
    points[from..(last + 1).min(points.len())].to_vec()
}

/// Plans `DeleteOutsideEvents`: see the command docs.
fn delete_outside_events(doc: &RackDoc, ids: &[ClipId]) -> Result<Edit, ModelError> {
    let mut edits = Vec::new();
    for (device, lane, c) in clips_in_song_order(doc, ids)? {
        let (lo, hi) = match c.loop_length {
            Some(n) => (0, n),
            None => (c.offset, c.offset + c.length),
        };
        let content = match &c.content {
            ClipContent::Notes(notes) => ClipContent::Notes(
                notes
                    .iter()
                    .filter(|n| (lo..hi).contains(&n.tick))
                    .copied()
                    .collect(),
            ),
            ClipContent::Automation(points) => {
                ClipContent::Automation(points_around(points, lo, hi))
            }
            ClipContent::Audio(_) => continue,
        };
        let controllers = canonical(
            c.controllers
                .iter()
                .map(|l| ControllerLane::new(l.kind, points_around(&l.points, lo, hi)))
                .collect(),
        );
        if content == c.content && controllers == c.controllers {
            continue;
        }
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: Some(Clip {
                content,
                controllers,
                ..c.clone()
            }),
        });
    }
    if edits.is_empty() {
        return Err(bad_song("no events lie outside the clips"));
    }
    Ok(Edit::Compound(edits))
}

/// Plans `SplitClips`: see the command docs.
fn split_clips(
    doc: &RackDoc,
    ids: &[ClipId],
    at: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    for (device, lane, c) in clips_in_song_order(doc, ids)? {
        if !(c.start < at && at < c.end()) {
            continue;
        }
        let (left, mut right) = crate::song_cmd::split_pair(doc, c, at)?;
        right.id = ClipId(next);
        next = next
            .checked_add(1)
            .ok_or_else(|| bad_song("clip ids are exhausted"))?;
        edits.push(Edit::Clip {
            device: device.clone(),
            lane,
            old: Some(c.clone()),
            new: Some(left),
        });
        edits.push(Edit::Clip {
            device,
            lane,
            old: None,
            new: Some(right),
        });
    }
    if next == first {
        return Err(bad_song(format!("no clip spans tick {at}")));
    }
    edits.push(Edit::NextClipId {
        old: first,
        new: next,
    });
    Ok((Edit::Compound(edits), Some(ClipId(next - 1))))
}

/// Plans `CutOut`: see the command docs.
fn cut_out(
    doc: &RackDoc,
    ids: &[ClipId],
    (start, end): (i64, i64),
    copy: bool,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if start >= end {
        return Err(bad_song("the range must end after it starts"));
    }
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut alloc = || -> Result<ClipId, ModelError> {
        let id = ClipId(next);
        next = next
            .checked_add(1)
            .ok_or_else(|| bad_song("clip ids are exhausted"))?;
        Ok(id)
    };
    let mut edits = Vec::new();
    for (device, lane, c) in clips_in_song_order(doc, ids)? {
        if c.end() <= start || c.start >= end {
            continue;
        }
        // The part inside the range, cut the way a split cuts.
        let mut inside = c.clone();
        let mut before = None;
        let mut after = None;
        if c.start < start {
            let (l, r) = crate::song_cmd::split_pair(doc, &inside, start)?;
            before = Some(l);
            inside = r;
        }
        if inside.end() > end {
            let (l, r) = crate::song_cmd::split_pair(doc, &inside, end)?;
            after = Some(r);
            inside = l;
        }
        let put = |old: Option<Clip>, new: Option<Clip>| Edit::Clip {
            device: device.clone(),
            lane,
            old,
            new,
        };
        if copy {
            let placed = Clip {
                id: alloc()?,
                start: inside.start + (end - start),
                ..inside
            };
            check_geometry(
                placed.start,
                placed.length,
                placed.offset,
                placed.loop_length,
            )
            .map_err(ModelError::OutOfRange)?;
            edits.push(put(None, Some(placed)));
            continue;
        }
        match (before, after) {
            (Some(l), Some(mut r)) => {
                r.id = alloc()?;
                edits.push(put(Some(c.clone()), Some(l)));
                edits.push(put(None, Some(r)));
            }
            (Some(l), None) => edits.push(put(Some(c.clone()), Some(l))),
            (None, Some(mut r)) => {
                // The clip loses its head and keeps its id.
                r.id = c.id;
                edits.push(put(Some(c.clone()), Some(r)));
            }
            (None, None) => edits.push(put(Some(c.clone()), None)),
        }
    }
    if edits.is_empty() {
        return Err(bad_song("no clip lies inside the range"));
    }
    if next == first {
        return Ok((Edit::Compound(edits), None));
    }
    edits.push(Edit::NextClipId {
        old: first,
        new: next,
    });
    Ok((Edit::Compound(edits), Some(ClipId(next - 1))))
}
