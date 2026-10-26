//! Library items dropped on the rack (patches, containers, device URIs at a slot) and on the
//! arrangement (note clips on a note lane), from the operating system or from the file
//! browser's own drag: both arrive as `FileHover` / `FileDrop`. Everything else is refused.

mod support;

use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_library::device_uri;
use atomik_model::{Command, Lane, RackDoc, RackSlot, WorkflowCommand};
use atomik_ui::container_panel::ContainerButton;
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::timeline::{Snap, snap_round};
use atomik_ui::{
    ArrangementView, COLUMN_GAP, DisplayList, Modifiers, RackView, UiAction, UiEvent, keymap::Os,
    keymap::Roles,
};
use std::path::PathBuf;
use support::container::*;
use support::song::*;
use support::*;

fn p(name: &str) -> PathBuf {
    PathBuf::from(format!("/lib/{name}"))
}

fn hover(pos: Point, paths: &[PathBuf]) -> UiEvent {
    UiEvent::FileHover {
        pos,
        paths: paths.to_vec(),
    }
}

fn drop_on(pos: Point, paths: &[PathBuf]) -> UiEvent {
    UiEvent::FileDrop {
        pos,
        paths: paths.to_vec(),
    }
}

fn paint_json(v: &mut RackView, doc: &RackDoc, reg: &Registry) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

// ---------------------------------------------------------------------------------- the rack

const WIDE: Size = Size::new(3200.0, 1200.0);

/// Synth and delay in column 0, the mixer alone in column 1.
fn two_columns() -> (RackDoc, Registry, RackView) {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.columns = 2;
    doc.devices[2].column = 1;
    let mut v = RackView::new(atomik_ui::Theme::dark());
    v.set_roles(Roles::defaults(Os::Windows));
    let _ = v.handle(&UiEvent::Resize(WIDE), &doc, &reg);
    v.sync(&doc, &reg);
    (doc, reg, v)
}

fn col_x(c: u16) -> f64 {
    f64::from(c) * (f64::from(RACK_WIDTH) + COLUMN_GAP)
}

fn in_column(v: &RackView, c: u16, y: f64) -> Point {
    v.rack_to_view(Point::new(col_x(c) + 300.0, y))
}

#[test]
fn a_patch_dropped_on_a_column_emits_insert_library_at_the_slot() {
    let (doc, reg, mut v) = two_columns();
    let path = p("Patches/Lead.atomikpatch");
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y1 + 20.0);
    assert!(
        v.handle(&hover(to, std::slice::from_ref(&path)), &doc, &reg)
            .is_empty()
    );
    assert_eq!(
        v.handle(&drop_on(to, std::slice::from_ref(&path)), &doc, &reg),
        vec![UiAction::InsertLibrary {
            path,
            container: None,
            slot: RackSlot {
                column: 1,
                index: 1
            },
        }]
    );
}

#[test]
fn a_drop_between_devices_lands_between_them() {
    let (doc, reg, mut v) = two_columns();
    let path = p("Patches/Lead.atomikpatch");
    let synth = v.device_rect(&key("synth")).unwrap();
    // Just below the synth's middle, above the delay's: index 1 of column 0.
    let to = in_column(&v, 0, synth.y1 - 4.0);
    let a = v.handle(&drop_on(to, std::slice::from_ref(&path)), &doc, &reg);
    assert_eq!(
        a,
        vec![UiAction::InsertLibrary {
            path,
            container: None,
            slot: RackSlot {
                column: 0,
                index: 1
            },
        }]
    );
}

#[test]
fn containers_and_device_uris_take_the_same_path() {
    let (doc, reg, mut v) = two_columns();
    let to = in_column(&v, 0, 4.0);
    for path in [
        p("Containers/Box.atomikcontainer"),
        device_uri("atomik.poly-synth"),
    ] {
        let a = v.handle(&drop_on(to, std::slice::from_ref(&path)), &doc, &reg);
        assert!(
            matches!(
                a.as_slice(),
                [UiAction::InsertLibrary { path: got, container: None, slot }]
                    if *got == path && slot.column == 0
            ),
            "{path:?}: {a:?}"
        );
    }
}

