//! The small picture of a song on the start screen's template and example cards: the arrangement
//! seen from far away, one row per track with a block for each clip in the track's colour.
//!
//! [`CardPicture::of_song`] reads a project file's text and keeps only what the picture needs
//! (where each clip starts and ends as a share of the song, and the colours). The picture is
//! cached on disk by [`PictureStore`], one small text file per project file, named by a hash of
//! the file's content: a file that did not change finds its picture ready, and a file that
//! changed has a new name, so the old picture is never shown for it (the oldest pictures are
//! removed past [`KEEP_FILES`]). The work (reading the file, hashing it, loading the project,
//! writing the picture) happens on the thread of a [`PictureWorker`]; the interface thread only
//! asks ([`PictureWorker::request`]) and collects what is done ([`PictureWorker::poll`]), and
//! draws a placeholder until the answer is there ([`paint`]). A file that cannot be read has no
//! picture and its card is drawn without one.
//!
//! The colours are not kept as colours: a track keeps its own colour when it has one and its
//! device type otherwise, so the picture takes the accents of the theme in use when it is drawn.

use atomik_device_sdk::Registry;
use atomik_model::Song;
use atomik_render::bundle::resolve_project_path;
use atomik_ui::kurbo::{Rect, RoundedRect};
use atomik_ui::peniko::Color;
use atomik_ui::{Canvas, CanvasExt, Theme};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// The most track rows a picture keeps.
pub const MAX_ROWS: usize = 12;

/// The most clips a picture keeps for one track.
pub const MAX_CLIPS: usize = 256;

/// How many cached pictures stay in the folder; past this the oldest are removed.
pub const KEEP_FILES: usize = 200;

/// The first line of a cached picture. A change of the format changes it, and a file with
/// another first line is made again.
const HEADER: &str = "atomik-card-picture 1";

/// A share of the song is kept in this many parts.
const SCALE: f64 = 10_000.0;

/// One clip: where it starts and ends as a share of the song (0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct PictureClip {
    /// Where it starts.
    pub start: f32,
    /// Where it ends.
    pub end: f32,
    /// The clip's own colour (or its lane's), else the track's.
    pub color: Option<[u8; 3]>,
}

/// One track's row.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureRow {
    /// The type of the track's device, for the accent it gets when it has no colour.
    pub type_id: String,
    /// The track's own colour.
    pub color: Option<[u8; 3]>,
    /// Its clips.
    pub clips: Vec<PictureClip>,
}

/// The picture of a song.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardPicture {
    /// The track rows, in the order of the arrangement.
    pub rows: Vec<PictureRow>,
}

fn share(tick: i64, end: i64) -> f32 {
    (tick.clamp(0, end) as f64 / end as f64) as f32
}

impl CardPicture {
    /// What the picture keeps of a song: the note and audio lanes of each track, merged into one
    /// row (automation lanes have no clip blocks worth showing).
    pub fn of_song(doc: &atomik_model::RackDoc) -> Self {
        let song: &Song = &doc.song;
        let end = song.end_tick().max(1);
        let rows = song
            .tracks
            .iter()
            .take(MAX_ROWS)
            .map(|track| {
                let device = doc.device(&track.device);
                let mut clips = Vec::new();
                for lane in &track.lanes {
                    if matches!(lane.kind, atomik_model::LaneKind::Automation(_)) {
                        continue;
                    }
                    for clip in &lane.clips {
                        if clips.len() >= MAX_CLIPS {
                            break;
                        }
                        let (start, stop) = (clip.start, clip.start + clip.length);
                        if stop <= 0 || start >= end {
                            continue;
                        }
                        clips.push(PictureClip {
                            start: share(start, end),
                            end: share(stop, end),
                            color: clip.color.or(lane.color),
                        });
                    }
                }
                PictureRow {
                    type_id: device.map_or_else(|| track.device.0.clone(), |d| d.type_id.clone()),
                    color: device.and_then(|d| d.color),
                    clips,
                }
            })
            .collect();
        Self { rows }
    }

    /// The text a picture is cached as.
    pub fn encode(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        let color = |c: Option<[u8; 3]>| {
            c.map_or_else(
                || "-".to_string(),
                |[r, g, b]| format!("{r:02x}{g:02x}{b:02x}"),
            )
        };
        for row in &self.rows {
            out.push_str(&format!("row {} {}\n", color(row.color), row.type_id));
            for c in &row.clips {
                out.push_str(&format!(
                    "clip {} {} {}\n",
                    (f64::from(c.start) * SCALE).round() as u32,
                    (f64::from(c.end) * SCALE).round() as u32,
                    color(c.color)
                ));
            }
        }
        out
    }

