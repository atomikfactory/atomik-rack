//! The rack view's side of the plugin panel: the app-fed status and
//! catalog, the panel's buttons, unpinning a knob and the RELINK menu.

use super::{DeviceView, Flip, Hit, RackView, UiAction, param_cmd};
use crate::autolayout::PanelSpec;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Modifiers, PointerButton};
use crate::plugin_data::{
    METER_DROPPED, METER_LATENCY, METER_STATE, PluginDeviceStatus, PluginEntryInfo,
    PluginEntryStatus,
};
use crate::plugin_panel::{self, PluginButton};
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::{Command, ConsoleCommand, DeviceKey, PluginCommand, PluginDoc};
use std::sync::Arc;
use vello::kurbo::{Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of one row of the RELINK menu.
const MENU_ROW_H: f64 = 18.0;
/// Width of the RELINK menu.
const MENU_W: f64 = 220.0;

/// The open RELINK menu of a plugin panel.
#[derive(Debug, Clone)]
pub(super) struct PluginMenu {
    dev: usize,
    /// (index in the catalog, label)
    entries: Vec<(usize, String)>,
    /// Rectangle in rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

fn menu_row(menu: Rect, i: usize) -> Rect {
    let y = menu.y0 + 2.0 + i as f64 * MENU_ROW_H;
    Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + MENU_ROW_H)
}

impl DeviceView {
    fn plugin_doc(&self) -> Option<&PluginDoc> {
        self.device.as_ref()?.plugin.as_deref()
    }

    fn plugin_button_visible(&self, b: PluginButton) -> bool {
        self.plugin_doc()
            .is_some_and(|doc| plugin_panel::button_visible(b, doc, &self.plugin_status))
    }
}

impl RackView {
    /// Tells the view what the app knows about the live instance behind a plugin device (not
    /// in the document): whether the plugin has an editor (OPEN GUI shows only then), that it
    /// is missing or failed (with the reason) or still loading. `key` is the path key (a local
    /// key inside a scope works too). A device with no status is a healthy plugin without an
    /// editor.
    pub fn set_plugin_status(&mut self, key: &DeviceKey, status: PluginDeviceStatus) {
        let abs = self.abs_key(&self.local_key(key));
        let unchanged = self.plugin_status.get(&abs).is_some_and(|s| *s == status)
            || (status == PluginDeviceStatus::default() && !self.plugin_status.contains_key(&abs));
        if unchanged {
            return;
        }
        if status == PluginDeviceStatus::default() {
            self.plugin_status.remove(&abs);
        } else {
            self.plugin_status.insert(abs, status);
        }
        self.synced = false;
        self.repaint = true;
    }

    /// The status last set for a plugin device (the default when none).
    pub fn plugin_status(&self, key: &DeviceKey) -> PluginDeviceStatus {
        let abs = self.abs_key(&self.local_key(key));
        self.plugin_status.get(&abs).cloned().unwrap_or_default()
    }

    /// Sets the installed plugins: the browser's Plugins group and the RELINK menu list them.
    pub fn set_plugin_catalog(&mut self, entries: Vec<PluginEntryInfo>) {
        if self.plugin_catalog == entries {
            return;
        }
        self.browser.plugins.clone_from(&entries);
        self.plugin_catalog = entries;
        self.plugin_menu = None;
        self.repaint = true;
    }

    /// Sets the plugin files that failed to scan (listed greyed at the end of the browser's
    /// Plugins group).
    pub fn set_plugin_failed_files(&mut self, files: Vec<crate::plugin_data::FailedFileInfo>) {
        if self.browser.plugin_failed != files {
            self.browser.plugin_failed = files;
            self.repaint = true;
        }
    }

    /// The panel of a plugin device: not cached by type, because the knobs are the device's
    /// own pins.
    pub(super) fn plugin_spec(
        &mut self,
        _type_id: &str,
        desc: &DeviceDescriptor,
        doc: &PluginDoc,
    ) -> Arc<PanelSpec> {
        Arc::new(plugin_panel::plugin_layout(desc, doc))
    }

    /// Feeds the meter values of a plugin device (state, latency, dropped): the panel repaints
    /// only when one of the three, rounded, changed.
    pub(super) fn plugin_meter_feed(&mut self, i: usize, values: &[f32]) {
        let at = |ix: usize| values.get(ix).copied().unwrap_or(0.0).round();
        let new = [
            0.0,
            0.0,
            at(METER_STATE),
            at(METER_LATENCY),
            at(METER_DROPPED),
        ];
        if self.devices[i].meters.as_slice() == new {
            return;
        }
        self.devices[i].meters.clear();
        self.devices[i].meters.extend_from_slice(&new);
        self.devices[i].revision += 1;
        self.repaint = true;
    }

    /// Centre of a button of a plugin panel (logical view coordinates); `None` when the button
    /// is not shown (OPEN GUI without an editor, TO CH on an effect, RELINK on a healthy
    /// plugin), on the back, or on a folded device.
    pub fn plugin_button_center(&self, key: &DeviceKey, b: PluginButton) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if self.flip != Flip::Front || d.folded || !d.plugin_button_visible(b) {
            return None;
        }
        let r = d.spec.plugin?.button(b);
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// The rows of the open RELINK menu: (label, centre in logical view coordinates).
    pub fn plugin_menu_rows(&self) -> Vec<(String, Point)> {
        let Some(m) = &self.plugin_menu else {
            return Vec::new();
        };
        m.entries
            .iter()
            .enumerate()
            .map(|(i, (_, label))| {
                (
                    label.clone(),
                    self.rack_to_view(menu_row(m.rect, i).center()),
                )
            })
            .collect()
    }

    pub(super) fn plugin_down(
        &mut self,
        i: usize,
        b: PluginButton,
        rp: Point,
        out: &mut Vec<UiAction>,
    ) {
        self.select(i, out);
        if !self.devices[i].plugin_button_visible(b) {
            return;
        }
        let key = self.devices[i].key.clone();
        let abs = self.abs_key(&key);
        let Some(doc) = self.devices[i].plugin_doc().cloned() else {
            return;
        };
        match b {
            PluginButton::Power => out.push(UiAction::Command(Command::Plugin(
                PluginCommand::SetPower {
                    device: key,
                    powered: !doc.powered,
                },
            ))),
            PluginButton::Bypass => {
                let on = self.devices[i]
                    .device
                    .as_ref()
                    .and_then(|d| d.params.get("bypass"))
                    .is_some_and(|v| *v >= 0.5);
                out.push(param_cmd(&key, "bypass", if on { 0.0 } else { 1.0 }, None));
            }
            PluginButton::AudioThread => out.push(UiAction::Command(Command::Plugin(
                PluginCommand::SetRunMode {
                    device: key,
                    run_on_audio_thread: !doc.run_on_audio_thread,
                },
            ))),
            PluginButton::OpenGui => self.direct.push(UiAction::OpenPluginGui { device: abs }),
            PluginButton::Params => self.direct.push(UiAction::OpenPluginParams { device: abs }),
            PluginButton::ToChannel => {
                if !self.scope.is_empty() {
                    self.toast =
                        Some("Only a device in the rack itself can feed a console channel".into());
                } else if !self.devices[i].has_channel {
                    out.push(UiAction::Command(Command::Console(
                        ConsoleCommand::AddChannel { source: key },
                    )));
                }
            }
            PluginButton::Reload => {
                if !self.devices[i].plugin_status.loading {
                    self.direct.push(UiAction::ReloadPlugin { device: abs });
                }
            }
            PluginButton::Relink => self.plugin_menu_open(i, &doc, rp),
        }
    }

    fn plugin_menu_open(&mut self, i: usize, doc: &PluginDoc, rp: Point) {
        self.repaint = true;
        let entries: Vec<(usize, String)> = self
            .plugin_catalog
            .iter()
            .enumerate()
            .filter(|(_, e)| e.status == PluginEntryStatus::Ok && e.kind == doc.layout.kind_hint)
            .map(|(n, e)| (n, e.title()))
            .collect();
        if entries.is_empty() {
            self.toast = Some("No installed plugin of the same kind to relink to".into());
            return;
        }
        let h = entries.len() as f64 * MENU_ROW_H + 4.0;
        let x0 =
            rp.x.min(self.devices[i].x + f64::from(atomik_device_sdk::RACK_WIDTH) - 16.0 - MENU_W);
        let bottom = self.view_to_rack(Point::new(0.0, self.size.height)).y;
        let y0 = if rp.y + h > bottom { rp.y - h } else { rp.y };
        self.plugin_menu = Some(PluginMenu {
            dev: i,
            entries,
            rect: Rect::new(x0, y0, x0 + MENU_W, y0 + h),
            hover: None,
        });
    }

    /// A press while the RELINK menu is open: a row relinks; any press closes the menu and is
    /// consumed. Returns whether a press was consumed.
    pub(super) fn plugin_menu_press(&mut self, pos: Point) -> bool {
        let Some(menu) = self.plugin_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        let row = (0..menu.entries.len()).find(|&k| menu_row(menu.rect, k).contains(rp));
        if let Some(k) = row
            && let Some(entry) = self.plugin_catalog.get(menu.entries[k].0).cloned()
            && let Some(d) = self.devices.get(menu.dev)
        {
            let device = self.abs_key(&d.key);
            self.direct
                .push(UiAction::Command(Command::Plugin(PluginCommand::Relink {
                    device,
                    plugin: entry.plugin.clone(),
                    layout: entry.layout.clone(),
                    path_hint: entry.path.clone(),
                })));
        }
        true
    }

    pub(super) fn plugin_menu_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.plugin_menu {
            let hover = (0..menu.entries.len()).find(|&k| menu_row(menu.rect, k).contains(rp));
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Shift + right-click on a pinned knob of a plugin panel unpins it (the panel keeps at
    /// least one pin).
    pub(super) fn plugin_unpin_wanted(
        &self,
        i: usize,
        button: PointerButton,
        modifiers: Modifiers,
    ) -> bool {
        button == PointerButton::Secondary
            && modifiers.shift
            && self.devices[i].spec.plugin.is_some()
    }

    pub(super) fn plugin_unpin(&mut self, i: usize, slot: usize, out: &mut Vec<UiAction>) {
        let Some((p, _, _)) = self.param_of(i, slot) else {
            return;
        };
        let (Some(id), Some(doc)) = (
            plugin_panel::param_id_of(p.key),
            self.devices[i].plugin_doc().cloned(),
        ) else {
            return;
        };
        let mut pins = plugin_panel::featured_pins(&doc);
        if pins.len() <= 1 {
            self.toast = Some("A plugin panel keeps at least one pinned param".into());
            return;
        }
        pins.retain(|x| *x != id);
        out.push(UiAction::Command(Command::Plugin(
            PluginCommand::SetPinned {
                device: self.devices[i].key.clone(),
                pinned: pins,
            },
        )));
    }

    /// Paints the open RELINK menu (rack coordinates).
    pub(super) fn paint_plugin_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.plugin_menu else { return };
        let theme = &self.theme;
        c.fill_with(RoundedRect::from_rect(m.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            Color::WHITE.with_alpha(0.14),
        );
        for (k, (_, label)) in m.entries.iter().enumerate() {
            let r = menu_row(m.rect, k);
            if m.hover == Some(k) {
                c.fill_with(
                    RoundedRect::from_rect(r.inset(-1.0), 3.0),
                    theme.browser_row,
                );
            }
            c.text(
                &TextRun::new(
                    crate::paint::fit_label(label, r.width() - 14.0, 9.5),
                    pt(r.x0 + 8.0, r.y1 - 5.5),
                    9.5,
                    theme.text,
                )
                .align(Align::Start),
            );
        }
    }

    /// The hit type of a plugin button, for the tree.
    pub(super) fn plugin_hits(d: &DeviceView, i: usize) -> Vec<(Rect, Hit)> {
        let Some(panel) = d.spec.plugin else {
            return Vec::new();
        };
        PluginButton::ALL
            .into_iter()
            .filter(|b| d.plugin_button_visible(*b))
            .map(|b| (panel.button(b), Hit::Plugin(i, b)))
            .collect()
    }
}
