//! The in-process clipboard.
//!
//! One typed snapshot of what was last copied or cut: clips with their track-relative layout,
//! notes, automation points, devices (with the cables between them) or tracks (a device
//! snapshot that also carries its song tracks). The snapshots are plain data from
//! `atomik_model::workflow` that own what they copied and reference no document, so the
//! holder survives New and Open inside one run (a cross-song paste). It is not shared with the
//! operating system's clipboard (no dependency, no serialisation contract: backlog).

use atomik_model::{ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot};

/// What the clipboard holds.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipboardData {
    /// Clips from the arrangement.
    Clips(ClipSnapshot),
    /// Notes from the piano roll.
    Notes(NoteSnapshot),
    /// Automation points from an automation clip.
    AutomationPoints(PointSnapshot),
    /// Devices from the rack, with the cables between them.
    Devices(DeviceSnapshot),
    /// Tracks: devices together with their song tracks.
    Tracks(DeviceSnapshot),
    /// The settings of one device (its whole sound), for Paste Settings.
    Settings(DeviceSnapshot),
}

/// The kind of [`ClipboardData`], for messages and enabling rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardKind {
    /// Clips.
    Clips,
    /// Notes.
    Notes,
    /// Automation points.
    AutomationPoints,
    /// Devices.
    Devices,
    /// Tracks.
    Tracks,
    /// The settings of one device.
    Settings,
}

impl ClipboardKind {
    /// A plural noun for messages (`3 clips` is built by the caller).
    pub fn noun(self) -> &'static str {
        match self {
            ClipboardKind::Clips => "clips",
            ClipboardKind::Notes => "notes",
            ClipboardKind::AutomationPoints => "automation points",
            ClipboardKind::Devices => "devices",
            ClipboardKind::Tracks => "tracks",
            ClipboardKind::Settings => "settings",
        }
    }
}

impl ClipboardData {
    /// The kind of data.
    pub fn kind(&self) -> ClipboardKind {
        match self {
            ClipboardData::Clips(_) => ClipboardKind::Clips,
            ClipboardData::Notes(_) => ClipboardKind::Notes,
            ClipboardData::AutomationPoints(_) => ClipboardKind::AutomationPoints,
            ClipboardData::Devices(_) => ClipboardKind::Devices,
            ClipboardData::Tracks(_) => ClipboardKind::Tracks,
            ClipboardData::Settings(_) => ClipboardKind::Settings,
        }
    }

    /// How many items it holds.
    pub fn len(&self) -> usize {
        match self {
            ClipboardData::Clips(s) => s.entries.len(),
            ClipboardData::Notes(s) => s.notes.len(),
            ClipboardData::AutomationPoints(s) => s.points.len(),
            ClipboardData::Devices(s) | ClipboardData::Tracks(s) | ClipboardData::Settings(s) => {
                s.devices.len()
            }
        }
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How far a repeated paste advances the playhead: the span of the copied clips, notes or
    /// points (0 for devices and tracks, which have no time).
    pub fn span(&self) -> i64 {
        match self {
            ClipboardData::Clips(s) => s.span,
            ClipboardData::Notes(s) => s.span,
            ClipboardData::AutomationPoints(s) => s.span,
            ClipboardData::Devices(_) | ClipboardData::Tracks(_) | ClipboardData::Settings(_) => 0,
        }
    }
}

/// The holder. It lives in the app core, which New and Open keep, so a copy survives both.
#[derive(Debug, Default, Clone)]
pub struct Clipboard {
    data: Option<ClipboardData>,
}

impl Clipboard {
    /// An empty clipboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the contents.
    pub fn set(&mut self, data: ClipboardData) {
        self.data = Some(data);
    }

    /// The contents, if any.
    pub fn get(&self) -> Option<&ClipboardData> {
        self.data.as_ref()
    }

    /// What the clipboard holds.
    pub fn kind(&self) -> Option<ClipboardKind> {
        self.data.as_ref().map(ClipboardData::kind)
    }

    /// Empties the clipboard.
    pub fn clear(&mut self) {
        self.data = None;
    }

    /// Whether nothing was copied.
    pub fn is_empty(&self) -> bool {
        self.data.as_ref().is_none_or(ClipboardData::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::Note;

    fn notes(n: usize, span: i64) -> ClipboardData {
        ClipboardData::Notes(NoteSnapshot {
            notes: (0..n).map(|i| Note::new(i as i64, 1, 60, 0.5)).collect(),
            span,
        })
    }

    #[test]
    fn an_empty_holder_reports_nothing_and_a_set_value_is_typed() {
        let mut c = Clipboard::new();
        assert!(c.is_empty() && c.get().is_none() && c.kind().is_none());
        c.set(notes(3, 48));
        assert_eq!(c.kind(), Some(ClipboardKind::Notes));
        assert_eq!(c.get().unwrap().len(), 3);
        assert_eq!(c.get().unwrap().span(), 48);
        assert!(!c.is_empty());
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn a_snapshot_with_no_items_counts_as_empty() {
        let mut c = Clipboard::new();
        c.set(notes(0, 0));
        assert!(c.is_empty());
    }

    #[test]
    fn the_kinds_have_plain_nouns() {
        assert_eq!(ClipboardKind::AutomationPoints.noun(), "automation points");
        assert_eq!(ClipboardKind::Tracks.noun(), "tracks");
    }
}
