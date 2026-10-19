//! The editor behind the Keyboard tab: list, search, rebind, conflicts, restore. Pure: the dialog drives it, the app saves its [`UserKeymap`].
//!
//! The editor owns a user diff over a profile on one OS and rebuilds the effective keymap
//! after every edit. A binding is addressed by a [`RowKey`] (command, argument, context).
//! Every edit writes the **full chord list** of the binding it touches as a user entry and
//! then drops entries that equal the profile on every OS, so `keymap.json` only ever holds
//! what differs.

use super::user::{Built, Report, reserved};
use super::{
    BindingDef, Chord, Conflict, Context, Group, Keymap, Mods, Os, PhysChord, Profile, Registry,
    Role, Roles, UserKeymap, find_conflicts,
};
use std::collections::BTreeSet;

/// The identity of a binding row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowKey {
    /// Command id.
    pub command: String,
    /// The argument (for commands that take one, one row per value).
    pub arg: Option<String>,
    /// The context the binding is in.
    pub context: Context,
}

/// Where a row's chords come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The profile.
    Default,
    /// Changed by the user.
    User,
}

/// One line of the Keyboard tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The binding.
    pub key: RowKey,
    /// The command's label (with the argument in parentheses when it has one).
    pub label: String,
    /// The group the row is listed under.
    pub group: Group,
    /// The chords on this OS, in order.
    pub chords: Vec<Chord>,
    /// Default or user.
    pub source: Source,
}

/// Which chord of a row an edit replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The chord at this index (appended when out of range).
    Replace(usize),
    /// A new chord after the existing ones.
    Add,
}

/// Another command on the chord being assigned, in the same context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    /// Command id.
    pub command: String,
    /// Its argument.
    pub arg: Option<String>,
    /// Its label, for the conflict dialog.
    pub label: String,
    /// The shared context.
    pub context: Context,
}

/// The result of an attempt to assign a chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebind {
    /// Done; the notes say what the new binding shadows (not an error).
    Applied {
        /// Shadow notes.
        notes: Vec<String>,
    },
    /// The row already had that chord.
    Unchanged,
    /// The OS, Esc or a text field owns the chord.
    Reserved(&'static str),
    /// Other commands hold the chord in this context: nothing changed, ask the user.
    Conflict(Vec<Clash>),
}

/// How the user settled a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolve {
    /// The other command gets the chord the slot had.
    Swap,
    /// The other command loses the chord.
    UnassignOther,
    /// Leave everything as it was.
    Cancel,
}

/// A user diff over a profile on one OS, with its effective keymap.
#[derive(Debug, Clone)]
pub struct KeymapEditor {
    os: Os,
    default_profile: Profile,
    user: UserKeymap,
    base: Vec<BindingDef>,
    built: Built,
}

impl KeymapEditor {
    /// An editor over `user`; `default_profile` applies until the user chooses one.
    pub fn new(os: Os, default_profile: Profile, user: UserKeymap) -> Self {
        let profile = user.profile.unwrap_or(default_profile);
        let base = Self::base_of(profile);
        let built = user.build(profile, os);
        Self {
            os,
            default_profile,
            user,
            base,
            built,
        }
    }

    fn base_of(profile: Profile) -> Vec<BindingDef> {
        match Keymap::bundled_defs(profile) {
            Ok(defs) => defs,
            Err(e) => unreachable!("the embedded keymap is valid: {e}"),
        }
    }

    fn refresh(&mut self) {
        let profile = self.profile();
        self.base = Self::base_of(profile);
        self.built = self.user.build(profile, self.os);
    }

    /// The OS the chords are shown and matched for.
    pub fn os(&self) -> Os {
        self.os
    }

    /// The active profile: the user's choice, else the default.
    pub fn profile(&self) -> Profile {
        self.user.profile.unwrap_or(self.default_profile)
    }

    /// Chooses the profile (kept in the file). The user's changes stay on top of it.
    pub fn set_profile(&mut self, profile: Profile) {
        self.user.profile = Some(profile);
        self.normalize();
        self.refresh();
    }

    /// The user diff (what is saved).
    pub fn user(&self) -> &UserKeymap {
        &self.user
    }

    /// The effective keymap.
    pub fn keymap(&self) -> &Keymap {
        &self.built.keymap
    }

    /// The effective roles.
    pub fn roles(&self) -> &Roles {
        &self.built.roles
    }

