//! Rack view code of the Chord Wheel panel (see [`crate::chord_wheel_panel`]): a click on a wedge,
//! the right-click menu, the key shortcut, the slot cards (select, padlock, length drag and
//! scroll), the slot count, the buttons and the geometry accessors for hosts and tests.
//!
//! The selected slot is the device view's `edit` (kept per device in the view, never saved);
//! SUGGEST is its `fx_mode`.

use super::{Drag, RackView, UiAction, param_cmd};
use crate::chord_wheel_panel::{self as panel, Wedge, WheelButton};
use crate::event::Modifiers;
use crate::keymap::{GestureClass, Role};
use atomik_model::{Command, DeviceKey};
use vello::kurbo::Point;

/// The length of a slot card is pressed: a drag steps it, a release in place only selects.
#[derive(Debug, Clone)]
pub(super) struct BeatsDrag {
    dev: usize,
    slot: u8,
    start_y: f64,
    /// The index in the lengths when the press began.
    start: usize,
    /// The index the drag has set so far.
    index: usize,
    merge: u64,
    moved: bool,
}

/// Pointer travel before a press on a length is a drag.
const DRAG_THRESHOLD: f64 = 3.0;

fn beats_param(slot: usize) -> String {
    format!("slot.{}.beats", slot + 1)
}

impl RackView {
    // ------------------------------------------------------------------ state and geometry

    fn wheel_state(&self, i: usize) -> Option<panel::WheelState> {
        let d = self.devices.get(i)?;
        d.spec.chord_wheel?;
        Some(panel::wheel_state(d.device.as_ref()?, &d.desc?))
    }

    /// The selected slot (0-based) of a Chord Wheel panel; `None` for other devices.
    pub fn wheel_slot(&self, key: &DeviceKey) -> Option<usize> {
        let i = self.device_index(key)?;
        let state = self.wheel_state(i)?;
        Some(panel::selected_slot(self.devices[i].edit, state.slots))
    }

