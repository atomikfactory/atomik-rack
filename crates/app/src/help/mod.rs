//! The long help of Help Mode: what a control does, in more words than its tooltip.
//!
//! The text lives in two data files compiled in with `include_str!`: `controls.json` (one entry
//! per stable control id: a title, one to four sentences and the manual section that goes
//! deeper) and `devices.json` (one "about" line per device type). A view says which control is
//! under the pointer with [`Help::id`]; [`resolve`] turns that [`Help`] into the card the panel
//! shows:
//!
//! - a parameter (`param:<device type>:<param key>`) is its name, its range and unit from the
//!   device descriptor, and the about line of its device;
//! - an id with an entry in `controls.json` is that entry, with the live shortcut of the command
//!   the control runs;
//! - anything else falls back to the tooltip text, split at its first colon into a title and a
//!   line, so a control without an entry still answers (the coverage test lists these).
//!
//! The manual sections are the headings of `docs/README.md`, and the "More" link of a card opens
//! the built-in manual ([`crate::manual`]) at that heading ([`Resolved::manual`]). A section the
//! manual does not hold falls back to [`section_url`], the address of the page in the browser.

use atomik_core::{ParamDescriptor, Unit};
use atomik_device_sdk::Registry as Devices;
use atomik_ui::keymap::Keymap;
use atomik_ui::{Help, HelpCard};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[cfg(test)]
mod tests;

/// The control entries, keyed by stable id.
const CONTROLS: &str = include_str!("controls.json");
/// The about line of every device type, keyed by type id.
const DEVICES: &str = include_str!("devices.json");

/// The prefix of the id of a parameter control.
pub const PARAM_PREFIX: &str = "param:";

/// One entry of `controls.json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Entry {
    /// The name of the control.
    pub title: String,
    /// One to four sentences about it.
    pub text: String,
    /// The manual section that explains it more.
    #[serde(default)]
    pub more: Option<String>,
}

/// All the long help text.
#[derive(Debug, Default)]
pub struct Book {
    controls: BTreeMap<String, Entry>,
    devices: BTreeMap<String, String>,
}

impl Book {
    /// Reads the two data files.
    ///
    /// # Errors
    /// The JSON of either file is malformed (a test keeps the shipped files valid).
    pub fn parse(controls: &str, devices: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            controls: serde_json::from_str(controls)?,
            devices: serde_json::from_str(devices)?,
        })
    }

    /// The entry of control `id`.
    pub fn control(&self, id: &str) -> Option<&Entry> {
        self.controls.get(id)
    }

    /// The about line of device type `type_id`.
    pub fn about(&self, type_id: &str) -> Option<&str> {
        self.devices.get(type_id).map(String::as_str)
    }

    /// Every control id that has an entry.
    pub fn control_ids(&self) -> impl Iterator<Item = &str> {
        self.controls.keys().map(String::as_str)
    }

    /// Every device type that has an about line.
    pub fn device_ids(&self) -> impl Iterator<Item = &str> {
        self.devices.keys().map(String::as_str)
    }
}

/// The shipped help text, read once.
pub fn book() -> &'static Book {
    static BOOK: OnceLock<Book> = OnceLock::new();
    BOOK.get_or_init(|| {
        Book::parse(CONTROLS, DEVICES).unwrap_or_else(|e| {
            log::warn!("the help text could not be read: {e}");
            Book::default()
        })
    })
}

/// The address of the documentation, where the manual sections live.
macro_rules! docs {
    ($tail:literal) => {
        concat!(
            "https://github.com/atomikfactory/atomik-rack/blob/main/docs/",
            $tail
        )
    };
}

/// The manual sections the help text links to, with the address that opens each in the browser
/// (the same way Help > Documentation opens the project page). The built-in manual holds all of
/// them; the address is where a section would open if it did not.
const SECTIONS: [(&str, &str); 5] = [
    ("The main window", docs!("README.md#the-main-window")),
    ("CPU use", docs!("README.md#cpu-use")),
    ("Launcher", docs!("README.md#launcher")),
    ("Help Mode", docs!("README.md#help-mode")),
    ("Devices", docs!("DEVICES.md")),
];

/// The address of manual section `section` (`None` for a section the manual does not have).
pub fn section_url(section: &str) -> Option<&'static str> {
    SECTIONS
        .iter()
        .find(|(s, _)| *s == section)
        .map(|(_, u)| *u)
}

