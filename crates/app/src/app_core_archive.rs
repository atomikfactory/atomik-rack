//! Project archives in the app core: File > Export Project Archive and Import Project Archive.
//!
//! An archive is one `.atomikz` file holding the project and the audio it uses (the format and its
//! checks are the `atomik-audio-file` crate's). Both directions run on a worker thread
//! ([`crate::archive_worker`]) that streams through a fixed buffer, so a project of many gigabytes
//! neither stalls the window nor fills the memory; one archive job runs at a time.
//!
//! * **Export** packs the song as File > Save would write it (the plugins' state captured, the
//!   unused audio left out) with the audio it uses, taken from the project folder or the session
//!   bundle. Audio the song lost is not in the archive, and the message counts it. The song, its
//!   file name and its unsaved state do not change.
//! * **Import** unpacks the archive into a new folder next to it, named like the archive (`Name 2`
//!   when that exists; nothing that exists is touched) and then asks the shell to open the
//!   project, which first asks about unsaved changes like any Open. The unpacked folder is a
//!   project folder, so it relinks and moves like one.
//! * **Share** (`app_core_share.rs`) is an export that also holds a preview, the stems and the
//!   credits; opening one is an import into a folder the user picked.
//! * **Cancel** ([`AppCore::cancel_archive`]) stops the worker, which removes its part file or its
//!   half-written folder; a failure does the same.

