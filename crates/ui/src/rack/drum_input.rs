//! Rack view code of the drum generator panel: the pattern slots and buttons, the step cells
//! (a click toggles, a vertical drag sets the velocity), the lane mutes, the page selector, the
//! length stepper, GENERATE and RANDOMISE, the in-place rename and the geometry accessors for
//! hosts and tests.

use super::{Drag, RackView, Rename, UiAction, param_cmd, pattern_count};
use crate::drum_gen_panel::{self, DrumButton, VISIBLE_STEPS};
use crate::event::{Key, Modifiers};
use crate::seq_panel::{self, RenameKey};
use atomik_model::{Command, DeviceKey, DrumPatternDoc, DrumStepDoc, MAX_PATTERNS};
use vello::kurbo::Point;

/// A cell of the drum grid is pressed: a release in place toggles the step, a drag sets its
/// velocity.
#[derive(Debug, Clone)]
pub(super) struct DrumDrag {
    dev: usize,
    index: u8,
    lane: u8,
    step: u8,
    start_y: f64,
    /// The velocity the cell had when it was pressed (0 for an empty step).
    start: u8,
    prob: u8,
    merge: u64,
    moved: bool,
    velocity: u8,
}

/// Pointer travel in pixels before a press on a cell counts as a drag.
const DRAG_THRESHOLD: f64 = 3.0;

fn set_step(
    device: &DeviceKey,
    index: u8,
    lane: u8,
    step: u8,
    s: DrumStepDoc,
    merge: Option<u64>,
) -> UiAction {
    UiAction::Command(Command::SetDrumStep {
        device: device.clone(),
        index,
        lane,
        step,
        velocity: s.velocity,
        prob: s.prob,
        merge,
    })
}

fn set_pattern(device: &DeviceKey, index: usize, pattern: DrumPatternDoc) -> UiAction {
    UiAction::Command(Command::SetDrumPattern {
        device: device.clone(),
        index: index as u8,
        pattern,
        merge: None,
    })
}

impl RackView {
    // ------------------------------------------------------------------ geometry and state for hosts and tests

    /// Centre of a button (or of the grid) of a drum generator panel (logical view
    /// coordinates), front side only.
    pub fn drum_button_center(&self, key: &DeviceKey, button: DrumButton) -> Option<Point> {
        let d = self.front_device(key)?;
        Some(self.view_center(d, d.spec.drum_gen?.rect(button)))
    }

    /// Centre of the cell of `lane` (0..16) and `step` (0..64) of a drum generator panel when
    /// that step is on the visible page, front side only.
    pub fn drum_cell_center(&self, key: &DeviceKey, lane: usize, step: usize) -> Option<Point> {
        let d = self.front_device(key)?;
        let panel = d.spec.drum_gen?;
        let col = step.checked_sub(d.page * VISIBLE_STEPS)?;
        (col < VISIBLE_STEPS && lane < atomik_device_sdk::DRUM_LANES)
            .then(|| self.view_center(d, panel.cell_rect(lane, col)))
    }

