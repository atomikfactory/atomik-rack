//! The CPU readout in the shell: the tooltip of the status line and the card that suggests
//! freezing the heaviest track (its buttons, and that it waits while a dialog is open).

use super::*;
use crate::cpu_hint::CpuHintCard;
use crate::session::NullBackend;
use atomik_ui::{DisplayList, HELP_DELAY};
use std::path::PathBuf;

const VIEW: Size = Size::new(1000.0, 800.0);

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell(tag: &str) -> (Shell<NullBackend>, Dir) {
    let dir = std::env::temp_dir().join(format!("atomik-shell-cpu-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: Some(atomik_ui::keymap::Profile::Standard),
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(VIEW));
    (s, Dir(dir))
}

fn click(s: &mut Shell<NullBackend>, pos: Point) {
    s.handle(&UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn card_button(s: &Shell<NullBackend>, label: crate::cpu_hint::Choice) -> Point {
    let card = s.cpu_hint().expect("the card shows");
    card.buttons(s.size())
        .into_iter()
        .find(|(_, c)| *c == label)
        .map(|(r, _)| r.center())
        .unwrap()
}

#[test]
fn the_status_line_has_a_tooltip_with_the_engine_load() {
    let (mut s, _dir) = shell("tip");
    s.status = "WASAPI - Speakers - 48 kHz - DSP 42%".to_string();
    s.audio_status.dsp_load = 0.42;
    s.audio_status.dsp_peak = 0.55;
    let on_status = Point::new(30.0, VIEW.height - 8.0);
    s.handle(&UiEvent::PointerMove {
        pos: on_status,
        modifiers: Modifiers::NONE,
    });
    s.frame_at(1.0);
    s.frame_at(1.0 + HELP_DELAY + 0.05);
    let tip = s.tooltip().expect("shown after the delay");
    assert!(
        tip.text.starts_with("Engine load 42% (peak 55%)"),
        "{}",
        tip.text
    );
    assert!(tip.rect.contains(on_status));
    // The tooltip paints inside the window, one row per line.
    let view = Rect::new(0.0, 0.0, VIEW.width, VIEW.height);
    let boxed = atomik_ui::tooltip::box_for(tip.rect, &tip.text, view);
    assert!(boxed.y0 >= 0.0 && boxed.y1 <= VIEW.height && boxed.x1 <= VIEW.width);
    // The empty end of the bar has none of it either.
    s.handle(&UiEvent::PointerMove {
        pos: Point::new(VIEW.width - 10.0, VIEW.height - 8.0),
        modifiers: Modifiers::NONE,
    });
    assert!(
        s.tooltip()
            .is_none_or(|t| !t.text.starts_with("Engine load"))
    );
    // Away from the status line there is none of it.
    s.handle(&UiEvent::PointerMove {
        pos: Point::new(30.0, 300.0),
        modifiers: Modifiers::NONE,
    });
    assert!(
        s.tooltip()
            .is_none_or(|t| !t.text.starts_with("Engine load"))
    );
}

#[test]
fn the_card_paints_and_not_now_closes_it() {
    let (mut s, _dir) = shell("card");
    s.cpu_hint = Some(CpuHintCard::new(DeviceKey("bass".into()), "Bass", 41.0));
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(
        list.to_json()
            .contains("Track Bass uses the most CPU (41%)"),
        "the text is drawn"
    );
    let not_now = card_button(&s, crate::cpu_hint::Choice::NotNow);
    click(&mut s, not_now);
    assert!(s.cpu_hint().is_none());
}

#[test]
fn freeze_runs_freeze_track_and_a_refusal_is_the_usual_message() {
    let (mut s, _dir) = shell("freeze");
    s.cpu_hint = Some(CpuHintCard::new(DeviceKey("ghost".into()), "Ghost", 80.0));
    let freeze = card_button(&s, crate::cpu_hint::Choice::Freeze);
    click(&mut s, freeze);
    assert!(s.cpu_hint().is_none(), "the card closes");
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("no track")),
        "{:?}",
        s.core().last_rejection()
    );
}

#[test]
fn a_card_for_a_track_that_has_gone_is_dropped_and_no_track_means_no_card() {
    let (mut s, _dir) = shell("gone");
    s.cpu_hint = Some(CpuHintCard::new(DeviceKey("ghost".into()), "Ghost", 80.0));
    s.sync_cpu_hint();
    assert!(s.cpu_hint().is_none(), "its track is not in the song");
    // A heavy engine with nothing to freeze says nothing, however long it lasts.
    s.audio_status.dsp_load = 0.95;
    for t in 0..12 {
        s.core_mut().transport_frame(f64::from(t));
        s.sync_cpu_hint();
    }
    assert!(s.cpu_hint().is_none());
}
