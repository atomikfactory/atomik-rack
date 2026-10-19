//! Procedural vector painting of panels, widgets, jacks and cables. Everything is
//! drawn from [`Theme`] tokens; there are no bitmaps.

use crate::autolayout::{
    self, EAR, JACK_DY, PLATE_X, PanelSpec, PlacedControl, PlacedSocket, Section,
};
use crate::canvas::{Align, Canvas, CanvasExt, Geom, TextRun, pt};
use crate::theme::{Theme, fnv1a, mix};
use crate::widget::{current_value, format_value};
use atomik_core::{ParamDescriptor, Taper};
use atomik_device_sdk::{ControlKind, DeviceDescriptor, RACK_WIDTH, SocketKind};
use atomik_model::RackDevice;
use std::f64::consts::PI;
use vello::kurbo::{
    Affine, Arc, BezPath, Cap, Circle, CubicBez, Line, Point, Rect, RoundedRect, Shape, Stroke,
    Vec2,
};
use vello::peniko::{Brush, Color, Gradient};

/// Start angle of the knob sweep (radians, y down: lower left).
pub const KNOB_START: f64 = 0.75 * PI;
/// Knob sweep (270°).
pub const KNOB_SWEEP: f64 = 1.5 * PI;
/// Knob cap radius.
pub const KNOB_R: f64 = 11.5;
/// Knob value-ring radius.
pub const KNOB_RING_R: f64 = 15.5;
/// Knob centre offset from the top of its cell.
pub const KNOB_DY: f64 = 21.0;
/// Jack nut radius.
pub const JACK_R: f64 = 10.0;

/// Which face of the rack is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Controls.
    Front,
    /// Sockets.
    Back,
}

fn solid(c: Color) -> Brush {
    Brush::Solid(c)
}

fn with_alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

fn vgrad(y0: f64, y1: f64, top: Color, bottom: Color) -> Brush {
    Brush::Gradient(Gradient::new_linear(pt(0.0, y0), pt(0.0, y1)).with_stops([top, bottom]))
}

fn arc_path(center: Point, r: f64, start: f64, sweep: f64) -> BezPath {
    Arc::new(center, (r, r), start, sweep, 0.0)
        .path_elements(0.1)
        .collect()
}

fn round_stroke(width: f64) -> Stroke {
    Stroke::new(width).with_caps(Cap::Round)
}

/// Cuts a label to roughly fit `width` at `size` (no text measurement: platform-independent).
pub fn fit_label(text: &str, width: f64, size: f32) -> String {
    let max = ((width / (f64::from(size) * 0.56)).floor() as usize).max(1);
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
        s.push('.');
        s
    }
}

/// Splits a name into at most two lines that fit `width` (word boundaries first).
pub fn wrap_name(text: &str, width: f64, size: f32) -> Vec<String> {
    let fits = |t: &str| fit_label(t, width, size) == t;
    if fits(text) {
        return vec![text.to_string()];
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for split in (1..words.len()).rev() {
        let first = words[..split].join(" ");
        if fits(&first) {
            return vec![first, fit_label(&words[split..].join(" "), width, size)];
        }
    }
    vec![fit_label(text, width, size)]
}

/// Estimated advance of a run (for tooltip boxes only).
pub fn estimate_width(text: &str, size: f32) -> f64 {
    text.chars().count() as f64 * f64::from(size) * 0.58
}

// ------------------------------------------------------------------ rack furniture

/// The window background.
pub fn background<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, rect: Rect) {
    c.fill(
        &rect.into(),
        &vgrad(
            rect.y0,
            rect.y1,
            mix(theme.background, Color::WHITE, 0.02),
            theme.background,
        ),
    );
}

/// The rack rails beside the devices, from `y0` to `y1` (rack coordinates).
pub fn rails<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, y0: f64, y1: f64) {
    let w = f64::from(RACK_WIDTH);
    for (x0, x1) in [(-12.0, EAR - 2.0), (w - EAR + 2.0, w + 12.0)] {
        c.fill_with(
            RoundedRect::new(x0, y0, x1, y1, 2.0),
            Brush::Gradient(Gradient::new_linear(pt(x0, 0.0), pt(x1, 0.0)).with_stops([
                mix(theme.rail, Color::WHITE, 0.08),
                theme.rail,
                mix(theme.rail, Color::BLACK, 0.3),
            ])),
        );
        let cx = (x0 + x1) / 2.0;
        let unit = f64::from(atomik_device_sdk::RACK_UNIT);
        let mut y = y0 + unit / 6.0;
        while y < y1 {
            c.fill_with(
                RoundedRect::new(cx - 3.0, y - 2.2, cx + 3.0, y + 2.2, 1.2),
                theme.rail_hole,
            );
            y += unit / 3.0;
        }
    }
}

fn screw<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point, seed: u64) {
    c.fill_with(
        Circle::new(center + Vec2::new(0.0, 0.8), 5.0),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        Circle::new(center, 4.6),
        Brush::Gradient(
            Gradient::new_linear(center - Vec2::new(3.0, 4.0), center + Vec2::new(3.0, 4.0))
                .with_stops([theme.screw_light, theme.screw_dark]),
        ),
    );
    // A hex socket, rotated per screw so the rack does not look stamped.
    let angle = (seed % 60) as f64 * PI / 180.0;
    let mut hex = BezPath::new();
    for i in 0..6 {
        let a = angle + f64::from(i) * PI / 3.0;
        let p = center + Vec2::new(a.cos(), a.sin()) * 2.1;
        if i == 0 {
            hex.move_to(p);
        } else {
            hex.line_to(p);
        }
    }
    hex.close_path();
    c.fill_with(hex, mix(theme.screw_dark, Color::BLACK, 0.55));
}

fn ears<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, h: f64, seed: u64) {
    let w = f64::from(RACK_WIDTH);
    for (i, (x0, x1)) in [(0.0, EAR), (w - EAR, w)].into_iter().enumerate() {
        c.fill_with(
            Rect::new(x0, 0.0, x1, h),
            Brush::Gradient(
                Gradient::new_linear(pt(x0, 0.0), pt(x1, 0.0)).with_stops(if i == 0 {
                    [mix(theme.ear, Color::WHITE, 0.07), theme.ear]
                } else {
                    [theme.ear, mix(theme.ear, Color::BLACK, 0.2)]
                }),
            ),
        );
        let unit = f64::from(atomik_device_sdk::RACK_UNIT);
        let n = ((h / unit).round() as usize).max(1);
        for k in 0..n {
            let y = if h < unit {
                h / 2.0
            } else {
                unit * k as f64 + unit / 2.0
            };
            screw(
                c,
                theme,
                pt((x0 + x1) / 2.0, y),
                seed.wrapping_add((k * 7 + i) as u64),
            );
        }
    }
}

