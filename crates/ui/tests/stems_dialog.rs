//! The Separate Stems dialog: the options, the consent to download the models, the progress and a
//! failure. The view is pure: it draws a state it is given. Each state's display list matches its
//! golden, in English here and in French in `french_goldens.rs`.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, Modifiers, StemsDialogChoice, StemsDialogState, StemsDialogView, StemsPhase,
    Theme, UiAction, UiEvent,
};
use support::*;

const WINDOW: (f64, f64) = (900.0, 600.0);

fn options() -> StemsDialogState {
    StemsDialogState::Options {
        clip: "Verse Guitar".into(),
        stems: [true, true, false, true],
        mute_original: true,
    }
}

/// The states the goldens show.
fn states() -> Vec<(&'static str, StemsDialogState)> {
    vec![
        ("options", options()),
        (
            "consent",
            StemsDialogState::Consent {
                source: "zenodo.org".into(),
                bytes: 142_551_184,
            },
        ),
        (
            "downloading",
            StemsDialogState::Downloading {
                received: 50_000_000,
                total: 142_551_184,
            },
        ),
        (
            "working",
            StemsDialogState::Working {
                phase: StemsPhase::Separating,
                progress: 0.42,
            },
        ),
        (
            "failed",
            StemsDialogState::Failed {
                message: "The server answered 500".into(),
                retry: true,
            },
        ),
    ]
}

fn dialog(state: StemsDialogState) -> StemsDialogView {
    StemsDialogView::new(WINDOW, state)
}

fn texts(v: &mut StemsDialogView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut StemsDialogView, p: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out
}

#[test]
fn the_options_list_the_four_stems_the_mute_box_and_the_clip() {
    let mut v = dialog(options());
    let t = texts(&mut v);
    for want in [
        "Separate Stems",
        "Clip: Verse Guitar",
        "Vocals",
        "Drums",
        "Bass",
        "Other",
        "Mute the original clip",
        "Separate",
        "Cancel",
    ] {
        assert!(t.iter().any(|s| s == want), "{want}: {t:?}");
    }
}

#[test]
fn a_press_on_a_box_or_button_asks_the_app_and_nothing_else_does() {
    let mut v = dialog(options());
    let boxes = v.boxes();
    assert_eq!(boxes.len(), 5);
    for (choice, r) in &boxes {
        assert_eq!(
            click(&mut v, r.center()),
            vec![UiAction::StemsDialog(*choice)]
        );
    }
    assert_eq!(boxes[2].0, StemsDialogChoice::Toggle(2));
    assert_eq!(boxes[4].0, StemsDialogChoice::ToggleMute);
    let choices = v.choices();
    let at = |c: StemsDialogChoice| choices.iter().find(|(k, _)| *k == c).unwrap().1.center();
    assert_eq!(
        click(&mut v, at(StemsDialogChoice::Separate)),
        vec![UiAction::StemsDialog(StemsDialogChoice::Separate)]
    );
    assert!(click(&mut v, Point::new(5.0, 5.0)).is_empty());
    let key = |k: Key| UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    assert_eq!(
        v.handle(&key(Key::Enter)),
        vec![UiAction::StemsDialog(StemsDialogChoice::Separate)]
    );
    assert_eq!(
        v.handle(&key(Key::Escape)),
        vec![UiAction::StemsDialog(StemsDialogChoice::Cancel)]
    );
}

#[test]
fn the_consent_says_what_is_downloaded_from_where_and_how_big() {
    let mut v = dialog(states().remove(1).1);
    let t = texts(&mut v);
    assert!(t.iter().any(|s| s == "Download the stem models?"), "{t:?}");
    assert!(
        t.iter()
            .any(|s| s.contains("142.6 MB") && s.contains("zenodo.org")),
        "the size and the place: {t:?}"
    );
    assert!(t.iter().any(|s| s.contains("Nothing about you")), "{t:?}");
    assert_eq!(
        v.choices().iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![StemsDialogChoice::Download, StemsDialogChoice::Cancel]
    );
    assert!(v.boxes().is_empty());
}

#[test]
fn progress_and_failure_offer_cancel_close_and_retry() {
    let mut v = dialog(states().remove(2).1);
    assert!(texts(&mut v).iter().any(|s| s == "50.0 MB of 142.6 MB"));
    assert_eq!(v.choices().len(), 1);
    v.set_state(states().remove(3).1);
    let t = texts(&mut v);
    assert!(t.iter().any(|s| s == "Separating: 42%"), "{t:?}");
    assert_eq!(v.choices()[0].0, StemsDialogChoice::Cancel);
    v.set_state(states().remove(4).1);
    assert_eq!(
        v.choices().iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![StemsDialogChoice::Retry, StemsDialogChoice::Close]
    );
    v.set_state(StemsDialogState::Failed {
        message: "cannot read the file".into(),
        retry: false,
    });
    assert_eq!(
        v.choices().iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![StemsDialogChoice::Close]
    );
}

#[test]
fn every_button_fits_the_card_and_none_overlap() {
    for (_, state) in states() {
        let v = dialog(state);
        let card = v.card();
        let rects: Vec<Rect> = v.choices().into_iter().map(|c| c.1).collect();
        for r in rects.iter().chain(v.boxes().iter().map(|b| &b.1)) {
            assert!(
                r.x0 >= card.x0 && r.x1 <= card.x1 && r.y1 <= card.y1,
                "{r:?}"
            );
        }
        for pair in rects.windows(2) {
            assert!(pair[0].x0 >= pair[1].x1, "{pair:?}");
        }
    }
}

#[test]
fn tooltips_are_short_and_every_control_has_one() {
    let v = dialog(options());
    for (_, r) in v.boxes().into_iter().chain(v.choices()) {
        let help = v.help_at(r.center()).expect("a tooltip");
        assert!(help.text.chars().count() <= 90, "{}", help.text);
    }
}

#[test]
fn the_dialog_in_each_state_matches_its_golden() {
    for (name, state) in states() {
        let mut v = dialog(state);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &Theme::dark());
        starter::check_golden(&format!("stems_dialog_{name}_1x.json"), &dl.to_json());
    }
}
