//! Modifier roles: momentary behaviours resolved from the modifiers of a pointer event, not
//! chords. The defaults per OS are data here; the user remap of roles is
//! [`Roles::with`].

use super::{Mods, Os};
use crate::event::Modifiers;

/// A momentary modifier behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Hold: the pointer acts with another tool for one gesture.
    AlternateTool,
    /// Drag: copy instead of move.
    CopyDrag,
    /// Drag: constrain to one axis.
    Constrain,
    /// Drag: fine adjustment of a control.
    FineAdjust,
    /// Click: reset a control to its default.
    ResetParam,
    /// Click: show the automation lane of a control (adding it when missing).
    EditAutomation,
    /// Click: range select.
    RangeSelect,
    /// Click: toggle an item in the selection.
    ToggleSelect,
    /// Hold: snap off while dragging.
    DisableSnap,
}

impl Role {
    /// Every role, in listing order.
    pub const ALL: [Role; 9] = [
        Role::AlternateTool,
        Role::DisableSnap,
        Role::CopyDrag,
        Role::ToggleSelect,
        Role::RangeSelect,
        Role::Constrain,
        Role::FineAdjust,
        Role::ResetParam,
        Role::EditAutomation,
    ];

    /// The source modules that act on this role. A role with none is not wired: Settings does
    /// not list it (a remap saved earlier stays in `keymap.json`), and the registry test checks
    /// that each module named here does read the role.
    pub const fn consumers(self) -> &'static [&'static str] {
        match self {
            Role::AlternateTool | Role::DisableSnap => &["viewport"],
            Role::ToggleSelect | Role::RangeSelect => &["rack/selection"],
            Role::CopyDrag => &["arrangement"],
            Role::FineAdjust => &["rack/selection", "console"],
            Role::ResetParam => &["rack/selection", "console"],
            Role::EditAutomation => &["rack/selection", "console"],
            Role::Constrain => &[],
        }
    }

    /// Whether something acts on this role when it is held.
    pub const fn is_wired(self) -> bool {
        !self.consumers().is_empty()
    }

    /// The roles Settings lists: those that are wired, in listing order.
    pub fn listed() -> impl Iterator<Item = Role> {
        Role::ALL.into_iter().filter(|r| r.is_wired())
    }

    /// The id used in `keymap.json`'s `roles`.
    pub fn id(self) -> &'static str {
        match self {
            Role::AlternateTool => "alternate_tool",
            Role::CopyDrag => "copy_drag",
            Role::Constrain => "constrain",
            Role::FineAdjust => "fine_adjust",
            Role::ResetParam => "reset_param",
            Role::EditAutomation => "edit_automation",
            Role::RangeSelect => "range_select",
            Role::ToggleSelect => "toggle_select",
            Role::DisableSnap => "disable_snap",
        }
    }
}

impl Role {
    /// The role with this id in `keymap.json`'s `roles`.
    pub fn from_id(id: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.id() == id)
    }

    /// The plain label Settings shows.
    pub fn label(self) -> &'static str {
        match self {
            Role::AlternateTool => "Alternate tool",
            Role::DisableSnap => "Disable snap",
            Role::CopyDrag => "Copy while dragging",
            Role::ToggleSelect => "Toggle selection",
            Role::RangeSelect => "Range select",
            Role::Constrain => "Constrain",
            Role::FineAdjust => "Fine adjust",
            Role::ResetParam => "Reset parameter",
            Role::EditAutomation => "Edit automation",
        }
    }
}

/// The kind of gesture a role belongs to: roles that share a modifier set are told apart by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GestureClass {
    /// A modifier held with no pointer gesture of its own.
    Hold,
    /// A click on an item (clip, note, device).
    ClickItem,
    /// A click on a control (knob, fader).
    ClickControl,
    /// A drag of an item.
    DragItem,
    /// A drag of a control.
    DragControl,
}

/// A role's modifier set and gesture class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleBinding {
    /// The role.
    pub role: Role,
    /// The modifiers (logical: `cmd` is Control on Windows and Linux, Command on macOS).
    pub mods: Mods,
    /// The gesture class.
    pub class: GestureClass,
}

/// The role table of one OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    os: Os,
    bindings: Vec<RoleBinding>,
    /// Roles this table does not act on (the legacy profile has no alternate tool). They stay
    /// listed in [`Self::bindings`] so Settings shows them.
    off: Vec<Role>,
}

const fn m(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        cmd,
        ctrl,
        alt,
        shift,
        meta: false,
    }
}

