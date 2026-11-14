//! The camera as a MIDI input: how a mapping names it and which controls it has.
//!
//! The webcam hand tracker (see the app's `camera_midi`) turns each hand into a fixed set of
//! MIDI messages that enter the same input path as hardware. A mapping learned from them uses
//! the fields every mapping has, so the project format is unchanged: its source port is
//! [`CAMERA_PORT`] and its channel is the hand's (1 for the right hand, 2 for the left). Nothing
//! else hears the camera: a mapping on any port, or on a real port, never sees these messages,
//! and a hardware controller sending the same controller number on the same channel never
//! drives a camera mapping.
//!
//! The controls are fixed and documented in the user manual:
//!
//! | control | message |
//! | --- | --- |
//! | twist while pinched | relative encoder, controller 20 (two's complement) |
//! | pinch | note 60 (on at the pinch, off at the release) |
//! | palm height | controller 21, top = 127 |
//! | palm left and right | controller 22 |
//! | hand closeness | controller 23, nearer = 127 |
//! | open palm | note 61 |
//! | fist | note 62 |
//! | thumbs up | note 63 |
//! | point (index finger up) | note 64 |
//! | peace (index and middle fingers up) | note 65 |
//! | swipe left | note 66 (on, then off at once) |
//! | swipe right | note 67 (on, then off at once) |
//!
//! The gestures send on the channel of their hand. Both hands pinched make the **pad**, which
//! sends on channel 3 ("both hands"): the midpoint of the hands as controllers 24 (left and
//! right) and 25 (height), and the distance between the hands as controller 26 (spread).

use crate::midi_map::{MidiSource, SourceKind};

/// The input name a camera mapping saves as its source port. It is a reserved name (no real
/// MIDI port has it), shown to the user as "Camera".
pub const CAMERA_PORT: &str = "atomik:camera";

/// The channel of the right hand (0-based; shown as channel 1).
pub const RIGHT_HAND_CHANNEL: u8 = 0;
/// The channel of the left hand (0-based; shown as channel 2).
pub const LEFT_HAND_CHANNEL: u8 = 1;

/// The twist: a relative encoder (two's complement).
pub const TWIST_CC: u8 = 20;
/// The palm height controller.
pub const HEIGHT_CC: u8 = 21;
/// The palm left and right controller.
pub const SIDE_CC: u8 = 22;
/// The hand closeness controller.
pub const CLOSENESS_CC: u8 = 23;
/// The pinch note.
pub const PINCH_NOTE: u8 = 60;
/// The open palm note.
pub const OPEN_PALM_NOTE: u8 = 61;
/// The fist note.
pub const FIST_NOTE: u8 = 62;
/// The thumbs up note.
pub const THUMBS_UP_NOTE: u8 = 63;
/// The point note.
pub const POINT_NOTE: u8 = 64;
/// The peace note.
pub const PEACE_NOTE: u8 = 65;
/// The swipe left note.
pub const SWIPE_LEFT_NOTE: u8 = 66;
/// The swipe right note.
pub const SWIPE_RIGHT_NOTE: u8 = 67;
/// The channel of the two-hand pad (0-based; shown as channel 3).
pub const BOTH_HANDS_CHANNEL: u8 = 2;
/// The pad's left and right controller.
pub const PAD_X_CC: u8 = 24;
/// The pad's height controller.
pub const PAD_Y_CC: u8 = 25;
/// The pad's spread controller.
pub const PAD_SPREAD_CC: u8 = 26;

/// A hand of the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraHand {
    /// The right hand, channel 1.
    Right,
    /// The left hand, channel 2.
    Left,
    /// Both hands, channel 3 (the two-hand pad).
    Both,
}

impl CameraHand {
    /// The 0-based MIDI channel of this hand.
    pub fn channel(self) -> u8 {
        match self {
            CameraHand::Right => RIGHT_HAND_CHANNEL,
            CameraHand::Left => LEFT_HAND_CHANNEL,
            CameraHand::Both => BOTH_HANDS_CHANNEL,
        }
    }

