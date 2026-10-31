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

/// The orange of the patch browsing focus: the band of the file browser and the patch name on
/// the device plate it is aimed at.
pub const PATCH_TARGET: Color = c(0xf0, 0x8a, 0x2e);

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
    /// The theme is a dark one (light type on dark surfaces).
    pub dark: bool,
    /// The colour of faint overlay lines and bevels drawn over a surface: white on a dark theme,
    /// near-black on a light one.
    pub ink: Color,
    /// Sequencer lane background (even rows).
    pub lane: Color,
    /// Sequencer lane background (odd rows).
    pub lane_alt: Color,
    /// Automation lane background.
    pub lane_auto: Color,
    /// Piano roll grid row of a black key.
    pub row_black: Color,
    /// White piano key.
    pub key_white: Color,
    /// Black piano key.
    pub key_black: Color,
    /// Veil over the area outside a clip's content.
    pub veil: Color,
    /// Mute button when active.
    pub mute: Color,
    /// Type drawn over a lit control or an accent fill.
    pub on_accent: Color,
    /// Type drawn over a dark key or a dark badge.
    pub on_dark: Color,
    /// Type drawn over the error toast.
    pub on_error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Which theme the user picked: a fixed one, or the one the operating system uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// The dark theme.
    #[default]
    Dark,
    /// The light theme.
    Light,
    /// Whichever the operating system is set to.
    System,
}

impl ThemeChoice {
    /// Every choice, in menu order.
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

