//! The Track Panel: the whole chain of the selected track in one column.
//!
//! From the top it lists the track's **players** (the note stack, top player first), its
//! **instrument**, the four **insert** slots of its console channel, the **sends** and the
//! channel **fader** with its mute. It mirrors the document: nothing here is stored, and every
//! edit goes through the commands the rack and the console use, so one click is one undo step.
//!
//! * a player or insert row has a bypass button (the device's own `bypass` param), buttons that
//!   move it one place up or down the stack and a remove button;
//! * a send row switches the send on or off;
//! * the fader row drags the channel fader (one undo step per drag) and has the channel mute.
//!
//! A move or a removal is planned on a scratch copy of the document when the panel syncs, and
//! its button shows only when the model accepts the command and the order really changes, so
//! a press never emits a command the model would refuse.

use crate::action::UiAction;
use crate::canvas::{Canvas, CanvasExt, TextRun, pt};
use crate::event::{PointerButton, UiEvent};
use crate::seq_paint::{self, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, Document, FX_SLOTS, INSERT_SLOTS, RackDoc, WorkflowCommand,
    insert_chain, player_stack,
};
use vello::kurbo::{Point, Rect, RoundedRect};

/// Width of the panel in logical pixels.
pub const TRACK_PANEL_WIDTH: f64 = 236.0;
const ROW_H: f64 = 22.0;
const HEADING_H: f64 = 20.0;
const BUTTON: f64 = 18.0;
const PAD: f64 = 6.0;
const MERGE_BASE: u64 = 19 << 40;

/// One bypass, move or remove control of a row, with the command a press emits.
#[derive(Debug, Clone, PartialEq)]
pub struct RowControls {
    /// The device whose `bypass` param the button flips, and whether it is on now.
    pub bypass: Option<(DeviceKey, bool)>,
    /// Move one place up the stack (`None`: it cannot move).
    pub up: Option<Command>,
    /// Move one place down the stack.
    pub down: Option<Command>,
    /// Take the device out of the chain.
    pub remove: Option<Command>,
}

/// One line of the panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A section heading.
    Heading(String),
    /// A note player of the stack.
    Player {
        /// The player's name.
        name: String,
        /// Its controls.
        controls: RowControls,
    },
    /// The track's instrument (or the audio track's name).
    Instrument {
        /// The device name.
        name: String,
    },
    /// A filled insert slot.
    Insert {
        /// The effect's name.
        name: String,
        /// Its controls.
        controls: RowControls,
    },
    /// A free insert slot.
    EmptyInsert,
    /// A send of the channel.
    Send {
        /// The send number, 1-based.
        number: usize,
        /// The channel.
        channel: DeviceKey,
        /// Whether it is on.
        on: bool,
        /// Its level in dB.
        level: f32,
    },
    /// The fader and mute of the channel.
    Fader {
        /// The channel.
        channel: DeviceKey,
        /// Fader position, 0 to 1.
        position: f32,
        /// Whether the channel is muted.
        muted: bool,
    },
    /// A line of explanation (a track without a channel).
    Note(String),
}

/// What a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Bypass,
    Up,
    Down,
    Remove,
    Send,
    Mute,
    Fader,
}

#[derive(Debug, Clone)]
struct FaderDrag {
    channel: DeviceKey,
    merge: u64,
    last: f32,
}

/// The Track Panel pane.
pub struct TrackPanelView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    track: Option<DeviceKey>,
    track_name: String,
    items: Vec<Item>,
    drag: Option<FaderDrag>,
    hover: Option<(usize, Part)>,
    merges: u64,
    repaint: std::cell::Cell<bool>,
}

