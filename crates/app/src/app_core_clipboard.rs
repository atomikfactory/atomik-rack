//! Clipboard, duplicate and selection wiring in the app core.
//!
//! The views know their selection and emit the commands that need nothing else (delete,
//! duplicate, join). Copy, cut and paste need the document and the in-process clipboard, so
//! they run here: [`AppCore::clipboard_command`] builds a typed snapshot from the focused
//! pane's selection, or turns the clipboard into a workflow command at the paste position the
//! rules of give (clips and notes at the playhead, repeated pastes lay copies end to
//! end, devices after the primary device, tracks after the selected track).
//!
//! After a workflow command was applied, [`AppCore::apply_command`] selects what came out of
//! it (the pasted clips from `next_clip_id` upward, the new devices, the pasted notes and
//! points), because only the app knows which view shows them.

use super::*;
use crate::clipboard::{Clipboard, ClipboardData, ClipboardKind, StripSnapshot};
use atomik_model::controller_edit::{self, ControllerSnapshot};
use atomik_model::{
    AssetMeta, ClipContent, ClipSnapshot, DeviceSnapshot, Note, NoteSnapshot, PointSnapshot,
    RackSlot, SongCommand, WorkflowCommand,
};
use std::collections::BTreeSet;

/// What was true before a workflow command ran, needed to find what it created.
#[derive(Debug, Default)]
pub(super) struct WorkflowPre {
    pub(super) next_clip: u64,
    /// The container whose devices a device command works in, and its device keys before.
    scope: Option<DeviceKey>,
    pub(super) scope_keys: BTreeSet<DeviceKey>,
    /// Notes: the clip, the content tick of the first pasted note and the notes to find again.
    notes: Option<(ClipId, i64, Vec<Note>)>,
    /// Points: the clip, the content tick of the first pasted point and the ticks and values.
    points: Option<(ClipId, i64, Vec<atomik_model::AutomationPoint>)>,
    /// The devices that have a song track before a creation command.
    pub(super) tracks: BTreeSet<DeviceKey>,
    /// What a `CreateEffect` with the master selected could not do: asked on the
    /// document from before the command.
    pub(super) note: Option<atomik_model::RouteNote>,
}

impl<S: EngineSink> AppCore<S> {
    /// The in-process clipboard (it survives New and Open within one run).
    pub fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// The clipboard, for the shell and tests.
    pub fn clipboard_mut(&mut self) -> &mut Clipboard {
        &mut self.clipboard
    }

    /// Copy, cut and paste for the focused pane. `None`: not a clipboard command, or a pane
    /// without one. A refusal is reported (status message and toast) and counts as taken.
    pub(super) fn clipboard_command(&mut self, pane: Pane, id: &str) -> Option<Vec<UiAction>> {
        if pane == Pane::Console {
            return self.strip_clipboard_command(id);
        }
        if !matches!(id, "edit.copy" | "edit.cut" | "edit.paste") {
            return None;
        }
        if !matches!(pane, Pane::Rack | Pane::Arrangement | Pane::PianoRoll) {
            return None;
        }
        let result = match id {
            "edit.copy" => self.copy(pane),
            "edit.cut" => self.copy(pane).and_then(|()| self.delete_selection(pane)),
            _ => self.paste(pane),
        };
        if let Err(message) = result
            && !message.is_empty()
        {
            self.report(message);
        }
        Some(Vec::new())
    }

    /// Copy, cut, paste and duplicate in the console, on its strips (see `app_core_strips.rs`).
    fn strip_clipboard_command(&mut self, id: &str) -> Option<Vec<UiAction>> {
        let result = match id {
            "edit.copy" => self.copy_strips(),
            "edit.cut" => self.copy_strips().and_then(|()| {
                if self.remove_selected_strips(Pane::Console) {
                    Ok(())
                } else {
                    // The reason is already reported.
                    Err(String::new())
                }
            }),
            "edit.duplicate" => self.duplicate_strips(),
            "edit.paste" => self.paste_strips_from_clipboard(),
            _ => return None,
        };
        if let Err(message) = result
            && !message.is_empty()
        {
            self.report(message);
        }
        Some(Vec::new())
    }

    fn paste_strips_from_clipboard(&mut self) -> Result<(), String> {
        match self.clipboard.get().cloned() {
            Some(ClipboardData::Strips(s))
                if !s.devices.devices.is_empty() || !s.loose.is_empty() =>
            {
                self.paste_strips(s)
            }
            Some(ClipboardData::Tracks(devices)) if !devices.devices.is_empty() => self
                .paste_strips(StripSnapshot {
                    devices,
                    settings: Vec::new(),
                    loose: Vec::new(),
                }),
            Some(held) if !held.is_empty() => Err(format!(
                "The clipboard holds {}: copy strips in the console, or tracks, to paste them here",
                held.kind().noun()
            )),
            _ => Err("The clipboard is empty: copy something first".into()),
        }
    }