fn body<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, h: f64, side: Side, accent: Color) {
    let w = f64::from(RACK_WIDTH);
    let (top, bottom) = match side {
        Side::Front => (theme.panel_top, theme.panel_bottom),
        Side::Back => (theme.back_top, theme.back_bottom),
    };
    let r = Rect::new(EAR, 0.0, w - EAR, h);
    c.fill_with(r, vgrad(0.0, h, top, bottom));
    // A faint accent wash from the left, so each device has its own tint.
    c.fill_with(
        r,
        Brush::Gradient(
            Gradient::new_linear(pt(EAR, 0.0), pt(EAR + 260.0, 0.0))
                .with_stops([with_alpha(accent, 0.07), with_alpha(accent, 0.0)]),
        ),
    );
    if side == Side::Back {
        // Ventilation slots: a quiet texture that tells the back from the front at a glance.
        let mut x = w - EAR - 30.0;
        let y0 = 8.0;
        let y1 = (h - 8.0).min(y0 + 60.0);
        for _ in 0..3 {
            c.fill_with(
                RoundedRect::new(x, y0, x + 4.0, y1, 2.0),
                with_alpha(Color::BLACK, 0.35),
            );
            x -= 9.0;
        }
    }
    c.stroke_with(
        Line::new(pt(EAR, 0.5), pt(w - EAR, 0.5)),
        1.0,
        theme.bevel_light,
    );
    c.stroke_with(
        Line::new(pt(EAR, h - 0.5), pt(w - EAR, h - 0.5)),
        1.0,
        theme.bevel_dark,
    );
    c.stroke_with(
        Line::new(pt(EAR + 0.5, 0.0), pt(EAR + 0.5, h)),
        1.0,
        theme.bevel_dark,
    );
}

/// Everything needed to record one panel.
#[derive(Debug, Clone, Copy)]
pub struct PanelInfo<'a> {
    /// Display name.
    pub name: &'a str,
    /// Small caption under the name (descriptor name, upper case).
    pub caption: &'a str,
    /// Device accent.
    pub accent: Color,
    /// Descriptor (None for unknown types).
    pub desc: Option<&'a DeviceDescriptor>,
    /// Document device (None for the interface).
    pub device: Option<&'a RackDevice>,
    /// Panel geometry.
    pub spec: &'a PanelSpec,
    /// Face.
    pub side: Side,
    /// Folded strip.
    pub folded: bool,
    /// Panel height.
    pub height: f64,
    /// Seed for small per-device variations (screw angles).
    pub seed: u64,
    /// Drum panel: the selected pad (1..=16).
    pub pad: u8,
    /// Drum panel: the short name of each of the 16 pads (empty for a pad without zones).
    pub pad_names: &'a [String],
    /// Drum panel: the selected pad's outputs are already cabled (the "TO CH" button is dim).
    pub pad_cabled: bool,
    /// Slicer panel: the sensitivity of its Slice button, percent.
    pub sensitivity: u8,
    /// Effect panels: the display meter values ([`crate::fx::update_display`]); empty without
    /// a feed.
    pub fx_meters: &'a [f32],
    /// Compressor panel: the name of the device cabled to its sidechain input, if any.
    pub sc_source: Option<&'a str>,
    /// Character panel: whether the Transform and the Dry/Wet CV inputs have a cable.
    pub fx_cv: [bool; 2],
    /// Character panel: the animation phase of the screen, in cycles (it advances while the
    /// published meter values move, see `RackView::set_device_meter_values`).
    pub fx_phase: f32,
    /// Player panels: the edited pattern, the published meter values and the name
    /// being edited.
    pub player: crate::seq_panel::PlayerInfo<'a>,
    /// Container panels: the macro name being edited and the channel state.
    pub container: crate::container_panel::ContainerInfo<'a>,
    /// Plugin panels: the instance status and meters.
    pub plugin: crate::plugin_panel::PluginInfo<'a>,
    /// Params driven by a macro of an enclosing container (device inside a container): the
    /// param key and the macro index (0-based). Their controls draw locked with a macro tag.
    pub driven: &'a [(String, u8)],
    /// Audio I/O panel: socket lamp states, displayed levels and clip flags.
    pub io: crate::io_panel::IoInfo<'a>,
    /// Master panel: the effect on each return and the key-in state.
    pub master: crate::master_panel::MasterInfo<'a>,
}

/// Fold button centre in panel coordinates.
pub fn fold_button_center(height: f64, folded: bool) -> Point {
    if folded {
        pt(PLATE_X.0 + 9.0, height / 2.0)
    } else {
        pt(PLATE_X.0 + 10.0, height - 13.0)
    }
}

/// Name plate rectangle in panel coordinates.
pub fn plate_rect(height: f64, folded: bool) -> Rect {
    if folded {
        Rect::new(PLATE_X.0, 4.0, PLATE_X.1 + 160.0, height - 4.0)
    } else {
        Rect::new(PLATE_X.0, 6.0, PLATE_X.1, height - 6.0)
    }
}

fn chevron<C: Canvas + ?Sized>(c: &mut C, center: Point, open: bool, color: Color) {
    let mut p = BezPath::new();
    if open {
        p.move_to(center + Vec2::new(-3.5, -1.5));
        p.line_to(center + Vec2::new(0.0, 2.0));
        p.line_to(center + Vec2::new(3.5, -1.5));
    } else {
        p.move_to(center + Vec2::new(-1.5, -3.5));
        p.line_to(center + Vec2::new(2.0, 0.0));
        p.line_to(center + Vec2::new(-1.5, 3.5));
    }
    c.stroke(&p.into(), &round_stroke(1.6), &solid(color));
}

