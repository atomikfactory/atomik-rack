//! The Plugins section of the settings dialog: a self-contained view the
//! app places inside its settings dialog.
//!
//! ```text
//! PLUGINS                                                        [ ENABLE ]
//! [x] Run plugins in a separate process (recommended)
//!     Applies after restarting Atomik
//! SEARCH FOLDERS
//!   D:/Plugins/CLAP                                                   [x]
//!   [ADD FOLDER...]
//! SCAN
//!   [SCAN] [FULL RESCAN] [RESCAN FAILED]        SCAN TIMEOUT  30 s [-][+]
//!   [=========>        ] 12 / 40 files                          [CANCEL]
//! FOUND PLUGINS (5)
//!   [ ] Test Synth   Atomik Test  1.2.0  CLAP  OK
//!   [x] Switched Off ...                       DISABLED
//! FAILED FILES (2)
//!   atomik-test-clap-bad.clap   init failed                      [RETRY]
//! PLUGIN LOG
//!   [Test Synth] loaded
//! ```
//!
//! Hosting follows the other views (`set_bounds`, `handle`, `paint`, `needs_repaint`,
//! `set_scale`), with no document: the app converts its `PluginSettings`, `PluginCatalog`,
//! blacklist, `ScanProgress` and plugin log into a [`PluginSettingsData`] and calls
//! [`PluginSettingsView::set_data`] whenever it changes (every frame is fine); what the user
//! does arrives as [`UiAction::PluginSettings`].
//!
//! * **Switch.** Plugins are off by default. While they are off only the switch and a hint
//!   show. The ENABLE switch emits [`PluginSettingsAction::SetEnabled`] at once, on or off.
//! * **Process.** The check under the header (always shown) emits
//!   [`PluginSettingsAction::SetSandbox`]; the note under it says the change applies at the next
//!   start.
//! * **Scanning.** `SCAN`, `FULL RESCAN` and `RESCAN FAILED` emit [`PluginSettingsAction::Scan`]
//!   with the matching [`crate::plugin_data::PluginRescan`]; while a scan runs they are dim and
//!   the progress bar and `CANCEL` show. The timeout steps by 5 s (Shift: 30 s) within 5 to 300.
//! * **Lists.** Each plugin has a DISABLE check (`SetPluginDisabled`), each failed file its
//!   reason and a RETRY (`RetryFile`). The list is scrolled with the wheel; only the rows in
//!   view are painted.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Modifiers, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::plugin_data::{
    MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS, PluginEntryStatus, PluginRescan, PluginSettingsAction,
    PluginSettingsData, format_badge,
};
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};
use vello::peniko::Color;

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const HINT_H: f64 = 30.0;
const SANDBOX_H: f64 = 40.0;
const HEADING_H: f64 = 26.0;
const FOLDER_H: f64 = 24.0;
const BUTTONS_H: f64 = 30.0;
const PROGRESS_H: f64 = 26.0;
const PLUGIN_H: f64 = 22.0;
const FAILED_H: f64 = 36.0;
const LOG_H: f64 = 14.0;
/// Log lines shown (the newest).
pub const LOG_LINES: usize = 12;

/// One block of the settings column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Header,
    Sandbox,
    OffHint,
    NoScanner,
    ScanSummary,
    Heading(Section),
    Folder(usize),
    AddFolder,
    ScanButtons,
    Timeout,
    Progress,
    Plugin(usize),
    Failed(usize),
    Log(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Folders,
    Scan,
    Plugins,
    FailedFiles,
    Log,
}

impl Item {
    fn height(self) -> f64 {
        match self {
            Item::Header => HEADER_H,
            Item::Sandbox => SANDBOX_H,
            Item::OffHint | Item::NoScanner | Item::ScanSummary => HINT_H,
            Item::Heading(_) => HEADING_H,
            Item::Folder(_) => FOLDER_H,
            Item::AddFolder | Item::ScanButtons | Item::Timeout => BUTTONS_H,
            Item::Progress => PROGRESS_H,
            Item::Plugin(_) => PLUGIN_H,
            Item::Failed(_) => FAILED_H,
            Item::Log(_) => LOG_H,
        }
    }
}

