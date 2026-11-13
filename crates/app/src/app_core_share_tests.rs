//! Sharing a project as one archive in the app core: what the archive holds, the credits, the WAV
//! fallback without FFmpeg, a cancel that leaves nothing, the size warning and the refusal of a
//! hostile archive. All fixtures are procedural and live in temporary folders.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, SongCommand, SongInfo, WorkflowCommand,
};
use atomik_ui::ImportTarget;
use atomik_video::Platform;
use atomik_video::fake::FakeTool;
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-share-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A folder holding stand-ins for the two FFmpeg programs.
    fn programs(&self) -> PathBuf {
        let dir = self.path("ffmpeg-folder");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(dir.join(Platform::current().exe_name(name)), b"x").unwrap();
        }
        dir
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn core(t: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_library_root(Some(t.path("Library")));
    c
}

/// A song with a synth that plays four bars and one imported audio file, saved in `t/home`.
fn song(t: &Tmp) -> (AppCore<RecordingSink>, String) {
    let mut c = core(t);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    let notes: Vec<Note> = (0..8)
        .map(|i| Note::new(i * PPQ * 2, PPQ, 60 + (i as u8 % 5), 0.8))
        .collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 4 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(notes.into()),
        },
    }))
    .unwrap();
    let frames = 9_600;
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * 0.5)
        .collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let before: Vec<String> = c.doc().assets.keys().cloned().collect();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    let hash = c
        .doc()
        .assets
        .keys()
        .find(|h| !before.contains(h))
        .expect("a new asset")
        .clone();
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    (c, hash)
}

fn values() -> ShareValues {
    ShareValues {
        title: "Night Drive".to_owned(),
        author: "Zoe".to_owned(),
        note: "first line\nsecond line".to_owned(),
        preview: true,
        stems: true,
        plugins: true,
    }
}

/// The names of the entries of the archive `path`, read from its central directory.
fn entry_names(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let mut names = Vec::new();
    let mut at = 0;
    while at + 46 <= bytes.len() {
        if bytes[at..at + 4] == [0x50, 0x4b, 0x01, 0x02] {
            let len =
                |o: usize| usize::from(u16::from_le_bytes([bytes[at + o], bytes[at + o + 1]]));
            let (name, extra, comment) = (len(28), len(30), len(32));
            names.push(String::from_utf8_lossy(&bytes[at + 46..at + 46 + name]).into_owned());
            at += 46 + name + extra + comment;
        } else {
            at += 1;
        }
    }
    names.sort();
    names
}

/// Unpacks the share `archive` into `t/<folder>` through the core and returns the project file.
fn open_share(
    c: &mut AppCore<RecordingSink>,
    t: &Tmp,
    archive: &Path,
    folder: &str,
) -> Result<PathBuf, String> {
    c.start_share_open(archive.to_path_buf(), &t.path(folder))?;
    c.wait_background();
    c.take_open_request()
        .ok_or_else(|| c.last_rejection().unwrap_or_default().to_owned())
}

fn render(project: &Path) -> Vec<Vec<f32>> {
    atomik_render::project::render_project_file(
        project,
        &atomik_render::default_registry(),
        &atomik_render::project::ProjectOptions::default(),
    )
    .unwrap()
    .channels
}

// ------------------------------------------------------------------ the archive

