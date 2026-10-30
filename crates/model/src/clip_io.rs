//! The `atomik-clip` file: one note clip, for the library (version 1).
//!
//! ```text
//! { "format": "atomik-clip", "version": 1, "ppq": 15360,
//!   "tempo_bpm": 120.0, "meter": {"num": 4, "den": 4},
//!   "clip": { "length": 61440, "offset": 0, "loop_length": null, "muted": false,
//!             "notes": [[tick, length, key, velocity], [tick, length, key, velocity, true] …],
//!             "grid": {…}, "scale": {…}, "controllers": […] } }
//! ```
//!
//! `"clip"` is a project note clip without its `id` and `start`: the same fields, limits and
//! note rules (sorted by tick and key, mute flag as a fifth element), and the same optional
//! `grid`, `scale` and `controllers` objects. The output is canonical: fixed field order, the
//! optional objects only when set, so saving a loaded file gives the same text back.
//!
//! `"tempo_bpm"` and `"meter"` only say what the clip was made against (informational, both
//! optional, 120 and 4/4 when absent); nothing is rescaled. The ticks are in the file's `ppq`,
//! and only [`PPQ`](atomik_core::PPQ) is read: a file with another resolution is refused rather
//! than rescaled. A newer `version` is refused, and so is an audio clip (the file holds note
//! clips only). Errors name the JSON path, like the project loader's.

use crate::patch::{JsonPath, array, f64_text, number, object, string, uint};
use crate::project::{
    clip_span, flag, load_controllers, load_grid, load_notes, load_scale, write_note_clip_tail,
};
use crate::song::{Clip, ClipContent, ClipId};
use crate::song_cmd::check_meter;
use crate::{ModelError, TEMPO_MAX, TEMPO_MIN};
use atomik_core::PPQ;
use serde_json::Value;
use std::fmt::Write as _;

/// The `"format"` value of a clip file.
pub const CLIP_FORMAT: &str = "atomik-clip";
/// The version this module writes and the newest it reads.
pub const CLIP_VERSION: u32 = 1;

const ROOT_FIELDS: &[&str] = &["format", "version", "ppq", "tempo_bpm", "meter", "clip"];
const ROOT_REQUIRED: &[&str] = &["format", "version", "ppq", "clip"];
const METER_FIELDS: &[&str] = &["num", "den"];
const CLIP_FIELDS: &[&str] = &[
    "length",
    "offset",
    "loop_length",
    "muted",
    "grid",
    "scale",
    "controllers",
    "notes",
];
const CLIP_REQUIRED: &[&str] = &["length", "notes"];

/// A note clip as the library stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipFile {
    /// The tempo the clip was made at (informational).
    pub tempo_bpm: f64,
    /// The meter it was made in, beats per bar and beat unit (informational).
    pub meter: (u8, u8),
    /// The clip: a note clip with id 0 and start 0.
    pub clip: Clip,
}

/// Writes `file` as an `atomik-clip` v1 text. Refuses a clip that is not a note clip.
pub fn save_clip_file(file: &ClipFile) -> Result<String, ModelError> {
    let c = &file.clip;
    let ClipContent::Notes(notes) = &c.content else {
        return Err(ModelError::BadSong(
            "only a note clip can be saved as a clip file".into(),
        ));
    };
    let mut w = String::new();
    let _ = writeln!(w, "{{");
    let _ = writeln!(w, "  \"format\": \"{CLIP_FORMAT}\",");
    let _ = writeln!(w, "  \"version\": {CLIP_VERSION},");
    let _ = writeln!(w, "  \"ppq\": {PPQ},");
    let _ = writeln!(w, "  \"tempo_bpm\": {},", f64_text(file.tempo_bpm));
    let _ = writeln!(
        w,
        "  \"meter\": {{\"num\": {}, \"den\": {}}},",
        file.meter.0, file.meter.1
    );
    let loop_length = c.loop_length.map_or("null".to_owned(), |n| n.to_string());
    let _ = writeln!(w, "  \"clip\": {{");
    let _ = writeln!(
        w,
        "    \"length\": {}, \"offset\": {}, \"loop_length\": {loop_length}, \"muted\": {},",
        c.length, c.offset, c.muted
    );
    write_note_clip_tail(&mut w, c, notes, "    ");
    let _ = writeln!(w, "\n  }}\n}}");
    Ok(w)
}

