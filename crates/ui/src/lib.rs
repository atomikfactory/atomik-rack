//! Atomik Rack UI toolkit.
//!
//! A platform-independent rack UI: a retained view tree, widgets, generated panels, cables
//! and a procedural vector theme, drawn through the [`Canvas`] trait. There is no window-system
//! dependency and no time source: the app feeds [`UiEvent`]s and applies the returned
//! [`UiAction`]s.
//!
//! # Hosting contract (for the app shell)
//!
//! ```text
//! let mut view = RackView::new(Theme::dark());
//! view.set_layout_provider(atomik_devices::panel_layout);       // optional hand-made panels
//! view.handle(&UiEvent::ScaleChanged(window.scale_factor()), doc, &registry);
//! view.handle(&UiEvent::Resize(logical_size), doc, &registry);
//! view.sync(document.doc(), &registry);
//!
//! // For each window event:
//! for action in view.handle(&translated_event, document.doc(), &registry) {
//!     match action {
//!         UiAction::Command(c) => match document.apply(c, &registry) {
//!             Ok(ops) => engine_sync(ops),
//!             Err(e) => view.notify_rejected(e.to_string()),
//!         },
//!         UiAction::EndMerge => document.end_merge(),
//!         UiAction::Undo => { document.undo(&registry); }
//!         UiAction::Redo => { document.redo(&registry); }
//!         UiAction::Select(_) | UiAction::OpenBrowser => {}  // menus, status bar…
//!         _ => {}  // `UiAction` is non-exhaustive; see the sequencer views below
//!     }
//! }
//! view.sync(document.doc(), &registry);
//! if view.needs_repaint() { window.request_redraw(); }
//!
//! // On redraw:
//! scene.reset();
//! let mut canvas = SceneCanvas::new(&mut scene, &mut text_engine).with_cache(&mut scene_cache);
//! view.paint(&mut canvas, document.doc(), &registry);
//! scene_cache.trim();
//! renderer.render_to_texture(.., &scene, .., RenderParams { width, height, .. });
//! ```
//!
//! - **Units.** Events are in *logical* pixels (physical / scale factor). [`RackView::paint`]
//!   emits *physical* pixels: it applies the scale factor itself, so the app renders the scene
//!   with an identity transform at [`RackView::physical_size`]. Forward winit
//!   `ScaleFactorChanged` as [`UiEvent::ScaleChanged`] and `Resized` (converted to logical)
//!   as [`UiEvent::Resize`].
//! - **Event translation (winit):** `CursorMoved` → `PointerMove` (logical position, current
//!   modifiers); `MouseInput` → `PointerDown`/`PointerUp` with a click count the app computes
//!   (same button within ~400 ms and 4 px → 2); `MouseWheel` → `Wheel` in notches
//!   (`LineDelta` as is, `PixelDelta` / 40, positive y = away from the user); `CursorLeft` →
//!   `PointerLeave`; `KeyboardInput` → `Key` (`Key::Character(s)` → [`Key::Char`], named keys
//!   to the matching [`Key`] variant, the numpad by its physical code to [`Key::Numpad`],
//!   everything else [`Key::Other`]; releases are delivered for every key); `ModifiersChanged`
//!   updates the modifiers the app attaches to events and is also sent as
//!   [`UiEvent::ModifiersChanged`]. Views keep the lenient [`Modifiers::command`] (Ctrl or Cmd)
//!   for pointer gestures; keyboard chords are matched strictly by the [`keymap`] router
//!   (command registry, keymap profiles, contexts), which the app runs before it
//!   forwards a key to a view.
//! - **Shortcuts handled by the view:** Ctrl/Cmd+Z undo, Ctrl/Cmd+Shift+Z and Ctrl/Cmd+Y redo,
//!   Esc cancels a drag (a knob drag restores its start value), Tab/Shift+Tab move focus
//!   between controls, arrows/PageUp/PageDown step the focused control, Space/Enter press a
//!   focused button or selector, Delete/Backspace remove the selected device, Home/End scroll.
//! - **Rack interactions:** drag a knob or fader vertically ([`widget`] documents the
//!   numbers); double-click resets; wheel steps; Ctrl/Cmd+wheel zooms (0.5×–2×) around the
//!   pointer; wheel elsewhere scrolls (Shift = horizontal); middle-drag pans; click a name plate
//!   to select, drag it to reorder; the chevron folds a device to a 0.5 U strip; the BYPASS
//!   button of a built-in effect's name strip toggles its `bypass` param and veils the panel
//!   body while on ([`autolayout::has_strip_bypass`]; plugins have their own BYPASS in the
//!   panel); on the back,
//!   drag between jacks to patch (dragging a connected input detaches its cable first;
//!   dropping on an occupied input replaces its cable); double-click empty rack space opens
//!   the browser, where typing filters and double-click/Enter adds a device below the
//!   selection.
//! - **The document is the single source of truth.** The view never mutates it; after
//!   applying commands call [`RackView::sync`] (also called by `handle` and `paint`).
//! - **Interface strip.** The top strip (key [`atomik_model::INTERFACE_KEY`]) shows
//!   `out_1..out_N` (from `RackDoc::output_channels`) on its back, matching the engine's
//!   interface sockets.
//! - **Caching.** Each device panel is recorded into a [`DisplayList`] stamped with
//!   `(id, revision)`; the revision changes only when that device's data changes. The vello
//!   backend keeps one encoded [`vello::Scene`] per list in a [`SceneCache`] and appends it.
//!   Cables, highlights, tooltips and the browser are a per-frame overlay.
//!
//! # Sequencer views
//!
//! [`ArrangementView`], [`PianoRollView`] and [`TransportBar`] share the same contract:
//! `new(theme)`, `set_bounds(rect)` (a sub-rectangle of the window, logical pixels; events use
//! the same window-logical coordinates), `sync`, `handle` → [`UiAction`]s, `paint` (scaled by
//! the scale factor like [`RackView::paint`], drawing only inside the bounds), `needs_repaint`,
//! `set_scale` / [`UiEvent::ScaleChanged`]. `Resize` is ignored by them: the app lays out
//! the window (rack on top, then the transport bar, then the arrangement or the piano roll)
//! and calls `set_bounds`.
//!
//! - **Routing.** Send a pointer event to the view whose bounds contain it, except while a
//!   view `is_dragging()`: keep sending it every pointer event until the button is released.
//!   Send key events to the focused view (the last one clicked; call `set_focused`). While
//!   [`TransportBar::is_editing_tempo`] or [`TransportBar::is_editing_meter`] is true, send
//!   keys to the transport bar.
//! - **Actions.** `Command` / `EndMerge` / `Undo` / `Redo` as for the rack.
//!   [`UiAction::OpenPianoRoll`]: call [`PianoRollView::open`] and show it in place of the
//!   arrangement; [`UiAction::ClosePianoRoll`]: show the arrangement again.
//!   [`UiAction::Transport`]: map [`TransportAction`] to the engine transport
//!   (`ToggleLoop` = apply `SongCommand::SetLoop` with the current range and the flag
//!   inverted). [`UiAction::Audition`]: send note on/off to the device live.
//! - **Time.** The app passes the transport position in (`set_playhead`,
//!   [`TransportBar::set_state`]) every frame while playing; nothing here reads a clock.
//! - **Merge ids.** Arrangement drags use ids from 2⁴⁰, piano-roll drags from 2·2⁴⁰, so they
//!   never collide with the rack view's knob drags.
//!
//! # Console view
//!
//! [`ConsoleView`] follows the same contract as the sequencer views (`set_bounds`, `sync`,
//! `handle`, `paint`, `is_dragging`, `set_focused`). It shares the lower pane with the
//! arrangement: show one or the other, swap on [`UiAction::ToggleConsole`] (F5, see
//! [`console_shortcut`]) and maximise / restore on [`UiAction::MaximiseConsole`] (Shift+F5).
//! Tell the view the pane state with [`ConsoleView::set_maximised`] (lights the MAX button).
//! Each frame the app passes `set_meters` (engine readings, linear amplitude) then
//! `set_frame_time` (monotonic seconds); after a routing change, `set_latency_readouts` and
//! `set_pdc_warning`. Console edits arrive as `Command::Console(..)`. The rack shows console
//! devices unless [`RackView::set_show_console_devices`] is off (on by default), draws
//! the channel to master links only while [`RackView::set_show_console_links`] is on (a channel
//! then reads "Output: Master", see [`RackView::output_display`]) and locks the Master Section
//! while it is the first device ([`RackView::is_locked`]).
//!
//! # Sample editor
//!
//! [`SampleEditorView`] follows the same contract (`set_bounds`, `sync`, `handle`, `paint`,
//! `is_dragging`, `set_focused`) and shares the lower pane with the arrangement.
//! [`UiAction::OpenSampleEditor`] (the Edit button or a double-click on a sampler panel):
//! call [`SampleEditorView::open`] and show it; [`UiAction::CloseSampleEditor`] (or
//! [`SampleEditorView::is_present`] turning false): show the arrangement again. Feed it the
//! asset peaks ([`SampleEditorView::set_peaks`]) and the assets whose audio is missing
//! ([`SampleEditorView::set_missing_assets`]). Edits arrive as `Command::SetZones` (drags
//! with merge ids from 4·2⁴⁰), previews as [`UiAction::AuditionVelocity`], drops as
//! [`UiAction::ImportSamples`] (also emitted by [`RackView`] for a drop on a sampler panel;
//! route `FileHover` and `FileDrop` to the view under the pointer and `FileHoverCancel` to
//! both; the editor uses window-logical coordinates, the rack its own, as for the other views).
//!
//! # Step editor and drum panel
//!
//! [`StepEditorView`] follows the same contract (`set_bounds`, `sync`, `handle`, `paint`,
//! `is_dragging`, `set_focused`) and shares the lower pane with the arrangement.
//! [`UiAction::OpenStepEditor`] (the drum panel's Edit button or a double-click on its pad
//! grid background; its `clip` is `None` from the rack, so resolve it with
//! [`StepEditorView::resolve_clip`]): call [`StepEditorView::open`] and show it;
//! [`UiAction::CloseStepEditor`] (or [`StepEditorView::is_present`] turning false): show the
//! arrangement again. [`UiAction::CreateStepClip`] asks the app to create the one-bar clip with a
//! `16 x 1/16` grid in one undo step and open it. Edits arrive as `Command::Song`
//! (`SetClipContent`, `SetClipGrid`) and `Command::Batch` (grid and length), drags with merge ids
//! from 5·2⁴⁰, previews as [`UiAction::AuditionVelocity`], drops as [`UiAction::ImportSamples`]
//! (`at_key: Some(pad key)` over a row header or a pad of the rack panel, `None` elsewhere; the
//! app interprets `at_key` per device type). The drum panel is part of [`RackView`]; the selected
//! pad is view state ([`RackView::selected_pad`]).
//!
//! # Slice editor and slicer panel
//!
//! [`SliceEditorView`] follows the same contract (`set_bounds`, `sync`, `handle`, `paint`,
//! `is_dragging`, `set_focused`, `set_peaks`, `set_missing_assets`) and shares the lower pane
//! with the arrangement. [`UiAction::OpenSliceEditor`] (the slicer panel's Edit button or a
//! double-click on its overview strip): call [`SliceEditorView::open`] and show it;
//! [`UiAction::CloseSliceEditor`] (or [`SliceEditorView::is_present`] turning false): show the
//! arrangement again. Marker, field and `loop.beats` edits arrive as `Command::SetZones` /
//! `Command::SetParam` (drags with merge ids from 6·2⁴⁰), previews as
//! [`UiAction::AuditionVelocity`], drops as [`UiAction::ImportSamples`] with `at_key: None`.
//! The UI has no samples: [`UiAction::SliceLoop`] ([`SliceMode`]) asks the app to compute the
//! markers (onset detection or `grid_markers`) and apply one `SetZones`, and
//! [`UiAction::SlicesToTrack`] asks it to build the clip. The sensitivity is view state
//! ([`RackView::slice_sensitivity`], [`SliceEditorView::sensitivity`]).
//!
//! # Players: step sequencer and arpeggiator
//!
//! Both devices have hand-made panels in [`RackView`] ([`seq_panel`], [`arp_panel`]); the
//! sequencer also has the grid editor [`SeqEditorView`], which follows the same contract
//! (`set_bounds`, `sync`, `handle`, `paint`, `is_dragging`, `set_focused`) and shares the lower
//! pane with the arrangement. [`UiAction::OpenSeqEditor`] (the panel's Edit button or a
//! double-click on its preview): call [`SeqEditorView::open`], then
//! [`SeqEditorView::set_pattern_index`] with [`RackView::edited_pattern`], and show it;
//! [`UiAction::CloseSeqEditor`] (or [`SeqEditorView::is_present`] turning false): show the
//! arrangement again. [`UiAction::SelectPattern`] reports the edited pattern (view state): mirror
//! it into the other view. Every edit is a `Command::SetPattern` (drags with merge ids from
//! 6·2⁴⁰), `AddPattern`, `DuplicatePattern`, `RemovePattern` or a `SetParam`; copy, paste and
//! clear are `SetPattern`s, the pattern clipboard is view state ([`RackView::pattern_clipboard`],
//! [`SeqEditorView::clipboard`]). Previews are [`UiAction::AuditionVelocity`]. Each frame the app
//! passes the device's published meter values to [`RackView::set_device_meter_values`] (and
//! [`SeqEditorView::set_meter_values`]): sequencer `[playing pattern, step, running]`,
//! arpeggiator `[held keys, next 8 keys]`; panels repaint only when a displayed value changes. The
//! browser lists the two players together as the "Players" group
//! ([`browser::PLAYER_TYPE_IDS`]).
//!
//! # Containers
//!
//! A container is a device with a rack inside. [`RackView`] shows the inside of one when its
//! **scope** is set ([`RackView::set_scope`]; a double-click on a container panel or its OPEN
//! button emits [`UiAction::EnterContainer`] and has already switched the scope): the inner
//! devices and cables with the two boundary panels (INPUT at the top, OUTPUT at the bottom) and
//! a breadcrumb bar ([`breadcrumb`]; the Back button, a segment, Esc or Backspace with nothing
//! selected emit [`UiAction::ExitContainer`] and have already gone up). Inside a scope every
//! command and action carries **path keys** (`layer/synth`; the boundary is `layer/boundary`):
//! `AddDevice` becomes `ContainerCommand::AddInner`, the browser offers the allowed types, and a
//! right-click on a control offers "Map to macro" (`ContainerCommand::SetMacro`). A scope whose
//! container disappears is left for the nearest one that exists (reported as `ExitContainer` by
//! the next event). Pass path keys to [`RackView::set_device_meter_values`] and the other
//! per-device accessors; they accept the local key too.
//!
//! [`ContainerEditorView`] (the panel's EDIT button, or "Edit range" in the macro menu, both
//! [`UiAction::OpenContainerEditor`]) follows the editor contract (`set_bounds`, `sync`,
//! `handle`, `paint`, `is_dragging`, `set_focused`) and shares the lower pane with the
//! arrangement: call [`ContainerEditorView::open`] (then [`ContainerEditorView::set_tab`] and
//! [`ContainerEditorView::select_macro`] for `macros: Some(i)`), show it, and hide it on
//! [`UiAction::CloseContainerEditor`] or when [`ContainerEditorView::is_present`] turns false.
//! Edits are `Command::Container` (`SetSplits`, `SetMacro`, `RemoveMacroTarget`,
//! `RenameMacro`) and `SetParam` of the container's `macroN` (drags with merge ids from
//! 7·2⁴⁰); the audition is [`UiAction::AuditionVelocity`] on the container, which the app plays
//! through its splits.
//!
//! # Third-party plugins
//!
//! The plugin views take **plain data** from the app ([`plugin_data`]); this crate depends on
//! neither the plugin host nor a backend.
//!
//! - **Rack panel.** A plugin device has a hand-made 2 U panel in [`RackView`] ([`plugin_panel`]):
//!   pinned knobs, POWER, BYPASS, AUDIO THR, OPEN GUI, PARAMS, TO CH, RELOAD and RELINK. Tell the
//!   view what the app knows about each instance with [`RackView::set_plugin_status`] (editor,
//!   MISSING, FAILED, loading) and feed the plugin's meter values to
//!   [`RackView::set_device_meter_values`]. The panel emits `PluginCommand`s,
//!   `SetParam`s and [`UiAction::OpenPluginGui`], [`UiAction::OpenPluginParams`] and
//!   [`UiAction::ReloadPlugin`] (path keys).
//! - **Browser.** [`RackView::set_plugin_catalog`] fills the browser's Plugins group and the
//!   RELINK menu; adding emits `PluginCommand::Add`.
//! - **Parameter pane.** [`PluginParamsView`] follows the editor contract (`open`,
//!   `set_bounds`, `sync`, `handle`, `paint`, `is_present`) and shares the lower pane with
//!   the arrangement: show it on [`UiAction::OpenPluginParams`], hide it on
//!   [`UiAction::ClosePluginParams`]. It asks for display texts ([`UiAction::PluginParamTexts`],
//!   answered with [`PluginParamsView::set_texts`]); slider drags use merge ids from
//!   `PLUGIN_GESTURE_MERGE_BASE`. [`PluginGestureMerger`] turns the gestures made in the plugin's
//!   own window into the same merged commands.
//! - **Settings and banner.** [`PluginSettingsView`] (the Plugins section of the settings dialog,
//!   emitting [`UiAction::PluginSettings`]) and [`PluginBannerView`] (the project and unclean-exit
//!   banner, emitting [`UiAction::PluginBanner`]) take their data from the app and show the
//!   native-code warning before anything is enabled.
//! - **Relink.** [`RelinkView`] lists the audio files a project lost and emits
//!   [`UiAction::Relink`] (`LOCATE...` per file, `SEARCH FOLDER...`, `CLOSE`); the app feeds it with
//!   [`RelinkView::set_missing`].
//! - **Quantize.** [`QuantizeView`] is the modal panel of grid, strength, swing, groove and humanize
//!   settings and the groove library; it emits [`UiAction::Quantize`] (`APPLY`, `EXTRACT GROOVE FROM
//!   CLIP`, `DELETE`, `CLOSE`) and the app feeds it with [`QuantizeView::set_target`] and
//!   [`QuantizeView::set_grooves`].
//! - **Strip silence.** [`SilenceView`] is the modal panel of threshold, minimum silence, padding
//!   and fade settings; it emits [`UiAction::StripSilence`] (`APPLY`, `CLOSE`) and the app feeds it
//!   with [`SilenceView::set_target`].
//!
//! # Backends
//! - [`SceneCanvas`]: vello GPU scene (text shaped with parley, system fonts via fontique).
//! - [`DisplayList`]: deterministic recorder; [`DisplayList::to_json`] feeds the snapshot tests.
//! - A `vello_cpu` canvas for PNG previews and screenshots is [`cpu_canvas::CpuCanvas`] (feature
//!   `cpu-canvas`).

