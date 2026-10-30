//! Macro banks, automation that follows the selection, and the parameter search in the app core.
//!
//! # Macros
//!
//! A bank is a rack device (`atomik.macro-bank`, see [`atomik_model::macros`]) and the project's
//! assignments say which params its eight macros drive. Everything here is an ordinary command
//! (`Command::Macro`, `Command::SetParam` on a macro), so it is validated, undoable and sent to the
//! engine the way a container's macro is: a `ParamChanged` of a bank's `macroN` and a
//! `MacrosChanged` make [`AppCore::send_macro_values`] send one `set_param` per target (the
//! device's own smoothers ramp it), and a song lane on a macro expands into one lane per target
//! when the song is built, live and in a bounce (see `atomik_render::project`). The **current
//! bank** is where "Assign to Macro" in the right-click menu and the search put a param; it is the
//! one chosen in the Macros panel, else the bank last made or used, else the first.
//!
//! # Automation follows the selection
//!
//! A switch (the workflow preference `follow_selection`, View menu) that makes the arrangement show the
//! lane of the parameter being touched. A press on a rack control, a console control or a row of
//! the parameter pane ([`atomik_ui::UiAction::ParamTouched`]) records the param as the last
//! touched one of its device and, with the switch on, brings its lane into view:
//!
//! * a param a macro drives follows the **macro's** lane on the bank's track;
//! * a device without a song track gets one when it has no Notes input too: an effect or a
//!   channel strip is automated on an automation track of its own (see
//!   [`atomik_model::song::Track::is_automation_only`]); only an audio track, a device inside a
//!   container and an internal param have no lane to show;
//! * the lane is added when it is missing (one undo step, an empty lane: **no automation data is
//!   created until something is written**), but never while an automation take is recording;
//! * the track's folders are opened (they are the project's, so that is a command), a collapsed
//!   track and a folded lane are shown open in the view without changing the project
//!   ([`atomik_ui::ArrangementView::reveal_lane`]), the list scrolls to the lane and the lane is
//!   outlined;
//! * selecting a device in the rack or a track in the arrangement shows the lane of the last
//!   param touched on it, when that lane exists.
//!
//! # The parameter search
//!
//! [`atomik_model::ParamIndex`] is built when the search is first used and again only when its
//! signature (the structure of the rack, not the values) changes; a query is a scan over names and
//! the rows shown (at most [`SEARCH_ROWS`]) read their current values at that moment, so the
//! search stays quick on a project with thousands of params.

use super::*;
use atomik_model::{
    MACRO_BANK_TYPE, MAX_RESULTS, MacroAssign, MacroCommand, ParamEntry, ParamIndex, SearchTarget,
    SongCommand, macro_key,
};
use atomik_ui::{MacroAssignRow, MacroBankRow, MacroRow, MacrosAction, ParamSearchRow};

/// The most rows the search lists.
pub const SEARCH_ROWS: usize = 100;

/// What the app keeps for macros, following the selection and the search (view state, not saved
/// with the project).
#[derive(Debug, Default)]
pub(super) struct MacroState {
    /// The bank new assignments go to.
    bank: Option<DeviceKey>,
    /// Automation follows the selection (the workflow preference).
    follow: bool,
    /// The last param touched on each device.
    touched: BTreeMap<DeviceKey, String>,
    /// The device whose selection the follow last handled.
    selection_seen: Option<DeviceKey>,
    /// The index of the search, rebuilt when the structure of the rack changes.
    index: Option<ParamIndex>,
    /// The rows the search shows now, in row order.
    results: Vec<ParamEntry>,
    /// How many params matched the last query (up to `MAX_RESULTS`).
    matches: usize,
}

