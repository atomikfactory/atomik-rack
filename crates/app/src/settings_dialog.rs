//! Audio ▸ Settings…: a modal drawn through the [`Canvas`].
//!
//! Choice: a minimal modal of the app's own rather than native per-setting menus. It works the
//! same on every platform (the in-window menu bar cannot nest submenus), shows every setting
//! at once, and its logic is plain data, unit-tested headlessly. Each section is a list of
//! radio options; nothing changes until **Apply** (Enter), and **Cancel** (Esc) discards the
//! draft. Applied settings are saved to the settings file.
//!
//! # Tabs
//!
//! With [`SettingsDialog::with_preferences`] the dialog gets a tab strip in its title row:
//! **Audio** (everything above, unchanged), **Keyboard** ([`crate::keyboard_tab`]) and
//! **Workflow** ([`crate::workflow_tab`]). The Keyboard and Workflow drafts are applied and
//! cancelled together with the audio draft: the shell reads [`SettingsDialog::preferences`]
//! when [`DialogOutcome::Apply`] arrives and saves `keymap.json` and `workflow.json`. Without
//! preferences the dialog is the audio-only dialog it always was.

use crate::keyboard_tab::{FOOTER, KeyboardLayout, KeyboardTab, PAD, TITLE, TabOutcome};
use crate::keymap_store::WorkflowPrefs;
use crate::menubar::text_width;
use crate::session::AudioBackend;
use crate::settings::{
    AudioSettings, BUFFER_CHOICES, HostChoice, LINGER_CHOICES, RECORD_OFFSET_CHOICES,
    WORKER_CHOICES,
};
use crate::workflow_tab::{WorkflowLayout, WorkflowTab};
use atomik_audio_io::HostInfo;
use atomik_ui::keymap::{KeymapEditor, UserKeymap};
use atomik_ui::kurbo::{Circle, Point, Rect, RoundedRect, Size, Vec2};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

/// The audio hosts, devices and MIDI ports the dialog offers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceLists {
    /// Available hosts and their devices.
    pub hosts: Vec<HostInfo>,
    /// MIDI input ports.
    pub midi_ports: Vec<String>,
    /// Listing problems to show.
    pub errors: Vec<String>,
    /// What the running stream opened for input (shown as a note), e.g.
    /// `Input: 2 channels open (Example USB ASIO)`.
    pub input_note: Option<String>,
}

impl DeviceLists {
    /// Lists what `backend` offers (errors are kept for display).
    pub fn query<B: AudioBackend>(backend: &B) -> Self {
        let mut errors = Vec::new();
        let hosts = backend.list_audio().unwrap_or_else(|e| {
            errors.push(format!("audio devices: {e}"));
            Vec::new()
        });
        let midi_ports = backend.list_midi().unwrap_or_else(|e| {
            errors.push(format!("MIDI ports: {e}"));
            Vec::new()
        });
        Self {
            hosts,
            midi_ports,
            errors,
            input_note: None,
        }
    }
}

/// One option of a section.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// Audio host.
    Host(HostChoice),
    /// Output device (`None` = host default).
    Device(Option<String>),
    /// Input device (`None` = the device named like the output, else the host default).
    Input(Option<String>),
    /// The user part of the recording compensation, in frames.
    RecordOffset(i32),
    /// Buffer size.
    Buffer(u32),
    /// MIDI input (`None` = none).
    Midi(Option<String>),
    /// Worker threads.
    Workers(u16),
    /// Worker linger spins.
    Linger(u64),
}

/// A titled group of options.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Title.
    pub title: &'static str,
    /// (label, choice) in display order.
    pub options: Vec<(String, Choice)>,
    /// Short options laid out side by side; otherwise one per line.
    pub chips: bool,
}

/// A tab of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Audio and MIDI devices (the original dialog).
    Audio,
    /// Keyboard shortcuts.
    Keyboard,
    /// Workflow preferences.
    Workflow,
}

impl Tab {
    /// Every tab, in strip order.
    pub const ALL: [Tab; 3] = [Tab::Audio, Tab::Keyboard, Tab::Workflow];

