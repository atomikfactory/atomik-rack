//! The start-up timeline: how long after the process began each stage of start-up was reached,
//! and which graphics adapter the renderer uses.
//!
//! The live app calls [`init`] first thing in `main`; each stage then calls [`mark`] and the
//! first call per stage writes one INFO line to the log file, `startup 412 ms: first frame
//! presented`. Without [`init`] (the tests, the headless QA server, the offline tools) [`mark`]
//! does nothing. The adapter is written once as `graphics adapter: <name> · <backend> · <type> ·
//! <driver>` ([`adapter_line`]) and kept for Help > Export Diagnostics ([`adapter`]).
//!
//! Wall time is read here for diagnostics only; nothing reaches the audio thread, the engine or
//! any rendered sample.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The stage reached when the application state is built and the fonts are about to be read.
pub const STATE_BUILT: &str = "application state built";
/// The stage reached when the fonts of the machine are read.
pub const FONTS_READY: &str = "fonts read";
/// The stage reached when the event loop of the system exists.
pub const EVENT_LOOP_READY: &str = "event loop created";
/// The stage reached when the graphics adapters of the machine are listed.
pub const ADAPTERS_LISTED: &str = "graphics adapters listed";
/// The stage reached when the first window exists.
pub const FIRST_WINDOW: &str = "first window created";
/// The stage reached when the graphics device was made (the adapter was picked before).
pub const DEVICE_CREATED: &str = "graphics device created";
/// The stage reached when the graphics adapter was picked.
pub const ADAPTER_CHOSEN: &str = "graphics adapter chosen";
/// The stage reached when the device and the renderer are ready.
pub const RENDERER_READY: &str = "device and renderer ready";
/// The stage reached when the first frame was shown.
pub const FIRST_FRAME: &str = "first frame presented";
/// The stage reached when the audio stream runs.
pub const AUDIO_RUNNING: &str = "audio stream running";

/// The stages already written, so each is written once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timeline {
    done: Vec<&'static str>,
}

impl Timeline {
    /// The log line for `stage` reached `elapsed` after the start, or `None` when the stage was
    /// written before.
    pub fn mark(&mut self, stage: &'static str, elapsed: Duration) -> Option<String> {
        if self.done.contains(&stage) {
            return None;
        }
        self.done.push(stage);
        Some(line(elapsed, stage))
    }
}

/// `startup 412 ms: first frame presented`.
#[must_use]
pub fn line(elapsed: Duration, stage: &str) -> String {
    format!("startup {} ms: {stage}", elapsed.as_millis())
}

/// `graphics adapter: <name> · <backend> · <type> · <driver>`; an empty part is written as
/// `unknown`.
#[must_use]
pub fn adapter_line(name: &str, backend: &str, kind: &str, driver: &str) -> String {
    let part = |s: &str| {
        let s = s.trim();
        if s.is_empty() { "unknown" } else { s }.to_string()
    };
    format!(
        "graphics adapter: {} \u{b7} {} \u{b7} {} \u{b7} {}",
        part(name),
        part(backend),
        part(kind),
        part(driver)
    )
}

static START: OnceLock<Instant> = OnceLock::new();
static TIMELINE: Mutex<Timeline> = Mutex::new(Timeline { done: Vec::new() });
static ADAPTER: Mutex<Option<String>> = Mutex::new(None);

/// Captures the process start (the first call wins) and turns [`mark`] on.
#[allow(clippy::disallowed_methods)] // Justification: diagnostics timing of the start-up only;
// it never reaches the engine or any rendered audio.
pub fn init() {
    START.get_or_init(Instant::now);
}

/// Writes the start-up line of `stage` the first time it is reached. A no-op before [`init`].
pub fn mark(stage: &'static str) {
    let Some(start) = START.get() else {
        return;
    };
    let elapsed = elapsed_since(start);
    let text = match TIMELINE.lock() {
        Ok(mut t) => t.mark(stage, elapsed),
        Err(_) => None,
    };
    if let Some(text) = text {
        log::info!("{text}");
    }
}

#[allow(clippy::disallowed_methods)] // Justification: diagnostics timing, as in [`init`].
fn elapsed_since(start: &Instant) -> Duration {
    Instant::now().saturating_duration_since(*start)
}

/// Keeps the adapter line for the diagnostics report and writes it to the log.
pub fn set_adapter(text: String) {
    log::info!("{text}");
    if let Ok(mut a) = ADAPTER.lock() {
        *a = Some(text);
    }
}

/// Keeps the adapter line for the diagnostics report without writing it to the log (the graphics
/// were built again after a loss, and the line of that says so itself).
pub fn keep_adapter(text: String) {
    if let Ok(mut a) = ADAPTER.lock() {
        *a = Some(text);
    }
}

/// The adapter line, once the renderer picked its adapter.
#[must_use]
pub fn adapter() -> Option<String> {
    ADAPTER.lock().ok().and_then(|a| a.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_startup_line_has_the_shape_of_the_log() {
        assert_eq!(
            line(Duration::from_millis(412), FIRST_FRAME),
            "startup 412 ms: first frame presented"
        );
        assert_eq!(
            line(Duration::from_micros(12_999), AUDIO_RUNNING),
            "startup 12 ms: audio stream running"
        );
    }

    #[test]
    fn each_stage_is_written_once() {
        let mut t = Timeline::default();
        let first = t.mark(FIRST_WINDOW, Duration::from_millis(80));
        assert_eq!(
            first.as_deref(),
            Some("startup 80 ms: first window created")
        );
        assert_eq!(t.mark(FIRST_WINDOW, Duration::from_millis(900)), None);
        assert!(t.mark(FIRST_FRAME, Duration::from_millis(900)).is_some());
    }

    #[test]
    fn the_adapter_line_names_the_four_facts() {
        assert_eq!(
            adapter_line("Example GPU 1", "DX12", "integrated", "Example driver 31.0"),
            "graphics adapter: Example GPU 1 \u{b7} DX12 \u{b7} integrated \u{b7} Example driver 31.0"
        );
        assert_eq!(
            adapter_line("", "Vulkan", "discrete", "  "),
            "graphics adapter: unknown \u{b7} Vulkan \u{b7} discrete \u{b7} unknown"
        );
    }

    #[test]
    fn marking_before_init_writes_nothing() {
        // `init` is never called by the unit tests: the stage must stay unwritten.
        mark(AUDIO_RUNNING);
        let written = TIMELINE.lock().map(|t| t.done.len()).unwrap_or(0);
        assert_eq!(written, 0);
    }
}
