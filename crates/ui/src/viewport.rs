//! The state the arrangement and the piano roll share for the daily keys: the active tool, the modifier roles, follow, the snap toggle and the zoom toggle.
//!
//! Pure data and rules, no painting and no window system: a view owns one [`ViewportState`] and
//! asks it what a gesture means (the effective tool with the alternate-tool role held, whether
//! the disable-snap role switches snapping off) and what the view must do (page to the
//! playhead, remember the snap or the zoom to come back to).

use crate::event::Modifiers;
use crate::keymap::{GestureClass, Os, Role, Roles};
use crate::timeline::{Meter, Snap};

/// The tools of the tool palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Select: the modeless gestures (click selects, drag moves or resizes, band select); it draws only on a double-click on the piano roll's empty grid.
    #[default]
    Select,
    /// Pencil: a click draws.
    Pencil,
    /// Eraser: a click deletes the clip or note under the pointer.
    Eraser,
    /// Razor: a click splits the clip or note under the pointer at the snapped position.
    Razor,
    /// Speaker: pressing on an audio clip plays the audio under the pointer as it is in its file, unprocessed, until the button is released. Only the arrangement uses it.
    Speaker,
    /// Mute: a click on a clip toggles its mute. Only the arrangement uses it.
    Mute,
    /// Magnifier: a click zooms in around the pointer (Shift zooms out) and a drag zooms to the rectangle. Only the arrangement uses it.
    Magnifier,
    /// Hand: a drag scrolls the view in both directions and edits nothing. Only the arrangement uses it.
    Hand,
}

impl Tool {
    /// Every tool, in palette order.
    pub const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Pencil,
        Tool::Eraser,
        Tool::Razor,
        Tool::Speaker,
        Tool::Mute,
        Tool::Magnifier,
        Tool::Hand,
    ];

    /// The tools of a note editor, in palette order: the editing tools only.
    pub const NOTE: [Tool; 4] = [Tool::Select, Tool::Pencil, Tool::Eraser, Tool::Razor];

    /// The registry command that selects this tool.
    pub fn command_id(self) -> &'static str {
        match self {
            Tool::Select => "tool.select",
            Tool::Pencil => "tool.pencil",
            Tool::Eraser => "tool.eraser",
            Tool::Razor => "tool.razor",
            Tool::Speaker => "tool.speaker",
            Tool::Mute => "tool.mute",
            Tool::Magnifier => "tool.magnifier",
            Tool::Hand => "tool.hand",
        }
    }

    /// The tool a `tool.*` command id selects.
    pub fn from_command(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.command_id() == id)
    }

    /// The short label on the toolbar.
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "SEL",
            Tool::Pencil => "PEN",
            Tool::Eraser => "ERS",
            Tool::Razor => "CUT",
            Tool::Speaker => "SPK",
            Tool::Mute => "MUT",
            Tool::Magnifier => "MAG",
            Tool::Hand => "HND",
        }
    }

    /// The tool the alternate-tool role switches to: the pairs are Select and Pencil, and
    /// Magnifier and Hand; every other tool switches to Select.
    pub fn alternate(self) -> Tool {
        match self {
            Tool::Select => Tool::Pencil,
            Tool::Magnifier => Tool::Hand,
            Tool::Hand => Tool::Magnifier,
            _ => Tool::Select,
        }
    }
}

/// How a move or a nudge meets the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapMode {
    /// The item lands on a grid line (what an item off the grid snaps to).
    #[default]
    Absolute,
    /// The item moves by whole grid steps and keeps its distance from the grid.
    Relative,
}

impl SnapMode {
    /// The mode a `sequencer.set_snap_mode` argument names.
    pub fn from_arg(arg: &str) -> Option<SnapMode> {
        match arg {
            "absolute" => Some(SnapMode::Absolute),
            "relative" => Some(SnapMode::Relative),
            _ => None,
        }
    }
}

