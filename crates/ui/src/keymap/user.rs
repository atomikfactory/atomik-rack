//! The user keymap: `keymap.json`, a diff over the active profile.
//!
//! ```json
//! { "version": 1, "profile": "standard",
//!   "bindings": [ { "command": "edit.duplicate", "context": "global", "chords": ["Cmd+Shift+D"] } ],
//!   "removed": [ { "command": "edit.redo", "chord": "Cmd+Y" } ],
//!   "roles": { "alternate_tool": "Alt" } }
//! ```
//!
//! - `bindings` replace the chords of the profile binding with the same command, argument and
//!   context (the entry holds the full list, in logical `Cmd` grammar, for every OS), or add a
//!   binding the profile does not have; an empty list unbinds.
//! - `removed` takes one chord away from the profile binding(s) of a command (also from its
//!   per-OS overrides).
//! - Loading is lenient: unknown fields are ignored, entries of the wrong shape are skipped with
//!   a note, **unknown command ids are kept** (a newer or older build may know them) and ignored
//!   when the keymap is built; a file that is not a JSON object is an error (the store renames
//!   it to `keymap.json.bad`). Saving is canonical: sorted, canonical chords, no timestamps.
//! - Building a keymap never fails: bad chords, reserved chords and unknown ids are dropped
//!   from the keymap and listed in a [`Report`], same-chord conflicts stay in (both commands
//!   run in order) and are listed, shadows are notes.

use super::{
    BindingDef, Chord, ChordKey, Conflict, Context, Keymap, Mods, Os, Profile, Registry, Role,
    Roles, Shadow, find_conflicts, find_shadows,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The file format version this build writes.
pub const USER_KEYMAP_VERSION: u32 = 1;

/// One chord taken away from the profile's binding(s) of a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedDef {
    /// The command id.
    pub command: String,
    /// The argument of the binding (`None`: bindings without one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    /// The context of the binding (`None`: every context).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
    /// The chord that is taken away.
    pub chord: String,
}

/// The user's changes over a profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserKeymap {
    /// The profile the user chose (`None`: the app default).
    pub profile: Option<Profile>,
    /// Changed and added bindings.
    pub bindings: Vec<BindingDef>,
    /// Chords taken away.
    pub removed: Vec<RemovedDef>,
    /// Modifier role overrides (role id, modifier set such as `Cmd+Alt`).
    pub roles: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bindings: Vec<BindingDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<RemovedDef>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    roles: &'a BTreeMap<String, String>,
}

fn canon(chord: &str) -> String {
    Chord::parse(chord).map_or_else(|_| chord.to_string(), |c| c.to_string())
}

fn str_field<'a>(obj: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a str> {
    obj.get(name).and_then(Value::as_str)
}

fn context_of(value: &Value) -> Option<Context> {
    serde_json::from_value::<Context>(value.clone()).ok()
}

fn parse_binding(value: &Value) -> Option<BindingDef> {
    let obj = value.as_object()?;
    let command = str_field(obj, "command").filter(|c| !c.is_empty())?;
    let context = context_of(obj.get("context")?)?;
    let chords = obj
        .get("chords")?
        .as_array()?
        .iter()
        .map(|c| c.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    let arg = match obj.get("arg") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str()?.to_string()),
    };
    Some(BindingDef {
        command: command.to_string(),
        arg,
        context,
        chords,
        os: None,
    })
}

fn parse_removed(value: &Value) -> Option<RemovedDef> {
    let obj = value.as_object()?;
    let command = str_field(obj, "command").filter(|c| !c.is_empty())?;
    let chord = str_field(obj, "chord")?;
    let arg = match obj.get("arg") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str()?.to_string()),
    };
    let context = match obj.get("context") {
        None | Some(Value::Null) => None,
        Some(v) => Some(context_of(v)?),
    };
    Some(RemovedDef {
        command: command.to_string(),
        arg,
        context,
        chord: chord.to_string(),
    })
}

