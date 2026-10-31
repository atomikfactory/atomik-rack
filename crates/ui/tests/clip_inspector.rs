//! The clip inspector in the transport bar: what it shows for none, one, several and mixed
//! audio clips or an audio track, the edits (drag, wheel, typing, reverse) on one and on several
//! clips, the refusals, and the narrow bar.

mod support;

use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, RackDoc, SongCommand, WarpMode,
};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{DisplayList, InspectorField, Key, Modifiers, TransportBar, UiAction, UiEvent};
use support::audio::*;
use support::song::*;
use support::*;

/// A bar `width` wide showing the inspector for the given clips (or track).
fn bar(doc: &RackDoc, ids: &[u64], width: f64) -> TransportBar {
    let reg = registry();
    let mut arr = audio_arrangement(doc, &reg);
    let ids: Vec<ClipId> = ids.iter().map(|i| ClipId(*i)).collect();
    arr.set_selected_clips(&ids);
    let mut bar = TransportBar::new(atomik_ui::Theme::dark());
    bar.set_bounds(Rect::new(0.0, 0.0, width, 44.0));
    bar.sync(doc, &reg);
    bar.set_inspector(arr.audio_inspector());
    bar
}

fn bar_for_track(doc: &RackDoc, device: &str) -> TransportBar {
    let reg = registry();
    let mut arr = audio_arrangement(doc, &reg);
    arr.set_selected_track(Some(key(device)));
    let mut bar = TransportBar::new(atomik_ui::Theme::dark());
    bar.set_bounds(Rect::new(0.0, 0.0, 1500.0, 44.0));
    bar.sync(doc, &reg);
    bar.set_inspector(arr.audio_inspector());
    bar
}

