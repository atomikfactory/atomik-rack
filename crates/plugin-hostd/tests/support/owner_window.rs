//! A top-level window of the test process that stands for the app's main window: it lives on the
//! thread that made it, and that thread pumps no messages unless the test says so, like the app's
//! UI thread while it waits for the child's reply. Windows only.
//!
//! The one `unsafe` file of the test suite: it calls `CreateWindowExW` and friends.
#![allow(unsafe_code)] // Justification: Win32 window calls have no safe API.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyWindow, DispatchMessageW, GW_CHILD, GetClientRect,
    GetParent, GetWindow, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};

/// A window of this thread; destroyed on drop.
pub struct OwnerWindow(HWND);

impl OwnerWindow {
    /// Makes a visible top-level window on the calling thread.
    pub fn new() -> Self {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "test owner\0".encode_utf16().collect();
        // SAFETY: both strings are NUL-terminated and outlive the call; STATIC is a system class;
        // the null parent, menu, instance and parameter are all valid for a top-level window.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "the owner window is created");
        Self(hwnd)
    }

    /// The handle, as the parent sends it.
    pub fn hwnd(&self) -> usize {
        self.0 as usize
    }

    /// The first child window of this window (a window of any process that was made its child),
    /// as a handle; `None` when it has none.
    pub fn first_child(&self) -> Option<usize> {
        // SAFETY: `self.0` is a live window of this thread; the call only reads window tree data.
        let child = unsafe { GetWindow(self.0, GW_CHILD) };
        (!child.is_null()).then_some(child as usize)
    }

    /// Every child of this window, topmost first, with its client size.
    pub fn children(&self) -> Vec<(usize, Option<(u32, u32)>)> {
        use windows_sys::Win32::UI::WindowsAndMessaging::GW_HWNDNEXT;
        let mut out = Vec::new();
        // SAFETY: only reads the window tree of a live window; a stale handle answers null.
        let mut at = unsafe { GetWindow(self.0, GW_CHILD) };
        while !at.is_null() {
            out.push((at as usize, self.client_size(at as usize)));
            // SAFETY: as above.
            at = unsafe { GetWindow(at, GW_HWNDNEXT) };
        }
        out
    }

    /// Whether `child` (a handle) has this window as its parent.
    pub fn is_parent_of(&self, child: usize) -> bool {
        // SAFETY: `GetParent` only reads the window tree; a stale handle answers null.
        unsafe { GetParent(child as HWND) == self.0 }
    }

    /// The client size of `child` (a handle of a window of any process), in pixels.
    pub fn client_size(&self, child: usize) -> Option<(u32, u32)> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `r` is a valid, writable RECT for the whole call; a stale handle fails.
        (unsafe { GetClientRect(child as HWND, &mut r) } != 0)
            .then(|| (r.right.max(0) as u32, r.bottom.max(0) as u32))
    }

    /// Handles every message queued for this thread, as a UI thread does between requests.
    pub fn pump(&self) {
        // SAFETY: `msg` is a plain-data out parameter; the calls only read and dispatch messages
        // of the calling thread, which owns this window.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: the window was made on this thread and is destroyed once, here.
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
