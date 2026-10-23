//! Relinking missing audio in the app core (see the module docs of `app_core_audio.rs`).

use super::{AppCore, Loaded};
use crate::audio_import::{display_name, spawn_scan};
use crate::audio_relink::Wanted;
use crate::sink::EngineSink;
use atomik_model::{AssetMeta, Command};
use atomik_ui::ImportTarget;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a relink batch is doing: which missing asset each file of the batch stands for.
pub(super) struct RelinkJob {
    /// The asset each file replaces, in the order of the batch's paths.
    from: Vec<String>,
    /// Files applied so far.
    relinked: usize,
    /// A note for the summary (a truncated scan).
    note: Option<String>,
}

impl<S: EngineSink> AppCore<S> {
    /// Relinks the missing asset `from` to the audio file `path`: the file is imported on a
    /// worker and then replaces the asset (one undo step), or, when it is the very same audio,
    /// restores the bundle's file without touching the document. See the module docs of
    /// `app_core_audio.rs`. Reports an error (and does nothing) when `from` is not missing.
    pub fn relink_asset(&mut self, from: &str, path: PathBuf) {
        if !self.audio.missing.iter().any(|m| m.hash == from) {
            let name = self.asset_name(from);
            return self.reject(format!("audio {name} is not missing"));
        }
        self.start_relink(vec![(from.to_owned(), path)], None);
    }

    /// Searches `root` (recursively, bounded: see [`crate::audio_relink`]) for the files of the
    /// missing assets, matching them by original file name in any case, on a worker. The matches
    /// are then relinked like [`Self::relink_asset`]; the ones without a file stay missing. The
    /// summary (`relinked N, M still missing`) is reported when the last file is done.
    pub fn relink_search_folder(&mut self, root: PathBuf) {
        let wanted: Vec<Wanted> = self
            .audio
            .missing
            .iter()
            .filter_map(|m| {
                let meta = self.doc().assets.get(&m.hash)?;
                Some(Wanted {
                    hash: m.hash.clone(),
                    name: m.name.clone(),
                    channels: meta.channels,
                    sample_rate: meta.sample_rate,
                    frames: meta.frames,
                })
            })
            .collect();
        if wanted.is_empty() {
            return self.reject("no missing audio files to relink".to_owned());
        }
        match spawn_scan(self.audio.tx.clone(), self.audio.generation, root, wanted) {
            Ok(()) => self.audio.pending += 1,
            Err(e) => self.reject(format!("audio relink: cannot start the search: {e}")),
        }
    }

    fn start_relink(&mut self, jobs: Vec<(String, PathBuf)>, note: Option<String>) {
        let (from, paths): (Vec<String>, Vec<PathBuf>) = jobs.into_iter().unzip();
        let job = RelinkJob {
            from,
            relinked: 0,
            note,
        };
        self.start_batch(paths, ImportTarget::NewTracks, 0, None, Some(job));
    }

    /// A folder scan finished: relinks what it found.
    pub(super) fn apply_scanned(
        &mut self,
        root: &Path,
        found: Vec<(String, PathBuf)>,
        truncated: bool,
    ) {
        // Assets relinked meanwhile (or by an undo) are no longer wanted.
        let found: Vec<(String, PathBuf)> = found
            .into_iter()
            .filter(|(hash, _)| self.audio.missing.iter().any(|m| &m.hash == hash))
            .collect();
        let note = truncated.then(|| {
            format!(
                "the search stopped after {} entries",
                crate::audio_relink::MAX_ENTRIES
            )
        });
        if found.is_empty() {
            let left = self.audio.missing.len();
            let suffix = note.map(|n| format!(" ({n})")).unwrap_or_default();
            self.reject(format!(
                "no matching audio files in {}: {left} still missing{suffix}",
                display_name(root)
            ));
            return;
        }
        self.start_relink(found, note);
    }

    /// Applies one imported file of a relink batch (the asset is already in the caches).
    pub(super) fn relink_loaded(&mut self, batch: u64, loaded: &Loaded) -> Result<(), String> {
        let info = loaded
            .info
            .clone()
            .ok_or_else(|| "not an imported asset".to_owned())?;
        let from = self
            .audio
            .batches
            .get(&batch)
            .and_then(|b| Some(b.relink.as_ref()?.from.get(b.done)?.clone()))
            .ok_or_else(|| "relink cancelled".to_owned())?;
        if info.hash == from {
            // The same audio: the bundle has its file back (the caches know it and the missing
            // list dropped it), so the clips and samplers only need to learn about it.
            self.submit_song(false);
            self.send_samples_using(&BTreeSet::from([from]));
        } else {
            self.apply_command(Command::RegisterAsset {
                hash: info.hash.clone(),
                meta: AssetMeta {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                    frames: info.frames,
                    original_name: info.original_name.clone(),
                },
            })
            .map_err(|e| e.to_string())?;
            self.apply_command(Command::RelinkAsset {
                from: from.clone(),
                to: info.hash,
            })
            .map_err(|e| e.to_string())?;
            // Missing no more, but an undo brings it back (see `refresh_missing`).
            if let Some(at) = self.audio.missing.iter().position(|m| m.hash == from) {
                let gone = self.audio.missing.remove(at);
                self.audio.relinked.push(gone);
            }
            self.forget_asset(&from);
        }
        if let Some(job) = self
            .audio
            .batches
            .get_mut(&batch)
            .and_then(|b| b.relink.as_mut())
        {
            job.relinked += 1;
        }
        self.after_view_change();
        Ok(())
    }

    /// The toast of a finished relink batch.
    pub(super) fn relink_summary(&self, job: &RelinkJob) -> String {
        let suffix = job
            .note
            .as_ref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        format!(
            "relinked {}, {} still missing{suffix}",
            job.relinked,
            self.audio.missing.len()
        )
    }

    /// Brings the missing list in line with the document after an undo or redo: an asset a
    /// relink replaced is missing again once the document uses it again, and one it no longer
    /// uses is not missing.
    pub(in crate::app_core) fn refresh_missing(&mut self) {
        if self.audio.missing.is_empty() && self.audio.relinked.is_empty() {
            return;
        }
        let used = self.doc().referenced_assets();
        let mut all = std::mem::take(&mut self.audio.missing);
        all.append(&mut self.audio.relinked);
        for m in all {
            if used.contains(&m.hash) {
                self.audio.missing.push(m);
            } else {
                self.audio.relinked.push(m);
            }
        }
        self.audio.missing.sort_by(|a, b| a.hash.cmp(&b.hash));
        self.audio.relinked.sort_by(|a, b| a.hash.cmp(&b.hash));
    }
}

#[cfg(test)]
#[path = "app_core_relink_tests.rs"]
mod tests;