fn plate<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, info: &PanelInfo<'_>) {
    let r = plate_rect(info.height, info.folded);
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.plate);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        with_alpha(Color::WHITE, 0.06),
    );
    // Accent bar.
    if info.folded {
        c.fill_with(
            RoundedRect::new(r.x0, r.y0, r.x0 + 3.0, r.y1, 1.5),
            info.accent,
        );
    } else {
        c.fill_with(
            RoundedRect::new(r.x0, r.y0, r.x1, r.y0 + 3.0, 1.5),
            info.accent,
        );
    }
    let fold = fold_button_center(info.height, info.folded);
    if info.device.is_some() {
        c.fill_with(Circle::new(fold, 7.0), mix(theme.plate, Color::WHITE, 0.07));
        chevron(c, fold, !info.folded, theme.text_muted);
    }
    if info.folded {
        let x = fold.x + 14.0;
        c.text(
            &TextRun::new(
                info.name,
                pt(x, r.center().y + 4.0),
                theme.name_size - 1.0,
                theme.text,
            )
            .weight(700),
        );
        c.text(
            &TextRun::new(
                info.caption,
                pt(r.x1 - 8.0, r.center().y + 3.0),
                7.0,
                theme.text_muted,
            )
            .align(Align::End)
            .spacing(0.8),
        );
        return;
    }
    let lines = wrap_name(info.name, r.width() - 10.0, theme.name_size);
    let mut y = r.y0 + 19.0;
    for line in &lines {
        c.text(
            &TextRun::new(
                line.as_str(),
                pt(r.x0 + 6.0, y),
                theme.name_size,
                theme.text,
            )
            .weight(700),
        );
        y += theme.name_size as f64 + 1.0;
    }
    c.text(
        &TextRun::new(
            fit_label(info.caption, r.width() - 10.0, 6.5),
            pt(r.x0 + 6.0, y - 3.0),
            6.5,
            theme.text_muted,
        )
        .weight(600)
        .spacing(0.9),
    );
    if info.side == Side::Back {
        c.text(
            &TextRun::new("REAR", pt(r.x1 - 6.0, r.y1 - 9.0), 6.5, theme.text_muted)
                .align(Align::End)
                .spacing(1.2),
        );
    } else {
        // Status LED.
        let led = pt(
            r.x1 - 9.0,
            r.y1 - 9.0 + if info.device.is_none() { 0.0 } else { 2.0 },
        );
        c.fill_with(Circle::new(led, 5.0), with_alpha(info.accent, 0.22));
        c.fill_with(Circle::new(led, 2.4), info.accent);
    }
}

/// A small padlock centred on `center`: the mark of the locked Master Section.
pub fn lock_icon<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point) {
    let color = theme.text_muted;
    let body = Rect::new(
        center.x - 4.5,
        center.y - 1.0,
        center.x + 4.5,
        center.y + 5.0,
    );
    c.fill_with(RoundedRect::from_rect(body, 1.2), color);
    let mut shackle = BezPath::new();
    shackle.move_to(pt(center.x - 2.6, center.y - 1.0));
    shackle.line_to(pt(center.x - 2.6, center.y - 3.0));
    shackle.curve_to(
        pt(center.x - 2.6, center.y - 6.2),
        pt(center.x + 2.6, center.y - 6.2),
        pt(center.x + 2.6, center.y - 3.0),
    );
    shackle.line_to(pt(center.x + 2.6, center.y - 1.0));
    c.stroke(&shackle.into(), &round_stroke(1.3), &solid(color));
    c.fill_with(Circle::new(pt(center.x, center.y + 2.0), 0.9), theme.plate);
}

/// The output display of a channel or the master on its name plate: the
/// label in the normal text colour, the warning colour when the output leaves the master and
/// the muted colour for "Output: none". A folded strip shows it to the right of its plate.
pub fn output_display<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plate: Rect,
    folded: bool,
    text: &str,
    warn: bool,
) {
    let color = if warn {
        theme.warning
    } else if text == "Output: none" {
        theme.text_muted
    } else {
        theme.text
    };
    if folded {
        // A folded strip is slim and its plate is full: the label goes in the empty panel to
        // the right of the plate.
        let size = 8.0;
        c.text(&TextRun::new(
            fit_label(text, 300.0, size),
            pt(plate.x1 + 12.0, plate.center().y + 3.0),
            size,
            color,
        ));
    } else {
        let size = 6.0;
        c.text(&TextRun::new(
            fit_label(text, plate.width() - 8.0, size),
            pt(plate.x0 + 6.0, plate.y1 - 24.0),
            size,
            color,
        ));
    }
}

fn section_title<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, s: &Section) {
    section_title_with(c, theme, s, None);
}

/// [`section_title`] with a name after the title (the effect of a master return).
fn section_title_with<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    s: &Section,
    suffix: Option<&str>,
) {
    c.text(
        &TextRun::new(
            s.label.as_str(),
            pt(s.rect.x0, s.rect.y0 + 7.5),
            theme.section_size,
            theme.text_muted,
        )
        .weight(600)
        .spacing(1.0),
    );
    let mut lx = s.rect.x0 + estimate_width(&s.label, theme.section_size) + 6.0;
    if let Some(name) = suffix {
        let room = (s.rect.x1 - lx - 6.0).max(0.0);
        let text = fit_label(name, room, theme.section_size + 0.5);
        if !text.is_empty() {
            c.text(
                &TextRun::new(
                    text.as_str(),
                    pt(lx, s.rect.y0 + 7.5),
                    theme.section_size + 0.5,
                    if name == crate::master_panel::FREE_LABEL {
                        theme.text_muted
                    } else {
                        theme.text
                    },
                )
                .weight(600),
            );
            lx += estimate_width(&text, theme.section_size + 0.5) + 6.0;
        }
    }
    if lx < s.rect.x1 {
        c.stroke_with(
            Line::new(pt(lx, s.rect.y0 + 4.5), pt(s.rect.x1, s.rect.y0 + 4.5)),
            1.0,
            theme.section_line,
        );
    }
}

