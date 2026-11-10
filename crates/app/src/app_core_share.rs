//! Sharing a project as one archive, and opening a shared one, in the app core.
//!
//! **File > Share Project** makes one `.atomikz` file (the project archive of
//! `app_core_archive.rs`, which also holds extra entries) that opens on any machine:
//!
//! * the project and every audio file it uses, taken from the session's bundle or from the
//!   folders a moved project keeps its audio in (the same places Collect Files looks), so the
//!   archive does not need the factory folder or the audio folder of the sender;
//! * `credits.txt`, written from the song information, the date, the version of the program and,
//!   when asked, the plugins the song uses with their vendor and format ([`share_credits`]);
//! * `preview.mp3`, a listening copy of the whole song at -14 LUFS through the export path (a
//!   `preview.wav` of 16 bits when FFmpeg is not set up), when asked;
//! * `stems/`, one 24-bit WAV file per mixer strip through the stems export, when asked.
//!
//! The title, author and note of the panel go into the song information first, as one undo
//! step, when they differ. Everything runs on the archive worker ([`crate::archive_worker`]): the
//! renders, then the archive, with one progress and one cancel, and a cancel or a failure leaves
//! no archive (and removes the renders). A share over [`SHARE_WARN_BYTES`] says its size first.
//!
//! **Opening a share** is File > Open (or a drop on the window) of an `.atomikz` file: the shell
//! asks which folder to unpack into (it starts in `Shared` beside the library folder,
//! [`AppCore::shared_dir`]), the archive goes into a new folder of it named like the file, and the
//! project opens. The bounded-reading rules of the archive apply (see `atomik-audio-file`):
//! entry names that leave the folder, unknown entries and oversize ones refuse the whole archive
//! before a byte is written. [`AppCore::take_share_credits`] hands the shell the credits once the
//! project is open.

use super::*;
use crate::archive_worker::{Job, RENDER_UNITS, ShareJob};
use crate::audio_import::display_name;
use atomik_audio_file::{ArchiveExtras, archive_work_with, is_valid_hash};
use atomik_render::export::{DEFAULT_TAIL_SECONDS, ExportSpan, span_frames};
use atomik_ui::{
    EncoderState, ExportDepth, ExportDither, ExportFile, ExportLoudness, ExportQuality,
    ExportScope, ExportValues, ShareValues,
};

/// The folder of the shares, next to the library folder.
pub const SHARED_FOLDER: &str = "Shared";

/// A share over this many bytes warns about its size.
pub const SHARE_WARN_BYTES: u64 = 500_000_000;

/// The loudness of the preview in LUFS.
const PREVIEW_LUFS: i8 = -14;

/// The bitrate of an MP3 preview, in kilobits per second.
const PREVIEW_KBPS: u16 = 192;

/// Most plugins credits.txt lists (a project cannot have more sensible ones).
const MAX_CREDITED_PLUGINS: usize = 300;

/// What the preview of a share is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePreview {
    /// There is none.
    Off,
    /// An MP3 file.
    Mp3,
    /// A WAV file: FFmpeg is not set up.
    Wav,
}

/// What a share would weigh, for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareSize {
    /// About how many bytes the archive would have.
    pub bytes: u64,
    /// What the preview would be.
    pub preview: SharePreview,
}

impl ShareSize {
    /// `This share will be about 820 MB`: the size first, as the warning must say it.
    pub fn text(&self) -> String {
        format!(
            "This share will be about {}",
            super::archive::size_text(self.bytes)
        )
    }

    /// Whether the share is big enough to warn about.
    pub fn is_big(&self) -> bool {
        self.bytes > SHARE_WARN_BYTES
    }
}

/// One plugin of the credits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCredit {
    /// The plugin's name.
    pub name: String,
    /// Who makes it.
    pub vendor: String,
    /// `CLAP` or `VST3`.
    pub format: &'static str,
}