#[test]
fn only_the_first_library_item_of_a_drop_is_inserted() {
    let (doc, reg, mut v) = two_columns();
    let to = in_column(&v, 0, 4.0);
    let paths = [
        p("kick.wav"),
        p("Clips/Riff.atomikclip"),
        p("Patches/A.atomikpatch"),
    ];
    let a = v.handle(&drop_on(to, &paths), &doc, &reg);
    assert!(
        matches!(a.as_slice(), [UiAction::InsertLibrary { path, .. }] if *path == p("Patches/A.atomikpatch")),
        "{a:?}"
    );
}

#[test]
fn a_drop_in_the_new_column_zone_inserts_into_the_new_column() {
    let (doc, reg, mut v) = two_columns();
    let zone = v.rack_to_view(Point::new(col_x(2) + 30.0, 80.0));
    let a = v.handle(&drop_on(zone, &[p("Patches/Lead.atomikpatch")]), &doc, &reg);
    assert!(
        matches!(a.as_slice(), [UiAction::InsertLibrary { slot, .. }] if slot.column == 2),
        "{a:?}"
    );
}

#[test]
fn the_slot_line_shows_while_a_library_item_hovers_and_goes_on_cancel_or_drop() {
    let (doc, reg, mut v) = two_columns();
    let plain = paint_json(&mut v, &doc, &reg);
    let to = in_column(&v, 1, 300.0);
    let item = [p("Patches/Lead.atomikpatch")];
    let _ = v.handle(&hover(to, &item), &doc, &reg);
    assert!(v.needs_repaint());
    let hovering = paint_json(&mut v, &doc, &reg);
    assert_ne!(hovering, plain, "the drop line is painted");
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!(paint_json(&mut v, &doc, &reg), plain);
    let _ = v.handle(&hover(to, &item), &doc, &reg);
    let _ = v.handle(&drop_on(to, &item), &doc, &reg);
    assert_eq!(paint_json(&mut v, &doc, &reg), plain);
    // The line follows the pointer from one column to the other.
    let _ = v.handle(&hover(in_column(&v, 0, 300.0), &item), &doc, &reg);
    assert_ne!(paint_json(&mut v, &doc, &reg), hovering);
}

#[test]
fn files_that_are_not_rack_items_show_no_line_and_insert_nothing() {
    let (doc, reg, mut v) = two_columns();
    let plain = paint_json(&mut v, &doc, &reg);
    let to = in_column(&v, 0, 300.0);
    for paths in [
        vec![p("kick.wav")],
        vec![p("Clips/Riff.atomikclip")],
        vec![p("Projects/Song.atomik")],
        Vec::new(),
    ] {
        assert!(v.handle(&hover(to, &paths), &doc, &reg).is_empty());
        assert_eq!(paint_json(&mut v, &doc, &reg), plain, "{paths:?}");
        assert!(v.handle(&drop_on(to, &paths), &doc, &reg).is_empty());
    }
}

#[test]
fn a_drop_over_the_browser_or_outside_the_rack_is_refused() {
    let (doc, reg, mut v) = two_columns();
    v.set_browser_visible(true);
    let item = [p("Patches/Lead.atomikpatch")];
    let over_browser = v.browser_row_center(0).unwrap();
    assert!(v.handle(&hover(over_browser, &item), &doc, &reg).is_empty());
    assert!(
        v.handle(&drop_on(over_browser, &item), &doc, &reg)
            .is_empty()
    );
    let outside = Point::new(-40.0, -40.0);
    assert!(v.handle(&drop_on(outside, &item), &doc, &reg).is_empty());
}

#[test]
fn a_library_item_over_a_sampler_panel_is_not_a_sample_import() {
    let (doc, reg) = (support::sampler::sampler_doc(Vec::new()), registry());
    let mut v = view(&doc, &reg);
    // The left rack ear of the sampler panel: neither a control nor the plate.
    let on = {
        let r = v.device_rect(&key("sampler")).unwrap();
        v.rack_to_view(Point::new(r.x0 + 6.0, r.center().y))
    };
    let item = [p("Patches/Lead.atomikpatch")];
    let _ = v.handle(&hover(on, &item), &doc, &reg);
    assert_eq!(v.drop_hover(), None, "the sampler does not light up");
    let a = v.handle(&drop_on(on, &item), &doc, &reg);
    assert!(
        matches!(a.as_slice(), [UiAction::InsertLibrary { .. }]),
        "{a:?}"
    );
    // Audio files still import.
    let a = v.handle(&drop_on(on, &[p("kick.wav")]), &doc, &reg);
    assert!(
        matches!(a.as_slice(), [UiAction::ImportSamples { .. }]),
        "{a:?}"
    );
}

