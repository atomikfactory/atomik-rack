//! Themes as data: the colour tokens, the JSON file a theme is saved in, and the three themes
//! that ship with the program.
//!
//! A theme file is a small JSON document with a `version`, a `name`, the built-in theme it is
//! `based_on` (`light` or `dark`: what it falls back to for any token it does not set) and the
//! `colors` it sets, each one `#rrggbb` or `#rrggbbaa`. The numbers (`accent_saturation`,
//! `accent_lightness`, `corner`) and the `dark` flag are optional too. The built-in themes
//! (`themes/light.json`, `dark.json` and `synthwave.json`, compiled in) go through the same loader
//! as a theme a user makes, and they set every token.
//!
//! [`TOKENS`] is the one list of the colour tokens: their names in the file, their labels in the
//! theme editor, their group, and how to read and write them in a [`Theme`].

use crate::theme::Theme;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use vello::peniko::Color;

/// The version of the theme file this program writes and reads.
pub const THEME_VERSION: u32 = 1;

/// The longest theme file read (a real one is about 3 KiB).
pub const MAX_THEME_BYTES: usize = 64 * 1024;

/// The longest theme name, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// The built-in theme a user theme falls back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The light theme.
    Light,
    /// The dark theme.
    Dark,
}

impl Base {
    /// The word written in a theme file.
    pub fn id(self) -> &'static str {
        match self {
            Base::Light => "light",
            Base::Dark => "dark",
        }
    }

    /// The base a word of a theme file names.
    pub fn from_id(id: &str) -> Option<Base> {
        match id {
            "light" => Some(Base::Light),
            "dark" => Some(Base::Dark),
            _ => None,
        }
    }
}

/// The groups of the theme editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Window, panels, plates and displays.
    Backgrounds,
    /// Type colours.
    Text,
    /// Highlights, focus, selection and states.
    Accents,
    /// Level meters.
    Meters,
    /// The arrangement: lanes, keys and grid lines.
    Arrangement,
    /// The rack: knobs, jacks, screws and cables.
    Rack,
    /// The console and the audio lamps.
    Console,
}

impl Group {
    /// Every group, in editor order.
    pub const ALL: [Group; 7] = [
        Group::Backgrounds,
        Group::Text,
        Group::Accents,
        Group::Meters,
        Group::Arrangement,
        Group::Rack,
        Group::Console,
    ];

    /// The heading in the editor.
    pub fn label(self) -> &'static str {
        match self {
            Group::Backgrounds => "Backgrounds",
            Group::Text => "Text",
            Group::Accents => "Accents",
            Group::Meters => "Meters",
            Group::Arrangement => "Arrangement",
            Group::Rack => "Rack",
            Group::Console => "Console",
        }
    }
}

/// One colour token of a [`Theme`].
#[derive(Clone, Copy)]
pub struct Token {
    /// The name in a theme file.
    pub key: &'static str,
    /// The name in the editor.
    pub label: &'static str,
    /// The editor group.
    pub group: Group,
    /// Reads the token.
    pub get: fn(&Theme) -> Color,
    /// Writes the token.
    pub set: fn(&mut Theme, Color),
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token").field("key", &self.key).finish()
    }
}

macro_rules! tokens {
    ($(($field:ident, $label:literal, $group:ident)),* $(,)?) => {
        /// Every colour token, in file and editor order (each group together).
        pub static TOKENS: &[Token] = &[
            $(Token {
                key: stringify!($field),
                label: $label,
                group: Group::$group,
                get: |t| t.$field,
                set: |t, v| t.$field = v,
            }),*
        ];
    };
}