impl Roles {
    /// The defaults of for `os`.
    pub fn defaults(os: Os) -> Roles {
        use GestureClass as G;
        use Role as R;
        let mac = os == Os::Mac;
        let shift = m(false, false, false, true);
        let cmd = m(true, false, false, false);
        let b = |role, mods, class| RoleBinding { role, mods, class };
        let bindings = vec![
            b(
                R::AlternateTool,
                if mac {
                    m(false, true, false, false)
                } else {
                    m(false, false, true, false)
                },
                G::Hold,
            ),
            b(R::DisableSnap, m(true, false, true, false), G::Hold),
            b(
                R::CopyDrag,
                if mac {
                    m(false, false, true, false)
                } else {
                    cmd
                },
                G::DragItem,
            ),
            b(R::ToggleSelect, cmd, G::ClickItem),
            b(R::RangeSelect, shift, G::ClickItem),
            b(R::Constrain, shift, G::DragItem),
            b(R::FineAdjust, shift, G::DragControl),
            b(R::ResetParam, cmd, G::ClickControl),
            b(
                R::EditAutomation,
                m(false, false, true, false),
                G::ClickControl,
            ),
        ];
        Roles {
            os,
            bindings,
            off: Vec::new(),
        }
    }

    /// The roles of the `legacy` profile: the earlier behaviour, so Alt alone disables
    /// snapping while dragging and there is no alternate tool (Ctrl and Cmd keep their
    /// earlier gesture meanings).
    pub fn legacy(os: Os) -> Roles {
        let alt = m(false, false, true, false);
        let mut roles = Roles::defaults(os).with(Role::DisableSnap, alt);
        roles.off.push(Role::AlternateTool);
        roles
    }

    /// Whether the table acts on `role` (false for the roles a profile switches off).
    pub fn is_on(&self, role: Role) -> bool {
        !self.off.contains(&role)
    }

