//! Warp in the arrangement and its panel: the audio commands (warp, mode, transpose), the menu
//! state, the left trim of a warped clip, the warp panel and the clip indicator.

mod support;

use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, RackDoc, SongCommand, WarpMarker, WarpMode,
    WarpSettings,
};
use atomik_ui::audio;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Vec2;
use atomik_ui::{Modifiers, Theme, UiAction, UiEvent, WarpAction, WarpRow, WarpValues, WarpView};
use support::audio::*;
use support::*;

fn edit_clip(doc: &mut RackDoc, id: u64, f: impl Fn(&mut AudioClipContent)) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(id)
                    && let ClipContent::Audio(a) = &mut c.content
                {
                    f(a);
                }
            }
        }
    }
}

fn run(
    v: &mut atomik_ui::ArrangementView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    id: &str,
    arg: Option<&str>,
) -> CommandOutcome {
    v.command(id, arg, doc, reg)
}

fn one_content(out: &CommandOutcome) -> AudioClipContent {
    match out.actions() {
        [UiAction::Command(Command::Song(SongCommand::SetAudioClip { content, merge, .. }))] => {
            assert_eq!(*merge, None);
            content.clone()
        }
        other => panic!("one SetAudioClip expected: {other:?}"),
    }
}

#[test]
fn warp_mode_and_transpose_commands_edit_the_selected_clip() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    assert!(matches!(
        run(&mut v, &doc, &reg, "sequencer.audio_warp", None),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_selected_clips(&[ClipId(10)]);
    assert!(!v.audio_menu().warped && v.audio_menu().can_warp);
    let on = one_content(&run(&mut v, &doc, &reg, "sequencer.audio_warp", None));
    assert!(on.warp.enabled);
    let tonal = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_warp_mode",
        Some("tonal"),
    ));
    assert_eq!(tonal.warp.mode, WarpMode::Tonal);
    let up = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_transpose",
        Some("up"),
    ));
    assert_eq!(up.warp.transpose_cents, 100);
    let down = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_transpose",
        Some("down"),
    ));
    assert_eq!(down.warp.transpose_cents, -100);
    let oct = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_transpose",
        Some("octave_up"),
    ));
    assert_eq!(oct.warp.transpose_cents, 1200);
    // Reset with nothing to reset changes nothing.
    assert!(matches!(
        run(&mut v, &doc, &reg, "sequencer.audio_transpose", Some("reset")),
        CommandOutcome::Handled(a) if a.is_empty()
    ));
    // The commands that need the samples or a panel are the app's.
    for (id, arg) in [
        ("sequencer.audio_warp_tempo", None),
        ("sequencer.audio_warp_bars", Some("2")),
        ("sequencer.audio_warp_panel", None),
        ("sequencer.audio_quantize", None),
    ] {
        assert_eq!(
            run(&mut v, &doc, &reg, id, arg),
            CommandOutcome::Declined,
            "{id}"
        );
    }
}

#[test]
fn a_warped_clip_toggles_off_and_transpose_clamps() {
    let (mut doc, reg) = (audio_doc(), registry());
    edit_clip(&mut doc, 10, |a| {
        a.warp = WarpSettings {
            enabled: true,
            transpose_cents: 2400,
            ..WarpSettings::default()
        };
    });
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10)]);
    assert!(v.audio_menu().warped);
    let off = one_content(&run(&mut v, &doc, &reg, "sequencer.audio_warp", None));
    assert!(!off.warp.enabled && off.warp.transpose_cents == 2400);
    // Already at the top: up changes nothing.
    assert!(matches!(
        run(&mut v, &doc, &reg, "sequencer.audio_transpose", Some("up")),
        CommandOutcome::Handled(a) if a.is_empty()
    ));
}

#[test]
fn a_reversed_clip_is_not_warped_by_the_commands() {
    let (mut doc, reg) = (audio_doc(), registry());
    edit_clip(&mut doc, 10, |a| a.reverse = true);
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10)]);
    assert!(!v.audio_menu().can_warp);
    for (id, arg) in [
        ("sequencer.audio_warp", None),
        ("sequencer.audio_transpose", Some("up")),
    ] {
        assert!(
            matches!(run(&mut v, &doc, &reg, id, arg), CommandOutcome::Handled(a) if a.is_empty()),
            "{id}"
        );
    }
}