#[test]
fn inside_a_container_the_drop_names_the_container() {
    let d = layered();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let open = v
        .container_button_center(&key("layer"), ContainerButton::Open)
        .expect("OPEN button");
    let _ = click(&mut v, d.doc(), open);
    assert!(!v.scope().is_empty());
    let synth = v.device_rect(&key("synth")).unwrap();
    let to = v.rack_to_view(Point::new(300.0, synth.y0 + 4.0));
    let a = v.handle(
        &drop_on(to, &[p("Patches/Lead.atomikpatch")]),
        d.doc(),
        &reg,
    );
    assert!(
        matches!(
            a.as_slice(),
            [UiAction::InsertLibrary { container: Some(c), slot, .. }]
                if *c == key("layer") && slot.column == 0
        ),
        "{a:?}"
    );
}

#[test]
fn inside_a_nested_container_the_drop_names_the_whole_path() {
    let d = nested();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer", "fx"]);
    let _ = v.handle(&UiEvent::Resize(WIDE), d.doc(), &reg);
    v.sync(d.doc(), &reg);
    let deep = v.device_rect(&key("deep")).unwrap();
    let to = v.rack_to_view(Point::new(300.0, deep.y0 + 4.0));
    let a = v.handle(
        &drop_on(to, &[p("Patches/Lead.atomikpatch")]),
        d.doc(),
        &reg,
    );
    assert!(
        matches!(
            a.as_slice(),
            [UiAction::InsertLibrary { container: Some(c), .. }] if *c == key("layer/fx")
        ),
        "{a:?}"
    );
}

// -------------------------------------------------------------- the add actions of a device row

fn desc(reg: &Registry, type_id: &str) -> &'static atomik_device_sdk::DeviceDescriptor {
    reg.descriptor(type_id).expect("a registered type")
}

#[test]
fn add_device_slot_action_for_an_instrument_goes_through_the_creation_chain() {
    let (_doc, reg, v) = two_columns();
    let slot = RackSlot {
        column: 1,
        index: 0,
    };
    let a = v.add_device_slot_action_for(desc(&reg, "atomik.poly-synth"), slot, Modifiers::NONE);
    assert!(
        matches!(
            a,
            UiAction::Command(Command::Workflow(WorkflowCommand::CreateInstrumentAt { slot: s, .. }))
                if s == slot
        ),
        "{a:?}"
    );
}

#[test]
fn add_device_slot_action_for_an_effect_and_a_utility_differ() {
    let (_doc, reg, v) = two_columns();
    let slot = RackSlot {
        column: 0,
        index: 1,
    };
    let fx = v.add_device_slot_action_for(desc(&reg, "atomik.stereo-delay"), slot, Modifiers::NONE);
    assert!(
        matches!(
            fx,
            UiAction::Command(Command::Workflow(WorkflowCommand::CreateEffectAt { .. }))
        ),
        "{fx:?}"
    );
    let util =
        v.add_device_slot_action_for(desc(&reg, "atomik.arpeggiator"), slot, Modifiers::NONE);
    assert!(
        matches!(
            &util,
            UiAction::Command(Command::AddDeviceAt { slot: s, type_id, .. })
                if *s == slot && type_id == "atomik.arpeggiator"
        ),
        "{util:?}"
    );
}

#[test]
fn add_device_action_for_adds_below_the_selection_else_at_the_end() {
    let (doc, reg, mut v) = two_columns();
    let mixer = desc(&reg, "atomik.line-mixer");
    let at_end = v.add_device_action_for(mixer, &doc, Modifiers::NONE);
    assert!(
        matches!(
            at_end,
            UiAction::Command(Command::AddDevice { index, .. }) if index == doc.devices.len()
        ),
        "{at_end:?}"
    );
    v.set_selected(Some(key("synth")));
    let below = v.add_device_action_for(mixer, &doc, Modifiers::NONE);
    assert!(
        matches!(
            below,
            UiAction::Command(Command::AddDevice { index: 1, .. })
        ),
        "{below:?}"
    );
}

