//! The webcam controller's settings for one project: the camera mode and the gesture command
//! table, kept in the project instead of in the user's settings.
//!
//! The settings (Settings > Camera) say what the hands do and which command each gesture runs.
//! With **Use for this project only** on, the project keeps its own copy: [`RackDoc::camera`]
//! holds a [`CameraOverride`] and the app uses it instead of the settings while the project is
//! open (other projects, and the settings, are untouched). It is `None` for a new project, saved
//! as the root field `"camera"` only when set (project format 45, ignorable: a reader that does
//! not know it plays and bounces the project as it did) and edited with
//! [`crate::Command::SetCameraOverride`], one undo step that reports
//! [`crate::SyncOp::ViewChanged`] and never touches the engine.
//!
//! The mode is one of [`CAMERA_MODES`] and a gesture row names a command by its id. The model
//! does not know the command list, so an id it cannot resolve is the app's to ignore.
//!
//! # Limits
//!
//! At most [`MAX_GESTURE_ROWS`] rows; a command id is 1 to [`MAX_COMMAND_CHARS`] characters with
//! no control characters. The loader and the command enforce the same rules.

use crate::camera_source::CameraGesture;
use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, array, json_str, object, string};
use crate::{ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;

/// The ids of the camera modes: turn the clicked knob, send MIDI, or both.
pub const CAMERA_MODES: [&str; 3] = ["knob", "midi", "both"];
/// Most rows of the gesture table.
pub const MAX_GESTURE_ROWS: usize = 64;
/// Most characters of a command id.
pub const MAX_COMMAND_CHARS: usize = 100;

const FIELDS: &[&str] = &["mode", "gestures"];
const ROW_FIELDS: &[&str] = &["gesture", "hand", "command"];

/// Which hand a gesture row listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum GestureHand {
    /// Either hand.
    #[default]
    Either,
    /// The hand the model labels left.
    Left,
    /// The hand the model labels right.
    Right,
}

impl GestureHand {
    /// Every choice.
    pub const ALL: [GestureHand; 3] = [GestureHand::Either, GestureHand::Left, GestureHand::Right];

    /// The id saved in a project.
    pub fn id(self) -> &'static str {
        match self {
            GestureHand::Either => "either",
            GestureHand::Left => "left",
            GestureHand::Right => "right",
        }
    }

    /// The choice with this id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.id() == id)
    }
}

/// One row of the gesture table: a gesture of a hand runs a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GestureRow {
    /// The gesture.
    pub gesture: CameraGesture,
    /// Which hand it counts for.
    pub hand: GestureHand,
    /// The id of the command it runs (`transport.stop`).
    pub command: String,
}

/// A project's own camera settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraOverride {
    /// The camera mode, one of [`CAMERA_MODES`].
    pub mode: String,
    /// The gesture command table.
    pub gestures: Vec<GestureRow>,
}

impl CameraOverride {
    /// Checks the limits (see the module documentation).
    ///
    /// # Errors
    /// [`ModelError::OutOfRange`] naming the first thing that breaks one.
    pub fn check(&self) -> Result<(), ModelError> {
        check(self).map_err(|m| ModelError::OutOfRange(format!("camera settings: {m}")))
    }
}

fn check(c: &CameraOverride) -> Result<(), String> {
    if !CAMERA_MODES.contains(&c.mode.as_str()) {
        return Err(format!(
            "the mode {:?} is none of {}",
            c.mode,
            CAMERA_MODES.join(", ")
        ));
    }
    if c.gestures.len() > MAX_GESTURE_ROWS {
        return Err(format!(
            "{} gesture rows, at most {MAX_GESTURE_ROWS} are allowed",
            c.gestures.len()
        ));
    }
    for row in &c.gestures {
        let chars = row.command.chars().count();
        if chars == 0 || chars > MAX_COMMAND_CHARS {
            return Err(format!(
                "a command id is 1 to {MAX_COMMAND_CHARS} characters, not {chars}"
            ));
        }
        if row.command.chars().any(char::is_control) {
            return Err("a command id must not hold control characters".into());
        }
    }
    Ok(())
}

