//! Compressed audio: encoding a WAV file to MP3, AAC (in `.m4a`) or Opus (in `.opus`) with the
//! user's FFmpeg, for the audio export.
//!
//! The export renders the song to a temporary WAV file; [`encode_audio`] turns it into the
//! compressed file. The call goes through the same [`VideoTool`] as every other FFmpeg call, so it
//! never touches a shell: the arguments are a list ([`encode_args`]), the song's title and author
//! go in as the values of `-metadata` arguments (cleaned by [`clean_tag`]), and no codec ships with
//! the program. The file is written under a temporary name next to the target and renamed when
//! FFmpeg has finished, so a stopped or failed job leaves nothing behind and an existing file is
//! replaced only by a complete one. Progress is the fraction of the song FFmpeg has written.

use crate::VideoError;
use crate::args::{file_input, os};
use crate::discover::Ffmpeg;
use crate::runner::VideoTool;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The longest tag value kept, in characters.
pub const MAX_TAG_CHARS: usize = 255;

/// A compressed format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossyCodec {
    /// MP3 (`libmp3lame`).
    Mp3,
    /// AAC in an MPEG-4 audio file (`.m4a`).
    Aac,
    /// Opus in an Ogg file (`.opus`).
    Opus,
}

impl LossyCodec {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
            Self::Opus => "opus",
        }
    }

    /// The name of the format for a message.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::Opus => "Opus",
        }
    }

    /// The sample rate a file of this format gets when `wanted` Hz was asked for: the smallest
    /// rate the format can hold that is at least `wanted`, and the highest it holds when `wanted`
    /// is above all of them (MP3 and Opus stop at 48 kHz, AAC at 96 kHz).
    pub fn output_rate(self, wanted: u32) -> u32 {
        let rates: &[u32] = match self {
            Self::Mp3 => &[
                8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
            ],
            Self::Opus => &[8_000, 12_000, 16_000, 24_000, 48_000],
            Self::Aac => &[
                8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 64_000,
                88_200, 96_000,
            ],
        };
        rates
            .iter()
            .copied()
            .find(|&r| r >= wanted)
            .unwrap_or(rates[rates.len() - 1])
    }

    /// The range of constant bitrates in kilobits per second the format takes.
    pub fn bitrates(self) -> (u32, u32) {
        match self {
            Self::Mp3 => (32, 320),
            Self::Aac => (32, 512),
            Self::Opus => (6, 510),
        }
    }
}

/// How hard the encoder squeezes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossyQuality {
    /// A constant bitrate in kilobits per second.
    Cbr(u32),
    /// A variable bitrate by quality, `0` (best) to `9` (smallest); MP3 only.
    Vbr(u8),
}

/// What to encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeSpec {
    /// The format.
    pub codec: LossyCodec,
    /// The quality.
    pub quality: LossyQuality,
    /// The sample rate of the WAV file, which is also the rate of the result (the export has
    /// converted it to [`LossyCodec::output_rate`] already).
    pub rate: u32,
    /// The channels of the WAV file; more than two are mixed down to stereo.
    pub channels: usize,
}

/// The song information that goes into the file's tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    /// The song's title.
    pub title: String,
    /// Who made it.
    pub author: String,
}

