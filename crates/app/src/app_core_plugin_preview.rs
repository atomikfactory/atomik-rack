//! Previews of plugin presets: while a preset row of a plugin is the one selected row of the
//! file browser (the rows the patch browser lists for a plugin of the rack), the keyboard plays
//! that preset through a **preview instance** of the plugin, and the instance stays while the
//! selection moves from preset to preset of the same plugin.
//!
//! * The instance is a second instance of the plugin, made like any other (`beginuntil(&mut c, |_| stats.destroyed() == destroyed + 1);create`, so a
//!   sandboxed plugin loads in its own process while the app goes on, polled each frame) and
//!   never put in the rack or the document. A plugin that is slow to load shows `Loading ...` in
//!   the status line and the keyboard stays on the track until the instance is there; nothing
//!   waits for it. A plugin that loads in this process blocks as it does everywhere else.
//! * Its device plays in a scratch project of its own ([`atomik_render::live_preview`], the way
//!   `app_core_live_preview.rs` plays a patch): an instrument through its own channel, an effect
//!   in the insert chain of a built-in synth. The worker owns the device, so the plugin is
//!   driven off the UI thread and off the audio thread like a patch is, and it is dropped there
//!   when the preview ends; the manager's next poll destroys the instance.
//! * A preset is loaded into the preview instance with the steps the rack's own presets use
//!   (`begin_load_preset`, polled), one at a time: the latest row wins and a load in flight is
//!   finished first. Notes held when the preset changes are released.
//! * Selecting a row of another plugin, anything that is not a preset, closing the panel or
//!   anything the keyboard preview of a patch ends on (see `app_core_live_preview.rs`) ends it:
//!   the instance goes with the worker. The document, the selection of the rack, the plugin's
//!   own state and the undo history are never touched, and a bounce never hears it.

use super::live_follow_ops::Spawn;
use super::*;
use atomik_core::NoteKind;
use atomik_device_sdk::{PluginRef, RunMode};
use atomik_library::preset_of;
use atomik_plugin_host::{CreateTicket, InstanceId, InstantiateRequest, PluginHandle};
use atomik_render::live_preview::LiveDevices;

/// The key of the plugin in the scratch project.
pub(super) const PLUGIN_KEY: &str = "preview_plugin";

/// The preview instance of one plugin, from the request for it to the end of the preview.
pub(super) struct PluginSession {
    /// The row that is followed.
    pub(super) row: PathBuf,
    /// Which plugin.
    plugin: PluginRef,
    /// Its name, for messages.
    name: String,
    /// The scratch project, until the worker takes it.
    scene: Option<(atomik_model::RackDoc, atomik_device_sdk::Registry)>,
    /// The preset to hear.
    wanted: String,
    /// The preset that is loaded in the instance.
    loaded: Option<String>,
    /// The preset a load is under way for.
    loading: Option<String>,
    /// The creation that is under way.
    ticket: Option<CreateTicket>,
    /// The instance, once it exists.
    instance: Option<InstanceId>,
}

