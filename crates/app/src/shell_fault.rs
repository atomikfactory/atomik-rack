//! The shell after the platform contained a panic of the UI thread (see [`crate::ui_guard`]).
//!
//! What a panic can leave behind is a gesture or a menu that never ends, so those are dropped
//! the way a lost focus or a failed edit drops them; the document is not touched (a command is
//! planned before it changes anything, and a panic inside one is caught where it applies). A
//! recovery copy is asked for, and the notice of the banner strip says so, with SAVE and DISMISS.
//!
//! When the panics keep coming the platform pauses the window's content: [`Shell::paint_in`]
//! then paints the notice alone and the pointer reaches only its buttons, so SAVE stays within
//! reach while the keyboard (Ctrl+S included) works as before. The pause lasts until the app is
//! restarted.

use super::*;
use atomik_ui::BannerKind;
use atomik_ui::plugin_data::PluginBannerAction;

impl<B: AudioBackend> Shell<B> {
    /// A panic was contained. `notice` raises the notice (it is up already when false);
    /// `halted` pauses the window's content. Every step is guarded: a state that panics again
    /// does not stop the ones after it.
    pub fn after_ui_panic(&mut self, notice: bool, halted: bool) {
        self.ui_halted = halted;
        if notice {
            self.banner.show(BannerKind::InternalError { halted });
        }
        self.repaint = true;
        self.core.request_recovery_snapshot();
        // Gestures and menus in flight: the releases that would end them may never come.
        let _ = crate::ui_guard::run(|| {
            self.capture = None;
            self.context_menu = None;
            self.file_drag = None;
            self.focus_lost();
        });
        let _ = crate::ui_guard::run(|| self.core.drop_gestures());
        let _ = crate::ui_guard::run(|| self.layout_view());
    }

    /// Whether repeated internal errors paused the window's content.
    #[must_use]
    pub fn ui_halted(&self) -> bool {
        self.ui_halted
    }

    /// Runs what the banner's buttons ask for: SAVE is File > Save, the rest is the core's.
    pub(super) fn run_banner_actions(&mut self, actions: Vec<UiAction>) -> Vec<ShellRequest> {
        let mut requests = Vec::new();
        for action in actions {
            match action {
                UiAction::PluginBanner(PluginBannerAction::SaveProject) => {
                    requests.extend(self.execute_command(AppCommand::Save));
                }
                // The notice hid itself.
                UiAction::PluginBanner(PluginBannerAction::DismissError) => {}
                other => self.core.handle_action(other),
            }
        }
        requests
    }

    /// The strip the notice takes while the content is paused: the top of the main window.
    fn halted_strip(&self) -> Rect {
        Rect::new(
            0.0,
            0.0,
            self.size().width,
            atomik_ui::plugin_banner::BAR_HEIGHT,
        )
    }

    /// A pointer event of a paused window: only the notice's buttons answer, in the main window.
    pub(super) fn halted_pointer(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Vec<ShellRequest> {
        if window != WindowKind::Main {
            return Vec::new();
        }
        let strip = self.halted_strip();
        self.banner.set_bounds(strip);
        let actions = self.banner.handle(event);
        let mut requests = self.run_banner_actions(actions);
        requests.append(&mut self.pending);
        self.repaint = true;
        requests
    }

    /// The paint of a paused window: the notice alone (the main window), nothing for the others.
    pub(super) fn paint_halted(&mut self, window: WindowKind, canvas: &mut dyn Canvas) {
        if window == WindowKind::Main {
            let strip = self.halted_strip();
            self.banner.set_bounds(strip);
            self.banner.paint(canvas);
        }
        self.mark_painted(window);
    }
}

#[cfg(test)]
#[path = "shell_fault_tests.rs"]
mod tests;
