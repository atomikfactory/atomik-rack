//! The video of a song: one film, linked by path, locked to seconds.
//!
//! [`RackDoc::video`] holds at most one [`VideoClip`] (the first version of video tracks keeps one
//! film per song). The film is **linked, never copied** (films are huge): the clip keeps the path
//! the user picked, and the project opens even when the file is gone (the app marks it missing
//! and offers a relink, see [`VideoCommand::Relink`]). It also keeps the [`VideoFacts`] the
//! import probe found (length, frame rate, picture size, start timecode, whether it has sound),
//! so the project shows the film's clip and the timecode without FFmpeg and a relink can compare
//! the replacement with the original.
//!
//! # Time
//!
//! Every position of a film is **integer microseconds**, not song ticks: a film runs at the speed
//! of the clock, so its start must not move when the tempo changes (a tick is a fraction of a
//! beat, and a tempo change moves every tick in time). Microseconds are exact for every frame
//! rate (a frame of 30000/1001 fps is 33 366.7 us, the viewer rounds, the model never does
//! arithmetic on frames), compare exactly and round-trip through the file as plain integers,
//! where seconds as floats would not. [`VideoClip::start_tick`] gives the tick a start falls on
//! under the song's tempo map for the places that need one (the arrangement ruler).
//!
//! The clip has a **start** (where, in song time, the film's trimmed beginning plays, between
//! [`MAX_START_US`] before and after the song's start: a film may begin before bar 1) and a
//! **trim**: `in_us..out_us` of the source, within `0..duration_us`, at least [`MIN_TRIM_US`]
//! long. Trimming the in point moves the start with it, so the picture that stays keeps its
//! place on the timeline.
//!
//! # Saved and edited
//!
//! `None` for a new project; saved as the root field `"video"` only when set (project format 44,
//! ignorable; the optional `"audio_link"` inside it is part of the same field) and edited with
//! [`crate::Command::Video`]: one undo step each, reporting [`crate::SyncOp::ViewChanged`].
//! [`VideoCommand::Move`] and [`VideoCommand::Trim`] carry a merge id, so the steps of one drag are
//! one undo step.
//!
//! # The film's sound
//!
//! Import Video Audio puts the film's own audio in the song as an ordinary audio clip and
//! links it with an [`AudioLink`]. While the link's `keep` flag is on, every edit of the film that
//! changes where its sound belongs (move, trim, relink, [`VideoCommand::KeepAudio`]) also moves
//! and trims that clip, **in the same undo step** (the step is a compound of the film's change and
//! the clip's, and a drag merges both), by the arithmetic of [`crate::video_audio`].

use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, json_str, object, string, uint};
use crate::song::{ClipContent, ClipId, Song};
use crate::song_cmd::{audio_span_frames, clamp_audio_span};
use crate::{ModelError, RackDoc, SongCommand};
use atomik_device_sdk::Registry;
use serde_json::Value;
use std::fmt::Write as _;

/// Longest path, in characters.
pub const MAX_VIDEO_PATH_CHARS: usize = 1_000;
/// How far the start may be from the song's start, in microseconds (24 hours).
pub const MAX_START_US: i64 = 86_400_000_000;
/// The longest film, in microseconds (48 hours).
pub const MAX_DURATION_US: u64 = 172_800_000_000;
/// The shortest trim, in microseconds (one millisecond).
pub const MIN_TRIM_US: u64 = 1_000;
/// The largest picture side and frame rate part the model accepts.
pub const MAX_DIMENSION: u32 = 16_384;
/// The largest frame rate numerator or denominator.
pub const MAX_RATE_PART: u32 = 1_000_000;
/// Longest start timecode text, in characters.
pub const MAX_TIMECODE_CHARS: usize = 16;

const FIELDS: &[&str] = &[
    "path",
    "start_us",
    "in_us",
    "out_us",
    "duration_us",
    "fps_num",
    "fps_den",
    "width",
    "height",
    "timecode",
    "audio",
    "audio_link",
];
const REQUIRED: &[&str] = &[
    "path",
    "start_us",
    "in_us",
    "out_us",
    "duration_us",
    "fps_num",
    "fps_den",
    "width",
    "height",
];

