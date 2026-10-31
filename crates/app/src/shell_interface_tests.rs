//! The interface preferences in a headless `Shell`: the light theme and the System choice applied
//! live and saved, the application zoom and what a shell does with it, the knob range and the
//! value tooltips, the Help menu (the shortcut list on F1, the links, the About box) and the
//! Window menu. Defaults leave the project and the sound alone.

use super::*;
use crate::about_box::{ISSUES_URL, REPOSITORY_URL};
use crate::commands::ZoomChange;
use crate::keymap_store::{WORKFLOW_FILE, WorkflowPrefs, load_workflow};
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::keymap::script::event_for;
use atomik_ui::widget::KnobRange;
use atomik_ui::{DisplayList, Modifiers, PointerButton, Theme, ThemeChoice};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-ifc-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_in(dir: Option<&Path>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: dir.map(|d| d.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_in(None)
}

fn painted(s: &mut Shell<NullBackend>) -> String {
    let mut dl = DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    dl.to_json()
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) -> Vec<ShellRequest> {
    let (key, modifiers) = event_for(Os::Windows, chord).unwrap();
    let r = s.handle(&UiEvent::Key {
        key: key.clone(),
        modifiers,
        pressed: true,
    });
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: false,
    });
    r
}

fn add_synth(s: &mut Shell<NullBackend>, k: &str) -> DeviceKey {
    let key = DeviceKey(k.into());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key.clone()),
        })
        .unwrap();
    key
}

