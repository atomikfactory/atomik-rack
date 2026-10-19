//! Chords: modifiers plus one key, parsed from and printed to the data-file grammar.

use crate::event::{Key, Modifiers, NumpadKey};
use std::fmt;

/// The operating system a keymap is resolved for. A parameter everywhere (tests pass it
/// explicitly); only [`Os::host`] reads `cfg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Os {
    /// Windows: `Cmd` is Control.
    Windows,
    /// macOS: `Cmd` is Command.
    Mac,
    /// Linux and other Unix: `Cmd` is Control.
    Linux,
}

impl Os {
    /// Every OS, in file order.
    pub const ALL: [Os; 3] = [Os::Windows, Os::Mac, Os::Linux];

    /// The OS this build runs on (the single `cfg` in the keymap code).
    pub fn host() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    /// The key of this OS in the data file's `os` overrides.
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "win",
            Os::Mac => "mac",
            Os::Linux => "linux",
        }
    }
}

/// Logical modifiers of a chord: `cmd` is the primary modifier (Ctrl on Windows and Linux,
/// Command on macOS); the others are literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Mods {
    /// The primary modifier.
    pub cmd: bool,
    /// Literal Control.
    pub ctrl: bool,
    /// Alt / Option.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Literal Meta (Super / Windows key; Command on macOS is `cmd`).
    pub meta: bool,
}

/// The key of a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChordKey {
    /// A character by what it types; letters are lower case.
    Char(char),
    /// Escape.
    Escape,
    /// Return / Enter.
    Return,
    /// Space.
    Space,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Tab.
    Tab,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Insert.
    Insert,
    /// A function key `1..=24`.
    F(u8),
    /// A numpad key.
    Numpad(NumpadKey),
    /// A main-row digit by physical position (`Digit1`).
    Digit(u8),
}

/// A chord as authored: logical modifiers plus a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Chord {
    /// The modifiers.
    pub mods: Mods,
    /// The key.
    pub key: ChordKey,
}

/// A chord resolved for one OS, in physical modifiers: what an event is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysChord {
    /// Control held.
    pub ctrl: bool,
    /// Alt / Option held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
    /// Meta (Super / Command) held.
    pub meta: bool,
    /// The key.
    pub key: ChordKey,
}

/// Why a chord string did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    /// Nothing to parse.
    #[error("empty chord")]
    Empty,
    /// A modifier name that is not Cmd, Ctrl, Alt, Shift or Meta.
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    /// The same modifier twice.
    #[error("modifier `{0}` repeated")]
    RepeatedModifier(String),
    /// A key token that is not a character or a named key.
    #[error("unknown key `{0}`")]
    UnknownKey(String),
}

const NAMED: [(&str, ChordKey); 15] = [
    ("Escape", ChordKey::Escape),
    ("Return", ChordKey::Return),
    ("Space", ChordKey::Space),
    ("Backspace", ChordKey::Backspace),
    ("Delete", ChordKey::Delete),
    ("Tab", ChordKey::Tab),
    ("Up", ChordKey::Up),
    ("Down", ChordKey::Down),
    ("Left", ChordKey::Left),
    ("Right", ChordKey::Right),
    ("PageUp", ChordKey::PageUp),
    ("PageDown", ChordKey::PageDown),
    ("Home", ChordKey::Home),
    ("End", ChordKey::End),
    ("Insert", ChordKey::Insert),
];

const NUMPAD_NAMED: [(&str, NumpadKey); 6] = [
    ("NumpadEnter", NumpadKey::Enter),
    ("NumpadAdd", NumpadKey::Add),
    ("NumpadSub", NumpadKey::Subtract),
    ("NumpadMul", NumpadKey::Multiply),
    ("NumpadDiv", NumpadKey::Divide),
    ("NumpadDecimal", NumpadKey::Decimal),
];

