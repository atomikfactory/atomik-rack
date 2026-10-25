//! The command registry: every action reachable by a key or a menu.

use super::context::Context;

/// The group a command is listed under (the areas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// File, settings and other app-wide commands.
    Global,
    /// Playback, recording, locators, tempo.
    Transport,
    /// Panes, zoom, cables.
    View,
    /// Clipboard, selection, undo.
    Edit,
    /// Rack devices.
    Rack,
    /// Arrangement.
    Sequencer,
    /// Clip, note and pattern editors.
    Editor,
    /// Console.
    Mixer,
    /// Device browser.
    Browser,
    /// The file browser panel.
    Files,
    /// Creating devices, tracks and channels.
    Create,
    /// The tool palette.
    Tools,
}

impl Group {
    /// Every group, in listing order.
    pub const ALL: [Group; 12] = [
        Group::Global,
        Group::Transport,
        Group::View,
        Group::Edit,
        Group::Rack,
        Group::Sequencer,
        Group::Editor,
        Group::Mixer,
        Group::Browser,
        Group::Files,
        Group::Create,
        Group::Tools,
    ];

    /// The group title.
    pub fn title(self) -> &'static str {
        match self {
            Group::Global => "Global",
            Group::Transport => "Transport",
            Group::View => "View",
            Group::Edit => "Edit",
            Group::Rack => "Rack",
            Group::Sequencer => "Sequencer",
            Group::Editor => "Editor",
            Group::Mixer => "Mixer",
            Group::Browser => "Browser",
            Group::Files => "Files",
            Group::Create => "Create",
            Group::Tools => "Tools",
        }
    }
}

/// The argument a command can be bound with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// One of a fixed list of names.
    Choice(&'static [&'static str]),
}

impl ArgKind {
    /// Whether `value` is a valid argument.
    pub fn accepts(&self, value: &str) -> bool {
        match self {
            ArgKind::Choice(choices) => choices.contains(&value),
        }
    }
}

/// One command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDef {
    /// Stable dotted id (`edit.duplicate`).
    pub id: &'static str,
    /// Menu text (Atomik's own words).
    pub label: &'static str,
    /// The group it is listed under.
    pub group: Group,
    /// The context it is bound in by default.
    pub context: Context,
    /// The argument it can be bound with.
    pub args: Option<ArgKind>,
    /// Whether holding the key repeats it.
    pub repeatable: bool,
    /// Whether it has a menu item.
    pub menu: bool,
    /// Whether it may decline, so the next binding (lower context, then Global) runs.
    pub can_decline: bool,
}

const fn cmd(id: &'static str, label: &'static str, group: Group, context: Context) -> CommandDef {
    CommandDef {
        id,
        label,
        group,
        context,
        args: None,
        repeatable: false,
        menu: false,
        can_decline: false,
    }
}

impl CommandDef {
    const fn arg(mut self, choices: &'static [&'static str]) -> Self {
        self.args = Some(ArgKind::Choice(choices));
        self
    }
    const fn rep(mut self) -> Self {
        self.repeatable = true;
        self
    }
    const fn menu(mut self) -> Self {
        self.menu = true;
        self
    }
    const fn decline(mut self) -> Self {
        self.can_decline = true;
        self
    }
}