/// Plans `Command::SetCameraOverride`: the settings must be within the limits and differ from
/// the current ones.
pub(crate) fn plan_set(doc: &RackDoc, new: &Option<CameraOverride>) -> Result<Edit, ModelError> {
    if let Some(c) = new {
        c.check()?;
    }
    if &doc.camera == new {
        return Err(ModelError::BadScope(
            "the project's camera settings are already that".into(),
        ));
    }
    Ok(Edit::Swap(Swap::Camera {
        old: doc.camera.clone(),
        new: new.clone(),
    }))
}

/// Writes `"camera"` (with its trailing comma and newline) after `"video"`; nothing when the
/// project has no settings of its own.
pub(crate) fn write_camera(w: &mut String, camera: &Option<CameraOverride>) {
    let Some(c) = camera else { return };
    let rows: Vec<String> = c
        .gestures
        .iter()
        .map(|r| {
            format!(
                "      {{\"gesture\": {}, \"hand\": {}, \"command\": {}}}",
                json_str(r.gesture.id()),
                json_str(r.hand.id()),
                json_str(&r.command)
            )
        })
        .collect();
    let gestures = if rows.is_empty() {
        "[]".to_owned()
    } else {
        format!("[\n{}\n    ]", rows.join(",\n"))
    };
    let _ = writeln!(
        w,
        "  \"camera\": {{\n    \"mode\": {},\n    \"gestures\": {gestures}\n  }},",
        json_str(&c.mode)
    );
}

/// Reads the project's `"camera"` object.
pub(crate) fn load_camera(v: &Value, path: &JsonPath) -> Result<CameraOverride, ModelError> {
    let o = object(v, path, FIELDS, FIELDS)?;
    let mode = string(&o["mode"], &path.field("mode"))?.to_owned();
    let list = path.field("gestures");
    let mut gestures = Vec::new();
    for (i, item) in array(&o["gestures"], &list)?.iter().enumerate() {
        let at = list.index(i);
        let r = object(item, &at, ROW_FIELDS, ROW_FIELDS)?;
        let gesture_id = string(&r["gesture"], &at.field("gesture"))?;
        let hand_id = string(&r["hand"], &at.field("hand"))?;
        gestures.push(GestureRow {
            gesture: CameraGesture::from_id(gesture_id).ok_or_else(|| {
                at.field("gesture")
                    .error(format!("unknown gesture {gesture_id:?}"))
            })?,
            hand: GestureHand::from_id(hand_id).ok_or_else(|| {
                at.field("hand")
                    .error(format!("expected either, left or right, found {hand_id:?}"))
            })?,
            command: string(&r["command"], &at.field("command"))?.to_owned(),
        });
    }
    let c = CameraOverride { mode, gestures };
    check(&c).map_err(|m| path.error(m))?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CameraOverride {
        CameraOverride {
            mode: "midi".into(),
            gestures: vec![
                GestureRow {
                    gesture: CameraGesture::Fist,
                    hand: GestureHand::Either,
                    command: "transport.stop".into(),
                },
                GestureRow {
                    gesture: CameraGesture::SwipeLeft,
                    hand: GestureHand::Right,
                    command: "edit.undo".into(),
                },
            ],
        }
    }

    #[test]
    fn a_camera_object_writes_and_reads_back() {
        let mut text = String::from("{\n");
        write_camera(&mut text, &Some(sample()));
        text.push_str("  \"x\": 0\n}");
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        let back = load_camera(&value["camera"], &JsonPath::lazy()).unwrap();
        assert_eq!(back, sample());
        let mut none = String::new();
        write_camera(&mut none, &None);
        assert!(none.is_empty());
    }

    #[test]
    fn the_limits_are_checked() {
        let mut c = sample();
        assert!(c.check().is_ok());
        c.mode = "wave".into();
        assert!(c.check().is_err());
        let mut c = sample();
        c.gestures[0].command.clear();
        assert!(c.check().is_err());
        let mut c = sample();
        c.gestures[0].command = "a\nb".into();
        assert!(c.check().is_err());
        let mut c = sample();
        c.gestures = vec![c.gestures[0].clone(); MAX_GESTURE_ROWS + 1];
        assert!(c.check().is_err());
    }
}
