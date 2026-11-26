//! The common control-surface protocol of eight motor faders (MCU-compatible), as a driver.
//!
//! What it speaks, all of it the openly documented behaviour of that class of surface:
//!
//! | Direction | Message | Meaning |
//! |---|---|---|
//! | in and out | pitch bend, channel 0..=7 (8 is the master) | the fader, 14 bit; the motor is driven with the same message |
//! | in | note `0x68 + n` (`0x70` master) | the finger touching fader `n` (velocity `0x7F` on, `0` off) |
//! | in | CC `0x10 + n` | V-pot `n`, relative: bit 6 is the sign, bits 0..=5 the detents |
//! | out | CC `0x30 + n` | the V-pot LED ring: `mode << 4 \| position (0..=11)`, bit 6 the centre LED |
//! | in and out | notes `0x00` rec, `0x08` solo, `0x10` mute, `0x18` select (`+ n`) | the buttons in, their LEDs out (`0x7F` on, `0` off) |
//! | in | notes `0x2E` `0x2F` `0x30` `0x31` | bank left and right (eight strips), channel left and right (one) |
//! | in and out | notes `0x5B` rewind, `0x5C` fast forward, `0x5D` stop, `0x5E` play, `0x5F` record, `0x56` cycle | the transport keys and their LEDs |
//! | in | CC `0x3C` | the jog wheel, relative as the V-pots |
//! | in | note `0x35` | switches the time display between beats and timecode |
//! | out | CC `0x40..=0x49` (digits, right to left), `0x4A` and `0x4B` (the two assignment digits) | the 7-segment displays: the low 6 bits of the ASCII code, bit 6 the decimal point |
//! | out | SysEx `F0 00 00 66 14 12 <offset> <ASCII> F7` | the scribble strips: 56 characters per row, 7 per strip, the lower row from offset `0x38` |
//! | both | SysEx `F0 00 00 66 14 00..04, 0F F7` | the device query, the host connection query and reply, go offline |
//! | out | channel pressure `D0 <strip << 4 \| level>` | the level meter of a strip, level `0..=0xB`, `0xC` when it clips |
//! | out | SysEx `F0 00 00 66 14 20 <strip> 03 F7` | the meter mode (signal lamp and level meter) of a strip, sent when the surface connects |
//! | in and out | note `0x32` | Flip: the faders and the V-pots swap what they drive (the LED shows it) |
//! | in and out | notes `0x29` send, `0x2A` pan, `0x2B` plug-in | what the V-pots drive (the LEDs show it) |
//! | in | notes `0x36..=0x3D` | the function keys F1 to F8 |
//!
//! The bank is kept here: bank left and right move eight strips (sixteen with an extender),
//! channel left and right one, and every move makes the next [`SurfaceDriver::feedback`] send
//! the whole surface again.
//!
//! **What the controls drive.** The faders move the strip levels and the V-pots the pans, until
//! the user presses Send (the V-pots drive the level of one send; pressing it again takes the
//! next of the eight), Plug-in (the eight V-pots drive eight parameters of the device the
//! surface follows, and the bank keys turn the pages) or Flip (the faders drive what the V-pots
//! drive and the V-pots the levels; the motors and the rings follow). The function keys only
//! say which key was pressed ([`SurfaceAction::Function`]): the app owns the commands.
//!
//! **An extender** ([`Mcu::extender`]) is a second unit with the same messages and the device id
//! `0x15`: its eight strips are the eight after the main unit's, it follows the main unit's
//! bank, flip and V-pot mode ([`SurfaceDriver::follow`]), and it has no master fader, transport
//! or displays of the time.

use super::{
    KnobMove, Linked, Needs, StripControl, StripState, SurfaceAction, SurfaceDriver, SurfaceState,
    TransportButton, VpotMode,
};
use crate::{RunningStatus, parse_message};
use atomik_core::{ChannelMessage, MessageKind};
use std::collections::BTreeMap;

/// The SysEx prefix (`F0`, the manufacturer id, the device id of the main unit).
const HEADER: [u8; 5] = [0xF0, 0x00, 0x00, 0x66, 0x14];
/// The device id of an extender (accepted on input).
const EXTENDER_ID: u8 = 0x15;

/// Characters per strip on a display row.
const CELL: usize = 7;
/// The lower display row starts at this offset.
const LOWER_ROW: u8 = 0x38;
/// Faders: eight strips and the master.
const FADERS: usize = 9;
/// Strips on the surface.
const STRIPS: usize = 8;
/// Largest 14 bit value.
const BEND_MAX: f32 = 16_383.0;

