//! The gesture commands page of Settings > Camera: which app command each gesture runs.
//!
//! One row for each gesture of the fixed vocabulary (fist, open palm, thumbs up, point, peace,
//! swipe left and swipe right): a button that says which hand counts (either, left or right; it
//! is on when the row has a command) and a button with the command's name. Pressing the command
//! button opens the **picker**, a list of every command that takes no argument with a search
//! field, like the keyboard shortcuts editor's: type to narrow the list, Up and Down move,
//! Enter chooses, Home and End jump to the ends, Escape closes the picker (a second Escape goes
//! back to the camera settings).
//! **None** at the top of the list takes the command away. **Suggested** fills in fist = Stop,
//! open palm held for a second = Play and thumbs up = Record; **Clear** empties the table.
//!
//! The table starts empty: nothing happens by surprise. The page edits the table it is given
//! (the settings', or the project's own while **Use for this project only** is on).

use crate::camera_gestures::{GestureBinding, suggested};
use crate::keymap_store::HandChoice;
use crate::video_tab::wrap;
use atomik_model::CameraGesture;
use atomik_ui::keymap::{CommandDef, Registry};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

const ROW: f64 = 40.0;
const GAP: f64 = 2.0;
const HAND_W: f64 = 76.0;
const COMMAND_W: f64 = 230.0;
const BUTTON_H: f64 = 28.0;
const LIST_ROW: f64 = 26.0;
/// How many commands the picker shows at once.
pub const PICKER_ROWS: usize = 8;
const NOTES: [&str; 2] = [
    "Nothing runs until you choose a command. A gesture runs once each time you make it, and not within half a second of another or while you pinch.",
    "The gestures are also sent as the notes 62 to 67 of the input Camera, for MIDI Learn.",
];
const NOTE_WIDTH: usize = 78;

/// The name of a gesture on the page.
pub fn gesture_name(gesture: CameraGesture) -> &'static str {
    match gesture {
        CameraGesture::Fist => "Fist",
        CameraGesture::OpenPalm => "Open palm",
        CameraGesture::ThumbsUp => "Thumbs up",
        CameraGesture::Point => "Point",
        CameraGesture::Peace => "Peace",
        CameraGesture::SwipeLeft => "Swipe left",
        CameraGesture::SwipeRight => "Swipe right",
    }
}

/// What the page says under a gesture's name.
fn gesture_hint(gesture: CameraGesture) -> &'static str {
    match gesture {
        CameraGesture::Fist => "All fingers curled, held",
        CameraGesture::OpenPalm => "All fingers out, held for a second",
        CameraGesture::ThumbsUp => "A fist with the thumb up",
        CameraGesture::Point => "Only the index finger up",
        CameraGesture::Peace => "Index and middle fingers up",
        CameraGesture::SwipeLeft => "The palm sweeps to the left",
        CameraGesture::SwipeRight => "The palm sweeps to the right",
    }
}

/// The commands the picker offers: every one that takes no argument, by name.
pub fn pickable() -> Vec<&'static CommandDef> {
    let mut list: Vec<&'static CommandDef> = Registry::bundled()
        .commands()
        .iter()
        .filter(|d| d.args.is_none())
        .collect();
    list.sort_by(|a, b| a.label.cmp(b.label).then(a.id.cmp(b.id)));
    list
}

/// The commands of [`pickable`] that match the typed `query` (words of it, in any order, found in
/// the name or the id).
pub fn matching(query: &str) -> Vec<&'static CommandDef> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    pickable()
        .into_iter()
        .filter(|d| {
            let name = d.label.to_lowercase();
            words.iter().all(|w| name.contains(w) || d.id.contains(w))
        })
        .collect()
}

/// The name of a command as the page shows it: its label, or the id of a command this version
/// does not know.
fn command_text(id: &str) -> String {
    Registry::bundled().get(id).map_or_else(
        || id.to_owned(),
        |d| atomik_ui::i18n::tr(d.label).into_owned(),
    )
}