/// What the probe found out about the film: kept in the project so it opens without FFmpeg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFacts {
    /// The length of the whole file, in microseconds.
    pub duration_us: u64,
    /// The frame rate as a fraction (`30000/1001`).
    pub fps_num: u32,
    /// The frame rate's denominator (never 0).
    pub fps_den: u32,
    /// The picture's width as it shows (rotation applied).
    pub width: u32,
    /// The picture's height as it shows.
    pub height: u32,
    /// The start timecode of the file (`01:00:00:00`), when it has one.
    pub timecode: Option<String>,
    /// The file has sound (the film's audio can be imported).
    pub has_audio: bool,
}

impl VideoFacts {
    /// The frame rate as a number.
    pub fn fps(&self) -> f64 {
        f64::from(self.fps_num) / f64::from(self.fps_den)
    }

    /// What differs between these facts (of the project) and `other` (of a replacement file),
    /// in words, for the warning after a relink: the length (more than half a frame apart) and
    /// the frame rate. Empty when the file is the same film as far as the facts say.
    pub fn differences(&self, other: &VideoFacts) -> Vec<String> {
        let mut out = Vec::new();
        let frame_us = 1_000_000.0 / self.fps().max(1.0);
        let delta = self.duration_us.abs_diff(other.duration_us) as f64;
        if delta > frame_us / 2.0 {
            out.push(format!(
                "the length is {:.2} s instead of {:.2} s",
                other.duration_us as f64 / 1e6,
                self.duration_us as f64 / 1e6
            ));
        }
        if u64::from(self.fps_num) * u64::from(other.fps_den)
            != u64::from(other.fps_num) * u64::from(self.fps_den)
        {
            out.push(format!(
                "the frame rate is {:.3} fps instead of {:.3} fps",
                other.fps(),
                self.fps()
            ));
        }
        out
    }
}

/// The audio clip kept with the film: the film's own sound, imported by Import Video Audio.
///
/// While `keep` is on, moving or trimming the film (or relinking it) moves and trims this clip in
/// the same undo step, so the sound stays under the picture: see [`crate::video_audio`] for how
/// the clip is placed from the film's start, its trim and `lead_us`. With `keep` off the clip is
/// an ordinary audio clip and the link only remembers how to line it up again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioLink {
    /// The audio clip.
    pub clip: ClipId,
    /// How long after the film's first frame the track's first sample sits, in microseconds
    /// (negative: before it), from the start times the probe found.
    pub lead_us: i64,
    /// The clip follows the film.
    pub keep: bool,
}

/// The farthest the sound may start from the picture, in microseconds (an hour).
pub const MAX_LEAD_US: i64 = 3_600_000_000;

/// The film of a song (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoClip {
    /// The file the user picked (an absolute path, or one relative to the project folder).
    pub path: String,
    /// What the probe found.
    pub facts: VideoFacts,
    /// Where the trimmed beginning plays, in song time, in microseconds (negative: before the
    /// song's start).
    pub start_us: i64,
    /// The trim's start in the source, in microseconds.
    pub in_us: u64,
    /// The trim's end in the source, in microseconds.
    pub out_us: u64,
    /// The film's own sound as an audio clip, when it was imported (see [`AudioLink`]).
    pub audio_link: Option<AudioLink>,
}

impl VideoClip {
    /// A clip of the whole film, starting at the song's start.
    pub fn new(path: impl Into<String>, facts: VideoFacts) -> Self {
        Self {
            path: path.into(),
            start_us: 0,
            in_us: 0,
            out_us: facts.duration_us,
            audio_link: None,
            facts,
        }
    }

    /// The file name without its folder, for lists.
    pub fn name(&self) -> &str {
        self.path
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(&self.path)
    }

    /// The length that plays, in microseconds.
    pub fn length_us(&self) -> u64 {
        self.out_us - self.in_us
    }

    /// Where the film ends in song time, in microseconds.
    pub fn end_us(&self) -> i64 {
        self.start_us + self.length_us() as i64
    }

    /// The source time that plays at song time `at_us`; `None` before the clip and after it.
    pub fn source_at(&self, at_us: i64) -> Option<u64> {
        let into = at_us.checked_sub(self.start_us)?;
        (into >= 0 && (into as u64) < self.length_us()).then(|| self.in_us + into as u64)
    }

    /// The tick the start falls on under the song's tempo map (0 for a start before the song).
    /// It moves when the tempo changes; the start in microseconds does not.
    pub fn start_tick(&self, song: &Song) -> i64 {
        tick_at_us(song, self.start_us)
    }

    /// The tick the clip ends on under the song's tempo map.
    pub fn end_tick(&self, song: &Song) -> i64 {
        tick_at_us(song, self.end_us())
    }
}

