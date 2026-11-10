//! A control surface in the app core: the strips of the console shown on its faders, and what
//! its keys and faders do to the project.
//!
//! [`AppCore::surface_state`] reads the document into the driver's [`SurfaceState`]: the strips
//! **in console order** (the channels, then the output buses, then the VCA groups, the order the
//! console view draws; the track folders of the arrangement do not reorder the console), their
//! names, fader positions (the position of the console's own fader, so the surface and the mouse
//! share one law), pans, mute, solo and record-ready flags, the selection, the master, the
//! transport lamps and the position. [`AppCore::surface_frame`] polls the link, hands each
//! message to the driver, applies the actions it returns as ordinary commands and sends the
//! driver's feedback, at most thirty times a second.
//!
//! **Actions are edits.** A fader move is a `SetParam` of the `fader` (a VCA's is its level) with
//! the merge id of the finger: touching a fader opens the gesture and releasing it closes it, so
//! the move is one undo step and a Touch lane of automation recording writes exactly while the
//! finger rests on it; a surface without touch sensors times out like a mapped controller
//! ([`crate::GESTURE_TIMEOUT`]). A V-pot turns the pan, the buttons flip mute, solo and the
//! track's arm, select selects the strip in the console (what the console does when clicked),
//! the transport keys are the transport buttons, rewind and fast forward move a bar, and the jog
//! wheel moves the playhead a sixteenth per detent.

use super::*;
use crate::app_core::midimap::{GestureKey, Resolved};
use crate::surface::SurfaceHost;
use atomik_core::PPQ;
use atomik_midi_io::surface::{
    KnobMove, MasterState, PositionState, RemoteDevice, StripKind, StripState, SurfaceAction,
    SurfaceDriver, SurfaceState, TransportButton, TransportLeds,
};
use atomik_ui::timeline::tick_to_seconds;

/// How many remote knobs a surface has: a page of the parameters of the device the surface
/// follows.
pub const REMOTE_KNOBS: usize = crate::controller_profile::KNOBS as usize;
/// The most parameters of one device the remote knobs reach (sixteen pages).
pub const MAX_REMOTE_PARAMS: usize = 16 * REMOTE_KNOBS;

/// True for a parameter the remote knobs may address.
fn is_remote_param(key: &str) -> bool {
    !atomik_model::console::is_hidden_param(key) && key != "bypass" && !key.starts_with("cv.")
}

/// What a surface strip is in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StripRef {
    /// A channel or a bus strip, by its device key.
    Strip(DeviceKey),
    /// A VCA group, by id.
    Vca(u32),
}

impl<S: EngineSink> AppCore<S> {
    /// Sets up (or, with `None`, tears down) the one control surface: the driver says hello and
    /// shows the project on the first frame; removing it lets the surface go offline. See
    /// [`Self::set_surfaces`] for several.
    pub fn set_surface(&mut self, host: Option<SurfaceHost>) {
        self.set_surfaces(host.into_iter().collect());
    }

    /// Sets up the control surfaces `hosts` (each runs its own driver on its own ports) and lets
    /// the ones that were set up before go offline. A lock stays only for a surface that is
    /// still there.
    pub fn set_surfaces(&mut self, hosts: Vec<SurfaceHost>) {
        for mut old in std::mem::take(&mut self.surfaces) {
            let mut out = Vec::new();
            old.driver.disconnect(&mut out);
            old.send_all(&mut out);
        }
        self.midi
            .locks
            .retain(|id, _| hosts.iter().any(|h| h.id == *id));
        self.surfaces = hosts;
    }

    /// Sets up the driver `driver` on an in-memory surface and returns the surface's end of it
    /// (what a test or the MCP server plays the hardware with). It replaces every surface; the
    /// surface is named after the driver.
    ///
    /// # Errors
    /// A message when no driver has that id.
    pub fn attach_loopback_surface(
        &mut self,
        driver: &str,
    ) -> Result<crate::surface::LoopbackSurface, String> {
        self.set_surfaces(Vec::new());
        self.add_loopback_surface(driver, driver)
    }

