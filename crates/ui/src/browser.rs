//! The device browser side panel: registered device types with search-as-you-type.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::plugin_data::{FailedFileInfo, PluginEntryInfo, PluginEntryStatus, format_badge};
use crate::theme::Theme;
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, PluginKindHint, Registry};
use vello::kurbo::{Circle, Line, Point, Rect, RoundedRect};

/// Width of the browser panel in logical units.
pub const BROWSER_WIDTH: f64 = 220.0;
const SEARCH: Rect = Rect::new(12.0, 36.0, 208.0, 62.0);
const ROWS_Y: f64 = 74.0;
const ROW_H: f64 = 36.0;

/// Type ids of the note players (step sequencer, arpeggiator): the browser lists them together
/// at the end as the "Players" group. Mirrors `atomik_devices::PLAYER_TYPE_IDS` (the UI does not
/// depend on the devices crate; a test pins the two lists together).
pub const PLAYER_TYPE_IDS: [&str; 2] = ["atomik.arpeggiator", "atomik.step-sequencer"];

/// Type ids of the modulators (CV sources): the browser lists them together as the "Modulators"
/// group, after the other devices and before the players. Mirrors
/// `atomik_devices::MODULATOR_TYPE_IDS` (a test pins the two lists together).
pub const MODULATOR_TYPE_IDS: [&str; 1] = ["atomik.mod-source"];

/// Whether `type_id` is a modulator (see [`MODULATOR_TYPE_IDS`]).
pub fn is_modulator(type_id: &str) -> bool {
    MODULATOR_TYPE_IDS.contains(&type_id)
}

/// Type ids of the internal devices (created by the container expansion, never offered by the
/// browser or an add menu). Mirrors `atomik_devices::INTERNAL_TYPE_IDS` (a test pins the two).
pub const INTERNAL_TYPE_IDS: [&str; 1] = ["atomik.note-split"];

/// Whether `type_id` is internal (see [`INTERNAL_TYPE_IDS`]).
pub fn is_internal(type_id: &str) -> bool {
    INTERNAL_TYPE_IDS.contains(&type_id)
}

/// Type id of the container device (the "Containers" group of the browser).
pub const CONTAINER_TYPE: &str = atomik_model::CONTAINER_TYPE;

/// The hint the browser shows where no container can be added (the nesting limit).
pub const NESTING_HINT: &str = "Containers nest 3 levels deep at most";

/// Whether `type_id` is a note player (listed under "Players").
pub fn is_player(type_id: &str) -> bool {
    PLAYER_TYPE_IDS.contains(&type_id)
}

/// What the browser was opened to create: the list shows only that kind,
/// Enter creates the highlighted device and Esc cancels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKind {
    /// An instrument (Create Instrument).
    Instrument,
    /// An effect or utility (Create Effect).
    Effect,
}

impl CreateKind {
    /// The panel title in create mode.
    pub fn title(self) -> &'static str {
        match self {
            CreateKind::Instrument => "CREATE INSTRUMENT",
            CreateKind::Effect => "CREATE EFFECT",
        }
    }

    /// Whether a device type is offered for this kind.
    pub fn accepts(self, desc: &DeviceDescriptor) -> bool {
        match self {
            CreateKind::Instrument => desc.kind == DeviceKind::Instrument,
            CreateKind::Effect => matches!(desc.kind, DeviceKind::Effect | DeviceKind::Utility),
        }
    }

    /// Whether a plugin of this hint is offered for this kind.
    pub fn accepts_plugin(self, hint: PluginKindHint) -> bool {
        match self {
            CreateKind::Instrument => hint == PluginKindHint::Instrument,
            CreateKind::Effect => hint == PluginKindHint::Effect,
        }
    }
}

/// The send entry of the effect create browser: with the Master Section
/// selected, a created effect goes to the first free send/return slot, which the entry names;
/// with all eight slots used it is greyed and the effect is only added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendEntry {
    /// The effect will be routed to this slot (0-based; shown as `slot + 1`).
    Slot(usize),
    /// All eight slots are in use: the entry is greyed.
    Full,
}

impl SendEntry {
    /// The entry text.
    pub fn text(self) -> String {
        match self {
            SendEntry::Slot(k) => format!("Routed to send/return {}", k + 1),
            SendEntry::Full => atomik_model::SEND_SLOTS_FULL.to_string(),
        }
    }
}

