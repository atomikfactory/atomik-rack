//! The editor window of the plugin (D3): a top-level window the child owns, owned in turn by the
//! app window of the parent where the platform allows, with the plugin's own editor embedded in
//! it.
//!
//! The logic here (which window the plugin goes into, how big it is, what follows a resize, what
//! a second open does) is separate from the window system, which is the [`WindowSystem`] trait:
//! `driver` implements it with winit, the tests with a recording fake. The real windows are not
//! covered by tests (they need a display); they are checked by hand.

use atomik_plugin_host::{BackendError, NativeParent, PluginInstance};

/// The size a window has before the plugin says what it wants.
pub const DEFAULT_SIZE: (u32, u32) = (640, 480);

/// A window the window system made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewWindow {
    /// The handle the plugin embeds into.
    pub parent: NativeParent,
    /// The window's display scale factor.
    pub scale: f64,
}

/// The window system, as far as the editor needs it. There is at most one window.
pub trait WindowSystem {
    /// Makes the window, hidden, titled `title`, owned by `owner` where the platform allows.
    ///
    /// # Errors
    /// A sentence saying why there is no window (no display, no handle, Wayland).
    fn create(
        &mut self,
        title: &str,
        owner: Option<NativeParent>,
        size: (u32, u32),
    ) -> Result<NewWindow, String>;
    /// Sets the inner size in the plugin's units (pixels; logical points on macOS).
    fn resize(&mut self, w: u32, h: u32);
    /// Shows the window, raises it and gives it the focus.
    fn show(&mut self);
    /// Hides the window.
    fn hide(&mut self);
    /// Destroys the window.
    fn destroy(&mut self);
}

/// The window handle a Windows owner needs: only a `HWND` can own a window there.
pub fn owner_hwnd(owner: Option<NativeParent>) -> Option<isize> {
    match owner {
        Some(NativeParent::Win32(h)) => Some(h as isize),
        Some(NativeParent::Cocoa(_) | NativeParent::X11(_)) | None => None,
    }
}

/// The scale the plugin is told. CLAP and VST3 editors are sized in pixels on Windows and X11
/// and in logical points on macOS (scale 1 there); the window's own factor wins over the
/// parent's, because the editor lives in the child's window.
pub fn effective_scale(window: f64, requested: f64, mac: bool) -> f64 {
    let usable = |s: f64| s.is_finite() && s > 0.0;
    if mac {
        1.0
    } else if usable(window) {
        window
    } else if usable(requested) {
        requested
    } else {
        1.0
    }
}

/// The size a user resize is reported to the plugin in: pixels, or logical points on macOS.
pub fn reported_size(physical: (u32, u32), scale: f64, mac: bool) -> (u32, u32) {
    if mac && scale.is_finite() && scale > 0.0 {
        (
            (f64::from(physical.0) / scale).round() as u32,
            (f64::from(physical.1) / scale).round() as u32,
        )
    } else {
        physical
    }
}

/// The editor of one instance.
#[derive(Debug, Default)]
pub struct Editor {
    open: bool,
    size: (u32, u32),
}

impl Editor {
    /// True while the window and the plugin's editor exist.
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the editor, or raises it when it is open already; returns its size.
    ///
    /// # Errors
    /// The window system's reason, or the plugin's own error (the window is destroyed again).
    pub fn open(
        &mut self,
        instance: &mut dyn PluginInstance,
        windows: &mut dyn WindowSystem,
        title: &str,
        owner: Option<NativeParent>,
        requested_scale: f64,
    ) -> Result<(u32, u32), BackendError> {
        if self.open {
            windows.show();
            return Ok(self.size);
        }
        let created = windows
            .create(title, owner, DEFAULT_SIZE)
            .map_err(BackendError::Failed)?;
        let scale = effective_scale(created.scale, requested_scale, cfg!(target_os = "macos"));
        match instance.open_gui(created.parent, scale) {
            Ok((w, h)) => {
                windows.resize(w, h);
                windows.show();
                self.open = true;
                self.size = (w, h);
                Ok((w, h))
            }
            Err(e) => {
                windows.destroy();
                Err(e)
            }
        }
    }

