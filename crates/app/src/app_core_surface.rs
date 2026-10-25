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
    MasterState, PositionState, StripKind, StripState, SurfaceAction, SurfaceState,
    TransportButton, TransportLeds,
};
use atomik_ui::timeline::tick_to_seconds;

/// What a surface strip is in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StripRef {
    /// A channel or a bus strip, by its device key.
    Strip(DeviceKey),
    /// A VCA group, by id.
    Vca(u32),
}

impl<S: EngineSink> AppCore<S> {
    /// Sets up (or, with `None`, tears down) the control surface: the driver says hello and
    /// shows the project on the first frame; removing it lets the surface go offline.
    pub fn set_surface(&mut self, host: Option<SurfaceHost>) {
        if let Some(mut old) = self.surface.take() {
            let mut out = Vec::new();
            old.driver.disconnect(&mut out);
            old.send_all(&mut out);
        }
        self.surface = host;
    }

    /// The control surface in use, if any.
    pub fn surface(&self) -> Option<&SurfaceHost> {
        self.surface.as_ref()
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
            },
            refs,
        )
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

    /// One frame of the control surface (UI thread): the messages that arrived are handled, and
    /// the feedback goes out when it is due. `now` is a monotonic time in seconds.
    pub fn surface_frame(&mut self, now: f64) {
        let Some(mut host) = self.surface.take() else {
            return;
        };
        let (state, refs) = self.surface_view();
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
        for bytes in &incoming {
            host.received += 1;
            host.driver
                .on_message(bytes, &state, &mut actions, &mut replies);
        }
        host.send_all(&mut replies);
        self.midi.set_now(now);
        for action in actions {
            self.apply_surface_action(action, &refs);
        }
        if now >= host.next_feedback {
            // What the actions changed is what the surface shows next.
            let (state, _) = self.surface_view();
            host.driver.feedback(&state, &mut out);
            host.send_all(&mut out);
            host.next_feedback = now + crate::surface::FEEDBACK_PERIOD;
        }
        self.surface = Some(host);
        self.end_expired_gestures();
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

    fn apply_surface_action(&mut self, action: SurfaceAction, refs: &[StripRef]) {
        match action {
            SurfaceAction::Fader { strip, value } => {
                let Some(r) = refs.get(strip) else { return };
                let key = GestureKey::Surface(format!("fader:{strip}"));
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
                    let key = GestureKey::Surface("fader:master".to_owned());
                    let held = self.midi.is_held(&key);
                    self.surface_set(key, held, &r, value);
                }
            }
            SurfaceAction::Touch { strip, touched } => {
                let key = GestureKey::Surface(format!("fader:{strip}"));
                self.surface_touch(key, touched);
            }
            SurfaceAction::MasterTouch(touched) => {
                self.surface_touch(GestureKey::Surface("fader:master".to_owned()), touched);
            }
            SurfaceAction::Pan { strip, delta } => {
                let Some(r) = refs.get(strip) else { return };
                let Some(resolved) = self.strip_param(r, "pan") else {
                    return;
                };
                if let Some(now) = self.target_value(&resolved) {
                    let key = GestureKey::Surface(format!("pan:{strip}"));
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
                if let Some(t) = doc.song.track(&device) {
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
