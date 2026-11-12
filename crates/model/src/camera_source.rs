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

/// A hand of the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraHand {
    /// The right hand, channel 1.
    Right,
    /// The left hand, channel 2.
    Left,
}

impl CameraHand {
    /// The 0-based MIDI channel of this hand.
    pub fn channel(self) -> u8 {
        match self {
            CameraHand::Right => RIGHT_HAND_CHANNEL,
            CameraHand::Left => LEFT_HAND_CHANNEL,
        }
    }

    /// The hand on `channel` (0-based), if it is one of the two.
    pub fn from_channel(channel: u8) -> Option<Self> {
        match channel {
            RIGHT_HAND_CHANNEL => Some(CameraHand::Right),
            LEFT_HAND_CHANNEL => Some(CameraHand::Left),
            _ => None,
        }
    }

    /// The words for lists: `right hand`.
    pub fn label(self) -> &'static str {
        match self {
            CameraHand::Right => "right hand",
            CameraHand::Left => "left hand",
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

    /// The message of the control.
    pub fn kind(self) -> SourceKind {
        match self {
            CameraControl::Twist => SourceKind::Cc(TWIST_CC),
            CameraControl::Height => SourceKind::Cc(HEIGHT_CC),
            CameraControl::Side => SourceKind::Cc(SIDE_CC),
            CameraControl::Closeness => SourceKind::Cc(CLOSENESS_CC),
            CameraControl::Pinch => SourceKind::Note(PINCH_NOTE),
            CameraControl::OpenPalm => SourceKind::Note(OPEN_PALM_NOTE),
        }
    }

    /// The control that sends `kind`, if any.
    pub fn of_kind(kind: SourceKind) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.kind() == kind)
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
        }
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
        Some((
            CameraHand::from_channel(self.channel?)?,
            CameraControl::of_kind(self.kind)?,
        ))
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
    fn the_right_hand_is_channel_1_and_the_left_channel_2() {
        assert_eq!(CameraHand::Right.channel(), 0);
        assert_eq!(CameraHand::Left.channel(), 1);
        assert_eq!(CameraHand::from_channel(2), None);
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