pub mod action;
pub mod arp_panel;
pub mod arrangement;
pub mod audio;
pub mod autolayout;
pub mod breadcrumb;
pub mod browser;
pub mod cables;
pub mod canvas;
pub mod character_panel;
pub mod character_screen;
pub mod chord_edit;
pub mod console;
pub mod console_layout;
pub mod console_meter;
pub mod container_edit;
pub mod container_editor;
pub mod container_panel;
pub mod context_menu;
pub mod controller_lane;
#[cfg(feature = "cpu-canvas")]
pub mod cpu_canvas;
pub mod display_list;
pub mod event;
pub mod file_browser;
pub mod fx;
pub mod fx_layout;
pub mod fx_paint;
pub mod interface;
pub mod io_panel;
pub mod key_rows;
pub mod keymap;
pub mod marker_edit;
pub mod master_panel;
pub mod modsrc_panel;
pub mod paint;
pub mod piano_roll;
pub mod plugin_banner;
pub mod plugin_data;
pub mod plugin_panel;
pub mod plugin_params;
pub mod plugin_settings;
pub mod quantize_panel;
pub mod rack;
pub mod relink_panel;
pub mod sample_editor;
pub mod scene_canvas;
pub mod seq_edit;
pub mod seq_editor;
pub mod seq_paint;
pub mod seq_panel;
pub mod sidechain;
pub mod silence_panel;
pub mod slice_editor;
pub mod step_editor;
pub mod tempo_lane;
pub mod text;
pub mod theme;
pub mod timeline;
pub mod transport;
pub mod tree;
pub mod viewport;
pub mod wave_paint;
pub mod widget;
pub mod zone_edit;

