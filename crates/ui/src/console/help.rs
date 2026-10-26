//! The tooltips of the console: what the button under the pointer does.

use super::{ConsoleButton, ConsoleView, Drag, Hit, STRIP_W, Target, insert_b_rect, insert_x_rect};
use crate::console_layout::CtrlKind;
use crate::console_vca::{VcaPart, vca_layout};
use crate::tooltip::Help;
use std::borrow::Cow;
use vello::kurbo::{Point, Rect, Vec2};

/// The help of a toolbar, snapshot bar or master button.
fn button(b: ConsoleButton, rect: Rect) -> Help {
    let help = |text: &'static str| Help::new(rect, text);
    match b {
        ConsoleButton::FoldAll => help("Fold every section of the strips, or unfold them all"),
        ConsoleButton::AddChannel => help("Add a mixer channel for an instrument that has none"),
        ConsoleButton::Compensation => {
            help("Delay compensation: line the strips up by their latency")
                .with_command("mixer.toggle_compensation")
        }
        ConsoleButton::Arrange => {
            help("Back to the arrangement").with_command("view.toggle_console")
        }
        ConsoleButton::Maximise => {
            help("Maximise the mixer, or restore it").with_command("view.maximise_console")
        }
        ConsoleButton::ClearSolos => help("Clear every solo"),
        ConsoleButton::ClearMutes => help("Clear every mute"),
        ConsoleButton::StoreSnapshot => {
            help("Store the mix as a new snapshot").with_command("mixer.store_snapshot")
        }
        ConsoleButton::Compare => help("Compare the mix with the snapshot: flips between the two")
            .with_command("mixer.compare_snapshot"),
        ConsoleButton::EndCompare => {
            help("End the compare and keep the live mix").with_command("mixer.end_compare")
        }
    }
}

impl ConsoleView {
    /// What the button under `pos` does, for its tooltip (`None` while a menu is open or a
    /// gesture runs, and away from any button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.menu.is_some() || !matches!(self.drag, Drag::None) || self.rename.is_some() {
            return None;
        }
        let top = self.area_top();
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit(pos) {
            Hit::Button(b) => Some(button(b, self.toolbar_rect(b)?)),
            Hit::Snap(i) => {
                let rect = self
                    .snapshot_layout()
                    .chips
                    .iter()
                    .find(|(k, _)| *k == i)
                    .map(|(_, r)| *r)?;
                help(
                    rect,
                    "Recall this snapshot: double-click to rename, right-click for more",
                )
            }
            Hit::Ctrl(t, c) => {
                let (pc, p, _) = self.ctrl_of(t, c)?;
                let rect = self.ctrl_rect(t, c)?;
                let text = match pc.kind {
                    CtrlKind::Button => format!("{}: click to switch on or off", p.name),
                    CtrlKind::Choice => format!("{}: click to step through the choices", p.name),
                    CtrlKind::Knob | CtrlKind::Slider | CtrlKind::Fader => return None,
                };
                Some(Help::new(rect, Cow::Owned(text)))
            }
            Hit::Header(_) => {
                let (i, s) = self.strips.iter().enumerate().find(|(i, _)| {
                    (self.strip_x(*i)..self.strip_x(*i) + STRIP_W).contains(&pos.x)
                })?;
                let l = self.layout_of(s.desc)?;
                let x = self.strip_x(i);
                let sec = l.sections.iter().find(|sec| {
                    let dy = if l.is_pinned(sec.id) {
                        0.0
                    } else {
                        -self.scroll.y
                    };
                    (sec.header + Vec2::new(x, top + dy)).contains(pos)
                })?;
                let dy = if l.is_pinned(sec.id) {
                    0.0
                } else {
                    -self.scroll.y
                };
                help(
                    sec.header + Vec2::new(x, top + dy),
                    "Fold or unfold this section of every strip",
                )
            }
            Hit::OutputSel(i) => {
                let l = self.layout_of(self.strips.get(i)?.desc)?;
                help(
                    l.output? + Vec2::new(self.strip_x(i), top),
                    "Where this strip's output goes: click to choose",
                )
            }
            Hit::Picker(slot) => {
                let l = self.master_layout.as_ref()?;
                let (_, r) = l.return_pickers.iter().find(|(s, _)| *s == slot)?;
                help(
                    *r + Vec2::new(self.master_x()?, top - self.master_scroll),
                    "Choose the effect for this return",
                )
            }
            Hit::Insert(t, slot) => help(
                self.slot_rect(t, slot)?,
                "Insert slot: click to choose or add an effect",
            ),
            Hit::InsertRemove(t, slot) => help(
                insert_x_rect(self.slot_rect(t, slot)?),
                "Remove this insert effect",
            ),
            Hit::InsertBypass(t, slot) => help(
                insert_b_rect(self.slot_rect(t, slot)?),
                "Bypass this insert effect",
            ),
            Hit::Vca(v, part) => {
                let l = vca_layout(self.area_height());
                let off = Vec2::new(self.vca_x(v), top);
                let (r, text) = match part {
                    VcaPart::Plate => (
                        l.plate,
                        "Select the group's members: double-click to rename, right-click for more",
                    ),
                    VcaPart::Mute => (l.mute, "Mute the group"),
                    VcaPart::Solo => (l.solo, "Solo the group"),
                    VcaPart::Readout => (l.lcd, "Group level offset: double-click to reset"),
                    VcaPart::Fader => return None,
                };
                help(r + off, text)
            }
            Hit::Name(_) | Hit::Menu(_) | Hit::MenuArea | Hit::Empty => None,
        }
    }

    /// The slot rectangle of an insert chain, in window coordinates.
    fn slot_rect(&self, t: Target, slot: usize) -> Option<Rect> {
        let top = self.area_top();
        match t {
            Target::Strip(i) => {
                let l = self.layout_of(self.strips.get(i)?.desc)?;
                Some(*l.insert_slots.get(slot)? + Vec2::new(self.strip_x(i), top - self.scroll.y))
            }
            Target::Master => {
                let l = self.master_layout.as_ref()?;
                Some(
                    *l.insert_slots.get(slot)?
                        + Vec2::new(self.master_x()?, top - self.master_scroll),
                )
            }
        }
    }
}
