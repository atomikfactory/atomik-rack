//! Preparing a film: probe it, find or build its proxy.
//!
//! [`prepare`] is the whole job of "Prepare Video": it is meant to run on a worker thread and
//! reports through shared counters ([`Control`]) so the window can show progress and stop it.
//! The proxy is written under a `.part` name and renamed only when ffmpeg has ended with success
//! and every frame is in the file, so a proxy under its final name is always whole and a stopped
//! or failed build leaves nothing behind.

use crate::VideoError;
use crate::args::{ProxySpec, probe_args, proxy_args};
use crate::cache::{ProxyCache, SourceIdentity};
use crate::discover::Ffmpeg;
use crate::jpeg::FrameSplitter;
use crate::probe::{ProbeInfo, parse_probe};
use crate::proxy::{ProxyInfo, ProxyReader, ProxyWriter};
use crate::runner::{VideoTool, capture};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The most of ffprobe's output read, in bytes (a film with thousands of chapters stays under).
const PROBE_LIMIT: usize = 4 * 1024 * 1024;

/// What a job shares with the thread that started it.
#[derive(Debug, Clone, Copy)]
pub struct Control<'a> {
    /// Set to stop the job: the process is killed and the part file removed.
    pub cancel: &'a AtomicBool,
    /// Frames written so far.
    pub done: &'a AtomicU64,
    /// Frames expected (set once the probe is read; an estimate for a variable-rate film).
    pub total: &'a AtomicU64,
}

/// A film that is ready for the viewer and the thumbnails.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    /// The source film.
    pub source: PathBuf,
    /// What the probe found.
    pub probe: ProbeInfo,
    /// The cache key.
    pub key: String,
    /// The proxy file.
    pub proxy: PathBuf,
    /// What the proxy states.
    pub info: ProxyInfo,
    /// The proxy was already in the cache.
    pub reused: bool,
}

/// Removes the part file unless it was renamed.
struct PartFile<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for PartFile<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// Probes `source` with `ffprobe`.
///
/// # Errors
/// As [`capture`] and [`parse_probe`].
pub fn probe(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    source: &Path,
    cancel: &AtomicBool,
) -> Result<ProbeInfo, VideoError> {
    let text = capture(
        tool,
        &ffmpeg.ffprobe,
        &probe_args(source),
        PROBE_LIMIT,
        cancel,
    )?;
    parse_probe(&text)
}