/// The values of `view.cables`.
pub const CABLE_ARGS: &[&str] = &["show", "hover", "hide"];
/// The values of `transport.input_quantize`.
pub const QUANTIZE_ARGS: &[&str] = &["off", "eighth", "sixteenth"];
/// The values of `transport.go_locator`.
pub const LOCATOR_ARGS: &[&str] = &["left", "right"];
/// The values of `sequencer.set_snap`.
pub const SNAP_ARGS: &[&str] = &[
    "off",
    "bar",
    "beat",
    "eighth",
    "sixteenth",
    "thirtysecond",
    "sixtyfourth",
    "quarter_triplet",
    "eighth_triplet",
    "sixteenth_triplet",
    "thirtysecond_triplet",
    "grid",
];
/// The values of `editor.set_scale_root`: the twelve pitch classes, C first.
pub const ROOT_ARGS: &[&str] = &[
    "c", "c_sharp", "d", "d_sharp", "e", "f", "f_sharp", "g", "g_sharp", "a", "a_sharp", "b",
];
/// The values of `editor.set_scale`: the scale kinds (`chromatic` clears the scale).
pub const SCALE_ARGS: &[&str] = &[
    "chromatic",
    "major",
    "natural_minor",
    "harmonic_minor",
    "melodic_minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "locrian",
    "major_pentatonic",
    "minor_pentatonic",
    "blues",
    "whole_tone",
    "phrygian_dominant",
    "hungarian_minor",
];
/// The values of `editor.set_chord`: no chord, the chords and the diatonic chords.
pub const CHORD_ARGS: &[&str] = &[
    "none",
    "major",
    "minor",
    "dim",
    "aug",
    "sus2",
    "sus4",
    "6",
    "m6",
    "7",
    "maj7",
    "m7",
    "m7b5",
    "dim7",
    "9",
    "maj9",
    "m9",
    "add9",
    "11",
    "13",
    "power",
    "octave",
    "diatonic_triad",
    "diatonic_seventh",
    "diatonic_ninth",
];
/// The values of `editor.set_chord_inversion`.
pub const INVERSION_ARGS: &[&str] = &["0", "1", "2", "3"];
/// The values of `editor.fold_set`.
pub const FOLD_ARGS: &[&str] = &["off", "scale", "used"];
/// The values of `editor.set_arp_pattern`.
pub const ARP_PATTERN_ARGS: &[&str] = &["up", "down", "up_down", "down_up", "random", "as_played"];
/// The values of `editor.set_arp_gate`: the gate in percent.
pub const ARP_GATE_ARGS: &[&str] = &["25", "50", "75", "100"];
/// The values of `editor.set_arp_octaves`.
pub const ARP_OCTAVE_ARGS: &[&str] = &["1", "2", "3"];
/// The values of `editor.set_strum_amount`: the note value of the delay per note (`64` is 1/64).
pub const STRUM_AMOUNT_ARGS: &[&str] = &["128", "64", "32", "16"];
/// The values of `editor.set_strum_direction`.
pub const STRUM_DIRECTION_ARGS: &[&str] = &["up", "down"];
/// The values of `sequencer.marker_color`: the palette names and `none`.
pub const MARKER_COLOR_ARGS: &[&str] =
    &["none", "red", "orange", "yellow", "green", "blue", "purple"];
/// The values of `sequencer.track_height`: the height presets.
pub const TRACK_HEIGHT_ARGS: &[&str] = &["small", "medium", "large", "fit"];
/// The values of `sequencer.fold_lanes`.
pub const LANE_FOLD_ARGS: &[&str] = &["fold", "unfold"];
/// The values of `sequencer.audio_fade_shape`.
pub const FADE_SHAPE_ARGS: &[&str] = &["linear", "equal_power", "exponential", "s_curve"];
/// The values of `sequencer.audio_crossfade`.
pub const CROSSFADE_ARGS: &[&str] = &["off", "linear", "equal_power", "exponential", "s_curve"];
/// The values of `sequencer.audio_gain`.
pub const AUDIO_GAIN_ARGS: &[&str] = &["up", "down", "reset"];
/// The values of `sequencer.audio_transpose`.
pub const AUDIO_TRANSPOSE_ARGS: &[&str] = &["up", "down", "octave_up", "octave_down", "reset"];
/// The values of `sequencer.audio_warp_mode`.
pub const AUDIO_WARP_MODE_ARGS: &[&str] = &["beats", "tonal", "repitch"];
/// The values of `sequencer.audio_warp_bars`: the bars the selected loop lasts.
pub const AUDIO_WARP_BARS_ARGS: &[&str] = &["1", "2", "4", "8", "16"];
/// The values of `sequencer.audio_fades`.
pub const AUDIO_FADES_ARGS: &[&str] = &["declick", "clear"];
/// The values of `sequencer.meter_insert` and `sequencer.meter_set`.
pub const METER_ARGS: &[&str] = &["2/4", "3/4", "4/4", "5/4", "6/8", "7/8", "12/8"];

use Context as C;
use Group as G;

