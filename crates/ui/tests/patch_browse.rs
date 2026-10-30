//! The file browser aimed at a device (`set_patch_target`): the band that names the device, the
//! list that keeps folders and the patches for the device's type, the audition on select, and
//! what Enter, a double-click and Escape ask for.

use atomik_library::{AudioFormat, AudioInfo, FileKind, FileRow, ItemInfo};
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::{
    BrowseAction, DisplayList, FileBrowserView, Key, Modifiers, PointerButton, Theme, UiAction,
    UiEvent, action::Place,
};
use std::path::PathBuf;

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";

// A 300 x 600 panel with the band: banner 0..16, header 16..50, tree 50..(50 + 0.4 * 550),
// then the splitter, the path bar and the column header (6 + 22 + 20) before the rows.
const BANNER: f64 = 16.0;
const LIST_Y: f64 = BANNER + 34.0 + 550.0 * 0.4 + 6.0 + 22.0 + 20.0;

fn row_pt(i: usize) -> Point {
    Point::new(80.0, LIST_Y + 22.0 * i as f64 + 11.0)
}

fn row(name: &str, kind: FileKind, device_type: Option<&str>) -> FileRow {
    FileRow {
        path: PathBuf::from(format!("/lib/{name}")),
        name: name.to_string(),
        kind,
        size: 100,
        info: matches!(kind, FileKind::Audio(_)).then_some(AudioInfo {
            sample_rate: 44100,
            channels: 2,
            frames: Some(100),
        }),
        item: device_type.map(|t| ItemInfo {
            device_type: t.to_string(),
            ..ItemInfo::default()
        }),
        favorite: false,
        tags: Vec::new(),
    }
}

fn rows() -> Vec<FileRow> {
    vec![
        row("Warm Pad.atomikpatch", FileKind::Patch, Some(SYNTH)),
        row("Echo.atomikpatch", FileKind::Patch, Some(DELAY)),
        row("Kit.atomikpatch", FileKind::Patch, Some("")),
        row("Unknown.atomikpatch", FileKind::Patch, None),
        row("kick.wav", FileKind::Audio(AudioFormat::Wav), None),
        row("more", FileKind::Folder, None),
    ]
}

fn aimed() -> FileBrowserView {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_location(Place::Folder(PathBuf::from("/lib")));
    v.set_rows(rows());
    v.set_patch_target(Some(("Lead".to_string(), SYNTH.to_string())));
    v.handle(&UiEvent::Resize(Size::new(300.0, 600.0)));
    v
}

fn texts(v: &mut FileBrowserView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().iter().map(|t| (*t).to_string()).collect()
}

fn has(texts: &[String], what: &str) -> bool {
    texts.iter().any(|t| t.contains(what))
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn down(pos: Point, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    }
}

fn browse(actions: Vec<UiAction>) -> Vec<BrowseAction> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            UiAction::Browse(BrowseAction::RequestInfo(_)) => None,
            UiAction::Browse(b) => Some(b),
            other => panic!("unexpected action {other:?}"),
        })
        .collect()
}

#[test]
fn a_band_names_the_device_and_the_list_keeps_folders_and_the_patches_for_its_type() {
    let mut v = aimed();
    assert_eq!(v.patch_target(), Some(SYNTH));
    let t = texts(&mut v);
    assert!(has(&t, "PATCHES FOR LEAD"), "{t:?}");
    assert!(has(&t, "Warm Pad"), "{t:?}");
    assert!(has(&t, "Unknown"), "a patch not probed yet stays: {t:?}");
    assert!(has(&t, "more"), "folders stay: {t:?}");
    assert!(
        !has(&t, "Echo"),
        "a patch for another type is hidden: {t:?}"
    );
    assert!(
        !has(&t, "Kit"),
        "a patch of several devices is hidden: {t:?}"
    );
    assert!(!has(&t, "kick"), "audio is hidden: {t:?}");

    // The probe of the unknown patch says it is for another type: it goes.
    v.patch_item_info(&[(
        PathBuf::from("/lib/Unknown.atomikpatch"),
        ItemInfo {
            device_type: DELAY.to_string(),
            ..ItemInfo::default()
        },
    )]);
    assert!(!has(&texts(&mut v), "Unknown"));

    // Released, everything is listed again and the band is gone.
    v.set_patch_target(None);
    let t = texts(&mut v);
    assert!(has(&t, "Echo") && has(&t, "kick"), "{t:?}");
    assert!(!has(&t, "PATCHES FOR"));
}

#[test]
fn selecting_a_patch_auditions_it_and_opening_it_loads_it() {
    let mut v = aimed();
    // A click on the patch row.
    let mut warm = None;
    for i in 0..4 {
        let acts = browse(v.handle(&down(row_pt(i), 1)));
        if let [BrowseAction::Audition(p)] = acts.as_slice()
            && p.ends_with("Warm Pad.atomikpatch")
        {
            warm = Some(i);
            break;
        }
    }
    let i = warm.expect("a click on the patch auditions it");
    // A double-click opens it.
    let acts = browse(v.handle(&down(row_pt(i), 2)));
    assert!(
        matches!(acts.as_slice(), [BrowseAction::Load(p)] if p.ends_with("Warm Pad.atomikpatch")),
        "{acts:?}"
    );
    // Enter on the cursor row opens it too.
    let acts = browse(v.handle(&key(Key::Enter)));
    assert!(
        matches!(acts.as_slice(), [BrowseAction::Load(p)] if p.ends_with("Warm Pad.atomikpatch")),
        "{acts:?}"
    );
}

#[test]
fn the_arrow_keys_audition_each_patch_they_reach() {
    let mut v = aimed();
    let mut heard = Vec::new();
    for _ in 0..3 {
        for a in browse(v.handle(&key(Key::Down))) {
            if let BrowseAction::Audition(p) = a {
                heard.push(p);
            }
        }
    }
    assert_eq!(
        heard,
        vec![
            PathBuf::from("/lib/Unknown.atomikpatch"),
            PathBuf::from("/lib/Warm Pad.atomikpatch")
        ],
        "each patch row, no folder"
    );
}

#[test]
fn escape_cancels_the_audition_and_stops_the_preview_without_a_target() {
    let mut v = aimed();
    assert_eq!(
        browse(v.handle(&key(Key::Escape))),
        vec![BrowseAction::Cancel]
    );
    v.set_patch_target(None);
    assert_eq!(
        browse(v.handle(&key(Key::Escape))),
        vec![BrowseAction::StopPreview]
    );
    // Without a target a patch row does not audition.
    let acts = browse(v.handle(&key(Key::Down)));
    assert!(
        !acts.iter().any(|a| matches!(a, BrowseAction::Audition(_))),
        "{acts:?}"
    );
}
