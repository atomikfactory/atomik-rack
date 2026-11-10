//! What the child learns about a panic of the plugin on its audio thread, so that the parent can
//! put it in the app log: the message, where it was raised, which operation it interrupted and a
//! short backtrace of our own frames.
//!
//! A panic hook (it keeps the default print to stderr) notes message, location and backtrace in
//! a slot of the panicking thread; the audio thread, which caught the panic, takes the note and
//! turns it into log lines. Nothing here runs unless a panic happens.

use atomik_plugin_ipc::{AudioOp, MAX_LOG_BYTES, PANIC_LOG_PREFIX};
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::panic::PanicHookInfo;
use std::sync::Once;

/// The name of the audio thread: only its panics are noted.
pub const AUDIO_THREAD_NAME: &str = "host-audio";

/// Most frames of the backtrace that reach the log.
const MAX_FRAMES: usize = 15;

/// What the hook noted.
struct Note {
    message: String,
    location: String,
    backtrace: Option<String>,
}

thread_local! {
    static NOTE: RefCell<Option<Note>> = const { RefCell::new(None) };
}

/// Installs the hook, once per process, in front of the one that is installed.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(AUDIO_THREAD_NAME) {
                note(info);
            }
            previous(info);
        }));
    });
}

fn note(info: &PanicHookInfo<'_>) {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no text".to_string());
    let location = info.location().map_or_else(
        || "an unknown place".to_string(),
        |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
    );
    let backtrace = our_frames(&Backtrace::force_capture().to_string());
    let note = Note {
        message,
        location,
        backtrace,
    };
    // A thread that is already gone from its slot (during its end) has nothing to note.
    let _ = NOTE.try_with(|slot| *slot.borrow_mut() = Some(note));
}

/// The frames of `raw` (a backtrace as std prints it) that belong to our crates, at most
/// [`MAX_FRAMES`], as `name (file:line)` joined with ` <- `. `None` when there is none.
fn our_frames(raw: &str) -> Option<String> {
    let mut frames: Vec<String> = Vec::new();
    let mut keep = false;
    for line in raw.lines() {
        let t = line.trim();
        if let Some((n, name)) = t.split_once(": ")
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            keep = name.trim_start_matches('<').starts_with("atomik_")
                && !name.contains("panic_report");
            if keep {
                frames.push(name.to_string());
            }
        } else if keep
            && let Some(at) = t.strip_prefix("at ")
            && let Some(last) = frames.last_mut()
        {
            last.push_str(&format!(" ({at})"));
        }
    }
    frames.truncate(MAX_FRAMES);
    (!frames.is_empty()).then(|| frames.join(" <- "))
}

/// The log lines for the panic the audio thread just caught during `op`: the panic itself, then
/// (when there is one) its backtrace. Takes the note, so a second call finds none.
pub fn take(op: AudioOp) -> Vec<String> {
    let note = NOTE
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten();
    let mut lines = Vec::new();
    match note {
        Some(n) => {
            lines.push(headline(op, &n.message, &n.location));
            if let Some(bt) = n.backtrace {
                lines.push(format!("backtrace of the panic (our frames): {bt}"));
            }
        }
        None => lines.push(format!("{PANIC_LOG_PREFIX}{op:?}: no message was captured")),
    }
    lines.into_iter().map(cut).collect()
}

/// The first line: `panicked in the plugin host during <op>: <message> at <file>:<line>:<col>`.
pub fn headline(op: AudioOp, message: &str, location: &str) -> String {
    format!("{PANIC_LOG_PREFIX}{op:?}: {message} at {location}")
}

/// `text` cut to what the parent accepts in a log line, at a character boundary.
fn cut(mut text: String) -> String {
    if text.len() > MAX_LOG_BYTES {
        let mut end = MAX_LOG_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_frames_survive_with_their_places_and_at_most_fifteen() {
        let raw = "   0: std::panicking::begin_panic\n             at /rustc/x/library/std/src/panicking.rs:1:1\n   1: atomik_plugin_hostd::audio::answer\n             at ./crates/plugin-hostd/src/audio.rs:10:5\n   2: core::ops::function::FnOnce::call_once\n   3: atomik_core::thing\n";
        assert_eq!(
            our_frames(raw).as_deref(),
            Some(
                "atomik_plugin_hostd::audio::answer (./crates/plugin-hostd/src/audio.rs:10:5) <- atomik_core::thing"
            )
        );
        let many: String = (0..40)
            .map(|i| format!("{i:4}: atomik_x::f{i}\n"))
            .collect();
        assert_eq!(
            our_frames(&many).unwrap().matches(" <- ").count(),
            MAX_FRAMES - 1
        );
        assert_eq!(our_frames("   0: std::rt::lang_start\n"), None);
    }

    #[test]
    fn the_headline_names_the_operation_the_message_and_the_place() {
        assert_eq!(
            headline(AudioOp::Process, "boom", "src/a.rs:3:9"),
            "panicked in the plugin host during Process: boom at src/a.rs:3:9"
        );
    }
}