#[test]
fn a_share_holds_the_project_the_audio_the_credits_the_preview_and_the_stems_and_opens_the_same() {
    let t = Tmp::new("entries");
    let (mut c, hash) = song(&t);
    std::fs::create_dir_all(t.path("out")).unwrap();
    let dest = t.path("out").join("Night Drive.atomikz");
    c.start_share_on(dest.clone(), &values(), "2026-11-10")
        .unwrap();
    assert!(c.archive_busy());
    let (fraction, text) = c.archive_progress().unwrap();
    assert!((0.0..=1.0).contains(&fraction));
    assert!(text.starts_with("Sharing "), "{text}");
    c.wait_background();
    assert!(!c.archive_busy());
    let message = c.last_rejection().unwrap().to_owned();
    assert!(
        message.starts_with("Shared as Night Drive.atomikz"),
        "{message}"
    );
    assert!(!t.path("out").join("Night Drive.atomikz.part").exists());
    // The entries: the project, its audio, the credits, the preview and the stems.
    let names = entry_names(&dest);
    let want = [
        "Night Drive/Night Drive.atomik".to_owned(),
        format!("Night Drive/Audio/{hash}.wav"),
        "Night Drive/credits.txt".to_owned(),
        "Night Drive/preview.wav".to_owned(),
    ];
    for name in &want {
        assert!(names.contains(name), "{name} is in {names:?}");
    }
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("Night Drive/stems/") && n.ends_with(".wav")),
        "stems are in {names:?}"
    );
    assert!(
        names.iter().all(|n| !n.contains("..") && !n.contains('\\')),
        "{names:?}"
    );
    // The panel's texts are the song information, as one undo step.
    assert_eq!(c.song_info().title, "Night Drive");
    assert_eq!(c.song_info().author, "Zoe");
    assert_eq!(c.song_info().notes, "first line\nsecond line");
    assert!(c.undo());
    assert!(c.song_info().is_empty(), "one undo step");

    // Opening the share gives a project that renders bit for bit like the original.
    let mut other = core(&t);
    let project = open_share(&mut other, &t, &dest, "opened").unwrap();
    assert_eq!(
        project,
        t.path("opened")
            .join("Night Drive")
            .join("Night Drive.atomik")
    );
    let root = t.path("opened").join("Night Drive");
    assert!(root.join("credits.txt").is_file() && root.join("preview.wav").is_file());
    assert!(root.join("stems").is_dir());
    let original = render(&t.path("home").join("song").join("song.atomik"));
    let copy = render(&project);
    assert!(original[0].len() > 48_000, "four bars of song");
    assert!(
        original[0].iter().any(|x| x.abs() > 1e-4),
        "it is not silence"
    );
    assert_eq!(original.len(), copy.len());
    for (a, b) in original.iter().zip(&copy) {
        assert_eq!(a.len(), b.len());
        assert!(
            a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()),
            "the opened copy renders bit for bit like the original"
        );
    }
    // The credits wait for the shell, once, for that project only.
    assert!(
        other
            .take_share_credits(&t.path("elsewhere.atomik"))
            .is_none()
    );
    let credits = other.take_share_credits(&project).unwrap();
    assert!(credits.contains("Title: Night Drive"), "{credits}");
    assert!(other.take_share_credits(&project).is_none());
    other.open(&project).unwrap();
    other.wait_background();
    assert!(other.missing_assets().is_empty());
    assert_eq!(other.song_info().title, "Night Drive");
}

