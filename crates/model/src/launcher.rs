//! The clip launcher: a grid of looping clips (slots) and scenes (rows) the song can launch live.
//!
//! The launcher is a second way to make music next to the arrangement. A *slot* holds one clip
//! of a note or audio track; a *scene* is one row of slots across the tracks. The launcher lives in
//! [`Song::launcher`](crate::song::Song::launcher): the slots of a track are kept under its
//! device key (like the track heights), indexed by scene row, so a track without slots costs
//! nothing and a track that is deleted and brought back by undo gets its slots back.
//!
//! A slot clip is an ordinary [`Clip`]: its notes, controller lanes, parameter lanes, step grid,
//! scale and groove are the clip editors' own, and the clip editors open it by its id. Four
//! fields mean something different from a lane's clip: `start` is always 0, `length` is the
//! loop length in ticks, `offset` and `loop_length` shape the content inside the loop as they
//! do in the arrangement, and `name` and `color` are the slot's. The launch quantize override
//! is the slot's own ([`LauncherClip::quantize`]).
//!
//! Note tracks and audio tracks have slots: a note slot holds a note clip, an audio slot an audio
//! clip (its file, gain, fades and so on; the launcher plays it unwarped).
//!
//! Every command is undoable and is one [`Edit::Launcher`] (plus the clip ids it allocates).

use crate::history::Edit;
use crate::song::{
    Clip, ClipContent, ClipId, LaneKind, MAX_MARKER_NAME, MAX_TICK, MetaChange, SongCommand, Track,
    clip_name_ok,
};
use crate::song_cmd::{bad, check_new_clip, insert_new, range, track_index};
use crate::{DeviceKey, ModelError, RackDoc, TEMPO_MAX, TEMPO_MIN};
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

/// The most scenes (rows) a song has.
pub const MAX_SCENES: usize = 128;
/// The longest scene name, in characters.
pub const MAX_SCENE_NAME: usize = MAX_MARKER_NAME;
/// The shortest slot loop, in ticks (a sixteenth note).
pub const MIN_LOOP_TICKS: i64 = PPQ / 4;
/// The longest slot loop, in ticks (128 bars of 4/4).
pub const MAX_LOOP_TICKS: i64 = PPQ * 4 * 128;
/// The loop length of a new empty slot clip: one bar of 4/4.
pub const DEFAULT_LOOP_TICKS: i64 = PPQ * 4;

/// Where a launch or a stop waits for before it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LaunchQuantize {
    /// At once (the next tick).
    None,
    /// The next sixteenth note.
    Sixteenth,
    /// The next eighth note.
    Eighth,
    /// The next quarter note.
    Quarter,
    /// The next half note.
    Half,
    /// The next bar line.
    #[default]
    Bar,
    /// The next bar line of every second bar.
    TwoBars,
    /// The next bar line of every fourth bar.
    FourBars,
}

impl LaunchQuantize {
    /// Every value, shortest first.
    pub const ALL: [LaunchQuantize; 8] = [
        LaunchQuantize::None,
        LaunchQuantize::Sixteenth,
        LaunchQuantize::Eighth,
        LaunchQuantize::Quarter,
        LaunchQuantize::Half,
        LaunchQuantize::Bar,
        LaunchQuantize::TwoBars,
        LaunchQuantize::FourBars,
    ];

    /// The name the project file and the commands use.
    pub fn key(self) -> &'static str {
        match self {
            LaunchQuantize::None => "none",
            LaunchQuantize::Sixteenth => "1/16",
            LaunchQuantize::Eighth => "1/8",
            LaunchQuantize::Quarter => "1/4",
            LaunchQuantize::Half => "1/2",
            LaunchQuantize::Bar => "1 bar",
            LaunchQuantize::TwoBars => "2 bars",
            LaunchQuantize::FourBars => "4 bars",
        }
    }

    /// The value a [`LaunchQuantize::key`] names.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.key() == key)
    }

    /// The short label menus show.
    pub fn label(self) -> &'static str {
        match self {
            LaunchQuantize::None => "None",
            other => other.key(),
        }
    }

    /// The grid in ticks of a quantize that does not follow the meter (`None` for the bar
    /// values, and 1 for `None`: the next tick).
    pub fn fixed_ticks(self) -> Option<i64> {
        match self {
            LaunchQuantize::None => Some(1),
            LaunchQuantize::Sixteenth => Some(PPQ / 4),
            LaunchQuantize::Eighth => Some(PPQ / 2),
            LaunchQuantize::Quarter => Some(PPQ),
            LaunchQuantize::Half => Some(PPQ * 2),
            _ => None,
        }
    }

    /// Bars a bar quantize counts (1, 2 or 4), `None` for the others.
    pub fn bars(self) -> Option<u32> {
        match self {
            LaunchQuantize::Bar => Some(1),
            LaunchQuantize::TwoBars => Some(2),
            LaunchQuantize::FourBars => Some(4),
            _ => None,
        }
    }
}

