//! Performance controllers shared by the playable instruments.
//!
//! A [`ControllerState`] lives in a device next to its voices and keeps what the channel
//! messages mean for the whole instrument:
//!
//! * **Sustain (CC 64).** While the pedal is down (value 64 or more) a note-off is held back:
//!   [`ControllerState::note_off`] says the voice must keep sounding and remembers the key in a
//!   fixed bit set. Lifting the pedal ([`ControllerState::handle`] returns
//!   [`Effect::ReleaseHeld`]) releases every remembered key at the frame of the pedal event with
//!   [`ControllerState::release_held`]. A note-on of a remembered key forgets it (the key is down
//!   again), `AllOff` releases everything and forgets every key but leaves the pedal where it is.
//! * **Volume and expression (CC 7, CC 11).** Both start at full and multiply: the gain is
//!   `(volume · expression)²`, smoothed over 5 ms and written per sample by
//!   [`ControllerState::fill_gain`]. At full the gain is exactly 1, so a device that never gets
//!   these controllers renders bit for bit as before.
//! * **Pressure.** Channel pressure and the polyphonic pressure of each key are kept as
//!   modulation sources: [`ControllerState::pressure`] is the larger of the channel value and
//!   the value of one key, `0..=1`.
//! * **Pitch bend and its range.** The bend is kept as `-1..=1`; its range in semitones starts at
//!   the `bend.range` param (default 2) and the registered parameter 0 (RPN 0, CC 101/100 select,
//!   CC 6/38 enter the semitones and cents) changes it on the fly. [`reset`](ControllerState::reset)
//!   returns to the param value.
//!
//! All of it is plain arrays and scalars (nothing is allocated, the hold list is two words), and
//! every change happens at the frame of its event, so renders do not depend on the block size.

use atomik_core::NoteKind;
use atomik_dsp::Smoother;

/// The pitch-bend range in semitones when nothing else is set.
pub const DEFAULT_BEND_RANGE: f32 = 2.0;
/// The widest pitch-bend range in semitones.
pub const MAX_BEND_RANGE: f32 = 24.0;
/// Time the volume and expression gain takes to follow a new value.
const GAIN_SMOOTH_MS: f32 = 5.0;
/// Pedal controller value at which the pedal counts as down (MIDI 64).
const PEDAL_DOWN: f32 = 0.5;

/// What the device has to do after [`ControllerState::handle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing beyond the state the helper already updated.
    None,
    /// The sustain pedal went up: call [`ControllerState::release_held`].
    ReleaseHeld,
}

/// Which registered parameter the RPN select controllers point at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rpn {
    /// Select controllers not seen, or the null parameter (127, 127).
    None,
    /// Parameter 0, the pitch-bend range.
    BendRange,
    /// Anything else: data entry is ignored.
    Other,
}

/// The per-instrument controller state. See the module docs.
#[derive(Debug, Clone)]
pub struct ControllerState {
    sustain: bool,
    /// Keys whose note-off arrived while the pedal was down, one bit per key.
    held: [u64; 2],
    volume: f32,
    expression: f32,
    gain: Smoother,
    channel_pressure: f32,
    poly_pressure: [f32; 128],
    /// Pitch bend, `-1..=1`.
    bend: f32,
    /// The range the `bend.range` param asks for, in semitones.
    range_param: f32,
    /// The range in force (the param, or what an RPN entered since).
    range: f32,
    rpn_select: [u8; 2],
    rpn: Rpn,
    /// Data-entry semitones and cents last entered for RPN 0.
    range_semitones: f32,
    range_cents: f32,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ControllerState {
    /// A state with no pedal, full volume, no pressure, centred bend and the default range.
    pub fn new() -> Self {
        Self {
            sustain: false,
            held: [0; 2],
            volume: 1.0,
            expression: 1.0,
            gain: Smoother::new(1.0, GAIN_SMOOTH_MS, 48_000.0),
            channel_pressure: 0.0,
            poly_pressure: [0.0; 128],
            bend: 0.0,
            range_param: DEFAULT_BEND_RANGE,
            range: DEFAULT_BEND_RANGE,
            rpn_select: [127, 127],
            rpn: Rpn::None,
            range_semitones: DEFAULT_BEND_RANGE,
            range_cents: 0.0,
        }
    }

    /// Sets the sample rate the gain smoothing runs at.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.gain.set_time(GAIN_SMOOTH_MS, sample_rate);
    }

    /// Returns to the power-on state: pedal up, nothing held, full volume, no pressure, centred
    /// bend and the range of the `bend.range` param.
    pub fn reset(&mut self) {
        self.sustain = false;
        self.held = [0; 2];
        self.volume = 1.0;
        self.expression = 1.0;
        self.gain.set_immediate(1.0);
        self.channel_pressure = 0.0;
        self.poly_pressure = [0.0; 128];
        self.bend = 0.0;
        self.range = self.range_param;
        self.rpn = Rpn::None;
        self.rpn_select = [127, 127];
        self.range_semitones = self.range_param.floor();
        self.range_cents = (self.range_param.fract() * 100.0).round();
    }