/// Records one device panel.
pub fn record_panel<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, info: &PanelInfo<'_>) {
    ears(c, theme, info.height, info.seed);
    body(c, theme, info.height, info.side, info.accent);
    plate(c, theme, info);
    if info.folded {
        return;
    }
    let Some(desc) = info.desc else {
        c.text(&TextRun::new(
            "Unknown device type",
            pt(autolayout::CONTENT_X.0, info.height / 2.0 + 4.0),
            10.0,
            theme.text_muted,
        ));
        return;
    };
    match info.side {
        Side::Front => {
            let is_master =
                info.device.is_some() && desc.type_id == crate::master_panel::MASTER_TYPE;
            for s in &info.spec.front_sections {
                let suffix = is_master
                    .then(|| crate::master_panel::section_suffix(&info.master, &s.label))
                    .flatten();
                section_title_with(c, theme, s, suffix.as_deref());
            }
            for pc in &info.spec.front {
                if info.spec.on_back(pc) {
                    continue;
                }
                let p = &desc.params[info.spec.param_index(desc, pc, info.pad)];
                let value = info.device.map_or(p.default, |d| current_value(d, p));
                if info.spec.fx.is_some() {
                    let label = crate::fx_layout::fx_label(p);
                    control_labelled(c, theme, pc, p, value, info.accent, &label);
                } else if info.spec.plugin.is_some() {
                    // Plugin panel: the knob is labelled with the plugin's own param name.
                    control_labelled(c, theme, pc, p, value, info.accent, p.name);
                } else if info.spec.container.is_some() {
                    // The macro name sits in its own target below the knob (container panel).
                    control_labelled(c, theme, pc, p, value, info.accent, "");
                } else if info.spec.seq.is_some() || info.spec.arp.is_some() {
                    let label = crate::seq_panel::player_label(p.key)
                        .map_or_else(|| autolayout::control_label(p), str::to_string);
                    control_labelled(c, theme, pc, p, value, info.accent, &label);
                } else if let Some(label) = is_master
                    .then(|| crate::master_panel::send_label(&info.master, p.key))
                    .flatten()
                {
                    control_labelled(c, theme, pc, p, value, info.accent, &label);
                } else {
                    control(c, theme, pc, p, value, info.accent);
                }
            }
            if is_master {
                crate::master_panel::paint_front(c, theme, info.spec, desc, &info.master);
            }
            if info.device.is_none() {
                interface_front(c, theme, desc, info);
            }
            if let (Some(strip), Some(device)) = (info.spec.samples, info.device) {
                sample_strip(c, theme, strip, device, info.accent);
            }
            if let Some(drum) = info.spec.drum {
                drum_panel(c, theme, drum, info);
            }
            if let (Some(slicer), Some(device)) = (info.spec.slicer, info.device) {
                slicer_panel(c, theme, slicer, device, info);
            }
            if let (Some(fx), Some(_)) = (info.spec.fx, info.device) {
                crate::fx_paint::fx_visual(c, theme, fx, info);
            }
            if let (Some(seq), Some(device)) = (info.spec.seq, info.device) {
                let pattern = desc
                    .param_by_key("pattern")
                    .map_or(1.0, |p| current_value(device, p));
                crate::seq_panel::paint_panel(
                    c,
                    theme,
                    &seq,
                    device,
                    pattern,
                    &info.player,
                    info.accent,
                );
            }
            if let (Some(panel), Some(device)) = (info.spec.container, info.device) {
                crate::container_panel::paint_panel(
                    c,
                    theme,
                    &panel,
                    device,
                    &info.container,
                    info.accent,
                );
            }
            if let (Some(panel), Some(device)) = (info.spec.plugin, info.device) {
                crate::plugin_panel::paint_panel(
                    c,
                    theme,
                    &panel,
                    device,
                    &info.plugin,
                    info.accent,
                );
            }
            if !info.driven.is_empty() {
                driven_tags(c, theme, desc, info, false);
            }
            if let (Some(arp), Some(device)) = (info.spec.arp, info.device) {
                crate::arp_panel::paint_panel(
                    c,
                    theme,
                    &arp,
                    device,
                    desc,
                    &info.player,
                    info.accent,
                );
            }
        }
        Side::Back => {
            for s in &info.spec.back_sections {
                section_title(c, theme, s);
            }
            for ps in &info.spec.back {
                let s = &desc.sockets[ps.socket];
                jack(c, theme, ps, s.name, s.kind);
                if s.direction == atomik_device_sdk::Direction::SummingInput {
                    summing_ring(c, theme, ps.center(), s.kind);
                }
            }
            // Controls placed on the back face (the Character CV trims).
            for pc in info.spec.front.iter().filter(|pc| info.spec.on_back(pc)) {
                let p = &desc.params[pc.param];
                let value = info.device.map_or(p.default, |d| current_value(d, p));
                let label = crate::fx_layout::fx_label(p);
                control_labelled(c, theme, pc, p, value, info.accent, &label);
            }
            if !info.driven.is_empty() {
                driven_tags(c, theme, desc, info, true);
            }
        }
    }
}

/// Locks the controls whose param a macro drives: a dim veil and a small `M<n>` tag.
fn driven_tags<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    desc: &DeviceDescriptor,
    info: &PanelInfo<'_>,
    back: bool,
) {
    for pc in info
        .spec
        .front
        .iter()
        .filter(|pc| info.spec.on_back(pc) == back)
    {
        let p = &desc.params[info.spec.param_index(desc, pc, info.pad)];
        let Some((_, m)) = info.driven.iter().find(|(k, _)| k == p.key) else {
            continue;
        };
        let r = pc.rect;
        c.fill_with(
            RoundedRect::from_rect(r, 4.0),
            with_alpha(theme.panel_bottom, 0.55),
        );
        let tag = Rect::new(r.x1 - 20.0, r.y0 - 1.0, r.x1 + 3.0, r.y0 + 9.0);
        c.fill_with(RoundedRect::from_rect(tag, 3.0), info.accent);
        c.text(
            &TextRun::new(
                format!("M{}", u16::from(*m) + 1),
                pt(tag.center().x, tag.center().y + 3.0),
                7.5,
                theme.plate,
            )
            .align(Align::Center)
            .weight(700),
        );
    }
}

/// The Edit button of a sample strip (panel coordinates).
pub fn sample_edit_button(strip: Rect) -> Rect {
    Rect::new(strip.x0, strip.y1 - 22.0, strip.x1, strip.y1 - 2.0)
}

