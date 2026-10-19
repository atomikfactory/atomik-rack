//! Loop slicer fixtures: a rack with one loop slicer `slicer` over a 2 s stereo loop (four
//! equal slices, the second one reversed and the third 3 dB down), and a synced slice editor.

use super::audio::pyramid;
use super::*;
use atomik_model::{AssetMeta, Command, Document, RackDoc, ZoneDoc, slices_from_markers};
use atomik_ui::kurbo::Rect;
use atomik_ui::{SliceEditorView, Theme, UiAction};

pub const LOOP: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
pub const LOOP_FRAMES: u64 = 96_000;
pub const SLICER_TYPE: &str = "atomik.loop-slicer";
pub const EDITOR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);
/// 1 ms at 48 kHz.
pub const MIN_LEN: u64 = 48;

/// The four default markers.
pub const MARKERS: [u64; 4] = [0, 24_000, 48_000, 72_000];

/// Slice zones over the loop for `markers`: slice 2 reversed, slice 3 at -3 dB.
pub fn sliced_zones(markers: &[u64]) -> Vec<ZoneDoc> {
    let mut zones = slices_from_markers(LOOP, LOOP_FRAMES, markers, &[], MIN_LEN);
    if let Some(z) = zones.get_mut(1) {
        z.reverse = true;
    }
    if let Some(z) = zones.get_mut(2) {
        z.gain_db = -3.0;
    }
    zones
}

/// The reference rack plus a loop slicer `slicer` with `zones` (and the loop in the pool).
pub fn slicer_doc(zones: Vec<ZoneDoc>) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("slicer", SLICER_TYPE, "Loop Slicer");
    d.zones = zones;
    doc.devices.insert(0, d);
    doc.assets.insert(
        LOOP.into(),
        AssetMeta {
            channels: 2,
            sample_rate: 48_000,
            frames: LOOP_FRAMES,
            original_name: "amen_loop.wav".into(),
        },
    );
    doc
}

/// An editor open on `slicer` with the loop's peaks.
pub fn editor(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> SliceEditorView {
    let mut v = SliceEditorView::new(Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    v.open(key("slicer"));
    v.set_peaks(LOOP, pyramid(LOOP_FRAMES, 2));
    v.sync(doc, reg);
    v
}

/// Applies every `SetZones` and `SetParam` among `actions` to `document`, asserting the model
/// accepts it, and returns the merge ids seen.
pub fn apply_all(
    document: &mut Document,
    reg: &atomik_device_sdk::Registry,
    actions: &[UiAction],
) -> Vec<Option<u64>> {
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(
                c @ (Command::SetZones { merge, .. } | Command::SetParam { merge, .. }),
            ) => {
                merges.push(*merge);
                document
                    .apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    merges
}

/// Every zone list among `actions` with its merge id.
pub fn zone_lists(actions: &[UiAction]) -> Vec<(Vec<ZoneDoc>, Option<u64>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::SetZones { zones, merge, .. }) => {
                Some((zones.clone(), *merge))
            }
            _ => None,
        })
        .collect()
}
