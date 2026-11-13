//! The gesture command table of the webcam controller and the rules that fire it.
//!
//! The camera recognises a small fixed vocabulary ([`CameraGesture`]: fist, open palm, thumbs up,
//! point, peace, swipe left and swipe right). The **gesture command table** (Settings > Camera)
//! says which app command each gesture runs, per hand (either, left or right). It is **empty by
//! default**: nothing happens by surprise. **Suggested** fills in fist = Stop, open palm held for
//! a second = Play and thumbs up = Record. A project may keep a table of its own (see
//! `atomik_model::CameraOverride`).
//!
//! # Firing
//!
//! [`GestureFirer`] is pure: the app core tells it when a pose starts or ends and when a swipe
//! happens, with the time of the frame, and it answers with the command ids to run.
//!
//! * A pose fires **once per hold**: it re-arms only after the hand has left the pose (any other
//!   shape ends it, and the same pose held again is a new hold).
//! * The open palm fires after [`PALM_COMMAND_HOLD`] seconds (a second), the other poses as soon
//!   as the recogniser calls them (a pose held for 300 ms); a swipe fires at once.
//! * A **cooldown** of [`COOLDOWN`] seconds follows every command: a gesture that is ready inside
//!   it does nothing, and a hold that missed its turn stays silent until the hand has changed
//!   pose.
//! * Gestures never fire while a hand is **pinching** (a pinch is a knob or a pad).
//! * With two rows for one gesture, the one for the hand that made it wins over "either".
//!
//! The commands are registry commands, run by the shell exactly as the key bound to them would
//! be: whatever confirmation that key asks for, the gesture asks for.

use crate::keymap_store::HandChoice;
use atomik_model::{CameraGesture, CameraOverride, GestureHand, GestureRow};
use atomik_vision::Handedness;
use atomik_vision::gesture::HandId;

/// The most rows a table may hold (as the project field allows).
pub const MAX_ROWS: usize = atomik_model::MAX_GESTURE_ROWS;
/// Seconds after a command during which no other gesture fires.
pub const COOLDOWN: f64 = 0.5;
/// Seconds an open palm is held before its command runs.
pub const PALM_COMMAND_HOLD: f64 = 1.0;
/// Seconds the recogniser holds an open palm before it says so (see `atomik_vision`).
const PALM_RECOGNISED_AFTER: f64 = 0.3;

/// One row of the table: a gesture of a hand runs a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GestureBinding {
    /// The gesture.
    pub gesture: CameraGesture,
    /// The hand that counts (the model's label).
    pub hand: HandChoice,
    /// The id of the registry command.
    pub command: String,
}

/// What **Suggested** fills in: fist = Stop, open palm held for a second = Play, thumbs up =
/// Record.
pub fn suggested() -> Vec<GestureBinding> {
    [
        (CameraGesture::Fist, "transport.stop"),
        (CameraGesture::OpenPalm, "transport.play"),
        (CameraGesture::ThumbsUp, "transport.record"),
    ]
    .into_iter()
    .map(|(gesture, command)| GestureBinding {
        gesture,
        hand: HandChoice::Either,
        command: command.to_owned(),
    })
    .collect()
}

/// The first row of `table` for `gesture` (the one the editor shows).
pub fn row_of(table: &[GestureBinding], gesture: CameraGesture) -> Option<&GestureBinding> {
    table.iter().find(|b| b.gesture == gesture)
}

/// The command a gesture of the hand the model labels `handedness` runs: the row for that hand
/// first, else the row for either.
pub fn lookup(
    table: &[GestureBinding],
    gesture: CameraGesture,
    handedness: Handedness,
) -> Option<&str> {
    table
        .iter()
        .filter(|b| b.gesture == gesture && !b.command.is_empty())
        .filter(|b| b.hand.allows(handedness.letter()))
        .min_by_key(|b| b.hand == HandChoice::Either)
        .map(|b| b.command.as_str())
}