/// The picker while it is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    /// The gesture whose command is being chosen.
    pub gesture: CameraGesture,
    /// What was typed.
    pub query: String,
    /// The highlighted entry (0 is None, then the matches).
    pub selected: usize,
    /// The first entry shown.
    pub top: usize,
}

/// One gesture row.
#[derive(Debug, Clone, PartialEq)]
pub struct GestureRowLayout {
    /// The gesture.
    pub gesture: CameraGesture,
    /// The whole row.
    pub rect: Rect,
    /// The button that cycles the hand.
    pub hand: Rect,
    /// The button that opens the picker.
    pub command: Rect,
}

/// The picker's parts.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerLayout {
    /// The whole area.
    pub rect: Rect,
    /// The search field.
    pub field: Rect,
    /// The visible entries: the index into the entries (0 is None) and its rectangle.
    pub rows: Vec<(usize, Rect)>,
}

/// Where the page's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLayout {
    /// Back to the camera settings.
    pub back: Rect,
    /// Fills in the suggested table.
    pub suggested: Rect,
    /// Empties the table.
    pub clear: Rect,
    /// One row for each gesture (empty while the picker is open).
    pub rows: Vec<GestureRowLayout>,
    /// The picker, while it is open.
    pub picker: Option<PickerLayout>,
    /// The notes under the rows.
    pub notes: Vec<(Point, String)>,
}

/// What the page asks of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOutcome {
    /// Stay on the page.
    Stay,
    /// Go back to the camera settings.
    Back,
    /// Apply the dialog (Enter outside the picker).
    Apply,
}

/// The gesture commands page. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct GesturePage {
    picker: Option<Picker>,
    hover: Option<Point>,
}

impl GesturePage {
    /// A page with no picker open.
    pub fn new() -> Self {
        Self::default()
    }

    /// The picker, while it is open.
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// Opens the picker for `gesture`, on its current command.
    pub fn open_picker(&mut self, gesture: CameraGesture, table: &[GestureBinding]) {
        let current = crate::camera_gestures::row_of(table, gesture).map(|b| b.command.as_str());
        let entries = matching("");
        let selected = current
            .and_then(|c| entries.iter().position(|d| d.id == c))
            .map_or(0, |i| i + 1);
        self.picker = Some(Picker {
            gesture,
            query: String::new(),
            selected,
            top: selected.saturating_sub(PICKER_ROWS / 2),
        });
    }

    /// The height the page needs.
    pub fn content_height() -> f64 {
        let notes: f64 = NOTES
            .iter()
            .map(|n| 16.0 * wrap(n, NOTE_WIDTH).len() as f64 + 6.0)
            .sum();
        // The buttons row, the seven gestures, then the notes.
        8.0 * (ROW + GAP) + 12.0 + notes
    }

