//! The plugin manager: find, organise and fix the installed plugins in one table.
//!
//! ```text
//! PLUGIN MANAGER                                                         [ CLOSE ]
//! [ search                  ]  [Format: ALL] [Status: ALL] [Category: ALL] [Tag: ALL]
//! *  Name          Vendor       Format  Category    Status        Path        Scanned
//! *  Example Synth Example Ven. VST3    Instrument  OK            /plugins/.. 2026-11-06
//!    Example Verb  Example Ven. CLAP    Effect      Quarantined   /plugins/.. 2026-11-06
//! Tag [          ] [ ADD TAG ]   pads x   warm x
//! [ DISABLE ] [ TRY AGAIN ] [ SHOW IN FOLDER ] [ RESCAN SELECTED ] [ RESCAN ALL ]
//! ```
//!
//! A modal view the app places over the window like the versions panel. The app gives it the
//! plugins as plain rows ([`PluginManagerData`]: the name, vendor, format, category and path come
//! from the plugin's own metadata, the status and the star and tags from what the user decided)
//! and answers its [`UiAction::PluginManager`]:
//!
//! * the star of a row (or Space) toggles the favourite: [`PluginManagerAction::Favourite`];
//! * DISABLE / ENABLE hides the chosen plugins from the browser and the menus (a project that
//!   uses one still loads it): [`PluginManagerAction::Disable`];
//! * TRY AGAIN takes quarantined plugins out of quarantine and scans failed files again:
//!   [`PluginManagerAction::TryAgain`] and [`PluginManagerAction::RescanFiles`];
//! * RESCAN SELECTED and RESCAN ALL start the scan worker, which shows its progress here (the
//!   same button then cancels it): [`PluginManagerAction::RescanFiles`], [`PluginManagerAction::RescanAll`],
//!   [`PluginManagerAction::CancelScan`];
//! * SHOW IN FOLDER asks the platform to open the file manager at the file;
//! * the tag field and ADD TAG tag the chosen plugins, a click on a tag of the first chosen plugin
//!   takes it away: [`PluginManagerAction::AddTag`], [`PluginManagerAction::RemoveTag`].
//!
//! The table sorts by a click on a column head (again to reverse it). The search field matches the
//! name, vendor, path and tags; the four filter buttons step through their choices on each click.
//! A click chooses a row, Ctrl (Cmd) toggles one, Shift chooses a range. The rows scroll with the
//! wheel and the arrow keys. The list geometry is the shared [`PickList`].

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::pick_list::{PAD as LIST_PAD, PickList, ROW_HEIGHT};
use crate::plugin_data::{PluginEntryStatus, PluginProcessChoice, ScanProgressInfo, format_badge};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_device_sdk::PluginFormat;
use std::collections::BTreeSet;
use vello::kurbo::{Affine, BezPath, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FILTER_H: f64 = 36.0;
const HEAD_H: f64 = 22.0;
/// How many rows the table shows.
pub const VISIBLE_ROWS: usize = 14;
const TAG_H: f64 = 34.0;
const ACTION_H: f64 = 40.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 920.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.0;
const LINE_H: f64 = 14.0;
/// The longest search text, in characters.
const MAX_QUERY_CHARS: usize = 80;

/// One row of the table: an installed plugin, or a plugin file that failed to scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManagerRow {
    /// The plugin's stable id (its type id); `None` for a file that failed to scan.
    pub type_id: Option<String>,
    /// Name (the file name for a failed file).
    pub name: String,
    /// Vendor, as the plugin says.
    pub vendor: String,
    /// VST3 or CLAP.
    pub format: PluginFormat,
    /// What the plugin says it is (instrument, effect, note effect).
    pub category: String,
    /// Usable, or why not.
    pub status: PluginEntryStatus,
    /// Which child process its instances go in (a click on the cell steps through the choices).
    pub process: PluginProcessChoice,
    /// The file that holds it.
    pub path: String,
    /// When it was scanned, seconds since the Unix epoch (0: not known).
    pub scanned_secs: u64,
    /// That time as text.
    pub scanned: String,
    /// The plugin has a star.
    pub favourite: bool,
    /// The user's tags.
    pub tags: Vec<String>,
}

impl PluginManagerRow {
    /// What selection and the app know the row by: the plugin id, or the path of a failed file.
    pub fn key(&self) -> String {
        self.type_id
            .clone()
            .unwrap_or_else(|| format!("file:{}", self.path))
    }
}

/// Everything the manager shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginManagerData {
    /// The rows, in any order (the view sorts them).
    pub rows: Vec<PluginManagerRow>,
    /// A scan is running, with its progress.
    pub scan: Option<ScanProgressInfo>,
    /// A scanner program can be run.
    pub scanner_available: bool,
    /// Third-party plugins are switched on.
    pub plugins_on: bool,
}

/// What the manager asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginManagerAction {
    /// Close the panel.
    Close,
    /// Set or clear the star of a plugin.
    Favourite {
        /// The plugin id.
        type_id: String,
        /// The new state.
        on: bool,
    },
    /// Hide the plugins from the browser and the menus, or show them again.
    Disable {
        /// The plugin ids.
        type_ids: Vec<String>,
        /// The new state.
        disabled: bool,
    },
    /// Take the plugins out of quarantine.
    TryAgain(Vec<String>),
    /// Choose the child process of one plugin's instances.
    SetProcess {
        /// The plugin id.
        type_id: String,
        /// The new choice.
        choice: PluginProcessChoice,
    },
    /// Add a tag to the plugins.
    AddTag {
        /// The plugin ids.
        type_ids: Vec<String>,
        /// The tag as typed.
        tag: String,
    },
    /// Take a tag off a plugin.
    RemoveTag {
        /// The plugin id.
        type_id: String,
        /// The tag.
        tag: String,
    },
    /// Scan these files again.
    RescanFiles(Vec<String>),
    /// Scan everything again.
    RescanAll,
    /// Stop the running scan.
    CancelScan,
    /// Open the file manager at this file.
    ShowInFolder(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Star,
    Name,
    Vendor,
    Format,
    Category,
    Status,
    Process,
    Path,
    Scanned,
}

