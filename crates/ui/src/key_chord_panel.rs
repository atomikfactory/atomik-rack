//! The Key & Chord rack panel: the generated control layout (SCALE and CHORD sections) with a
//! FROM CLIP button in a strip at the left.
//!
//! FROM CLIP copies the scale of the clip open in the piano roll into `scale.root` and
//! `scale.kind`, as one `Command::Batch` (one undo step). The rack view does not know the piano
//! roll: the app tells it the scale of the open clip with [`crate::RackView::set_clip_scale`],
//! and the button is drawn dim and does nothing while that is `None` (no clip open, or a clip
//! without a scale).

use crate::autolayout::{self, CONTENT_X, MARGIN_TOP, PanelSpec};
use crate::canvas::Canvas;
use crate::paint::button_sized;
use crate::theme::Theme;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::theory::{Scale, ScaleKind};
use atomik_model::{Command, DeviceKey};
use vello::kurbo::Rect;

/// Type id of the Key & Chord device.
pub const KEY_CHORD_TYPE: &str = "atomik.key-chord";
/// Width of the strip left of the sections.
const STRIP_W: f64 = 76.0;
/// Size of the FROM CLIP button.
const BUTTON: (f64, f64) = (STRIP_W, 26.0);

/// Geometry of the panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyChordPanel {
    /// The FROM CLIP button.
    pub from_clip: Rect,
}

/// The section of a param key: the chord selector (`chord`) shares its section with the
/// `chord.*` params instead of a MAIN section of its own.
fn section_prefix(key: &str) -> &str {
    if key == "chord" {
        "chord"
    } else {
        autolayout::group_prefix(key)
    }
}

/// The panel of `atomik.key-chord`: the generated layout with the button strip reserved; the back
/// is the generic socket layout.
pub fn key_chord_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec = autolayout::auto_layout_with_strip(desc, STRIP_W, section_prefix);
    let mid = spec.height() / 2.0;
    let top = (mid - BUTTON.1 / 2.0).max(MARGIN_TOP);
    spec.key_chord = Some(KeyChordPanel {
        from_clip: Rect::new(CONTENT_X.0, top, CONTENT_X.0 + BUTTON.0, top + BUTTON.1),
    });
    spec
}

/// Draws the panel's extras; `from_clip` is whether the button is enabled.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &KeyChordPanel,
    from_clip: bool,
) {
    button_sized(c, theme, panel.from_clip, "FROM CLIP", from_clip, 8.5, 0.6);
}

/// The command of a FROM CLIP click: `scale.root` and `scale.kind` of `device` set from `scale`,
/// one batch so the change is one undo step.
pub fn from_clip_command(device: &DeviceKey, scale: Scale) -> Command {
    let kind = ScaleKind::ALL
        .iter()
        .position(|k| *k == scale.kind)
        .unwrap_or(0);
    let set = |param: &str, value: f32| Command::SetParam {
        key: device.clone(),
        param: param.to_string(),
        value,
        merge: None,
    };
    Command::Batch(vec![
        set("scale.root", f32::from(scale.root)),
        set("scale.kind", kind as f32),
    ])
}
