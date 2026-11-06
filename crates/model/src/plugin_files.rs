//! Files a plugin's saved state names.
//!
//! A plugin's state is opaque to the document, but many plugins keep a text state (XML, JSON or
//! plain text) that names the files they loaded: an impulse response, a preset, a sample. A
//! project that names files outside its own folder stops working when it moves, so Collect Files
//! looks for them ([`RackDoc::plugin_file_refs`]), copies them into the project and points the
//! state at the copies ([`rewrite_paths`]).
//!
//! Only a state that is valid text is read: a binary state is never searched or changed (a path
//! inside it could not be replaced without moving what follows). A path counts when it is
//! absolute (a drive letter or a root slash), ends the way a file name does (a dot and up to
//! eight letters or digits) and sits between delimiters (quotes, angle brackets, `=` or line
//! ends); nothing else is touched.

use crate::{DeviceKey, RackDoc};
use atomik_device_sdk::PluginStateBlob;

/// The longest path read from a state, in characters.
const MAX_PATH_CHARS: usize = 1024;

/// A file one plugin's state names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFileRef {
    /// The plugin device (path key).
    pub device: DeviceKey,
    /// The text as the state holds it (a JSON state doubles its backslashes).
    pub raw: String,
    /// The path, with the doubled backslashes of `raw` made single.
    pub path: String,
}

fn is_delimiter(c: char) -> bool {
    matches!(c, '"' | '\'' | '<' | '>' | '=' | '\n' | '\r' | '\0')
}

/// Whether `text` begins like an absolute path.
fn starts_path(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(d), Some(':'), Some('\\' | '/')) => d.is_ascii_alphabetic(),
        (Some('/'), Some(c), _) => c != '/' && !c.is_whitespace(),
        _ => false,
    }
}

/// Whether `path` ends in a file name with an extension, and is deep enough to be a file (a lone
/// `/name` is more likely markup).
fn looks_like_file(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    let separators = path.chars().filter(|c| matches!(c, '\\' | '/')).count();
    let ext = name.rsplit_once('.').map(|(_, e)| e);
    separators >= 2
        && ext.is_some_and(|e| {
            (1..=8).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// The paths a state text names, as the text holds them, without repeats, in order.
fn raw_paths(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut previous: Option<char> = None;
    for (i, c) in text.char_indices() {
        if previous.is_none_or(is_delimiter) && starts_path(&text[i..]) {
            let rest = &text[i..];
            let end = rest.find(is_delimiter).unwrap_or(rest.len());
            let raw = &rest[..end];
            if raw.chars().count() <= MAX_PATH_CHARS
                && looks_like_file(&unescape(raw))
                && !found.iter().any(|f| f == raw)
            {
                found.push(raw.to_owned());
            }
        }
        previous = Some(c);
    }
    found
}

/// A JSON-style path with its doubled backslashes made single.
fn unescape(raw: &str) -> String {
    raw.replace("\\\\", "\\")
}

impl RackDoc {
    /// The files the plugin states of this document name, device by device (see the module
    /// docs). Not checked against the disk.
    pub fn plugin_file_refs(&self) -> Vec<PluginFileRef> {
        let mut out = Vec::new();
        for (device, d) in self.plugin_devices() {
            let Some(state) = d.plugin.as_ref().and_then(|p| p.state.as_ref()) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&state.0) else {
                continue;
            };
            for raw in raw_paths(text) {
                let path = unescape(&raw);
                out.push(PluginFileRef {
                    device: device.clone(),
                    raw,
                    path,
                });
            }
        }
        out
    }
}

/// The state with each path of `changes` (`raw` text of [`PluginFileRef`], new path) replaced,
/// written the way the old one was (doubled backslashes stay doubled). `None` when the state
/// is not text or none of the paths is in it.
pub fn rewrite_paths(
    state: &PluginStateBlob,
    changes: &[(String, String)],
) -> Option<PluginStateBlob> {
    let mut text = std::str::from_utf8(&state.0).ok()?.to_owned();
    let mut changed = false;
    for (raw, new) in changes {
        // Only a whole path between delimiters is replaced, never a part of a longer one.
        let doubled = raw.contains("\\\\");
        let new = if doubled {
            new.replace('\\', "\\\\")
        } else {
            new.clone()
        };
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        while let Some(found) = text[at..].find(raw.as_str()) {
            let start = at + found;
            let end = start + raw.len();
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            out.push_str(&text[at..start]);
            if before.is_none_or(is_delimiter) && after.is_none_or(is_delimiter) {
                out.push_str(&new);
                changed = true;
            } else {
                out.push_str(raw);
            }
            at = end;
        }
        out.push_str(&text[at..]);
        text = out;
    }
    changed.then(|| PluginStateBlob(text.into_bytes()))
}
