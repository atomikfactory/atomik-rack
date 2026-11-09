//! Film export: a new film made of the song's mix and the picture of the song's film, written by
//! the user's FFmpeg.
//!
//! The export renders the mix to a temporary WAV file; [`export_film`] puts it together with the
//! film. The picture is either **copied** without re-encoding or **re-encoded** (H.264, with a
//! quality choice); the mix goes in as AAC in an MP4 file or as PCM in a MOV file; the film's own
//! sound can stay as a second track. Every call goes through the same [`VideoTool`] as the other
//! FFmpeg calls: the arguments are a list ([`film_args`]), no shell, paths as `file:` inputs.
//!
//! # Time
//!
//! [`FilmCut::layout`] turns where the film plays in the song (start, trim) and what part of the
//! song the mix covers into the numbers of the call, in integer microseconds: the part of the film
//! that plays, the black before and after it (only when the whole song is exported), how long the
//! mix waits before its first sample, and the length of the result. The mix's first sample sits at
//! the film's first exported frame (or exactly as far after it as the song's start is after the
//! film's, in samples): it is delayed by the cut's `mix_delay_us`, and a mix that starts before the
//! film (a film that begins after the song's start, whole song) comes with black pictures in front.
//!
//! # Copy or re-encode
//!
//! A copied picture is cut at the packets of the file, and a packet can only start at a
//! keyframe: a cut that is not at the start of the film **snaps to a keyframe**, so the picture
//! can begin a little earlier than the trim says while the mix still starts at the trim. Copy is
//! therefore used automatically only when the cut is at the film's start ([`PictureChoice::Auto`]);
//! choose re-encode for exact cuts. Black before or after the film always needs a re-encode.
//!
//! The file is written under a temporary name next to the target and renamed when FFmpeg has
//! finished, so a stopped or failed job leaves nothing behind and an existing film is replaced
//! only by a complete one.

use crate::VideoError;
use crate::args::{file_input, os};
use crate::discover::Ffmpeg;
use crate::encode::{LossyCodec, Partial, Tags, bitrate_arg, clean_tag, partial_path};
use crate::probe::{ProbeInfo, Rational};
use crate::runner::VideoTool;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The container of the new film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmContainer {
    /// An MP4 file (AAC sound).
    Mp4,
    /// A MOV file (PCM sound).
    Mov,
}

impl FilmContainer {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }

    /// The name of the format for FFmpeg's `-f`.
    fn format(self) -> &'static str {
        self.extension()
    }

    /// The video codecs that go into the container without being re-encoded.
    fn copies(self) -> &'static [&'static str] {
        match self {
            Self::Mp4 => &["h264", "hevc", "av1", "mpeg4", "vp9"],
            Self::Mov => &["h264", "hevc", "prores", "mjpeg", "mpeg4", "dnxhd"],
        }
    }
}

/// The sound of the new film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmSound {
    /// AAC at this many kilobits per second, in an MP4 file.
    Aac(u32),
    /// 24-bit PCM, in a MOV file.
    Pcm,
}

impl FilmSound {
    /// The container the sound goes in.
    pub fn container(self) -> FilmContainer {
        match self {
            Self::Aac(_) => FilmContainer::Mp4,
            Self::Pcm => FilmContainer::Mov,
        }
    }
}

/// How hard the encoder squeezes a re-encoded picture (H.264, constant quality).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmQuality {
    /// Nearly indistinguishable from the source, big.
    High,
    /// A good picture at a moderate size.
    Standard,
    /// Small, with visible softening.
    Small,
}

impl FilmQuality {
    /// The constant rate factor of the H.264 encoder (lower is better).
    pub fn crf(self) -> u8 {
        match self {
            Self::High => 18,
            Self::Standard => 23,
            Self::Small => 28,
        }
    }

    /// The name of the quality for a message.
    pub fn name(self) -> &'static str {
        match self {
            Self::High => "high quality",
            Self::Standard => "standard quality",
            Self::Small => "small size",
        }
    }
}

/// What was asked for the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureChoice {
    /// Copy it when the cut points allow it (see the module docs), else re-encode it at this
    /// quality.
    Auto(FilmQuality),
    /// Always copy it (cuts snap to keyframes); refused when the picture needs black.
    Copy,
    /// Always re-encode it, cut exactly.
    Encode(FilmQuality),
}