    /// Lays the page out inside `panel`.
    pub fn layout(&self, panel: Rect, table: &[GestureBinding]) -> PageLayout {
        let x0 = panel.x0 + crate::keyboard_tab::PAD;
        let inner = panel.width() - 2.0 * crate::keyboard_tab::PAD;
        let mut y = panel.y0 + crate::keyboard_tab::TITLE;
        let head = Rect::new(x0, y, x0 + inner, y + ROW);
        y += ROW + GAP;
        let button = |r: Rect, left: f64, w: f64| {
            Rect::new(left, r.y0 + 6.0, left + w, r.y0 + 6.0 + BUTTON_H)
        };
        let back = button(head, head.x0, 76.0);
        let clear = button(head, head.x1 - 76.0, 76.0);
        let suggested = button(head, clear.x0 - GAP - 104.0, 104.0);

        let mut rows = Vec::new();
        let mut picker = None;
        if let Some(p) = &self.picker {
            let area = Rect::new(x0, y, x0 + inner, y + 7.0 * (ROW + GAP));
            let field = Rect::new(area.x0, area.y0 + 4.0, area.x1, area.y0 + 4.0 + BUTTON_H);
            let entries = matching(&p.query).len() + 1;
            let first = field.y1 + 6.0;
            let list: Vec<(usize, Rect)> = (p.top..entries.min(p.top + PICKER_ROWS))
                .enumerate()
                .map(|(n, index)| {
                    let top = first + n as f64 * LIST_ROW;
                    (
                        index,
                        Rect::new(area.x0, top, area.x1, top + LIST_ROW - 2.0),
                    )
                })
                .collect();
            picker = Some(PickerLayout {
                rect: area,
                field,
                rows: list,
            });
        } else {
            for gesture in CameraGesture::ALL {
                let rect = Rect::new(x0, y, x0 + inner, y + ROW);
                y += ROW + GAP;
                let command = button(rect, rect.x1 - COMMAND_W, COMMAND_W);
                let hand = button(rect, command.x0 - GAP - HAND_W, HAND_W);
                rows.push(GestureRowLayout {
                    gesture,
                    rect,
                    hand,
                    command,
                });
            }
        }
        let _ = table;
        let mut ny = panel.y0 + crate::keyboard_tab::TITLE + 8.0 * (ROW + GAP) + 12.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(&atomik_ui::i18n::tr(note), NOTE_WIDTH) {
                notes.push((Point::new(x0, ny + 12.0), line));
                ny += 16.0;
            }
            ny += 6.0;
        }
        PageLayout {
            back,
            suggested,
            clear,
            rows,
            picker,
            notes,
        }
    }

    /// Sets the command of `gesture` in `table` (an empty command takes the row away).
    fn set_command(table: &mut Vec<GestureBinding>, gesture: CameraGesture, command: &str) {
        match table.iter().position(|b| b.gesture == gesture) {
            Some(i) if command.is_empty() => {
                table.remove(i);
            }
            Some(i) => table[i].command = command.to_owned(),
            None if command.is_empty() => {}
            None => table.push(GestureBinding {
                gesture,
                hand: HandChoice::Either,
                command: command.to_owned(),
            }),
        }
    }

    /// Chooses entry `index` of the picker (0 is None).
    fn choose(&mut self, index: usize, table: &mut Vec<GestureBinding>) {
        let Some(p) = self.picker.take() else { return };
        let command = if index == 0 {
            ""
        } else {
            matching(&p.query).get(index - 1).map_or("", |d| d.id)
        };
        Self::set_command(table, p.gesture, command);
    }

    /// Moves the highlight of the picker by `delta` entries, keeping it in view.
    fn step(&mut self, delta: i64) {
        let Some(p) = &mut self.picker else { return };
        let last = matching(&p.query).len();
        p.selected = (p.selected as i64 + delta).clamp(0, last as i64) as usize;
        if p.selected < p.top {
            p.top = p.selected;
        } else if p.selected >= p.top + PICKER_ROWS {
            p.top = p.selected + 1 - PICKER_ROWS;
        }
    }

    /// Handles an event inside `panel`.
    pub fn handle(
        &mut self,
        event: &UiEvent,
        panel: Rect,
        table: &mut Vec<GestureBinding>,
    ) -> PageOutcome {
        match event {
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Wheel { delta, .. } => {
                if let Some(p) = &mut self.picker {
                    let entries = matching(&p.query).len() + 1;
                    let more = entries.saturating_sub(PICKER_ROWS);
                    let by = if delta.y > 0.0 { -2_i64 } else { 2 };
                    p.top = (p.top as i64 + by).clamp(0, more as i64) as usize;
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                if self.picker.is_some() {
                    match key {
                        Key::Escape => self.picker = None,
                        Key::Enter => {
                            let selected = self.picker.as_ref().map_or(0, |p| p.selected);
                            self.choose(selected, table);
                        }
                        Key::Up => self.step(-1),
                        Key::Down => self.step(1),
                        Key::Home => self.step(i64::MIN / 2),
                        Key::End => self.step(i64::MAX / 2),
                        Key::PageUp => self.step(-(PICKER_ROWS as i64)),
                        Key::PageDown => self.step(PICKER_ROWS as i64),
                        Key::Backspace => {
                            if let Some(p) = &mut self.picker {
                                p.query.pop();
                                p.selected = 0;
                                p.top = 0;
                            }
                        }
                        Key::Space | Key::Char(_) => {
                            let c = if let Key::Char(c) = key { *c } else { ' ' };
                            if let Some(p) = &mut self.picker
                                && !c.is_control()
                                && p.query.chars().count() < 40
                            {
                                p.query.push(c);
                                p.selected = usize::from(!matching(&p.query).is_empty());
                                p.top = 0;
                            }
                        }
                        _ => {}
                    }
                } else {
                    match key {
                        Key::Escape => return PageOutcome::Back,
                        Key::Enter => return PageOutcome::Apply,
                        _ => {}
                    }
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let l = self.layout(panel, table);
                if let Some(pl) = &l.picker {
                    if let Some((index, _)) = pl.rows.iter().find(|(_, r)| r.contains(*pos)) {
                        self.choose(*index, table);
                    }
                    return PageOutcome::Stay;
                }
                if l.back.contains(*pos) {
                    return PageOutcome::Back;
                } else if l.suggested.contains(*pos) {
                    *table = suggested();
                } else if l.clear.contains(*pos) {
                    table.clear();
                } else if let Some(row) = l.rows.iter().find(|r| r.command.contains(*pos)) {
                    self.open_picker(row.gesture, table);
                } else if let Some(row) = l.rows.iter().find(|r| r.hand.contains(*pos))
                    && let Some(b) = table.iter_mut().find(|b| b.gesture == row.gesture)
                {
                    let all = HandChoice::ALL;
                    let at = all.iter().position(|h| *h == b.hand).unwrap_or(0);
                    b.hand = all[(at + 1) % all.len()];
                }
            }
            _ => {}
        }
        PageOutcome::Stay
    }

    /// Paints the page inside `panel`.
    pub fn paint(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        panel: Rect,
        table: &[GestureBinding],
    ) {
        let l = self.layout(panel, table);
        let lit = |r: Rect| self.hover.is_some_and(|p| r.contains(p));
        let button = |canvas: &mut dyn Canvas, r: Rect, label: &str, on: bool, enabled: bool| {
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if on || (enabled && lit(r)) {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 18.0),
                    12.0,
                    if enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                )
                .align(Align::Center),
            );
        };
        button(canvas, l.back, "Back", false, true);
        button(canvas, l.suggested, "Suggested", false, true);
        button(canvas, l.clear, "Clear", false, !table.is_empty());
        for row in &l.rows {
            canvas.text(&TextRun::new(
                gesture_name(row.gesture),
                Point::new(row.rect.x0 + 8.0, row.rect.y0 + 18.0),
                13.0,
                theme.text,
            ));
            canvas.text(&TextRun::new(
                gesture_hint(row.gesture),
                Point::new(row.rect.x0 + 8.0, row.rect.y0 + 36.0),
                11.0,
                theme.text_muted,
            ));
            let binding = crate::camera_gestures::row_of(table, row.gesture);
            button(
                canvas,
                row.hand,
                binding.map_or(HandChoice::Either, |b| b.hand).label(),
                false,
                binding.is_some(),
            );
            match binding {
                Some(b) => {
                    canvas.fill_with(
                        RoundedRect::from_rect(row.command, 6.0),
                        if lit(row.command) {
                            theme.selection
                        } else {
                            theme.browser_row
                        },
                    );
                    canvas.text(
                        &TextRun::verbatim(
                            atomik_ui::paint::fit_label_raw(
                                &command_text(&b.command),
                                COMMAND_W - 16.0,
                                12.0,
                            ),
                            Point::new(row.command.center().x, row.command.y0 + 18.0),
                            12.0,
                            theme.text,
                        )
                        .align(Align::Center),
                    );
                }
                None => button(canvas, row.command, "None", false, true),
            }
        }
        if let (Some(p), Some(pl)) = (&self.picker, &l.picker) {
            canvas.text(&TextRun::new(
                gesture_name(p.gesture),
                Point::new(pl.field.x0, pl.rect.y0 - 4.0),
                11.0,
                theme.text_muted,
            ));
            canvas.fill_with(RoundedRect::from_rect(pl.field, 6.0), theme.browser_row);
            canvas.stroke_with(RoundedRect::from_rect(pl.field, 6.0), 1.2, theme.focus);
            let shown = if p.query.is_empty() {
                atomik_ui::i18n::tr("Type to search the commands").into_owned()
            } else {
                p.query.clone()
            };
            canvas.text(&TextRun::verbatim(
                shown,
                Point::new(pl.field.x0 + 8.0, pl.field.y0 + 18.0),
                12.0,
                if p.query.is_empty() {
                    theme.text_muted
                } else {
                    theme.text
                },
            ));
            let found = matching(&p.query);
            for (index, rect) in &pl.rows {
                let on = *index == p.selected;
                if on || lit(*rect) {
                    canvas.fill_with(RoundedRect::from_rect(*rect, 4.0), theme.selection);
                }
                let text = if *index == 0 {
                    atomik_ui::i18n::tr("None").into_owned()
                } else {
                    found
                        .get(index - 1)
                        .map_or_else(String::new, |d| atomik_ui::i18n::tr(d.label).into_owned())
                };
                canvas.text(&TextRun::verbatim(
                    atomik_ui::paint::fit_label_raw(&text, rect.width() - 16.0, 12.0),
                    Point::new(rect.x0 + 8.0, rect.y0 + 16.0),
                    12.0,
                    theme.text,
                ));
            }
            if found.is_empty() {
                canvas.text(&TextRun::new(
                    "No command matches",
                    Point::new(pl.rect.x0 + 8.0, pl.field.y1 + 40.0),
                    12.0,
                    theme.text_muted,
                ));
            }
        }
        for (at, line) in &l.notes {
            canvas.text(&TextRun::verbatim(
                line.clone(),
                *at,
                11.5,
                theme.text_muted,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: Rect = Rect::new(0.0, 0.0, 640.0, 700.0);

    fn down(page: &mut GesturePage, table: &mut Vec<GestureBinding>, at: Point) -> PageOutcome {
        page.handle(
            &UiEvent::PointerDown {
                pos: at,
                button: PointerButton::Primary,
                modifiers: atomik_ui::Modifiers::NONE,
                clicks: 1,
            },
            PANEL,
            table,
        )
    }

    fn press(page: &mut GesturePage, table: &mut Vec<GestureBinding>, key: Key) -> PageOutcome {
        page.handle(
            &UiEvent::Key {
                key,
                modifiers: atomik_ui::Modifiers::NONE,
                pressed: true,
            },
            PANEL,
            table,
        )
    }

    fn type_text(page: &mut GesturePage, table: &mut Vec<GestureBinding>, text: &str) {
        for c in text.chars() {
            press(
                page,
                table,
                if c == ' ' { Key::Space } else { Key::Char(c) },
            );
        }
    }

    #[test]
    fn the_page_has_a_row_for_each_gesture_and_starts_with_none() {
        let page = GesturePage::new();
        let l = page.layout(PANEL, &[]);
        let gestures: Vec<CameraGesture> = l.rows.iter().map(|r| r.gesture).collect();
        assert_eq!(gestures, CameraGesture::ALL);
        assert!(l.picker.is_none());
    }

    #[test]
    fn suggested_fills_in_three_rows_and_clear_empties_the_table() {
        let mut page = GesturePage::new();
        let mut table = Vec::new();
        let l = page.layout(PANEL, &table);
        down(&mut page, &mut table, l.suggested.center());
        assert_eq!(table, suggested());
        let l = page.layout(PANEL, &table);
        down(&mut page, &mut table, l.clear.center());
        assert!(table.is_empty());
    }

    #[test]
    fn a_command_is_picked_by_typing_and_enter_and_none_takes_it_away() {
        let mut page = GesturePage::new();
        let mut table = Vec::new();
        let l = page.layout(PANEL, &table);
        let point = l
            .rows
            .iter()
            .find(|r| r.gesture == CameraGesture::Point)
            .unwrap();
        down(&mut page, &mut table, point.command.center());
        assert!(page.picker().is_some());
        type_text(&mut page, &mut table, "undo");
        press(&mut page, &mut table, Key::Enter);
        assert!(page.picker().is_none());
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].gesture, CameraGesture::Point);
        assert!(table[0].command.contains("undo"), "{}", table[0].command);
        // Again, and pick None.
        let l = page.layout(PANEL, &table);
        let point = l
            .rows
            .iter()
            .find(|r| r.gesture == CameraGesture::Point)
            .unwrap();
        down(&mut page, &mut table, point.command.center());
        press(&mut page, &mut table, Key::Home);
        let l = page.layout(PANEL, &table);
        let none = l.picker.unwrap().rows[0];
        assert_eq!(none.0, 0, "None is the first entry");
        down(&mut page, &mut table, none.1.center());
        assert!(table.is_empty());
    }

    #[test]
    fn the_hand_button_cycles_only_when_the_row_has_a_command() {
        let mut page = GesturePage::new();
        let mut table = suggested();
        let l = page.layout(PANEL, &table);
        let fist = l
            .rows
            .iter()
            .find(|r| r.gesture == CameraGesture::Fist)
            .unwrap();
        down(&mut page, &mut table, fist.hand.center());
        assert_eq!(table[0].hand, HandChoice::Left);
        down(&mut page, &mut table, fist.hand.center());
        down(&mut page, &mut table, fist.hand.center());
        assert_eq!(table[0].hand, HandChoice::Either);
        let point = l
            .rows
            .iter()
            .find(|r| r.gesture == CameraGesture::Point)
            .unwrap();
        down(&mut page, &mut table, point.hand.center());
        assert_eq!(table.len(), 3, "a row without a command gains nothing");
    }

    #[test]
    fn escape_closes_the_picker_first_and_then_leaves_the_page() {
        let mut page = GesturePage::new();
        let mut table = Vec::new();
        page.open_picker(CameraGesture::Fist, &table);
        assert_eq!(press(&mut page, &mut table, Key::Escape), PageOutcome::Stay);
        assert!(page.picker().is_none());
        assert_eq!(press(&mut page, &mut table, Key::Escape), PageOutcome::Back);
        assert_eq!(press(&mut page, &mut table, Key::Enter), PageOutcome::Apply);
    }

    #[test]
    fn only_commands_without_an_argument_are_offered_and_the_search_matches_names_and_ids() {
        assert!(pickable().iter().all(|d| d.args.is_none()));
        assert!(matching("stop").iter().any(|d| d.id == "transport.stop"));
        assert!(
            matching("transport play")
                .iter()
                .any(|d| d.id == "transport.play")
        );
        assert!(matching("zzzzqq").is_empty());
        for id in ["transport.stop", "transport.play", "transport.record"] {
            assert!(pickable().iter().any(|d| d.id == id), "{id}");
        }
    }

    #[test]
    fn up_and_down_move_the_highlight_and_scroll_the_list() {
        let mut page = GesturePage::new();
        let mut table = Vec::new();
        page.open_picker(CameraGesture::Fist, &table);
        for _ in 0..12 {
            press(&mut page, &mut table, Key::Down);
        }
        let p = page.picker().unwrap();
        assert_eq!(p.selected, 12);
        assert!(p.top > 0 && p.selected < p.top + PICKER_ROWS);
        for _ in 0..40 {
            press(&mut page, &mut table, Key::Up);
        }
        assert_eq!(page.picker().unwrap().selected, 0);
        assert_eq!(page.picker().unwrap().top, 0);
    }
}