    /// Whether Copy, Cut and Duplicate have something to work on in `pane` (menus).
    pub fn has_selection(&self, pane: Pane) -> bool {
        match pane {
            Pane::Rack => self.view.selection_len() > 0,
            Pane::Console => !self.console.selected_strips().is_empty(),
            Pane::Arrangement => {
                !self.arrangement.selected_clips().is_empty()
                    || self.arrangement.lane_range().is_some()
                    || self.arrangement.take_focus().is_some()
                    || self.arrangement.selected_points().is_some()
                    || self.arrangement.selected_envelope_points().is_some()
                    || self.arrangement.selected_track().is_some()
            }
            Pane::PianoRoll => {
                !self.piano_roll.selected_notes().is_empty()
                    || self.piano_roll.controller_focus()
                    || self.piano_roll.message_focus()
            }
            _ => false,
        }
    }

    /// Whether the clipboard holds something `pane` can paste (menus).
    pub fn can_paste(&self, pane: Pane) -> bool {
        match (pane, self.clipboard.kind()) {
            (
                Pane::Rack,
                Some(ClipboardKind::Devices | ClipboardKind::Tracks | ClipboardKind::Strips),
            )
            | (
                Pane::Arrangement,
                Some(
                    ClipboardKind::Clips
                    | ClipboardKind::AutomationPoints
                    | ClipboardKind::Tracks
                    | ClipboardKind::Strips
                    | ClipboardKind::Takes,
                ),
            ) => !self.clipboard.is_empty(),
            (Pane::Console, Some(ClipboardKind::Strips | ClipboardKind::Tracks)) => {
                !self.clipboard.is_empty()
            }
            (Pane::PianoRoll, Some(ClipboardKind::Notes)) => {
                !self.clipboard.is_empty() && self.piano_roll.clip().is_some()
            }
            (Pane::PianoRoll, Some(ClipboardKind::Controllers)) => {
                !self.clipboard.is_empty()
                    && (self.piano_roll.clip().is_some() || self.piano_roll.track().is_some())
                    && self.piano_roll.controllers_open()
            }
            _ => false,
        }
    }

    /// Select the track of the primary rack device in the arrangement ("Go to Track").
    /// False when the device has no song track.
    pub fn go_to_track(&mut self) -> bool {
        let Some(device) = self.view.primary() else {
            return false;
        };
        if self.doc().song.track(&device).is_none() {
            self.report("The device has no song track");
            return false;
        }
        self.arrangement.set_selected_track(Some(device));
        true
    }

    // ------------------------------------------------------------------ copy and cut

    fn copy(&mut self, pane: Pane) -> Result<(), String> {
        let data = self.capture(pane)?;
        self.clipboard.set(data);
        Ok(())
    }

    pub(super) fn capture(&self, pane: Pane) -> Result<ClipboardData, String> {
        let doc = self.document.doc();
        let nothing = || "Nothing is selected to copy".to_string();
        let err = |e: atomik_model::ModelError| e.to_string();
        match pane {
            Pane::Rack => {
                let keys = self.view.selection();
                if keys.is_empty() {
                    return Err(nothing());
                }
                DeviceSnapshot::capture(doc, &keys, false)
                    .map(ClipboardData::Devices)
                    .map_err(err)
            }
            Pane::Arrangement => {
                // A time range selected on one or several lanes copies the parts of the clips
                // of those lanes inside it.
                if let Some((lanes, start, end)) = self.arrangement.lane_ranges() {
                    return ClipSnapshot::capture_ranges(doc, &lanes, (start, end))
                        .map(ClipboardData::Clips)
                        .map_err(err);
                }
                if let Some(taken) = self.capture_focused_take() {
                    return taken;
                }
                if let Some((clip, indices)) = self.arrangement.selected_points() {
                    let (_, _, c) = doc.song.clip(clip).ok_or_else(nothing)?;
                    return PointSnapshot::capture(c, &indices)
                        .map(ClipboardData::AutomationPoints)
                        .map_err(err);
                }
                let ids = self.arrangement.selected_clips();
                if !ids.is_empty() {
                    return ClipSnapshot::capture(doc, &ids)
                        .map(ClipboardData::Clips)
                        .map_err(err);
                }
                match self.arrangement.selected_track() {
                    Some(device) => {
                        DeviceSnapshot::capture(doc, std::slice::from_ref(device), true)
                            .map(ClipboardData::Tracks)
                            .map_err(err)
                    }
                    None => Err(nothing()),
                }
            }
            Pane::PianoRoll => {
                // Points selected in the controller area are what Copy takes while the area has
                // the focus (a press in its lane); otherwise the selected notes. The lanes are
                // the clip's, or the track's own in the track mode.
                if self.piano_roll.controller_focus() {
                    let lanes = match self.piano_roll.track() {
                        Some(device) => doc.song.track(device).map(|t| t.controllers.as_slice()),
                        None => self
                            .piano_roll
                            .clip()
                            .and_then(|id| doc.song.clip_or_take(id))
                            .map(|(_, c)| c.controllers.as_slice()),
                    }
                    .ok_or_else(nothing)?;
                    let indices = self.piano_roll.selected_controller_points();
                    return ControllerSnapshot::capture(
                        lanes,
                        self.piano_roll.controller_kind(),
                        &indices,
                    )
                    .map(ClipboardData::Controllers)
                    .ok_or_else(nothing);
                }
                let id = self.piano_roll.clip().ok_or_else(nothing)?;
                let indices = self.piano_roll.selected_notes();
                if indices.is_empty() {
                    return Err(nothing());
                }
                let (_, c) = doc.song.clip_or_take(id).ok_or_else(nothing)?;
                NoteSnapshot::capture(c, &indices)
                    .map(ClipboardData::Notes)
                    .map_err(err)
            }
            _ => Err(nothing()),
        }
    }

