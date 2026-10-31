//! Native macOS menus (muda), built from the same [`crate::commands::menus`] model as the
//! in-window menu bar. `Menu::init_for_nsapp` is safe, unlike the Windows attach.

use crate::commands::{
    AppCommand, Menu as MenuModel, MenuEntry, Submenu as SubModel, native_accelerator_text,
};
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// The app's native menu bar.
pub struct NativeMenus {
    _menu: Menu,
    items: Vec<(muda::MenuId, AppCommand, Item)>,
    /// The submenus whose entries change while the app runs (File > Open Recent).
    dynamic: Vec<DynamicSubmenu>,
}

/// A submenu that is rebuilt when its entries change: muda cannot relabel an item in place,
/// so the whole list is replaced.
struct DynamicSubmenu {
    label: &'static str,
    submenu: Submenu,
    /// What the submenu shows now, to rebuild only when the model says otherwise: the label,
    /// command and enabled state of each entry (`None` for a separator).
    shown: Vec<Option<(String, AppCommand, bool)>>,
    /// The items of the entries, to find the command a click chose.
    chosen: Vec<(muda::MenuId, AppCommand, MenuItem)>,
}

/// The entries of `sub` as [`DynamicSubmenu::shown`] keeps them.
fn shape(sub: &SubModel) -> Vec<Option<(String, AppCommand, bool)>> {
    sub.entries
        .iter()
        .map(|e| match e {
            MenuEntry::Item(i) => Some((i.label.to_string(), i.command, i.enabled)),
            _ => None,
        })
        .collect()
}

impl DynamicSubmenu {
    /// Replaces the entries of the native submenu with those of `model`.
    fn rebuild(&mut self, model: &SubModel) -> Option<()> {
        while self.submenu.remove_at(0).is_some() {}
        self.chosen.clear();
        for entry in &model.entries {
            match entry {
                MenuEntry::Item(item) => {
                    let i = MenuItem::new(item.label.as_ref(), item.enabled, None);
                    self.submenu.append(&i).ok()?;
                    self.chosen.push((i.id().clone(), item.command, i));
                }
                _ => self.submenu.append(&PredefinedMenuItem::separator()).ok()?,
            }
        }
        self.shown = shape(model);
        Some(())
    }
}

enum Item {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

impl std::fmt::Debug for NativeMenus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMenus")
            .field("items", &self.items.len())
            .field("dynamic", &self.dynamic.len())
            .finish()
    }
}

/// `Cmd+Shift+S` (the derived shortcut text on macOS) → `CmdOrCtrl+Shift+S`; shortcuts that
/// do not start with the primary modifier get no accelerator (Tab and F-keys stay with the
/// window).
fn accelerator(shortcut: Option<&str>) -> Option<Accelerator> {
    native_accelerator_text(shortcut?)?.parse().ok()
}

impl NativeMenus {
    /// Builds the menus and installs them as the application menu bar.
    pub fn attach(model: &[MenuModel]) -> Option<Self> {
        let menu = Menu::new();
        let mut items = Vec::new();
        let mut dynamic = Vec::new();
        let app = Submenu::new("Atomik Rack", true);
        let quit = MenuItem::new(
            "Quit Atomik Rack",
            true,
            "CmdOrCtrl+Q".parse::<Accelerator>().ok(),
        );
        app.append(&quit).ok()?;
        items.push((quit.id().clone(), AppCommand::Quit, Item::Plain(quit)));
        menu.append(&app).ok()?;
        for m in model {
            let sub = Submenu::new(m.title, true);
            for entry in &m.entries {
                match entry {
                    MenuEntry::Separator => {
                        sub.append(&PredefinedMenuItem::separator()).ok()?;
                    }
                    MenuEntry::Submenu(model) => {
                        let mut d = DynamicSubmenu {
                            label: model.label,
                            submenu: Submenu::new(model.label, true),
                            shown: Vec::new(),
                            chosen: Vec::new(),
                        };
                        d.rebuild(model)?;
                        sub.append(&d.submenu).ok()?;
                        dynamic.push(d);
                    }
                    MenuEntry::Item(item) => {
                        if item.command == AppCommand::Quit {
                            continue; // in the app menu
                        }
                        let accel = accelerator(item.shortcut.as_deref());
                        match item.checked {
                            Some(checked) => {
                                let c = CheckMenuItem::new(
                                    item.label.as_ref(),
                                    item.enabled,
                                    checked,
                                    accel,
                                );
                                sub.append(&c).ok()?;
                                items.push((c.id().clone(), item.command, Item::Check(c)));
                            }
                            None => {
                                let i = MenuItem::new(item.label.as_ref(), item.enabled, accel);
                                sub.append(&i).ok()?;
                                items.push((i.id().clone(), item.command, Item::Plain(i)));
                            }
                        }
                    }
                }
            }
            menu.append(&sub).ok()?;
        }
        menu.init_for_nsapp();
        Some(Self {
            _menu: menu,
            items,
            dynamic,
        })
    }

    /// Updates check marks and enabled states, and rebuilds the submenus whose entries changed
    /// (the recent songs).
    pub fn update(&mut self, model: &[MenuModel]) {
        for d in &mut self.dynamic {
            let wanted = model.iter().flat_map(|m| &m.entries).find_map(|e| match e {
                MenuEntry::Submenu(s) if s.label == d.label => Some(s),
                _ => None,
            });
            if let Some(wanted) = wanted
                && shape(wanted) != d.shown
            {
                d.rebuild(wanted);
            }
        }
        for (_, command, item) in &self.items {
            let Some(m) = model
                .iter()
                .flat_map(|m| m.items())
                .find(|i| i.command == *command)
            else {
                continue;
            };
            match item {
                Item::Plain(i) => {
                    i.set_enabled(m.enabled);
                    i.set_text(m.label.as_ref());
                }
                Item::Check(c) => {
                    c.set_enabled(m.enabled);
                    c.set_checked(m.checked == Some(true));
                }
            }
        }
    }

    /// Commands chosen since the last call.
    pub fn poll(&self) -> Vec<AppCommand> {
        let mut out = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let chosen = self
                .items
                .iter()
                .map(|(id, command, _)| (id, command))
                .chain(
                    self.dynamic
                        .iter()
                        .flat_map(|d| d.chosen.iter().map(|(id, command, _)| (id, command))),
                )
                .find(|(id, _)| **id == event.id);
            if let Some((_, command)) = chosen {
                out.push(*command);
            }
        }
        out
    }
}