    /// Selects a slot of a Chord Wheel panel (clamped to the active slots; no action emitted;
    /// a click on a card does the same).
    pub fn set_wheel_slot(&mut self, key: &DeviceKey, slot: usize) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        if let Some(state) = self.wheel_state(i) {
            self.set_edit(i, panel::selected_slot(slot, state.slots));
        }
    }

    /// Whether SUGGEST is on on a Chord Wheel panel.
    pub fn wheel_suggest(&self, key: &DeviceKey) -> bool {
        self.device_index(key)
            .map(|i| &self.devices[i])
            .is_some_and(|d| d.spec.chord_wheel.is_some() && d.fx_mode != 0)
    }

    /// The slot the device is sounding (0-based), from its published meter values.
    pub fn wheel_playing(&self, key: &DeviceKey) -> Option<usize> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.chord_wheel?;
        panel::playing_slot(&d.meters)
    }

    /// Centre of a wedge of a Chord Wheel panel (logical view coordinates), front side only.
    pub fn wheel_wedge_center(&self, key: &DeviceKey, wedge: Wedge) -> Option<Point> {
        let d = self.front_device(key)?;
        Some(self.view_center_of(d, d.spec.chord_wheel?.wedge_centre(wedge)))
    }

    /// Centre of a button or card of a Chord Wheel panel (logical view coordinates).
    pub fn wheel_button_center(&self, key: &DeviceKey, button: WheelButton) -> Option<Point> {
        let d = self.front_device(key)?;
        Some(self.view_center(d, d.spec.chord_wheel?.rect(button)))
    }

    fn view_center_of(&self, d: &super::DeviceView, p: Point) -> Point {
        self.rack_to_view(p + d.offset())
    }

    // ------------------------------------------------------------------ presses

    /// A primary press on a part of the Chord Wheel panel of device `i`.
    pub(super) fn wheel_down(
        &mut self,
        i: usize,
        b: WheelButton,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some(state) = self.wheel_state(i) else {
            return;
        };
        let key = self.devices[i].key.clone();
        let slot = panel::selected_slot(self.devices[i].edit, state.slots);
        match b {
            WheelButton::Wheel => {
                let Some(w) = self.wedge_under_pointer(i) else {
                    return;
                };
                let make_key =
                    self.roles
                        .is_active(Role::RangeSelect, modifiers, GestureClass::ClickItem);
                if make_key {
                    if let Some(k) = panel::key_of(w) {
                        out.push(UiAction::Command(set_key_command(&key, k)));
                    }
                    return;
                }
                out.push(UiAction::WheelSetChord {
                    device: key,
                    slot: slot as u8,
                    chord: w.chord(),
                });
                self.set_edit(i, panel::next_slot(slot, state.slots));
            }
            WheelButton::Card(n) => {
                if usize::from(n) < state.slots {
                    self.set_edit(i, usize::from(n));
                }
            }
            WheelButton::Lock(n) => {
                if usize::from(n) < state.slots {
                    out.push(UiAction::WheelToggleLock {
                        device: key,
                        slot: n,
                    });
                }
            }
            WheelButton::Beats(n) => self.beats_down(i, usize::from(n), &state),
            WheelButton::SlotsDec | WheelButton::SlotsInc => {
                let next = if b == WheelButton::SlotsInc {
                    state.slots + 1
                } else {
                    state.slots.saturating_sub(1)
                }
                .clamp(1, panel::SLOTS);
                if next != state.slots {
                    out.push(UiAction::WheelSetSlots {
                        device: key,
                        slots: next as u8,
                    });
                    if self.devices[i].edit >= next {
                        self.set_edit(i, next - 1);
                    }
                }
            }
            WheelButton::Generate => out.push(UiAction::WheelGenerate(key)),
            WheelButton::Suggest => {
                let next = u8::from(self.devices[i].fx_mode == 0);
                self.devices[i].fx_mode = next;
                self.fx_modes.insert(key, next);
                self.bump(i);
            }
            WheelButton::ToLane => out.push(UiAction::WheelToChordLane(key)),
            WheelButton::ToClip => out.push(UiAction::WheelToClip(key)),
        }
    }

    /// The wedge under the pointer of the panel of device `i`.
    fn wedge_under_pointer(&self, i: usize) -> Option<Wedge> {
        let d = &self.devices[i];
        let at = self.view_to_rack(self.pointer) - d.offset();
        d.spec.chord_wheel?.wedge_at(at)
    }

    /// A right click on a wedge of the panel of device `i`: selects the device and asks the shell
    /// for the menu of that wedge (`rp` is the press in rack coordinates, `at` in view
    /// coordinates).
    pub(super) fn wheel_menu(&mut self, i: usize, rp: Point, at: Point, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let Some(state) = self.wheel_state(i) else {
            return;
        };
        let d = &self.devices[i];
        let Some(wedge) = d.spec.chord_wheel.and_then(|p| p.wedge_at(rp - d.offset())) else {
            return;
        };
        let slot = panel::selected_slot(d.edit, state.slots);
        out.push(UiAction::WheelMenu {
            device: d.key.clone(),
            entries: panel::menu_entries(slot, wedge, state.key, state.chords[slot]),
            at,
        });
    }

    // ------------------------------------------------------------------ the length of a slot

    /// A press on the length of card `n`: selects the slot and starts the drag.
    fn beats_down(&mut self, i: usize, n: usize, state: &panel::WheelState) {
        if n >= state.slots {
            return;
        }
        self.set_edit(i, n);
        let merge = self.next_merge;
        self.next_merge += 1;
        let start = beats_index(state.beats[n]);
        self.drag = Drag::Beats(BeatsDrag {
            dev: i,
            slot: n as u8,
            start_y: self.pointer.y,
            start,
            index: start,
            merge,
            moved: false,
        });
    }

    pub(super) fn beats_move(&mut self, mut d: BeatsDrag, pos: Point, out: &mut Vec<UiAction>) {
        if !d.moved && (pos.y - d.start_y).abs() < DRAG_THRESHOLD {
            self.drag = Drag::Beats(d);
            return;
        }
        d.moved = true;
        let index = panel::dragged_beats(d.start, d.start_y - pos.y);
        if index != d.index {
            let key = self.devices[d.dev].key.clone();
            out.push(param_cmd(
                &key,
                &beats_param(usize::from(d.slot)),
                index as f32,
                Some(d.merge),
            ));
            d.index = index;
        }
        self.drag = Drag::Beats(d);
        self.repaint = true;
    }

    pub(super) fn beats_up(&mut self, d: &BeatsDrag, out: &mut Vec<UiAction>) {
        if d.moved {
            out.push(UiAction::EndMerge);
        }
    }

    pub(super) fn beats_cancel(&mut self, d: &BeatsDrag, out: &mut Vec<UiAction>) {
        if d.moved && d.index != d.start {
            let key = self.devices[d.dev].key.clone();
            out.push(param_cmd(
                &key,
                &beats_param(usize::from(d.slot)),
                d.start as f32,
                Some(d.merge),
            ));
        }
        out.push(UiAction::EndMerge);
    }

    /// One wheel notch on the length of card `n` (up lengthens).
    pub(super) fn beats_step(&mut self, i: usize, n: usize, dir: f64, out: &mut Vec<UiAction>) {
        let Some(state) = self.wheel_state(i) else {
            return;
        };
        if n >= state.slots || dir == 0.0 {
            return;
        }
        let now = beats_index(state.beats[n]);
        let next = if dir > 0.0 {
            (now + 1).min(7)
        } else {
            now.saturating_sub(1)
        };
        if next != now {
            let key = self.devices[i].key.clone();
            out.push(param_cmd(&key, &beats_param(n), next as f32, None));
        }
    }
}

fn beats_index(beats: u8) -> usize {
    atomik_core::progression::beats_index(beats)
}

/// `key.root` and `key.mode` of `device` set to `key`, one batch so it is one undo step.
pub fn set_key_command(device: &DeviceKey, key: atomik_core::progression::Key) -> Command {
    let set = |param: &str, value: f32| Command::SetParam {
        key: device.clone(),
        param: param.to_string(),
        value,
        merge: None,
    };
    let minor = key.mode == atomik_core::progression::KeyMode::Minor;
    Command::Batch(vec![
        set("key.root", f32::from(key.root)),
        set("key.mode", if minor { 1.0 } else { 0.0 }),
    ])
}