#[test]
fn the_commands_are_in_the_registry_with_menu_items() {
    let r = atomik_ui::keymap::Registry::bundled();
    for (id, arg) in [
        ("sequencer.audio_warp", None),
        ("sequencer.audio_warp_mode", Some("beats")),
        ("sequencer.audio_warp_mode", Some("repitch")),
        ("sequencer.audio_warp_tempo", None),
        ("sequencer.audio_warp_bars", Some("4")),
        ("sequencer.audio_warp_panel", None),
        ("sequencer.audio_transpose", Some("octave_down")),
        ("sequencer.audio_quantize", None),
    ] {
        assert!(r.validate(id, arg), "{id} {arg:?}");
        assert!(r.get(id).unwrap().menu, "{id} has a menu item");
    }
    assert!(!r.validate("sequencer.audio_warp_mode", Some("granular")));
    assert!(!r.validate("sequencer.audio_warp_bars", Some("3")));
    assert!(r.get("sequencer.audio_transpose").unwrap().repeatable);
}

#[test]
fn a_left_trim_of_a_warped_clip_moves_the_offset_through_the_map() {
    let rate = 48_000;
    // A clip of 4 beats at 120 BPM whose source runs at 100 BPM: 28 800 source frames a beat.
    let content = AudioClipContent {
        source_offset: 1_000,
        warp: WarpSettings {
            enabled: true,
            source_bpm: 100.0,
            markers: vec![WarpMarker {
                tick: 3 * atomik_core::PPQ,
                source: 100_000,
            }],
            ..WarpSettings::default()
        },
        ..AudioClipContent::new(HASH_A, 0)
    };
    let (start, length) = (4 * atomik_core::PPQ, 4 * atomik_core::PPQ);
    let wanted = start + atomik_core::PPQ;
    let (t, markers) =
        audio::trim_left_warped(rate, (start, length, &content), 1_000_000, wanted, 240);
    assert_eq!(t.start, wanted);
    assert_eq!(t.start + t.length, start + length);
    // The marker pins source frame 100 000 two beats after the new start: before it the map
    // runs at the source tempo (28 800 frames a beat at 100 BPM).
    assert_eq!(t.source_offset, 1_000 + 100_000 - 2 * 28_800);
    // The marker keeps its place on the timeline (2 beats after the new start now) and its
    // source frame (relative to the new offset); one marker pins the new start.
    assert_eq!(markers[0], WarpMarker { tick: 0, source: 0 });
    assert_eq!(markers[1].tick, 2 * atomik_core::PPQ);
    assert_eq!(markers[1].source, 100_000 + 1_000 - t.source_offset);
    // The start cannot go before frame 0 of the asset or past the asset's end.
    let (t, _) = audio::trim_left_warped(rate, (start, length, &content), 1_000_000, 0, 240);
    assert!(t.start > 0 && t.source_offset <= 28_800, "{t:?}");
    let (t, _) =
        audio::trim_left_warped(rate, (start, length, &content), 60_000, start + length, 240);
    assert!(t.source_offset <= 60_000 && t.start + 240 <= start + length);
}