tokens! {
    (background, "Window background", Backgrounds),
    (rail, "Rack rail", Backgrounds),
    (rail_hole, "Rack rail hole", Backgrounds),
    (ear, "Rack ear", Backgrounds),
    (panel_top, "Panel, top", Backgrounds),
    (panel_bottom, "Panel, bottom", Backgrounds),
    (back_top, "Back panel, top", Backgrounds),
    (back_bottom, "Back panel, bottom", Backgrounds),
    (plate, "Name plate", Backgrounds),
    (lcd, "Display", Backgrounds),
    (button, "Button", Backgrounds),
    (browser_bg, "Browser", Backgrounds),
    (browser_row, "Browser row", Backgrounds),
    (tooltip_bg, "Tooltip", Backgrounds),
    (error_bg, "Error message", Backgrounds),
    (bevel_light, "Bevel light", Backgrounds),
    (bevel_dark, "Bevel shadow", Backgrounds),
    (section_line, "Section lines", Backgrounds),
    (ink, "Overlay ink", Backgrounds),
    (veil, "Veil", Backgrounds),
    (dim, "Dimmed area", Backgrounds),
    (text, "Text", Text),
    (text_muted, "Dimmed text", Text),
    (on_accent, "Text on accent", Text),
    (on_dark, "Text on dark keys", Text),
    (on_error, "Text on error message", Text),
    (highlight, "Highlight", Accents),
    (focus, "Focus ring", Accents),
    (selection, "Selection", Accents),
    (warning, "Warning", Accents),
    (mute, "Mute", Accents),
    (meter_low, "Meter, quiet", Meters),
    (meter_mid, "Meter, loud", Meters),
    (meter_high, "Meter, peak", Meters),
    (lane, "Lane", Arrangement),
    (lane_alt, "Lane, alternate", Arrangement),
    (lane_auto, "Automation lane", Arrangement),
    (grid_bar, "Grid, bar", Arrangement),
    (grid_beat, "Grid, beat", Arrangement),
    (grid_sub, "Grid, subdivision", Arrangement),
    (row_black, "Piano roll black row", Arrangement),
    (key_white, "White key", Arrangement),
    (key_black, "Black key", Arrangement),
    (screw_light, "Screw, light", Rack),
    (screw_dark, "Screw, shadow", Rack),
    (knob_top, "Knob, top", Rack),
    (knob_bottom, "Knob, bottom", Rack),
    (knob_track, "Knob track", Rack),
    (knob_pointer, "Knob pointer", Rack),
    (jack_light, "Jack, light", Rack),
    (jack_dark, "Jack, shadow", Rack),
    (jack_hole, "Jack hole", Rack),
    (led_off, "Lamp, off", Rack),
    (cable_audio, "Audio cable", Rack),
    (cable_cv, "CV cable", Rack),
    (cable_notes, "Note cable", Rack),
    (cable_insert, "Insert cable", Rack),
    (cable_send, "Send cable", Rack),
    (cable_sidechain, "Sidechain cable", Rack),
    (cable_shadow, "Cable shadow", Rack),
    (io_connected, "Socket connected", Console),
    (io_free, "Socket free", Console),
    (io_unavailable, "Socket unavailable", Console),
    (clip, "Clip lamp", Console),
}

/// The token of a file name.
pub fn token(key: &str) -> Option<&'static Token> {
    TOKENS.iter().find(|t| t.key == key)
}

/// Why a theme file was not used.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThemeError {
    /// The text is not a theme file (not JSON, or a field has the wrong kind of value).
    #[error("not a theme file: {0}")]
    Invalid(String),
    /// The file is longer than [`MAX_THEME_BYTES`].
    #[error("the file is too long to be a theme")]
    TooLong,
    /// A version this program does not know.
    #[error("theme file version {0} is not known")]
    Version(u64),
    /// A colour that is not `#rrggbb` or `#rrggbbaa`.
    #[error("{key}: {text:?} is not a colour (use #rrggbb)")]
    Colour {
        /// The token.
        key: String,
        /// What the file says.
        text: String,
    },
    /// A built-in theme that does not set this token.
    #[error("the theme does not set {0}")]
    Missing(&'static str),
}

