//! The arrangement rows of an automation track (the track of a console channel or an effect: no
//! note lane, automation lanes alone): the header has a mute button and the first lane's
//! controls, the first lane's clips sit in the header row, and the track collapses like any
//! other. `cargo test -p atomik-ui --test automation_track -- --ignored` writes a preview PNG to
//! `target/preview`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipId, Command, DeviceKey, Document, SongCommand};
use atomik_ui::arrangement::TRACK_LIST_WIDTH;
use atomik_ui::{
    ArrangementView, DisplayList, LaneButton, PointerButton, TextEngine, TrackButton, UiAction,
    UiEvent,
};
use std::path::PathBuf;
use support::cpu::CpuCanvas;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const MIX: &str = "mix";
const FEEDBACK: &str = "feedback";

/// The song of the other arrangement tests with an automation track on its `delay`: a `mix`
/// lane holding one clip (id 9) and a `feedback` lane.
fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    for c in [
        SongCommand::AddAutomationLane {
            device: key("delay"),
            param: MIX.into(),
        },
        SongCommand::AddClip {
            device: key("delay"),
            lane: 0,
            clip: auto_clip(
                0,
                BAR,
                2 * BAR,
                vec![
                    ap(0, 0.2, atomik_model::Curve::Linear),
                    ap(BAR, 0.9, atomik_model::Curve::Linear),
                ],
            ),
        },
        SongCommand::AddAutomationLane {
            device: key("delay"),
            param: FEEDBACK.into(),
        },
    ] {
        d.apply(Command::Song(c), &reg).unwrap();
    }
    let v = arrangement(d.doc(), &reg);
    (d, reg, v)
}

fn click(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    p: atomik_ui::kurbo::Point,
) -> Vec<UiAction> {
    run(|e| v.handle(e, d.doc(), reg), &[down(p), up(p)])
}

fn clip_id(d: &Document) -> ClipId {
    d.doc().song.track(&delay()).unwrap().lanes[0].clips[0].id
}

fn delay() -> DeviceKey {
    key("delay")
}

#[test]
fn the_header_has_a_mute_button_and_a_lane_picker_but_no_solo_or_arm() {
    let (d, reg, mut v) = setup();
    assert!(
        v.track_button_center(&delay(), TrackButton::Expand)
            .is_some()
    );
    for b in [TrackButton::Solo, TrackButton::Arm] {
        assert!(v.track_button_center(&delay(), b).is_none(), "{b:?}");
    }
    // The picker lists the params with a check mark on the ones that have a lane; clicking the
    // lane that is the track's only one removes the lane and so the track.
    let picker = v
        .track_button_center(&delay(), TrackButton::AddLane)
        .unwrap();
    let a = click(&d, &reg, &mut v, picker);
    assert!(a.is_empty());
    let items = v.param_menu_items();
    let at = items.iter().position(|k| k == MIX).unwrap();
    assert!(v.param_menu_checked()[at]);
    let row = v.param_menu_row_center(at).unwrap();
    let a = click(&d, &reg, &mut v, row);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveAutomationLane {
            device: delay(),
            param: MIX.into(),
        }]
    );
    let p = v.track_button_center(&delay(), TrackButton::Mute).unwrap();
    let a = click(&d, &reg, &mut v, p);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetTrackFlags {
            device: delay(),
            muted: true,
            solo: false,
            armed: false,
        }]
    );
}

#[test]
fn the_first_lane_has_its_mode_and_arm_buttons_in_the_header() {
    let (d, reg, mut v) = setup();
    let arm = v
        .lane_button_center(&delay(), MIX, LaneButton::Arm)
        .unwrap();
    let a = click(&d, &reg, &mut v, arm);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::ToggleLaneArm { device, param }
            if *device == delay() && param == MIX)),
        "{a:?}"
    );
    let mode = v
        .lane_button_center(&delay(), MIX, LaneButton::Mode)
        .unwrap();
    let a = click(&d, &reg, &mut v, mode);
    assert!(
        matches!(song_commands(&a).as_slice(), [SongCommand::SetLaneWriteMode { device, param, .. }]
            if *device == delay() && param == MIX),
        "{a:?}"
    );
    // The two buttons do not overlap the mute button.
    let mute = v.track_button_center(&delay(), TrackButton::Mute).unwrap();
    assert!(arm.x > mute.x && mode.x > mute.x);
    assert!(arm.x < TRACK_LIST_WIDTH && mode.x < TRACK_LIST_WIDTH);
}

#[test]
fn the_first_lane_shares_the_header_row_and_its_clip_is_in_it() {
    let (d, _, v) = setup();
    let lane0 = v.lane_rect(&delay(), 0).unwrap();
    let lane1 = v.lane_rect(&delay(), 1).unwrap();
    assert!(lane0.y1 <= lane1.y0 + 1e-9);
    let clip = v.clip_rect(clip_id(&d)).unwrap();
    assert!(clip.y0 >= lane0.y0 && clip.y1 <= lane0.y1);
    // The other lane is a short automation row, as on any track.
    assert!(lane1.height() < lane0.height());
}