    /// Adds an in-memory surface named `name` with the driver `driver` beside the surfaces that
    /// are set up (one with the same name is replaced) and returns the surface's end of it.
    ///
    /// # Errors
    /// A message when no driver has that id.
    pub fn add_loopback_surface(
        &mut self,
        name: &str,
        driver: &str,
    ) -> Result<crate::surface::LoopbackSurface, String> {
        let d = atomik_midi_io::surface::driver_by_id(driver)
            .ok_or_else(|| format!("no control surface driver {driver:?}"))?;
        Ok(self.add_loopback_driver(name, d))
    }

    /// Adds an in-memory surface named `name` run by `d` beside the surfaces that are set up (one
    /// with the same name is replaced) and returns the surface's end of it.
    pub fn add_loopback_driver(
        &mut self,
        name: &str,
        d: Box<dyn SurfaceDriver>,
    ) -> crate::surface::LoopbackSurface {
        let surface = crate::surface::LoopbackSurface::new();
        let host = SurfaceHost::new(d, Box::new(surface.clone())).named(name, name);
        let mut hosts = std::mem::take(&mut self.surfaces);
        match hosts.iter().position(|h| h.id == name) {
            Some(i) => {
                let mut old = std::mem::replace(&mut hosts[i], host);
                let mut out = Vec::new();
                old.driver.disconnect(&mut out);
                old.send_all(&mut out);
            }
            None => hosts.push(host),
        }
        self.surfaces = hosts;
        surface
    }

    /// The first control surface, if any is set up.
    pub fn surface(&self) -> Option<&SurfaceHost> {
        self.surfaces.first()
    }

    /// Every control surface that is set up, in settings order.
    pub fn surfaces(&self) -> &[SurfaceHost] {
        &self.surfaces
    }

    /// The device the surface `name` is locked to, if it is.
    pub fn surface_lock(&self, name: &str) -> Option<&DeviceKey> {
        self.midi.locks.get(name)
    }

    /// Locks the surface `name` to `device` (`None` unlocks it): the surface's mappings that
    /// address the focused device then address that device whatever is selected in the rack.
    /// False, with a message, when no surface has that name or the device does not exist.
    pub fn lock_surface(&mut self, name: &str, device: Option<DeviceKey>) -> bool {
        if !self.surfaces.iter().any(|h| h.id == name) {
            self.report(format!("There is no control surface named {name:?}"));
            return false;
        }
        match device {
            Some(device) => {
                if self.doc().resolve(&device).is_none() {
                    self.report(format!("There is no device {device}"));
                    return false;
                }
                let label = self
                    .doc()
                    .device(&device)
                    .map_or_else(|| device.0.clone(), |d| d.name.clone());
                self.midi.locks.insert(name.to_owned(), device);
                self.report(format!("{name} is locked to {label}"));
            }
            None => {
                if self.midi.locks.remove(name).is_some() {
                    self.report(format!("{name} follows the selected device"));
                }
            }
        }
        true
    }

