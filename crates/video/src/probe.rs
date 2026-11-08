//! Reading what `ffprobe` says about a film.
//!
//! The call is `ffprobe -v error -print_format json -show_format -show_streams` (see
//! [`crate::args::probe_args`]); [`parse_probe`] turns its JSON into a [`ProbeInfo`]. The parser
//! is forgiving about what containers leave out (a stream without a duration takes the
//! container's, a Matroska file states its duration in upper-case tags) and strict about what a
//! film cannot do without: a picture and a frame rate.
//!
//! **Frame rate.** `avg_frame_rate` is the rate of the frames as they run; `r_frame_rate` is the
//! lowest rate all timestamps are a multiple of, which for a phone's variable-rate file is a
//! tick rate (600). The nominal rate is the average when it is valid, else `r_frame_rate`; a
//! rate within 0.02 % of a standard one (23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60) snaps to
//! it exactly; [`VideoStream::variable_rate`] says the two disagree by more than 1 %, and the
//! proxy then resamples to the nominal rate.

use crate::VideoError;
use serde_json::Value;

/// A frame rate as the fraction of two whole numbers (`30000/1001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    /// The numerator.
    pub num: u32,
    /// The denominator (never 0).
    pub den: u32,
}

impl Rational {
    /// The rate as a number.
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// Reads `30000/1001` or `25`; `None` for `0/0`, a zero rate or text that is no fraction.
    pub fn parse(text: &str) -> Option<Self> {
        let (num, den) = match text.trim().split_once('/') {
            Some((n, d)) => (n.trim().parse::<u32>().ok()?, d.trim().parse::<u32>().ok()?),
            None => (text.trim().parse::<u32>().ok()?, 1),
        };
        (num > 0 && den > 0).then_some(Self { num, den })
    }

    /// The `30000/1001` form an FFmpeg argument takes.
    pub fn arg(self) -> String {
        format!("{}/{}", self.num, self.den)
    }
}

/// The standard rates a measured one snaps to.
const STANDARD_RATES: [(u32, u32); 9] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (48, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// `rate`, or the nearest standard rate when one is within 0.02 % of it.
fn snap_rate(rate: Rational) -> Rational {
    let value = rate.as_f64();
    STANDARD_RATES
        .iter()
        .map(|&(num, den)| Rational { num, den })
        .map(|s| (s, (s.as_f64() - value).abs() / value))
        .filter(|(_, error)| *error < 0.0002)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(rate, |(s, _)| s)
}

/// The film's picture.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    /// The stream number in the file (what `-map 0:N` takes).
    pub index: u32,
    /// The codec's short name (`h264`, `prores`).
    pub codec: String,
    /// The width of the stored picture.
    pub width: u32,
    /// The height of the stored picture.
    pub height: u32,
    /// The degrees (0, 90, 180 or 270, counter-clockwise, as the display matrix states them) the
    /// picture is turned by when shown;
    /// FFmpeg applies it when it decodes, so a proxy is already upright.
    pub rotation: u16,
    /// The nominal frame rate (see the module docs).
    pub frame_rate: Rational,
    /// The frames do not run at one rate.
    pub variable_rate: bool,
    /// The number of frames, when the file states it.
    pub frames: Option<u64>,
}

impl VideoStream {
    /// The size the picture shows at: the stored size, with width and height swapped for a
    /// quarter turn.
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation == 90 || self.rotation == 270 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// One audio track of the file (item for the import of a film's sound).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    /// The stream number in the file.
    pub index: u32,
    /// The codec's short name.
    pub codec: String,
    /// The number of channels.
    pub channels: u16,
    /// The sample rate in hertz.
    pub sample_rate: u32,
    /// The language tag (`eng`), when there is one that says something.
    pub language: Option<String>,
    /// The title the file gives the track.
    pub title: Option<String>,
}

/// What a probe found.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    /// The container's names (`mov,mp4,m4a,3gp,3g2,mj2`).
    pub container: String,
    /// The length of the film in seconds.
    pub duration_s: f64,
    /// Where the picture starts on the file's clock, in seconds (a broadcast file starts at 3600).
    pub start_time_s: f64,
    /// The start timecode (`01:00:00:00`), when the file has one.
    pub timecode: Option<String>,
    /// The picture.
    pub video: VideoStream,
    /// The audio tracks, in file order.
    pub audio: Vec<AudioStream>,
}