impl UserKeymap {
    /// Parses a `keymap.json` leniently (module docs). The notes name the entries that were
    /// skipped. An error means the text is not a JSON object at all.
    pub fn from_json(text: &str) -> Result<(UserKeymap, Vec<String>), String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = value
            .as_object()
            .ok_or_else(|| "the keymap file must be a JSON object".to_string())?;
        let mut notes = Vec::new();
        let mut user = UserKeymap::default();
        match obj.get("profile") {
            None | Some(Value::Null) => {}
            Some(v) => match v.as_str() {
                // `reference` is the old name of `standard`; still read from existing files.
                Some("standard" | "reference") => user.profile = Some(Profile::Standard),
                Some("legacy") => user.profile = Some(Profile::Legacy),
                _ => notes.push(format!("profile {v} is not known; the default is used")),
            },
        }
        if let Some(list) = obj.get("bindings").and_then(Value::as_array) {
            for (i, entry) in list.iter().enumerate() {
                match parse_binding(entry) {
                    Some(b) => user.bindings.push(b),
                    None => notes.push(format!("bindings[{i}] is not a binding; skipped")),
                }
            }
        }
        if let Some(list) = obj.get("removed").and_then(Value::as_array) {
            for (i, entry) in list.iter().enumerate() {
                match parse_removed(entry) {
                    Some(r) => user.removed.push(r),
                    None => notes.push(format!("removed[{i}] is not a removal; skipped")),
                }
            }
        }
        if let Some(roles) = obj.get("roles").and_then(Value::as_object) {
            for (id, v) in roles {
                match v.as_str() {
                    Some(mods) => {
                        user.roles.insert(id.clone(), mods.to_string());
                    }
                    None => notes.push(format!("role `{id}` is not a modifier set; skipped")),
                }
            }
        }
        Ok((user, notes))
    }

    /// The canonical text of the file: sorted, canonical chords, a trailing newline.
    pub fn to_json(&self) -> String {
        let mut bindings: Vec<BindingDef> = self
            .bindings
            .iter()
            .map(|b| BindingDef {
                chords: b.chords.iter().map(|c| canon(c)).collect(),
                os: None,
                ..b.clone()
            })
            .collect();
        bindings
            .sort_by(|a, b| (&a.command, &a.arg, a.context).cmp(&(&b.command, &b.arg, b.context)));
        let mut removed: Vec<RemovedDef> = self
            .removed
            .iter()
            .map(|r| RemovedDef {
                chord: canon(&r.chord),
                ..r.clone()
            })
            .collect();
        removed.sort_by(|a, b| {
            (&a.command, &a.arg, a.context, &a.chord)
                .cmp(&(&b.command, &b.arg, b.context, &b.chord))
        });
        let out = FileOut {
            version: USER_KEYMAP_VERSION,
            profile: self.profile.map(Profile::name),
            bindings,
            removed,
            roles: &self.roles,
        };
        // Plain data with string keys: serialization cannot fail.
        let mut text = serde_json::to_string_pretty(&out).expect("keymap serializes");
        text.push('\n');
        text
    }

    /// Whether the diff changes nothing (the chosen profile is not part of the diff).
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.removed.is_empty() && self.roles.is_empty()
    }

    /// The profile's bindings with this diff applied (module docs). Chords are not checked
    /// here; [`Self::build`] does that.
    pub fn effective_defs(&self, profile_defs: &[BindingDef]) -> Vec<BindingDef> {
        let mut defs: Vec<BindingDef> = profile_defs.to_vec();
        for r in &self.removed {
            let Ok(chord) = Chord::parse(&r.chord) else {
                continue;
            };
            for d in defs
                .iter_mut()
                .filter(|d| d.command == r.command && d.arg == r.arg)
                .filter(|d| r.context.is_none_or(|c| c == d.context))
            {
                let keep = |texts: &mut Vec<String>| {
                    texts.retain(|t| Chord::parse(t) != Ok(chord));
                };
                keep(&mut d.chords);
                if let Some(os) = &mut d.os {
                    os.values_mut().for_each(keep);
                }
            }
        }
        for b in &self.bindings {
            let existing = defs
                .iter_mut()
                .find(|d| d.command == b.command && d.arg == b.arg && d.context == b.context);
            match existing {
                Some(d) => {
                    d.chords = b.chords.clone();
                    d.os = None;
                }
                None => defs.push(BindingDef {
                    os: None,
                    ..b.clone()
                }),
            }
        }
        defs
    }

    /// Builds the keymap and the role table of `profile` on `os` with this diff; never fails
    /// (module docs).
    pub fn build(&self, profile: Profile, os: Os) -> Built {
        let base = match Keymap::bundled_defs(profile) {
            Ok(defs) => defs,
            Err(e) => unreachable!("the embedded keymap is valid: {e}"),
        };
        let registry = Registry::bundled();
        let mut report = Report::default();
        let mut clean = UserKeymap {
            profile: self.profile,
            ..UserKeymap::default()
        };
        let unknown = |command: &str, report: &mut Report| {
            if !report.unknown_commands.iter().any(|c| c == command) {
                report.unknown_commands.push(command.to_string());
            }
        };
        for b in &self.bindings {
            if !registry.validate(&b.command, b.arg.as_deref()) {
                unknown(&b.command, &mut report);
                continue;
            }
            let mut kept = Vec::new();
            for text in &b.chords {
                match Chord::parse(text) {
                    Err(_) => report.bad_chords.push((b.command.clone(), text.clone())),
                    Ok(chord) => {
                        let own_default = default_has(&base, os, b, &chord);
                        match reserved(os, b.context, &chord) {
                            Some(why) if !own_default => {
                                report
                                    .reserved
                                    .push((b.command.clone(), chord.to_string(), why));
                            }
                            _ => kept.push(chord.to_string()),
                        }
                    }
                }
            }
            if kept.is_empty() && !b.chords.is_empty() {
                continue;
            }
            clean.bindings.push(BindingDef {
                chords: kept,
                os: None,
                ..b.clone()
            });
        }
        for r in &self.removed {
            if registry.validate(&r.command, r.arg.as_deref()) {
                clean.removed.push(r.clone());
            } else {
                unknown(&r.command, &mut report);
            }
        }
        let defs = clean.effective_defs(&base);
        let keymap = match Keymap::from_defs(os, &defs) {
            Ok(k) => k,
            Err(e) => unreachable!("every chord was parsed above: {e}"),
        };
        report.conflicts = find_conflicts(&keymap);
        report.shadows = find_shadows(&keymap);
        let roles = self.build_roles(profile, os, &mut report);
        Built {
            keymap,
            roles,
            report,
        }
    }

    fn build_roles(&self, profile: Profile, os: Os, report: &mut Report) -> Roles {
        let mut roles = match profile {
            Profile::Standard => Roles::defaults(os),
            Profile::Legacy => Roles::legacy(os),
        };
        for role in Role::ALL {
            let Some(text) = self.roles.get(role.id()) else {
                continue;
            };
            match Mods::parse(text) {
                Ok(mods) if roles.clash(role, mods).is_none() => roles = roles.with(role, mods),
                _ => report.bad_roles.push((role.id().to_string(), text.clone())),
            }
        }
        roles
    }
}