    /// The `bend.range` param changed: the range becomes `semitones`.
    pub fn set_bend_range(&mut self, semitones: f32) {
        let r = semitones.clamp(0.0, MAX_BEND_RANGE);
        self.range_param = r;
        self.range = r;
        self.range_semitones = r.floor();
        self.range_cents = (r.fract() * 100.0).round();
    }

    /// The pitch-bend range in force, in semitones each way.
    pub fn bend_range(&self) -> f32 {
        self.range
    }

    /// The pitch bend in semitones (`-range..=range`).
    pub fn bend_semitones(&self) -> f32 {
        self.bend * self.range
    }

    /// True while the sustain pedal is down.
    pub fn sustained(&self) -> bool {
        self.sustain
    }

    /// The channel and key pressure as one modulation source: the larger of the channel
    /// pressure and the pressure of `key`, `0..=1`.
    pub fn pressure(&self, key: u8) -> f32 {
        self.channel_pressure
            .max(self.poly_pressure[usize::from(key & 0x7f)])
    }

    /// The channel pressure alone, `0..=1`.
    pub fn channel_pressure(&self) -> f32 {
        self.channel_pressure
    }

    /// The gain that volume and expression ask for (the smoother's target), `0..=1`.
    pub fn target_gain(&self) -> f32 {
        let g = self.volume * self.expression;
        g * g
    }

    /// A note starts on `key`: the key is down again, so a held-back release is forgotten.
    pub fn note_on(&mut self, key: u8) {
        self.held[usize::from(key >> 6 & 1)] &= !(1u64 << (key & 63));
    }

    /// A note-off for `key`. True when the voice must be released now; false when the pedal is
    /// down and the release waits for it.
    pub fn note_off(&mut self, key: u8) -> bool {
        if self.sustain {
            self.held[usize::from(key >> 6 & 1)] |= 1u64 << (key & 63);
            false
        } else {
            true
        }
    }

    /// Every voice is released at once: nothing is held back any more (the pedal stays as it is).
    pub fn all_off(&mut self) {
        self.held = [0; 2];
    }

    /// Calls `release` for every key whose note-off the pedal held back (lowest first) and
    /// forgets them.
    pub fn release_held(&mut self, mut release: impl FnMut(u8)) {
        for (word, bits) in self.held.iter_mut().enumerate() {
            let mut b = std::mem::take(bits);
            while b != 0 {
                let bit = b.trailing_zeros();
                b &= b - 1;
                release((word as u32 * 64 + bit) as u8);
            }
        }
    }

    /// Takes a controller, pressure or bend event. Note events, the mod wheel and everything a
    /// device does by itself are left to the caller (they return [`Effect::None`]).
    pub fn handle(&mut self, kind: NoteKind) -> Effect {
        match kind {
            NoteKind::PitchBend { value } => self.bend = value.clamp(-1.0, 1.0),
            NoteKind::ChannelPressure { value } => self.channel_pressure = value.clamp(0.0, 1.0),
            NoteKind::PolyPressure { key, value } => {
                self.poly_pressure[usize::from(key & 0x7f)] = value.clamp(0.0, 1.0);
            }
            NoteKind::Cc { controller, value } => {
                return self.cc(controller, value.clamp(0.0, 1.0));
            }
            _ => {}
        }
        Effect::None
    }

    fn cc(&mut self, controller: u8, value: f32) -> Effect {
        // A 7-bit value as MIDI sent it.
        let raw = (value * 127.0).round() as u8;
        match controller {
            64 => {
                let down = value >= PEDAL_DOWN;
                let was = std::mem::replace(&mut self.sustain, down);
                if was && !down {
                    return Effect::ReleaseHeld;
                }
            }
            7 => {
                self.volume = value;
                self.gain.set_target(self.target_gain());
            }
            11 => {
                self.expression = value;
                self.gain.set_target(self.target_gain());
            }
            101 => {
                self.rpn_select[0] = raw;
                self.select_rpn();
            }
            100 => {
                self.rpn_select[1] = raw;
                self.select_rpn();
            }
            // Selecting a non-registered parameter ends the RPN data entry.
            98 | 99 => {
                self.rpn_select = [127, 127];
                self.rpn = Rpn::None;
            }
            6 if self.rpn == Rpn::BendRange => {
                self.range_semitones = f32::from(raw);
                self.apply_entered_range();
            }
            38 if self.rpn == Rpn::BendRange => {
                self.range_cents = f32::from(raw.min(99));
                self.apply_entered_range();
            }
            _ => {}
        }
        Effect::None
    }

    fn select_rpn(&mut self) {
        self.rpn = match self.rpn_select {
            [0, 0] => Rpn::BendRange,
            [127, 127] => Rpn::None,
            _ => Rpn::Other,
        };
    }

    fn apply_entered_range(&mut self) {
        self.range = (self.range_semitones + self.range_cents * 0.01).min(MAX_BEND_RANGE);
    }