    /// The tab label.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Audio => "Audio",
            Tab::Keyboard => "Keyboard",
            Tab::Workflow => "Workflow",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Tab::Audio => "Audio settings",
            Tab::Keyboard => "Keyboard shortcuts",
            Tab::Workflow => "Workflow",
        }
    }
}

/// The drafts of the Keyboard and Workflow tabs.
#[derive(Debug, Clone)]
struct Prefs {
    keyboard: KeyboardTab,
    workflow: WorkflowTab,
}

/// Where everything is, for the current view size and scroll (logical units).
#[derive(Debug, Clone, PartialEq)]
pub struct DialogLayout {
    /// The dialog panel.
    pub panel: Rect,
    /// The scrolling area (options are clipped to it).
    pub content: Rect,
    /// Section titles.
    pub headers: Vec<(Rect, &'static str)>,
    /// Options: rectangle, label, choice.
    pub options: Vec<(Rect, String, Choice)>,
    /// Footer notes (target rule, listing errors).
    pub notes: Vec<(Point, String)>,
    /// Apply button.
    pub apply: Rect,
    /// Cancel button.
    pub cancel: Rect,
    /// The tab strip (empty for the audio-only dialog).
    pub tabs: Vec<(Rect, Tab)>,
    /// Height of all sections (for scrolling).
    pub content_height: f64,
}

/// The dialog result after an event.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogOutcome {
    /// Still open.
    Open,
    /// Apply these settings (sanitized) and close.
    Apply(AudioSettings),
    /// Close without changes.
    Cancel,
    /// The Keyboard tab asks for a file to export the keymap to (the dialog stays open).
    ExportKeymap,
    /// The Keyboard tab asks for a file to import a keymap from (the dialog stays open).
    ImportKeymap,
}

/// The settings modal.
#[derive(Debug, Clone)]
pub struct SettingsDialog {
    draft: AudioSettings,
    lists: DeviceLists,
    scroll: f64,
    hover: Option<Point>,
    tab: Tab,
    prefs: Option<Prefs>,
}

impl SettingsDialog {
    /// A dialog editing a copy of `current`.
    pub fn new(current: AudioSettings, lists: DeviceLists) -> Self {
        Self {
            draft: current,
            lists,
            scroll: 0.0,
            hover: None,
            tab: Tab::Audio,
            prefs: None,
        }
    }

    /// Adds the Keyboard and Workflow tabs, editing copies of `keymap` and `workflow`.
    pub fn with_preferences(mut self, keymap: KeymapEditor, workflow: WorkflowPrefs) -> Self {
        self.prefs = Some(Prefs {
            keyboard: KeyboardTab::new(keymap),
            workflow: WorkflowTab::new(workflow),
        });
        self
    }

    /// The tab that shows.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The Keyboard tab, when the dialog has preferences.
    pub fn keyboard(&self) -> Option<&KeyboardTab> {
        self.prefs.as_ref().map(|p| &p.keyboard)
    }

    /// The Keyboard tab layout, while that tab shows.
    pub fn keyboard_layout(&self, view: Size) -> Option<KeyboardLayout> {
        if self.tab != Tab::Keyboard {
            return None;
        }
        self.keyboard().map(|k| k.layout(self.panel(view)))
    }

    /// The Workflow tab layout, while that tab shows.
    pub fn workflow_layout(&self, view: Size) -> Option<WorkflowLayout> {
        if self.tab != Tab::Workflow {
            return None;
        }
        self.prefs
            .as_ref()
            .map(|p| p.workflow.layout(self.panel(view)))
    }

    /// The keymap diff and the workflow preferences as edited (`None` for the audio-only
    /// dialog): what the shell saves when the dialog is applied.
    pub fn preferences(&self) -> Option<(UserKeymap, WorkflowPrefs)> {
        self.prefs.as_ref().map(|p| {
            (
                p.keyboard.editor().user().clone(),
                p.workflow.prefs().clone(),
            )
        })
    }

    /// Types `text` into the Keyboard tab search field.
    pub fn keyboard_search(&mut self, text: &str) {
        if let Some(p) = &mut self.prefs {
            p.keyboard.set_search(text);
        }
    }

