//! The Separate Stems dialog in the shell: the options, the consent to download the models, the
//! progress and the failure.
//!
//! It is a view of `atomik_ui` ([`atomik_ui::StemsDialogView`]) that the core feeds once a frame:
//! the dialog shows while [`crate::AppCore::stems_dialog`] says so and takes every event while it
//! does (it is modal).

use super::*;
use atomik_ui::StemsDialogView;

impl<B: AudioBackend> Shell<B> {
    /// Whether the Separate Stems dialog shows.
    pub fn stems_dialog_visible(&self) -> bool {
        self.stems_dialog.is_some()
    }

    /// The dialog, while it shows.
    pub fn stems_dialog_view(&self) -> Option<&StemsDialogView> {
        self.stems_dialog.as_ref()
    }

    /// Hands the dialog the core's state; called once a frame.
    pub(super) fn sync_stems_ui(&mut self) {
        let size = self.size();
        match self.core.stems_dialog() {
            Some(state) => {
                match &mut self.stems_dialog {
                    Some(view) => {
                        view.set_state(state);
                        view.set_size((size.width, size.height));
                    }
                    None => {
                        self.stems_dialog =
                            Some(StemsDialogView::new((size.width, size.height), state));
                    }
                }
                self.repaint = true;
            }
            None => {
                if self.stems_dialog.take().is_some() {
                    self.repaint = true;
                }
            }
        }
    }

    /// An event for the dialog (it takes every one while it shows).
    pub(super) fn stems_dialog_event(&mut self, event: &UiEvent) -> Option<Vec<ShellRequest>> {
        let view = self.stems_dialog.as_mut()?;
        self.repaint = true;
        let actions = view.handle(event);
        for action in actions {
            if let UiAction::StemsDialog(choice) = action {
                self.core.stems_dialog_choice(choice);
            }
        }
        self.sync_stems_ui();
        Some(Vec::new())
    }

    /// The tooltip of the dialog under `pos`.
    pub(super) fn stems_help(&self, pos: Point) -> Option<atomik_ui::Help> {
        self.stems_dialog.as_ref()?.help_at(pos)
    }

    /// Paints the dialog.
    pub(super) fn paint_stems_dialog(&mut self, canvas: &mut dyn Canvas, theme: &atomik_ui::Theme) {
        if let Some(view) = &mut self.stems_dialog {
            view.paint(canvas, theme);
        }
    }
}