impl std::fmt::Debug for PluginSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginSession")
            .field("row", &self.row)
            .field("plugin", &self.plugin)
            .field("wanted", &self.wanted)
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The selection came to the preset row `row`: the same plugin as before only changes the
    /// preset to hear, another one starts a new preview instance.
    pub(super) fn begin_plugin_follow(&mut self, row: &Path) {
        self.live_follow.tried = Some(row.to_path_buf());
        let Some((device, preset)) = preset_of(row) else {
            return;
        };
        let key = DeviceKey(device.to_string());
        let Some(pd) = self
            .doc()
            .resolve(&key)
            .and_then(|d| d.plugin.as_deref().cloned())
        else {
            return;
        };
        let plugin = pd.plugin_ref();
        if let Some(session) = &mut self.live_follow.plugin
            && session.plugin == plugin
        {
            session.row = row.to_path_buf();
            session.wanted = preset.to_string();
            self.live_follow_moved(row);
            // The notes of the old sound are let go of.
            self.target.preview().push(NoteKind::AllOff);
            return;
        }
        self.stop_live_follow();
        let name = self
            .doc()
            .resolve(&key)
            .map_or_else(|| plugin.type_id(), |d| d.name.clone());
        if !(self.plugins.settings.enabled && self.plugins.host.is_installed(&plugin)) {
            return;
        }
        let Some(scene) = self.plugin_scene(&pd) else {
            return;
        };
        let request = InstantiateRequest {
            plugin: plugin.clone(),
            state: None,
            sample_rate: f64::from(self.doc().sample_rate),
            mode: RunMode::Realtime,
            pinned: false,
            powered: true,
        };
        match self.plugins.host.manager().begin_create(&request) {
            Ok(ticket) => {
                self.report(format!("Loading '{name}' to preview its presets..."));
                self.live_follow.plugin = Some(PluginSession {
                    row: row.to_path_buf(),
                    plugin,
                    name,
                    scene: Some(scene),
                    wanted: preset.to_string(),
                    loaded: None,
                    loading: None,
                    ticket: Some(ticket),
                    instance: None,
                });
            }
            Err(e) => self.report(format!("Cannot load '{name}' for the preview: {e}")),
        }
    }

    /// The scratch project of the plugin `pd` and the registry the
    /// worker builds it with: the built-in devices and this plugin's type.
    fn plugin_scene(
        &self,
        pd: &atomik_model::PluginDoc,
    ) -> Option<(atomik_model::RackDoc, atomik_device_sdk::Registry)> {
        let doc = super::patch_preview_ops::plugin_doc(pd, &self.registry).ok()?;
        let mut registry = atomik_render::default_registry();
        registry
            .register_plugin_from(&self.registry, &pd.type_id())
            .ok()?;
        Some((doc, registry))
    }

    /// One frame of the preview instance: the creation moves on, then the preset to hear is
    /// loaded.
    pub(super) fn drive_plugin_follow(&mut self) {
        let Some(session) = self.live_follow.plugin.as_mut() else {
            return;
        };
        if let Some(ticket) = session.ticket {
            let Some(answer) = self.plugins.host.manager().poll_create(ticket) else {
                return;
            };
            session.ticket = None;
            match answer {
                Ok(handle) => self.attach_preview_instance(handle),
                Err(e) => {
                    let name = session.name.clone();
                    self.live_follow.plugin = None;
                    self.report(format!("Cannot load '{name}' for the preview: {e}"));
                }
            }
            return;
        }
        let (Some(id), Some(_)) = (session.instance, &self.live_follow.active) else {
            return;
        };
        if let Some(loading) = session.loading.clone() {
            match self.plugins.host.manager().poll_load_preset(id) {
                None => return,
                Some(Ok(())) => session.loaded = Some(loading),
                Some(Err(e)) => {
                    let message = format!("The preset cannot be loaded for the preview: {e}");
                    session.loaded = Some(loading);
                    session.loading = None;
                    self.report(message);
                    return;
                }
            }
            session.loading = None;
        }
        if session.loaded.as_deref() == Some(session.wanted.as_str()) {
            return;
        }
        let wanted = session.wanted.clone();
        match self.plugins.host.manager().begin_load_preset(id, &wanted) {
            Ok(true) => session.loaded = Some(wanted),
            Ok(false) => session.loading = Some(wanted),
            Err(e) => {
                session.loaded = Some(wanted);
                let message = format!("The preset cannot be loaded for the preview: {e}");
                self.report(message);
            }
        }
    }

    /// The instance is made: its device goes to a worker that plays the scratch project.
    fn attach_preview_instance(&mut self, handle: PluginHandle) {
        let PluginHandle {
            id,
            device,
            restore_error,
            ..
        } = handle;
        let Some(session) = self.live_follow.plugin.as_mut() else {
            return;
        };
        let name = session.name.clone();
        if let Some(reason) = restore_error {
            self.live_follow.plugin = None;
            self.report(format!("Cannot load '{name}' for the preview: {reason}"));
            return;
        }
        session.instance = Some(id);
        let row = session.row.clone();
        let Some((doc, registry)) = session.scene.take() else {
            return;
        };
        let mut supplied = LiveDevices::new();
        supplied.insert(DeviceKey(PLUGIN_KEY.to_string()), Box::new(device));
        let spawned = self.spawn_follow(
            &row,
            Spawn {
                doc,
                root: None,
                registry: Some(registry),
                supplied,
            },
        );
        if !spawned {
            self.live_follow.plugin = None;
        }
    }

    /// Ends the preview instance's session: a creation that is still under way is given up (the
    /// device of a running one goes with its worker).
    pub(super) fn end_plugin_session(&mut self) {
        if let Some(session) = self.live_follow.plugin.take()
            && let Some(ticket) = session.ticket
        {
            self.plugins.host.manager().cancel_create(ticket);
        }
    }

    /// The preset the preview instance has loaded, and the one it is asked for (tests).
    #[cfg(test)]
    pub(crate) fn plugin_follow_presets(&self) -> Option<(Option<String>, String)> {
        self.live_follow
            .plugin
            .as_ref()
            .map(|s| (s.loaded.clone(), s.wanted.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_test_support::*;
    use crate::preview_notes::PREVIEW_TARGET;
    use crate::sink::RecordingSink;
    use atomik_library::{FileKind, FileRow, ItemInfo, preset_uri};
    use atomik_model::empty_doc;
    use atomik_plugin_host::{NullBackend, NullBehavior, NullPlugin, NullStats};
    use atomik_render::plugins::{PluginHost, PluginIsolation};
    use std::time::Duration;

    /// A generous bound for anything the worker does.
    const BOUND: Duration = Duration::from_secs(30);
    const IDS: [&str; 3] = ["init", "warm", "bright"];

    /// A core with the test gain effect on its rack (an in-process instance from the null
    /// backend, which offers three presets), the panel showing, and the preset rows listed.
    fn preset_core() -> (AppCore<RecordingSink>, Arc<NullStats>, DeviceKey) {
        let config = config(true, &[PKG]);
        let scanned = config
            .cache
            .files()
            .flat_map(|f| f.plugins.iter())
            .find(|p| p.plugin.type_id() == GAIN)
            .expect("the test gain effect is scanned")
            .clone();
        let backend = NullBackend::new().with_plugin(NullPlugin {
            path: PathBuf::from("null.clap"),
            plugin: scanned.plugin,
            layout: scanned.layout,
            behavior: NullBehavior::Gain(0.5),
            latency: 0,
        });
        let stats = backend.stats();
        stats.set_presets(true);
        let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
        core.plugins.host =
            PluginHost::with_backends(PluginIsolation::InProcess, vec![Box::new(backend)]);
        core.configure_plugins(config);
        add_plugin(&mut core, GAIN, "echo");
        core.frame();
        let dev = key("echo");
        let rows = IDS
            .iter()
            .map(|id| FileRow {
                path: preset_uri(dev.as_str(), id),
                name: (*id).to_string(),
                kind: FileKind::Preset,
                size: 0,
                info: None,
                item: Some(ItemInfo {
                    detail: String::new(),
                    category: "PRESET".to_string(),
                    device_type: String::new(),
                }),
                favorite: false,
                tags: Vec::new(),
                author: String::new(),
                categories: Vec::new(),
            })
            .collect();
        core.set_file_browser_visible(true);
        // The panel lists its first place on a worker: let that land before the rows are set.
        for _ in 0..3_000 {
            core.frame();
            if !core.browser_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        core.file_browser.set_rows(rows);
        (core, stats, dev)
    }

    /// Runs frames until `done` holds (a generous bound, never a count of frames).
    fn until(c: &mut AppCore<RecordingSink>, done: impl Fn(&AppCore<RecordingSink>) -> bool) {
        let mut waited = Duration::ZERO;
        loop {
            // Stands in for the voice, which plays what the worker feeds.
            for held in c.sink_mut().previews.iter_mut() {
                held.0.discard_buffered();
            }
            c.frame();
            if done(c) {
                return;
            }
            assert!(
                waited < BOUND,
                "the condition was not reached: {:?} {:?}",
                c.last_rejection(),
                c.live_follow_frames()
            );
            std::thread::sleep(Duration::from_millis(2));
            waited += Duration::from_millis(2);
        }
    }

    fn select(c: &mut AppCore<RecordingSink>, dev: &DeviceKey, id: &str) {
        assert!(c.file_browser.select_path(&preset_uri(dev.as_str(), id)));
        c.frame();
    }

    fn loaded(c: &AppCore<RecordingSink>, id: &str) -> bool {
        c.plugin_follow_presets()
            .is_some_and(|(loaded, _)| loaded.as_deref() == Some(id))
    }

    fn sounding(c: &AppCore<RecordingSink>) -> u64 {
        c.live_follow_frames().map_or(0, |(_, sounding)| sounding)
    }

    #[test]
    fn presets_of_one_plugin_are_heard_through_one_preview_instance() {
        let (mut c, stats, dev) = preset_core();
        assert_eq!(stats.created(), 1, "the instance of the rack");
        let destroyed = stats.destroyed();
        let revision = c.document().revision();
        select(&mut c, &dev, "init");
        until(&mut c, |c| loaded(c, "init") && c.live_follow_built());
        assert_eq!(stats.created(), 2, "one preview instance");
        assert_eq!(c.notes_target(), Some(PREVIEW_TARGET));
        // A key is heard through the plugin.
        c.note_target_cell()
            .preview()
            .push(atomik_core::NoteKind::On {
                key: 60,
                velocity: 0.8,
            });
        until(&mut c, |c| sounding(c) > 0);
        // The other presets of the plugin reuse the instance.
        select(&mut c, &dev, "warm");
        until(&mut c, |c| loaded(c, "warm"));
        select(&mut c, &dev, "bright");
        until(&mut c, |c| loaded(c, "bright"));
        assert_eq!(stats.created(), 2, "no new instance for another preset");
        assert_eq!(
            stats.destroyed(),
            destroyed,
            "nothing was destroyed while browsing"
        );
        // Nothing of the document changed.
        assert_eq!(c.document().revision(), revision);
        // Leaving the presets ends it: the keys go back and the instance is destroyed.
        c.file_browser.set_rows(Vec::new());
        c.frame();
        assert_ne!(c.notes_target(), Some(PREVIEW_TARGET));
        assert!(c.plugin_follow_presets().is_none());
        until(&mut c, |_| stats.destroyed() == 1);
        assert_eq!(stats.created(), 2, "the instance of the rack stays");
    }

    #[test]
    fn a_plugin_that_is_slow_to_load_shows_loading_and_blocks_nothing() {
        let (mut c, stats, dev) = preset_core();
        stats.set_background_create(Some(2));
        stats.hold_next_creates(1);
        let before = c.notes_target();
        select(&mut c, &dev, "warm");
        for _ in 0..20 {
            c.frame();
        }
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.starts_with("Loading ")),
            "{:?}",
            c.last_rejection()
        );
        assert!(!c.live_follow_has_keys(), "no instance yet");
        assert_eq!(c.notes_target(), before, "the keys stay on the track");
        stats.release_held_creates();
        until(&mut c, |c| loaded(c, "warm") && c.live_follow_built());
        assert!(c.live_follow_has_keys());
    }
}
