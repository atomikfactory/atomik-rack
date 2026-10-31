//! Rack view code of the Audio I/O panel: the host channel counts
//! that drive the lamp colours, the per-frame peak feed, the clip lamps and their reset.

use super::{Flip, Hit, RackView};
use crate::interface::INTERFACE_TYPE_ID;
use crate::io_panel::{self, IoSocketState};
use vello::kurbo::Point;

impl RackView {
    /// The index of the Audio I/O panel, when the view shows it (not inside a container).
    fn io_index(&self) -> Option<usize> {
        self.devices
            .first()
            .filter(|d| d.desc.is_some_and(|desc| desc.type_id == INTERFACE_TYPE_ID))
            .map(|_| 0)
    }

    /// Re-records the Audio I/O panel and asks for a repaint when it is on screen.
    fn io_changed(&mut self) {
        let Some(i) = self.io_index() else { return };
        self.devices[i].revision += 1;
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == Flip::Front && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// Tells the view how many channels the host audio device has: `Some((inputs, outputs))`
    /// for the active stream, `None` for the offline sink (every channel exists). A cabled
    /// socket beyond those counts shows the "unavailable" colour. Cheap when nothing
    /// changed.
    pub fn set_io_host_channels(&mut self, host: Option<(u16, u16)>) {
        if self.io.host == host {
            return;
        }
        self.io.host = host;
        self.refresh_io();
    }

    /// Recomputes the lamp states from the document's cables and the host channel counts.
    pub(super) fn refresh_io(&mut self) {
        let now = io_panel::states_of(
            self.input_channels,
            self.output_channels,
            &self.cables,
            self.io.host,
        );
        if now != self.io.states {
            self.io.states = now;
            self.io_changed();
        }
    }

    /// Feeds the linear peak of every Audio I/O socket since the last call (descriptor order:
    /// host inputs, then host outputs), once per frame while the rack shows.
    /// The view turns them into displayed
    /// levels and latched clip flags ([`io_panel::update_levels`]), re-records the panel only
    /// when something changed, and repaints only when it is on screen. Allocation-free after
    /// the first call.
    pub fn set_io_peaks(&mut self, peaks: &[f32]) {
        self.big_meter_feed(peaks);
        if self.io_index().is_none() {
            return;
        }
        let n = self.io.states.len();
        let changed = io_panel::update_levels(peaks, n, &mut self.io.levels, &mut self.io.clips);
        if changed {
            self.io_changed();
        }
    }

    /// How many sockets the Audio I/O panel has (inputs, then outputs).
    pub fn io_socket_count(&self) -> usize {
        self.io.states.len()
    }

    /// The lamp state of every Audio I/O socket (descriptor order).
    pub fn io_states(&self) -> &[IoSocketState] {
        &self.io.states
    }

    /// The displayed level of every Audio I/O socket, dB.
    pub fn io_levels(&self) -> &[f32] {
        &self.io.levels
    }

    /// The latched clip flag of every Audio I/O socket.
    pub fn io_clips(&self) -> &[bool] {
        &self.io.clips
    }

    /// Clears one clip lamp (`Some(socket)`) or all of them (`None`).
    pub fn reset_io_clips(&mut self, socket: Option<usize>) {
        let mut changed = false;
        for (i, clip) in self.io.clips.iter_mut().enumerate() {
            if socket.is_none_or(|s| s == i) && *clip {
                *clip = false;
                changed = true;
            }
        }
        if changed {
            self.io_changed();
        }
    }

    /// Centre of the clip lamp of Audio I/O socket `socket` (logical view coordinates), front
    /// side only.
    pub fn io_clip_center(&self, socket: usize) -> Option<Point> {
        let i = self.io_index()?;
        if self.flip != Flip::Front {
            return None;
        }
        let cell = io_panel::cells(self.devices[i].desc.as_ref()?)
            .into_iter()
            .find(|c| c.socket == socket)?;
        Some(self.rack_to_view(cell.clip.center() + self.devices[i].offset()))
    }

    /// Centre of the Audio I/O panel's name plate (logical view coordinates).
    pub fn io_header_center(&self) -> Option<Point> {
        let i = self.io_index()?;
        let d = &self.devices[i];
        let r = crate::paint::plate_rect(d.height, d.folded);
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// A press on the clip lamp of `socket` clears it, a press on the plate clears them all.
    pub(super) fn io_down(&mut self, hit: Hit) {
        match hit {
            Hit::IoClip(_, socket) => self.reset_io_clips(Some(socket)),
            Hit::IoHeader(_) => self.reset_io_clips(None),
            _ => {}
        }
    }

    /// The tooltip of an Audio I/O socket cell or of its plate.
    pub(super) fn io_tooltip(&self, hit: Hit) -> Option<String> {
        match hit {
            Hit::IoHeader(_) => Some("Click to reset every clip lamp".to_string()),
            Hit::IoCell(i, socket) | Hit::IoClip(i, socket) => {
                let desc = self.devices.get(i)?.desc?;
                let name = desc.sockets.get(socket)?.name;
                let state = self.io.states.get(socket).copied().unwrap_or_default();
                let clipped = self.io.clips.get(socket).copied().unwrap_or(false);
                Some(if clipped {
                    format!(
                        "{name}: {}. It clipped: click the lamp to reset",
                        state.word()
                    )
                } else {
                    format!("{name}: {}", state.word())
                })
            }
            _ => None,
        }
    }
}