use super::*;
use crate::archive_worker::{self, Done, Job};
use crate::audio_import::display_name;
use atomik_audio_file::{archive_work, is_safe_component};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The archive job that runs.
pub(super) struct Running {
    job: u64,
    what: What,
    /// The archive's file name, for the messages.
    file: String,
    /// Audio files the song lost that the archive does not hold (an export).
    missing: usize,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum What {
    Export,
    /// A share: an export with a preview, stems and credits.
    Share,
    Import,
}

/// The archive state of the core: it outlives the open project (a job goes on when the song is
/// replaced).
#[derive(Default)]
pub(super) struct ArchiveState {
    running: Option<Running>,
    next_job: u64,
    /// The project of an unpacked share and the text of its credits, until the shell has opened
    /// the project and shown them ([`AppCore::take_share_credits`]).
    shared: Option<(PathBuf, String)>,
}

impl ArchiveState {
    /// Stops the running job (the core goes away).
    pub(super) fn stop(&self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The credits of an unpacked share: `credits.txt` in its folder, when there is one.
fn read_credits(root: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(root.join("credits.txt")).ok()?;
    let mut text = String::new();
    file.take(atomik_audio_file::MAX_CREDITS_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// `1.5 GB` or `640 KB`: the size of an archive for a message.
pub(super) fn size_text(bytes: u64) -> String {
    const UNITS: [(&str, f64); 3] = [("GB", 1e9), ("MB", 1e6), ("KB", 1e3)];
    for (unit, size) in UNITS {
        if bytes as f64 >= size {
            return format!("{:.1} {unit}", bytes as f64 / size);
        }
    }
    format!("{bytes} bytes")
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// True while an archive is being written or unpacked.
    pub fn archive_busy(&self) -> bool {
        self.archive.running.is_some()
    }

    /// The fraction done, `0.0..=1.0`, and a line saying what runs.
    pub fn archive_progress(&self) -> Option<(f32, String)> {
        let r = self.archive.running.as_ref()?;
        let total = r.total.load(Ordering::Relaxed);
        let fraction = if total == 0 {
            0.0
        } else {
            (r.progress.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32
        };
        let verb = match r.what {
            What::Export => "Writing",
            What::Share => "Sharing",
            What::Import => "Unpacking",
        };
        Some((fraction, format!("{verb} {}", r.file)))
    }

    // ------------------------------------------------------------------ commands

    /// The name an archive of the song goes by: its title, else its file name, else `project`,
    /// as a usable folder name.
    pub fn archive_name(&self) -> String {
        let name = self.template_name();
        if is_safe_component(&name) {
            name
        } else {
            "project".to_owned()
        }
    }

    /// Starts writing the archive `dest` of the song (see the module docs).
    ///
    /// # Errors
    /// An archive job runs already, the project cannot be written, or the worker cannot start;
    /// the message is also shown as the toast.
    pub fn start_archive_export(&mut self, dest: PathBuf) -> Result<(), String> {
        if self.archive.running.is_some() {
            return self.archive_refused("A project archive is already being made".to_owned());
        }
        self.plugins_capture_all();
        let mut saved = self.doc().clone();
        saved.prune_unreferenced_assets();
        let text = match atomik_model::save_project_checked(&saved) {
            Ok(t) => t,
            Err(e) => return self.archive_refused(format!("The project cannot be archived: {e}")),
        };
        let bundle = self.audio.bundle_ref();
        let mut audio = Vec::new();
        let mut missing = 0;
        for hash in saved.assets.keys() {
            match bundle.filter(|b| b.exists(hash)) {
                Some(b) => audio.push((hash.clone(), b.asset_path(hash))),
                None => missing += 1,
            }
        }
        let paths: Vec<PathBuf> = audio.iter().map(|(_, p)| p.clone()).collect();
        let total = archive_work(text.len() as u64, &paths);
        let file = display_name(&dest);
        let job = Job::Pack {
            dest,
            name: self.archive_name(),
            project: text.into_bytes(),
            audio,
        };
        self.spawn_archive(job, What::Export, file, missing, total)
    }

    /// Starts unpacking the archive `archive` into a new folder beside it (see the module docs);
    /// the project opens when it is done.
    ///
    /// # Errors
    /// An archive job runs already or the worker cannot start; the message is also shown as the
    /// toast.
    pub fn start_archive_import(&mut self, archive: PathBuf) -> Result<(), String> {
        let parent = archive
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        self.start_archive_import_in(archive, &parent)
    }

    /// Starts unpacking the archive `archive` into a new folder of `parent`, named like the
    /// archive (`Name 2` when that exists); the project opens when it is done.
    ///
    /// # Errors
    /// As [`Self::start_archive_import`].
    pub fn start_archive_import_in(
        &mut self,
        archive: PathBuf,
        parent: &Path,
    ) -> Result<(), String> {
        if self.archive.running.is_some() {
            return self.archive_refused("A project archive is already being made".to_owned());
        }
        if !archive.is_file() {
            return self.archive_refused(format!("{} is not a file", archive.display()));
        }
        let parent = parent.to_path_buf();
        let stem = super::templates::template_stem(
            &archive
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        );
        let dest = (1u32..)
            .map(|n| {
                parent.join(if n == 1 {
                    stem.clone()
                } else {
                    format!("{stem} {n}")
                })
            })
            .find(|p| {
                let mut part = p.clone().into_os_string();
                part.push(".part");
                !p.exists() && !Path::new(&part).exists()
            })
            .unwrap_or_else(|| parent.join(&stem));
        let file = display_name(&archive);
        self.spawn_archive(Job::Unpack { archive, dest }, What::Import, file, 0, 0)
    }

    /// The credits of the share that was unpacked to the project `project`, once: the shell shows
    /// them after it opened the project. `None` for any other project.
    pub fn take_share_credits(&mut self, project: &Path) -> Option<String> {
        self.archive
            .shared
            .take_if(|(path, _)| path == project)
            .map(|(_, credits)| credits)
    }

    /// Stops the running archive job; it leaves no part file or half-written folder.
    pub fn cancel_archive(&mut self) {
        match &self.archive.running {
            Some(r) => r.cancel.store(true, Ordering::Relaxed),
            None => self.reject("No project archive is being made".to_owned()),
        }
    }

    pub(super) fn archive_refused(&mut self, why: String) -> Result<(), String> {
        self.reject(why.clone());
        Err(why)
    }

    pub(super) fn spawn_archive(
        &mut self,
        job: Job,
        what: What,
        file: String,
        missing: usize,
        total: u64,
    ) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(total));
        let id = self.archive.next_job;
        self.archive.next_job += 1;
        let started = archive_worker::spawn(
            self.audio.tx.clone(),
            id,
            job,
            (
                Arc::clone(&cancel),
                Arc::clone(&progress),
                Arc::clone(&total),
            ),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                let verb = match what {
                    What::Export => "Writing",
                    What::Share => "Sharing",
                    What::Import => "Unpacking",
                };
                self.archive.running = Some(Running {
                    job: id,
                    what,
                    file: file.clone(),
                    missing,
                    cancel,
                    progress,
                    total,
                });
                self.reject(format!("{verb} {file}..."));
                Ok(())
            }
            Err(e) => self.archive_refused(format!("Cannot start the archive worker: {e}")),
        }
    }

    /// An archive job finished (or failed, or was stopped): says so, and for an unpacked
    /// project asks the shell to open it.
    pub(super) fn apply_archived(
        &mut self,
        job: u64,
        result: Result<Done, atomik_audio_file::ArchiveError>,
    ) {
        let Some(running) = self.archive.running.take_if(|r| r.job == job) else {
            return;
        };
        let file = &running.file;
        match (result, running.what) {
            (Err(atomik_audio_file::ArchiveError::Cancelled), What::Export) => {
                self.reject(format!("Writing {file} cancelled"));
            }
            (Err(atomik_audio_file::ArchiveError::Cancelled), What::Share) => {
                self.reject(format!("Sharing {file} cancelled"));
            }
            (Err(atomik_audio_file::ArchiveError::Cancelled), What::Import) => {
                self.reject(format!("Unpacking {file} cancelled"));
            }
            (Err(e), What::Export) => self.reject(format!("Writing {file} failed: {e}")),
            (Err(e), What::Share) => self.reject(format!("Sharing {file} failed: {e}")),
            (Err(e), What::Import) => self.reject(format!("Unpacking {file} failed: {e}")),
            (Ok(Done::Packed { report }), _) => {
                let files = report.audio_files;
                let plural = if files == 1 { "" } else { "s" };
                let lost = match running.missing {
                    0 => String::new(),
                    n => format!(
                        "; {n} missing audio file{} not included",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                let shared = if running.what == What::Share {
                    "Shared as"
                } else {
                    "Wrote"
                };
                self.reject(format!(
                    "{shared} {file}: the project and {files} audio file{plural}, {}{lost}",
                    size_text(report.bytes)
                ));
            }
            (Ok(Done::Unpacked(done)), _) => {
                self.reject(format!("Unpacked {file} to {}", display_name(&done.root)));
                // A share has its credits beside the project: the shell shows them once the
                // project is open.
                if let Some(credits) = read_credits(&done.root) {
                    self.archive.shared = Some((done.project.clone(), credits));
                }
                self.request_open(done.project);
            }
        }
    }
}