/// Reads an `atomik-clip` text. Errors are [`ModelError::Load`] with `"<json path>: <what is
/// wrong>"`.
pub fn load_clip_file(text: &str) -> Result<ClipFile, ModelError> {
    let root = JsonPath::default();
    let value: Value =
        serde_json::from_str(text).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
    let obj = object(&value, &root, ROOT_FIELDS, &["format"])?;
    let format_path = root.field("format");
    let format = string(&obj["format"], &format_path)?;
    if format != CLIP_FORMAT {
        return Err(format_path.error(format!("expected \"{CLIP_FORMAT}\", found {format:?}")));
    }
    object(&value, &root, ROOT_FIELDS, ROOT_REQUIRED)?;
    let version_path = root.field("version");
    let version = uint(&obj["version"], &version_path, u64::MAX)?;
    if version > u64::from(CLIP_VERSION) {
        return Err(version_path.error(format!(
            "version {version} is newer than this reader (it reads version {CLIP_VERSION}): update Atomik Rack"
        )));
    }
    if version == 0 {
        return Err(version_path.error(format!(
            "unsupported version 0; this reader reads version {CLIP_VERSION}"
        )));
    }
    let ppq_path = root.field("ppq");
    let ppq = uint(&obj["ppq"], &ppq_path, u64::MAX)?;
    if i64::try_from(ppq) != Ok(PPQ) {
        return Err(ppq_path.error(format!(
            "this clip uses {ppq} ticks per quarter note; Atomik Rack reads {PPQ}"
        )));
    }
    let tempo_bpm = match obj.get("tempo_bpm") {
        Some(v) => {
            let p = root.field("tempo_bpm");
            let bpm = number(v, &p)?;
            if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
                return Err(p.error(format!("{bpm} BPM is outside {TEMPO_MIN}..={TEMPO_MAX}")));
            }
            bpm
        }
        None => 120.0,
    };
    let meter = match obj.get("meter") {
        Some(v) => {
            let p = root.field("meter");
            let m = object(v, &p, METER_FIELDS, METER_FIELDS)?;
            let num = uint(&m["num"], &p.field("num"), 255)? as u8;
            let den = uint(&m["den"], &p.field("den"), 255)? as u8;
            check_meter(num, 4).map_err(|e| p.field("num").error(e))?;
            check_meter(1, den).map_err(|e| p.field("den").error(e))?;
            (num, den)
        }
        None => (4, 4),
    };
    let clip_path = root.field("clip");
    let clip = load_clip_body(&obj["clip"], &clip_path)?;
    Ok(ClipFile {
        tempo_bpm,
        meter,
        clip,
    })
}