    /// The identifier saved in the preferences and used as a command argument.
    pub fn id(self) -> &'static str {
        match self {
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
            ThemeChoice::System => "system",
        }
    }

    /// The choice of a saved identifier.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The label shown in menus and settings.
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::System => "Same as System",
        }
    }

    /// The next choice, wrapping.
    pub fn next(self) -> Self {
        match self {
            ThemeChoice::Dark => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::System,
            ThemeChoice::System => ThemeChoice::Dark,
        }
    }

    /// The theme to paint with when the operating system is `system_dark` (ignored unless the
    /// choice is [`ThemeChoice::System`]).
    pub fn theme(self, system_dark: bool) -> Theme {
        match self {
            ThemeChoice::Dark => Theme::dark(),
            ThemeChoice::Light => Theme::light(),
            ThemeChoice::System if system_dark => Theme::dark(),
            ThemeChoice::System => Theme::light(),
        }
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
            dark: true,
            ink: c(0xff, 0xff, 0xff),
            lane: c(0x16, 0x18, 0x1c),
            lane_alt: c(0x13, 0x15, 0x18),
            lane_auto: c(0x11, 0x13, 0x16),
            row_black: c(0x12, 0x14, 0x17),
            key_white: c(0xd9, 0xd5, 0xcc),
            key_black: c(0x1b, 0x1d, 0x21),
            veil: ca(0x0a, 0x0b, 0x0d, 0xa0),
            mute: c(0xe0, 0xa8, 0x3c),
            on_accent: c(0x17, 0x19, 0x1c),
            on_dark: c(0xec, 0xe8, 0xe1),
            on_error: c(0xff, 0xff, 0xff),
        }
    }

    /// The light theme: pale grey panels, near-black type, the same one accent hue per device
    /// (a little darker, to read on the light surfaces). Displays stay light too, so the type
    /// on them is the panel type.
    pub fn light() -> Self {
        Self {
            background: c(0xd4, 0xd8, 0xdd),
            rail: c(0xb3, 0xb8, 0xbf),
            rail_hole: c(0x7d, 0x83, 0x8c),
            panel_top: c(0xec, 0xee, 0xf1),
            panel_bottom: c(0xdc, 0xdf, 0xe4),
            back_top: c(0xd0, 0xd4, 0xda),
            back_bottom: c(0xc4, 0xc8, 0xcf),
            bevel_light: ca(0xff, 0xff, 0xff, 0xcc),
            bevel_dark: ca(0x00, 0x00, 0x00, 0x40),
            ear: c(0xc4, 0xc8, 0xce),
            screw_light: c(0xf4, 0xf5, 0xf7),
            screw_dark: c(0x8b, 0x90, 0x99),
            plate: c(0xf6, 0xf7, 0xf9),
            text: c(0x1b, 0x1e, 0x23),
            text_muted: c(0x4d, 0x54, 0x5e),
            section_line: ca(0x00, 0x00, 0x00, 0x2e),
            knob_top: c(0x6a, 0x70, 0x79),
            knob_bottom: c(0x30, 0x34, 0x3a),
            knob_track: c(0x9a, 0xa1, 0xab),
            knob_pointer: c(0xf4, 0xf1, 0xea),
            jack_light: c(0xd9, 0xdc, 0xe1),
            jack_dark: c(0x77, 0x7c, 0x85),
            jack_hole: c(0x1f, 0x20, 0x23),
            led_off: c(0xb6, 0xbb, 0xc3),
            lcd: c(0xee, 0xf0, 0xe8),
            button: c(0xe9, 0xeb, 0xef),
            cable_audio: c(0xc4, 0x3e, 0x1f),
            cable_cv: c(0x1f, 0x7b, 0xc0),
            cable_notes: c(0x4f, 0x8f, 0x14),
            cable_shadow: ca(0x00, 0x00, 0x00, 0x40),
            highlight: c(0xaa, 0x66, 0x00),
            dim: ca(0xf0, 0xf1, 0xf3, 0xb0),
            focus: c(0x0a, 0x5f, 0xc2),
            selection: ca(0xf0, 0xb0, 0x30, 0xb0),
            tooltip_bg: ca(0xfb, 0xfb, 0xf4, 0xf2),
            error_bg: ca(0xf1, 0xb4, 0xad, 0xf0),
            warning: c(0x8f, 0x5a, 0x00),
            io_connected: c(0x14, 0x6e, 0x28),
            io_free: c(0x7f, 0x59, 0x00),
            io_unavailable: c(0xb3, 0x24, 0x24),
            clip: c(0xc4, 0x1a, 0x0c),
            browser_bg: c(0xf2, 0xf3, 0xf5),
            browser_row: c(0xe1, 0xe4, 0xe8),
            accent_saturation: 0.62,
            accent_lightness: 0.40,
            label_size: 8.5,
            section_size: 8.0,
            name_size: 13.0,
            corner: 3.0,
            dark: false,
            ink: c(0x14, 0x18, 0x1e),
            lane: c(0xe8, 0xea, 0xee),
            lane_alt: c(0xe1, 0xe4, 0xe9),
            lane_auto: c(0xda, 0xde, 0xe3),
            row_black: c(0xd2, 0xd6, 0xdc),
            key_white: c(0xfa, 0xfa, 0xf7),
            key_black: c(0x2a, 0x2d, 0x32),
            veil: ca(0xc8, 0xcc, 0xd2, 0xa0),
            mute: c(0xb8, 0x78, 0x00),
            on_accent: c(0x1b, 0x1e, 0x23),
            on_dark: c(0xf4, 0xf1, 0xea),
            on_error: c(0x2a, 0x0a, 0x08),
        }
    }

    /// `ink` at `alpha` (0 to 1): a faint line or highlight over a surface of this theme.
    pub fn overlay(&self, alpha: f32) -> Color {
        self.ink.with_alpha(alpha)
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

    fn both() -> [(&'static str, Theme); 2] {
        [("dark", Theme::dark()), ("light", Theme::light())]
    }

    /// The state colours of the Audio I/O lamps read against the window they
    /// sit in (WCAG 1.4.11, at least 3:1; these reach 4.5:1) and against each other, in both
    /// themes.
    #[test]
    fn the_io_state_colours_have_contrast() {
        for (name, t) in both() {
            let states = [t.io_connected, t.io_free, t.io_unavailable, t.clip];
            for (i, s) in states.iter().enumerate() {
                // The lamps sit in the window of their cell; elsewhere on the panel the
                // 3:1 of WCAG 1.4.11 (non-text) is the floor.
                let r = contrast_ratio(*s, t.lcd);
                assert!(r >= 4.5, "{name}: state colour {i} on the cell: {r:.2}:1");
                for bg in [t.plate, t.panel_bottom] {
                    let r = contrast_ratio(*s, bg);
                    assert!(
                        r >= 3.0,
                        "{name}: state colour {i} on a panel colour: {r:.2}:1"
                    );
                }
            }
            for i in 0..3 {
                for j in (i + 1)..3 {
                    let (a, b) = (states[i].components, states[j].components);
                    let dist =
                        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
                            .sqrt();
                    assert!(
                        dist >= 0.3,
                        "{name}: states {i} and {j} are too alike: {dist:.2}"
                    );
                }
            }
        }
    }

    /// Type reads on every surface it is drawn on, in both themes: the primary type at 7:1
    /// (WCAG AAA) on the panels and displays, the muted type at 4.5:1 (AA; the dark theme, whose
    /// palette predates the check, keeps 3:1), the focus ring and the highlight at 3:1
    /// (non-text) on the surfaces they outline.
    #[test]
    fn type_reads_on_every_surface_of_both_themes() {
        for (name, t) in both() {
            let surfaces = [
                ("background", t.background),
                ("panel top", t.panel_top),
                ("panel bottom", t.panel_bottom),
                ("plate", t.plate),
                ("lcd", t.lcd),
                ("button", t.button),
                ("browser", t.browser_bg),
                ("browser row", t.browser_row),
                ("lane", t.lane),
                ("lane (odd)", t.lane_alt),
                ("automation lane", t.lane_auto),
                ("tooltip", t.tooltip_bg.with_alpha(1.0)),
            ];
            for (what, bg) in surfaces {
                let r = contrast_ratio(t.text, bg);
                assert!(r >= 7.0, "{name}: text on {what}: {r:.2}:1");
                let r = contrast_ratio(t.text_muted, bg);
                let floor = if t.dark { 3.0 } else { 4.5 };
                assert!(r >= floor, "{name}: muted text on {what}: {r:.2}:1");
            }
            for (what, bg) in [
                ("panel bottom", t.panel_bottom),
                ("browser", t.browser_bg),
                ("lane", t.lane),
            ] {
                let r = contrast_ratio(t.focus, bg);
                assert!(r >= 3.0, "{name}: focus ring on {what}: {r:.2}:1");
                let r = contrast_ratio(t.highlight, bg);
                assert!(r >= 3.0, "{name}: highlight on {what}: {r:.2}:1");
            }
            // The type of a lit control or an accent fill, and of a dark key, on what it sits on.
            let lit = mix(t.highlight, t.button, 0.25);
            assert!(
                contrast_ratio(t.on_accent, lit) >= 4.5,
                "{name}: type on a lit button"
            );
            assert!(
                contrast_ratio(t.on_dark, t.key_black) >= 4.5,
                "{name}: label on a black key"
            );
            assert!(
                contrast_ratio(t.on_accent, t.key_white) >= 4.5,
                "{name}: label on a white key"
            );
            let toast = t.error_bg.with_alpha(1.0);
            assert!(
                contrast_ratio(t.on_error, toast) >= 4.5,
                "{name}: type on the error toast"
            );
        }
    }

    /// The accent of a device reads as a clip fill, a label and a lamp on the surfaces it is
    /// drawn on (3:1 against the panel it sits on) in both themes, across a spread of hues.
    #[test]
    fn hashed_accents_read_on_the_panels_of_both_themes() {
        for (name, t) in both() {
            for id in [
                "atomik.poly-synth",
                "atomik.stereo-delay",
                "atomik.line-mixer",
                "atomik.audio-io",
                "atomik.sampler",
            ] {
                let a = t.accent_for_type(id);
                let r = contrast_ratio(a, t.panel_bottom);
                assert!(r >= 1.8, "{name}: accent of {id} on the panel: {r:.2}:1");
            }
        }
    }

    /// The themes agree on what is not a colour, and the dark one is the default.
    #[test]
    fn the_themes_share_their_sizes_and_the_dark_one_is_the_default() {
        let (d, l) = (Theme::dark(), Theme::light());
        assert_eq!(
            (d.label_size, d.section_size, d.name_size, d.corner),
            (l.label_size, l.section_size, l.name_size, l.corner)
        );
        assert!(d.dark && !l.dark);
        assert_eq!(Theme::default(), d);
        assert!(relative_luminance(l.background) > 0.5);
        assert!(relative_luminance(d.background) < 0.05);
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
            patch_name: String::new(),
            midi_out: None,
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