/// A tag value as FFmpeg is given it: control characters (line breaks, tabs) become spaces, the
/// ends are trimmed and the length is capped. `None` for a value with nothing left (no tag is
/// written then).
pub fn clean_tag(text: &str) -> Option<String> {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned: String = spaced.trim().chars().take(MAX_TAG_CHARS).collect();
    let cleaned = cleaned.trim_end().to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The bitrate as FFmpeg writes it: `192k`, clamped to what the format takes.
pub(crate) fn bitrate_arg(codec: LossyCodec, kbps: u32) -> String {
    let (low, high) = codec.bitrates();
    format!("{}k", kbps.clamp(low, high))
}

/// `ffmpeg` encoding the WAV file `source` to `out` (see the module docs). The output format is
/// named, never guessed from the file name (the file is written under a temporary name).
/// Progress goes to the standard output as `out_time_us=` lines ([`crate::audio::progress_us`]).
pub fn encode_args(source: &Path, out: &Path, spec: &EncodeSpec, tags: &Tags) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostats",
        "-progress",
        "pipe:1",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .map(os)
    .into();
    args.push(file_input(source));
    args.extend(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1"].map(os));
    // The export has brought the rate to one the format holds; if not, FFmpeg converts it.
    args.extend(
        [
            "-ar".to_owned(),
            spec.codec.output_rate(spec.rate).to_string(),
        ]
        .map(OsString::from),
    );
    if spec.channels > 2 {
        args.extend(["-ac", "2"].map(os));
    } else if spec.channels == 1 {
        // A mono WAV file carries the layout "front left" only, which the MP3 encoder would turn
        // into two channels: say one.
        args.extend(["-ac", "1"].map(os));
    }
    match spec.codec {
        LossyCodec::Mp3 => {
            args.extend(["-c:a", "libmp3lame"].map(os));
            match spec.quality {
                LossyQuality::Cbr(kbps) => {
                    args.extend(
                        ["-b:a".to_owned(), bitrate_arg(spec.codec, kbps)].map(OsString::from),
                    );
                }
                LossyQuality::Vbr(q) => {
                    args.extend(["-q:a".to_owned(), q.min(9).to_string()].map(OsString::from));
                }
            }
            args.extend(["-id3v2_version", "3"].map(os));
        }
        LossyCodec::Aac => {
            args.extend(["-c:a", "aac"].map(os));
            args.extend(
                [
                    "-b:a".to_owned(),
                    bitrate_arg(spec.codec, quality_kbps(spec.quality, 192)),
                ]
                .map(OsString::from),
            );
        }
        LossyCodec::Opus => {
            args.extend(["-c:a", "libopus", "-vbr", "on", "-application", "audio"].map(os));
            args.extend(
                [
                    "-b:a".to_owned(),
                    bitrate_arg(spec.codec, quality_kbps(spec.quality, 128)),
                ]
                .map(OsString::from),
            );
        }
    }
    for (key, value) in [("title", &tags.title), ("artist", &tags.author)] {
        if let Some(value) = clean_tag(value) {
            args.push(os("-metadata"));
            args.push(OsString::from(format!("{key}={value}")));
        }
    }
    let format = match spec.codec {
        LossyCodec::Mp3 => "mp3",
        LossyCodec::Aac => "ipod",
        LossyCodec::Opus => "opus",
    };
    args.extend(["-f", format, "-y"].map(os));
    args.push(file_input(out));
    args
}

/// The bitrate of a quality that has one (`fallback` for a variable-bitrate quality, which only
/// MP3 uses).
fn quality_kbps(quality: LossyQuality, fallback: u32) -> u32 {
    match quality {
        LossyQuality::Cbr(kbps) => kbps,
        LossyQuality::Vbr(_) => fallback,
    }
}

/// `<file name>.partial` next to `path`: what the encoder writes to.
pub fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

/// Removes the file unless the job ended well.
pub(crate) struct Partial<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> Partial<'a> {
    /// Removes `path` when dropped, unless [`Partial::disarm`] was called.
    pub(crate) fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    /// The job ended well: the file stays.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// One encoding job.
#[derive(Debug, Clone, Copy)]
pub struct EncodeJob<'a> {
    /// The WAV file.
    pub source: &'a Path,
    /// The compressed file to write.
    pub out: &'a Path,
    /// The format and quality.
    pub spec: EncodeSpec,
    /// The song information for the tags.
    pub tags: &'a Tags,
    /// The length of the audio in seconds (progress is measured against it).
    pub duration_s: f64,
}