    /// Deletes what the pane has selected, through the view's own `edit.delete`.
    fn delete_selection(&mut self, pane: Pane) -> Result<(), String> {
        if pane == Pane::Arrangement
            && let Some(deleted) = self.delete_focused_take()
        {
            return deleted;
        }
        match self.pane_command(pane, "edit.delete", None) {
            Some(_) => Ok(()),
            None => Err("Nothing is selected to cut".into()),
        }
    }

    // ------------------------------------------------------------------ paste

    fn paste(&mut self, pane: Pane) -> Result<(), String> {
        let data = match self.clipboard.get() {
            Some(d) if !d.is_empty() => d.clone(),
            _ => return Err("The clipboard is empty: copy something first".into()),
        };
        let foreign = |held: ClipboardKind, hint: &str| -> Result<(), String> {
            Err(format!("The clipboard holds {}: {hint}", held.noun()))
        };
        match (pane, data) {
            (Pane::Rack, ClipboardData::Devices(s) | ClipboardData::Tracks(s)) => {
                self.paste_devices(s, pane)
            }
            (Pane::Rack | Pane::Arrangement, ClipboardData::Strips(s)) => {
                if s.devices.devices.is_empty() {
                    return foreign(
                        ClipboardKind::Strips,
                        "buses and mixer channels paste in the console",
                    );
                }
                self.paste_devices(s.devices, pane)
            }
            (Pane::Arrangement, ClipboardData::Tracks(s)) => self.paste_devices(s, pane),
            (Pane::Arrangement, ClipboardData::Takes(s)) => self.paste_takes_here(s),
            (Pane::Arrangement, ClipboardData::Clips(s)) => {
                let span = s.span;
                // A range on one lane names the track and lane the clips land on; a range over
                // several lanes puts each part back on a lane of its own kind of its own track.
                let ranged = self.arrangement.lane_ranges().map(|(lanes, ..)| lanes);
                let (track, lane) = match ranged.as_deref() {
                    Some([(device, lane)]) => (Some(device.clone()), Some(*lane)),
                    Some(_) => (None, None),
                    // Note clips land on the selected lane of the selected track.
                    None => (
                        self.arrangement.selected_track().cloned(),
                        self.arrangement.selected_lane().map(|(_, l)| l),
                    ),
                };
                let at_tick = self.position;
                self.apply_workflow(WorkflowCommand::PasteClips {
                    track,
                    lane,
                    at_tick,
                    clips: s,
                })?;
                self.advance_playhead(span);
                Ok(())
            }
            (Pane::Arrangement, ClipboardData::AutomationPoints(s)) => {
                let clip = self
                    .arrangement
                    .selected_points()
                    .map(|(c, _)| c)
                    .or_else(|| self.single_automation_clip())
                    .ok_or_else(|| {
                        "Select an automation clip to paste the points into".to_string()
                    })?;
                let span = s.span;
                let at_tick = self.position;
                self.apply_workflow(WorkflowCommand::PasteAutomationPoints {
                    clip,
                    at_tick,
                    points: s,
                })?;
                self.advance_playhead(span);
                Ok(())
            }
            (Pane::PianoRoll, ClipboardData::Notes(s)) => {
                let clip = self
                    .piano_roll
                    .clip()
                    .ok_or_else(|| "Open a clip to paste the notes into".to_string())?;
                let span = s.span;
                let at_tick = self.paste_tick_in(clip);
                self.apply_workflow(WorkflowCommand::PasteNotes {
                    clip,
                    at_tick,
                    notes: s,
                })?;
                self.advance_playhead(span);
                Ok(())
            }
            (Pane::PianoRoll, ClipboardData::Controllers(s)) => self.paste_controllers(&s),
            (Pane::Rack, held) => foreign(
                held.kind(),
                "click the arrangement or the piano roll to paste them",
            ),
            (Pane::Arrangement, held @ ClipboardData::Devices(_)) => {
                foreign(held.kind(), "click the rack to paste them")
            }
            (Pane::Arrangement, held) => {
                foreign(held.kind(), "open a clip in the piano roll to paste them")
            }
            (Pane::PianoRoll, held) => {
                foreign(held.kind(), "open a clip in the piano roll to paste notes")
            }
            _ => Err("This pane has no clipboard".into()),
        }
    }

