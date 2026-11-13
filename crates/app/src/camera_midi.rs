//! The camera as a virtual MIDI controller: what each hand sends.
//!
//! [`CameraMidi`] is pure: it takes what the hand tracker saw (a frame's [`HandState`] and the
//! gesture events) and returns [`ChannelMessage`]s. The app core feeds them into the same input
//! path as hardware MIDI (see `app_core_camera_midi.rs`); nothing here touches a device or the
//! clock, so tests drive it with a time of their own.
//!
//! # The controls
//!
//! Each hand has a channel (the right hand 1, the left hand 2) and a fixed set of messages
//! (documented in `atomik_model::camera_source`):
//!
//! * **twist** while pinched: a relative encoder, controller 20 in two's complement. A full turn
//!   of the setting (default 270 degrees) is 64 detents, so a mapping with the default encoder
//!   sensitivity sweeps its whole range over one full turn. The turn since the last message is
//!   kept (never lost) while the rate limit holds a message back.
//! * **pinch**: note 60, on at the pinch and off at the release.
//! * **palm height**: controller 21, over the middle 80% of the frame, the top is 127.
//! * **palm left and right**: controller 22, over the middle 80%, as the user sees the mirrored
//!   preview (moving the hand to the right raises it).
//! * **closeness**: controller 23, from a hand far away (0) to one near the camera (127).
//! * **open palm**: note 61.
//! * **poses and swipes**: fist note 62, thumbs up 63, point 64, peace 65 (each on while the pose
//!   is held, off when it ends), swipe left 66 and swipe right 67 (on and off at once).
//! * **the pad** (both hands pinched), on channel 3: the midpoint as controllers 24 (left and
//!   right, like controller 22) and 25 (height, like controller 21) and the distance between the
//!   hands as controller 26. They hold their values when the hands let go.
//!
//! # The rate
//!
//! A controller sends only when its value moved by a step of 1 or more, and at most
//! [`MAX_PER_SECOND`] times a second per control. The value comes from the recogniser's filtered
//! palm position, so it is already smoothed. A hand that is lost sends nothing: its controllers
//! hold their last value (there is no jump to 0), and its notes end with the recogniser's
//! release events.

use atomik_core::{ChannelMessage, MessageKind};
use atomik_model::{
    BOTH_HANDS_CHANNEL, CLOSENESS_CC, CameraControl, CameraGesture, CameraHand, HEIGHT_CC,
    OPEN_PALM_NOTE, PAD_SPREAD_CC, PAD_X_CC, PAD_Y_CC, PINCH_NOTE, SIDE_CC, TWIST_CC,
};
use atomik_vision::Handedness;
use atomik_vision::gesture::{HandId, HandState, Pad, Pose, SwipeDirection};
use std::collections::BTreeMap;

/// The most messages one control sends in a second.
pub const MAX_PER_SECOND: f64 = 60.0;
/// The detents of a full turn of the twist setting.
pub const DETENTS_PER_TURN: f32 = 64.0;
/// The part of the frame, at each edge, that the palm controllers leave out (the middle 80% is
/// the range).
const EDGE: f32 = 0.1;
/// The hand size (in frame heights) at or below which closeness is 0.
pub const FAR_SIZE: f32 = 0.12;
/// The hand size at or above which closeness is 127.
pub const NEAR_SIZE: f32 = 0.30;
/// The velocity of the notes.
const VELOCITY: u8 = 127;
/// The twist bar of the preview falls back by this factor each frame.
const TWIST_FALL: f32 = 0.85;

/// The channel of the hand the model labels `handedness`.
pub fn hand_of(handedness: Handedness) -> CameraHand {
    match handedness {
        Handedness::Right => CameraHand::Right,
        Handedness::Left => CameraHand::Left,
    }
}