/// Every command, sorted by id.
const COMMANDS: &[CommandDef] = &[
    cmd("audio.plugins", "Plugins", G::Global, C::Global).menu(),
    cmd("audio.restart", "Restart Audio", G::Global, C::Global).menu(),
    cmd("audio.settings", "Settings", G::Global, C::Global).menu(),
    cmd(
        "browser.cancel",
        "Cancel Browser Search",
        G::Browser,
        C::Browser,
    ),
    cmd(
        "browser.create",
        "Create Selected Device",
        G::Browser,
        C::Browser,
    ),
    cmd("browser.move_down", "Browser Down", G::Browser, C::Browser).rep(),
    cmd("browser.move_up", "Browser Up", G::Browser, C::Browser).rep(),
    cmd(
        "create.audio_track",
        "Create Audio Track",
        G::Create,
        C::Global,
    )
    .menu(),
    cmd("create.effect", "Create Effect", G::Create, C::Global).menu(),
    cmd(
        "create.instrument",
        "Create Instrument",
        G::Create,
        C::Global,
    )
    .menu(),
    cmd(
        "create.mixer_channel",
        "Create Mixer Channel",
        G::Create,
        C::Global,
    )
    .menu(),
    cmd("edit.copy", "Copy", G::Edit, C::Global)
        .decline()
        .menu(),
    cmd("edit.cut", "Cut", G::Edit, C::Global).decline().menu(),
    cmd("edit.delete", "Delete", G::Edit, C::Global)
        .decline()
        .menu(),
    cmd("edit.duplicate", "Duplicate", G::Edit, C::Global)
        .decline()
        .menu(),
    cmd("edit.paste", "Paste", G::Edit, C::Global)
        .decline()
        .menu(),
    cmd(
        "edit.quantize_options",
        "Quantize Options",
        G::Edit,
        C::Global,
    )
    .menu(),
    cmd("edit.redo", "Redo", G::Edit, C::Global).rep().menu(),
    cmd("edit.select_all", "Select All", G::Edit, C::Global)
        .decline()
        .menu(),
    cmd("edit.undo", "Undo", G::Edit, C::Global).rep().menu(),
    cmd(
        "editor.arpeggiate",
        "Arpeggiate Selection",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "editor.chordify",
        "Chordify Selection",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "editor.conform_to_scale",
        "Conform Selection to Scale",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "editor.detect_scale",
        "Detect Scale from Notes",
        G::Editor,
        C::Editor,
    ),
    cmd("editor.fold_set", "Set Fold", G::Editor, C::Editor).arg(FOLD_ARGS),
    cmd("editor.fold_toggle", "Fold", G::Editor, C::Editor),
    cmd("editor.nudge_left", "Nudge Left", G::Editor, C::Editor).rep(),
    cmd(
        "editor.nudge_left_beat",
        "Nudge Left by Beat",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd(
        "editor.nudge_left_tick",
        "Nudge Left by Tick",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd("editor.nudge_right", "Nudge Right", G::Editor, C::Editor).rep(),
    cmd(
        "editor.nudge_right_beat",
        "Nudge Right by Beat",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd(
        "editor.nudge_right_tick",
        "Nudge Right by Tick",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd(
        "editor.octave_down",
        "Transpose Octave Down",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd(
        "editor.octave_up",
        "Transpose Octave Up",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd("editor.quantize", "Quantize", G::Editor, C::Editor).menu(),
    cmd(
        "editor.scale_snap_toggle",
        "Snap to Scale",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "editor.set_arp_gate",
        "Set Arpeggio Gate",
        G::Editor,
        C::Editor,
    )
    .arg(ARP_GATE_ARGS),
    cmd(
        "editor.set_arp_octaves",
        "Set Arpeggio Octaves",
        G::Editor,
        C::Editor,
    )
    .arg(ARP_OCTAVE_ARGS),
    cmd(
        "editor.set_arp_pattern",
        "Set Arpeggio Pattern",
        G::Editor,
        C::Editor,
    )
    .arg(ARP_PATTERN_ARGS),
    cmd("editor.set_chord", "Set Chord", G::Editor, C::Editor).arg(CHORD_ARGS),
    cmd(
        "editor.set_chord_inversion",
        "Set Chord Inversion",
        G::Editor,
        C::Editor,
    )
    .arg(INVERSION_ARGS),
    cmd("editor.set_scale", "Set Scale", G::Editor, C::Editor).arg(SCALE_ARGS),
    cmd(
        "editor.set_scale_root",
        "Set Scale Root",
        G::Editor,
        C::Editor,
    )
    .arg(ROOT_ARGS),
    cmd(
        "editor.set_strum_amount",
        "Set Strum Amount",
        G::Editor,
        C::Editor,
    )
    .arg(STRUM_AMOUNT_ARGS),
    cmd(
        "editor.set_strum_direction",
        "Set Strum Direction",
        G::Editor,
        C::Editor,
    )
    .arg(STRUM_DIRECTION_ARGS),
    cmd("editor.strum", "Strum Selection", G::Editor, C::Editor),
    cmd(
        "editor.toggle_note_mute",
        "Mute Notes",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "editor.transpose_down",
        "Transpose Down",
        G::Editor,
        C::Editor,
    )
    .rep(),
    cmd("editor.transpose_up", "Transpose Up", G::Editor, C::Editor).rep(),
    cmd("editor.zoom_keys_in", "Taller Keys", G::Editor, C::Editor).rep(),
    cmd("editor.zoom_keys_out", "Shorter Keys", G::Editor, C::Editor).rep(),
    cmd(
        "editor.zoom_keys_reset",
        "Reset Key Height",
        G::Editor,
        C::Editor,
    ),
    cmd(
        "file.export_midi_clip",
        "Export MIDI Clip",
        G::Global,
        C::Global,
    )
    .menu(),
    cmd(
        "file.export_midi_song",
        "Export MIDI Song",
        G::Global,
        C::Global,
    )
    .menu(),
    cmd(
        "file.export_midi_track",
        "Export MIDI Track",
        G::Global,
        C::Global,
    )
    .menu(),
    cmd("file.export_patch", "Export Patch", G::Global, C::Global).menu(),
    cmd("file.import_audio", "Import Audio", G::Global, C::Global).menu(),
    cmd("file.import_midi", "Import MIDI", G::Global, C::Global).menu(),
    cmd(
        "file.import_midi_song",
        "Import MIDI as Song",
        G::Global,
        C::Global,
    )
    .menu(),
    cmd("file.new", "New", G::Global, C::Global).menu(),
    cmd("file.open", "Open", G::Global, C::Global).menu(),
    cmd("file.quit", "Quit", G::Global, C::Global).menu(),
    cmd(
        "file.relink_audio",
        "Relink Missing Audio",
        G::Global,
        C::Global,
    )
    .menu(),
    cmd("file.save", "Save", G::Global, C::Global).menu(),
    cmd("file.save_as", "Save As", G::Global, C::Global).menu(),
    cmd("file.stress_rack", "Stress Rack", G::Global, C::Global).menu(),
    cmd("file.stress_song", "Stress Song", G::Global, C::Global).menu(),
    cmd("files.down", "Next Item", G::Files, C::Files).rep(),
    cmd("files.end", "Last Item", G::Files, C::Files),
    cmd("files.favorite", "Toggle Favourite", G::Files, C::Files),
    cmd("files.home", "First Item", G::Files, C::Files),
    cmd("files.left", "Collapse", G::Files, C::Files).rep(),
    cmd("files.open", "Open", G::Files, C::Files),
    cmd("files.page_down", "Next Page", G::Files, C::Files).rep(),
    cmd("files.page_up", "Previous Page", G::Files, C::Files).rep(),
    cmd("files.parent", "Parent Folder", G::Files, C::Files),
    cmd("files.right", "Expand", G::Files, C::Files).rep(),
    cmd("files.search", "Search Files", G::Files, C::Files),
    cmd("files.select_all", "Select All Files", G::Files, C::Files),
    cmd("files.stop", "Stop Preview", G::Files, C::Files),
    cmd(
        "files.toggle_preview",
        "Play / Stop Preview",
        G::Files,
        C::Files,
    ),
    cmd("files.up", "Previous Item", G::Files, C::Files).rep(),
    cmd("mixer.add_channel", "Add Channel", G::Mixer, C::Global).menu(),
    cmd("mixer.add_to_vca", "Add to VCA", G::Mixer, C::Global).menu(),
    cmd(
        "mixer.bypass_all_inserts",
        "Bypass All Inserts",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd("mixer.compare_snapshot", "Compare A/B", G::Mixer, C::Global).menu(),
    cmd(
        "mixer.create_vca",
        "Create VCA from Selection",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.delete_snapshot",
        "Delete Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd("mixer.delete_vca", "Delete VCA", G::Mixer, C::Global).menu(),
    cmd("mixer.end_compare", "End Compare", G::Mixer, C::Global).menu(),
    cmd(
        "mixer.recall_next_snapshot",
        "Recall Next Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.recall_previous_snapshot",
        "Recall Previous Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.recall_snapshot",
        "Recall Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.remove_from_vca",
        "Remove from VCA",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.route_to_new_bus",
        "Route to New Output Bus",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.scroll_end",
        "Scroll to Last Channel",
        G::Mixer,
        C::Mixer,
    ),
    cmd(
        "mixer.scroll_home",
        "Scroll to First Channel",
        G::Mixer,
        C::Mixer,
    ),
    cmd(
        "mixer.scroll_left",
        "Scroll Channels Left",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.scroll_right",
        "Scroll Channels Right",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.store_snapshot",
        "Store Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.toggle_compensation",
        "Delay Compensation",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.toggle_polarity",
        "Toggle Polarity",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.update_snapshot",
        "Update Snapshot",
        G::Mixer,
        C::Global,
    )
    .menu(),
    cmd(
        "mixer.value_down",
        "Lower Focused Control",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_down_fine",
        "Lower Focused Control (Fine)",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_page_down",
        "Lower Focused Control by Ten",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_page_down_fine",
        "Lower Focused Control by Ten (Fine)",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_page_up",
        "Raise Focused Control by Ten",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_page_up_fine",
        "Raise Focused Control by Ten (Fine)",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_up",
        "Raise Focused Control",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd(
        "mixer.value_up_fine",
        "Raise Focused Control (Fine)",
        G::Mixer,
        C::Mixer,
    )
    .rep(),
    cmd("rack.add_column", "Add Column", G::Rack, C::Rack),
    cmd("rack.assign_send", "Use as Send Effect", G::Rack, C::Rack),
    cmd("rack.auto_route", "Auto-route", G::Rack, C::Rack),
    cmd("rack.disconnect", "Disconnect", G::Rack, C::Rack),
    cmd(
        "rack.extend_next",
        "Extend Selection Down",
        G::Rack,
        C::Rack,
    )
    .rep(),
    cmd("rack.extend_prev", "Extend Selection Up", G::Rack, C::Rack).rep(),
    cmd("rack.go_to_track", "Go to Track", G::Rack, C::Rack),
    cmd("rack.remove_column", "Remove Column", G::Rack, C::Rack),
    cmd("rack.select_next", "Select Next Device", G::Rack, C::Rack).rep(),
    cmd(
        "rack.select_prev",
        "Select Previous Device",
        G::Rack,
        C::Rack,
    )
    .rep(),
    cmd(
        "sequencer.audio_crossfade",
        "Audio Crossfade",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(CROSSFADE_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_envelope_clear",
        "Clear Gain Envelope",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_fade_shape",
        "Audio Fade Shape",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(FADE_SHAPE_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_fades",
        "Audio Fades",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(AUDIO_FADES_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_gain",
        "Audio Clip Gain",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(AUDIO_GAIN_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_normalise",
        "Normalise Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_quantize",
        "Quantize Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_reverse",
        "Reverse Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_strip_silence",
        "Strip Silence",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_to_notes",
        "Audio to Notes",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_transpose",
        "Transpose Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(AUDIO_TRANSPOSE_ARGS)
    .menu()
    .decline()
    .rep(),
    cmd(
        "sequencer.audio_warp",
        "Warp Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_warp_bars",
        "Warp Loop as Bars",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(AUDIO_WARP_BARS_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_warp_mode",
        "Warp Mode",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(AUDIO_WARP_MODE_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_warp_panel",
        "Warp Settings",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.audio_warp_tempo",
        "Warp to Song Tempo",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.bounce_in_place",
        "Bounce in Place",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.bounce_in_place_mute",
        "Bounce in Place and Mute Originals",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.fold_lanes",
        "Fold Automation Lanes",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(LANE_FOLD_ARGS)
    .menu()
    .decline(),
    cmd(
        "sequencer.follow",
        "Follow Playhead",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd(
        "sequencer.freeze_cancel",
        "Cancel Freeze",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.freeze_tail",
        "Freeze with Tail",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.freeze_track",
        "Freeze Track",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.freeze_track_to_device",
        "Freeze Track up to Selected Device",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.group_tracks",
        "Group Tracks",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd("sequencer.join", "Join", G::Sequencer, C::Sequencer),
    cmd(
        "sequencer.marker_add_here",
        "Add Marker Here",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.marker_color",
        "Marker Colour",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(MARKER_COLOR_ARGS)
    .decline(),
    cmd(
        "sequencer.marker_delete",
        "Delete Marker",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.marker_loop_range",
        "Loop This Range",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.marker_range_from_selection",
        "Add Range Marker from Selection",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.marker_rename",
        "Rename Marker...",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.marker_select_clips",
        "Select Clips in Range",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.meter_delete",
        "Delete Meter Change",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.meter_insert",
        "Insert Meter Change",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(METER_ARGS)
    .decline(),
    cmd(
        "sequencer.meter_set",
        "Set Meter",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(METER_ARGS)
    .decline(),
    cmd(
        "sequencer.move_track_down",
        "Move Track Down",
        G::Sequencer,
        C::Sequencer,
    )
    .rep()
    .menu()
    .decline(),
    cmd(
        "sequencer.move_track_up",
        "Move Track Up",
        G::Sequencer,
        C::Sequencer,
    )
    .rep()
    .menu()
    .decline(),
    cmd(
        "sequencer.open_clip",
        "Open Clip",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.rename_folder",
        "Rename Folder...",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.select_next_clip",
        "Select Next Clip",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd(
        "sequencer.select_next_track",
        "Select Next Track",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd(
        "sequencer.select_prev_clip",
        "Select Previous Clip",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd(
        "sequencer.select_prev_track",
        "Select Previous Track",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd("sequencer.set_snap", "Set Snap", G::Sequencer, C::Sequencer).arg(SNAP_ARGS),
    cmd(
        "sequencer.snap_toggle",
        "Snap On/Off",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd(
        "sequencer.split_at_playhead",
        "Split at Playhead",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd(
        "sequencer.tempo_add_point",
        "Add Tempo Point",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.tempo_delete_point",
        "Delete Tempo Point",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.tempo_set_value",
        "Set Tempo...",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.tempo_toggle_ramp",
        "Ramp to Next",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.toggle_chase",
        "Chase Notes",
        G::Sequencer,
        C::Sequencer,
    )
    .decline(),
    cmd(
        "sequencer.toggle_folder",
        "Collapse or Expand Folder",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.toggle_mute",
        "Mute Clips",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd(
        "sequencer.track_color",
        "Track Colour",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(MARKER_COLOR_ARGS)
    .decline(),
    cmd(
        "sequencer.track_comment",
        "Track Comment...",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.track_height",
        "Track Height",
        G::Sequencer,
        C::Sequencer,
    )
    .arg(TRACK_HEIGHT_ARGS)
    .menu(),
    cmd(
        "sequencer.unfreeze_track",
        "Unfreeze Track",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.unfreeze_track_keep",
        "Unfreeze Track and Keep Audio",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.ungroup_tracks",
        "Ungroup",
        G::Sequencer,
        C::Sequencer,
    )
    .menu()
    .decline(),
    cmd(
        "sequencer.zoom_selection",
        "Zoom to Selection",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd(
        "sequencer.zoom_tracks_in",
        "Taller Tracks",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd(
        "sequencer.zoom_tracks_out",
        "Shorter Tracks",
        G::Sequencer,
        C::Sequencer,
    )
    .rep(),
    cmd(
        "sequencer.zoom_tracks_reset",
        "Reset Track Heights",
        G::Sequencer,
        C::Sequencer,
    ),
    cmd("tool.eraser", "Eraser Tool", G::Tools, C::Sequencer),
    cmd("tool.pencil", "Pencil Tool", G::Tools, C::Sequencer),
    cmd("tool.razor", "Razor Tool", G::Tools, C::Sequencer),
    cmd("tool.select", "Select Tool", G::Tools, C::Sequencer),
    cmd(
        "transport.add_marker",
        "Add Marker",
        G::Transport,
        C::Global,
    )
    .menu(),
    cmd(
        "transport.add_range_from_loop",
        "Add Range Marker from Loop",
        G::Transport,
        C::Global,
    )
    .menu(),
    cmd(
        "transport.bar_back",
        "Back One Bar",
        G::Transport,
        C::Global,
    )
    .rep(),
    cmd(
        "transport.bar_forward",
        "Forward One Bar",
        G::Transport,
        C::Global,
    )
    .rep(),
    cmd(
        "transport.forward_bar",
        "Fast Forward One Bar",
        G::Transport,
        C::Global,
    )
    .rep(),
    cmd(
        "transport.go_locator",
        "Go to Locator",
        G::Transport,
        C::Global,
    )
    .arg(LOCATOR_ARGS),
    cmd(
        "transport.go_start",
        "Return to Zero",
        G::Transport,
        C::Global,
    )
    .menu(),
    cmd(
        "transport.input_quantize",
        "Input Quantize",
        G::Transport,
        C::Global,
    )
    .arg(QUANTIZE_ARGS)
    .menu(),
    cmd("transport.loop", "Loop", G::Transport, C::Global).menu(),
    cmd(
        "transport.loop_and_play",
        "Loop Selection and Play",
        G::Transport,
        C::Sequencer,
    ),
    cmd(
        "transport.loop_to_selection",
        "Loop to Selection",
        G::Transport,
        C::Sequencer,
    ),
    cmd("transport.metronome", "Metronome", G::Transport, C::Global).menu(),
    cmd(
        "transport.next_marker",
        "Next Marker",
        G::Transport,
        C::Global,
    )
    .rep()
    .menu(),
    cmd("transport.play", "Play", G::Transport, C::Global),
    cmd(
        "transport.play_stop",
        "Play / Stop",
        G::Transport,
        C::Global,
    )
    .menu(),
    cmd(
        "transport.prev_marker",
        "Previous Marker",
        G::Transport,
        C::Global,
    )
    .rep()
    .menu(),
    cmd("transport.record", "Record", G::Transport, C::Global).menu(),
    cmd(
        "transport.rewind_bar",
        "Rewind One Bar",
        G::Transport,
        C::Global,
    )
    .rep(),
    cmd("transport.stop", "Stop", G::Transport, C::Global),
    cmd(
        "transport.tempo_down",
        "Tempo Down",
        G::Transport,
        C::Global,
    )
    .rep(),
    cmd("transport.tempo_up", "Tempo Up", G::Transport, C::Global).rep(),
    cmd("view.cables", "Cables", G::View, C::Global)
        .arg(CABLE_ARGS)
        .menu(),
    cmd(
        "view.cables_cycle",
        "Cycle Cable Display",
        G::View,
        C::Global,
    ),
    cmd(
        "view.focus_browser_search",
        "Focus Browser Search",
        G::View,
        C::Global,
    ),
    cmd(
        "view.maximise_console",
        "Maximise Console",
        G::View,
        C::Global,
    )
    .menu(),
    cmd("view.sequencer_only", "Sequencer Only", G::View, C::Global).menu(),
    cmd("view.toggle_browser", "Device Browser", G::View, C::Global).menu(),
    cmd("view.toggle_console", "Console", G::View, C::Global).menu(),
    cmd(
        "view.toggle_console_devices",
        "Show Console Devices in Rack",
        G::View,
        C::Global,
    )
    .menu(),
    cmd(
        "view.toggle_console_links",
        "Show Console Links",
        G::View,
        C::Global,
    )
    .menu(),
    cmd("view.toggle_editor", "Clip Editor", G::View, C::Global).menu(),
    cmd(
        "view.toggle_file_browser",
        "File Browser",
        G::View,
        C::Global,
    )
    .menu(),
    cmd("view.toggle_flip", "Back of Rack", G::View, C::Global).menu(),
    cmd("view.toggle_marker_list", "Marker List", G::View, C::Global).menu(),
    cmd(
        "view.toggle_marker_strip",
        "Marker Strip",
        G::View,
        C::Global,
    )
    .menu(),
    cmd("view.toggle_piano_roll", "Piano Roll", G::View, C::Global).menu(),
    cmd("view.toggle_rack", "Rack", G::View, C::Global).menu(),
    cmd(
        "view.toggle_rack_navigator",
        "Rack Navigator",
        G::View,
        C::Global,
    )
    .menu(),
    cmd(
        "view.toggle_sample_editor",
        "Sample Editor",
        G::View,
        C::Global,
    )
    .menu(),
    cmd(
        "view.toggle_slice_editor",
        "Slice Editor",
        G::View,
        C::Global,
    )
    .menu(),
    cmd("view.toggle_song", "Arrangement", G::View, C::Global).menu(),
    cmd("view.toggle_step_editor", "Step Editor", G::View, C::Global).menu(),
    cmd("view.toggle_tempo_lane", "Tempo Lane", G::View, C::Global).menu(),
    cmd("view.zoom_in", "Zoom In", G::View, C::Global)
        .rep()
        .menu(),
    cmd("view.zoom_out", "Zoom Out", G::View, C::Global)
        .rep()
        .menu(),
    cmd("view.zoom_reset", "Actual Size", G::View, C::Global).menu(),
];

/// The command registry.
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    defs: &'static [CommandDef],
}

impl Registry {
    /// Every command of the application.
    pub fn bundled() -> Registry {
        Registry { defs: COMMANDS }
    }

    /// All commands, sorted by id.
    pub fn commands(&self) -> &'static [CommandDef] {
        self.defs
    }

    /// The command with this id.
    pub fn get(&self, id: &str) -> Option<&'static CommandDef> {
        self.defs
            .binary_search_by(|d| d.id.cmp(id))
            .ok()
            .map(|i| &self.defs[i])
    }

    /// Whether `arg` is valid for command `id`: no argument for a command without args, one
    /// of the choices for a command with. Unknown ids are not valid.
    pub fn validate(&self, id: &str, arg: Option<&str>) -> bool {
        match (self.get(id), arg) {
            (Some(d), None) => d.args.is_none(),
            (Some(d), Some(a)) => d.args.is_some_and(|k| k.accepts(a)),
            (None, _) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_sorted_and_dotted() {
        let defs = Registry::bundled().commands();
        assert!(defs.len() > 80);
        for w in defs.windows(2) {
            assert!(w[0].id < w[1].id, "{} before {}", w[0].id, w[1].id);
        }
        for d in defs {
            let (group, name) = d.id.split_once('.').unwrap();
            assert!(
                !group.is_empty()
                    && !name.is_empty()
                    && d.id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "{}",
                d.id
            );
        }
    }

    #[test]
    fn every_command_has_a_label_and_a_group_prefix_that_matches() {
        for d in Registry::bundled().commands() {
            assert!(!d.label.trim().is_empty(), "{}", d.id);
            let prefix = d.id.split('.').next().unwrap();
            let expected: &[&str] = match d.group {
                Group::Global => &["file", "audio"],
                Group::Transport => &["transport"],
                Group::View => &["view"],
                Group::Edit => &["edit"],
                Group::Rack => &["rack"],
                Group::Sequencer => &["sequencer"],
                Group::Editor => &["editor"],
                Group::Mixer => &["mixer"],
                Group::Browser => &["browser"],
                Group::Files => &["files"],
                Group::Create => &["create"],
                Group::Tools => &["tool"],
            };
            assert!(expected.contains(&prefix), "{} in {:?}", d.id, d.group);
        }
    }

    #[test]
    fn lookup_and_argument_validation() {
        let r = Registry::bundled();
        assert_eq!(r.get("edit.duplicate").unwrap().label, "Duplicate");
        assert!(r.get("edit.nope").is_none());
        assert!(r.validate("edit.undo", None));
        assert!(!r.validate("edit.undo", Some("x")));
        assert!(r.validate("view.cables", Some("hover")));
        assert!(!r.validate("view.cables", Some("sideways")));
        assert!(
            !r.validate("view.cables", None),
            "a command with args needs one"
        );
        assert!(r.validate("transport.go_locator", Some("left")));
        assert!(!r.validate("no.such", None));
    }

    #[test]
    fn decline_and_repeat_flags_follow_the_plan() {
        let r = Registry::bundled();
        assert!(r.get("sequencer.open_clip").unwrap().can_decline);
        assert!(!r.get("transport.play_stop").unwrap().repeatable);
        assert!(r.get("edit.undo").unwrap().repeatable);
        assert!(r.get("view.zoom_in").unwrap().repeatable);
        assert!(!r.get("file.save").unwrap().repeatable);
    }

    #[test]
    fn group_titles_cover_every_group() {
        assert_eq!(Group::ALL.len(), 12);
        for g in Group::ALL {
            assert!(!g.title().is_empty());
        }
    }
}