/// A scene: one row of the launcher.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    /// The scene's name; empty shows "Scene N". Trimmed, at most [`MAX_SCENE_NAME`] characters.
    pub name: String,
    /// Optional colour.
    pub color: Option<[u8; 3]>,
    /// A tempo the scene is meant for, in BPM. Stored with the scene; launching does not apply it
    /// yet.
    pub tempo: Option<f64>,
}

/// The clip in a slot.
#[derive(Debug, Clone, PartialEq)]
pub struct LauncherClip {
    /// The clip: `start` 0, `length` the loop length in ticks (see the module docs).
    pub clip: Clip,
    /// Launch quantize of this slot alone; `None` follows [`Launcher::quantize`].
    pub quantize: Option<LaunchQuantize>,
}

impl LauncherClip {
    /// The loop length in ticks.
    pub fn loop_ticks(&self) -> i64 {
        self.clip.length
    }
}

/// The song's launcher.
#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    /// The scenes, top to bottom.
    pub scenes: Vec<Scene>,
    /// The launch quantize slots follow unless they override it.
    pub quantize: LaunchQuantize,
    /// Launching a scene stops the tracks whose slot in that row is empty.
    pub stop_empty: bool,
    /// The slots of each track by scene row. A list never has more entries than there are scenes
    /// and never ends in an empty slot ([`Launcher::canonical`]).
    pub slots: BTreeMap<DeviceKey, Vec<Option<LauncherClip>>>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            scenes: Vec::new(),
            quantize: LaunchQuantize::Bar,
            stop_empty: true,
            slots: BTreeMap::new(),
        }
    }
}

impl Launcher {
    /// The clip in slot `row` of `device`'s track.
    pub fn slot(&self, device: &DeviceKey, row: usize) -> Option<&LauncherClip> {
        self.slots.get(device)?.get(row)?.as_ref()
    }

    /// A slot clip by its clip id, with its track and row.
    pub fn find(&self, id: ClipId) -> Option<(&DeviceKey, usize, &LauncherClip)> {
        self.slots.iter().find_map(|(d, list)| {
            list.iter()
                .enumerate()
                .find_map(|(row, s)| s.as_ref().filter(|c| c.clip.id == id).map(|c| (d, row, c)))
        })
    }

    /// Every slot clip, by track and row.
    pub fn clips(&self) -> impl Iterator<Item = (&DeviceKey, usize, &LauncherClip)> {
        self.slots.iter().flat_map(|(d, list)| {
            list.iter()
                .enumerate()
                .filter_map(move |(row, s)| s.as_ref().map(|c| (d, row, c)))
        })
    }

    /// The launch quantize slot `row` of `device` uses.
    pub fn quantize_of(&self, device: &DeviceKey, row: usize) -> LaunchQuantize {
        self.slot(device, row)
            .and_then(|c| c.quantize)
            .unwrap_or(self.quantize)
    }

    /// True when nothing differs from a song without a launcher.
    pub fn is_default(&self) -> bool {
        self.scenes.is_empty()
            && self.quantize == LaunchQuantize::Bar
            && self.stop_empty
            && self.slots.is_empty()
    }

    /// Drops trailing empty slots and empty lists, so equal launchers compare equal.
    pub fn canonical(&mut self) {
        for list in self.slots.values_mut() {
            while list.last().is_some_and(Option::is_none) {
                list.pop();
            }
        }
        self.slots.retain(|_, list| !list.is_empty());
    }