impl ProbeInfo {
    /// The number of frames the proxy will hold, as far as the probe can tell: the stated count
    /// when the rate is steady, else the length times the nominal rate (at least 1).
    pub fn expected_frames(&self) -> u64 {
        let counted = (self.duration_s * self.video.frame_rate.as_f64()).round() as u64;
        match self.video.frames {
            Some(n) if !self.video.variable_rate && n > 0 => n,
            _ => counted.max(1),
        }
    }
}

/// A number that ffprobe wrote as a JSON number or as text.
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// The value of tag `name` of `owner` (its `tags` object), ignoring case.
fn tag<'a>(owner: &'a Value, name: &str) -> Option<&'a str> {
    owner
        .get("tags")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `00:01:30.063` as seconds.
fn clock_seconds(text: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in text.trim().split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(seconds).filter(|s| s.is_finite() && *s >= 0.0)
}

fn count(v: Option<&Value>) -> Option<u64> {
    number(v).filter(|n| *n >= 0.0).map(|n| n as u64)
}

/// The degrees of a stream, from its display matrix or its `rotate` tag, as 0, 90, 180 or 270.
fn rotation_of(stream: &Value) -> u16 {
    let from_matrix = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|d| number(d.get("rotation"))));
    // The old `rotate` tag counts clockwise, the display matrix counter-clockwise.
    let degrees = from_matrix.or_else(|| {
        tag(stream, "rotate")
            .and_then(|t| t.parse::<f64>().ok())
            .map(|clockwise| -clockwise)
    });
    let turns = (degrees.unwrap_or(0.0) / 90.0).round() as i64;
    (turns.rem_euclid(4) * 90) as u16
}

fn is_attached_picture(stream: &Value) -> bool {
    stream
        .get("disposition")
        .and_then(|d| number(d.get("attached_pic")))
        .is_some_and(|n| n > 0.0)
}