const NOTE_REC: u8 = 0x00;
const NOTE_SOLO: u8 = 0x08;
const NOTE_MUTE: u8 = 0x10;
const NOTE_SELECT: u8 = 0x18;
const NOTE_BANK_LEFT: u8 = 0x2E;
const NOTE_BANK_RIGHT: u8 = 0x2F;
const NOTE_CHANNEL_LEFT: u8 = 0x30;
const NOTE_CHANNEL_RIGHT: u8 = 0x31;
const NOTE_SEND: u8 = 0x29;
const NOTE_PAN: u8 = 0x2A;
const NOTE_PLUGIN: u8 = 0x2B;
const NOTE_FLIP: u8 = 0x32;
const NOTE_FUNCTION: u8 = 0x36;
/// The function keys F1 to F8.
const FUNCTION_KEYS: u8 = 8;
const NOTE_TIME_MODE: u8 = 0x35;
const NOTE_CYCLE: u8 = 0x56;
const NOTE_REWIND: u8 = 0x5B;
const NOTE_FAST_FORWARD: u8 = 0x5C;
const NOTE_STOP: u8 = 0x5D;
const NOTE_PLAY: u8 = 0x5E;
const NOTE_RECORD: u8 = 0x5F;
const NOTE_TOUCH: u8 = 0x68;
const NOTE_TOUCH_MASTER: u8 = 0x70;
const NOTE_LED_TIMECODE: u8 = 0x71;
const NOTE_LED_BEATS: u8 = 0x72;
const CC_VPOT: u8 = 0x10;
const CC_JOG: u8 = 0x3C;
const CC_RING: u8 = 0x30;
const CC_DIGITS: u8 = 0x40;
const CC_ASSIGN_RIGHT: u8 = 0x4A;
const CC_ASSIGN_LEFT: u8 = 0x4B;

/// The number of sends a strip has.
const SENDS: u8 = 8;
/// The loudest meter step that is not the overload mark.
const METER_TOP: u8 = 0x0B;
/// The meter step that marks a clip.
const METER_CLIP: u8 = 0x0C;
/// One V-pot detent as a fraction of the pan range.
const PAN_PER_DETENT: f32 = 1.0 / 32.0;
/// Timecode frame rate of the time display.
const TIMECODE_FPS: f64 = 30.0;

/// What was last sent, so that only changes are sent again.
#[derive(Debug, Clone, Default)]
struct Sent {
    faders: [Option<u16>; FADERS],
    leds: BTreeMap<u8, u8>,
    rings: [Option<u8>; STRIPS],
    lcd: [[Option<[u8; CELL]>; STRIPS]; 2],
    digits: [Option<u8>; 10],
    assign: [Option<u8>; 2],
    meters: [Option<u8>; STRIPS],
}

/// Which unit of a surface a driver is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Unit {
    #[default]
    Main,
    Extender,
}

/// What a fader or a V-pot drives at the moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drives {
    Level,
    Pan,
    Send(u8),
    /// A parameter of the followed device, by its index in the remote parameters.
    Param(usize),
}

/// The driver of an eight-fader surface that speaks the protocol of the module docs.
#[derive(Debug, Clone, Default)]
pub struct Mcu {
    /// The index of the strip on the first fader.
    bank: usize,
    /// A finger rests on each fader (the master last).
    touched: [bool; FADERS],
    /// The time display shows timecode instead of beats.
    timecode: bool,
    /// Which unit this is.
    unit: Unit,
    /// Faders and V-pots are swapped.
    flip: bool,
    /// What the V-pots drive.
    vpot: VpotMode,
    /// The send (1-based, `0` is the first) the send mode addresses.
    send_slot: u8,
    /// The page of device parameters (of `8 * units` parameters).
    page: usize,
    /// How many units share the bank (at least one).
    units: usize,
    sent: Sent,
}

impl Mcu {
    /// A driver showing strips from the first one, beats on the time display.
    pub fn new() -> Self {
        Self {
            units: 1,
            send_slot: 1,
            ..Self::default()
        }
    }

    /// The driver of an extender unit: eight more strips after the main unit's.
    pub fn extender() -> Self {
        Self {
            unit: Unit::Extender,
            units: 2,
            ..Self::new()
        }
    }

    /// The index of the strip on the first fader.
    pub fn bank(&self) -> usize {
        self.bank
    }

    /// Whether the time display shows timecode (else bars and beats).
    pub fn shows_timecode(&self) -> bool {
        self.timecode
    }

    /// What the V-pots drive.
    pub fn vpot_mode(&self) -> VpotMode {
        self.vpot
    }