    /// Lays copied controller points into the lane the roll shows, from the playhead (the clip's
    /// start when the playhead is outside it): one undo step, and the pasted points are the
    /// selection afterwards.
    fn paste_controllers(&mut self, snapshot: &ControllerSnapshot) -> Result<(), String> {
        if let Some(device) = self.piano_roll.track().cloned() {
            return self.paste_track_controllers(&device, snapshot);
        }
        let clip = self
            .piano_roll
            .clip()
            .ok_or_else(|| "Open a clip to paste the points into".to_string())?;
        if !self.piano_roll.controllers_open() {
            return Err("Show the controller area to paste the points into its lane".into());
        }
        let kind = self.piano_roll.controller_kind();
        let at_song = self.paste_tick_in(clip);
        let (_, c) = self
            .doc()
            .song
            .clip_or_take(clip)
            .ok_or_else(|| "The clip is gone".to_string())?;
        // Content ticks: the model's own mapping for a paste (the clip's offset shifts them), and
        // the content a looped clip plays ends at its loop.
        let at = at_song - c.start + c.offset;
        let limit = c.loop_length.unwrap_or(c.offset + c.length);
        let (lanes, picked) =
            controller_edit::paste_points(&c.controllers, kind, at, snapshot, limit)
                .ok_or_else(|| "Nothing of the copied points falls inside the clip".to_string())?;
        self.apply_command(Command::Song(SongCommand::SetClipControllers {
            clip,
            lanes,
            merge: None,
        }))
        .map_err(|_| String::new())?;
        self.piano_roll.set_controller_selection(&picked);
        self.advance_playhead(snapshot.span);
        Ok(())
    }

    /// [`Self::paste_controllers`] in the roll's track mode: the points go into the lane the roll
    /// shows, from the playhead (song ticks), up to the song end.
    fn paste_track_controllers(
        &mut self,
        device: &DeviceKey,
        snapshot: &ControllerSnapshot,
    ) -> Result<(), String> {
        let song = &self.doc().song;
        let track = song
            .track(device)
            .ok_or_else(|| "The track is gone".to_string())?;
        let (lanes, picked) = controller_edit::paste_points(
            &track.controllers,
            self.piano_roll.controller_kind(),
            self.position,
            snapshot,
            song.end,
        )
        .ok_or_else(|| "Nothing of the copied points falls inside the song".to_string())?;
        self.apply_command(Command::Song(SongCommand::SetTrackControllers {
            device: device.clone(),
            lanes,
            merge: None,
        }))
        .map_err(|_| String::new())?;
        self.piano_roll.set_controller_selection(&picked);
        self.advance_playhead(snapshot.span);
        Ok(())
    }

    /// The song tick a note paste starts at: the playhead when it is inside the clip, else the
    /// clip's start.
    fn paste_tick_in(&self, clip: ClipId) -> i64 {
        match self.doc().song.clip_or_take(clip) {
            Some((_, c)) if self.position >= c.start && self.position < c.end() => self.position,
            Some((_, c)) => c.start,
            None => self.position,
        }
    }

    /// The one selected clip, when it is an automation clip.
    fn single_automation_clip(&self) -> Option<ClipId> {
        let ids = self.arrangement.selected_clips();
        let [id] = ids.as_slice() else { return None };
        let (_, _, c) = self.doc().song.clip(*id)?;
        matches!(c.content, ClipContent::Automation(_)).then_some(*id)
    }

