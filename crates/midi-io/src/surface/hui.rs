//! The older hardware-user-interface protocol of eight motor faders (HUI-compatible), as a
//! driver. It is a protocol of its own, not a variant of the eight-fader driver in [`super::mcu`]:
//! a switch is a pair of controller messages (a zone, then a port), the faders are pairs of
//! controllers, and the displays and meters are SysEx. All on MIDI channel 1.
//!
//! What this driver speaks (the layout the protocol's public descriptions give; check a unit
//! that behaves differently against the constants below):
//!
//! | Direction | Message | Meaning |
//! |---|---|---|
//! | in | note on `00` velocity 0 | the surface's ping: answered with `90 00 7F` |
//! | in | CC `0F zone`, then CC `2F port` (bit 6 set on press) | a switch: zone `0..=7` is a strip, then the port below |
//! | out | CC `0C zone`, then CC `2C port` (bit 6 set for on) | a switch's lamp |
//! | in and out | CC `n`, `20 + n` (`n` in `0..=7`) | fader `n`: the upper then the lower 7 bits of 14 |
//! | in | CC `40 + n` | V-pot `n`, relative: bit 6 is the sign, bits 0 to 5 the detents |
//! | out | CC `10 + n` | the V-pot ring of strip `n`, as the eight-fader driver's |
//! | in | CC `0D` | the jog wheel, relative as the V-pots |
//! | out | SysEx `F0 00 00 66 05 00 10 <strip> <4 ASCII> F7` | the four characters of a strip's display |
//! | out | SysEx `F0 00 00 66 05 00 20 <strip> <side << 4 \| level> F7` | a meter (side 0 left, 1 right), level `0..=0xC` |
//!
//! Ports of a strip zone: `0` fader touch, `1` select, `2` mute, `3` solo, `7` record-ready.
//! Zone `0x0E` is the transport (`0` rewind, `1` fast forward, `2` stop, `3` play, `4` record);
//! zone `0x0A` moves the bank (`0` one strip left, `1` eight left, `2` one right, `3` eight
//! right). A fader the user holds is left alone by the motors, as in the other drivers.

use super::mcu::{bend_to_fader, fader_to_bend, meter_step, relative_detents, ring_value};
use super::{Needs, SurfaceAction, SurfaceDriver, SurfaceState, TransportButton};
use crate::{RunningStatus, parse_message};
use atomik_core::{ChannelMessage, MessageKind};
use std::collections::BTreeMap;

/// Strips on the surface.
const STRIPS: usize = 8;
/// The SysEx prefix: the manufacturer id and the product id of the protocol.
const HEADER: [u8; 6] = [0xF0, 0x00, 0x00, 0x66, 0x05, 0x00];
/// Characters in a strip's display.
const CHARS: usize = 4;
/// One V-pot detent as a fraction of the pan range.
const PAN_PER_DETENT: f32 = 1.0 / 32.0;

const CC_ZONE_IN: u8 = 0x0F;
const CC_PORT_IN: u8 = 0x2F;
const CC_ZONE_OUT: u8 = 0x0C;
const CC_PORT_OUT: u8 = 0x2C;
const CC_JOG: u8 = 0x0D;
const CC_VPOT: u8 = 0x40;
const CC_RING: u8 = 0x10;
const CC_LOW: u8 = 0x20;

const PORT_TOUCH: u8 = 0;
const PORT_SELECT: u8 = 1;
const PORT_MUTE: u8 = 2;
const PORT_SOLO: u8 = 3;
const PORT_REC: u8 = 7;
const ZONE_BANK: u8 = 0x0A;
const ZONE_TRANSPORT: u8 = 0x0E;

/// What was last sent, so that only changes are sent again.
#[derive(Debug, Clone, Default)]
struct Sent {
    faders: [Option<u16>; STRIPS],
    lamps: BTreeMap<(u8, u8), bool>,
    rings: [Option<u8>; STRIPS],
    names: [Option<[u8; CHARS]>; STRIPS],
    meters: [Option<u8>; STRIPS],
}

/// The driver of an eight-fader surface that speaks the protocol of the module docs.
#[derive(Debug, Clone, Default)]
pub struct Hui {
    /// The index of the strip on the first fader.
    bank: usize,
    /// A finger rests on each fader.
    touched: [bool; STRIPS],
    /// The zone the next port message belongs to.
    zone: Option<u8>,
    /// The upper bits of each fader, waiting for the lower ones.
    upper: [u8; STRIPS],
    sent: Sent,
}

impl Hui {
    /// A driver showing strips from the first one.
    pub fn new() -> Self {
        Self::default()
    }