impl ChordKey {
    fn parse(token: &str) -> Result<Self, ChordError> {
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Ok(ChordKey::Char(lower(c)));
        }
        if let Some((_, k)) = NAMED.iter().find(|(n, _)| *n == token) {
            return Ok(*k);
        }
        if token == "Esc" {
            return Ok(ChordKey::Escape);
        }
        if token == "Enter" {
            return Ok(ChordKey::Return);
        }
        if let Some((_, k)) = NUMPAD_NAMED.iter().find(|(n, _)| *n == token) {
            return Ok(ChordKey::Numpad(*k));
        }
        let digit = |rest: &str| match rest.as_bytes() {
            [d @ b'0'..=b'9'] => Some(d - b'0'),
            _ => None,
        };
        if let Some(d) = token.strip_prefix("Numpad").and_then(digit) {
            return Ok(ChordKey::Numpad(NumpadKey::Digit(d)));
        }
        if let Some(d) = token.strip_prefix("Digit").and_then(digit) {
            return Ok(ChordKey::Digit(d));
        }
        if let Some(rest) = token.strip_prefix('F')
            && !rest.starts_with('0')
            && let Ok(n) = rest.parse::<u8>()
            && (1..=24).contains(&n)
        {
            return Ok(ChordKey::F(n));
        }
        Err(ChordError::UnknownKey(token.to_string()))
    }

    /// The canonical token (`K`, `Return`, `Numpad0`, `F5`, `Digit1`, `[`).
    pub fn token(self) -> String {
        match self {
            ChordKey::Char(c) => c.to_uppercase().collect(),
            ChordKey::F(n) => format!("F{n}"),
            ChordKey::Digit(d) => format!("Digit{d}"),
            ChordKey::Numpad(NumpadKey::Digit(d)) => format!("Numpad{d}"),
            ChordKey::Numpad(k) => NUMPAD_NAMED
                .iter()
                .find(|(_, n)| *n == k)
                .map_or_else(String::new, |(n, _)| (*n).to_string()),
            other => NAMED
                .iter()
                .find(|(_, k)| *k == other)
                .map_or_else(String::new, |(n, _)| (*n).to_string()),
        }
    }

    /// The text a menu shows for the key (`Enter` for Return, `Esc` for Escape).
    pub fn menu_text(self) -> String {
        match self {
            ChordKey::Return => "Enter".to_string(),
            ChordKey::Escape => "Esc".to_string(),
            other => other.token(),
        }
    }
}

/// The lower-case form of a character when it has a single one (letters), else itself.
fn lower(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// The chord key a key event stands for (letters lower-cased); `None` for [`Key::Other`].
fn chord_key_of(key: &Key) -> Option<ChordKey> {
    Some(match key {
        Key::Char(c) => ChordKey::Char(lower(*c)),
        Key::Escape => ChordKey::Escape,
        Key::Enter => ChordKey::Return,
        Key::Space => ChordKey::Space,
        Key::Backspace => ChordKey::Backspace,
        Key::Delete => ChordKey::Delete,
        Key::Tab => ChordKey::Tab,
        Key::Up => ChordKey::Up,
        Key::Down => ChordKey::Down,
        Key::Left => ChordKey::Left,
        Key::Right => ChordKey::Right,
        Key::PageUp => ChordKey::PageUp,
        Key::PageDown => ChordKey::PageDown,
        Key::Home => ChordKey::Home,
        Key::End => ChordKey::End,
        Key::Insert => ChordKey::Insert,
        Key::F(n) => ChordKey::F(*n),
        Key::Numpad(k) => ChordKey::Numpad(*k),
        Key::Digit(d) => ChordKey::Digit(*d),
        Key::Other => return None,
    })
}

impl Mods {
    /// No modifier.
    pub fn is_empty(&self) -> bool {
        *self == Mods::default()
    }

    /// Parses modifiers alone (`Alt`, `Cmd+Alt`, `Ctrl+Shift`), as the `roles` of the user
    /// keymap write them. At least one modifier is required.
    pub fn parse(text: &str) -> Result<Mods, ChordError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ChordError::Empty);
        }
        let mut mods = Mods::default();
        for name in text.split('+') {
            let slot = match name.trim().to_ascii_lowercase().as_str() {
                "cmd" => &mut mods.cmd,
                "ctrl" => &mut mods.ctrl,
                "alt" | "opt" => &mut mods.alt,
                "shift" => &mut mods.shift,
                "meta" => &mut mods.meta,
                other => return Err(ChordError::UnknownModifier(other.to_string())),
            };
            if *slot {
                return Err(ChordError::RepeatedModifier(name.trim().to_string()));
            }
            *slot = true;
        }
        Ok(mods)
    }

    /// The text Settings shows on `os` (`Ctrl+Alt`, `Cmd+Opt`; empty for no modifier).
    pub fn menu_text(&self, os: Os) -> String {
        let probe = Chord {
            mods: *self,
            key: ChordKey::Space,
        };
        probe
            .menu_text(os)
            .strip_suffix("Space")
            .unwrap_or_default()
            .trim_end_matches('+')
            .to_string()
    }
}

impl fmt::Display for Mods {
    /// The canonical form, in the order Cmd, Ctrl, Alt, Shift, Meta, joined by `+`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = [
            (self.cmd, "Cmd"),
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.meta, "Meta"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| n)
        .collect();
        f.write_str(&names.join("+"))
    }
}