fn unit_to_cc(unit: f32) -> u8 {
    (unit.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// The height controller of a palm centre at image height `y` (0 top, 1 bottom).
pub fn height_value(y: f32) -> u8 {
    unit_to_cc((1.0 - EDGE - y) / (1.0 - 2.0 * EDGE))
}

/// The left and right controller of a palm centre at image position `x` (0 is the left of the
/// raw camera image, which is the right of the mirrored preview).
pub fn side_value(x: f32) -> u8 {
    unit_to_cc((1.0 - EDGE - x) / (1.0 - 2.0 * EDGE))
}

/// The closeness controller of a hand of `size` frame heights.
pub fn closeness_value(size: f32) -> u8 {
    unit_to_cc((size - FAR_SIZE) / (NEAR_SIZE - FAR_SIZE))
}

/// The distance between the hands (in frame heights) at or below which the pad's spread is 0.
pub const SPREAD_MIN: f32 = 0.15;
/// The distance between the hands at or above which the pad's spread is 127.
pub const SPREAD_MAX: f32 = 1.2;

/// The spread controller of hands `spread` frame heights apart.
pub fn spread_value(spread: f32) -> u8 {
    unit_to_cc((spread - SPREAD_MIN) / (SPREAD_MAX - SPREAD_MIN))
}

/// The gesture a pose is.
pub fn gesture_of(pose: Pose) -> CameraGesture {
    match pose {
        Pose::Fist => CameraGesture::Fist,
        Pose::ThumbsUp => CameraGesture::ThumbsUp,
        Pose::Point => CameraGesture::Point,
        Pose::Peace => CameraGesture::Peace,
    }
}

/// The gesture a swipe is.
pub fn swipe_gesture(direction: SwipeDirection) -> CameraGesture {
    match direction {
        SwipeDirection::Left => CameraGesture::SwipeLeft,
        SwipeDirection::Right => CameraGesture::SwipeRight,
    }
}

/// A controller that sends only when it moved.
#[derive(Debug, Clone, Copy, Default)]
struct Absolute {
    last: Option<u8>,
    at: f64,
}

impl Absolute {
    /// Sends `value` when it is new and the control is not in its pause.
    fn update(&mut self, t: f64, value: u8) -> Option<u8> {
        if self.last == Some(value) {
            return None;
        }
        if self.last.is_some() && t - self.at < 1.0 / MAX_PER_SECOND - 1e-9 {
            return None;
        }
        self.last = Some(value);
        self.at = t;
        Some(value)
    }
}

/// What one hand has sent.
#[derive(Debug, Clone)]
struct Out {
    hand: CameraHand,
    present: bool,
    /// The turn not sent yet, in detents.
    detents: f32,
    twist_at: Option<f64>,
    /// How far the last twist message went, for the preview (-1 to 1, falling back).
    twist_level: f32,
    height: Absolute,
    side: Absolute,
    closeness: Absolute,
    pinch: bool,
    palm: bool,
    /// The pose whose note is on.
    pose: Option<Pose>,
}

impl Out {
    fn new(hand: CameraHand) -> Self {
        Self {
            hand,
            present: false,
            detents: 0.0,
            twist_at: None,
            twist_level: 0.0,
            height: Absolute::default(),
            side: Absolute::default(),
            closeness: Absolute::default(),
            pinch: false,
            palm: false,
            pose: None,
        }
    }
}

/// What the pad has sent.
#[derive(Debug, Clone, Copy, Default)]
struct PadOut {
    x: Absolute,
    y: Absolute,
    spread: Absolute,
}

/// The messages the camera's hands send. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct CameraMidi {
    hands: BTreeMap<HandId, Out>,
    pad: PadOut,
}

/// One control of the preview: a bar showing what the camera sent last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// Which control.
    pub control: CameraControl,
    /// 0 to 1 (an encoder's bar is -1 to 1).
    pub value: f32,
}

fn cc(channel: u8, number: u8, value: u8) -> ChannelMessage {
    ChannelMessage {
        channel,
        kind: MessageKind::Cc { number, value },
    }
}

fn note(channel: u8, key: u8, on: bool) -> ChannelMessage {
    ChannelMessage {
        channel,
        kind: if on {
            MessageKind::NoteOn {
                key,
                velocity: VELOCITY,
            }
        } else {
            MessageKind::NoteOff { key }
        },
    }
}

impl CameraMidi {
    /// A camera that has sent nothing.
    pub fn new() -> Self {
        Self::default()
    }

    fn out(&mut self, id: HandId, hand: CameraHand) -> &mut Out {
        let out = self.hands.entry(id).or_insert_with(|| Out::new(hand));
        out.hand = hand;
        out
    }

    /// Forgets every hand and what it sent (the camera was switched off or the mode changed).
    pub fn reset(&mut self) {
        self.hands.clear();
        self.pad = PadOut::default();
    }

