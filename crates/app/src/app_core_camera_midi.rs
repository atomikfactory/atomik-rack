//! The camera as a virtual MIDI controller in the app core.
//!
//! [`crate::camera_midi`] turns what the hand tracker sees into MIDI messages. This module feeds
//! them into the app's MIDI input path ([`AppCore::camera_message`]): they are counted and shown
//! in the status line, learned from, and run through the mappings that name the camera (and only
//! those: a mapping on a hardware port, or on any port, never hears the camera, and a hardware
//! controller never drives a camera mapping). Messages never leave the app and never reach a
//! hardware port.
//!
//! # Modes
//!
//! The camera's mode ([`CameraMode`]) decides what the hands do:
//!
//! * **Turn clicked knob**: a pinch and a twist turn the last clicked knob. Nothing is sent.
//! * **MIDI controller**: the hands send their fixed controls (see [`crate::camera_midi`]); a
//!   pinch turns no knob.
//! * **Both**: the twist turns the clicked knob and the other controls send. The twist
//!   controller is not sent, so one gesture never drives two targets.
//!
//! While MIDI Learn is armed (a target is waiting for a control, from the global Learn mode or a
//! plugin window's LEARN MIDI) the camera sends every control whatever the mode, and a pinch and
//! twist turn no knob, so a control can be learned without switching mode. The preview says so.
//!
//! # Notes and tracks
//!
//! The camera is a controller, not an instrument: its pinch and open palm notes feed the
//! mappings, and by default no note track hears them, whatever it listens to. With the setting
//! **Camera notes play the selected instrument** on, they play the selected instrument as a
//! keyboard would (and are recorded when a take is armed), unless a mapping that consumes the
//! note took it.

use super::*;
use crate::camera_midi::Bar;
use atomik_core::{ChannelMessage, MessageKind, NoteKind};
use atomik_model::CameraControl;
use atomik_ui::{CameraBar, CameraControlsState};
use atomik_vision::gesture::Pad;

/// What the camera does with its hands right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Flags {
    /// A pinch and a twist turn the clicked knob.
    pub(super) turns: bool,
    /// The camera sends MIDI.
    pub(super) midi: bool,
    /// The twist controller is sent.
    pub(super) twist: bool,
    /// The two-hand pad's position controllers are sent (its spread always is, with `midi`).
    pub(super) pad_xy: bool,
}

impl<S: EngineSink> AppCore<S> {
    /// The camera's mode as it applies now: MIDI Learn armed overrides it (the camera sends
    /// everything and turns no knob).
    pub(super) fn camera_flags(&self) -> Flags {
        let mode = self.camera_mode();
        let learning = self.midi.learning();
        let turns = mode.turns_knob() && !learning;
        let midi = mode.sends_midi() || learning;
        Flags {
            turns,
            midi,
            twist: midi && !turns,
            pad_xy: midi && !turns,
        }
    }

    /// Whether the camera is sending only because MIDI Learn is armed.
    pub fn camera_learning_midi(&self) -> bool {
        self.vision.session.is_some() && !self.camera_mode().sends_midi() && self.midi.learning()
    }

    /// Hands every message to the MIDI input path, and the notes to the selected instrument when
    /// the settings say so.
    fn camera_sends(&mut self, messages: Vec<ChannelMessage>) {
        for msg in messages {
            let consumed = self.camera_message(msg);
            if self.vision.prefs.play_notes && !consumed {
                self.camera_play(msg);
            }
        }
    }

    /// Plays a camera note on the selected instrument, through a router of its own so a note-off
    /// follows its note-on.
    fn camera_play(&mut self, msg: ChannelMessage) {
        let kind = match msg.kind {
            MessageKind::NoteOn { key, velocity } => NoteKind::On {
                key,
                velocity: f32::from(velocity.clamp(1, 127)) / 127.0,
            },
            MessageKind::NoteOff { key } => NoteKind::Off { key },
            _ => return,
        };
        let current = self.notes_target();
        let mut sends = Vec::new();
        self.vision
            .notes
            .route(kind, current, |target, kind| sends.push((target, kind)));
        for (target, kind) in sends {
            let result = self.sink.play_note(target, kind);
            self.engine_result(result);
        }
    }