    /// The label of scene `row`: its name, or "Scene N".
    pub fn scene_label(&self, row: usize) -> String {
        match self.scenes.get(row) {
            Some(s) if !s.name.is_empty() => s.name.clone(),
            _ => format!("Scene {}", row + 1),
        }
    }

    /// Grows the scene list to `rows` scenes.
    fn ensure_rows(&mut self, rows: usize) -> Result<(), ModelError> {
        if rows > MAX_SCENES {
            return Err(range(format!("a song has at most {MAX_SCENES} scenes")));
        }
        while self.scenes.len() < rows {
            self.scenes.push(Scene::default());
        }
        Ok(())
    }

    fn put(&mut self, device: &DeviceKey, row: usize, clip: Option<LauncherClip>) {
        let list = self.slots.entry(device.clone()).or_default();
        if list.len() <= row {
            list.resize(row + 1, None);
        }
        list[row] = clip;
    }

    /// Checks the launcher against its rules; `Err` names the first one broken.
    pub fn check(&self) -> Result<(), String> {
        if self.scenes.len() > MAX_SCENES {
            return Err(format!("{} scenes", self.scenes.len()));
        }
        for s in &self.scenes {
            if !s.name.is_empty() && !clip_name_ok(&s.name) {
                return Err(format!("scene name {:?}", s.name));
            }
            if let Some(t) = s.tempo
                && !(TEMPO_MIN..=TEMPO_MAX).contains(&t)
            {
                return Err(format!("scene tempo {t}"));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for (device, list) in &self.slots {
            if list.len() > self.scenes.len() {
                return Err(format!("{device} has slots below the last scene"));
            }
            for (row, slot) in list.iter().enumerate() {
                let Some(c) = slot else { continue };
                if !ids.insert(c.clip.id) {
                    return Err(format!("clip id {} used twice", c.clip.id.0));
                }
                check_slot_clip(&c.clip).map_err(|e| format!("{device} slot {row}: {e}"))?;
            }
        }
        Ok(())
    }
}

/// What a slot clip must be.
fn check_slot_clip(c: &Clip) -> Result<(), String> {
    if c.start != 0 {
        return Err("starts at tick 0".into());
    }
    if !(MIN_LOOP_TICKS..=MAX_LOOP_TICKS).contains(&c.length) {
        return Err(format!("loop length {}", c.length));
    }
    if matches!(c.content, ClipContent::Automation(_)) {
        return Err("holds notes or audio".into());
    }
    if c.content.is_audio() && c.offset != 0 {
        return Err("an audio clip has no content offset".into());
    }
    if c.offset < 0 || c.offset > MAX_TICK || c.loop_length.is_some_and(|n| n <= 0 || n > MAX_TICK)
    {
        return Err("offset or loop out of range".into());
    }
    Ok(())
}

/// A slot's address: a track's device and a scene row.
pub type SlotRef = (DeviceKey, usize);

/// Launcher edits (wrapped in [`SongCommand::Launcher`]). Every command is undoable and one
/// undo step. A command that names a row past the last scene adds scenes up to it.
#[derive(Debug, Clone, PartialEq)]
// A command is built once per user action; boxing the one ready clip would only add an
// indirection at every site that makes one.
#[allow(clippy::large_enum_variant)]
pub enum LauncherCommand {
    /// Puts a copy of the arrangement clip `clip` in a slot (what dragging a clip into the
    /// launcher does). The slot's old clip goes.
    PutClip {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
        /// The arrangement clip to copy.
        clip: ClipId,
    },
    /// A new empty note clip of `length` ticks in a slot.
    NewClip {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
        /// Loop length in ticks.
        length: i64,
    },
    /// Puts a ready clip in a slot (a paste). The clip gets a new id.
    SetSlot {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
        /// The clip and its quantize override.
        clip: LauncherClip,
    },
    /// Moves a slot, or copies it with `copy`; the target slot's old clip goes. The tracks must
    /// take the same kind of clip.
    MoveSlot {
        /// Where the clip is now.
        from: SlotRef,
        /// Where it goes.
        to: SlotRef,
        /// Keep the clip where it is too (the copy gets a new id).
        copy: bool,
    },
    /// Empties slots.
    ClearSlots {
        /// The slots.
        slots: Vec<SlotRef>,
    },
    /// Changes a slot's name, colour, loop length or quantize.
    EditSlot {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
        /// The clip's name.
        name: MetaChange<String>,
        /// The clip's colour.
        color: MetaChange<[u8; 3]>,
        /// A new loop length in ticks.
        length: Option<i64>,
        /// A new quantize override (`Some(None)` follows the song's).
        quantize: Option<Option<LaunchQuantize>>,
    },
    /// Copies slot `row` of `device` into the arrangement at `start`, as a clip one loop long, on
    /// the same track or, with `onto`, on another track that takes the same kind of clip.
    ToArrangement {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
        /// Song tick where the clip starts.
        start: i64,
        /// The track that gets the clip (`None`: the slot's own).
        onto: Option<DeviceKey>,
    },
    /// Inserts an empty scene before row `at` (`at == scenes` adds one at the end).
    InsertScene {
        /// Row of the new scene.
        at: usize,
    },
    /// Deletes scene `at` and its slots.
    DeleteScene {
        /// The row.
        at: usize,
    },
    /// Duplicates scene `at` with its slots (new clip ids) just below it.
    DuplicateScene {
        /// The row.
        at: usize,
    },
    /// Renames a scene; `None` goes back to "Scene N".
    RenameScene {
        /// The row.
        at: usize,
        /// The name.
        name: Option<String>,
    },
    /// Colours a scene.
    SetSceneColor {
        /// The row.
        at: usize,
        /// The colour, or none.
        color: Option<[u8; 3]>,
    },
    /// Gives a scene a tempo, or none.
    SetSceneTempo {
        /// The row.
        at: usize,
        /// BPM.
        tempo: Option<f64>,
    },
    /// The launch quantize slots follow.
    SetQuantize(LaunchQuantize),
    /// Whether launching a scene stops the tracks whose slot is empty.
    SetStopEmpty(bool),
}

/// The lane of `track` a slot clip is edited on: its first note lane, or the audio lane.
pub(crate) fn slot_lane(track: &Track) -> usize {
    track
        .lanes
        .iter()
        .position(|l| matches!(l.kind, LaneKind::Notes | LaneKind::Audio))
        .unwrap_or(0)
}

/// The track a slot can be on: a note track or an audio track.
fn slot_track<'d>(doc: &'d RackDoc, device: &DeviceKey) -> Result<&'d Track, ModelError> {
    let t = &doc.song.tracks[track_index(doc, device)?];
    if t.is_automation_only() || t.lanes.is_empty() {
        return Err(bad(format!("{device} has no slots")));
    }
    Ok(t)
}

