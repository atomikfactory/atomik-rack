//! The Key & Chord rack panel: the generated control layout (SCALE and CHORD sections) with a
//! FROM CLIP button in a strip at the left.
//!
//! FROM CLIP copies the scale of the clip open in the piano roll into `scale.root` and
//! `scale.kind`, as one `Command::Batch` (one undo step). The rack view does not know the piano
//! roll: the app tells it the scale of the open clip with [`crate::RackView::set_clip_scale`],
//! and the button is drawn dim and does nothing while that is `None` (no clip open, or a clip
//! without a scale).
//!
//! The `Source` switches of the SCALE and CHORD sections make the device follow the song key and
//! chord lane; while it follows, a SONG tag at the bottom of the strip names the chord (or the key) at the
//! playhead, which the app tells the rack with [`crate::RackView::set_song_harmony`].

use crate::autolayout::{self, CONTENT_X, MARGIN_TOP, PanelSpec};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{button_sized, fit_label_raw};
use crate::theme::Theme;
use crate::widget::current_value;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::theory::{Scale, ScaleKind};
use atomik_model::{Command, DeviceKey, RackDevice};
use vello::kurbo::{Rect, RoundedRect};
use vello::peniko::Color;

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
    /// The SONG tag (drawn while the device follows the song).
    pub song_tag: Rect,
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
    let bottom = spec.height() - autolayout::MARGIN_BOTTOM - 4.0;
    spec.key_chord = Some(KeyChordPanel {
        from_clip: Rect::new(CONTENT_X.0, top, CONTENT_X.0 + BUTTON.0, top + BUTTON.1),
        song_tag: Rect::new(CONTENT_X.0, bottom - 16.0, CONTENT_X.0 + STRIP_W, bottom),
    });
    spec
}

/// What a Key & Chord device names on its SONG tag: `Some(true)` the chord (it follows the chord
/// lane), `Some(false)` the key (it follows the song key only), `None` when it follows nothing.
pub fn follows_song(device: &RackDevice, desc: &DeviceDescriptor) -> Option<bool> {
    let on = |key: &str| {
        desc.param_by_key(key)
            .is_some_and(|p| current_value(device, p) >= 0.5)
    };
    if on("chord.source") {
        Some(true)
    } else if on("scale.source") {
        Some(false)
    } else {
        None
    }
}

/// Draws a SONG tag in `r`: the word, then `name` (a chord or a key, drawn as it is; a dash
/// when the song has none at the playhead).
pub(crate) fn paint_song_tag<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    name: Option<&str>,
    accent: Color,
) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(RoundedRect::from_rect(r.inset(-0.5), 3.5), 1.0, accent);
    let mid = r.y0 + r.height() / 2.0 + 3.0;
    c.text(&TextRun::new("SONG", pt(r.x0 + 4.0, mid), 7.5, accent).weight(700));
    let name = fit_label_raw(name.unwrap_or("-"), r.width() - 38.0, 8.5);
    c.text(
        &TextRun::verbatim(name, pt(r.x1 - 4.0, mid), 8.5, theme.text)
            .align(Align::End)
            .weight(700),
    );
}

/// Draws the panel's extras; `from_clip` is whether the button is enabled, `song` the SONG tag's
/// name when the device follows the song (`Some(None)`: nothing at the playhead).
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &KeyChordPanel,
    from_clip: bool,
    song: Option<Option<&str>>,
    accent: Color,
) {
    button_sized(c, theme, panel.from_clip, "FROM CLIP", from_clip, 8.5, 0.6);
    if let Some(name) = song {
        paint_song_tag(c, theme, panel.song_tag, name, accent);
    }
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