    /// The hand on `channel` (0-based), if it is one of the two.
    pub fn from_channel(channel: u8) -> Option<Self> {
        match channel {
            RIGHT_HAND_CHANNEL => Some(CameraHand::Right),
            LEFT_HAND_CHANNEL => Some(CameraHand::Left),
            BOTH_HANDS_CHANNEL => Some(CameraHand::Both),
            _ => None,
        }
    }

    /// The words for lists: `right hand`.
    pub fn label(self) -> &'static str {
        match self {
            CameraHand::Right => "right hand",
            CameraHand::Left => "left hand",
            CameraHand::Both => "both hands",
        }
    }
}

/// One control of a hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraControl {
    /// The wrist's turn while the fingers are pinched: a relative encoder.
    Twist,
    /// The pinch: a button.
    Pinch,
    /// How high the palm is in the frame.
    Height,
    /// How far left or right the palm is.
    Side,
    /// How near the hand is to the camera.
    Closeness,
    /// The open palm: a button.
    OpenPalm,
    /// A fist: a button.
    Fist,
    /// A thumbs up: a button.
    ThumbsUp,
    /// The index finger up: a button.
    Point,
    /// The index and middle fingers up: a button.
    Peace,
    /// A swipe to the left: a button.
    SwipeLeft,
    /// A swipe to the right: a button.
    SwipeRight,
    /// The pad's left and right (both hands).
    PadX,
    /// The pad's height (both hands).
    PadY,
    /// The pad's spread (both hands).
    PadSpread,
}

impl CameraControl {
    /// Every control, in the order the preview shows them.
    pub const ALL: [CameraControl; 6] = [
        CameraControl::Twist,
        CameraControl::Height,
        CameraControl::Side,
        CameraControl::Closeness,
        CameraControl::Pinch,
        CameraControl::OpenPalm,
    ];

    /// Every control the camera sends, the ones of [`Self::ALL`] first.
    pub const EVERY: [CameraControl; 15] = [
        CameraControl::Twist,
        CameraControl::Height,
        CameraControl::Side,
        CameraControl::Closeness,
        CameraControl::Pinch,
        CameraControl::OpenPalm,
        CameraControl::Fist,
        CameraControl::ThumbsUp,
        CameraControl::Point,
        CameraControl::Peace,
        CameraControl::SwipeLeft,
        CameraControl::SwipeRight,
        CameraControl::PadX,
        CameraControl::PadY,
        CameraControl::PadSpread,
    ];

    /// Whether this is one of the pad's controllers (they come from both hands, on their own
    /// channel).
    pub fn is_pad(self) -> bool {
        matches!(
            self,
            CameraControl::PadX | CameraControl::PadY | CameraControl::PadSpread
        )
    }

    /// The message of the control.
    pub fn kind(self) -> SourceKind {
        match self {
            CameraControl::Twist => SourceKind::Cc(TWIST_CC),
            CameraControl::Height => SourceKind::Cc(HEIGHT_CC),
            CameraControl::Side => SourceKind::Cc(SIDE_CC),
            CameraControl::Closeness => SourceKind::Cc(CLOSENESS_CC),
            CameraControl::Pinch => SourceKind::Note(PINCH_NOTE),
            CameraControl::OpenPalm => SourceKind::Note(OPEN_PALM_NOTE),
            CameraControl::Fist => SourceKind::Note(FIST_NOTE),
            CameraControl::ThumbsUp => SourceKind::Note(THUMBS_UP_NOTE),
            CameraControl::Point => SourceKind::Note(POINT_NOTE),
            CameraControl::Peace => SourceKind::Note(PEACE_NOTE),
            CameraControl::SwipeLeft => SourceKind::Note(SWIPE_LEFT_NOTE),
            CameraControl::SwipeRight => SourceKind::Note(SWIPE_RIGHT_NOTE),
            CameraControl::PadX => SourceKind::Cc(PAD_X_CC),
            CameraControl::PadY => SourceKind::Cc(PAD_Y_CC),
            CameraControl::PadSpread => SourceKind::Cc(PAD_SPREAD_CC),
        }
    }

