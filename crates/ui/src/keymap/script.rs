//! `KeyScript`: a small text format that drives the router without a window.
//!
//! One step per line; `#` starts a comment.
//!
//! ```text
//! os mac                      # win | mac | linux (default win); rebuilds the router
//! profile legacy              # standard | legacy (default standard); rebuilds the router
//! ctx rack                    # the context stack, most specific first (default: global)
//! down Cmd+D                  # press: the chord's key with the chord's modifiers
//! up Cmd+D                    # release
//! mods Alt                    # the modifiers held for role checks (`none` clears)
//! expect command edit.duplicate [arg]   # the first command the last press resolved to
//! expect commands a.b,c.d     # the whole list, in order
//! expect text | unbound | release | repeat-ignored
//! expect role alternate_tool hold       # a role is active for the held modifiers
//! expect no-role copy_drag drag-item
//! ```
//!
//! The app's end-to-end key scripts reuse this driver through the `testkit` feature.

use super::{
    Chord, ChordKey, Context, ContextStack, GestureClass, Keymap, Os, Profile, Role, Roles, Route,
    Router,
};
use crate::event::{Key, Modifiers};

/// A running script.
#[derive(Debug)]
pub struct KeyScript {
    os: Os,
    profile: Profile,
    router: Router,
    stack: ContextStack,
    held_mods: Modifiers,
    last: Option<Route>,
}

impl Default for KeyScript {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyScript {
    /// A script on Windows with the standard profile and the `Global` context.
    pub fn new() -> Self {
        let (os, profile) = (Os::Windows, Profile::Standard);
        Self {
            os,
            profile,
            router: Router::new(Keymap::bundled(profile, os)),
            stack: ContextStack::global(),
            held_mods: Modifiers::NONE,
            last: None,
        }
    }

    /// Runs a whole script; the error names the line.
    pub fn run(text: &str) -> Result<KeyScript, String> {
        let mut s = KeyScript::new();
        for (i, line) in text.lines().enumerate() {
            s.step(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        }
        Ok(s)
    }

    /// The route of the last `down` or `up`.
    pub fn last(&self) -> Option<&Route> {
        self.last.as_ref()
    }

    fn rebuild(&mut self) {
        self.router = Router::new(Keymap::bundled(self.profile, self.os));
    }

    /// Runs one line.
    pub fn step(&mut self, line: &str) -> Result<(), String> {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            return Ok(());
        }
        let mut words = line.split_whitespace();
        let verb = words.next().unwrap_or_default();
        let rest: Vec<&str> = words.collect();
        match (verb, rest.as_slice()) {
            ("os", [name]) => {
                self.os = Os::ALL
                    .into_iter()
                    .find(|o| o.name() == *name)
                    .ok_or(format!("unknown os `{name}`"))?;
                self.rebuild();
            }
            ("profile", [name]) => {
                self.profile = Profile::ALL
                    .into_iter()
                    .find(|p| p.name() == *name)
                    .ok_or(format!("unknown profile `{name}`"))?;
                self.rebuild();
            }
            ("ctx", names) => {
                let mut contexts = Vec::new();
                for n in names {
                    contexts.push(
                        Context::ALL
                            .into_iter()
                            .find(|c| c.name() == *n)
                            .ok_or(format!("unknown context `{n}`"))?,
                    );
                }
                self.stack = ContextStack::new(&contexts);
            }
            ("down", [chord]) | ("up", [chord]) => {
                let (key, modifiers) = self.event_of(chord)?;
                self.last = Some(
                    self.router
                        .route(&key, modifiers, verb == "down", &self.stack),
                );
            }
            ("mods", [text]) => self.held_mods = self.mods_of(text)?,
            ("expect", args) => self.expect(args)?,
            _ => return Err(format!("cannot understand `{line}`")),
        }
        Ok(())
    }

    fn event_of(&self, chord: &str) -> Result<(Key, Modifiers), String> {
        event_for(self.os, chord)
    }

    fn mods_of(&self, text: &str) -> Result<Modifiers, String> {
        if text == "none" {
            return Ok(Modifiers::NONE);
        }
        let c = Chord::parse(&format!("{text}+A")).map_err(|e| e.to_string())?;
        let p = c.physical(self.os);
        Ok(Modifiers {
            shift: p.shift,
            ctrl: p.ctrl,
            alt: p.alt,
            meta: p.meta,
        })
    }

