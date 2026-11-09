//! The argument lists of the FFmpeg calls, built as lists of separate arguments.
//!
//! No call goes through a shell and no path is put into a longer string, except for the
//! `file:` prefix that makes FFmpeg treat the name as a file (never as `http:`, `concat:` or
//! another protocol a hostile file name could spell). `-protocol_whitelist file` stops a file
//! (a playlist, say) from making FFmpeg open anything else.

use crate::probe::{AudioStream, ProbeInfo};
use std::ffi::OsString;
use std::path::Path;

/// How the proxy is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxySpec {
    /// The height of the proxy picture in pixels (a film that is smaller is not enlarged).
    pub height: u32,
    /// The JPEG quality as FFmpeg's `-q:v` scale: 2 is best, 31 is worst.
    pub quality: u8,
}

impl Default for ProxySpec {
    fn default() -> Self {
        Self {
            height: 360,
            quality: 5,
        }
    }
}

pub(crate) fn os(text: &str) -> OsString {
    OsString::from(text)
}

/// The input as one argument: `file:` and the path.
pub(crate) fn file_input(path: &Path) -> OsString {
    let mut arg = os("file:");
    arg.push(path.as_os_str());
    arg
}

/// `ffmpeg -version`.
pub fn version_args() -> Vec<OsString> {
    vec![os("-hide_banner"), os("-version")]
}

/// `ffprobe` printing the format and the streams of `source` as JSON.
pub fn probe_args(source: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .map(os)
    .into();
    args.push(file_input(source));
    args
}

/// The height the proxy picture gets: `spec.height`, no more than the film shows, even.
pub fn proxy_height(info: &ProbeInfo, spec: &ProxySpec) -> u32 {
    let shown = info.video.display_size().1;
    (spec.height.min(shown) & !1).max(2)
}

/// `ffmpeg` writing every frame of the picture of `source` as an independent JPEG on its
/// standard output: resampled to the nominal rate (so frame `n` is at `n / rate` whatever the
/// file's timestamps were), scaled to [`proxy_height`] keeping the aspect, upright.
pub fn proxy_args(source: &Path, info: &ProbeInfo, spec: &ProxySpec) -> Vec<OsString> {
    let filter = format!(
        "fps={},scale=-2:{}:flags=bicubic",
        info.video.frame_rate.arg(),
        proxy_height(info, spec)
    );
    let mut args: Vec<OsString> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .map(os)
    .into();
    args.push(file_input(source));
    args.extend(
        [
            "-map",
            &format!("0:{}", info.video.index),
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filter,
            "-c:v",
            "mjpeg",
            "-q:v",
            &spec.quality.to_string(),
            "-pix_fmt",
            "yuvj420p",
            "-f",
            "image2pipe",
            "pipe:1",
        ]
        .map(os),
    );
    args
}

