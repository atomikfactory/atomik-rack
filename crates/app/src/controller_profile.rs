//! Controller profiles: what a hardware controller's knobs, faders and buttons send, saved once
//! under the controller's MIDI port name so the controller just works the next time.
//!
//! A profile is a small JSON file in the `controllers` folder next to `settings.json`. It is
//! plain data (the setup wizard writes it, a person can edit or share it):
//!
//! ```text
//! {
//!   "format": 1,
//!   "name": "Desk Controller",
//!   "input": "USB MIDI Controller",
//!   "output": "USB MIDI Controller",
//!   "controls": [
//!     { "role": "knob", "index": 1, "message": "cc", "number": 21, "channel": 1,
//!       "mode": "absolute", "feedback": true },
//!     { "role": "fader", "index": 1, "message": "pitchbend", "channel": 1, "mode": "absolute" },
//!     { "role": "play", "message": "note", "number": 94, "channel": 1, "mode": "button" }
//!   ]
//! }
//! ```
//!
//! `role` is `knob` or `fader` (with `index` 1..=8), `master`, `play`, `stop`, `record`, `loop`,
//! `bank_left` or `bank_right`. `message` is `cc`, `cc14` (a pair, `number` is the MSB controller
//! 0..=31), `nrpn`, `note` or `pitchbend` (no `number`); `channel` counts from 1. `mode` is
//! `absolute`, `relative_twos`, `relative_offset`, `relative_sign` (encoders) or `button`.
//! `feedback` (default false) says the control accepts values back (a motorised fader, an LED
//! ring, a lit button); it only has an effect when `output` names a port.
//!
//! The knobs drive the first eight parameters of the selected device, the faders the levels of
//! the eight visible mixer strips (bank left and right move the eight), the buttons the
//! transport. A file is read strictly: a message or role this build does not know is an error
//! (never silently dropped), a file from a newer format is left alone.

use atomik_model::{MAX_PORT_NAME_CHARS, RelativeKind, SourceKind};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The format version this build writes and reads.
pub const PROFILE_FORMAT: u64 = 1;
/// The knobs a profile maps.
pub const KNOBS: u8 = 8;
/// The faders a profile maps (one per visible strip).
pub const FADERS: u8 = 8;
/// The most controls one profile holds.
pub const MAX_CONTROLS: usize = 64;
/// The longest profile name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The biggest profile file read, in bytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// The prefix of the surface driver id of a profile (`profile:desk-controller`).
pub const DRIVER_PREFIX: &str = "profile:";
/// The folder of the profiles, next to the settings file.
pub const PROFILE_FOLDER: &str = "controllers";
/// The file extension of a profile.
pub const PROFILE_EXTENSION: &str = "json";

/// What a control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Knob `n` (1-based), a remote parameter of the selected device.
    Knob(u8),
    /// Fader `n` (1-based), the level of a visible mixer strip.
    Fader(u8),
    /// The master fader.
    MasterFader,
    /// The Play key.
    Play,
    /// The Stop key.
    Stop,
    /// The Record key.
    Record,
    /// The loop switch.
    Loop,
    /// One bank of strips to the left.
    BankLeft,
    /// One bank of strips to the right.
    BankRight,
}

impl Role {
    /// True for the roles a button plays (everything but knobs and faders).
    pub fn is_button(self) -> bool {
        !matches!(self, Role::Knob(_) | Role::Fader(_) | Role::MasterFader)
    }

