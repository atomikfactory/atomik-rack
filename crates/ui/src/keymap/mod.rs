//! The command registry, the keymap and the key router.
//!
//! Pure: no window system, no clock. One [`Registry`] lists every command by a stable dotted
//! id; one [`Keymap`] (the embedded `default_keymap.json`, profile `standard` or `legacy`,
//! resolved for an [`Os`]) says which [`Chord`] runs which command in which [`Context`]; the
//! [`Router`] turns a key event plus the [`ContextStack`] the shell computed from the focused
//! pane into the commands to try, most specific context first, then `Global`.
//!
//! - `Cmd` in the data file is the logical primary modifier: Control on Windows and Linux,
//!   Command on macOS. Keyboard matching is strict (Control and Command are never
//!   interchanged, Super never matches `Cmd` on Windows and Linux); pointer gestures keep the
//!   lenient [`crate::Modifiers::command`].
//! - A command can **decline** ([`CommandDef::can_decline`]): the shell then tries the next
//!   binding of the list.
//! - While a text owner is active (`Context::Text` in the stack) only chords with the primary
//!   modifier, function keys and Esc are resolved; everything else goes to the text owner.
//! - Key repeat is derived from the keys the router has seen held ([`HeldKeys`]): a repeat only
//!   reaches commands marked repeatable.
//! - User remaps (`keymap.json`, a diff over a profile step 2) are [`UserKeymap`];
//!   the [`KeymapEditor`] behind the Settings Keyboard tab edits one and rebuilds the keymap.

mod chord;
mod command;
mod conflict;
mod context;
mod editor;
mod roles;
#[cfg(any(test, feature = "testkit"))]
pub mod script;
pub mod sheet;
mod user;

pub use chord::{Chord, ChordError, ChordKey, Mods, Os, PhysChord};
pub use command::{
    ARP_GATE_ARGS, ARP_OCTAVE_ARGS, ARP_PATTERN_ARGS, ArgKind, CHORD_ARGS, CommandDef, FOLD_ARGS,
    Group, INVERSION_ARGS, RECENT_ARGS, ROOT_ARGS, Registry, SCALE_ARGS, STRUM_AMOUNT_ARGS,
    STRUM_DIRECTION_ARGS,
};
pub use conflict::{Conflict, Shadow, find_conflicts, find_shadows};
pub use context::{Context, ContextStack};
pub use editor::{Clash, KeymapEditor, Rebind, Resolve, Row, RowKey, Slot, Source};
pub use roles::{GestureClass, Role, RoleBinding, Roles};
pub use user::{Built, RemovedDef, Report, USER_KEYMAP_VERSION, UserKeymap, reserved};

use crate::event::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The embedded default keymap.
const DEFAULT_KEYMAP: &str = include_str!("default_keymap.json");

/// A named set of bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// The standard keys (`reference` is accepted as the old name when loading).
    #[serde(alias = "reference")]
    Standard,
    /// The keys Atomik had before the standard keymap.
    Legacy,
}

impl Profile {
    /// Every profile.
    pub const ALL: [Profile; 2] = [Profile::Standard, Profile::Legacy];

    /// The name in the data file.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Standard => "standard",
            Profile::Legacy => "legacy",
        }
    }
}

/// One binding as authored in a data file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingDef {
    /// Command id (unknown ids are kept and ignored by consumers, never an error here).
    pub command: String,
    /// The command's argument, if it takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    /// Where the binding applies.
    pub context: Context,
    /// The chords (see [`Chord::parse`]).
    pub chords: Vec<String>,
    /// Per-OS replacements of `chords` (`win`, `mac`, `linux`); an empty list unbinds there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Deserialize)]
struct ProfileDef {
    name: String,
    bindings: Vec<BindingDef>,
}

#[derive(Debug, Deserialize)]
struct KeymapFile {
    version: u32,
    profiles: Vec<ProfileDef>,
}

/// Why a keymap could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeymapError {
    /// A chord string did not parse.
    #[error("command `{command}`: {source}")]
    Chord {
        /// The command whose chord is bad.
        command: String,
        /// The parse error.
        source: ChordError,
    },
    /// The file is not valid JSON of the expected shape.
    #[error("keymap data: {0}")]
    Data(String),
}

/// A binding with parsed chords, resolved for one OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Command id.
    pub command: String,
    /// Argument.
    pub arg: Option<String>,
    /// Context.
    pub context: Context,
    /// The chords for this OS, in file order.
    pub chords: Vec<Chord>,
}

/// A command a key resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Command id.
    pub command: String,
    /// Argument.
    pub arg: Option<String>,
    /// The context the binding is in.
    pub context: Context,
}

/// What a view answers when the shell offers it a command: the shell asks the
/// focused view first, then runs the commands that are its own.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// The view ran the command; these are the actions the host must carry out.
    Handled(Vec<crate::action::UiAction>),
    /// The view does not own the command, or has nothing to apply it to (no selection): the
    /// next binding of the list runs.
    Declined,
    /// The command is the view's but cannot run now; the message is for the status bar.
    NotApplicable(String),
}

