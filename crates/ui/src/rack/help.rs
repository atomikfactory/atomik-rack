//! The tooltips of the rack: what the button under the pointer does. The controls and jacks
//! that already show a tooltip of their own ([`RackView::tooltip`]) are left out here.

use super::{Drag, Hit, PlayerBtn, RackView};
use crate::arp_panel::{ARP_MODES, ARP_RATES};
use crate::autolayout;
use crate::breadcrumb::{self, CrumbHit};
use crate::container_panel::ContainerButton;
use crate::drum_gen_panel::DrumButton;
use crate::mix_panel::MixButton;
use crate::plugin_panel::PluginButton;
use crate::resonance_marks::ResonanceBtn;
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
        SeqButton::Copy => "Copy the edited pattern, with its curves on a CV sequencer",
        SeqButton::Paste => "Paste the copied pattern, and its curves, into the edited one",
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
        DrumButton::Generate => "Write a new rhythm from the style (a new seed every press)",
        DrumButton::Randomise => {
            "Draw style, density, swing and a new seed, then write the pattern"
        }
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
        PluginButton::Mpe => {
            "Send the notes as MPE on channels 2 to 16 to a plugin that takes MIDI"
        }
        PluginButton::Cv(_) => {
            "The parameter this CV input drives: click to choose it in the parameter pane"
        }
    }
}

