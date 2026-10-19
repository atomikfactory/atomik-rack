//! Sampler fixtures: a rack with one sampler and three assets (the third has no audio),
//! layered zones, and a synced sample editor over it.

use super::audio::pyramid;
use super::*;
use atomik_device_sdk::{LoopMode, TriggerMode};
use atomik_model::{AssetMeta, Command, Document, RackDoc, ZoneDoc};
use atomik_ui::kurbo::Rect;
use atomik_ui::{SampleEditorView, Theme, UiAction};

pub const S_A: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
pub const S_B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
pub const S_C: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";
pub const FR_A: u64 = 96_000;
pub const FR_B: u64 = 48_000;
pub const FR_C: u64 = 24_000;
pub const EDITOR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);

fn meta(frames: u64, channels: u16, name: &str) -> AssetMeta {
    AssetMeta {
        channels,
        sample_rate: 48_000,
        frames,
        original_name: name.to_string(),
    }
}

/// The reference rack plus an empty sampler `sampler` and three assets in the pool.
pub fn sampler_doc(zones: Vec<ZoneDoc>) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("sampler", "atomik.sampler", "Sampler");
    d.zones = zones;
    doc.devices.insert(0, d);
    doc.assets.insert(S_A.into(), meta(FR_A, 2, "piano_C3.wav"));
    doc.assets
        .insert(S_B.into(), meta(FR_B, 1, "strings_C5.flac"));
    doc.assets.insert(S_C.into(), meta(FR_C, 1, "lost.wav"));
    doc
}

fn zone(asset: &str, frames: u64, keys: (u8, u8), vel: (u8, u8), root: u8) -> ZoneDoc {
    let mut z = ZoneDoc::new(asset, frames);
    (z.key_lo, z.key_hi) = keys;
    (z.vel_lo, z.vel_hi) = vel;
    z.root_key = root;
    z
}

/// Four layered zones: two overlapping on asset A (a velocity split), one on B and one on the
/// missing asset C.
pub fn layered_zones() -> Vec<ZoneDoc> {
    vec![
        zone(S_A, FR_A, (0, 59), (1, 127), 48),
        zone(S_A, FR_A, (48, 71), (1, 63), 60),
        zone(S_B, FR_B, (60, 95), (64, 127), 72),
        zone(S_C, FR_C, (96, 127), (1, 127), 108),
    ]
}

/// One forward-looped zone with a crossfade.
pub fn looped_zone() -> ZoneDoc {
    let mut z = zone(S_A, FR_A, (36, 84), (1, 127), 60);
    z.start = 4_800;
    z.end = 91_200;
    z.loop_mode = LoopMode::Forward;
    z.loop_start = 24_000;
    z.loop_end = 72_000;
    z.crossfade = 4_800;
    z.trigger = TriggerMode::Gate;
    z
}

/// An editor open on `sampler`, with peaks for A and B and C reported missing.
pub fn editor(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> SampleEditorView {
    let mut v = SampleEditorView::new(Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    v.open(key("sampler"));
    v.set_peaks(S_A, pyramid(FR_A, 2));
    v.set_peaks(S_B, pyramid(FR_B, 1));
    v.set_missing_assets([S_C.to_string()]);
    v.sync(doc, reg);
    v
}

/// Applies every `SetZones` among `actions` to `document`, asserting the model accepts it,
/// and returns the merge ids seen.
pub fn apply_zones(
    document: &mut Document,
    reg: &atomik_device_sdk::Registry,
    actions: &[UiAction],
) -> Vec<Option<u64>> {
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(c @ Command::SetZones { merge, .. }) => {
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

/// Every zone list among `actions`.
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

/// The `(key, velocity, on)` triples of the audition actions among `actions`.
pub fn auditions(actions: &[UiAction]) -> Vec<(u8, u8, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => {
                assert_eq!(device, &super::key("sampler"));
                Some((*key, *velocity, *on))
            }
            _ => None,
        })
        .collect()
}