    /// The name saved in files.
    pub fn id(self) -> &'static str {
        match self {
            Role::Knob(_) => "knob",
            Role::Fader(_) => "fader",
            Role::MasterFader => "master",
            Role::Play => "play",
            Role::Stop => "stop",
            Role::Record => "record",
            Role::Loop => "loop",
            Role::BankLeft => "bank_left",
            Role::BankRight => "bank_right",
        }
    }

    /// The index saved in files (knobs and faders only).
    fn index(self) -> Option<u8> {
        match self {
            Role::Knob(n) | Role::Fader(n) => Some(n),
            _ => None,
        }
    }

    /// The role saved under `id` (with `index` for knobs and faders).
    fn from_parts(id: &str, index: Option<u64>) -> Result<Self, ProfileError> {
        let numbered = |limit: u8, make: fn(u8) -> Role| {
            let n = index.ok_or_else(|| ProfileError::Invalid(format!("a {id} needs an index")))?;
            if n == 0 || n > u64::from(limit) {
                return Err(ProfileError::Invalid(format!(
                    "the {id} index {n} is not in 1..={limit}"
                )));
            }
            Ok(make(n as u8))
        };
        match id {
            "knob" => numbered(KNOBS, Role::Knob),
            "fader" => numbered(FADERS, Role::Fader),
            "master" => Ok(Role::MasterFader),
            "play" => Ok(Role::Play),
            "stop" => Ok(Role::Stop),
            "record" => Ok(Role::Record),
            "loop" => Ok(Role::Loop),
            "bank_left" => Ok(Role::BankLeft),
            "bank_right" => Ok(Role::BankRight),
            other => Err(ProfileError::Invalid(format!("unknown role {other:?}"))),
        }
    }

    /// The words the wizard shows (`Knob 3`, `Play`).
    pub fn label(self) -> String {
        match self {
            Role::Knob(n) => format!("Knob {n}"),
            Role::Fader(n) => format!("Fader {n}"),
            Role::MasterFader => "Master fader".to_owned(),
            Role::Play => "Play".to_owned(),
            Role::Stop => "Stop".to_owned(),
            Role::Record => "Record".to_owned(),
            Role::Loop => "Loop".to_owned(),
            Role::BankLeft => "Bank left".to_owned(),
            Role::BankRight => "Bank right".to_owned(),
        }
    }
}

/// How a control's messages are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMode {
    /// The message's value is the position.
    Absolute,
    /// An encoder: each message is a number of detents.
    Relative(RelativeKind),
    /// A key: a press counts, the release does not.
    Button,
}

impl ControlMode {
    /// The name saved in files.
    pub fn id(self) -> &'static str {
        match self {
            ControlMode::Absolute => "absolute",
            ControlMode::Relative(RelativeKind::TwosComplement) => "relative_twos",
            ControlMode::Relative(RelativeKind::BinaryOffset) => "relative_offset",
            ControlMode::Relative(RelativeKind::SignMagnitude) => "relative_sign",
            ControlMode::Button => "button",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "absolute" => ControlMode::Absolute,
            "relative_twos" => ControlMode::Relative(RelativeKind::TwosComplement),
            "relative_offset" => ControlMode::Relative(RelativeKind::BinaryOffset),
            "relative_sign" => ControlMode::Relative(RelativeKind::SignMagnitude),
            "button" => ControlMode::Button,
            _ => return None,
        })
    }

    /// A short phrase for the wizard (`absolute`, `relative (offset)`).
    pub fn describe(self) -> &'static str {
        match self {
            ControlMode::Absolute => "absolute",
            ControlMode::Relative(RelativeKind::TwosComplement) => "relative (two's complement)",
            ControlMode::Relative(RelativeKind::BinaryOffset) => "relative (offset)",
            ControlMode::Relative(RelativeKind::SignMagnitude) => "relative (sign and size)",
            ControlMode::Button => "button",
        }
    }
}

/// One control of a profile: the message it sends and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlSpec {
    /// What the control does.
    pub role: Role,
    /// The kind of message and its number.
    pub source: SourceKind,
    /// The MIDI channel, 0..=15.
    pub channel: u8,
    /// How the messages are read.
    pub mode: ControlMode,
    /// The control accepts values back (needs the profile's output port).
    pub feedback: bool,
}

impl ControlSpec {
    /// True when `self` and `other` listen to the same message.
    pub fn same_message(&self, other: &ControlSpec) -> bool {
        self.source == other.source && self.channel == other.channel
    }
}

/// A controller as the wizard learned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerProfile {
    /// The name the user sees; its slug names the file.
    pub name: String,
    /// The MIDI input port the controller was set up on (how it is remembered).
    pub input: String,
    /// The MIDI output port for the feedback, when there is one.
    pub output: Option<String>,
    /// The controls, in the order the wizard learned them.
    pub controls: Vec<ControlSpec>,
}

/// Why a profile was not used.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// The text is not a profile this build understands.
    #[error("{0}")]
    Invalid(String),
    /// Written by a newer version.
    #[error("written by a newer version ({0})")]
    Newer(u64),
    /// The file could not be read or written.
    #[error("{0}")]
    Io(String),
}

impl ControllerProfile {
    /// The control that does `role`, if the profile has one.
    pub fn control(&self, role: Role) -> Option<&ControlSpec> {
        self.controls.iter().find(|c| c.role == role)
    }