/// Browser state.
#[derive(Debug, Clone, Default)]
pub struct Browser {
    /// Create mode: only this kind is listed (`None`: the normal browser).
    pub create: Option<CreateKind>,
    /// In create mode, the browser stays open after a device was created (the Workflow
    /// setting "Create keeps the browser open").
    pub keep_open: bool,
    /// Shown.
    pub visible: bool,
    /// Search text.
    pub query: String,
    /// The search field has keyboard focus.
    pub focused: bool,
    /// Highlighted row.
    pub highlighted: usize,
    /// Row under the pointer.
    pub hover: Option<usize>,
    /// Scroll offset of the list.
    pub scroll: f64,
    /// List the console device types (`atomik.channel`, `atomik.master`) too. Off by default:
    /// console devices are created by the console, not from the browser.
    pub show_console: bool,
    /// How many containers deep the rack view is (0 at the top level). Inside a container the
    /// browser offers only the device types a container may hold: no console devices, and no
    /// container once [`atomik_model::MAX_CONTAINER_DEPTH`] is reached.
    pub scope_depth: usize,
    /// The installed plugins: the "Plugins" group lists the usable ones.
    pub plugins: Vec<PluginEntryInfo>,
    /// The plugin files that failed to scan, listed greyed at the end of the group.
    pub plugin_failed: Vec<FailedFileInfo>,
}

/// One row of the browser list: the device types, then the plugin group.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserRow {
    /// A registered device type (selectable).
    Device(&'static DeviceDescriptor),
    /// A group heading (not selectable).
    Heading(String),
    /// An installed, usable plugin: an index into [`Browser::plugins`] (selectable).
    Plugin(usize),
    /// A hint line (not selectable).
    Hint(String),
    /// A plugin or file that failed, greyed with its reason (not selectable).
    Failed {
        /// What failed.
        title: String,
        /// Why.
        reason: String,
    },
}

impl BrowserRow {
    /// Whether the row can be highlighted and added.
    pub fn selectable(&self) -> bool {
        matches!(self, Self::Device(_) | Self::Plugin(_))
    }
}

/// The heading of the plugin group.
pub const PLUGINS_HEADING: &str = "PLUGINS";

/// Case-insensitive match on the type id, the name or the kind.
fn matches(desc: &DeviceDescriptor, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || desc.type_id.to_lowercase().contains(&q)
        || desc.name.to_lowercase().contains(&q)
        || kind_label(desc).to_lowercase().contains(&q)
}

/// The group label of a row: the kind, `PLAYER` for the note players or `MODULATOR` for the CV
/// sources.
fn kind_label(desc: &DeviceDescriptor) -> &'static str {
    if is_modulator(desc.type_id) {
        return "MODULATOR";
    }
    if is_player(desc.type_id) {
        return "PLAYER";
    }
    if desc.type_id == CONTAINER_TYPE {
        return "CONTAINER";
    }
    match desc.kind {
        DeviceKind::Instrument => "INSTRUMENT",
        DeviceKind::Effect => "EFFECT",
        DeviceKind::Mixer => "MIXER",
        DeviceKind::Utility => "UTILITY",
        DeviceKind::Interface => "I/O",
        DeviceKind::Container => "CONTAINER",
    }
}

/// The sort group of a row: devices, the modulators, the players, then the containers.
fn group_of(type_id: &str) -> u8 {
    if type_id == CONTAINER_TYPE {
        3
    } else if is_player(type_id) {
        2
    } else {
        u8::from(is_modulator(type_id))
    }
}

/// Type id of the audio-track device (hidden from the browser).
pub const AUDIO_TRACK_TYPE: &str = "atomik.audio-track";

