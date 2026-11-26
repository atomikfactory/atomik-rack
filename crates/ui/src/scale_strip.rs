//! The scale strip of the CV Quantizer panel: where the quantizer takes its scale from, chosen by
//! name from the scales saved in the song.
//!
//! The strip sits left of the generated controls. Its name tag shows the source: `OWN` (the
//! scale of the device's own controls), `KEY` (the song key) or the name of a song scale. A click
//! on the tag goes to the next source, a right click to the one before, through `OWN`, `KEY` and
//! then the song scales in the order of the song. `NEW` saves the device's own scale as a new song
//! scale (the picker opens on it) and `EDIT` opens the picker on the scale the tag names; both
//! ask the app with [`UiAction::SongScalePicker`](crate::UiAction::SongScalePicker). The source
//! and the id are the device's `scale.source` and `scale.id` params, so a choice is one undo step
//! and can be automated like any other param.

use crate::autolayout::{self, CONTENT_X, MARGIN_TOP, PanelSpec};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{button_sized, fit_label_raw};
use crate::theme::Theme;
use crate::widget::current_value;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::{Command, DeviceKey, RackDevice};
use vello::kurbo::{Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the CV Quantizer.
pub const QUANTIZER_TYPE: &str = "atomik.cv-quantizer";
const STRIP_W: f64 = 76.0;
const TAG_H: f64 = 20.0;
const BTN_H: f64 = 22.0;
const GAP: f64 = 6.0;

/// A song scale as the strip lists it: id and name.
pub type SongScaleName = (u8, String);

/// Geometry of the strip (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleStrip {
    /// The name tag: click for the next source, right click for the one before.
    pub tag: Rect,
    /// The NEW button.
    pub new: Rect,
    /// The EDIT button.
    pub edit: Rect,
}

/// The panel of `atomik.cv-quantizer`: the generated layout without the source and id params
/// (the tag shows them) and the strip reserved at the left.
pub fn quantizer_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec =
        autolayout::auto_layout_skipping(desc, STRIP_W, autolayout::group_prefix, &[], |k| {
            matches!(k, "scale.source" | "scale.id")
        });
    let total = TAG_H + GAP + BTN_H + GAP + BTN_H;
    let top = ((spec.height() - total) / 2.0).max(MARGIN_TOP);
    let x = CONTENT_X.0;
    let tag = Rect::new(x, top, x + STRIP_W, top + TAG_H);
    let new = Rect::new(
        x,
        tag.y1 + GAP,
        x + STRIP_W / 2.0 - 2.0,
        tag.y1 + GAP + BTN_H,
    );
    let edit = Rect::new(new.x1 + 4.0, new.y0, x + STRIP_W, new.y1);
    spec.scale_strip = Some(ScaleStrip { tag, new, edit });
    spec
}

/// Where a quantizer takes its scale from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The device's own scale.
    Own,
    /// The song key at the playhead.
    Key,
    /// The song scale with this id.
    Named(u8),
}

/// The source of `device` (a named id the song lacks is still `Named`).
pub fn source_of(device: &RackDevice, desc: &DeviceDescriptor) -> Source {
    let get = |key: &str| {
        desc.param_by_key(key)
            .map_or(0.0, |p| current_value(device, p))
    };
    match get("scale.source").round() as i32 {
        1 => Source::Key,
        2 => Source::Named(get("scale.id").round().clamp(0.0, 255.0) as u8),
        _ => Source::Own,
    }
}

/// The text of the tag for `source`: `OWN`, `KEY`, the scale's name, or `?` for an id the song
/// lacks.
pub fn tag_text(source: Source, scales: &[SongScaleName]) -> String {
    match source {
        Source::Own => "OWN".to_owned(),
        Source::Key => "KEY".to_owned(),
        Source::Named(id) => scales
            .iter()
            .find(|(i, _)| *i == id)
            .map_or_else(|| "?".to_owned(), |(_, n)| n.clone()),
    }
}

/// The sources in the order a click goes through them.
fn order(scales: &[SongScaleName]) -> Vec<Source> {
    let mut v = vec![Source::Own, Source::Key];
    v.extend(scales.iter().map(|(id, _)| Source::Named(*id)));
    v
}