    /// Whether the faders and the V-pots are swapped.
    pub fn is_flipped(&self) -> bool {
        self.flip
    }

    /// The page of device parameters the V-pots show in plug-in mode (0-based).
    pub fn page(&self) -> usize {
        self.page
    }

    /// The send (1-based) the send mode addresses.
    pub fn send_slot(&self) -> u8 {
        self.send_slot
    }

    fn header(&self) -> [u8; 5] {
        let mut h = HEADER;
        if self.unit == Unit::Extender {
            h[4] = EXTENDER_ID;
        }
        h
    }

    fn is_main(&self) -> bool {
        self.unit == Unit::Main
    }

    /// The strips between the first fader of the main unit and this unit's first fader.
    fn unit_offset(&self) -> usize {
        if self.is_main() { 0 } else { STRIPS }
    }

    /// What the fader `fader` drives.
    fn fader_drives(&self, fader: usize) -> Drives {
        if self.flip {
            self.mode_drives(fader)
        } else {
            Drives::Level
        }
    }

    /// What the V-pot `fader` drives.
    fn vpot_drives(&self, fader: usize) -> Drives {
        if self.flip {
            Drives::Level
        } else {
            self.mode_drives(fader)
        }
    }

    /// What the V-pot mode asks the faders or the V-pots to drive.
    fn mode_drives(&self, fader: usize) -> Drives {
        match self.vpot {
            VpotMode::Pan => Drives::Pan,
            VpotMode::Send => Drives::Send(self.send_slot),
            VpotMode::Plugin => {
                Drives::Param(self.page * STRIPS * self.units + self.unit_offset() + fader)
            }
        }
    }

    fn invalidate(&mut self) {
        self.sent = Sent::default();
    }

    fn strip_at(&self, fader: usize, count: usize) -> Option<usize> {
        strip_index(self.bank + self.unit_offset(), fader, count)
    }

    fn move_bank(&mut self, to: usize) {
        if to != self.bank {
            self.bank = to;
            self.touched = [false; FADERS];
            self.invalidate();
        }
    }

    /// The V-pots drive something else now (or the faders and V-pots swapped).
    fn drives_changed(&mut self) {
        self.touched = [false; FADERS];
        self.invalidate();
    }

    /// The pages of device parameters of `state`'s device (at least one).
    fn pages(&self, state: &SurfaceState) -> usize {
        state.knobs.len().div_ceil(STRIPS * self.units).max(1)
    }

    /// Moves the parameter page `delta` on (wrapping) and says where it is.
    fn turn_page(&mut self, delta: i32, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
        if state.remote.id.is_none() || state.knobs.is_empty() {
            actions.push(SurfaceAction::Notice(
                "No device with knobs is selected".to_owned(),
            ));
            return;
        }
        let pages = self.pages(state);
        let to = (self.page as i64 + i64::from(delta)).rem_euclid(pages as i64) as usize;
        if to != self.page {
            self.page = to;
            self.drives_changed();
        }
        let span = STRIPS * self.units;
        let (first, last) = (to * span + 1, ((to + 1) * span).min(state.knobs.len()));
        actions.push(SurfaceAction::Notice(format!(
            "{}: parameters {first} to {last} of {} (page {} of {pages})",
            state.remote.name,
            state.knobs.len(),
            to + 1,
        )));
    }

    /// The action for a move of the fader or V-pot that drives `drives`.
    fn drive(drives: Drives, strip: usize, change: KnobMove, actions: &mut Vec<SurfaceAction>) {
        actions.push(match drives {
            Drives::Level => match change {
                // A fader at its default job keeps the action every driver has.
                KnobMove::To(value) => SurfaceAction::Fader { strip, value },
                KnobMove::By(_) => SurfaceAction::Strip {
                    strip,
                    control: StripControl::Level,
                    change,
                },
            },
            Drives::Pan => match change {
                KnobMove::By(delta) => SurfaceAction::Pan { strip, delta },
                KnobMove::To(_) => SurfaceAction::Strip {
                    strip,
                    control: StripControl::Pan,
                    change,
                },
            },
            Drives::Send(slot) => SurfaceAction::Strip {
                strip,
                control: StripControl::Send(slot),
                change,
            },
            Drives::Param(knob) => SurfaceAction::Knob { knob, change },
        });
    }