/// Every manual section the help text can link to.
pub fn sections() -> impl Iterator<Item = &'static str> {
    SECTIONS.iter().map(|(s, _)| *s)
}

/// What the panel shows for one control, and where its "More" link goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The card of the panel.
    pub card: HelpCard,
    /// The address the "More" link opens in the browser (`None`: it opens the manual, or the
    /// card has no link).
    pub url: Option<&'static str>,
    /// The manual heading the "More" link opens in the built-in manual.
    pub manual: Option<String>,
}

/// Where the "More" link of `section` goes: the built-in manual when it has that heading, else
/// the address of the page in the browser.
fn link_of(section: &str) -> (Option<&'static str>, Option<String>) {
    if crate::manual::manual().find_heading(section).is_some() {
        (None, Some(section.to_owned()))
    } else {
        (section_url(section), None)
    }
}

/// The card for the control `help` describes. `keymap` gives the live shortcut of its command
/// and `devices` the descriptors a parameter card reads.
pub fn resolve(help: &Help, keymap: &Keymap, devices: &Devices) -> Resolved {
    let shortcut = help.command.and_then(|c| keymap.shortcut_text(c, help.arg));
    if let Some(id) = help.id.as_deref() {
        if let Some(rest) = id.strip_prefix(PARAM_PREFIX)
            && let Some((type_id, key)) = rest.split_once(':')
            && let Some(resolved) = param_card(book(), type_id, key, help, devices)
        {
            return resolved;
        }
        if let Some(entry) = book().control(id) {
            return entry_card(entry, shortcut);
        }
    }
    fallback_card(help, shortcut)
}

fn entry_card(entry: &Entry, shortcut: Option<String>) -> Resolved {
    let mut card = HelpCard::new(entry.title.clone(), entry.text.clone()).with_shortcut(shortcut);
    let mut link = (None, None);
    if let Some(section) = &entry.more {
        link = link_of(section);
        if link.0.is_some() || link.1.is_some() {
            card = card.with_more(section.clone());
        }
    }
    Resolved {
        card,
        url: link.0,
        manual: link.1,
    }
}

/// The card of a control without an entry: the tooltip text, split at its first colon into a
/// title and a line.
fn fallback_card(help: &Help, shortcut: Option<String>) -> Resolved {
    let text = help.text.lines().next().unwrap_or_default().trim();
    let (title, line) = match text.split_once(": ") {
        Some((title, rest)) => (title.to_owned(), sentence(rest)),
        None => (text.to_owned(), String::new()),
    };
    Resolved {
        card: HelpCard::new(title, line).with_shortcut(shortcut),
        url: None,
        manual: None,
    }
}

/// `text` with a capital first letter and a closing full stop.
fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches('.');
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// The card of parameter `key` of device `type_id`: its name, what a click or drag on its kind
/// of control does, its range and unit, and the about line of the device.
fn param_card(
    book: &Book,
    type_id: &str,
    key: &str,
    help: &Help,
    devices: &Devices,
) -> Option<Resolved> {
    let desc = devices.descriptor(type_id)?;
    let param = desc.params.iter().find(|p| p.key == key)?;
    let mut sentences = Vec::new();
    // A toggle or a choice says how it is operated ("Bypass: click to switch on or off").
    if let Some((_, how)) = help.text.split_once(": ") {
        sentences.push(sentence(how));
    }
    sentences.push(range_text(param));
    if let Some(about) = book.about(type_id) {
        sentences.push(format!("{}: {about}", desc.name));
    }
    let mut card = HelpCard::new(param.name, sentences.join(" "));
    let (url, manual) = link_of("Devices");
    if url.is_some() || manual.is_some() {
        card = card.with_more("Devices");
    }
    Some(Resolved { card, url, manual })
}

/// The unit and range of a parameter in a sentence ("From 20 Hz to 20.00 kHz, default 1.00 kHz.").
pub fn range_text(p: &ParamDescriptor) -> String {
    const SHOWN: usize = 6;
    match p.unit {
        Unit::Toggle => "It is either on or off.".to_owned(),
        Unit::Choice(names) => {
            let list = names
                .iter()
                .take(SHOWN)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            if names.len() > SHOWN {
                format!("{} choices, among them {list}.", names.len())
            } else {
                format!("Choices: {list}.")
            }
        }
        _ => format!(
            "From {} to {}, default {}.",
            atomik_ui::widget::format_value(p, p.min),
            atomik_ui::widget::format_value(p, p.max),
            atomik_ui::widget::format_value(p, p.default),
        ),
    }
}