impl Chord {
    /// Parses `Cmd+Shift+S`, `Alt+X`, `Numpad0`, `F8`, `Cmd++`, `Insert`.
    ///
    /// Modifiers are `Cmd`, `Ctrl`, `Alt` (also `Opt`), `Shift`, `Meta`, in any order; the key
    /// is the last token. A `+` key is written `Cmd++` (or `+`).
    pub fn parse(text: &str) -> Result<Chord, ChordError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ChordError::Empty);
        }
        let (mod_part, key_token) = if text == "+" {
            ("", "+")
        } else if let Some(head) = text.strip_suffix("++") {
            (head, "+")
        } else if let Some((head, tail)) = text.rsplit_once('+') {
            (head, tail)
        } else {
            ("", text)
        };
        let mut mods = Mods::default();
        for name in mod_part.split('+').filter(|m| !m.is_empty()) {
            let slot = match name.to_ascii_lowercase().as_str() {
                "cmd" => &mut mods.cmd,
                "ctrl" => &mut mods.ctrl,
                "alt" | "opt" => &mut mods.alt,
                "shift" => &mut mods.shift,
                "meta" => &mut mods.meta,
                other => return Err(ChordError::UnknownModifier(other.to_string())),
            };
            if *slot {
                return Err(ChordError::RepeatedModifier(name.to_string()));
            }
            *slot = true;
        }
        if key_token.is_empty() {
            return Err(ChordError::Empty);
        }
        Ok(Chord {
            mods,
            key: ChordKey::parse(key_token)?,
        })
    }

    /// The chord a key press stands for on `os`, as Settings records it when the user presses
    /// a new binding: the primary modifier becomes `Cmd` (Control on Windows and Linux,
    /// Command on macOS), the others stay literal. `None` for keys that are no chord
    /// ([`Key::Other`], which also covers the modifier keys themselves).
    pub fn from_event(key: &Key, modifiers: Modifiers, os: Os) -> Option<Chord> {
        let key = chord_key_of(key)?;
        let mac = os == Os::Mac;
        Some(Chord {
            mods: Mods {
                cmd: if mac { modifiers.meta } else { modifiers.ctrl },
                ctrl: mac && modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                meta: !mac && modifiers.meta,
            },
            key,
        })
    }

    /// The chord resolved for `os`: `Cmd` becomes Control (Windows, Linux) or Meta (macOS).
    pub fn physical(&self, os: Os) -> PhysChord {
        let primary_is_meta = os == Os::Mac;
        PhysChord {
            ctrl: self.mods.ctrl || (self.mods.cmd && !primary_is_meta),
            alt: self.mods.alt,
            shift: self.mods.shift,
            meta: self.mods.meta || (self.mods.cmd && primary_is_meta),
            key: self.key,
        }
    }

    /// The text a menu shows on `os`: `Ctrl+Shift+S` on Windows and Linux, `Cmd+Shift+S` and
    /// `Opt` for Alt on macOS (plain text, no glyphs).
    pub fn menu_text(&self, os: Os) -> String {
        let mac = os == Os::Mac;
        let mut out = String::new();
        let mut push = |s: &str| {
            out.push_str(s);
            out.push('+');
        };
        if self.mods.cmd {
            push(if mac { "Cmd" } else { "Ctrl" });
        }
        if self.mods.ctrl {
            push("Ctrl");
        }
        if self.mods.alt {
            push(if mac { "Opt" } else { "Alt" });
        }
        if self.mods.shift {
            push("Shift");
        }
        if self.mods.meta {
            push(match os {
                Os::Windows => "Win",
                Os::Mac => "Cmd",
                Os::Linux => "Super",
            });
        }
        out.push_str(&self.key.menu_text());
        out
    }
}

impl fmt::Display for Chord {
    /// The canonical form: modifiers in the order Cmd, Ctrl, Alt, Shift, Meta, then the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.mods.cmd, "Cmd"),
            (self.mods.ctrl, "Ctrl"),
            (self.mods.alt, "Alt"),
            (self.mods.shift, "Shift"),
            (self.mods.meta, "Meta"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key.token())
    }
}

impl PhysChord {
    /// The chord an event matches: strict modifiers (Ctrl and Meta are never interchanged;
    /// Command on macOS and Super elsewhere both arrive as `meta`), letters lower-cased.
    /// `None` for [`Key::Other`].
    pub fn from_event(key: &Key, modifiers: Modifiers) -> Option<PhysChord> {
        let key = chord_key_of(key)?;
        Some(PhysChord {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            meta: modifiers.meta,
            key,
        })
    }