#[test]
fn dragging_a_point_of_the_first_lane_edits_the_automation() {
    let (d, reg, mut v) = setup();
    let p = v.automation_point_pos(clip_id(&d), 0).unwrap();
    let to = atomik_ui::kurbo::Point::new(p.x, p.y + 10.0);
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), mv(to), up(to)]);
    assert!(
        song_commands(&a)
            .iter()
            .any(|c| matches!(c, SongCommand::SetClipContent { .. })),
        "{a:?}"
    );
}

#[test]
fn collapsing_keeps_the_first_lane_and_hides_the_others() {
    let (d, reg, mut v) = setup();
    let p = v
        .track_button_center(&delay(), TrackButton::Expand)
        .unwrap();
    click(&d, &reg, &mut v, p);
    assert!(v.lane_rect(&delay(), 0).is_some());
    assert!(v.lane_rect(&delay(), 1).is_none());
    assert!(v.clip_rect(clip_id(&d)).is_some());
}

#[test]
fn a_folded_first_lane_is_still_a_whole_header_row() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetLaneFolded {
            device: delay(),
            param: MIX.into(),
            folded: true,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.track_button_center(&delay(), TrackButton::Mute).is_some());
    let lane0 = v.lane_rect(&delay(), 0).unwrap();
    assert!(lane0.height() >= 40.0, "{lane0:?}");
}

#[test]
fn a_right_click_on_the_header_asks_for_the_track_menu() {
    let (d, reg, mut v) = setup();
    let p = v.track_header_center(&delay()).unwrap();
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down_with(p, PointerButton::Secondary, 1), up(p)],
    );
    assert!(
        a.iter().any(|a| matches!(
            a,
            UiAction::ContextMenu {
                target: atomik_ui::ContextTarget::TrackHeader,
                ..
            }
        )),
        "{a:?}"
    );
}

// ---- golden ----------------------------------------------------------------------------

fn record() -> DisplayList {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetTrackFlags {
            device: delay(),
            muted: true,
            solo: false,
            armed: false,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    v.set_selected_track(Some(delay()));
    v.set_playhead(BAR + PPQ);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    dl
}

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

#[test]
fn the_arrangement_with_an_automation_track_matches_its_golden() {
    check("arrangement_automation_track_1x.json", &record().to_json());
}

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    let (d, reg, mut v) = setup();
    let mut text = TextEngine::new();
    v.set_scale(1.5);
    v.set_focused(true);
    v.set_selected_track(Some(delay()));
    let (w, h) = (
        (ARR_BOUNDS.width() * 1.5) as u16,
        (ARR_BOUNDS.height() * 1.5) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/arrangement_automation_track.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

/// Keeps the `UiEvent` import for the helpers above.
#[test]
fn events_are_plain_values() {
    let _ = UiEvent::Resize(atomik_ui::kurbo::Size::new(1.0, 1.0));
}

#[test]
fn folder_solo_and_arm_leave_an_automation_track_alone_and_mute_includes_it() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::GroupTracks {
            devices: vec![key("bass"), delay()],
            name: "Mix".into(),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    for (button, check) in [
        (
            atomik_ui::arrangement::FolderButton::Arm,
            (|t: &atomik_model::Track| t.armed) as fn(&atomik_model::Track) -> bool,
        ),
        (atomik_ui::arrangement::FolderButton::Solo, |t| t.solo),
        (atomik_ui::arrangement::FolderButton::Mute, |t| t.muted),
    ] {
        let p = v.folder_button_center(1, button).unwrap();
        let a = click(&d, &reg, &mut v, p);
        for c in commands(&a) {
            d.apply(c, &reg)
                .unwrap_or_else(|e| panic!("{button:?}: {e}"));
        }
        v.sync(d.doc(), &reg);
        let song = &d.doc().song;
        assert!(check(song.track(&key("bass")).unwrap()), "{button:?}");
        assert_eq!(
            check(song.track(&delay()).unwrap()),
            button == atomik_ui::arrangement::FolderButton::Mute,
            "{button:?}"
        );
    }
}

#[test]
fn the_pencil_on_the_first_lane_makes_an_automation_clip_there() {
    let (d, reg, mut v) = setup();
    v.set_tool(atomik_ui::Tool::Pencil);
    v.set_snap(atomik_ui::timeline::Snap::Bar);
    let lane = v.lane_rect(&delay(), 0).unwrap();
    let y = lane.center().y;
    let (x0, x1) = (v.tick_to_x(5 * BAR + 10), v.tick_to_x(6 * BAR + 10));
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[
            down(atomik_ui::kurbo::Point::new(x0, y)),
            mv(atomik_ui::kurbo::Point::new(x1, y)),
            up(atomik_ui::kurbo::Point::new(x1, y)),
        ],
    );
    let cmds = song_commands(&a);
    let [SongCommand::AddClip { device, lane, clip }] = cmds.as_slice() else {
        panic!("{cmds:?}")
    };
    assert_eq!((device, *lane), (&delay(), 0));
    assert_eq!(
        clip.content,
        atomik_model::ClipContent::Automation(Vec::new())
    );
}