/// The sample strip of a sampler panel: a title, an LCD with the zone and sample counts and the
/// Edit button. The whole panel is also the drop target (highlighted by the rack view).
fn sample_strip<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    strip: Rect,
    device: &RackDevice,
    accent: Color,
) {
    let title = Section {
        label: "SAMPLES".to_string(),
        rect: Rect::new(strip.x0, strip.y0, strip.x1, strip.y0 + 11.0),
    };
    section_title(c, theme, &title);
    let lcd = Rect::new(strip.x0, strip.y0 + 14.0, strip.x1, strip.y0 + 54.0);
    c.fill_with(RoundedRect::from_rect(lcd, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(lcd.inset(-0.5), 3.5),
        1.0,
        with_alpha(Color::WHITE, 0.07),
    );
    let n = device.zones.len();
    let samples = device
        .zones
        .iter()
        .map(|z| z.asset.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let (l1, l2) = if n == 0 {
        ("No samples".to_string(), "drop files here".to_string())
    } else {
        (
            format!("{n} {}", if n == 1 { "zone" } else { "zones" }),
            format!(
                "{samples} {}",
                if samples == 1 { "sample" } else { "samples" }
            ),
        )
    };
    let l1_color = if n == 0 { theme.text_muted } else { accent };
    c.text(&TextRun::new(l1, pt(lcd.x0 + 8.0, lcd.y0 + 16.0), 10.5, l1_color).weight(700));
    c.text(&TextRun::new(
        l2,
        pt(lcd.x0 + 8.0, lcd.y0 + 31.0),
        8.5,
        theme.text_muted,
    ));
    edit_button(c, theme, sample_edit_button(strip), "EDIT", true);
}

/// A panel button (EDIT of the sample strip and the drum panel, TO CH of the drum panel);
/// drawn dim when disabled.
pub(crate) fn edit_button<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    b: Rect,
    label: &str,
    enabled: bool,
) {
    button_sized(c, theme, b, label, enabled, theme.label_size + 1.0, 1.0);
}

/// [`edit_button`] with its own text size and letter spacing (narrow buttons).
pub(crate) fn button_sized<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    b: Rect,
    label: &str,
    enabled: bool,
    size: f32,
    spacing: f32,
) {
    let dim = if enabled { 1.0 } else { 0.4 };
    c.fill_with(
        RoundedRect::from_rect(b + Vec2::new(0.0, 1.0), 3.0),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(b, 3.0),
        vgrad(
            b.y0,
            b.y1,
            mix(theme.button, Color::WHITE, 0.08),
            theme.button,
        ),
    );
    c.text(
        &TextRun::new(
            label,
            pt(b.center().x, b.center().y + 3.5),
            size,
            with_alpha(theme.text, dim),
        )
        .align(Align::Center)
        .weight(700)
        .spacing(spacing),
    );
}

/// The drum panel's extras: the 4x4 pad grid (selected pad highlighted, empty pads dimmed),
/// the readout of the selected pad and the Edit button.
fn drum_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    drum: autolayout::DrumPanel,
    info: &PanelInfo<'_>,
) {
    section_title(
        c,
        theme,
        &Section {
            label: format!("PAD {}", info.pad),
            rect: drum.pad_title,
        },
    );
    for pad in 1..=16u8 {
        let r = autolayout::drum_pad_rect(drum.pads, pad);
        let name = info
            .pad_names
            .get(usize::from(pad - 1))
            .map_or("", String::as_str);
        let loaded = !name.is_empty();
        let selected = pad == info.pad;
        let face = match (selected, loaded) {
            (true, true) => mix(theme.button, info.accent, 0.35),
            (true, false) => mix(theme.lcd, info.accent, 0.2),
            (false, true) => mix(theme.button, info.accent, 0.12),
            (false, false) => with_alpha(theme.lcd, 0.8),
        };
        let rr = RoundedRect::from_rect(r, 4.0);
        c.fill_with(rr, face);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 4.5),
            if selected { 2.0 } else { 1.0 },
            if selected {
                info.accent
            } else {
                theme.bevel_dark
            },
        );
        let dim = if loaded { 1.0 } else { 0.45 };
        c.text(
            &TextRun::new(
                pad.to_string(),
                pt(r.x0 + 5.0, r.y0 + 11.0),
                9.0,
                if loaded {
                    info.accent
                } else {
                    theme.text_muted
                }
                .with_alpha(dim),
            )
            .weight(700),
        );
        if loaded {
            c.text(
                &TextRun::new(
                    fit_label(name, r.width() - 2.0, 8.0),
                    pt(r.center().x, r.y1 - 6.0),
                    8.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
    }
    let lcd = drum.info;
    c.fill_with(RoundedRect::from_rect(lcd, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(lcd.inset(-0.5), 3.5),
        1.0,
        with_alpha(Color::WHITE, 0.07),
    );
    let key = atomik_model::pad_key(info.pad);
    c.text(
        &TextRun::new(
            format!("PAD {}  ·  {}", info.pad, crate::zone_edit::note_name(key)),
            pt(lcd.x0 + 8.0, lcd.y0 + 14.0),
            10.0,
            info.accent,
        )
        .weight(700),
    );
    let name = info
        .pad_names
        .get(usize::from(info.pad - 1))
        .filter(|n| !n.is_empty());
    c.text(&TextRun::new(
        name.map_or_else(
            || "empty: drop a sample here".to_string(),
            |n| fit_label(n, lcd.width() - 16.0, 8.5),
        ),
        pt(lcd.x0 + 8.0, lcd.y0 + 28.0),
        8.5,
        theme.text_muted,
    ));
    edit_button(c, theme, drum.edit, "EDIT", true);
    edit_button(c, theme, drum.to_channel, "TO CH", !info.pad_cabled);
}

/// The loop slicer panel's extras: the overview strip (slice count and boundaries), the
/// sensitivity knob and the SLICE, TO TRACK and EDIT buttons (the first two are dim without a
/// loop).
fn slicer_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: autolayout::SlicerPanel,
    device: &RackDevice,
    info: &PanelInfo<'_>,
) {
    let keys =
        atomik_model::SLICE_KEY_BASE..atomik_model::SLICE_KEY_BASE + atomik_model::MAX_SLICES as u8;
    let mut slices: Vec<&atomik_model::ZoneDoc> = device
        .zones
        .iter()
        .filter(|z| z.key_lo == z.key_hi && keys.contains(&z.key_lo))
        .collect();
    slices.sort_by_key(|z| z.key_lo);
    let lcd = panel.strip;
    c.fill_with(RoundedRect::from_rect(lcd, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(lcd.inset(-0.5), 3.5),
        1.0,
        with_alpha(Color::WHITE, 0.07),
    );
    let total = slices.iter().map(|z| z.end).max().unwrap_or(0);
    if slices.is_empty() || total == 0 {
        c.text(
            &TextRun::new(
                "No loop",
                pt(lcd.x0 + 8.0, lcd.y0 + 16.0),
                10.5,
                theme.text_muted,
            )
            .weight(700),
        );
        c.text(&TextRun::new(
            "drop a loop here",
            pt(lcd.x0 + 8.0, lcd.y0 + 31.0),
            8.5,
            theme.text_muted,
        ));
    } else {
        let n = slices.len();
        c.text(
            &TextRun::new(
                format!("{n} {}", if n == 1 { "slice" } else { "slices" }),
                pt(lcd.x0 + 8.0, lcd.y0 + 16.0),
                10.5,
                info.accent,
            )
            .weight(700),
        );
        let bar = Rect::new(lcd.x0 + 8.0, lcd.y0 + 24.0, lcd.x1 - 8.0, lcd.y1 - 6.0);
        c.fill_with(bar, mix(theme.lcd, info.accent, 0.12));
        let x_of = |f: u64| bar.x0 + f as f64 / total as f64 * bar.width();
        for z in &slices {
            if z.reverse {
                c.fill_with(
                    Rect::new(x_of(z.start), bar.y0, x_of(z.end), bar.y1),
                    with_alpha(info.accent, 0.35),
                );
            }
            let x = x_of(z.start).round() + 0.5;
            c.stroke_with(Line::new(pt(x, bar.y0), pt(x, bar.y1)), 1.0, info.accent);
        }
    }
    let n = f64::from(info.sensitivity) / 100.0;
    knob(
        c,
        theme,
        autolayout::slicer_sens_knob(panel.sens),
        n,
        false,
        info.accent,
    );
    c.text(
        &TextRun::new(
            format!("SENS {}%", info.sensitivity),
            pt(panel.sens.center().x, panel.sens.y0 + 52.0),
            theme.label_size,
            theme.text_muted,
        )
        .align(Align::Center)
        .weight(600),
    );
    let loaded = !slices.is_empty();
    edit_button(c, theme, panel.slice, "SLICE", loaded);
    edit_button(c, theme, panel.to_track, "TO TRACK", loaded);
    edit_button(c, theme, panel.edit, "EDIT", true);
}

fn interface_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    desc: &DeviceDescriptor,
    info: &PanelInfo<'_>,
) {
    if desc.type_id == crate::interface::INTERFACE_TYPE_ID {
        crate::io_panel::paint_front(c, theme, desc, &info.io);
        return;
    }
    let y = info.height / 2.0;
    let x0 = autolayout::CONTENT_X.0;
    c.text(
        &TextRun::new(
            "OUTPUTS",
            pt(x0, y - 12.0),
            theme.section_size,
            theme.text_muted,
        )
        .weight(600)
        .spacing(1.0),
    );
    for (i, s) in desc.sockets.iter().enumerate() {
        let cx = x0 + 10.0 + i as f64 * 34.0;
        c.fill_with(
            RoundedRect::new(cx - 9.0, y - 4.0, cx + 9.0, y + 8.0, 2.0),
            theme.lcd,
        );
        c.fill_with(Circle::new(pt(cx, y + 2.0), 2.4), info.accent);
        c.text(
            &TextRun::new(
                s.name.trim_start_matches("Output "),
                pt(cx, y + 22.0),
                theme.label_size,
                theme.text_muted,
            )
            .align(Align::Center),
        );
    }
}

/// Normalized position and whether the param is bipolar (arc from the centre).
fn norm_of(p: &ParamDescriptor, value: f32) -> (f64, bool) {
    let bipolar = p.min < 0.0 && p.max > 0.0 && matches!(p.taper, Taper::Linear);
    (f64::from(p.to_normalized(value)), bipolar)
}

/// Draws one control in its cell.
pub fn control<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    pc: &PlacedControl,
    p: &ParamDescriptor,
    value: f32,
    accent: Color,
) {
    control_labelled(
        c,
        theme,
        pc,
        p,
        value,
        accent,
        &autolayout::control_label(p),
    );
}

