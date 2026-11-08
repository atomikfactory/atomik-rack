//! Taking the sound out of a film: choosing a track and writing it to a WAV file.
//!
//! [`pick_stream`] chooses the track to import (the only one, the one asked for, or "ask the
//! user" when there are several). [`extract_audio`] runs `ffmpeg` on a worker thread, reports
//! progress through [`Control`] (as milliseconds of the film's length) and stops when the cancel
//! flag is set; the WAV file is written under the name it is given and removed again when the job
//! fails or is stopped, so a file that is there is whole. [`lead_us`] is the distance between
//! the first sample of the track and the film's first frame, which the song needs to line the
//! sound up with the picture.

use crate::VideoError;
use crate::args::audio_args;
use crate::discover::Ffmpeg;
use crate::prepare::Control;
use crate::probe::{AudioStream, ProbeInfo};
use crate::runner::VideoTool;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

/// The largest lead the song takes, in microseconds (an hour: a broadcast file may start its
/// sound a few frames off its picture, never an hour).
pub const MAX_LEAD_US: i64 = 3_600_000_000;

/// The track the import uses, or the tracks to choose from.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick<'a> {
    /// This track.
    One(&'a AudioStream),
    /// The film has several tracks and none was asked for: the user chooses.
    Choose(&'a [AudioStream]),
}

/// The track to import: `wanted` (a stream number of the file) when given, else the only one.
///
/// # Errors
/// [`VideoError::NoAudio`] when the film has no audio track and [`VideoError::NoSuchAudio`] when
/// `wanted` is not one of them.
pub fn pick_stream(info: &ProbeInfo, wanted: Option<u32>) -> Result<Pick<'_>, VideoError> {
    match (wanted, info.audio.as_slice()) {
        (_, []) => Err(VideoError::NoAudio),
        (Some(index), tracks) => tracks
            .iter()
            .find(|t| t.index == index)
            .map(Pick::One)
            .ok_or(VideoError::NoSuchAudio(index)),
        (None, [only]) => Ok(Pick::One(only)),
        (None, tracks) => Ok(Pick::Choose(tracks)),
    }
}

/// How far after the film's first frame the first sample of `stream` sits, in microseconds
/// (negative: the sound starts before the picture), from the start times of the probe.
pub fn lead_us(info: &ProbeInfo, stream: &AudioStream) -> i64 {
    let us = ((stream.start_time_s - info.start_time_s) * 1e6).round();
    (us as i64).clamp(-MAX_LEAD_US, MAX_LEAD_US)
}

