//! What Copy puts on the system clipboard, so another running instance can paste it.
//!
//! The in-process clipboard ([`crate::clipboard`]) stays the fast path. Every Copy or Cut of
//! clips, notes, automation points, devices or tracks also writes a text envelope to the system
//! clipboard through an [`OsClipboard`]; a Paste that finds an envelope there that this instance
//! did not write decodes it and uses it. The envelope is self-describing and versioned:
//!
//! ```text
//! atomik-clipboard/1 clips
//! {"audio": "<folder>"}
//! <body: the model's clipboard text of that kind>
//! ```
//!
//! Line 1 names the version and the kind (`clips`, `notes`, `points`, `devices`, `tracks`), line 2
//! is a JSON object (`audio` is the folder of the audio bundle the copied audio lives in, so the
//! instance that pastes can fetch the files by their content hash; it is left out when nothing
//! copied has audio), the rest is the body (see `atomik_model::clips_to_text` and its siblings).
//! Text that does not start with the magic is not ours and is ignored; a newer version or an
//! unknown kind is refused with a message, never a panic.

use crate::clipboard::{ClipboardData, ClipboardKind};
use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, clips_from_text, clips_to_text, devices_from_text, devices_to_text, notes_from_text,
    notes_to_text, points_from_text, points_to_text,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The text every envelope starts with, up to the version.
pub const MAGIC: &str = "atomik-clipboard/";
/// The version this build writes and the newest it reads.
pub const VERSION: u32 = 1;
/// The largest envelope Copy writes and Paste reads (bytes). A bigger selection is copied inside
/// this instance only: the write would stall the UI and the other instance's read.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// The system clipboard as text: the seam the app copies through, so tests never touch the real
/// one. Both calls are fallible by design: a headless session has no clipboard, and then the
/// in-process clipboard alone works.
pub trait OsClipboard {
    /// The text on the clipboard, `None` when there is none or it cannot be read.
    fn read(&mut self) -> Option<String>;
    /// Puts `text` on the clipboard. False when it could not be written.
    fn write(&mut self, text: &str) -> bool;
}

/// The clipboard of the UI thread (`atomik_ui::clipboard`): the system clipboard once
/// [`crate::text_clipboard::install`] ran, a private buffer of the thread before that (tests,
/// hosts without a clipboard).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemText;

impl OsClipboard for SystemText {
    fn read(&mut self) -> Option<String> {
        atomik_ui::clipboard::read()
    }

    fn write(&mut self, text: &str) -> bool {
        atomik_ui::clipboard::write(text);
        true
    }
}

/// The clipboard a new core starts with: the thread's own (see [`SystemText`]). The unit tests of
/// this crate start every core with a private in-memory one instead, so no test sees another's
/// copy, and none reaches the real clipboard.
pub fn default_clipboard() -> Box<dyn OsClipboard> {
    #[cfg(test)]
    {
        Box::new(FakeClipboard::default())
    }
    #[cfg(not(test))]
    {
        Box::new(SystemText)
    }
}

/// What an envelope is made from: the copy and where its audio lives.
pub struct Source<'a> {
    /// The document's sample rate and tempo (a device text carries them).
    pub rate: u32,
    /// See `rate`.
    pub tempo: f64,
    /// The folder of the audio bundle of the copying song, if it has one.
    pub bundle: Option<&'a std::path::Path>,
}

/// Why nothing was written for a copy.
#[derive(Debug, PartialEq, Eq)]
pub enum Skipped {
    /// The kind is not shared between instances (strips, takes, settings, controller points).
    Local,
    /// The text would be over [`MAX_BYTES`].
    TooBig(usize),
    /// The writer refused the data.
    Failed(String),
}

/// The envelope of `data`.
pub fn encode(data: &ClipboardData, source: &Source<'_>) -> Result<String, Skipped> {
    let (kind, body, assets): (&str, String, Option<&BTreeMap<String, AssetMeta>>) = match data {
        ClipboardData::Clips(s) => ("clips", clips_to_text(s), Some(&s.assets)),
        ClipboardData::Notes(s) => (
            "notes",
            notes_to_text(s).map_err(|e| Skipped::Failed(e.to_string()))?,
            None,
        ),
        ClipboardData::AutomationPoints(s) => ("points", points_to_text(s), None),
        ClipboardData::Devices(s) => (
            "devices",
            devices_to_text(s, source.rate, source.tempo),
            Some(&s.assets),
        ),
        ClipboardData::Tracks(s) => (
            "tracks",
            devices_to_text(s, source.rate, source.tempo),
            Some(&s.assets),
        ),
        ClipboardData::Strips(_)
        | ClipboardData::Settings(_)
        | ClipboardData::Takes(_)
        | ClipboardData::Controllers(_) => return Err(Skipped::Local),
    };
    let audio = match (assets, source.bundle) {
        (Some(a), Some(folder)) if !a.is_empty() => Some(folder.to_string_lossy().into_owned()),
        _ => None,
    };
    let meta = match audio {
        Some(folder) => format!("{{\"audio\": {}}}", json_string(&folder)),
        None => "{}".to_owned(),
    };
    let text = format!("{MAGIC}{VERSION} {kind}\n{meta}\n{body}");
    if text.len() > MAX_BYTES {
        return Err(Skipped::TooBig(text.len()));
    }
    Ok(text)
}

fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

/// What a clipboard text turned out to be.
#[derive(Debug)]
pub enum Decoded {
    /// Not an Atomik envelope: ignored.
    NotOurs,
    /// An envelope this build cannot use (a newer version, an unknown kind) or one that is
    /// damaged; the message says which.
    Refused(String),
    /// A copy: the data, and the folder its audio lives in.
    Data(Box<ClipboardData>, Option<PathBuf>),
}

/// The kind of an envelope without decoding it (`None`: not an envelope this build reads).
pub fn peek_kind(text: &str) -> Option<ClipboardKind> {
    let (version, kind) = header(text)?;
    if version > VERSION {
        return None;
    }
    kind_of(kind)
}

fn header(text: &str) -> Option<(u32, &str)> {
    let first = text.strip_prefix(MAGIC)?.lines().next()?;
    let (version, kind) = first.split_once(' ')?;
    Some((version.parse().ok()?, kind.trim()))
}

fn kind_of(name: &str) -> Option<ClipboardKind> {
    match name {
        "clips" => Some(ClipboardKind::Clips),
        "notes" => Some(ClipboardKind::Notes),
        "points" => Some(ClipboardKind::AutomationPoints),
        "devices" => Some(ClipboardKind::Devices),
        "tracks" => Some(ClipboardKind::Tracks),
        _ => None,
    }
}

/// Reads a clipboard text. `registry` validates devices (a plugin it lacks loads as a stand-in).
pub fn decode(text: &str, registry: &Registry) -> Decoded {
    if !text.starts_with(MAGIC) {
        return Decoded::NotOurs;
    }
    if text.len() > MAX_BYTES {
        return Decoded::Refused("The clipboard holds more than Atomik Rack pastes".into());
    }
    let Some((version, kind)) = header(text) else {
        return Decoded::Refused("The clipboard holds a damaged Atomik Rack copy".into());
    };
    if version > VERSION {
        return Decoded::Refused(format!(
            "The clipboard was copied by a newer Atomik Rack (copy format {version}): update to paste it"
        ));
    }
    if version == 0 {
        return Decoded::Refused("The clipboard holds a damaged Atomik Rack copy".into());
    }
    let Some(kind) = kind_of(kind) else {
        return Decoded::Refused(format!(
            "The clipboard holds {kind:?}, which this Atomik Rack cannot paste"
        ));
    };
    let mut lines = text.splitn(3, '\n');
    let (_, meta, body) = (lines.next(), lines.next(), lines.next());
    let (Some(meta), Some(body)) = (meta, body) else {
        return Decoded::Refused("The clipboard holds a damaged Atomik Rack copy".into());
    };
    let audio = match serde_json::from_str::<serde_json::Value>(meta) {
        Ok(v) if v.is_object() => v
            .get("audio")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from),
        _ => {
            return Decoded::Refused("The clipboard holds a damaged Atomik Rack copy".into());
        }
    };
    let broken = |what: &str, e: atomik_model::ModelError| {
        Decoded::Refused(format!("The copied {what} cannot be pasted: {e}"))
    };
    let data = match kind {
        ClipboardKind::Clips => clips_from_text(body).map(ClipboardData::Clips),
        ClipboardKind::Notes => notes_from_text(body).map(ClipboardData::Notes),
        ClipboardKind::AutomationPoints => {
            points_from_text(body).map(ClipboardData::AutomationPoints)
        }
        ClipboardKind::Devices => devices_from_text(body, registry).map(ClipboardData::Devices),
        ClipboardKind::Tracks => devices_from_text(body, registry).map(ClipboardData::Tracks),
        _ => unreachable!("kind_of names only the kinds above"),
    };
    match data {
        Ok(d) if d.is_empty() => Decoded::Refused("The clipboard copy holds nothing".into()),
        Ok(d) => Decoded::Data(Box::new(d), audio),
        Err(e) => broken(kind.noun(), e),
    }
}

/// An in-memory clipboard for tests: clones share one text, like two instances of the app share
/// the system clipboard.
#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub struct FakeClipboard {
    /// The text on the clipboard.
    pub text: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    /// How many times anything wrote to it.
    pub writes: std::rc::Rc<std::cell::Cell<usize>>,
    /// Refuses every call, like a session with no clipboard.
    pub broken: bool,
}

#[cfg(test)]
impl OsClipboard for FakeClipboard {
    fn read(&mut self) -> Option<String> {
        if self.broken {
            return None;
        }
        self.text.borrow().clone()
    }