fn tick_at_us(song: &Song, us: i64) -> i64 {
    crate::audio_trim::TempoMap::new(&song.tempo)
        .tick_at_seconds(us as f64 / 1e6)
        .round() as i64
}

/// An edit of the song's video, one undo step each (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCommand {
    /// Puts a film in the song, replacing the one it has.
    Set(VideoClip),
    /// Takes the film out of the song (the file is untouched).
    Remove,
    /// Moves the start to `start_us` (microseconds from the song's start).
    Move {
        /// The new start.
        start_us: i64,
        /// One id per drag: the steps of a drag merge into one.
        merge: Option<u64>,
    },
    /// Sets the trim to `in_us..out_us` of the source. The start moves by the change of the in
    /// point, so the picture that stays keeps its place.
    Trim {
        /// The new in point.
        in_us: u64,
        /// The new out point.
        out_us: u64,
        /// One id per drag: the steps of a drag merge into one.
        merge: Option<u64>,
    },
    /// Sets (`Some`) or clears (`None`) the audio clip kept with the film. It changes the link
    /// only: the clip is placed by the caller (Import Video Audio does both in one undo step).
    LinkAudio(Option<AudioLink>),
    /// Turns "keep with video" on or off for the linked audio clip. Turning it on puts the clip
    /// where the film's sound belongs again (as at import), in the same undo step.
    KeepAudio(bool),
    /// Points the clip at another file (the film was moved, or this is another copy of it) with
    /// the replacement's facts. The trim and the start are kept; the out point is limited to
    /// the new length.
    Relink {
        /// The new path.
        path: String,
        /// The replacement's facts.
        facts: VideoFacts,
    },
}

/// The label of an undo step.
pub(crate) fn label(c: &VideoCommand) -> &'static str {
    match c {
        VideoCommand::Set(_) => "Set Video",
        VideoCommand::Remove => "Remove Video",
        VideoCommand::Move { .. } => "Move Video",
        VideoCommand::Trim { .. } => "Trim Video",
        VideoCommand::Relink { .. } => "Relink Video",
        VideoCommand::LinkAudio(_) => "Link Video Audio",
        VideoCommand::KeepAudio(true) => "Keep Audio with Video",
        VideoCommand::KeepAudio(false) => "Free Audio from Video",
    }
}

/// Checks a clip against the limits.
///
/// # Errors
/// [`ModelError::OutOfRange`] naming the first value that breaks one.
pub fn check_video(v: &VideoClip) -> Result<(), ModelError> {
    check_one(v).map_err(|m| ModelError::OutOfRange(format!("video: {m}")))
}

fn check_facts(f: &VideoFacts) -> Result<(), String> {
    if f.duration_us < MIN_TRIM_US || f.duration_us > MAX_DURATION_US {
        return Err(format!(
            "the length {} us is outside {MIN_TRIM_US}..={MAX_DURATION_US}",
            f.duration_us
        ));
    }
    for (name, n, max) in [
        ("frame rate", f.fps_num, MAX_RATE_PART),
        ("frame rate divisor", f.fps_den, MAX_RATE_PART),
        ("width", f.width, MAX_DIMENSION),
        ("height", f.height, MAX_DIMENSION),
    ] {
        if n == 0 || n > max {
            return Err(format!("the {name} {n} is outside 1..={max}"));
        }
    }
    if let Some(t) = &f.timecode
        && (t.is_empty()
            || t.chars().count() > MAX_TIMECODE_CHARS
            || t.chars().any(char::is_control))
    {
        return Err("the start timecode is not a short line of text".into());
    }
    Ok(())
}

fn check_one(v: &VideoClip) -> Result<(), String> {
    let chars = v.path.chars().count();
    if chars == 0 {
        return Err("the path is empty".into());
    }
    if chars > MAX_VIDEO_PATH_CHARS {
        return Err(format!(
            "the path is {chars} characters, at most {MAX_VIDEO_PATH_CHARS} are allowed"
        ));
    }
    if v.path.chars().any(char::is_control) {
        return Err("the path must not hold control characters".into());
    }
    check_facts(&v.facts)?;
    if let Some(link) = &v.audio_link
        && !(-MAX_LEAD_US..=MAX_LEAD_US).contains(&link.lead_us)
    {
        return Err(format!(
            "the audio lead {} us is outside -{MAX_LEAD_US}..={MAX_LEAD_US}",
            link.lead_us
        ));
    }
    if !(-MAX_START_US..=MAX_START_US).contains(&v.start_us) {
        return Err(format!(
            "the start {} us is outside -{MAX_START_US}..={MAX_START_US}",
            v.start_us
        ));
    }
    if v.out_us > v.facts.duration_us {
        return Err(format!(
            "the trim ends at {} us, after the film's end at {} us",
            v.out_us, v.facts.duration_us
        ));
    }
    if v.out_us < v.in_us.saturating_add(MIN_TRIM_US) {
        return Err(format!(
            "the trim {}..{} us is shorter than {MIN_TRIM_US} us",
            v.in_us, v.out_us
        ));
    }
    Ok(())
}