    fn expect(&self, args: &[&str]) -> Result<(), String> {
        let last = self.last.as_ref().ok_or("no key event yet")?;
        let mismatch = |want: &str| format!("expected {want}, got {last:?}");
        match args {
            ["command", id, rest @ ..] => {
                let arg = rest.first().copied();
                match last {
                    Route::Commands(list)
                        if list
                            .first()
                            .is_some_and(|r| r.command == *id && r.arg.as_deref() == arg) => {}
                    _ => return Err(mismatch(&format!("command {id} {arg:?}"))),
                }
            }
            ["commands", ids] => {
                let want: Vec<&str> = ids.split(',').collect();
                match last {
                    Route::Commands(list)
                        if list
                            .iter()
                            .map(|r| r.command.as_str())
                            .eq(want.iter().copied()) => {}
                    _ => return Err(mismatch(ids)),
                }
            }
            ["text"] if matches!(last, Route::Text) => {}
            ["unbound"] if matches!(last, Route::Unbound) => {}
            ["release"] if matches!(last, Route::Release) => {}
            ["repeat-ignored"] if matches!(last, Route::RepeatIgnored) => {}
            ["text" | "unbound" | "release" | "repeat-ignored"] => {
                return Err(mismatch(args[0]));
            }
            [kind @ ("role" | "no-role"), id, class] => {
                let role = Role::ALL
                    .into_iter()
                    .find(|r| r.id() == *id)
                    .ok_or(format!("unknown role `{id}`"))?;
                let class = match *class {
                    "hold" => GestureClass::Hold,
                    "click-item" => GestureClass::ClickItem,
                    "click-control" => GestureClass::ClickControl,
                    "drag-item" => GestureClass::DragItem,
                    "drag-control" => GestureClass::DragControl,
                    other => return Err(format!("unknown gesture class `{other}`")),
                };
                let on = Roles::defaults(self.os).is_active(role, self.held_mods, class);
                if on != (*kind == "role") {
                    return Err(format!("{kind} {id} {class:?}: active = {on}"));
                }
            }
            _ => return Err(format!("cannot understand expect {args:?}")),
        }
        Ok(())
    }
}

/// The key event a chord text stands for on `os`: the chord's key with the chord's physical
/// modifiers (`Cmd+S` is Ctrl+S on Windows and Linux, Meta+S on macOS). Shared with the app's
/// end-to-end key scripts.
pub fn event_for(os: Os, chord: &str) -> Result<(Key, Modifiers), String> {
    let c = Chord::parse(chord).map_err(|e| e.to_string())?;
    let p = c.physical(os);
    let key = match c.key {
        ChordKey::Char(ch) => Key::Char(ch),
        ChordKey::Escape => Key::Escape,
        ChordKey::Return => Key::Enter,
        ChordKey::Space => Key::Space,
        ChordKey::Backspace => Key::Backspace,
        ChordKey::Delete => Key::Delete,
        ChordKey::Tab => Key::Tab,
        ChordKey::Up => Key::Up,
        ChordKey::Down => Key::Down,
        ChordKey::Left => Key::Left,
        ChordKey::Right => Key::Right,
        ChordKey::PageUp => Key::PageUp,
        ChordKey::PageDown => Key::PageDown,
        ChordKey::Home => Key::Home,
        ChordKey::End => Key::End,
        ChordKey::Insert => Key::Insert,
        ChordKey::F(n) => Key::F(n),
        ChordKey::Numpad(k) => Key::Numpad(k),
        ChordKey::Digit(d) => Key::Digit(d),
    };
    Ok((
        key,
        Modifiers {
            shift: p.shift,
            ctrl: p.ctrl,
            alt: p.alt,
            meta: p.meta,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_drives_the_router() {
        KeyScript::run(
            "
            os win
            ctx rack
            down Cmd+D
            expect command edit.duplicate
            up Cmd+D
            expect release
            down Cmd+Z
            expect command edit.undo
            up Cmd+Z
            ctx editor
            down Q
            expect command tool.select
            ",
        )
        .unwrap();
    }

    #[test]
    fn script_errors_name_the_line() {
        let e = KeyScript::run("down Cmd+D\nexpect command edit.undo").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
        assert!(KeyScript::run("frobnicate").is_err());
        assert!(KeyScript::run("expect text").is_err());
        assert!(KeyScript::run("os beos").is_err());
        assert!(KeyScript::run("down Hyper+X").is_err());
    }

    #[test]
    fn roles_are_checked_from_held_modifiers() {
        KeyScript::run(
            "os win\nmods Alt\ndown A\nexpect role alternate_tool hold\nexpect no-role disable_snap hold\n\
             mods Cmd+Alt\nexpect role disable_snap hold\nos mac\nmods Ctrl\nexpect role alternate_tool hold",
        )
        .unwrap();
    }
}