    /// The surface driver id of this profile (`profile:desk-controller`).
    pub fn driver_id(&self) -> String {
        format!("{DRIVER_PREFIX}{}", slug(&self.name))
    }

    /// Checks the profile the way a file is checked.
    ///
    /// # Errors
    /// [`ProfileError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let bad = |m: String| Err(ProfileError::Invalid(m));
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_CHARS {
            return bad(format!("the name must be 1..={MAX_NAME_CHARS} characters"));
        }
        if slug(&self.name).is_empty() {
            return bad("the name needs a letter or a digit".to_owned());
        }
        if self.input.is_empty() || self.input.chars().count() > MAX_PORT_NAME_CHARS {
            return bad(format!(
                "the input port name must be 1..={MAX_PORT_NAME_CHARS} characters"
            ));
        }
        if self
            .output
            .as_ref()
            .is_some_and(|o| o.is_empty() || o.chars().count() > MAX_PORT_NAME_CHARS)
        {
            return bad(format!(
                "the output port name must be 1..={MAX_PORT_NAME_CHARS} characters"
            ));
        }
        if self.controls.len() > MAX_CONTROLS {
            return bad(format!("more than {MAX_CONTROLS} controls"));
        }
        for (i, c) in self.controls.iter().enumerate() {
            check_control(c)?;
            if let Some(earlier) = self.controls[..i].iter().find(|o| o.role == c.role) {
                return bad(format!("{} appears twice", earlier.role.label()));
            }
            if let Some(other) = self.controls[..i].iter().find(|o| o.same_message(c)) {
                return bad(format!(
                    "{} and {} listen to the same message",
                    other.role.label(),
                    c.role.label()
                ));
            }
        }
        Ok(())
    }

    /// The file form (see the module doc).
    pub fn to_json(&self) -> String {
        let controls: Vec<Value> = self.controls.iter().map(control_json).collect();
        let mut root = Map::new();
        root.insert("format".into(), json!(PROFILE_FORMAT));
        root.insert("name".into(), json!(self.name));
        root.insert("input".into(), json!(self.input));
        if let Some(out) = &self.output {
            root.insert("output".into(), json!(out));
        }
        root.insert("controls".into(), Value::Array(controls));
        let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads the file form.
    ///
    /// # Errors
    /// [`ProfileError::Invalid`] for anything this build does not understand,
    /// [`ProfileError::Newer`] for a later format.
    pub fn from_json(text: &str) -> Result<Self, ProfileError> {
        let bad = |m: &str| ProfileError::Invalid(m.to_owned());
        let root: Value =
            serde_json::from_str(text).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        let format = root
            .get("format")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("no format"))?;
        if format > PROFILE_FORMAT {
            return Err(ProfileError::Newer(format));
        }
        let text_of = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_owned);
        let list = root
            .get("controls")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("no controls"))?;
        if list.len() > MAX_CONTROLS {
            return Err(ProfileError::Invalid(format!(
                "more than {MAX_CONTROLS} controls"
            )));
        }
        let controls = list
            .iter()
            .map(control_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let profile = ControllerProfile {
            name: text_of("name").ok_or_else(|| bad("no name"))?,
            input: text_of("input").ok_or_else(|| bad("no input port"))?,
            output: text_of("output"),
            controls,
        };
        profile.validate()?;
        Ok(profile)
    }
}

fn check_control(c: &ControlSpec) -> Result<(), ProfileError> {
    let bad = |m: String| Err(ProfileError::Invalid(format!("{}: {m}", c.role.label())));
    if c.channel > 15 {
        return bad("the channel must be 1..=16".to_owned());
    }
    let (limit, name) = match c.source {
        SourceKind::Cc(n) => (127, ("cc", n as u16)),
        SourceKind::Cc14(n) => (31, ("cc14", u16::from(n))),
        SourceKind::Note(n) => (127, ("note", u16::from(n))),
        SourceKind::Nrpn(n) => (16_383, ("nrpn", n)),
        SourceKind::PitchBend => (0, ("pitchbend", 0)),
        SourceKind::Rpn(_) => return bad("RPN controls are not supported".to_owned()),
    };
    if name.1 > limit {
        return bad(format!("the {} number {} is over {limit}", name.0, name.1));
    }
    let message_ok = match (c.source, c.mode) {
        (SourceKind::Note(_), ControlMode::Button) => true,
        (SourceKind::Note(_), _) => false,
        (SourceKind::PitchBend, ControlMode::Absolute) => true,
        (SourceKind::PitchBend, _) => false,
        (SourceKind::Cc14(_), ControlMode::Absolute) => true,
        (SourceKind::Cc14(_), _) => false,
        _ => true,
    };
    if !message_ok {
        return bad(format!(
            "a {} cannot be read as {}",
            name.0,
            c.mode.describe()
        ));
    }
    let want_button = c.role.is_button();
    if want_button != (c.mode == ControlMode::Button) {
        return bad(if want_button {
            "a key must be read as a button".to_owned()
        } else {
            "a knob or fader cannot be read as a button".to_owned()
        });
    }
    Ok(())
}

