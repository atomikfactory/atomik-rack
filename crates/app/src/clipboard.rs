//! The in-process clipboard.
//!
//! One typed snapshot of what was last copied or cut: clips with their track-relative layout,
//! notes, automation points, devices (with the cables between them) or tracks (a device
//! snapshot that also carries its song tracks). The snapshots are plain data from
//! `atomik_model::workflow` that own what they copied and reference no document, so the
//! holder survives New and Open inside one run (a cross-song paste). Clips, notes, automation
//! points, devices and tracks are also written to the operating system's clipboard as text, so
//! another running instance can paste them (see [`crate::clipboard_os`]); the holder remembers
//! which system text it already stands for.

use atomik_model::controller_edit::ControllerSnapshot;
use atomik_model::{
    ChannelSettingsClip, ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot, TakeSnapshot,
};

/// Console strips: the devices (and song tracks) the channels carry, and the settings of each
/// channel, which a device snapshot leaves out (a pasted instrument gets a fresh channel).
#[derive(Debug, Clone, PartialEq)]
pub struct StripSnapshot {
    /// The devices that feed the copied channels, with their tracks.
    pub devices: DeviceSnapshot,
    /// The settings of the channel of each device, keyed by its index in `devices.devices`.
    pub settings: Vec<(usize, ChannelSettingsClip)>,
    /// The strips with no device behind them: mixer channels and output buses.
    pub loose: Vec<LooseStrip>,
}

/// What kind of strip a [`LooseStrip`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LooseKind {
    /// A mixer channel (a channel with no source device).
    Mix,
    /// An output bus.
    Bus,
}

/// A copied strip that has no instrument or track: only its name and settings.
#[derive(Debug, Clone, PartialEq)]
pub struct LooseStrip {
    /// Channel or bus.
    pub kind: LooseKind,
    /// The name the strip had.
    pub name: String,
    /// Its settings (every section).
    pub clip: ChannelSettingsClip,
}

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
    /// Console strips: tracks together with the settings of their channels.
    Strips(StripSnapshot),
    /// The settings of one device (its whole sound), for Paste Settings.
    Settings(DeviceSnapshot),
    /// Takes of a take group, for Paste Takes.
    Takes(TakeSnapshot),
    /// Points of one controller lane of a note clip (bend, aftertouch or a CC).
    Controllers(ControllerSnapshot),
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
    /// Console strips.
    Strips,
    /// The settings of one device.
    Settings,
    /// Takes of a take group.
    Takes,
    /// Controller points of a note clip.
    Controllers,
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
            ClipboardKind::Strips => "strips",
            ClipboardKind::Settings => "settings",
            ClipboardKind::Takes => "takes",
            ClipboardKind::Controllers => "controller points",
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
            ClipboardData::Strips(_) => ClipboardKind::Strips,
            ClipboardData::Settings(_) => ClipboardKind::Settings,
            ClipboardData::Takes(_) => ClipboardKind::Takes,
            ClipboardData::Controllers(_) => ClipboardKind::Controllers,
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
            ClipboardData::Strips(s) => s.devices.devices.len() + s.loose.len(),
            ClipboardData::Takes(s) => s.takes.len(),
            ClipboardData::Controllers(s) => s.points.len(),
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
            ClipboardData::Controllers(s) => s.span,
            ClipboardData::Devices(_)
            | ClipboardData::Tracks(_)
            | ClipboardData::Strips(_)
            | ClipboardData::Settings(_)
            | ClipboardData::Takes(_) => 0,
        }
    }
}

/// The holder. It lives in the app core, which New and Open keep, so a copy survives both.
#[derive(Debug, Default, Clone)]
pub struct Clipboard {
    data: Option<ClipboardData>,
    /// The system clipboard text this holder stands for: what the last Copy wrote, or the text it
    /// adopted from another instance. A Paste that finds the same text there uses `data`; one
    /// that finds another Atomik envelope decodes it.
    known: Option<String>,
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

    /// The system clipboard text the contents stand for (see the field).
    pub fn known(&self) -> Option<&str> {
        self.known.as_deref()
    }

    /// Records the system clipboard text the contents stand for.
    pub fn set_known(&mut self, text: Option<String>) {
        self.known = text;
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