pub use action::{
    BrowseAction, ContextTarget, ImportTarget, Place, SliceMode, TransportAction, UiAction,
};
pub use arrangement::{
    ArrangementView, AudioHandle, LaneButton, MARKER_LIST_WIDTH, MARKER_STRIP_HEIGHT, TrackButton,
};
pub use canvas::{Align, Canvas, CanvasExt, Geom, TextRun};
pub use console::{ConsoleButton, ConsoleView, console_shortcut, fader_db, is_console_device};
pub use console_meter::ChannelMeters;
pub use container_editor::{ContainerEditorView, ContainerTab};
pub use display_list::{DisplayList, Op};
pub use event::{Key, Modifiers, NumpadKey, PointerButton, UiEvent};
pub use file_browser::{FileBrowserView, PlacesData, TreeChild};
pub use io_panel::IoSocketState;
pub use marker_edit::MarkSel;
pub use piano_roll::PianoRollView;
pub use plugin_banner::{BannerKind, PluginBannerView};
pub use plugin_params::{PluginGestureMerger, PluginParamsView};
pub use plugin_settings::PluginSettingsView;
pub use quantize_panel::{GrooveEntry, QuantizeAction, QuantizeRow, QuantizeValues, QuantizeView};
pub use rack::{
    COLUMN_GAP, CableVisibility, Flip, LayoutProvider, NAVIGATOR_WIDTH, RackView, SocketHighlight,
};
pub use relink_panel::{RelinkAction, RelinkEntry, RelinkView};
pub use sample_editor::SampleEditorView;
pub use scene_canvas::{SceneCache, SceneCanvas};
pub use seq_editor::SeqEditorView;
pub use seq_paint::SeqTheme;
pub use seq_panel::SeqButton;
pub use silence_panel::{SilenceAction, SilenceRow, SilenceValues, SilenceView};
pub use slice_editor::SliceEditorView;
pub use step_editor::{StepEditorView, VelocityLevel};
pub use text::{ShapedGlyph, ShapedRun, TextEngine};
pub use theme::Theme;
pub use transport::{TransportBar, TransportButton, TransportState, transport_shortcut};
pub use vello;
pub use vello::{kurbo, peniko};
pub use viewport::{Tool, ViewportState};