#[test]
fn add_device_action_for_a_player_creates_it_on_the_stack_of_the_selection() {
    let (doc, reg, mut v) = two_columns();
    let arp = desc(&reg, "atomik.arpeggiator");
    v.set_selected(Some(key("synth")));
    let a = v.add_device_action_for(arp, &doc, Modifiers::NONE);
    assert!(
        matches!(
            &a,
            UiAction::Command(Command::Workflow(WorkflowCommand::CreatePlayer { type_id, target, options }))
                if type_id == "atomik.arpeggiator"
                    && *target == atomik_model::CreateTarget::Selected(key("synth"))
                    && options.route
        ),
        "{a:?}"
    );
}

// ----------------------------------------------------------------------------- the arrangement

type Reg = Registry;

/// The song with `keys` holding a second note lane (clip 11) and `bass` a note lane before its
/// cutoff lane, on an arrangement tall enough to show every row, snapping to bars.
fn setup() -> (RackDoc, Reg, ArrangementView) {
    let (mut doc, reg) = (song_doc(), registry());
    let keys = doc
        .song
        .tracks
        .iter_mut()
        .find(|t| t.device == key("keys"))
        .unwrap();
    keys.lanes.push(Lane::notes().with_clips(vec![notes_clip(
        11,
        BAR,
        BAR,
        vec![n(0, PPQ, 64, 0.7)],
    )]));
    doc.song.next_clip_id = 12;
    let mut v = arrangement(&doc, &reg);
    v.set_bounds(Rect::new(0.0, 0.0, 1200.0, 800.0));
    v.set_snap(Snap::Bar);
    (doc, reg, v)
}

fn on_lane(v: &ArrangementView, device: &str, lane: usize, x: f64) -> Point {
    Point::new(x, v.lane_rect(&key(device), lane).unwrap().center().y)
}

fn clip_item() -> Vec<PathBuf> {
    vec![p("Clips/Riff.atomikclip")]
}