fn moved(p: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

fn down(p: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn up(p: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

/// The window point over the control `param` of `key` in the main window.
fn control_point(s: &Shell<NullBackend>, key: &DeviceKey, param: &str) -> Point {
    let p = s.core().view().control_center(key, param).unwrap();
    let r = s.layout().rack;
    Point::new(r.x0 + p.x, r.y0 + p.y)
}

fn prefs_with(s: &mut Shell<NullBackend>, change: impl FnOnce(&mut WorkflowPrefs)) {
    let mut w = s.workflow().clone();
    change(&mut w);
    let user = s.user_keymap().clone();
    s.apply_preferences(Some((user, w)));
}

// ------------------------------------------------------------------ the theme

#[test]
fn a_new_shell_is_dark_and_the_defaults_are_the_old_look() {
    let s = shell();
    assert_eq!(s.workflow().theme, ThemeChoice::Dark);
    assert_eq!(s.workflow().app_zoom, 100);
    assert_eq!(s.theme(), Theme::dark());
    assert_eq!(s.core().view().theme(), &Theme::dark());
    assert_eq!(s.app_zoom(), 1.0);
    assert_eq!(s.ui_scale(2.0), 2.0);
    let state = s.menu_state();
    assert_eq!((state.theme, state.app_zoom), (ThemeChoice::Dark, 100));
}

#[test]
fn switching_the_theme_repaints_with_the_other_colours_and_leaves_no_stale_ones() {
    let mut s = shell();
    let dark = painted(&mut s);
    s.execute(AppCommand::SetTheme(ThemeChoice::Light));
    assert_eq!(s.core().view().theme(), &Theme::light());
    let light = painted(&mut s);
    assert_ne!(dark, light);
    // A shell that was light from the start paints exactly what the switched one paints.
    let mut fresh = shell();
    prefs_with(&mut fresh, |w| w.theme = ThemeChoice::Light);
    assert_eq!(painted(&mut fresh), light);
    // And back: the dark paint comes back exactly.
    s.execute(AppCommand::SetTheme(ThemeChoice::Dark));
    assert_eq!(painted(&mut s), dark);
}

#[test]
fn the_system_choice_follows_the_operating_system_and_a_fixed_one_does_not() {
    let mut s = shell();
    s.execute(AppCommand::SetTheme(ThemeChoice::System));
    assert_eq!(
        s.theme(),
        Theme::dark(),
        "a dark system is assumed at first"
    );
    s.set_system_dark(false);
    assert_eq!(s.core().view().theme(), &Theme::light());
    s.set_system_dark(true);
    assert_eq!(s.core().view().theme(), &Theme::dark());
    s.execute(AppCommand::SetTheme(ThemeChoice::Dark));
    s.set_system_dark(false);
    assert_eq!(
        s.core().view().theme(),
        &Theme::dark(),
        "a fixed choice ignores the system"
    );
}

#[test]
fn the_theme_applies_to_the_panels_that_are_open_and_is_saved() {
    let dir = temp_dir("theme-saved");
    {
        let mut s = shell_in(Some(&dir));
        s.execute(AppCommand::MidiMappings);
        let before = {
            let mut dl = DisplayList::new();
            s.midi_maps_view().unwrap().paint(&mut dl);
            dl.to_json()
        };
        s.execute(AppCommand::SetTheme(ThemeChoice::Light));
        let after = {
            let mut dl = DisplayList::new();
            s.midi_maps_view().unwrap().paint(&mut dl);
            dl.to_json()
        };
        assert_ne!(before, after, "an open panel switches with the window");
    }
    assert_eq!(
        load_workflow(&dir.join(WORKFLOW_FILE)).value.theme,
        ThemeChoice::Light
    );
    // The next start is light.
    let s = shell_in(Some(&dir));
    assert_eq!(s.core().view().theme(), &Theme::light());
}

#[test]
fn the_interface_switches_never_touch_the_project() {
    let mut s = shell();
    add_synth(&mut s, "syn");
    let doc = s.core().doc().clone();
    let dirty = s.core().is_dirty();
    for c in [
        AppCommand::SetTheme(ThemeChoice::Light),
        AppCommand::SetTheme(ThemeChoice::System),
        AppCommand::AppZoom(ZoomChange::To(150)),
        AppCommand::AppZoom(ZoomChange::Reset),
    ] {
        s.execute(c);
    }
    prefs_with(&mut s, |w| {
        w.knob_range = KnobRange::VeryPrecise;
        w.value_tooltips = true;
    });
    assert_eq!(s.core().doc(), &doc);
    assert_eq!(s.core().is_dirty(), dirty);
}

// ------------------------------------------------------------------ the zoom

#[test]
fn the_zoom_commands_step_set_and_reset_the_percentage() {
    let mut s = shell();
    s.execute(AppCommand::AppZoom(ZoomChange::In));
    assert_eq!(s.workflow().app_zoom, 125);
    assert_eq!(s.app_zoom(), 1.25);
    s.execute(AppCommand::AppZoom(ZoomChange::To(200)));
    s.execute(AppCommand::AppZoom(ZoomChange::In));
    assert_eq!(s.workflow().app_zoom, 200, "200 % is the largest");
    s.execute(AppCommand::AppZoom(ZoomChange::To(75)));
    s.execute(AppCommand::AppZoom(ZoomChange::Out));
    assert_eq!(s.workflow().app_zoom, 75, "75 % is the smallest");
    s.execute(AppCommand::AppZoom(ZoomChange::Reset));
    assert_eq!(s.workflow().app_zoom, 100);
    assert_eq!(s.menu_state().app_zoom, 100);
}

#[test]
fn the_zoom_is_saved_and_the_next_start_uses_it() {
    let dir = temp_dir("zoom-saved");
    {
        let mut s = shell_in(Some(&dir));
        s.execute(AppCommand::AppZoom(ZoomChange::To(150)));
    }
    assert_eq!(load_workflow(&dir.join(WORKFLOW_FILE)).value.app_zoom, 150);
    let s = shell_in(Some(&dir));
    assert_eq!(s.app_zoom(), 1.5);
    assert_eq!(s.ui_scale(2.0), 3.0, "the display scale times the zoom");
}

/// What the platform does: the window gets the display scale times the zoom as its scale and
/// the physical size divided by it as its logical size.
fn open_like_the_platform(s: &mut Shell<NullBackend>, os_scale: f64, physical: (f64, f64)) {
    let scale = s.ui_scale(os_scale);
    s.handle(&UiEvent::ScaleChanged(scale));
    s.handle(&UiEvent::Resize(Size::new(
        physical.0 / scale,
        physical.1 / scale,
    )));
}

#[test]
fn at_150_percent_the_window_lays_out_in_smaller_logical_units_and_pointers_land_on_the_same_things()
 {
    // A 1800 x 1350 pixel window at 150 % is 1200 x 900 logical units: it lays out exactly as a
    // window of 1200 x 900 pixels does at 100 %, drawn 1.5 times larger.
    let mut plain = shell();
    open_like_the_platform(&mut plain, 1.0, (1200.0, 900.0));
    let key = add_synth(&mut plain, "syn");
    let mut zoomed = shell();
    zoomed.execute(AppCommand::AppZoom(ZoomChange::To(150)));
    open_like_the_platform(&mut zoomed, 1.0, (1800.0, 1350.0));
    add_synth(&mut zoomed, "syn");
    assert_eq!(zoomed.size(), Size::new(1200.0, 900.0));
    assert_eq!(plain.size(), Size::new(1200.0, 900.0));
    assert_eq!(zoomed.scale(), 1.5);
    assert_eq!(
        zoomed.physical_size_in(WindowKind::Main),
        Some((1800, 1350))
    );
    assert_eq!(plain.physical_size_in(WindowKind::Main), Some((1200, 900)));
    assert_eq!(zoomed.layout(), plain.layout());
    // The display scale and the zoom multiply: a 2x display at 150 % is 3x.
    let mut hidpi = shell();
    hidpi.execute(AppCommand::AppZoom(ZoomChange::To(150)));
    open_like_the_platform(&mut hidpi, 2.0, (3600.0, 2700.0));
    assert_eq!(hidpi.scale(), 3.0);
    assert_eq!(hidpi.size(), Size::new(1200.0, 900.0));
    // A pointer at physical (px, py) arrives at (px / scale, py / scale): the click on the
    // synth's panel selects it at both zooms.
    for (s, scale) in [(&mut plain, 1.0), (&mut zoomed, 1.5)] {
        let r = s.core().view().device_rect(&key).unwrap();
        let in_view = s
            .core()
            .view()
            .rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
        let rack = s.layout().rack;
        let logical = Point::new(rack.x0 + in_view.x, rack.y0 + in_view.y);
        // What the platform would compute from the physical pixel under the pointer.
        let physical_px = (logical.x * scale, logical.y * scale);
        let at = Point::new(physical_px.0 / scale, physical_px.1 / scale);
        s.handle(&moved(at));
        s.handle(&down(at));
        s.handle(&up(at));
        assert_eq!(
            s.core().view().primary(),
            Some(key.clone()),
            "scale {scale}"
        );
    }
}

// ------------------------------------------------------------------ knobs and tooltips

fn mix_after_dragging(range: KnobRange) -> f32 {
    let mut s = shell();
    let key = add_synth(&mut s, "syn");
    prefs_with(&mut s, |w| w.knob_range = range);
    let at = control_point(&s, &key, "osc.mix");
    s.handle(&moved(at));
    s.handle(&down(at));
    s.handle(&moved(Point::new(at.x, at.y - 50.0)));
    s.handle(&up(Point::new(at.x, at.y - 50.0)));
    s.core().doc().device(&key).unwrap().params["osc.mix"]
}

#[test]
fn the_knob_range_setting_reaches_the_rack_drag() {
    let normal = mix_after_dragging(KnobRange::Normal);
    let precise = mix_after_dragging(KnobRange::Precise);
    let very = mix_after_dragging(KnobRange::VeryPrecise);
    let start = {
        let mut s = shell();
        let key = add_synth(&mut s, "syn");
        let registry = crate::app_core::default_registry();
        let device = s.core().doc().device(&key).unwrap().clone();
        let desc = registry.get(&device.type_id).unwrap().descriptor;
        atomik_ui::widget::current_value(&device, desc.param_by_key("osc.mix").unwrap())
    };
    let moved_by = |v: f32| (v - start).abs();
    assert!(moved_by(normal) > 0.0);
    assert!(
        (moved_by(precise) * 2.0 - moved_by(normal)).abs() < 1e-3,
        "{normal} {precise}"
    );
    assert!(
        (moved_by(very) * 4.0 - moved_by(normal)).abs() < 1e-3,
        "{normal} {very}"
    );
}

#[test]
fn the_value_tooltips_setting_shows_a_plain_controls_value_on_hover() {
    let mut s = shell();
    let key = add_synth(&mut s, "syn");
    let at = control_point(&s, &key, "osc.mix");
    s.handle(&moved(at));
    assert_eq!(s.core().view().tooltip(), None);
    prefs_with(&mut s, |w| w.value_tooltips = true);
    assert!(
        s.core()
            .view()
            .tooltip()
            .is_some_and(|t| t.starts_with("Mix"))
    );
    prefs_with(&mut s, |w| w.value_tooltips = false);
    assert_eq!(s.core().view().tooltip(), None);
}

// ------------------------------------------------------------------ the Help menu

fn help_items(s: &Shell<NullBackend>) -> Vec<(String, Option<String>)> {
    let model = s.menu_model();
    let help = model.iter().find(|m| m.title == "Help").unwrap();
    help.items()
        .map(|i| (i.label.to_string(), i.shortcut.clone()))
        .collect()
}

#[test]
fn the_help_menu_lists_its_items_and_the_shortcut_list_shows_f1() {
    let s = shell();
    let items = help_items(&s);
    let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Keyboard Shortcuts",
            "Documentation",
            "Report an Issue",
            "Export Diagnostics\u{2026}",
            "About Atomik Rack"
        ]
    );
    assert_eq!(items[0].1.as_deref(), Some("F1"));
}

#[test]
fn f1_and_the_menu_item_open_the_shortcut_list() {
    let mut s = shell();
    assert!(!s.dialog_open());
    tap(&mut s, "F1");
    assert!(s.dialog_open());
    assert_eq!(
        s.dialog().unwrap().tab(),
        crate::settings_dialog::Tab::Keyboard
    );
    // Escape closes it, and the menu item opens it again.
    tap(&mut s, "Escape");
    assert!(!s.dialog_open());
    s.execute(AppCommand::KeyboardShortcuts);
    assert_eq!(
        s.dialog().unwrap().tab(),
        crate::settings_dialog::Tab::Keyboard
    );
}

#[test]
fn the_documentation_and_the_issue_links_are_the_projects_own_addresses() {
    let mut s = shell();
    assert_eq!(
        s.execute(AppCommand::Documentation),
        [ShellRequest::OpenUrl(REPOSITORY_URL)]
    );
    assert_eq!(
        s.execute(AppCommand::ReportIssue),
        [ShellRequest::OpenUrl(ISSUES_URL)]
    );
    for url in [REPOSITORY_URL, ISSUES_URL] {
        assert!(url.starts_with("https://github.com/atomikfactory/atomik-rack"));
    }
}

#[test]
fn the_about_box_opens_says_the_version_and_the_licence_and_closes() {
    let mut s = shell();
    s.execute(AppCommand::About);
    assert!(s.modal_open());
    let text = painted(&mut s);
    assert!(
        text.contains(&format!("Version {}", crate::about_box::VERSION)),
        "the version of this build"
    );
    assert!(text.contains("GPL-3.0-or-later"), "the licence");
    assert!(text.contains("ABSOLUTELY NO WARRANTY"));
    // F1 does not open anything behind the box; Escape closes it.
    tap(&mut s, "F1");
    assert!(!s.dialog_open());
    tap(&mut s, "Escape");
    assert!(!s.modal_open());
}

#[test]
fn the_about_box_link_opens_the_repository_and_a_click_outside_closes_it() {
    let mut s = shell();
    s.execute(AppCommand::About);
    let size = s.size();
    let link = AboutBox::new("1.2.3").link(size).center();
    let requests = s.handle(&down(link));
    assert_eq!(requests, [ShellRequest::OpenUrl(REPOSITORY_URL)]);
    assert!(s.modal_open(), "the box stays open after the link");
    s.handle(&down(Point::new(2.0, 2.0)));
    assert!(!s.modal_open());
}

// ------------------------------------------------------------------ the Window menu

#[test]
fn the_window_menu_brings_the_main_window_forward_and_a_detached_pane_when_there_is_one() {
    let mut s = shell();
    assert_eq!(
        s.execute(AppCommand::FocusWindow(WindowKind::Main)),
        [ShellRequest::FocusWindow(WindowKind::Main)]
    );
    // The rack is in the main window: there is nothing to bring forward, and it says so.
    assert!(
        s.execute(AppCommand::FocusWindow(WindowKind::Rack))
            .is_empty()
    );
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("not open")),
        "{:?}",
        s.core().last_rejection()
    );
    s.execute(AppCommand::DetachRack);
    assert_eq!(
        s.execute(AppCommand::FocusWindow(WindowKind::Rack)),
        [ShellRequest::FocusWindow(WindowKind::Rack)]
    );
    let model = s.menu_model();
    let window = model.iter().find(|m| m.title == "Window").unwrap();
    let enabled: Vec<bool> = window.items().map(|i| i.enabled).collect();
    assert_eq!(enabled, [true, true, false]);
}

#[test]
fn every_new_command_is_reachable_by_its_registry_id() {
    for (id, arg, command) in [
        (
            "help.keyboard_shortcuts",
            None,
            AppCommand::KeyboardShortcuts,
        ),
        ("help.documentation", None, AppCommand::Documentation),
        ("help.report_issue", None, AppCommand::ReportIssue),
        ("help.about", None, AppCommand::About),
        (
            "view.theme",
            Some("light"),
            AppCommand::SetTheme(ThemeChoice::Light),
        ),
        (
            "view.app_zoom",
            Some("175"),
            AppCommand::AppZoom(ZoomChange::To(175)),
        ),
        (
            "view.focus_window",
            Some("main"),
            AppCommand::FocusWindow(WindowKind::Main),
        ),
    ] {
        assert_eq!(AppCommand::from_command(id, arg), Some(command), "{id}");
    }
    let mut s = shell();
    let handled = s.run_command("view.theme", Some("light"));
    assert!(handled.is_some());
    assert_eq!(s.core().view().theme(), &Theme::light());
    s.run_command("view.app_zoom", Some("150"));
    assert_eq!(s.workflow().app_zoom, 150);
}