    /// Writes the volume and expression gain for the next `out.len()` samples (smoothed, so a
    /// change ramps over 5 ms). Full volume writes exactly 1.
    pub fn fill_gain(&mut self, out: &mut [f32]) {
        for g in out {
            *g = self.gain.next_value();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(controller: u8, raw: u8) -> NoteKind {
        NoteKind::Cc {
            controller,
            value: f32::from(raw) / 127.0,
        }
    }

    #[test]
    fn pedal_holds_note_offs_and_releases_them_in_key_order() {
        let mut s = ControllerState::new();
        assert!(s.note_off(60), "pedal up: released at once");
        assert_eq!(s.handle(cc(64, 127)), Effect::None);
        assert!(s.sustained());
        assert!(!s.note_off(72));
        assert!(!s.note_off(60));
        assert!(!s.note_off(127));
        s.note_on(72);
        let mut keys = Vec::new();
        assert_eq!(s.handle(cc(64, 0)), Effect::ReleaseHeld);
        s.release_held(|k| keys.push(k));
        assert_eq!(keys, [60, 127]);
        s.release_held(|k| keys.push(k));
        assert_eq!(keys.len(), 2, "forgotten once released");
    }

    #[test]
    fn pedal_threshold_is_midi_64_and_all_off_forgets_held_keys() {
        let mut s = ControllerState::new();
        s.handle(cc(64, 63));
        assert!(!s.sustained());
        s.handle(cc(64, 64));
        assert!(s.sustained());
        assert!(!s.note_off(10));
        s.all_off();
        s.handle(cc(64, 0));
        let mut n = 0;
        s.release_held(|_| n += 1);
        assert_eq!(n, 0);
    }

    #[test]
    fn volume_and_expression_multiply_and_square() {
        let mut s = ControllerState::new();
        s.set_sample_rate(48_000.0);
        let mut g = [0.0; 4];
        s.fill_gain(&mut g);
        assert_eq!(g, [1.0; 4], "full volume is exactly one");
        s.handle(cc(7, 127));
        s.handle(cc(11, 0));
        assert_eq!(s.target_gain(), 0.0);
        s.handle(cc(11, 127));
        assert_eq!(s.target_gain(), 1.0);
        let half = NoteKind::Cc {
            controller: 7,
            value: 0.5,
        };
        s.handle(half);
        assert_eq!(s.target_gain(), 0.25);
        let mut ramp = [0.0; 480];
        s.fill_gain(&mut ramp);
        assert!(ramp[0] < 1.0 && ramp[0] > 0.25, "it ramps, no jump");
        assert_eq!(ramp[479], 0.25);
    }

    #[test]
    fn pressure_is_the_larger_of_channel_and_key() {
        let mut s = ControllerState::new();
        assert_eq!(s.pressure(60), 0.0);
        s.handle(NoteKind::PolyPressure {
            key: 60,
            value: 0.4,
        });
        assert_eq!((s.pressure(60), s.pressure(61)), (0.4, 0.0));
        s.handle(NoteKind::ChannelPressure { value: 0.7 });
        assert_eq!((s.pressure(60), s.pressure(61)), (0.7, 0.7));
        s.handle(NoteKind::PolyPressure {
            key: 60,
            value: 1.0,
        });
        assert_eq!(s.pressure(60), 1.0);
    }

    #[test]
    fn rpn_zero_sets_the_bend_range_and_the_param_resets_it() {
        let mut s = ControllerState::new();
        s.handle(NoteKind::PitchBend { value: 1.0 });
        assert_eq!(s.bend_semitones(), 2.0);
        // Data entry before a select does nothing.
        s.handle(cc(6, 12));
        assert_eq!(s.bend_range(), 2.0);
        s.handle(cc(101, 0));
        s.handle(cc(100, 0));
        s.handle(cc(6, 12));
        assert_eq!(s.bend_semitones(), 12.0);
        s.handle(cc(38, 50));
        assert_eq!(s.bend_range(), 12.5);
        // Another parameter, or the null one, is not the bend range.
        s.handle(cc(100, 1));
        s.handle(cc(6, 3));
        assert_eq!(s.bend_range(), 12.5);
        s.handle(cc(101, 127));
        s.handle(cc(100, 127));
        s.handle(cc(6, 3));
        assert_eq!(s.bend_range(), 12.5);
        s.set_bend_range(7.0);
        assert_eq!(s.bend_semitones(), 7.0);
        s.handle(cc(101, 0));
        s.handle(cc(100, 0));
        s.handle(cc(6, 24));
        s.handle(cc(6, 100));
        assert_eq!(s.bend_range(), MAX_BEND_RANGE, "clamped");
        s.reset();
        assert_eq!((s.bend_range(), s.bend_semitones()), (7.0, 0.0));
    }

    #[test]
    fn reset_clears_everything_but_the_param() {
        let mut s = ControllerState::new();
        s.handle(cc(64, 127));
        s.note_off(5);
        s.handle(cc(7, 10));
        s.handle(NoteKind::ChannelPressure { value: 1.0 });
        s.reset();
        assert!(!s.sustained());
        assert_eq!((s.target_gain(), s.pressure(5)), (1.0, 0.0));
        let mut n = 0;
        s.release_held(|_| n += 1);
        assert_eq!(n, 0);
    }
}