impl TrackPanelView {
    /// A panel with nothing shown.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, TRACK_PANEL_WIDTH, 400.0),
            scale: 1.0,
            track: None,
            track_name: String::new(),
            items: Vec::new(),
            drag: None,
            hover: None,
            merges: 0,
            repaint: std::cell::Cell::new(true),
        }
    }

    /// Switches to `theme` (the next paint uses it).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint.set(true);
    }

    /// Whether something the panel shows changed since the last paint (a hover, a drag, a sync).
    pub fn needs_repaint(&self) -> bool {
        self.repaint.get()
    }

    /// Where the panel is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    /// Where the panel is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Display scale factor (the panel paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
    }

    /// The track the panel shows.
    pub fn track(&self) -> Option<&DeviceKey> {
        self.track.as_ref()
    }

    /// The lines of the panel, top to bottom.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Rebuilds the lines for `track` from the document (nothing when it is `None` or is not a
    /// track). A fader drag in progress is left running.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry, track: Option<&DeviceKey>) {
        self.track = track.cloned();
        self.items = build_items(doc, registry, track);
        self.repaint.set(true);
        self.track_name = track
            .and_then(|t| doc.device(t))
            .map(|d| d.name.clone())
            .unwrap_or_default();
    }

    // ------------------------------------------------------------------ geometry

    fn row_rect(&self, i: usize) -> Rect {
        let mut y = self.bounds.y0 + 26.0;
        for item in &self.items[..i] {
            y += item_height(item);
        }
        Rect::new(
            self.bounds.x0,
            y,
            self.bounds.x1,
            y + item_height(&self.items[i]),
        )
    }

    /// The button `part` of a row, counted from the right edge.
    fn button_rect(&self, row: Rect, part: Part) -> Rect {
        let slot = match part {
            Part::Remove | Part::Mute | Part::Send => 0.0,
            Part::Down => 1.0,
            Part::Up => 2.0,
            Part::Bypass => 3.0,
            Part::Fader => return self.fader_rect(row),
        };
        let x1 = row.x1 - PAD - slot * (BUTTON + 2.0);
        Rect::new(
            x1 - BUTTON,
            row.center().y - BUTTON / 2.0,
            x1,
            row.center().y + BUTTON / 2.0,
        )
    }

    fn fader_rect(&self, row: Rect) -> Rect {
        Rect::new(
            row.x0 + 46.0,
            row.center().y - 5.0,
            row.x1 - PAD * 2.0 - BUTTON,
            row.center().y + 5.0,
        )
    }

    fn parts_of(item: &Item) -> &'static [Part] {
        match item {
            Item::Player { .. } | Item::Insert { .. } => {
                &[Part::Bypass, Part::Up, Part::Down, Part::Remove]
            }
            Item::Send { .. } => &[Part::Send],
            Item::Fader { .. } => &[Part::Fader, Part::Mute],
            _ => &[],
        }
    }

    fn hit(&self, pos: Point) -> Option<(usize, Part)> {
        if !self.bounds.contains(pos) {
            return None;
        }
        (0..self.items.len()).find_map(|i| {
            let row = self.row_rect(i);
            Self::parts_of(&self.items[i])
                .iter()
                .find(|p| self.button_rect(row, **p).inset(2.0).contains(pos))
                .map(|p| (i, *p))
        })
    }

    // ------------------------------------------------------------------ events

    /// The control under `pos`, for the hover help.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let (i, part) = self.hit(pos)?;
        let text = match part {
            Part::Bypass => "Bypass the device",
            Part::Up => "Move up the chain",
            Part::Down => "Move down the chain",
            Part::Remove => "Take the device out of the chain",
            Part::Send => "Switch the send on or off",
            Part::Mute => "Mute the channel",
            Part::Fader => "Channel fader",
        };
        Some(Help::new(self.button_rect(self.row_rect(i), part), text))
    }

    /// Handles a pointer event; the actions are commands, one undo step each.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let hover = self.hit(*pos);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint.set(true);
                }
                if self.drag.is_some() {
                    self.drag_to(pos.x, &mut out);
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some((i, part)) = self.hit(*pos) {
                    self.press(i, part, pos.x, &mut out);
                }
            }
            UiEvent::PointerUp { .. } => {
                if self.drag.take().is_some() {
                    out.push(UiAction::EndMerge);
                }
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint.set(true);
            }
            _ => {}
        }
        out
    }

    fn press(&mut self, i: usize, part: Part, x: f64, out: &mut Vec<UiAction>) {
        let command = |c: &Option<Command>| c.clone().map(UiAction::Command);
        match (&self.items[i], part) {
            (
                Item::Player { controls, .. } | Item::Insert { controls, .. },
                Part::Bypass | Part::Up | Part::Down | Part::Remove,
            ) => {
                let action = match part {
                    Part::Bypass => controls.bypass.as_ref().map(|(key, on)| {
                        UiAction::Command(Command::SetParam {
                            key: key.clone(),
                            param: "bypass".to_owned(),
                            value: if *on { 0.0 } else { 1.0 },
                            merge: None,
                        })
                    }),
                    Part::Up => command(&controls.up),
                    Part::Down => command(&controls.down),
                    _ => command(&controls.remove),
                };
                out.extend(action);
            }
            (
                Item::Send {
                    number,
                    channel,
                    on,
                    ..
                },
                Part::Send,
            ) => out.push(UiAction::Command(Command::SetParam {
                key: channel.clone(),
                param: format!("send{number}.on"),
                value: if *on { 0.0 } else { 1.0 },
                merge: None,
            })),
            (Item::Fader { channel, muted, .. }, Part::Mute) => {
                out.push(UiAction::Command(Command::SetParam {
                    key: channel.clone(),
                    param: "mute".to_owned(),
                    value: if *muted { 0.0 } else { 1.0 },
                    merge: None,
                }));
            }
            (
                Item::Fader {
                    channel, position, ..
                },
                Part::Fader,
            ) => {
                self.merges += 1;
                self.drag = Some(FaderDrag {
                    channel: channel.clone(),
                    merge: MERGE_BASE + self.merges,
                    last: *position,
                });
                self.drag_to_row(i, x, out);
            }
            _ => {}
        }
    }

    fn drag_to(&mut self, x: f64, out: &mut Vec<UiAction>) {
        let Some(i) = self
            .items
            .iter()
            .position(|it| matches!(it, Item::Fader { .. }))
        else {
            return;
        };
        self.drag_to_row(i, x, out);
    }

    fn drag_to_row(&mut self, i: usize, x: f64, out: &mut Vec<UiAction>) {
        let track = self.fader_rect(self.row_rect(i));
        let value = ((x - track.x0) / track.width().max(1.0)).clamp(0.0, 1.0) as f32;
        let Some(drag) = &mut self.drag else { return };
        if (value - drag.last).abs() < 1e-4 {
            return;
        }
        drag.last = value;
        out.push(UiAction::Command(Command::SetParam {
            key: drag.channel.clone(),
            param: "fader".to_owned(),
            value,
            merge: Some(drag.merge),
        }));
    }

    // ------------------------------------------------------------------ paint

    /// Draws the panel inside its bounds.
    pub fn paint(&self, c: &mut dyn Canvas) {
        self.repaint.set(false);
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        self.paint_logical(c);
        c.pop_transform();
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(b, self.st.header);
        c.fill_with(Rect::new(b.x0, b.y0, b.x0 + 1.0, b.y1), self.st.separator);
        let title = if self.track.is_some() {
            self.track_name.to_uppercase()
        } else {
            "TRACK PANEL".to_owned()
        };
        c.text(&TextRun::new(title, pt(b.x0 + PAD * 2.0, b.y0 + 17.0), 10.5, t.text).weight(700));
        if self.track.is_none() {
            c.text(&TextRun::new(
                "Select a track",
                pt(b.x0 + PAD * 2.0, b.y0 + 44.0),
                10.0,
                t.text_muted,
            ));
        }
        for (i, item) in self.items.iter().enumerate() {
            let row = self.row_rect(i);
            if row.y1 > b.y1 {
                break;
            }
            self.paint_item(c, i, item, row);
        }
    }

    fn label(&self, c: &mut dyn Canvas, row: Rect, text: &str, width: f64, muted: bool) {
        let t = &self.st.base;
        let max = ((width - 12.0) / 5.6).max(4.0) as usize;
        let text = if text.chars().count() > max {
            let cut: String = text.chars().take(max.saturating_sub(1)).collect();
            format!("{cut}...")
        } else {
            text.to_owned()
        };
        c.text(&TextRun::new(
            text,
            pt(row.x0 + PAD * 2.0, row.center().y + 3.5),
            10.0,
            if muted { t.text_muted } else { t.text },
        ));
    }

    fn button(&self, c: &mut dyn Canvas, i: usize, row: Rect, part: Part, text: &str, lit: bool) {
        let hover = self.hover == Some((i, part));
        let color = lit.then_some(self.st.base.highlight);
        seq_paint::button(c, &self.st, self.button_rect(row, part), text, color, hover);
    }

    fn paint_item(&self, c: &mut dyn Canvas, i: usize, item: &Item, row: Rect) {
        let t = &self.st.base;
        match item {
            Item::Heading(text) => {
                c.fill_with(
                    Rect::new(row.x0 + 1.0, row.y1 - 1.0, row.x1, row.y1),
                    self.st.separator,
                );
                c.text(
                    &TextRun::new(
                        text.to_uppercase(),
                        pt(row.x0 + PAD * 2.0, row.y1 - 5.0),
                        8.0,
                        t.text_muted,
                    )
                    .weight(700),
                );
            }
            Item::Player { name, controls } | Item::Insert { name, controls } => {
                let bypassed = controls.bypass.as_ref().is_some_and(|(_, on)| *on);
                self.label(c, row, name, row.width() - 4.0 * (BUTTON + 2.0), bypassed);
                if controls.bypass.is_some() {
                    self.button(c, i, row, Part::Bypass, "B", bypassed);
                }
                if controls.up.is_some() {
                    self.button(c, i, row, Part::Up, "^", false);
                }
                if controls.down.is_some() {
                    self.button(c, i, row, Part::Down, "v", false);
                }
                if controls.remove.is_some() {
                    self.button(c, i, row, Part::Remove, "x", false);
                }
            }
            Item::Instrument { name } => self.label(c, row, name, row.width(), false),
            Item::EmptyInsert => self.label(c, row, "(empty)", row.width(), true),
            Item::Send {
                number, on, level, ..
            } => {
                self.label(
                    c,
                    row,
                    &format!("Send {number}  {level:+.1} dB"),
                    row.width() - BUTTON,
                    !on,
                );
                self.button(c, i, row, Part::Send, if *on { "ON" } else { "-" }, *on);
            }
            Item::Fader {
                position, muted, ..
            } => {
                c.text(&TextRun::new(
                    "Fader",
                    pt(row.x0 + PAD * 2.0, row.center().y + 3.5),
                    10.0,
                    t.text,
                ));
                let track = self.fader_rect(row);
                c.fill_with(RoundedRect::from_rect(track, 3.0), t.lcd);
                let x = track.x0 + track.width() * f64::from(*position);
                c.fill_with(
                    RoundedRect::from_rect(Rect::new(track.x0, track.y0, x, track.y1), 3.0),
                    mix(t.highlight, t.lcd, 0.35),
                );
                c.fill_with(
                    RoundedRect::from_rect(
                        Rect::new(x - 2.0, track.y0 - 3.0, x + 2.0, track.y1 + 3.0),
                        1.5,
                    ),
                    t.text,
                );
                self.button(c, i, row, Part::Mute, "M", *muted);
            }
            Item::Note(text) => {
                c.text(&TextRun::new(
                    text.clone(),
                    pt(row.x0 + PAD * 2.0, row.center().y + 3.5),
                    9.5,
                    t.text_muted,
                ));
            }
        }
    }
}