    /// The index of the strip on the first fader.
    pub fn bank(&self) -> usize {
        self.bank
    }

    fn invalidate(&mut self) {
        self.sent = Sent::default();
    }

    fn move_bank(&mut self, to: usize) {
        if to != self.bank {
            self.bank = to;
            self.touched = [false; STRIPS];
            self.invalidate();
        }
    }

    fn strip_at(&self, fader: usize, count: usize) -> Option<usize> {
        let index = self.bank + fader;
        (index < count).then_some(index)
    }

    /// A switch changed: `port` of `zone`, pressed or released.
    fn switch(
        &mut self,
        zone: u8,
        port: u8,
        down: bool,
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
    ) {
        let count = state.strips.len();
        if usize::from(zone) < STRIPS {
            let fader = usize::from(zone);
            let Some(strip) = self.strip_at(fader, count) else {
                return;
            };
            match port {
                PORT_TOUCH => {
                    self.touched[fader] = down;
                    actions.push(SurfaceAction::Touch {
                        strip,
                        touched: down,
                    });
                }
                PORT_SELECT if down => actions.push(SurfaceAction::Select(strip)),
                PORT_MUTE if down => actions.push(SurfaceAction::ToggleMute(strip)),
                PORT_SOLO if down => actions.push(SurfaceAction::ToggleSolo(strip)),
                PORT_REC if down => actions.push(SurfaceAction::ToggleArm(strip)),
                _ => {}
            }
            return;
        }
        if !down {
            return;
        }
        let last_page = count.saturating_sub(1) / STRIPS * STRIPS;
        match (zone, port) {
            (ZONE_TRANSPORT, 0) => actions.push(SurfaceAction::Transport(TransportButton::Rewind)),
            (ZONE_TRANSPORT, 1) => {
                actions.push(SurfaceAction::Transport(TransportButton::FastForward));
            }
            (ZONE_TRANSPORT, 2) => actions.push(SurfaceAction::Transport(TransportButton::Stop)),
            (ZONE_TRANSPORT, 3) => actions.push(SurfaceAction::Transport(TransportButton::Play)),
            (ZONE_TRANSPORT, 4) => actions.push(SurfaceAction::Transport(TransportButton::Record)),
            (ZONE_BANK, 0) => self.move_bank(self.bank.saturating_sub(1)),
            (ZONE_BANK, 1) => self.move_bank(self.bank.saturating_sub(STRIPS)),
            (ZONE_BANK, 2) => self.move_bank((self.bank + 1).min(count.saturating_sub(1))),
            (ZONE_BANK, 3) => self.move_bank((self.bank + STRIPS).min(last_page)),
            _ => {}
        }
    }
}

/// The four characters of a display: `text` cut or padded with spaces (printable ASCII only).
fn chars(text: &str) -> [u8; CHARS] {
    let mut out = [b' '; CHARS];
    for (slot, ch) in out.iter_mut().zip(text.chars()) {
        *slot = if ch.is_ascii() && !ch.is_ascii_control() {
            ch as u8
        } else {
            b'?'
        };
    }
    out
}

/// The two messages that set a switch lamp.
fn lamp_messages(zone: u8, port: u8, on: bool) -> [Vec<u8>; 2] {
    [
        ChannelMessage::cc(0, CC_ZONE_OUT, zone).to_bytes().to_vec(),
        ChannelMessage::cc(0, CC_PORT_OUT, if on { 0x40 | port } else { port })
            .to_bytes()
            .to_vec(),
    ]
}

