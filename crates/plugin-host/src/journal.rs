//! The crash journal and the recovery snapshot helpers.
//!
//! `plugins-in-use.json` lists the plugins that are loaded in the app. It exists only while at
//! least one plugin instance exists and is removed on a clean exit, so finding it at the next
//! start means the app ended while those plugins were loaded. The recovery helpers name,
//! rotate and find the project snapshots the app writes while plugins are loaded.

use crate::cache::{FileStamp, PluginCache};
use atomik_device_sdk::{PluginFormat, PluginRef};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One plugin loaded in the app: what the journal records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InUsePlugin {
    /// Format and id.
    pub plugin: PluginRef,
    /// The file it was loaded from.
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct JournalEntry {
    format: String,
    id: String,
    path: String,
}

#[derive(Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    plugins: Vec<JournalEntry>,
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The journal of plugins in use.
#[derive(Debug)]
pub struct CrashJournal {
    path: PathBuf,
    /// Distinct plugins in first-load order, with how many instances use each.
    loaded: Vec<(InUsePlugin, usize)>,
}

impl CrashJournal {
    /// A journal that writes to `path` (nothing is written until a plugin is loaded).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            loaded: Vec::new(),
        }
    }

    fn sync(&self) -> std::io::Result<()> {
        if self.loaded.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let file = JournalFile {
            version: 1,
            plugins: self
                .loaded
                .iter()
                .map(|(p, _)| JournalEntry {
                    format: p.plugin.format.name().to_string(),
                    id: p.plugin.id.clone(),
                    path: p.path.to_string_lossy().into_owned(),
                })
                .collect(),
        };
        write_atomic(
            &self.path,
            &serde_json::to_string_pretty(&file).unwrap_or_default(),
        )
    }

    /// A plugin instance was created: record it (the file is written at the first one).
    ///
    /// # Errors
    /// The I/O error of writing the journal.
    pub fn loaded(&mut self, plugin: InUsePlugin) -> std::io::Result<()> {
        match self.loaded.iter_mut().find(|(p, _)| *p == plugin) {
            Some((_, n)) => *n += 1,
            None => self.loaded.push((plugin, 1)),
        }
        self.sync()
    }

    /// A plugin instance was destroyed. The file goes away with the last one.
    ///
    /// # Errors
    /// The I/O error of writing or removing the journal.
    pub fn unloaded(&mut self, plugin: &InUsePlugin) -> std::io::Result<()> {
        if let Some(i) = self.loaded.iter().position(|(p, _)| p == plugin) {
            self.loaded[i].1 -= 1;
            if self.loaded[i].1 == 0 {
                self.loaded.remove(i);
            }
        }
        self.sync()
    }

    /// The app is exiting normally: removes the journal.
    ///
    /// # Errors
    /// The I/O error of removing the file.
    pub fn clean_exit(&mut self) -> std::io::Result<()> {
        self.loaded.clear();
        self.sync()
    }

    /// At start: the plugins a previous run left in the journal (it ended while they were
    /// loaded), and the journal is removed so the report happens once. `None` when there is no
    /// journal or it is unreadable.
    pub fn take_previous(path: &Path) -> Option<Vec<InUsePlugin>> {
        let text = std::fs::read_to_string(path).ok()?;
        let _ = std::fs::remove_file(path);
        let file: JournalFile = serde_json::from_str(&text).ok()?;
        let plugins: Vec<InUsePlugin> = file
            .plugins
            .into_iter()
            .filter_map(|e| {
                let format = match e.format.as_str() {
                    "clap" => PluginFormat::Clap,
                    "vst3" => PluginFormat::Vst3,
                    _ => return None,
                };
                Some(InUsePlugin {
                    plugin: PluginRef { format, id: e.id },
                    path: PathBuf::from(e.path),
                })
            })
            .collect();
        (!plugins.is_empty()).then_some(plugins)
    }
}

/// The "DISABLE" answer of the unclean-exit dialog: puts the plugins' files on the blacklist
/// with a reason (they are retried when the file changes or the user presses RESCAN FAILED).
pub fn blacklist_in_use(cache: &mut PluginCache, plugins: &[InUsePlugin]) {
    for p in plugins {
        let stamp = FileStamp::of(&p.path).unwrap_or(FileStamp {
            size: 0,
            mtime_ns: 0,
        });
        cache.blacklist_file(
            &p.path,
            p.plugin.format,
            stamp,
            format!(
                "Atomik ended unexpectedly while `{}` was loaded",
                p.plugin.id
            ),
        );
    }
}

/// The folder of recovery snapshots (`<config>/recovery`): naming, rotation, lookup. Snapshots
/// are named `<project>-<sequence>.recover.atomik`; the sequence (the caller's clock, for
/// example Unix seconds) orders them.
#[derive(Debug, Clone)]
pub struct RecoveryDir {
    dir: PathBuf,
}

const RECOVERY_SUFFIX: &str = ".recover.atomik";

impl RecoveryDir {
    /// The folder `dir` (created by the first snapshot's writer).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// The file name for snapshot `sequence` of project `name`; never the user's own file.
    pub fn next_path(&self, name: &str, sequence: u64) -> PathBuf {
        let stem: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stem = if stem.is_empty() { "project" } else { &stem };
        self.dir
            .join(format!("{stem}-{sequence:020}{RECOVERY_SUFFIX}"))
    }

    fn snapshots(&self) -> Vec<(u64, PathBuf)> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(u64, PathBuf)> = read
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?;
                let base = name.strip_suffix(RECOVERY_SUFFIX)?;
                let seq = base.rsplit_once('-')?.1.parse::<u64>().ok()?;
                Some((seq, p))
            })
            .collect();
        out.sort();
        out
    }

    /// The newest snapshot, if any.
    pub fn newest(&self) -> Option<PathBuf> {
        self.snapshots().pop().map(|(_, p)| p)
    }

    /// Deletes all but the newest `keep` snapshots. Other files in the folder are left alone.
    ///
    /// # Errors
    /// The I/O error of deleting a file.
    pub fn prune(&self, keep: usize) -> std::io::Result<()> {
        let all = self.snapshots();
        let drop = all.len().saturating_sub(keep);
        for (_, p) in all.into_iter().take(drop) {
            std::fs::remove_file(p)?;
        }
        Ok(())
    }
}