impl Column {
    const ALL: [Self; 9] = [
        Self::Star,
        Self::Name,
        Self::Vendor,
        Self::Format,
        Self::Category,
        Self::Status,
        Self::Process,
        Self::Path,
        Self::Scanned,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Star => "",
            Self::Name => "NAME",
            Self::Vendor => "VENDOR",
            Self::Format => "FORMAT",
            Self::Category => "CATEGORY",
            Self::Status => "STATUS",
            Self::Process => "PROCESS",
            Self::Path => "PATH",
            Self::Scanned => "SCANNED",
        }
    }

    /// The fixed width; the path takes what is left.
    fn width(self) -> f64 {
        match self {
            Self::Star => 24.0,
            Self::Name => 170.0,
            Self::Vendor => 120.0,
            Self::Format => 56.0,
            Self::Category => 96.0,
            Self::Status => 150.0,
            Self::Process => 64.0,
            Self::Path => 0.0,
            Self::Scanned => 116.0,
        }
    }
}

/// The status filter's choices, in the order the button steps through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Every plugin.
    All,
    /// Plugins that can be used.
    Ok,
    /// Plugins and files that failed.
    Failed,
    /// Plugins the host cannot run.
    Unsupported,
    /// Quarantined plugins.
    Quarantined,
    /// Disabled plugins.
    Disabled,
}

impl StatusFilter {
    const ALL_CHOICES: [Self; 6] = [
        Self::All,
        Self::Ok,
        Self::Failed,
        Self::Unsupported,
        Self::Quarantined,
        Self::Disabled,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Ok => "OK",
            Self::Failed => "FAILED",
            Self::Unsupported => "UNSUPPORTED",
            Self::Quarantined => "QUARANTINED",
            Self::Disabled => "DISABLED",
        }
    }

    fn accepts(self, status: &PluginEntryStatus) -> bool {
        match self {
            Self::All => true,
            Self::Ok => *status == PluginEntryStatus::Ok,
            Self::Failed => matches!(
                status,
                PluginEntryStatus::Failed(_) | PluginEntryStatus::Crashed(_)
            ),
            Self::Unsupported => matches!(status, PluginEntryStatus::Unsupported(_)),
            Self::Quarantined => matches!(status, PluginEntryStatus::Quarantined(_)),
            Self::Disabled => *status == PluginEntryStatus::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Search,
    Tag,
    List,
}

/// The status as a word of the table.
fn status_word(status: &PluginEntryStatus) -> &'static str {
    match status {
        PluginEntryStatus::Ok => "OK",
        PluginEntryStatus::Unsupported(_) => "Unsupported",
        PluginEntryStatus::Disabled => "Disabled",
        PluginEntryStatus::Failed(_) => "Failed",
        PluginEntryStatus::Crashed(_) => "Crashed",
        PluginEntryStatus::Quarantined(_) => "Quarantined",
    }
}

/// The process choice as a word of the table.
fn process_word(choice: PluginProcessChoice) -> &'static str {
    match choice {
        PluginProcessChoice::Default => "default",
        PluginProcessChoice::Own => "own",
        PluginProcessChoice::Shared => "shared",
    }
}

/// The status as the table writes it: the word, and the reason after it when there is one.
fn status_text(status: &PluginEntryStatus) -> String {
    match status.reason() {
        Some(reason) => format!("{}: {reason}", status_word(status)),
        None => status_word(status).to_owned(),
    }
}

/// The plugin manager. See the module documentation.
#[derive(Debug)]
pub struct PluginManagerView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: PluginManagerData,
    search: TextField,
    tag: TextField,
    focus: Focus,
    dragging: Option<Focus>,
    format: Option<PluginFormat>,
    status: StatusFilter,
    category: Option<String>,
    tag_filter: Option<String>,
    sort: Column,
    ascending: bool,
    /// Indices into the rows that pass the filters, in sort order.
    visible: Vec<usize>,
    /// The first row shown.
    top: usize,
    selected: BTreeSet<String>,
    /// The row (a position in `visible`) the keys act from and a Shift range starts at.
    cursor: Option<usize>,
    /// Where a Shift range starts (a position in `visible`).
    anchor: Option<usize>,
    hover: Option<Point>,
    repaint: bool,
}