/// What happens to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmPicture {
    /// Copied packet by packet.
    Copy,
    /// Re-encoded to H.264.
    Encode(FilmQuality),
}

/// The numbers of one film export, in integer microseconds (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmCut {
    /// Where in the source the picture that plays begins.
    pub in_us: u64,
    /// How much of the source plays.
    pub length_us: u64,
    /// Black picture before the film.
    pub black_before_us: u64,
    /// Black picture after the film.
    pub black_after_us: u64,
    /// How long the mix waits before its first sample.
    pub mix_delay_us: u64,
    /// The length of the new film.
    pub total_us: u64,
}

impl FilmCut {
    /// The cut of a film that plays at `start_us` of the song (negative: before the song's start)
    /// with the trim `in_us..out_us` of the source, for a mix whose first sample is song time
    /// `mix_start_us` and which is `mix_len_us` long.
    ///
    /// With `whole_song` false the new film is exactly the film's trimmed span, the mix being cut
    /// or padded with silence to it. With `whole_song` true it covers the film and the mix both
    /// (the earliest start to the latest end), with black pictures where only the mix runs; a
    /// black stretch shorter than half a frame (`frame_us` is the frame's length) is dropped.
    pub fn layout(
        (start_us, in_us, out_us): (i64, u64, u64),
        (mix_start_us, mix_len_us): (i64, u64),
        whole_song: bool,
        frame_us: u64,
    ) -> Self {
        let length_us = out_us.saturating_sub(in_us);
        let end_us = start_us.saturating_add(i64::try_from(length_us).unwrap_or(i64::MAX));
        let mix_end_us = mix_start_us.saturating_add(i64::try_from(mix_len_us).unwrap_or(i64::MAX));
        let (from, to) = if whole_song {
            (start_us.min(mix_start_us), end_us.max(mix_end_us))
        } else {
            (start_us, end_us)
        };
        let gap = |us: i64| {
            let us = u64::try_from(us).unwrap_or(0);
            if us.saturating_mul(2) < frame_us {
                0
            } else {
                us
            }
        };
        Self {
            in_us,
            length_us,
            black_before_us: gap(start_us - from),
            black_after_us: gap(to - end_us),
            mix_delay_us: u64::try_from(mix_start_us - from).unwrap_or(0),
            total_us: u64::try_from(to - from).unwrap_or(0),
        }
    }

    /// The picture needs black before or after the film, so it cannot be copied.
    pub fn needs_black(&self) -> bool {
        self.black_before_us > 0 || self.black_after_us > 0
    }
}

/// What to make of the film besides the cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmSpec {
    /// What happens to the picture.
    pub picture: FilmPicture,
    /// The sound of the mix (and so the container).
    pub sound: FilmSound,
    /// Keep the film's own sound as a second track (the film's first audio track).
    pub own_audio: bool,
    /// The sample rate of the WAV file with the mix, which is also the rate of every sound track
    /// of the result (the export has brought it to one the sound codec holds).
    pub rate: u32,
    /// The channels of the WAV file; more than two are mixed down to stereo.
    pub channels: usize,
}

/// Decides what happens to the picture: `choice`, as far as the cut and the film allow.
///
/// # Errors
/// [`VideoError::Film`] when a copy was asked for and the picture needs black, or its codec does
/// not go into the container without a re-encode.
pub fn plan_picture(
    choice: PictureChoice,
    info: &ProbeInfo,
    container: FilmContainer,
    cut: &FilmCut,
) -> Result<FilmPicture, VideoError> {
    let fits = container.copies().contains(&info.video.codec.as_str());
    let at_start = cut.in_us == 0;
    match choice {
        PictureChoice::Encode(quality) => Ok(FilmPicture::Encode(quality)),
        PictureChoice::Auto(_) if fits && at_start && !cut.needs_black() => Ok(FilmPicture::Copy),
        PictureChoice::Auto(quality) => Ok(FilmPicture::Encode(quality)),
        PictureChoice::Copy if cut.needs_black() => Err(VideoError::Film(
            "the picture has to be re-encoded to put black before or after the film: \
             choose RE-ENCODE, or export only the film's span"
                .to_owned(),
        )),
        PictureChoice::Copy if !fits => Err(VideoError::Film(format!(
            "a {} picture cannot be copied into a {} file: choose RE-ENCODE",
            info.video.codec,
            container.extension().to_uppercase()
        ))),
        PictureChoice::Copy => Ok(FilmPicture::Copy),
    }
}

