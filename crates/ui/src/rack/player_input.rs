//! Rack view code of the two player panels: the pattern slots and buttons of the
//! step sequencer, the in-place rename, the arpeggiator's mode, rate and octave buttons, the
//! meter feed of both and the geometry accessors for hosts and tests.

use super::{
    DeviceView, Flip, PlayerBtn, RackView, Rename, RenameKind, UiAction, param_cmd, pattern_count,
};
use crate::event::{Key, Modifiers};
use crate::seq_panel::{self, PlayState, RenameKey, SeqButton};
use crate::widget::current_value;
use atomik_model::{Command, DeviceKey, MAX_PATTERNS, PatternDoc};
use vello::kurbo::{Point, Rect};

/// Meter values kept per player panel (sequencer 3, arpeggiator 9).
const MAX_PLAYER_METERS: usize = 16;

impl RackView {
    // ------------------------------------------------------------------ meter feed

    /// The player half of [`RackView::set_device_meter_values`]: the values are rounded (the
    /// panels draw whole numbers), the panel is re-recorded only when a rounded value changed
    /// and a repaint is requested only when the panel is on screen.
    pub(super) fn player_meter_feed(&mut self, i: usize, values: &[f32]) {
        let q: Vec<f32> = values
            .iter()
            .take(MAX_PLAYER_METERS)
            .map(|v| {
                if v.is_finite() {
                    v.round().clamp(0.0, 255.0)
                } else {
                    0.0
                }
            })
            .collect();
        if q == self.devices[i].meters {
            return;
        }
        self.devices[i].meters = q;
        self.devices[i].bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    // ------------------------------------------------------------------ geometry and state for hosts and tests

    pub(super) fn front_device(&self, key: &DeviceKey) -> Option<&DeviceView> {
        let d = &self.devices[self.device_index(key)?];
        (self.flip == Flip::Front && !d.folded).then_some(d)
    }

    pub(super) fn view_center(&self, d: &DeviceView, r: Rect) -> Point {
        self.rack_to_view(r.center() + d.offset())
    }

    /// Centre of a button of a step sequencer panel (logical view coordinates), front side
    /// only.
    pub fn seq_button_center(&self, key: &DeviceKey, button: SeqButton) -> Option<Point> {
        let d = self.front_device(key)?;
        Some(self.view_center(d, d.spec.seq?.rect(button)))
    }

    /// Centre of mode button `mode` (0..6) of an arpeggiator panel.
    pub fn arp_mode_center(&self, key: &DeviceKey, mode: u8) -> Option<Point> {
        let d = self.front_device(key)?;
        let r = *d.spec.arp?.modes.get(usize::from(mode))?;
        Some(self.view_center(d, r))
    }

    /// Centre of rate button `rate` (0..7) of an arpeggiator panel.
    pub fn arp_rate_center(&self, key: &DeviceKey, rate: u8) -> Option<Point> {
        let d = self.front_device(key)?;
        let r = *d.spec.arp?.rates.get(usize::from(rate))?;
        Some(self.view_center(d, r))
    }

    /// Centre of step cell `step` (0..16) of an arpeggiator panel.
    pub fn arp_step_center(&self, key: &DeviceKey, step: u8) -> Option<Point> {
        let d = self.front_device(key)?;
        let r = *d.spec.arp?.steps.get(usize::from(step))?;
        Some(self.view_center(d, r))
    }

    /// Centre of the octave stepper's plus (`up`) or minus button.
    pub fn arp_octave_center(&self, key: &DeviceKey, up: bool) -> Option<Point> {
        let d = self.front_device(key)?;
        let a = d.spec.arp?;
        Some(self.view_center(d, if up { a.octave_inc } else { a.octave_dec }))
    }

    /// The edited pattern (0-based) of a step sequencer panel; `None` for other devices.
    pub fn edited_pattern(&self, key: &DeviceKey) -> Option<usize> {
        let d = &self.devices[self.device_index(key)?];
        (d.spec.seq.is_some() || d.spec.drum_gen.is_some()).then_some(d.edit)
    }

    /// Selects the edited pattern of a step sequencer panel (clamped to the pattern count; no
    /// action emitted; also done by a click on a slot).
    pub fn set_edited_pattern(&mut self, key: &DeviceKey, index: usize) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        let d = &self.devices[i];
        if let (true, Some(dev)) = (
            d.spec.seq.is_some() || d.spec.drum_gen.is_some(),
            d.device.as_ref(),
        ) {
            let index = index.min(pattern_count(dev) - 1);
            self.set_edit(i, index);
        }
    }

    pub(super) fn set_edit(&mut self, i: usize, index: usize) -> bool {
        let d = &mut self.devices[i];
        if d.edit == index {
            return false;
        }
        d.edit = index;
        d.revision += 1;
        self.edits.insert(d.key.clone(), index);
        self.repaint = true;
        true
    }