fn control_json(c: &ControlSpec) -> Value {
    let mut m = Map::new();
    m.insert("role".into(), json!(c.role.id()));
    if let Some(i) = c.role.index() {
        m.insert("index".into(), json!(i));
    }
    let (message, number) = match c.source {
        SourceKind::Cc(n) => ("cc", Some(u32::from(n))),
        SourceKind::Cc14(n) => ("cc14", Some(u32::from(n))),
        SourceKind::Note(n) => ("note", Some(u32::from(n))),
        SourceKind::Nrpn(n) => ("nrpn", Some(u32::from(n))),
        SourceKind::Rpn(n) => ("rpn", Some(u32::from(n))),
        SourceKind::PitchBend => ("pitchbend", None),
    };
    m.insert("message".into(), json!(message));
    if let Some(n) = number {
        m.insert("number".into(), json!(n));
    }
    m.insert("channel".into(), json!(u32::from(c.channel) + 1));
    m.insert("mode".into(), json!(c.mode.id()));
    if c.feedback {
        m.insert("feedback".into(), json!(true));
    }
    Value::Object(m)
}

fn control_from_json(v: &Value) -> Result<ControlSpec, ProfileError> {
    let bad = |m: String| ProfileError::Invalid(m);
    let text = |key: &str| v.get(key).and_then(Value::as_str);
    let int = |key: &str| v.get(key).and_then(Value::as_u64);
    let role_id = text("role").ok_or_else(|| bad("a control has no role".to_owned()))?;
    let role = Role::from_parts(role_id, int("index"))?;
    let message = text("message").ok_or_else(|| bad(format!("{role_id} has no message")))?;
    let number = || int("number").ok_or_else(|| bad(format!("{role_id} needs a number")));
    let ranged = |limit: u64| -> Result<u64, ProfileError> {
        let n = number()?;
        if n > limit {
            return Err(bad(format!("{role_id}: the number {n} is over {limit}")));
        }
        Ok(n)
    };
    let source = match message {
        "cc" => SourceKind::Cc(ranged(127)? as u8),
        "cc14" => SourceKind::Cc14(ranged(31)? as u8),
        "note" => SourceKind::Note(ranged(127)? as u8),
        "nrpn" => SourceKind::Nrpn(ranged(16_383)? as u16),
        "pitchbend" => SourceKind::PitchBend,
        other => return Err(bad(format!("{role_id}: unknown message {other:?}"))),
    };
    let channel = int("channel").ok_or_else(|| bad(format!("{role_id} has no channel")))?;
    if !(1..=16).contains(&channel) {
        return Err(bad(format!("{role_id}: the channel must be 1..=16")));
    }
    let mode_id = text("mode").ok_or_else(|| bad(format!("{role_id} has no mode")))?;
    let mode = ControlMode::from_id(mode_id)
        .ok_or_else(|| bad(format!("{role_id}: unknown mode {mode_id:?}")))?;
    let feedback = v.get("feedback").and_then(Value::as_bool).unwrap_or(false);
    Ok(ControlSpec {
        role,
        source,
        channel: (channel - 1) as u8,
        mode,
        feedback,
    })
}

/// The file stem of a name: lower-case letters and digits, other runs becoming one `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The profile folder next to `settings_path` (`None` when the run has no settings file).
pub fn profile_dir(settings_path: Option<&Path>) -> Option<PathBuf> {
    settings_path
        .and_then(Path::parent)
        .map(|d| d.join(PROFILE_FOLDER))
}