fn texts(bar: &mut TransportBar, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    bar.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn audio_mut(doc: &mut RackDoc, id: u64, mut f: impl FnMut(&mut AudioClipContent)) {
    for t in &mut doc.song_mut().tracks {
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

type Set = (ClipId, AudioClipContent, Option<u64>);

fn contents(actions: &[UiAction]) -> Vec<Set> {
    fn one(c: &SongCommand, out: &mut Vec<Set>) {
        if let SongCommand::SetAudioClip {
            id, content, merge, ..
        } = c
        {
            out.push((*id, content.clone(), *merge));
        }
    }
    let mut out = Vec::new();
    for c in commands(actions) {
        match c {
            Command::Song(s) => one(&s, &mut out),
            Command::Batch(list) => {
                for c in list {
                    if let Command::Song(s) = c {
                        one(&s, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn handle(bar: &mut TransportBar, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| bar.handle(e, doc, &reg), events)
}

fn wheel(pos: Point, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        pos,
        delta: Vec2::new(0.0, dy),
        modifiers: Modifiers::NONE,
    }
}

fn key_down(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn above(p: Point, px: f64) -> Point {
    Point::new(p.x, p.y - px)
}

// ---------------------------------------------------------------- display

#[test]
fn nothing_selected_shows_no_inspector() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[], 1500.0);
    assert!(b.inspector_fields().is_empty());
    let t = texts(&mut b, &doc);
    assert!(!t.iter().any(|s| s == "FADE IN"), "{t:?}");
    // A notes clip is not applicable either.
    let mut b = bar(&doc, &[1], 1500.0);
    assert!(b.inspector_fields().is_empty());
    assert!(!texts(&mut b, &doc).iter().any(|s| s == "TRANSPOSE"));
}

#[test]
fn one_clip_shows_its_fades_transpose_gain_and_name() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let t = texts(&mut b, &doc);
    for want in ["FADE IN", "FADE OUT", "TRANSPOSE", "GAIN", "REV"] {
        assert!(t.iter().any(|s| s == want), "{want} in {t:?}");
    }
    // 24 000 and 48 000 frames at 48 kHz.
    for want in ["500 ms", "1000 ms", "+0.00", "-3.0 dB", "drums.wav"] {
        assert!(t.iter().any(|s| s == want), "{want} in {t:?}");
    }
}

#[test]
fn clips_that_differ_show_a_dash_and_equal_ones_the_value() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10, 11], 1500.0);
    let t = texts(&mut b, &doc);
    // Fade in 500 / 0 ms, fade out 1000 / 0 ms, gain -3 / 0 dB: all mixed; transpose equal.
    assert_eq!(t.iter().filter(|s| *s == "\u{2014}").count(), 3, "{t:?}");
    assert!(t.iter().any(|s| s == "+0.00"));
    assert!(t.iter().any(|s| s == "2 audio clips"));
}

#[test]
fn an_audio_track_alone_shows_its_name_and_dimmed_empty_fields() {
    let doc = audio_doc();
    let mut b = bar_for_track(&doc, "audio-track-2");
    let t = texts(&mut b, &doc);
    assert!(t.iter().any(|s| s == "Vox"), "{t:?}");
    assert!(t.iter().any(|s| s == "FADE IN"));
    assert!(!t.iter().any(|s| s.ends_with(" ms")));
    // Nothing to edit: a drag, the wheel and a click do nothing, and the tooltip says why.
    let p = b.inspector_point(InspectorField::FadeIn).unwrap();
    let a = handle(
        &mut b,
        &doc,
        &[down(p), mv(above(p, 40.0)), up(p), wheel(p, 1.0)],
    );
    assert!(a.is_empty(), "{a:?}");
    let help = b.help_at(p).unwrap();
    assert!(help.text.contains("select an audio clip"), "{}", help.text);
    // A note track gives no inspector.
    let b = bar_for_track(&doc, "bass");
    assert!(b.inspector_fields().is_empty());
}

#[test]
fn every_field_has_a_tooltip() {
    let doc = audio_doc();
    let b = bar(&doc, &[10], 1500.0);
    for f in b.inspector_fields() {
        let p = b.inspector_point(f).unwrap();
        assert!(b.help_at(p).is_some_and(|h| !h.text.is_empty()), "{f:?}");
    }
}

// ---------------------------------------------------------------- narrow bars

#[test]
fn a_narrow_bar_drops_the_name_then_fields_from_the_right_and_never_overlaps() {
    let doc = audio_doc();
    let mut last = usize::MAX;
    for w in [1500.0, 1200.0, 1100.0, 1000.0, 900.0, 800.0, 700.0, 500.0] {
        let b = bar(&doc, &[10], w);
        let n = b.inspector_fields().len();
        assert!(n <= last, "{w}: {n} after {last}");
        last = n;
        let meter_right = b.meter_center().x + 23.0;
        for f in b.inspector_fields() {
            let p = b.inspector_point(f).unwrap();
            assert!(p.x > meter_right, "{w}: {f:?} at {p:?} over the readouts");
            assert!(p.x < w, "{w}: {f:?} outside the bar");
        }
    }
    assert_eq!(bar(&doc, &[10], 1500.0).inspector_fields().len(), 6);
    assert_eq!(bar(&doc, &[10], 500.0).inspector_fields().len(), 0);
    // The name goes first: a width that holds every field but not the name.
    let mut b = bar(&doc, &[10], 1500.0);
    assert!(texts(&mut b, &doc).iter().any(|s| s == "drums.wav"));
    let mut b = bar(&doc, &[10], 1090.0);
    assert_eq!(b.inspector_fields().len(), 6);
    assert!(!texts(&mut b, &doc).iter().any(|s| s == "drums.wav"));
}

// ---------------------------------------------------------------- edits

#[test]
fn a_drag_on_fade_in_sets_it_on_every_selected_clip_as_one_merge_group() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10, 11], 1500.0);
    let p = b.inspector_point(InspectorField::FadeIn).unwrap();
    // 20 px up is four steps of 10 ms from the first clip's 500 ms.
    let a = handle(&mut b, &doc, &[down(p), mv(above(p, 20.0)), up(p)]);
    let set = contents(&a);
    assert_eq!(set.len(), 2, "{a:?}");
    let merge = set[0].2.expect("a drag merges");
    for (id, content, m) in &set {
        assert_eq!(*m, Some(merge), "{id:?}");
        assert_eq!(content.fade_in, 540 * 48, "{id:?}");
    }
    assert_eq!(
        set.iter().map(|s| s.0).collect::<Vec<_>>(),
        [ClipId(10), ClipId(11)]
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn a_fade_never_passes_the_clip_less_the_other_fade() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::FadeIn).unwrap();
    // The clip is 4 s with a 1 s fade out: the fade in tops out at 3000 ms.
    let a = handle(&mut b, &doc, &[down(p), mv(above(p, 5000.0)), up(p)]);
    let last = contents(&a).pop().unwrap().1;
    assert_eq!(last.fade_in, 3000 * 48);
}

#[test]
fn the_wheel_steps_the_transposition_in_semitones_or_cents() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let whole = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    let cents = b.inspector_point(InspectorField::TransposeCents).unwrap();
    let a = handle(&mut b, &doc, &[wheel(whole, 1.0)]);
    assert_eq!(contents(&a)[0].1.warp.transpose_cents, 100);
    let a = handle(&mut b, &doc, &[wheel(cents, -1.0)]);
    assert_eq!(contents(&a)[0].1.warp.transpose_cents, -1);
}

