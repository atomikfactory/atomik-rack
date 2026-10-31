//! An estimate of the memory an undo step holds, for [`crate::Document`]'s history bound.
//!
//! The figures are deliberately rough (a type's size plus its vectors' lengths times an element
//! size): the bound only has to keep a long session of big edits from growing without limit, and
//! the notes of a clip count in full for every step that holds the clip even though copies of a
//! clip share them, so the estimate errs on the large side.

use crate::history::Edit;
use crate::song::{Clip, ClipContent, Lane, Track};
use crate::{Cable, RackDevice};
use std::mem::size_of;

/// What a map entry, a name or a small vector of a device costs, on average.
const ENTRY_BYTES: usize = 64;
/// What one stored pattern of a step, CV or drum generator costs.
const PATTERN_BYTES: usize = 4096;

impl Edit {
    /// The estimated bytes this step keeps alive (including the edit itself).
    pub(crate) fn approx_bytes(&self) -> usize {
        size_of::<Edit>()
            + match self {
                Edit::Insert { device, cables, .. }
                | Edit::Remove { device, cables, .. }
                | Edit::InnerInsert { device, cables, .. }
                | Edit::InnerRemove { device, cables, .. } => {
                    device_bytes(device) + cables.len() * size_of::<Cable>()
                }
                Edit::Layout { old, new } => (old.len() + new.len()) * ENTRY_BYTES,
                Edit::Track { old, new, .. } => track_bytes(old) + track_bytes(new),
                Edit::Lane { old, new, .. } => lane_bytes(old) + lane_bytes(new),
                Edit::Clip { old, new, .. } => clip_bytes(old.as_ref()) + clip_bytes(new.as_ref()),
                Edit::TakeGroups { old, new, .. } => old
                    .iter()
                    .chain(new)
                    .map(|g| {
                        size_of::<crate::TakeGroup>()
                            + g.takes.iter().map(|c| clip_bytes(Some(c))).sum::<usize>()
                    })
                    .sum(),
                Edit::TempoMap { old, new } => {
                    (old.len() + new.len()) * size_of::<crate::TempoEvent>()
                }
                Edit::MeterMap { old, new } => {
                    (old.len() + new.len()) * size_of::<crate::MeterEvent>()
                }
                Edit::Markers { old, new } => (old.len() + new.len()) * size_of::<crate::Marker>(),
                Edit::Ranges { old, new } => {
                    (old.len() + new.len()) * size_of::<crate::RangeMarker>()
                }
                Edit::Grooves { old, new } => (old.len() + new.len()) * PATTERN_BYTES,
                Edit::TrackHeights { old, new } => (old.len() + new.len()) * ENTRY_BYTES,
                Edit::Organise { old, new } => {
                    (old.order.len() + new.order.len()) * ENTRY_BYTES
                        + (old.org.members.len() + new.org.members.len()) * ENTRY_BYTES
                }
                Edit::Macros { old, new } => {
                    (old.assigns.len() + new.assigns.len() + old.names.len() + new.names.len())
                        * ENTRY_BYTES
                }
                Edit::MidiMaps { old, new } => (old.len() + new.len()) * ENTRY_BYTES,
                Edit::ConsoleVcas { old, new } => (old.len() + new.len()) * ENTRY_BYTES,
                Edit::ConsoleSnapshots { old, new } => (old.len() + new.len()) * PATTERN_BYTES,
                Edit::Zones { old, new, .. } => {
                    (old.len() + new.len()) * size_of::<crate::zones::ZoneDoc>()
                }
                Edit::Steps { old, new, .. } => {
                    (old.as_ref().map_or(0, |d| d.patterns.len())
                        + new.as_ref().map_or(0, |d| d.patterns.len()))
                        * PATTERN_BYTES
                }
                Edit::Curves { old, new, .. } => {
                    (old.as_ref().map_or(0, |d| d.patterns.len())
                        + new.as_ref().map_or(0, |d| d.patterns.len()))
                        * PATTERN_BYTES
                }
                Edit::Drums { old, new, .. } => {
                    (old.as_ref().map_or(0, |d| d.patterns.len())
                        + new.as_ref().map_or(0, |d| d.patterns.len()))
                        * PATTERN_BYTES
                }
                Edit::Splits { old, new, .. } => (old.len() + new.len()) * ENTRY_BYTES,
                Edit::Macro { old, new, .. } => {
                    (old.targets.len() + new.targets.len()) * ENTRY_BYTES
                }
                Edit::PluginState { old, new, .. } => {
                    old.as_ref().map_or(0, |b| b.0.len()) + new.as_ref().map_or(0, |b| b.0.len())
                }
                Edit::Compound(edits) => edits.iter().map(Edit::approx_bytes).sum(),
                _ => 0,
            }
    }
}

fn device_bytes(d: &RackDevice) -> usize {
    size_of::<RackDevice>()
        + d.params.len() * ENTRY_BYTES
        + d.zones.len() * size_of::<crate::zones::ZoneDoc>()
        + d.steps.as_ref().map_or(0, |s| s.patterns.len()) * PATTERN_BYTES
        + d.curves.as_ref().map_or(0, |c| c.patterns.len()) * PATTERN_BYTES
        + d.drums.as_ref().map_or(0, |c| c.patterns.len()) * PATTERN_BYTES
        + d.inner.as_ref().map_or(0, |c| {
            c.devices.iter().map(device_bytes).sum::<usize>()
                + c.cables.len() * size_of::<Cable>()
                + c.splits.len() * ENTRY_BYTES
        })
        + d.plugin
            .as_ref()
            .and_then(|p| p.state.as_ref())
            .map_or(0, |b| b.0.len())
}

fn clip_bytes(clip: Option<&Clip>) -> usize {
    let Some(c) = clip else { return 0 };
    size_of::<Clip>()
        + c.controllers
            .iter()
            .map(|l| l.points.len() * size_of::<crate::AutomationPoint>())
            .sum::<usize>()
        + match &c.content {
            ClipContent::Notes(n) => n.len() * size_of::<crate::Note>(),
            ClipContent::Automation(p) => p.len() * size_of::<crate::AutomationPoint>(),
            ClipContent::Audio(a) => {
                a.envelope.len() * size_of::<crate::GainPoint>() + a.asset.len()
            }
        }
}

fn lane_size(l: &Lane) -> usize {
    size_of::<Lane>()
        + l.clips.iter().map(|c| clip_bytes(Some(c))).sum::<usize>()
        + l.take_groups
            .iter()
            .flat_map(|g| &g.takes)
            .map(|c| clip_bytes(Some(c)))
            .sum::<usize>()
}

fn lane_bytes(lane: &Option<Lane>) -> usize {
    lane.as_ref().map_or(0, lane_size)
}

fn track_bytes(track: &Option<Track>) -> usize {
    track.as_ref().map_or(0, |t| {
        size_of::<Track>() + t.lanes.iter().map(lane_size).sum::<usize>()
    })
}
