//! The tooltips of the rack: what the button under the pointer does. The controls and jacks
//! that already show a tooltip of their own ([`RackView::tooltip`]) are left out here.

use super::{Drag, Hit, PlayerBtn, RackView};
use crate::arp_panel::{ARP_MODES, ARP_RATES};
use crate::autolayout;
use crate::breadcrumb::{self, CrumbHit};
use crate::container_panel::ContainerButton;
use crate::drum_gen_panel::DrumButton;
use crate::plugin_panel::PluginButton;
use crate::seq_panel::SeqButton;
use crate::tooltip::Help;
use atomik_device_sdk::ControlKind;
use std::borrow::Cow;
use vello::kurbo::{Point, Rect};

fn seq_button(b: SeqButton) -> Cow<'static, str> {
    Cow::Borrowed(match b {
        SeqButton::Slot(n) => return Cow::Owned(format!("Edit pattern {}", n + 1)),
        SeqButton::Add => "Add an empty pattern",
        SeqButton::Duplicate => "Add a copy of the edited pattern",
        SeqButton::Remove => "Remove the edited pattern",
        SeqButton::Copy => "Copy the edited pattern",
        SeqButton::Paste => "Paste the copied pattern into the edited one",
        SeqButton::Clear => "Clear the steps of the edited pattern",
        SeqButton::Edit => "Open the step grid editor",
        SeqButton::Preview => "Pattern preview: double-click to open the step grid editor",
    })
}

fn drum_button(b: DrumButton) -> Option<Cow<'static, str>> {
    Some(Cow::Borrowed(match b {
        DrumButton::Slot(n) => return Some(Cow::Owned(format!("Edit pattern {}", n + 1))),
        DrumButton::Add => "Add an empty pattern",
        DrumButton::Duplicate => "Add a copy of the edited pattern",
        DrumButton::Remove => "Remove the edited pattern",
        DrumButton::Copy => "Copy the edited pattern",
        DrumButton::Paste => "Paste the copied pattern into the edited one",
        DrumButton::Clear => "Clear the steps of the edited pattern",
        DrumButton::Generate => "Write the pattern from the role templates",
        DrumButton::Randomise => "Write the pattern again from the next seed",
        DrumButton::LenDec => "One step shorter (Shift: four)",
        DrumButton::LenInc => "One step longer (Shift: four)",
        DrumButton::Bank(n) => {
            return Some(Cow::Owned(format!(
                "Patterns {} to {}",
                u32::from(n) * 8 + 1,
                u32::from(n) * 8 + 8
            )));
        }
        DrumButton::ResDec => "Previous step resolution of the edited pattern",
        DrumButton::ResInc => "Next step resolution of the edited pattern",
        DrumButton::ShufDec => "Less shuffle (Shift: finer steps)",
        DrumButton::ShufInc => "More shuffle (Shift: finer steps)",
        DrumButton::Mode => {
            "What the cells edit: velocity, flam (click or drag a hit) or ratchet (click or drag a hit)"
        }
        DrumButton::Page(n) => {
            return Some(Cow::Owned(format!(
                "Show steps {} to {}",
                u32::from(n) * 16 + 1,
                u32::from(n) * 16 + 16
            )));
        }
        DrumButton::Mute(n) => return Some(Cow::Owned(format!("Mute lane {}", n + 1))),
        DrumButton::Grid => return None,
    }))
}

fn plugin_button(b: PluginButton) -> &'static str {
    match b {
        PluginButton::Power => "Switch the plugin on or off",
        PluginButton::Bypass => "Bypass the plugin: the dry signal passes",
        PluginButton::AudioThread => "Run this plugin on the audio thread (off by default)",
        PluginButton::OpenGui => "Open the plugin's own editor window",
        PluginButton::Params => "Open the parameter pane",
        PluginButton::ToChannel => "Add a mixer channel for this instrument",
        PluginButton::Reload => "Recreate the plugin instance from its saved state",
        PluginButton::Relink => "Point this stand-in at another installed plugin",
    }
}