    /// A frame of hands: the absolute controllers. With the camera's MIDI off, notes still held
    /// from before end.
    pub(super) fn camera_midi_frame(&mut self, t: f64, hands: &[HandState], pad: Option<Pad>) {
        if !self.camera_flags().midi {
            if self.vision.midi.holds_notes() {
                self.camera_midi_release();
            }
            return;
        }
        let mut out = Vec::new();
        self.vision.midi.frame(t, hands, &mut out);
        let xy = self.camera_flags().pad_xy;
        self.vision.midi.pad(t, pad, xy, &mut out);
        self.camera_sends(out);
    }

    /// A gesture change: the pinch and open palm notes, and (when `twist`) the twist encoder.
    pub(super) fn camera_midi_gesture(&mut self, event: &GestureEvent, twist: bool) {
        let t = self.vision.clock;
        let full_turn = f32::from(self.vision.prefs.full_turn).clamp(90.0, 360.0);
        let mut out = Vec::new();
        match *event {
            GestureEvent::PinchStart { hand, handedness } => {
                self.vision.midi.pinch_start(hand, handedness, &mut out);
            }
            GestureEvent::PinchEnd { hand } => self.vision.midi.pinch_end(hand, &mut out),
            GestureEvent::Twist {
                hand,
                delta_radians,
            } => {
                if twist {
                    self.vision
                        .midi
                        .twist(t, hand, delta_radians, full_turn, &mut out);
                }
            }
            GestureEvent::OpenPalmStart { hand, handedness } => {
                self.vision.midi.palm_start(hand, handedness, &mut out);
            }
            GestureEvent::OpenPalmEnd { hand } => self.vision.midi.palm_end(hand, &mut out),
            GestureEvent::PoseStart {
                hand,
                handedness,
                pose,
            } => self
                .vision
                .midi
                .pose_start(hand, handedness, pose, &mut out),
            GestureEvent::PoseEnd { hand, .. } => self.vision.midi.pose_end(hand, &mut out),
            GestureEvent::Swipe {
                handedness,
                direction,
                ..
            } => self.vision.midi.swipe(handedness, direction, &mut out),
            // The pad's controllers go with the frames.
            GestureEvent::PadStart | GestureEvent::PadEnd => {}
        }
        self.camera_sends(out);
    }

    /// Ends every note the camera holds (the camera went off, or its MIDI did), so a mapped
    /// momentary button or a played note does not stick.
    pub(super) fn camera_midi_release(&mut self) {
        let mut out = Vec::new();
        self.vision.midi.release_all(&mut out);
        self.vision.midi.drop_twist();
        self.camera_sends(out);
    }

    /// The bars of the preview, while the camera sends MIDI.
    pub(super) fn camera_controls_state(&self) -> Option<CameraControlsState> {
        if !self.camera_flags().midi {
            return None;
        }
        let (hand, bars) = self.vision.midi.bars().unwrap_or_else(|| {
            let zero = |control| Bar {
                control,
                value: 0.0,
            };
            (
                atomik_model::CameraHand::Right,
                CameraControl::ALL.map(zero),
            )
        });
        Some(CameraControlsState {
            hand: match hand {
                atomik_model::CameraHand::Right => "Right hand, channel 1",
                atomik_model::CameraHand::Left => "Left hand, channel 2",
                atomik_model::CameraHand::Both => "Both hands, channel 3",
            },
            bars: bars
                .iter()
                .map(|b| CameraBar {
                    label: match b.control {
                        CameraControl::Twist => "Twist",
                        CameraControl::Height => "Height",
                        CameraControl::Side => "Left/right",
                        CameraControl::Closeness => "Closeness",
                        CameraControl::Pinch => "Pinch",
                        CameraControl::OpenPalm => "Palm",
                        _ => "",
                    },
                    value: b.value,
                    centred: b.control == CameraControl::Twist,
                })
                .collect(),
            learning: self.camera_learning_midi(),
        })
    }
}