fn item_height(item: &Item) -> f64 {
    match item {
        Item::Heading(_) => HEADING_H,
        _ => ROW_H,
    }
}

// ---------------------------------------------------------------------- derivation

fn param(doc: &RackDoc, registry: &Registry, key: &DeviceKey, name: &str) -> Option<f32> {
    let device = doc.device(key)?;
    let p = *registry
        .get(&device.type_id)?
        .descriptor
        .param_by_key(name)?;
    Some(p.clamp(device.params.get(name).copied().unwrap_or(p.default)))
}

fn bypass_of(doc: &RackDoc, registry: &Registry, key: &DeviceKey) -> Option<(DeviceKey, bool)> {
    param(doc, registry, key, "bypass").map(|v| (key.clone(), v >= 0.5))
}

/// The document after `command`, or `None` when the model refuses it.
fn plan(doc: &RackDoc, registry: &Registry, command: &Command) -> Option<RackDoc> {
    let mut scratch = Document::new(doc.clone());
    scratch.apply(command.clone(), registry).ok()?;
    Some(scratch.doc().clone())
}

fn name_of(doc: &RackDoc, key: &DeviceKey) -> String {
    doc.device(key)
        .map_or_else(|| key.0.clone(), |d| d.name.clone())
}

/// The commands that move player `at` of `stack` up or down one place, kept only when the
/// planned document really has the stack in the new order.
fn player_moves(
    doc: &RackDoc,
    registry: &Registry,
    track: &DeviceKey,
    stack: &[DeviceKey],
    at: usize,
) -> (Option<Command>, Option<Command>) {
    let key = &stack[at];
    let others: Vec<&DeviceKey> = doc
        .devices
        .iter()
        .map(|d| &d.key)
        .filter(|k| *k != key)
        .collect();
    let place = |neighbour: &DeviceKey, after: bool| {
        others
            .iter()
            .position(|k| *k == neighbour)
            .map(|p| p + usize::from(after))
    };
    let check = |to: Option<usize>, swap_with: usize| {
        let to = to?;
        let command = Command::Workflow(WorkflowCommand::MoveDevices {
            keys: vec![key.clone()],
            to,
            route: true,
        });
        let planned = plan(doc, registry, &command)?;
        let now = planned
            .song
            .track(track)
            .map(|t| player_stack(&planned, registry, t))?;
        let mut want = stack.to_vec();
        want.swap(at, swap_with);
        (now == want).then_some(command)
    };
    let up = at
        .checked_sub(1)
        .and_then(|prev| check(place(&stack[prev], false), prev));
    let down = stack
        .get(at + 1)
        .and_then(|next| check(place(next, true), at + 1));
    (up, down)
}

