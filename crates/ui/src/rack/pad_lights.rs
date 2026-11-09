//! The pad lights of a drum machine panel: a pad flashes when the engine played it, whatever sent
//! the note (a generator, the sequencer, a MIDI input, a click on the pad).
//!
//! The drum machine publishes its meter values ([`atomik_device_sdk::Device::meter_values`]): a
//! hit counter for each of its 16 pads, then the velocity of each pad's last hit. The app feeds
//! them once per frame ([`RackView::set_device_meter_values`]); a counter that moved lights its
//! pad at once (louder hits light it brighter) and the light fades over [`FLASH_SECONDS`] on the
//! clock of [`RackView::tick`]. Nothing here runs on the audio thread: the device only counts.
//!
//! The shown `meters` of a drum machine view hold the 16 light levels (0 to 1), then the 16
//! counters last seen.

use super::RackView;
use crate::autolayout::DRUM_METERS;
use vello::kurbo::Point;

/// Pads of a drum machine.
const PADS: usize = DRUM_METERS / 2;
/// How long a light takes to fade out, in seconds.
pub const FLASH_SECONDS: f64 = 0.25;
/// The level a hit at the softest velocity lights its pad at (a hit at full velocity lights 1).
const FLASH_FLOOR: f32 = 0.55;
/// The longest step one tick fades by, so a stalled frame does not skip the light.
const MAX_FADE_STEP: f64 = 0.1;

impl RackView {
    /// Takes the values a drum machine published: the pads whose counter moved light up.
    pub(super) fn drum_meter_feed(&mut self, i: usize, values: &[f32]) {
        if values.len() < DRUM_METERS {
            return;
        }
        let shown = &mut self.devices[i].meters;
        if shown.len() != DRUM_METERS {
            // The first feed only learns the counters: nothing played while it was away.
            shown.clear();
            shown.resize(PADS, 0.0);
            shown.extend_from_slice(&values[..PADS]);
            return;
        }
        let mut lit = false;
        for pad in 0..PADS {
            if shown[PADS + pad].to_bits() != values[pad].to_bits() {
                shown[PADS + pad] = values[pad];
                let velocity = values[PADS + pad].clamp(0.0, 1.0);
                shown[pad] = FLASH_FLOOR + (1.0 - FLASH_FLOOR) * velocity;
                lit = true;
            }
        }
        if lit {
            self.lights_changed(i);
        }
    }

    /// Fades the pad lights on to `now_seconds` (any monotonic clock; the first call after the
    /// lights went out only starts the clock). Returns whether a light changed, which has
    /// already asked for a repaint.
    pub(super) fn pad_lights_tick(&mut self, now_seconds: f64) -> bool {
        if !self.pad_lights_active() {
            self.lights_clock = None;
            return false;
        }
        let Some(last) = self.lights_clock.replace(now_seconds) else {
            return false;
        };
        let step = ((now_seconds - last).clamp(0.0, MAX_FADE_STEP) / FLASH_SECONDS) as f32;
        let mut changed = false;
        for i in 0..self.devices.len() {
            if self.devices[i].spec.drum.is_none() || self.devices[i].meters.len() != DRUM_METERS {
                continue;
            }
            let mut moved = false;
            for level in &mut self.devices[i].meters[..PADS] {
                if *level > 0.0 {
                    *level = (*level - step).max(0.0);
                    moved = true;
                }
            }
            if moved {
                self.lights_changed(i);
                changed = true;
            }
        }
        changed
    }

    /// Whether any pad light is on (the host keeps calling [`Self::tick`] while one is).
    pub fn pad_lights_active(&self) -> bool {
        self.devices.iter().any(|d| {
            d.spec.drum.is_some()
                && d.meters.len() == DRUM_METERS
                && d.meters[..PADS].iter().any(|l| *l > 0.0)
        })
    }

    /// How brightly pad `pad` (1 to 16) of drum machine `key` is lit, 0 to 1.
    pub fn pad_light(&self, key: &atomik_model::DeviceKey, pad: u8) -> f32 {
        let Some(i) = self.device_index(key) else {
            return 0.0;
        };
        let d = &self.devices[i];
        if d.meters.len() != DRUM_METERS || !(1..=PADS as u8).contains(&pad) {
            return 0.0;
        }
        d.meters[usize::from(pad) - 1]
    }

    /// The panel of device `i` changed what it draws: record it again, and repaint when it is on
    /// screen.
    fn lights_changed(&mut self, i: usize) {
        self.devices[i].bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }
}