/// Like [`control`] with an explicit label text.
pub fn control_labelled<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    pc: &PlacedControl,
    p: &ParamDescriptor,
    value: f32,
    accent: Color,
    text: &str,
) {
    let r = pc.rect;
    let cx = r.center().x;
    let label = fit_label(text, r.width() + 6.0, theme.label_size);
    match pc.kind {
        ControlKind::Knob => {
            let (n, bipolar) = norm_of(p, value);
            knob(c, theme, pt(cx, r.y0 + KNOB_DY), n, bipolar, accent);
            c.text(
                &TextRun::new(
                    label,
                    pt(cx, r.y0 + 52.0),
                    theme.label_size,
                    theme.text_muted,
                )
                .align(Align::Center),
            );
        }
        ControlKind::Fader => {
            let (n, _) = norm_of(p, value);
            fader(c, theme, r, n, accent);
            c.text(
                &TextRun::new(
                    label,
                    pt(cx, r.y1 - 4.0),
                    theme.label_size,
                    theme.text_muted,
                )
                .align(Align::Center),
            );
        }
        ControlKind::Toggle => {
            let on = value >= (p.min + p.max) / 2.0;
            let b = Rect::new(cx - 15.0, r.y0 + 2.0, cx + 15.0, r.y0 + 15.0);
            c.fill_with(
                RoundedRect::from_rect(b + Vec2::new(0.0, 1.0), 3.0),
                with_alpha(Color::BLACK, 0.5),
            );
            c.fill_with(
                RoundedRect::from_rect(b, 3.0),
                vgrad(
                    b.y0,
                    b.y1,
                    mix(theme.button, Color::WHITE, 0.08),
                    theme.button,
                ),
            );
            let led = pt(cx, b.center().y);
            if on {
                c.fill_with(Circle::new(led, 6.0), with_alpha(accent, 0.25));
                c.fill_with(Circle::new(led, 2.6), mix(accent, Color::WHITE, 0.25));
            } else {
                c.fill_with(Circle::new(led, 2.6), theme.led_off);
            }
            c.text(
                &TextRun::new(
                    label,
                    pt(cx, r.y0 + 25.0),
                    theme.label_size,
                    theme.text_muted,
                )
                .align(Align::Center),
            );
        }
        ControlKind::Choice | ControlKind::Display => {
            let lcd = Rect::new(cx - 28.0, r.y0 + 3.0, cx + 28.0, r.y0 + 19.0);
            c.fill_with(RoundedRect::from_rect(lcd, 3.0), theme.lcd);
            c.stroke_with(
                RoundedRect::from_rect(lcd.inset(-0.5), 3.5),
                1.0,
                with_alpha(Color::WHITE, 0.07),
            );
            let text = fit_label(&format_value(p, value), lcd.width() - 14.0, 8.5);
            c.text(
                &TextRun::new(
                    text,
                    pt(cx, lcd.y0 + 11.5),
                    8.5,
                    mix(accent, Color::WHITE, 0.3),
                )
                .weight(600)
                .align(Align::Center),
            );
            if pc.kind == ControlKind::Choice && crate::master_panel::is_pick_list(p) {
                // A long choice opens a list: one caret instead of the two arrows.
                let (x, y) = (lcd.x1 - 7.0, lcd.center().y);
                let mut t = BezPath::new();
                t.move_to(pt(x - 2.8, y - 1.6));
                t.line_to(pt(x + 2.8, y - 1.6));
                t.line_to(pt(x, y + 2.0));
                t.close_path();
                c.fill_with(t, theme.text_muted);
            } else if pc.kind == ControlKind::Choice {
                for (x, dir) in [(lcd.x0 + 5.0, -1.0), (lcd.x1 - 5.0, 1.0)] {
                    let mut t = BezPath::new();
                    let y = lcd.center().y;
                    t.move_to(pt(x + 1.6 * dir, y));
                    t.line_to(pt(x - 1.6 * dir, y - 2.6));
                    t.line_to(pt(x - 1.6 * dir, y + 2.6));
                    t.close_path();
                    c.fill_with(t, theme.text_muted);
                }
            }
            c.text(
                &TextRun::new(
                    label,
                    pt(cx, r.y0 + 31.0),
                    theme.label_size,
                    theme.text_muted,
                )
                .align(Align::Center),
            );
        }
    }
}

