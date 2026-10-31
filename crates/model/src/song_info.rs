//! Song information: the title, author, notes, web address and e-mail the user writes about a
//! project.
//!
//! The information is project data with no part in the sound: [`RackDoc::info`]. It is empty
//! for a new project (nothing is ever filled in from the system or the user account), saved as
//! the root field `"info"` only when something is set (project format 38) and edited with
//! [`crate::Command::SetSongInfo`], one undo step that reports [`crate::SyncOp::ViewChanged`]
//! (the window title shows it) and never touches the engine.
//!
//! # Limits
//!
//! A text is stored as typed (nothing is trimmed, so a save, a load and a save give the same
//! bytes). The title, author, web address and e-mail are one line of at most
//! [`MAX_INFO_TITLE_CHARS`], [`MAX_INFO_AUTHOR_CHARS`], [`MAX_INFO_URL_CHARS`] and
//! [`MAX_INFO_EMAIL_CHARS`] characters; the notes may have several lines (`\n`, and tabs) and
//! hold at most [`MAX_INFO_NOTES_CHARS`] characters. No text holds any other control character.
//! The loader and the command enforce the same rules; the loader names the JSON path of the field
//! that breaks one.

use crate::history::Edit;
use crate::patch::{JsonPath, json_str, object, string};
use crate::{ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;

/// Most characters of the title.
pub const MAX_INFO_TITLE_CHARS: usize = 120;
/// Most characters of the author.
pub const MAX_INFO_AUTHOR_CHARS: usize = 120;
/// Most characters of the web address.
pub const MAX_INFO_URL_CHARS: usize = 500;
/// Most characters of the e-mail address.
pub const MAX_INFO_EMAIL_CHARS: usize = 254;
/// Most characters of the notes.
pub const MAX_INFO_NOTES_CHARS: usize = 10_000;

const FIELDS: &[&str] = &["title", "author", "email", "url", "notes"];

/// What the project says about itself. Every text may be empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SongInfo {
    /// The song's title (shown in the window title).
    pub title: String,
    /// Who made it.
    pub author: String,
    /// A free text, several lines.
    pub notes: String,
    /// A web address.
    pub url: String,
    /// An e-mail address.
    pub email: String,
}

impl SongInfo {
    /// True when no text is set (such an info is not saved).
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Checks the limits (see the module docs); the error names the first text that breaks one.
    ///
    /// # Errors
    /// [`ModelError::OutOfRange`] naming the field.
    pub fn check(&self) -> Result<(), ModelError> {
        for (name, text) in self.texts() {
            check_text(name, text).map_err(ModelError::OutOfRange)?;
        }
        Ok(())
    }

    fn texts(&self) -> [(&'static str, &str); 5] {
        [
            ("title", &self.title),
            ("author", &self.author),
            ("email", &self.email),
            ("url", &self.url),
            ("notes", &self.notes),
        ]
    }
}

fn limit_of(name: &str) -> usize {
    match name {
        "title" => MAX_INFO_TITLE_CHARS,
        "author" => MAX_INFO_AUTHOR_CHARS,
        "email" => MAX_INFO_EMAIL_CHARS,
        "url" => MAX_INFO_URL_CHARS,
        _ => MAX_INFO_NOTES_CHARS,
    }
}

/// Checks one text of the info against the rules of its field.
fn check_text(name: &str, text: &str) -> Result<(), String> {
    let limit = limit_of(name);
    let chars = text.chars().count();
    if chars > limit {
        return Err(format!(
            "the song {name} is {chars} characters, at most {limit} are allowed"
        ));
    }
    let multiline = name == "notes";
    if text
        .chars()
        .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t')))
    {
        return Err(format!(
            "the song {name} must not hold control characters{}",
            if multiline {
                " other than new lines and tabs"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

/// Plans `Command::SetSongInfo`: the new info must be within the limits and differ from the
/// current one.
pub(crate) fn plan_set(doc: &RackDoc, info: &SongInfo) -> Result<Edit, ModelError> {
    info.check()?;
    if &doc.info == info {
        return Err(ModelError::BadScope(
            "the song information is already that".into(),
        ));
    }
    Ok(Edit::Info {
        old: doc.info.clone(),
        new: info.clone(),
    })
}

/// Writes `"info"` (with its trailing comma and newline) after `"macros"`; nothing for an empty
/// info.
pub(crate) fn write_info(w: &mut String, info: &SongInfo) {
    if info.is_empty() {
        return;
    }
    let rows: Vec<String> = info
        .texts()
        .iter()
        .filter(|(_, text)| !text.is_empty())
        .map(|(name, text)| format!("    \"{name}\": {}", json_str(text)))
        .collect();
    let _ = writeln!(w, "  \"info\": {{\n{}\n  }},", rows.join(",\n"));
}

/// Reads the project's `"info"` object (see the module documentation). A text that is present is
/// kept as written, also when it is empty.
pub(crate) fn load_info(v: &Value, path: &JsonPath) -> Result<SongInfo, ModelError> {
    let o = object(v, path, FIELDS, &[])?;
    let mut info = SongInfo::default();
    for name in FIELDS {
        let Some(value) = o.get(*name) else {
            continue;
        };
        let p = path.field(name);
        let text = string(value, &p)?;
        check_text(name, text).map_err(|message| p.error(message))?;
        let slot = match *name {
            "title" => &mut info.title,
            "author" => &mut info.author,
            "email" => &mut info.email,
            "url" => &mut info.url,
            _ => &mut info.notes,
        };
        *slot = text.to_owned();
    }
    Ok(info)
}
