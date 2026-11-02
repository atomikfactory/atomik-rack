//! Note players in the app core: pattern data for the engine, the sequencer
//! grid editor pane, the shared pattern clipboard and auditions through a player.
//!
//! # Pattern data
//!
//! A step sequencer's patterns live in the document ([`atomik_model::RackDevice::steps`]); the
//! device plays an immutable [`atomik_device_sdk::PlayerData`] built from them with
//! [`atomik_model::player_data_for`] (the renderer calls the same function, so a bounce plays
//! what the app plays). [`AppCore::send_player_data`] hands it to the engine with
//! [`EngineSink::set_device_player_data`]
//!
//! * right after a sequencer is inserted (a restart, an added, pasted or un-removed device), in
//!   the restart order: insert, then data,
//! * and on every [`atomik_model::SyncOp::PatternsChanged`] (every pattern edit, and its undo
//!   and redo), one call per edit.
//!
//! The engine applies a swap at a batch boundary and never cuts a sounding note.
//!
//! # Sequencer grid editor
//!
//! [`Pane::SeqEditor`](crate::Pane::SeqEditor) shows [`SeqEditorView`] in place of the
//! arrangement, exclusive with the other editors. It opens on [`UiAction::OpenSeqEditor`] (the
//! panel's Edit button) on the pattern the panel is editing, and closes on
//! [`UiAction::CloseSeqEditor`], when the device goes away and on a new or opened document.
//! [`UiAction::SelectPattern`] is mirrored between the panel and the editor, and the pattern
//! clipboard is one value shared by both.
//!
//! # Meters
//!
//! [`AppCore::feed_device_meters`] also feeds the players' published values (the sequencer's
//! playing pattern and step, the arpeggiator's held and upcoming keys) to the rack view, and the
//! sequencer's to the editor for its cursor.
//!
//! # Audition
//!
//! A player ignores live notes, so [`AppCore::audition_targets`] sends an audition of a
//! sequencer (the grid editor's key header or note click) to what its `notes_out` is cabled to.

use super::AppCore;
use crate::sink::EngineSink;
use crate::target::Target;
use atomik_devices::drum_generator::{DrumStyle, generate_pattern};
use atomik_model::{Command, DeviceKey, DrumPatternDoc};
use atomik_ui::SeqEditorView;

