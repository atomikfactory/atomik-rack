//! The OSC surface: the control-surface model spoken in OSC messages, for a tablet app or other
//! software. It is a driver like the others: what it reads and writes are packets (the bytes of
//! a UDP datagram), and the app owns the socket (`atomik_app::surface::OscLink`).
//!
//! Strips are numbered from 1, **in console order** (channels, then output buses, then VCA
//! groups), with no bank: an OSC client addresses any strip. A switch argument is an integer, a
//! float or `T` / `F` (above one half is on); a switch message with no argument toggles. A fader
//! or pan argument is a float.
//!
//! | Address | Arguments | Direction | Meaning |
//! |---|---|---|---|
//! | `/strip/<n>/fader` | float 0..1 | both | the fader (the position of the console's fader) |
//! | `/strip/<n>/touch` | switch | in | a finger on the fader: starts and ends one undo step |
//! | `/strip/<n>/mute`, `solo`, `arm`, `select` | switch | both | the strip's switches |
//! | `/strip/<n>/pan` | float -1..1 | both | the pan (strips that have one) |
//! | `/strip/<n>/send/<k>` | float 0..1 | in | the level of send `k` (1 to 8) |
//! | `/strip/<n>/name` | string | out | the strip's name |
//! | `/strip/<n>/meter` | float 0..1 | out | the level meter (1 is full scale) |
//! | `/master/fader` | float 0..1 | both | the master fader |
//! | `/master/touch` | switch | in | a finger on the master fader |
//! | `/transport/play`, `stop`, `record`, `rewind`, `forward`, `cycle` | switch or none | both | the transport keys (feedback: `play`, `record` and `cycle` as 0 or 1) |
//! | `/transport/position` | string | out | `bar.beat.sixteenth` |
//! | `/jog` | integer | in | moves the playhead by that many sixteenths |
//! | `/function/<n>` | none | in | function key `n` (1 to 8), the command the user bound to it |
//! | `/device/param/<n>` | float 0..1 | both | the `n`th parameter of the device the surface follows |
//! | `/device/param/<n>/name` | string | out | its name |
//! | `/device/name` | string | out | the device's name (empty when none is followed) |
//! | `/surface/refresh` | none | in | sends everything again |
//! | `/ping` | any | in | answered with `/pong` and the same arguments |
//!
//! Feedback goes out like the other drivers': only what changed since the last pass, packed into
//! bundles that fit one datagram. The link adds the message `/surface/hello` to what it reads
//! when a new client sends its first packet, so a client that arrives late is shown everything.

use super::osc_wire::{self, Arg, Message};
use super::{
    KnobMove, Needs, StripControl, SurfaceAction, SurfaceDriver, SurfaceState, TransportButton,
};
use std::collections::BTreeMap;

/// The most bytes of a bundle of feedback (a datagram that fits an Ethernet frame).
const PACKET_LIMIT: usize = 1200;
/// The most remote parameters sent as feedback.
const MAX_PARAMS: usize = 128;

/// What was last sent for an address.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Int(i32),
    Float(i32),
    Text(String),
}

/// The driver of an OSC client.
#[derive(Debug, Clone, Default)]
pub struct Osc {
    sent: BTreeMap<String, Shown>,
}

