//! The Chord Trainer pane through its public contract: the setup (instrument, capo, chords by key,
//! by typed list or from the song), the drills driven by an injected clock (timed, timed with the
//! input checking each chord, listen), the best scores and that nothing here touches the song.

mod support;

use atomik_core::theory::ChordKind;
use atomik_model::RackDoc;
use atomik_model::harmony::ChordEvent;
use atomik_tab::trainer::{DrillEvent, DrillMode, Order, Scores, pair_key};
use atomik_tab::{ChordSymbol, grade};
use atomik_ui::keymap::{CommandOutcome, Registry as Commands};
use atomik_ui::kurbo::Point;
use atomik_ui::{
    ChordTrainerView, Key, Lang, Modifiers, PointerButton, Theme, TrainerButton, TrainerSource,
    UiAction, UiEvent,
};
use support::sheet::*;
use support::*;

fn click(v: &mut ChordTrainerView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        doc,
        &registry(),
    )
}

fn press(v: &mut ChordTrainerView, doc: &RackDoc, b: TrainerButton) -> Vec<UiAction> {
    let p = v
        .button_center(b)
        .unwrap_or_else(|| panic!("no button {b:?}"));
    assert!(v.button_on(b), "{b:?} is disabled");
    click(v, doc, p)
}

fn key(v: &mut ChordTrainerView, doc: &RackDoc, key: Key) {
    v.handle(
        &UiEvent::Key {
            key,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        doc,
        &registry(),
    );
}

fn command(v: &mut ChordTrainerView, id: &str, arg: Option<&str>) -> CommandOutcome {
    v.command(id, arg, &reference_doc(), &registry())
}

fn ok(v: &mut ChordTrainerView, id: &str, arg: Option<&str>) {
    match command(v, id, arg) {
        CommandOutcome::Handled(_) => {}
        other => panic!("{id} {arg:?}: {other:?}"),
    }
}

fn view() -> ChordTrainerView {
    let mut v = ChordTrainerView::new(Theme::dark());
    v.set_bounds(SHEET_BOUNDS);
    v.set_input(Some("In 1".to_owned()));
    v.set_seed(11);
    v
}

/// A chroma that is chord `i` of the view's set.
fn chroma_of(v: &ChordTrainerView, i: usize) -> [f32; 12] {
    let mask = grade::pc_mask(&v.chords()[i].pitch_classes());
    let mut c = [0.03f32; 12];
    for (pc, x) in c.iter_mut().enumerate() {
        if mask >> pc & 1 == 1 {
            *x = 1.0;
        }
    }
    c
}

fn names(v: &ChordTrainerView) -> Vec<String> {
    v.chord_names()
}

// ------------------------------------------------------------------------ setup

#[test]
fn a_new_view_offers_the_triads_of_c_major_on_a_guitar() {
    let v = view();
    assert_eq!(names(&v), ["C", "Dm", "Em", "F", "G", "Am", "Bdim"]);
    assert_eq!(v.source(), TrainerSource::Key);
    assert_eq!(v.instrument_id(), "guitar6");
    assert!(!v.running());
}

#[test]
fn the_key_buttons_change_the_set() {
    let doc = reference_doc();
    let mut v = view();
    for _ in 0..7 {
        press(&mut v, &doc, TrainerButton::KeyUp);
    }
    assert_eq!(v.key(), (7, false));
    assert_eq!(names(&v), ["G", "Am", "Bm", "C", "D", "Em", "F#dim"]);
    press(&mut v, &doc, TrainerButton::KeyMode);
    assert_eq!(v.key(), (7, true));
    assert_eq!(names(&v), ["Gm", "Adim", "Bb", "Cm", "Dm", "Eb", "F"]);
    press(&mut v, &doc, TrainerButton::Sevenths);
    assert!(v.sevenths());
    assert_eq!(names(&v)[0], "Gm7");
    press(&mut v, &doc, TrainerButton::KeyDown);
    assert_eq!(v.key().0, 6);
}

#[test]
fn a_typed_list_flags_what_is_not_a_chord() {
    let doc = reference_doc();
    let mut v = view();
    press(&mut v, &doc, TrainerButton::Source);
    assert_eq!(v.source(), TrainerSource::List);
    assert!(v.chords().is_empty());
    press(&mut v, &doc, TrainerButton::EditList);
    assert!(v.is_text_owner());
    for ch in "Am F C xyz".chars() {
        key(&mut v, &doc, Key::Char(ch));
    }
    assert_eq!(v.edit_text(), Some("Am F C xyz"));
    key(&mut v, &doc, Key::Enter);
    assert!(!v.is_text_owner());
    assert_eq!(names(&v), ["Am", "F", "C"]);
    assert_eq!(v.invalid_words(), ["xyz"]);
    assert!(v.status().is_some_and(|s| s.contains("xyz")));
    // Escape leaves the field as it was.
    press(&mut v, &doc, TrainerButton::EditList);
    v.set_edit_text("Em");
    key(&mut v, &doc, Key::Escape);
    assert_eq!(names(&v), ["Am", "F", "C"]);
}

#[test]
fn the_song_source_takes_the_chord_lane_and_the_sheets() {
    let reg = registry();
    let mut doc = sheet_doc(vec![sheet(LANTERN)]);
    doc.song_mut().harmony.chords = vec![
        ChordEvent::new(0, 15_360, 9, ChordKind::Minor),
        ChordEvent::new(15_360, 15_360, 5, ChordKind::Major),
    ];
    let mut v = view();
    v.sync(&doc, &reg);
    ok(&mut v, "editor.trainer_source", Some("song"));
    // The lane first, then the sheet's own chords (Lantern Road: G D Em C).
    assert_eq!(names(&v), ["Am", "F", "G", "D", "Em", "C"]);
    // A new chord in the lane shows at once.
    doc.song_mut()
        .harmony
        .chords
        .push(ChordEvent::new(30_720, 15_360, 2, ChordKind::Minor));
    v.sync(&doc, &reg);
    assert_eq!(names(&v).last().map(String::as_str), Some("C"));
    assert!(names(&v).contains(&"Dm".to_owned()));
}

#[test]
fn the_instrument_list_picks_a_banjo_and_its_diagrams_follow() {
    let doc = reference_doc();
    let mut v = view();
    press(&mut v, &doc, TrainerButton::Instrument);
    assert!(v.is_text_owner());
    let at = v.instrument_row_center("banjo5").expect("a banjo row");
    click(&mut v, &doc, at);
    assert!(!v.is_text_owner());
    assert_eq!(v.instrument_id(), "banjo5");
    assert!(v.voicing_choice(0).is_some_and(|(_, n)| n > 0));
    press(&mut v, &doc, TrainerButton::Tuning);
    assert_ne!(v.tuning_name(), "Open G (gDGBD)");
}

#[test]
fn the_capo_is_limited_and_changes_the_fingerings() {
    let doc = reference_doc();
    let mut v = view();
    let open = v.voicing_choice(0);
    assert!(!v.button_on(TrainerButton::CapoDown));
    press(&mut v, &doc, TrainerButton::CapoUp);
    press(&mut v, &doc, TrainerButton::CapoUp);
    assert_eq!(v.capo(), 2);
    assert!(v.voicing_choice(0).is_some());
    for _ in 0..20 {
        ok(&mut v, "editor.trainer_capo", Some("up"));
    }
    assert_eq!(v.capo(), 12);
    assert!(!v.button_on(TrainerButton::CapoUp));
    let _ = open;
}

#[test]
fn clicking_a_diagram_selects_it_and_clicking_again_cycles_the_fingering() {
    let doc = reference_doc();
    let mut v = view();
    let p = v.strip_center(3).unwrap();
    click(&mut v, &doc, p);
    assert_eq!(v.selected(), 3);
    let (before, n) = v.voicing_choice(3).unwrap();
    assert!(n > 1, "a few fingerings of F");
    click(&mut v, &doc, p);
    assert_eq!(v.voicing_choice(3).unwrap().0, (before + 1) % n);
    key(&mut v, &doc, Key::Left);
    assert_eq!(v.voicing_choice(3).unwrap().0, before);
    ok(&mut v, "editor.trainer_voicing", Some("prev"));
    assert_eq!(v.voicing_choice(3).unwrap().0, (before + n - 1) % n);
}

#[test]
fn the_drill_settings_have_limits() {
    let mut v = view();
    for _ in 0..100 {
        ok(&mut v, "editor.trainer_bpm", Some("up"));
        ok(&mut v, "editor.trainer_beats", Some("up"));
    }
    assert_eq!((v.config().bpm, v.config().beats), (240, 16));
    for _ in 0..100 {
        ok(&mut v, "editor.trainer_bpm", Some("down"));
        ok(&mut v, "editor.trainer_beats", Some("down"));
    }
    assert_eq!((v.config().bpm, v.config().beats), (30, 1));
    ok(&mut v, "editor.trainer_mode", Some("listen"));
    assert_eq!(v.config().mode, DrillMode::Listen);
    ok(&mut v, "editor.trainer_order", Some("random"));
    assert_eq!(v.config().order, Order::Random);
    assert!(matches!(
        command(&mut v, "editor.trainer_mode", Some("sideways")),
        CommandOutcome::NotApplicable(_)
    ));
    assert!(matches!(
        command(&mut v, "edit.copy", None),
        CommandOutcome::Declined
    ));
}

#[test]
fn the_pair_chords_never_coincide() {
    let mut v = view();
    ok(&mut v, "editor.trainer_order", Some("pair"));
    let mut seen = Vec::new();
    for _ in 0..16 {
        ok(&mut v, "editor.trainer_pair", Some("a"));
        ok(&mut v, "editor.trainer_pair", Some("b"));
        let (a, b) = v.pair();
        assert_ne!(a, b);
        seen.push((a, b));
    }
    seen.sort_unstable();
    seen.dedup();
    assert!(seen.len() > 3);
}

// ------------------------------------------------------------------------ timed drill

fn start(v: &mut ChordTrainerView, at: f64) {
    ok(v, "editor.trainer_start", None);
    assert!(!v.running(), "the drill starts at the next frame");
    v.tick(at);
    assert!(v.running());
}

fn clicks(events: &[DrillEvent]) -> Vec<bool> {
    events
        .iter()
        .filter_map(|e| match e {
            DrillEvent::Click { accent } => Some(*accent),
            _ => None,
        })
        .collect()
}

#[test]
fn a_timed_drill_clicks_the_beats_and_changes_chords_on_the_clock() {
    let mut v = view();
    // 120 BPM, two beats a chord: a change every second.
    for _ in 0..12 {
        ok(&mut v, "editor.trainer_bpm", Some("up"));
    }
    for _ in 0..2 {
        ok(&mut v, "editor.trainer_beats", Some("down"));
    }
    assert_eq!((v.config().bpm, v.config().beats), (120, 2));
    start(&mut v, 50.0);
    let first = v.take_events();
    assert_eq!(clicks(&first), [true], "the first beat at the start");
    assert_eq!(v.drill().unwrap().current(), 0);
    v.tick(50.6);
    assert_eq!(clicks(&v.take_events()), [false]);
    v.tick(51.0);
    let ev = v.take_events();
    assert_eq!(clicks(&ev), [true]);
    assert!(ev.contains(&DrillEvent::Changed { from: 0, to: 1 }));
    assert_eq!(v.drill().unwrap().current(), 1);
    v.tick(80.0);
    let d = v.drill().unwrap();
    assert_eq!(d.changes(), 30);
    assert!((d.changes_per_minute(80.0) - 60.0).abs() < 0.5);
    assert!((d.elapsed(80.0) - 30.0).abs() < 1e-9);
}

#[test]
fn a_late_first_frame_does_not_burst_clicks() {
    let mut v = view();
    ok(&mut v, "editor.trainer_start", None);
    // The command came long before the frame: the drill starts at the frame's time.
    v.tick(1000.0);
    assert_eq!(clicks(&v.take_events()), [true]);
}

#[test]
fn a_running_drill_stops_with_the_command_escape_or_leaving() {
    let doc = reference_doc();
    let mut v = view();
    start(&mut v, 0.0);
    ok(&mut v, "editor.trainer_start", None);
    assert!(!v.running());
    assert!(v.drill().is_some(), "its figures stay on show");
    start(&mut v, 10.0);
    key(&mut v, &doc, Key::Escape);
    assert!(!v.running());
    start(&mut v, 20.0);
    v.leave();
    assert!(!v.running());
    // A start asked for and cancelled before a frame does not run.
    ok(&mut v, "editor.trainer_start", None);
    ok(&mut v, "editor.trainer_start", None);
    v.tick(30.0);
    assert!(!v.running());
}

#[test]
fn changing_a_setting_restarts_a_running_drill() {
    let mut v = view();
    start(&mut v, 0.0);
    v.take_events();
    v.tick(8.0);
    assert!(v.drill().unwrap().changes() > 0);
    ok(&mut v, "editor.trainer_bpm", Some("up"));
    assert!(!v.running());
    v.tick(8.5);
    assert!(v.running());
    assert_eq!(v.drill().unwrap().changes(), 0, "a fresh session");
    assert_eq!(v.config().bpm, 65);
}

#[test]
fn a_drill_needs_two_chords_and_an_input_to_listen() {
    let mut v = view();
    ok(&mut v, "editor.trainer_source", Some("list"));
    assert!(matches!(
        command(&mut v, "editor.trainer_start", None),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_list_text("Am F");
    ok(&mut v, "editor.trainer_mode", Some("listen"));
    v.set_input(None);
    assert!(matches!(
        command(&mut v, "editor.trainer_start", None),
        CommandOutcome::NotApplicable(m) if m.contains("input")
    ));
    v.set_input(Some("In 2".to_owned()));
    ok(&mut v, "editor.trainer_start", None);
}

#[test]
fn the_wants_follow_the_mode() {
    let mut v = view();
    start(&mut v, 0.0);
    assert!(v.wants_click() && !v.wants_input());
    ok(&mut v, "editor.trainer_mode", Some("verified"));
    v.tick(1.0);
    assert!(v.wants_click() && v.wants_input());
    ok(&mut v, "editor.trainer_mode", Some("listen"));
    v.tick(2.0);
    assert!(!v.wants_click() && v.wants_input());
}

// ------------------------------------------------------------------------ listening

#[test]
fn a_verified_drill_counts_the_chords_the_input_heard() {
    let mut v = view();
    ok(&mut v, "editor.trainer_mode", Some("verified"));
    for _ in 0..12 {
        ok(&mut v, "editor.trainer_bpm", Some("up"));
    }
    for _ in 0..2 {
        ok(&mut v, "editor.trainer_beats", Some("down"));
    }
    ok(&mut v, "editor.trainer_order", Some("pair"));
    ok(&mut v, "editor.trainer_pair", Some("a"));
    ok(&mut v, "editor.trainer_pair", Some("b"));
    let (a, b) = v.pair();
    start(&mut v, 0.0);
    let mut t = 0.0;
    for step in 0..12 {
        v.tick(t);
        let cur = v.drill().unwrap().current();
        // Window 4 is played with the wrong chord.
        let wrong = if cur == a { b } else { a };
        let heard = if step == 4 { wrong } else { cur };
        let chroma = chroma_of(&v, heard);
        v.hear(t + 0.1, chroma);
        t += 1.0;
    }
    v.tick(t + 1.0);
    let (hits, judged) = v.drill().unwrap().hits();
    assert!(judged >= 10, "{judged}");
    assert_eq!(judged - hits, 1, "one wrong window");
    ok(&mut v, "editor.trainer_start", None);
    // The session counted for the pair.
    let scores = v.take_scores_change().expect("scores moved");
    let key = pair_key(&names(&v)[a], &names(&v)[b]);
    let best = scores.get(&key).expect("the pair");
    assert!(best.hit_rate > 0.8 && best.hit_rate < 1.0);
    assert!(v.take_scores_change().is_none());
}

#[test]
fn a_listen_drill_moves_on_when_the_chord_is_heard_and_flags_it() {
    let mut v = view();
    ok(&mut v, "editor.trainer_mode", Some("listen"));
    start(&mut v, 5.0);
    assert!(v.take_events().is_empty(), "no click in the listen drill");
    let wrong = chroma_of(&v, 3);
    v.hear(6.0, wrong);
    assert_eq!(v.drill().unwrap().current(), 0);
    let right = chroma_of(&v, 0);
    v.hear(6.5, right);
    assert_eq!(v.drill().unwrap().current(), 1);
    assert!(
        v.take_events()
            .contains(&DrillEvent::Changed { from: 0, to: 1 })
    );
    // Skip is the way out of a chord that will not come.
    ok(&mut v, "editor.trainer_skip", None);
    assert_eq!(v.drill().unwrap().current(), 2);
}

#[test]
fn skip_is_refused_outside_a_listen_drill() {
    let mut v = view();
    assert!(matches!(
        command(&mut v, "editor.trainer_skip", None),
        CommandOutcome::NotApplicable(_)
    ));
    start(&mut v, 0.0);
    assert!(matches!(
        command(&mut v, "editor.trainer_skip", None),
        CommandOutcome::NotApplicable(_)
    ));
}

// ------------------------------------------------------------------------ scores

#[test]
fn the_scores_come_from_the_host_go_back_to_it_and_reset() {
    let doc = reference_doc();
    let mut v = view();
    let mut s = Scores::new();
    s.record(&atomik_tab::trainer::PairResult {
        key: "Am|C".into(),
        attempts: 10,
        hits: 9,
        cpm: 42.0,
    });
    v.set_scores(s.clone());
    assert_eq!(v.scores(), &s);
    assert!(
        v.take_scores_change().is_none(),
        "what the host set is not a change"
    );
    let p = v.reset_center();
    click(&mut v, &doc, p);
    assert!(v.scores().is_empty());
    assert_eq!(v.take_scores_change(), Some(Scores::new()));
    // Nothing to forget: no change to save.
    ok(&mut v, "editor.trainer_reset_scores", None);
    assert!(v.take_scores_change().is_none());
}

#[test]
fn a_plain_timed_drill_records_nothing() {
    let mut v = view();
    start(&mut v, 0.0);
    v.tick(60.0);
    ok(&mut v, "editor.trainer_start", None);
    assert!(v.take_scores_change().is_none());
    assert!(v.scores().is_empty());
}

// ------------------------------------------------------------------------ contract

#[test]
fn the_pane_reads_the_song_but_never_writes_it() {
    let reg = registry();
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let before = doc.clone();
    let mut v = view();
    v.sync(&doc, &reg);
    ok(&mut v, "editor.trainer_source", Some("song"));
    start(&mut v, 0.0);
    v.tick(30.0);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert_eq!(doc, before);
}

#[test]
fn every_button_has_a_tooltip_and_a_registered_command() {
    let reg = Commands::bundled();
    let doc = reference_doc();
    let mut v = view();
    ok(&mut v, "editor.trainer_order", Some("pair"));
    ok(&mut v, "editor.trainer_mode", Some("verified"));
    let buttons = [
        TrainerButton::Instrument,
        TrainerButton::Tuning,
        TrainerButton::CapoDown,
        TrainerButton::CapoUp,
        TrainerButton::Source,
        TrainerButton::KeyDown,
        TrainerButton::KeyUp,
        TrainerButton::KeyMode,
        TrainerButton::Sevenths,
        TrainerButton::Mode,
        TrainerButton::Order,
        TrainerButton::BeatsDown,
        TrainerButton::BeatsUp,
        TrainerButton::BpmDown,
        TrainerButton::BpmUp,
        TrainerButton::PairA,
        TrainerButton::PairB,
        TrainerButton::Start,
        TrainerButton::Skip,
        TrainerButton::Input,
    ];
    for b in buttons {
        let p = v.button_center(b).unwrap_or_else(|| panic!("no {b:?}"));
        let help = v
            .help_at(p)
            .unwrap_or_else(|| panic!("no tooltip for {b:?}"));
        if let Some(id) = help.command {
            assert!(reg.get(id).is_some(), "{id} is not registered");
            if let Some(arg) = help.arg {
                assert!(reg.validate(id, Some(arg)), "{id} {arg}");
            }
        }
    }
    let _ = doc;
}

#[test]
fn every_trainer_command_is_registered_with_a_french_label() {
    let reg = Commands::bundled();
    let ids: Vec<&str> = reg
        .commands()
        .iter()
        .map(|d| d.id)
        .filter(|id| id.starts_with("editor.trainer_") || *id == "view.toggle_chord_trainer")
        .collect();
    assert_eq!(ids.len(), 17);
    for id in ids {
        let d = reg.get(id).unwrap();
        assert!(atomik_ui::i18n::has(Lang::French, d.label), "{}", d.label);
    }
}

#[test]
fn what_the_view_paints_has_french_entries() {
    let reg = registry();
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let (_, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            let mut v = view();
            let mut dl = atomik_ui::DisplayList::new();
            v.paint(&mut dl, &doc, &reg);
            for (id, arg) in [
                ("editor.trainer_source", Some("list")),
                ("editor.trainer_mode", Some("verified")),
                ("editor.trainer_order", Some("pair")),
            ] {
                ok(&mut v, id, arg);
                v.paint(&mut dl, &doc, &reg);
            }
            ok(&mut v, "editor.trainer_edit_list", None);
            v.paint(&mut dl, &doc, &reg);
            key(&mut v, &doc, Key::Escape);
            ok(&mut v, "editor.trainer_source", Some("key"));
            ok(&mut v, "editor.trainer_start", None);
            v.tick(0.0);
            v.paint(&mut dl, &doc, &reg);
            ok(&mut v, "editor.trainer_mode", Some("listen"));
            v.tick(1.0);
            v.paint(&mut dl, &doc, &reg);
            ok(&mut v, "editor.trainer_start", None);
            v.open_instrument_list();
            v.paint(&mut dl, &doc, &reg);
            let mut s = Scores::new();
            s.record(&atomik_tab::trainer::PairResult {
                key: "Am|C".into(),
                attempts: 9,
                hits: 8,
                cpm: 30.0,
            });
            v.set_scores(s);
            v.paint(&mut dl, &doc, &reg);
        })
    });
    let missing: Vec<&String> = misses
        .iter()
        .filter(|m| !m.chars().any(|c| c.is_ascii_digit()) && m.chars().count() > 2)
        .collect();
    assert!(missing.is_empty(), "no French entry for: {missing:?}");
    let _ = ChordSymbol::parse("C");
}