/// Whether the profile's own binding of `b`'s command on `os` already has `chord` (a default
/// may keep a chord that is reserved for the OS, such as Quit on Cmd+Q).
fn default_has(base: &[BindingDef], os: Os, b: &BindingDef, chord: &Chord) -> bool {
    base.iter()
        .filter(|d| d.command == b.command && d.arg == b.arg && d.context == b.context)
        .any(|d| {
            let texts =
                d.os.as_ref()
                    .and_then(|m| m.get(os.name()))
                    .unwrap_or(&d.chords);
            texts
                .iter()
                .any(|t| Chord::parse(t).is_ok_and(|c| c.physical(os) == chord.physical(os)))
        })
}

/// What building a keymap found, for the status bar and the Keyboard tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Commands of the file this build does not know (kept in the file, ignored).
    pub unknown_commands: Vec<String>,
    /// Chords that did not parse: (command, text).
    pub bad_chords: Vec<(String, String)>,
    /// Chords the OS or the text fields own: (command, chord, why).
    pub reserved: Vec<(String, String, &'static str)>,
    /// Same chord, same context, different commands (an error).
    pub conflicts: Vec<Conflict>,
    /// A context binding over a `Global` one (a note).
    pub shadows: Vec<Shadow>,
    /// Role entries that did not parse or clash with another role: (role id, text).
    pub bad_roles: Vec<(String, String)>,
}

