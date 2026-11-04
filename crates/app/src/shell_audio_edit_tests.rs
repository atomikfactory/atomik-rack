//! `Shell` audio editing tests: the audio items of the Edit and Audio menus, their command ids,
//! Reverse, Normalise and the Strip Silence panel (opening, settings kept between openings,
//! APPLY as one undo step, Esc).

use super::*;
use crate::commands::{GainStep, MenuEntry};
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{ClipContent, ClipId, FadeCurve};
use atomik_ui::{ImportTarget, SilenceRow};

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

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

/// Three bursts of 0.5 in silence: 48 000 frames, mono.
fn imported(s: &mut Shell<NullBackend>) -> ClipId {
    // One folder per call: the tests run in parallel and each removes its own.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-audio-edit-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..48_000)
        .map(|i| {
            let loud = (1_000..9_000).contains(&i)
                || (20_000..30_000).contains(&i)
                || (40_000..46_000).contains(&i);
            if loud { 0.25 } else { 0.0 }
        })
        .collect();
    let path = dir.join("bursts.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    s.core_mut()
        .import_audio(vec![path], ImportTarget::NewTracks, 0);
    s.core_mut().wait_background();
    let _ = std::fs::remove_dir_all(&dir);
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .find(|c| c.content.is_audio())
        .map(|c| c.id)
        .expect("an audio clip")
}

fn item(s: &Shell<NullBackend>, menu: &str, label: &str) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .find(|m| m.title == menu)
        .unwrap()
        .items()
        .find(|i| i.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("{menu} > {label}"))
}

fn audio(s: &Shell<NullBackend>, id: ClipId) -> atomik_model::AudioClipContent {
    match &s.core().doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .map(|l| l.clips.len())
        .sum()
}

#[test]
fn the_audio_items_follow_the_selection_and_carry_their_registry_ids() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    for (menu, label) in [
        ("Edit", "Reverse Audio"),
        ("Edit", "Normalise Audio"),
        ("Edit", "Strip Silence\u{2026}"),
        ("Audio", "Clip Gain: Up 1 dB"),
        ("Audio", "Fades: 5 ms Declick"),
        ("Audio", "Fade Shape: S-curve"),
        ("Audio", "Crossfade: Off"),
    ] {
        assert!(!item(&s, menu, label).enabled, "{label} needs a clip");
    }
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let reverse = item(&s, "Edit", "Reverse Audio");
    assert!(reverse.enabled);
    assert_eq!(reverse.command, AppCommand::AudioReverse);
    assert_eq!(
        reverse.command.command_id(),
        Some(("sequencer.audio_reverse", None))
    );
    assert_eq!(
        item(&s, "Audio", "Fade Shape: S-curve").command,
        AppCommand::AudioFadeShape(FadeCurve::SCurve)
    );
    assert_eq!(
        AppCommand::from_command("sequencer.audio_crossfade", Some("off")),
        Some(AppCommand::AudioCrossfade(None))
    );
    assert_eq!(
        AppCommand::from_command("sequencer.audio_gain", Some("down")),
        Some(AppCommand::AudioGain(GainStep::Down))
    );
    assert!(
        !item(&s, "Audio", "Clear Gain Envelope").enabled,
        "no envelope yet"
    );
}

#[test]
fn reverse_and_the_gain_steps_edit_the_selected_clip_and_undo() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    let before = s.core().doc().clone();
    assert!(s.execute(AppCommand::AudioReverse).is_empty());
    assert!(audio(&s, id).reverse);
    assert!(s.execute(AppCommand::AudioGain(GainStep::Up)).is_empty());
    assert!((audio(&s, id).gain_db - 1.0).abs() < 1e-6);
    s.execute(AppCommand::AudioFadeShape(FadeCurve::Exponential));
    assert_eq!(audio(&s, id).fade_curve, FadeCurve::Exponential);
    s.execute(AppCommand::AudioCrossfade(None));
    assert_eq!(audio(&s, id).xfade, None);
    for _ in 0..4 {
        s.core_mut().undo();
    }
    assert_eq!(s.core().doc(), &before);
    // The menu item is checked by the selection: reversing again turns it back.
    s.execute(AppCommand::AudioReverse);
    s.execute(AppCommand::AudioReverse);
    assert!(!audio(&s, id).reverse);
}