impl Browser {
    /// Matching descriptors in registry (type id) order, the players next as their
    /// own group, the containers last. Inside a container (see
    /// [`Self::scope_depth`]) the list is filtered to the types it may hold.
    pub fn rows(&self, registry: &Registry) -> Vec<&'static DeviceDescriptor> {
        let mut rows: Vec<&'static DeviceDescriptor> = registry
            .type_ids()
            .filter_map(|id| registry.get(id).map(|f| f.descriptor))
            .filter(|d| {
                let console = d.type_id == crate::console_layout::CHANNEL_TYPE
                    || d.type_id == crate::console_layout::MASTER_TYPE;
                // Console devices are never offered inside a container.
                !console || (self.show_console && self.scope_depth == 0)
            })
            // Audio tracks are created as tracks (AddAudioTrack), never dropped into the rack.
            .filter(|d| d.type_id != AUDIO_TRACK_TYPE)
            // Output buses are created by the console, never from the browser (even with the
            // console toggle on).
            .filter(|d| d.type_id != crate::console_layout::BUS_TYPE)
            // Internal devices are created by the container expansion, never by hand.
            .filter(|d| !is_internal(d.type_id))
            // A container may not nest deeper than the limit.
            .filter(|d| d.type_id != CONTAINER_TYPE || !self.at_nesting_limit())
            .filter(|d| self.create.is_none_or(|k| k.accepts(d)))
            .filter(|d| matches(d, &self.query))
            .collect();
        rows.sort_by_key(|d| group_of(d.type_id));
        rows
    }

    /// Every row of the list: the device rows of [`Self::rows`], then (when plugins are known) the
    /// **Plugins** group: a heading, the usable plugins by Instruments, Effects and Note effects
    /// (entries `name (vendor)`, filtered on name, vendor and format), a hint counting the unsupported
    /// plugins that are hidden, and the failed ones greyed with their reason. Disabled plugins
    /// are hidden.
    pub fn list(&self, registry: &Registry) -> Vec<BrowserRow> {
        let mut out: Vec<BrowserRow> = self
            .rows(registry)
            .into_iter()
            .map(BrowserRow::Device)
            .collect();
        let q = self.query.trim().to_lowercase();
        let shown: Vec<(usize, &PluginEntryInfo)> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, e)| e.matches(&q))
            .collect();
        let failed_entries: Vec<&PluginEntryInfo> = shown
            .iter()
            .filter(|(_, e)| matches!(e.status, PluginEntryStatus::Failed(_)))
            .map(|(_, e)| *e)
            .collect();
        let failed_files: Vec<&FailedFileInfo> = self
            .plugin_failed
            .iter()
            .filter(|f| q.is_empty() || f.path.to_lowercase().contains(&q))
            .collect();
        let unsupported = shown
            .iter()
            .filter(|(_, e)| matches!(e.status, PluginEntryStatus::Unsupported(_)))
            .count();
        // Create mode lists only what can be created: no hint, no failed entries.
        let create = self.create.is_some();
        let unsupported = if create { 0 } else { unsupported };
        let failed_entries: Vec<&PluginEntryInfo> =
            if create { Vec::new() } else { failed_entries };
        let failed_files: Vec<&FailedFileInfo> = if create { Vec::new() } else { failed_files };
        let usable = |kind: PluginKindHint| {
            shown
                .iter()
                .filter(|(_, e)| e.status == PluginEntryStatus::Ok && e.kind == kind)
                .map(|(i, _)| *i)
                .collect::<Vec<usize>>()
        };
        let offered = |hint: PluginKindHint| self.create.is_none_or(|k| k.accepts_plugin(hint));
        let groups = [
            ("Instruments", usable(PluginKindHint::Instrument)),
            ("Effects", usable(PluginKindHint::Effect)),
            ("Note effects", usable(PluginKindHint::NoteEffect)),
        ]
        .map(|(name, idx)| {
            let hint = match name {
                "Instruments" => PluginKindHint::Instrument,
                "Effects" => PluginKindHint::Effect,
                _ => PluginKindHint::NoteEffect,
            };
            (name, if offered(hint) { idx } else { Vec::new() })
        });
        let any = groups.iter().any(|(_, v)| !v.is_empty())
            || unsupported > 0
            || !failed_entries.is_empty()
            || !failed_files.is_empty();
        if !any {
            return out;
        }
        out.push(BrowserRow::Heading(PLUGINS_HEADING.into()));
        for (name, idx) in groups {
            if idx.is_empty() {
                continue;
            }
            out.push(BrowserRow::Heading(name.into()));
            out.extend(idx.into_iter().map(BrowserRow::Plugin));
        }
        if unsupported > 0 {
            out.push(BrowserRow::Hint(format!(
                "{unsupported} unsupported plugin{} hidden",
                if unsupported == 1 { "" } else { "s" }
            )));
        }
        if !failed_entries.is_empty() || !failed_files.is_empty() {
            out.push(BrowserRow::Heading("Failed".into()));
            for e in failed_entries {
                out.push(BrowserRow::Failed {
                    title: e.title(),
                    reason: e.status.reason().unwrap_or_default().to_string(),
                });
            }
            for f in failed_files {
                out.push(BrowserRow::Failed {
                    title: f
                        .path
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(&f.path)
                        .to_string(),
                    reason: f.reason.clone(),
                });
            }
        }
        out
    }

    /// After an edit of the search text: the list starts over, so the highlight goes to the
    /// first match and the scroll to the top (a scroll kept from the full list would leave
    /// the few matches above the panel).
    pub fn query_changed(&mut self, registry: &Registry) {
        self.scroll = 0.0;
        self.highlighted = Self::first_selectable(&self.list(registry));
    }

    /// The first row that can be highlighted (0 when there is none).
    pub fn first_selectable(rows: &[BrowserRow]) -> usize {
        rows.iter().position(BrowserRow::selectable).unwrap_or(0)
    }

    /// The next selectable row after `from` (moving down), else `from`.
    pub fn next_selectable(rows: &[BrowserRow], from: usize) -> usize {
        (from + 1..rows.len())
            .find(|&i| rows[i].selectable())
            .unwrap_or(from)
    }

    /// The previous selectable row before `from` (moving up), else `from`.
    pub fn prev_selectable(rows: &[BrowserRow], from: usize) -> usize {
        (0..from)
            .rev()
            .find(|&i| rows[i].selectable())
            .unwrap_or(from)
    }

    /// True when the rack view is as deep as containers nest: no container can be added here.
    pub fn at_nesting_limit(&self) -> bool {
        self.scope_depth >= atomik_model::MAX_CONTAINER_DEPTH
    }

    /// The hint to show instead of the container rows at the nesting limit.
    pub fn hint(&self) -> Option<&'static str> {
        self.at_nesting_limit().then_some(NESTING_HINT)
    }

    /// Row rectangle (logical view coordinates).
    pub fn row_rect(&self, i: usize) -> Rect {
        let y = ROWS_Y + i as f64 * ROW_H - self.scroll;
        Rect::new(8.0, y, BROWSER_WIDTH - 8.0, y + ROW_H - 4.0)
    }

    /// The row under `p`.
    pub fn row_at(&self, p: Point, rows: usize) -> Option<usize> {
        (0..rows).find(|&i| self.row_rect(i).contains(p))
    }

    /// True when `p` is in the search field.
    pub fn in_search(p: Point) -> bool {
        SEARCH.contains(p)
    }

    /// Paints the panel at the left of a view of height `height`.
    pub fn paint<C: Canvas + ?Sized>(
        &self,
        c: &mut C,
        theme: &Theme,
        registry: &Registry,
        height: f64,
        send: Option<SendEntry>,
    ) {
        c.fill_with(Rect::new(0.0, 0.0, BROWSER_WIDTH, height), theme.browser_bg);
        c.stroke_with(
            Line::new(
                pt(BROWSER_WIDTH - 0.5, 0.0),
                pt(BROWSER_WIDTH - 0.5, height),
            ),
            1.0,
            theme.bevel_dark,
        );
        c.text(
            &TextRun::new(
                self.create.map_or("DEVICES", CreateKind::title),
                pt(14.0, 24.0),
                9.0,
                theme.text_muted,
            )
            .weight(700)
            .spacing(1.4),
        );
        let field = RoundedRect::from_rect(SEARCH, 5.0);
        c.fill_with(field, theme.lcd);
        let border = if self.focused {
            theme.focus
        } else {
            theme.section_line
        };
        c.stroke_with(field, 1.0, border);
        let (text, color) = if self.query.is_empty() {
            ("Search".to_string(), theme.text_muted)
        } else {
            (self.query.clone(), theme.text)
        };
        let shown = if self.focused {
            format!("{text}|")
        } else {
            text
        };
        c.text(&TextRun::new(
            shown,
            pt(SEARCH.x0 + 9.0, SEARCH.y0 + 17.0),
            11.0,
            color,
        ));
        if let Some(hint) = self.hint() {
            c.text(&TextRun::new(hint, pt(14.0, 70.0), 8.0, theme.text_muted));
        }
        if let Some(entry) = send {
            // Greyed when every slot is used; otherwise in the normal text colour.
            let (dot, color) = match entry {
                SendEntry::Slot(_) => (theme.cable_audio, theme.text),
                SendEntry::Full => (
                    theme.text_muted.with_alpha(0.5),
                    theme.text_muted.with_alpha(0.7),
                ),
            };
            c.fill_with(Circle::new(pt(17.0, 67.5), 2.5), dot);
            c.text(&TextRun::new(
                crate::paint::fit_label(&entry.text(), BROWSER_WIDTH - 36.0, 8.0),
                pt(24.0, 70.0),
                8.0,
                color,
            ));
        }
        c.push_clip(&Rect::new(0.0, ROWS_Y - 2.0, BROWSER_WIDTH, height).into());
        for (i, row) in self.list(registry).iter().enumerate() {
            let r = self.row_rect(i);
            if row.selectable() && (self.hover == Some(i) || self.highlighted == i) {
                c.fill_with(RoundedRect::from_rect(r, 5.0), theme.browser_row);
            }
            match row {
                BrowserRow::Device(d) => {
                    c.fill_with(
                        Circle::new(pt(r.x0 + 13.0, r.center().y), 5.0),
                        theme.accent_for_type(d.type_id),
                    );
                    c.text(
                        &TextRun::new(d.name, pt(r.x0 + 26.0, r.y0 + 14.0), 11.0, theme.text)
                            .weight(600),
                    );
                    c.text(&TextRun::new(
                        d.type_id,
                        pt(r.x0 + 26.0, r.y0 + 26.0),
                        8.0,
                        theme.text_muted,
                    ));
                    c.text(
                        &TextRun::new(
                            kind_label(d),
                            pt(r.x1 - 8.0, r.y0 + 14.0),
                            7.0,
                            theme.text_muted,
                        )
                        .align(Align::End)
                        .spacing(0.8),
                    );
                }
                BrowserRow::Plugin(n) => {
                    let Some(e) = self.plugins.get(*n) else {
                        continue;
                    };
                    let accent = theme.accent_for_type(&e.type_id());
                    c.fill_with(Circle::new(pt(r.x0 + 13.0, r.center().y), 5.0), accent);
                    c.text(
                        &TextRun::new(
                            crate::paint::fit_label(&e.title(), r.width() - 34.0, 11.0),
                            pt(r.x0 + 26.0, r.y0 + 14.0),
                            11.0,
                            theme.text,
                        )
                        .weight(600),
                    );
                    c.text(&TextRun::new(
                        crate::paint::fit_label(&e.plugin.id, r.width() - 70.0, 8.0),
                        pt(r.x0 + 26.0, r.y0 + 26.0),
                        8.0,
                        theme.text_muted,
                    ));
                    c.text(
                        &TextRun::new(
                            format_badge(e.plugin.format),
                            pt(r.x1 - 8.0, r.y0 + 26.0),
                            7.0,
                            accent,
                        )
                        .align(Align::End)
                        .weight(700)
                        .spacing(0.8),
                    );
                }
                BrowserRow::Heading(h) => {
                    c.text(
                        &TextRun::new(
                            h.clone(),
                            pt(r.x0 + 6.0, r.y0 + 20.0),
                            8.0,
                            theme.text_muted,
                        )
                        .weight(700)
                        .spacing(1.2),
                    );
                }
                BrowserRow::Hint(h) => {
                    c.text(&TextRun::new(
                        crate::paint::fit_label(h, r.width() - 12.0, 8.5),
                        pt(r.x0 + 6.0, r.y0 + 18.0),
                        8.5,
                        theme.text_muted,
                    ));
                }
                BrowserRow::Failed { title, reason } => {
                    let dim = theme.text_muted.with_alpha(0.7);
                    c.text(&TextRun::new(
                        crate::paint::fit_label(title, r.width() - 14.0, 10.0),
                        pt(r.x0 + 6.0, r.y0 + 14.0),
                        10.0,
                        dim,
                    ));
                    c.text(&TextRun::new(
                        crate::paint::fit_label(reason, r.width() - 14.0, 8.0),
                        pt(r.x0 + 6.0, r.y0 + 26.0),
                        8.0,
                        dim,
                    ));
                }
            }
        }
        c.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        let mut r = Registry::new();
        atomik_devices::register_all(&mut r).expect("register");
        r
    }

    #[test]
    fn the_player_list_matches_the_devices_crate() {
        assert_eq!(PLAYER_TYPE_IDS, atomik_devices::PLAYER_TYPE_IDS);
        assert_eq!(INTERNAL_TYPE_IDS, atomik_devices::INTERNAL_TYPE_IDS);
        assert_eq!(MODULATOR_TYPE_IDS, atomik_devices::MODULATOR_TYPE_IDS);
        assert!(is_modulator("atomik.mod-source") && !is_modulator("atomik.modulation"));
        assert!(is_player("atomik.arpeggiator") && !is_player("atomik.poly-synth"));
    }

    #[test]
    fn players_are_listed_last_and_found_by_the_player_label() {
        let reg = registry();
        let mut browser = Browser::default();
        let ids = |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        let everything = ids(&browser);
        assert_eq!(everything.last(), Some(&CONTAINER_TYPE), "containers last");
        let all: Vec<&str> = everything
            .into_iter()
            .filter(|t| *t != CONTAINER_TYPE)
            .collect();
        let first = all.iter().position(|t| is_player(t)).expect("players");
        assert!(all[first..].iter().all(|t| is_player(t)), "{all:?}");
        assert_eq!(all.len() - first, PLAYER_TYPE_IDS.len());
        // Within each group the registry order is kept.
        assert!(all[first..].windows(2).all(|w| w[0] < w[1]));
        let before: Vec<&str> = all[..first]
            .iter()
            .copied()
            .filter(|t| !is_modulator(t))
            .collect();
        assert!(before.windows(2).all(|w| w[0] < w[1]));
        browser.query = "player".into();
        assert_eq!(ids(&browser), PLAYER_TYPE_IDS.to_vec());
    }

    #[test]
    fn modulators_are_a_group_after_the_devices_and_before_the_players() {
        let reg = registry();
        let mut browser = Browser::default();
        let ids = |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        let all = ids(&browser);
        let m = all
            .iter()
            .position(|t| is_modulator(t))
            .expect("modulators");
        let p = all.iter().position(|t| is_player(t)).expect("players");
        assert!(all[..m].iter().all(|t| !is_modulator(t) && !is_player(t)));
        assert!(m < p, "modulators come before the players: {all:?}");
        assert_eq!(p - m, MODULATOR_TYPE_IDS.len());
        browser.query = "modulator".into();
        assert_eq!(ids(&browser), MODULATOR_TYPE_IDS.to_vec());
        // The Create Effect browser lists them too (they are utilities), and the instrument
        // browser does not.
        browser.query.clear();
        browser.create = Some(CreateKind::Effect);
        assert!(ids(&browser).contains(&"atomik.mod-source"));
        browser.create = Some(CreateKind::Instrument);
        assert!(!ids(&browser).contains(&"atomik.mod-source"));
        assert_eq!(
            kind_label(reg.get("atomik.mod-source").unwrap().descriptor),
            "MODULATOR"
        );
        assert_eq!(
            kind_label(reg.get("atomik.modulation").unwrap().descriptor),
            "EFFECT"
        );
    }

    #[test]
    fn containers_are_a_group_and_the_internal_split_stays_hidden() {
        let reg = registry();
        let mut browser = Browser::default();
        let ids = |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        assert!(ids(&browser).contains(&CONTAINER_TYPE));
        assert!(!ids(&browser).contains(&"atomik.note-split"));
        browser.query = "container".into();
        assert_eq!(ids(&browser), [CONTAINER_TYPE]);
        browser.query = "atomik.note-split".into();
        assert!(ids(&browser).is_empty());
    }

    #[test]
    fn a_scope_offers_the_allowed_types_and_containers_while_the_depth_allows() {
        let reg = registry();
        let mut browser = Browser {
            show_console: true,
            ..Browser::default()
        };
        let ids = |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        assert!(ids(&browser).contains(&"atomik.channel"));
        for depth in 1..atomik_model::MAX_CONTAINER_DEPTH {
            browser.scope_depth = depth;
            let all = ids(&browser);
            assert!(all.contains(&CONTAINER_TYPE), "depth {depth}");
            assert!(!all.contains(&"atomik.channel") && !all.contains(&"atomik.master"));
            assert!(!all.contains(&AUDIO_TRACK_TYPE));
            assert_eq!(browser.hint(), None);
        }
        browser.scope_depth = atomik_model::MAX_CONTAINER_DEPTH;
        assert!(!ids(&browser).contains(&CONTAINER_TYPE));
        assert_eq!(browser.hint(), Some(NESTING_HINT));
        assert!(ids(&browser).contains(&"atomik.poly-synth"));
    }

    #[test]
    fn the_audio_track_device_is_hidden_from_the_browser() {
        let reg = registry();
        assert!(
            reg.get(AUDIO_TRACK_TYPE).is_some(),
            "the type is registered, so hiding it is a choice"
        );
        let mut browser = Browser::default();
        let listed =
            |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        assert!(!listed(&browser).contains(&AUDIO_TRACK_TYPE));
        // Neither a search for it nor the console toggle shows it.
        browser.query = "audio".into();
        assert!(!listed(&browser).contains(&AUDIO_TRACK_TYPE));
        browser.query = "atomik.audio-track".into();
        assert!(listed(&browser).is_empty());
        browser.query.clear();
        browser.show_console = true;
        assert!(!listed(&browser).contains(&AUDIO_TRACK_TYPE));
        assert!(!listed(&browser).is_empty());
    }

    #[test]
    fn the_output_bus_device_is_hidden_from_the_browser() {
        let reg = registry();
        assert!(
            reg.get("atomik.bus").is_some(),
            "registered, hidden by choice"
        );
        let mut browser = Browser::default();
        let listed =
            |b: &Browser| -> Vec<&str> { b.rows(&reg).iter().map(|d| d.type_id).collect() };
        assert!(!listed(&browser).contains(&"atomik.bus"));
        browser.show_console = true;
        assert!(listed(&browser).contains(&"atomik.channel"));
        assert!(!listed(&browser).contains(&"atomik.bus"));
        browser.query = "bus".into();
        assert!(!listed(&browser).contains(&"atomik.bus"));
    }

    fn plugin(name: &str, vendor: &str, kind: PluginKindHint) -> PluginEntryInfo {
        use atomik_device_sdk::{PluginFormat, PluginLayout, PluginRef};
        PluginEntryInfo {
            plugin: PluginRef {
                format: PluginFormat::Vst3,
                id: name.to_lowercase().replace(' ', "-"),
            },
            name: name.into(),
            vendor: vendor.into(),
            version: "1".into(),
            kind,
            status: PluginEntryStatus::Ok,
            path: None,
            layout: PluginLayout {
                name: name.into(),
                vendor: vendor.into(),
                version: "1".into(),
                kind_hint: kind,
                main_in: None,
                main_out: None,
                sidechain: false,
                notes_in: false,
                notes_out: false,
                params: Vec::new(),
            },
        }
    }

    fn listed_plugins(b: &Browser, reg: &Registry) -> Vec<String> {
        b.list(reg)
            .into_iter()
            .filter_map(|r| match r {
                BrowserRow::Plugin(i) => Some(b.plugins[i].name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_prefix_of_a_plugin_name_lists_it_whatever_the_case() {
        let reg = registry();
        let mut browser = Browser {
            plugins: vec![
                plugin("Serum 2", "Xfer Records", PluginKindHint::Instrument),
                plugin("Serum 2 FX", "Xfer Records", PluginKindHint::Effect),
                plugin("Surge XT", "Surge Synth Team", PluginKindHint::Instrument),
            ],
            ..Browser::default()
        };
        for q in ["ser", "SER", " Ser ", "serum", "rum 2"] {
            browser.query = q.into();
            let found = listed_plugins(&browser, &reg);
            assert!(found.contains(&"Serum 2".to_string()), "{q:?}: {found:?}");
            assert!(!found.contains(&"Surge XT".to_string()), "{q:?}: {found:?}");
        }
        // The effect create list offers the effect only, the instrument list the synth only.
        browser.query = "ser".into();
        browser.create = Some(CreateKind::Instrument);
        assert_eq!(listed_plugins(&browser, &reg), ["Serum 2"]);
        browser.create = Some(CreateKind::Effect);
        assert_eq!(listed_plugins(&browser, &reg), ["Serum 2 FX"]);
    }

    #[test]
    fn editing_the_search_brings_the_matches_back_into_view() {
        let reg = registry();
        let mut browser = Browser {
            plugins: vec![plugin(
                "Serum 2",
                "Xfer Records",
                PluginKindHint::Instrument,
            )],
            scroll: 900.0,
            highlighted: 40,
            ..Browser::default()
        };
        browser.query = "ser".into();
        browser.query_changed(&reg);
        assert_eq!(
            browser.scroll, 0.0,
            "a scroll kept from the full list hides the matches"
        );
        let rows = browser.list(&reg);
        assert!(matches!(rows.get(browser.highlighted), Some(r) if r.selectable()));
    }
}