impl CommandOutcome {
    /// The actions of a handled command (empty otherwise).
    pub fn actions(&self) -> &[crate::action::UiAction] {
        match self {
            CommandOutcome::Handled(a) => a,
            _ => &[],
        }
    }

    /// Whether the command was taken (handled or refused with a message): the list stops.
    pub fn is_taken(&self) -> bool {
        !matches!(self, CommandOutcome::Declined)
    }
}

/// What a chord resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Commands to try in order: the most specific context first, `Global` last. The first one
    /// that does not decline wins.
    Bound(Vec<Resolved>),
    /// A text owner is active and the chord is not one that runs in text: the key goes to the
    /// text owner.
    Text,
    /// Nothing is bound.
    Unbound,
}

/// A keymap for one OS.
#[derive(Debug, Clone)]
pub struct Keymap {
    os: Os,
    bindings: Vec<Binding>,
    index: BTreeMap<(Context, PhysChord), Vec<usize>>,
}

impl Keymap {
    /// The bundled keymap of `profile` for `os`.
    ///
    /// # Panics
    /// Never for the embedded data: a test builds every profile for every OS.
    pub fn bundled(profile: Profile, os: Os) -> Keymap {
        match Self::try_bundled(profile, os) {
            Ok(k) => k,
            Err(e) => unreachable!("the embedded keymap is valid: {e}"),
        }
    }

    /// The bundled keymap, reporting an error instead of panicking.
    pub fn try_bundled(profile: Profile, os: Os) -> Result<Keymap, KeymapError> {
        let defs = Self::bundled_defs(profile)?;
        Self::from_defs(os, &defs)
    }

    /// The authored bindings of a bundled profile (before OS resolution).
    pub fn bundled_defs(profile: Profile) -> Result<Vec<BindingDef>, KeymapError> {
        let file: KeymapFile =
            serde_json::from_str(DEFAULT_KEYMAP).map_err(|e| KeymapError::Data(e.to_string()))?;
        if file.version != 1 {
            return Err(KeymapError::Data(format!("version {}", file.version)));
        }
        file.profiles
            .into_iter()
            .find(|p| p.name == profile.name())
            .map(|p| p.bindings)
            .ok_or_else(|| KeymapError::Data(format!("no profile `{}`", profile.name())))
    }

    /// Builds a keymap from authored bindings for `os`: applies the `os` overrides and parses
    /// the chords.
    pub fn from_defs(os: Os, defs: &[BindingDef]) -> Result<Keymap, KeymapError> {
        let mut bindings = Vec::with_capacity(defs.len());
        for d in defs {
            let texts =
                d.os.as_ref()
                    .and_then(|m| m.get(os.name()))
                    .unwrap_or(&d.chords);
            let chords = texts
                .iter()
                .map(|t| Chord::parse(t))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| KeymapError::Chord {
                    command: d.command.clone(),
                    source,
                })?;
            bindings.push(Binding {
                command: d.command.clone(),
                arg: d.arg.clone(),
                context: d.context,
                chords,
            });
        }
        let mut index: BTreeMap<(Context, PhysChord), Vec<usize>> = BTreeMap::new();
        for (i, b) in bindings.iter().enumerate() {
            for c in &b.chords {
                index
                    .entry((b.context, c.physical(os)))
                    .or_default()
                    .push(i);
            }
        }
        Ok(Keymap {
            os,
            bindings,
            index,
        })
    }

    /// The OS the keymap is resolved for.
    pub fn os(&self) -> Os {
        self.os
    }

    /// The bindings, in file order.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// What a chord does in the active contexts (module docs).
    pub fn resolve(&self, chord: &PhysChord, stack: &ContextStack) -> Resolution {
        if stack.is_text() && !chord.runs_in_text(self.os) {
            return Resolution::Text;
        }
        let mut out = Vec::new();
        for ctx in stack.contexts() {
            if *ctx == Context::Text {
                continue;
            }
            for &i in self.index.get(&(*ctx, *chord)).into_iter().flatten() {
                let b = &self.bindings[i];
                out.push(Resolved {
                    command: b.command.clone(),
                    arg: b.arg.clone(),
                    context: b.context,
                });
            }
        }
        if out.is_empty() {
            Resolution::Unbound
        } else {
            Resolution::Bound(out)
        }
    }

    /// What a key event does (see [`Self::resolve`]); `Unbound` for keys that are no chord.
    pub fn resolve_event(
        &self,
        key: &Key,
        modifiers: Modifiers,
        stack: &ContextStack,
    ) -> Resolution {
        match PhysChord::from_event(key, modifiers) {
            Some(chord) => self.resolve(&chord, stack),
            None => Resolution::Unbound,
        }
    }

    /// The shortcut text a menu or tooltip shows for a command: its first chord in file order,
    /// printed for this keymap's OS (`Ctrl+Shift+S`, `Cmd+Shift+S`). `None` when unbound.
    pub fn shortcut_text(&self, command: &str, arg: Option<&str>) -> Option<String> {
        self.bindings
            .iter()
            .filter(|b| b.command == command && b.arg.as_deref() == arg)
            .find_map(|b| b.chords.first())
            .map(|c| c.menu_text(self.os))
    }

    /// The chords of a command (every context), for listings.
    pub fn chords_of(&self, command: &str, arg: Option<&str>) -> Vec<(Context, Chord)> {
        self.bindings
            .iter()
            .filter(|b| b.command == command && b.arg.as_deref() == arg)
            .flat_map(|b| b.chords.iter().map(|c| (b.context, *c)))
            .collect()
    }

    /// A stable text listing (`command[ arg] | context | chord, chord`), sorted by command, arg
    /// and context, chords as this OS shows them: the golden files of the bundled maps.
    pub fn listing(&self) -> String {
        let mut rows: Vec<(String, String, Context, String)> = self
            .bindings
            .iter()
            .map(|b| {
                let chords: Vec<String> = b.chords.iter().map(|c| c.menu_text(self.os)).collect();
                (
                    b.command.clone(),
                    b.arg.clone().unwrap_or_default(),
                    b.context,
                    chords.join(", "),
                )
            })
            .collect();
        rows.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
        let mut out = String::new();
        for (command, arg, ctx, chords) in rows {
            let name = if arg.is_empty() {
                command
            } else {
                format!("{command} {arg}")
            };
            out.push_str(&format!("{name} | {} | {chords}\n", ctx.name()));
        }
        out
    }
}