    /// Repeated pastes lay copies end to end: the playhead moves by the span of what was
    /// pasted. While the transport runs the playhead is left alone (it would jump the music).
    fn advance_playhead(&mut self, span: i64) {
        if span > 0 && !self.playing {
            let to = self.position.saturating_add(span);
            self.transport_action(TransportAction::Locate(to));
        }
    }

    pub(super) fn paste_devices(
        &mut self,
        snapshot: DeviceSnapshot,
        pane: Pane,
    ) -> Result<(), String> {
        let (container, at) = match pane {
            Pane::Arrangement => {
                let at = self
                    .arrangement
                    .selected_track()
                    .and_then(|t| {
                        self.doc()
                            .scope_devices(None)
                            .and_then(|d| d.iter().position(|d| &d.key == t))
                    })
                    .map_or(usize::MAX, |i| i + 1);
                (None, at)
            }
            _ => {
                let container = self.view.scope_path();
                if container.is_none() {
                    // At the top level a paste has a place in a column: the slot of the last
                    // right click on the empty rack, else below the selected device in its
                    // column, else the end of the rack.
                    let below = self
                        .view
                        .primary()
                        .and_then(|k| self.doc().slot_of(&k))
                        .map(|s| RackSlot {
                            column: s.column,
                            index: s.index + 1,
                        });
                    if let Some(slot) = self.view.take_context_slot().or(below) {
                        return self.apply_workflow(WorkflowCommand::PasteDevicesAt {
                            snapshot,
                            container: None,
                            slot,
                            route: self.view.auto_route(),
                        });
                    }
                }
                let at = self.rack_add_index(container.as_ref());
                (container, at)
            }
        };
        self.apply_workflow(WorkflowCommand::PasteDevices {
            snapshot,
            container,
            at,
            route: self.view.auto_route(),
        })
    }

    /// Where a device added with no position goes in `container` (`None`: the top level): below
    /// the primary selected device when it is in that scope, else the end.
    pub(super) fn rack_add_index(&self, container: Option<&DeviceKey>) -> usize {
        let scope = self.doc().scope_devices(container);
        let primary = self.view.primary().map(|k| {
            k.0.rsplit('/')
                .next()
                .map(str::to_string)
                .unwrap_or_default()
        });
        match (scope, primary) {
            (Some(devices), Some(p)) => devices
                .iter()
                .position(|d| d.key.0 == p)
                .map_or(usize::MAX, |i| i + 1),
            _ => usize::MAX,
        }
    }

    /// Applies a workflow command, reporting a rejection as the message the refusal needs.
    pub(super) fn apply_workflow(&mut self, command: WorkflowCommand) -> Result<(), String> {
        // A rejection is already reported by `apply_command`: the empty message tells the
        // caller not to report it a second time.
        self.apply_command(Command::Workflow(command))
            .map_err(|_| String::new())
    }

    // ------------------------------------------------------------------ audio from other songs

    /// Names the bundle folders of the other open songs: what was copied there may bring audio
    /// that this song's bundle lacks, and a paste fetches it from them (see
    /// [`Self::fetch_lent_audio`]). The workspace sets it when the clipboard crosses to this song.
    pub fn set_audio_lenders(&mut self, folders: Vec<PathBuf>) {
        self.audio.lenders = folders;
    }

    /// The bundle folders audio can be fetched from.
    pub fn audio_lenders(&self) -> &[PathBuf] {
        &self.audio.lenders
    }

    /// A paste registered the assets `hashes`, which this song's bundle does not hold: a worker
    /// copies the files from the lending songs' bundles into this song's bundle (a scratch one
    /// when the song has none yet) and loads them, so the pasted clips and zones play. Without a
    /// lender, or when the bundle cannot be made, the assets stay missing and are listed so.
    fn fetch_lent_audio(&mut self, hashes: Vec<String>) {
        if hashes.is_empty() || self.audio.lenders.is_empty() {
            return;
        }
        let bundle = match self.import_bundle() {
            Ok(bundle) => bundle,
            Err(e) => {
                self.reject(format!("audio: {e}"));
                return;
            }
        };
        let sources = self.audio.lenders.clone();
        let threshold = self.stream_threshold();
        let started = crate::audio_import::spawn_fetch(
            self.audio.tx.clone(),
            self.audio.generation,
            bundle,
            sources,
            threshold,
            hashes,
        );
        match started {
            Ok(()) => self.audio.pending += 1,
            Err(e) => self.reject(format!("audio: cannot start the loader: {e}")),
        }
    }

    // ------------------------------------------------------------------ before and after