fn load_clip_body(v: &Value, path: &JsonPath) -> Result<Clip, ModelError> {
    if v.as_object().is_some_and(|o| o.contains_key("audio")) {
        return Err(path
            .field("audio")
            .error("an audio clip cannot be a clip file (note clips only)"));
    }
    let obj = object(v, path, CLIP_FIELDS, CLIP_REQUIRED)?;
    let (length, offset, loop_length) = clip_span(obj, path)?;
    let muted = flag(obj, path, "muted")?;
    let notes_path = path.field("notes");
    let notes = load_notes(array(&obj["notes"], &notes_path)?, &notes_path, 30)?;
    Ok(Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset,
        loop_length,
        muted,
        grid: match obj.get("grid") {
            Some(g) => Some(load_grid(g, &path.field("grid"))?),
            None => None,
        },
        scale: match obj.get("scale") {
            Some(s) => Some(load_scale(s, &path.field("scale"))?),
            None => None,
        },
        controllers: match obj.get("controllers") {
            Some(c) => load_controllers(c, &path.field("controllers"))?,
            None => Vec::new(),
        },
        content: ClipContent::Notes(notes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{ControllerKind, ControllerLane};
    use crate::song::{AutomationPoint, Curve, Note, StepGrid, StepResolution};
    use crate::theory::{Scale, ScaleKind};

    fn clip(notes: Vec<Note>) -> Clip {
        Clip {
            id: ClipId(0),
            start: 0,
            length: 61_440,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            scale: None,
            controllers: Vec::new(),
            content: ClipContent::Notes(notes),
        }
    }

    fn file(clip: Clip) -> ClipFile {
        ClipFile {
            tempo_bpm: 120.0,
            meter: (4, 4),
            clip,
        }
    }

    fn full() -> ClipFile {
        let mut muted = Note::new(15_360, 7_680, 64, 0.5);
        muted.muted = true;
        let mut c = clip(vec![
            Note::new(0, 3_840, 60, 0.8),
            Note::new(0, 3_840, 64, 0.8),
            muted,
        ]);
        c.offset = 3_840;
        c.loop_length = Some(30_720);
        c.muted = true;
        c.grid = Some(StepGrid {
            resolution: StepResolution::Sixteenth,
            steps: 16,
            swing: 10,
        });
        c.scale = Some(Scale::new(2, ScaleKind::Dorian));
        c.controllers = vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![AutomationPoint {
                tick: 0,
                value: 0.25,
                curve: Curve::Linear,
            }],
        )];
        ClipFile {
            tempo_bpm: 98.5,
            meter: (7, 8),
            clip: c,
        }
    }

    #[test]
    fn save_clip_file_then_load_gives_the_same_clip() {
        let f = full();
        let text = save_clip_file(&f).unwrap();
        assert_eq!(load_clip_file(&text).unwrap(), f);
    }

    #[test]
    fn save_clip_file_twice_is_canonical() {
        let text = save_clip_file(&full()).unwrap();
        let again = save_clip_file(&load_clip_file(&text).unwrap()).unwrap();
        assert_eq!(text, again);
    }

    #[test]
    fn save_clip_file_empty_clip_has_no_optional_objects() {
        let text = save_clip_file(&file(clip(Vec::new()))).unwrap();
        for field in ["grid", "scale", "controllers"] {
            assert!(!text.contains(field), "{field}");
        }
        assert!(text.contains("\"notes\": []"));
        assert!(text.contains("\"format\": \"atomik-clip\""));
        assert!(text.contains("\"ppq\": 15360"));
        assert_eq!(load_clip_file(&text).unwrap().clip, clip(Vec::new()));
    }

    #[test]
    fn save_clip_file_automation_clip_is_refused() {
        let mut c = clip(Vec::new());
        c.content = ClipContent::Automation(Vec::new());
        assert!(matches!(
            save_clip_file(&file(c)),
            Err(ModelError::BadSong(_))
        ));
    }

    fn text_with(edit: impl Fn(&mut Value)) -> String {
        let mut v: Value = serde_json::from_str(&save_clip_file(&full()).unwrap()).unwrap();
        edit(&mut v);
        v.to_string()
    }

    fn error_of(text: &str) -> String {
        load_clip_file(text).unwrap_err().to_string()
    }

    #[test]
    fn load_clip_file_other_ppq_is_refused_naming_the_path() {
        let e = error_of(&text_with(|v| v["ppq"] = 960.into()));
        assert!(
            e.contains("ppq: this clip uses 960 ticks per quarter note"),
            "{e}"
        );
    }

    #[test]
    fn load_clip_file_newer_version_is_refused() {
        let e = error_of(&text_with(|v| v["version"] = 2.into()));
        assert!(e.contains("version: version 2 is newer"), "{e}");
    }

    #[test]
    fn load_clip_file_wrong_format_is_refused() {
        let e = error_of(&text_with(|v| v["format"] = "atomik-project".into()));
        assert!(e.contains("format: expected \"atomik-clip\""), "{e}");
    }

    #[test]
    fn load_clip_file_audio_clip_is_refused() {
        let e = error_of(&text_with(|v| {
            v["clip"]["audio"] = serde_json::json!({"asset": "x"});
        }));
        assert!(
            e.contains("clip.audio: an audio clip cannot be a clip file"),
            "{e}"
        );
    }

    #[test]
    fn load_clip_file_unsorted_notes_name_the_note() {
        let e = error_of(&text_with(|v| {
            v["clip"]["notes"] = serde_json::json!([[100, 10, 60, 0.5], [0, 10, 60, 0.5]]);
        }));
        assert!(e.contains("clip.notes[1]: notes must be sorted"), "{e}");
    }

    type Case = (&'static str, fn(&mut Value));

    #[test]
    fn load_clip_file_bad_values_name_their_paths() {
        let cases: [Case; 6] = [
            ("clip.length", |v| v["clip"]["length"] = 0.into()),
            ("clip.notes[0]", |v| {
                v["clip"]["notes"] = serde_json::json!([[0, 10, 200, 0.5]]);
            }),
            ("clip.grid", |v| v["clip"]["grid"] = serde_json::json!({})),
            ("tempo_bpm", |v| v["tempo_bpm"] = 5.into()),
            ("meter.den", |v| v["meter"]["den"] = 3.into()),
            ("clip.loop_length", |v| {
                v["clip"]["loop_length"] = 1.into();
            }),
        ];
        for (path, edit) in cases {
            let e = error_of(&text_with(edit));
            assert!(e.contains(path), "{path}: {e}");
        }
    }

    #[test]
    fn load_clip_file_not_json_or_not_an_object_is_an_error() {
        assert!(error_of("{").contains("invalid JSON"));
        assert!(error_of("[]").contains("$: expected an object"));
    }

    #[test]
    fn load_clip_file_tempo_and_meter_are_optional() {
        let text = text_with(|v| {
            let o = v.as_object_mut().unwrap();
            o.remove("tempo_bpm");
            o.remove("meter");
        });
        let f = load_clip_file(&text).unwrap();
        assert_eq!((f.tempo_bpm, f.meter), (120.0, (4, 4)));
    }

    #[test]
    fn load_clip_file_unknown_field_is_refused() {
        let e = error_of(&text_with(|v| v["clip"]["id"] = 1.into()));
        assert!(e.contains("clip"), "{e}");
    }
}
