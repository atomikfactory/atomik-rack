//! The plugin catalog: what the scan cache says is installed, as the
//! browser, the settings list and the registry need it.
//!
//! The catalog is built from the cache and the user's settings alone: no plugin file is opened,
//! checked or loaded, so opening a project loads no plugin code. Registering it in the
//! [`Registry`] makes each usable plugin a device type with the descriptor built from its scanned
//! layout; the backend learns which file holds which plugin through a callback (for CLAP,
//! `ClapBackend::register_plugin_file`), still without loading it.

use crate::cache::PluginCache;
use crate::settings::PluginSettings;
use atomik_device_sdk::{
    PluginFormat, PluginKindHint, PluginLayout, PluginRef, Registry, descriptor_for,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Whether a plugin can be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// Usable: it is in the browser and can be added.
    Ok,
    /// Listed but cannot be hosted (the reason).
    Unsupported(String),
    /// The user disabled it: hidden from the browser, existing devices show MISSING.
    Disabled,
    /// Found but unusable (a duplicate id, a layout that makes no device); the reason.
    Failed(String),
}

/// One installed plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntry {
    /// Format and id; `plugin.type_id()` is the registry type id.
    pub plugin: PluginRef,
    /// Display name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Version text.
    pub version: String,
    /// Instrument, effect or note effect.
    pub kind: PluginKindHint,
    /// Feature words derived from the layout (`instrument`, `audio-effect`, `note-effect`,
    /// `sidechain`, `notes-in`, `notes-out`), for the 4d index to filter on.
    pub features: Vec<String>,
    /// The file that holds it.
    pub path: PathBuf,
    /// Usable or why not.
    pub status: PluginStatus,
    /// The param table was cut at 2 048.
    pub params_truncated: bool,
    /// The scanned layout (for descriptors).
    pub layout: PluginLayout,
}

/// A file that failed to scan (the blacklist, as the settings list shows it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedFile {
    /// The file.
    pub path: PathBuf,
    /// Its format.
    pub format: PluginFormat,
    /// Why.
    pub reason: String,
    /// When, seconds since the Unix epoch.
    pub date_secs: u64,
}

/// What registering the catalog did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterReport {
    /// Device types added to the registry.
    pub registered: usize,
    /// Plugins that could not be registered, with the reason (they are marked `Failed`).
    pub errors: Vec<(PluginRef, String)>,
}

/// The installed plugins, sorted by format, name (case-insensitive) and id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginCatalog {
    entries: Vec<PluginEntry>,
    failed: Vec<FailedFile>,
    registered: Vec<String>,
}

fn features(layout: &PluginLayout) -> Vec<String> {
    let mut f = vec![
        match layout.kind_hint {
            PluginKindHint::Instrument => "instrument",
            PluginKindHint::Effect => "audio-effect",
            PluginKindHint::NoteEffect => "note-effect",
        }
        .to_string(),
    ];
    if layout.sidechain {
        f.push("sidechain".to_string());
    }
    if layout.notes_in {
        f.push("notes-in".to_string());
    }
    if layout.notes_out {
        f.push("notes-out".to_string());
    }
    f
}

impl PluginCatalog {
    /// Builds the catalog from the cache. With plugins switched off it is empty.
    pub fn from_cache(cache: &PluginCache, settings: &PluginSettings) -> Self {
        if !settings.enabled {
            return Self::default();
        }
        let mut entries: Vec<PluginEntry> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for file in cache.files() {
            for p in &file.plugins {
                let type_id = p.plugin.type_id();
                let status = if !seen.insert(type_id.clone()) {
                    PluginStatus::Failed(format!(
                        "duplicate id: `{}` is already provided by another file",
                        p.plugin.id
                    ))
                } else if let Some(reason) = &p.unsupported {
                    PluginStatus::Unsupported(reason.clone())
                } else if settings.is_plugin_disabled(&type_id) {
                    PluginStatus::Disabled
                } else {
                    PluginStatus::Ok
                };
                entries.push(PluginEntry {
                    plugin: p.plugin.clone(),
                    name: p.layout.name.clone(),
                    vendor: p.layout.vendor.clone(),
                    version: p.layout.version.clone(),
                    kind: p.layout.kind_hint,
                    features: features(&p.layout),
                    path: file.path.clone(),
                    status,
                    params_truncated: p.params_truncated,
                    layout: p.layout.clone(),
                });
            }
        }
        entries.sort_by(|a, b| {
            (
                a.plugin.format,
                a.name.to_lowercase(),
                &a.plugin.id,
                &a.path,
            )
                .cmp(&(
                    b.plugin.format,
                    b.name.to_lowercase(),
                    &b.plugin.id,
                    &b.path,
                ))
        });
        let failed = cache
            .blacklist()
            .map(|b| FailedFile {
                path: b.path.clone(),
                format: b.format,
                reason: b.reason.clone(),
                date_secs: b.date_secs,
            })
            .collect();
        Self {
            entries,
            failed,
            registered: Vec::new(),
        }
    }

    /// Every plugin found, usable or not (id, name, vendor, features, format, status).
    pub fn entries(&self) -> &[PluginEntry] {
        &self.entries
    }

    /// The files that failed to scan.
    pub fn failed(&self) -> &[FailedFile] {
        &self.failed
    }

    /// One plugin by reference.
    pub fn entry(&self, plugin: &PluginRef) -> Option<&PluginEntry> {
        self.entries.iter().find(|e| &e.plugin == plugin)
    }

    /// Registers every usable plugin as a device type and tells `files` which file holds it.
    /// A plugin whose descriptor cannot be built, or whose type is already registered, is
    /// marked `Failed`, listed in the report and skipped; the others still register.
    pub fn register_into(
        &mut self,
        registry: &mut Registry,
        files: &mut dyn FnMut(&PluginRef, &Path),
    ) -> RegisterReport {
        let mut report = RegisterReport::default();
        for entry in &mut self.entries {
            if entry.status != PluginStatus::Ok {
                continue;
            }
            let type_id = entry.plugin.type_id();
            let result = descriptor_for(&entry.plugin, &entry.layout)
                .map_err(|e| e.to_string())
                .and_then(|d| {
                    registry
                        .register_plugin(type_id.clone(), d)
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(()) => {
                    files(&entry.plugin, &entry.path);
                    self.registered.push(type_id);
                    report.registered += 1;
                }
                Err(reason) => {
                    report.errors.push((entry.plugin.clone(), reason.clone()));
                    entry.status = PluginStatus::Failed(reason);
                }
            }
        }
        report
    }

    /// Removes from `registry` the types this catalog registered (before a rescan swaps the set).
    pub fn unregister_from(&mut self, registry: &mut Registry) {
        for type_id in self.registered.drain(..) {
            let _ = registry.unregister_plugin(&type_id);
        }
    }
}