/// Which pane a revealed param is in, for the shell to bring up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revealed {
    /// The device is selected in the rack.
    Rack,
    /// The strips or a group are selected in the console.
    Console,
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ banks and assignments

    /// The bank new assignments go to: the chosen one when it still exists, else the first.
    pub fn macro_bank(&self) -> Option<DeviceKey> {
        let doc = self.doc();
        self.macros
            .bank
            .clone()
            .filter(|k| doc.device(k).is_some_and(|d| d.type_id == MACRO_BANK_TYPE))
            .or_else(|| doc.banks().next().map(|d| d.key.clone()))
    }

    /// Makes `bank` the one new assignments go to.
    pub fn set_macro_bank(&mut self, bank: DeviceKey) {
        if self
            .doc()
            .device(&bank)
            .is_some_and(|d| d.type_id == MACRO_BANK_TYPE)
        {
            self.macros.bank = Some(bank);
        }
    }

    /// Adds a macro bank at the end of the rack (one undo step) and makes it the current one.
    pub fn new_macro_bank(&mut self) -> bool {
        let before = self.doc().banks().count();
        let done = self
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: MACRO_BANK_TYPE.to_owned(),
                key: None,
            })
            .is_ok();
        if done
            && self.doc().banks().count() > before
            && let Some(d) = self.doc().banks().last()
        {
            self.macros.bank = Some(d.key.clone());
        }
        done
    }

    /// Assigns `param` of `device` to macro `index` (0-based) of the current bank, or of a new
    /// bank when `new_bank` or when there is none. One undo step; the reason is shown when the
    /// model refuses.
    pub fn assign_to_macro(
        &mut self,
        device: &DeviceKey,
        param: &str,
        index: u8,
        new_bank: bool,
    ) -> bool {
        let bank = if new_bank { None } else { self.macro_bank() };
        let made = bank.is_none();
        let done = self
            .apply_command(Command::Macro(MacroCommand::Assign {
                bank,
                index,
                device: device.clone(),
                param: param.to_owned(),
                min: None,
                max: None,
                invert: false,
            }))
            .is_ok();
        if done
            && made
            && let Some(d) = self.doc().banks().last()
        {
            self.macros.bank = Some(d.key.clone());
        }
        if done && let Some(a) = self.doc().macros.driving(device, param).cloned() {
            let name = self.doc().macro_name(&a.bank, a.index);
            self.report(format!("Assigned to {name} (macro {})", a.index + 1));
        }
        done
    }

    /// Assigns the param to the first macro of the current bank that drives nothing yet (the first
    /// macro when all are in use).
    pub fn assign_to_next_macro(&mut self, device: &DeviceKey, param: &str) -> bool {
        let index = self.macro_bank().map_or(0, |bank| {
            (0..8u8)
                .find(|i| self.doc().macros.of_macro(&bank, *i).next().is_none())
                .unwrap_or(0)
        });
        self.assign_to_macro(device, param, index, false)
    }

    /// Takes the param out of its macro (one undo step); it keeps the value the macro gave it.
    pub fn remove_from_macro(&mut self, device: &DeviceKey, param: &str) -> bool {
        let Some(id) = self.doc().macros.driving(device, param).map(|a| a.id) else {
            self.report("That control is not assigned to a macro");
            return false;
        };
        self.apply_command(Command::Macro(MacroCommand::Remove { id }))
            .is_ok()
    }

    /// The live assignment that drives `param` of `device`, if any.
    pub fn macro_driver(&self, device: &DeviceKey, param: &str) -> Option<&MacroAssign> {
        self.doc().bank_driver(device, param)
    }

    /// What the control menu needs: whether the param is assignable to a macro (a param that no
    /// macro drives and that can be) and whether it is driven (Remove from Macro).
    pub fn macro_control_state(&self, device: &DeviceKey, param: &str) -> (bool, bool) {
        let doc = self.doc();
        let driven = doc.bank_driver(device, param).is_some();
        let can = !driven
            && doc
                .macro_target_problem(&self.registry, device, param)
                .is_none();
        (can, driven)
    }

    // ------------------------------------------------------------------ the Macros panel

    /// The banks of the project as the Macros panel lists them.
    pub fn macro_bank_rows(&self) -> Vec<MacroBankRow> {
        let doc = self.doc();
        doc.banks()
            .map(|bank| MacroBankRow {
                key: bank.key.clone(),
                name: bank.name.clone(),
                macros: (0..8u8)
                    .map(|i| MacroRow {
                        index: i,
                        name: doc.macro_name(&bank.key, i),
                        value: doc.bank_macro_value(&bank.key, i),
                        mapped: self.is_midi_mapped(&bank.key, &macro_key(i)),
                        assigns: doc
                            .macros
                            .of_macro(&bank.key, i)
                            .map(|a| self.assign_row(a))
                            .collect(),
                    })
                    .collect(),
            })
            .collect()
    }

    fn assign_row(&self, a: &MacroAssign) -> MacroAssignRow {
        let doc = self.doc();
        let device = doc.device(&a.device);
        let desc = device
            .and_then(|d| self.registry.get(&d.type_id))
            .and_then(|f| f.descriptor.param_by_key(&a.param));
        let name = device.map_or_else(|| a.device.0.clone(), |d| d.name.clone());
        let (param_min, param_max, step) =
            desc.map_or((a.min.min(a.max), a.min.max(a.max), 1.0), |p| {
                (
                    p.min,
                    p.max,
                    if p.discrete {
                        1.0
                    } else {
                        (p.max - p.min) / 50.0
                    },
                )
            });
        let text =
            |v: f32| desc.map_or_else(|| format!("{v}"), |p| atomik_ui::widget::format_value(p, v));
        MacroAssignRow {
            id: a.id,
            device: a.device.clone(),
            param: a.param.clone(),
            target: format!("{name} {}", desc.map_or(a.param.as_str(), |p| p.name)),
            min: a.min,
            max: a.max,
            invert: a.invert,
            param_min,
            param_max,
            step,
            min_text: text(a.min),
            max_text: text(a.max),
            missing: desc.is_none() || !doc.macro_assign_live(a),
        }
    }

    /// Applies what the Macros panel asked for. True when the panel should close.
    pub fn macros_action(&mut self, action: MacrosAction) -> bool {
        match action {
            MacrosAction::SelectBank(key) => self.set_macro_bank(key),
            MacrosAction::NewBank => {
                self.new_macro_bank();
            }
            MacrosAction::SetValue { bank, index, value } => {
                let _ = self.apply_command(Command::SetParam {
                    key: bank,
                    param: macro_key(index),
                    value,
                    merge: None,
                });
            }
            MacrosAction::SetRange {
                id,
                min,
                max,
                invert,
            } => {
                let _ = self.apply_command(Command::Macro(MacroCommand::SetRange {
                    id,
                    min,
                    max,
                    invert,
                }));
            }
            MacrosAction::Remove(id) => {
                let _ = self.apply_command(Command::Macro(MacroCommand::Remove { id }));
            }
            MacrosAction::Clear { bank, index } => {
                let _ = self.apply_command(Command::Macro(MacroCommand::Clear { bank, index }));
            }
            MacrosAction::Rename { bank, index, name } => {
                let _ =
                    self.apply_command(Command::Macro(MacroCommand::Rename { bank, index, name }));
            }
            MacrosAction::Learn { bank, index } => {
                return self.midi_arm(atomik_model::MidiTarget::Param {
                    device: bank,
                    param: macro_key(index),
                });
            }
            MacrosAction::Reveal { device, .. } => {
                self.select(Some(device));
            }
            MacrosAction::Close => return true,
        }
        false
    }

    // ------------------------------------------------------------------ following the selection

    /// Whether automation follows the selection.
    pub fn follow_selection(&self) -> bool {
        self.macros.follow
    }

    /// Switches following on or off (the workflow preference); off clears the highlight.
    pub fn set_follow_selection(&mut self, on: bool) {
        if self.macros.follow == on {
            return;
        }
        self.macros.follow = on;
        self.macros.selection_seen = None;
        if !on {
            self.arrangement.follow_lane(None);
        }
        self.after_view_change();
    }

    /// The song lane that shows `param` of `device`: the param's own lane when the device has a
    /// track (a device without a Notes input, a channel strip or an effect, gets an automation
    /// track), the macro's lane on the bank's track when a macro drives it, `None` when neither
    /// can exist (an audio track, a device inside a container, an internal param).
    pub(super) fn lane_host(&self, device: &DeviceKey, param: &str) -> Option<(DeviceKey, String)> {
        let doc = self.doc();
        let host = match doc.bank_driver(device, param) {
            Some(a) => (a.bank.clone(), macro_key(a.index)),
            None => (device.clone(), param.to_owned()),
        };
        let d = doc.device(&host.0)?;
        let desc = self.registry.get(&d.type_id)?.descriptor;
        (desc.type_id != atomik_model::song::AUDIO_TRACK_TYPE
            && desc.param_by_key(&host.1).is_some()
            && !host.1.starts_with("internal."))
        .then_some(host)
    }

    /// Whether the lane that shows `param` of `device` exists (Clear Automation works).
    pub fn has_lane_to_clear(&self, device: &DeviceKey, param: &str) -> bool {
        self.lane_host(device, param).is_some_and(|(host, lane)| {
            self.doc()
                .song
                .track(&host)
                .is_some_and(|t| t.automation_lane(&lane).is_some())
        })
    }

    /// Whether `param` of `device` has a lane to show or add (see [`Self::lane_host`]).
    pub fn has_lane_for(&self, device: &DeviceKey, param: &str) -> bool {
        self.lane_host(device, param).is_some()
    }

    /// Records that `param` of `device` is being touched and, when following, shows its lane.
    pub(super) fn param_touched(&mut self, device: &DeviceKey, param: &str) {
        self.macros.touched.insert(device.clone(), param.to_owned());
        if self.macros.follow && !self.midi.learning() {
            self.follow_param(device, param, true, false);
        }
    }

    /// Shows the automation lane of `param` of `device` in the arrangement, adding it when it
    /// is missing (an empty lane: no automation is created). `say` reports why nothing is shown.
    /// True when a lane is shown.
    fn follow_param(&mut self, device: &DeviceKey, param: &str, create: bool, say: bool) -> bool {
        let Some((host, lane)) = self.lane_host(device, param) else {
            if say {
                self.report(format!(
                    "{device} {param} has no automation lane: it is an internal param, an audio track's or a container's inner device"
                ));
            }
            return false;
        };
        let exists = self
            .doc()
            .song
            .track(&host)
            .is_some_and(|t| t.automation_lane(&lane).is_some());
        if !exists {
            if !create {
                return false;
            }
            if self.automation_take_running() {
                if say {
                    self.report("Stop the automation take before adding a lane");
                }
                return false;
            }
            let added = self
                .apply_command(Command::Song(SongCommand::AddAutomationLane {
                    device: host.clone(),
                    param: lane.clone(),
                }))
                .is_ok();
            if !added {
                return false;
            }
        }
        self.open_folders_of(&host);
        self.arrangement.reveal_lane(&host, &lane);
        true
    }

    /// Opens every folder that hides the track of `device` (the folders are the project's, so
    /// each is a command).
    fn open_folders_of(&mut self, device: &DeviceKey) {
        let mut group = self.doc().song.org.members.get(device).copied();
        while let Some(id) = group {
            let Some(g) = self.doc().song.org.group(id) else {
                break;
            };
            let (collapsed, parent) = (g.collapsed, g.parent);
            if collapsed {
                let _ = self.apply_command(Command::Song(SongCommand::SetGroupCollapsed {
                    group: id,
                    collapsed: false,
                }));
            }
            group = parent;
        }
    }

    /// Shows the automation lane of `param` of `device`, adding the empty lane when it is missing;
    /// the reason is shown when there is none to show (the MCP `lane` action).
    pub fn show_lane_of(&mut self, device: &DeviceKey, param: &str) -> bool {
        self.macros.touched.insert(device.clone(), param.to_owned());
        let shown = self.follow_param(device, param, true, true);
        self.after_view_change();
        shown
    }

    /// The View menu's "Show Automation Lane": the lane of the last param touched on the selected
    /// device (or, with none, why there is no lane to show).
    pub fn show_touched_lane(&mut self) -> bool {
        let selected = self
            .view
            .selected()
            .cloned()
            .or_else(|| self.arrangement.selected_track().cloned());
        let Some(device) = selected else {
            self.report("Select a device first, and touch one of its controls");
            return false;
        };
        let Some(param) = self.macros.touched.get(&device).cloned() else {
            self.report("Touch a control of the device first");
            return false;
        };
        self.follow_param(&device, &param, true, true)
    }

    /// With following on, a new selection (a device in the rack, a track in the arrangement) shows
    /// the lane of the last param touched on it, when that lane exists.
    pub(super) fn follow_selected_device(&mut self) {
        if !self.macros.follow {
            return;
        }
        let selected = self
            .view
            .selected()
            .cloned()
            .or_else(|| self.arrangement.selected_track().cloned());
        if selected == self.macros.selection_seen {
            return;
        }
        self.macros.selection_seen.clone_from(&selected);
        if let Some(device) = selected
            && let Some(param) = self.macros.touched.get(&device).cloned()
        {
            self.follow_param(&device, &param, false, false);
        }
    }

    // ------------------------------------------------------------------ the search

    /// The index of the search, rebuilt only when the structure of the rack changed.
    fn search_index(&mut self) -> &ParamIndex {
        let signature = ParamIndex::signature_of(self.doc());
        if self
            .macros
            .index
            .as_ref()
            .is_none_or(|i| i.signature() != signature)
        {
            let index = ParamIndex::build(self.doc(), &self.registry);
            self.macros.index = Some(index);
        }
        self.macros
            .index
            .as_ref()
            .expect("the index was just built")
    }

    /// The rows of the search for `query` (at most [`SEARCH_ROWS`], best first) and how many
    /// params match in all (up to [`MAX_RESULTS`]). The targets of the rows are kept for the
    /// actions that name a row.
    pub fn param_search(&mut self, query: &str) -> (Vec<ParamSearchRow>, usize) {
        let mut entries: Vec<ParamEntry> = {
            let index = self.search_index();
            index
                .search(query, MAX_RESULTS)
                .into_iter()
                .cloned()
                .collect()
        };
        let total = entries.len();
        entries.truncate(SEARCH_ROWS);
        let rows = entries.iter().map(|e| self.search_row(e)).collect();
        self.macros.results = entries;
        self.macros.matches = total;
        (rows, total)
    }

    /// The rows of the last search with their values read again (no new search): the values move
    /// while the panel is open (playing, a controller, an automation lane).
    pub fn param_search_refresh(&self) -> (Vec<ParamSearchRow>, usize) {
        let rows = self
            .macros
            .results
            .iter()
            .map(|e| self.search_row(e))
            .collect();
        (rows, self.macros.matches)
    }

    fn search_row(&self, e: &ParamEntry) -> ParamSearchRow {
        let doc = self.doc();
        match &e.target {
            SearchTarget::Param { device, param } => {
                let desc = doc
                    .resolve(device)
                    .and_then(|d| self.registry.get(&d.type_id))
                    .and_then(|f| f.descriptor.param_by_key(param));
                let driver = doc.bank_driver(device, param);
                let value = doc
                    .effective_param(&self.registry, device, param)
                    .zip(desc)
                    .map_or_else(String::new, |(v, p)| atomik_ui::widget::format_value(p, v));
                ParamSearchRow {
                    track: e.track.clone(),
                    device: e.device.clone(),
                    param: e.param.clone(),
                    value,
                    macro_tag: driver.map(|a| format!("M{}", a.index + 1)),
                    midi: self.is_midi_mapped(device, param),
                    lane: self.lane_host(device, param).is_some_and(|(h, l)| {
                        doc.song
                            .track(&h)
                            .is_some_and(|t| t.automation_lane(&l).is_some())
                    }),
                    can_lane: self.lane_host(device, param).is_some(),
                    can_macro: driver.is_none()
                        && doc
                            .macro_target_problem(&self.registry, device, param)
                            .is_none(),
                    can_adjust: driver.is_none() && desc.is_some(),
                }
            }
            SearchTarget::Vca { id } => ParamSearchRow {
                track: e.track.clone(),
                device: e.device.clone(),
                param: e.param.clone(),
                value: doc
                    .console
                    .vca(*id)
                    .map_or_else(String::new, |v| format!("{:+.1} dB", v.level_db)),
                macro_tag: None,
                midi: doc.midi_maps.iter().any(
                    |m| matches!(m.target, atomik_model::MidiTarget::Vca { id: i } if i == *id),
                ),
                lane: false,
                can_lane: false,
                can_macro: false,
                can_adjust: true,
            },
        }
    }

    /// The targets of the rows of the last search, in row order.
    pub fn param_search_targets(&self) -> Vec<SearchTarget> {
        self.macros
            .results
            .iter()
            .map(|e| e.target.clone())
            .collect()
    }

    fn result(&self, row: usize) -> Option<SearchTarget> {
        self.macros.results.get(row).map(|e| e.target.clone())
    }

    /// Selects the param of search row `row` in the rack (a device) or the console (a strip, a
    /// group) and records it as the last touched; with following on its lane is shown too. The
    /// pane to bring up.
    pub fn search_reveal(&mut self, row: usize) -> Option<Revealed> {
        match self.result(row)? {
            SearchTarget::Param { device, param } => {
                let console_strip = self.doc().console.channel_index(&device).is_some()
                    || self.doc().console.bus_index(&device).is_some()
                    || self.doc().console.master.as_ref() == Some(&device);
                self.param_touched(&device, &param);
                if console_strip {
                    self.console
                        .set_selected_strips(std::slice::from_ref(&device));
                    self.select(Some(device));
                    Some(Revealed::Console)
                } else {
                    // Inside a container the rack shows the container first.
                    self.select(Some(device));
                    Some(Revealed::Rack)
                }
            }
            SearchTarget::Vca { id } => {
                let members = self
                    .doc()
                    .console
                    .vca(id)
                    .map(|v| v.members.clone())
                    .unwrap_or_default();
                self.console.set_selected_strips(&members);
                self.after_view_change();
                Some(Revealed::Console)
            }
        }
    }

    /// Shows the automation lane of search row `row` (see [`Self::follow_param`]).
    pub fn search_show_lane(&mut self, row: usize) -> bool {
        match self.result(row) {
            Some(SearchTarget::Param { device, param }) => {
                self.macros.touched.insert(device.clone(), param.clone());
                let shown = self.follow_param(&device, &param, true, true);
                self.after_view_change();
                shown
            }
            Some(SearchTarget::Vca { .. }) => {
                self.report("A VCA level has no lane in the arrangement");
                false
            }
            None => false,
        }
    }

    /// Assigns search row `row` to the next free macro of the current bank.
    pub fn search_assign(&mut self, row: usize) -> bool {
        match self.result(row) {
            Some(SearchTarget::Param { device, param }) => {
                self.assign_to_next_macro(&device, &param)
            }
            Some(SearchTarget::Vca { .. }) => {
                self.report("A VCA level cannot be assigned to a macro");
                false
            }
            None => false,
        }
    }

    /// Arms search row `row` for MIDI learn.
    pub fn search_learn(&mut self, row: usize) -> bool {
        let target = match self.result(row) {
            Some(SearchTarget::Param { device, param }) => {
                atomik_model::MidiTarget::Param { device, param }
            }
            Some(SearchTarget::Vca { id }) => atomik_model::MidiTarget::Vca { id },
            None => return false,
        };
        self.midi_arm(target)
    }

    /// Moves the value of search row `row` by `steps` fiftieths of its range (a step of a stepped
    /// param; `fine` is a tenth of that), one undo step.
    pub fn search_adjust(&mut self, row: usize, steps: i32, fine: bool) -> bool {
        match self.result(row) {
            Some(SearchTarget::Param { device, param }) => {
                let doc = self.doc();
                let Some(p) = doc
                    .resolve(&device)
                    .and_then(|d| self.registry.get(&d.type_id))
                    .and_then(|f| f.descriptor.param_by_key(&param))
                    .copied()
                else {
                    return false;
                };
                let Some(now) = doc.effective_param(&self.registry, &device, &param) else {
                    return false;
                };
                let value = if p.discrete {
                    p.clamp(now + steps as f32)
                } else {
                    let by = steps as f32 * if fine { 0.002 } else { 0.02 };
                    p.from_normalized((p.to_normalized(now) + by).clamp(0.0, 1.0))
                };
                if value.to_bits() == now.to_bits() {
                    return false;
                }
                self.apply_command(Command::SetParam {
                    key: device,
                    param,
                    value,
                    merge: None,
                })
                .is_ok()
            }
            Some(SearchTarget::Vca { id }) => {
                let Some(level) = self.doc().console.vca(id).map(|v| v.level_db) else {
                    return false;
                };
                let by = steps as f32 * if fine { 0.1 } else { 1.0 };
                let db = (level + by).clamp(atomik_core::VCA_MIN_DB, atomik_core::VCA_MAX_DB);
                if db.to_bits() == level.to_bits() {
                    return false;
                }
                self.apply_command(Command::Console(
                    atomik_model::ConsoleCommand::SetVcaLevel {
                        vca: id,
                        db,
                        merge: None,
                    },
                ))
                .is_ok()
            }
            None => false,
        }
    }
}
