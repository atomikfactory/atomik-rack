//! The plugin manager's side of the core: what the manager panel shows of the plugins, what its
//! buttons do, and the quarantine counter.
//!
//! The user's decisions (stars, tags, disabled plugins, the quarantine) live in
//! [`crate::plugins_user::PluginsUser`], keyed by the plugin's stable id, and are saved by the
//! shell to `plugins_user.json` when [`AppCore::take_plugins_user_change`] says they changed. They
//! never touch the catalog or the registry: a plugin the user disabled (or that was quarantined)
//! is hidden from the browser and the menus, and a project that uses it still loads it.
//!
//! A plugin that crashes (its process ends), or fails to load, three times in a row is
//! quarantined and a message says so once. An instance that ends without a fault, or a load that
//! works after a failed one, starts the count again; TRY AGAIN clears the quarantine.

use super::*;
use crate::plugins_user::QUARANTINE_AFTER;
use atomik_device_sdk::PluginKindHint;
use atomik_ui::{PluginManagerAction, PluginManagerData, PluginManagerRow};

/// What the plugin says it is, as the manager's category column writes it.
fn category_of(kind: PluginKindHint) -> &'static str {
    match kind {
        PluginKindHint::Instrument => "Instrument",
        PluginKindHint::Effect => "Effect",
        PluginKindHint::NoteEffect => "Note effect",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl<S: EngineSink> AppCore<S> {
    /// The user's stars, tags, disabled plugins and quarantine.
    pub fn plugins_user(&self) -> &PluginsUser {
        &self.plugins.user
    }

    /// Sets what the user decided, as the shell read it from `plugins_user.json` at start.
    pub fn set_plugins_user(&mut self, user: PluginsUser) {
        self.plugins.user = user;
        self.plugins.user_changed = false;
        self.plugins.catalog_dirty = true;
    }

    /// The decisions after a change, once: the shell saves them.
    pub fn take_plugins_user_change(&mut self) -> Option<PluginsUser> {
        std::mem::take(&mut self.plugins.user_changed).then(|| self.plugins.user.clone())
    }

    fn user_changed(&mut self) {
        self.plugins.user_changed = true;
        self.plugins.catalog_dirty = true;
    }

    /// A plugin crashed or failed to load. The third failure in a row quarantines it, and says so
    /// once.
    pub(crate) fn plugin_failed_in_a_row(&mut self, type_id: &str) {
        let newly = self.plugins.user.record_failure(type_id);
        self.user_changed();
        if newly {
            let name = PluginRef::from_type_id(type_id)
                .and_then(|p| {
                    self.plugins
                        .host
                        .catalog()
                        .entry(&p)
                        .map(|e| e.name.clone())
                })
                .unwrap_or_else(|| type_id.to_owned());
            self.reject(format!(
                "{name} was quarantined after {QUARANTINE_AFTER} failures in a row; \
                 the Plugin Manager can try it again"
            ));
        }
    }

    /// A plugin worked: the count of failures in a row starts again.
    pub(crate) fn plugin_worked(&mut self, type_id: &str) {
        if self.plugins.user.record_success(type_id) {
            self.user_changed();
        }
    }

    /// What the plugin manager shows now: every plugin the scan found with its status (a usable
    /// plugin the user disabled or that is quarantined says so), and the files that failed.
    pub fn plugin_manager_data(&self) -> PluginManagerData {
        let catalog = self.plugins.host.catalog();
        let user = &self.plugins.user;
        let stamp = |secs: u64| {
            if secs == 0 {
                String::new()
            } else {
                crate::logging::minute_stamp(secs)
            }
        };
        let mut rows: Vec<PluginManagerRow> = catalog
            .entries()
            .iter()
            .map(|e| {
                let id = e.plugin.type_id();
                let status = match entry_status(&e.status) {
                    PluginEntryStatus::Ok if user.is_quarantined(&id) => {
                        PluginEntryStatus::Quarantined
                    }
                    PluginEntryStatus::Ok if user.is_disabled(&id) => PluginEntryStatus::Disabled,
                    PluginEntryStatus::Ok => match self.plugins.settings.crashed.get(&id) {
                        Some(reason) => PluginEntryStatus::Crashed(reason.clone()),
                        None => PluginEntryStatus::Ok,
                    },
                    status => status,
                };
                let scanned_secs = self
                    .plugins
                    .cache
                    .file(&e.path)
                    .map_or(0, |f| f.scanned_secs);
                PluginManagerRow {
                    favourite: user.is_favourite(&id),
                    tags: user.tags_of(&id).to_vec(),
                    type_id: Some(id),
                    name: e.name.clone(),
                    vendor: e.vendor.clone(),
                    format: e.plugin.format,
                    category: category_of(e.kind).to_owned(),
                    status,
                    path: e.path.to_string_lossy().into_owned(),
                    scanned_secs,
                    scanned: stamp(scanned_secs),
                }
            })
            .collect();
        rows.extend(catalog.failed().iter().map(|f| PluginManagerRow {
            type_id: None,
            name: file_name(&f.path),
            vendor: String::new(),
            format: f.format,
            category: String::new(),
            status: PluginEntryStatus::Failed(f.reason.clone()),
            path: f.path.to_string_lossy().into_owned(),
            scanned_secs: f.date_secs,
            scanned: stamp(f.date_secs),
            favourite: false,
            tags: Vec::new(),
        }));
        PluginManagerData {
            rows,
            scan: self.plugins.job.as_ref().map(|j| {
                let (done, total) = j.progress();
                ScanProgressInfo { done, total }
            }),
            scanner_available: self.plugins.runner.is_some(),
            plugins_on: self.plugins.settings.enabled,
        }
    }

    /// Does what a button of the plugin manager asked. Closing the panel and showing a file in
    /// its folder are the shell's.
    pub fn plugin_manager_action(&mut self, action: &PluginManagerAction) {
        match action {
            PluginManagerAction::Close | PluginManagerAction::ShowInFolder(_) => {}
            PluginManagerAction::Favourite { type_id, on } => {
                if self.plugins.user.set_favourite(type_id, *on) {
                    self.user_changed();
                }
            }
            PluginManagerAction::Disable { type_ids, disabled } => {
                let mut settings = false;
                for id in type_ids {
                    if self.plugins.user.set_disabled(id, *disabled) {
                        self.user_changed();
                    }
                    // ENABLE also lifts the older switch of the Plugins settings.
                    if !*disabled && self.plugins.settings.disabled.remove(id) {
                        settings = true;
                    }
                }
                if settings {
                    self.settings_changed();
                }
            }
            PluginManagerAction::TryAgain(type_ids) => {
                for id in type_ids {
                    let cleared = self.plugins.user.try_again(id);
                    if self.plugins.settings.clear_crashed(id) {
                        self.plugins.settings_changed = true;
                    }
                    self.plugins.load_failed.remove(id);
                    if cleared {
                        self.user_changed();
                    }
                }
                self.plugins.catalog_dirty = true;
            }
            PluginManagerAction::AddTag { type_ids, tag } => {
                for id in type_ids {
                    match self.plugins.user.add_tag(id, tag) {
                        Ok(true) => self.user_changed(),
                        Ok(false) => {}
                        Err(e) => {
                            self.reject(e.to_string());
                            return;
                        }
                    }
                }
            }
            PluginManagerAction::RemoveTag { type_id, tag } => {
                if self.plugins.user.remove_tag(type_id, tag) {
                    self.user_changed();
                }
            }
            PluginManagerAction::RescanFiles(paths) => self.rescan_plugin_files(paths),
            PluginManagerAction::RescanAll => self.start_scan(RescanMode::Full),
            PluginManagerAction::CancelScan => {
                if let Some(job) = &self.plugins.job {
                    job.cancel();
                }
            }
        }
    }

    /// Scans these plugin files again on the scan worker (with progress and cancel like any
    /// scan): what is known about them is forgotten, so the incremental scan visits them. A scan
    /// that is refused (plugins are off, no scanner) forgets nothing.
    pub fn rescan_plugin_files(&mut self, paths: &[String]) {
        let can_run = self.plugins.job.is_none()
            && self.plugins.settings.enabled
            && self.plugins.runner.is_some();
        if can_run {
            for path in paths {
                self.plugins.cache.forget(Path::new(path));
            }
        }
        self.start_scan(RescanMode::Incremental);
    }
}
