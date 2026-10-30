//! The panic hook: a `crash-<timestamp>.txt` report for every panic.
//!
//! The report holds the message and place of the panic, the thread, a backtrace, the app version,
//! the OS and the audio and MIDI devices the session had open (see [`set_context`]). The hook then
//! calls the hook that was installed before it (the default one prints to stderr).
//!
//! A panic on an audio or render-worker thread writes nothing: it only raises a flag
//! ([`AUDIO_PANIC`]) and the UI thread writes the report on its next frame
//! ([`flush_deferred`]). The engine already catches a panicking device or callback and goes
//! silent (see `atomik_engine::FaultState`); this report says it happened.

use crate::logging::{file_stamp, unix_seconds, utc_stamp};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type PreviousHook = dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static;

/// Where the reports go; `None` while no hook is installed.
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
/// What the session had open, kept current by the UI thread.
static CONTEXT: Mutex<String> = Mutex::new(String::new());
/// Raised by a panic on an audio or render-worker thread; the UI thread writes its report.
static AUDIO_PANIC: AtomicBool = AtomicBool::new(false);
/// Set while the hook is installed: it is installed once.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Keeps the hook installed; dropping it puts the previous hook back.
pub struct HookGuard {
    previous: Arc<PreviousHook>,
}

impl std::fmt::Debug for HookGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookGuard")
    }
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        let previous = Arc::clone(&self.previous);
        // Dropping the hook during a panic would be a panic in a destructor.
        if !std::thread::panicking() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
        *DIR.lock().unwrap_or_else(PoisonError::into_inner) = None;
        INSTALLED.store(false, Ordering::SeqCst);
    }
}

/// Installs the hook, writing reports into `dir` (made when a report is written). Installed
/// once: a second call while the first guard is alive returns `None`.
pub fn install(dir: &Path) -> Option<HookGuard> {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return None;
    }
    *DIR.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir.to_path_buf());
    let previous: Arc<PreviousHook> = Arc::from(std::panic::take_hook());
    let chained = Arc::clone(&previous);
    std::panic::set_hook(Box::new(move |info| {
        on_panic(info);
        chained(info);
    }));
    Some(HookGuard { previous })
}

/// Records what the session has open (devices, ports) for the next report. UI thread.
pub fn set_context(text: &str) {
    let mut context = CONTEXT.lock().unwrap_or_else(PoisonError::into_inner);
    if *context != text {
        context.clear();
        context.push_str(text);
    }
}

/// Whether a thread called `name` is an audio or render-worker thread: the hook must not do I/O
/// there.
pub fn is_realtime_thread(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("atomik-render")
        || name.starts_with("host-audio")
        || ["audio", "cpal", "wasapi", "coreaudio", "alsa", "jack"]
            .iter()
            .any(|word| name.contains(word))
}

fn on_panic(info: &PanicHookInfo<'_>) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    if is_realtime_thread(name) {
        AUDIO_PANIC.store(true, Ordering::SeqCst);
        return;
    }
    let dir = DIR.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let report = report_text(info, name);
    log::error!("panic on thread {name}: {}", first_line_of(info));
    if let Some(dir) = dir {
        write_report(&dir, &report);
    }
}

/// Writes the report a panic on an audio or render-worker thread asked for. Call once per UI
/// frame; returns the file written, if there was a pending panic.
pub fn flush_deferred() -> Option<PathBuf> {
    if !AUDIO_PANIC.swap(false, Ordering::SeqCst) {
        return None;
    }
    log::error!("a panic was caught on an audio or render thread; the engine is silent");
    let dir = DIR.lock().unwrap_or_else(PoisonError::into_inner).clone()?;
    let text = format!(
        "{}\nA panic was caught on an audio or render thread. The hook does no file I/O \
         there, so the message and the backtrace are only on stderr and in the log.\n",
        header("audio or render thread")
    );
    write_report(&dir, &text)
}

fn first_line_of(info: &PanicHookInfo<'_>) -> String {
    message_of(info).lines().next().unwrap_or("").to_string()
}

fn message_of(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(the panic carried no text)".to_string()
    }
}

/// The lines every report starts with.
fn header(thread: &str) -> String {
    let context = CONTEXT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    format!(
        "Atomik Rack crash report\n\
         time: {}\n\
         version: {}\n\
         os: {} {}\n\
         thread: {thread}\n\
         session: {}\n",
        utc_stamp(unix_seconds()),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if context.is_empty() {
            "(nothing recorded)"
        } else {
            &context
        }
    )
}