    /// Before a workflow command: registers the pool entries a paste needs (a cross-song paste
    /// of audio clips or sampler zones: `RegisterAsset` is not an undo step) and remembers
    /// what the command will create.
    pub(super) fn workflow_pre(&mut self, command: &WorkflowCommand) -> WorkflowPre {
        let assets: Option<&std::collections::BTreeMap<String, AssetMeta>> = match command {
            WorkflowCommand::PasteDevices { snapshot, .. }
            | WorkflowCommand::PasteDevicesAt { snapshot, .. } => Some(&snapshot.assets),
            WorkflowCommand::ApplyPatch { snapshot, .. } => Some(&snapshot.assets),
            WorkflowCommand::PasteClips { clips, .. } => Some(&clips.assets),
            WorkflowCommand::PasteTakes { takes, .. } => Some(&takes.assets),
            _ => None,
        };
        if let Some(assets) = assets {
            let missing: Vec<(String, AssetMeta)> = assets
                .iter()
                .filter(|(h, _)| !self.doc().assets.contains_key(*h))
                .map(|(h, m)| (h.clone(), m.clone()))
                .collect();
            let hashes: Vec<String> = missing.iter().map(|(h, _)| h.clone()).collect();
            for (hash, meta) in missing {
                let _ = self
                    .document
                    .apply(Command::RegisterAsset { hash, meta }, &self.registry);
            }
            self.fetch_lent_audio(hashes);
        }
        let doc = self.document.doc();
        let mut pre = WorkflowPre {
            next_clip: doc.song.next_clip_id,
            tracks: doc.song.tracks.iter().map(|t| t.device.clone()).collect(),
            note: match command {
                WorkflowCommand::CreateEffect {
                    source,
                    target,
                    options,
                    ..
                } => atomik_model::effect_route_note(doc, &self.registry, source, target, options),
                // Routed as if the device above the slot (none at the top of a column).
                WorkflowCommand::CreateEffectAt {
                    slot,
                    source,
                    options,
                    ..
                } => {
                    let above = doc.resolve_slot(*slot).ok().and_then(|at| {
                        (!at.adds_column && at.index > doc.column_range(at.column).start)
                            .then(|| doc.devices[at.index - 1].key.clone())
                    });
                    above.and_then(|k| {
                        atomik_model::effect_route_note(
                            doc,
                            &self.registry,
                            source,
                            &atomik_model::CreateTarget::Selected(k),
                            options,
                        )
                    })
                }
                _ => None,
            },
            ..WorkflowPre::default()
        };
        let scope_of = |key: &DeviceKey| {
            key.0
                .rsplit_once('/')
                .map(|(p, _)| DeviceKey(p.to_string()))
        };
        match command {
            WorkflowCommand::PasteDevices { container, .. }
            | WorkflowCommand::PasteDevicesAt { container, .. } => pre.scope = container.clone(),
            WorkflowCommand::DuplicateDevices { keys, .. } => {
                pre.scope = keys.first().and_then(scope_of);
            }
            _ => {}
        }
        if matches!(
            command,
            WorkflowCommand::PasteDevices { .. }
                | WorkflowCommand::PasteDevicesAt { .. }
                | WorkflowCommand::DuplicateDevices { .. }
                | WorkflowCommand::DuplicateTrack { .. }
                | WorkflowCommand::DuplicateTracks { .. }
                | WorkflowCommand::CreateInstrument { .. }
                | WorkflowCommand::CreateInstrumentAt { .. }
                | WorkflowCommand::CreateEffect { .. }
                | WorkflowCommand::CreateEffectAt { .. }
                | WorkflowCommand::CreatePlayer { .. }
                | WorkflowCommand::AutoRoute { .. }
        ) {
            pre.scope_keys = doc
                .scope_devices(pre.scope.as_ref())
                .map(|d| d.iter().map(|d| d.key.clone()).collect())
                .unwrap_or_default();
        }
        match command {
            WorkflowCommand::PasteNotes {
                clip,
                at_tick,
                notes,
            } => {
                if let Some((_, c)) = doc.song.clip_or_take(*clip) {
                    let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
                    pre.notes = Some((*clip, base, notes.notes.clone()));
                }
            }
            WorkflowCommand::DuplicateNotes { clip, indices } => {
                if let Some((_, c)) = doc.song.clip_or_take(*clip)
                    && let Ok(s) = NoteSnapshot::capture(c, indices)
                    && let ClipContent::Notes(all) = &c.content
                {
                    let first = indices
                        .iter()
                        .filter_map(|i| all.get(*i))
                        .map(|n| n.tick)
                        .min();
                    if let Some(first) = first {
                        pre.notes = Some((*clip, first + s.span, s.notes));
                    }
                }
            }
            WorkflowCommand::PasteAutomationPoints {
                clip,
                at_tick,
                points,
            } => {
                if let Some((_, _, c)) = doc.song.clip(*clip) {
                    let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
                    pre.points = Some((*clip, base, points.points.clone()));
                }
            }
            WorkflowCommand::DuplicatePoints { clip, indices } => {
                if let Some((_, _, c)) = doc.song.clip(*clip)
                    && let Ok(s) = PointSnapshot::capture(c, indices)
                    && let ClipContent::Automation(all) = &c.content
                {
                    let first = indices
                        .iter()
                        .filter_map(|i| all.get(*i))
                        .map(|p| p.tick)
                        .min();
                    if let Some(first) = first {
                        pre.points = Some((*clip, first + s.span, s.points));
                    }
                }
            }
            _ => {}
        }
        pre
    }

