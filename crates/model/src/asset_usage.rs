//! Where the sample pool is used: the counts behind the Song Samples list of the file browser.
//!
//! [`RackDoc::asset_usage`] walks the same places [`RackDoc::referenced_assets`] does (clips on
//! every lane, the takes of take groups, the render of a frozen track, the zones of every device
//! including those inside containers) but counts instead of collecting. An asset nothing uses is
//! *unassigned*: it is in the pool (a recording that was undone, a sample taken off a pad) until
//! a save prunes it.

use crate::{ClipContent, Document, RackDevice, RackDoc};
use std::collections::{BTreeMap, BTreeSet};

/// What uses one pool asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetUse {
    /// Audio clips of the song (muted or not, take groups' compiled clips included).
    pub clips: usize,
    /// Takes stacked in take groups.
    pub takes: usize,
    /// Zones of samplers, drum pads, slicers and granular samplers.
    pub zones: usize,
    /// Tracks whose frozen render this is.
    pub frozen: usize,
}

impl AssetUse {
    /// How many places use the asset.
    pub fn total(&self) -> usize {
        self.clips + self.takes + self.zones + self.frozen
    }

    /// True when the song or a device uses the asset.
    pub fn assigned(&self) -> bool {
        self.total() > 0
    }
}

/// Why [`Document::remove_asset`] kept an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssetRemoval {
    /// The pool has no such asset.
    #[error("the sample is not in the pool")]
    Unknown,
    /// A clip, a zone or a frozen render uses it.
    #[error("the sample is in use")]
    InUse,
    /// Nothing uses it, but an undo or redo step does.
    #[error("the sample is kept for undo")]
    History,
}

fn count_zones(device: &RackDevice, out: &mut BTreeMap<String, AssetUse>) {
    for zone in &device.zones {
        out.entry(zone.asset.clone()).or_default().zones += 1;
    }
    if let Some(inner) = &device.inner {
        for d in &inner.devices {
            count_zones(d, out);
        }
    }
}

impl RackDoc {
    /// The pool assets and what uses them, by asset id: every pool asset has an entry (all zero
    /// when unused) and an id the song or a zone names that the pool lacks (a missing file) has
    /// one too, so the list shows what a project needs.
    pub fn asset_usage(&self) -> BTreeMap<String, AssetUse> {
        let mut out: BTreeMap<String, AssetUse> = self
            .assets
            .keys()
            .map(|h| (h.clone(), AssetUse::default()))
            .collect();
        for track in &self.song.tracks {
            for lane in &track.lanes {
                for clip in &lane.clips {
                    if let ClipContent::Audio(a) = &clip.content {
                        out.entry(a.asset.clone()).or_default().clips += 1;
                    }
                }
                for group in &lane.take_groups {
                    for take in &group.takes {
                        if let ClipContent::Audio(a) = &take.content {
                            out.entry(a.asset.clone()).or_default().takes += 1;
                        }
                    }
                }
            }
            if let Some(f) = &track.frozen {
                out.entry(f.audio.asset.clone()).or_default().frozen += 1;
            }
        }
        for d in &self.devices {
            count_zones(d, &mut out);
        }
        out
    }
}

impl Document {
    /// Takes `hash` out of the sample pool when nothing needs it: refused while the song or a
    /// device uses it ([`AssetRemoval::InUse`]) and while an undo or redo step does
    /// ([`AssetRemoval::History`], so undo never meets a missing sample). Not an undo step; the
    /// file in the bundle goes at the next save.
    pub fn remove_asset(&mut self, hash: &str) -> Result<(), AssetRemoval> {
        if !self.doc.assets.contains_key(hash) {
            return Err(AssetRemoval::Unknown);
        }
        let mut now = BTreeSet::new();
        now.extend(self.doc.referenced_assets());
        if now.contains(hash) {
            return Err(AssetRemoval::InUse);
        }
        if self.referenced_assets().contains(hash) {
            return Err(AssetRemoval::History);
        }
        self.doc.assets.remove(hash);
        self.revision = crate::next_revision();
        Ok(())
    }
}