/// Plans `Command::Video`.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: &VideoCommand,
) -> Result<Edit, ModelError> {
    let none = || ModelError::BadScope("the song has no video".into());
    // Whether the audio clip kept with the film goes where the new film state puts it.
    let mut follow_audio = false;
    let new = match command {
        VideoCommand::Set(clip) => Some(clip.clone()),
        VideoCommand::Remove => {
            doc.video.as_ref().ok_or_else(none)?;
            None
        }
        VideoCommand::Move { start_us, .. } => {
            let mut clip = doc.video.clone().ok_or_else(none)?;
            clip.start_us = *start_us;
            follow_audio = true;
            Some(clip)
        }
        VideoCommand::Trim { in_us, out_us, .. } => {
            let mut clip = doc.video.clone().ok_or_else(none)?;
            // The start follows the in point; the arithmetic is on i128 so a hostile value
            // cannot overflow before the limits see it.
            let moved = i128::from(clip.start_us) + i128::from(*in_us) - i128::from(clip.in_us);
            clip.start_us = i64::try_from(moved).unwrap_or(i64::MAX);
            clip.in_us = *in_us;
            clip.out_us = *out_us;
            follow_audio = true;
            Some(clip)
        }
        VideoCommand::LinkAudio(link) => {
            let mut clip = doc.video.clone().ok_or_else(none)?;
            clip.audio_link = *link;
            Some(clip)
        }
        VideoCommand::KeepAudio(keep) => {
            let mut clip = doc.video.clone().ok_or_else(none)?;
            let link = clip
                .audio_link
                .as_mut()
                .ok_or_else(|| ModelError::BadScope("the song has no video audio".into()))?;
            link.keep = *keep;
            follow_audio = *keep;
            Some(clip)
        }
        VideoCommand::Relink { path, facts } => {
            let mut clip = doc.video.clone().ok_or_else(none)?;
            clip.path.clone_from(path);
            clip.facts = facts.clone();
            clip.out_us = clip.out_us.min(facts.duration_us);
            follow_audio = true;
            Some(clip)
        }
    };
    if let Some(clip) = &new {
        check_video(clip)?;
    }
    let swap = Edit::Swap(Swap::Video {
        old: doc.video.clone(),
        new: new.clone(),
    });
    let kept = new
        .as_ref()
        .filter(|_| follow_audio)
        .and_then(|clip| follow(doc, registry, clip));
    if doc.video == new && kept.is_none() {
        return Err(ModelError::BadScope("the video is already that".into()));
    }
    Ok(match kept {
        // The film and its sound are one undo step; the film comes first, so undo puts the
        // sound back first.
        Some(clip_edit) if doc.video != new => Edit::Compound(vec![swap, clip_edit]),
        Some(clip_edit) => clip_edit,
        None => swap,
    })
}

/// The edit that puts the audio clip kept with the film (`film.audio_link`, with `keep` on) where
/// the film's sound belongs (see [`crate::video_audio::placement`]). `None` when there is no
/// such clip, it is not a plain audio clip of an asset at the song's rate, no part of the sound
/// is inside the film that plays, or the clip already is there.
fn follow(doc: &RackDoc, registry: &Registry, film: &VideoClip) -> Option<Edit> {
    let link = film.audio_link.filter(|l| l.keep)?;
    let (_, _, clip) = doc.song.clip(link.clip)?;
    let ClipContent::Audio(audio) = &clip.content else {
        return None;
    };
    // A warped clip follows the tempo, not the clock: it cannot be lined up to the sample.
    if audio.warp.enabled || clip.loop_length.is_some() {
        return None;
    }
    let meta = doc.assets.get(&audio.asset)?;
    if meta.sample_rate != doc.sample_rate {
        return None;
    }
    let at =
        crate::video_audio::placement(&doc.song, doc.sample_rate, film, link.lead_us, meta.frames)?;
    let mut content = audio.clone();
    content.source_offset = at.source_offset;
    clamp_audio_span(
        &mut content,
        audio_span_frames(doc, at.start, at.length, clip.loop_length),
    );
    if (at.start, at.length) == (clip.start, clip.length) && content == *audio {
        return None;
    }
    crate::song_cmd::plan(
        doc,
        registry,
        SongCommand::SetAudioClip {
            id: link.clip,
            start: at.start,
            length: at.length,
            muted: clip.muted,
            content,
            merge: None,
        },
    )
    .ok()
    .map(|(edit, _)| edit)
}