/// The source after (or before) `current`; one the song lacks goes to `OWN`.
pub fn cycled(current: Source, scales: &[SongScaleName], forward: bool) -> Source {
    let all = order(scales);
    let Some(at) = all.iter().position(|s| *s == current) else {
        return Source::Own;
    };
    let n = all.len();
    all[(at + if forward { 1 } else { n - 1 }) % n]
}

/// The command that sets `source` on `device`: `scale.source` and `scale.id`, one undo step.
pub fn source_command(device: &DeviceKey, source: Source) -> Command {
    let set = |param: &str, value: f32| Command::SetParam {
        key: device.clone(),
        param: param.to_string(),
        value,
        merge: None,
    };
    let (kind, id) = match source {
        Source::Own => (0.0, 0.0),
        Source::Key => (1.0, 0.0),
        Source::Named(id) => (2.0, f32::from(id)),
    };
    Command::Batch(vec![set("scale.source", kind), set("scale.id", id)])
}

/// The notes of the device's own scale (its custom toggles, or the root and scale kind) as a
/// 12-bit mask.
pub fn own_mask(device: &RackDevice, desc: &DeviceDescriptor) -> u16 {
    let get = |key: &str| {
        desc.param_by_key(key)
            .map_or(0.0, |p| current_value(device, p))
    };
    if get("scale.custom") >= 0.5 {
        (0..12u16).fold(0, |m, pc| {
            if get(&format!("scale.note.{pc}")) >= 0.5 {
                m | (1 << pc)
            } else {
                m
            }
        })
    } else {
        let kinds = atomik_model::theory::ScaleKind::ALL;
        let kind = kinds[(get("scale").round().max(0.0) as usize).min(kinds.len() - 1)];
        let root = get("root").round().clamp(0.0, 11.0) as u8;
        atomik_model::theory::Scale::new(root, kind).pitch_mask()
    }
}

/// Draws the strip: the tag (accent while a song scale is chosen) and the two buttons.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    strip: &ScaleStrip,
    text: &str,
    named: bool,
    accent: Color,
) {
    let r = strip.tag;
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    let edge = if named { accent } else { theme.text_muted };
    c.stroke_with(RoundedRect::from_rect(r.inset(-0.5), 3.5), 1.0, edge);
    let label = fit_label_raw(text, r.width() - 8.0, 8.5);
    c.text(
        &TextRun::verbatim(
            label,
            pt(r.x0 + r.width() / 2.0, r.y0 + r.height() / 2.0 + 3.0),
            8.5,
            theme.text,
        )
        .align(Align::Center)
        .weight(700),
    );
    button_sized(c, theme, strip.new, "NEW", true, 8.5, 0.6);
    button_sized(c, theme, strip.edit, "EDIT", named, 8.5, 0.6);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scales() -> Vec<SongScaleName> {
        vec![(1, "Pent".to_owned()), (4, "Triad".to_owned())]
    }

    #[test]
    fn a_click_goes_through_own_key_and_the_song_scales_and_wraps() {
        let s = scales();
        let mut at = Source::Own;
        let mut seen = vec![at];
        for _ in 0..4 {
            at = cycled(at, &s, true);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [
                Source::Own,
                Source::Key,
                Source::Named(1),
                Source::Named(4),
                Source::Own
            ]
        );
        assert_eq!(cycled(Source::Own, &s, false), Source::Named(4));
        assert_eq!(
            cycled(Source::Named(9), &s, true),
            Source::Own,
            "a deleted scale"
        );
    }

    #[test]
    fn the_tag_names_the_source() {
        let s = scales();
        assert_eq!(tag_text(Source::Own, &s), "OWN");
        assert_eq!(tag_text(Source::Key, &s), "KEY");
        assert_eq!(tag_text(Source::Named(4), &s), "Triad");
        assert_eq!(tag_text(Source::Named(9), &s), "?");
    }

    #[test]
    fn choosing_a_source_sets_the_two_params_in_one_batch() {
        let k = DeviceKey("q".into());
        let Command::Batch(v) = source_command(&k, Source::Named(4)) else {
            panic!("one batch");
        };
        let vals: Vec<(String, f32)> = v
            .iter()
            .map(|c| match c {
                Command::SetParam { param, value, .. } => (param.clone(), *value),
                _ => panic!("a param"),
            })
            .collect();
        assert_eq!(
            vals,
            [
                ("scale.source".to_owned(), 2.0),
                ("scale.id".to_owned(), 4.0)
            ]
        );
    }
}
