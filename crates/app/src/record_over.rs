//! Recording over what is already on a lane, and what to do with notes that land outside their
//! clip.
//!
//! # Record mode
//!
//! [`RecordMode`] says what a finished MIDI take does when the lane already has clips:
//!
//! - **Replace** (the default) writes the take as it always was: one `AddClip`, or one
//!   `NewTakeGroup` when the loop made several passes (the passes stacked as takes, the last one
//!   playing). A clip that starts later than the take's start still hides the take from there on
//!   (the lane's rule is by start, see `atomik_model::song::audible_spans`).
//! - **Layer** merges every pass into one note list and, when the lane has a clip under the take's
//!   start, merges that list into the clip (`WorkflowCommand::LayerRecording`: the old notes
//!   stay, a new note replaces one with the same start and key); otherwise the take is added as a
//!   new clip that plays over the clips that start inside it (`EngulfRecording`). Either way
//!   all the passes sound and the whole take is one undo step.
//!
//! Audio recorded over an existing clip becomes a take group (see the audio recording module):
//! that does not depend on the mode.
//!
//! # Keep events in clip
//!
//! A note drawn, moved or pasted so that it starts at or after the end of its clip is silent
//! (a clip plays only the notes inside its window). [`KeepEvents`] says what the editors do about
//! it, as part of the same undo step as the edit:
//!
//! - **Keep** leaves the note where it is (it stays in the clip, silent, as before).
//! - **Expand clip** lengthens the clip to the bar line after the farthest such note.
//! - **New clip** moves such notes to a new clip that starts on the bar line at or before the first
//!   of them (never before the old clip's end) and runs to the bar line after the last.
//!
//! The follow-up is computed from the document after the edit ([`keep_events_follow_up`]); a
//! looped clip, whose content past the loop never plays whatever the clip length, is left alone.

use atomik_model::song::{Clip, ClipContent, ClipId, Note};
use atomik_model::{Command, DeviceKey, RackDoc, SongCommand, WorkflowCommand};

/// What a finished MIDI take does with the clips already on its lane (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordMode {
    /// A new clip, or a take group for a loop recording.
    #[default]
    Replace,
    /// Merge into the clip under the take, or play over the clips it overlaps.
    Layer,
}

impl RecordMode {
    /// Every mode, in menu order.
    pub const ALL: [Self; 2] = [Self::Replace, Self::Layer];

    /// The name saved in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Layer => "layer",
        }
    }

    /// The mode saved as `id`, if it is one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Replace => "Replace (takes)",
            Self::Layer => "Layer",
        }
    }

    /// The next mode in [`Self::ALL`], wrapping.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// What the editors do with a note that starts outside its clip (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeepEvents {
    /// Leave it, silent.
    #[default]
    Keep,
    /// Lengthen the clip to hold it.
    Expand,
    /// Move it to a new clip.
    NewClip,
}

impl KeepEvents {
    /// Every policy, in menu order.
    pub const ALL: [Self; 3] = [Self::Keep, Self::Expand, Self::NewClip];

    /// The name saved in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Expand => "expand",
            Self::NewClip => "new_clip",
        }
    }

    /// The policy saved as `id`, if it is one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Keep => "Keep",
            Self::Expand => "Expand clip",
            Self::NewClip => "New clip",
        }
    }

    /// The next policy in [`Self::ALL`], wrapping.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The command that writes the passes `laps` (in pass order, at least one) of a take on note lane
/// `lane` of `device`, as `mode` says (module docs).
pub fn note_take_command(
    doc: &RackDoc,
    device: DeviceKey,
    lane: usize,
    mut laps: Vec<Clip>,
    mode: RecordMode,
) -> Command {
    if mode == RecordMode::Layer
        && let Some(merged) = atomik_model::merge_laps(&laps)
    {
        let covering = doc
            .song
            .track(&device)
            .and_then(|t| t.lanes.get(lane))
            .and_then(|l| atomik_model::covering_note_clip(l, merged.start));
        return Command::Workflow(match covering {
            Some(clip) => WorkflowCommand::LayerRecording { clip, take: merged },
            None => WorkflowCommand::EngulfRecording {
                device,
                lane,
                clip: merged,
            },
        });
    }
    Command::Song(if laps.len() > 1 {
        SongCommand::NewTakeGroup {
            device,
            lane,
            takes: laps,
        }
    } else {
        SongCommand::AddClip {
            device,
            lane,
            clip: laps.remove(0),
        }
    })
}

/// The commands that complete an edit of note clip `id` under `policy` (module docs), computed
/// from `doc` as the edit left it: empty when the policy is Keep, the clip is gone, is not an
/// unlooped note clip or holds no note that starts past its end.
pub fn keep_events_follow_up(doc: &RackDoc, id: ClipId, policy: KeepEvents) -> Vec<Command> {
    if policy == KeepEvents::Keep {
        return Vec::new();
    }
    let Some((track, lane, clip)) = doc.song.clip(id) else {
        return Vec::new();
    };
    let ClipContent::Notes(notes) = &clip.content else {
        return Vec::new();
    };
    if clip.loop_length.is_some() {
        return Vec::new();
    }
    let window_end = clip.offset + clip.length;
    let outside: Vec<&Note> = notes.iter().filter(|n| n.tick >= window_end).collect();
    let Some(first) = outside.first() else {
        return Vec::new();
    };
    let table = doc.song.meter_table();
    let meter = table.map();
    let ceil_bar = |t: i64| {
        if meter.bar_start(meter.bar_at(t)) == t {
            t
        } else {
            meter.next_bar_line(t)
        }
    };
    // Song tick of a content tick of this clip.
    let song_tick = |content: i64| clip.start + (content - clip.offset);
    let reach = outside
        .iter()
        .map(|n| song_tick(n.tick + n.length))
        .max()
        .unwrap_or(0);
    match policy {
        KeepEvents::Keep => Vec::new(),
        KeepEvents::Expand => vec![Command::Song(SongCommand::SetClipGeometry {
            id,
            start: clip.start,
            length: ceil_bar(reach) - clip.start,
            offset: clip.offset,
            loop_length: clip.loop_length,
            muted: clip.muted,
            merge: None,
        })],
        KeepEvents::NewClip => {
            let first_song = song_tick(first.tick);
            let start = meter
                .bar_start(meter.bar_at(first_song))
                .max(clip.end())
                .min(first_song);
            let end = ceil_bar(reach).max(start + 1);
            let moved: Vec<Note> = outside
                .iter()
                .map(|n| Note {
                    tick: song_tick(n.tick) - start,
                    ..**n
                })
                .collect();
            let kept: Vec<Note> = notes
                .iter()
                .filter(|n| n.tick < window_end)
                .copied()
                .collect();
            vec![
                Command::Song(SongCommand::SetClipContent {
                    id,
                    content: ClipContent::Notes(kept),
                    merge: None,
                }),
                Command::Song(SongCommand::AddClip {
                    device: track.device.clone(),
                    lane,
                    clip: Clip {
                        id: ClipId(0),
                        start,
                        length: end - start,
                        offset: 0,
                        loop_length: None,
                        muted: false,
                        grid: clip.grid,
                        scale: clip.scale,
                        controllers: Vec::new(),
                        content: ClipContent::Notes(moved),
                    },
                }),
            ]
        }
    }
}

#[cfg(test)]
#[path = "record_over_tests.rs"]
mod tests;