    /// The pad (both hands pinched) at `t`: its controllers on the third channel, only when they
    /// moved. With `xy` off only the spread is sent (the position is turning two knobs). A pad
    /// that has ended sends nothing: the controllers hold their last values.
    pub fn pad(&mut self, t: f64, pad: Option<Pad>, xy: bool, out: &mut Vec<ChannelMessage>) {
        let Some(pad) = pad else { return };
        let channel = BOTH_HANDS_CHANNEL;
        if xy {
            if let Some(v) = self.pad.x.update(t, side_value(pad.x)) {
                out.push(cc(channel, PAD_X_CC, v));
            }
            if let Some(v) = self.pad.y.update(t, height_value(pad.y)) {
                out.push(cc(channel, PAD_Y_CC, v));
            }
        }
        if let Some(v) = self.pad.spread.update(t, spread_value(pad.spread)) {
            out.push(cc(channel, PAD_SPREAD_CC, v));
        }
    }

    /// A pose has been held long enough: its note goes on.
    pub fn pose_start(
        &mut self,
        id: HandId,
        handedness: Handedness,
        pose: Pose,
        out: &mut Vec<ChannelMessage>,
    ) {
        let hand = hand_of(handedness);
        let o = self.out(id, hand);
        if o.pose != Some(pose) {
            if let Some(old) = o.pose.replace(pose) {
                out.push(note(hand.channel(), gesture_of(old).note(), false));
            }
            out.push(note(hand.channel(), gesture_of(pose).note(), true));
        }
    }

    /// The pose ended (or the hand was lost): its note goes off.
    pub fn pose_end(&mut self, id: HandId, out: &mut Vec<ChannelMessage>) {
        if let Some(o) = self.hands.get_mut(&id)
            && let Some(pose) = o.pose.take()
        {
            out.push(note(o.hand.channel(), gesture_of(pose).note(), false));
        }
    }

    /// A swipe: its note goes on and off at once.
    pub fn swipe(
        &mut self,
        handedness: Handedness,
        direction: SwipeDirection,
        out: &mut Vec<ChannelMessage>,
    ) {
        let channel = hand_of(handedness).channel();
        let key = swipe_gesture(direction).note();
        out.push(note(channel, key, true));
        out.push(note(channel, key, false));
    }

    /// A frame: the absolute controllers of every hand that is in view. A hand that is not in
    /// view sends nothing (its controllers hold their last value).
    pub fn frame(&mut self, t: f64, hands: &[HandState], out: &mut Vec<ChannelMessage>) {
        for out_hand in self.hands.values_mut() {
            out_hand.present = false;
            out_hand.twist_level *= TWIST_FALL;
        }
        for h in hands.iter().filter(|h| h.present) {
            let channel = hand_of(h.handedness).channel();
            let o = self.out(h.id, hand_of(h.handedness));
            o.present = true;
            if let Some(v) = o.height.update(t, height_value(h.palm_center.1)) {
                out.push(cc(channel, HEIGHT_CC, v));
            }
            if let Some(v) = o.side.update(t, side_value(h.palm_center.0)) {
                out.push(cc(channel, SIDE_CC, v));
            }
            if let Some(v) = o.closeness.update(t, closeness_value(h.depth_proxy)) {
                out.push(cc(channel, CLOSENESS_CC, v));
            }
            Self::flush_twist(o, t, out);
        }
    }

    /// The thumb and index finger came together.
    pub fn pinch_start(
        &mut self,
        id: HandId,
        handedness: Handedness,
        out: &mut Vec<ChannelMessage>,
    ) {
        let hand = hand_of(handedness);
        let o = self.out(id, hand);
        o.detents = 0.0;
        if !o.pinch {
            o.pinch = true;
            out.push(note(hand.channel(), PINCH_NOTE, true));
        }
    }

    /// The fingers opened (or the hand was lost): the pinch note ends, and a turn not sent yet
    /// is dropped.
    pub fn pinch_end(&mut self, id: HandId, out: &mut Vec<ChannelMessage>) {
        if let Some(o) = self.hands.get_mut(&id) {
            o.detents = 0.0;
            if std::mem::take(&mut o.pinch) {
                out.push(note(o.hand.channel(), PINCH_NOTE, false));
            }
        }
    }