impl PluginManagerView {
    /// A manager showing `data`, with the keys on the search field; call [`Self::set_bounds`].
    pub fn new(theme: Theme, data: PluginManagerData) -> Self {
        let mut view = Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 700.0),
            scale: 1.0,
            data,
            search: TextField::new("", MAX_QUERY_CHARS, false),
            tag: TextField::new("", atomik_library::MAX_TAG_CHARS, false),
            focus: Focus::Search,
            dragging: None,
            format: None,
            status: StatusFilter::All,
            category: None,
            tag_filter: None,
            sort: Column::Name,
            ascending: true,
            visible: Vec::new(),
            top: 0,
            selected: BTreeSet::new(),
            cursor: None,
            anchor: None,
            hover: None,
            repaint: true,
        };
        view.refilter();
        view
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal and every key goes to it).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// What the view shows now.
    pub fn data(&self) -> &PluginManagerData {
        &self.data
    }

    /// Replaces the data. The choice stays on the rows that are still there; a filter whose
    /// value is gone (a tag nobody has any more) goes back to ALL.
    pub fn set_data(&mut self, data: PluginManagerData) {
        if self.data == data {
            return;
        }
        self.data = data;
        let keys: BTreeSet<String> = self.data.rows.iter().map(PluginManagerRow::key).collect();
        self.selected.retain(|k| keys.contains(k));
        if let Some(tag) = &self.tag_filter
            && !self.tags().contains(tag)
        {
            self.tag_filter = None;
        }
        if let Some(cat) = &self.category
            && !self.categories().contains(cat)
        {
            self.category = None;
        }
        self.refilter();
    }

    // ------------------------------------------------------------------ state

    /// The search text.
    pub fn query(&self) -> &str {
        self.search.text()
    }

    /// Types `text` in place of the search (tests and the headless driver).
    pub fn set_query(&mut self, text: &str) {
        self.search.set_text(text);
        self.refilter();
    }

    /// The text of the tag field.
    pub fn tag_text(&self) -> &str {
        self.tag.text()
    }

    /// Types `text` in the tag field (tests and the headless driver).
    pub fn set_tag_text(&mut self, text: &str) {
        self.tag.set_text(text);
        self.repaint = true;
    }

    /// The rows that pass the filters, in the order shown.
    pub fn shown(&self) -> Vec<&PluginManagerRow> {
        self.visible.iter().map(|i| &self.data.rows[*i]).collect()
    }

    /// The keys of the chosen rows.
    pub fn selected_keys(&self) -> Vec<String> {
        self.selected.iter().cloned().collect()
    }

    /// The format filter.
    pub fn format_filter(&self) -> Option<PluginFormat> {
        self.format
    }

    /// The status filter.
    pub fn status_filter(&self) -> StatusFilter {
        self.status
    }

    /// The category filter.
    pub fn category_filter(&self) -> Option<&str> {
        self.category.as_deref()
    }

    /// The tag filter.
    pub fn tag_filter(&self) -> Option<&str> {
        self.tag_filter.as_deref()
    }

    /// The column the table is sorted by (its title) and whether it ascends.
    pub fn sorted_by(&self) -> (&'static str, bool) {
        (
            if self.sort == Column::Star {
                "FAVOURITE"
            } else {
                self.sort.title()
            },
            self.ascending,
        )
    }

    /// The categories of the rows, by name.
    fn categories(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .data
            .rows
            .iter()
            .map(|r| r.category.clone())
            .filter(|c| !c.is_empty())
            .collect();
        all.sort_by_key(|c| c.to_lowercase());
        all.dedup();
        all
    }

    /// The tags in use, by name.
    fn tags(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .data
            .rows
            .iter()
            .flat_map(|r| r.tags.iter().cloned())
            .collect();
        all.sort_by_key(|t| t.to_lowercase());
        all.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        all
    }

    fn passes(&self, row: &PluginManagerRow) -> bool {
        if self.format.is_some_and(|f| f != row.format) {
            return false;
        }
        if !self.status.accepts(&row.status) {
            return false;
        }
        if self.category.as_ref().is_some_and(|c| *c != row.category) {
            return false;
        }
        if let Some(tag) = &self.tag_filter
            && !row.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
        {
            return false;
        }
        let q = self.search.text().trim().to_lowercase();
        q.is_empty()
            || row.name.to_lowercase().contains(&q)
            || row.vendor.to_lowercase().contains(&q)
            || row.path.to_lowercase().contains(&q)
            || row.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }

    fn compare(&self, a: &PluginManagerRow, b: &PluginManagerRow) -> std::cmp::Ordering {
        let by_name = |a: &PluginManagerRow, b: &PluginManagerRow| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.type_id.cmp(&b.type_id))
        };
        let key = match self.sort {
            Column::Star => b.favourite.cmp(&a.favourite),
            Column::Name => std::cmp::Ordering::Equal,
            Column::Vendor => a.vendor.to_lowercase().cmp(&b.vendor.to_lowercase()),
            Column::Format => a.format.cmp(&b.format),
            Column::Category => a.category.to_lowercase().cmp(&b.category.to_lowercase()),
            Column::Status => status_word(&a.status).cmp(status_word(&b.status)),
            Column::Process => process_word(a.process).cmp(process_word(b.process)),
            Column::Path => a.path.to_lowercase().cmp(&b.path.to_lowercase()),
            Column::Scanned => a.scanned_secs.cmp(&b.scanned_secs),
        };
        let ordered = key.then_with(|| by_name(a, b));
        if self.ascending {
            ordered
        } else {
            ordered.reverse()
        }
    }

    /// Rebuilds the visible rows after any change of the data, the filters or the sort. The
    /// cursor follows its row when it is still shown.
    fn refilter(&mut self) {
        let cursor_key = self.cursor_key();
        let mut visible: Vec<usize> = (0..self.data.rows.len())
            .filter(|i| self.passes(&self.data.rows[*i]))
            .collect();
        visible.sort_by(|a, b| self.compare(&self.data.rows[*a], &self.data.rows[*b]));
        self.visible = visible;
        self.cursor = cursor_key.and_then(|k| {
            self.visible
                .iter()
                .position(|i| self.data.rows[*i].key() == k)
        });
        self.anchor = None;
        self.top = self
            .top
            .min(self.visible.len().saturating_sub(VISIBLE_ROWS));
        self.repaint = true;
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor
            .and_then(|c| self.visible.get(c))
            .map(|i| self.data.rows[*i].key())
    }

    /// The chosen rows, in the order the table shows them.
    fn chosen(&self) -> Vec<&PluginManagerRow> {
        self.visible
            .iter()
            .map(|i| &self.data.rows[*i])
            .filter(|r| self.selected.contains(&r.key()))
            .collect()
    }

    fn chosen_ids(&self) -> Vec<String> {
        self.chosen()
            .iter()
            .filter_map(|r| r.type_id.clone())
            .collect()
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H + FILTER_H + HEAD_H + ROW_HEIGHT * VISIBLE_ROWS as f64 + TAG_H + ACTION_H + PAD
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    fn filter_y(&self) -> f64 {
        self.panel().y0 + HEADER_H + 6.0
    }

    /// The search field.
    pub fn search_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.filter_y();
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 220.0, y + BTN_H + 2.0)
    }

    fn filter_rects(&self) -> [Rect; 4] {
        let s = self.search_rect();
        let widths = [96.0, 150.0, 140.0, 130.0];
        let mut x = s.x1 + 10.0;
        widths.map(|w| {
            let r = Rect::new(x, s.y0 + 1.0, x + w, s.y0 + 1.0 + BTN_H);
            x += w + 8.0;
            r
        })
    }

    /// The format filter button.
    pub fn format_rect(&self) -> Rect {
        self.filter_rects()[0]
    }

    /// The status filter button.
    pub fn status_rect(&self) -> Rect {
        self.filter_rects()[1]
    }

    /// The category filter button.
    pub fn category_rect(&self) -> Rect {
        self.filter_rects()[2]
    }

    /// The tag filter button.
    pub fn tag_filter_rect(&self) -> Rect {
        self.filter_rects()[3]
    }

    fn head_y(&self) -> f64 {
        self.panel().y0 + HEADER_H + FILTER_H
    }

    fn columns(&self) -> [(Column, f64, f64); 9] {
        let p = self.panel();
        let width = p.width() - 2.0 * PAD;
        let fixed: f64 = Column::ALL.iter().map(|c| c.width()).sum();
        let path = (width - fixed).max(60.0);
        let mut x = p.x0 + PAD;
        Column::ALL.map(|c| {
            let w = if c == Column::Path { path } else { c.width() };
            let span = (c, x, x + w);
            x += w;
            span
        })
    }

    fn list(&self) -> PickList {
        let p = self.panel();
        PickList::new(
            Point::new(p.x0 + PAD - LIST_PAD, self.head_y() + HEAD_H - LIST_PAD),
            p.width() - 2.0 * PAD + 2.0 * LIST_PAD,
            VISIBLE_ROWS,
            self.visible.len(),
            self.top,
        )
    }

    /// The whole table body, the area that takes the wheel.
    pub fn table_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.head_y() + HEAD_H;
        Rect::new(
            p.x0 + PAD,
            y,
            p.x1 - PAD,
            y + ROW_HEIGHT * VISIBLE_ROWS as f64,
        )
    }

    /// The box of the row at `position` of the rows shown, `None` when it is scrolled out.
    pub fn row_rect(&self, position: usize) -> Option<Rect> {
        self.list().row_rect(position)
    }

    /// The centre of a row, for tests and the headless driver.
    pub fn row_center(&self, position: usize) -> Option<Point> {
        self.row_rect(position).map(|r| r.center())
    }

    /// The star of the row at `position`.
    pub fn star_center(&self, position: usize) -> Option<Point> {
        let r = self.row_rect(position)?;
        let (_, x0, x1) = self.columns()[0];
        Some(Point::new((x0 + x1) / 2.0, r.center().y))
    }

    /// The process cell of the row at `position`.
    pub fn process_center(&self, position: usize) -> Option<Point> {
        let r = self.row_rect(position)?;
        let (_, x0, x1) = self.columns()[6];
        Some(Point::new((x0 + x1) / 2.0, r.center().y))
    }

    /// The status cell of the row at `position`.
    pub fn status_center(&self, position: usize) -> Option<Point> {
        let r = self.row_rect(position)?;
        let (_, x0, x1) = self.columns()[5];
        Some(Point::new((x0 + x1) / 2.0, r.center().y))
    }

    /// The head of the column called `title` (NAME, VENDOR, FORMAT, CATEGORY, STATUS, PROCESS,
    /// PATH or SCANNED; an empty title is the star column).
    pub fn head_center(&self, title: &str) -> Option<Point> {
        self.columns()
            .iter()
            .find(|(c, ..)| c.title() == title)
            .map(|(_, x0, x1)| Point::new((x0 + x1) / 2.0, self.head_y() + HEAD_H / 2.0))
    }

    fn foot_y(&self) -> f64 {
        self.table_rect().y1
    }

    /// The tag field.
    pub fn tag_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.foot_y() + 6.0;
        Rect::new(p.x0 + PAD + 30.0, y, p.x0 + PAD + 190.0, y + BTN_H + 2.0)
    }

    /// The ADD TAG button.
    pub fn add_tag_rect(&self) -> Rect {
        let t = self.tag_rect();
        Rect::new(
            t.x1 + 8.0,
            t.y0 + 1.0,
            t.x1 + 8.0 + 84.0,
            t.y0 + 1.0 + BTN_H,
        )
    }

    /// The tags of the first chosen plugin as chips: the tag and its box.
    pub fn tag_chips(&self) -> Vec<(String, Rect)> {
        let Some(row) = self.chosen().into_iter().find(|r| r.type_id.is_some()) else {
            return Vec::new();
        };
        let a = self.add_tag_rect();
        let limit = self.panel().x1 - PAD;
        let mut x = a.x1 + 14.0;
        let mut chips = Vec::new();
        for tag in &row.tags {
            let w = (tag.chars().count() as f64 * 6.2 + 26.0).min(160.0);
            if x + w > limit {
                break;
            }
            chips.push((tag.clone(), Rect::new(x, a.y0, x + w, a.y1)));
            x += w + 6.0;
        }
        chips
    }

    fn action_y(&self) -> f64 {
        self.foot_y() + TAG_H + (ACTION_H - BTN_H) / 2.0 - 2.0
    }

    fn action_rects(&self) -> [Rect; 5] {
        let p = self.panel();
        let widths = [96.0, 100.0, 130.0, 150.0, 120.0];
        let mut x = p.x0 + PAD;
        let y = self.action_y();
        widths.map(|w| {
            let r = Rect::new(x, y, x + w, y + BTN_H);
            x += w + 8.0;
            r
        })
    }

    /// The DISABLE / ENABLE button.
    pub fn disable_rect(&self) -> Rect {
        self.action_rects()[0]
    }

    /// The TRY AGAIN button.
    pub fn try_again_rect(&self) -> Rect {
        self.action_rects()[1]
    }

    /// The SHOW IN FOLDER button.
    pub fn folder_rect(&self) -> Rect {
        self.action_rects()[2]
    }

    /// The RESCAN SELECTED button.
    pub fn rescan_selected_rect(&self) -> Rect {
        self.action_rects()[3]
    }

    /// The RESCAN ALL button (CANCEL while a scan runs).
    pub fn rescan_all_rect(&self) -> Rect {
        self.action_rects()[4]
    }

    // ------------------------------------------------------------------ what the buttons do

    /// Whether the chosen plugins are all hidden by the user already (the button then enables).
    fn disable_enables(&self) -> bool {
        let rows: Vec<&PluginManagerRow> = self
            .chosen()
            .into_iter()
            .filter(|r| r.type_id.is_some())
            .collect();
        !rows.is_empty() && rows.iter().all(|r| r.status == PluginEntryStatus::Disabled)
    }

    fn can_disable(&self) -> bool {
        !self.chosen_ids().is_empty()
    }

    fn try_again_targets(&self) -> (Vec<String>, Vec<String>) {
        let mut ids = Vec::new();
        let mut files = Vec::new();
        for row in self.chosen() {
            match (&row.type_id, &row.status) {
                (Some(id), PluginEntryStatus::Quarantined(_) | PluginEntryStatus::Crashed(_)) => {
                    ids.push(id.clone());
                }
                (None, _) | (Some(_), PluginEntryStatus::Failed(_))
                    if !files.contains(&row.path) =>
                {
                    files.push(row.path.clone());
                }
                _ => {}
            }
        }
        (ids, files)
    }

    fn can_try_again(&self) -> bool {
        let (ids, files) = self.try_again_targets();
        !ids.is_empty() || !files.is_empty()
    }

    fn chosen_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for row in self.chosen() {
            if !row.path.is_empty() && !paths.contains(&row.path) {
                paths.push(row.path.clone());
            }
        }
        paths
    }

    fn disable_action(&self) -> Option<UiAction> {
        self.can_disable().then(|| {
            act(PluginManagerAction::Disable {
                type_ids: self.chosen_ids(),
                disabled: !self.disable_enables(),
            })
        })
    }

    fn try_again_actions(&self) -> Vec<UiAction> {
        let (ids, files) = self.try_again_targets();
        let mut out = Vec::new();
        if !ids.is_empty() {
            out.push(act(PluginManagerAction::TryAgain(ids)));
        }
        if !files.is_empty() {
            out.push(act(PluginManagerAction::RescanFiles(files)));
        }
        out
    }

    fn add_tag_action(&mut self) -> Option<UiAction> {
        let ids = self.chosen_ids();
        let tag = self.tag.text().trim().to_owned();
        if ids.is_empty() || tag.is_empty() {
            return None;
        }
        self.tag.set_text("");
        Some(act(PluginManagerAction::AddTag { type_ids: ids, tag }))
    }

    // ------------------------------------------------------------------ choosing

    fn cursor_to(&mut self, at: usize, extend: bool, toggle: bool) {
        let Some(row) = self.visible.get(at).map(|i| self.data.rows[*i].key()) else {
            return;
        };
        if extend && let Some(from) = self.anchor.or(self.cursor) {
            // The range runs from where the last plain choice was to the row now: the cursor
            // moves, the start stays.
            let (a, b) = (from.min(at), from.max(at));
            self.selected = (a..=b)
                .filter_map(|p| self.visible.get(p))
                .map(|i| self.data.rows[*i].key())
                .collect();
            self.cursor = Some(at);
        } else if toggle {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
            self.cursor = Some(at);
            self.anchor = Some(at);
        } else {
            self.selected = BTreeSet::from([row]);
            self.cursor = Some(at);
            self.anchor = Some(at);
        }
        self.show_cursor();
        self.repaint = true;
    }

    fn show_cursor(&mut self) {
        let Some(at) = self.cursor else { return };
        if at < self.top {
            self.top = at;
        } else if at >= self.top + VISIBLE_ROWS {
            self.top = at + 1 - VISIBLE_ROWS;
        }
        self.top = self
            .top
            .min(self.visible.len().saturating_sub(VISIBLE_ROWS));
    }

    fn cycle_format(&mut self) {
        self.format = match self.format {
            None => Some(PluginFormat::Vst3),
            Some(PluginFormat::Vst3) => Some(PluginFormat::Clap),
            Some(PluginFormat::Clap) => None,
        };
        self.refilter();
    }

    fn cycle_status(&mut self) {
        let at = StatusFilter::ALL_CHOICES
            .iter()
            .position(|s| *s == self.status)
            .unwrap_or(0);
        self.status = StatusFilter::ALL_CHOICES[(at + 1) % StatusFilter::ALL_CHOICES.len()];
        self.refilter();
    }

    fn cycle_category(&mut self) {
        let all = self.categories();
        self.category = match &self.category {
            None => all.first().cloned(),
            Some(c) => all
                .iter()
                .position(|x| x == c)
                .and_then(|i| all.get(i + 1))
                .cloned(),
        };
        self.refilter();
    }

    fn cycle_tag(&mut self) {
        let all = self.tags();
        self.tag_filter = match &self.tag_filter {
            None => all.first().cloned(),
            Some(t) => all
                .iter()
                .position(|x| x == t)
                .and_then(|i| all.get(i + 1))
                .cloned(),
        };
        self.refilter();
    }

    fn sort_by(&mut self, column: Column) {
        if self.sort == column {
            self.ascending = !self.ascending;
        } else {
            self.sort = column;
            self.ascending = true;
        }
        self.refilter();
    }

    fn toggle_star(&self, position: usize) -> Option<UiAction> {
        let row = self.visible.get(position).map(|i| &self.data.rows[*i])?;
        let type_id = row.type_id.clone()?;
        Some(act(PluginManagerAction::Favourite {
            type_id,
            on: !row.favourite,
        }))
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let tip = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        let controls = [
            (self.close_rect(), "Close the plugin manager"),
            (
                self.search_rect(),
                "Find plugins by name, vendor, path or tag",
            ),
            (
                self.format_rect(),
                "Show only VST3 or only CLAP plugins; click to step",
            ),
            (
                self.status_rect(),
                "Show plugins by status; click to step through them",
            ),
            (
                self.category_rect(),
                "Show plugins of one category; click to step",
            ),
            (
                self.tag_filter_rect(),
                "Show plugins with one tag; click to step",
            ),
            (
                self.add_tag_rect(),
                "Tag the chosen plugins with the text in the field",
            ),
            (
                self.disable_rect(),
                "Hide the chosen plugins from the browser; projects that use them still load",
            ),
            (
                self.try_again_rect(),
                "Take quarantined plugins out of quarantine and scan failed files again",
            ),
            (
                self.folder_rect(),
                "Open the file manager at the chosen plugin's file",
            ),
            (
                self.rescan_selected_rect(),
                "Scan the files of the chosen plugins again",
            ),
        ];
        for (rect, text) in controls {
            if rect.contains(pos) {
                return tip(rect, text);
            }
        }
        if self.rescan_all_rect().contains(pos) {
            let r = self.rescan_all_rect();
            return tip(
                r,
                if self.data.scan.is_some() {
                    "Stop the scan"
                } else {
                    "Scan every plugin folder again"
                },
            );
        }
        if let Some((_, rect)) = self.tag_chips().into_iter().find(|(_, r)| r.contains(pos)) {
            return tip(rect, "Click to take this tag off the plugin");
        }
        for (column, x0, x1) in self.columns() {
            let head = Rect::new(x0, self.head_y(), x1, self.head_y() + HEAD_H);
            if head.contains(pos) {
                return tip(
                    head,
                    if column == Column::Star {
                        "Sort with the favourites first; click again to reverse"
                    } else {
                        "Sort by this column; click again to reverse"
                    },
                );
            }
        }
        let at = self.list().row_at(pos)?;
        let row = self.visible.get(at).map(|i| &self.data.rows[*i])?;
        let rect = self.row_rect(at)?;
        let status_x = self.columns()[5];
        if pos.x >= status_x.1 && pos.x < status_x.2 && row.status.reason().is_some() {
            return Some(Help::new(rect, status_text(&row.status)));
        }
        let process_x = self.columns()[6];
        if pos.x >= process_x.1 && pos.x < process_x.2 && row.type_id.is_some() {
            return tip(
                rect,
                "Click to step: default (the Settings choice), own process, shared process",
            );
        }
        if pos.x < self.columns()[0].2 && row.type_id.is_some() {
            return tip(rect, "Favourites are listed first in the browser");
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
                match self.dragging {
                    Some(Focus::Search) => self.search.drag(*pos),
                    Some(Focus::Tag) => self.tag.drag(*pos),
                    _ => {}
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging = None,
            UiEvent::Wheel { delta, pos, .. } => {
                if self.table_rect().contains(*pos) && delta.y != 0.0 {
                    let mut list = self.list();
                    if list.wheel(delta.y) {
                        self.top = list.top;
                        self.repaint = true;
                    }
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                out.extend(self.press(*pos, *modifiers, *clicks));
            }
            UiEvent::Ime(ime) => {
                let changed = match self.focus {
                    Focus::Search => self.search.handle_ime(ime),
                    Focus::Tag => self.tag.handle_ime(ime),
                    Focus::List => false,
                };
                if changed {
                    if self.focus == Focus::Search {
                        self.refilter();
                    }
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                self.repaint = true;
                out.extend(self.key(key, *modifiers));
            }
            _ => {}
        }
        out
    }

    fn press(
        &mut self,
        pos: Point,
        modifiers: crate::event::Modifiers,
        clicks: u8,
    ) -> Vec<UiAction> {
        let mut out = Vec::new();
        if self.close_rect().contains(pos) {
            out.push(act(PluginManagerAction::Close));
        } else if self.search_rect().contains(pos) {
            self.focus = Focus::Search;
            self.dragging = Some(Focus::Search);
            self.search.press(pos, clicks, modifiers.shift);
        } else if self.tag_rect().contains(pos) {
            self.focus = Focus::Tag;
            self.dragging = Some(Focus::Tag);
            self.tag.press(pos, clicks, modifiers.shift);
        } else if self.format_rect().contains(pos) {
            self.cycle_format();
        } else if self.status_rect().contains(pos) {
            self.cycle_status();
        } else if self.category_rect().contains(pos) {
            self.cycle_category();
        } else if self.tag_filter_rect().contains(pos) {
            self.cycle_tag();
        } else if self.add_tag_rect().contains(pos) {
            out.extend(self.add_tag_action());
        } else if self.disable_rect().contains(pos) {
            out.extend(self.disable_action());
        } else if self.try_again_rect().contains(pos) {
            out.extend(self.try_again_actions());
        } else if self.folder_rect().contains(pos) {
            if let Some(path) = self.chosen_paths().into_iter().next() {
                out.push(act(PluginManagerAction::ShowInFolder(path)));
            }
        } else if self.rescan_selected_rect().contains(pos) {
            let paths = self.chosen_paths();
            if !paths.is_empty() && self.data.scan.is_none() {
                out.push(act(PluginManagerAction::RescanFiles(paths)));
            }
        } else if self.rescan_all_rect().contains(pos) {
            out.push(act(if self.data.scan.is_some() {
                PluginManagerAction::CancelScan
            } else {
                PluginManagerAction::RescanAll
            }));
        } else if let Some((tag, _)) = self.tag_chips().into_iter().find(|(_, r)| r.contains(pos)) {
            if let Some(row) = self.chosen().into_iter().find(|r| r.type_id.is_some())
                && let Some(type_id) = row.type_id.clone()
            {
                out.push(act(PluginManagerAction::RemoveTag { type_id, tag }));
            }
        } else if let Some((column, ..)) = self.columns().into_iter().find(|(_, x0, x1)| {
            Rect::new(*x0, self.head_y(), *x1, self.head_y() + HEAD_H).contains(pos)
        }) {
            self.sort_by(column);
        } else if let Some(at) = self.list().row_at(pos) {
            self.focus = Focus::List;
            let star = self.columns()[0];
            let process = self.columns()[6];
            if pos.x >= star.1 && pos.x < star.2 {
                out.extend(self.toggle_star(at));
            } else if pos.x >= process.1
                && pos.x < process.2
                && let Some(row) = self.visible.get(at).map(|i| &self.data.rows[*i])
                && let Some(type_id) = row.type_id.clone()
            {
                out.push(UiAction::PluginManager(PluginManagerAction::SetProcess {
                    type_id,
                    choice: row.process.next(),
                }));
            } else {
                self.cursor_to(at, modifiers.shift, modifiers.command());
            }
        }
        out
    }

    fn key(&mut self, key: &Key, m: crate::event::Modifiers) -> Vec<UiAction> {
        let mut out = Vec::new();
        if *key == Key::Tab {
            self.focus = match (self.focus, m.shift) {
                (Focus::Search, false) | (Focus::List, true) => Focus::Tag,
                (Focus::Tag, false) | (Focus::Search, true) => Focus::List,
                (Focus::List, false) | (Focus::Tag, true) => Focus::Search,
            };
            return out;
        }
        match self.focus {
            Focus::Search => {
                let before = self.search.text().to_owned();
                if self.search.handle_key(key, m).handled() {
                    if self.search.text() != before {
                        self.refilter();
                    }
                } else if matches!(key, Key::Down | Key::Enter) {
                    self.focus = Focus::List;
                    if self.cursor.is_none() && !self.visible.is_empty() {
                        self.cursor_to(0, false, false);
                    }
                }
            }
            Focus::Tag => {
                if self.tag.handle_key(key, m).handled() {
                    return out;
                }
                match key {
                    Key::Enter => out.extend(self.add_tag_action()),
                    Key::Up => self.focus = Focus::Search,
                    Key::Down => self.focus = Focus::List,
                    _ => {}
                }
            }
            Focus::List => out.extend(self.list_key(key, m)),
        }
        out
    }

    fn list_key(&mut self, key: &Key, m: crate::event::Modifiers) -> Vec<UiAction> {
        let last = self.visible.len().saturating_sub(1);
        let at = self.cursor.unwrap_or(0);
        let extend = m.shift;
        match key {
            Key::Up if self.cursor.is_none() => self.cursor_to(0, false, false),
            Key::Down if self.cursor.is_none() => self.cursor_to(0, false, false),
            Key::Up if at == 0 && !extend => self.focus = Focus::Search,
            Key::Up => self.cursor_to(at.saturating_sub(1), extend, false),
            Key::Down => self.cursor_to((at + 1).min(last), extend, false),
            Key::PageUp => self.cursor_to(at.saturating_sub(VISIBLE_ROWS), extend, false),
            Key::PageDown => self.cursor_to((at + VISIBLE_ROWS).min(last), extend, false),
            Key::Home => self.cursor_to(0, extend, false),
            Key::End => self.cursor_to(last, extend, false),
            Key::Space => {
                if let Some(c) = self.cursor {
                    return self.toggle_star(c).into_iter().collect();
                }
            }
            Key::Char('a' | 'A') if m.command() => {
                self.selected = self
                    .visible
                    .iter()
                    .map(|i| self.data.rows[*i].key())
                    .collect();
            }
            _ => {}
        }
        Vec::new()
    }

    // ------------------------------------------------------------------ painting

    fn paint_star(&self, c: &mut dyn Canvas, centre: Point, on: bool) {
        let t = &self.st.base;
        let mut path = BezPath::new();
        for i in 0..10 {
            let angle = std::f64::consts::PI * (f64::from(i) * 0.2 - 0.5);
            let r = if i % 2 == 0 { 5.5 } else { 2.3 };
            let p = Point::new(centre.x + r * angle.cos(), centre.y + r * angle.sin());
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        if on {
            c.fill_with(path, t.highlight);
        } else {
            c.stroke_with(path, 1.0, t.text_muted);
        }
    }

    fn status_color(&self, status: &PluginEntryStatus) -> vello::peniko::Color {
        let t = &self.st.base;
        match status {
            PluginEntryStatus::Ok => t.text,
            PluginEntryStatus::Failed(_)
            | PluginEntryStatus::Crashed(_)
            | PluginEntryStatus::Quarantined(_) => mix(t.text, t.focus, 0.35),
            PluginEntryStatus::Unsupported(_) | PluginEntryStatus::Disabled => t.text_muted,
        }
    }

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "PLUGIN MANAGER",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let count = format!("{} of {} shown", self.visible.len(), self.data.rows.len());
        sp::label(
            c,
            &count,
            self.close_rect().x0 - 14.0,
            p.y0 + 21.0,
            9.5,
            t.text_muted,
            Align::End,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));

        self.paint_filters(c, &hovered);
        self.paint_table(c, &hovered);
        self.paint_foot(c, &hovered);
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_filters(&self, c: &mut dyn Canvas, hovered: &dyn Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let search = self.search_rect();
        let in_search = self.focus == Focus::Search;
        sp::lcd(c, st, search, in_search.then_some(t.focus));
        if self.search.is_empty() && self.search.preedit().is_none() {
            sp::label(
                c,
                "Search plugins",
                search.x0 + 8.0,
                search.y0 + 15.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        self.search
            .paint(c, search, FONT, LINE_H, self.field_colors(), in_search);
        let labels = [
            format!("Format: {}", self.format.map_or("ALL", format_badge)),
            format!("Status: {}", self.status.label()),
            format!("Category: {}", self.category.as_deref().unwrap_or("ALL")),
            format!("Tag: {}", self.tag_filter.as_deref().unwrap_or("ALL")),
        ];
        let lit = [
            self.format.is_some(),
            self.status != StatusFilter::All,
            self.category.is_some(),
            self.tag_filter.is_some(),
        ];
        for ((rect, label), lit) in self.filter_rects().into_iter().zip(labels).zip(lit) {
            let shown = fit_label(&label, rect.width() - 10.0, 9.5);
            sp::button(
                c,
                st,
                rect,
                &shown,
                lit.then_some(st.loop_on),
                hovered(rect),
            );
        }
    }

    fn field_colors(&self) -> FieldColors {
        let t = &self.st.base;
        FieldColors {
            text: t.text,
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        }
    }

    fn paint_table(&self, c: &mut dyn Canvas, hovered: &dyn Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let in_list = self.focus == Focus::List;
        let table = self.table_rect();
        // The column heads.
        for (column, x0, x1) in self.columns() {
            let head = Rect::new(x0, self.head_y(), x1, self.head_y() + HEAD_H);
            if hovered(head) {
                sp::rect(c, head, mix(t.lcd, t.text, 0.08));
            }
            let mark = if self.sort == column {
                if self.ascending { " ^" } else { " v" }
            } else {
                ""
            };
            if column == Column::Star {
                self.paint_star(c, head.center(), self.sort == column);
            } else {
                let text = format!("{}{mark}", column.title());
                let ink = if self.sort == column {
                    t.text
                } else {
                    t.text_muted
                };
                sp::label(
                    c,
                    &fit_label(&text, head.width() - 8.0, 9.0),
                    head.x0 + 4.0,
                    head.y0 + 15.0,
                    9.0,
                    ink,
                    Align::Start,
                );
            }
        }
        sp::lcd(c, st, table, in_list.then_some(t.focus));
        c.push_clip(&table.into());
        let list = self.list();
        let columns = self.columns();
        for position in list.visible() {
            let Some(row) = self.visible.get(position).map(|i| &self.data.rows[*i]) else {
                break;
            };
            let Some(r) = list.row_rect(position) else {
                continue;
            };
            let chosen = self.selected.contains(&row.key());
            if chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if hovered(r) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            if self.cursor == Some(position) && in_list {
                sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, t.focus);
            }
            let ink = if row.status == PluginEntryStatus::Ok || chosen {
                t.text
            } else {
                t.text_muted
            };
            let y = r.y0 + 14.0;
            for (column, x0, x1) in columns {
                let width = x1 - x0 - 8.0;
                let text = match column {
                    Column::Star => {
                        if row.type_id.is_some() {
                            self.paint_star(
                                c,
                                Point::new((x0 + x1) / 2.0, r.center().y),
                                row.favourite,
                            );
                        }
                        continue;
                    }
                    Column::Name => row.name.clone(),
                    Column::Vendor => row.vendor.clone(),
                    Column::Format => format_badge(row.format).to_owned(),
                    Column::Category => row.category.clone(),
                    Column::Status => {
                        sp::label(
                            c,
                            &fit_label(&status_text(&row.status), width, FONT),
                            x0 + 4.0,
                            y,
                            FONT,
                            self.status_color(&row.status),
                            Align::Start,
                        );
                        continue;
                    }
                    Column::Process => {
                        if row.type_id.is_none() {
                            continue;
                        }
                        process_word(row.process).to_owned()
                    }
                    Column::Path => row.path.clone(),
                    Column::Scanned => row.scanned.clone(),
                };
                sp::label(
                    c,
                    &fit_label(&text, width, FONT),
                    x0 + 4.0,
                    y,
                    FONT,
                    ink,
                    Align::Start,
                );
            }
        }
        if self.visible.is_empty() {
            let text = if !self.data.plugins_on {
                "Third-party plugins are off: turn them on in Settings, Plugins"
            } else if self.data.rows.is_empty() {
                "No plugins found yet: RESCAN ALL looks in every plugin folder"
            } else {
                "No plugin matches the search and the filters"
            };
            sp::label(
                c,
                text,
                table.x0 + 8.0,
                table.y0 + 18.0,
                FONT + 0.5,
                t.text_muted,
                Align::Start,
            );
        }
        c.pop_clip();
    }

    fn paint_foot(&self, c: &mut dyn Canvas, hovered: &dyn Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let tag = self.tag_rect();
        sp::label(
            c,
            "Tag",
            p.x0 + PAD,
            tag.y0 + 15.0,
            FONT,
            t.text,
            Align::Start,
        );
        let in_tag = self.focus == Focus::Tag;
        sp::lcd(c, st, tag, in_tag.then_some(t.focus));
        self.tag
            .paint(c, tag, FONT, LINE_H, self.field_colors(), in_tag);
        let can_tag = !self.chosen_ids().is_empty() && !self.tag.text().trim().is_empty();
        let add = self.add_tag_rect();
        sp::button(
            c,
            st,
            add,
            "ADD TAG",
            can_tag.then_some(st.loop_on),
            can_tag && hovered(add),
        );
        for (name, rect) in self.tag_chips() {
            sp::button(c, st, rect, "", None, hovered(rect));
            sp::label(
                c,
                &fit_label(&format!("{name}  x"), rect.width() - 10.0, 9.0),
                rect.x0 + 7.0,
                rect.y0 + 15.0,
                9.0,
                t.text,
                Align::Start,
            );
        }
        let chosen = self.chosen();
        let any = !chosen.is_empty();
        let disable = self.disable_rect();
        let label = if self.disable_enables() {
            "ENABLE"
        } else {
            "DISABLE"
        };
        self.paint_action(c, disable, label, self.can_disable(), hovered);
        self.paint_action(
            c,
            self.try_again_rect(),
            "TRY AGAIN",
            self.can_try_again(),
            hovered,
        );
        self.paint_action(c, self.folder_rect(), "SHOW IN FOLDER", any, hovered);
        self.paint_action(
            c,
            self.rescan_selected_rect(),
            "RESCAN SELECTED",
            any && self.data.scan.is_none(),
            hovered,
        );
        let all = self.rescan_all_rect();
        let scanning = self.data.scan.is_some();
        sp::button(
            c,
            st,
            all,
            if scanning { "CANCEL" } else { "RESCAN ALL" },
            scanning.then_some(st.loop_on),
            hovered(all),
        );
        let after = all.x1 + 14.0;
        let note = match (self.data.scan, self.data.scanner_available) {
            (Some(ScanProgressInfo { done, total }), _) => format!("Scanning {done} of {total}"),
            (None, false) => "No plugin scanner can be run".to_owned(),
            (None, true) => {
                let chosen = chosen.len();
                if chosen > 1 {
                    format!("{chosen} chosen")
                } else {
                    "Ctrl-click and Shift-click choose several".to_owned()
                }
            }
        };
        sp::label(
            c,
            &fit_label(&note, p.x1 - PAD - after, 9.5),
            after,
            all.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }

    fn paint_action(
        &self,
        c: &mut dyn Canvas,
        rect: Rect,
        text: &str,
        enabled: bool,
        hovered: &dyn Fn(Rect) -> bool,
    ) {
        let t = &self.st.base;
        if enabled {
            sp::button(c, &self.st, rect, text, None, hovered(rect));
        } else {
            sp::button(c, &self.st, rect, "", None, false);
            sp::label(
                c,
                text,
                rect.center().x,
                rect.center().y + 3.5,
                9.5,
                t.text_muted,
                Align::Center,
            );
        }
    }
}

fn act(a: PluginManagerAction) -> UiAction {
    UiAction::PluginManager(a)
}