fn hand_of(hand: GestureHand) -> HandChoice {
    match hand {
        GestureHand::Either => HandChoice::Either,
        GestureHand::Left => HandChoice::Left,
        GestureHand::Right => HandChoice::Right,
    }
}

fn gesture_hand(hand: HandChoice) -> GestureHand {
    match hand {
        HandChoice::Either => GestureHand::Either,
        HandChoice::Left => GestureHand::Left,
        HandChoice::Right => GestureHand::Right,
    }
}

/// The table a project keeps, as the app's rows.
pub fn from_override(camera: &CameraOverride) -> Vec<GestureBinding> {
    camera
        .gestures
        .iter()
        .map(|r| GestureBinding {
            gesture: r.gesture,
            hand: hand_of(r.hand),
            command: r.command.clone(),
        })
        .collect()
}

/// The rows a project saves for `table` (rows without a command are left out).
pub fn to_rows(table: &[GestureBinding]) -> Vec<GestureRow> {
    table
        .iter()
        .filter(|b| !b.command.is_empty())
        .take(MAX_ROWS)
        .map(|b| GestureRow {
            gesture: b.gesture,
            hand: gesture_hand(b.hand),
            command: b.command.clone(),
        })
        .collect()
}

/// A pose that is held and has not run its command yet.
#[derive(Debug, Clone, Copy)]
struct Held {
    hand: HandId,
    handedness: Handedness,
    gesture: CameraGesture,
    /// When the app heard of it.
    since: f64,
    /// Seconds after `since` that it is ready.
    after: f64,
    /// Its turn came (it ran, or was held back by the cooldown or a pinch).
    done: bool,
}

/// Decides which commands the gestures run. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct GestureFirer {
    held: Vec<Held>,
    last_fire: Option<f64>,
}

impl GestureFirer {
    /// A firer that has fired nothing.
    pub fn new() -> Self {
        Self::default()
    }

    fn cooled(&self, t: f64) -> bool {
        self.last_fire.is_none_or(|at| t - at >= COOLDOWN - 1e-9)
    }

    /// A pose (or the open palm) was recognised at `t`.
    pub fn start(&mut self, t: f64, hand: HandId, handedness: Handedness, gesture: CameraGesture) {
        self.held
            .retain(|h| !(h.hand == hand && h.gesture == gesture));
        let after = if gesture == CameraGesture::OpenPalm {
            (PALM_COMMAND_HOLD - PALM_RECOGNISED_AFTER).max(0.0)
        } else {
            0.0
        };
        self.held.push(Held {
            hand,
            handedness,
            gesture,
            since: t,
            after,
            done: false,
        });
    }

    /// The pose of `hand` ended (it re-arms: the next hold is a new one).
    pub fn end(&mut self, hand: HandId, gesture: CameraGesture) {
        self.held
            .retain(|h| !(h.hand == hand && h.gesture == gesture));
    }

    /// Forgets every hold (the camera went off).
    pub fn reset(&mut self) {
        self.held.clear();
        self.last_fire = None;
    }

    /// A swipe at `t`: the command to run, if the table has one and nothing holds it back
    /// (`pinching`: some hand is pinched).
    pub fn swipe(
        &mut self,
        t: f64,
        handedness: Handedness,
        gesture: CameraGesture,
        table: &[GestureBinding],
        pinching: bool,
    ) -> Option<String> {
        if pinching || !self.cooled(t) {
            return None;
        }
        let command = lookup(table, gesture, handedness)?.to_owned();
        self.last_fire = Some(t);
        Some(command)
    }