impl SurfaceDriver for Hui {
    fn name(&self) -> &'static str {
        "hui"
    }

    fn needs(&self) -> Needs {
        Needs {
            meters: true,
            send: None,
        }
    }

    fn connect(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        self.invalidate();
        self.touched = [false; STRIPS];
        self.zone = None;
        self.feedback(state, out);
    }

    fn on_message(
        &mut self,
        bytes: &[u8],
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        replies: &mut Vec<Vec<u8>>,
    ) {
        let count = state.strips.len();
        let Some(ChannelMessage { kind, .. }) = parse_message(bytes, &mut RunningStatus::default())
        else {
            return;
        };
        match kind {
            // The ping: a note on with velocity 0 (which reads as a note off).
            MessageKind::NoteOff { key: 0 } | MessageKind::NoteOn { key: 0, .. } => {
                replies.push(vec![0x90, 0x00, 0x7F]);
            }
            MessageKind::Cc { number, value } => match number {
                CC_ZONE_IN => self.zone = Some(value),
                CC_PORT_IN => {
                    if let Some(zone) = self.zone {
                        self.switch(zone, value & 0x0F, value & 0x40 != 0, state, actions);
                    }
                }
                CC_JOG => {
                    let detents = relative_detents(value);
                    if detents != 0 {
                        actions.push(SurfaceAction::Jog(detents));
                    }
                }
                n if usize::from(n) < STRIPS => self.upper[usize::from(n)] = value,
                n if (CC_LOW..CC_LOW + STRIPS as u8).contains(&n) => {
                    let fader = usize::from(n - CC_LOW);
                    let bend = u16::from(self.upper[fader]) << 7 | u16::from(value);
                    if let Some(strip) = self.strip_at(fader, count) {
                        // The surface already is where the fader is: no echo.
                        self.sent.faders[fader] = Some(bend);
                        actions.push(SurfaceAction::Fader {
                            strip,
                            value: bend_to_fader(bend),
                        });
                    }
                }
                n if (CC_VPOT..CC_VPOT + STRIPS as u8).contains(&n) => {
                    let fader = usize::from(n - CC_VPOT);
                    if let Some(strip) = self.strip_at(fader, count) {
                        actions.push(SurfaceAction::Pan {
                            strip,
                            delta: relative_detents(value) as f32 * PAN_PER_DETENT,
                        });
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn feedback(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        let count = state.strips.len();
        for fader in 0..STRIPS {
            let strip = self.strip_at(fader, count).map(|i| &state.strips[i]);
            // The motor of a touched fader is left alone; one step is the round trip.
            if !self.touched[fader] {
                let value = fader_to_bend(strip.map_or(0.0, |s| s.fader));
                let moved = self.sent.faders[fader].is_none_or(|old| old.abs_diff(value) > 1);
                if moved {
                    self.sent.faders[fader] = Some(value);
                    let n = fader as u8;
                    out.push(
                        ChannelMessage::cc(0, n, (value >> 7) as u8)
                            .to_bytes()
                            .to_vec(),
                    );
                    out.push(
                        ChannelMessage::cc(0, CC_LOW + n, (value & 0x7F) as u8)
                            .to_bytes()
                            .to_vec(),
                    );
                }
            }
            let mut lamp = |zone: u8, port: u8, on: bool, out: &mut Vec<Vec<u8>>| {
                if self.sent.lamps.get(&(zone, port)) != Some(&on) {
                    self.sent.lamps.insert((zone, port), on);
                    out.extend(lamp_messages(zone, port, on));
                }
            };
            let zone = fader as u8;
            lamp(zone, PORT_SELECT, strip.is_some_and(|s| s.selected), out);
            lamp(zone, PORT_MUTE, strip.is_some_and(|s| s.mute), out);
            lamp(zone, PORT_SOLO, strip.is_some_and(|s| s.solo), out);
            lamp(zone, PORT_REC, strip.is_some_and(|s| s.armed), out);
            let ring = ring_value(strip.and_then(|s| s.pan));
            if self.sent.rings[fader] != Some(ring) {
                self.sent.rings[fader] = Some(ring);
                out.push(
                    ChannelMessage::cc(0, CC_RING + fader as u8, ring)
                        .to_bytes()
                        .to_vec(),
                );
            }
            let name = chars(strip.map_or("", |s| s.name.as_str()));
            if self.sent.names[fader] != Some(name) {
                self.sent.names[fader] = Some(name);
                let mut m = HEADER.to_vec();
                m.extend_from_slice(&[0x10, fader as u8]);
                m.extend_from_slice(&name);
                m.push(0xF7);
                out.push(m);
            }
            // A meter per side; the strip has one level, so both sides show it.
            let step = strip.map_or(0, |s| meter_step(s.meter));
            if self.sent.meters[fader] != Some(step) {
                self.sent.meters[fader] = Some(step);
                for side in 0..2u8 {
                    let mut m = HEADER.to_vec();
                    m.extend_from_slice(&[0x20, fader as u8, (side << 4) | step, 0xF7]);
                    out.push(m);
                }
            }
        }
        let mut lamp = |zone: u8, port: u8, on: bool, out: &mut Vec<Vec<u8>>| {
            if self.sent.lamps.get(&(zone, port)) != Some(&on) {
                self.sent.lamps.insert((zone, port), on);
                out.extend(lamp_messages(zone, port, on));
            }
        };
        lamp(ZONE_TRANSPORT, 2, !state.transport.playing, out);
        lamp(ZONE_TRANSPORT, 3, state.transport.playing, out);
        lamp(ZONE_TRANSPORT, 4, state.transport.recording, out);
    }

    fn disconnect(&mut self, _out: &mut Vec<Vec<u8>>) {
        self.invalidate();
    }
}

#[cfg(test)]
#[path = "hui_tests.rs"]
mod tests;
