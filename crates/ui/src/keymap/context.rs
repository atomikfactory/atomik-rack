//! Contexts: where a binding applies.

use serde::{Deserialize, Serialize};

/// Where a binding applies. The shell computes a [`ContextStack`] from the focused pane and
/// the text state; the router looks the most specific context first, then `Global`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Context {
    /// Everywhere.
    Global,
    /// The rack (device plates and cables).
    Rack,
    /// The sequencer (arrangement and what shares its time axis).
    Sequencer,
    /// An editor: piano roll, step, sample, slice, sequencer-grid and container editors.
    Editor,
    /// The console.
    Mixer,
    /// The device browser with its query focused.
    Browser,
    /// Any text owner: tempo field, rename, browser filter, settings text.
    Text,
}

impl Context {
    /// Every context, `Global` first, in listing order.
    pub const ALL: [Context; 7] = [
        Context::Global,
        Context::Rack,
        Context::Sequencer,
        Context::Editor,
        Context::Mixer,
        Context::Browser,
        Context::Text,
    ];

    /// The name used in the data file and in listings.
    pub fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Rack => "rack",
            Context::Sequencer => "sequencer",
            Context::Editor => "editor",
            Context::Mixer => "mixer",
            Context::Browser => "browser",
            Context::Text => "text",
        }
    }
}

/// The contexts that are active, most specific first. `Global` is always the last entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextStack {
    contexts: Vec<Context>,
}

impl ContextStack {
    /// A stack of `specific` contexts (most specific first); `Global` is appended when absent
    /// and moved last when present.
    pub fn new(specific: &[Context]) -> Self {
        let mut contexts: Vec<Context> = Vec::new();
        for c in specific {
            if *c != Context::Global && !contexts.contains(c) {
                contexts.push(*c);
            }
        }
        contexts.push(Context::Global);
        Self { contexts }
    }

    /// Only `Global`.
    pub fn global() -> Self {
        Self::new(&[])
    }

    /// The contexts, most specific first, `Global` last.
    pub fn contexts(&self) -> &[Context] {
        &self.contexts
    }

    /// Whether a text owner is active: plain keys then go to it.
    pub fn is_text(&self) -> bool {
        self.contexts.contains(&Context::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_is_always_last_and_once() {
        let s = ContextStack::new(&[Context::Editor, Context::Global, Context::Sequencer]);
        assert_eq!(
            s.contexts(),
            [Context::Editor, Context::Sequencer, Context::Global]
        );
        assert_eq!(ContextStack::global().contexts(), [Context::Global]);
        let dup = ContextStack::new(&[Context::Rack, Context::Rack]);
        assert_eq!(dup.contexts(), [Context::Rack, Context::Global]);
    }

    #[test]
    fn text_is_detected() {
        assert!(ContextStack::new(&[Context::Text, Context::Browser]).is_text());
        assert!(!ContextStack::new(&[Context::Browser]).is_text());
    }

    #[test]
    fn names_are_the_serde_names() {
        for c in Context::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.name()));
        }
    }
}