    fn play_state_of(&self, d: &DeviceView) -> Option<PlayState> {
        let dev = d.device.as_ref()?;
        if d.spec.seq.is_none() && d.spec.drum_gen.is_none() {
            return None;
        }
        let param = d
            .desc?
            .param_by_key("pattern")
            .map_or(1.0, |p| current_value(dev, p));
        Some(seq_panel::play_state(param, &d.meters, pattern_count(dev)))
    }

    fn seq_state(&self, key: &DeviceKey) -> Option<PlayState> {
        self.play_state_of(&self.devices[self.device_index(key)?])
    }

    /// The pattern a step sequencer is playing (0-based): the `pattern` param until the device
    /// publishes that it is running, then the published one.
    pub fn playing_pattern(&self, key: &DeviceKey) -> Option<usize> {
        self.seq_state(key).map(|s| s.playing)
    }

    /// The pattern the `pattern` param points to while the running one has not ended.
    pub fn queued_pattern(&self, key: &DeviceKey) -> Option<usize> {
        self.seq_state(key).and_then(|s| s.queued)
    }

    /// The step that started last while the sequencer runs (the panel's cursor).
    pub fn step_cursor(&self, key: &DeviceKey) -> Option<usize> {
        let d = &self.devices[self.device_index(key)?];
        let state = self.play_state_of(d)?;
        let dev = d.device.as_ref()?;
        let len = if d.spec.drum_gen.is_some() {
            dev.drums
                .clone()
                .unwrap_or_default()
                .patterns
                .get(state.playing)?
                .length
        } else {
            dev.steps
                .clone()
                .unwrap_or_default()
                .patterns
                .get(state.playing)?
                .length
        };
        state.cursor.filter(|c| *c < usize::from(len))
    }

    /// The pattern clipboard of the sequencer panels.
    pub fn pattern_clipboard(&self) -> Option<&PatternDoc> {
        self.pattern_clip.as_ref()
    }

    /// Sets the pattern clipboard (the app mirrors it with the grid editor's).
    pub fn set_pattern_clipboard(&mut self, clip: Option<PatternDoc>) {
        self.pattern_clip = clip;
    }

    /// The pattern name being edited in place on the panel of `key`.
    pub fn renaming(&self, key: &DeviceKey) -> Option<&str> {
        let r = self.rename.as_ref()?;
        (self.devices.get(r.dev)?.key == *key).then_some(r.text.as_str())
    }

