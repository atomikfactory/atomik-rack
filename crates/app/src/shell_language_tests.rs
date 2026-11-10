//! The language of the interface in a headless `Shell`: the preference decides the language, the
//! menus and the views change with it at once, English is untouched, and nothing here reads the
//! language of the machine (each test scopes its language to its own thread).

use super::*;
use crate::session::NullBackend;
use atomik_ui::i18n;
use atomik_ui::{DisplayList, Lang, LanguageChoice};

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 800.0)));
    s
}

fn painted(s: &mut Shell<NullBackend>) -> String {
    let mut dl = DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    dl.to_json()
}

fn titles(s: &Shell<NullBackend>) -> Vec<&'static str> {
    s.menu_model().iter().map(|m| m.title).collect()
}

#[test]
fn the_language_follows_the_preference() {
    let mut s = shell();
    // The test machine's language is never read: with no locale given, System is English.
    assert_eq!(s.workflow.language, LanguageChoice::System);
    assert_eq!(s.language(), Lang::English);
    s.workflow.language = LanguageChoice::Is(Lang::French);
    assert_eq!(s.language(), Lang::French);
    s.workflow.language = LanguageChoice::Is(Lang::English);
    assert_eq!(s.language(), Lang::English);
}

#[test]
fn english_menus_and_views_are_unchanged() {
    let mut s = shell();
    assert_eq!(titles(&s)[..3], ["File", "Edit", "View"]);
    let english = painted(&mut s);
    assert!(english.contains("\"text\":\"File\""));
    assert!(!english.contains("Fichier"));
}

#[test]
fn french_changes_the_menus_and_the_views_at_once() {
    let mut s = shell();
    let english = painted(&mut s);
    i18n::with_language(Lang::French, || {
        // The next frame notices the change and hands the views a fresh theme, so the cached
        // drawing of the English words is dropped.
        s.frame();
        assert_eq!(titles(&s)[..3], ["Fichier", "Édition", "Affichage"]);
        let french = painted(&mut s);
        assert!(french.contains("\"text\":\"Fichier\""), "the bar is French");
        assert_ne!(french, english);
        assert!(!french.contains("\"text\":\"File\""));
    });
    // Back to English: everything draws as before, caches included.
    s.frame();
    assert_eq!(painted(&mut s), english);
}

#[test]
fn the_names_of_songs_and_tracks_in_the_menus_are_never_translated() {
    // The owned texts of a menu (recent songs, templates, tracks, presets) are the same in
    // every language: only the fixed words change.
    let owned = |model: &[Menu]| -> Vec<String> {
        model
            .iter()
            .flat_map(Menu::items)
            .filter_map(|i| match &i.label {
                std::borrow::Cow::Owned(name) => Some(name.clone()),
                std::borrow::Cow::Borrowed(_) => None,
            })
            .collect()
    };
    let english = owned(&shell().menu_model());
    let french = i18n::with_language(Lang::French, || owned(&shell().menu_model()));
    assert_eq!(english, french);
}

#[test]
fn the_shortcuts_of_the_menus_use_the_french_key_names() {
    let model = i18n::with_language(Lang::French, || shell().menu_model());
    let save_as = model
        .iter()
        .flat_map(Menu::items)
        .find(|i| i.label == "Enregistrer sous…")
        .expect("File > Save As in French");
    assert_eq!(save_as.shortcut.as_deref(), Some("Ctrl+Maj+S"));
}