    /// What building the keymap found (conflicts, reserved and bad chords, shadows).
    pub fn report(&self) -> &Report {
        &self.built.report
    }

    // ------------------------------------------------------------------ rows

    /// Every command as rows, by group then label, filtered by `query` (blank: all). The query
    /// is matched, ignoring case, against the label, the id, the argument, the group and the
    /// chords as this OS shows them.
    pub fn rows(&self, query: &str) -> Vec<Row> {
        let query = query.trim().to_lowercase();
        let mut rows = Vec::new();
        for d in Registry::bundled().commands() {
            let args: Vec<Option<&str>> = match d.args {
                Some(super::ArgKind::Choice(choices)) => choices.iter().map(|c| Some(*c)).collect(),
                None => vec![None],
            };
            for arg in args {
                let mut contexts: Vec<Context> = self
                    .built
                    .keymap
                    .bindings()
                    .iter()
                    .filter(|b| b.command == d.id && b.arg.as_deref() == arg)
                    .map(|b| b.context)
                    .collect();
                contexts.sort();
                contexts.dedup();
                if contexts.is_empty() {
                    contexts.push(d.context);
                }
                for context in contexts {
                    let key = RowKey {
                        command: d.id.to_string(),
                        arg: arg.map(str::to_string),
                        context,
                    };
                    let label = match arg {
                        Some(a) => format!("{} ({a})", d.label),
                        None => d.label.to_string(),
                    };
                    let row = Row {
                        chords: self.current_chords(&key),
                        source: self.source_of(&key),
                        key,
                        label,
                        group: d.group,
                    };
                    if query.is_empty() || self.matches(&row, d.group, &query) {
                        rows.push(row);
                    }
                }
            }
        }
        rows.sort_by(|a, b| {
            (a.group, a.label.to_lowercase(), &a.key).cmp(&(
                b.group,
                b.label.to_lowercase(),
                &b.key,
            ))
        });
        rows
    }

    fn matches(&self, row: &Row, group: Group, query: &str) -> bool {
        let mut hay = vec![
            row.label.to_lowercase(),
            row.key.command.to_lowercase(),
            group.title().to_lowercase(),
        ];
        hay.extend(row.key.arg.iter().map(|a| a.to_lowercase()));
        hay.extend(
            row.chords
                .iter()
                .map(|c| c.menu_text(self.os).to_lowercase()),
        );
        hay.iter().any(|h| h.contains(query))
    }

    fn source_of(&self, key: &RowKey) -> Source {
        let edited = self
            .user
            .bindings
            .iter()
            .any(|b| b.command == key.command && b.arg == key.arg && b.context == key.context);
        let removed = self.user.removed.iter().any(|r| {
            r.command == key.command
                && r.arg == key.arg
                && r.context.is_none_or(|c| c == key.context)
        });
        if edited || removed {
            Source::User
        } else {
            Source::Default
        }
    }

    /// The chords of a row on this OS, in order.
    pub fn current_chords(&self, key: &RowKey) -> Vec<Chord> {
        self.built
            .keymap
            .bindings()
            .iter()
            .filter(|b| b.command == key.command && b.arg == key.arg && b.context == key.context)
            .flat_map(|b| b.chords.iter().copied())
            .collect()
    }

    // ------------------------------------------------------------------ edits

    /// Assigns `chord` to a slot of a row. Conflicts and reserved chords change nothing; a
    /// conflict is settled with [`Self::resolve`].
    pub fn rebind(&mut self, key: &RowKey, slot: Slot, chord: Chord) -> Rebind {
        if let Some(why) = self.refusal(key, &chord) {
            return Rebind::Reserved(why);
        }
        let phys = chord.physical(self.os);
        if self
            .current_chords(key)
            .iter()
            .any(|c| c.physical(self.os) == phys)
        {
            return Rebind::Unchanged;
        }
        let clashes = self.clashes(key, phys);
        if !clashes.is_empty() {
            return Rebind::Conflict(clashes);
        }
        self.apply(key, slot, chord)
    }

