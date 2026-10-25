//! The mark of a control that a macro bank drives: a dim veil over the control and a small
//! `M<n>` tag in its top right corner (the container panels draw the same tag for the params a
//! container's macro drives). The rack's panels draw their own through `PanelInfo::driven`; the
//! console strips and the parameter pane call [`paint_tag`].

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::Theme;
use vello::kurbo::{Rect, RoundedRect};

/// Paints the veil and the tag `M<index + 1>` over the control cell `rect`.
pub fn paint_tag(c: &mut dyn Canvas, theme: &Theme, rect: Rect, index: u8) {
    c.fill_with(
        RoundedRect::from_rect(rect, 4.0),
        theme.panel_bottom.with_alpha(0.55),
    );
    let tag = Rect::new(rect.x1 - 20.0, rect.y0 - 1.0, rect.x1 + 3.0, rect.y0 + 9.0);
    c.fill_with(RoundedRect::from_rect(tag, 3.0), theme.highlight);
    c.text(
        &TextRun::new(
            format!("M{}", u16::from(index) + 1),
            pt(tag.center().x, tag.center().y + 3.0),
            7.5,
            theme.plate,
        )
        .align(Align::Center)
        .weight(700),
    );
}
