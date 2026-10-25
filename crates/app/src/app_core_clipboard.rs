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
use crate::clipboard::{Clipboard, ClipboardData, ClipboardKind};
use atomik_model::{
    AssetMeta, ClipContent, ClipSnapshot, DeviceSnapshot, Note, NoteSnapshot, PointSnapshot,
    RackSlot, WorkflowCommand,
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

    /// Whether Copy, Cut and Duplicate have something to work on in `pane` (menus).
    pub fn has_selection(&self, pane: Pane) -> bool {
        match pane {
            Pane::Rack => self.view.selection_len() > 0,
            Pane::Arrangement => {
                !self.arrangement.selected_clips().is_empty()
                    || self.arrangement.selected_points().is_some()
                    || self.arrangement.selected_track().is_some()
            }
            Pane::PianoRoll => !self.piano_roll.selected_notes().is_empty(),
            _ => false,
        }
    }

    /// Whether the clipboard holds something `pane` can paste (menus).
    pub fn can_paste(&self, pane: Pane) -> bool {
        match (pane, self.clipboard.kind()) {
            (Pane::Rack, Some(ClipboardKind::Devices | ClipboardKind::Tracks))
            | (
                Pane::Arrangement,
                Some(
                    ClipboardKind::Clips | ClipboardKind::AutomationPoints | ClipboardKind::Tracks,
                ),
            ) => !self.clipboard.is_empty(),
            (Pane::PianoRoll, Some(ClipboardKind::Notes)) => {
                !self.clipboard.is_empty() && self.piano_roll.clip().is_some()
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

    fn capture(&self, pane: Pane) -> Result<ClipboardData, String> {
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
                let id = self.piano_roll.clip().ok_or_else(nothing)?;
                let indices = self.piano_roll.selected_notes();
                if indices.is_empty() {
                    return Err(nothing());
                }
                let (_, _, c) = doc.song.clip(id).ok_or_else(nothing)?;
                NoteSnapshot::capture(c, &indices)
                    .map(ClipboardData::Notes)
                    .map_err(err)
            }
            _ => Err(nothing()),
        }
    }

    /// Deletes what the pane has selected, through the view's own `edit.delete`.
    fn delete_selection(&mut self, pane: Pane) -> Result<(), String> {
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
            (Pane::Arrangement, ClipboardData::Tracks(s)) => self.paste_devices(s, pane),
            (Pane::Arrangement, ClipboardData::Clips(s)) => {
                let span = s.span;
                let track = self.arrangement.selected_track().cloned();
                let at_tick = self.position;
                self.apply_workflow(WorkflowCommand::PasteClips {
                    track,
                    lane: None,
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

    /// The song tick a note paste starts at: the playhead when it is inside the clip, else the
    /// clip's start.
    fn paste_tick_in(&self, clip: ClipId) -> i64 {
        match self.doc().song.clip(clip) {
            Some((_, _, c)) if self.position >= c.start && self.position < c.end() => self.position,
            Some((_, _, c)) => c.start,
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

    fn paste_devices(&mut self, snapshot: DeviceSnapshot, pane: Pane) -> Result<(), String> {
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
                        });
                    }
                }
                let at = {
                    let scope = self.doc().scope_devices(container.as_ref());
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
                };
                (container, at)
            }
        };
        self.apply_workflow(WorkflowCommand::PasteDevices {
            snapshot,
            container,
            at,
        })
    }

    /// Applies a workflow command, reporting a rejection as the message the refusal needs.
    fn apply_workflow(&mut self, command: WorkflowCommand) -> Result<(), String> {
        // A rejection is already reported by `apply_command`: the empty message tells the
        // caller not to report it a second time.
        self.apply_command(Command::Workflow(command))
            .map_err(|_| String::new())
    }

    // ------------------------------------------------------------------ before and after

    /// Before a workflow command: registers the pool entries a paste needs (a cross-song paste
    /// of audio clips or sampler zones: `RegisterAsset` is not an undo step) and remembers
    /// what the command will create.
    pub(super) fn workflow_pre(&mut self, command: &WorkflowCommand) -> WorkflowPre {
        let assets: Option<&std::collections::BTreeMap<String, AssetMeta>> = match command {
            WorkflowCommand::PasteDevices { snapshot, .. } => Some(&snapshot.assets),
            WorkflowCommand::PasteClips { clips, .. } => Some(&clips.assets),
            _ => None,
        };
        if let Some(assets) = assets {
            let missing: Vec<(String, AssetMeta)> = assets
                .iter()
                .filter(|(h, _)| !self.doc().assets.contains_key(*h))
                .map(|(h, m)| (h.clone(), m.clone()))
                .collect();
            for (hash, meta) in missing {
                let _ = self
                    .document
                    .apply(Command::RegisterAsset { hash, meta }, &self.registry);
            }
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
            WorkflowCommand::DuplicateDevices { keys } => {
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
                | WorkflowCommand::CreateInstrument { .. }
                | WorkflowCommand::CreateInstrumentAt { .. }
                | WorkflowCommand::CreateEffect { .. }
                | WorkflowCommand::CreateEffectAt { .. }
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
                if let Some((_, _, c)) = doc.song.clip(*clip) {
                    let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
                    pre.notes = Some((*clip, base, notes.notes.clone()));
                }
            }
            WorkflowCommand::DuplicateNotes { clip, indices } => {
                if let Some((_, _, c)) = doc.song.clip(*clip)
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
            WorkflowCommand::JoinClips { ids } => {
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
            WorkflowCommand::PasteDevices { .. }
            | WorkflowCommand::PasteDevicesAt { .. }
            | WorkflowCommand::DuplicateDevices { .. }
            | WorkflowCommand::DuplicateTrack { .. } => self.select_new_devices(&pre),
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
            | WorkflowCommand::AutoRoute { .. } => self.created(command, &pre),
            WorkflowCommand::RemoveDevices { .. }
            | WorkflowCommand::DeleteDevices { .. }
            | WorkflowCommand::MoveDevices { .. }
            | WorkflowCommand::MoveDevicesToSlot { .. }
            | WorkflowCommand::RemoveClips { .. }
            | WorkflowCommand::SetClipsMuted { .. }
            | WorkflowCommand::DisconnectDevice { .. }
            | WorkflowCommand::AssignSend { .. }
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
        let Some((_, _, c)) = self.doc().song.clip(clip) else {
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