impl Osc {
    /// An OSC driver that has shown nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn handle(
        &mut self,
        m: &Message,
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        replies: &mut Vec<Vec<u8>>,
    ) {
        let parts: Vec<&str> = m.address.split('/').skip(1).collect();
        let arg = m.args.first();
        let float = arg.and_then(Arg::as_f32);
        // `Some(true)` for a press (no argument, or on), `Some(false)` for a release.
        let pressed = arg.map_or(Some(true), Arg::as_bool);
        match parts.as_slice() {
            ["strip", n, rest @ ..] => {
                let Some(strip) = index(n, state.strips.len()) else {
                    return;
                };
                self.strip_message(strip, rest, arg, float, state, actions);
            }
            ["master", "fader"] => {
                if let Some(v) = float {
                    actions.push(SurfaceAction::MasterFader(v.clamp(0.0, 1.0)));
                }
            }
            ["master", "touch"] => {
                if let Some(on) = pressed {
                    actions.push(SurfaceAction::MasterTouch(on));
                }
            }
            ["transport", key] => transport(key, arg, state, actions),
            ["jog"] => {
                if let Some(v) = float.filter(|v| v.is_finite()) {
                    actions.push(SurfaceAction::Jog(v as i32));
                }
            }
            ["function", n] => {
                if let Some(k) = n.parse::<u8>().ok().filter(|k| (1..=8).contains(k)) {
                    actions.push(SurfaceAction::Function(k));
                }
            }
            ["device", "param", n] => {
                if let (Some(v), Some(knob)) = (float, index(n, usize::MAX)) {
                    actions.push(SurfaceAction::Knob {
                        knob,
                        change: KnobMove::To(v.clamp(0.0, 1.0)),
                    });
                }
            }
            ["surface", "refresh" | "hello"] => self.sent.clear(),
            ["ping"] => replies.push(osc_wire::encode(&Message {
                address: "/pong".to_owned(),
                args: m.args.clone(),
            })),
            _ => {}
        }
    }

    fn strip_message(
        &mut self,
        strip: usize,
        rest: &[&str],
        arg: Option<&Arg>,
        float: Option<f32>,
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
    ) {
        let s = &state.strips[strip];
        // A switch: set it to what the message says, toggling when it says nothing.
        let flip = |now: bool| match arg.map(Arg::as_bool) {
            None => true,
            Some(want) => want.is_some_and(|w| w != now),
        };
        match rest {
            ["fader"] => {
                if let Some(value) = float {
                    actions.push(SurfaceAction::Fader {
                        strip,
                        value: value.clamp(0.0, 1.0),
                    });
                }
            }
            ["touch"] => {
                if let Some(touched) = arg.map_or(Some(true), Arg::as_bool) {
                    actions.push(SurfaceAction::Touch { strip, touched });
                }
            }
            ["mute"] if flip(s.mute) => actions.push(SurfaceAction::ToggleMute(strip)),
            ["solo"] if flip(s.solo) => actions.push(SurfaceAction::ToggleSolo(strip)),
            ["arm"] if flip(s.armed) => actions.push(SurfaceAction::ToggleArm(strip)),
            ["select"] if arg.is_none_or(|a| a.as_bool() == Some(true)) => {
                actions.push(SurfaceAction::Select(strip));
            }
            ["pan"] => {
                if let Some(pan) = float {
                    actions.push(SurfaceAction::Strip {
                        strip,
                        control: StripControl::Pan,
                        change: KnobMove::To((pan.clamp(-1.0, 1.0) + 1.0) / 2.0),
                    });
                }
            }
            ["send", k] => {
                let slot = k.parse::<u8>().ok().filter(|k| (1..=8).contains(k));
                if let (Some(value), Some(slot)) = (float, slot) {
                    actions.push(SurfaceAction::Strip {
                        strip,
                        control: StripControl::Send(slot),
                        change: KnobMove::To(value.clamp(0.0, 1.0)),
                    });
                }
            }
            _ => {}
        }
    }

    /// Every value the client should show for `state`, with its address.
    fn picture(state: &SurfaceState) -> Vec<(String, Arg)> {
        let mut all = Vec::with_capacity(state.strips.len() * 8 + 16);
        let mut add = |address: String, arg: Arg| all.push((address, arg));
        for (i, s) in state.strips.iter().enumerate() {
            let n = i + 1;
            add(format!("/strip/{n}/name"), Arg::Str(s.name.clone()));
            add(format!("/strip/{n}/fader"), Arg::Float(s.fader));
            add(format!("/strip/{n}/mute"), Arg::Int(i32::from(s.mute)));
            add(format!("/strip/{n}/solo"), Arg::Int(i32::from(s.solo)));
            add(format!("/strip/{n}/arm"), Arg::Int(i32::from(s.armed)));
            add(
                format!("/strip/{n}/select"),
                Arg::Int(i32::from(s.selected)),
            );
            if let Some(pan) = s.pan {
                add(format!("/strip/{n}/pan"), Arg::Float(pan));
            }
            add(format!("/strip/{n}/meter"), Arg::Float(s.meter));
        }
        if let Some(m) = &state.master {
            add("/master/fader".to_owned(), Arg::Float(m.fader));
        }
        let t = &state.transport;
        add("/transport/play".to_owned(), Arg::Int(i32::from(t.playing)));
        add(
            "/transport/record".to_owned(),
            Arg::Int(i32::from(t.recording)),
        );
        add(
            "/transport/cycle".to_owned(),
            Arg::Int(i32::from(t.looping)),
        );
        let p = &state.position;
        add(
            "/transport/position".to_owned(),
            Arg::Str(format!("{}.{}.{}", p.bar, p.beat, p.sixteenth)),
        );
        add(
            "/device/name".to_owned(),
            Arg::Str(state.remote.name.clone()),
        );
        for (i, value) in state.knobs.iter().take(MAX_PARAMS).enumerate() {
            let n = i + 1;
            add(
                format!("/device/param/{n}"),
                Arg::Float(value.unwrap_or(0.0)),
            );
            let name = state.remote.params.get(i).cloned().unwrap_or_default();
            add(format!("/device/param/{n}/name"), Arg::Str(name));
        }
        all
    }
}