fn paint_arr(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

#[test]
fn clip_drop_target_at_names_the_note_lane_and_the_snapped_tick() {
    let (_, _, v) = setup();
    let at = on_lane(&v, "keys", 1, 420.0);
    let tick = snap_round(v.x_to_tick(420.0), Some(BAR));
    assert_eq!(v.clip_drop_target_at(at), Some((key("keys"), 1, tick)));
    assert!(tick > 0 && tick % BAR == 0);
    let first = on_lane(&v, "keys", 0, 420.0);
    assert_eq!(v.clip_drop_target_at(first), Some((key("keys"), 0, tick)));
    // Further left than the timeline counts as its left edge.
    let list = Point::new(v.lane_rect(&key("keys"), 1).unwrap().x0 - 20.0, at.y);
    assert_eq!(v.clip_drop_target_at(list), Some((key("keys"), 1, 0)));
}

#[test]
fn clip_drop_target_at_is_none_off_the_note_lanes() {
    let (_, _, v) = setup();
    // The automation lane of the bass, the ruler, and outside the view.
    let cutoff = on_lane(&v, "bass", 1, 420.0);
    assert_eq!(v.clip_drop_target_at(cutoff), None);
    assert_eq!(v.clip_drop_target_at(Point::new(420.0, 5.0)), None);
    assert_eq!(v.clip_drop_target_at(Point::new(-30.0, -30.0)), None);
    assert_eq!(v.clip_drop_target_at(Point::new(420.0, 790.0)), None);
}

#[test]
fn clip_drop_target_at_is_none_on_an_audio_lane() {
    let (doc, reg) = (support::audio::audio_doc(), registry());
    let v = support::audio::audio_arrangement(&doc, &reg);
    let r = v.lane_rect(&key("audio-track"), 0).unwrap();
    assert_eq!(v.clip_drop_target_at(Point::new(420.0, r.center().y)), None);
}

#[test]
fn a_clip_dropped_on_a_note_lane_emits_insert_library_clip() {
    let (doc, reg, mut v) = setup();
    let at = on_lane(&v, "keys", 1, 420.0);
    let tick = snap_round(v.x_to_tick(420.0), Some(BAR));
    assert!(v.handle(&hover(at, &clip_item()), &doc, &reg).is_empty());
    assert_eq!(
        v.handle(&drop_on(at, &clip_item()), &doc, &reg),
        vec![UiAction::InsertLibraryClip {
            path: p("Clips/Riff.atomikclip"),
            track: key("keys"),
            lane: 1,
            tick,
        }]
    );
}

#[test]
fn the_hover_marker_shows_over_a_note_lane_and_goes_on_cancel_or_drop() {
    let (doc, reg, mut v) = setup();
    let plain = paint_arr(&mut v, &doc, &reg);
    let at = on_lane(&v, "keys", 1, 420.0);
    let _ = v.handle(&hover(at, &clip_item()), &doc, &reg);
    assert!(v.needs_repaint());
    assert_eq!(v.drop_hover(), None, "it is not an audio import indicator");
    assert_eq!(
        v.clip_drop_hover(),
        Some((key("keys"), 1, snap_round(v.x_to_tick(420.0), Some(BAR))))
    );
    let hovering = paint_arr(&mut v, &doc, &reg);
    assert_ne!(hovering, plain, "the marker is painted");
    // Moving to another lane moves the marker.
    let _ = v.handle(
        &hover(on_lane(&v, "keys", 0, 420.0), &clip_item()),
        &doc,
        &reg,
    );
    assert_ne!(paint_arr(&mut v, &doc, &reg), hovering);
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!(v.clip_drop_hover(), None);
    assert_eq!(paint_arr(&mut v, &doc, &reg), plain);
    let _ = v.handle(&hover(at, &clip_item()), &doc, &reg);
    let _ = v.handle(&drop_on(at, &clip_item()), &doc, &reg);
    assert_eq!(v.clip_drop_hover(), None);
    // Over the ruler there is no marker.
    let _ = v.handle(&hover(Point::new(420.0, 5.0), &clip_item()), &doc, &reg);
    assert_eq!(v.clip_drop_hover(), None);
}

#[test]
fn a_clip_dropped_elsewhere_is_refused_and_never_imported_as_audio() {
    let (doc, reg, mut v) = setup();
    for at in [
        on_lane(&v, "bass", 1, 420.0),
        Point::new(420.0, 5.0),
        Point::new(420.0, 790.0),
    ] {
        assert!(v.handle(&hover(at, &clip_item()), &doc, &reg).is_empty());
        assert_eq!(v.clip_drop_hover(), None);
        assert_eq!(v.drop_hover(), None);
        assert!(
            v.handle(&drop_on(at, &clip_item()), &doc, &reg).is_empty(),
            "{at:?}"
        );
    }
}

#[test]
fn other_library_items_are_refused_everywhere_in_the_arrangement() {
    let (doc, reg, mut v) = setup();
    let at = on_lane(&v, "keys", 1, 420.0);
    for path in [
        p("Patches/Lead.atomikpatch"),
        p("Containers/Box.atomikcontainer"),
        p("Projects/Song.atomik"),
        device_uri("atomik.poly-synth"),
    ] {
        assert!(
            v.handle(&hover(at, std::slice::from_ref(&path)), &doc, &reg)
                .is_empty()
        );
        assert_eq!(v.clip_drop_hover(), None);
        assert_eq!(v.drop_hover(), None, "{path:?}");
        assert!(
            v.handle(&drop_on(at, std::slice::from_ref(&path)), &doc, &reg)
                .is_empty(),
            "{path:?}"
        );
    }
}

#[test]
fn audio_drops_still_import() {
    let (doc, reg, mut v) = setup();
    let at = on_lane(&v, "keys", 1, 420.0);
    let a = v.handle(&drop_on(at, &[p("kick.wav")]), &doc, &reg);
    assert!(
        matches!(a.as_slice(), [UiAction::ImportAudio { .. }]),
        "{a:?}"
    );
}