#[test]
fn normalise_from_the_edit_menu_sets_the_gain() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(s.execute(AppCommand::AudioNormalise).is_empty());
    // The bursts peak at 0.25: +12.04 dB brings them to full scale.
    let db = audio(&s, id).gain_db;
    assert!((db - 12.0412).abs() < 1e-3, "{db}");
    s.core_mut().undo();
    assert_eq!(audio(&s, id).gain_db, 0.0);
}

#[test]
fn the_strip_silence_panel_opens_keeps_its_settings_and_applies_in_one_step() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(!s.silence_open());
    assert!(s.execute(AppCommand::AudioStripSilence).is_empty());
    assert!(s.silence_open());
    let view = s.silence_view().unwrap();
    // The selection feeds the panel; APPLY works.
    let apply = view.apply_rect().center();
    // The defaults keep 10 ms around each sound and cut silences from 100 ms: the 1000-frame
    // lead-in and the 2000-frame tail are shorter than 100 ms (4800 frames), the two gaps of
    // 11000 and 10000 frames are not.
    let before = s.core().doc().clone();
    click(&mut s, apply);
    assert_eq!(clip_count(&s), 3, "two silences cut out of one clip");
    assert!(s.silence_open(), "the panel stays for another go");
    s.core_mut().undo();
    assert_eq!(s.core().doc(), &before);
    // A changed value is kept for the next opening.
    let plus = s
        .silence_view()
        .unwrap()
        .plus_rect(SilenceRow::Threshold)
        .center();
    click(&mut s, plus);
    assert_eq!(s.silence_values().threshold_db, -39);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.silence_open());
    s.execute(AppCommand::AudioStripSilence);
    assert_eq!(s.silence_view().unwrap().values().threshold_db, -39);
    // CLOSE closes it too.
    let close = s.silence_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.silence_open());
}

#[test]
fn the_panel_cannot_apply_without_an_audio_clip_and_paints() {
    let mut s = shell();
    let _ = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.execute(AppCommand::AudioStripSilence);
    let apply = s.silence_view().unwrap().apply_rect().center();
    let before = s.core().doc().clone();
    click(&mut s, apply);
    assert_eq!(s.core().doc(), &before);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"STRIP SILENCE"));
    let _ = MenuEntry::Separator;
}

#[test]
fn reverse_on_a_warped_clip_turns_the_warp_off_and_reverses() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    s.execute(AppCommand::AudioWarp);
    assert!(audio(&s, id).warp.enabled, "warp is on");
    s.execute(AppCommand::AudioReverse);
    let a = audio(&s, id);
    assert!(a.reverse, "the clip is reversed");
    assert!(!a.warp.active(), "and no longer warped");
    s.core_mut().undo();
    let a = audio(&s, id);
    assert!(!a.reverse && a.warp.enabled, "one undo restores both");
}

// ------------------------------------------------------------------ restructuring follow-ups

fn clip_length(s: &Shell<NullBackend>, id: ClipId) -> i64 {
    s.core().doc().song.clip(id).unwrap().2.length
}

fn type_chars(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        s.handle(&UiEvent::Key {
            key: Key::Char(c),
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
}

fn press_key(s: &mut Shell<NullBackend>, key: Key) {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

#[test]
fn scale_tempo_has_an_other_item_that_opens_the_percent_panel() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let edit = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Edit")
        .unwrap();
    let scale = edit
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu(sub) if sub.label == "Scale Tempo" => Some(sub),
            _ => None,
        })
        .expect("the Scale Tempo submenu");
    let last = scale.entries.last().unwrap();
    let MenuEntry::Item(other) = last else {
        panic!("an item");
    };
    assert_eq!(other.label, "Other\u{2026}");
    assert_eq!(
        other.command.command_id(),
        Some(("sequencer.scale_tempo", Some("dialog")))
    );
    assert_eq!(
        AppCommand::from_command("sequencer.scale_tempo", Some("dialog")),
        Some(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other))
    );
    assert!(!s.scale_open());
    assert!(s.execute(other.command).is_empty());
    assert!(s.scale_open());
}