/// The film time of an `out_time_us=` progress line, in microseconds.
pub fn progress_us(line: &str) -> Option<u64> {
    line.trim()
        .strip_prefix("out_time_us=")
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// Removes the output file unless the job ended well.
struct OutFile<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for OutFile<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// Writes the audio track `stream` of `source` to the WAV file `out` (see the module docs).
/// `ctl.total` is set to the film's length in milliseconds and `ctl.done` follows ffmpeg.
///
/// # Errors
/// [`VideoError::Cancelled`] when `ctl.cancel` is set (nothing is left behind), the failure of
/// ffmpeg with the end of its error output, and [`VideoError::Io`] when the file is not there
/// afterwards.
pub fn extract_audio(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    source: &Path,
    (info, stream): (&ProbeInfo, &AudioStream),
    out: &Path,
    ctl: &Control<'_>,
) -> Result<(), VideoError> {
    ctl.done.store(0, Ordering::Relaxed);
    ctl.total.store(
        (info.duration_s * 1e3).round().max(1.0) as u64,
        Ordering::Relaxed,
    );
    let mut guard = OutFile {
        path: out,
        armed: true,
    };
    let mut pending = String::new();
    let exit = tool.run(
        &ffmpeg.ffmpeg,
        &audio_args(source, stream, out),
        &mut |chunk| {
            pending.push_str(&String::from_utf8_lossy(chunk));
            while let Some(end) = pending.find('\n') {
                if let Some(us) = progress_us(&pending[..end]) {
                    ctl.done.store(us / 1_000, Ordering::Relaxed);
                }
                pending.drain(..=end);
            }
            // A line that never ends is not progress: keep the buffer small.
            if pending.len() > 4096 {
                pending.clear();
            }
            true
        },
        ctl.cancel,
    )?;
    if ctl.cancel.load(Ordering::Relaxed) {
        return Err(VideoError::Cancelled);
    }
    if !exit.success {
        return Err(exit.failure(&ffmpeg.ffmpeg));
    }
    let size = fs::metadata(out)?.len();
    if size <= 44 {
        return Err(VideoError::Proxy(
            "ffmpeg wrote no sound for that track".into(),
        ));
    }
    ctl.done
        .store(ctl.total.load(Ordering::Relaxed), Ordering::Relaxed);
    guard.armed = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeTool, wav_pcm16};
    use crate::probe::parse_probe;
    use crate::samples::{MKV_HEVC, MOV_PRORES, MP4_H264, NO_AUDIO};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    fn ffmpeg() -> Ffmpeg {
        Ffmpeg {
            ffmpeg: PathBuf::from("/bin/ffmpeg"),
            ffprobe: PathBuf::from("/bin/ffprobe"),
            version: crate::parse_version("ffmpeg version 7.1 x").unwrap(),
        }
    }

    fn folder(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "atomik-video-audio-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Counters {
        cancel: Arc<AtomicBool>,
        done: AtomicU64,
        total: AtomicU64,
    }

    impl Counters {
        fn new() -> Self {
            Self {
                cancel: Arc::new(AtomicBool::new(false)),
                done: AtomicU64::new(0),
                total: AtomicU64::new(0),
            }
        }

        fn run(&self, tool: &FakeTool, out: &Path) -> Result<(), VideoError> {
            let info = parse_probe(MP4_H264).unwrap();
            extract_audio(
                tool,
                &ffmpeg(),
                Path::new("film.mp4"),
                (&info, &info.audio[0]),
                out,
                &Control {
                    cancel: &self.cancel,
                    done: &self.done,
                    total: &self.total,
                },
            )
        }
    }

    #[test]
    fn the_only_track_is_used_and_several_ask_the_user() {
        let one = parse_probe(MP4_H264).unwrap();
        assert!(matches!(pick_stream(&one, None), Ok(Pick::One(s)) if s.index == 1));
        let two = parse_probe(MKV_HEVC).unwrap();
        assert!(matches!(pick_stream(&two, None), Ok(Pick::Choose(t)) if t.len() == 2));
        assert!(matches!(pick_stream(&two, Some(2)), Ok(Pick::One(s)) if s.index == 2));
        assert!(matches!(
            pick_stream(&two, Some(7)),
            Err(VideoError::NoSuchAudio(7))
        ));
        let none = parse_probe(NO_AUDIO).unwrap();
        assert!(matches!(pick_stream(&none, None), Err(VideoError::NoAudio)));
        assert!(matches!(
            pick_stream(&none, Some(1)),
            Err(VideoError::NoAudio)
        ));
    }

    #[test]
    fn the_lead_is_the_audio_start_against_the_picture_start() {
        let p = parse_probe(MP4_H264).unwrap();
        assert_eq!(lead_us(&p, &p.audio[0]), 0);
        // The sound of an Opus track starts 7 ms after the picture.
        let mkv = parse_probe(MKV_HEVC).unwrap();
        assert_eq!(mkv.start_time_s, 0.0);
        assert_eq!(lead_us(&mkv, &mkv.audio[0]), 7_000);
        assert_eq!(lead_us(&mkv, &mkv.audio[1]), 7_000);
        // Both start at 01:00:00 on the file's clock: no lead.
        let broadcast = parse_probe(MOV_PRORES).unwrap();
        assert_eq!(lead_us(&broadcast, &broadcast.audio[0]), 0);
        // A sound that starts before the picture has a negative lead.
        let mut early = parse_probe(MP4_H264).unwrap();
        early.start_time_s = 0.040;
        early.audio[0].start_time_s = 0.0;
        assert_eq!(lead_us(&early, &early.audio[0]), -40_000);
        early.audio[0].start_time_s = 1e9;
        assert_eq!(lead_us(&early, &early.audio[0]), MAX_LEAD_US);
    }

    #[test]
    fn progress_lines_are_read() {
        assert_eq!(progress_us("out_time_us=1500000"), Some(1_500_000));
        assert_eq!(progress_us("out_time_us=N/A"), None);
        assert_eq!(progress_us("out_time_ms=1500000"), None);
        assert_eq!(progress_us("progress=continue"), None);
    }

    #[test]
    fn the_wav_is_written_with_progress() {
        let dir = folder("ok");
        let out = dir.join("film audio.wav");
        let tool = FakeTool::new().audio(wav_pcm16(48_000, 2, &[0i16; 2000]));
        let c = Counters::new();
        c.run(&tool, &out).unwrap();
        assert!(out.is_file());
        assert_eq!(c.total.load(Ordering::Relaxed), 12_021);
        assert_eq!(c.done.load(Ordering::Relaxed), 12_021);
        let calls = tool.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].has("-vn"));
        assert_eq!(calls[0].after("-map").unwrap(), "0:1");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stopped_job_leaves_no_file() {
        let dir = folder("cancel");
        let out = dir.join("a.wav");
        let c = Counters::new();
        let flag = Arc::clone(&c.cancel);
        let tool = FakeTool::new()
            .audio(wav_pcm16(48_000, 2, &[1i16; 2000]))
            .on_frame(move |n| {
                if n == 2 {
                    flag.store(true, Ordering::Relaxed);
                }
            });
        let result = c.run(&tool, &out);
        assert!(matches!(result, Err(VideoError::Cancelled)));
        assert!(!out.exists(), "the half-written file is removed");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failure_says_what_ffmpeg_printed_and_leaves_no_file() {
        let dir = folder("fail");
        let out = dir.join("a.wav");
        let tool = FakeTool::new()
            .audio(wav_pcm16(48_000, 2, &[1i16; 100]))
            .stream_fails("Invalid data found when processing input");
        let err = Counters::new().run(&tool, &out).unwrap_err();
        assert!(err.to_string().contains("Invalid data"), "{err}");
        assert!(!out.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_output_is_an_error() {
        let dir = folder("empty");
        let out = dir.join("a.wav");
        let tool = FakeTool::new().audio(Vec::new());
        let err = Counters::new().run(&tool, &out).unwrap_err();
        assert!(
            matches!(err, VideoError::Io(_) | VideoError::Proxy(_)),
            "{err}"
        );
        assert!(!out.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