    /// Settles a conflict returned by [`Self::rebind`].
    pub fn resolve(&mut self, key: &RowKey, slot: Slot, chord: Chord, how: Resolve) -> Rebind {
        if how == Resolve::Cancel {
            return Rebind::Unchanged;
        }
        if let Some(why) = self.refusal(key, &chord) {
            return Rebind::Reserved(why);
        }
        let phys = chord.physical(self.os);
        let previous = match slot {
            Slot::Replace(i) => self.current_chords(key).get(i).copied(),
            Slot::Add => None,
        };
        for clash in self.clashes(key, phys) {
            let other = RowKey {
                command: clash.command,
                arg: clash.arg,
                context: clash.context,
            };
            let mut list: Vec<Chord> = Vec::new();
            for c in self.current_chords(&other) {
                if c.physical(self.os) != phys {
                    list.push(c);
                } else if let (Resolve::Swap, Some(p)) = (how, previous)
                    && !list
                        .iter()
                        .any(|x| x.physical(self.os) == p.physical(self.os))
                {
                    list.push(p);
                }
            }
            self.set_chords(&other, list);
        }
        self.apply(key, slot, chord)
    }

    /// Removes the chord at `index` of a row (the command may end up unbound).
    pub fn clear(&mut self, key: &RowKey, index: usize) {
        let mut list = self.current_chords(key);
        if index < list.len() {
            list.remove(index);
            self.set_chords(key, list);
        }
    }

    /// Gives a row its profile chords back. Returns the conflicts that involve the command
    /// afterwards (another command may have taken the chord since).
    pub fn restore(&mut self, key: &RowKey) -> Vec<Conflict> {
        self.drop_entries(key);
        self.normalize();
        self.refresh();
        self.conflicts_of(key)
    }

    /// Drops every user binding, removal and role override; the profile choice stays.
    pub fn restore_all(&mut self) {
        self.user.bindings.clear();
        self.user.removed.clear();
        self.user.roles.clear();
        self.refresh();
    }

    /// Replaces the user diff (an import); the effective keymap follows.
    pub fn replace_user(&mut self, user: UserKeymap) {
        self.user = user;
        self.normalize();
        self.refresh();
    }