    fn button(
        &mut self,
        key: u8,
        pressed: bool,
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
    ) {
        let count = state.strips.len();
        // Touch is the only note that matters on release.
        if (NOTE_TOUCH..NOTE_TOUCH + STRIPS as u8).contains(&key) {
            let fader = usize::from(key - NOTE_TOUCH);
            self.touched[fader] = pressed;
            // The lower row shows the fader value while it is touched.
            self.sent.lcd[1][fader] = None;
            if let Some(strip) = self.strip_at(fader, count) {
                actions.push(SurfaceAction::Touch {
                    strip,
                    touched: pressed,
                });
            }
            return;
        }
        if key == NOTE_TOUCH_MASTER {
            if self.is_main() {
                self.touched[STRIPS] = pressed;
                actions.push(SurfaceAction::MasterTouch(pressed));
            }
            return;
        }
        if !pressed {
            return;
        }
        let per_strip = |base: u8| {
            (base..base + STRIPS as u8)
                .contains(&key)
                .then(|| usize::from(key - base))
                .and_then(|fader| self.strip_at(fader, count))
        };
        if let Some(strip) = per_strip(NOTE_REC) {
            actions.push(SurfaceAction::ToggleArm(strip));
        } else if let Some(strip) = per_strip(NOTE_SOLO) {
            actions.push(SurfaceAction::ToggleSolo(strip));
        } else if let Some(strip) = per_strip(NOTE_MUTE) {
            actions.push(SurfaceAction::ToggleMute(strip));
        } else if let Some(strip) = per_strip(NOTE_SELECT) {
            actions.push(SurfaceAction::Select(strip));
        } else if self.is_main() {
            self.main_button(key, state, actions);
        }
    }

    /// The keys only the main unit has.
    fn main_button(&mut self, key: u8, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
        let count = state.strips.len();
        let step = STRIPS * self.units;
        let last_page = count.saturating_sub(1) / step * step;
        let plugin = self.vpot == VpotMode::Plugin;
        match key {
            // In plug-in mode the bank keys turn the pages of parameters.
            NOTE_BANK_LEFT if plugin => self.turn_page(-1, state, actions),
            NOTE_BANK_RIGHT if plugin => self.turn_page(1, state, actions),
            NOTE_BANK_LEFT => self.move_bank(self.bank.saturating_sub(step)),
            NOTE_BANK_RIGHT => self.move_bank((self.bank + step).min(last_page)),
            NOTE_CHANNEL_LEFT => self.move_bank(self.bank.saturating_sub(1)),
            NOTE_CHANNEL_RIGHT => self.move_bank((self.bank + 1).min(count.saturating_sub(1))),
            NOTE_FLIP => {
                self.flip = !self.flip;
                self.drives_changed();
            }
            NOTE_PAN => {
                self.vpot = VpotMode::Pan;
                self.drives_changed();
                actions.push(SurfaceAction::Notice("V-pots: pan".to_owned()));
            }
            NOTE_SEND => {
                // Pressed again, the key takes the next send.
                self.send_slot = if self.vpot == VpotMode::Send {
                    self.send_slot % SENDS + 1
                } else {
                    self.send_slot.clamp(1, SENDS)
                };
                self.vpot = VpotMode::Send;
                self.drives_changed();
                actions.push(SurfaceAction::Notice(format!(
                    "V-pots: send {}",
                    self.send_slot
                )));
            }
            NOTE_PLUGIN => {
                self.vpot = VpotMode::Plugin;
                self.drives_changed();
                self.page = self.page.min(self.pages(state) - 1);
                actions.push(SurfaceAction::Notice(if state.remote.id.is_some() {
                    format!("V-pots: {}", state.remote.name)
                } else {
                    "V-pots: plug-in parameters (select a device)".to_owned()
                }));
            }
            NOTE_TIME_MODE => {
                self.timecode = !self.timecode;
                self.sent.digits = [None; 10];
                self.sent.leds.remove(&NOTE_LED_TIMECODE);
                self.sent.leds.remove(&NOTE_LED_BEATS);
            }
            NOTE_PLAY => actions.push(SurfaceAction::Transport(TransportButton::Play)),
            NOTE_STOP => actions.push(SurfaceAction::Transport(TransportButton::Stop)),
            NOTE_RECORD => actions.push(SurfaceAction::Transport(TransportButton::Record)),
            NOTE_REWIND => actions.push(SurfaceAction::Transport(TransportButton::Rewind)),
            NOTE_FAST_FORWARD => {
                actions.push(SurfaceAction::Transport(TransportButton::FastForward));
            }
            NOTE_CYCLE => actions.push(SurfaceAction::Transport(TransportButton::Cycle)),
            k if (NOTE_FUNCTION..NOTE_FUNCTION + FUNCTION_KEYS).contains(&k) => {
                actions.push(SurfaceAction::Function(k - NOTE_FUNCTION + 1));
            }
            _ => {}
        }
    }