/// A name for one line of the credits: control characters become spaces.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The text of `credits.txt`: the title, the author and the note of `info`, the date (`2026-11-10`)
/// and the version of the program, and, when `plugins` is given, the plugins the song uses with
/// their vendor and format, so the receiver knows what is missing.
pub fn share_credits(
    info: &atomik_model::SongInfo,
    date: &str,
    version: &str,
    plugins: Option<&[PluginCredit]>,
) -> String {
    let mut out = String::new();
    let title = one_line(&info.title);
    out.push_str(&format!(
        "Title: {}\n",
        if title.is_empty() { "Untitled" } else { &title }
    ));
    let author = one_line(&info.author);
    if !author.is_empty() {
        out.push_str(&format!("Author: {author}\n"));
    }
    let note = info.notes.trim();
    if !note.is_empty() {
        out.push_str("Note:\n");
        for line in note.lines() {
            let line: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            out.push_str(&format!("  {}\n", line.trim_end()));
        }
    }
    out.push_str(&format!("Date: {date}\n"));
    out.push_str(&format!("Made with: Atomik Rack {version}\n"));
    if let Some(plugins) = plugins {
        if plugins.is_empty() {
            out.push_str("Plugins used: none\n");
        } else {
            out.push_str(&format!("Plugins used ({}):\n", plugins.len()));
            for p in plugins.iter().take(MAX_CREDITED_PLUGINS) {
                let vendor = one_line(&p.vendor);
                let by = if vendor.is_empty() {
                    String::new()
                } else {
                    format!(" by {vendor}")
                };
                out.push_str(&format!("  {}{by} ({})\n", one_line(&p.name), p.format));
            }
            if plugins.len() > MAX_CREDITED_PLUGINS {
                out.push_str(&format!(
                    "  and {} more\n",
                    plugins.len() - MAX_CREDITED_PLUGINS
                ));
            }
            out.push_str("Install the plugins above to hear the song as it was made.\n");
        }
    }
    out
}

/// The folder a share renders its preview and stems into: one per process and share (several
/// songs of one process can share at the same time).
fn share_temp_folder() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "atomik-share-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

/// Today's date (UTC) as `2026-11-10`.
#[allow(
    clippy::disallowed_methods,
    reason = "the wall clock only dates the credits; it never reaches audio or the project"
)]
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    crate::logging::date_stamp(secs)
}

/// The values of the preview's export: MP3 at [`PREVIEW_KBPS`], or a 16-bit WAV file without
/// FFmpeg, both brought to [`PREVIEW_LUFS`].
fn preview_values(kind: SharePreview) -> ExportValues {
    let loudness = ExportLoudness::Target(PREVIEW_LUFS);
    match kind {
        SharePreview::Mp3 => ExportValues {
            file: ExportFile::Mp3,
            mp3: ExportQuality::Cbr(PREVIEW_KBPS),
            loudness,
            ..ExportValues::default()
        },
        _ => ExportValues {
            file: ExportFile::Wav,
            depth: ExportDepth::Pcm16,
            dither: ExportDither::Tpdf,
            loudness,
            ..ExportValues::default()
        },
    }
}

/// The values of the stems' export: 24-bit WAV, the whole song.
fn stems_values() -> ExportValues {
    ExportValues {
        dither: ExportDither::Off,
        ..ExportValues::default()
    }
}

/// What the archive is made of: the project text and the audio files found.
struct Collected {
    text: String,
    audio: Vec<(String, PathBuf)>,
    /// Audio the song uses that no folder has.
    missing: usize,
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// The folder of the shares: `Shared` next to the library folder. `None` while the library
    /// folder is not known or has no parent.
    pub fn shared_dir(&self) -> Option<PathBuf> {
        let library = self.library_root()?;
        Some(library.parent()?.join(SHARED_FOLDER))
    }

    /// The panel's starting values: the song's title, author and note, a preview and the plugin
    /// list on, the stems off.
    pub fn share_defaults(&self) -> ShareValues {
        let info = self.song_info();
        ShareValues {
            title: info.title.clone(),
            author: info.author.clone(),
            note: info.notes.clone(),
            ..ShareValues::default()
        }
    }