    /// Replaces the keymap draft (an imported file).
    pub fn replace_user(&mut self, user: UserKeymap) {
        if let Some(p) = &mut self.prefs {
            p.keyboard.replace_user(user);
        }
    }

    /// Whether the dialog needs every key as pressed (a chord or a role is being learned): the
    /// shell then passes numpad keys through as keys, not as the characters they type.
    pub fn capturing_keys(&self) -> bool {
        self.tab == Tab::Keyboard && self.keyboard().is_some_and(KeyboardTab::is_capturing)
    }

    /// The panel of the tab that shows.
    pub fn panel(&self, view: Size) -> Rect {
        match self.tab {
            Tab::Audio => self.layout(view).panel,
            Tab::Keyboard => {
                let width = (view.width - 32.0).clamp(320.0, 780.0);
                let height = (view.height - 48.0).clamp(300.0, (view.height - 32.0).max(300.0));
                let x0 = ((view.width - width) / 2.0).max(0.0);
                let y0 = ((view.height - height) / 2.0).max(0.0);
                Rect::new(x0, y0, x0 + width, y0 + height)
            }
            Tab::Workflow => {
                let width = (view.width - 32.0).clamp(240.0, 640.0);
                let height = (TITLE + 3.0 * 56.0 + 60.0 + FOOTER).min(view.height - 32.0);
                let x0 = ((view.width - width) / 2.0).max(0.0);
                let y0 = ((view.height - height) / 2.0).max(0.0);
                Rect::new(x0, y0, x0 + width, y0 + height)
            }
        }
    }

    /// The Apply and Cancel buttons of the tab that shows.
    pub fn buttons(&self, view: Size) -> (Rect, Rect) {
        let panel = self.panel(view);
        let by = panel.y1 - FOOTER + 12.0;
        let apply = Rect::new(panel.x1 - PAD - 96.0, by, panel.x1 - PAD, by + 28.0);
        let cancel = Rect::new(apply.x0 - 8.0 - 96.0, by, apply.x0 - 8.0, by + 28.0);
        (apply, cancel)
    }

    /// The tab strip of the tab that shows (it sits in the title row of that tab panel).
    pub fn tab_strip(&self, view: Size) -> Vec<(Rect, Tab)> {
        self.tab_rects(self.panel(view))
    }

    fn tab_rects(&self, panel: Rect) -> Vec<(Rect, Tab)> {
        if self.prefs.is_none() {
            return Vec::new();
        }
        let mut right = panel.x1 - PAD;
        let mut out = Vec::new();
        for tab in Tab::ALL.into_iter().rev() {
            let w = text_width(tab.label()) + 20.0;
            out.push((
                Rect::new(right - w, panel.y0 + 10.0, right, panel.y0 + 34.0),
                tab,
            ));
            right -= w + 4.0;
        }
        out.reverse();
        out
    }

    /// The edited settings.
    pub fn draft(&self) -> &AudioSettings {
        &self.draft
    }