    /// Closes the plugin's editor and destroys the window. False when it was not open.
    pub fn close(
        &mut self,
        instance: &mut dyn PluginInstance,
        windows: &mut dyn WindowSystem,
    ) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        instance.close_gui();
        windows.destroy();
        true
    }

    /// The user resized the window to `w` x `h`: the plugin answers with the size it takes and
    /// the window follows when that is another size.
    pub fn user_resized(
        &mut self,
        instance: &mut dyn PluginInstance,
        windows: &mut dyn WindowSystem,
        w: u32,
        h: u32,
    ) {
        if !self.open {
            return;
        }
        match instance.resize_gui(w, h) {
            Ok((aw, ah)) => {
                self.size = (aw, ah);
                if (aw, ah) != (w, h) {
                    windows.resize(aw, ah);
                }
            }
            Err(_) => self.size = (w, h),
        }
    }

    /// The plugin asked for another size: the window follows.
    pub fn plugin_resized(&mut self, windows: &mut dyn WindowSystem, w: u32, h: u32) {
        if self.open {
            self.size = (w, h);
            windows.resize(w, h);
        }
    }

    /// The plugin asked to show (`true`) or hide its editor.
    pub fn plugin_visible(&mut self, windows: &mut dyn WindowSystem, visible: bool) {
        if self.open {
            if visible {
                windows.show();
            } else {
                windows.hide();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    //! A window system and a plugin instance that record what they are asked.

    use super::*;
    use atomik_device_sdk::{PluginLayout, PluginStateBlob};
    use atomik_plugin_host::{ActivateConfig, HostHandle, PluginMessage, PluginProcessor};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Everything the fakes were asked, in order.
    pub type Log = Rc<RefCell<Vec<String>>>;

    pub struct FakeWindows {
        pub log: Log,
        pub scale: f64,
        pub fail_create: Option<String>,
        pub native: NativeParent,
    }

    impl FakeWindows {
        pub fn new(log: &Log) -> Self {
            Self {
                log: Rc::clone(log),
                scale: 1.5,
                fail_create: None,
                native: NativeParent::Win32(0xABC),
            }
        }
    }

    impl WindowSystem for FakeWindows {
        fn create(
            &mut self,
            title: &str,
            owner: Option<NativeParent>,
            size: (u32, u32),
        ) -> Result<NewWindow, String> {
            if let Some(why) = &self.fail_create {
                return Err(why.clone());
            }
            self.log
                .borrow_mut()
                .push(format!("create {title} {owner:?} {size:?}"));
            Ok(NewWindow {
                parent: self.native,
                scale: self.scale,
            })
        }
        fn resize(&mut self, w: u32, h: u32) {
            self.log.borrow_mut().push(format!("resize {w}x{h}"));
        }
        fn show(&mut self) {
            self.log.borrow_mut().push("show".to_string());
        }
        fn hide(&mut self) {
            self.log.borrow_mut().push("hide".to_string());
        }
        fn destroy(&mut self) {
            self.log.borrow_mut().push("destroy".to_string());
        }
    }

    pub struct FakeInstance {
        pub log: Log,
        pub layout: PluginLayout,
        pub gui_size: (u32, u32),
        pub fail_open: bool,
        /// What `resize_gui` answers (`None`: the size asked for).
        pub resize_answer: Option<(u32, u32)>,
        /// The host handle the plugin posts through, and what it posts at its next `poll`.
        pub host: Option<HostHandle>,
        pub on_poll: Vec<PluginMessage>,
    }

    impl FakeInstance {
        pub fn new(log: &Log) -> Self {
            Self {
                log: Rc::clone(log),
                layout: PluginLayout {
                    name: "Fake".to_string(),
                    vendor: String::new(),
                    version: String::new(),
                    kind_hint: atomik_device_sdk::PluginKindHint::Effect,
                    main_in: None,
                    main_out: None,
                    sidechain: false,
                    notes_in: false,
                    notes_out: false,
                    params: Vec::new(),
                },
                gui_size: (400, 300),
                fail_open: false,
                resize_answer: None,
                host: None,
                on_poll: Vec::new(),
            }
        }
    }

    impl PluginInstance for FakeInstance {
        fn layout(&self) -> &PluginLayout {
            &self.layout
        }
        fn activate(
            &mut self,
            _cfg: &ActivateConfig,
        ) -> Result<Box<dyn PluginProcessor>, BackendError> {
            Err(BackendError::Unsupported("fake".to_string()))
        }
        fn deactivate(&mut self, _processor: Box<dyn PluginProcessor>) {}
        fn latency_frames(&self) -> u32 {
            0
        }
        fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
            Ok(PluginStateBlob::default())
        }
        fn restore_state(&mut self, _state: &PluginStateBlob) -> Result<(), BackendError> {
            Ok(())
        }
        fn param_text(&mut self, _id: u32, _plain: f64) -> Option<String> {
            None
        }
        fn open_gui(
            &mut self,
            parent: NativeParent,
            scale: f64,
        ) -> Result<(u32, u32), BackendError> {
            self.log
                .borrow_mut()
                .push(format!("open_gui {parent:?} {scale}"));
            if self.fail_open {
                Err(BackendError::Failed("no editor today".to_string()))
            } else {
                Ok(self.gui_size)
            }
        }
        fn close_gui(&mut self) {
            self.log.borrow_mut().push("close_gui".to_string());
        }
        fn poll(&mut self) {
            if let Some(host) = &self.host {
                for m in self.on_poll.drain(..) {
                    host.messenger().post(m);
                }
            }
        }
        fn resize_gui(&mut self, w: u32, h: u32) -> Result<(u32, u32), BackendError> {
            self.log.borrow_mut().push(format!("resize_gui {w}x{h}"));
            Ok(self.resize_answer.unwrap_or((w, h)))
        }
    }

    pub fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::{FakeInstance, FakeWindows, log};
    use super::*;

    fn lines(l: &fakes::Log) -> Vec<String> {
        l.borrow().clone()
    }

    #[test]
    fn opening_embeds_the_plugin_in_a_window_owned_by_the_parent_and_sizes_it_to_the_answer() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        let size = ed
            .open(
                &mut inst,
                &mut ws,
                "Fake",
                Some(NativeParent::Win32(77)),
                2.0,
            )
            .unwrap();
        assert_eq!(size, (400, 300));
        assert!(ed.is_open());
        let want_scale = if cfg!(target_os = "macos") { 1.0 } else { 1.5 };
        assert_eq!(
            lines(&l),
            vec![
                format!("create Fake Some(Win32(77)) {DEFAULT_SIZE:?}"),
                format!("open_gui Win32(2748) {want_scale}"),
                "resize 400x300".to_string(),
                "show".to_string(),
            ]
        );
    }

    #[test]
    fn a_second_open_raises_the_window_and_does_not_open_the_plugin_again() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        l.borrow_mut().clear();
        let size = ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        assert_eq!(size, (400, 300));
        assert_eq!(lines(&l), vec!["show".to_string()]);
    }

    #[test]
    fn a_plugin_that_cannot_open_its_editor_leaves_no_window() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        inst.fail_open = true;
        let e = ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap_err();
        assert_eq!(e, BackendError::Failed("no editor today".to_string()));
        assert!(!ed.is_open());
        assert_eq!(lines(&l).last().map(String::as_str), Some("destroy"));
    }

    #[test]
    fn a_window_that_cannot_be_made_is_reported_and_the_plugin_is_not_asked() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ws.fail_create = Some("no display".to_string());
        let e = ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap_err();
        assert_eq!(e, BackendError::Failed("no display".to_string()));
        assert!(!ed.is_open());
        assert!(lines(&l).is_empty());
    }

    #[test]
    fn closing_closes_the_plugin_editor_then_destroys_the_window() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        assert!(!ed.close(&mut inst, &mut ws), "nothing to close yet");
        assert!(lines(&l).is_empty());
        ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        l.borrow_mut().clear();
        assert!(ed.close(&mut inst, &mut ws));
        assert!(!ed.is_open());
        assert_eq!(
            lines(&l),
            vec!["close_gui".to_string(), "destroy".to_string()]
        );
    }

    #[test]
    fn a_user_resize_goes_to_the_plugin_and_the_window_follows_a_different_answer() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        l.borrow_mut().clear();
        ed.user_resized(&mut inst, &mut ws, 500, 400);
        assert_eq!(lines(&l), vec!["resize_gui 500x400".to_string()]);
        l.borrow_mut().clear();
        inst.resize_answer = Some((480, 360));
        ed.user_resized(&mut inst, &mut ws, 500, 400);
        assert_eq!(
            lines(&l),
            vec![
                "resize_gui 500x400".to_string(),
                "resize 480x360".to_string()
            ]
        );
    }

    #[test]
    fn a_resize_without_an_open_editor_is_ignored() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ed.user_resized(&mut inst, &mut ws, 500, 400);
        ed.plugin_resized(&mut ws, 500, 400);
        ed.plugin_visible(&mut ws, true);
        assert!(lines(&l).is_empty());
    }

    #[test]
    fn a_plugin_resize_request_resizes_the_window() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        l.borrow_mut().clear();
        ed.plugin_resized(&mut ws, 800, 600);
        assert_eq!(lines(&l), vec!["resize 800x600".to_string()]);
    }

    #[test]
    fn a_plugin_can_show_and_hide_its_editor() {
        let l = log();
        let (mut inst, mut ws, mut ed) = (
            FakeInstance::new(&l),
            FakeWindows::new(&l),
            Editor::default(),
        );
        ed.open(&mut inst, &mut ws, "Fake", None, 1.0).unwrap();
        l.borrow_mut().clear();
        ed.plugin_visible(&mut ws, false);
        ed.plugin_visible(&mut ws, true);
        assert_eq!(lines(&l), vec!["hide".to_string(), "show".to_string()]);
    }

    #[test]
    fn only_a_win_32_owner_can_own_a_window_on_windows() {
        assert_eq!(owner_hwnd(Some(NativeParent::Win32(0x1234))), Some(0x1234));
        assert_eq!(owner_hwnd(Some(NativeParent::Cocoa(1))), None);
        assert_eq!(owner_hwnd(Some(NativeParent::X11(1))), None);
        assert_eq!(owner_hwnd(None), None);
    }

    #[test]
    fn the_scale_comes_from_the_window_except_on_macos_and_when_it_is_unusable() {
        assert!((effective_scale(1.25, 2.0, false) - 1.25).abs() < f64::EPSILON);
        assert!((effective_scale(0.0, 2.0, false) - 2.0).abs() < f64::EPSILON);
        assert!((effective_scale(f64::NAN, f64::NAN, false) - 1.0).abs() < f64::EPSILON);
        assert!((effective_scale(2.0, 2.0, true) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_user_size_is_reported_in_pixels_or_in_logical_points_on_macos() {
        assert_eq!(reported_size((800, 600), 2.0, false), (800, 600));
        assert_eq!(reported_size((800, 600), 2.0, true), (400, 300));
        assert_eq!(reported_size((801, 601), 2.0, true), (401, 301));
        assert_eq!(reported_size((800, 600), 0.0, true), (800, 600));
    }
}