/// A clip of `content` goes in the slots of `track`: notes on a note track, audio on an audio track.
fn content_fits(track: &Track, content: &ClipContent) -> Result<(), ModelError> {
    match (track.is_audio(), content) {
        (false, ClipContent::Notes(_)) | (true, ClipContent::Audio(_)) => Ok(()),
        (true, _) => Err(bad("an audio track takes audio clips")),
        (false, _) => Err(bad("a note track takes note clips")),
    }
}

fn scene_at(l: &Launcher, at: usize) -> Result<(), ModelError> {
    if at < l.scenes.len() {
        Ok(())
    } else {
        Err(range(format!("no scene {at}")))
    }
}

fn check_name(name: &str) -> Result<(), ModelError> {
    if name.is_empty() || clip_name_ok(name) {
        Ok(())
    } else {
        Err(range(
            "a name is trimmed, short and has no control characters",
        ))
    }
}

/// A fresh id source over `next_clip_id`.
struct Ids {
    first: u64,
    next: u64,
}

impl Ids {
    fn take(&mut self) -> Result<ClipId, ModelError> {
        let id = self.next;
        self.next = id
            .checked_add(1)
            .ok_or_else(|| bad("clip ids are exhausted"))?;
        Ok(ClipId(id))
    }
}

fn finish(old: &Launcher, mut new: Launcher, ids: &Ids) -> Result<Edit, ModelError> {
    new.canonical();
    new.check().map_err(bad)?;
    let launcher = Edit::Launcher {
        old: Box::new(old.clone()),
        new: Box::new(new),
    };
    if ids.next == ids.first {
        return Ok(launcher);
    }
    Ok(Edit::Compound(vec![
        launcher,
        Edit::NextClipId {
            old: ids.first,
            new: ids.next,
        },
    ]))
}