/// A colour as `#rrggbb` (opaque) or `#rrggbbaa`.
pub fn hex_text([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// The colour of `#rrggbb` or `#rrggbbaa` (the `#` is optional, case does not matter).
pub fn parse_colour(text: &str) -> Option<[u8; 4]> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.is_ascii() || (digits.len() != 6 && digits.len() != 8) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok();
    let a = if digits.len() == 8 { byte(3)? } else { 255 };
    Some([byte(0)?, byte(1)?, byte(2)?, a])
}

/// The bytes of a colour.
pub fn rgba_of(color: Color) -> [u8; 4] {
    color.to_rgba8().to_u8_array()
}

/// The colour of bytes.
pub fn colour_of([r, g, b, a]: [u8; 4]) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// A theme as written in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeDoc {
    /// The name shown in the lists.
    pub name: String,
    /// What the tokens it does not set fall back to.
    pub based_on: Base,
    /// Light type on dark surfaces (`None`: as the base).
    pub dark: Option<bool>,
    /// Saturation of the hashed device accents (`None`: as the base).
    pub accent_saturation: Option<f32>,
    /// Lightness of the hashed device accents (`None`: as the base).
    pub accent_lightness: Option<f32>,
    /// Panel corner radius (`None`: as the base).
    pub corner: Option<f64>,
    /// The tokens it sets, by file name, as red, green, blue, alpha.
    pub colors: BTreeMap<String, [u8; 4]>,
}

impl ThemeDoc {
    /// A theme that sets nothing yet.
    pub fn new(name: &str, based_on: Base) -> Self {
        Self {
            name: clean_name(name),
            based_on,
            dark: None,
            accent_saturation: None,
            accent_lightness: None,
            corner: None,
            colors: BTreeMap::new(),
        }
    }

    /// A theme that sets every token as `theme` has it.
    pub fn from_theme(name: &str, based_on: Base, theme: &Theme) -> Self {
        let mut doc = Self::new(name, based_on);
        doc.dark = Some(theme.dark);
        doc.accent_saturation = Some(theme.accent_saturation);
        doc.accent_lightness = Some(theme.accent_lightness);
        doc.corner = Some(theme.corner);
        for t in TOKENS {
            doc.colors
                .insert(t.key.to_string(), rgba_of((t.get)(theme)));
        }
        doc
    }