    /// The sections: host, output device, buffer, MIDI input, workers, linger. Saved values
    /// that are no longer available are still listed (marked) so they stay selectable.
    pub fn sections(&self) -> Vec<Section> {
        let d = &self.draft;
        let mut hosts: Vec<HostChoice> = Vec::new();
        for h in &self.lists.hosts {
            let c = HostChoice::from_kind(h.kind);
            if !hosts.contains(&c) {
                hosts.push(c);
            }
        }
        let mut host_options: Vec<(String, Choice)> = hosts
            .iter()
            .map(|h| (h.label().to_string(), Choice::Host(*h)))
            .collect();
        if !hosts.contains(&d.host) {
            host_options.push((
                format!("{} (not available)", d.host.label()),
                Choice::Host(d.host),
            ));
        }
        let names = self.output_devices(d.host);
        let mut devices = vec![("Host default".to_string(), Choice::Device(None))];
        devices.extend(
            names
                .iter()
                .map(|n| (n.clone(), Choice::Device(Some(n.clone())))),
        );
        if let Some(saved) = d.output_device.as_ref().filter(|n| !names.contains(n)) {
            devices.push((
                format!("{saved} (not found)"),
                Choice::Device(Some(saved.clone())),
            ));
        }
        let inputs = self.input_devices(d.host);
        let mut input_options =
            vec![("Same as the output device".to_string(), Choice::Input(None))];
        input_options.extend(
            inputs
                .iter()
                .map(|n| (n.clone(), Choice::Input(Some(n.clone())))),
        );
        if let Some(saved) = d.input_device.as_ref().filter(|n| !inputs.contains(n)) {
            input_options.push((
                format!("{saved} (not found)"),
                Choice::Input(Some(saved.clone())),
            ));
        }
        let mut midi = vec![("None".to_string(), Choice::Midi(None))];
        midi.extend(
            self.lists
                .midi_ports
                .iter()
                .map(|p| (p.clone(), Choice::Midi(Some(p.clone())))),
        );
        if let Some(saved) = d
            .midi_input
            .as_ref()
            .filter(|p| !self.lists.midi_ports.contains(p))
        {
            midi.push((
                format!("{saved} (not found)"),
                Choice::Midi(Some(saved.clone())),
            ));
        }
        let with_current = |values: &[u64], current: u64| {
            let mut v = values.to_vec();
            if !v.contains(&current) {
                v.push(current);
            }
            v
        };
        let buffers = with_current(&BUFFER_CHOICES.map(u64::from), u64::from(d.buffer_frames));
        let workers = with_current(&WORKER_CHOICES.map(u64::from), u64::from(d.workers));
        let lingers = with_current(&LINGER_CHOICES, d.worker_linger_spins);
        let mut offsets = RECORD_OFFSET_CHOICES.to_vec();
        if !offsets.contains(&d.record_offset_frames) {
            offsets.push(d.record_offset_frames);
            offsets.sort_unstable();
        }
        vec![
            Section {
                title: "Audio host",
                options: host_options,
                chips: true,
            },
            Section {
                title: "Output device",
                options: devices,
                chips: false,
            },
            Section {
                title: "Input device",
                options: input_options,
                chips: false,
            },
            Section {
                title: "Buffer size (frames)",
                options: buffers
                    .iter()
                    .map(|b| (b.to_string(), Choice::Buffer(*b as u32)))
                    .collect(),
                chips: true,
            },
            Section {
                title: "MIDI input",
                options: midi,
                chips: false,
            },
            Section {
                title: "Worker threads",
                options: workers
                    .iter()
                    .map(|w| (w.to_string(), Choice::Workers(*w as u16)))
                    .collect(),
                chips: true,
            },
            Section {
                title: "Worker linger (spins)",
                options: lingers
                    .iter()
                    .map(|l| {
                        let label = if *l == 0 {
                            "Engine default".to_string()
                        } else {
                            l.to_string()
                        };
                        (label, Choice::Linger(*l))
                    })
                    .collect(),
                chips: true,
            },
            Section {
                title: "Record offset (frames)",
                options: offsets
                    .iter()
                    .map(|o| (o.to_string(), Choice::RecordOffset(*o)))
                    .collect(),
                chips: true,
            },
        ]
    }

    fn output_devices(&self, host: HostChoice) -> Vec<String> {
        self.lists
            .hosts
            .iter()
            .filter(|h| HostChoice::from_kind(h.kind) == host)
            .flat_map(|h| h.devices.iter())
            .filter(|dev| dev.max_outputs > 0)
            .map(|dev| dev.name.clone())
            .collect()
    }

    fn input_devices(&self, host: HostChoice) -> Vec<String> {
        self.lists
            .hosts
            .iter()
            .filter(|h| HostChoice::from_kind(h.kind) == host)
            .flat_map(|h| h.devices.iter())
            .filter(|dev| dev.max_inputs > 0)
            .map(|dev| dev.name.clone())
            .collect()
    }

    /// Whether `choice` is the draft's current value.
    pub fn is_selected(&self, choice: &Choice) -> bool {
        let d = &self.draft;
        match choice {
            Choice::Host(h) => d.host == *h,
            Choice::Device(n) => d.output_device == *n,
            Choice::Input(n) => d.input_device == *n,
            Choice::RecordOffset(o) => d.record_offset_frames == *o,
            Choice::Buffer(b) => d.buffer_frames == *b,
            Choice::Midi(p) => d.midi_input == *p,
            Choice::Workers(w) => d.workers == *w,
            Choice::Linger(l) => d.worker_linger_spins == *l,
        }
    }