/// A copy of `clip` as a slot clip with id `id`.
fn slot_copy(clip: &Clip, id: ClipId) -> Clip {
    Clip {
        id,
        start: 0,
        length: clip.length.clamp(MIN_LOOP_TICKS, MAX_LOOP_TICKS),
        muted: false,
        ..clip.clone()
    }
}

/// Plans a launcher command: the edit and the id of the clip it allocated, if any.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: LauncherCommand,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let old = &doc.song.launcher;
    let mut new = old.clone();
    let mut ids = Ids {
        first: doc.song.next_clip_id,
        next: doc.song.next_clip_id,
    };
    let mut made = None;
    match command {
        LauncherCommand::PutClip { device, row, clip } => {
            let target = slot_track(doc, &device)?;
            let (_, _, source) = doc
                .song
                .clip(clip)
                .ok_or_else(|| bad(format!("no clip {}", clip.0)))?;
            content_fits(target, &source.content)?;
            new.ensure_rows(row + 1)?;
            let id = ids.take()?;
            made = Some(id);
            new.put(
                &device,
                row,
                Some(LauncherClip {
                    clip: slot_copy(source, id),
                    quantize: None,
                }),
            );
        }
        LauncherCommand::NewClip {
            device,
            row,
            length,
        } => {
            if slot_track(doc, &device)?.is_audio() {
                return Err(bad("an audio slot needs a file: put an audio clip in it"));
            }
            if !(MIN_LOOP_TICKS..=MAX_LOOP_TICKS).contains(&length) {
                return Err(range("a loop is between a sixteenth note and 128 bars"));
            }
            new.ensure_rows(row + 1)?;
            let id = ids.take()?;
            made = Some(id);
            let clip = Clip {
                id,
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                name: None,
                color: None,
                grid: None,
                scale: None,
                controllers: Vec::new(),
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                content: ClipContent::Notes(Vec::new().into()),
            };
            new.put(
                &device,
                row,
                Some(LauncherClip {
                    clip,
                    quantize: None,
                }),
            );
        }
        LauncherCommand::SetSlot { device, row, clip } => {
            let track = slot_track(doc, &device)?;
            let lane = slot_lane(track);
            let checked = check_new_clip(doc, registry, &device, lane, clip.clip)?;
            new.ensure_rows(row + 1)?;
            let id = ids.take()?;
            made = Some(id);
            new.put(
                &device,
                row,
                Some(LauncherClip {
                    clip: slot_copy(&checked, id),
                    quantize: clip.quantize,
                }),
            );
        }
        LauncherCommand::MoveSlot { from, to, copy } => {
            let target = slot_track(doc, &to.0)?;
            let source = old
                .slot(&from.0, from.1)
                .ok_or_else(|| bad("that slot is empty"))?
                .clone();
            if from == to {
                return Err(bad("the slot did not move"));
            }
            content_fits(target, &source.clip.content)?;
            new.ensure_rows(to.1 + 1)?;
            let clip = if copy {
                let id = ids.take()?;
                made = Some(id);
                LauncherClip {
                    clip: slot_copy(&source.clip, id),
                    ..source
                }
            } else {
                made = Some(source.clip.id);
                new.put(&from.0, from.1, None);
                source
            };
            new.put(&to.0, to.1, Some(clip));
        }
        LauncherCommand::ClearSlots { slots } => {
            let mut any = false;
            for (device, row) in slots {
                if new.slot(&device, row).is_some() {
                    new.put(&device, row, None);
                    any = true;
                }
            }
            if !any {
                return Err(bad("those slots are empty"));
            }
        }
        LauncherCommand::EditSlot {
            device,
            row,
            name,
            color,
            length,
            quantize,
        } => {
            let mut slot = old
                .slot(&device, row)
                .ok_or_else(|| bad("that slot is empty"))?
                .clone();
            if let MetaChange::Set(n) = &name
                && !clip_name_ok(n)
            {
                return Err(range(
                    "a name is trimmed, short and has no control characters",
                ));
            }
            slot.clip.name = name.apply(&slot.clip.name);
            slot.clip.color = color.apply(&slot.clip.color);
            if let Some(len) = length {
                if !(MIN_LOOP_TICKS..=MAX_LOOP_TICKS).contains(&len) {
                    return Err(range("a loop is between a sixteenth note and 128 bars"));
                }
                slot.clip.length = len;
            }
            if let Some(q) = quantize {
                slot.quantize = q;
            }
            new.put(&device, row, Some(slot));
        }
        LauncherCommand::ToArrangement {
            device,
            row,
            start,
            onto,
        } => {
            let slot = old
                .slot(&device, row)
                .ok_or_else(|| bad("that slot is empty"))?;
            let target_key = onto.unwrap_or_else(|| device.clone());
            let target = slot_track(doc, &target_key)?;
            content_fits(target, &slot.clip.content)?;
            let lane = slot_lane(target);
            if !(0..=MAX_TICK).contains(&start) {
                return Err(range("tick out of range"));
            }
            let clip = Clip {
                start,
                ..slot.clip.clone()
            };
            let clip = check_new_clip(doc, registry, &target_key, lane, clip)?;
            let (edit, id) = insert_new(doc, target_key, lane, clip)?;
            return Ok((edit, Some(id)));
        }
        LauncherCommand::InsertScene { at } => {
            if at > new.scenes.len() {
                return Err(range(format!("no scene {at}")));
            }
            if new.scenes.len() >= MAX_SCENES {
                return Err(range(format!("a song has at most {MAX_SCENES} scenes")));
            }
            new.scenes.insert(at, Scene::default());
            for list in new.slots.values_mut() {
                if at < list.len() {
                    list.insert(at, None);
                }
            }
        }
        LauncherCommand::DeleteScene { at } => {
            scene_at(old, at)?;
            new.scenes.remove(at);
            for list in new.slots.values_mut() {
                if at < list.len() {
                    list.remove(at);
                }
            }
        }
        LauncherCommand::DuplicateScene { at } => {
            scene_at(old, at)?;
            if new.scenes.len() >= MAX_SCENES {
                return Err(range(format!("a song has at most {MAX_SCENES} scenes")));
            }
            new.scenes.insert(at + 1, old.scenes[at].clone());
            for list in new.slots.values_mut() {
                if at < list.len() {
                    let copy = match list.get(at).cloned().flatten() {
                        Some(c) => Some(LauncherClip {
                            clip: slot_copy(&c.clip, ids.take()?),
                            ..c
                        }),
                        None => None,
                    };
                    list.insert(at + 1, copy);
                }
            }
        }
        LauncherCommand::RenameScene { at, name } => {
            scene_at(old, at)?;
            let name = name.unwrap_or_default();
            check_name(&name)?;
            new.scenes[at].name = name;
        }
        LauncherCommand::SetSceneColor { at, color } => {
            scene_at(old, at)?;
            new.scenes[at].color = color;
        }
        LauncherCommand::SetSceneTempo { at, tempo } => {
            scene_at(old, at)?;
            if let Some(t) = tempo
                && !(TEMPO_MIN..=TEMPO_MAX).contains(&t)
            {
                return Err(range("tempo out of range"));
            }
            new.scenes[at].tempo = tempo;
        }
        LauncherCommand::SetQuantize(q) => new.quantize = q,
        LauncherCommand::SetStopEmpty(on) => new.stop_empty = on,
    }
    Ok((finish(old, new, &ids)?, made))
}

