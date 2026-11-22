//! An idle frame of the shell allocates nothing: with no input, no panel open and nothing
//! animating, the per-frame passes only compare what they already hold. Each of them used to
//! build a list, a string or a map entry before it found there was nothing to do (the MIDI
//! mapping rows for a panel that is not open, the plugin device list, the pinned plugin params,
//! the list of open menus, the inputs listening offers).

use atomik_app::AppCore;
use atomik_app::session::{LiveSession, NullBackend};
use atomik_app::settings::AudioSettings;
use atomik_app::shell::{Shell, ShellOptions};
use atomik_model::{
    Command, MapOptions, MidiMapCommand, MidiMapping, MidiSource, MidiTarget, SourceKind,
};
use atomik_ui::UiEvent;
use atomik_ui::kurbo::Size;

atomik_testkit::install_alloc_guard!();

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
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

/// Frames at the idle rate (ten a second): a warm-up that fills the caches, then the measured
/// stretch, which is long enough to contain every once-a-second pass.
fn idle_allocations(s: &mut Shell<NullBackend>) -> u64 {
    for i in 0..30 {
        s.frame_at(f64::from(i) * 0.1);
    }
    let (_, n) = atomik_testkit::no_alloc::allocations_of(|| {
        for i in 30..60 {
            s.frame_at(f64::from(i) * 0.1);
        }
    });
    n
}

#[test]
fn an_idle_frame_allocates_nothing() {
    assert_eq!(idle_allocations(&mut shell()), 0);
}

#[test]
fn an_idle_frame_with_a_patch_open_allocates_nothing() {
    let mut s = shell();
    s.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    )));
    assert!(!s.core().doc().devices.is_empty(), "the patch opened");
    assert_eq!(idle_allocations(&mut s), 0);
}

#[test]
fn an_idle_frame_with_midi_mappings_and_no_panel_allocates_nothing() {
    let mut s = shell();
    s.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    )));
    let synth = s.core().doc().devices[0].key.clone();
    for cc in 1..=8u8 {
        let mapping = MidiMapping {
            id: 0,
            source: MidiSource {
                port: None,
                channel: None,
                kind: SourceKind::Cc(cc),
                surface: None,
            },
            target: MidiTarget::Param {
                device: synth.clone(),
                param: "filter.cutoff".into(),
            },
            options: MapOptions::default(),
        };
        s.core_mut()
            .apply_command(Command::MidiMap(MidiMapCommand::Add(mapping)))
            .unwrap();
    }
    assert_eq!(s.core().doc().midi_maps.len(), 8);
    assert!(!s.midi_maps_open(), "the panel is closed");
    assert_eq!(idle_allocations(&mut s), 0);
}