    fn sysex(&mut self, bytes: &[u8], replies: &mut Vec<Vec<u8>>) {
        // `F0 00 00 66 <id> <command> ... F7`
        if bytes.len() < 7
            || bytes[..4] != HEADER[..4]
            || !(bytes[4] == HEADER[4] || bytes[4] == EXTENDER_ID)
        {
            return;
        }
        // Host connection query: serial (7 bytes) and a challenge (4 bytes).
        if bytes[5] == 0x01 && bytes.len() > 5 + 1 + 7 + 4 {
            let serial = &bytes[6..13];
            let challenge = [bytes[13], bytes[14], bytes[15], bytes[16]];
            let mut reply = self.header().to_vec();
            reply.push(0x02);
            reply.extend_from_slice(serial);
            reply.extend_from_slice(&challenge_response(challenge));
            reply.push(0xF7);
            replies.push(reply);
            // A surface that has just come online shows nothing yet.
            self.invalidate();
        }
    }
}

/// The value (0..=1) of what `drives` drives, as the strip (or the device) has it.
fn position_of(drives: Drives, strip: Option<&StripState>, state: &SurfaceState) -> Option<f32> {
    match drives {
        Drives::Level => strip.map(|s| s.fader),
        Drives::Pan => strip
            .and_then(|s| s.pan)
            .map(|p| (p.clamp(-1.0, 1.0) + 1.0) / 2.0),
        Drives::Send(_) => strip.and_then(|s| s.send),
        Drives::Param(knob) => state.knobs.get(knob).copied().flatten(),
    }
}

/// The text of that value (a parameter's is a percentage).
fn text_of(drives: Drives, strip: Option<&StripState>, state: &SurfaceState) -> String {
    match drives {
        Drives::Level => strip.map(|s| s.fader_text.clone()),
        Drives::Pan => strip.map(|s| s.pan_text.clone()),
        Drives::Send(_) => strip.map(|s| s.send_text.clone()),
        Drives::Param(knob) => state
            .knobs
            .get(knob)
            .copied()
            .flatten()
            .map(|v| format!("{:.0}%", v * 100.0)),
    }
    .unwrap_or_default()
}

/// The LED ring of a V-pot that drives `drives` and shows `position`: a dot with the centre
/// LED for the pan, a bar filling from the left for anything else.
fn ring_of(drives: Drives, strip: Option<&StripState>, position: Option<f32>) -> u8 {
    match drives {
        Drives::Pan => ring_value(strip.and_then(|s| s.pan)),
        _ => position.map_or(0, fill_ring),
    }
}

/// A ring that fills from the left up to `value` (0..=1): ring mode 2, position 1..=11, all off
/// at zero.
fn fill_ring(value: f32) -> u8 {
    let position = (value.clamp(0.0, 1.0) * 10.0).round() as u8;
    if position == 0 {
        0x20
    } else {
        0x20 | (position + 1)
    }
}

/// The meter step (0..=0xC) of a strip's level (0..=1): the top step marks a clip.
pub fn meter_step(level: f32) -> u8 {
    if !level.is_finite() || level <= 0.0 {
        0
    } else if level >= 1.0 {
        METER_CLIP
    } else {
        (level * f32::from(METER_TOP)).round() as u8
    }
}

/// The strip on surface fader `fader` when the bank starts at `bank` (none past the last strip).
fn strip_index(bank: usize, fader: usize, count: usize) -> Option<usize> {
    let index = bank + fader;
    (index < count).then_some(index)
}

/// The host's answer to a connection query's four challenge bytes.
pub fn challenge_response(challenge: [u8; 4]) -> [u8; 4] {
    let [l1, l2, l3, l4] = challenge.map(i32::from);
    [
        (l1 + (l2 ^ 0x0A) - l4) & 0x7F,
        ((l3 >> 4) ^ (l1 + l4)) & 0x7F,
        ((l4 - (l3 << 2)) ^ (l1 | l2)) & 0x7F,
        (l2 - l3 + (0xF0 ^ (l4 << 4))) & 0x7F,
    ]
    .map(|v| v as u8)
}

/// The SysEx frame that writes `text` on a display row at `offset` (0..=0x6F).
pub fn lcd_message(offset: u8, text: &[u8]) -> Vec<u8> {
    lcd_message_for(&HEADER, offset, text)
}