/// The text of a rack button, or `None` for a hit that is no button (or has its own tooltip).
fn text(hit: Hit) -> Option<Cow<'static, str>> {
    let s = |t: &'static str| Some(Cow::Borrowed(t));
    match hit {
        Hit::Name(_) => s("Double-click to rename"),
        Hit::Fold(_) => s("Fold the device to a thin strip, or unfold it"),
        Hit::Edit(_) => s("Open the editor for this device's samples"),
        Hit::ToChannel(_) => s("Add a mixer channel for this device"),
        Hit::Sample(_) => s(
            "Record from the audio input into this device (click again to stop; right-click picks the input)",
        ),
        Hit::SliceBtn(_) => s("Slice the loop at its transients (the knob sets how many)"),
        Hit::ToTrack(_) => s("Lay the slices out as notes on a new track"),
        Hit::ScButton(_) => s("Choose the sidechain source of this effect"),
        Hit::TunerButton(_) => s("Show or hide the tuner: it reads this track's input"),
        Hit::Container(_, ContainerButton::Open) => s("Enter the container to see its devices"),
        Hit::Container(_, ContainerButton::Edit) => s("Open the container editor"),
        Hit::Container(_, ContainerButton::ToChannel) => {
            s("Add a mixer channel for this container")
        }
        Hit::Plugin(_, b) => s(plugin_button(b)),
        Hit::Player(_, PlayerBtn::Seq(b)) => Some(seq_button(b)),
        Hit::Player(_, PlayerBtn::Drum(b)) => drum_button(b),
        Hit::Player(_, PlayerBtn::Mode(m)) => ARP_MODES
            .get(usize::from(m))
            .map(|n| Cow::Owned(format!("Arpeggio order: {}", n.to_lowercase()))),
        Hit::Player(_, PlayerBtn::Rate(m)) => ARP_RATES
            .get(usize::from(m))
            .map(|n| Cow::Owned(format!("Arpeggio rate: {n} note"))),
        Hit::Player(_, PlayerBtn::Step(n)) => Some(Cow::Owned(format!(
            "Arpeggio step {}: click to cycle rest, on, tie, accent",
            n + 1
        ))),
        Hit::Player(_, PlayerBtn::OctaveDec) => s("Fewer octaves"),
        Hit::Player(_, PlayerBtn::OctaveInc) => s("More octaves"),
        Hit::Player(_, PlayerBtn::FromClip) => {
            s("Copy the scale of the clip open in the piano roll into this device")
        }
        _ => None,
    }
}

impl RackView {
    /// What the button under the view position `pos` does, for its tooltip (`None` during a
    /// gesture, while a name is typed, and away from any button). A toggle or choice control
    /// says what it switches.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None)
            || self.rename.is_some()
            || !self.rack_area().contains(pos)
        {
            return None;
        }
        if !self.scope.is_empty() && self.bar_rect().contains(pos) {
            let labels = self.breadcrumb_labels();
            let (hit, rect) = breadcrumb::layout(self.bar_rect(), &labels)
                .into_iter()
                .find(|(_, r)| r.contains(pos))?;
            return Some(match hit {
                CrumbHit::Back => Help::new(rect, "Back out of the container, one level up"),
                CrumbHit::Segment(k) if k < self.scope.len() => {
                    Help::new(rect, "Go up to this level")
                }
                CrumbHit::Segment(_) => return None,
            });
        }
        let node = self.tree.hit_test(self.view_to_rack(pos))?;
        let hit = *self.tree.data(node);
        let r = self.tree.rect(node);
        let (a, b) = (
            self.rack_to_view(Point::new(r.x0, r.y0)),
            self.rack_to_view(Point::new(r.x1, r.y1)),
        );
        let rect = Rect::new(a.x, a.y, b.x, b.y);
        if let Hit::Control(i, slot) = hit {
            let (p, kind, _) = self.param_of(i, slot)?;
            if self.driven_by(i, p.key).is_some() {
                return None;
            }
            // The guitar amp explains each of its controls.
            if self.devices[i].fx == Some(crate::fx::FxKind::GuitarAmp)
                && let Some(text) = crate::fx::amp_help(p.key)
            {
                return Some(Help::new(rect, text));
            }
            if self.devices[i]
                .desc
                .is_some_and(|d| d.type_id == crate::fm_panel::FM_TYPE)
                && let Some(text) = crate::fm_panel::fm_help(p.key)
            {
                return Some(Help::new(rect, text));
            }
            if self.devices[i]
                .desc
                .is_some_and(|d| d.type_id == crate::granular_panel::GRANULAR_TYPE)
                && let Some(text) = crate::granular_panel::granular_help(p.key)
            {
                return Some(Help::new(rect, text));
            }
            let label = autolayout::control_label(p);
            return match kind {
                ControlKind::Toggle => Some(Help::new(
                    rect,
                    Cow::Owned(format!("{label}: click to switch on or off")),
                )),
                ControlKind::Choice => Some(Help::new(
                    rect,
                    Cow::Owned(format!("{label}: click to step through the choices")),
                )),
                _ => None,
            };
        }
        Some(Help::new(rect, text(hit)?))
    }
}