/// Whether devices of `type_id` are edited as step patterns (the step sequencer and the CV
/// sequencer).
fn takes_step_doc(type_id: &str) -> bool {
    atomik_ui::seq_panel::takes_step_doc(type_id)
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ sequencer editor pane

    /// The sequencer grid editor.
    pub fn seq_editor(&self) -> &SeqEditorView {
        &self.seq_editor
    }

    /// The sequencer grid editor, mutably (bounds, scale, focus).
    pub fn seq_editor_mut(&mut self) -> &mut SeqEditorView {
        &mut self.seq_editor
    }

    /// The step sequencer open in the grid editor (shown in place of the arrangement).
    pub fn seq_editor_device(&self) -> Option<&DeviceKey> {
        self.seq_editor.device()
    }

    fn is_step_sequencer(&self, device: &DeviceKey) -> bool {
        self.doc()
            .resolve(device)
            .is_some_and(|d| takes_step_doc(&d.type_id))
    }

    /// Opens the grid editor on a step sequencer (ignored for other devices), on the pattern its
    /// panel is editing; it replaces the piano roll and the other editors.
    pub fn open_seq_editor(&mut self, device: DeviceKey) {
        if !self.is_step_sequencer(&device) {
            return;
        }
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_container_editor();
        self.seq_editor.open(device.clone());
        self.seq_editor.sync(self.document.doc(), &self.registry);
        let edited = self.view.edited_pattern(&device).unwrap_or(0);
        self.seq_editor.set_pattern_index(edited);
        // The next feed brings the cursor even if the device published nothing new.
        self.fx_meter_seq.remove(&device);
        self.share_pattern_clipboard();
    }

    /// Opens the grid editor on the selected step sequencer, else the first one of the rack.
    /// False when the rack has none.
    pub fn open_seq_editor_default(&mut self) -> bool {
        let doc = self.doc();
        let key = self
            .view
            .selected()
            .filter(|k| doc.resolve(k).is_some_and(|d| takes_step_doc(&d.type_id)))
            .cloned()
            .or_else(|| {
                self.flat_devices()
                    .into_iter()
                    .find(|(_, d)| takes_step_doc(&d.type_id))
                    .map(|(k, _)| k)
            });
        match key {
            Some(key) => {
                self.open_seq_editor(key);
                true
            }
            None => false,
        }
    }

    /// Closes the grid editor (the arrangement shows again).
    pub fn close_seq_editor(&mut self) {
        self.seq_editor.close();
    }

    /// Mirrors the edited pattern between the panel and the editor (view state only).
    pub(super) fn select_pattern(&mut self, device: &DeviceKey, index: usize) {
        self.view.set_edited_pattern(device, index);
        if self.seq_editor.device() == Some(device) {
            self.seq_editor.set_pattern_index(index);
        }
    }

    /// Keeps one pattern clipboard for the panel and the editor: whichever changed since the
    /// last agreement is copied to the other.
    pub(super) fn share_pattern_clipboard(&mut self) {
        let rack = self.view.pattern_clipboard().cloned();
        let editor = self.seq_editor.clipboard().cloned();
        if rack == editor {
            self.pattern_clip_seen = rack;
        } else if rack != self.pattern_clip_seen {
            self.seq_editor.set_clipboard(rack.clone());
            self.pattern_clip_seen = rack;
        } else {
            self.view.set_pattern_clipboard(editor.clone());
            self.pattern_clip_seen = editor;
        }
    }

    // ------------------------------------------------------------------ pattern data

    /// Sends a device that takes pattern data its data from the document (the default pattern for
    /// a sequencer without stored `steps`); other devices are left alone.
    pub(super) fn send_player_data(&mut self, key: &DeviceKey) {
        let Some(id) = self.ids.get(key).copied() else {
            return;
        };
        let Some(device) = self.doc().resolve(key) else {
            return;
        };
        let Some(data) = atomik_model::player_data_for(device, &self.registry) else {
            return;
        };
        let result = self.sink.set_device_player_data(id, Some(data));
        self.engine_result(result);
    }

    // ------------------------------------------------------------------ drum generator

    /// GENERATE or RANDOMISE on a drum generator panel: writes the steps of pattern `index` from
    /// the device's `style` and `density` params and a seed ([`generate_pattern`], a pure function
    /// of them, so the same seed writes the same pattern) as one undo step. Every press picks a
    /// NEW seed (never the one the device holds) and writes it to `random.seed` in the same step,
    /// so GENERATE gives another rhythm each time. RANDOMISE also draws the character: `style`,
    /// `density`, `variation`, `accent` and the pattern's swing. The lanes, the length, the grid
    /// and the name of the pattern stay.
    pub fn generate_drums(&mut self, device: DeviceKey, index: u8, randomise: bool) {
        let mut draw = self.next_drum_draw();
        let Some(dev) = self
            .doc()
            .resolve(&device)
            .filter(|d| self.registry.takes_drums(&d.type_id))
        else {
            return self.reject(format!("{device} is not a drum generator"));
        };
        let Some(desc) = self.registry.get(&dev.type_id).map(|f| f.descriptor) else {
            return;
        };
        let value = |key: &str| {
            desc.param_by_key(key).map_or(0.0, |p| {
                p.clamp(dev.params.get(key).copied().unwrap_or(p.default))
            })
        };
        let held = value("random.seed").round() as u16;
        let drums = dev.drums.clone().unwrap_or_default();
        let Some(current) = drums.patterns.get(usize::from(index)) else {
            return self.reject(format!("{device} has no pattern {}", index + 1));
        };
        // A seed that is not the held one, whatever the draw.
        let seed = {
            let s = ((draw >> 33) % 999) as u16;
            if s >= held { s + 1 } else { s }
        };
        let mut params = vec![("random.seed", f32::from(seed))];
        let mut data = current.to_data();
        let (mut style, mut density) = (value("style").round() as usize, value("density"));
        if randomise {
            let mut pick = |n: u64| {
                draw = draw
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (draw >> 33) % n
            };
            style = pick(DrumStyle::ALL.len() as u64) as usize;
            density = (pick(81) as f32 - 30.0) / 100.0;
            const SWINGS: [u8; 7] = [0, 0, 8, 12, 16, 22, 28];
            data.swing = SWINGS[pick(SWINGS.len() as u64) as usize];
            params.push(("style", style as f32));
            params.push(("density", density));
            params.push(("variation", pick(61) as f32 / 100.0));
            params.push(("accent", pick(51) as f32 / 100.0));
        }
        let density = (f64::from(density) * 100.0).round() / 100.0;
        let written = generate_pattern(
            &data,
            DrumStyle::from_index(style),
            density,
            u64::from(seed),
        );
        let pattern = DrumPatternDoc::from_data(&written, current.name.clone());
        let mut batch: Vec<Command> = params
            .into_iter()
            .map(|(param, value)| Command::SetParam {
                key: device.clone(),
                param: param.into(),
                value,
                merge: None,
            })
            .collect();
        batch.push(Command::SetDrumPattern {
            device,
            index,
            pattern,
            merge: None,
        });
        self.document.end_merge();
        let _ = self.apply_command(Command::Batch(batch));
    }

    /// The next draw of the drum generator's seed and character (a linear congruential step of
    /// a counter that starts from the clock when the app starts, so every launch differs).
    fn next_drum_draw(&mut self) -> u64 {
        self.drum_draws = self
            .drum_draws
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.drum_draws
    }

    // ------------------------------------------------------------------ audition

    /// Where an audition of `device` goes: its own notes input, or, for a note player, the notes
    /// inputs its `notes_out` is cabled to.
    pub(super) fn audition_targets(&self, device: &DeviceKey) -> Vec<Target> {
        let doc = self.doc();
        let Some(d) = doc.resolve(device) else {
            return Vec::new();
        };
        if !atomik_devices::is_player(&d.type_id) {
            return self.target_of(device).into_iter().collect();
        }
        self.player_audition_targets(device)
    }

    /// Where a click on a pad or key OF `device` goes: the device's own notes input, never the
    /// top of its track's player stack (a generator above a drum machine does not pass a pad's
    /// note through, so a cabled drum machine would fall silent). A note player forwards to
    /// what it is cabled to, as [`Self::audition_targets`] does.
    pub(super) fn own_audition_targets(&self, device: &DeviceKey) -> Vec<Target> {
        let Some(d) = self.doc().resolve(device) else {
            return Vec::new();
        };
        if atomik_devices::is_player(&d.type_id) {
            return self.player_audition_targets(device);
        }
        self.own_target_of(device).into_iter().collect()
    }
}