/// Seconds as FFmpeg reads them, to the microsecond: `2.500000`.
fn seconds(us: u64) -> String {
    format!("{}.{:06}", us / 1_000_000, us % 1_000_000)
}

/// The delay filter for a mix of `channels` channels at `rate` Hz that waits `us` microseconds:
/// samples, so the delay is exact (a rounding to the nearest sample at most).
fn delay_filter(us: u64, rate: u32, channels: usize) -> Option<String> {
    let samples = (u128::from(us) * u128::from(rate) + 500_000) / 1_000_000;
    if samples == 0 {
        return None;
    }
    let one = format!("{samples}S");
    let all = vec![one; channels.max(1)];
    Some(format!("adelay={}", all.join("|")))
}

/// `ffmpeg` making the new film from `film` (trimmed as `cut` says) and the WAV file `mix`, to
/// `out` (see the module docs). The output format is named, never guessed from the file name (the
/// file is written under a temporary name). Progress goes to the standard output as
/// `out_time_us=` lines ([`crate::audio::progress_us`]).
pub fn film_args(
    (film, mix, out): (&Path, &Path, &Path),
    info: &ProbeInfo,
    spec: &FilmSpec,
    cut: &FilmCut,
    tags: &Tags,
) -> Vec<OsString> {
    let container = spec.sound.container();
    let mut args: Vec<OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error"]
        .map(os)
        .into();
    args.extend(["-nostats", "-progress", "pipe:1"].map(os));
    // The film: the trim is applied to the input, so the picture and its own sound stay in step.
    args.extend(["-protocol_whitelist", "file"].map(os));
    if cut.in_us > 0 {
        args.extend([os("-ss"), OsString::from(seconds(cut.in_us))]);
    }
    args.extend([os("-t"), OsString::from(seconds(cut.length_us))]);
    args.push(os("-i"));
    args.push(file_input(film));
    args.extend(["-protocol_whitelist", "file", "-i"].map(os));
    args.push(file_input(mix));

    let mut graph: Vec<String> = Vec::new();
    let video = format!("0:{}", info.video.index);
    // The picture.
    let picture_map = match spec.picture {
        FilmPicture::Copy => video,
        FilmPicture::Encode(_) if !cut.needs_black() => video,
        FilmPicture::Encode(_) => {
            let (w, h) = info.video.display_size();
            let rate = info.video.frame_rate.arg();
            let mut parts = Vec::new();
            let black = |us: u64, name: &str, graph: &mut Vec<String>| {
                graph.push(format!(
                    "color=c=black:s={w}x{h}:r={rate}:d={},format=yuv420p,setsar=1[{name}]",
                    seconds(us)
                ));
            };
            if cut.black_before_us > 0 {
                black(cut.black_before_us, "bb", &mut graph);
                parts.push("[bb]");
            }
            graph.push(format!(
                "[{video}]scale={w}:{h}:flags=bicubic,fps={rate},format=yuv420p,setsar=1[fv]"
            ));
            parts.push("[fv]");
            if cut.black_after_us > 0 {
                black(cut.black_after_us, "ba", &mut graph);
                parts.push("[ba]");
            }
            graph.push(format!(
                "{}concat=n={}:v=1:a=0[v]",
                parts.concat(),
                parts.len()
            ));
            "[v]".to_owned()
        }
    };
    // The mix, delayed to its place and padded so the sound is as long as the film.
    let mut mix_chain = Vec::new();
    mix_chain.extend(delay_filter(cut.mix_delay_us, spec.rate, spec.channels));
    mix_chain.push("apad".to_owned());
    graph.push(format!("[1:a:0]{}[mix]", mix_chain.join(",")));
    // The film's own sound, delayed by the black in front of the picture.
    let own = spec
        .own_audio
        .then(|| info.audio.first())
        .flatten()
        .map(|track| {
            let delay = delay_filter(
                cut.black_before_us,
                track.sample_rate,
                usize::from(track.channels),
            );
            match delay {
                Some(delay) => {
                    graph.push(format!("[0:{}]{delay}[own]", track.index));
                    "[own]".to_owned()
                }
                None => format!("0:{}", track.index),
            }
        });
    args.extend([os("-filter_complex"), OsString::from(graph.join(";"))]);
    args.extend([os("-map"), OsString::from(picture_map)]);
    args.extend(["-map", "[mix]"].map(os));
    if let Some(own) = &own {
        args.extend([os("-map"), OsString::from(own)]);
    }
    args.extend(["-map_metadata", "-1", "-map_chapters", "-1"].map(os));
    match spec.picture {
        FilmPicture::Copy => args.extend(["-c:v", "copy"].map(os)),
        FilmPicture::Encode(quality) => args.extend(
            [
                "-c:v".to_owned(),
                "libx264".to_owned(),
                "-preset".to_owned(),
                "medium".to_owned(),
                "-crf".to_owned(),
                quality.crf().to_string(),
                "-pix_fmt".to_owned(),
                "yuv420p".to_owned(),
            ]
            .map(OsString::from),
        ),
    }
    match spec.sound {
        FilmSound::Aac(kbps) => {
            args.extend(["-c:a", "aac"].map(os));
            args.extend(
                [
                    "-b:a".to_owned(),
                    bitrate_arg(LossyCodec::Aac, kbps),
                    "-ar".to_owned(),
                    LossyCodec::Aac.output_rate(spec.rate).to_string(),
                ]
                .map(OsString::from),
            );
        }
        FilmSound::Pcm => {
            args.extend(["-c:a", "pcm_s24le"].map(os));
            args.extend([os("-ar"), OsString::from(spec.rate.to_string())]);
        }
    }
    if spec.channels > 2 {
        args.extend(["-ac", "2"].map(os));
    }
    args.extend(
        [
            "-metadata:s:a:0",
            "title=Mix",
            "-disposition:a:0",
            "default",
        ]
        .map(os),
    );
    if own.is_some() {
        args.extend(
            [
                "-metadata:s:a:1",
                "title=Film audio",
                "-disposition:a:1",
                "0",
            ]
            .map(os),
        );
    }
    for (key, value) in [("title", &tags.title), ("artist", &tags.author)] {
        if let Some(value) = clean_tag(value) {
            args.push(os("-metadata"));
            args.push(OsString::from(format!("{key}={value}")));
        }
    }
    args.extend([os("-t"), OsString::from(seconds(cut.total_us))]);
    if container == FilmContainer::Mp4 {
        args.extend(["-movflags", "+faststart"].map(os));
    }
    args.extend(["-f", container.format(), "-y"].map(os));
    args.push(file_input(out));
    args
}