/// The slot clip a clip-editing command names, when its clip id is a slot clip and not a clip of
/// a lane or a take (what the piano roll sends while it is open on a slot).
pub(crate) fn slot_target(doc: &RackDoc, command: &SongCommand) -> Option<ClipId> {
    let id = match command {
        SongCommand::SetClipContent { id, .. }
        | SongCommand::SetAudioClip { id, .. }
        | SongCommand::SetWarpMarkers { id, .. }
        | SongCommand::SetPitchEdits { id, .. } => *id,
        SongCommand::SetClipGrid { clip, .. }
        | SongCommand::SetClipControllers { clip, .. }
        | SongCommand::SetClipMidi { clip, .. }
        | SongCommand::SetClipParamLanes { clip, .. }
        | SongCommand::SetClipScale { clip, .. }
        | SongCommand::QuantizeNotes { clip, .. } => *clip,
        _ => return None,
    };
    is_slot(doc, id).then_some(id)
}

/// The slot clip a note paste or duplicate (the piano roll's clipboard) names.
pub(crate) fn slot_target_workflow(
    doc: &RackDoc,
    command: &crate::WorkflowCommand,
) -> Option<ClipId> {
    match command {
        crate::WorkflowCommand::PasteNotes { clip, .. }
        | crate::WorkflowCommand::DuplicateNotes { clip, .. } => {
            is_slot(doc, *clip).then_some(*clip)
        }
        _ => None,
    }
}

