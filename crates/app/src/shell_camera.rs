//! The webcam controller in the shell: the consent dialog of the model download and the camera
//! preview box.
//!
//! Both are views of `atomik_ui` ([`atomik_ui::CameraDialogView`], [`atomik_ui::CameraPreviewView`])
//! that the core feeds once a frame: the dialog shows while [`crate::AppCore::camera_dialog`]
//! says so and takes every event while it does (it is modal); the preview box floats in the lower
//! right corner above the status line while the camera is on and the preview is switched on, takes
//! only the events that land on it, and never opens a window of its own. Camera pictures are
//! handed over in memory and are never written anywhere.

use super::*;
use atomik_ui::{CAMERA_PREVIEW_WIDTH, CameraDialogView, CameraPreviewView};

/// The gap between the preview box and the window edge, and above the status line.
const MARGIN: f64 = 14.0;

impl<B: AudioBackend> Shell<B> {
    /// Whether the camera preview box shows.
    pub fn camera_preview_visible(&self) -> bool {
        self.core.camera_is_on() && self.core.camera_preview_on()
    }

    /// Whether the consent dialog shows.
    pub fn camera_dialog_visible(&self) -> bool {
        self.camera_dialog.is_some()
    }

    /// The preview box (its state and rectangle, for tests and the paint).
    pub fn camera_preview_view(&self) -> &CameraPreviewView {
        &self.camera_preview
    }

    /// The consent dialog, while it shows.
    pub fn camera_dialog_view(&self) -> Option<&CameraDialogView> {
        self.camera_dialog.as_ref()
    }

    /// Keeps the project's own camera settings as the Settings dialog left them: one undo step
    /// when they changed, nothing when they did not.
    pub(super) fn apply_camera_project(&mut self, camera: Option<atomik_model::CameraOverride>) {
        if !self.core.set_camera_override(camera) {
            self.notify("The camera settings could not be kept in the project");
        }
    }

    /// Runs the commands the camera's gestures asked for, each as the key bound to it would (the
    /// same view first, then the shell's own), and says so when one does not apply here.
    pub(super) fn run_camera_commands(&mut self) {
        for id in self.core.take_camera_commands() {
            match self.run_command(&id, None) {
                Some(mut requests) => self.pending.append(&mut requests),
                None => {
                    let name = atomik_ui::keymap::Registry::bundled()
                        .get(&id)
                        .map_or(id.as_str(), |d| d.label);
                    self.notify(format!("{name} is not available in this view"));
                }
            }
            self.repaint = true;
        }
    }

    /// Hands the views the core's state; called once a frame.
    pub(super) fn sync_camera_ui(&mut self) {
        let size = self.size();
        match self.core.camera_dialog().cloned() {
            Some(state) => {
                match &mut self.camera_dialog {
                    Some(view) => {
                        view.set_state(state);
                        view.set_size((size.width, size.height));
                    }
                    None => {
                        self.camera_dialog =
                            Some(CameraDialogView::new((size.width, size.height), state));
                    }
                }
                self.repaint = true;
            }
            None => {
                if self.camera_dialog.take().is_some() {
                    self.repaint = true;
                }
            }
        }
        if self.camera_preview_visible() {
            let state = self.core.camera_preview_state();
            let origin = Point::new(
                (size.width - CAMERA_PREVIEW_WIDTH - MARGIN).max(0.0),
                (size.height - STATUS_BAR_HEIGHT - CameraPreviewView::height_for(&state) - MARGIN)
                    .max(0.0),
            );
            self.camera_preview.set_origin(origin);
            self.camera_preview.set_state(state);
            if self.camera_preview.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// An event for the consent dialog (it takes every one while it shows).
    pub(super) fn camera_dialog_event(&mut self, event: &UiEvent) -> Option<Vec<ShellRequest>> {
        let view = self.camera_dialog.as_mut()?;
        self.repaint = true;
        let actions = view.handle(event);
        for action in actions {
            if let UiAction::CameraDialog(choice) = action {
                self.core.camera_dialog_choice(choice);
            }
        }
        Some(Vec::new())
    }

    /// An event for the preview box: `Some` when it took the event.
    pub(super) fn camera_preview_event(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Option<Vec<ShellRequest>> {
        if window != WindowKind::Main
            || !self.camera_preview_visible()
            || self.capture.is_some()
            || self.modal_open()
        {
            return None;
        }
        if !matches!(
            event,
            UiEvent::PointerMove { .. } | UiEvent::PointerDown { .. } | UiEvent::PointerLeave
        ) {
            return None;
        }
        let (taken, actions) = self.camera_preview.handle(event);
        if self.camera_preview.needs_repaint() {
            self.repaint = true;
        }
        for action in actions {
            if let UiAction::RunCommand { id, arg } = action
                && let Some(mut requests) = self.run_command(&id, arg.as_deref())
            {
                self.pending.append(&mut requests);
            }
        }
        // A move over the box is also the pointer being somewhere else's (the tooltip, the
        // hover): only a press is the box's own.
        (taken && matches!(event, UiEvent::PointerDown { .. })).then(Vec::new)
    }

    /// The tooltip of the preview box or the dialog under `pos`.
    pub(super) fn camera_help(&self, pos: Point) -> Option<atomik_ui::Help> {
        if let Some(view) = &self.camera_dialog {
            return view.help_at(pos);
        }
        if self.camera_preview_visible() {
            return self.camera_preview.help_at(pos);
        }
        None
    }

    /// Paints the preview box.
    pub(super) fn paint_camera_preview(
        &mut self,
        canvas: &mut dyn Canvas,
        theme: &atomik_ui::Theme,
    ) {
        if self.camera_preview_visible() {
            self.camera_preview.paint(canvas, theme);
        }
    }

    /// Paints the consent dialog.
    pub(super) fn paint_camera_dialog(
        &mut self,
        canvas: &mut dyn Canvas,
        theme: &atomik_ui::Theme,
    ) {
        if let Some(view) = &mut self.camera_dialog {
            view.paint(canvas, theme);
        }
    }
}
