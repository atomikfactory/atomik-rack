//! Rack view code of the mix channel panel (see [`crate::mix_panel`]): the data the panel shows,
//! the meter feed, the fold state of the insert area and the two buttons.

use super::{Hit, RackView, UiAction, param_cmd};
use crate::mix_panel::{self, MixButton, MixData};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use atomik_model::{Command, DeviceKey, RackDoc, bypass_inserts_commands, inserts_bypassed};
use vello::kurbo::Point;

/// The strip's own insert bypass param: what BYPASS INSERT FX sets while there is no chain.
const STRIP_BYPASS: &str = "insert.bypass";

/// The text a control value shows in a tooltip: a fader position as the gain it stands for,
/// anything else as [`crate::widget::format_value`] words it.
pub(super) fn value_text(p: &ParamDescriptor, value: f32) -> String {
    if p.key == "fader" {
        format!("{} dB", mix_panel::fader_text(value))
    } else {
        crate::widget::format_value(p, value)
    }
}

/// The source instrument whose name the plate of channel `key` shows, with that name: the
/// console strip's rule (a second channel of one source, such as a drum pad's own channel,
/// shows its own name).
fn followed_source(doc: &RackDoc, key: &DeviceKey) -> Option<(DeviceKey, String)> {
    let channels = &doc.console.channels;
    let i = channels.iter().position(|c| &c.channel == key)?;
    let source = channels[i].source.as_ref()?;
    if channels[..i]
        .iter()
        .any(|o| o.source.as_ref() == Some(source))
    {
        return None;
    }
    let device = doc.device(source)?;
    Some((device.key.clone(), device.name.clone()))
}

impl RackView {
    // ------------------------------------------------------------------ insert area

    /// Whether the insert area of device `key` (a channel, a bus or the master) is shown: the
    /// state of its SHOW INSERT FX switch. Every device starts shown. View state: it is not
    /// saved with the project and a device that leaves the document takes it along.
    pub fn inserts_shown(&self, key: &DeviceKey) -> bool {
        !self.inserts_hidden.contains(key)
    }

    /// Shows or hides the insert area of device `key`. Returns whether anything changed.
    pub fn set_inserts_shown(&mut self, key: &DeviceKey, shown: bool) -> bool {
        let changed = if shown {
            self.inserts_hidden.remove(key)
        } else {
            self.inserts_hidden.insert(key.clone())
        };
        if changed {
            if let Some(i) = self.device_index(key) {
                self.devices[i].mix.shown = shown;
                self.devices[i].revision += 1;
            }
            // The stack under the strip comes or goes: the rack is laid out again.
            self.synced = false;
            self.repaint = true;
        }
        changed
    }

    // ------------------------------------------------------------------ panel data

    /// Re-reads what the mix channel panels show from `doc`: the insert chain's bypass state,
    /// the fold state of the insert area and the output display. A panel whose data changed is
    /// re-recorded.
    pub(super) fn refresh_mix(&mut self, doc: &RackDoc, registry: &Registry) {
        for i in 0..self.devices.len() {
            if !self.devices[i].is_mix() {
                continue;
            }
            let key = self.devices[i].key.clone();
            let data = MixData {
                follows: followed_source(doc, &key),
                bypassed: inserts_bypassed(doc, registry, &key),
                shown: self.inserts_shown(&key),
                output: self
                    .outputs
                    .get(&key)
                    .map(|o| (mix_panel::output_words(&o.text), o.warn)),
            };
            if self.devices[i].mix != data {
                self.devices[i].mix = data;
                self.devices[i].revision += 1;
                self.repaint = true;
            }
        }
    }