    /// The palm has been open long enough.
    pub fn palm_start(
        &mut self,
        id: HandId,
        handedness: Handedness,
        out: &mut Vec<ChannelMessage>,
    ) {
        let hand = hand_of(handedness);
        let o = self.out(id, hand);
        if !o.palm {
            o.palm = true;
            out.push(note(hand.channel(), OPEN_PALM_NOTE, true));
        }
    }

    /// The palm closed (or the hand was lost).
    pub fn palm_end(&mut self, id: HandId, out: &mut Vec<ChannelMessage>) {
        if let Some(o) = self.hands.get_mut(&id)
            && std::mem::take(&mut o.palm)
        {
            out.push(note(o.hand.channel(), OPEN_PALM_NOTE, false));
        }
    }

    /// The wrist turned by `radians` (positive: clockwise) while pinched. `full_turn` is the
    /// setting's degrees for a whole range.
    pub fn twist(
        &mut self,
        t: f64,
        id: HandId,
        radians: f32,
        full_turn: f32,
        out: &mut Vec<ChannelMessage>,
    ) {
        let Some(o) = self.hands.get_mut(&id) else {
            return;
        };
        o.detents += radians.to_degrees() / full_turn.max(1.0) * DETENTS_PER_TURN;
        Self::flush_twist(o, t, out);
    }

    /// Sends the whole detents of the turn kept back, when the control's pause is over.
    fn flush_twist(o: &mut Out, t: f64, out: &mut Vec<ChannelMessage>) {
        let whole = (o.detents + o.detents.signum() * 1e-4)
            .trunc()
            .clamp(-63.0, 63.0);
        if whole == 0.0 {
            return;
        }
        if o.twist_at
            .is_some_and(|at| t - at < 1.0 / MAX_PER_SECOND - 1e-9)
        {
            return;
        }
        o.detents -= whole;
        o.twist_at = Some(t);
        o.twist_level = whole / 63.0;
        // Two's complement in 7 bits: 1..=63 clockwise, 127 down to 65 counter-clockwise.
        let value = (whole as i32 & 0x7F) as u8;
        out.push(cc(o.hand.channel(), TWIST_CC, value));
    }

    /// Drops the turn kept back (the twist is not being sent any more).
    pub fn drop_twist(&mut self) {
        for o in self.hands.values_mut() {
            o.detents = 0.0;
        }
    }

    /// The note-offs of every note still held (the camera went off, or its MIDI did).
    pub fn release_all(&mut self, out: &mut Vec<ChannelMessage>) {
        for o in self.hands.values_mut() {
            if std::mem::take(&mut o.pinch) {
                out.push(note(o.hand.channel(), PINCH_NOTE, false));
            }
            if std::mem::take(&mut o.palm) {
                out.push(note(o.hand.channel(), OPEN_PALM_NOTE, false));
            }
            if let Some(pose) = o.pose.take() {
                out.push(note(o.hand.channel(), gesture_of(pose).note(), false));
            }
        }
    }

    /// Whether any note is held.
    pub fn holds_notes(&self) -> bool {
        self.hands
            .values()
            .any(|o| o.pinch || o.palm || o.pose.is_some())
    }