    /// What the preview of a share would be now: an MP3 when FFmpeg is set up, else a WAV file.
    pub fn share_preview_kind(&self) -> SharePreview {
        if self.video_tool_and_folder().is_some() && self.export_encoder() == EncoderState::Ready {
            SharePreview::Mp3
        } else {
            SharePreview::Wav
        }
    }

    /// The name a share of the song goes by: `<title>.atomikz`.
    pub fn share_file_name(&self, title: &str) -> String {
        let title = title.trim();
        let stem = if title.is_empty() {
            self.archive_name()
        } else {
            super::templates::template_stem(title)
        };
        format!("{stem}.{}", atomik_audio_file::ARCHIVE_EXTENSION)
    }

    /// The project text and the audio files a share holds. The text is the song as File > Save
    /// would write it (without the audio it lost).
    fn share_collect(&self) -> Result<Collected, String> {
        let mut saved = self.doc().clone();
        saved.prune_unreferenced_assets();
        let text = atomik_model::save_project_checked(&saved)
            .map_err(|e| format!("The project cannot be shared: {e}"))?;
        let roots = self.collect_sources();
        let mut audio = Vec::new();
        let mut missing = 0;
        for hash in saved.assets.keys() {
            let found = is_valid_hash(hash)
                .then(|| {
                    roots
                        .iter()
                        .map(|r| r.join("Audio").join(format!("{hash}.wav")))
                        .find(|p| p.is_file())
                })
                .flatten();
            match found {
                Some(path) => audio.push((hash.clone(), path)),
                None => missing += 1,
            }
        }
        Ok(Collected {
            text,
            audio,
            missing,
        })
    }

    /// About how many bytes the preview and the stems of a share would have.
    fn share_render_bytes(&self, values: &ShareValues, preview: SharePreview) -> (u64, u64) {
        let doc = self.doc();
        let Ok((from, to)) = span_frames(doc, ExportSpan::Song) else {
            return (0, 0);
        };
        let rate = u64::from(doc.sample_rate.max(1));
        let frames = to - from + (DEFAULT_TAIL_SECONDS * rate as f64) as u64;
        let seconds = frames / rate + 1;
        let preview_bytes = if !values.preview {
            0
        } else if preview == SharePreview::Mp3 {
            seconds * u64::from(PREVIEW_KBPS) * 125
        } else {
            frames * 2 * 2
        };
        let console = &doc.console;
        let strips = console.channels.len()
            + console.buses.len()
            + console.returns.iter().flatten().count()
            + usize::from(console.master.is_some());
        let stem_bytes = if values.stems {
            strips as u64 * frames * 2 * 3
        } else {
            0
        };
        (preview_bytes, stem_bytes)
    }

    /// What a share with these choices would weigh, for the panel. It is an estimate: the audio and
    /// the project are counted exactly, the preview and the stems from the length of the song.
    pub fn share_size(&self, values: &ShareValues) -> ShareSize {
        let kind = self.share_preview_kind();
        let (project, audio) = self.share_collect().map_or((0, 0), |c| {
            let audio: u64 = c
                .audio
                .iter()
                .map(|(_, p)| std::fs::metadata(p).map_or(0, |m| m.len()))
                .sum();
            (c.text.len() as u64, audio)
        });
        let (preview, stems) = self.share_render_bytes(values, kind);
        ShareSize {
            bytes: project + audio + preview + stems + 1024,
            preview: if values.preview {
                kind
            } else {
                SharePreview::Off
            },
        }
    }