    /// The visible page (0-based) of a drum generator panel; `None` for other devices.
    pub fn drum_page(&self, key: &DeviceKey) -> Option<usize> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.drum_gen.is_some().then_some(d.page)
    }

    /// Selects the visible page of a drum generator panel (clamped to the pages of the edited
    /// pattern).
    pub fn set_drum_page(&mut self, key: &DeviceKey, page: usize) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        if self.devices[i].spec.drum_gen.is_none() {
            return;
        }
        let length = self
            .drum_pattern_of(i, self.devices[i].edit)
            .map_or(16, |p| p.length);
        let page = drum_gen_panel::clamp_page(page, length);
        if self.devices[i].page != page {
            self.devices[i].page = page;
            self.pages.insert(key.clone(), page);
            self.bump(i);
        }
    }

    /// The pattern clipboard of the drum generator panels.
    pub fn drum_clipboard(&self) -> Option<&DrumPatternDoc> {
        self.drum_clip.as_ref()
    }

    /// Sets the drum pattern clipboard.
    pub fn set_drum_clipboard(&mut self, clip: Option<DrumPatternDoc>) {
        self.drum_clip = clip;
    }

    /// The lane labels a drum generator panel shows: the pad name of the drum machine it is
    /// cabled to for the keys 36 to 51, else the note name of the key; `None` for other devices.
    pub fn drum_lane_labels(&self, key: &DeviceKey) -> Option<Vec<String>> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.drum_gen?;
        let p = self.drum_pattern_of(self.device_index(key)?, d.edit)?;
        Some(
            p.lanes
                .iter()
                .map(|l| drum_gen_panel::lane_label(l.key, &d.pad_names))
                .collect(),
        )
    }

    fn drum_pattern_of(&self, i: usize, index: usize) -> Option<DrumPatternDoc> {
        let dev = self.devices[i].device.as_ref()?;
        let drums = dev.drums.clone().unwrap_or_default();
        drums.patterns.get(index).cloned()
    }

    // ------------------------------------------------------------------ gestures

    pub(super) fn drum_down(
        &mut self,
        i: usize,
        b: DrumButton,
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
        let Some(target) = self.drum_pattern_of(i, edit) else {
            return;
        };
        match b {
            DrumButton::Slot(n) => {
                let n = usize::from(n);
                if n >= count {
                    return;
                }
                self.select_pattern(i, n, out);
                if modifiers.command() {
                    out.push(param_cmd(&key, "pattern", (n + 1) as f32, None));
                }
                if clicks >= 2
                    && let Some(p) = self.drum_pattern_of(i, n)
                {
                    self.rename = Some(Rename {
                        dev: i,
                        index: n as u8,
                        text: p.name,
                        is_macro: false,
                    });
                    self.bump(i);
                }
            }
            DrumButton::Add => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::AddDrumPattern { device: key }));
                    self.select_pattern(i, count, out);
                }
            }
            DrumButton::Duplicate => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::DuplicateDrumPattern {
                        device: key,
                        index: edit as u8,
                    }));
                    self.select_pattern(i, count, out);
                }
            }
            DrumButton::Remove => {
                if count > 1 {
                    out.push(UiAction::Command(Command::RemoveDrumPattern {
                        device: key,
                        index: edit as u8,
                    }));
                    self.select_pattern(i, edit.min(count - 2), out);
                }
            }
            DrumButton::Copy => self.drum_clip = Some(target),
            DrumButton::Paste => {
                if let Some(clip) = &self.drum_clip {
                    let pattern = drum_gen_panel::pasted(&target, clip);
                    if pattern != target {
                        out.push(set_pattern(&key, edit, pattern));
                    }
                }
            }
            DrumButton::Clear => {
                let pattern = target.cleared();
                if pattern != target {
                    out.push(set_pattern(&key, edit, pattern));
                }
            }
            DrumButton::Generate | DrumButton::Randomise => {
                out.push(UiAction::GenerateDrums {
                    device: key,
                    index: edit as u8,
                    randomise: b == DrumButton::Randomise,
                });
            }
            DrumButton::LenDec | DrumButton::LenInc => {
                let step = if modifiers.shift { 4 } else { 1 };
                let delta = if b == DrumButton::LenInc { step } else { -step };
                let pattern = drum_gen_panel::resized(&target, delta);
                if pattern != target {
                    out.push(set_pattern(&key, edit, pattern));
                }
            }
            DrumButton::Page(n) => self.set_drum_page(&key, usize::from(n)),
            DrumButton::Mute(lane) => {
                let lane = usize::from(lane);
                if let Some(mut setting) = target.lanes.get(lane).copied() {
                    setting.mute = !setting.mute;
                    out.push(UiAction::Command(Command::SetDrumLane {
                        device: key,
                        index: edit as u8,
                        lane: lane as u8,
                        setting,
                    }));
                }
            }
            DrumButton::Grid => self.drum_cell_down(i, edit, &target),
        }
    }

    /// A press on the cells: remembers the cell; the command comes with the release or the drag.
    fn drum_cell_down(&mut self, i: usize, edit: usize, target: &DrumPatternDoc) {
        let rp = self.view_to_rack(self.pointer);
        let panel_pos = rp - self.devices[i].offset();
        let Some(panel) = self.devices[i].spec.drum_gen else {
            return;
        };
        let Some((lane, col)) = panel.cell_at(panel_pos) else {
            return;
        };
        let step = self.devices[i].page * VISIBLE_STEPS + col;
        if step >= usize::from(target.length) {
            return;
        }
        let s = target.steps[lane][step];
        let merge = self.next_merge;
        self.next_merge += 1;
        self.drag = Drag::Drum(DrumDrag {
            dev: i,
            index: edit as u8,
            lane: lane as u8,
            step: step as u8,
            start_y: self.pointer.y,
            start: s.velocity,
            prob: s.prob,
            merge,
            moved: false,
            velocity: s.velocity,
        });
    }

    /// The pointer moved with a cell pressed: past the threshold the cell follows the pointer's
    /// height as a velocity (an empty step starts at the default velocity).
    pub(super) fn drum_move(&mut self, mut d: DrumDrag, pos: Point, out: &mut Vec<UiAction>) {
        if !d.moved && (pos.y - d.start_y).abs() < DRAG_THRESHOLD {
            self.drag = Drag::Drum(d);
            return;
        }
        d.moved = true;
        let base = if d.start == 0 {
            drum_gen_panel::DEFAULT_VELOCITY
        } else {
            d.start
        };
        let v = drum_gen_panel::dragged_velocity(base, d.start_y - pos.y);
        if v != d.velocity {
            let key = self.devices[d.dev].key.clone();
            let s = DrumStepDoc {
                velocity: v,
                prob: d.prob,
            };
            out.push(set_step(&key, d.index, d.lane, d.step, s, Some(d.merge)));
            d.velocity = v;
        }
        self.drag = Drag::Drum(d);
        self.repaint = true;
    }

    /// The release: without a drag the step toggles.
    pub(super) fn drum_up(&mut self, d: &DrumDrag, out: &mut Vec<UiAction>) {
        let key = self.devices[d.dev].key.clone();
        if d.moved {
            out.push(UiAction::EndMerge);
            return;
        }
        let s = DrumStepDoc {
            velocity: drum_gen_panel::toggled(d.start),
            prob: d.prob,
        };
        out.push(set_step(&key, d.index, d.lane, d.step, s, None));
    }

    /// Esc during a drag: the step goes back to the velocity it had.
    pub(super) fn drum_cancel(&mut self, d: &DrumDrag, out: &mut Vec<UiAction>) {
        if d.moved && d.velocity != d.start {
            let key = self.devices[d.dev].key.clone();
            let s = DrumStepDoc {
                velocity: d.start,
                prob: d.prob,
            };
            out.push(set_step(&key, d.index, d.lane, d.step, s, Some(d.merge)));
        }
        out.push(UiAction::EndMerge);
    }

    /// One key press while a drum pattern's name is being edited.
    pub(super) fn drum_rename_key(&mut self, mut r: Rename, key: &Key, out: &mut Vec<UiAction>) {
        match seq_panel::rename_key(&mut r.text, key) {
            RenameKey::Editing => {
                let dev = r.dev;
                self.rename = Some(r);
                self.bump(dev);
            }
            RenameKey::Commit => {
                self.bump(r.dev);
                let name = r.text.trim().to_string();
                let target = self.drum_pattern_of(r.dev, usize::from(r.index));
                if let Some(t) = target.filter(|t| !name.is_empty() && name != t.name) {
                    let device = self.devices[r.dev].key.clone();
                    let pattern = DrumPatternDoc { name, ..t };
                    out.push(set_pattern(&device, usize::from(r.index), pattern));
                }
            }
            RenameKey::Cancel => self.bump(r.dev),
        }
    }
}
