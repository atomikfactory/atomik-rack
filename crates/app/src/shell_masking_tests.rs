//! Mixer > Masking Map in the shell: the menu row, the panel, ANALYSE and its result, choosing a
//! band, DE-MASK as one undo step, locating a clash and closing with Esc.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};
use atomik_ui::{Key, Modifiers, PointerButton};

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
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
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

/// A song with a loud and a quiet synth playing the same two notes.
fn with_two_synths(s: &mut Shell<NullBackend>) {
    for (k, level) in [("loud", 0.0f32), ("quiet", -18.0)] {
        let core = s.core_mut();
        core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 0.0),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
        ] {
            core.apply_command(Command::SetParam {
                key: key(k),
                param: p.into(),
                value: v,
                merge: None,
            })
            .unwrap();
        }
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                follow_song_key: true,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(
                    [52u8, 59]
                        .iter()
                        .map(|&n| Note::new(0, 2 * BAR, n, 0.9))
                        .collect::<Vec<_>>()
                        .into(),
                ),
            },
        }))
        .unwrap();
    }
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetSongEnd(2 * BAR)))
        .unwrap();
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

/// The panel opened and analysed.
fn analysed() -> Shell<NullBackend> {
    let mut s = shell();
    with_two_synths(&mut s);
    s.execute(AppCommand::MaskingMap);
    let analyse = s.masking_view().expect("open").analyse_rect().center();
    click(&mut s, analyse);
    assert!(s.core().masking_running());
    s.core_mut().wait_background();
    s.frame();
    s
}

fn dynamic_eqs(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.dynamic-eq")
        .count()
}

#[test]
fn the_mixer_menu_has_the_masking_map() {
    let s = shell();
    let mixer = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Analyze")
        .unwrap();
    let row = mixer
        .items()
        .find(|i| i.command == AppCommand::MaskingMap)
        .expect("the row");
    assert_eq!(row.label, "Masking Map\u{2026}");
    assert!(row.enabled);
    assert_eq!(
        AppCommand::MaskingMap.command_id(),
        Some(("mixer.masking_map", None))
    );
}

#[test]
fn the_panel_opens_empty_and_esc_closes_it() {
    let mut s = shell();
    assert!(!s.masking_open());
    s.execute(AppCommand::MaskingMap);
    assert!(s.masking_open());
    let view = s.masking_view().unwrap();
    assert!(view.data().is_none() && !view.busy());
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.masking_open());
}

#[test]
fn analysing_a_song_of_one_channel_says_why_not() {
    let mut s = shell();
    s.execute(AppCommand::MaskingMap);
    let analyse = s.masking_view().unwrap().analyse_rect().center();
    click(&mut s, analyse);
    assert!(!s.core().masking_running());
    assert!(
        s.masking_view().unwrap().data().is_none(),
        "nothing to show"
    );
    assert!(s.core().last_rejection().is_some_and(|m| m.contains("two")));
}

#[test]
fn analyse_ranks_the_pair_and_the_panel_shows_it() {
    let s = analysed();
    let view = s.masking_view().unwrap();
    let data = view.data().expect("a result");
    assert_eq!(data.tracks, 2);
    let top = &data.pairs[0];
    assert!(top.score > 30.0);
    assert!(top.zone[top.worst]);
    assert!(!top.ranges.is_empty());
    assert_eq!(view.band(), Some(top.worst));
    assert!(!view.busy());
    // The song is untouched.
    assert_eq!(dynamic_eqs(&s), 0);
}

#[test]
fn clicking_a_band_chooses_it_and_de_mask_is_one_undo_step() {
    let mut s = analysed();
    let before = s.core().doc().clone();
    let worst = s.masking_view().unwrap().data().unwrap().pairs[0].worst;
    let other = (worst + 1).min(atomik_dsp::masking::BANDS - 1);
    let slot = s.masking_view().unwrap().band_rect(other).center();
    click(&mut s, slot);
    assert_eq!(s.masking_view().unwrap().band(), Some(other));
    let button = s.masking_view().unwrap().demask_rect().center();
    click(&mut s, button);
    assert_eq!(dynamic_eqs(&s), 1);
    let note = s.core().last_rejection().unwrap_or_default().to_owned();
    assert!(note.starts_with("De-masked"), "{note}");
    assert!(s.core_mut().undo());
    assert_eq!(*s.core().doc(), before);
}

#[test]
fn a_time_range_locates_the_playhead_and_the_list_keys_choose_pairs() {
    let mut s = analysed();
    let range = s.masking_view().unwrap().range_rect(0).center();
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(0));
    click(&mut s, range);
    // The clash starts with the first block: at the start of the song.
    assert!(s.core().position() <= BAR);
    s.handle(&UiEvent::Key {
        key: Key::Down,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    let pairs = s.masking_view().unwrap().data().unwrap().pairs.len();
    assert_eq!(s.masking_view().unwrap().pair(), 1.min(pairs - 1));
}

#[test]
fn the_panel_paints_and_answers_a_tooltip() {
    let mut s = analysed();
    let r = s.masking_view().unwrap().demask_rect().center();
    assert!(s.masking_view().unwrap().help_at(r).is_some());
    s.handle(&UiEvent::PointerMove {
        pos: r,
        modifiers: Modifiers::NONE,
    });
    assert!(s.needs_repaint());
}
