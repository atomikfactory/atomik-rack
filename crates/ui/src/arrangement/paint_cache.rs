//! The recorded layers of the arrangement: the lanes with their clips and the track list are
//! drawn into display lists that are kept between frames, so a frame in which only the playhead
//! (or a track meter) moved replays them instead of walking every clip again, and a GPU backend
//! reuses the encoding of the very same list.
//!
//! * **What invalidates.** Anything that sets the view's `repaint` flag (every edit, scroll,
//!   zoom, selection, hover, tool, drag, theme, scale...) re-records both layers. On top of that
//!   a layer is stamped with the view rectangles, the scale and the document revision it was
//!   recorded for, so a paint at another size or revision never replays an older recording.
//! * **What does not.** A playhead that moved a pixel or a meter that changed only raises its
//!   own flag: the playhead line, the ruler flag, the marker strip and the tempo lane are drawn
//!   live over the cached lanes, and a meter re-records the track list alone.
//!
//! The op stream a paint produces is the same with and without the cache (the cached lists are
//! flattened into a recording), so the display-list snapshots do not tell them apart.

use crate::display_list::DisplayList;
use std::sync::atomic::{AtomicU64, Ordering};
use vello::kurbo::Rect;

/// Cache id of the lanes layer (the list ids of the arrangement are in a range of their own).
const LANES_ID: u64 = 0x00A4_0000_0000_0001;
/// Cache id of the track list layer.
const HEADERS_ID: u64 = 0x00A4_0000_0000_0002;

/// Revisions are unique across every view, so two views sharing an id never replay each other's
/// encoding.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// What a layer was recorded for besides the view's repaint flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    bounds: [u64; 4],
    outer: [u64; 4],
    scale: u64,
    doc_rev: Option<u64>,
}

impl Stamp {
    pub(super) fn new(bounds: Rect, outer: Rect, scale: f64, doc_rev: Option<u64>) -> Self {
        let bits = |r: Rect| [r.x0, r.y0, r.x1, r.y1].map(f64::to_bits);
        Self {
            bounds: bits(bounds),
            outer: bits(outer),
            scale: scale.to_bits(),
            doc_rev,
        }
    }
}

/// One cached layer.
#[derive(Debug, Default)]
struct Layer {
    list: Option<DisplayList>,
    stamp: Option<Stamp>,
    /// How many times it was recorded.
    records: u64,
}

impl Layer {
    fn fresh(&self, stamp: Stamp, dirty: bool) -> bool {
        !dirty && self.list.is_some() && self.stamp == Some(stamp)
    }

    fn store(&mut self, id: u64, stamp: Stamp, record: impl FnOnce(&mut DisplayList)) {
        let mut list = DisplayList::cached(id, next_revision());
        record(&mut list);
        self.list = Some(list);
        self.stamp = Some(stamp);
        self.records += 1;
    }
}

/// The two cached layers of an arrangement view.
#[derive(Debug, Default)]
pub(super) struct PaintCache {
    lanes: Layer,
    headers: Layer,
    /// Records every layer on every paint (for the cached-equals-uncached tests).
    pub(super) off: bool,
}

impl PaintCache {
    /// The lanes layer: re-recorded with `record` unless it is still current.
    pub(super) fn lanes(
        &mut self,
        stamp: Stamp,
        dirty: bool,
        record: impl FnOnce(&mut DisplayList),
    ) -> &DisplayList {
        let dirty = dirty || self.off;
        if !self.lanes.fresh(stamp, dirty) {
            self.lanes.store(LANES_ID, stamp, record);
        }
        self.lanes.list.get_or_insert_with(DisplayList::new)
    }

    /// The track list layer.
    pub(super) fn headers(
        &mut self,
        stamp: Stamp,
        dirty: bool,
        record: impl FnOnce(&mut DisplayList),
    ) -> &DisplayList {
        let dirty = dirty || self.off;
        if !self.headers.fresh(stamp, dirty) {
            self.headers.store(HEADERS_ID, stamp, record);
        }
        self.headers.list.get_or_insert_with(DisplayList::new)
    }

    /// How many times the lanes and the track list were recorded.
    pub(super) fn records(&self) -> (u64, u64) {
        (self.lanes.records, self.headers.records)
    }
}