    /// Reads a theme file. Tokens the program does not know are ignored (a newer file keeps
    /// working); a colour that is not one is an error.
    pub fn parse(text: &str) -> Result<Self, ThemeError> {
        if text.len() > MAX_THEME_BYTES {
            return Err(ThemeError::TooLong);
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| ThemeError::Invalid(e.to_string()))?;
        let obj = value
            .as_object()
            .ok_or_else(|| ThemeError::Invalid("the top level is not an object".into()))?;
        match obj.get("version").and_then(serde_json::Value::as_u64) {
            Some(v) if v == u64::from(THEME_VERSION) => {}
            Some(v) => return Err(ThemeError::Version(v)),
            None => return Err(ThemeError::Invalid("no version".into())),
        }
        let name = obj
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(clean_name)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| ThemeError::Invalid("no name".into()))?;
        let based_on = match obj.get("based_on") {
            None => Base::Dark,
            Some(v) => v
                .as_str()
                .and_then(Base::from_id)
                .ok_or_else(|| ThemeError::Invalid("based_on is light or dark".into()))?,
        };
        let number = |key: &str| -> Result<Option<f64>, ThemeError> {
            match obj.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .map(Some)
                    .ok_or_else(|| ThemeError::Invalid(format!("{key} is not a number"))),
            }
        };
        let dark = match obj.get("dark") {
            None => None,
            Some(v) => Some(
                v.as_bool()
                    .ok_or_else(|| ThemeError::Invalid("dark is true or false".into()))?,
            ),
        };
        let mut colors = BTreeMap::new();
        if let Some(map) = obj.get("colors") {
            let map = map
                .as_object()
                .ok_or_else(|| ThemeError::Invalid("colors is not an object".into()))?;
            for (key, v) in map {
                if token(key).is_none() {
                    continue;
                }
                let text = v.as_str().unwrap_or_default();
                let rgba = parse_colour(text).ok_or_else(|| ThemeError::Colour {
                    key: key.clone(),
                    text: v.to_string(),
                })?;
                colors.insert(key.clone(), rgba);
            }
        }
        Ok(Self {
            name,
            based_on,
            dark,
            accent_saturation: number("accent_saturation")?.map(|n| (n as f32).clamp(0.0, 1.0)),
            accent_lightness: number("accent_lightness")?.map(|n| (n as f32).clamp(0.05, 0.95)),
            corner: number("corner")?.map(|n| n.clamp(0.0, 16.0)),
            colors,
        })
    }

    /// The text of the theme file: tokens in editor order, one per line.
    pub fn to_json(&self) -> String {
        let quote = |s: &str| serde_json::Value::String(s.to_string()).to_string();
        let mut lines = vec![
            format!("  \"version\": {THEME_VERSION}"),
            format!("  \"name\": {}", quote(&self.name)),
            format!("  \"based_on\": \"{}\"", self.based_on.id()),
        ];
        if let Some(d) = self.dark {
            lines.push(format!("  \"dark\": {d}"));
        }
        if let Some(n) = self.accent_saturation {
            lines.push(format!("  \"accent_saturation\": {n}"));
        }
        if let Some(n) = self.accent_lightness {
            lines.push(format!("  \"accent_lightness\": {n}"));
        }
        if let Some(n) = self.corner {
            lines.push(format!("  \"corner\": {n}"));
        }
        let set: Vec<String> = TOKENS
            .iter()
            .filter_map(|t| {
                self.colors
                    .get(t.key)
                    .map(|c| format!("    \"{}\": \"{}\"", t.key, hex_text(*c)))
            })
            .collect();
        lines.push(format!("  \"colors\": {{\n{}\n  }}", set.join(",\n")));
        format!("{{\n{}\n}}\n", lines.join(",\n"))
    }

    /// The theme to paint with: the base, with what this file sets on top.
    pub fn resolve(&self) -> Theme {
        let mut theme = builtin(self.based_on).clone();
        if let Some(d) = self.dark {
            theme.dark = d;
        }
        if let Some(n) = self.accent_saturation {
            theme.accent_saturation = n;
        }
        if let Some(n) = self.accent_lightness {
            theme.accent_lightness = n;
        }
        if let Some(n) = self.corner {
            theme.corner = n;
        }
        for t in TOKENS {
            if let Some(c) = self.colors.get(t.key) {
                (t.set)(&mut theme, colour_of(*c));
            }
        }
        theme
    }

    /// The colour of a token as the theme has it (set here, else from the base).
    pub fn colour(&self, key: &str) -> Option<[u8; 4]> {
        let t = token(key)?;
        Some(
            self.colors
                .get(key)
                .copied()
                .unwrap_or_else(|| rgba_of((t.get)(builtin(self.based_on)))),
        )
    }

    /// Sets a token.
    pub fn set_colour(&mut self, key: &str, rgba: [u8; 4]) {
        if token(key).is_some() {
            self.colors.insert(key.to_string(), rgba);
        }
    }

    /// Takes a token back to what the base says.
    pub fn reset_colour(&mut self, key: &str) {
        self.colors.remove(key);
    }

    /// Whether the file sets the token itself.
    pub fn sets(&self, key: &str) -> bool {
        self.colors.contains_key(key)
    }

    /// A built-in file: it must set every token.
    fn complete(&self) -> Result<Theme, ThemeError> {
        for t in TOKENS {
            if !self.colors.contains_key(t.key) {
                return Err(ThemeError::Missing(t.key));
            }
        }
        let mut theme = blank();
        theme.dark = self.dark.unwrap_or(theme.dark);
        theme.accent_saturation = self.accent_saturation.unwrap_or(theme.accent_saturation);
        theme.accent_lightness = self.accent_lightness.unwrap_or(theme.accent_lightness);
        theme.corner = self.corner.unwrap_or(theme.corner);
        for t in TOKENS {
            if let Some(c) = self.colors.get(t.key) {
                (t.set)(&mut theme, colour_of(*c));
            }
        }
        Ok(theme)
    }
}