    fn refusal(&self, key: &RowKey, chord: &Chord) -> Option<&'static str> {
        let why = reserved(self.os, key.context, chord)?;
        (!self.is_default_chord(key, chord)).then_some(why)
    }

    /// Whether the profile itself gives `chord` to the row on this OS.
    fn is_default_chord(&self, key: &RowKey, chord: &Chord) -> bool {
        let probe = BindingDef {
            command: key.command.clone(),
            arg: key.arg.clone(),
            context: key.context,
            chords: Vec::new(),
            os: None,
        };
        self.base
            .iter()
            .filter(|d| {
                d.command == probe.command && d.arg == probe.arg && d.context == probe.context
            })
            .any(|d| {
                let texts =
                    d.os.as_ref()
                        .and_then(|m| m.get(self.os.name()))
                        .unwrap_or(&d.chords);
                texts.iter().any(|t| {
                    Chord::parse(t).is_ok_and(|c| c.physical(self.os) == chord.physical(self.os))
                })
            })
    }

    fn clashes(&self, key: &RowKey, phys: PhysChord) -> Vec<Clash> {
        let registry = Registry::bundled();
        let mut out: Vec<Clash> = Vec::new();
        for b in self.built.keymap.bindings() {
            if b.context != key.context || (b.command == key.command && b.arg == key.arg) {
                continue;
            }
            if !b.chords.iter().any(|c| c.physical(self.os) == phys) {
                continue;
            }
            if out.iter().any(|c| c.command == b.command && c.arg == b.arg) {
                continue;
            }
            let base = registry
                .get(&b.command)
                .map_or(b.command.as_str(), |d| d.label);
            out.push(Clash {
                label: match &b.arg {
                    Some(a) => format!("{base} ({a})"),
                    None => base.to_string(),
                },
                command: b.command.clone(),
                arg: b.arg.clone(),
                context: b.context,
            });
        }
        out
    }

    fn apply(&mut self, key: &RowKey, slot: Slot, chord: Chord) -> Rebind {
        let mut list = self.current_chords(key);
        match slot {
            Slot::Replace(i) if i < list.len() => list[i] = chord,
            _ => list.push(chord),
        }
        self.set_chords(key, list);
        Rebind::Applied {
            notes: self.shadow_notes(key, chord),
        }
    }

    fn shadow_notes(&self, key: &RowKey, chord: Chord) -> Vec<String> {
        let registry = Registry::bundled();
        let phys = chord.physical(self.os);
        let label = |id: &str| {
            registry
                .get(id)
                .map_or(id.to_string(), |d| d.label.to_string())
        };
        let text = chord.menu_text(self.os);
        let mut notes = Vec::new();
        let mut seen: BTreeSet<(String, Context)> = BTreeSet::new();
        for b in self.built.keymap.bindings() {
            if b.command == key.command && b.arg == key.arg {
                continue;
            }
            if !b.chords.iter().any(|c| c.physical(self.os) == phys) {
                continue;
            }
            if !seen.insert((b.command.clone(), b.context)) {
                continue;
            }
            if key.context != Context::Global && b.context == Context::Global {
                notes.push(format!(
                    "{text} now runs {} in the {}; {} keeps it elsewhere",
                    label(&key.command),
                    key.context.name(),
                    label(&b.command)
                ));
            } else if key.context == Context::Global && b.context != Context::Global {
                notes.push(format!(
                    "{} keeps {text} in the {}",
                    label(&b.command),
                    b.context.name()
                ));
            }
        }
        notes
    }

    fn conflicts_of(&self, key: &RowKey) -> Vec<Conflict> {
        find_conflicts(&self.built.keymap)
            .into_iter()
            .filter(|c| {
                c.commands
                    .iter()
                    .any(|(id, arg)| *id == key.command && *arg == key.arg)
            })
            .collect()
    }

    fn drop_entries(&mut self, key: &RowKey) {
        self.user.bindings.retain(|b| {
            !(b.command == key.command && b.arg == key.arg && b.context == key.context)
        });
        self.user.removed.retain(|r| {
            !(r.command == key.command
                && r.arg == key.arg
                && r.context.is_none_or(|c| c == key.context))
        });
    }

    fn set_chords(&mut self, key: &RowKey, chords: Vec<Chord>) {
        self.drop_entries(key);
        self.user.bindings.push(BindingDef {
            command: key.command.clone(),
            arg: key.arg.clone(),
            context: key.context,
            chords: chords.iter().map(Chord::to_string).collect(),
            os: None,
        });
        self.normalize();
        self.refresh();
    }

    /// The chords the profile gives a row on `os`.
    fn profile_chords(&self, key: &(String, Option<String>, Context), os: Os) -> Vec<Chord> {
        self.base
            .iter()
            .filter(|d| d.command == key.0 && d.arg == key.1 && d.context == key.2)
            .flat_map(|d| {
                d.os.as_ref()
                    .and_then(|m| m.get(os.name()))
                    .unwrap_or(&d.chords)
                    .iter()
                    .filter_map(|t| Chord::parse(t).ok())
            })
            .collect()
    }

    /// Drops user bindings that equal the profile on every OS, so the file holds only changes.
    fn normalize(&mut self) {
        let keep: Vec<bool> = self
            .user
            .bindings
            .iter()
            .map(|b| {
                let key = (b.command.clone(), b.arg.clone(), b.context);
                let user: Option<Vec<Chord>> =
                    b.chords.iter().map(|t| Chord::parse(t).ok()).collect();
                match user {
                    // Unparseable text is left alone (the report names it).
                    None => true,
                    Some(user) => !Os::ALL
                        .iter()
                        .all(|os| self.profile_chords(&key, *os) == user),
                }
            })
            .collect();
        let mut it = keep.into_iter();
        self.user.bindings.retain(|_| it.next().unwrap_or(true));
    }

    // ------------------------------------------------------------------ roles

    /// Rebinds a role's modifier set. A set another role of the same gesture class already
    /// uses, or no modifier at all, is refused with a message.
    pub fn set_role(&mut self, role: Role, mods: Mods) -> Result<(), String> {
        if mods.is_empty() {
            return Err("a role needs at least one modifier".to_string());
        }
        if let Some(other) = self.built.roles.clash(role, mods) {
            return Err(format!(
                "{} already uses {} for the same kind of gesture",
                other.label(),
                mods.menu_text(self.os)
            ));
        }
        let default = Roles::defaults(self.os).binding(role).map(|b| b.mods);
        if default == Some(mods) {
            self.user.roles.remove(role.id());
        } else {
            self.user
                .roles
                .insert(role.id().to_string(), mods.to_string());
        }
        self.refresh();
        Ok(())
    }

    /// Gives a role its default modifiers back.
    pub fn reset_role(&mut self, role: Role) {
        self.user.roles.remove(role.id());
        self.refresh();
    }
}
