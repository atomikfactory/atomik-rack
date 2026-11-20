//! The master chain in the app core: the master section's insert effects and FX send effects
//! saved as a library patch and loaded back (see [`atomik_model::MasterChain`]).
//!
//! * **Save** ([`AppCore::save_master_chain`], the platform's save dialog has asked for the
//!   name and confirmed an overwrite) writes the item as a patch file, copies the audio its
//!   effects use beside it like a device patch, and, when the file is inside the library
//!   folder, puts it in the **Master Chains** category so the browser lists it under that name.
//!   Saving changes neither the document nor the undo history.
//! * **Load** ([`AppCore::load_master_chain`], and any load of such a patch item: a double click
//!   in the browser, a drop on the master or on the rack) replaces the master's insert and send
//!   effects by the item's with [`WorkflowCommand::ApplyMasterChain`]: one undo step, no question
//!   asked, Undo puts the old chain back. A plugin the machine lacks arrives as a stand-in; the
//!   item's audio is adopted like a loaded item's. A patch item that is not a master chain loads
//!   as it always did.

use super::*;
use atomik_audio_file::save_text_atomic;
use atomik_library::{FileKind, LibrarySection, PATCH_EXT, classify};
use atomik_model::{
    WorkflowCommand, capture_master_chain, empty_doc, is_master_chain_item, load_master_chain_item,
    register_stand_ins, save_master_chain_item,
};

/// The category the library gives the master chain patches.
pub(super) const MASTER_CHAINS_CATEGORY: &str = "Master Chains";

/// The name a master chain patch is suggested under.
const SUGGESTED_NAME: &str = "Master Chain";

impl<S: EngineSink> AppCore<S> {
    /// The folder and file name the save dialog starts with: the library's Patches folder and
    /// "Master Chain". `None` without a library folder.
    pub fn master_chain_save_suggestion(&self) -> Option<(PathBuf, String)> {
        let dir = self
            .library_root()?
            .join(LibrarySection::Patches.folder_name()?);
        Some((dir, format!("{SUGGESTED_NAME}.{PATCH_EXT}")))
    }

    /// The folder the open dialog starts in: the library's Patches folder.
    pub fn master_chain_folder(&self) -> Option<PathBuf> {
        self.master_chain_save_suggestion().map(|(dir, _)| dir)
    }

    /// Saves the master chain as the patch item `path` (see the module docs). A refusal or a
    /// write error goes to the status line.
    pub fn save_master_chain(&mut self, path: &Path) {
        if let Err(message) = self.save_master_chain_to(path) {
            self.report(message);
        }
    }

    fn save_master_chain_to(&mut self, path: &Path) -> Result<(), String> {
        let mut path = path.to_path_buf();
        if classify(&path, false) != Some(FileKind::Patch) {
            path.set_extension(PATCH_EXT);
        }
        let doc = self.document.doc();
        let chain = capture_master_chain(doc, &self.registry).map_err(|e| e.to_string())?;
        let text = save_master_chain_item(&chain, doc).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        save_text_atomic(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.categorise_in_library(&path, MASTER_CHAINS_CATEGORY);
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let hashes: Vec<String> = chain.snapshot.assets.keys().cloned().collect();
        if hashes.is_empty() {
            self.library_saved(&stem, LibrarySection::Patches, path, None);
        } else {
            let job = crate::browser_workers::CopyAssets {
                name: stem.clone(),
                section: LibrarySection::Patches,
                to_root: path.with_extension(""),
                path,
                from: self.bundle_clone(),
                hashes,
            };
            self.copy_item_assets(job);
        }
        Ok(())
    }

    /// Loads the master chain patch `path` onto the master section (see the module docs). A
    /// refusal or a read error goes to the status line.
    pub fn load_master_chain(&mut self, path: &Path) {
        let result = match self.read_master_chain_item(path) {
            Ok(Some(text)) => self.apply_master_chain_text(path, &text),
            Ok(None) => Err(format!(
                "'{}' is not a master chain patch",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            )),
            Err(message) => Err(message),
        };
        if let Err(message) = result
            && !message.is_empty()
        {
            self.report(message);
        }
    }

    /// The text of the patch item `path` when it is a master chain, `None` when it is another
    /// kind of item (or cannot be read: the ordinary load says why).
    pub(super) fn master_chain_text_of(&self, path: &Path) -> Option<String> {
        if classify(path, false) != Some(FileKind::Patch) {
            return None;
        }
        let text = atomik_model::read_document_text(path).ok()?;
        is_master_chain_item(&text).then_some(text)
    }

    fn read_master_chain_item(&self, path: &Path) -> Result<Option<String>, String> {
        let text = atomik_model::read_document_text(path).map_err(|e| {
            format!(
                "'{}' cannot be read: {e}",
                path.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            )
        })?;
        Ok(is_master_chain_item(&text).then_some(text))
    }

    /// Replaces the master's effects by the chain of the item `text` read from `path`.
    pub(super) fn apply_master_chain_text(
        &mut self,
        path: &Path,
        text: &str,
    ) -> Result<(), String> {
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let chain = load_master_chain_item(text, &mut self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        // The plugins of the item that are not installed get stand-ins built from its layouts.
        let mut holder = empty_doc();
        holder.devices.clone_from(&chain.snapshot.devices);
        register_stand_ins(&holder, &mut self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        let assets = chain.snapshot.assets.clone();
        let effects = chain.snapshot.devices.len();
        let sends = chain.send_count();
        self.apply_workflow(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(chain),
        })?;
        self.adopt_assets(&path.with_extension(""), &assets);
        self.after_view_change();
        self.report(format!(
            "Loaded master chain '{name}': {effects} effect{}, {sends} send{}",
            if effects == 1 { "" } else { "s" },
            if sends == 1 { "" } else { "s" },
        ));
        Ok(())
    }
}