fn build_items(doc: &RackDoc, registry: &Registry, track: Option<&DeviceKey>) -> Vec<Item> {
    let Some(track_key) = track else {
        return Vec::new();
    };
    let Some(song_track) = doc.song.track(track_key) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    items.push(Item::Heading("Players".to_owned()));
    let stack = player_stack(doc, registry, song_track);
    if stack.is_empty() {
        items.push(Item::Note(
            "No players: notes go to the instrument".to_owned(),
        ));
    }
    for (i, key) in stack.iter().enumerate() {
        let (up, down) = player_moves(doc, registry, track_key, &stack, i);
        items.push(Item::Player {
            name: name_of(doc, key),
            controls: RowControls {
                bypass: bypass_of(doc, registry, key),
                up,
                down,
                remove: Some(Command::Workflow(WorkflowCommand::RemoveDevices {
                    keys: vec![key.clone()],
                }))
                .filter(|c| plan(doc, registry, c).is_some()),
            },
        });
    }
    items.push(Item::Heading(
        if song_track.is_audio() {
            "Audio track"
        } else {
            "Instrument"
        }
        .to_owned(),
    ));
    items.push(Item::Instrument {
        name: name_of(doc, track_key),
    });
    let Some(channel) = doc.console.channel_of(track_key).map(|c| c.channel.clone()) else {
        items.push(Item::Heading("Console".to_owned()));
        items.push(Item::Note("No console channel".to_owned()));
        return items;
    };
    items.push(Item::Heading("Inserts".to_owned()));
    let chain = insert_chain(doc, registry, &channel);
    for slot in 0..INSERT_SLOTS {
        let Some(effect) = chain.get(slot) else {
            items.push(Item::EmptyInsert);
            continue;
        };
        let moved = |to: usize| {
            let command = Command::Console(ConsoleCommand::MoveInsert {
                channel: channel.clone(),
                from: slot,
                to,
            });
            let planned = plan(doc, registry, &command)?;
            let now = insert_chain(&planned, registry, &channel);
            let mut want = chain.clone();
            want.swap(slot, to);
            (now == want).then_some(command)
        };
        items.push(Item::Insert {
            name: name_of(doc, effect),
            controls: RowControls {
                bypass: bypass_of(doc, registry, effect),
                up: slot.checked_sub(1).and_then(moved),
                down: (slot + 1 < chain.len()).then(|| moved(slot + 1)).flatten(),
                remove: Some(Command::Console(ConsoleCommand::SetInsert {
                    channel: channel.clone(),
                    slot,
                    effect: None,
                }))
                .filter(|c| plan(doc, registry, c).is_some()),
            },
        });
    }
    items.push(Item::Heading("Sends".to_owned()));
    for k in 1..=FX_SLOTS {
        let on = param(doc, registry, &channel, &format!("send{k}.on"));
        let Some(on) = on else { continue };
        items.push(Item::Send {
            number: k,
            channel: channel.clone(),
            on: on >= 0.5,
            level: param(doc, registry, &channel, &format!("send{k}.level")).unwrap_or(0.0),
        });
    }
    items.push(Item::Fader {
        position: param(doc, registry, &channel, "fader").unwrap_or(0.75),
        muted: param(doc, registry, &channel, "mute").is_some_and(|v| v >= 0.5),
        channel,
    });
    items
}