#[test]
fn the_percent_panel_previews_while_typing_and_applies_in_one_undo_step() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let original = clip_length(&s, id);
    let depth = s.core().document().undo_depth();
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    assert!(s.scale_open());
    assert!(s.wants_text_input());
    // The panel opens on 150 % and the clip already shows it, without an undo step.
    assert_eq!(s.scale_view().unwrap().percent(), Some(150.0));
    assert_eq!(clip_length(&s, id), original * 100 / 150);
    assert_eq!(s.core().document().undo_depth(), depth);
    // Typing replaces the selected text: 2 is no usable value (the clip is as it was), 20 and
    // 200 each preview from the original.
    type_chars(&mut s, "2");
    assert_eq!(clip_length(&s, id), original);
    type_chars(&mut s, "0");
    assert_eq!(clip_length(&s, id), original * 5);
    type_chars(&mut s, "0");
    assert_eq!(clip_length(&s, id), original / 2);
    assert_eq!(s.core().document().undo_depth(), depth);
    // Enter applies as one undo step and closes the panel.
    press_key(&mut s, Key::Enter);
    assert!(!s.scale_open());
    assert_eq!(clip_length(&s, id), original / 2);
    assert_eq!(s.core().document().undo_depth(), depth + 1);
    s.core_mut().undo();
    assert_eq!(clip_length(&s, id), original);
    // The panel opens on the percentage of the last time.
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    assert_eq!(s.scale_view().unwrap().text(), "200");
}

#[test]
fn escape_and_close_put_the_clips_back_and_apply_waits_for_a_usable_value() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let original = clip_length(&s, id);
    let before = s.core().doc().clone();
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    assert_ne!(clip_length(&s, id), original);
    press_key(&mut s, Key::Escape);
    assert!(!s.scale_open());
    assert_eq!(s.core().doc(), &before);
    // CLOSE does the same.
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    let close = s.scale_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.scale_open());
    assert_eq!(s.core().doc(), &before);
    // 100 changes nothing: APPLY and Enter do nothing and the panel stays.
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    type_chars(&mut s, "100");
    press_key(&mut s, Key::Enter);
    assert!(s.scale_open());
    assert_eq!(s.core().doc(), &before);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"SCALE TEMPO"));
    press_key(&mut s, Key::Escape);
    // Nothing selected: the panel says so and applies nothing.
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.execute(AppCommand::ScaleTempo(crate::commands::ScaleStep::Other));
    press_key(&mut s, Key::Enter);
    assert!(s.scale_open());
    assert_eq!(s.core().doc(), &before);
}

#[test]
fn a_crossfade_zone_drag_previews_without_an_undo_step_and_commits_as_one() {
    use atomik_core::PPQ;
    use atomik_model::{Command, SongCommand};
    let mut s = shell();
    let first = imported(&mut s);
    let asset = audio(&s, first).asset;
    let track = s
        .core()
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| {
            t.lanes
                .iter()
                .any(|l| l.clips.iter().any(|c| c.id == first))
        })
        .map(|t| t.device.clone())
        .unwrap();
    // One second of audio is two beats: a second clip of the file starts over the end of the
    // first one and plays on from half a beat in.
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddAudioClip {
            device: track,
            start: PPQ,
            length: PPQ + PPQ / 2,
            asset,
            source_offset: 12_000,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: FadeCurve::Linear,
        }))
        .unwrap();
    let later = s.core().document().last_clip_id().unwrap();
    let before = s.core().doc().clone();
    let depth = s.core().document().undo_depth();
    let ends = |s: &Shell<NullBackend>| {
        let song = &s.core().doc().song;
        (
            song.clip(first).unwrap().2.end(),
            song.clip(later).unwrap().2.start,
        )
    };
    assert_eq!(ends(&s), (2 * PPQ, PPQ));
    // The zone moves earlier by a quarter beat: the first clip ends and the second starts
    // sooner by as much, with no undo step until the drag ends.
    let delta = -(PPQ / 4);
    s.core_mut()
        .handle_action(UiAction::CrossfadeStep { later, delta });
    assert_eq!(ends(&s), (2 * PPQ + delta, PPQ + delta));
    assert_eq!(s.core().document().undo_depth(), depth);
    // Another step starts again from the original.
    s.core_mut().handle_action(UiAction::CrossfadeStep {
        later,
        delta: delta / 2,
    });
    assert_eq!(ends(&s), (2 * PPQ + delta / 2, PPQ + delta / 2));
    s.core_mut()
        .handle_action(UiAction::CrossfadeEnd { commit: false });
    assert_eq!(s.core().doc(), &before);
    s.core_mut()
        .handle_action(UiAction::CrossfadeStep { later, delta });
    s.core_mut()
        .handle_action(UiAction::CrossfadeEnd { commit: true });
    assert_eq!(s.core().document().undo_depth(), depth + 1);
    assert_eq!(ends(&s), (2 * PPQ + delta, PPQ + delta));
    s.core_mut().undo();
    assert_eq!(s.core().doc(), &before);
    // A move the model refuses (the file has no audio before the second clip's start that far)
    // says why and leaves the clips alone.
    s.core_mut().handle_action(UiAction::CrossfadeStep {
        later,
        delta: -PPQ / 2 - 1,
    });
    assert_eq!(s.core().doc(), &before);
    assert!(s.core().last_rejection().is_some());
}