/// The 0-based index of the 1-based number `text`, when it is below `count`.
fn index(text: &str, count: usize) -> Option<usize> {
    let n: usize = text.parse().ok()?;
    (1..=count).contains(&n).then(|| n - 1)
}

fn transport(key: &str, arg: Option<&Arg>, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
    let on = arg.map_or(Some(true), Arg::as_bool);
    let Some(on) = on else { return };
    let t = &state.transport;
    // A key with an argument says what the switch should be, so a client that repeats itself
    // does not flip it back.
    let button = match key {
        "play" => (arg.is_none() || on != t.playing).then_some(TransportButton::Play),
        "record" => (arg.is_none() || on != t.recording).then_some(TransportButton::Record),
        "cycle" => (arg.is_none() || on != t.looping).then_some(TransportButton::Cycle),
        "stop" => on.then_some(TransportButton::Stop),
        "rewind" => on.then_some(TransportButton::Rewind),
        "forward" => on.then_some(TransportButton::FastForward),
        _ => None,
    };
    actions.extend(button.map(SurfaceAction::Transport));
}

/// The quantised float that is compared to know whether a value changed.
fn quantise(v: f32) -> i32 {
    (v * 10_000.0).round() as i32
}

fn shown(arg: &Arg) -> Shown {
    match arg {
        Arg::Float(v) => Shown::Float(quantise(*v)),
        Arg::Int(v) => Shown::Int(*v),
        Arg::Str(s) => Shown::Text(s.clone()),
        _ => Shown::Int(0),
    }
}

impl SurfaceDriver for Osc {
    fn name(&self) -> &'static str {
        "osc"
    }

    fn needs(&self) -> Needs {
        Needs {
            meters: true,
            send: None,
        }
    }

    fn connect(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        self.sent.clear();
        self.feedback(state, out);
    }

    fn on_message(
        &mut self,
        bytes: &[u8],
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        replies: &mut Vec<Vec<u8>>,
    ) {
        let mut messages = Vec::new();
        // A damaged packet gives the messages before the fault, and nothing more.
        let _ = osc_wire::decode(bytes, &mut messages);
        for m in &messages {
            self.handle(m, state, actions, replies);
        }
    }

    fn feedback(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        let mut changed = Vec::new();
        let mut live = BTreeMap::new();
        for (address, arg) in Self::picture(state) {
            let value = shown(&arg);
            if self.sent.get(&address) != Some(&value) {
                changed.push(Message {
                    address: address.clone(),
                    args: vec![arg],
                });
            }
            live.insert(address, value);
        }
        self.sent = live;
        out.extend(osc_wire::pack(&changed, PACKET_LIMIT));
    }

    fn disconnect(&mut self, _out: &mut Vec<Vec<u8>>) {
        self.sent.clear();
    }
}

#[cfg(test)]
#[path = "osc_tests.rs"]
mod tests;