fn is_slot(doc: &RackDoc, id: ClipId) -> bool {
    doc.song.clip(id).is_none()
        && doc.song.take_clip(id).is_none()
        && doc.song.launcher.find(id).is_some()
}

/// Plans a clip-editing command on slot clip `id`: `plan` is the command's planner, run on a
/// scratch document where the slot clip is a clip of its track's note lane, and the clip the
/// edit leaves replaces the slot's clip. One undo step, like the command makes on a plain clip.
pub(crate) fn plan_slot_edit(
    doc: &RackDoc,
    id: ClipId,
    plan: impl FnOnce(&RackDoc) -> Result<Edit, ModelError>,
) -> Result<Edit, ModelError> {
    let (device, row, slot) = doc
        .song
        .launcher
        .find(id)
        .ok_or_else(|| ModelError::UnknownKey(format!("slot clip {}", id.0)))?;
    let (device, slot) = (device.clone(), slot.clone());
    let ti = track_index(doc, &device)?;
    let lane = slot_lane(&doc.song.tracks[ti]);
    let mut scratch = doc.clone();
    scratch.song_mut().tracks[ti].lanes[lane]
        .clips
        .push(slot.clip.clone());
    let edit = plan(&scratch)?;
    edit.perform(&mut scratch)
        .map_err(|e| bad(format!("that edit cannot be made on a slot: {e}")))?;
    let edited = scratch.song.tracks[ti].lanes[lane]
        .clips
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| bad("that edit cannot be made on a slot"))?;
    let mut new = doc.song.launcher.clone();
    let length = edited.length.clamp(MIN_LOOP_TICKS, MAX_LOOP_TICKS);
    new.put(
        &device,
        row,
        Some(LauncherClip {
            clip: Clip {
                start: 0,
                length,
                ..edited.clone()
            },
            quantize: slot.quantize,
        }),
    );
    let ids = Ids {
        first: doc.song.next_clip_id,
        next: scratch.song.next_clip_id.max(doc.song.next_clip_id),
    };
    finish(&doc.song.launcher, new, &ids)
}

/// The undo-menu label of a launcher command.
pub(crate) fn label(command: &LauncherCommand) -> &'static str {
    match command {
        LauncherCommand::PutClip { .. }
        | LauncherCommand::NewClip { .. }
        | LauncherCommand::SetSlot { .. } => "Set Launcher Clip",
        LauncherCommand::MoveSlot { copy: true, .. } => "Copy Launcher Clip",
        LauncherCommand::MoveSlot { .. } => "Move Launcher Clip",
        LauncherCommand::ClearSlots { .. } => "Delete Launcher Clip",
        LauncherCommand::EditSlot { .. } => "Edit Launcher Clip",
        LauncherCommand::ToArrangement { .. } => "Launcher Clip to Arrangement",
        LauncherCommand::InsertScene { .. } => "Insert Scene",
        LauncherCommand::DeleteScene { .. } => "Delete Scene",
        LauncherCommand::DuplicateScene { .. } => "Duplicate Scene",
        LauncherCommand::RenameScene { .. } => "Rename Scene",
        LauncherCommand::SetSceneColor { .. } | LauncherCommand::SetSceneTempo { .. } => {
            "Change Scene"
        }
        LauncherCommand::SetQuantize(_) | LauncherCommand::SetStopEmpty(_) => {
            "Change Launcher Settings"
        }
    }
}