/// A knob at `center` showing normalized value `n`.
pub fn knob<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    center: Point,
    n: f64,
    bipolar: bool,
    accent: Color,
) {
    c.stroke(
        &arc_path(center, KNOB_RING_R, KNOB_START, KNOB_SWEEP).into(),
        &round_stroke(2.6),
        &solid(theme.knob_track),
    );
    let from: f64 = if bipolar { 0.5 } else { 0.0 };
    let (a, b) = (from.min(n), from.max(n));
    if b - a > 1e-4 {
        c.stroke(
            &arc_path(
                center,
                KNOB_RING_R,
                KNOB_START + a * KNOB_SWEEP,
                (b - a) * KNOB_SWEEP,
            )
            .into(),
            &round_stroke(2.6),
            &solid(accent),
        );
    }
    c.fill_with(
        Circle::new(center + Vec2::new(0.0, 2.0), KNOB_R + 0.5),
        with_alpha(Color::BLACK, 0.55),
    );
    c.fill_with(
        Circle::new(center, KNOB_R),
        vgrad(
            center.y - KNOB_R,
            center.y + KNOB_R,
            theme.knob_top,
            theme.knob_bottom,
        ),
    );
    c.fill_with(
        Circle::new(center, KNOB_R - 2.4),
        vgrad(
            center.y - KNOB_R,
            center.y + KNOB_R,
            mix(theme.knob_bottom, theme.knob_top, 0.35),
            mix(theme.knob_top, Color::WHITE, 0.04),
        ),
    );
    c.stroke_with(
        Circle::new(center, KNOB_R - 0.5),
        1.0,
        with_alpha(Color::WHITE, 0.07),
    );
    let angle = KNOB_START + n * KNOB_SWEEP;
    let dir = Vec2::new(angle.cos(), angle.sin());
    c.stroke(
        &Line::new(center + dir * 3.0, center + dir * (KNOB_R - 2.0)).into(),
        &round_stroke(2.0),
        &solid(theme.knob_pointer),
    );
}

/// Fader slot geometry for a cell: (x, top, bottom) of the travel.
pub fn fader_travel(r: Rect) -> (f64, f64, f64) {
    (r.center().x, r.y0 + 7.0, r.y1 - 19.0)
}

fn fader<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, n: f64, accent: Color) {
    let (x, top, bottom) = fader_travel(r);
    let steps = 8;
    for i in 0..=steps {
        let y = top + (bottom - top) * f64::from(i) / f64::from(steps);
        let w = if i % 4 == 0 { 5.0 } else { 3.0 };
        c.stroke_with(
            Line::new(pt(x - 9.0 - w, y), pt(x - 9.0, y)),
            1.0,
            with_alpha(theme.text_muted, 0.5),
        );
    }
    c.fill_with(
        RoundedRect::new(x - 2.5, top - 2.0, x + 2.5, bottom + 2.0, 2.5),
        theme.knob_track,
    );
    let y = bottom - (bottom - top) * n;
    c.stroke(
        &Line::new(pt(x, bottom), pt(x, y)).into(),
        &round_stroke(2.0),
        &solid(accent),
    );
    let cap = Rect::new(x - 10.0, y - 5.5, x + 10.0, y + 5.5);
    c.fill_with(
        RoundedRect::from_rect(cap + Vec2::new(0.0, 1.5), 2.0),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(cap, 2.0),
        vgrad(cap.y0, cap.y1, theme.knob_top, theme.knob_bottom),
    );
    c.stroke_with(
        Line::new(pt(cap.x0 + 3.0, y), pt(cap.x1 - 3.0, y)),
        1.2,
        theme.knob_pointer,
    );
}

/// Ring colour of a jack.
pub fn kind_color(theme: &Theme, kind: SocketKind) -> Color {
    theme.cable_color(kind)
}

/// Draws a jack in its cell.
pub fn jack<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    ps: &PlacedSocket,
    name: &str,
    kind: SocketKind,
) {
    let center = ps.center();
    let mut nut = BezPath::new();
    for i in 0..6 {
        let a = f64::from(i) * PI / 3.0 + PI / 6.0;
        let p = center + Vec2::new(a.cos(), a.sin()) * JACK_R;
        if i == 0 {
            nut.move_to(p);
        } else {
            nut.line_to(p);
        }
    }
    nut.close_path();
    c.fill_with(
        Circle::new(center + Vec2::new(0.0, 1.5), JACK_R),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        nut,
        Brush::Gradient(
            Gradient::new_linear(center - Vec2::new(6.0, 9.0), center + Vec2::new(6.0, 9.0))
                .with_stops([theme.jack_light, theme.jack_dark]),
        ),
    );
    c.stroke_with(Circle::new(center, 6.6), 2.2, kind_color(theme, kind));
    c.fill_with(Circle::new(center, 4.4), theme.jack_hole);
    c.stroke(
        &arc_path(center, 3.2, 1.1 * PI, 0.5 * PI).into(),
        &Stroke::new(0.8),
        &solid(with_alpha(Color::WHITE, 0.18)),
    );
    let label = fit_label(name, ps.rect.width() + 4.0, 7.5);
    c.text(
        &TextRun::new(
            label,
            pt(center.x, ps.rect.y0 + JACK_DY + 22.0),
            7.5,
            theme.text_muted,
        )
        .align(Align::Center),
    );
}