/// `ffmpeg` writing the audio track `stream` of `source` to the WAV file `out`, 24 bit, at the
/// track's own rate (the song's import resamples it), mixed down to stereo when it has more than
/// two channels. Progress goes to the standard output as `out_time_us=` lines
/// ([`crate::audio::progress_us`]). An `rf64` header is written when the file passes 4 GiB.
pub fn audio_args(source: &Path, stream: &AudioStream, out: &Path) -> Vec<OsString> {
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
    args.extend(["-map", &format!("0:{}", stream.index), "-vn", "-sn", "-dn"].map(os));
    if stream.channels > 2 {
        args.extend(["-ac", "2"].map(os));
    }
    args.extend(["-c:a", "pcm_s24le", "-rf64", "auto", "-f", "wav", "-y"].map(os));
    args.push(file_input(out));
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::parse_probe;
    use crate::samples::{MP4_H264, NO_AUDIO, PHONE_ROTATED};
    use std::path::PathBuf;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_probe_arguments_are_a_list_with_the_path_as_one_argument() {
        let path = PathBuf::from("My Films/a film; rm -rf $HOME & `x`.mp4");
        let args = strings(&probe_args(&path));
        assert_eq!(
            args,
            [
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                "-protocol_whitelist",
                "file",
                "-i",
                "file:My Films/a film; rm -rf $HOME & `x`.mp4",
            ]
        );
    }

    #[test]
    fn a_name_that_looks_like_an_option_or_a_protocol_stays_a_file() {
        for name in ["-f lavfi", "http://example.invalid/x.mp4", "concat:a|b"] {
            let args = strings(&probe_args(Path::new(name)));
            assert_eq!(args.last().unwrap(), &format!("file:{name}"));
            let args = strings(&proxy_args(
                Path::new(name),
                &parse_probe(MP4_H264).unwrap(),
                &ProxySpec::default(),
            ));
            assert!(args.contains(&format!("file:{name}")));
            // The input name follows `-i`, so it can never be read as an option.
            let at = args.iter().position(|a| a == "-i").unwrap();
            assert_eq!(args[at + 1], format!("file:{name}"));
        }
    }

    #[test]
    fn the_proxy_arguments_never_read_input_and_pipe_jpeg_frames() {
        let info = parse_probe(MP4_H264).unwrap();
        let args = strings(&proxy_args(
            Path::new("film.mp4"),
            &info,
            &ProxySpec::default(),
        ));
        assert_eq!(args[0], "-nostdin");
        assert!(args.windows(2).any(|w| w == ["-map", "0:0"]));
        assert!(args.windows(2).any(|w| w == ["-c:v", "mjpeg"]));
        assert!(args.windows(2).any(|w| w == ["-f", "image2pipe"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["-protocol_whitelist", "file"])
        );
        assert!(args.contains(&"-an".to_owned()));
        assert_eq!(args.last().unwrap(), "pipe:1");
        let filter = args.iter().position(|a| a == "-vf").unwrap();
        assert_eq!(args[filter + 1], "fps=25/1,scale=-2:360:flags=bicubic");
    }

    #[test]
    fn the_height_never_enlarges_and_is_even() {
        let spec = ProxySpec::default();
        assert_eq!(proxy_height(&parse_probe(MP4_H264).unwrap(), &spec), 360);
        // 720 high shows at 360; a 5 high film would stay 4.
        assert_eq!(proxy_height(&parse_probe(NO_AUDIO).unwrap(), &spec), 360);
        let mut small = parse_probe(NO_AUDIO).unwrap();
        small.video.height = 245;
        assert_eq!(proxy_height(&small, &spec), 244);
        // A sideways film is judged by the height it shows (1920), not the stored one.
        let phone = parse_probe(PHONE_ROTATED).unwrap();
        assert_eq!(proxy_height(&phone, &spec), 360);
        let mut tiny = parse_probe(MP4_H264).unwrap();
        tiny.video.height = 1;
        assert_eq!(proxy_height(&tiny, &spec), 2);
    }

    #[test]
    fn a_variable_rate_film_is_resampled_to_its_nominal_rate_and_maps_its_stream() {
        let info = parse_probe(PHONE_ROTATED).unwrap();
        let args = strings(&proxy_args(
            Path::new("p.mov"),
            &info,
            &ProxySpec::default(),
        ));
        let filter = args.iter().position(|a| a == "-vf").unwrap();
        assert!(args[filter + 1].starts_with("fps=2983/100,"));
        assert!(args.windows(2).any(|w| w == ["-map", "0:0"]));
    }

    #[test]
    fn the_audio_arguments_pick_one_track_and_write_a_wav_file() {
        let info = parse_probe(MP4_H264).unwrap();
        let args = strings(&audio_args(
            Path::new("My Films/a film.mp4"),
            &info.audio[0],
            Path::new("tmp dir/out file.wav"),
        ));
        assert_eq!(args[0], "-nostdin");
        let at = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[at + 1], "file:My Films/a film.mp4");
        assert!(args.windows(2).any(|w| w == ["-map", "0:1"]));
        assert!(args.windows(2).any(|w| w == ["-progress", "pipe:1"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_s24le"]));
        assert!(args.windows(2).any(|w| w == ["-f", "wav"]));
        assert!(args.contains(&"-vn".to_owned()));
        // No rate is set: the import resamples to the song's rate.
        assert!(!args.contains(&"-ar".to_owned()));
        // Stereo stays stereo.
        assert!(!args.contains(&"-ac".to_owned()));
        assert_eq!(args.last().unwrap(), "file:tmp dir/out file.wav");
    }

    #[test]
    fn a_surround_track_is_mixed_down_to_stereo_and_a_mono_one_is_kept() {
        let info = parse_probe(crate::samples::MKV_HEVC).unwrap();
        let surround = strings(&audio_args(
            Path::new("f.mkv"),
            &info.audio[0],
            Path::new("o.wav"),
        ));
        assert!(surround.windows(2).any(|w| w == ["-ac", "2"]));
        assert!(surround.windows(2).any(|w| w == ["-map", "0:1"]));
        let second = strings(&audio_args(
            Path::new("f.mkv"),
            &info.audio[1],
            Path::new("o.wav"),
        ));
        assert!(second.windows(2).any(|w| w == ["-map", "0:2"]));
        let mono = parse_probe(crate::samples::MOV_PRORES).unwrap();
        let args = strings(&audio_args(
            Path::new("f.mov"),
            &mono.audio[0],
            Path::new("o.wav"),
        ));
        assert!(!args.contains(&"-ac".to_owned()));
    }

    #[test]
    fn the_version_call_has_no_path() {
        assert_eq!(strings(&version_args()), ["-hide_banner", "-version"]);
    }
}