#[test]
fn a_share_with_nothing_extra_is_a_plain_project_archive_with_credits() {
    let t = Tmp::new("plain");
    let (mut c, hash) = song(&t);
    let dest = t.path("plain.atomikz");
    let only = ShareValues {
        preview: false,
        stems: false,
        plugins: false,
        ..values()
    };
    c.start_share_on(dest.clone(), &only, "2026-11-10").unwrap();
    c.wait_background();
    assert_eq!(
        entry_names(&dest),
        [
            format!("Night Drive/Audio/{hash}.wav"),
            "Night Drive/Night Drive.atomik".to_owned(),
            "Night Drive/credits.txt".to_owned(),
        ]
        .map(String::from)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    // The plain import takes it too, beside the file.
    let mut other = core(&t);
    other.start_archive_import(dest).unwrap();
    other.wait_background();
    assert_eq!(
        other.take_open_request(),
        Some(t.path("plain").join("plain.atomik"))
    );
}

#[test]
fn nothing_changes_when_the_share_is_refused() {
    let t = Tmp::new("refused");
    let mut c = core(&t);
    // An empty song has no strips to take as stems.
    let err = c
        .start_share_on(t.path("a.atomikz"), &values(), "2026-11-10")
        .unwrap_err();
    assert!(err.contains("Cannot share with"), "{err}");
    assert!(c.song_info().is_empty(), "the texts were not applied");
    assert!(!c.archive_busy() && !t.path("a.atomikz").exists());
    // Without a preview and stems it works, and a second share waits for the first.
    let bare = ShareValues {
        preview: false,
        stems: false,
        ..values()
    };
    c.start_share_on(t.path("a.atomikz"), &bare, "2026-11-10")
        .unwrap();
    let err = c
        .start_share_on(t.path("b.atomikz"), &bare, "2026-11-10")
        .unwrap_err();
    assert!(err.contains("already being made"), "{err}");
    c.wait_background();
    assert!(t.path("a.atomikz").is_file() && !t.path("b.atomikz").exists());
}

// ------------------------------------------------------------------ the credits

fn credits_info() -> SongInfo {
    SongInfo {
        title: "Night Drive".to_owned(),
        author: "Zoe".to_owned(),
        notes: "first line\nsecond line".to_owned(),
        ..SongInfo::default()
    }
}

#[test]
fn the_credits_say_title_author_note_date_version_and_the_plugins_with_vendor_and_format() {
    let plugins = [
        PluginCredit {
            name: "Big Verb".to_owned(),
            vendor: "Example Audio".to_owned(),
            format: "VST3",
        },
        PluginCredit {
            name: "Tiny\nSynth".to_owned(),
            vendor: String::new(),
            format: "CLAP",
        },
    ];
    let text = share_credits(&credits_info(), "2026-11-10", "1.2.3", Some(&plugins));
    assert_eq!(
        text,
        "Title: Night Drive\nAuthor: Zoe\nNote:\n  first line\n  second line\n\
         Date: 2026-11-10\nMade with: Atomik Rack 1.2.3\nPlugins used (2):\n\
         \x20 Big Verb by Example Audio (VST3)\n  Tiny Synth (CLAP)\n\
         Install the plugins above to hear the song as it was made.\n"
    );
    // Without the plugin list there is no plugin section; without plugins there is "none".
    let bare = share_credits(&credits_info(), "2026-11-10", "1.2.3", None);
    assert!(!bare.contains("Plugins"), "{bare}");
    assert!(
        share_credits(&credits_info(), "2026-11-10", "1.2.3", Some(&[]))
            .contains("Plugins used: none")
    );
    // An empty title is Untitled, and an empty author and note leave no line.
    let empty = share_credits(&SongInfo::default(), "2026-11-10", "1.2.3", None);
    assert_eq!(
        empty,
        "Title: Untitled\nDate: 2026-11-10\nMade with: Atomik Rack 1.2.3\n"
    );
}

#[test]
fn the_plugin_credits_list_each_plugin_once_with_its_format() {
    use crate::plugin_test_support::{GAIN, SYNTH, add_plugin, plugin_core};
    let t = Tmp::new("plugins");
    let mut c = plugin_core(RecordingSink::new());
    add_plugin(&mut c, SYNTH, "p1");
    add_plugin(&mut c, GAIN, "p2");
    add_plugin(&mut c, GAIN, "p3");
    let credits = c.plugin_credits();
    assert_eq!(credits.len(), 2, "{credits:?}");
    assert!(
        credits
            .iter()
            .all(|p| p.format == "CLAP" && !p.name.is_empty())
    );
    // They are in the credits file of the share when asked for, and absent when not.
    let with = ShareValues {
        preview: false,
        stems: false,
        ..values()
    };
    c.start_share_on(t.path("with.atomikz"), &with, "2026-11-10")
        .unwrap();
    c.wait_background();
    let without = ShareValues {
        plugins: false,
        ..with.clone()
    };
    c.start_share_on(t.path("without.atomikz"), &without, "2026-11-10")
        .unwrap();
    c.wait_background();
    let mut other = core(&t);
    let project = open_share(&mut other, &t, &t.path("with.atomikz"), "with").unwrap();
    let text = std::fs::read_to_string(project.parent().unwrap().join("credits.txt")).unwrap();
    assert!(
        text.contains("Plugins used (2):") && text.contains("(CLAP)"),
        "{text}"
    );
    for p in &credits {
        assert!(text.contains(&p.name), "{} is in {text}", p.name);
    }
    let project = open_share(&mut other, &t, &t.path("without.atomikz"), "without").unwrap();
    let text = std::fs::read_to_string(project.parent().unwrap().join("credits.txt")).unwrap();
    assert!(!text.contains("Plugins used"), "{text}");
}

// ------------------------------------------------------------------ the preview

#[test]
fn without_ffmpeg_the_preview_is_a_16_bit_wav_at_minus_14_and_with_it_an_mp3() {
    let t = Tmp::new("preview");
    let (mut c, _) = song(&t);
    assert_eq!(c.share_preview_kind(), SharePreview::Wav);
    let only_preview = ShareValues {
        stems: false,
        ..values()
    };
    c.start_share_on(t.path("wav.atomikz"), &only_preview, "2026-11-10")
        .unwrap();
    c.wait_background();
    let names = entry_names(&t.path("wav.atomikz"));
    assert!(
        names.contains(&"Night Drive/preview.wav".to_owned()),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("preview.mp3")),
        "{names:?}"
    );
    let mut other = core(&t);
    let project = open_share(&mut other, &t, &t.path("wav.atomikz"), "wav").unwrap();
    let preview = project.parent().unwrap().join("preview.wav");
    let spec = hound::WavReader::open(&preview).unwrap().spec();
    assert_eq!((spec.bits_per_sample, spec.channels), (16, 2));
    // Brought to -14 LUFS through the export path.
    let decoded = atomik_audio_file::decode_file(&preview).unwrap();
    let loud = atomik_render::loudness::measure(&decoded);
    let lufs = loud.integrated_lufs.expect("the preview is not silent");
    assert!((lufs + 14.0).abs() < 1.0, "{lufs} LUFS");

    // FFmpeg found: an MP3 through the encoder.
    let tool = Arc::new(FakeTool::new().audio(b"ENCODED".to_vec()));
    c.set_video_tool(Arc::clone(&tool) as Arc<dyn atomik_video::VideoTool>);
    c.set_video_folder(Some(t.programs()));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert_eq!(c.share_preview_kind(), SharePreview::Mp3);
    c.start_share_on(t.path("mp3.atomikz"), &only_preview, "2026-11-10")
        .unwrap();
    c.wait_background();
    let names = entry_names(&t.path("mp3.atomikz"));
    assert!(
        names.contains(&"Night Drive/preview.mp3".to_owned()),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("preview.wav")),
        "{names:?}"
    );
    let project = open_share(&mut other, &t, &t.path("mp3.atomikz"), "mp3").unwrap();
    assert_eq!(
        std::fs::read(project.parent().unwrap().join("preview.mp3")).unwrap(),
        b"ENCODED"
    );
    let encodes: Vec<_> = tool.calls().into_iter().filter(|c| c.has("-vn")).collect();
    assert_eq!(encodes.len(), 1);
    assert_eq!(encodes[0].after("-b:a").unwrap(), "192k");
}