/// Encodes `job.source` to `job.out` (see the module docs). `progress` receives the fraction done,
/// `0.0..=1.0`, as FFmpeg reports it; `cancel` stops the job (FFmpeg is killed first).
///
/// # Errors
/// [`VideoError::Cancelled`] when `cancel` is set (nothing is left behind), the failure of ffmpeg
/// with the end of its error output (a build without the encoder says so there), and
/// [`VideoError::Io`] when the file cannot be put in place.
pub fn encode_audio(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    job: &EncodeJob<'_>,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), VideoError> {
    let partial = partial_path(job.out);
    let mut guard = Partial {
        path: &partial,
        armed: true,
    };
    // A leftover of an earlier run must not pass for this one's output.
    let _ = fs::remove_file(&partial);
    let total_us = (job.duration_s * 1e6).max(1.0);
    progress(0.0);
    let mut pending = String::new();
    let exit = tool.run(
        &ffmpeg.ffmpeg,
        &encode_args(job.source, &partial, &job.spec, job.tags),
        &mut |chunk| {
            pending.push_str(&String::from_utf8_lossy(chunk));
            while let Some(end) = pending.find('\n') {
                if let Some(us) = crate::audio::progress_us(&pending[..end]) {
                    progress((us as f64 / total_us).clamp(0.0, 1.0) as f32);
                }
                pending.drain(..=end);
            }
            if pending.len() > 4096 {
                pending.clear();
            }
            true
        },
        cancel,
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err(VideoError::Cancelled);
    }
    if !exit.success {
        return Err(exit.failure(&ffmpeg.ffmpeg));
    }
    if fs::metadata(&partial)?.len() == 0 {
        return Err(VideoError::Proxy(format!(
            "ffmpeg wrote no {} sound",
            job.spec.codec.name()
        )));
    }
    fs::rename(&partial, job.out)?;
    guard.armed = false;
    progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeTool;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn spec(codec: LossyCodec, quality: LossyQuality) -> EncodeSpec {
        EncodeSpec {
            codec,
            quality,
            rate: 44_100,
            channels: 2,
        }
    }

    fn args_of(codec: LossyCodec, quality: LossyQuality, tags: &Tags) -> Vec<String> {
        strings(&encode_args(
            Path::new("tmp dir/mix.wav"),
            Path::new("out dir/song.mp3.partial"),
            &spec(codec, quality),
            tags,
        ))
    }

    fn has(args: &[String], pair: [&str; 2]) -> bool {
        args.windows(2).any(|w| w == pair)
    }

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
            "atomik-video-encode-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn mp3_constant_bitrates_are_a_list_of_arguments() {
        for kbps in [128u32, 192, 256, 320] {
            let a = args_of(LossyCodec::Mp3, LossyQuality::Cbr(kbps), &Tags::default());
            assert_eq!(a[0], "-nostdin");
            assert!(has(&a, ["-c:a", "libmp3lame"]), "{a:?}");
            assert!(has(&a, ["-b:a", &format!("{kbps}k")]), "{a:?}");
            assert!(has(&a, ["-f", "mp3"]));
            assert!(has(&a, ["-ar", "44100"]));
            assert!(!a.contains(&"-q:a".to_owned()));
            assert!(has(&a, ["-protocol_whitelist", "file"]));
            let at = a.iter().position(|x| x == "-i").unwrap();
            assert_eq!(a[at + 1], "file:tmp dir/mix.wav");
            assert_eq!(a.last().unwrap(), "file:out dir/song.mp3.partial");
        }
    }

    #[test]
    fn mp3_variable_bitrate_is_a_quality_not_a_bitrate() {
        for q in [0u8, 2, 4] {
            let a = args_of(LossyCodec::Mp3, LossyQuality::Vbr(q), &Tags::default());
            assert!(has(&a, ["-q:a", &q.to_string()]), "{a:?}");
            assert!(!a.contains(&"-b:a".to_owned()));
        }
        let a = args_of(LossyCodec::Mp3, LossyQuality::Vbr(99), &Tags::default());
        assert!(has(&a, ["-q:a", "9"]), "the scale stops at 9");
    }

    #[test]
    fn aac_goes_into_an_m4a_container_at_the_asked_bitrate() {
        for kbps in [128u32, 192, 256, 320] {
            let a = args_of(LossyCodec::Aac, LossyQuality::Cbr(kbps), &Tags::default());
            assert!(has(&a, ["-c:a", "aac"]), "{a:?}");
            assert!(has(&a, ["-b:a", &format!("{kbps}k")]), "{a:?}");
            assert!(has(&a, ["-f", "ipod"]));
        }
    }

    #[test]
    fn opus_goes_into_an_ogg_file_at_the_asked_bitrate() {
        for kbps in [64u32, 96, 128, 160, 192] {
            let a = args_of(LossyCodec::Opus, LossyQuality::Cbr(kbps), &Tags::default());
            assert!(has(&a, ["-c:a", "libopus"]), "{a:?}");
            assert!(has(&a, ["-vbr", "on"]));
            assert!(has(&a, ["-b:a", &format!("{kbps}k")]), "{a:?}");
            assert!(has(&a, ["-f", "opus"]));
        }
    }

    #[test]
    fn a_bitrate_outside_the_format_is_clamped_and_a_quality_never_leaks_into_aac() {
        let a = args_of(LossyCodec::Mp3, LossyQuality::Cbr(9_999), &Tags::default());
        assert!(has(&a, ["-b:a", "320k"]));
        let a = args_of(LossyCodec::Opus, LossyQuality::Cbr(0), &Tags::default());
        assert!(has(&a, ["-b:a", "6k"]));
        let a = args_of(LossyCodec::Aac, LossyQuality::Vbr(2), &Tags::default());
        assert!(has(&a, ["-b:a", "192k"]));
        assert!(!a.contains(&"-q:a".to_owned()));
    }

    #[test]
    fn a_rate_the_format_cannot_hold_is_converted_by_ffmpeg() {
        let a = args_of(LossyCodec::Opus, LossyQuality::Cbr(128), &Tags::default());
        assert!(has(&a, ["-ar", "48000"]), "{a:?}");
        let a = args_of(LossyCodec::Mp3, LossyQuality::Cbr(128), &Tags::default());
        assert!(has(&a, ["-ar", "44100"]), "{a:?}");
    }

    #[test]
    fn more_than_two_channels_are_mixed_down() {
        let mut s = spec(LossyCodec::Mp3, LossyQuality::Cbr(192));
        let a = strings(&encode_args(
            Path::new("a.wav"),
            Path::new("b.mp3"),
            &s,
            &Tags::default(),
        ));
        assert!(!a.contains(&"-ac".to_owned()));
        s.channels = 6;
        let a = strings(&encode_args(
            Path::new("a.wav"),
            Path::new("b.mp3"),
            &s,
            &Tags::default(),
        ));
        assert!(has(&a, ["-ac", "2"]));
    }

    #[test]
    fn one_channel_stays_one_channel() {
        let mut s = spec(LossyCodec::Mp3, LossyQuality::Cbr(128));
        s.channels = 1;
        let a = strings(&encode_args(
            Path::new("a.wav"),
            Path::new("b.mp3"),
            &s,
            &Tags::default(),
        ));
        assert!(has(&a, ["-ac", "1"]), "{a:?}");
    }

    #[test]
    fn the_tags_are_separate_arguments_and_nothing_goes_through_a_shell() {
        let tags = Tags {
            title: "Say \"hi\"; rm -rf $HOME `x` & a=b".to_owned(),
            author: "-f lavfi\nsecond line\ttab".to_owned(),
        };
        let a = args_of(LossyCodec::Mp3, LossyQuality::Cbr(192), &tags);
        // Each tag is the value of its own -metadata argument.
        let at: Vec<usize> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "-metadata")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 2);
        assert_eq!(
            a[at[0] + 1],
            "title=Say \"hi\"; rm -rf $HOME `x` & a=b",
            "quotes, semicolons, dollars and equals signs are plain text"
        );
        assert_eq!(
            a[at[1] + 1],
            "artist=-f lavfi second line tab",
            "a line break cannot start a new argument or a new tag"
        );
        // No argument is a flag made of tag text.
        assert!(!a.contains(&"-f lavfi".to_owned()));
        // The output is still the last argument.
        assert!(a.last().unwrap().starts_with("file:"));
    }

    #[test]
    fn empty_tags_are_not_written_and_text_is_cleaned_and_capped() {
        let a = args_of(LossyCodec::Opus, LossyQuality::Cbr(128), &Tags::default());
        assert!(!a.contains(&"-metadata".to_owned()));
        let blank = Tags {
            title: "  \n\t ".to_owned(),
            author: "Zoë Ünïcode 音楽".to_owned(),
        };
        let a = args_of(LossyCodec::Aac, LossyQuality::Cbr(128), &blank);
        assert_eq!(a.iter().filter(|x| *x == "-metadata").count(), 1);
        assert!(a.contains(&"artist=Zoë Ünïcode 音楽".to_owned()));
        assert_eq!(
            clean_tag(&"x".repeat(1000)).unwrap().chars().count(),
            MAX_TAG_CHARS
        );
        assert_eq!(clean_tag("\u{7}bell\u{0}").as_deref(), Some("bell"));
        assert_eq!(clean_tag("   "), None);
    }

    #[test]
    fn the_rate_is_one_the_format_can_hold() {
        assert_eq!(LossyCodec::Mp3.output_rate(44_100), 44_100);
        assert_eq!(LossyCodec::Mp3.output_rate(48_000), 48_000);
        assert_eq!(LossyCodec::Mp3.output_rate(88_200), 48_000);
        assert_eq!(LossyCodec::Mp3.output_rate(96_000), 48_000);
        assert_eq!(LossyCodec::Opus.output_rate(44_100), 48_000);
        assert_eq!(LossyCodec::Opus.output_rate(48_000), 48_000);
        assert_eq!(LossyCodec::Opus.output_rate(96_000), 48_000);
        assert_eq!(LossyCodec::Aac.output_rate(44_100), 44_100);
        assert_eq!(LossyCodec::Aac.output_rate(96_000), 96_000);
        assert_eq!(LossyCodec::Aac.output_rate(192_000), 96_000);
        assert_eq!(LossyCodec::Mp3.extension(), "mp3");
        assert_eq!(LossyCodec::Aac.extension(), "m4a");
        assert_eq!(LossyCodec::Opus.extension(), "opus");
    }

    fn job<'a>(wav: &'a Path, out: &'a Path, tags: &'a Tags) -> EncodeJob<'a> {
        EncodeJob {
            source: wav,
            out,
            spec: spec(LossyCodec::Mp3, LossyQuality::Cbr(192)),
            tags,
            duration_s: 12.021,
        }
    }

    #[test]
    fn the_file_appears_complete_and_progress_runs_to_one() {
        let dir = folder("ok");
        let (wav, out) = (dir.join("mix.wav"), dir.join("song.mp3"));
        fs::write(&wav, b"RIFF").unwrap();
        let tool = FakeTool::new().audio(b"ID3encoded".to_vec());
        let seen = Mutex::new(Vec::new());
        let tags = Tags::default();
        encode_audio(
            &tool,
            &ffmpeg(),
            &job(&wav, &out, &tags),
            &AtomicBool::new(false),
            &|p| seen.lock().unwrap().push(p),
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"ID3encoded");
        assert!(!partial_path(&out).exists(), "the partial file was renamed");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&0.0));
        assert_eq!(seen.last(), Some(&1.0));
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        let calls = tool.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].exe, PathBuf::from("/bin/ffmpeg"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_existing_file_is_replaced_only_by_a_complete_one() {
        let dir = folder("replace");
        let (wav, out) = (dir.join("mix.wav"), dir.join("song.mp3"));
        fs::write(&wav, b"RIFF").unwrap();
        fs::write(&out, b"old").unwrap();
        let tags = Tags::default();
        let failing = FakeTool::new()
            .audio(b"half".to_vec())
            .stream_fails("Unknown encoder 'libmp3lame'");
        let err = encode_audio(
            &failing,
            &ffmpeg(),
            &job(&wav, &out, &tags),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("libmp3lame"), "{err}");
        assert_eq!(fs::read(&out).unwrap(), b"old", "the old file is untouched");
        assert!(!partial_path(&out).exists(), "no partial file is left");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stopped_job_leaves_no_file() {
        let dir = folder("cancel");
        let (wav, out) = (dir.join("mix.wav"), dir.join("song.mp3"));
        fs::write(&wav, b"RIFF").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let tool = FakeTool::new().audio(b"half".to_vec()).on_frame(move |n| {
            if n == 2 {
                flag.store(true, Ordering::Relaxed);
            }
        });
        let tags = Tags::default();
        let result = encode_audio(&tool, &ffmpeg(), &job(&wav, &out, &tags), &cancel, &|_| {});
        assert!(matches!(result, Err(VideoError::Cancelled)), "{result:?}");
        assert!(!out.exists());
        assert!(
            !partial_path(&out).exists(),
            "the half-written file is removed"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_output_is_an_error_and_leaves_nothing() {
        let dir = folder("empty");
        let (wav, out) = (dir.join("mix.wav"), dir.join("song.mp3"));
        fs::write(&wav, b"RIFF").unwrap();
        let tags = Tags::default();
        let err = encode_audio(
            &FakeTool::new().audio(Vec::new()),
            &ffmpeg(),
            &job(&wav, &out, &tags),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap_err();
        assert!(
            matches!(err, VideoError::Io(_) | VideoError::Proxy(_)),
            "{err}"
        );
        assert!(!out.exists() && !partial_path(&out).exists());
        let _ = fs::remove_dir_all(dir);
    }
}