    /// The plugins the song uses, once each, in the order of the rack.
    pub fn plugin_credits(&self) -> Vec<PluginCredit> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (_, device) in self.doc().plugin_devices() {
            let Some(plugin) = &device.plugin else {
                continue;
            };
            if !seen.insert((plugin.format.name(), plugin.id.clone())) {
                continue;
            }
            out.push(PluginCredit {
                name: plugin.layout.name.clone(),
                vendor: plugin.layout.vendor.clone(),
                format: match plugin.format {
                    atomik_device_sdk::PluginFormat::Clap => "CLAP",
                    atomik_device_sdk::PluginFormat::Vst3 => "VST3",
                },
            });
        }
        out
    }

    // ------------------------------------------------------------------ commands

    /// Starts sharing the song as the archive `dest` with the panel's `values` (see the module
    /// docs).
    ///
    /// # Errors
    /// An archive job or an export runs already, a preview or the stems are asked for that the
    /// song cannot give, the project cannot be written, or the worker cannot start; the message
    /// is also shown as the toast. Nothing is changed then.
    pub fn start_share(&mut self, dest: PathBuf, values: &ShareValues) -> Result<(), String> {
        self.start_share_on(dest, values, &today())
    }

    /// [`Self::start_share`] with the date of the credits given.
    pub(super) fn start_share_on(
        &mut self,
        dest: PathBuf,
        values: &ShareValues,
        date: &str,
    ) -> Result<(), String> {
        if self.archive_busy() {
            return self.archive_refused("A project archive is already being made".to_owned());
        }
        if values.preview
            && let Some(why) = self.export_audio_refusal(ExportScope::Song)
        {
            return self.archive_refused(format!("Cannot share with a preview: {why}"));
        }
        if values.stems
            && let Some(why) = self.export_audio_refusal(ExportScope::Stems)
        {
            return self.archive_refused(format!("Cannot share with stems: {why}"));
        }
        // The panel's texts become the song information first (one undo step).
        let mut info = self.song_info().clone();
        info.title.clone_from(&values.title);
        info.author.clone_from(&values.author);
        info.notes.clone_from(&values.note);
        if let Err(e) = self.set_song_info(info) {
            return Err(e.to_string());
        }
        self.plugins_capture_all();
        let collected = match self.share_collect() {
            Ok(c) => c,
            Err(why) => return self.archive_refused(why),
        };
        let plugins = values.plugins.then(|| self.plugin_credits());
        let credits = share_credits(
            self.song_info(),
            date,
            crate::about_box::VERSION,
            plugins.as_deref(),
        );
        let size = self.share_size(values);
        let kind = self.share_preview_kind();
        let temp = share_temp_folder();
        let preview = values.preview.then(|| {
            let v = preview_values(kind);
            let path = super::export::export_file(ExportScope::Song, &temp.join("preview"), &v);
            (self.export_work(ExportScope::Song, path.clone(), &v), path)
        });
        let stems = values
            .stems
            .then(|| self.export_work(ExportScope::Stems, temp.join("stems"), &stems_values()));
        let renders = u64::from(preview.is_some()) + u64::from(stems.is_some());
        let paths: Vec<PathBuf> = collected.audio.iter().map(|(_, p)| p.clone()).collect();
        let (preview_bytes, stem_bytes) = self.share_render_bytes(values, kind);
        let guessed = ArchiveExtras {
            credits: Some(credits.clone()),
            ..ArchiveExtras::default()
        };
        let total = renders * RENDER_UNITS
            + archive_work_with(collected.text.len() as u64, &paths, &guessed)
            + 2 * (preview_bytes + stem_bytes);
        let file = display_name(&dest);
        let job = Job::Share(Box::new(ShareJob {
            dest,
            name: self.archive_name(),
            project: collected.text.into_bytes(),
            audio: collected.audio,
            credits,
            temp,
            preview,
            stems,
        }));
        self.spawn_archive(
            job,
            super::archive::What::Share,
            file.clone(),
            collected.missing,
            total,
        )?;
        if size.is_big() {
            self.reject(format!("{}: sharing {file}...", size.text()));
        }
        Ok(())
    }

    /// Starts opening the shared archive `archive`: it is unpacked into a new folder of `parent`
    /// named like the file, and the project opens when that is done.
    ///
    /// # Errors
    /// As [`Self::start_archive_import_in`].
    pub fn start_share_open(&mut self, archive: PathBuf, parent: &Path) -> Result<(), String> {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let why = format!("Cannot make the folder {}: {e}", parent.display());
            return self.archive_refused(why);
        }
        self.start_archive_import_in(archive, parent)
    }
}

#[cfg(test)]
#[path = "app_core_share_tests.rs"]
mod tests;