/// The stable id of a rack button (the key of its long help in Help Mode).
fn id_of(hit: Hit) -> Option<&'static str> {
    Some(match hit {
        Hit::Name(_) => "rack.name",
        Hit::Fold(_) => "rack.fold",
        Hit::Edit(_) => "rack.edit_samples",
        Hit::ToChannel(_) => "rack.to_channel",
        Hit::Sample(_) => "rack.sample",
        Hit::SliceBtn(_) => "rack.slice",
        Hit::ToTrack(_) => "rack.slices_to_track",
        Hit::ScButton(_) => "rack.sidechain",
        Hit::Resonance(_, ResonanceBtn::Find) => "rack.resonance_find",
        Hit::Resonance(_, ResonanceBtn::Cut) => "rack.resonance_cut",
        Hit::DuckLearn(_) => "rack.duck_learn",
        Hit::SpecRef(_) => "rack.spectrum_ref",
        Hit::TableEdit(_) => "rack.table_edit",
        Hit::DuckPlot(_) => "rack.duck_plot",
        Hit::TunerButton(_) => "rack.tuner",
        Hit::Preview(_) => "rack.preview_player",
        Hit::Container(_, ContainerButton::Fold) => "rack.container_fold",
        Hit::Container(_, ContainerButton::Open) => "rack.container_open",
        Hit::Container(_, ContainerButton::Edit) => "rack.container_edit",
        Hit::Container(_, ContainerButton::ToChannel) => "rack.to_channel",
        Hit::Plugin(_, b) => match b {
            PluginButton::Power => "rack.plugin_power",
            PluginButton::Bypass => "rack.plugin_bypass",
            PluginButton::AudioThread => "rack.plugin_audio_thread",
            PluginButton::OpenGui => "rack.plugin_gui",
            PluginButton::Params => "rack.plugin_params",
            PluginButton::ToChannel => "rack.to_channel",
            PluginButton::Reload => "rack.plugin_reload",
            PluginButton::Relink => "rack.plugin_relink",
            PluginButton::Mpe => "rack.plugin_mpe",
            PluginButton::Cv(_) => "rack.plugin_cv",
        },
        Hit::Player(_, PlayerBtn::Seq(_) | PlayerBtn::Drum(_)) => "rack.pattern",
        Hit::Player(_, PlayerBtn::Mode(_) | PlayerBtn::Rate(_) | PlayerBtn::Step(_)) => {
            "rack.arpeggio"
        }
        Hit::Player(_, PlayerBtn::OctaveDec | PlayerBtn::OctaveInc) => "rack.arpeggio",
        Hit::Player(_, PlayerBtn::FromClip) => "rack.scale_from_clip",
        Hit::Mix(_, MixButton::BypassInserts) => "rack.bypass_inserts",
        Hit::Mix(_, MixButton::ShowInserts) => "rack.show_inserts",
        _ => return None,
    })
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
        Hit::Resonance(_, ResonanceBtn::Find) => s(
            "Listen to the range between the locators at this device's input and mark its resonances",
        ),
        Hit::Resonance(_, ResonanceBtn::Cut) => s(
            "Cut the marked resonances with the free bells: dynamic cuts that act only while the peak sounds",
        ),
        Hit::DuckLearn(_) => s(
            "Listen to the range between the locators at the key and draw the duck curve from its hits",
        ),
        Hit::SpecRef(_) => s("Draw another strip or the reference track as a second curve"),
        Hit::TableEdit(_) => {
            s("Open the table editor: draw frames, set harmonics, morph, clean up a sample")
        }
        Hit::DuckPlot(_) => s(
            "Drag a point or the line between two; click to add a point, double-click a point to remove it",
        ),
        Hit::TunerButton(_) => s("Show or hide the tuner: it reads this track's input"),
        Hit::Preview(_) => s(
            "Preview this player: it and the players after it run while the song is stopped (click again to stop)",
        ),
        Hit::Container(_, ContainerButton::Fold) => {
            s("Unfold the container's devices under its panel, or fold them away")
        }
        Hit::Container(_, ContainerButton::Open) => {
            s("Show the container's devices in a view of their own")
        }
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
        self.help_at_with(pos, false)
    }

    /// [`Self::help_at`], and with `knobs` the controls that show a value tooltip of their own
    /// (knobs, faders and sliders) answer too: the long help of Help Mode wants every control.
    /// A parameter's id is `param:<device type>:<param key>`.
    pub fn help_at_with(&self, pos: Point, knobs: bool) -> Option<Help> {
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
            let param_id = || {
                self.devices[i]
                    .desc
                    .map(|d| format!("param:{}:{}", d.type_id, p.key))
            };
            let with_id = |h: Help| match param_id() {
                Some(id) => h.id(id),
                None => h,
            };
            // The guitar amp explains each of its controls.
            if self.devices[i].fx == Some(crate::fx::FxKind::GuitarAmp)
                && let Some(text) = crate::fx::amp_help(p.key)
            {
                return Some(with_id(Help::new(rect, text)));
            }
            if self.devices[i].fx == Some(crate::fx::FxKind::DynamicEq)
                && let Some(text) = crate::fx::dyneq_help(p.key)
            {
                return Some(with_id(Help::new(rect, text)));
            }
            if self.devices[i].fx == Some(crate::fx::FxKind::Duck)
                && let Some(text) = crate::duck_panel::duck_help(p.key)
            {
                return Some(with_id(Help::new(rect, text)));
            }
            if self.devices[i]
                .desc
                .is_some_and(|d| d.type_id == crate::fm_panel::FM_TYPE)
                && let Some(text) = crate::fm_panel::fm_help(p.key)
            {
                return Some(with_id(Help::new(rect, text)));
            }
            if self.devices[i]
                .desc
                .is_some_and(|d| d.type_id == crate::granular_panel::GRANULAR_TYPE)
                && let Some(text) = crate::granular_panel::granular_help(p.key)
            {
                return Some(with_id(Help::new(rect, text)));
            }
            let label = autolayout::control_label(p);
            return match kind {
                ControlKind::Toggle => Some(with_id(Help::new(
                    rect,
                    Cow::Owned(format!("{label}: click to switch on or off")),
                ))),
                ControlKind::Choice if crate::master_panel::is_pick_list(p) => {
                    Some(with_id(Help::new(
                        rect,
                        Cow::Owned(format!(
                            "{label}: click to pick from the list, right-click for the previous"
                        )),
                    )))
                }
                ControlKind::Choice => Some(with_id(Help::new(
                    rect,
                    Cow::Owned(format!("{label}: click to step through the choices")),
                ))),
                _ if knobs => Some(with_id(Help::new(rect, Cow::Owned(label.to_string())))),
                _ => None,
            };
        }
        if let Hit::Mix(i, b) = hit {
            let help = Help::new(rect, self.mix_button_help(i, b));
            return Some(match id_of(hit) {
                Some(id) => help.id(id),
                None => help,
            });
        }
        let help = Help::new(rect, text(hit)?);
        Some(match id_of(hit) {
            Some(id) => help.id(id),
            None => help,
        })
    }
}