/// A name as it is kept: trimmed, one line, at most [`MAX_NAME_CHARS`] characters.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A theme with nothing set (all transparent): what a built-in file is filled into.
fn blank() -> Theme {
    let z = Color::from_rgba8(0, 0, 0, 0);
    let mut theme = Theme {
        background: z,
        rail: z,
        rail_hole: z,
        panel_top: z,
        panel_bottom: z,
        back_top: z,
        back_bottom: z,
        bevel_light: z,
        bevel_dark: z,
        ear: z,
        screw_light: z,
        screw_dark: z,
        plate: z,
        text: z,
        text_muted: z,
        section_line: z,
        knob_top: z,
        knob_bottom: z,
        knob_track: z,
        knob_pointer: z,
        jack_light: z,
        jack_dark: z,
        jack_hole: z,
        led_off: z,
        lcd: z,
        button: z,
        cable_audio: z,
        cable_cv: z,
        cable_notes: z,
        cable_insert: z,
        cable_send: z,
        cable_sidechain: z,
        cable_shadow: z,
        highlight: z,
        dim: z,
        focus: z,
        selection: z,
        tooltip_bg: z,
        error_bg: z,
        warning: z,
        io_connected: z,
        io_free: z,
        io_unavailable: z,
        clip: z,
        browser_bg: z,
        browser_row: z,
        accent_saturation: 0.62,
        accent_lightness: 0.60,
        label_size: 8.5,
        section_size: 8.0,
        name_size: 13.0,
        corner: 3.0,
        dark: true,
        ink: z,
        lane: z,
        lane_alt: z,
        lane_auto: z,
        row_black: z,
        key_white: z,
        key_black: z,
        veil: z,
        mute: z,
        on_accent: z,
        on_dark: z,
        on_error: z,
        grid_bar: z,
        grid_beat: z,
        grid_sub: z,
        meter_low: z,
        meter_mid: z,
        meter_high: z,
    };
    theme.dark = true;
    theme
}

/// The text of the built-in files, by name.
pub const BUILTIN_SOURCES: [(&str, &str); 3] = [
    ("light", include_str!("../themes/light.json")),
    ("dark", include_str!("../themes/dark.json")),
    ("synthwave", include_str!("../themes/synthwave.json")),
];

/// The names of the built-in themes, in list order.
pub const BUILTIN_IDS: [&str; 3] = ["dark", "light", "synthwave"];

/// The file of a built-in theme.
pub fn builtin_doc(id: &str) -> Option<ThemeDoc> {
    let (_, text) = BUILTIN_SOURCES.iter().find(|(n, _)| *n == id)?;
    ThemeDoc::parse(text).ok()
}

fn loaded(id: &str) -> Theme {
    builtin_doc(id)
        .and_then(|d| d.complete().ok())
        .unwrap_or_else(blank)
}

/// The built-in light or dark theme, read once from its file.
pub fn builtin(base: Base) -> &'static Theme {
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    static DARK: OnceLock<Theme> = OnceLock::new();
    match base {
        Base::Light => LIGHT.get_or_init(|| loaded("light")),
        Base::Dark => DARK.get_or_init(|| loaded("dark")),
    }
}

/// The built-in synthwave theme, read once from its file.
pub fn synthwave() -> &'static Theme {
    static SYNTH: OnceLock<Theme> = OnceLock::new();
    SYNTH.get_or_init(|| loaded("synthwave"))
}