    /// Whether the chord carries the primary modifier of `os` (Ctrl, or Meta on macOS), or is
    /// a function key or Esc: the chords that still run while a text field has focus.
    pub fn runs_in_text(&self, os: Os) -> bool {
        let primary = if os == Os::Mac { self.meta } else { self.ctrl };
        primary || matches!(self.key, ChordKey::F(_) | ChordKey::Escape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Chord {
        Chord::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn parse_and_print_round_trip() {
        for s in [
            "Cmd+K",
            "Cmd+Shift+S",
            "Alt+X",
            "Numpad0",
            "NumpadDecimal",
            "NumpadEnter",
            "NumpadAdd",
            "F8",
            "F24",
            "Insert",
            "Return",
            "Space",
            "Shift+Return",
            "Cmd+Alt+Digit3",
            "Cmd++",
            "Cmd+=",
            "Cmd+,",
            "Cmd+-",
            "[",
            "Cmd+Ctrl+Alt+Shift+Meta+Up",
            "Escape",
            "Tab",
            "PageDown",
            "L",
            "Cmd+0",
        ] {
            assert_eq!(p(s).to_string(), s, "{s}");
        }
    }

    #[test]
    fn parsing_is_lenient_in_order_and_case_of_letters_and_prints_canonically() {
        assert_eq!(p("Shift+Cmd+s").to_string(), "Cmd+Shift+S");
        assert_eq!(p("Alt+Cmd+x").to_string(), "Cmd+Alt+X");
        assert_eq!(p("Opt+X"), p("Alt+X"));
        assert_eq!(p("Esc"), p("Escape"));
        assert_eq!(p("Enter"), p("Return"));
        assert_eq!(p("+").to_string(), "+");
    }

    #[test]
    fn bad_chords_are_errors() {
        assert_eq!(Chord::parse(""), Err(ChordError::Empty));
        assert_eq!(Chord::parse("Cmd+"), Err(ChordError::Empty));
        assert!(matches!(
            Chord::parse("Hyper+K"),
            Err(ChordError::UnknownModifier(_))
        ));
        assert!(matches!(
            Chord::parse("Cmd+Cmd+K"),
            Err(ChordError::RepeatedModifier(_))
        ));
        for bad in [
            "Cmd+Foo", "F0", "F25", "F05", "Numpad10", "Digit", "NumpadX",
        ] {
            assert!(
                matches!(Chord::parse(bad), Err(ChordError::UnknownKey(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn cmd_is_control_on_windows_and_linux_and_command_on_macos() {
        let c = p("Cmd+Shift+S");
        for os in [Os::Windows, Os::Linux] {
            let ph = c.physical(os);
            assert!(ph.ctrl && !ph.meta && ph.shift && !ph.alt, "{os:?}");
        }
        let mac = c.physical(Os::Mac);
        assert!(mac.meta && !mac.ctrl && mac.shift);
        // A literal Ctrl stays Control on macOS; a literal Meta stays Meta on Windows.
        assert!(p("Ctrl+Space").physical(Os::Mac).ctrl);
        assert!(p("Meta+Space").physical(Os::Windows).meta);
    }

    #[test]
    fn events_match_strictly_ctrl_is_not_cmd_and_super_is_not_cmd() {
        let key = Key::Char('S');
        let ctrl = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        let meta = Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        };
        let chord = p("Cmd+Shift+S");
        let ev = |m| PhysChord::from_event(&key, m).unwrap();
        assert_eq!(ev(ctrl), chord.physical(Os::Windows));
        assert_ne!(ev(meta), chord.physical(Os::Windows), "Super is not Cmd");
        assert_eq!(ev(meta), chord.physical(Os::Mac));
        assert_ne!(ev(ctrl), chord.physical(Os::Mac), "Control is not Command");
    }

    #[test]
    fn numpad_and_main_row_keys_are_distinct() {
        let n = PhysChord::from_event(&Key::Numpad(NumpadKey::Digit(1)), Modifiers::NONE);
        let m = PhysChord::from_event(&Key::Char('1'), Modifiers::NONE);
        let d = PhysChord::from_event(&Key::Digit(1), Modifiers::NONE);
        assert_ne!(n, m);
        assert_ne!(m, d);
        assert_eq!(n, Some(p("Numpad1").physical(Os::Windows)));
        assert_eq!(m, Some(p("1").physical(Os::Windows)));
        assert_eq!(d, Some(p("Digit1").physical(Os::Windows)));
        assert_eq!(PhysChord::from_event(&Key::Other, Modifiers::NONE), None);
    }

    #[test]
    fn letters_are_case_folded_in_events() {
        let a = PhysChord::from_event(&Key::Char('Z'), Modifiers::SHIFT);
        let b = PhysChord::from_event(&Key::Char('z'), Modifiers::SHIFT);
        assert_eq!(a, b);
    }

    #[test]
    fn menu_text_per_os_is_plain_text() {
        let c = p("Cmd+Alt+Shift+I");
        assert_eq!(c.menu_text(Os::Windows), "Ctrl+Alt+Shift+I");
        assert_eq!(c.menu_text(Os::Linux), "Ctrl+Alt+Shift+I");
        assert_eq!(c.menu_text(Os::Mac), "Cmd+Opt+Shift+I");
        assert_eq!(p("Return").menu_text(Os::Windows), "Enter");
        assert_eq!(p("Cmd+Space").menu_text(Os::Mac), "Cmd+Space");
        assert_eq!(p("Ctrl+Space").menu_text(Os::Mac), "Ctrl+Space");
    }

    #[test]
    fn text_fields_let_primary_chords_and_function_keys_through() {
        let run = |c: &str, os| p(c).physical(os).runs_in_text(os);
        for os in Os::ALL {
            assert!(run("Cmd+S", os), "{os:?}");
            assert!(run("F5", os));
            assert!(run("Escape", os));
            assert!(!run("S", os));
            assert!(!run("Space", os));
            assert!(!run("Return", os));
            assert!(!run("Up", os));
            assert!(!run("Shift+Z", os));
        }
        assert!(
            !run("Ctrl+S", Os::Mac),
            "Control is not the primary on macOS"
        );
    }

    #[test]
    fn a_pressed_key_becomes_the_chord_that_would_match_it() {
        let ev = |k: Key, m: Modifiers, os| Chord::from_event(&k, m, os);
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        let meta_shift = Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        };
        for os in [Os::Windows, Os::Linux] {
            assert_eq!(ev(Key::Char('S'), ctrl_shift, os), Some(p("Cmd+Shift+S")));
            assert_eq!(ev(Key::Char('s'), meta_shift, os), Some(p("Meta+Shift+S")));
        }
        assert_eq!(
            ev(Key::Char('s'), meta_shift, Os::Mac),
            Some(p("Cmd+Shift+S"))
        );
        assert_eq!(
            ev(Key::Char('s'), ctrl_shift, Os::Mac),
            Some(p("Ctrl+Shift+S"))
        );
        assert_eq!(
            ev(
                Key::Numpad(NumpadKey::Digit(4)),
                Modifiers::NONE,
                Os::Windows
            ),
            Some(p("Numpad4"))
        );
        assert_eq!(ev(Key::F(8), Modifiers::NONE, Os::Mac), Some(p("F8")));
        assert_eq!(ev(Key::Other, Modifiers::NONE, Os::Windows), None);
        // The chord matches the very event it came from.
        for os in Os::ALL {
            let m = Modifiers {
                alt: true,
                ctrl: true,
                ..Modifiers::NONE
            };
            let c = ev(Key::Up, m, os).unwrap();
            assert_eq!(PhysChord::from_event(&Key::Up, m), Some(c.physical(os)));
        }
    }

    #[test]
    fn modifier_sets_parse_print_and_show_per_os() {
        let m = Mods::parse("Alt+Cmd").unwrap();
        assert_eq!(m.to_string(), "Cmd+Alt");
        assert_eq!(m.menu_text(Os::Windows), "Ctrl+Alt");
        assert_eq!(m.menu_text(Os::Mac), "Cmd+Opt");
        assert_eq!(Mods::parse("Shift").unwrap().menu_text(Os::Linux), "Shift");
        assert!(Mods::default().is_empty() && !m.is_empty());
        assert_eq!(Mods::default().menu_text(Os::Windows), "");
        assert_eq!(Mods::parse(""), Err(ChordError::Empty));
        assert!(matches!(
            Mods::parse("Cmd+K"),
            Err(ChordError::UnknownModifier(_))
        ));
        assert!(matches!(
            Mods::parse("Alt+Alt"),
            Err(ChordError::RepeatedModifier(_))
        ));
    }

    #[test]
    fn the_meta_key_is_named_per_os() {
        assert_eq!(p("Meta+K").menu_text(Os::Windows), "Win+K");
        assert_eq!(p("Meta+K").menu_text(Os::Linux), "Super+K");
        assert_eq!(p("Meta+K").menu_text(Os::Mac), "Cmd+K");
        assert_eq!(Os::host() == Os::Windows, cfg!(windows));
    }
}
