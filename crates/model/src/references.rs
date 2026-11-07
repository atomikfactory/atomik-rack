//! Reference tracks: the audio files a song is compared against.
//!
//! [`RackDoc::references`] lists up to [`MAX_REFERENCES`] files, each a path (a project-relative
//! path once Collect Files has copied the file next to the project, otherwise the path the user
//! picked) and an optional **cue**: the position in the file, in milliseconds, the reference
//! starts from when the user does not want it to follow the song position. The list is project
//! data with no part in the sound (a reference plays after the master chain, live only, and is
//! never part of a bounce or an export). It is empty for a new project, saved as the root field
//! `"references"` only when it holds an entry (project format 44, ignorable) and edited with
//! [`crate::Command::SetReferences`], one undo step that reports [`crate::SyncOp::ViewChanged`].
//!
//! # Limits
//!
//! A path is one line of 1 to [`MAX_REFERENCE_PATH_CHARS`] characters without control
//! characters; a cue is at most [`MAX_REFERENCE_CUE_MS`] (24 hours). The same path may not be
//! listed twice. The loader and the command enforce the same rules.

use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, array, json_str, object, string, uint};
use crate::{ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;

/// Most reference tracks a song keeps.
pub const MAX_REFERENCES: usize = 16;
/// Most characters of a reference path.
pub const MAX_REFERENCE_PATH_CHARS: usize = 1_000;
/// Largest cue in milliseconds (24 hours).
pub const MAX_REFERENCE_CUE_MS: u64 = 86_400_000;

const FIELDS: &[&str] = &["path", "cue_ms"];

/// One reference track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The audio file.
    pub path: String,
    /// Where the reference starts from, in milliseconds (`None`: it follows the song position).
    pub cue_ms: Option<u64>,
}

impl Reference {
    /// A reference that follows the song position.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            cue_ms: None,
        }
    }

    /// The file name without its folder, for lists.
    pub fn name(&self) -> &str {
        self.path
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(&self.path)
    }
}

/// Checks a list against the limits; the error names the first entry that breaks one.
///
/// # Errors
/// [`ModelError::OutOfRange`] naming the entry.
pub fn check_references(list: &[Reference]) -> Result<(), ModelError> {
    if list.len() > MAX_REFERENCES {
        return Err(ModelError::OutOfRange(format!(
            "{} reference tracks, at most {MAX_REFERENCES} are allowed",
            list.len()
        )));
    }
    for (i, r) in list.iter().enumerate() {
        check_one(r).map_err(|m| ModelError::OutOfRange(format!("reference {}: {m}", i + 1)))?;
        if list[..i].iter().any(|p| p.path == r.path) {
            return Err(ModelError::OutOfRange(format!(
                "reference {}: the file is already listed",
                i + 1
            )));
        }
    }
    Ok(())
}

fn check_one(r: &Reference) -> Result<(), String> {
    let chars = r.path.chars().count();
    if chars == 0 {
        return Err("the path is empty".into());
    }
    if chars > MAX_REFERENCE_PATH_CHARS {
        return Err(format!(
            "the path is {chars} characters, at most {MAX_REFERENCE_PATH_CHARS} are allowed"
        ));
    }
    if r.path.chars().any(char::is_control) {
        return Err("the path must not hold control characters".into());
    }
    if r.cue_ms.is_some_and(|c| c > MAX_REFERENCE_CUE_MS) {
        return Err(format!("the cue is beyond {MAX_REFERENCE_CUE_MS} ms"));
    }
    Ok(())
}

/// Plans `Command::SetReferences`: the list must be within the limits and differ from the
/// current one.
pub(crate) fn plan_set(doc: &RackDoc, list: &[Reference]) -> Result<Edit, ModelError> {
    check_references(list)?;
    if doc.references == list {
        return Err(ModelError::BadScope(
            "the reference tracks are already that".into(),
        ));
    }
    Ok(Edit::Swap(Swap::References {
        old: doc.references.clone(),
        new: list.to_vec(),
    }))
}

/// Writes `"references"` (with its trailing comma and newline) after `"rack_sort"`'s place;
/// nothing for an empty list.
pub(crate) fn write_references(w: &mut String, list: &[Reference]) {
    if list.is_empty() {
        return;
    }
    let rows: Vec<String> = list
        .iter()
        .map(|r| match r.cue_ms {
            Some(c) => format!("    {{\"path\": {}, \"cue_ms\": {c}}}", json_str(&r.path)),
            None => format!("    {{\"path\": {}}}", json_str(&r.path)),
        })
        .collect();
    let _ = writeln!(w, "  \"references\": [\n{}\n  ],", rows.join(",\n"));
}

/// Reads the project's `"references"` array.
pub(crate) fn load_references(v: &Value, path: &JsonPath) -> Result<Vec<Reference>, ModelError> {
    let items = array(v, path)?;
    let mut list = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let o = object(item, &p, FIELDS, &["path"])?;
        let file = string(&o["path"], &p.field("path"))?;
        let cue_ms = match o.get("cue_ms") {
            Some(c) => Some(uint(c, &p.field("cue_ms"), MAX_REFERENCE_CUE_MS)?),
            None => None,
        };
        let r = Reference {
            path: file.to_owned(),
            cue_ms,
        };
        check_one(&r).map_err(|m| p.error(m))?;
        list.push(r);
    }
    check_references(&list).map_err(|e| path.error(e.to_string()))?;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_last_path_part() {
        assert_eq!(Reference::new("music\\mixes\\song.wav").name(), "song.wav");
        assert_eq!(Reference::new("a/b/song.wav").name(), "song.wav");
        assert_eq!(Reference::new("song.wav").name(), "song.wav");
    }

    #[test]
    fn limits_are_enforced() {
        assert!(check_references(&[Reference::new("")]).is_err());
        assert!(check_references(&[Reference::new("a\nb")]).is_err());
        let dup = [Reference::new("a.wav"), Reference::new("a.wav")];
        assert!(check_references(&dup).is_err());
        let many: Vec<Reference> = (0..=MAX_REFERENCES)
            .map(|i| Reference::new(format!("{i}.wav")))
            .collect();
        assert!(check_references(&many).is_err());
        assert!(check_references(&many[..MAX_REFERENCES]).is_ok());
        let far = Reference {
            path: "a.wav".into(),
            cue_ms: Some(MAX_REFERENCE_CUE_MS + 1),
        };
        assert!(check_references(&[far]).is_err());
    }

    #[test]
    fn writing_and_reading_round_trip() {
        let list = vec![
            Reference::new("ref one.wav"),
            Reference {
                path: "dir/ref \"two\".wav".into(),
                cue_ms: Some(61_500),
            },
        ];
        let mut text = String::from("{\n");
        write_references(&mut text, &list);
        text.push_str("  \"end\": 0\n}");
        let value: Value = serde_json::from_str(&text).unwrap();
        let back = load_references(&value["references"], &JsonPath::lazy()).unwrap();
        assert_eq!(back, list);
        let mut empty = String::new();
        write_references(&mut empty, &[]);
        assert!(empty.is_empty());
    }
}
