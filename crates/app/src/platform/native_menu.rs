//! Native macOS menus (muda), built from the same [`crate::commands::menus`] model as the
//! in-window menu bar. `Menu::init_for_nsapp` is safe, unlike the Windows attach.

use crate::commands::{AppCommand, Menu as MenuModel, MenuEntry, native_accelerator_text};
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// The app's native menu bar.
pub struct NativeMenus {
    _menu: Menu,
    items: Vec<(muda::MenuId, AppCommand, Item)>,
}

enum Item {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

impl std::fmt::Debug for NativeMenus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMenus")
            .field("items", &self.items.len())
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
                    MenuEntry::Item(item) => {
                        if item.command == AppCommand::Quit {
                            continue; // in the app menu
                        }
                        let accel = accelerator(item.shortcut.as_deref());
                        match item.checked {
                            Some(checked) => {
                                let c =
                                    CheckMenuItem::new(item.label, item.enabled, checked, accel);
                                sub.append(&c).ok()?;
                                items.push((c.id().clone(), item.command, Item::Check(c)));
                            }
                            None => {
                                let i = MenuItem::new(item.label, item.enabled, accel);
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
        Some(Self { _menu: menu, items })
    }

    /// Updates check marks and enabled states.
    pub fn update(&mut self, model: &[MenuModel]) {
        for (_, command, item) in &self.items {
            let Some(m) = model
                .iter()
                .flat_map(|m| m.items())
                .find(|i| i.command == *command)
            else {
                continue;
            };
            match item {
                Item::Plain(i) => i.set_enabled(m.enabled),
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
            if let Some((_, command, _)) = self.items.iter().find(|(id, ..)| *id == event.id) {
                out.push(*command);
            }
        }
        out
    }
}
