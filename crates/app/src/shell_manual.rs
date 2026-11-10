//! The manual in the shell (Help > Manual): a large floating panel over the window with the
//! user documents ([`crate::manual`]), a table of contents, search and Back and Forward.
//!
//! The panel takes every event while it is open, like the command palette. It opens at the top
//! of the manual, or at a section: the "More" link of the Help Mode panel and a "Manual:
//! <heading>" row of the command palette open it there. A link that leaves the manual opens the
//! page of the repository it names (a fixed address, the way Help > Documentation opens the
//! project page); a link to anything else says it is not part of the manual.

use super::*;
use atomik_ui::ManualAction;
use atomik_ui::manual::Location;

impl<B: AudioBackend> Shell<B> {
    /// Whether the manual is open.
    pub fn manual_open(&self) -> bool {
        self.manual.is_some()
    }

    /// The manual panel, while it is open.
    pub fn manual_view(&self) -> Option<&ManualView> {
        self.manual.as_ref()
    }

    /// Opens the manual (at the top), or closes it when it is open.
    pub(super) fn toggle_manual(&mut self) {
        if self.manual.is_some() {
            self.manual = None;
            self.repaint = true;
        } else {
            self.open_manual(None);
        }
    }

    /// Opens the manual at the section called `section` (at the top for `None` or a name the
    /// manual does not have). An open manual goes there.
    pub(super) fn open_manual(&mut self, section: Option<&str>) {
        let view = self.manual_view_mut();
        if let Some(section) = section {
            view.goto_section(section);
        }
        self.repaint = true;
    }

    /// Opens the manual at `at` (a heading found in it).
    pub(super) fn open_manual_at(&mut self, at: Location) {
        self.manual_view_mut().goto(at);
        self.repaint = true;
    }

    /// The open manual, made if it was closed.
    fn manual_view_mut(&mut self) -> &mut ManualView {
        let theme = self.core.view().theme().clone();
        let scale = self.scale();
        let bounds = self.plugin_modal_rect();
        self.manual.get_or_insert_with(|| {
            let mut view = ManualView::new(theme, crate::manual::manual());
            view.set_scale(scale);
            view.set_bounds(bounds);
            view
        })
    }

    /// What the manual asked for.
    pub(super) fn manual_action(&mut self, action: ManualAction) -> Vec<ShellRequest> {
        self.repaint = true;
        match action {
            ManualAction::Close => {
                self.manual = None;
                Vec::new()
            }
            ManualAction::Open(target) => match crate::manual::outside_url(&target) {
                Some(url) => vec![ShellRequest::OpenUrl(url)],
                None => {
                    self.notify("That link is not part of the manual");
                    Vec::new()
                }
            },
        }
    }
}
