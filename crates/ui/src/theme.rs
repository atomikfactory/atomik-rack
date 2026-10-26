//! The procedural dark theme: every colour and size comes from these
//! tokens; nothing is a bitmap.

use atomik_device_sdk::SocketKind;
use atomik_model::RackDevice;
use vello::peniko::Color;

const fn c(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}
const fn ca(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// Theme tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Window background behind the rack.
    pub background: Color,
    /// Rack rail metal.
    pub rail: Color,
    /// Rack rail mounting holes.
    pub rail_hole: Color,
    /// Front panel gradient, top.
    pub panel_top: Color,
    /// Front panel gradient, bottom.
    pub panel_bottom: Color,
    /// Back panel gradient, top.
    pub back_top: Color,
    /// Back panel gradient, bottom.
    pub back_bottom: Color,
    /// Bevel highlight (top edge).
    pub bevel_light: Color,
    /// Bevel shadow (bottom edge).
    pub bevel_dark: Color,
    /// Rack ear (mounting flange) fill.
    pub ear: Color,
    /// Screw head highlight.
    pub screw_light: Color,
    /// Screw head shadow.
    pub screw_dark: Color,
    /// Name plate background.
    pub plate: Color,
    /// Primary text (names, values).
    pub text: Color,
    /// Secondary text (labels).
    pub text_muted: Color,
    /// Section rule lines.
    pub section_line: Color,
    /// Knob cap gradient, top.
    pub knob_top: Color,
    /// Knob cap gradient, bottom.
    pub knob_bottom: Color,
    /// Knob track (unfilled value arc).
    pub knob_track: Color,
    /// Knob pointer line.
    pub knob_pointer: Color,
    /// Jack ring highlight.
    pub jack_light: Color,
    /// Jack ring shadow.
    pub jack_dark: Color,
    /// Jack hole.
    pub jack_hole: Color,
    /// LED when off.
    pub led_off: Color,
    /// LCD / selector window background.
    pub lcd: Color,
    /// Button face.
    pub button: Color,
    /// Audio cable colour.
    pub cable_audio: Color,
    /// CV cable colour.
    pub cable_cv: Color,
    /// Note cable colour.
    pub cable_notes: Color,
    /// Cable drop shadow.
    pub cable_shadow: Color,
    /// Highlight ring (hover, compatible sockets).
    pub highlight: Color,
    /// Overlay dimming incompatible sockets during a cable drag.
    pub dim: Color,
    /// Keyboard focus ring.
    pub focus: Color,
    /// Selected device outline.
    pub selection: Color,
    /// Tooltip background.
    pub tooltip_bg: Color,
    /// Error toast background.
    pub error_bg: Color,
    /// Warning text (an output that leaves the master).
    pub warning: Color,
    /// Audio I/O socket lamp: the socket has a cable and the host device has the channel.
    pub io_connected: Color,
    /// Audio I/O socket lamp: no cable, the channel exists.
    pub io_free: Color,
    /// Audio I/O socket lamp: a cable exists but the host device has no such channel.
    pub io_unavailable: Color,
    /// Audio I/O clip lamp when lit (a peak above 0 dBFS since the last reset).
    pub clip: Color,
    /// Browser panel background.
    pub browser_bg: Color,
    /// Browser row hover / selection.
    pub browser_row: Color,
    /// Accent saturation (HSL) for hashed accents.
    pub accent_saturation: f32,
    /// Accent lightness (HSL) for hashed accents.
    pub accent_lightness: f32,
    /// Label font size.
    pub label_size: f32,
    /// Section title font size.
    pub section_size: f32,
    /// Device name font size.
    pub name_size: f32,
    /// Panel corner radius.
    pub corner: f64,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The default dark theme: graphite panels, warm-white type, one accent hue per device.
    pub fn dark() -> Self {
        Self {
            background: c(0x0e, 0x10, 0x13),
            rail: c(0x2a, 0x2e, 0x34),
            rail_hole: c(0x0b, 0x0c, 0x0e),
            panel_top: c(0x2f, 0x33, 0x39),
            panel_bottom: c(0x23, 0x26, 0x2b),
            back_top: c(0x1f, 0x22, 0x26),
            back_bottom: c(0x18, 0x1a, 0x1d),
            bevel_light: ca(0xff, 0xff, 0xff, 0x24),
            bevel_dark: ca(0x00, 0x00, 0x00, 0x8c),
            ear: c(0x3a, 0x3e, 0x45),
            screw_light: c(0xb9, 0xbd, 0xc4),
            screw_dark: c(0x4c, 0x50, 0x57),
            plate: c(0x17, 0x19, 0x1c),
            text: c(0xec, 0xe8, 0xe1),
            text_muted: c(0x8f, 0x96, 0xa0),
            section_line: ca(0xff, 0xff, 0xff, 0x1c),
            knob_top: c(0x4a, 0x4f, 0x57),
            knob_bottom: c(0x1c, 0x1e, 0x22),
            knob_track: c(0x14, 0x15, 0x18),
            knob_pointer: c(0xf4, 0xf1, 0xea),
            jack_light: c(0xc8, 0xcc, 0xd2),
            jack_dark: c(0x5a, 0x5e, 0x66),
            jack_hole: c(0x05, 0x05, 0x06),
            led_off: c(0x26, 0x29, 0x2d),
            lcd: c(0x0f, 0x12, 0x14),
            button: c(0x3b, 0x40, 0x47),
            cable_audio: c(0xe8, 0x5d, 0x3f),
            cable_cv: c(0x4f, 0xa8, 0xe0),
            cable_notes: c(0x9c, 0xd3, 0x5b),
            cable_shadow: ca(0x00, 0x00, 0x00, 0x66),
            highlight: c(0xff, 0xe0, 0x8a),
            dim: ca(0x0a, 0x0b, 0x0d, 0xb0),
            focus: c(0x7c, 0xc4, 0xff),
            selection: ca(0xff, 0xe0, 0x8a, 0xb0),
            tooltip_bg: ca(0x0b, 0x0c, 0x0e, 0xec),
            error_bg: ca(0x8a, 0x22, 0x22, 0xf0),
            warning: c(0xe6, 0xb4, 0x3c),
            io_connected: c(0x5c, 0xc8, 0x6e),
            io_free: c(0xe6, 0xc2, 0x3c),
            io_unavailable: c(0xe8, 0x54, 0x54),
            clip: c(0xff, 0x4d, 0x3d),
            browser_bg: c(0x16, 0x18, 0x1b),
            browser_row: c(0x26, 0x2a, 0x30),
            accent_saturation: 0.62,
            accent_lightness: 0.60,
            label_size: 8.5,
            section_size: 8.0,
            name_size: 13.0,
            corner: 3.0,
        }
    }

    /// The accent colour of a device: its own colour, else a hue hashed from its type id.
    pub fn accent_for(&self, device: &RackDevice) -> Color {
        match device.color {
            Some([r, g, b]) => c(r, g, b),
            None => self.accent_for_type(&device.type_id),
        }
    }

    /// The hashed accent for a type id (stable across runs and platforms).
    pub fn accent_for_type(&self, type_id: &str) -> Color {
        let h = fnv1a(type_id.as_bytes());
        let hue = (h % 360) as f32;
        hsl(hue, self.accent_saturation, self.accent_lightness)
    }

    /// The lamp colour of an Audio I/O socket state.
    pub fn io_state_color(&self, state: crate::io_panel::IoSocketState) -> Color {
        use crate::io_panel::IoSocketState;
        match state {
            IoSocketState::Connected => self.io_connected,
            IoSocketState::Free => self.io_free,
            IoSocketState::Unavailable => self.io_unavailable,
        }
    }

    /// Cable colour by kind.
    pub fn cable_color(&self, kind: SocketKind) -> Color {
        match kind {
            SocketKind::Audio => self.cable_audio,
            SocketKind::Cv => self.cable_cv,
            SocketKind::Notes => self.cable_notes,
        }
    }
}