/// The keys the router has seen pressed and not yet released, to tell a repeat from a press.
///
/// Letters are folded to lower case so that a release after Shift came up still matches its
/// press. `Key::Other` (modifier keys and unknown keys) is not tracked.
#[derive(Debug, Clone, Default)]
pub struct HeldKeys {
    keys: Vec<Key>,
}

impl HeldKeys {
    /// No key held.
    pub fn new() -> Self {
        Self::default()
    }

    fn norm(key: &Key) -> Option<Key> {
        match key {
            Key::Other => None,
            Key::Char(c) => Some(Key::Char(c.to_lowercase().next().unwrap_or(*c))),
            other => Some(other.clone()),
        }
    }

    /// Records a press; returns `true` when the key was already held (auto-repeat).
    pub fn press(&mut self, key: &Key) -> bool {
        let Some(k) = Self::norm(key) else {
            return false;
        };
        if self.keys.contains(&k) {
            true
        } else {
            self.keys.push(k);
            false
        }
    }

    /// Records a release.
    pub fn release(&mut self, key: &Key) {
        if let Some(k) = Self::norm(key) {
            self.keys.retain(|h| *h != k);
        }
    }

    /// Forgets every key (focus lost: the releases will never arrive).
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// Whether any key is held.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// What the router decided for one key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A release: nothing to run (the view may still want it).
    Release,
    /// A repeat of a key whose command is not repeatable: swallowed.
    RepeatIgnored,
    /// Commands to try in order; the first that does not decline wins.
    Commands(Vec<Resolved>),
    /// A text owner is active: the key goes to it.
    Text,
    /// Nothing is bound: the key goes to the focused view.
    Unbound,
}

/// The router: a keymap, the registry and the held keys.
#[derive(Debug, Clone)]
pub struct Router {
    keymap: Keymap,
    held: HeldKeys,
}

impl Router {
    /// A router over `keymap`.
    pub fn new(keymap: Keymap) -> Self {
        Self {
            keymap,
            held: HeldKeys::new(),
        }
    }

    /// The keymap.
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// Replaces the keymap (a profile switch or a user remap); the held keys stay.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
    }

    /// Forgets the held keys (window focus lost).
    pub fn clear_held(&mut self) {
        self.held.clear();
    }

    /// Routes one key event.
    pub fn route(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        pressed: bool,
        stack: &ContextStack,
    ) -> Route {
        if !pressed {
            self.held.release(key);
            return Route::Release;
        }
        let repeat = self.held.press(key);
        match self.keymap.resolve_event(key, modifiers, stack) {
            Resolution::Text => Route::Text,
            Resolution::Unbound => Route::Unbound,
            Resolution::Bound(list) => {
                if !repeat {
                    return Route::Commands(list);
                }
                let registry = Registry::bundled();
                let list: Vec<Resolved> = list
                    .into_iter()
                    .filter(|r| registry.get(&r.command).is_some_and(|d| d.repeatable))
                    .collect();
                if list.is_empty() {
                    Route::RepeatIgnored
                } else {
                    Route::Commands(list)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_tests;