    /// Feeds the output peaks of a channel or bus in dB (left, right; the app passes the
    /// ballistics of the console's meters, once per frame while the rack shows). The panel is
    /// re-recorded when a bar moved by half a dB or more, and a repaint requested only while
    /// that panel is on screen, so a quiet strip settles and stops repainting. Other devices
    /// are ignored.
    pub fn set_mix_meters(&mut self, key: &DeviceKey, peaks_db: [f32; 2]) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        if !self.devices[i].is_mix() {
            return;
        }
        let shown = peaks_db.map(mix_panel::quantise_meter);
        let d = &mut self.devices[i];
        if d.meters == shown {
            return;
        }
        d.meters.clear();
        d.meters.extend(shown);
        d.bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// Whether a mix channel panel is open on the front of the rack (so its meter is worth a
    /// feed).
    pub fn shows_mix_panels(&self) -> bool {
        self.flip == super::Flip::Front && self.devices.iter().any(|d| d.is_mix() && !d.folded)
    }

    /// The centre of a button of the mix channel panel of `key` (logical view coordinates), on
    /// the front of an unfolded strip.
    pub fn mix_button_center(&self, key: &DeviceKey, b: MixButton) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        (d.is_mix() && !d.folded && self.flip == super::Flip::Front)
            .then(|| self.rack_to_view(mix_panel::geometry().button(b).center() + d.offset()))
    }

    /// The keys of the channels and buses the rack draws as mix channel panels, in rack order.
    pub fn mix_panel_keys(&self) -> Vec<DeviceKey> {
        self.devices
            .iter()
            .filter(|d| d.is_mix())
            .map(|d| d.key.clone())
            .collect()
    }

    // ------------------------------------------------------------------ buttons

    /// Whether param `key` of device `dev` is the horizontal fader of a mix channel panel.
    pub(super) fn is_mix_fader(&self, dev: usize, key: &str) -> bool {
        key == "fader" && self.devices[dev].is_mix()
    }

    /// A press on BYPASS INSERT FX or SHOW INSERT FX of panel `i`.
    ///
    /// SHOW INSERT FX flips the view state. BYPASS INSERT FX bypasses every insert effect (and
    /// the dynamics and EQ sections in use) when one plays, else restores them all, as one
    /// undo step: the commands of the console strip's own switch. A strip with nothing to
    /// bypass toggles its `insert.bypass` param, as the console does.
    pub(super) fn mix_down(
        &mut self,
        i: usize,
        b: MixButton,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.select(i, out);
        let key = self.devices[i].key.clone();
        match b {
            MixButton::ShowInserts => {
                let shown = self.inserts_shown(&key);
                self.set_inserts_shown(&key, !shown);
            }
            MixButton::BypassInserts => {
                let Some(state) = inserts_bypassed(doc, registry, &key) else {
                    let on = self.devices[i]
                        .device
                        .as_ref()
                        .and_then(|d| d.params.get(STRIP_BYPASS))
                        .is_some_and(|v| *v >= 0.5);
                    out.push(param_cmd(
                        &key,
                        STRIP_BYPASS,
                        if on { 0.0 } else { 1.0 },
                        None,
                    ));
                    return;
                };
                let mut commands = bypass_inserts_commands(doc, registry, &key, !state);
                match commands.len() {
                    0 => {}
                    1 => out.push(UiAction::Command(commands.remove(0))),
                    _ => out.push(UiAction::Command(Command::Batch(commands))),
                }
            }
        }
    }

    /// The tooltip of a mix panel button.
    pub(super) fn mix_button_help(&self, i: usize, b: MixButton) -> &'static str {
        match (b, self.devices[i].mix.bypassed) {
            (MixButton::BypassInserts, Some(true)) => {
                "The insert effects are bypassed. Click to switch them all back in"
            }
            (MixButton::BypassInserts, _) => {
                "Bypass every insert effect of this strip at once (one undo step)"
            }
            (MixButton::ShowInserts, _) => "Show or hide the insert effects of this strip",
        }
    }

    /// The hit of a mix panel button, for the tree.
    pub(super) fn mix_hits(&self, i: usize) -> Vec<(vello::kurbo::Rect, Hit)> {
        if !self.devices[i].is_mix() {
            return Vec::new();
        }
        let g = mix_panel::geometry();
        [MixButton::BypassInserts, MixButton::ShowInserts]
            .into_iter()
            .map(|b| (g.button(b), Hit::Mix(i, b)))
            .collect()
    }
}