    /// The held-key count and the next notes an arpeggiator panel shows (from the published
    /// meter values); `None` for other devices and before the first feed.
    pub fn arp_display(&self, key: &DeviceKey) -> Option<(u8, Vec<u8>)> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.arp?;
        crate::arp_panel::arp_display(&d.meters)
    }

    // ------------------------------------------------------------------ gestures

    pub(super) fn bump(&mut self, i: usize) {
        if let Some(d) = self.devices.get_mut(i) {
            d.revision += 1;
            self.repaint = true;
        }
    }

    /// Ends a rename without a command (a click elsewhere).
    pub(super) fn cancel_rename(&mut self) {
        if let Some(r) = self.rename.take() {
            self.bump(r.dev);
        }
    }

    fn param_value(&self, i: usize, key: &str) -> f32 {
        let d = &self.devices[i];
        match (d.desc, d.device.as_ref()) {
            (Some(desc), Some(dev)) => desc
                .param_by_key(key)
                .map_or(0.0, |p| current_value(dev, p)),
            _ => 0.0,
        }
    }

    fn pattern_of(&self, i: usize, index: usize) -> Option<PatternDoc> {
        let dev = self.devices[i].device.as_ref()?;
        let steps = dev.steps.clone().unwrap_or_default();
        steps.patterns.get(index).cloned()
    }

    pub(super) fn player_down(
        &mut self,
        i: usize,
        b: PlayerBtn,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        self.select(i, out);
        let key = self.devices[i].key.clone();
        match b {
            PlayerBtn::Seq(sb) => self.seq_down(i, sb, modifiers, clicks, out),
            PlayerBtn::Drum(db) => self.drum_down(i, db, modifiers, clicks, out),
            PlayerBtn::Mode(m) => self.choose(i, "mode", m, out),
            PlayerBtn::Rate(m) => self.choose(i, "rate", m, out),
            PlayerBtn::Step(n) => {
                let param = format!("step.{}", u32::from(n) + 1);
                let now = self.param_value(i, &param).round() as i32;
                out.push(param_cmd(&key, &param, ((now + 1) % 4) as f32, None));
            }
            PlayerBtn::FromClip => {
                if let Some(scale) = self.clip_scale {
                    let cmd = crate::key_chord_panel::from_clip_command(&key, scale);
                    out.push(UiAction::Command(cmd));
                }
            }
            PlayerBtn::OctaveDec | PlayerBtn::OctaveInc => {
                let now = self.param_value(i, "octaves").round() as i32;
                let next = if b == PlayerBtn::OctaveInc {
                    now + 1
                } else {
                    now - 1
                };
                let next = next.clamp(1, 4);
                if next != now {
                    out.push(param_cmd(&key, "octaves", next as f32, None));
                }
            }
        }
    }

    /// `SetParam` of a choice param unless it already has that value.
    fn choose(&self, i: usize, param: &str, value: u8, out: &mut Vec<UiAction>) {
        if self.param_value(i, param).round() as i32 != i32::from(value) {
            let device = &self.devices[i].key;
            out.push(param_cmd(device, param, f32::from(value), None));
        }
    }

    pub(super) fn select_pattern(&mut self, i: usize, index: usize, out: &mut Vec<UiAction>) {
        if self.set_edit(i, index) {
            out.push(UiAction::SelectPattern {
                device: self.devices[i].key.clone(),
                index: index as u8,
            });
        }
    }

    fn seq_down(
        &mut self,
        i: usize,
        b: SeqButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some(dev) = self.devices[i].device.as_ref() else {
            return;
        };
        let count = pattern_count(dev);
        let edit = self.devices[i].edit.min(count - 1);
        let key = self.devices[i].key.clone();
        match b {
            SeqButton::Slot(n) => {
                let n = usize::from(n);
                if n >= count {
                    return;
                }
                self.select_pattern(i, n, out);
                if modifiers.command() {
                    out.push(param_cmd(&key, "pattern", (n + 1) as f32, None));
                }
                if clicks >= 2
                    && let Some(p) = self.pattern_of(i, n)
                {
                    self.rename = Some(Rename {
                        dev: i,
                        index: n as u8,
                        text: p.name,
                        kind: RenameKind::Pattern,
                    });
                    self.bump(i);
                }
            }
            SeqButton::Add => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::AddPattern { device: key }));
                    self.select_pattern(i, count, out);
                }
            }
            SeqButton::Duplicate => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::DuplicatePattern {
                        device: key,
                        index: edit as u8,
                    }));
                    self.select_pattern(i, count, out);
                }
            }
            SeqButton::Remove => {
                if count > 1 {
                    out.push(UiAction::Command(Command::RemovePattern {
                        device: key,
                        index: edit as u8,
                    }));
                    self.select_pattern(i, edit.min(count - 2), out);
                }
            }
            SeqButton::Copy => self.pattern_clip = self.pattern_of(i, edit),
            SeqButton::Paste => {
                if let (Some(clip), Some(target)) = (&self.pattern_clip, self.pattern_of(i, edit)) {
                    let pattern = seq_panel::pasted(&target, clip);
                    if pattern != target {
                        out.push(set_pattern(&key, edit, pattern));
                    }
                }
            }
            SeqButton::Clear => {
                if let Some(target) = self.pattern_of(i, edit) {
                    let pattern = seq_panel::cleared(&target);
                    if pattern != target {
                        out.push(set_pattern(&key, edit, pattern));
                    }
                }
            }
            SeqButton::Edit => out.push(UiAction::OpenSeqEditor { device: key }),
            SeqButton::Preview => {
                if clicks >= 2 {
                    out.push(UiAction::OpenSeqEditor { device: key });
                }
            }
        }
    }

    // ------------------------------------------------------------------ rename

    pub(super) fn rename_key_press(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some(mut r) = self.rename.take() else {
            return;
        };
        if r.kind == RenameKind::Macro {
            self.macro_rename_key(r, key);
            return;
        }
        if r.kind == RenameKind::Device {
            self.device_rename_key(r, key, out);
            return;
        }
        if self.devices[r.dev].spec.drum_gen.is_some() {
            self.drum_rename_key(r, key, out);
            return;
        }
        match seq_panel::rename_key(&mut r.text, key) {
            RenameKey::Editing => {
                let dev = r.dev;
                self.rename = Some(r);
                self.bump(dev);
            }
            RenameKey::Commit => {
                self.bump(r.dev);
                let target = self.pattern_of(r.dev, usize::from(r.index));
                if let Some(p) = target.and_then(|t| seq_panel::renamed(&t, &r.text)) {
                    let device = self.devices[r.dev].key.clone();
                    out.push(set_pattern(&device, usize::from(r.index), p));
                }
            }
            RenameKey::Cancel => self.bump(r.dev),
        }
    }
}

fn set_pattern(device: &DeviceKey, index: usize, pattern: PatternDoc) -> UiAction {
    UiAction::Command(Command::SetPattern {
        device: device.clone(),
        index: index as u8,
        pattern,
        merge: None,
    })
}
