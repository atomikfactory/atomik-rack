//! The Plucked Strings rack panel: the generated layout with the params gathered by what they
//! shape (STRING, PLUCK, COURSE, BODY, OUTPUT, PLAY) instead of by key prefix.
//!
//! The device has nothing to display (no waveform, no strip), so the panel is the generic one
//! with its own sections, in the order a string is played: the string itself, the way it is
//! struck, the doubled course, the body it sounds through, the output and the playing response.
//! The controls are labelled by the generic rule (the param name for a param with no key group,
//! the last key segment for the others), which reads well in these sections.

use crate::autolayout::{self, PanelSpec};
use atomik_device_sdk::DeviceDescriptor;

/// Type id of the Plucked Strings device.
pub const PLUCKED_TYPE: &str = "atomik.plucked";

/// The sections in layout order.
const SECTION_ORDER: [&str; 6] = ["string", "pluck", "course", "body", "output", "play"];

/// The section of a param key: the string params have no key group, the pick params are the
/// way the string is plucked, and the velocity and bend params are how the device is played.
fn section_prefix(key: &str) -> &str {
    match key {
        "decay" | "sustain" | "brightness" | "stiffness" | "palm_mute" => "string",
        "pick.position" | "pick.hardness" => "pluck",
        "course.mode" | "course.detune" => "course",
        "body" | "body.mix" => "body",
        "out.gain" | "stereo.width" => "output",
        "vel.sens" | "bend.range" => "play",
        _ => autolayout::group_prefix(key),
    }
}

/// The panel of `atomik.plucked`: the generated layout in six sections; the back is the generic
/// socket layout.
pub fn plucked_layout(desc: &DeviceDescriptor) -> PanelSpec {
    autolayout::auto_layout_with_sections(desc, section_prefix, &SECTION_ORDER)
}