/// The Plugins section of the settings dialog. See the module documentation.
#[derive(Debug)]
pub struct PluginSettingsView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: PluginSettingsData,
    scroll: f64,
    items: Vec<Item>,
    /// Prefix sums of the item heights (`offsets[i]` is the top of item `i`).
    offsets: Vec<f64>,
    hover: Option<Point>,
    focused: bool,
    repaint: bool,
}

impl PluginSettingsView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A view with the default (plugins off) data; call [`Self::set_bounds`] and
    /// [`Self::set_data`].
    pub fn new(theme: Theme) -> Self {
        let mut v = Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 760.0, 560.0),
            scale: 1.0,
            data: PluginSettingsData::default(),
            scroll: 0.0,
            items: Vec::new(),
            offsets: vec![0.0],
            hover: None,
            focused: false,
            repaint: true,
        };
        v.relayout();
        v
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the view as focused (thin top line).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Replaces what the view shows. Cheap when nothing changed.
    pub fn set_data(&mut self, data: PluginSettingsData) {
        if data == self.data {
            return;
        }
        self.data = data;
        self.relayout();
        self.repaint = true;
    }

    /// What the view shows.
    pub fn data(&self) -> &PluginSettingsData {
        &self.data
    }

    /// The scroll offset in pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Height of all the blocks.
    pub fn content_height(&self) -> f64 {
        self.offsets.last().copied().unwrap_or(0.0) + PAD
    }

    fn relayout(&mut self) {
        let d = &self.data;
        let mut items = vec![Item::Header];
        items.push(Item::Sandbox);
        if !d.enabled {
            items.push(Item::OffHint);
        } else {
            if !d.scanner_available {
                items.push(Item::NoScanner);
            }
            items.push(Item::Heading(Section::Folders));
            items.extend((0..d.extra_paths.len()).map(Item::Folder));
            items.push(Item::AddFolder);
            items.push(Item::Heading(Section::Scan));
            items.push(Item::ScanButtons);
            items.push(Item::Timeout);
            if d.scan.is_some() {
                items.push(Item::Progress);
            } else if d.last_scan.is_some() {
                items.push(Item::ScanSummary);
            }
            items.push(Item::Heading(Section::Plugins));
            items.extend((0..d.plugins.len()).map(Item::Plugin));
            if !d.failed.is_empty() {
                items.push(Item::Heading(Section::FailedFiles));
                items.extend((0..d.failed.len()).map(Item::Failed));
            }
            if !d.log.is_empty() {
                items.push(Item::Heading(Section::Log));
                let first = d.log.len().saturating_sub(LOG_LINES);
                items.extend((first..d.log.len()).map(Item::Log));
            }
        }
        let mut offsets = Vec::with_capacity(items.len() + 1);
        let mut y = PAD / 2.0;
        for it in &items {
            offsets.push(y);
            y += it.height();
        }
        offsets.push(y);
        self.items = items;
        self.offsets = offsets;
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_height() - self.bounds.height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// The row rectangle of an item, in view coordinates (`None` when out of view).
    fn rect_of(&self, idx: usize) -> Option<Rect> {
        let y = self.bounds.y0 + self.offsets[idx] - self.scroll;
        let h = self.items[idx].height();
        let r = Rect::new(self.bounds.x0 + PAD, y, self.bounds.x1 - PAD, y + h);
        (r.y1 > self.bounds.y0 && r.y0 < self.bounds.y1).then_some(r)
    }

    fn find(&self, item: Item) -> Option<(usize, Rect)> {
        let idx = self.items.iter().position(|i| *i == item)?;
        Some((idx, self.rect_of(idx)?))
    }

    fn visible(&self) -> Vec<(usize, Rect)> {
        let first = self
            .offsets
            .partition_point(|o| *o <= self.scroll)
            .saturating_sub(1);
        let mut out = Vec::new();
        for idx in first..self.items.len() {
            if self.bounds.y0 + self.offsets[idx] - self.scroll >= self.bounds.y1 {
                break;
            }
            if let Some(r) = self.rect_of(idx) {
                out.push((idx, r));
            }
        }
        out
    }

    // ------------------------------------------------------------------ geometry

    fn button_in(row: Rect, x: f64, w: f64) -> Rect {
        Rect::new(x, row.y0 + 4.0, x + w, row.y1 - 4.0)
    }

    /// The ENABLE switch.
    pub fn enable_rect(&self) -> Rect {
        let r = self.find(Item::Header).map_or(
            Rect::new(self.bounds.x1 - 110.0, 0.0, self.bounds.x1, 0.0),
            |(_, r)| r,
        );
        Rect::new(r.x1 - 96.0, r.y0 + 5.0, r.x1, r.y1 - 5.0)
    }

    /// The ADD FOLDER button.
    pub fn add_folder_rect(&self) -> Option<Rect> {
        let (_, r) = self.find(Item::AddFolder)?;
        Some(Self::button_in(r, r.x0, 120.0))
    }

    /// The remove button of search folder `i`.
    pub fn folder_remove_rect(&self, i: usize) -> Option<Rect> {
        let (_, r) = self.find(Item::Folder(i))?;
        Some(Rect::new(r.x1 - 26.0, r.y0 + 2.0, r.x1 - 4.0, r.y1 - 2.0))
    }

    fn scan_buttons(&self) -> Option<[Rect; 3]> {
        let (_, r) = self.find(Item::ScanButtons)?;
        Some([
            Self::button_in(r, r.x0, 90.0),
            Self::button_in(r, r.x0 + 98.0, 110.0),
            Self::button_in(r, r.x0 + 216.0, 130.0),
        ])
    }

    /// The SCAN button.
    pub fn scan_rect(&self) -> Option<Rect> {
        self.scan_buttons().map(|b| b[0])
    }

    /// The FULL RESCAN button.
    pub fn full_rescan_rect(&self) -> Option<Rect> {
        self.scan_buttons().map(|b| b[1])
    }

    /// The RESCAN FAILED button.
    pub fn rescan_failed_rect(&self) -> Option<Rect> {
        self.scan_buttons().map(|b| b[2])
    }

    /// The `-` of the timeout stepper.
    pub fn timeout_dec_rect(&self) -> Option<Rect> {
        let (_, r) = self.find(Item::Timeout)?;
        Some(Self::button_in(r, r.x0 + 190.0, 24.0))
    }

    /// The `+` of the timeout stepper.
    pub fn timeout_inc_rect(&self) -> Option<Rect> {
        let (_, r) = self.find(Item::Timeout)?;
        Some(Self::button_in(r, r.x0 + 218.0, 24.0))
    }

    /// The CANCEL button of a running scan.
    pub fn cancel_scan_rect(&self) -> Option<Rect> {
        let (_, r) = self.find(Item::Progress)?;
        Some(Self::button_in(r, r.x0 + 420.0, 80.0))
    }

    /// The DISABLE check of plugin row `i`.
    pub fn plugin_toggle_rect(&self, i: usize) -> Option<Rect> {
        let (_, r) = self.find(Item::Plugin(i))?;
        Some(Rect::new(r.x0 + 2.0, r.y0 + 4.0, r.x0 + 16.0, r.y0 + 18.0))
    }

    /// The check of the separate-process row.
    pub fn sandbox_toggle_rect(&self) -> Option<Rect> {
        let (_, r) = self.find(Item::Sandbox)?;
        Some(Rect::new(r.x0 + 2.0, r.y0 + 4.0, r.x0 + 16.0, r.y0 + 18.0))
    }

    /// The RETRY button of failed file `i`.
    pub fn retry_rect(&self, i: usize) -> Option<Rect> {
        let (_, r) = self.find(Item::Failed(i))?;
        Some(Rect::new(r.x1 - 64.0, r.y0 + 7.0, r.x1 - 4.0, r.y1 - 7.0))
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let at = |r: Option<Rect>, text: &'static str| {
            r.filter(|r| r.contains(pos)).map(|r| Help::new(r, text))
        };
        let visible = self.visible();
        let shown = |item: Item| {
            self.items
                .iter()
                .position(|i| *i == item)
                .is_some_and(|k| visible.iter().any(|(i, _)| *i == k))
        };
        if let Some(h) = at(
            Some(self.enable_rect()),
            "Turn third-party plugins on or off",
        ) {
            return Some(h);
        }
        let fixed = [
            (
                Item::Sandbox,
                self.sandbox_toggle_rect(),
                "Run each plugin in its own process so a crash cannot take Atomik down",
            ),
            (
                Item::AddFolder,
                self.add_folder_rect(),
                "Add a folder to search for plugins",
            ),
            (
                Item::ScanButtons,
                self.scan_rect(),
                "Scan the folders for new and changed plugins",
            ),
            (
                Item::ScanButtons,
                self.full_rescan_rect(),
                "Scan every plugin again",
            ),
            (
                Item::ScanButtons,
                self.rescan_failed_rect(),
                "Try the files that failed again",
            ),
            (
                Item::Timeout,
                self.timeout_dec_rect(),
                "Shorter time allowed per plugin",
            ),
            (
                Item::Timeout,
                self.timeout_inc_rect(),
                "Longer time allowed per plugin",
            ),
            (Item::Progress, self.cancel_scan_rect(), "Stop the scan"),
        ];
        for (item, rect, text) in fixed {
            if shown(item)
                && let Some(h) = at(rect, text)
            {
                return Some(h);
            }
        }
        for (k, item) in self.items.iter().enumerate() {
            if !visible.iter().any(|(i, _)| *i == k) {
                continue;
            }
            let found = match *item {
                Item::Folder(i) => at(self.folder_remove_rect(i), "Remove this search folder"),
                Item::Plugin(i) => at(
                    self.plugin_toggle_rect(i),
                    "Disable this plugin: it stays installed but is not offered",
                ),
                Item::Failed(i) => at(self.retry_rect(i), "Try this file again"),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => self.pointer_down(*pos, *modifiers, &mut out),
            UiEvent::Wheel { delta, .. } => {
                self.scroll -= delta.y * 40.0;
                self.clamp_scroll();
                self.repaint = true;
            }
            _ => {}
        }
        out
    }

    fn emit(out: &mut Vec<UiAction>, a: PluginSettingsAction) {
        out.push(UiAction::PluginSettings(a));
    }

    fn pointer_down(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if !self.bounds.contains(pos) {
            return;
        }
        if self.enable_rect().contains(pos) && self.find(Item::Header).is_some() {
            Self::emit(
                out,
                PluginSettingsAction::SetEnabled {
                    enabled: !self.data.enabled,
                },
            );
            return;
        }
        if self
            .sandbox_toggle_rect()
            .is_some_and(|r| r.inflate(4.0, 4.0).contains(pos))
        {
            Self::emit(out, PluginSettingsAction::SetSandbox(!self.data.sandbox));
            return;
        }
        if !self.data.enabled {
            return;
        }
        let scanning = self.data.scan.is_some();
        if self.add_folder_rect().is_some_and(|r| r.contains(pos)) {
            Self::emit(out, PluginSettingsAction::AddFolderDialog);
            return;
        }
        for (i, path) in self.data.extra_paths.clone().into_iter().enumerate() {
            if self.folder_remove_rect(i).is_some_and(|r| r.contains(pos)) {
                Self::emit(out, PluginSettingsAction::RemoveFolder(path));
                return;
            }
        }
        for (rect, mode) in [
            (self.scan_rect(), PluginRescan::Incremental),
            (self.full_rescan_rect(), PluginRescan::Full),
            (self.rescan_failed_rect(), PluginRescan::FailedOnly),
        ] {
            if rect.is_some_and(|r| r.contains(pos)) {
                if !scanning {
                    Self::emit(out, PluginSettingsAction::Scan(mode));
                }
                return;
            }
        }
        if self.cancel_scan_rect().is_some_and(|r| r.contains(pos)) {
            Self::emit(out, PluginSettingsAction::CancelScan);
            return;
        }
        let step = if modifiers.shift { 30 } else { 5 };
        let cur = self.data.timeout_secs;
        for (rect, next) in [
            (
                self.timeout_dec_rect(),
                cur.saturating_sub(step).max(MIN_TIMEOUT_SECS),
            ),
            (self.timeout_inc_rect(), (cur + step).min(MAX_TIMEOUT_SECS)),
        ] {
            if rect.is_some_and(|r| r.contains(pos)) {
                if next != cur {
                    Self::emit(out, PluginSettingsAction::SetTimeout(next));
                }
                return;
            }
        }
        for (i, row) in self.data.plugins.clone().into_iter().enumerate() {
            if self.plugin_toggle_rect(i).is_some_and(|r| r.contains(pos)) {
                Self::emit(
                    out,
                    PluginSettingsAction::SetPluginDisabled {
                        type_id: row.plugin.type_id(),
                        disabled: row.status != PluginEntryStatus::Disabled,
                    },
                );
                return;
            }
        }
        for (i, f) in self.data.failed.clone().into_iter().enumerate() {
            if self.retry_rect(i).is_some_and(|r| r.contains(pos)) {
                Self::emit(out, PluginSettingsAction::RetryFile(f.path));
                return;
            }
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the view (scaled by the scale factor, clipped to the bounds).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let b = self.bounds;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&b.into());
        crate::seq_paint::rect(c, b, self.st.background);
        for (idx, r) in self.visible() {
            self.paint_item(c, self.items[idx], r);
        }
        if self.focused {
            crate::seq_paint::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, self.st.base.focus);
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn hovered(&self, r: Rect) -> bool {
        self.hover.is_some_and(|p| r.contains(p))
    }

    fn paint_item(&self, c: &mut dyn Canvas, item: Item, r: Rect) {
        use crate::seq_paint as sp;
        let st = &self.st;
        let t = &st.base;
        let d = &self.data;
        let accent = t.highlight;
        let label =
            |c: &mut dyn Canvas, s: &str, x: f64, y: f64, size: f32, col: Color, al: Align| {
                sp::label(c, s, x, y, size, col, al);
            };
        match item {
            Item::Header => {
                label(c, "PLUGINS", r.x0, r.y0 + 22.0, 11.0, t.text, Align::Start);
                let e = self.enable_rect();
                sp::button(
                    c,
                    st,
                    e,
                    "ENABLE",
                    d.enabled.then_some(accent),
                    self.hovered(e),
                );
            }
            Item::Sandbox => {
                if let Some(tg) = self.sandbox_toggle_rect() {
                    c.stroke_with(RoundedRect::from_rect(tg, 2.0), 1.0, t.text_muted);
                    if d.sandbox {
                        c.fill_with(RoundedRect::from_rect(tg.inset(-3.0), 1.0), accent);
                    }
                }
                label(
                    c,
                    &fit_label(
                        "Run plugins in a separate process (recommended)",
                        r.width() - 26.0,
                        10.0,
                    ),
                    r.x0 + 26.0,
                    r.y0 + 15.0,
                    10.0,
                    t.text,
                    Align::Start,
                );
                label(
                    c,
                    "Applies after restarting Atomik",
                    r.x0 + 26.0,
                    r.y0 + 30.0,
                    9.0,
                    t.text_muted,
                    Align::Start,
                );
            }
            Item::OffHint => label(
                c,
                "Third-party plugins are off. Press ENABLE (top right) to scan folders and use plugins.",
                r.x0,
                r.y0 + 20.0,
                10.0,
                t.text_muted,
                Align::Start,
            ),
            Item::NoScanner => label(
                c,
                &fit_label(
                    "No plugin scanner can be run, so plugins cannot be scanned. Reinstall Atomik.",
                    r.width(),
                    10.0,
                ),
                r.x0,
                r.y0 + 20.0,
                10.0,
                Color::from_rgba8(255, 120, 96, 255),
                Align::Start,
            ),
            Item::ScanSummary => label(
                c,
                &fit_label(d.last_scan.as_deref().unwrap_or_default(), r.width(), 9.5),
                r.x0,
                r.y0 + 20.0,
                9.5,
                t.text,
                Align::Start,
            ),
            Item::Heading(s) => {
                let (text, n) = match s {
                    Section::Folders => ("SEARCH FOLDERS".to_string(), None),
                    Section::Scan => ("SCAN".to_string(), None),
                    Section::Plugins => ("FOUND PLUGINS".to_string(), Some(d.plugins.len())),
                    Section::FailedFiles => ("FAILED FILES".to_string(), Some(d.failed.len())),
                    Section::Log => ("PLUGIN LOG".to_string(), None),
                };
                let text = n.map_or(text.clone(), |n| format!("{text} ({n})"));
                label(c, &text, r.x0, r.y0 + 18.0, 8.5, t.text_muted, Align::Start);
                sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
            }
            Item::Folder(i) => {
                label(
                    c,
                    &fit_label(&d.extra_paths[i], r.width() - 40.0, 10.0),
                    r.x0 + 8.0,
                    r.y0 + 16.0,
                    10.0,
                    t.text,
                    Align::Start,
                );
                if let Some(x) = self.folder_remove_rect(i) {
                    sp::button(c, st, x, "x", None, self.hovered(x));
                }
            }
            Item::AddFolder => {
                if let Some(b) = self.add_folder_rect() {
                    sp::button(c, st, b, "ADD FOLDER...", None, self.hovered(b));
                }
            }
            Item::ScanButtons => {
                let scanning = d.scan.is_some();
                if let (Some(a), Some(f), Some(x)) = (
                    self.scan_rect(),
                    self.full_rescan_rect(),
                    self.rescan_failed_rect(),
                ) {
                    for (rect, text) in [(a, "SCAN"), (f, "FULL RESCAN"), (x, "RESCAN FAILED")] {
                        sp::button(c, st, rect, text, None, !scanning && self.hovered(rect));
                        if scanning {
                            c.fill_with(
                                RoundedRect::from_rect(rect, 3.0),
                                st.background.with_alpha(0.5),
                            );
                        }
                    }
                }
            }
            Item::Timeout => {
                label(
                    c,
                    "SCAN TIMEOUT",
                    r.x0,
                    r.y0 + 19.0,
                    9.0,
                    t.text_muted,
                    Align::Start,
                );
                if let (Some(dec), Some(inc)) = (self.timeout_dec_rect(), self.timeout_inc_rect()) {
                    label(
                        c,
                        &format!("{} s", d.timeout_secs),
                        dec.x0 - 8.0,
                        r.y0 + 19.0,
                        10.0,
                        t.text,
                        Align::End,
                    );
                    sp::button(c, st, dec, "-", None, self.hovered(dec));
                    sp::button(c, st, inc, "+", None, self.hovered(inc));
                }
            }
            Item::Progress => {
                let p = d.scan.unwrap_or_default();
                let bar = Rect::new(r.x0, r.y0 + 8.0, r.x0 + 300.0, r.y1 - 8.0);
                c.fill_with(RoundedRect::from_rect(bar, 3.0), t.lcd);
                c.stroke_with(
                    RoundedRect::from_rect(bar.inset(-0.5), 3.5),
                    1.0,
                    t.text_muted.with_alpha(0.35),
                );
                let frac = if p.total == 0 {
                    0.0
                } else {
                    (p.done as f64 / p.total as f64).clamp(0.0, 1.0)
                };
                c.fill_with(
                    RoundedRect::from_rect(
                        Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * frac, bar.y1),
                        3.0,
                    ),
                    accent,
                );
                let text = if p.total == 0 {
                    "Scanning...".to_string()
                } else {
                    format!("{} / {} files", p.done, p.total)
                };
                label(
                    c,
                    &text,
                    bar.x1 + 10.0,
                    r.y0 + 17.0,
                    9.5,
                    t.text,
                    Align::Start,
                );
                if let Some(x) = self.cancel_scan_rect() {
                    sp::button(c, st, x, "CANCEL", None, self.hovered(x));
                }
            }
            Item::Plugin(i) => {
                let row = &d.plugins[i];
                if let Some(tg) = self.plugin_toggle_rect(i) {
                    let off = row.status == PluginEntryStatus::Disabled;
                    c.stroke_with(RoundedRect::from_rect(tg, 2.0), 1.0, t.text_muted);
                    if off {
                        c.fill_with(RoundedRect::from_rect(tg.inset(-3.0), 1.0), accent);
                    }
                }
                let dim = row.status != PluginEntryStatus::Ok;
                let col = if dim { t.text_muted } else { t.text };
                let y = r.y0 + 15.0;
                label(
                    c,
                    &fit_label(&row.name, 170.0, 10.0),
                    r.x0 + 26.0,
                    y,
                    10.0,
                    col,
                    Align::Start,
                );
                label(
                    c,
                    &fit_label(&row.vendor, 130.0, 9.0),
                    r.x0 + 204.0,
                    y,
                    9.0,
                    t.text_muted,
                    Align::Start,
                );
                label(
                    c,
                    &fit_label(&row.version, 60.0, 9.0),
                    r.x0 + 344.0,
                    y,
                    9.0,
                    t.text_muted,
                    Align::Start,
                );
                label(
                    c,
                    format_badge(row.plugin.format),
                    r.x0 + 410.0,
                    y,
                    7.5,
                    accent,
                    Align::Start,
                );
                let status_col = match row.status {
                    PluginEntryStatus::Ok => accent,
                    PluginEntryStatus::Disabled => t.text_muted,
                    _ => Color::from_rgba8(255, 120, 96, 255),
                };
                label(
                    c,
                    row.status.label(),
                    r.x0 + 460.0,
                    y,
                    8.5,
                    status_col,
                    Align::Start,
                );
                if let Some(reason) = row.status.reason() {
                    label(
                        c,
                        &fit_label(reason, r.x1 - (r.x0 + 560.0), 9.0),
                        r.x0 + 560.0,
                        y,
                        9.0,
                        t.text_muted,
                        Align::Start,
                    );
                }
            }
            Item::Failed(i) => {
                let f = &d.failed[i];
                let name = f.path.rsplit(['/', '\\']).next().unwrap_or(&f.path);
                label(
                    c,
                    &fit_label(name, r.width() - 80.0, 10.0),
                    r.x0 + 8.0,
                    r.y0 + 14.0,
                    10.0,
                    t.text,
                    Align::Start,
                );
                label(
                    c,
                    &fit_label(&f.reason, r.width() - 80.0, 8.5),
                    r.x0 + 8.0,
                    r.y0 + 27.0,
                    8.5,
                    Color::from_rgba8(255, 120, 96, 255),
                    Align::Start,
                );
                if let Some(x) = self.retry_rect(i) {
                    sp::button(c, st, x, "RETRY", None, self.hovered(x));
                }
            }
            Item::Log(i) => label(
                c,
                &fit_label(&d.log[i], r.width() - 8.0, 8.5),
                r.x0 + 8.0,
                r.y0 + 11.0,
                8.5,
                t.text_muted,
                Align::Start,
            ),
        }
    }
}