impl Report {
    /// No error: unknown ids and shadows are not errors.
    pub fn is_clean(&self) -> bool {
        self.bad_chords.is_empty()
            && self.reserved.is_empty()
            && self.conflicts.is_empty()
            && self.bad_roles.is_empty()
    }

    /// One line for the status bar (`None` when clean).
    pub fn summary(&self) -> Option<String> {
        if self.is_clean() {
            return None;
        }
        let registry = Registry::bundled();
        let label = |id: &str, arg: &Option<String>| {
            let base = registry.get(id).map_or(id, |d| d.label);
            match arg {
                Some(a) => format!("{base} ({a})"),
                None => base.to_string(),
            }
        };
        let mut parts = Vec::new();
        for c in &self.conflicts {
            let names: Vec<String> = c.commands.iter().map(|(id, a)| label(id, a)).collect();
            parts.push(format!(
                "{} is bound to {} in {}",
                c.chord,
                names.join(" and "),
                c.context.name()
            ));
        }
        for (command, text) in &self.bad_chords {
            parts.push(format!("`{text}` for {command} is not a chord"));
        }
        for (command, chord, why) in &self.reserved {
            parts.push(format!("{chord} for {command} is {why}"));
        }
        for (id, text) in &self.bad_roles {
            parts.push(format!("role {id}: `{text}` cannot be used"));
        }
        Some(format!("keymap.json: {}", parts.join("; ")))
    }
}

/// A keymap, its roles and what building them found.
#[derive(Debug, Clone)]
pub struct Built {
    /// The keymap (unknown commands and unusable chords left out).
    pub keymap: Keymap,
    /// The role table with the user's overrides.
    pub roles: Roles,
    /// What was found.
    pub report: Report,
}

/// Why `chord` cannot be assigned in `context` on `os`, if it cannot: the keys the OS
/// owns, Esc, and Space, Enter and Tab as plain keys in text fields.
pub fn reserved(os: Os, context: Context, chord: &Chord) -> Option<&'static str> {
    let m = chord.mods;
    if chord.key == ChordKey::Escape {
        return Some("reserved for Esc (cancel)");
    }
    if context == Context::Text
        && !m.cmd
        && !m.ctrl
        && matches!(
            chord.key,
            ChordKey::Space | ChordKey::Return | ChordKey::Tab
        )
    {
        return Some("reserved for typing in text fields");
    }
    match os {
        Os::Mac => {
            if m.cmd
                && !m.ctrl
                && !m.alt
                && !m.shift
                && matches!(chord.key, ChordKey::Char('q' | 'h' | 'm') | ChordKey::Tab)
            {
                return Some("reserved by macOS");
            }
        }
        Os::Windows | Os::Linux => {
            if m.meta {
                return Some("reserved by the operating system (Windows or Super key)");
            }
            if m.alt
                && !m.cmd
                && !m.ctrl
                && !m.shift
                && matches!(chord.key, ChordKey::F(4) | ChordKey::Tab)
            {
                return Some("reserved by the operating system");
            }
        }
    }
    None
}