/// Tool, roles, follow, snap and zoom toggles of one time view.
#[derive(Debug, Clone)]
pub struct ViewportState {
    tool: Tool,
    roles: Roles,
    held: Modifiers,
    follow: bool,
    paused: bool,
    playing: bool,
    snap_stash: Option<Snap>,
    snap_mode: SnapMode,
    zoomed_to_selection: bool,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportState {
    /// Select tool, the host's default roles, follow off.
    pub fn new() -> Self {
        Self {
            tool: Tool::Select,
            roles: Roles::defaults(Os::host()),
            held: Modifiers::NONE,
            follow: false,
            paused: false,
            playing: false,
            snap_stash: None,
            snap_mode: SnapMode::Absolute,
            zoomed_to_selection: false,
        }
    }

    /// The active tool.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Selects a tool; true when it changed.
    pub fn set_tool(&mut self, tool: Tool) -> bool {
        let changed = tool != self.tool;
        self.tool = tool;
        changed
    }

    /// The role table in use.
    pub fn roles(&self) -> &Roles {
        &self.roles
    }

    /// Replaces the role table (the shell pushes it on every keymap change); true when the
    /// effective tool changed with the modifiers held now.
    pub fn set_roles(&mut self, roles: Roles) -> bool {
        let before = self.effective_tool(self.held);
        self.roles = roles;
        before != self.effective_tool(self.held)
    }

    /// The modifiers held now (from `ModifiersChanged`); true when the effective tool changed,
    /// so the toolbar repaints without a pointer move.
    pub fn set_held(&mut self, held: Modifiers) -> bool {
        let before = self.effective_tool(self.held);
        self.held = held;
        before != self.effective_tool(held)
    }

    /// The tool a gesture with `modifiers` uses: the alternate tool while the role is held.
    pub fn effective_tool(&self, modifiers: Modifiers) -> Tool {
        if self
            .roles
            .is_active(Role::AlternateTool, modifiers, GestureClass::Hold)
        {
            self.tool.alternate()
        } else {
            self.tool
        }
    }

    /// The tool shown as active on the toolbar (the modifiers held now count).
    pub fn shown_tool(&self) -> Tool {
        self.effective_tool(self.held)
    }

    /// Whether `modifiers` hold the disable-snap role.
    pub fn snap_disabled(&self, modifiers: Modifiers) -> bool {
        self.roles
            .is_active(Role::DisableSnap, modifiers, GestureClass::Hold)
    }

    /// The grid step of a gesture: the snap step unless the disable-snap role is held.
    pub fn step(
        &self,
        snap: Snap,
        meter: Meter,
        px_per_tick: f64,
        modifiers: Modifiers,
    ) -> Option<i64> {
        if self.snap_disabled(modifiers) {
            None
        } else {
            snap.step_zoomed(meter, px_per_tick)
        }
    }

    /// Whether moves snap to grid lines or by whole steps.
    pub fn snap_mode(&self) -> SnapMode {
        self.snap_mode
    }

    /// Sets the snap mode.
    pub fn set_snap_mode(&mut self, mode: SnapMode) {
        self.snap_mode = mode;
    }

    /// Switches snapping off and back to the value it had; returns the new snap.
    pub fn toggle_snap(&mut self, current: Snap) -> Snap {
        if current == Snap::Off {
            self.snap_stash.take().unwrap_or_default()
        } else {
            self.snap_stash = Some(current);
            Snap::Off
        }
    }

    /// Whether follow is on.
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Switches follow on or off (also ends a pause); returns the new state.
    pub fn set_follow(&mut self, on: bool) -> bool {
        self.follow = on;
        self.paused = false;
        on
    }

    /// Follow toggled by its key: on again after a manual scroll paused it.
    pub fn toggle_follow(&mut self) -> bool {
        let on = !self.follow || self.paused;
        self.set_follow(on)
    }

    /// The transport runs or stops; stopping ends a pause.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        if !playing {
            self.paused = false;
        }
    }

    /// The user scrolled by hand: while playing, follow waits for the next toggle or stop.
    pub fn manual_scroll(&mut self) {
        if self.follow && self.playing {
            self.paused = true;
        }
    }

    /// Whether the view pages to the playhead now.
    pub fn following(&self) -> bool {
        self.follow && self.playing && !self.paused
    }

    /// Whether follow is paused by a manual scroll.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Whether the view is zoomed to the selection (Z again fits the whole song or clip).
    pub fn zoomed_to_selection(&self) -> bool {
        self.zoomed_to_selection
    }

    /// Records that the view is (or no longer is) zoomed to the selection.
    pub fn set_zoomed_to_selection(&mut self, on: bool) {
        self.zoomed_to_selection = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn tools_have_commands_and_alternates() {
        for t in Tool::ALL {
            assert_eq!(Tool::from_command(t.command_id()), Some(t));
            assert!(!t.label().is_empty());
        }
        assert_eq!(Tool::from_command("tool.nope"), None);
        assert_eq!(Tool::Select.alternate(), Tool::Pencil);
        for t in [
            Tool::Pencil,
            Tool::Eraser,
            Tool::Razor,
            Tool::Speaker,
            Tool::Mute,
        ] {
            assert_eq!(t.alternate(), Tool::Select);
        }
        assert_eq!(Tool::Magnifier.alternate(), Tool::Hand);
        assert_eq!(Tool::Hand.alternate(), Tool::Magnifier);
    }

    #[test]
    fn the_alternate_tool_role_switches_the_tool_for_the_held_gesture_only() {
        let mut v = ViewportState::new();
        v.set_roles(Roles::defaults(Os::Windows));
        assert_eq!(v.effective_tool(Modifiers::NONE), Tool::Select);
        assert_eq!(v.effective_tool(alt()), Tool::Pencil);
        assert_eq!(v.tool(), Tool::Select, "the stored tool does not change");
        v.set_tool(Tool::Razor);
        assert_eq!(v.effective_tool(alt()), Tool::Select);
        // Ctrl+Alt is the disable-snap role, not the alternate tool.
        let both = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(v.effective_tool(both), Tool::Razor);
        assert!(v.snap_disabled(both));
        assert!(!v.snap_disabled(alt()));
    }

    #[test]
    fn modifier_changes_report_a_toolbar_change_only_when_the_shown_tool_changes() {
        let mut v = ViewportState::new();
        v.set_roles(Roles::defaults(Os::Windows));
        assert!(v.set_held(alt()));
        assert_eq!(v.shown_tool(), Tool::Pencil);
        assert!(!v.set_held(alt()), "no change, no repaint");
        assert!(v.set_held(Modifiers::NONE));
        assert_eq!(v.shown_tool(), Tool::Select);
    }

    #[test]
    fn snap_toggle_remembers_the_value() {
        let mut v = ViewportState::new();
        assert_eq!(v.toggle_snap(Snap::Eighth), Snap::Off);
        assert_eq!(v.toggle_snap(Snap::Off), Snap::Eighth);
        assert_eq!(
            v.toggle_snap(Snap::Off),
            Snap::default(),
            "nothing stored: the default"
        );
    }

    #[test]
    fn follow_pauses_on_a_manual_scroll_while_playing_and_resumes_on_toggle_or_stop() {
        let mut v = ViewportState::new();
        v.set_follow(true);
        assert!(!v.following(), "not while stopped");
        v.set_playing(true);
        assert!(v.following());
        v.manual_scroll();
        assert!(!v.following() && v.paused());
        assert!(v.toggle_follow(), "the toggle resumes");
        assert!(v.following());
        v.manual_scroll();
        v.set_playing(false);
        v.set_playing(true);
        assert!(v.following(), "stopping ends the pause");
        assert!(!v.toggle_follow(), "toggling again switches it off");
    }
}