    fn choose(&mut self, choice: Choice) {
        match choice {
            Choice::Host(h) => {
                let names = self.output_devices(h);
                let inputs = self.input_devices(h);
                let d = &mut self.draft;
                d.host = h;
                if d.output_device.as_ref().is_some_and(|n| !names.contains(n)) {
                    d.output_device = None;
                }
                if d.input_device.as_ref().is_some_and(|n| !inputs.contains(n)) {
                    d.input_device = None;
                }
            }
            Choice::Device(n) => self.draft.output_device = n,
            Choice::Input(n) => self.draft.input_device = n,
            Choice::RecordOffset(o) => self.draft.record_offset_frames = o,
            Choice::Buffer(b) => self.draft.buffer_frames = b,
            Choice::Midi(p) => self.draft.midi_input = p,
            Choice::Workers(w) => self.draft.workers = w,
            Choice::Linger(l) => self.draft.worker_linger_spins = l,
        }
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = vec![
            "Live MIDI notes go to the selected device (else the first device with a notes input)."
                .to_string(),
        ];
        notes.extend(self.lists.input_note.iter().cloned());
        notes.push(
            "Record offset: positive places recordings earlier (added to the measured latency)."
                .to_string(),
        );
        notes.extend(self.lists.errors.iter().cloned());
        notes
    }

    /// Lays the dialog out in a view of `view` logical size.
    pub fn layout(&self, view: Size) -> DialogLayout {
        let notes = self.notes();
        let notes_h = notes.len() as f64 * NOTE_LINE + 6.0;
        let width = (view.width - 32.0).clamp(240.0, 640.0);
        let inner = width - 2.0 * PAD;
        let mut headers = Vec::new();
        let mut options = Vec::new();
        let mut y = 0.0;
        for (si, section) in self.sections().into_iter().enumerate() {
            if si > 0 {
                y += SECTION_GAP;
            }
            headers.push((y, section.title));
            y += HEADER;
            let mut x = 0.0;
            for (oi, (label, choice)) in section.options.into_iter().enumerate() {
                if section.chips {
                    let w = (text_width(&label) + 24.0).min(inner);
                    if x > 0.0 && x + w > inner {
                        x = 0.0;
                        y += ROW + GAP;
                    }
                    options.push((Rect::new(x, y, x + w, y + ROW), label, choice));
                    x += w + GAP;
                } else {
                    if oi > 0 {
                        y += GAP;
                    }
                    options.push((Rect::new(0.0, y, inner, y + ROW), label, choice));
                    y += ROW;
                }
            }
            if section.chips {
                y += ROW;
            }
        }
        let content_height = y;
        let height = (TITLE + content_height + notes_h + FOOTER).min(view.height - 32.0);
        let x0 = ((view.width - width) / 2.0).max(0.0);
        let y0 = ((view.height - height) / 2.0).max(0.0);
        let panel = Rect::new(x0, y0, x0 + width, y0 + height);
        let content = Rect::new(
            x0 + PAD,
            y0 + TITLE,
            x0 + PAD + inner,
            (panel.y1 - FOOTER - notes_h).max(y0 + TITLE),
        );
        let max_scroll = (content_height - content.height()).max(0.0);
        let scroll = self.scroll.clamp(0.0, max_scroll);
        let shift = |r: Rect| r + Vec2::new(content.x0, content.y0 - scroll);
        let by = panel.y1 - FOOTER + 12.0;
        let apply = Rect::new(panel.x1 - PAD - 96.0, by, panel.x1 - PAD, by + 28.0);
        let cancel = Rect::new(apply.x0 - 8.0 - 96.0, by, apply.x0 - 8.0, by + 28.0);
        DialogLayout {
            panel,
            content,
            headers: headers
                .into_iter()
                .map(|(y, t)| (shift(Rect::new(0.0, y, inner, y + HEADER)), t))
                .collect(),
            options: options
                .into_iter()
                .map(|(r, l, c)| (shift(r), l, c))
                .collect(),
            notes: notes
                .into_iter()
                .enumerate()
                .map(|(i, n)| {
                    (
                        Point::new(content.x0, content.y1 + 18.0 + i as f64 * NOTE_LINE),
                        n,
                    )
                })
                .collect(),
            apply,
            cancel,
            tabs: self.tab_rects(panel),
            content_height,
        }
    }