/// The extra ring that marks a summing (bus) input: a dashed circle outside the nut, in the
/// socket kind's colour, so it reads as "accepts many cables".
pub fn summing_ring<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point, kind: SocketKind) {
    let dashes = 12;
    let step = 2.0 * PI / f64::from(dashes);
    for i in 0..dashes {
        c.stroke(
            &arc_path(center, JACK_R + 3.0, f64::from(i) * step, step * 0.55).into(),
            &round_stroke(1.4),
            &solid(with_alpha(kind_color(theme, kind), 0.85)),
        );
    }
}

// ------------------------------------------------------------------ overlays

/// How a cable is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableLook {
    /// Normal.
    Normal,
    /// Hovered.
    Hovered,
    /// Being dragged.
    Dragging,
}

/// Draws a cable with shadow, glossy highlight and plugs.
pub fn cable<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    curve: CubicBez,
    color: Color,
    look: CableLook,
    plug_end: bool,
) {
    let shadow = Affine::translate((3.0, 6.0));
    let shadow_curve = CubicBez::new(
        shadow * curve.p0,
        shadow * curve.p1,
        shadow * curve.p2,
        shadow * curve.p3,
    );
    let width = if look == CableLook::Hovered { 6.0 } else { 4.6 };
    c.stroke(
        &Geom::Cubic(shadow_curve),
        &round_stroke(width + 1.0),
        &solid(theme.cable_shadow),
    );
    if look == CableLook::Hovered {
        c.stroke(
            &Geom::Cubic(curve),
            &round_stroke(width + 7.0),
            &solid(with_alpha(theme.highlight, 0.25)),
        );
    }
    let body = if look == CableLook::Dragging {
        with_alpha(color, 0.85)
    } else {
        color
    };
    c.stroke(&Geom::Cubic(curve), &round_stroke(width), &solid(body));
    let gloss = Affine::translate((-0.9, -0.9));
    c.stroke(
        &Geom::Cubic(CubicBez::new(
            gloss * curve.p0,
            gloss * curve.p1,
            gloss * curve.p2,
            gloss * curve.p3,
        )),
        &round_stroke(1.2),
        &solid(with_alpha(mix(color, Color::WHITE, 0.6), 0.55)),
    );
    let ends: &[Point] = if plug_end {
        &[curve.p0, curve.p3]
    } else {
        &[curve.p0]
    };
    for &p in ends {
        c.fill_with(
            Circle::new(p, 6.2),
            mix(theme.jack_hole, Color::WHITE, 0.12),
        );
        c.stroke_with(Circle::new(p, 5.0), 2.0, color);
        c.fill_with(Circle::new(p, 2.2), mix(color, Color::WHITE, 0.3));
    }
}

/// Socket overlay ring states used while dragging or hovering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring {
    /// Bright ring.
    Highlight,
    /// Strong ring (drop target / source).
    Strong,
    /// Dimmed.
    Dim,
}

/// Draws a ring or dimming disc over a jack.
pub fn socket_ring<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point, ring: Ring) {
    match ring {
        Ring::Highlight => {
            c.stroke_with(
                Circle::new(center, JACK_R + 3.0),
                1.6,
                with_alpha(theme.highlight, 0.8),
            );
        }
        Ring::Strong => {
            c.fill_with(
                Circle::new(center, JACK_R + 6.0),
                with_alpha(theme.highlight, 0.18),
            );
            c.stroke_with(Circle::new(center, JACK_R + 3.0), 2.4, theme.highlight);
        }
        Ring::Dim => {
            c.fill_with(Circle::new(center, JACK_R + 1.5), theme.dim);
        }
    }
}

/// A tooltip box with `text` near `at` (logical view coordinates).
pub fn tooltip<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, at: Point, text: &str) {
    let w = estimate_width(text, 10.0) + 14.0;
    let r = Rect::new(at.x + 14.0, at.y - 30.0, at.x + 14.0 + w, at.y - 10.0);
    c.fill_with(RoundedRect::from_rect(r, 4.0), theme.tooltip_bg);
    c.stroke_with(
        RoundedRect::from_rect(r, 4.0),
        1.0,
        with_alpha(Color::WHITE, 0.1),
    );
    c.text(&TextRun::new(text, pt(r.x0 + 7.0, r.y1 - 6.5), 10.0, theme.text).weight(600));
}

/// An error toast centred at the bottom of a `width` × `height` view.
pub fn toast<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, width: f64, height: f64, text: &str) {
    let w = estimate_width(text, 11.0) + 28.0;
    let r = Rect::new(
        (width - w) / 2.0,
        height - 52.0,
        (width + w) / 2.0,
        height - 22.0,
    );
    c.fill_with(RoundedRect::from_rect(r, 6.0), theme.error_bg);
    c.text(
        &TextRun::new(text, pt(r.center().x, r.y1 - 10.5), 11.0, Color::WHITE)
            .weight(600)
            .align(Align::Center),
    );
}

/// Seed for per-device variation.
pub fn seed_of(key: &str) -> u64 {
    fnv1a(key.as_bytes())
}

/// Shape helper used by overlays.
pub fn outline<C: Canvas + ?Sized>(c: &mut C, rect: Rect, radius: f64, width: f64, color: Color) {
    c.stroke(
        &RoundedRect::from_rect(rect, radius).into(),
        &Stroke::new(width),
        &solid(color),
    );
}

/// Fills a rectangle.
pub fn fill_rect<C: Canvas + ?Sized>(c: &mut C, rect: Rect, color: Color) {
    c.fill(&rect.into(), &solid(color));
}

/// Bounding box helper.
pub fn bbox(shape: &impl Shape) -> Rect {
    shape.bounding_box()
}