/// Writes the proxy of `source` to `cache` under `key` (see the module docs).
fn build(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    source: &Path,
    info: &ProbeInfo,
    (cache, key): (&ProxyCache, &str),
    spec: &ProxySpec,
    ctl: &Control<'_>,
) -> Result<PathBuf, VideoError> {
    fs::create_dir_all(cache.dir())?;
    let part = cache.part_path(key);
    let target = cache.proxy_path(key);
    let mut cleanup = PartFile {
        path: &part,
        armed: true,
    };
    let file = BufWriter::new(File::create(&part)?);
    let mut writer = ProxyWriter::new(file, info.video.frame_rate)?;
    let mut splitter = FrameSplitter::new();
    let mut failure: Option<VideoError> = None;
    let args = proxy_args(source, info, spec);
    let exit = tool.run(
        &ffmpeg.ffmpeg,
        &args,
        &mut |chunk| {
            let pushed = splitter.push(chunk, &mut |frame| match writer.push_frame(frame) {
                Ok(()) => {
                    ctl.done.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(e) => {
                    failure = Some(e);
                    false
                }
            });
            match pushed {
                Ok(more) => more,
                Err(e) => {
                    failure.get_or_insert(e);
                    false
                }
            }
        },
        ctl.cancel,
    )?;
    if let Some(e) = failure {
        return Err(e);
    }
    if !exit.success {
        return Err(exit.failure(&ffmpeg.ffmpeg));
    }
    if splitter.has_partial() {
        return Err(VideoError::Proxy("the last frame was cut short".into()));
    }
    if ctl.cancel.load(Ordering::Relaxed) {
        return Err(VideoError::Cancelled);
    }
    let file = writer
        .finish()?
        .into_inner()
        .map_err(|e| VideoError::Io(e.into_error()))?;
    file.sync_all()?;
    drop(file);
    fs::rename(&part, &target)?;
    cleanup.armed = false;
    Ok(target)
}

/// Probes `source` and returns its proxy: from the cache when it is there, else built (and the
/// cache pruned to its limit).
///
/// # Errors
/// [`VideoError::Cancelled`] when `ctl.cancel` is set (nothing is left behind), the errors of the
/// probe, of the build and of the cache folder otherwise.
pub fn prepare(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    source: &Path,
    cache: &ProxyCache,
    spec: &ProxySpec,
    ctl: &Control<'_>,
) -> Result<Prepared, VideoError> {
    ctl.done.store(0, Ordering::Relaxed);
    ctl.total.store(0, Ordering::Relaxed);
    let probe = probe(tool, ffmpeg, source, ctl.cancel)?;
    ctl.total.store(probe.expected_frames(), Ordering::Relaxed);
    let key = SourceIdentity::read(source)?.key();
    let (proxy, reused) = match cache.lookup(&key) {
        Some(path) => (path, true),
        None => {
            let path = build(tool, ffmpeg, source, &probe, (cache, &key), spec, ctl)?;
            if let Err(e) = cache.prune(Some(&path)) {
                log::warn!("video proxy cache not pruned: {e}");
            }
            (path, false)
        }
    };
    let info = ProxyReader::open(File::open(&proxy)?)?.info();
    ctl.done.store(u64::from(info.frames), Ordering::Relaxed);
    ctl.total.store(u64::from(info.frames), Ordering::Relaxed);
    Ok(Prepared {
        source: source.to_path_buf(),
        probe,
        key,
        proxy,
        info,
        reused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeTool, tiny_jpeg};
    use std::path::PathBuf;
    use std::sync::Arc;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::AtomicU64;
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "atomik-video-prep-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn film(&self) -> PathBuf {
            let p = self.0.join("film.mp4");
            fs::write(&p, b"pretend film bytes").unwrap();
            p
        }

        fn cache(&self) -> ProxyCache {
            ProxyCache::new(self.0.join("cache"))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ffmpeg() -> Ffmpeg {
        Ffmpeg {
            ffmpeg: PathBuf::from("/bin/ffmpeg"),
            ffprobe: PathBuf::from("/bin/ffprobe"),
            version: crate::parse_version("ffmpeg version 7.1 x").unwrap(),
        }
    }

    fn frames(n: u8) -> Vec<Vec<u8>> {
        (0..n).map(|i| tiny_jpeg(640, 360, i)).collect()
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    struct Counters {
        cancel: AtomicBool,
        done: AtomicU64,
        total: AtomicU64,
    }

    impl Counters {
        fn new() -> Self {
            Self {
                cancel: AtomicBool::new(false),
                done: AtomicU64::new(0),
                total: AtomicU64::new(0),
            }
        }

        fn ctl(&self) -> Control<'_> {
            Control {
                cancel: &self.cancel,
                done: &self.done,
                total: &self.total,
            }
        }
    }

    #[test]
    fn a_film_is_probed_and_its_proxy_built_with_progress() {
        let dir = Dir::new("build");
        let tool = FakeTool::new().frames(frames(30)).chunk(1000);
        let c = Counters::new();
        let got = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        assert!(!got.reused);
        assert_eq!(got.probe.video.width, 1920);
        assert_eq!(got.info.frames, 30);
        assert_eq!((got.info.width, got.info.height), (640, 360));
        assert_eq!(got.info.rate.num, 25);
        assert_eq!(c.done.load(Ordering::Relaxed), 30);
        assert_eq!(c.total.load(Ordering::Relaxed), 30);
        // The proxy is whole, under its final name, and nothing else is in the cache.
        assert_eq!(listing(dir.cache().dir()), [format!("{}.atproxy", got.key)]);
        let mut r = ProxyReader::open(File::open(&got.proxy).unwrap()).unwrap();
        let mut frame = Vec::new();
        r.read_frame(17, &mut frame).unwrap();
        assert_eq!(frame, tiny_jpeg(640, 360, 17));
    }

    #[test]
    fn the_calls_are_a_probe_then_one_frame_stream_with_the_path_as_an_argument() {
        let dir = Dir::new("calls");
        let tool = FakeTool::new().frames(frames(2));
        let c = Counters::new();
        let film = dir.film();
        prepare(
            &tool,
            &ffmpeg(),
            &film,
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        let calls = tool.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].exe, PathBuf::from("/bin/ffprobe"));
        assert!(calls[0].has("-show_streams"));
        assert_eq!(calls[1].exe, PathBuf::from("/bin/ffmpeg"));
        assert!(calls[1].has("-nostdin"));
        let mut input = std::ffi::OsString::from("file:");
        input.push(&film);
        assert_eq!(calls[1].after("-i"), Some(&input));
        assert_eq!(calls[1].args.last().unwrap(), "pipe:1");
    }

    #[test]
    fn a_second_prepare_reuses_the_proxy_without_running_ffmpeg() {
        let dir = Dir::new("reuse");
        let tool = FakeTool::new().frames(frames(5));
        let c = Counters::new();
        let film = dir.film();
        let first = prepare(
            &tool,
            &ffmpeg(),
            &film,
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        let second = prepare(
            &tool,
            &ffmpeg(),
            &film,
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        assert!(second.reused);
        assert_eq!(
            (first.key.as_str(), &first.proxy),
            (second.key.as_str(), &second.proxy)
        );
        // Two probes and one frame stream.
        let streams = tool.calls().iter().filter(|c| c.has("-nostdin")).count();
        assert_eq!(streams, 1);
        assert_eq!(c.done.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn a_changed_film_gets_a_new_proxy() {
        let dir = Dir::new("changed");
        let tool = FakeTool::new().frames(frames(3));
        let c = Counters::new();
        let film = dir.film();
        let a = prepare(
            &tool,
            &ffmpeg(),
            &film,
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        fs::write(&film, b"a different film, longer than before").unwrap();
        let b = prepare(
            &tool,
            &ffmpeg(),
            &film,
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        assert!(!b.reused);
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn a_stopped_build_leaves_no_proxy_and_no_part_file() {
        let dir = Dir::new("cancel");
        let c = Arc::new(Counters::new());
        let hook = Arc::clone(&c);
        let tool = FakeTool::new()
            .frames(frames(50))
            .chunk(300)
            .on_frame(move |n| {
                if n == 3 {
                    hook.cancel.store(true, Ordering::Relaxed);
                }
            });
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap_err();
        assert!(matches!(err, VideoError::Cancelled), "{err}");
        assert_eq!(c.done.load(Ordering::Relaxed), 3);
        assert_eq!(listing(dir.cache().dir()), Vec::<String>::new());
    }

    #[test]
    fn a_cancel_before_the_start_does_nothing() {
        let dir = Dir::new("precancel");
        let tool = FakeTool::new().frames(frames(2));
        let c = Counters::new();
        c.cancel.store(true, Ordering::Relaxed);
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap_err();
        assert!(matches!(err, VideoError::Cancelled));
        assert!(tool.calls().is_empty());
        assert!(!dir.cache().dir().exists());
    }

    #[test]
    fn a_failing_ffmpeg_gives_its_message_and_leaves_nothing() {
        let dir = Dir::new("fail");
        let tool = FakeTool::new()
            .frames(frames(4))
            .stream_fails("Invalid data found when processing input");
        let c = Counters::new();
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("ffmpeg failed"), "{text}");
        assert!(text.contains("Invalid data found"), "{text}");
        assert_eq!(listing(dir.cache().dir()), Vec::<String>::new());
    }

    #[test]
    fn a_stream_with_no_frames_or_garbage_is_an_error_and_leaves_nothing() {
        let dir = Dir::new("garbage");
        let c = Counters::new();
        for tool in [
            FakeTool::new().frames(Vec::new()),
            FakeTool::new().frames(vec![b"this is not a picture".to_vec()]),
            FakeTool::new().frames(vec![tiny_jpeg(8, 8, 1)[..30].to_vec()]),
        ] {
            let err = prepare(
                &tool,
                &ffmpeg(),
                &dir.film(),
                &dir.cache(),
                &ProxySpec::default(),
                &c.ctl(),
            );
            assert!(err.is_err());
            assert_eq!(listing(dir.cache().dir()), Vec::<String>::new());
        }
    }

    #[test]
    fn a_probe_failure_or_a_file_without_picture_stops_before_ffmpeg() {
        let dir = Dir::new("probe");
        let c = Counters::new();
        let tool = FakeTool::new().probe_fails();
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("ffprobe failed"), "{err}");
        let tool = FakeTool::new().probe(crate::samples::COVER_ART_ONLY);
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.film(),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap_err();
        assert!(matches!(err, VideoError::NoVideo));
        assert!(tool.calls().iter().all(|c| !c.has("-nostdin")));
    }

    #[test]
    fn a_missing_film_is_an_error() {
        let dir = Dir::new("missing");
        let tool = FakeTool::new().frames(frames(2));
        let c = Counters::new();
        let err = prepare(
            &tool,
            &ffmpeg(),
            &dir.0.join("gone.mp4"),
            &dir.cache(),
            &ProxySpec::default(),
            &c.ctl(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn the_cache_is_pruned_after_a_build() {
        let dir = Dir::new("prune");
        let tool = FakeTool::new().frames(frames(3));
        let c = Counters::new();
        let cache = dir.cache().with_limit(1);
        let a = dir.film();
        prepare(
            &tool,
            &ffmpeg(),
            &a,
            &cache,
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        let b = dir.0.join("other.mp4");
        fs::write(&b, b"another film").unwrap();
        let second = prepare(
            &tool,
            &ffmpeg(),
            &b,
            &cache,
            &ProxySpec::default(),
            &c.ctl(),
        )
        .unwrap();
        // The older one went; the one just built stays although it is over the limit.
        assert_eq!(listing(cache.dir()), [format!("{}.atproxy", second.key)]);
    }
}