/// Writes `"video"` (with its trailing comma and newline) after `"delivery"`; nothing when the
/// song has no film.
pub(crate) fn write_video(w: &mut String, video: &Option<VideoClip>) {
    let Some(v) = video else {
        return;
    };
    let f = &v.facts;
    let _ = write!(
        w,
        "  \"video\": {{\"path\": {}, \"start_us\": {}, \"in_us\": {}, \"out_us\": {}, \
         \"duration_us\": {}, \"fps_num\": {}, \"fps_den\": {}, \"width\": {}, \"height\": {}",
        json_str(&v.path),
        v.start_us,
        v.in_us,
        v.out_us,
        f.duration_us,
        f.fps_num,
        f.fps_den,
        f.width,
        f.height
    );
    if let Some(t) = &f.timecode {
        let _ = write!(w, ", \"timecode\": {}", json_str(t));
    }
    if f.has_audio {
        let _ = write!(w, ", \"audio\": true");
    }
    if let Some(link) = &v.audio_link {
        let _ = write!(
            w,
            ", \"audio_link\": {{\"clip\": {}, \"lead_us\": {}, \"keep\": {}}}",
            link.clip.0, link.lead_us, link.keep
        );
    }
    let _ = writeln!(w, "}},");
}

/// Reads the project's `"video"` object.
pub(crate) fn load_video(v: &Value, path: &JsonPath) -> Result<VideoClip, ModelError> {
    let o = object(v, path, FIELDS, REQUIRED)?;
    let part = |name: &str, max: u64| uint(&o[name], &path.field(name), max);
    let start = &o["start_us"];
    let start_us = start.as_i64().ok_or_else(|| {
        path.field("start_us")
            .error(format!("expected a whole number, found {start}"))
    })?;
    let timecode = match o.get("timecode") {
        Some(t) => Some(string(t, &path.field("timecode"))?.to_owned()),
        None => None,
    };
    let has_audio = match o.get("audio") {
        Some(Value::Bool(b)) => *b,
        Some(other) => {
            return Err(path
                .field("audio")
                .error(format!("expected true or false, found {other}")));
        }
        None => false,
    };
    let audio_link = match o.get("audio_link") {
        Some(link) => {
            let at = path.field("audio_link");
            let l = object(
                link,
                &at,
                &["clip", "lead_us", "keep"],
                &["clip", "lead_us"],
            )?;
            let lead = &l["lead_us"];
            let keep = match l.get("keep") {
                Some(Value::Bool(b)) => *b,
                Some(other) => {
                    return Err(at
                        .field("keep")
                        .error(format!("expected true or false, found {other}")));
                }
                None => true,
            };
            Some(AudioLink {
                clip: ClipId(uint(&l["clip"], &at.field("clip"), u64::MAX)?),
                lead_us: lead.as_i64().ok_or_else(|| {
                    at.field("lead_us")
                        .error(format!("expected a whole number, found {lead}"))
                })?,
                keep,
            })
        }
        None => None,
    };
    let clip = VideoClip {
        path: string(&o["path"], &path.field("path"))?.to_owned(),
        audio_link,
        start_us,
        in_us: part("in_us", MAX_DURATION_US)?,
        out_us: part("out_us", MAX_DURATION_US)?,
        facts: VideoFacts {
            duration_us: part("duration_us", MAX_DURATION_US)?,
            fps_num: part("fps_num", u64::from(MAX_RATE_PART))? as u32,
            fps_den: part("fps_den", u64::from(MAX_RATE_PART))? as u32,
            width: part("width", u64::from(MAX_DIMENSION))? as u32,
            height: part("height", u64::from(MAX_DIMENSION))? as u32,
            timecode,
            has_audio,
        },
    };
    check_one(&clip).map_err(|m| path.error(m))?;
    Ok(clip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> VideoFacts {
        VideoFacts {
            duration_us: 90_000_000,
            fps_num: 30_000,
            fps_den: 1_001,
            width: 1920,
            height: 1080,
            timecode: Some("01:00:00:00".into()),
            has_audio: true,
        }
    }

    #[test]
    fn a_new_clip_covers_the_whole_film_from_the_start() {
        let c = VideoClip::new("films\\scene.mov", facts());
        assert_eq!((c.start_us, c.in_us, c.out_us), (0, 0, 90_000_000));
        assert_eq!(c.length_us(), 90_000_000);
        assert_eq!(c.end_us(), 90_000_000);
        assert_eq!(c.name(), "scene.mov");
        assert!(check_video(&c).is_ok());
    }

    #[test]
    fn the_source_time_follows_the_start_and_the_trim() {
        let mut c = VideoClip::new("a.mov", facts());
        c.start_us = 2_000_000;
        c.in_us = 10_000_000;
        c.out_us = 20_000_000;
        assert_eq!(c.source_at(1_999_999), None);
        assert_eq!(c.source_at(2_000_000), Some(10_000_000));
        assert_eq!(c.source_at(5_000_000), Some(13_000_000));
        assert_eq!(c.source_at(12_000_000), None);
        assert_eq!(c.end_us(), 12_000_000);
    }

    #[test]
    fn limits_are_enforced() {
        let ok = VideoClip::new("a.mov", facts());
        let mut bad = ok.clone();
        bad.path.clear();
        assert!(check_video(&bad).is_err());
        bad.path = "a\nb".into();
        assert!(check_video(&bad).is_err());
        bad = ok.clone();
        bad.out_us = bad.facts.duration_us + 1;
        assert!(check_video(&bad).is_err());
        bad = ok.clone();
        bad.in_us = bad.out_us - 10;
        assert!(check_video(&bad).is_err());
        bad = ok.clone();
        bad.start_us = MAX_START_US + 1;
        assert!(check_video(&bad).is_err());
        bad = ok.clone();
        bad.facts.fps_den = 0;
        assert!(check_video(&bad).is_err());
        bad = ok;
        bad.facts.width = MAX_DIMENSION + 1;
        assert!(check_video(&bad).is_err());
    }

    #[test]
    fn facts_name_what_differs() {
        let a = facts();
        assert!(a.differences(&a).is_empty());
        let mut b = a.clone();
        b.duration_us += 1_000;
        assert!(a.differences(&b).is_empty(), "a millisecond is no frame");
        b.duration_us += 5_000_000;
        b.fps_num = 25;
        b.fps_den = 1;
        let d = a.differences(&b);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(
            d[0].contains("length") && d[1].contains("frame rate"),
            "{d:?}"
        );
        // Another way to write the same rate is no difference.
        b = a.clone();
        b.fps_num *= 2;
        b.fps_den *= 2;
        assert!(a.differences(&b).is_empty());
    }

    #[test]
    fn writing_and_reading_round_trip() {
        for clip in [
            VideoClip::new("film.mov", {
                let mut f = facts();
                f.timecode = None;
                f.has_audio = false;
                f
            }),
            VideoClip {
                path: "dir/my \"cut\".mp4".into(),
                facts: facts(),
                start_us: -4_500_000,
                in_us: 1_000_000,
                out_us: 80_000_000,
                audio_link: Some(AudioLink {
                    clip: ClipId(3),
                    lead_us: -7_000,
                    keep: false,
                }),
            },
        ] {
            let mut text = String::from("{\n");
            write_video(&mut text, &Some(clip.clone()));
            text.push_str("  \"end\": 0\n}");
            let value: Value = serde_json::from_str(&text).unwrap();
            let back = load_video(&value["video"], &JsonPath::lazy()).unwrap();
            assert_eq!(back, clip);
        }
        let mut empty = String::new();
        write_video(&mut empty, &None);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_hand_edited_file_is_checked() {
        let bad = serde_json::json!({
            "path": "a.mov", "start_us": 0, "in_us": 5, "out_us": 4, "duration_us": 90_000_000,
            "fps_num": 25, "fps_den": 1, "width": 10, "height": 10
        });
        assert!(load_video(&bad, &JsonPath::lazy()).is_err());
        let extra = serde_json::json!({
            "path": "a.mov", "start_us": 0, "in_us": 0, "out_us": 2_000, "duration_us": 90_000_000,
            "fps_num": 25, "fps_den": 1, "width": 10, "height": 10, "colour": 1
        });
        assert!(load_video(&extra, &JsonPath::lazy()).is_err());
    }
}