/// Reads the JSON `ffprobe` printed.
///
/// # Errors
/// [`VideoError::NoVideo`] when no stream is a picture (cover art does not count) and
/// [`VideoError::Probe`] when the text is not JSON or lacks a size, a frame rate or a length.
pub fn parse_probe(json: &str) -> Result<ProbeInfo, VideoError> {
    let root: Value =
        serde_json::from_str(json).map_err(|e| VideoError::Probe(format!("not JSON ({e})")))?;
    let streams: &[Value] = root
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let format = root.get("format").cloned().unwrap_or(Value::Null);
    let of_type = |kind: &'static str| {
        streams
            .iter()
            .filter(move |s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
    };
    let video = of_type("video")
        .find(|s| !is_attached_picture(s))
        .ok_or(VideoError::NoVideo)?;
    let bad = |what: &str| VideoError::Probe(format!("the picture has no {what}"));
    let width = count(video.get("width"))
        .filter(|w| *w > 0)
        .ok_or_else(|| bad("width"))? as u32;
    let height = count(video.get("height"))
        .filter(|h| *h > 0)
        .ok_or_else(|| bad("height"))? as u32;
    let valid = |key: &str| {
        video
            .get(key)
            .and_then(Value::as_str)
            .and_then(Rational::parse)
    };
    let (average, steady) = (valid("avg_frame_rate"), valid("r_frame_rate"));
    let nominal = average.or(steady).ok_or_else(|| bad("frame rate"))?;
    let variable_rate = match (average, steady) {
        (Some(a), Some(s)) => (a.as_f64() - s.as_f64()).abs() / s.as_f64() > 0.01,
        _ => false,
    };
    let frames = count(video.get("nb_frames"))
        .or_else(|| tag(video, "NUMBER_OF_FRAMES").and_then(|t| t.parse().ok()))
        .filter(|n| *n > 0);
    let start_time_s = number(video.get("start_time"))
        .or_else(|| number(format.get("start_time")))
        .unwrap_or(0.0)
        .max(0.0);
    let duration_s = number(format.get("duration"))
        .or_else(|| number(video.get("duration")))
        .or_else(|| tag(video, "DURATION").and_then(clock_seconds))
        .filter(|d| *d > 0.0)
        .ok_or_else(|| VideoError::Probe("the file has no length".into()))?;
    let timecode = tag(video, "timecode")
        .or_else(|| tag(&format, "timecode"))
        .or_else(|| streams.iter().find_map(|s| tag(s, "timecode")))
        .map(str::to_owned);
    let audio = of_type("audio")
        .map(|s| {
            let language = tag(s, "language")
                .filter(|l| !l.eq_ignore_ascii_case("und"))
                .map(str::to_owned);
            AudioStream {
                index: count(s.get("index")).unwrap_or(0) as u32,
                codec: s
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                channels: count(s.get("channels"))
                    .unwrap_or(0)
                    .min(u64::from(u16::MAX)) as u16,
                sample_rate: count(s.get("sample_rate")).unwrap_or(0) as u32,
                language,
                title: tag(s, "title").map(str::to_owned),
            }
        })
        .collect();
    Ok(ProbeInfo {
        container: format
            .get("format_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        duration_s,
        start_time_s,
        timecode,
        video: VideoStream {
            index: count(video.get("index")).unwrap_or(0) as u32,
            codec: video
                .get("codec_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            width,
            height,
            rotation: rotation_of(video),
            frame_rate: snap_rate(nominal),
            variable_rate,
            frames,
        },
        audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples::*;

    fn rate(num: u32, den: u32) -> Rational {
        Rational { num, den }
    }

    #[test]
    fn an_mp4_with_h264_and_aac() {
        let p = parse_probe(MP4_H264).unwrap();
        assert_eq!(p.container, "mov,mp4,m4a,3gp,3g2,mj2");
        assert!((p.duration_s - 12.021333).abs() < 1e-9);
        assert_eq!((p.video.width, p.video.height), (1920, 1080));
        assert_eq!((p.video.codec.as_str(), p.video.index), ("h264", 0));
        assert_eq!(p.video.frame_rate, rate(25, 1));
        assert!(!p.video.variable_rate);
        assert_eq!(p.video.rotation, 0);
        assert_eq!(p.video.frames, Some(300));
        assert_eq!(p.start_time_s, 0.0);
        assert_eq!(p.timecode, None);
        assert_eq!(p.audio.len(), 1);
        let a = &p.audio[0];
        assert_eq!((a.index, a.channels, a.sample_rate), (1, 2, 48000));
        assert_eq!(a.codec, "aac");
        assert_eq!(a.language.as_deref(), Some("eng"));
        assert_eq!(a.title, None);
    }

    #[test]
    fn a_prores_mov_with_a_timecode_and_a_late_start() {
        let p = parse_probe(MOV_PRORES).unwrap();
        assert_eq!(p.video.codec, "prores");
        assert_eq!(p.video.frame_rate, rate(24000, 1001));
        assert_eq!(p.start_time_s, 3600.0);
        assert_eq!(p.timecode.as_deref(), Some("01:00:00:00"));
        assert_eq!(p.audio.len(), 1);
        assert_eq!(p.audio[0].channels, 1);
        assert_eq!(p.audio[0].codec, "pcm_s24le");
        // The timecode data stream is not an audio or video track.
        assert_eq!(p.expected_frames(), 250);
    }

    #[test]
    fn an_mkv_takes_its_length_from_the_container_and_has_two_audio_tracks() {
        let p = parse_probe(MKV_HEVC).unwrap();
        assert!((p.duration_s - 90.063).abs() < 1e-9);
        assert_eq!(p.video.frame_rate, rate(30000, 1001));
        // The frame count is in an upper-case tag.
        assert_eq!(p.video.frames, Some(2700));
        assert_eq!(p.audio.len(), 2);
        assert_eq!(p.audio[0].channels, 6);
        assert_eq!(p.audio[0].title.as_deref(), Some("Surround"));
        assert_eq!(p.audio[1].language.as_deref(), Some("fre"));
        assert_eq!(p.audio[1].index, 2);
        assert_eq!(p.audio[1].sample_rate, 48000);
    }

    #[test]
    fn a_film_without_audio_has_an_empty_track_list() {
        let p = parse_probe(NO_AUDIO).unwrap();
        assert!(p.audio.is_empty());
        assert_eq!(p.video.frame_rate, rate(30, 1));
        assert_eq!(p.expected_frames(), 150);
    }

    #[test]
    fn a_phone_film_is_rotated_variable_and_has_two_audio_tracks() {
        let p = parse_probe(PHONE_ROTATED).unwrap();
        assert_eq!(p.video.rotation, 270);
        assert_eq!((p.video.width, p.video.height), (1920, 1080));
        assert_eq!(p.video.display_size(), (1080, 1920));
        assert!(p.video.variable_rate);
        // The average rate, not the 600 fps tick rate.
        assert_eq!(p.video.frame_rate, rate(2983, 100));
        assert_eq!(p.audio.len(), 2);
        assert_eq!(p.audio[1].language.as_deref(), Some("spa"));
        assert_eq!(p.audio[1].title.as_deref(), Some("Comentario"));
        // The stated count is not trusted for a variable rate: length times the nominal rate.
        assert_eq!(p.expected_frames(), (20.02f64 * 29.83).round() as u64);
    }

    #[test]
    fn rotation_comes_from_the_matrix_or_the_tag_and_is_normalised() {
        let with = |extra: &str| {
            format!(
                r#"{{"streams":[{{"index":0,"codec_type":"video","width":4,"height":2,
                "avg_frame_rate":"25/1","r_frame_rate":"25/1",{extra}}}],
                "format":{{"duration":"1.0"}}}}"#
            )
        };
        let rot = |extra: &str| parse_probe(&with(extra)).unwrap().video.rotation;
        assert_eq!(rot(r#""tags":{"rotate":"90"}"#), 270);
        assert_eq!(rot(r#""tags":{"rotate":"180"}"#), 180);
        assert_eq!(rot(r#""tags":{"rotate":"-90"}"#), 90);
        assert_eq!(rot(r#""side_data_list":[{"rotation":90}]"#), 90);
        assert_eq!(rot(r#""side_data_list":[{"rotation":-180}]"#), 180);
        assert_eq!(rot(r#""tags":{}"#), 0);
    }

    #[test]
    fn cover_art_is_not_a_film() {
        assert!(matches!(
            parse_probe(COVER_ART_ONLY),
            Err(VideoError::NoVideo)
        ));
    }

    #[test]
    fn broken_output_is_an_error_not_a_panic() {
        for text in [
            "",
            "not json",
            "{}",
            "[]",
            r#"{"streams":[]}"#,
            r#"{"streams":5}"#,
        ] {
            assert!(parse_probe(text).is_err(), "{text}");
        }
        // A picture without a frame rate, a size or any length.
        let no_rate = r#"{"streams":[{"codec_type":"video","width":2,"height":2,
            "avg_frame_rate":"0/0","r_frame_rate":"0/0"}],"format":{"duration":"1"}}"#;
        assert!(matches!(parse_probe(no_rate), Err(VideoError::Probe(_))));
        let no_size = r#"{"streams":[{"codec_type":"video","avg_frame_rate":"25/1"}],
            "format":{"duration":"1"}}"#;
        assert!(matches!(parse_probe(no_size), Err(VideoError::Probe(_))));
        let no_length = r#"{"streams":[{"codec_type":"video","width":2,"height":2,
            "avg_frame_rate":"25/1"}],"format":{}}"#;
        assert!(matches!(parse_probe(no_length), Err(VideoError::Probe(_))));
    }

    #[test]
    fn rates_parse_and_snap() {
        assert_eq!(Rational::parse("30000/1001"), Some(rate(30000, 1001)));
        assert_eq!(Rational::parse("25"), Some(rate(25, 1)));
        assert_eq!(Rational::parse("0/0"), None);
        assert_eq!(Rational::parse("25/0"), None);
        assert_eq!(Rational::parse("fast"), None);
        assert_eq!(rate(30000, 1001).arg(), "30000/1001");
        assert_eq!(snap_rate(rate(2997, 100)), rate(30000, 1001));
        assert_eq!(snap_rate(rate(2398, 100)), rate(24000, 1001));
        assert_eq!(snap_rate(rate(30, 1)), rate(30, 1));
        assert_eq!(snap_rate(rate(24, 1)), rate(24, 1));
        assert_eq!(snap_rate(rate(2983, 100)), rate(2983, 100));
        assert_eq!(snap_rate(rate(15, 1)), rate(15, 1));
    }

    #[test]
    fn a_picture_that_states_only_the_container_rate_uses_it() {
        let json = r#"{"streams":[{"index":0,"codec_type":"video","width":2,"height":2,
            "avg_frame_rate":"0/0","r_frame_rate":"25/1"}],"format":{"duration":"2"}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.video.frame_rate, rate(25, 1));
        assert!(!p.video.variable_rate);
        assert_eq!(p.expected_frames(), 50);
    }
}