fn report_text(info: &PanicHookInfo<'_>, thread: &str) -> String {
    let place = info
        .location()
        .map_or_else(|| "(unknown)".to_string(), ToString::to_string);
    format!(
        "{}message: {}\nlocation: {place}\nbacktrace:\n{}\n",
        header(thread),
        message_of(info),
        std::backtrace::Backtrace::force_capture()
    )
}

/// Writes `text` to a fresh `crash-<timestamp>.txt` (with a number when the second already has
/// one) in `dir`. A failure is only logged: a crash report must not cause a second failure.
fn write_report(dir: &Path, text: &str) -> Option<PathBuf> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("cannot make {} for the crash report: {e}", dir.display());
        return None;
    }
    let stamp = file_stamp(unix_seconds());
    for n in 0..100 {
        let name = if n == 0 {
            format!("crash-{stamp}.txt")
        } else {
            format!("crash-{stamp}-{n}.txt")
        };
        let path = dir.join(name);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match file {
            Ok(mut file) => {
                use std::io::Write;
                return match file.write_all(text.as_bytes()) {
                    Ok(()) => Some(path),
                    Err(e) => {
                        log::warn!("cannot write {}: {e}", path.display());
                        None
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                log::warn!("cannot create {}: {e}", path.display());
                return None;
            }
        }
    }
    None
}

/// The newest `crash-*.txt` in `dir`, if any (names sort by their timestamp).
pub fn latest_report(dir: &Path) -> Option<PathBuf> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        .collect();
    names.sort();
    names.pop().map(|n| dir.join(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hook is process-wide: the tests that install it run one at a time.
    static HOOK_TESTS: Mutex<()> = Mutex::new(());

    /// The text of the reports in `dir` that mention `needle` (other tests may panic too).
    fn reports_with(dir: &Path, needle: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter(|t| t.contains(needle))
            .collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atomik-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn audio_and_render_threads_are_told_apart_from_the_others() {
        for name in [
            "atomik-render-1",
            "host-audio",
            "cpal_wasapi_out",
            "CoreAudio IO",
            "alsa-out",
        ] {
            assert!(is_realtime_thread(name), "{name}");
        }
        for name in ["main", "atomik-import", "atomik-export", "<unnamed>"] {
            assert!(!is_realtime_thread(name), "{name}");
        }
    }

    #[test]
    fn a_panic_writes_a_report_and_the_guard_restores_the_previous_hook() {
        let _serial = HOOK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = scratch("panic");
        set_context("WASAPI \u{b7} Test Output \u{b7} MIDI Test Port");
        let guard = install(&dir).expect("the hook is not installed yet");
        assert!(install(&dir).is_none(), "installed once");
        let result = std::thread::Builder::new()
            .name("ui-test-thread".into())
            .spawn(|| panic!("crash-report-test-message"))
            .unwrap()
            .join();
        assert!(result.is_err());
        drop(guard);
        let file = latest_report(&dir).expect("a crash file was written");
        assert!(
            file.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("crash-")
        );
        let reports = reports_with(&dir, "crash-report-test-message");
        assert_eq!(reports.len(), 1, "one report for the panic");
        let text = &reports[0];
        assert!(
            text.contains("message: crash-report-test-message"),
            "{text}"
        );
        assert!(text.contains("thread: ui-test-thread"), "{text}");
        assert!(text.contains("backtrace:"), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains("MIDI Test Port"), "{text}");
        assert!(text.contains("location: "), "{text}");
        // The previous hook is back: a new guard can be installed, and dropping it is clean.
        assert!(install(&dir).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panic_on_a_render_thread_only_raises_the_flag() {
        let _serial = HOOK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = scratch("deferred");
        let guard = install(&dir).expect("the hook is not installed yet");
        let result = std::thread::Builder::new()
            .name("atomik-render-9".into())
            .spawn(|| panic!("render-thread-test-message"))
            .unwrap()
            .join();
        assert!(result.is_err());
        assert!(
            reports_with(&dir, "render-thread-test-message").is_empty(),
            "no file I/O on the render thread"
        );
        let file = flush_deferred().expect("the UI thread writes the report");
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("audio or render thread"), "{text}");
        assert!(flush_deferred().is_none(), "the flag is taken");
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