    /// The control that sends `kind`, if any.
    pub fn of_kind(kind: SourceKind) -> Option<Self> {
        Self::EVERY.into_iter().find(|c| c.kind() == kind)
    }

    /// The words for lists: `twist`, `left and right`.
    pub fn label(self) -> &'static str {
        match self {
            CameraControl::Twist => "twist",
            CameraControl::Pinch => "pinch",
            CameraControl::Height => "height",
            CameraControl::Side => "left and right",
            CameraControl::Closeness => "closeness",
            CameraControl::OpenPalm => "open palm",
            CameraControl::Fist => "fist",
            CameraControl::ThumbsUp => "thumbs up",
            CameraControl::Point => "point",
            CameraControl::Peace => "peace",
            CameraControl::SwipeLeft => "swipe left",
            CameraControl::SwipeRight => "swipe right",
            CameraControl::PadX => "pad left and right",
            CameraControl::PadY => "pad height",
            CameraControl::PadSpread => "pad spread",
        }
    }
}

/// A gesture of the fixed vocabulary that can run a command (the gesture command table of the
/// settings) and that the camera sends as a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CameraGesture {
    /// A fist.
    Fist,
    /// An open palm, held.
    OpenPalm,
    /// A thumbs up.
    ThumbsUp,
    /// The index finger up.
    Point,
    /// The index and middle fingers up.
    Peace,
    /// A fast swipe to the left.
    SwipeLeft,
    /// A fast swipe to the right.
    SwipeRight,
}

impl CameraGesture {
    /// Every gesture, in the order of the table.
    pub const ALL: [CameraGesture; 7] = [
        CameraGesture::Fist,
        CameraGesture::OpenPalm,
        CameraGesture::ThumbsUp,
        CameraGesture::Point,
        CameraGesture::Peace,
        CameraGesture::SwipeLeft,
        CameraGesture::SwipeRight,
    ];

    /// The id saved in settings and projects.
    pub fn id(self) -> &'static str {
        match self {
            CameraGesture::Fist => "fist",
            CameraGesture::OpenPalm => "open_palm",
            CameraGesture::ThumbsUp => "thumbs_up",
            CameraGesture::Point => "point",
            CameraGesture::Peace => "peace",
            CameraGesture::SwipeLeft => "swipe_left",
            CameraGesture::SwipeRight => "swipe_right",
        }
    }

    /// The gesture with this id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.id() == id)
    }

    /// The camera control that sends this gesture as a note.
    pub fn control(self) -> CameraControl {
        match self {
            CameraGesture::Fist => CameraControl::Fist,
            CameraGesture::OpenPalm => CameraControl::OpenPalm,
            CameraGesture::ThumbsUp => CameraControl::ThumbsUp,
            CameraGesture::Point => CameraControl::Point,
            CameraGesture::Peace => CameraControl::Peace,
            CameraGesture::SwipeLeft => CameraControl::SwipeLeft,
            CameraGesture::SwipeRight => CameraControl::SwipeRight,
        }
    }

    /// The note it is sent as.
    pub fn note(self) -> u8 {
        match self.control().kind() {
            SourceKind::Note(n) => n,
            _ => 0,
        }
    }

    /// The words for lists: `thumbs up`.
    pub fn label(self) -> &'static str {
        self.control().label()
    }
}

impl MidiSource {
    /// A source for `control` of `hand` of the camera.
    pub fn camera(hand: CameraHand, control: CameraControl) -> Self {
        Self {
            port: Some(CAMERA_PORT.to_owned()),
            channel: Some(hand.channel()),
            kind: control.kind(),
            surface: None,
        }
    }

    /// True when the source listens to the camera (whatever it listens for).
    pub fn is_camera(&self) -> bool {
        self.port.as_deref() == Some(CAMERA_PORT)
    }