    fn write(&mut self, text: &str) -> bool {
        if self.broken {
            return false;
        }
        self.writes.set(self.writes.get() + 1);
        *self.text.borrow_mut() = Some(text.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;
    use atomik_model::{
        AutomationPoint, ClipSnapshot, Curve, DeviceSnapshot, Note, NoteSnapshot, PointSnapshot,
    };

    fn source() -> Source<'static> {
        Source {
            rate: 48_000,
            tempo: 120.0,
            bundle: None,
        }
    }

    fn notes() -> ClipboardData {
        ClipboardData::Notes(NoteSnapshot {
            notes: vec![Note::new(0, PPQ, 60, 0.5), Note::new(PPQ, PPQ, 64, 0.7)],
            span: 2 * PPQ,
        })
    }

    fn registry() -> Registry {
        crate::app_core::default_registry()
    }

    #[test]
    fn notes_and_points_round_trip_through_the_envelope() {
        let text = encode(&notes(), &source()).unwrap();
        assert!(text.starts_with("atomik-clipboard/1 notes\n"));
        let Decoded::Data(back, audio) = decode(&text, &registry()) else {
            panic!("a notes envelope decodes");
        };
        assert_eq!(*back, notes());
        assert_eq!(audio, None);
        let points = ClipboardData::AutomationPoints(PointSnapshot {
            points: vec![AutomationPoint {
                tick: 0,
                value: 0.5,
                curve: Curve::Bend(10),
            }],
            span: 0,
        });
        let text = encode(&points, &source()).unwrap();
        let Decoded::Data(back, _) = decode(&text, &registry()) else {
            panic!("a points envelope decodes");
        };
        assert_eq!(*back, points);
    }

    #[test]
    fn kinds_that_stay_in_the_instance_are_not_written() {
        let takes = ClipboardData::Takes(atomik_model::TakeSnapshot {
            takes: Vec::new(),
            span: 0,
            assets: BTreeMap::new(),
        });
        assert_eq!(encode(&takes, &source()), Err(Skipped::Local));
        let settings = ClipboardData::Settings(DeviceSnapshot {
            devices: Vec::new(),
            cables: Vec::new(),
            tracks: Vec::new(),
            assets: BTreeMap::new(),
        });
        assert_eq!(encode(&settings, &source()), Err(Skipped::Local));
    }

    #[test]
    fn a_selection_over_the_cap_is_not_written() {
        let big = ClipboardData::Notes(NoteSnapshot {
            notes: (0..400_000)
                .map(|i| Note::new(i * 10, 5, (i % 100) as u8, 0.5))
                .collect(),
            span: 4_000_000,
        });
        let skipped = encode(&big, &source()).unwrap_err();
        assert!(matches!(skipped, Skipped::TooBig(n) if n > MAX_BYTES));
    }

    #[test]
    fn foreign_and_damaged_text_never_panics() {
        let r = registry();
        assert!(matches!(decode("hello", &r), Decoded::NotOurs));
        assert!(matches!(decode("", &r), Decoded::NotOurs));
        for bad in [
            "atomik-clipboard/",
            "atomik-clipboard/x notes\n{}\n{}",
            "atomik-clipboard/1 notes",
            "atomik-clipboard/1 notes\nnot json\n{}",
            "atomik-clipboard/1 notes\n{}\n{\"format\": 1}",
            "atomik-clipboard/1 video\n{}\n{}",
            "atomik-clipboard/0 notes\n{}\n{}",
        ] {
            assert!(
                matches!(decode(bad, &r), Decoded::Refused(_)),
                "{bad:?} is refused"
            );
        }
    }

    #[test]
    fn a_newer_version_is_refused_with_a_message() {
        let text = encode(&notes(), &source()).unwrap().replacen(
            "atomik-clipboard/1",
            "atomik-clipboard/2",
            1,
        );
        let Decoded::Refused(message) = decode(&text, &registry()) else {
            panic!("a newer envelope is refused");
        };
        assert!(message.contains("newer"), "{message}");
        assert_eq!(peek_kind(&text), None);
    }

    #[test]
    fn the_kind_can_be_read_without_decoding() {
        let text = encode(&notes(), &source()).unwrap();
        assert_eq!(peek_kind(&text), Some(ClipboardKind::Notes));
        assert_eq!(peek_kind("atomik-clipboard/1 video\n{}\n"), None);
        assert_eq!(peek_kind("plain text"), None);
    }

    #[test]
    fn audio_clips_name_the_folder_of_their_files() {
        let snapshot = ClipSnapshot {
            entries: Vec::new(),
            origin: 0,
            span: 0,
            assets: [(
                "a".repeat(64),
                AssetMeta {
                    channels: 1,
                    sample_rate: 48_000,
                    frames: 10,
                    original_name: "x.wav".into(),
                },
            )]
            .into(),
        };
        let folder = std::path::Path::new("bundle-folder");
        let text = encode(
            &ClipboardData::Clips(snapshot),
            &Source {
                bundle: Some(folder),
                ..source()
            },
        )
        .unwrap();
        assert!(text.lines().nth(1).unwrap().contains("bundle-folder"));
    }
}
