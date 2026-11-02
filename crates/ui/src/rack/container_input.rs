//! Rack view code of the container: the scope (the rack showing a
//! container's inside), the breadcrumb, the lifting of local keys to path keys, the container
//! panel's buttons and macro names, and the "map to macro" menu of the inner devices' controls.
//!
//! # How a scope works
//!
//! The rack view keeps working on a [`RackDoc`]: inside a container the host document is
//! replaced, for every `sync`, `handle` and `paint`, by a **scope document** whose devices and
//! cables are the container's inside (local keys, the boundary as the pseudo-device
//! `boundary`). Every panel, hit test and gesture is therefore the one of the top level. What
//! the view emits is **lifted** on the way out: local keys get the container path as a prefix
//! (`synth` inside `layer` is `layer/synth`, `boundary` is `layer/boundary`), `AddDevice`
//! becomes `ContainerCommand::AddInner`. Actions about the scope itself (enter, exit, the
//! container editor, the macro menu's commands) are built with absolute keys and are not lifted.

use super::{Drag, RackView, Rename, RenameKind, UiAction};
use crate::breadcrumb::{self, CrumbHit};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::container_panel::ContainerButton;
use crate::event::{Modifiers, PointerButton};
use crate::seq_panel::{self, RenameKey};
use atomik_device_sdk::{ControlKind, DeviceKind, PluginKindHint, RACK_WIDTH, Registry};
use atomik_model::{
    Cable, Command, Console, ConsoleCommand, ContainerCommand, DeviceKey, MACRO_COUNT,
    MAX_MACRO_NAME_CHARS, MacroTarget, PluginCommand, RackDoc, SocketRef,
};
use std::borrow::Cow;
use vello::kurbo::{Point, Rect, RoundedRect, Vec2};

/// Height of one row of the macro menu.
const MENU_ROW_H: f64 = 18.0;
/// Width of the macro menu.
const MENU_W: f64 = 150.0;

/// An entry of the macro menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuEntry {
    /// Map the param to macro `n` (0-based).
    Map(u8),
    /// Remove the param from macro `n`.
    Unmap(u8),
    /// Open the container editor on macro `n`.
    EditRange(u8),
}

/// The open "map to macro" menu of a control.
#[derive(Debug, Clone)]
pub(super) struct MacroMenu {
    dev: usize,
    param: String,
    entries: Vec<(MenuEntry, String)>,
    /// Rectangle in rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

fn join(parent: Option<&DeviceKey>, local: &DeviceKey) -> DeviceKey {
    match parent {
        Some(p) => DeviceKey(format!("{}/{}", p.0, local.0)),
        None => local.clone(),
    }
}

impl RackView {
    // ------------------------------------------------------------------ scope

    /// The containers whose inside the rack shows, as local keys from the top level down
    /// (empty at the top level).
    pub fn scope(&self) -> &[DeviceKey] {
        &self.scope
    }

    /// The path key of the container whose inside is shown (`None` at the top level).
    pub fn scope_path(&self) -> Option<DeviceKey> {
        self.path_of(self.scope.len())
    }

    /// The path of the first `n` scope segments.
    fn path_of(&self, n: usize) -> Option<DeviceKey> {
        self.scope[..n.min(self.scope.len())]
            .iter()
            .fold(None, |acc, k| Some(join(acc.as_ref(), k)))
    }

    /// Shows the inside of the container at `scope` (local keys from the top level down; an
    /// element may also be a `/`-joined path). An empty slice shows the top level. A scope that
    /// does not exist in the document is left on the next sync. No action is emitted.
    pub fn set_scope(&mut self, scope: &[DeviceKey]) {
        let scope: Vec<DeviceKey> = scope
            .iter()
            .flat_map(|k| k.0.split('/'))
            .filter(|s| !s.is_empty())
            .map(|s| DeviceKey(s.to_string()))
            .collect();
        self.apply_scope(scope);
    }

    /// The display names of the containers of the scope (the breadcrumb's segments after
    /// `Project`).
    pub fn scope_names(&self) -> &[String] {
        &self.scope_names
    }

    /// How many containers deep the view is.
    pub fn scope_depth(&self) -> usize {
        self.scope.len()
    }