// ------------------------------------------------------------------ cancel

#[test]
fn a_cancelled_share_leaves_no_archive_and_no_part_file() {
    let t = Tmp::new("cancel");
    let (mut c, _) = song(&t);
    let release = crate::archive_worker::hold::next_job();
    c.start_share_on(t.path("c.atomikz"), &values(), "2026-11-10")
        .unwrap();
    assert!(c.archive_busy());
    c.cancel_archive();
    drop(release);
    c.wait_background();
    assert!(!c.archive_busy());
    assert!(!t.path("c.atomikz").exists());
    assert!(!t.path("c.atomikz.part").exists());
    let message = c.last_rejection().unwrap();
    assert_eq!(message, "Sharing c.atomikz cancelled");
    // A new share can start, and an old file is not touched by a cancelled one.
    std::fs::write(t.path("keep.atomikz"), b"old").unwrap();
    let release = crate::archive_worker::hold::next_job();
    c.start_share_on(t.path("keep.atomikz"), &values(), "2026-11-10")
        .unwrap();
    c.cancel_archive();
    drop(release);
    c.wait_background();
    assert_eq!(std::fs::read(t.path("keep.atomikz")).unwrap(), b"old");
    assert!(!t.path("keep.atomikz.part").exists());
}

// ------------------------------------------------------------------ size and names

#[test]
fn a_share_over_500_mb_warns_with_its_size_first() {
    let size = |bytes| ShareSize {
        bytes,
        preview: SharePreview::Wav,
    };
    assert_eq!(
        size(820_000_000).text(),
        "This share will be about 820.0 MB"
    );
    assert!(size(820_000_000).is_big());
    assert!(!size(SHARE_WARN_BYTES).is_big());
    assert!(size(SHARE_WARN_BYTES + 1).is_big());
    assert_eq!(SHARE_WARN_BYTES, 500_000_000);
    assert_eq!(size(2_500).text(), "This share will be about 2.5 KB");
}