    /// The table with one role rebound.
    pub fn with(mut self, role: Role, mods: Mods) -> Roles {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.role == role) {
            b.mods = mods;
        }
        self
    }

    /// The other role that already uses `mods` for the same kind of gesture as `role`, if any
    /// (two roles of one OS never share both modifier set and class).
    pub fn clash(&self, role: Role, mods: Mods) -> Option<Role> {
        let class = self.binding(role)?.class;
        self.bindings
            .iter()
            .find(|b| b.role != role && self.is_on(b.role) && b.class == class && b.mods == mods)
            .map(|b| b.role)
    }

    /// The bindings, in [`Role::ALL`] order.
    pub fn bindings(&self) -> &[RoleBinding] {
        &self.bindings
    }

    /// The binding of a role.
    pub fn binding(&self, role: Role) -> Option<&RoleBinding> {
        self.bindings.iter().find(|b| b.role == role)
    }

    /// The roles active for a gesture of `class` with `modifiers` held: strict (the modifier
    /// set must be exactly the role's, so Alt alone and Ctrl+Alt are different roles).
    pub fn active(&self, modifiers: Modifiers, class: GestureClass) -> Vec<Role> {
        self.bindings
            .iter()
            .filter(|b| b.class == class && self.is_on(b.role) && self.matches(b.mods, modifiers))
            .map(|b| b.role)
            .collect()
    }

    /// Whether `role` is active for `class` and `modifiers`.
    pub fn is_active(&self, role: Role, modifiers: Modifiers, class: GestureClass) -> bool {
        self.active(modifiers, class).contains(&role)
    }

    fn matches(&self, mods: Mods, held: Modifiers) -> bool {
        let mac = self.os == Os::Mac;
        let want_ctrl = mods.ctrl || (mods.cmd && !mac);
        let want_meta = mods.meta || (mods.cmd && mac);
        held.ctrl == want_ctrl
            && held.meta == want_meta
            && held.alt == mods.alt
            && held.shift == mods.shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Modifiers {
        Modifiers {
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    #[test]
    fn no_two_roles_of_one_os_share_modifiers_and_class() {
        for os in Os::ALL {
            let r = Roles::defaults(os);
            let b = r.bindings();
            assert_eq!(b.len(), Role::ALL.len());
            for (i, x) in b.iter().enumerate() {
                for y in &b[i + 1..] {
                    assert!(
                        !(x.mods == y.mods && x.class == y.class),
                        "{os:?}: {:?} and {:?}",
                        x.role,
                        y.role
                    );
                }
            }
            for role in Role::ALL {
                assert!(r.binding(role).is_some(), "{os:?} {role:?}");
            }
        }
    }

    #[test]
    fn windows_and_linux_defaults() {
        for os in [Os::Windows, Os::Linux] {
            let r = Roles::defaults(os);
            assert!(r.is_active(
                Role::AlternateTool,
                held(false, true, false, false),
                GestureClass::Hold
            ));
            assert!(r.is_active(
                Role::DisableSnap,
                held(true, true, false, false),
                GestureClass::Hold
            ));
            assert!(r.is_active(
                Role::CopyDrag,
                held(true, false, false, false),
                GestureClass::DragItem
            ));
            assert!(r.is_active(
                Role::ToggleSelect,
                held(true, false, false, false),
                GestureClass::ClickItem
            ));
            assert!(r.is_active(
                Role::RangeSelect,
                held(false, false, true, false),
                GestureClass::ClickItem
            ));
            assert!(r.is_active(
                Role::ResetParam,
                held(true, false, false, false),
                GestureClass::ClickControl
            ));
            assert!(r.is_active(
                Role::FineAdjust,
                held(false, false, true, false),
                GestureClass::DragControl
            ));
        }
    }

    #[test]
    fn macos_defaults_use_command_option_and_control() {
        let r = Roles::defaults(Os::Mac);
        assert!(r.is_active(
            Role::AlternateTool,
            held(true, false, false, false),
            GestureClass::Hold
        ));
        assert!(r.is_active(
            Role::DisableSnap,
            held(false, true, false, true),
            GestureClass::Hold
        ));
        assert!(r.is_active(
            Role::CopyDrag,
            held(false, true, false, false),
            GestureClass::DragItem
        ));
        assert!(r.is_active(
            Role::ToggleSelect,
            held(false, false, false, true),
            GestureClass::ClickItem
        ));
        assert!(!r.is_active(
            Role::ToggleSelect,
            held(true, false, false, false),
            GestureClass::ClickItem
        ));
        assert!(!r.is_active(
            Role::AlternateTool,
            held(false, false, false, true),
            GestureClass::Hold
        ));
    }

    #[test]
    fn alt_alone_and_ctrl_alt_are_different_holds() {
        let r = Roles::defaults(Os::Windows);
        assert_eq!(
            r.active(held(false, true, false, false), GestureClass::Hold),
            [Role::AlternateTool]
        );
        assert_eq!(
            r.active(held(true, true, false, false), GestureClass::Hold),
            [Role::DisableSnap]
        );
        assert!(r.active(Modifiers::NONE, GestureClass::Hold).is_empty());
    }

    #[test]
    fn a_role_can_be_rebound() {
        let r = Roles::defaults(Os::Windows).with(
            Role::AlternateTool,
            Mods {
                shift: true,
                alt: true,
                ..Mods::default()
            },
        );
        assert!(r.is_active(
            Role::AlternateTool,
            held(false, true, true, false),
            GestureClass::Hold
        ));
        assert!(!r.is_active(
            Role::AlternateTool,
            held(false, true, false, false),
            GestureClass::Hold
        ));
    }

    #[test]
    fn the_legacy_roles_keep_alt_as_snap_off_and_have_no_alternate_tool() {
        for os in Os::ALL {
            let r = Roles::legacy(os);
            assert_eq!(
                r.active(held(false, true, false, false), GestureClass::Hold),
                [Role::DisableSnap],
                "{os:?}"
            );
            assert!(!r.is_on(Role::AlternateTool));
            assert!(
                r.active(held(true, true, false, false), GestureClass::Hold)
                    .is_empty()
            );
            assert!(Roles::defaults(os).is_on(Role::AlternateTool));
        }
    }

    /// The sources of the consumer modules, for the registry test.
    fn source_of(module: &str) -> &'static str {
        match module {
            "viewport" => include_str!("../viewport.rs"),
            "rack/selection" => include_str!("../rack/selection.rs"),
            "arrangement" => include_str!("../arrangement.rs"),
            "console" => include_str!("../console.rs"),
            other => panic!("no source registered for {other}"),
        }
    }

    #[test]
    fn every_listed_role_has_a_consumer_that_reads_it() {
        let listed: Vec<Role> = Role::listed().collect();
        assert!(listed.len() >= 6, "{listed:?}");
        for role in listed {
            let name = format!("Role::{role:?}");
            for module in role.consumers() {
                assert!(
                    source_of(module).contains(&name),
                    "{module} does not read {name}"
                );
            }
        }
    }

    #[test]
    fn the_roles_without_a_consumer_are_not_listed_but_stay_known() {
        let unlisted: Vec<Role> = Role::ALL.into_iter().filter(|r| !r.is_wired()).collect();
        assert_eq!(unlisted, [Role::Constrain]);
        for role in unlisted {
            assert!(!Role::listed().any(|r| r == role));
            assert_eq!(Role::from_id(role.id()), Some(role));
        }
    }

    #[test]
    fn the_snap_off_default_on_windows_is_ctrl_alt_which_altgr_also_sends() {
        // AltGr reports Ctrl+Alt on many European layouts, so on Windows typing with it
        // while dragging switches snapping off. The default stays; Settings remaps it.
        let r = Roles::defaults(Os::Windows);
        assert_eq!(
            r.binding(Role::DisableSnap).unwrap().mods,
            m(true, false, true, false)
        );
        let altgr = held(true, true, false, false);
        assert!(r.is_active(Role::DisableSnap, altgr, GestureClass::Hold));
        let remapped = r.with(Role::DisableSnap, m(false, false, true, true));
        assert!(!remapped.is_active(Role::DisableSnap, altgr, GestureClass::Hold));
    }

    #[test]
    fn role_ids_are_unique() {
        let mut ids: Vec<_> = Role::ALL.iter().map(|r| r.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Role::ALL.len());
    }
}