    /// A picture from its cached text; `None` when the text is not one (another version, cut
    /// short or damaged).
    pub fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let color = |s: &str| -> Option<Option<[u8; 3]>> {
            if s == "-" {
                return Some(None);
            }
            if s.len() != 6 || !s.is_ascii() {
                return None;
            }
            let byte = |i: usize| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok();
            Some(Some([byte(0)?, byte(1)?, byte(2)?]))
        };
        let mut rows: Vec<PictureRow> = Vec::new();
        for line in lines {
            let mut parts = line.splitn(3, ' ');
            match parts.next()? {
                "row" => {
                    let own = color(parts.next()?)?;
                    rows.push(PictureRow {
                        type_id: parts.next()?.to_string(),
                        color: own,
                        clips: Vec::new(),
                    });
                    if rows.len() > MAX_ROWS {
                        return None;
                    }
                }
                "clip" => {
                    let start: u32 = parts.next()?.parse().ok()?;
                    let mut rest = parts.next()?.split(' ');
                    let end: u32 = rest.next()?.parse().ok()?;
                    let own = color(rest.next()?)?;
                    let row = rows.last_mut()?;
                    if row.clips.len() >= MAX_CLIPS || start > end || end > SCALE as u32 {
                        return None;
                    }
                    row.clips.push(PictureClip {
                        start: (f64::from(start) / SCALE) as f32,
                        end: (f64::from(end) / SCALE) as f32,
                        color: own,
                    });
                }
                _ => return None,
            }
        }
        Some(Self { rows })
    }
}

/// The name a project's text is cached under: a 64-bit FNV-1a hash of its bytes and its length.
pub fn content_key(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}-{:x}", text.len())
}

/// Where pictures are cached.
#[derive(Debug, Clone, Default)]
pub struct PictureStore {
    dir: Option<PathBuf>,
}

/// Where a picture came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The cache had it.
    Cache,
    /// It was made from the project file (and cached when there is a folder).
    Made,
}

impl PictureStore {
    /// A store in `dir`; `None` keeps nothing on disk (every picture is made each time).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    fn file(&self, key: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{key}.cardpic")))
    }

    /// The cached picture of a content key, when there is a good one.
    pub fn fetch(&self, key: &str) -> Option<CardPicture> {
        let text = std::fs::read_to_string(self.file(key)?).ok()?;
        CardPicture::decode(&text)
    }

    /// Caches a picture (through a temporary file, so a half-written picture is never read). A
    /// failure only means the picture is made again next time.
    pub fn put(&self, key: &str, picture: &CardPicture) {
        let (Some(dir), Some(file)) = (&self.dir, self.file(key)) else {
            return;
        };
        let temp = file.with_extension("tmp");
        let written = std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&temp, picture.encode()))
            .and_then(|()| std::fs::rename(&temp, &file));
        match written {
            Ok(()) => self.trim(dir),
            Err(e) => log::debug!("card picture not cached: {e}"),
        }
    }

    /// Removes the oldest pictures past [`KEEP_FILES`].
    fn trim(&self, dir: &Path) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = read
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "cardpic"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        if files.len() <= KEEP_FILES {
            return;
        }
        files.sort();
        for (_, path) in files.iter().take(files.len() - KEEP_FILES) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The picture of the project file at `path`: the cached one when the file's content has
    /// one, else made with `registry` and cached. `None` when the file cannot be read or is not
    /// a project this build can load.
    pub fn picture_of(&self, path: &Path, registry: &Registry) -> Option<(CardPicture, Source)> {
        let file = resolve_project_path(path);
        let text = atomik_model::read_document_text(&file).ok()?;
        let key = content_key(&text);
        if let Some(picture) = self.fetch(&key) {
            return Some((picture, Source::Cache));
        }
        let (doc, _) = atomik_model::load_project_with_warnings(&text, registry).ok()?;
        let picture = CardPicture::of_song(&doc);
        self.put(&key, &picture);
        Some((picture, Source::Made))
    }
}

/// What the worker answers.
#[derive(Debug)]
pub struct Reply {
    /// The number the request had.
    pub id: u32,
    /// The picture, or `None` when the file has none.
    pub picture: Option<CardPicture>,
    /// Where it came from.
    pub source: Option<Source>,
}

/// Makes pictures on a thread of its own, one request at a time in the order asked. The thread
/// ends when the worker is dropped.
pub struct PictureWorker {
    requests: Sender<(u32, PathBuf)>,
    replies: Receiver<Reply>,
}