    /// The commands of the holds that are ready at `t` (at most one: the cooldown follows it).
    pub fn poll(&mut self, t: f64, table: &[GestureBinding], pinching: bool) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.held.len() {
            let h = self.held[i];
            if h.done || t - h.since < h.after - 1e-9 {
                continue;
            }
            self.held[i].done = true;
            if pinching || !self.cooled(t) {
                continue;
            }
            if let Some(command) = lookup(table, h.gesture, h.handedness) {
                self.last_fire = Some(t);
                out.push(command.to_owned());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<GestureBinding> {
        suggested()
    }

    fn row(gesture: CameraGesture, hand: HandChoice, command: &str) -> GestureBinding {
        GestureBinding {
            gesture,
            hand,
            command: command.into(),
        }
    }

    const R: Handedness = Handedness::Right;
    const L: Handedness = Handedness::Left;

    #[test]
    fn the_suggested_table_is_stop_play_and_record_and_nothing_else() {
        let t = suggested();
        assert_eq!(lookup(&t, CameraGesture::Fist, R), Some("transport.stop"));
        assert_eq!(
            lookup(&t, CameraGesture::OpenPalm, L),
            Some("transport.play")
        );
        assert_eq!(
            lookup(&t, CameraGesture::ThumbsUp, R),
            Some("transport.record")
        );
        for g in [
            CameraGesture::Point,
            CameraGesture::Peace,
            CameraGesture::SwipeLeft,
            CameraGesture::SwipeRight,
        ] {
            assert_eq!(lookup(&t, g, R), None);
        }
    }

    #[test]
    fn the_hand_that_made_the_gesture_wins_over_either() {
        let t = vec![
            row(CameraGesture::Fist, HandChoice::Either, "transport.stop"),
            row(CameraGesture::Fist, HandChoice::Left, "edit.undo"),
        ];
        assert_eq!(lookup(&t, CameraGesture::Fist, L), Some("edit.undo"));
        assert_eq!(lookup(&t, CameraGesture::Fist, R), Some("transport.stop"));
        let only_left = vec![row(CameraGesture::Fist, HandChoice::Left, "edit.undo")];
        assert_eq!(lookup(&only_left, CameraGesture::Fist, R), None);
    }

    #[test]
    fn a_pose_fires_once_per_hold_and_again_after_the_hand_changed_pose() {
        let mut f = GestureFirer::new();
        let t = table();
        f.start(1.0, HandId(0), R, CameraGesture::Fist);
        assert_eq!(f.poll(1.0, &t, false), ["transport.stop"]);
        for i in 1..60 {
            assert!(f.poll(1.0 + f64::from(i) * 0.033, &t, false).is_empty());
        }
        f.end(HandId(0), CameraGesture::Fist);
        f.start(4.0, HandId(0), R, CameraGesture::Fist);
        assert_eq!(f.poll(4.0, &t, false), ["transport.stop"]);
    }

    #[test]
    fn the_open_palm_waits_a_second_in_all() {
        let mut f = GestureFirer::new();
        let t = table();
        // The recogniser says so after 300 ms; the command comes 700 ms later.
        f.start(0.3, HandId(0), R, CameraGesture::OpenPalm);
        assert!(f.poll(0.9, &t, false).is_empty());
        assert!(f.poll(0.99, &t, false).is_empty());
        assert_eq!(f.poll(1.0, &t, false), ["transport.play"]);
        assert!(f.poll(2.0, &t, false).is_empty(), "once per hold");
    }

    #[test]
    fn an_open_palm_that_closes_before_its_second_never_fires() {
        let mut f = GestureFirer::new();
        let t = table();
        f.start(0.3, HandId(0), R, CameraGesture::OpenPalm);
        assert!(f.poll(0.8, &t, false).is_empty());
        f.end(HandId(0), CameraGesture::OpenPalm);
        assert!(f.poll(3.0, &t, false).is_empty());
    }

    #[test]
    fn a_cooldown_of_half_a_second_follows_every_command() {
        let mut f = GestureFirer::new();
        let t = vec![
            row(CameraGesture::Fist, HandChoice::Either, "transport.stop"),
            row(CameraGesture::SwipeLeft, HandChoice::Either, "edit.undo"),
            row(CameraGesture::SwipeRight, HandChoice::Either, "edit.redo"),
        ];
        f.start(1.0, HandId(0), R, CameraGesture::Fist);
        assert_eq!(f.poll(1.0, &t, false), ["transport.stop"]);
        // A swipe 400 ms later does nothing; one 500 ms later runs.
        assert_eq!(f.swipe(1.4, R, CameraGesture::SwipeLeft, &t, false), None);
        assert_eq!(
            f.swipe(1.5, R, CameraGesture::SwipeLeft, &t, false)
                .as_deref(),
            Some("edit.undo")
        );
        assert_eq!(f.swipe(1.9, R, CameraGesture::SwipeRight, &t, false), None);
        assert_eq!(
            f.swipe(2.0, R, CameraGesture::SwipeRight, &t, false)
                .as_deref(),
            Some("edit.redo")
        );
    }

    #[test]
    fn a_hold_that_is_ready_inside_the_cooldown_stays_silent_for_that_hold() {
        let mut f = GestureFirer::new();
        let t = vec![
            row(CameraGesture::Fist, HandChoice::Either, "transport.stop"),
            row(CameraGesture::Peace, HandChoice::Either, "edit.undo"),
        ];
        f.start(1.0, HandId(0), R, CameraGesture::Fist);
        assert_eq!(f.poll(1.0, &t, false), ["transport.stop"]);
        f.end(HandId(0), CameraGesture::Fist);
        f.start(1.2, HandId(0), R, CameraGesture::Peace);
        assert!(f.poll(1.2, &t, false).is_empty());
        assert!(
            f.poll(3.0, &t, false).is_empty(),
            "no late firing from a hold that missed its turn"
        );
        f.end(HandId(0), CameraGesture::Peace);
        f.start(3.5, HandId(0), R, CameraGesture::Peace);
        assert_eq!(f.poll(3.5, &t, false), ["edit.undo"]);
    }

    #[test]
    fn nothing_fires_while_a_hand_is_pinching() {
        let mut f = GestureFirer::new();
        let t = table();
        f.start(1.0, HandId(0), R, CameraGesture::Fist);
        assert!(f.poll(1.0, &t, true).is_empty());
        assert!(
            f.poll(1.5, &t, false).is_empty(),
            "the hold missed its turn"
        );
        let t = vec![row(
            CameraGesture::SwipeLeft,
            HandChoice::Either,
            "edit.undo",
        )];
        assert_eq!(f.swipe(5.0, R, CameraGesture::SwipeLeft, &t, true), None);
        assert!(
            f.swipe(5.0, R, CameraGesture::SwipeLeft, &t, false)
                .is_some(),
            "a blocked swipe does not start the cooldown"
        );
    }

    #[test]
    fn an_empty_table_fires_nothing() {
        let mut f = GestureFirer::new();
        f.start(1.0, HandId(0), R, CameraGesture::Fist);
        assert!(f.poll(1.0, &[], false).is_empty());
        assert_eq!(f.swipe(2.0, R, CameraGesture::SwipeLeft, &[], false), None);
        let blank = vec![row(CameraGesture::Fist, HandChoice::Either, "")];
        f.end(HandId(0), CameraGesture::Fist);
        f.start(3.0, HandId(0), R, CameraGesture::Fist);
        assert!(f.poll(3.0, &blank, false).is_empty());
    }

    #[test]
    fn rows_convert_to_and_from_a_project() {
        let t = vec![
            row(CameraGesture::Fist, HandChoice::Left, "transport.stop"),
            row(CameraGesture::Peace, HandChoice::Either, ""),
        ];
        let rows = to_rows(&t);
        assert_eq!(rows.len(), 1, "a row without a command is not saved");
        let back = from_override(&CameraOverride {
            mode: "knob".into(),
            gestures: rows,
        });
        assert_eq!(back, vec![t[0].clone()]);
    }
}