fn paint_json(
    v: &mut atomik_ui::ArrangementView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

#[test]
fn a_warped_clip_is_marked_and_a_transposed_one_shows_the_shift() {
    let (mut doc, reg) = (audio_doc(), registry());
    let plain = {
        let mut v = audio_arrangement(&doc, &reg);
        paint_json(&mut v, &doc, &reg)
    };
    assert!(!plain.contains("\"W\""));
    edit_clip(&mut doc, 10, |a| {
        a.warp = WarpSettings {
            enabled: true,
            transpose_cents: 300,
            markers: vec![WarpMarker {
                tick: 1000,
                source: 5000,
            }],
            ..WarpSettings::default()
        };
    });
    let mut v = audio_arrangement(&doc, &reg);
    let marked = paint_json(&mut v, &doc, &reg);
    assert!(marked.contains("W +3"), "{marked}");
    assert_ne!(marked, plain);
}

// ---------------------------------------------------------------- the panel

fn click(v: &mut WarpView, pos: atomik_ui::kurbo::Point) -> Vec<UiAction> {
    v.handle(&down(pos))
}

#[test]
fn the_panel_steps_its_values_and_clamps_them() {
    let mut v = WarpView::new(Theme::dark(), WarpValues::default());
    assert_eq!(v.value_text(WarpRow::Warp), "Off");
    assert_eq!(v.value_text(WarpRow::Mode), "Beats");
    assert_eq!(v.value_text(WarpRow::Tempo), "120.0 BPM");
    let plus = v.plus_rect(WarpRow::Warp).center();
    click(&mut v, plus);
    assert!(v.values().enabled);
    let plus = v.plus_rect(WarpRow::Mode).center();
    click(&mut v, plus);
    assert_eq!(v.values().mode, WarpMode::Tonal);
    let minus = v.minus_rect(WarpRow::Tempo).center();
    click(&mut v, minus);
    assert_eq!(v.values().bpm_tenths, 1190);
    // Halve and double.
    let up = v.factor_rect(true).center();
    click(&mut v, up);
    assert_eq!(v.values().bpm_tenths, 2380);
    let down = v.factor_rect(false).center();
    click(&mut v, down);
    assert_eq!(v.values().bpm_tenths, 1190);
    for _ in 0..2000 {
        v.step(WarpRow::Tempo, 1);
    }
    assert_eq!(v.values().bpm_tenths, 9990);
    for _ in 0..2000 {
        v.step(WarpRow::Tempo, -1);
    }
    assert_eq!(v.values().bpm_tenths, 200);
    for _ in 0..40 {
        v.step(WarpRow::Transpose, 1);
    }
    assert_eq!(v.values().semitones, 24);
    v.step(WarpRow::Fine, 1);
    assert_eq!(v.values().cents, 0, "two octaves is the most");
    for _ in 0..80 {
        v.step(WarpRow::Transpose, -1);
    }
    v.step(WarpRow::Fine, -1);
    assert_eq!((v.values().semitones, v.values().cents), (-24, 0));
    v.step(WarpRow::Transpose, 1);
    v.step(WarpRow::Fine, 1);
    assert_eq!(v.value_text(WarpRow::Fine), "+10 cents");
    let applied = v.values().apply_to(&WarpSettings::default());
    assert_eq!(applied.transpose_cents, -2300 + 10);
}

#[test]
fn the_panel_asks_for_apply_detect_and_close() {
    let mut v = WarpView::new(Theme::dark(), WarpValues::default());
    let (apply, detect, close) = (
        v.apply_rect().center(),
        v.detect_rect().center(),
        v.close_rect().center(),
    );
    assert!(click(&mut v, apply).is_empty() && click(&mut v, detect).is_empty());
    v.set_target("1 audio clip".into(), true);
    assert_eq!(
        click(&mut v, apply),
        vec![UiAction::Warp(WarpAction::Apply)]
    );
    assert_eq!(
        click(&mut v, detect),
        vec![UiAction::Warp(WarpAction::Detect)]
    );
    assert_eq!(
        click(&mut v, close),
        vec![UiAction::Warp(WarpAction::Close)]
    );
    let row = v.row_rect(WarpRow::Tempo).center();
    v.handle(&UiEvent::Wheel {
        pos: row,
        delta: Vec2::new(0.0, 1.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.values().bpm_tenths, 1210);
    v.set_values(WarpValues {
        enabled: true,
        bpm_tenths: 905,
        ..WarpValues::default()
    });
    assert_eq!(v.value_text(WarpRow::Tempo), "90.5 BPM");
}

#[test]
fn the_panel_paints_and_its_picture_changes_with_the_values() {
    let paint = |v: &WarpView| {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl);
        dl.to_json()
    };
    let mut v = WarpView::new(Theme::dark(), WarpValues::default());
    v.set_target("1 audio clip".into(), true);
    let a = paint(&v);
    assert!(a.contains("WARP") && a.contains("1 audio clip") && a.contains("120.0 BPM"));
    assert!(a.contains("DETECT TEMPO"));
    v.step(WarpRow::Tempo, 1);
    assert_ne!(paint(&v), a);
}
