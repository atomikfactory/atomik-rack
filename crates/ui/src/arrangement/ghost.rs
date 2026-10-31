//! The recording in progress: a translucent red clip that grows from where the take started to
//! the playhead on each armed track's lane, with a tick for every note captured so far. It is
//! only drawn: the app owns the take, nothing here is saved or undoable, and the real clip
//! replaces it when recording stops.

use super::*;

/// A take running on one lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeGhost {
    /// The track's device.
    pub device: DeviceKey,
    /// The lane the take records to.
    pub lane: usize,
    /// The song tick the take started at.
    pub start: i64,
    /// The notes so far as `(tick, length, key)`, held ones ending at the playhead (none for an
    /// audio take, which shows its span only).
    pub notes: Vec<(i64, i64, u8)>,
}

impl ArrangementView {
    /// Shows the takes in progress (empty: none).
    pub fn set_take_ghosts(&mut self, ghosts: Vec<TakeGhost>) {
        if ghosts != self.ghosts {
            self.ghosts = ghosts;
            self.repaint = true;
        }
    }

    /// The takes shown.
    pub fn take_ghosts(&self) -> &[TakeGhost] {
        &self.ghosts
    }

    /// The rectangle a take's ghost fills, from its start to the playhead (logical pixels).
    pub fn ghost_rect(&self, ghost: &TakeGhost) -> Option<Rect> {
        let lane = self.lane_rect(&ghost.device, ghost.lane)?;
        let x0 = self.axis.tick_to_x(ghost.start);
        let x1 = self.axis.tick_to_x(self.playhead.max(ghost.start));
        Some(Rect::new(
            x0,
            lane.y0 + 3.0,
            x1.max(x0 + 1.0),
            lane.y1 - 3.0,
        ))
    }

    pub(super) fn paint_ghosts(&self, c: &mut dyn Canvas) {
        let red = self.st.record;
        for g in &self.ghosts {
            let Some(r) = self.ghost_rect(g) else {
                continue;
            };
            c.fill_with(RoundedRect::from_rect(r, 2.0), red.with_alpha(0.22));
            c.stroke(
                &RoundedRect::from_rect(r, 2.0).into(),
                &Stroke::new(1.0),
                &red.with_alpha(0.8).into(),
            );
            let (lo, hi) = g
                .notes
                .iter()
                .fold((u8::MAX, 0u8), |(lo, hi), n| (lo.min(n.2), hi.max(n.2)));
            let span = f64::from(hi.saturating_sub(lo)).max(11.0);
            let inner = r.inset(-2.0);
            for &(tick, length, key) in &g.notes {
                let x0 = self.axis.tick_to_x(tick).max(inner.x0);
                let x1 = self
                    .axis
                    .tick_to_x(tick + length)
                    .clamp(x0 + 1.0, inner.x1.max(x0 + 1.0));
                let y = inner.y1 - (f64::from(key.saturating_sub(lo)) / span) * inner.height();
                c.fill_with(Rect::new(x0, y - 1.0, x1, y + 1.0), red);
            }
        }
    }
}