impl PictureWorker {
    /// Starts the thread. `None` when the operating system refuses a thread.
    pub fn start(store: PictureStore) -> Option<Self> {
        let (requests, wanted) = channel::<(u32, PathBuf)>();
        let (answer, replies) = channel::<Reply>();
        std::thread::Builder::new()
            .name("atomik-card-pictures".into())
            .spawn(move || {
                // Built on the first request: a start screen with nothing to draw costs nothing.
                let mut registry: Option<Registry> = None;
                for (id, path) in wanted {
                    let registry = registry.get_or_insert_with(crate::app_core::default_registry);
                    let made = store.picture_of(&path, registry);
                    let reply = Reply {
                        id,
                        source: made.as_ref().map(|(_, s)| *s),
                        picture: made.map(|(p, _)| p),
                    };
                    if answer.send(reply).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Self { requests, replies })
    }

    /// Asks for the picture of the project at `path`; returns at once.
    pub fn request(&self, id: u32, path: PathBuf) {
        let _ = self.requests.send((id, path));
    }

    /// The pictures that are done since the last call.
    pub fn poll(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }
}

/// How a card's picture stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Slot {
    /// Not asked for yet.
    #[default]
    Idle,
    /// Asked for, not done.
    Making,
    /// Done.
    Ready(CardPicture),
    /// The file has no picture: the card is drawn without one.
    Missing,
}

impl Slot {
    /// Whether the card keeps room for a picture (a placeholder or the picture).
    pub fn has_room(&self) -> bool {
        !matches!(self, Slot::Missing)
    }
}

/// The colour of a row of the picture in `theme`.
fn row_color(theme: &Theme, row: &PictureRow) -> Color {
    match row.color {
        Some([r, g, b]) => Color::from_rgba8(r, g, b, 255),
        None => theme.accent_for_type(&row.type_id),
    }
}

/// Draws a picture slot in `area`: the picture, or a quiet placeholder while it is made.
pub fn paint(canvas: &mut dyn Canvas, theme: &Theme, area: Rect, slot: &Slot) {
    canvas.fill_with(RoundedRect::from_rect(area, 4.0), theme.lcd);
    let inner = area.inset(-3.0);
    match slot {
        Slot::Ready(picture) if !picture.rows.is_empty() => {
            canvas.push_clip(&RoundedRect::from_rect(area, 4.0).into());
            let n = picture.rows.len() as f64;
            let gap = if inner.height() / n > 4.0 { 1.0 } else { 0.0 };
            let h = (inner.height() - gap * (n - 1.0)) / n;
            for (i, row) in picture.rows.iter().enumerate() {
                let y = inner.y0 + (h + gap) * i as f64;
                let base = row_color(theme, row);
                for clip in &row.clips {
                    let x0 = inner.x0 + inner.width() * f64::from(clip.start);
                    let x1 = (inner.x0 + inner.width() * f64::from(clip.end)).max(x0 + 1.0);
                    let color = clip
                        .color
                        .map_or(base, |[r, g, b]| Color::from_rgba8(r, g, b, 255));
                    canvas.fill_with(Rect::new(x0, y, x1.min(inner.x1), y + h), color);
                }
            }
            canvas.pop_clip();
        }
        // Being made (or an empty song): three faint bars, nothing that moves.
        _ => {
            let bar = inner.height() * 0.2;
            for (k, (a, b)) in [(0.05, 0.55), (0.30, 0.85), (0.15, 0.70)]
                .into_iter()
                .enumerate()
            {
                let y = inner.y0 + inner.height() * (0.08 + 0.33 * k as f64);
                canvas.fill_with(
                    Rect::new(
                        inner.x0 + inner.width() * a,
                        y,
                        inner.x0 + inner.width() * b,
                        y + bar,
                    ),
                    theme.section_line,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CardPicture {
        CardPicture {
            rows: vec![
                PictureRow {
                    type_id: "atomik.synth".into(),
                    color: Some([1, 2, 3]),
                    clips: vec![
                        PictureClip {
                            start: 0.0,
                            end: 0.25,
                            color: None,
                        },
                        PictureClip {
                            start: 0.5,
                            end: 1.0,
                            color: Some([255, 0, 128]),
                        },
                    ],
                },
                PictureRow {
                    type_id: "atomik.drums".into(),
                    color: None,
                    clips: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn a_picture_survives_its_cached_text_and_damage_is_refused() {
        let p = sample();
        assert_eq!(CardPicture::decode(&p.encode()), Some(p.clone()));
        assert_eq!(CardPicture::decode(""), None);
        assert_eq!(CardPicture::decode("atomik-card-picture 2\n"), None);
        assert_eq!(
            CardPicture::decode(&format!("{HEADER}\nclip 0 1 -\n")),
            None
        );
        assert_eq!(
            CardPicture::decode(&format!("{HEADER}\nrow - a\nclip 5 1 -\n")),
            None
        );
        assert_eq!(CardPicture::decode(&format!("{HEADER}\nrow zz a\n")), None);
    }

    #[test]
    fn the_key_follows_the_content() {
        assert_eq!(content_key("abc"), content_key("abc"));
        assert_ne!(content_key("abc"), content_key("abd"));
        assert_ne!(content_key("abc"), content_key("abc "));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("atomik-card-picture-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const DEMO: &str = include_str!("../../../examples/projects/demo.atomik");

    fn cached(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|r| {
                r.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "cardpic"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The picture is made once, read back from the cache the second time, and made again for a
    /// file whose content changed; a file that is not a project has none.
    #[test]
    fn a_picture_is_made_once_reused_and_refreshed_when_the_file_changes() {
        let dir = scratch("cache");
        let song = dir.join("song.atomik");
        std::fs::write(&song, DEMO).unwrap();
        let store = PictureStore::new(Some(dir.join("cards")));
        let registry = crate::app_core::default_registry();

        let (first, source) = store.picture_of(&song, &registry).unwrap();
        assert_eq!(source, Source::Made);
        assert!(!first.rows.is_empty(), "the demo has tracks");
        assert!(first.rows.iter().any(|r| !r.clips.is_empty()), "and clips");
        assert_eq!(cached(&dir.join("cards")), 1);

        let (again, source) = store.picture_of(&song, &registry).unwrap();
        assert_eq!(source, Source::Cache, "an unchanged file finds its picture");
        assert_eq!(again, first);

        // A change of the file's content (here a trailing space) is a new key: made again.
        std::fs::write(
            &song,
            format!(
                "{DEMO}
 "
            ),
        )
        .unwrap();
        let (_, source) = store.picture_of(&song, &registry).unwrap();
        assert_eq!(
            source,
            Source::Made,
            "a changed file is not given the old picture"
        );
        assert_eq!(cached(&dir.join("cards")), 2);

        // Not a project, not there: no picture.
        std::fs::write(dir.join("junk.atomik"), "not a project").unwrap();
        assert!(
            store
                .picture_of(&dir.join("junk.atomik"), &registry)
                .is_none()
        );
        assert!(
            store
                .picture_of(&dir.join("gone.atomik"), &registry)
                .is_none()
        );
        // No folder: made each time, nothing written.
        let none = PictureStore::new(None);
        assert_eq!(none.picture_of(&song, &registry).unwrap().1, Source::Made);
        assert_eq!(none.picture_of(&song, &registry).unwrap().1, Source::Made);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A damaged cache file is not shown: the picture is made again over it.
    #[test]
    fn a_damaged_cached_picture_is_made_again() {
        let dir = scratch("damaged");
        let song = dir.join("song.atomik");
        std::fs::write(&song, DEMO).unwrap();
        let store = PictureStore::new(Some(dir.join("cards")));
        let registry = crate::app_core::default_registry();
        let key = content_key(DEMO);
        std::fs::create_dir_all(dir.join("cards")).unwrap();
        std::fs::write(dir.join("cards").join(format!("{key}.cardpic")), "junk").unwrap();
        let (_, source) = store.picture_of(&song, &registry).unwrap();
        assert_eq!(source, Source::Made);
        assert_eq!(store.picture_of(&song, &registry).unwrap().1, Source::Cache);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The worker answers off the asking thread: a request returns at once, and the reply comes
    /// (within a generous bound) with the picture, or none for a file that is not a project.
    #[test]
    fn the_worker_answers_requests_in_the_background() {
        let dir = scratch("worker");
        let song = dir.join("song.atomik");
        std::fs::write(&song, DEMO).unwrap();
        std::fs::write(dir.join("junk.atomik"), "nope").unwrap();
        let worker = PictureWorker::start(PictureStore::new(Some(dir.join("cards")))).unwrap();
        worker.request(7, song);
        worker.request(8, dir.join("junk.atomik"));
        let mut got: Vec<Reply> = Vec::new();
        // A generous bound (a minute of waits), however slow the machine is.
        for _ in 0..12_000 {
            if got.len() >= 2 {
                break;
            }
            got.extend(worker.poll());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got.len(), 2, "both requests are answered");
        assert_eq!(got[0].id, 7);
        assert!(got[0].picture.is_some() && got[0].source == Some(Source::Made));
        assert_eq!(got[1].id, 8);
        assert!(got[1].picture.is_none());
        drop(worker);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slot_without_a_picture_keeps_no_room() {
        assert!(Slot::Idle.has_room() && Slot::Making.has_room());
        assert!(Slot::Ready(sample()).has_room());
        assert!(!Slot::Missing.has_room());
    }
}