    /// Handles an event (logical window coordinates). The dialog is modal: every event is
    /// consumed.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> DialogOutcome {
        if self.prefs.is_some() {
            if let UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } = event
                && let Some((_, tab)) = self
                    .tab_rects(self.panel(view))
                    .into_iter()
                    .find(|(r, _)| r.contains(*pos))
                && !self.keyboard().is_some_and(KeyboardTab::has_conflict)
            {
                self.tab = tab;
                return DialogOutcome::Open;
            }
            if self.tab != Tab::Audio {
                return self.handle_preferences(event, view);
            }
        }
        match event {
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => return DialogOutcome::Cancel,
                Key::Enter => return DialogOutcome::Apply(self.draft.clone().sanitized()),
                _ => {}
            },
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Wheel { delta, .. } => {
                let l = self.layout(view);
                let max = (l.content_height - l.content.height()).max(0.0);
                self.scroll = (self.scroll - delta.y * SCROLL_STEP).clamp(0.0, max);
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let l = self.layout(view);
                if l.apply.contains(*pos) {
                    return DialogOutcome::Apply(self.draft.clone().sanitized());
                }
                if l.cancel.contains(*pos) {
                    return DialogOutcome::Cancel;
                }
                if l.content.contains(*pos)
                    && let Some((_, _, choice)) =
                        l.options.into_iter().find(|(r, ..)| r.contains(*pos))
                {
                    self.choose(choice);
                }
            }
            _ => {}
        }
        DialogOutcome::Open
    }

    fn handle_preferences(&mut self, event: &UiEvent, view: Size) -> DialogOutcome {
        let panel = self.panel(view);
        let (apply, cancel) = self.buttons(view);
        let blocked = self.keyboard().is_some_and(KeyboardTab::has_conflict);
        if let UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            ..
        } = event
            && !blocked
        {
            if apply.contains(*pos) {
                return DialogOutcome::Apply(self.draft.clone().sanitized());
            }
            if cancel.contains(*pos) {
                return DialogOutcome::Cancel;
            }
        }
        let tab = self.tab;
        let Some(prefs) = &mut self.prefs else {
            return DialogOutcome::Open;
        };
        let out = match tab {
            Tab::Keyboard => prefs.keyboard.handle(event, panel),
            Tab::Workflow => prefs.workflow.handle(event, panel),
            Tab::Audio => TabOutcome::Open,
        };
        match out {
            TabOutcome::Open => DialogOutcome::Open,
            TabOutcome::Apply => DialogOutcome::Apply(self.draft.clone().sanitized()),
            TabOutcome::Cancel => DialogOutcome::Cancel,
            TabOutcome::Export => DialogOutcome::ExportKeymap,
            TabOutcome::Import => DialogOutcome::ImportKeymap,
        }
    }

    /// Paints the dimmed backdrop and the dialog (logical units).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        let l = self.layout(view);
        let panel_rect = self.panel(view);
        canvas.fill_with(
            Rect::from_origin_size(Point::ZERO, view),
            Color::from_rgba8(0, 0, 0, 140),
        );
        let panel = RoundedRect::from_rect(panel_rect, 10.0);
        canvas.fill_with(panel, theme.browser_bg);
        canvas.stroke_with(panel, 1.0, theme.section_line);
        canvas.text(
            &TextRun::new(
                self.tab.title(),
                Point::new(panel_rect.x0 + PAD, panel_rect.y0 + 28.0),
                16.0,
                theme.text,
            )
            .weight(600),
        );
        for (r, tab) in self.tab_rects(panel_rect) {
            let shape = RoundedRect::from_rect(r, 5.0);
            if tab == self.tab {
                canvas.fill_with(shape, theme.selection);
            } else if self.hover.is_some_and(|p| r.contains(p)) {
                canvas.fill_with(shape, theme.browser_row);
            }
            canvas.text(
                &TextRun::new(
                    tab.label(),
                    Point::new(r.center().x, r.y0 + 16.5),
                    13.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
        match (self.tab, &self.prefs) {
            (Tab::Keyboard, Some(p)) => {
                p.keyboard.paint(canvas, theme, panel_rect);
                self.paint_buttons(canvas, theme, panel_rect);
                return;
            }
            (Tab::Workflow, Some(p)) => {
                p.workflow.paint(canvas, theme, panel_rect);
                self.paint_buttons(canvas, theme, panel_rect);
                return;
            }
            _ => {}
        }
        canvas.push_clip(&l.content.into());
        for (r, title) in &l.headers {
            canvas.text(
                &TextRun::new(
                    *title,
                    Point::new(r.x0, r.y0 + 17.0),
                    12.0,
                    theme.text_muted,
                )
                .weight(600),
            );
        }
        for (r, label, choice) in &l.options {
            let selected = self.is_selected(choice);
            let hovered = self
                .hover
                .is_some_and(|p| r.contains(p) && l.content.contains(p));
            let chip = matches!(
                choice,
                Choice::Host(_)
                    | Choice::Buffer(_)
                    | Choice::Workers(_)
                    | Choice::Linger(_)
                    | Choice::RecordOffset(_)
            );
            let shape = RoundedRect::from_rect(*r, 5.0);
            if selected {
                canvas.fill_with(shape, theme.selection);
            } else if hovered {
                canvas.fill_with(shape, theme.browser_row);
            }
            if chip {
                canvas.stroke_with(shape, 1.0, theme.section_line);
                canvas.text(
                    &TextRun::new(
                        label.clone(),
                        Point::new(r.center().x, r.y0 + 16.5),
                        13.0,
                        theme.text,
                    )
                    .align(Align::Center),
                );
            } else {
                let c = Point::new(r.x0 + 12.0, r.center().y);
                canvas.stroke_with(Circle::new(c, 5.0), 1.2, theme.text_muted);
                if selected {
                    canvas.fill_with(Circle::new(c, 2.8), theme.text);
                }
                canvas.text(&TextRun::new(
                    label.clone(),
                    Point::new(r.x0 + 26.0, r.y0 + 16.5),
                    13.0,
                    theme.text,
                ));
            }
        }
        canvas.pop_clip();
        for (p, note) in &l.notes {
            canvas.text(&TextRun::new(note.clone(), *p, 11.5, theme.text_muted));
        }
        for (r, label, primary) in [(l.cancel, "Cancel", false), (l.apply, "Apply", true)] {
            Self::paint_button(canvas, theme, r, label, primary);
        }
    }

    fn paint_buttons(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let by = panel.y1 - FOOTER + 12.0;
        let apply = Rect::new(panel.x1 - PAD - 96.0, by, panel.x1 - PAD, by + 28.0);
        let cancel = Rect::new(apply.x0 - 8.0 - 96.0, by, apply.x0 - 8.0, by + 28.0);
        for (r, label, primary) in [(cancel, "Cancel", false), (apply, "Apply", true)] {
            Self::paint_button(canvas, theme, r, label, primary);
        }
    }

    fn paint_button(canvas: &mut dyn Canvas, theme: &Theme, r: Rect, label: &str, primary: bool) {
        let shape = RoundedRect::from_rect(r, 6.0);
        canvas.fill_with(
            shape,
            if primary {
                theme.focus
            } else {
                theme.browser_row
            },
        );
        canvas.text(
            &TextRun::new(
                label,
                Point::new(r.center().x, r.y0 + 18.5),
                13.0,
                if primary {
                    theme.background
                } else {
                    theme.text
                },
            )
            .align(Align::Center)
            .weight(600),
        );
    }
}

const HEADER: f64 = 24.0;
const ROW: f64 = 24.0;
const GAP: f64 = 6.0;
const SECTION_GAP: f64 = 12.0;
const NOTE_LINE: f64 = 16.0;
const SCROLL_STEP: f64 = 40.0;

#[cfg(test)]
#[path = "settings_dialog_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "settings_tabs_tests.rs"]
mod tab_tests;