    /// After a workflow command was applied: selects what it created.
    pub(super) fn workflow_post(&mut self, command: &WorkflowCommand, pre: WorkflowPre) {
        match command {
            WorkflowCommand::PasteClips { clips, .. } => {
                self.select_new_clips(pre.next_clip, clips.entries.len());
            }
            WorkflowCommand::DuplicateClips { ids } => {
                let n = ids.iter().collect::<BTreeSet<_>>().len();
                self.select_new_clips(pre.next_clip, n);
            }
            // The takes live in their group, not on the lane: nothing new to select.
            WorkflowCommand::PasteTakes { .. } => self.after_view_change(),
            WorkflowCommand::MoveClips { moves, copy, .. } => {
                // A copy takes the selection; a move keeps it (the ids stay).
                if *copy {
                    self.select_new_clips(pre.next_clip, moves.len());
                } else {
                    self.after_view_change();
                }
            }
            WorkflowCommand::ImportMidi(import) => {
                // The imported clips took the next ids in track order.
                self.select_new_clips(pre.next_clip, import.tracks.len());
            }
            WorkflowCommand::JoinClips { ids } | WorkflowCommand::RenderJoin { ids, .. } => {
                // The earliest clip keeps its id and now spans the others.
                let song = &self.doc().song;
                let keep = ids
                    .iter()
                    .filter_map(|i| song.clip(*i).map(|(_, _, c)| (c.start, c.id)))
                    .min()
                    .map(|(_, id)| id);
                if let Some(id) = keep {
                    self.arrangement.set_selected_clips(&[id]);
                    self.after_view_change();
                }
            }
            WorkflowCommand::ConvertLoop { ids, .. } => {
                // The clips and the repetitions that became clips of their own.
                let now = self.doc().song.next_clip_id;
                let mut all = ids.clone();
                all.extend((pre.next_clip..now).map(ClipId));
                self.after_view_change();
                self.arrangement.set_selected_clips(&all);
            }
            WorkflowCommand::ExtractNotes { .. } | WorkflowCommand::ExplodeNotes { .. } => {
                // The clips on the new lanes took the next ids.
                let now = self.doc().song.next_clip_id;
                let new: Vec<ClipId> = (pre.next_clip..now).map(ClipId).collect();
                self.after_view_change();
                self.arrangement.set_selected_clips(&new);
            }
            WorkflowCommand::DuplicateLoopInstance { id, index } => {
                // The instance keeps the id when it is the first, else it took the last new one.
                let now = self.doc().song.next_clip_id;
                let instance = if *index == 0 {
                    *id
                } else {
                    ClipId(now.saturating_sub(1))
                };
                self.after_view_change();
                self.arrangement.set_selected_clips(&[instance]);
            }
            WorkflowCommand::RenderLoop { id, .. } => {
                self.after_view_change();
                self.arrangement.set_selected_clips(&[*id]);
            }
            WorkflowCommand::PasteDevices { .. }
            | WorkflowCommand::PasteDevicesAt { .. }
            | WorkflowCommand::DuplicateDevices { .. }
            | WorkflowCommand::DuplicateTrack { .. }
            | WorkflowCommand::DuplicateTracks { .. } => self.select_new_devices(&pre),
            WorkflowCommand::PasteNotes { .. } | WorkflowCommand::DuplicateNotes { .. } => {
                if let Some((clip, base, notes)) = &pre.notes {
                    self.select_new_notes(*clip, *base, notes);
                }
            }
            WorkflowCommand::PasteAutomationPoints { .. }
            | WorkflowCommand::DuplicatePoints { .. } => {
                if let Some((clip, base, points)) = &pre.points {
                    self.select_new_points(*clip, *base, points);
                }
            }
            WorkflowCommand::CreateInstrument { .. }
            | WorkflowCommand::CreateInstrumentAt { .. }
            | WorkflowCommand::CreateEffect { .. }
            | WorkflowCommand::CreateEffectAt { .. }
            | WorkflowCommand::CreatePlayer { .. }
            | WorkflowCommand::AutoRoute { .. } => self.created(command, &pre),
            WorkflowCommand::RemoveDevices { .. }
            | WorkflowCommand::DeleteDevices { .. }
            | WorkflowCommand::MoveDevices { .. }
            | WorkflowCommand::MoveDevicesToSlot { .. }
            | WorkflowCommand::RemoveClips { .. }
            | WorkflowCommand::AutomationIntoClip { .. }
            | WorkflowCommand::SetClipsMuted { .. }
            | WorkflowCommand::TrimClips { .. }
            | WorkflowCommand::ExtendClips { .. }
            | WorkflowCommand::DeleteOutsideEvents { .. }
            | WorkflowCommand::SplitClips { .. }
            | WorkflowCommand::CutOut { .. }
            | WorkflowCommand::ReverseClips { .. }
            | WorkflowCommand::ScaleTempo { .. }
            | WorkflowCommand::StretchClips { .. }
            | WorkflowCommand::MoveCrossfade { .. }
            | WorkflowCommand::ResizeCrossfade { .. }
            | WorkflowCommand::ChopClips { .. }
            | WorkflowCommand::CleanupAutomation { .. }
            | WorkflowCommand::InsertTime { .. }
            | WorkflowCommand::RemoveTime { .. }
            | WorkflowCommand::LayerRecording { .. }
            | WorkflowCommand::EngulfRecording { .. }
            | WorkflowCommand::DisconnectDevice { .. }
            | WorkflowCommand::ApplyPatch { .. }
            | WorkflowCommand::ResetDevice { .. }
            | WorkflowCommand::SetPatchName { .. }
            | WorkflowCommand::SetMidiOutPort { .. }
            | WorkflowCommand::SetModSlot { .. }
            | WorkflowCommand::AssignSend { .. }
            | WorkflowCommand::ReplaceDevice { .. }
            | WorkflowCommand::SplitSub { .. }
            | WorkflowCommand::AddReverbSend { .. }
            | WorkflowCommand::AddRumble { .. }
            | WorkflowCommand::SetFoldedMany { .. }
            | WorkflowCommand::DisconnectDevices { .. } => {}
        }
    }