/// One film export.
#[derive(Debug, Clone, Copy)]
pub struct FilmJob<'a> {
    /// The song's film.
    pub film: &'a Path,
    /// The WAV file with the mix.
    pub mix: &'a Path,
    /// The new film to write.
    pub out: &'a Path,
    /// What the probe found in the film.
    pub info: &'a ProbeInfo,
    /// What to make of it.
    pub spec: FilmSpec,
    /// The numbers of the cut.
    pub cut: FilmCut,
    /// The song information for the tags.
    pub tags: &'a Tags,
}

/// Makes the new film (see the module docs). `progress` receives the fraction done, `0.0..=1.0`,
/// as FFmpeg reports it; `cancel` stops the job (FFmpeg is killed first).
///
/// # Errors
/// [`VideoError::Cancelled`] when `cancel` is set (nothing is left behind), the failure of ffmpeg
/// with the end of its error output (a build without the encoder says so there), and
/// [`VideoError::Io`] when the file cannot be put in place.
pub fn export_film(
    tool: &dyn VideoTool,
    ffmpeg: &Ffmpeg,
    job: &FilmJob<'_>,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), VideoError> {
    let partial = partial_path(job.out);
    let mut guard = Partial::new(&partial);
    // A leftover of an earlier run must not pass for this one's output.
    let _ = fs::remove_file(&partial);
    let total_us = (job.cut.total_us as f64).max(1.0);
    progress(0.0);
    let mut pending = String::new();
    let exit = tool.run(
        &ffmpeg.ffmpeg,
        &film_args(
            (job.film, job.mix, &partial),
            job.info,
            &job.spec,
            &job.cut,
            job.tags,
        ),
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
        return Err(VideoError::Film("ffmpeg wrote no film".to_owned()));
    }
    fs::rename(&partial, job.out)?;
    guard.disarm();
    progress(1.0);
    Ok(())
}

/// The frame length of a rate in microseconds (rounded; at least 1).
pub fn frame_us(rate: Rational) -> u64 {
    ((1_000_000.0 / rate.as_f64()).round() as u64).max(1)
}

#[cfg(test)]
mod tests;