    /// The Lock Surfaces to Selection item: every surface is unlocked when any is locked, else
    /// every surface is locked to the device selected in the rack. False, with a message, when
    /// nothing could change.
    pub fn toggle_surface_locks(&mut self) -> bool {
        if self.surfaces.is_empty() {
            self.report("No control surface is set up (Settings > Surfaces)");
            return false;
        }
        if !self.midi.locks.is_empty() {
            self.midi.locks.clear();
            self.report("The surfaces follow the selected device");
            return true;
        }
        let Some(device) = self.view.primary() else {
            self.report("Select a device to lock the surfaces to");
            return false;
        };
        let names: Vec<String> = self.surfaces.iter().map(|h| h.id.clone()).collect();
        for name in names {
            self.midi.locks.insert(name, device.clone());
        }
        let label = self
            .doc()
            .device(&device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        self.report(format!("The surfaces are locked to {label}"));
        true
    }

    /// Locks the surface `name` to the device selected in the rack, or unlocks it when it is
    /// locked already. False, with a message, when it cannot.
    pub fn toggle_surface_lock(&mut self, name: &str) -> bool {
        if self.midi.locks.contains_key(name) {
            return self.lock_surface(name, None);
        }
        match self.view.primary() {
            Some(device) => self.lock_surface(name, Some(device)),
            None => {
                self.report("Select a device to lock the surface to");
                false
            }
        }
    }

    /// The surface's view of the project, with what each strip stands for.
    pub(super) fn surface_view(&self) -> (SurfaceState, Vec<StripRef>) {
        let doc = self.doc();
        let console = &doc.console;
        let reg = &self.registry;
        let selected = self.console.selected_strips();
        let norm = |key: &DeviceKey, p: &str| -> f32 {
            let value = doc.param_value(key, p, reg);
            let desc = doc
                .resolve(key)
                .and_then(|d| reg.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(p));
            match (value, desc) {
                (Some(v), Some(d)) => d.to_normalized(v),
                _ => 0.0,
            }
        };
        let flag =
            |key: &DeviceKey, p: &str| doc.param_value(key, p, reg).is_some_and(|v| v >= 0.5);
        let name_of = |key: &DeviceKey| {
            doc.device(key)
                .map_or_else(|| key.0.clone(), |d| d.name.clone())
        };
        let mut strips = Vec::new();
        let mut refs = Vec::new();
        let channel_strip = |key: &DeviceKey, name: String, kind: StripKind, armed: bool| {
            let pan = doc.param_value(key, "pan", reg);
            let fader_db = doc.effective_fader_db(key, reg);
            StripState {
                name,
                kind,
                fader: norm(key, "fader"),
                fader_text: db_text(fader_db),
                pan,
                pan_text: pan.map_or_else(String::new, pan_text),
                mute: flag(key, "mute"),
                solo: flag(key, "solo"),
                armed,
                selected: selected.contains(key),
            }
        };
        for c in &console.channels {
            let name = c
                .source
                .as_ref()
                .map_or_else(|| name_of(&c.channel), name_of);
            let armed = c
                .source
                .as_ref()
                .and_then(|s| doc.song.track(s))
                .is_some_and(|t| t.armed);
            strips.push(channel_strip(&c.channel, name, StripKind::Channel, armed));
            refs.push(StripRef::Strip(c.channel.clone()));
        }
        for b in &console.buses {
            strips.push(channel_strip(&b.bus, b.name.clone(), StripKind::Bus, false));
            refs.push(StripRef::Strip(b.bus.clone()));
        }
        for v in &console.vcas {
            let fader = ((v.level_db - atomik_core::VCA_MIN_DB)
                / (atomik_core::VCA_MAX_DB - atomik_core::VCA_MIN_DB))
                .clamp(0.0, 1.0);
            strips.push(StripState {
                name: v.name.clone(),
                kind: StripKind::Vca,
                fader,
                fader_text: db_text(Some(v.level_db)),
                pan: None,
                pan_text: String::new(),
                mute: v.mute,
                solo: v.solo,
                armed: false,
                selected: !v.members.is_empty() && v.members.iter().all(|m| selected.contains(m)),
            });
            refs.push(StripRef::Vca(v.id));
        }
        let master = console.master.as_ref().map(|m| MasterState {
            fader: norm(m, "fader"),
        });
        let position = self.surface_position();
        (
            SurfaceState {
                strips,
                master,
                transport: TransportLeds {
                    playing: self.playing,
                    recording: self.take.is_some(),
                    looping: doc.song.loop_enabled,
                },
                position,
                knobs: Vec::new(),
                remote: RemoteDevice::default(),
            },
            refs,
        )
    }

    /// The surface `id`'s view of the project: [`Self::surface_view`] plus the value of every
    /// remote parameter of the device the surface follows (the driver shows them a page at a
    /// time).
    pub(super) fn surface_view_of(&self, id: &str) -> (SurfaceState, Vec<StripRef>) {
        let (mut state, refs) = self.surface_view();
        (state.knobs, state.remote) = self.remote_knobs(id);
        (state, refs)
    }

    /// The device the surface `id` follows: the one it is locked to, else the device selected in
    /// the rack (the primary one first). `None` when there is none.
    pub fn followed_device(&self, id: &str) -> Option<DeviceKey> {
        let fits = |key: &DeviceKey| self.doc().resolve(key).is_some();
        if let Some(lock) = self.midi.locks.get(id) {
            return fits(lock).then(|| lock.clone());
        }
        self.view
            .primary()
            .into_iter()
            .chain(self.view.selection())
            .find(|k| fits(k))
    }

    /// The remote parameter `knob` (0-based) of `device`: the knobs address the parameters of the
    /// device in descriptor order (a page of [`REMOTE_KNOBS`] at a time), leaving out the hidden
    /// ones, the plugin CV rows and the bypass switch.
    pub(super) fn remote_param(&self, device: &DeviceKey, knob: usize) -> Option<Resolved> {
        let d = self.doc().resolve(device)?;
        let desc = self
            .registry
            .get(&d.type_id)?
            .descriptor
            .params
            .iter()
            .filter(|p| is_remote_param(p.key))
            .nth(knob)?;
        Some(Resolved::Param {
            key: device.clone(),
            desc,
        })
    }

    /// The value of every remote parameter of the device the surface `id` follows, and what the
    /// driver needs to know about the device (see [`RemoteDevice`]).
    fn remote_knobs(&self, id: &str) -> (Vec<Option<f32>>, RemoteDevice) {
        let Some(device) = self.followed_device(id) else {
            return (Vec::new(), RemoteDevice::default());
        };
        let Some(d) = self.doc().resolve(&device) else {
            return (Vec::new(), RemoteDevice::default());
        };
        let Some(entry) = self.registry.get(&d.type_id) else {
            return (Vec::new(), RemoteDevice::default());
        };
        let params: Vec<_> = entry
            .descriptor
            .params
            .iter()
            .filter(|p| is_remote_param(p.key))
            .take(MAX_REMOTE_PARAMS)
            .collect();
        let values = params
            .iter()
            .map(|desc| {
                let r = Resolved::Param {
                    key: device.clone(),
                    desc,
                };
                self.target_value(&r)
            })
            .collect();
        let remote = RemoteDevice {
            id: Some(device.0.clone()),
            name: d.name.clone(),
            params: params.iter().map(|p| p.name.to_owned()).collect(),
            stepped: params.iter().map(|p| p.discrete).collect(),
        };
        (values, remote)
    }

    /// Moves the knobs of every control surface `delta` pages on (the next page when positive,
    /// the previous when negative, wrapping round) and shows where they are in the status line.
    /// A surface with no knob pages ignores it. The surface shows the new values at its next
    /// feedback pass.
    pub fn knob_page(&mut self, delta: i32) {
        let mut hosts = std::mem::take(&mut self.surfaces);
        for host in &mut hosts {
            let (state, refs) = self.surface_view_of(&host.id);
            let mut actions = Vec::new();
            host.driver.knob_page(delta, &state, &mut actions);
            for action in actions {
                self.apply_surface_action(action, &refs, &host.id);
            }
            host.next_feedback = 0.0;
        }
        self.surfaces = hosts;
    }

    fn surface_knob(&mut self, id: &str, knob: usize, change: KnobMove) {
        let Some(device) = self.followed_device(id) else {
            return;
        };
        let Some(r) = self.remote_param(&device, knob) else {
            return;
        };
        let norm = match change {
            KnobMove::To(v) => v,
            KnobMove::By(delta) => {
                let Some(now) = self.target_value(&r) else {
                    return;
                };
                // A stepped parameter moves one step per click of the encoder.
                let delta = match &r {
                    Resolved::Param { desc, .. } if desc.discrete && delta != 0.0 => {
                        delta.signum() * delta.abs().max(1.0 / (desc.max - desc.min).max(1.0))
                    }
                    _ => delta,
                };
                now + delta
            }
        };
        let key = GestureKey::Surface(format!("{id}:knob:{knob}"));
        self.surface_set(key, false, &r, norm.clamp(0.0, 1.0));
    }

    fn surface_position(&self) -> PositionState {
        let song = &self.doc().song;
        let tick = self.position.max(0);
        let table = song.meter_table();
        let (bar, beat, in_beat) = table.map().bbt(tick);
        let sixteenth_ticks = (table.map().beat_ticks_at(tick) / 4).max(1);
        let sixteenth = in_beat as i64 / sixteenth_ticks;
        let rest = in_beat as i64 % sixteenth_ticks;
        PositionState {
            bar,
            beat,
            sixteenth: sixteenth as u32 + 1,
            ticks: (rest * 240 / sixteenth_ticks) as u32,
            seconds: tick_to_seconds(tick, &song.tempo),
        }
    }

    /// One frame of the control surfaces (UI thread): the messages that arrived are handled, and
    /// the feedback goes out when it is due. `now` is a monotonic time in seconds. Each surface
    /// is handled in settings order; the messages of a generic controller go to the mappings
    /// tagged with the surface, the actions of a console surface are applied as commands.
    pub fn surface_frame(&mut self, now: f64) {
        if self.surfaces.is_empty() {
            return;
        }
        let mut hosts = std::mem::take(&mut self.surfaces);
        self.midi.set_now(now);
        for host in &mut hosts {
            self.surface_host_frame(host, now);
        }
        self.surfaces = hosts;
        self.end_expired_gestures();
    }

    fn surface_host_frame(&mut self, host: &mut SurfaceHost, now: f64) {
        let (state, refs) = self.surface_view_of(&host.id);
        let mut out = Vec::new();
        if !host.connected {
            host.connected = true;
            host.driver.connect(&state, &mut out);
            host.next_feedback = now + crate::surface::FEEDBACK_PERIOD;
            host.send_all(&mut out);
        }
        let mut incoming = Vec::new();
        host.link.poll(&mut incoming);
        let mut actions = Vec::new();
        let mut replies = Vec::new();
        let mut mapped = Vec::new();
        let feeds_mappings = host.driver.feeds_mappings();
        for bytes in &incoming {
            host.received += 1;
            if feeds_mappings {
                // A controller of the mappings: what it sends is a note, a controller or a pitch
                // bend (the rest, SysEx and the like, is not a control).
                mapped.extend(atomik_midi_io::parse_message(bytes, &mut host.parser));
            } else {
                host.driver
                    .on_message(bytes, &state, &mut actions, &mut replies);
            }
        }
        host.send_all(&mut replies);
        for message in mapped {
            self.surface_message(&host.port, &host.id, message);
        }
        for action in actions {
            self.apply_surface_action(action, &refs, &host.id);
        }
        if now >= host.next_feedback {
            // What the actions changed is what the surface shows next.
            let (state, _) = self.surface_view_of(&host.id);
            host.driver.feedback(&state, &mut out);
            host.send_all(&mut out);
            host.next_feedback = now + crate::surface::FEEDBACK_PERIOD;
        }
    }

    fn strip_param(&self, r: &StripRef, param: &str) -> Option<Resolved> {
        match r {
            StripRef::Strip(key) => self.resolve_param(key, param),
            StripRef::Vca(_) => None,
        }
    }

    fn surface_set(&mut self, key: GestureKey, held: bool, r: &Resolved, norm: f32) {
        let merge = self.midi.gesture(key, held);
        self.set_resolved(r, norm, Some(merge));
    }

    fn apply_surface_action(&mut self, action: SurfaceAction, refs: &[StripRef], id: &str) {
        match action {
            SurfaceAction::Fader { strip, value } => {
                let Some(r) = refs.get(strip) else { return };
                let key = GestureKey::Surface(format!("{id}:fader:{strip}"));
                let held = self.midi.is_held(&key);
                let resolved = match r {
                    StripRef::Strip(_) => self.strip_param(r, "fader"),
                    StripRef::Vca(id) => Some(Resolved::Vca(*id)),
                };
                if let Some(resolved) = resolved {
                    self.surface_set(key, held, &resolved, value);
                }
            }
            SurfaceAction::MasterFader(value) => {
                let Some(master) = self.doc().console.master.clone() else {
                    return;
                };
                if let Some(r) = self.resolve_param(&master, "fader") {
                    let key = GestureKey::Surface(format!("{id}:fader:master"));
                    let held = self.midi.is_held(&key);
                    self.surface_set(key, held, &r, value);
                }
            }
            SurfaceAction::Touch { strip, touched } => {
                let key = GestureKey::Surface(format!("{id}:fader:{strip}"));
                self.surface_touch(key, touched);
            }
            SurfaceAction::MasterTouch(touched) => {
                self.surface_touch(GestureKey::Surface(format!("{id}:fader:master")), touched);
            }
            SurfaceAction::Pan { strip, delta } => {
                let Some(r) = refs.get(strip) else { return };
                let Some(resolved) = self.strip_param(r, "pan") else {
                    return;
                };
                if let Some(now) = self.target_value(&resolved) {
                    let key = GestureKey::Surface(format!("{id}:pan:{strip}"));
                    self.surface_set(key, false, &resolved, (now + delta).clamp(0.0, 1.0));
                }
            }
            SurfaceAction::ToggleMute(strip) => self.surface_flag(refs.get(strip), "mute"),
            SurfaceAction::ToggleSolo(strip) => self.surface_flag(refs.get(strip), "solo"),
            SurfaceAction::ToggleArm(strip) => {
                let Some(StripRef::Strip(key)) = refs.get(strip) else {
                    return;
                };
                let doc = self.doc();
                let Some(device) = doc
                    .console
                    .channel_index(key)
                    .and_then(|i| doc.console.channels[i].source.clone())
                else {
                    return;
                };
                if let Some(t) = doc.song.track(&device).filter(|t| !t.is_automation_only()) {
                    let command = SongCommand::SetTrackFlags {
                        device,
                        muted: t.muted,
                        solo: t.solo,
                        armed: !t.armed,
                    };
                    let _ = self.apply_command(Command::Song(command));
                }
            }
            SurfaceAction::Select(strip) => {
                let keys: Vec<DeviceKey> = match refs.get(strip) {
                    Some(StripRef::Strip(k)) => vec![k.clone()],
                    Some(StripRef::Vca(id)) => self
                        .doc()
                        .console
                        .vca(*id)
                        .map(|v| v.members.clone())
                        .unwrap_or_default(),
                    None => return,
                };
                self.console.set_selected_strips(&keys);
                self.follow_console_selection();
                self.after_view_change();
            }
            SurfaceAction::Transport(button) => self.surface_transport(button),
            SurfaceAction::Knob { knob, change } => self.surface_knob(id, knob, change),
            SurfaceAction::Notice(message) => self.reject(message),
            SurfaceAction::Jog(detents) => {
                let step = PPQ / 4;
                let tick = (self.position + i64::from(detents) * step).max(0);
                self.transport_action(TransportAction::Locate(tick));
            }
        }
    }

    fn surface_touch(&mut self, key: GestureKey, touched: bool) {
        if touched {
            let _ = self.midi.gesture(key, true);
        } else if self.midi.close_gesture(&key) {
            self.document.end_merge();
            self.release_automation();
        }
    }

    /// Flips a switch param of a strip (a VCA's mute and solo are its own commands).
    fn surface_flag(&mut self, strip: Option<&StripRef>, param: &str) {
        match strip {
            Some(StripRef::Strip(key)) => {
                let Some(r) = self.resolve_param(key, param) else {
                    return;
                };
                let on = self.target_value(&r).is_some_and(|v| v >= 0.5);
                self.set_resolved(&r, if on { 0.0 } else { 1.0 }, None);
            }
            Some(StripRef::Vca(id)) => {
                let Some(v) = self.doc().console.vca(*id) else {
                    return;
                };
                let command = if param == "mute" {
                    atomik_model::ConsoleCommand::SetVcaMute {
                        vca: *id,
                        on: !v.mute,
                    }
                } else {
                    atomik_model::ConsoleCommand::SetVcaSolo {
                        vca: *id,
                        on: !v.solo,
                    }
                };
                let _ = self.apply_command(Command::Console(command));
            }
            None => {}
        }
    }

    fn surface_transport(&mut self, button: TransportButton) {
        match button {
            TransportButton::Play => self.transport_action(TransportAction::TogglePlay),
            TransportButton::Stop => self.transport_action(TransportAction::Stop),
            TransportButton::Record => self.transport_action(TransportAction::ToggleRecord),
            TransportButton::Cycle => self.transport_action(TransportAction::ToggleLoop),
            TransportButton::Rewind | TransportButton::FastForward => {
                let table = self.doc().song.meter_table();
                let map = table.map();
                let here = self.position.max(0);
                let to = if button == TransportButton::Rewind {
                    map.prev_bar_line(here)
                } else {
                    map.next_bar_line(here)
                };
                self.transport_action(TransportAction::Locate(to));
            }
        }
    }
}

/// A level as text: `-6.0dB`, `-inf`.
fn db_text(db: Option<f32>) -> String {
    match db {
        Some(d) if d.is_finite() => format!("{d:.1}dB"),
        Some(_) => "-inf".to_owned(),
        None => String::new(),
    }
}

/// A pan as text: `C`, `L20`, `R35` (percent of the way to the side).
fn pan_text(pan: f32) -> String {
    let pct = (pan.abs() * 100.0).round() as i32;
    match pan {
        _ if pct == 0 => "C".to_owned(),
        p if p < 0.0 => format!("L{pct}"),
        _ => format!("R{pct}"),
    }
}

#[cfg(test)]
#[path = "app_core_surface_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "app_core_surfaces_tests.rs"]
mod surfaces_tests;

#[cfg(test)]
#[path = "app_core_controller_tests.rs"]
mod controller_tests;