/// [`lcd_message`] for the unit whose SysEx prefix is `header`.
fn lcd_message_for(header: &[u8; 5], offset: u8, text: &[u8]) -> Vec<u8> {
    let mut m = header.to_vec();
    m.push(0x12);
    m.push(offset);
    m.extend(text.iter().map(|b| b & 0x7F));
    m.push(0xF7);
    m
}

/// A fader position as the 14 bit value on the wire.
pub fn fader_to_bend(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * BEND_MAX).round() as u16
}

/// The position a 14 bit fader value stands for.
pub fn bend_to_fader(value: u16) -> f32 {
    f32::from(value.min(16_383)) / BEND_MAX
}

/// The ring of a V-pot showing `pan` (-1..=1) as a single dot, with the centre LED lit at the
/// middle position; 0 (all off) without a pan.
pub fn ring_value(pan: Option<f32>) -> u8 {
    let Some(pan) = pan else {
        return 0;
    };
    let position = 1 + ((pan.clamp(-1.0, 1.0) + 1.0) / 2.0 * 10.0).round() as u8;
    let centre = if position == 6 { 0x40 } else { 0 };
    centre | position
}

/// The relative value of a V-pot or the jog wheel: detents, positive clockwise.
pub fn relative_detents(value: u8) -> i32 {
    let magnitude = i32::from(value & 0x3F);
    if value & 0x40 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// A display cell: `text` cut or padded to six characters and a space (printable ASCII only).
fn cell(text: &str) -> [u8; CELL] {
    let mut out = [b' '; CELL];
    for (slot, ch) in out.iter_mut().take(CELL - 1).zip(text.chars()) {
        *slot = if ch.is_ascii() && !ch.is_ascii_control() {
            ch as u8
        } else {
            b'?'
        };
    }
    out
}

/// The ten digits of the time display, left to right, as the values of CC `0x49` down to
/// `0x40`: the low 6 bits of the ASCII digit, bit 6 the decimal point after the field.
fn time_digits(state: &SurfaceState, timecode: bool) -> [u8; 10] {
    let p = &state.position;
    let fields: [(u32, usize); 4] = if timecode {
        let total = (p.seconds.max(0.0) * TIMECODE_FPS).floor() as u64;
        let fps = TIMECODE_FPS as u64;
        let frames = (total % fps) as u32;
        let seconds = ((total / fps) % 60) as u32;
        let minutes = ((total / fps / 60) % 60) as u32;
        let hours = ((total / fps / 3600) % 1000) as u32;
        [(hours, 3), (minutes, 2), (seconds, 2), (frames, 3)]
    } else {
        [
            (p.bar % 1000, 3),
            (p.beat % 100, 2),
            (p.sixteenth % 100, 2),
            (p.ticks % 1000, 3),
        ]
    };
    let mut digits = [0u8; 10];
    let mut at = 0;
    for (value, width) in fields {
        let text = format!("{value:0width$}");
        for (i, ch) in text.bytes().enumerate() {
            digits[at + i] = ch & 0x3F;
        }
        at += width;
        // A decimal point after each field but the last.
        if at < 10 {
            digits[at - 1] |= 0x40;
        }
    }
    digits
}

fn assign_digits(bank: usize) -> [u8; 2] {
    let n = (bank + 1) % 100;
    [b'0' + (n / 10) as u8, b'0' + (n % 10) as u8].map(|c| c & 0x3F)
}

impl SurfaceDriver for Mcu {
    fn name(&self) -> &'static str {
        if self.is_main() {
            "mcu"
        } else {
            "mcu-extender"
        }
    }

    fn needs(&self) -> Needs {
        Needs {
            meters: true,
            send: (self.vpot == VpotMode::Send).then_some(self.send_slot),
        }
    }

    fn linked(&self) -> Option<Linked> {
        self.is_main().then_some(Linked {
            bank: self.bank,
            flip: self.flip,
            vpot: self.vpot,
            send_slot: self.send_slot,
            page: self.page,
            units: self.units,
        })
    }

    fn extends(&self) -> bool {
        !self.is_main()
    }

    fn follow(&mut self, main: Linked) {
        if self.is_main() {
            return;
        }
        let changed = (
            self.bank,
            self.flip,
            self.vpot,
            self.send_slot,
            self.page,
            self.units,
        ) != (
            main.bank,
            main.flip,
            main.vpot,
            main.send_slot,
            main.page,
            main.units,
        );
        if changed {
            self.bank = main.bank;
            self.flip = main.flip;
            self.vpot = main.vpot;
            self.send_slot = main.send_slot;
            self.page = main.page;
            self.units = main.units.max(2);
            self.drives_changed();
        }
    }

    fn set_units(&mut self, units: usize) {
        let units = units.max(1);
        if self.is_main() && units != self.units {
            self.units = units;
            // The bank steps by the new size: land on a whole page.
            self.bank = self.bank / (STRIPS * units) * (STRIPS * units);
            self.drives_changed();
        }
    }

    fn connect(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        self.invalidate();
        self.touched = [false; FADERS];
        let mut query = self.header().to_vec();
        query.extend_from_slice(&[0x00, 0xF7]);
        out.push(query);
        // The meters of every strip: the signal lamp and the level meter.
        for strip in 0..STRIPS as u8 {
            let mut mode = self.header().to_vec();
            mode.extend_from_slice(&[0x20, strip, 0x03, 0xF7]);
            out.push(mode);
        }
        self.feedback(state, out);
    }

    fn on_message(
        &mut self,
        bytes: &[u8],
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        replies: &mut Vec<Vec<u8>>,
    ) {
        if bytes.first() == Some(&0xF0) {
            self.sysex(bytes, replies);
            return;
        }
        let count = state.strips.len();
        let Some(ChannelMessage { channel, kind }) =
            parse_message(bytes, &mut RunningStatus::default())
        else {
            return;
        };
        match kind {
            MessageKind::PitchBend { value } => {
                let fader = usize::from(channel);
                if fader >= FADERS {
                    return;
                }
                let position = bend_to_fader(value);
                if fader == STRIPS {
                    if self.is_main() {
                        self.sent.faders[fader] = Some(value);
                        actions.push(SurfaceAction::MasterFader(position));
                    }
                } else if let Some(strip) = self.strip_at(fader, count) {
                    // The surface already is where the fader is: no echo.
                    self.sent.faders[fader] = Some(value);
                    Self::drive(
                        self.fader_drives(fader),
                        strip,
                        KnobMove::To(position),
                        actions,
                    );
                }
            }
            MessageKind::Cc { number, value } => {
                let detents = relative_detents(value);
                if (CC_VPOT..CC_VPOT + STRIPS as u8).contains(&number) {
                    let fader = usize::from(number - CC_VPOT);
                    if let Some(strip) = self.strip_at(fader, count) {
                        Self::drive(
                            self.vpot_drives(fader),
                            strip,
                            KnobMove::By(detents as f32 * PAN_PER_DETENT),
                            actions,
                        );
                    }
                } else if number == CC_JOG && detents != 0 && self.is_main() {
                    actions.push(SurfaceAction::Jog(detents));
                }
            }
            MessageKind::NoteOn { key, .. } => self.button(key, true, state, actions),
            MessageKind::NoteOff { key } => self.button(key, false, state, actions),
        }
    }

    fn feedback(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        let count = state.strips.len();
        let main = self.is_main();
        // Faders: the motors. A touched fader is left alone, and a change of one step is the
        // rounding of the round trip, not a move.
        for fader in 0..FADERS {
            if self.touched[fader] || (!main && fader == STRIPS) {
                continue;
            }
            let position = if fader == STRIPS {
                state.master.as_ref().map(|m| m.fader)
            } else {
                let strip = self.strip_at(fader, count).map(|i| &state.strips[i]);
                position_of(self.fader_drives(fader), strip, state)
            };
            let value = fader_to_bend(position.unwrap_or(0.0));
            let moved = match self.sent.faders[fader] {
                None => true,
                Some(old) => old.abs_diff(value) > 1,
            };
            if moved {
                self.sent.faders[fader] = Some(value);
                out.push(
                    ChannelMessage::pitch_bend(fader as u8, value)
                        .to_bytes()
                        .to_vec(),
                );
            }
        }
        // Button LEDs.
        let first = self.bank + self.unit_offset();
        let (flip, vpot, timecode) = (self.flip, self.vpot, self.timecode);
        let mut led = |note: u8, on: bool, out: &mut Vec<Vec<u8>>| {
            let value = if on { 0x7F } else { 0x00 };
            if self.sent.leds.get(&note) != Some(&value) {
                self.sent.leds.insert(note, value);
                // An LED is set with a note on; velocity 0 is the wire's note off.
                out.push(vec![0x90, note, value]);
            }
        };
        for fader in 0..STRIPS {
            let strip = strip_index(first, fader, count).map(|i| &state.strips[i]);
            let n = fader as u8;
            led(NOTE_REC + n, strip.is_some_and(|s| s.armed), out);
            led(NOTE_SOLO + n, strip.is_some_and(|s| s.solo), out);
            led(NOTE_MUTE + n, strip.is_some_and(|s| s.mute), out);
            led(NOTE_SELECT + n, strip.is_some_and(|s| s.selected), out);
        }
        if main {
            led(NOTE_PLAY, state.transport.playing, out);
            led(NOTE_STOP, !state.transport.playing, out);
            led(NOTE_RECORD, state.transport.recording, out);
            led(NOTE_CYCLE, state.transport.looping, out);
            led(NOTE_LED_TIMECODE, timecode, out);
            led(NOTE_LED_BEATS, !timecode, out);
            led(NOTE_FLIP, flip, out);
            led(NOTE_PAN, vpot == VpotMode::Pan, out);
            led(NOTE_SEND, vpot == VpotMode::Send, out);
            led(NOTE_PLUGIN, vpot == VpotMode::Plugin, out);
        }
        // V-pot rings.
        for fader in 0..STRIPS {
            let strip = self.strip_at(fader, count).map(|i| &state.strips[i]);
            let drives = self.vpot_drives(fader);
            let position = position_of(drives, strip, state);
            let value = ring_of(drives, strip, position);
            if self.sent.rings[fader] != Some(value) {
                self.sent.rings[fader] = Some(value);
                out.push(
                    ChannelMessage::cc(0, CC_RING + fader as u8, value)
                        .to_bytes()
                        .to_vec(),
                );
            }
        }
        // Scribble strips: the name above (the parameter's while the controls drive one), the
        // V-pot value (the fader's while it is touched) below. One message per changed strip.
        for fader in 0..STRIPS {
            let strip = self.strip_at(fader, count).map(|i| &state.strips[i]);
            let (fader_drives, vpot_drives) = (self.fader_drives(fader), self.vpot_drives(fader));
            let upper = match (fader_drives, vpot_drives) {
                (Drives::Param(k), _) | (_, Drives::Param(k)) => {
                    cell(state.remote.params.get(k).map_or("", String::as_str))
                }
                _ => cell(strip.map_or("", |s| s.name.as_str())),
            };
            let shown = if self.touched[fader] {
                fader_drives
            } else {
                vpot_drives
            };
            let mut text = text_of(shown, strip, state);
            if text.is_empty() {
                // Nothing to show for this control (a bus has no pan): the fader's value.
                text = text_of(Drives::Level, strip, state);
            }
            let lower = cell(&text);
            for (row, text) in [upper, lower].into_iter().enumerate() {
                if self.sent.lcd[row][fader] != Some(text) {
                    self.sent.lcd[row][fader] = Some(text);
                    let base = if row == 0 { 0 } else { LOWER_ROW };
                    out.push(lcd_message_for(
                        &self.header(),
                        base + (fader * CELL) as u8,
                        &text,
                    ));
                }
            }
        }
        // The level meters: channel pressure, the strip in the high nibble.
        for fader in 0..STRIPS {
            let step = self
                .strip_at(fader, count)
                .map_or(0, |i| meter_step(state.strips[i].meter));
            if self.sent.meters[fader] != Some(step) {
                self.sent.meters[fader] = Some(step);
                out.push(vec![0xD0, ((fader as u8) << 4) | step]);
            }
        }
        if !main {
            return;
        }
        // The time display, one digit at a time, and the two assignment digits (the first strip).
        let digits = time_digits(state, self.timecode);
        for (i, value) in digits.into_iter().enumerate() {
            if self.sent.digits[i] != Some(value) {
                self.sent.digits[i] = Some(value);
                let cc = CC_DIGITS + (9 - i) as u8;
                out.push(ChannelMessage::cc(0, cc, value).to_bytes().to_vec());
            }
        }
        for (i, value) in assign_digits(self.bank).into_iter().enumerate() {
            if self.sent.assign[i] != Some(value) {
                self.sent.assign[i] = Some(value);
                let cc = if i == 0 {
                    CC_ASSIGN_LEFT
                } else {
                    CC_ASSIGN_RIGHT
                };
                out.push(ChannelMessage::cc(0, cc, value).to_bytes().to_vec());
            }
        }
    }

    fn knob_page(&mut self, delta: i32, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
        if self.is_main() && self.vpot == VpotMode::Plugin {
            self.turn_page(delta, state, actions);
        }
    }

    fn disconnect(&mut self, out: &mut Vec<Vec<u8>>) {
        let mut off = self.header().to_vec();
        off.extend_from_slice(&[0x0F, 0xF7]);
        out.push(off);
        self.invalidate();
    }
}

#[cfg(test)]
#[path = "mcu_tests.rs"]
mod tests;