/// 64-bit FNV-1a hash (deterministic; used for accents and screw angles).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// HSL (hue in degrees, saturation and lightness 0..1) to an opaque sRGB colour.
pub fn hsl(hue: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (hue.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::new([r + m, g + m, b + m, 1.0])
}

/// Mixes two colours (`t` = 0 gives `a`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (x, y) = (a.components, b.components);
    Color::new([
        x[0] + (y[0] - x[0]) * t,
        x[1] + (y[1] - x[1]) * t,
        x[2] + (y[2] - x[2]) * t,
        x[3] + (y[3] - x[3]) * t,
    ])
}

/// WCAG relative luminance of an opaque colour.
pub fn relative_luminance(color: Color) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b, _] = color.components;
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio of two opaque colours (1 to 21).
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state colours of the Audio I/O lamps read against the window they
    /// sit in (WCAG 1.4.11, at least 3:1; these reach 4.5:1) and against each other. `Theme`
    /// has the single `dark()` theme, so there is no light theme to compare.
    #[test]
    fn the_io_state_colours_have_contrast() {
        let t = Theme::dark();
        let states = [t.io_connected, t.io_free, t.io_unavailable, t.clip];
        for (i, s) in states.iter().enumerate() {
            // The lamps sit in the dark window of their cell; elsewhere on the panel the
            // 3:1 of WCAG 1.4.11 (non-text) is the floor.
            let r = contrast_ratio(*s, t.lcd);
            assert!(r >= 4.5, "state colour {i} on the cell: {r:.2}:1");
            for bg in [t.plate, t.panel_bottom] {
                let r = contrast_ratio(*s, bg);
                assert!(r >= 3.0, "state colour {i} on a panel colour: {r:.2}:1");
            }
        }
        for i in 0..3 {
            for j in (i + 1)..3 {
                let (a, b) = (states[i].components, states[j].components);
                let dist =
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                assert!(dist >= 0.3, "states {i} and {j} are too alike: {dist:.2}");
            }
        }
    }

    #[test]
    fn contrast_ratio_matches_the_wcag_extremes() {
        let white = Color::from_rgba8(255, 255, 255, 255);
        let black = Color::from_rgba8(0, 0, 0, 255);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn accent_is_stable_and_respects_doc_colour() {
        let t = Theme::dark();
        let a = t.accent_for_type("atomik.poly-synth");
        assert_eq!(a, t.accent_for_type("atomik.poly-synth"));
        assert_ne!(a, t.accent_for_type("atomik.stereo-delay"));
        let dev = RackDevice {
            key: atomik_model::DeviceKey("d".into()),
            type_id: "x".into(),
            name: "D".into(),
            params: Default::default(),
            folded: false,
            color: Some([1, 2, 3]),
            zones: Vec::new(),
            steps: None,
            curves: None,
            drums: None,
            inner: None,
            plugin: None,
            column: 0,
        };
        assert_eq!(t.accent_for(&dev).to_rgba8().to_u8_array(), [1, 2, 3, 255]);
    }

    #[test]
    fn hsl_primaries() {
        assert_eq!(
            hsl(0.0, 1.0, 0.5).to_rgba8().to_u8_array(),
            [255, 0, 0, 255]
        );
        assert_eq!(
            hsl(120.0, 1.0, 0.5).to_rgba8().to_u8_array(),
            [0, 255, 0, 255]
        );
        assert_eq!(
            hsl(240.0, 1.0, 0.5).to_rgba8().to_u8_array(),
            [0, 0, 255, 255]
        );
    }
}
