//! The shell after a contained panic of the UI thread: it is still usable, a recovery copy is
//! asked for, the notice is up once, and the paused state paints the notice alone with SAVE
//! within reach. Headless (null audio backend).

use super::*;
use crate::app_core::{PluginConfig, PluginPaths};
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{DisplayList, Modifiers, PointerButton};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-fault-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell(recovery: &Path) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.configure_plugins(PluginConfig {
        paths: PluginPaths {
            recovery: Some(recovery.to_path_buf()),
            ..PluginPaths::default()
        },
        ..PluginConfig::default()
    });
    let mut s = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 760.0)));
    s
}

fn edit(s: &mut Shell<NullBackend>, key: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".to_string(),
            key: Some(DeviceKey(key.to_string())),
        })
        .unwrap();
}

fn snapshots(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".recover.atomik"))
                .count()
        })
        .unwrap_or(0)
}

fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

fn painted(s: &mut Shell<NullBackend>) -> Vec<String> {
    let mut list = DisplayList::new();
    s.paint_in(WindowKind::Main, &mut list);
    list.texts().into_iter().map(str::to_owned).collect()
}

fn texts_after(s: &mut Shell<NullBackend>) -> Vec<String> {
    painted(s)
}

fn centre(r: atomik_ui::kurbo::Rect) -> Point {
    Point::new((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
}

#[test]
fn a_contained_panic_asks_for_a_recovery_copy_and_raises_the_notice() {
    let dir = temp("notice");
    let mut s = shell(&dir);
    edit(&mut s, "a");
    s.core_mut().wait_recovery_writes();
    assert_eq!(snapshots(&dir), 0, "nothing written before the panic");
    assert!(!s.banner_view().is_visible());

    s.after_ui_panic(true, false);

    assert_eq!(
        s.banner_view().kind(),
        Some(&BannerKind::InternalError { halted: false })
    );
    s.core_mut().plugin_tick(1.0);
    s.core_mut().wait_recovery_writes();
    assert_eq!(
        snapshots(&dir),
        1,
        "the recovery copy was written at the next tick"
    );
    assert!(!s.ui_halted());
}

#[test]
fn the_shell_is_usable_after_a_panic_and_a_second_one_does_not_stack_notices() {
    let dir = temp("usable");
    let mut s = shell(&dir);
    s.after_ui_panic(true, false);
    let height = s.banner_view().height();
    // The platform says the notice is up already.
    s.after_ui_panic(false, false);
    assert_eq!(s.banner_view().height(), height, "one strip, not two");
    assert_eq!(
        s.banner_view().kind(),
        Some(&BannerKind::InternalError { halted: false })
    );
    // Events, commands and paints still work.
    s.handle(&UiEvent::PointerMove {
        pos: Point::new(300.0, 300.0),
        modifiers: Modifiers::NONE,
    });
    edit(&mut s, "b");
    assert!(s.core().is_dirty());
    assert!(painted(&mut s).len() > 5, "the content still paints");
}

#[test]
fn dismiss_hides_the_notice_and_save_asks_to_save() {
    let dir = temp("buttons");
    let mut s = shell(&dir);
    s.after_ui_panic(true, false);
    let save = s.banner_view().save_rect().expect("SAVE is offered");
    let dismiss = s.banner_view().dismiss_rect().expect("DISMISS is offered");

    // A project with no file asks where to save it; the notice stays up.
    let requests = click(&mut s, centre(save));
    assert!(
        requests.contains(&ShellRequest::PickSaveFile),
        "{requests:?}"
    );
    assert!(s.banner_view().is_visible());

    click(&mut s, centre(dismiss));
    assert!(!s.banner_view().is_visible());
}

#[test]
fn repeated_panics_pause_the_content_and_leave_save_within_reach() {
    let dir = temp("halted");
    let mut s = shell(&dir);
    s.after_ui_panic(true, false);
    s.after_ui_panic(true, true);
    assert!(s.ui_halted());
    assert_eq!(
        s.banner_view().kind(),
        Some(&BannerKind::InternalError { halted: true })
    );
    assert!(
        s.banner_view().dismiss_rect().is_none(),
        "only SAVE remains"
    );

    // The window paints the notice and nothing else.
    let texts = painted(&mut s);
    assert!(
        texts.iter().any(|t| t.contains("Repeated internal errors")),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "SAVE"), "{texts:?}");
    assert!(texts.len() <= 3, "only the notice: {texts:?}");

    // A click on SAVE gets through; a click elsewhere does nothing.
    let save = s.banner_view().save_rect().unwrap();
    let requests = click(&mut s, centre(save));
    assert!(
        requests.contains(&ShellRequest::PickSaveFile),
        "{requests:?}"
    );
    assert!(click(&mut s, Point::new(500.0, 400.0)).is_empty());
    assert!(texts_after(&mut s).len() <= 3, "the click changed nothing");
}

#[test]
fn a_panic_drops_the_open_menu_and_the_drag_group() {
    let dir = temp("drop");
    let mut s = shell(&dir);
    s.handle(&UiEvent::PointerDown {
        pos: Point::new(600.0, 400.0),
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.after_ui_panic(true, false);
    assert!(s.context_menu.is_none());
    assert!(s.capture.is_none());
}