#[test]
fn the_estimate_follows_the_choices_and_the_names_follow_the_title() {
    let t = Tmp::new("size");
    let (c, _) = song(&t);
    let all = c.share_size(&values());
    let none = c.share_size(&ShareValues {
        preview: false,
        stems: false,
        ..values()
    });
    assert!(all.bytes > none.bytes, "{all:?} {none:?}");
    assert!(none.bytes > 9_600 * 4, "the audio is counted");
    assert_eq!(none.preview, SharePreview::Off);
    assert_eq!(all.preview, SharePreview::Wav);
    assert_eq!(c.share_file_name("Night Drive"), "Night Drive.atomikz");
    assert_eq!(c.share_file_name("a/b:c"), "a_b_c.atomikz");
    assert_eq!(c.share_file_name("  "), "song.atomikz");
    assert_eq!(
        c.shared_dir(),
        Some(t.path("Shared")),
        "Shared sits beside the library folder"
    );
    assert!(c.share_defaults().preview);
    assert!(!c.share_defaults().stems);
}

// ------------------------------------------------------------------ hostile archives

/// A stored ZIP file with these `(name, data)` entries, made by hand with any names.
fn hostile_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for b in data {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let (crc, len, n) = (crc(data), data.len() as u32, name.len() as u16);
        let mut head = Vec::new();
        head.extend(0x0403_4b50u32.to_le_bytes());
        head.extend([20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        head.extend(crc.to_le_bytes());
        head.extend(len.to_le_bytes());
        head.extend(len.to_le_bytes());
        head.extend(n.to_le_bytes());
        head.extend(0u16.to_le_bytes());
        out.extend(&head);
        out.extend(name.as_bytes());
        out.extend(*data);
        let mut dir = Vec::new();
        dir.extend(0x0201_4b50u32.to_le_bytes());
        dir.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        dir.extend(crc.to_le_bytes());
        dir.extend(len.to_le_bytes());
        dir.extend(len.to_le_bytes());
        dir.extend(n.to_le_bytes());
        dir.extend([0u8; 12]);
        dir.extend(offset.to_le_bytes());
        dir.extend(name.as_bytes());
        directory.extend(dir);
    }
    let start = out.len() as u32;
    out.extend(&directory);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((directory.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[test]
fn a_hostile_share_is_refused_before_anything_is_written() {
    let t = Tmp::new("hostile");
    let mut c = core(&t);
    for (tag, name) in [
        ("up", "../evil.txt"),
        ("deep", "song/stems/../../evil.txt"),
        ("abs", "/evil.txt"),
        ("drive", "C:/evil.txt"),
        ("back", "song/stems/..\\evil.wav"),
        ("other", "song/notes.txt"),
    ] {
        let file = t.path(&format!("{tag}.atomikz"));
        std::fs::write(
            &file,
            hostile_zip(&[("song/song.atomik", b"{}"), (name, b"x")]),
        )
        .unwrap();
        let parent = t.path(&format!("into-{tag}"));
        c.start_share_open(file, &parent).unwrap();
        c.wait_background();
        assert!(c.take_open_request().is_none(), "{name}");
        let message = c.last_rejection().unwrap_or_default().to_owned();
        assert!(
            message.contains("failed") && message.contains("refused"),
            "{name}: {message}"
        );
        // The folder was made for the unpack, and nothing is in it or beside it.
        assert_eq!(
            std::fs::read_dir(&parent).unwrap().count(),
            0,
            "{name}: nothing was written"
        );
        assert!(!t.path("evil.txt").exists() && !t.0.parent().unwrap().join("evil.txt").exists());
    }
    // A share that is not an archive says so.
    std::fs::write(t.path("junk.atomikz"), b"not an archive").unwrap();
    c.start_share_open(t.path("junk.atomikz"), &t.path("into-junk"))
        .unwrap();
    c.wait_background();
    assert!(c.take_open_request().is_none());
    assert!(c.last_rejection().unwrap().contains("failed"));
}