/// A shell with one synth track and a one-bar note clip with two notes (the first is a beat
/// long at tick 0, the second half a beat long at beat 1).
fn note_clip_shell() -> Shell<NullBackend> {
    use atomik_core::PPQ;
    use atomik_model::{Command, DeviceKey, Note, SongCommand};
    let mut s = shell();
    let key = DeviceKey("a".to_string());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".to_string(),
            key: Some(key.clone()),
        })
        .unwrap();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddTrack {
            device: key.clone(),
        }))
        .unwrap();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddClip {
            device: key,
            lane: 0,
            clip: atomik_model::Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                midi: Default::default(),
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(
                    vec![Note::new(0, PPQ, 60, 0.8), Note::new(PPQ, PPQ / 2, 62, 0.8)].into(),
                ),
            },
        }))
        .unwrap();
    s.core_mut().after_view_change();
    s.set_focus(Pane::Arrangement);
    s
}

fn first_note_tick(s: &Shell<NullBackend>) -> i64 {
    let clip = s.core().doc().song.tracks[0].lanes[0].clips[0].clone();
    let ClipContent::Notes(notes) = &clip.content else {
        panic!("notes");
    };
    notes.iter().map(|n| n.tick).min().unwrap()
}

#[test]
fn reverse_clips_follows_the_reverse_mode_preference() {
    use crate::keymap_store::ReverseMode;
    use atomik_core::PPQ;
    let item_command = |s: &Shell<NullBackend>| item(s, "Edit", "Reverse Clips").command;
    let mut s = note_clip_shell();
    let id = s.core().doc().song.tracks[0].lanes[0].clips[0].id;
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert_eq!(item_command(&s), AppCommand::ReverseClipsDefault);
    assert_eq!(
        item_command(&s).command_id(),
        Some(("sequencer.reverse_clips", Some("default")))
    );
    assert_eq!(
        AppCommand::from_command("sequencer.reverse_clips", None),
        Some(AppCommand::ReverseClipsDefault)
    );
    // By default notes mirror as bars: the half-beat note at beat 1 now starts at beat 2.5.
    s.execute(AppCommand::ReverseClipsDefault);
    assert_eq!(first_note_tick(&s), PPQ * 5 / 2);
    s.core_mut().undo();
    // The preference makes the same command mirror only the starts: the first stays first.
    let mut prefs = s.workflow().clone();
    prefs.reverse_mode = ReverseMode::Starts;
    let user = s.user_keymap.clone();
    s.apply_preferences(Some((user, prefs)));
    s.execute(AppCommand::ReverseClipsDefault);
    assert_eq!(first_note_tick(&s), 0);
    s.core_mut().undo();
    // The explicit items do not follow it.
    s.execute(AppCommand::ReverseClips(true));
    assert_eq!(first_note_tick(&s), PPQ * 5 / 2);
}