    /// The hand the preview shows (the first in view, else the first seen) and one bar for each
    /// control, in the order of [`CameraControl::ALL`].
    pub fn bars(&self) -> Option<(CameraHand, [Bar; 6])> {
        let o = self
            .hands
            .values()
            .find(|o| o.present)
            .or_else(|| self.hands.values().next())?;
        let level = |a: &Absolute| a.last.map_or(0.0, |v| f32::from(v) / 127.0);
        let bar = |control, value| Bar { control, value };
        Some((
            o.hand,
            [
                bar(CameraControl::Twist, o.twist_level),
                bar(CameraControl::Height, level(&o.height)),
                bar(CameraControl::Side, level(&o.side)),
                bar(CameraControl::Closeness, level(&o.closeness)),
                bar(CameraControl::Pinch, if o.pinch { 1.0 } else { 0.0 }),
                bar(CameraControl::OpenPalm, if o.palm { 1.0 } else { 0.0 }),
            ],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(id: u32, handedness: Handedness, x: f32, y: f32, size: f32) -> HandState {
        HandState {
            id: HandId(id),
            present: true,
            handedness,
            pinch: false,
            pinch_strength: 0.0,
            twist_angle: 0.0,
            palm_center: (x, y),
            palm_height: 0.2,
            hand_size: size,
            depth_proxy: size,
            open_palm: false,
            pose: None,
        }
    }

    fn frame(m: &mut CameraMidi, t: f64, hands: &[HandState]) -> Vec<ChannelMessage> {
        let mut out = Vec::new();
        m.frame(t, hands, &mut out);
        out
    }

    fn ccs(out: &[ChannelMessage], number: u8) -> Vec<(u8, u8)> {
        out.iter()
            .filter_map(|m| match m.kind {
                MessageKind::Cc { number: n, value } if n == number => Some((m.channel, value)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn height_runs_over_the_middle_80_percent_with_the_top_at_127() {
        assert_eq!(height_value(0.1), 127);
        assert_eq!(height_value(0.0), 127, "above the band is the top");
        assert_eq!(height_value(0.9), 0);
        assert_eq!(height_value(1.0), 0);
        assert_eq!(height_value(0.3), 95);
    }

    #[test]
    fn left_and_right_rise_as_the_hand_moves_right_in_the_mirrored_preview() {
        assert_eq!(side_value(0.9), 0, "the far left of the preview");
        assert_eq!(side_value(0.1), 127, "the far right of the preview");
        assert!(side_value(0.3) > side_value(0.6));
    }

    #[test]
    fn closeness_runs_from_a_far_hand_to_a_near_one() {
        assert_eq!(closeness_value(0.05), 0);
        assert_eq!(closeness_value(FAR_SIZE), 0);
        assert_eq!(closeness_value(NEAR_SIZE), 127);
        assert_eq!(closeness_value(0.5), 127);
        assert!(closeness_value(0.2) > closeness_value(0.15));
    }

    #[test]
    fn each_hand_has_its_own_channel() {
        let mut m = CameraMidi::new();
        let out = frame(
            &mut m,
            0.0,
            &[
                hand(0, Handedness::Right, 0.5, 0.5, 0.2),
                hand(1, Handedness::Left, 0.3, 0.5, 0.2),
            ],
        );
        let heights = ccs(&out, HEIGHT_CC);
        assert_eq!(heights.len(), 2);
        assert!(
            heights.iter().any(|h| h.0 == 0),
            "the right hand is channel 1"
        );
        assert!(
            heights.iter().any(|h| h.0 == 1),
            "the left hand is channel 2"
        );
    }

    #[test]
    fn a_controller_sends_only_when_its_value_moved() {
        let mut m = CameraMidi::new();
        let h = hand(0, Handedness::Right, 0.5, 0.5, 0.2);
        assert_eq!(ccs(&frame(&mut m, 0.0, &[h]), HEIGHT_CC).len(), 1);
        assert!(
            frame(&mut m, 0.1, &[h]).is_empty(),
            "the same value is not sent again"
        );
        let moved = hand(0, Handedness::Right, 0.5, 0.3, 0.2);
        assert_eq!(ccs(&frame(&mut m, 0.2, &[moved]), HEIGHT_CC).len(), 1);
    }

    #[test]
    fn a_control_sends_at_most_60_times_a_second() {
        let mut m = CameraMidi::new();
        let mut sent = 0;
        // The hand rises steadily, 240 frames a second for a second: the value changes every
        // frame, the control may send 60 times.
        for i in 0..240 {
            let y = 0.9 - 0.8 * (i as f32 / 240.0);
            let out = frame(
                &mut m,
                f64::from(i) / 240.0,
                &[hand(0, Handedness::Right, 0.5, y, 0.2)],
            );
            sent += ccs(&out, HEIGHT_CC).len();
        }
        assert!((55..=60).contains(&sent), "{sent} messages");
    }

    #[test]
    fn a_lost_hand_sends_nothing_and_holds_its_values() {
        let mut m = CameraMidi::new();
        frame(&mut m, 0.0, &[hand(0, Handedness::Right, 0.5, 0.2, 0.2)]);
        assert!(frame(&mut m, 0.1, &[]).is_empty());
        let mut gone = hand(0, Handedness::Right, 0.5, 0.9, 0.0);
        gone.present = false;
        assert!(frame(&mut m, 0.2, &[gone]).is_empty(), "no jump to zero");
        let (_, bars) = m.bars().unwrap();
        assert!(
            bars[1].value > 0.8,
            "the bar holds its value: {:?}",
            bars[1]
        );
        // The hand coming back at the same place is not news either.
        assert!(frame(&mut m, 0.3, &[hand(0, Handedness::Right, 0.5, 0.2, 0.2)]).is_empty());
    }

    #[test]
    fn a_twist_is_an_endless_encoder_in_twos_complement() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pinch_start(HandId(0), Handedness::Right, &mut out);
        out.clear();
        // 270 degrees is a full turn: 64 detents, but one message carries at most 63.
        m.twist(0.0, HandId(0), 42.1875f32.to_radians(), 270.0, &mut out);
        assert_eq!(ccs(&out, TWIST_CC), vec![(0, 10)], "10 detents clockwise");
        out.clear();
        m.twist(1.0, HandId(0), (-16.875f32).to_radians(), 270.0, &mut out);
        assert_eq!(
            ccs(&out, TWIST_CC),
            vec![(0, 128 - 4)],
            "4 detents back: 124"
        );
        // The decoder the mappings use reads them as +10 and -4.
        let kind = atomik_model::RelativeKind::TwosComplement;
        assert_eq!(kind.detents(10), 10);
        assert_eq!(kind.detents(124), -4);
    }

    #[test]
    fn a_turn_too_small_for_a_detent_is_kept_until_it_adds_up() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pinch_start(HandId(0), Handedness::Right, &mut out);
        out.clear();
        for i in 0..10 {
            m.twist(
                f64::from(i),
                HandId(0),
                1.0f32.to_radians(),
                270.0,
                &mut out,
            );
        }
        // 10 degrees of a 270 degree turn is 2.4 detents: two messages' worth in total.
        let total: i32 = ccs(&out, TWIST_CC).iter().map(|c| i32::from(c.1)).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn the_twist_obeys_the_rate_and_loses_no_turn() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pinch_start(HandId(0), Handedness::Right, &mut out);
        out.clear();
        let mut total = 0;
        // 480 twists in one second, each exactly one detent: only 60 messages, but the
        // detents not sent wait and arrive in the next ones.
        for i in 0..480 {
            let one = 270.0 / DETENTS_PER_TURN;
            m.twist(
                f64::from(i) / 480.0,
                HandId(0),
                one.to_radians(),
                270.0,
                &mut out,
            );
        }
        let sent = ccs(&out, TWIST_CC);
        assert!(sent.len() <= 60, "{} messages", sent.len());
        for (_, v) in &sent {
            total += kind_detents(*v);
        }
        let waiting = m.hands[&HandId(0)].detents.round() as i32;
        assert_eq!(total + waiting, 480);
    }

    fn kind_detents(v: u8) -> i32 {
        atomik_model::RelativeKind::TwosComplement.detents(v)
    }

    #[test]
    fn the_pinch_and_the_open_palm_are_notes_on_and_off() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pinch_start(HandId(0), Handedness::Left, &mut out);
        m.palm_start(HandId(0), Handedness::Left, &mut out);
        assert_eq!(
            out,
            vec![note(1, 60, true), note(1, 61, true)],
            "the left hand is channel 2"
        );
        out.clear();
        // A second start with the note on sends nothing.
        m.pinch_start(HandId(0), Handedness::Left, &mut out);
        assert!(out.is_empty());
        m.pinch_end(HandId(0), &mut out);
        m.palm_end(HandId(0), &mut out);
        assert_eq!(out, vec![note(1, 60, false), note(1, 61, false)]);
        out.clear();
        m.pinch_end(HandId(0), &mut out);
        assert!(out.is_empty(), "no note off without a note on");
    }

    #[test]
    fn release_all_ends_every_held_note() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pinch_start(HandId(0), Handedness::Right, &mut out);
        m.palm_start(HandId(1), Handedness::Left, &mut out);
        assert!(m.holds_notes());
        out.clear();
        m.release_all(&mut out);
        assert_eq!(out, vec![note(0, 60, false), note(1, 61, false)]);
        assert!(!m.holds_notes());
    }

    #[test]
    fn the_preview_shows_the_hand_in_view() {
        let mut m = CameraMidi::new();
        assert!(m.bars().is_none());
        frame(&mut m, 0.0, &[hand(0, Handedness::Left, 0.5, 0.1, 0.2)]);
        let (hand, bars) = m.bars().unwrap();
        assert_eq!(hand, CameraHand::Left);
        assert_eq!(bars.map(|b| b.control), CameraControl::ALL);
        assert_eq!(bars[1].value, 1.0, "the palm is at the top");
    }

    // ---- Poses, swipes and the pad ----------------------------------------------------------

    #[test]
    fn each_pose_is_a_note_on_its_hands_channel_while_it_is_held() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        for (pose, key) in [
            (Pose::Fist, 62),
            (Pose::ThumbsUp, 63),
            (Pose::Point, 64),
            (Pose::Peace, 65),
        ] {
            out.clear();
            m.pose_start(HandId(0), Handedness::Right, pose, &mut out);
            assert_eq!(out, vec![note(0, key, true)]);
            assert!(m.holds_notes());
            out.clear();
            m.pose_end(HandId(0), &mut out);
            assert_eq!(out, vec![note(0, key, false)]);
            assert!(!m.holds_notes());
        }
        out.clear();
        m.pose_start(HandId(1), Handedness::Left, Pose::Peace, &mut out);
        assert_eq!(out, vec![note(1, 65, true)], "the left hand is channel 2");
        out.clear();
        m.pose_end(HandId(1), &mut out);
        m.pose_end(HandId(1), &mut out);
        assert_eq!(out.len(), 1, "one note off for one note on");
    }

    #[test]
    fn a_swipe_is_a_note_on_and_off_at_once() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.swipe(Handedness::Right, SwipeDirection::Left, &mut out);
        m.swipe(Handedness::Left, SwipeDirection::Right, &mut out);
        assert_eq!(
            out,
            vec![
                note(0, 66, true),
                note(0, 66, false),
                note(1, 67, true),
                note(1, 67, false)
            ]
        );
        assert!(!m.holds_notes());
    }

    #[test]
    fn the_pad_sends_three_controllers_on_channel_3_and_only_when_they_move() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        let pad = |x, y, spread| Some(Pad { x, y, spread });
        m.pad(0.0, pad(0.5, 0.5, 0.675), true, &mut out);
        assert_eq!(
            out,
            vec![
                cc(2, 24, side_value(0.5)),
                cc(2, 25, height_value(0.5)),
                cc(2, 26, spread_value(0.675)),
            ]
        );
        assert!(
            [cc(2, 26, 63), cc(2, 26, 64)].contains(&out[2]),
            "half way between the two ends"
        );
        out.clear();
        m.pad(0.1, pad(0.5, 0.5, 0.675), true, &mut out);
        assert!(out.is_empty(), "nothing moved");
        // Only the height moved; then it moves again inside the pause of 1/60 s and waits.
        m.pad(0.2, pad(0.5, 0.3, 0.675), true, &mut out);
        assert_eq!(out, vec![cc(2, 25, height_value(0.3))]);
        out.clear();
        m.pad(0.205, pad(0.5, 0.5, 0.675), true, &mut out);
        assert!(out.is_empty(), "inside the pause");
        m.pad(0.3, pad(0.5, 0.5, 0.675), true, &mut out);
        assert_eq!(out, vec![cc(2, 25, height_value(0.5))]);
        // The pad ends: no more messages, the controllers hold their values.
        out.clear();
        m.pad(0.3, None, true, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn with_the_position_driving_knobs_only_the_spread_is_sent() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pad(
            0.0,
            Some(Pad {
                x: 0.2,
                y: 0.8,
                spread: 0.3,
            }),
            false,
            &mut out,
        );
        assert_eq!(out, vec![cc(2, 26, spread_value(0.3))]);
    }

    #[test]
    fn the_spread_runs_from_the_hands_touching_to_the_width_of_the_frame() {
        assert_eq!(spread_value(0.0), 0);
        assert_eq!(spread_value(SPREAD_MIN), 0);
        assert_eq!(spread_value(SPREAD_MAX), 127);
        assert_eq!(spread_value(2.0), 127);
        assert!(spread_value(0.8) > spread_value(0.4));
    }

    #[test]
    fn release_all_ends_a_held_pose_too() {
        let mut m = CameraMidi::new();
        let mut out = Vec::new();
        m.pose_start(HandId(0), Handedness::Right, Pose::Fist, &mut out);
        out.clear();
        m.release_all(&mut out);
        assert_eq!(out, vec![note(0, 62, false)]);
    }
}