/// The slug a surface driver id names (`profile:desk` gives `desk`).
pub fn slug_of_driver(id: &str) -> Option<&str> {
    id.strip_prefix(DRIVER_PREFIX).filter(|s| !s.is_empty())
}

fn file_of(dir: &Path, slug: &str) -> PathBuf {
    dir.join(format!("{slug}.{PROFILE_EXTENSION}"))
}

/// Writes `profile` into `dir` (replacing the file of the same name) and returns the path.
///
/// # Errors
/// An invalid profile or the I/O error.
pub fn save(dir: &Path, profile: &ControllerProfile) -> Result<PathBuf, ProfileError> {
    profile.validate()?;
    let path = file_of(dir, &slug(&profile.name));
    crate::recovery::atomic_write(&path, profile.to_json().as_bytes())
        .map_err(|e| ProfileError::Io(format!("{}: {e}", path.display())))?;
    Ok(path)
}

fn read_limited(path: &Path) -> Result<String, ProfileError> {
    let io = |e: std::io::Error| ProfileError::Io(format!("{}: {e}", path.display()));
    let len = std::fs::metadata(path).map_err(io)?.len();
    if len > MAX_FILE_BYTES {
        return Err(ProfileError::Invalid(format!(
            "the file is over {MAX_FILE_BYTES} bytes"
        )));
    }
    std::fs::read_to_string(path).map_err(io)
}

/// Reads the profile `slug` of `dir`.
///
/// # Errors
/// [`ProfileError`] when it is missing or not a profile.
pub fn load(dir: &Path, slug: &str) -> Result<ControllerProfile, ProfileError> {
    ControllerProfile::from_json(&read_limited(&file_of(dir, slug))?)
}

/// Every profile of `dir`, by file name; a file that is not a profile is listed with its error.
pub fn list(dir: &Path) -> Vec<(String, Result<ControllerProfile, ProfileError>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Result<ControllerProfile, ProfileError>)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            (path.extension().and_then(|x| x.to_str()) == Some(PROFILE_EXTENSION)).then(|| {
                (
                    stem,
                    read_limited(&path).and_then(|t| ControllerProfile::from_json(&t)),
                )
            })
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The profile of `dir` that was set up on the input port `port`: an exact name, else the one
/// profile whose port name contains or is contained in it (ports are often renamed with an index
/// when plugged in elsewhere). `None` when none or several fit.
pub fn for_port(dir: &Path, port: &str) -> Option<ControllerProfile> {
    let profiles: Vec<ControllerProfile> =
        list(dir).into_iter().filter_map(|(_, p)| p.ok()).collect();
    if let Some(p) = profiles.iter().find(|p| p.input == port) {
        return Some(p.clone());
    }
    let near = |p: &&ControllerProfile| port.contains(&p.input) || p.input.contains(port);
    let mut near_ones = profiles.iter().filter(near);
    match (near_ones.next(), near_ones.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

/// Removes the profile `slug` from `dir` (a missing file is fine).
///
/// # Errors
/// The I/O error.
pub fn delete(dir: &Path, slug: &str) -> Result<(), ProfileError> {
    let path = file_of(dir, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ProfileError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Writes a copy of `profile` to `path` for sharing.
///
/// # Errors
/// An invalid profile or the I/O error.
pub fn export(profile: &ControllerProfile, path: &Path) -> Result<(), ProfileError> {
    profile.validate()?;
    crate::recovery::atomic_write(path, profile.to_json().as_bytes())
        .map_err(|e| ProfileError::Io(format!("{}: {e}", path.display())))
}

/// Reads the profile file `path` and saves it into `dir` under a name that is not taken (`Name`,
/// then `Name 2`, `Name 3`...). Returns the saved profile.
///
/// # Errors
/// [`ProfileError`] when the file is not a profile or cannot be saved.
pub fn import(dir: &Path, path: &Path) -> Result<ControllerProfile, ProfileError> {
    let mut profile = ControllerProfile::from_json(&read_limited(path)?)?;
    let base = profile.name.clone();
    let mut n = 1;
    while file_of(dir, &slug(&profile.name)).exists() {
        n += 1;
        let suffix = format!(" {n}");
        let keep = MAX_NAME_CHARS - suffix.chars().count();
        profile.name = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
    }
    save(dir, &profile)?;
    Ok(profile)
}

#[cfg(test)]
#[path = "controller_profile_tests.rs"]
mod tests;