    /// The hand and control of a source that names one control of the camera, for lists
    /// ("Camera, right hand twist"). `None` for a hardware source, and for a camera source that
    /// has been edited to something the camera does not send (any channel, another number).
    pub fn camera_control(&self) -> Option<(CameraHand, CameraControl)> {
        if !self.is_camera() || self.surface.is_some() {
            return None;
        }
        let hand = CameraHand::from_channel(self.channel?)?;
        let control = CameraControl::of_kind(self.kind)?;
        // The pad comes from both hands, on a channel of its own; nothing else does.
        (control.is_pad() == (hand == CameraHand::Both)).then_some((hand, control))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_source_names_its_hand_and_control() {
        for hand in [CameraHand::Right, CameraHand::Left] {
            for control in CameraControl::ALL {
                let s = MidiSource::camera(hand, control);
                assert!(s.is_camera());
                assert_eq!(s.camera_control(), Some((hand, control)));
            }
        }
    }

    #[test]
    fn the_right_hand_is_channel_1_the_left_channel_2_and_both_hands_channel_3() {
        assert_eq!(CameraHand::Right.channel(), 0);
        assert_eq!(CameraHand::Left.channel(), 1);
        assert_eq!(CameraHand::Both.channel(), 2);
        assert_eq!(CameraHand::from_channel(3), None);
    }

    #[test]
    fn the_gestures_are_the_documented_notes_and_the_pad_the_documented_controllers() {
        let notes: Vec<(CameraGesture, u8)> =
            CameraGesture::ALL.iter().map(|g| (*g, g.note())).collect();
        assert_eq!(
            notes,
            [
                (CameraGesture::Fist, 62),
                (CameraGesture::OpenPalm, 61),
                (CameraGesture::ThumbsUp, 63),
                (CameraGesture::Point, 64),
                (CameraGesture::Peace, 65),
                (CameraGesture::SwipeLeft, 66),
                (CameraGesture::SwipeRight, 67),
            ]
        );
        assert_eq!(CameraControl::PadX.kind(), SourceKind::Cc(24));
        assert_eq!(CameraControl::PadY.kind(), SourceKind::Cc(25));
        assert_eq!(CameraControl::PadSpread.kind(), SourceKind::Cc(26));
        for g in CameraGesture::ALL {
            assert_eq!(CameraGesture::from_id(g.id()), Some(g));
        }
    }

    #[test]
    fn every_control_has_its_own_message_and_a_source_names_it() {
        for (i, a) in CameraControl::EVERY.iter().enumerate() {
            for b in &CameraControl::EVERY[..i] {
                assert_ne!(a.kind(), b.kind(), "{a:?} and {b:?}");
            }
            let hand = if a.is_pad() {
                CameraHand::Both
            } else {
                CameraHand::Left
            };
            let s = MidiSource::camera(hand, *a);
            assert_eq!(s.camera_control(), Some((hand, *a)));
        }
        // The pad is on its own channel and nothing else is.
        let mut wrong = MidiSource::camera(CameraHand::Right, CameraControl::PadX);
        assert!(wrong.camera_control().is_none());
        wrong = MidiSource::camera(CameraHand::Both, CameraControl::Fist);
        assert!(wrong.camera_control().is_none());
    }

    #[test]
    fn the_controls_are_the_documented_messages() {
        assert_eq!(CameraControl::Twist.kind(), SourceKind::Cc(20));
        assert_eq!(CameraControl::Height.kind(), SourceKind::Cc(21));
        assert_eq!(CameraControl::Side.kind(), SourceKind::Cc(22));
        assert_eq!(CameraControl::Closeness.kind(), SourceKind::Cc(23));
        assert_eq!(CameraControl::Pinch.kind(), SourceKind::Note(60));
        assert_eq!(CameraControl::OpenPalm.kind(), SourceKind::Note(61));
    }

    #[test]
    fn hardware_and_edited_sources_are_not_camera_controls() {
        let hardware = MidiSource::any_port(Some(0), SourceKind::Cc(21));
        assert!(!hardware.is_camera() && hardware.camera_control().is_none());
        let mut edited = MidiSource::camera(CameraHand::Right, CameraControl::Height);
        edited.channel = None;
        assert!(edited.is_camera() && edited.camera_control().is_none());
        edited.channel = Some(5);
        assert!(edited.camera_control().is_none());
        edited.channel = Some(0);
        edited.kind = SourceKind::Cc(99);
        assert!(edited.camera_control().is_none());
    }
}