    pub(super) fn apply_scope(&mut self, scope: Vec<DeviceKey>) {
        if scope == self.scope {
            return;
        }
        self.scope = scope;
        self.scope_names.clear();
        self.devices.clear();
        self.tree = crate::tree::ViewTree::new(Rect::ZERO, super::Hit::Rack);
        self.synced = false;
        self.clear_selection();
        self.scroll = Vec2::ZERO;
        self.drag = Drag::None;
        self.hover = None;
        self.hover_cable = None;
        self.rename = None;
        self.sc_menu = None;
        self.macro_menu = None;
        self.crumb_hover = None;
        self.pending_scope = None;
        self.driven.clear();
        self.automated.clear();
        self.epoch += 1 << 20;
        self.browser.scope_depth = self.scope.len();
        self.repaint = true;
    }

    /// The document the views are built from: the host document at the top level, else the
    /// scope document of the container in view (see the module documentation). Leaves a scope
    /// whose container is gone for the nearest one that exists.
    pub(super) fn prepare<'a>(
        &mut self,
        doc: &'a RackDoc,
        registry: &Registry,
    ) -> Cow<'a, RackDoc> {
        if self.scope.is_empty() {
            self.compute_bank_driven(doc);
            self.compute_automated(doc);
            // The panels show the value an automation lane gives a param while the song plays
            // or is positioned (`shown`), and the value a macro gives it, not the stored one.
            let base = doc.view_with_shown(&self.shown);
            let base = match base.view_with_driven_values(registry) {
                Some(view) => Cow::Owned(view),
                None => base,
            };
            // Unfolded containers: the devices of their inside join the rack's own (see
            // [`super::inline`]).
            let inline = base.devices.iter().any(super::inline::is_open);
            if inline != self.inline {
                self.inline = inline;
                self.synced = false;
            }
            if inline {
                self.compute_inline_driven(&base);
                if let Some(flat) = super::inline::flatten(&base) {
                    return Cow::Owned(flat);
                }
            }
            return base;
        }
        // A scope shows one container's inside on its own: nothing is unfolded in it.
        if self.inline {
            self.inline = false;
            self.synced = false;
        }
        let mut keep = 0;
        let mut path: Option<DeviceKey> = None;
        for k in &self.scope {
            let next = join(path.as_ref(), k);
            if doc.inner(&next).is_none() {
                break;
            }
            path = Some(next);
            keep += 1;
        }
        if keep < self.scope.len() {
            let levels = self.scope.len() - keep;
            let new = self.scope[..keep].to_vec();
            self.apply_scope(new);
            self.popped += levels;
            if keep == 0 {
                self.driven.clear();
                self.automated.clear();
                return Cow::Borrowed(doc);
            }
        }
        self.scope_rack_at = doc
            .devices
            .iter()
            .position(|d| d.key == self.scope[0])
            .map_or(usize::MAX, |i| i + 1);
        self.scope_names = (1..=self.scope.len())
            .map(|n| {
                self.path_of(n)
                    .and_then(|p| doc.resolve(&p))
                    .map_or_else(String::new, |d| d.name.clone())
            })
            .collect();
        self.compute_driven(doc);
        self.automated.clear();
        let Some(container) = path.as_ref().and_then(|p| doc.inner(p)) else {
            return Cow::Borrowed(doc);
        };
        self.scope_macros = std::array::from_fn(|i| container.macros[i].name.clone());
        Cow::Owned(RackDoc {
            sample_rate: doc.sample_rate,
            tempo_bpm: doc.tempo_bpm,
            seed: doc.seed,
            output_channels: 2,
            input_channels: 2,
            assets: doc.assets.clone(),
            devices: container.devices.clone(),
            columns: 1,
            cables: container.cables.clone(),
            song: atomik_model::empty_song(doc.tempo_bpm).into(),
            console: Console::default(),
            midi_maps: Vec::new(),
            macros: Default::default(),
            info: Default::default(),
        })
    }

    /// Which params of the devices in view a macro drives (of this container or one around it).
    fn compute_driven(&mut self, doc: &RackDoc) {
        self.driven.clear();
        let depth = self.scope.len();
        for d in 1..=depth {
            let Some(inner) = self.path_of(d).and_then(|p| doc.inner(&p)) else {
                continue;
            };
            // The shown devices are `rest` below this container.
            let rest: Vec<&str> = self.scope[d..].iter().map(|k| k.0.as_str()).collect();
            let prefix = if rest.is_empty() {
                String::new()
            } else {
                format!("{}/", rest.join("/"))
            };
            for (m, md) in inner.macros.iter().enumerate() {
                for t in &md.targets {
                    if let Some(local) = t.device.0.strip_prefix(&prefix)
                        && !local.contains('/')
                    {
                        self.driven
                            .entry(DeviceKey(local.to_string()))
                            .or_default()
                            .push((t.param.clone(), m as u8, d == depth));
                    }
                }
            }
        }
    }

    /// Which params of the top-level devices a macro bank drives (the locks and tags). These are
    /// never the scope's own container macros, so the "map to macro" menu ignores them.
    fn compute_bank_driven(&mut self, doc: &RackDoc) {
        self.driven.clear();
        for a in &doc.macros.assigns {
            if doc.macro_assign_live(a) {
                self.driven.entry(a.device.clone()).or_default().push((
                    a.param.clone(),
                    a.index,
                    false,
                ));
            }
        }
    }

    /// Which params of the top-level devices have an automation lane (their controls are
    /// marked).
    fn compute_automated(&mut self, doc: &RackDoc) {
        self.automated.clear();
        for (device, param) in doc.automated_params() {
            self.automated.entry(device).or_default().push(param);
        }
    }

    /// The params of a shown device that have an automation lane, for its panel.
    pub(super) fn automated_for(&self, key: &DeviceKey) -> Vec<String> {
        self.automated.get(key).cloned().unwrap_or_default()
    }

    /// The driven params of a shown device, for its panel.
    pub(super) fn driven_for(&self, key: &DeviceKey) -> Vec<(String, u8)> {
        self.driven
            .get(key)
            .map(|v| v.iter().map(|(p, m, _)| (p.clone(), *m)).collect())
            .unwrap_or_default()
    }

    /// The macro (0-based) that drives `param` of the device view `i`.
    pub(super) fn driven_by(&self, i: usize, param: &str) -> Option<u8> {
        self.devices[i]
            .driven
            .iter()
            .find(|(k, _)| k == param)
            .map(|(_, m)| *m)
    }

    /// Whether `param` of the device view `i` is driven by the macro of the container in view.
    fn driven_here(&self, i: usize, param: &str) -> Option<u8> {
        self.driven
            .get(&self.devices[i].key)?
            .iter()
            .find(|(k, _, here)| *here && k == param)
            .map(|(_, m, _)| *m)
    }

    pub(super) fn notify_driven(&mut self, i: usize, param: &str) {
        if let Some(m) = self.driven_by(i, param) {
            let in_container =
                !self.scope.is_empty() || super::inline::is_inner(&self.devices[i].key);
            self.toast = Some(if !in_container {
                format!(
                    "Driven by macro {}: right-click to remove it from the macro first",
                    u16::from(m) + 1
                )
            } else {
                format!(
                    "Driven by macro {}: right-click to unmap it first",
                    u16::from(m) + 1
                )
            });
        }
    }

    /// The key of a device view as the document names it (the path key inside a container).
    pub(super) fn abs_key(&self, key: &DeviceKey) -> DeviceKey {
        join(self.scope_path().as_ref(), key)
    }

    /// The view key of a device: a path key inside the scope loses the scope's prefix.
    pub(super) fn local_key(&self, key: &DeviceKey) -> DeviceKey {
        if let Some(p) = self.scope_path()
            && let Some(rest) = key.0.strip_prefix(&format!("{}/", p.0))
        {
            return DeviceKey(rest.to_string());
        }
        key.clone()
    }

    /// The device views that are real devices (not the boundary panels).
    pub(super) fn inner_range(&self) -> std::ops::Range<usize> {
        let trailing = usize::from(!self.scope.is_empty());
        1..self.devices.len().saturating_sub(trailing).max(1)
    }

    // ------------------------------------------------------------------ lifting

    /// Turns what the view emitted (local keys) into what the host applies, appends the actions
    /// about the scope itself and applies a scope change decided by the gesture.
    pub(super) fn finish(&mut self, out: Vec<UiAction>, registry: &Registry) -> Vec<UiAction> {
        let mut res: Vec<UiAction> = if self.scope.is_empty() {
            out
        } else {
            out.into_iter()
                .filter_map(|a| self.lift(a, registry))
                .collect()
        };
        res.append(&mut self.direct);
        if self.popped > 0 {
            res.push(UiAction::ExitContainer {
                levels: std::mem::take(&mut self.popped),
            });
        }
        if let Some(scope) = self.pending_scope.take() {
            self.apply_scope(scope);
        }
        res
    }

    fn up(&self, k: &DeviceKey) -> DeviceKey {
        self.abs_key(k)
    }

    pub(super) fn up_socket(&self, s: &SocketRef) -> SocketRef {
        SocketRef {
            device: self.up(&s.device),
            socket: s.socket.clone(),
        }
    }

    fn lift(&self, a: UiAction, registry: &Registry) -> Option<UiAction> {
        let k = |d: DeviceKey| self.up(&d);
        Some(match a {
            UiAction::Command(c) => UiAction::Command(self.lift_command(c, registry)?),
            UiAction::Select(d) => UiAction::Select(k(d)),
            UiAction::SelectMany { primary, keys } => UiAction::SelectMany {
                primary: k(primary),
                keys: keys.into_iter().map(k).collect(),
            },
            UiAction::Audition { device, key, on } => UiAction::Audition {
                device: k(device),
                key,
                on,
            },
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => UiAction::AuditionVelocity {
                device: k(device),
                key,
                velocity,
                on,
            },
            UiAction::ImportSamples {
                paths,
                device,
                at_key,
            } => UiAction::ImportSamples {
                paths,
                device: k(device),
                at_key,
            },
            UiAction::SampleInto { device, at_key } => UiAction::SampleInto {
                device: k(device),
                at_key,
            },
            UiAction::EditSample {
                device,
                zone,
                all_uses,
                edit,
            } => UiAction::EditSample {
                device: k(device),
                zone,
                all_uses,
                edit,
            },
            UiAction::OpenSampleEditor(d) => UiAction::OpenSampleEditor(k(d)),
            UiAction::OpenSliceEditor(d) => UiAction::OpenSliceEditor(k(d)),
            UiAction::OpenStepEditor { device, clip } => UiAction::OpenStepEditor {
                device: k(device),
                clip,
            },
            UiAction::SliceLoop { device, mode } => UiAction::SliceLoop {
                device: k(device),
                mode,
            },
            UiAction::SlicesToTrack { device } => UiAction::SlicesToTrack { device: k(device) },
            UiAction::OpenSeqEditor { device } => UiAction::OpenSeqEditor { device: k(device) },
            UiAction::GenerateDrums {
                device,
                index,
                randomise,
            } => UiAction::GenerateDrums {
                device: k(device),
                index,
                randomise,
            },
            UiAction::ControlMenu { device, param, at } => UiAction::ControlMenu {
                device: k(device),
                param,
                at,
            },
            UiAction::JackMenu { socket, at } => UiAction::JackMenu {
                socket: self.up_socket(&socket),
                at,
            },
            UiAction::ParamTouched { device, param } => UiAction::ParamTouched {
                device: k(device),
                param,
            },
            UiAction::SelectPattern { device, index } => UiAction::SelectPattern {
                device: k(device),
                index,
            },
            other => other,
        })
    }

    fn lift_command(&self, c: Command, registry: &Registry) -> Option<Command> {
        let k = |d: DeviceKey| self.up(&d);
        Some(match c {
            Command::SetParam {
                key,
                param,
                value,
                merge,
            } => Command::SetParam {
                key: k(key),
                param,
                value,
                merge,
            },
            Command::SetFolded { key, folded } => Command::SetFolded {
                key: k(key),
                folded,
            },
            Command::RemoveDevice { key } => Command::RemoveDevice { key: k(key) },
            Command::MoveDevice { key, to } => Command::MoveDevice { key: k(key), to },
            Command::Rename { key, name } => Command::Rename { key: k(key), name },
            Command::SetColor { key, color } => Command::SetColor { key: k(key), color },
            Command::Connect(cable) => Command::Connect(Cable {
                from: self.up_socket(&cable.from),
                to: self.up_socket(&cable.to),
            }),
            Command::Disconnect { to } => Command::Disconnect {
                to: self.up_socket(&to),
            },
            Command::Batch(v) => Command::Batch(
                v.into_iter()
                    .filter_map(|c| self.lift_command(c, registry))
                    .collect(),
            ),
            Command::Workflow(w) => Command::Workflow(self.lift_workflow(w)?),
            Command::Container(ContainerCommand::DuplicateContainer { container }) => {
                Command::Container(ContainerCommand::DuplicateContainer {
                    container: k(container),
                })
            }
            Command::AddDevice {
                index,
                type_id,
                key,
            } => {
                let instrument = registry
                    .get(&type_id)
                    .is_some_and(|f| f.descriptor.kind == DeviceKind::Instrument);
                Command::Container(ContainerCommand::AddInner {
                    container: self.scope_path()?,
                    index,
                    type_id,
                    key,
                    auto_split: instrument,
                })
            }
            Command::SetZones {
                device,
                zones,
                merge,
            } => Command::SetZones {
                device: k(device),
                zones,
                merge,
            },
            Command::SetPattern {
                device,
                index,
                pattern,
                merge,
            } => Command::SetPattern {
                device: k(device),
                index,
                pattern,
                merge,
            },
            Command::Plugin(p) => Command::Plugin(self.lift_plugin(p)?),
            Command::AddPattern { device } => Command::AddPattern { device: k(device) },
            Command::RemovePattern { device, index } => Command::RemovePattern {
                device: k(device),
                index,
            },
            Command::DuplicatePattern { device, index } => Command::DuplicatePattern {
                device: k(device),
                index,
            },
            Command::SetCurves {
                device,
                index,
                curves,
                merge,
            } => Command::SetCurves {
                device: k(device),
                index,
                curves,
                merge,
            },
            Command::SetDrumStep {
                device,
                index,
                lane,
                step,
                velocity,
                prob,
                merge,
            } => Command::SetDrumStep {
                device: k(device),
                index,
                lane,
                step,
                velocity,
                prob,
                merge,
            },
            Command::SetDrumLane {
                device,
                index,
                lane,
                setting,
            } => Command::SetDrumLane {
                device: k(device),
                index,
                lane,
                setting,
            },
            Command::SetDrumPattern {
                device,
                index,
                pattern,
                merge,
            } => Command::SetDrumPattern {
                device: k(device),
                index,
                pattern,
                merge,
            },
            Command::AddDrumPattern { device } => Command::AddDrumPattern { device: k(device) },
            Command::RemoveDrumPattern { device, index } => Command::RemoveDrumPattern {
                device: k(device),
                index,
            },
            Command::DuplicateDrumPattern { device, index } => Command::DuplicateDrumPattern {
                device: k(device),
                index,
            },
            other => other,
        })
    }

    /// Lifts a plugin command of the scope: devices get path keys, an add goes into the
    /// container (an instrument with its full-range split).
    fn lift_plugin(&self, p: PluginCommand) -> Option<PluginCommand> {
        let k = |d: DeviceKey| self.up(&d);
        Some(match p {
            PluginCommand::Add {
                index,
                plugin,
                layout,
                path_hint,
                key,
                ..
            } => PluginCommand::Add {
                index,
                container: Some(self.scope_path()?),
                auto_split: layout.kind_hint == PluginKindHint::Instrument,
                plugin,
                layout,
                path_hint,
                key,
            },
            PluginCommand::SetPinned { device, pinned } => PluginCommand::SetPinned {
                device: k(device),
                pinned,
            },
            PluginCommand::SetRunMode {
                device,
                run_on_audio_thread,
            } => PluginCommand::SetRunMode {
                device: k(device),
                run_on_audio_thread,
            },
            PluginCommand::SetPower { device, powered } => PluginCommand::SetPower {
                device: k(device),
                powered,
            },
            PluginCommand::SetState {
                device,
                state,
                merge,
            } => PluginCommand::SetState {
                device: k(device),
                state,
                merge,
            },
            PluginCommand::Relink {
                device,
                plugin,
                layout,
                path_hint,
            } => PluginCommand::Relink {
                device: k(device),
                plugin,
                layout,
                path_hint,
            },
            other => other,
        })
    }

    // ------------------------------------------------------------------ going in and out

    /// Enters the container of device view `i` (the view has changed when `handle` returns).
    fn enter(&mut self, i: usize) {
        let key = self.devices[i].key.clone();
        let mut scope = self.scope.clone();
        // An inner container of an unfolded one has a path key: one scope level per segment.
        scope.extend(key.0.split('/').map(|s| DeviceKey(s.to_string())));
        let device = self.abs_key(&key);
        self.pending_scope = Some(scope);
        self.direct.push(UiAction::EnterContainer { device });
        self.repaint = true;
    }

    /// Goes up `levels` scopes.
    pub(super) fn go_up(&mut self, levels: usize) {
        let levels = levels.min(self.scope.len());
        if levels == 0 {
            return;
        }
        let scope = self.scope[..self.scope.len() - levels].to_vec();
        self.pending_scope = Some(scope);
        self.direct.push(UiAction::ExitContainer { levels });
        self.repaint = true;
    }

    /// Esc leaves the container when nothing else wants it: no drag, nothing selected, no
    /// focused control.
    pub(super) fn escape_goes_up(&self) -> bool {
        !self.scope.is_empty()
            && matches!(self.drag, Drag::None)
            && self.selected.is_none()
            && self.tree.focused().is_none()
    }

    // ------------------------------------------------------------------ breadcrumb

    pub(super) fn bar_rect(&self) -> Rect {
        let a = self.rack_area();
        Rect::new(a.x0, 0.0, a.x1, breadcrumb::BAR_HEIGHT)
    }

    /// The breadcrumb labels: the project, then each container of the scope.
    pub fn breadcrumb_labels(&self) -> Vec<String> {
        let mut v = vec![breadcrumb::PROJECT_LABEL.to_string()];
        v.extend(
            self.scope
                .iter()
                .enumerate()
                .map(|(n, k)| match self.scope_names.get(n) {
                    Some(name) if !name.is_empty() => name.clone(),
                    _ => k.0.clone(),
                }),
        );
        v
    }

    /// Centre of a breadcrumb element (logical view coordinates); `None` at the top level.
    pub fn breadcrumb_center(&self, hit: CrumbHit) -> Option<Point> {
        if self.scope.is_empty() {
            return None;
        }
        breadcrumb::layout(self.bar_rect(), &self.breadcrumb_labels())
            .into_iter()
            .find(|(h, _)| *h == hit)
            .map(|(_, r)| r.center())
    }

    pub(super) fn paint_breadcrumb(&self, c: &mut dyn Canvas) {
        breadcrumb::paint(
            c,
            &self.theme,
            self.bar_rect(),
            &self.breadcrumb_labels(),
            self.crumb_hover,
        );
    }

    pub(super) fn crumb_hover_at(&mut self, pos: Point) {
        let now = (!self.scope.is_empty() && !matches!(self.drag, Drag::Value { .. }))
            .then(|| breadcrumb::hit(self.bar_rect(), &self.breadcrumb_labels(), pos))
            .flatten();
        if now != self.crumb_hover {
            self.crumb_hover = now;
            self.repaint = true;
        }
    }

    /// A press on the bar: Back and the earlier segments go up. Returns whether the press was
    /// on the bar (and so consumed).
    pub(super) fn crumb_press(&mut self, pos: Point, button: PointerButton) -> bool {
        if self.scope.is_empty() || !self.bar_rect().contains(pos) {
            return false;
        }
        if button == PointerButton::Primary {
            match breadcrumb::hit(self.bar_rect(), &self.breadcrumb_labels(), pos) {
                Some(CrumbHit::Back) => self.go_up(1),
                Some(CrumbHit::Segment(k)) if k < self.scope.len() => {
                    self.go_up(self.scope.len() - k);
                }
                _ => {}
            }
        }
        true
    }

    // ------------------------------------------------------------------ container panel

    /// Centre of a button of a container panel (logical view coordinates), front side,
    /// unfolded.
    pub fn container_button_center(&self, key: &DeviceKey, b: ContainerButton) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if self.flip != super::Flip::Front || d.folded {
            return None;
        }
        let r = d.spec.container?.button(b);
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// Centre of a macro's name on a container panel (logical view coordinates), macro `n`
    /// 0-based; a double-click renames it.
    pub fn macro_name_center(&self, key: &DeviceKey, n: u8) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if self.flip != super::Flip::Front || d.folded || usize::from(n) >= MACRO_COUNT {
            return None;
        }
        let r = d.spec.container?.names[usize::from(n)];
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// The macro name being edited in place on the panel of `key`: (macro 0-based, text).
    pub fn renaming_macro(&self, key: &DeviceKey) -> Option<(u8, &str)> {
        let r = self
            .rename
            .as_ref()
            .filter(|r| r.kind == RenameKind::Macro)?;
        (self.devices.get(r.dev)?.key == self.local_key(key)).then_some((r.index, r.text.as_str()))
    }

    pub(super) fn container_down(&mut self, i: usize, b: ContainerButton, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let key = self.devices[i].key.clone();
        match b {
            ContainerButton::Fold => {
                let open = self.devices[i]
                    .device
                    .as_ref()
                    .and_then(|d| d.inner.as_ref())
                    .is_some_and(|inner| inner.open);
                self.direct.push(UiAction::Command(Command::Container(
                    ContainerCommand::SetOpen {
                        container: self.abs_key(&key),
                        open: !open,
                    },
                )));
            }
            ContainerButton::Open => self.enter(i),
            ContainerButton::Edit => {
                let device = self.abs_key(&key);
                self.direct.push(UiAction::OpenContainerEditor {
                    device,
                    macros: None,
                });
            }
            ContainerButton::ToChannel => {
                if !self.scope.is_empty() || super::inline::is_inner(&key) {
                    self.toast = Some(
                        "Only a container in the rack itself can feed a console channel".into(),
                    );
                } else if !self.devices[i].has_channel {
                    out.push(UiAction::Command(Command::Console(
                        ConsoleCommand::AddChannel { source: key },
                    )));
                }
            }
        }
    }

    pub(super) fn macro_name_down(&mut self, i: usize, n: u8, clicks: u8, out: &mut Vec<UiAction>) {
        self.select(i, out);
        if clicks >= 2 {
            let text = self.devices[i]
                .device
                .as_ref()
                .and_then(|d| d.inner.as_ref())
                .map_or_else(String::new, |inner| {
                    inner.macros[usize::from(n)].name.clone()
                });
            self.rename = Some(Rename {
                dev: i,
                index: n,
                text,
                kind: RenameKind::Macro,
            });
            self.bump(i);
        }
    }

    /// One key press while a macro name is edited.
    pub(super) fn macro_rename_key(&mut self, mut r: Rename, key: &crate::event::Key) {
        match seq_panel::rename_key(&mut r.text, key) {
            RenameKey::Editing => {
                r.text = r.text.chars().take(MAX_MACRO_NAME_CHARS).collect();
                let dev = r.dev;
                self.rename = Some(r);
                self.bump(dev);
            }
            RenameKey::Commit => {
                self.bump(r.dev);
                let name = r.text.trim().to_string();
                let current = self.devices[r.dev]
                    .device
                    .as_ref()
                    .and_then(|d| d.inner.as_ref())
                    .map(|inner| inner.macros[usize::from(r.index)].name.clone());
                if current.is_some_and(|c| c != name) {
                    let container = self.abs_key(&self.devices[r.dev].key);
                    self.direct.push(UiAction::Command(Command::Container(
                        ContainerCommand::RenameMacro {
                            container,
                            index: r.index,
                            name,
                        },
                    )));
                }
            }
            RenameKey::Cancel => self.bump(r.dev),
        }
    }

    // ------------------------------------------------------------------ the macro menu

    /// Whether a press on a control opens the macro menu: inside a container, a right-click on
    /// a knob, fader or toggle (Ctrl/Cmd + right-click on any control; a plain right-click on a
    /// choice still steps it back).
    pub(super) fn macro_menu_wanted(
        &self,
        i: usize,
        slot: usize,
        button: PointerButton,
        modifiers: Modifiers,
    ) -> bool {
        if self.macro_container(i).is_none() || button != PointerButton::Secondary {
            return false;
        }
        let Some((_, kind, _)) = self.param_of(i, slot) else {
            return false;
        };
        modifiers.command() || kind != ControlKind::Choice
    }

    pub(super) fn macro_menu_open(&mut self, i: usize, slot: usize, rp: Point) {
        self.repaint = true;
        let Some((p, _, _)) = self.param_of(i, slot) else {
            return;
        };
        let param = p.key.to_string();
        let entries: Vec<(MenuEntry, String)> = if let Some(m) = self.driven_here(i, &param) {
            vec![
                (MenuEntry::EditRange(m), "Edit range".into()),
                (MenuEntry::Unmap(m), "Unmap".into()),
            ]
        } else if self.driven_by(i, &param).is_some() {
            self.notify_driven(i, &param);
            return;
        } else {
            (0..MACRO_COUNT as u8)
                .map(|m| {
                    let name = &self.macro_names(i)[usize::from(m)];
                    let label = if name.is_empty() {
                        format!("Map to macro {}", m + 1)
                    } else {
                        format!("Map to macro {} ({name})", m + 1)
                    };
                    (MenuEntry::Map(m), label)
                })
                .collect()
        };
        let h = entries.len() as f64 * MENU_ROW_H + 4.0;
        let x0 =
            rp.x.min(self.devices[i].x + f64::from(RACK_WIDTH) - 16.0 - MENU_W);
        let bottom = self.view_to_rack(Point::new(0.0, self.size.height)).y;
        let y0 = if rp.y + h > bottom { rp.y - h } else { rp.y };
        self.macro_menu = Some(MacroMenu {
            dev: i,
            param,
            entries,
            rect: Rect::new(x0, y0, x0 + MENU_W, y0 + h),
            hover: None,
        });
    }

    fn menu_row(menu: Rect, i: usize) -> Rect {
        let y = menu.y0 + 2.0 + i as f64 * MENU_ROW_H;
        Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + MENU_ROW_H)
    }

    fn menu_row_at(menu: &MacroMenu, rp: Point) -> Option<usize> {
        (0..menu.entries.len()).find(|&i| Self::menu_row(menu.rect, i).contains(rp))
    }

    /// A press while the menu is open: a row applies its entry; any press closes the menu and
    /// is consumed. Returns whether a press was consumed.
    pub(super) fn macro_press(&mut self, pos: Point, _out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.macro_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        if let Some(row) = Self::menu_row_at(&menu, rp) {
            self.apply_entry(&menu, menu.entries[row].0);
        }
        true
    }

    /// The container whose macros the controls of device view `i` map to: the open container
    /// in a scope, else the unfolded container the device is drawn in (`None` for a device of
    /// the rack itself).
    pub(super) fn macro_container(&self, i: usize) -> Option<DeviceKey> {
        if !self.scope.is_empty() {
            return self.scope_path();
        }
        let key = &self.devices.get(i)?.key;
        super::inline::parent_of(key)
    }

    /// The names of the macros of the container device view `i` maps to (empty: unnamed).
    fn macro_names(&self, i: usize) -> [String; 8] {
        if !self.scope.is_empty() {
            return self.scope_macros.clone();
        }
        let container = self.macro_container(i);
        self.devices
            .iter()
            .find(|d| Some(&d.key) == container.as_ref())
            .and_then(|d| d.device.as_ref())
            .and_then(|d| d.inner.as_ref())
            .map(|inner| std::array::from_fn(|n| inner.macros[n].name.clone()))
            .unwrap_or_default()
    }

    fn apply_entry(&mut self, menu: &MacroMenu, entry: MenuEntry) {
        let Some(container) = self.macro_container(menu.dev) else {
            return;
        };
        let d = &self.devices[menu.dev];
        // A macro names its target relative to the container: the view key in a scope, the last
        // segment of the path key of an unfolded device.
        let device = if self.scope.is_empty() {
            atomik_model::split_path(&d.key).1
        } else {
            d.key.clone()
        };
        let param = menu.param.clone();
        let action = match entry {
            MenuEntry::Map(index) => {
                let Some(p) = d.desc.and_then(|desc| desc.param_by_key(&param).copied()) else {
                    return;
                };
                UiAction::Command(Command::Container(ContainerCommand::SetMacro {
                    container,
                    index,
                    target: MacroTarget {
                        device,
                        param,
                        min: p.min,
                        max: p.max,
                    },
                    merge: None,
                }))
            }
            MenuEntry::Unmap(index) => {
                UiAction::Command(Command::Container(ContainerCommand::RemoveMacroTarget {
                    container,
                    index,
                    device,
                    param,
                }))
            }
            MenuEntry::EditRange(index) => UiAction::OpenContainerEditor {
                device: container,
                macros: Some(index),
            },
        };
        self.direct.push(action);
    }

    pub(super) fn macro_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.macro_menu {
            let hover = Self::menu_row_at(menu, rp);
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// The rows of the open macro menu: (label, centre in logical view coordinates).
    pub fn macro_menu_rows(&self) -> Vec<(String, Point)> {
        let Some(m) = &self.macro_menu else {
            return Vec::new();
        };
        m.entries
            .iter()
            .enumerate()
            .map(|(i, (_, label))| {
                (
                    label.clone(),
                    self.rack_to_view(Self::menu_row(m.rect, i).center()),
                )
            })
            .collect()
    }

    /// Paints the open macro menu (rack coordinates).
    pub(super) fn paint_macro_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.macro_menu else { return };
        let theme = &self.theme;
        c.fill_with(RoundedRect::from_rect(m.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            theme.ink.with_alpha(0.14),
        );
        for (k, (_, label)) in m.entries.iter().enumerate() {
            let r = Self::menu_row(m.rect, k);
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
}

impl RackView {
    /// The workflow commands the rack view emits, with path keys.
    fn lift_workflow(
        &self,
        w: atomik_model::WorkflowCommand,
    ) -> Option<atomik_model::WorkflowCommand> {
        use atomik_model::WorkflowCommand as W;
        let k = |d: DeviceKey| self.up(&d);
        Some(match w {
            W::RemoveDevices { keys } => W::RemoveDevices {
                keys: keys.into_iter().map(k).collect(),
            },
            W::DuplicateDevices { keys, route } => W::DuplicateDevices {
                keys: keys.into_iter().map(k).collect(),
                route,
            },
            W::MoveDevices { keys, to, route } => W::MoveDevices {
                keys: keys.into_iter().map(k).collect(),
                to,
                route,
            },
            W::SetFoldedMany { keys, folded } => W::SetFoldedMany {
                keys: keys.into_iter().map(k).collect(),
                folded,
            },
            W::DeleteDevices { keys } => W::DeleteDevices {
                keys: keys.into_iter().map(k).collect(),
            },
            W::DisconnectDevice { key } => W::DisconnectDevice { key: k(key) },
            W::DisconnectDevices { keys } => W::DisconnectDevices {
                keys: keys.into_iter().map(k).collect(),
            },
            other => other,
        })
    }
}