/// The built-in theme of an id (`dark`, `light`, `synthwave`).
pub fn builtin_theme(id: &str) -> Option<Theme> {
    match id {
        "dark" => Some(builtin(Base::Dark).clone()),
        "light" => Some(builtin(Base::Light).clone()),
        "synthwave" => Some(synthwave().clone()),
        _ => None,
    }
}

/// The word shown for a built-in theme.
pub fn builtin_label(id: &str) -> &'static str {
    match id {
        "dark" => "Dark",
        "light" => "Light",
        _ => "Synthwave",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &Theme, b: &Theme) {
        for t in TOKENS {
            assert_eq!(rgba_of((t.get)(a)), rgba_of((t.get)(b)), "token {}", t.key);
        }
        assert_eq!(a, b);
    }

    /// The built-in Light and Dark files give back exactly the palettes that were in code, so
    /// nothing drawn with them changes.
    #[test]
    fn the_builtin_light_and_dark_files_are_todays_palettes() {
        same(&Theme::dark(), &Theme::legacy_dark());
        same(&Theme::light(), &Theme::legacy_light());
    }

    #[test]
    fn every_builtin_file_sets_every_token() {
        for (id, text) in BUILTIN_SOURCES {
            let doc = ThemeDoc::parse(text).unwrap_or_else(|e| panic!("{id}: {e}"));
            doc.complete().unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(doc.colors.len(), TOKENS.len(), "{id}");
        }
    }

    #[test]
    fn the_token_names_are_unique_and_cover_every_group() {
        let mut keys: Vec<_> = TOKENS.iter().map(|t| t.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TOKENS.len());
        for g in Group::ALL {
            assert!(TOKENS.iter().any(|t| t.group == g), "{g:?} is empty");
        }
        // Tokens of a group sit together, in the order of the groups.
        let order: Vec<usize> = TOKENS
            .iter()
            .map(|t| Group::ALL.iter().position(|g| *g == t.group).unwrap_or(99))
            .collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
    }

    /// Writing a theme and reading it back gives the same file and the same colours.
    #[test]
    fn a_theme_survives_a_round_trip_through_its_file() {
        let mut doc = ThemeDoc::from_theme("My \"night\" theme", Base::Light, &Theme::synthwave());
        doc.set_colour("selection", [1, 2, 3, 4]);
        let text = doc.to_json();
        let back = ThemeDoc::parse(&text).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.to_json(), text);
        assert_eq!(back.colour("selection"), Some([1, 2, 3, 4]));
        same(&back.resolve(), &doc.resolve());
    }

    /// A token a file leaves out comes from the theme it is based on.
    #[test]
    fn a_missing_token_falls_back_to_the_base() {
        let text = r##"{"version": 1, "name": "Mine", "based_on": "light",
            "colors": {"text": "#ff0000", "nothing_here": "#00ff00"}}"##;
        let doc = ThemeDoc::parse(text).unwrap();
        let theme = doc.resolve();
        assert_eq!(rgba_of(theme.text), [255, 0, 0, 255]);
        assert_eq!(theme.background, Theme::light().background);
        assert!(!theme.dark);
        let dark = ThemeDoc::parse(r#"{"version": 1, "name": "Plain"}"#).unwrap();
        assert_eq!(dark.based_on, Base::Dark);
        same(&dark.resolve(), &Theme::dark());
        let mut doc = doc;
        doc.reset_colour("text");
        assert_eq!(doc.resolve().text, Theme::light().text);
    }

    #[test]
    fn a_bad_file_is_refused_with_its_reason() {
        let err = |text: &str| ThemeDoc::parse(text).unwrap_err();
        assert!(matches!(err("not json"), ThemeError::Invalid(_)));
        assert!(matches!(err("[]"), ThemeError::Invalid(_)));
        assert!(matches!(err(r#"{"name": "x"}"#), ThemeError::Invalid(_)));
        assert_eq!(
            err(r#"{"version": 2, "name": "x"}"#),
            ThemeError::Version(2)
        );
        assert!(matches!(
            err(r#"{"version": 1, "name": "  "}"#),
            ThemeError::Invalid(_)
        ));
        assert!(matches!(
            err(r#"{"version": 1, "name": "x", "based_on": "blue"}"#),
            ThemeError::Invalid(_)
        ));
        assert!(matches!(
            err(r#"{"version": 1, "name": "x", "colors": {"text": "red"}}"#),
            ThemeError::Colour { .. }
        ));
        let long = format!(
            r#"{{"version": 1, "name": "{}"}}"#,
            "x".repeat(MAX_THEME_BYTES)
        );
        assert_eq!(err(&long), ThemeError::TooLong);
    }

    #[test]
    fn colours_are_written_and_read_as_hex() {
        assert_eq!(parse_colour("#FF2e97"), Some([255, 46, 151, 255]));
        assert_eq!(parse_colour("ff2e9780"), Some([255, 46, 151, 128]));
        assert_eq!(parse_colour("#fff"), None);
        assert_eq!(parse_colour("#gg0000"), None);
        assert_eq!(parse_colour("#\u{e9}\u{e9}\u{e9}"), None);
        assert_eq!(hex_text([255, 46, 151, 255]), "#ff2e97");
        assert_eq!(hex_text([255, 46, 151, 128]), "#ff2e9780");
    }

    #[test]
    fn names_are_kept_short_and_on_one_line() {
        assert_eq!(clean_name("  a\nb  "), "ab");
        assert_eq!(clean_name(&"x".repeat(100)).chars().count(), MAX_NAME_CHARS);
    }

    /// The Synthwave file is the palette of the brief and the built-ins agree on what is not a
    /// colour.
    #[test]
    fn synthwave_is_a_neon_night() {
        let s = Theme::synthwave();
        assert!(s.dark);
        assert_eq!(rgba_of(s.text), [0xf3, 0xe9, 0xff, 255]);
        assert_eq!(rgba_of(s.text_muted), [0xa9, 0x9b, 0xc9, 255]);
        assert_eq!(rgba_of(s.panel_bottom), [0x22, 0x15, 0x3f, 255]);
        assert_eq!(rgba_of(s.cable_audio), [0xff, 0x2e, 0x97, 255]);
        assert_eq!(rgba_of(s.cable_cv), [0x2d, 0xe2, 0xe6, 255]);
        assert_eq!(rgba_of(s.meter_low), [0x2d, 0xe2, 0xe6, 255]);
        assert_eq!(rgba_of(s.meter_mid), [0xff, 0x2e, 0x97, 255]);
        assert_eq!(rgba_of(s.meter_high), [0xff, 0x8b, 0x3d, 255]);
        assert!(s.grid_bar.components[3] > s.grid_beat.components[3]);
        assert!(s.grid_beat.components[3] > s.grid_sub.components[3]);
        assert_eq!(
            (s.label_size, s.section_size, s.name_size),
            (
                Theme::dark().label_size,
                Theme::dark().section_size,
                Theme::dark().name_size
            )
        );
    }

    /// Body type reads on every surface of every built-in theme, to the AA ratio of WCAG (the
    /// primary type reaches AAA).
    #[test]
    fn body_text_has_contrast_on_every_surface_of_every_builtin() {
        use crate::theme::contrast_ratio;
        for id in BUILTIN_IDS {
            let t = builtin_theme(id).unwrap();
            let surfaces = [
                ("background", t.background),
                ("panel top", t.panel_top),
                ("panel bottom", t.panel_bottom),
                ("plate", t.plate),
                ("display", t.lcd),
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
                assert!(r >= 4.5, "{id}: text on {what}: {r:.2}:1");
                let r = contrast_ratio(t.text_muted, bg);
                if id != "dark" {
                    assert!(r >= 4.5, "{id}: dimmed text on {what}: {r:.2}:1");
                }
            }
        }
    }
}