    fn select_new_clips(&mut self, from: u64, n: usize) {
        let ids: Vec<ClipId> = (from..from.saturating_add(n as u64)).map(ClipId).collect();
        self.after_view_change();
        self.arrangement.set_selected_clips(&ids);
    }

    fn select_new_devices(&mut self, pre: &WorkflowPre) {
        let now: Vec<DeviceKey> = self
            .doc()
            .scope_devices(pre.scope.as_ref())
            .map(|d| d.iter().map(|d| d.key.clone()).collect())
            .unwrap_or_default();
        let fresh: Vec<DeviceKey> = now
            .into_iter()
            .filter(|k| !pre.scope_keys.contains(k))
            .map(|k| match &pre.scope {
                Some(c) => DeviceKey(format!("{}/{}", c.0, k.0)),
                None => k,
            })
            .collect();
        self.after_view_change();
        if self.view.scope_path() == pre.scope && !fresh.is_empty() {
            // The pasted instrument comes with its console channel: the view shows the
            // instrument's own key last in rack order.
            self.view.set_selection(&fresh, None);
            self.after_view_change();
        }
    }

    fn select_new_notes(&mut self, clip: ClipId, base: i64, pasted: &[Note]) {
        self.after_view_change();
        if self.piano_roll.clip() != Some(clip) {
            return;
        }
        let Some((_, c)) = self.doc().song.clip_or_take(clip) else {
            return;
        };
        let ClipContent::Notes(all) = &c.content else {
            return;
        };
        let wanted: BTreeSet<(i64, u8)> = pasted.iter().map(|n| (base + n.tick, n.key)).collect();
        let indices: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, n)| wanted.contains(&(n.tick, n.key)))
            .map(|(i, _)| i)
            .collect();
        self.piano_roll.set_selected_notes(&indices);
    }

    fn select_new_points(
        &mut self,
        clip: ClipId,
        base: i64,
        pasted: &[atomik_model::AutomationPoint],
    ) {
        self.after_view_change();
        let Some((_, _, c)) = self.doc().song.clip(clip) else {
            return;
        };
        let ClipContent::Automation(all) = &c.content else {
            return;
        };
        let wanted: BTreeSet<i64> = pasted.iter().map(|p| base + p.tick).collect();
        let indices: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, p)| wanted.contains(&p.tick))
            .map(|(i, _)| i)
            .collect();
        self.arrangement.set_selected_points(clip, &indices);
    }
}