#[test]
fn a_wheel_notch_on_several_clips_is_one_batch() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10, 11], 1500.0);
    let p = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    let a = handle(&mut b, &doc, &[wheel(p, 1.0)]);
    assert_eq!(a.len(), 1);
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::Batch(list)] if list.len() == 2
    ));
    assert!(contents(&a).iter().all(|c| c.1.warp.transpose_cents == 100));
}

#[test]
fn double_click_types_a_value_and_enter_applies_it() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::FadeOut).unwrap();
    let a = handle(&mut b, &doc, &[down_mods(p, Modifiers::NONE, 2), up(p)]);
    assert!(a.is_empty() && b.is_editing_inspector());
    assert_eq!(b.inspector_text(), Some("1000"));
    for c in "250".chars() {
        handle(&mut b, &doc, &[key_down(Key::Char(c))]);
    }
    assert_eq!(b.inspector_text(), Some("250"));
    let a = handle(&mut b, &doc, &[key_down(Key::Enter)]);
    assert!(!b.is_editing_inspector());
    assert_eq!(contents(&a)[0].1.fade_out, 250 * 48);
    // Escape cancels.
    handle(&mut b, &doc, &[down_mods(p, Modifiers::NONE, 2), up(p)]);
    let a = handle(
        &mut b,
        &doc,
        &[key_down(Key::Char('9')), key_down(Key::Escape)],
    );
    assert!(a.is_empty() && !b.is_editing_inspector());
}

#[test]
fn typed_semitones_become_cents() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    handle(&mut b, &doc, &[down_mods(p, Modifiers::NONE, 2), up(p)]);
    for c in "-2.5".chars() {
        handle(&mut b, &doc, &[key_down(Key::Char(c))]);
    }
    let a = handle(&mut b, &doc, &[key_down(Key::Enter)]);
    assert_eq!(contents(&a)[0].1.warp.transpose_cents, -250);
}

#[test]
fn a_reversed_clip_cannot_be_transposed_and_says_so() {
    let mut doc = audio_doc();
    audio_mut(&mut doc, 10, |a| a.reverse = true);
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    let a = handle(
        &mut b,
        &doc,
        &[wheel(p, 1.0), down(p), mv(above(p, 30.0)), up(p)],
    );
    assert!(a.is_empty(), "{a:?}");
    let help = b.help_at(p).unwrap();
    assert!(help.text.contains("reversed"), "{}", help.text);
    // The other fields still work.
    let q = b.inspector_point(InspectorField::Gain).unwrap();
    assert!(!handle(&mut b, &doc, &[wheel(q, 1.0)]).is_empty());
}

#[test]
fn the_repitch_mode_disables_the_transposition() {
    let mut doc = audio_doc();
    audio_mut(&mut doc, 10, |a| a.warp.mode = WarpMode::Repitch);
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    assert!(handle(&mut b, &doc, &[wheel(p, 1.0)]).is_empty());
    assert!(b.help_at(p).unwrap().text.contains("Repitch"));
}

#[test]
fn a_mixed_selection_transposes_only_the_clips_that_can() {
    let mut doc = audio_doc();
    audio_mut(&mut doc, 10, |a| a.reverse = true);
    let mut b = bar(&doc, &[10, 11], 1500.0);
    let p = b.inspector_point(InspectorField::TransposeWhole).unwrap();
    let a = handle(&mut b, &doc, &[wheel(p, 1.0)]);
    let set = contents(&a);
    assert_eq!(set.len(), 1);
    assert_eq!(set[0].0, ClipId(11));
}

#[test]
fn reverse_toggles_all_and_switches_the_warp_off() {
    let mut doc = audio_doc();
    audio_mut(&mut doc, 10, |a| {
        a.warp.enabled = true;
        a.warp.transpose_cents = 200;
    });
    let mut b = bar(&doc, &[10, 11], 1500.0);
    let p = b.inspector_point(InspectorField::Reverse).unwrap();
    let a = handle(&mut b, &doc, &[down(p), up(p)]);
    let set = contents(&a);
    assert_eq!(set.len(), 2);
    for (_, c, _) in &set {
        assert!(c.reverse && !c.warp.enabled && c.warp.transpose_cents == 0);
    }
    assert!(matches!(commands(&a).as_slice(), [Command::Batch(_)]));
}

#[test]
fn a_gain_wheel_steps_a_decibel() {
    let doc = audio_doc();
    let mut b = bar(&doc, &[10], 1500.0);
    let p = b.inspector_point(InspectorField::Gain).unwrap();
    let a = handle(&mut b, &doc, &[wheel(p, 1.0)]);
    assert!((contents(&a)[0].1.gain_db - -2.0).abs() < 1e-6);
}
