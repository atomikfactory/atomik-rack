//! Rack editing follow-ups in the views: the "+" handles inside an unfolded container, the
//! device types grouped by family for the Create menu, and a browser row dragged onto a track
//! header of the arrangement.

mod support;

use atomik_model::{Command, ContainerCommand, CreateSource, Document, RackDoc};
use atomik_ui::browser::device_groups;
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{Key, Modifiers, PointerButton, RackView, Theme, UiEvent};
use support::container::*;
use support::song::*;
use support::*;

const TALL: Size = Size::new(1000.0, 3600.0);

fn tall_view(doc: &RackDoc) -> RackView {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(TALL), doc, &reg);
    v.sync(doc, &reg);
    v
}

fn unfolded(open: bool) -> Document {
    let mut d = layered();
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetOpen {
            container: key("layer"),
            open,
        }),
    );
    d
}

fn click_at(v: &mut RackView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, at: Point) {
    let _ = v.handle(&mv(at), doc, reg);
    let _ = v.handle(&down(at), doc, reg);
    let _ = v.handle(&up(at), doc, reg);
}

/// The handle centre below the device `k` (just under its panel).
fn handle_below(v: &RackView, k: &str) -> Point {
    let bottom = v.device_rect(&key(k)).expect("view").y1;
    v.handle_centers()
        .into_iter()
        .map(|p| (p, (v.view_to_rack(p).y - bottom).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, d)| {
            assert!(d < 1.0, "no seam below {k}");
            p
        })
        .unwrap()
}

// ------------------------------------------------------------ "+" handles inside a container

#[test]
fn an_unfolded_container_has_a_handle_below_its_input_strip_and_each_device() {
    let (folded, open) = (unfolded(false), unfolded(true));
    let before = tall_view(folded.doc()).handle_centers().len();
    let v = tall_view(open.doc());
    // Below the INPUT strip, then below synth, pad, delay and reverb.
    assert_eq!(v.handle_centers().len(), before + 5);
    // None on the back side.
    let mut back = tall_view(open.doc());
    back.set_flip(atomik_ui::Flip::Back);
    assert!(back.handle_centers().is_empty());
}

#[test]
fn a_handle_inside_a_container_inserts_the_row_in_it_at_that_place() {
    let (doc, reg) = (unfolded(true), registry());
    let mut v = tall_view(doc.doc());
    let at = handle_below(&v, "layer/pad");
    click_at(&mut v, doc.doc(), &reg, at);
    assert!(v.browser_visible());
    assert_eq!(v.pending_insert(), None);
    assert_eq!(v.pending_insert_inside(), Some((&key("layer"), 2)));
    let i = v
        .browser_rows(&reg)
        .iter()
        .position(|t| *t == "atomik.reverb")
        .expect("row");
    let row = v.browser_row_center(i).expect("browser open");
    let a = v.handle(&down_with(row, PointerButton::Primary, 2), doc.doc(), &reg);
    match commands(&a).as_slice() {
        [
            Command::Container(ContainerCommand::AddInner {
                container,
                index,
                type_id,
                ..
            }),
        ] => {
            assert_eq!(container, &key("layer"));
            assert_eq!(*index, 2);
            assert_eq!(type_id, "atomik.reverb");
        }
        other => panic!("{other:?}"),
    }
    assert!(!v.browser_visible(), "the browser closes once it inserted");
    assert_eq!(v.pending_insert_inside(), None);
}

#[test]
fn the_handle_below_the_input_strip_is_the_first_place_and_escape_forgets_it() {
    let (doc, reg) = (unfolded(true), registry());
    let mut v = tall_view(doc.doc());
    let at = handle_below(&v, "layer/boundary");
    click_at(&mut v, doc.doc(), &reg, at);
    assert_eq!(v.pending_insert_inside(), Some((&key("layer"), 0)));
    let esc = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    let a = v.handle(&esc, doc.doc(), &reg);
    assert!(commands(&a).is_empty());
    assert!(!v.browser_visible());
    assert_eq!(v.pending_insert_inside(), None);
    // The last handle of the container sits above its OUTPUT strip: index 4.
    let at = handle_below(&v, "layer/reverb");
    click_at(&mut v, doc.doc(), &reg, at);
    assert_eq!(v.pending_insert_inside(), Some((&key("layer"), 4)));
}

#[test]
fn a_top_level_handle_still_inserts_at_a_slot_of_the_rack() {
    let (doc, reg) = (unfolded(true), registry());
    let mut v = tall_view(doc.doc());
    let at = handle_below(&v, "lead");
    click_at(&mut v, doc.doc(), &reg, at);
    assert!(v.pending_insert().is_some());
    assert_eq!(v.pending_insert_inside(), None);
}

// ---------------------------------------------------------------- device types by family

#[test]
fn the_device_groups_list_every_offered_type_once_by_family_and_name() {
    let reg = registry();
    let groups = device_groups(&reg, |_| true);
    let names: Vec<&str> = groups.iter().map(|g| g.name).collect();
    assert_eq!(&names[..2], ["Instruments", "Effects"]);
    assert!(names.contains(&"Players"));
    let mut all: Vec<&str> = groups
        .iter()
        .flat_map(|g| g.types.iter().map(|(t, _)| *t))
        .collect();
    let browser = RackView::new(Theme::dark());
    let mut offered = browser.browser_rows(&reg);
    // The menus never list the console devices, which the browser shows on demand.
    offered.retain(|t| !matches!(*t, "atomik.channel" | "atomik.master"));
    all.sort_unstable();
    offered.sort_unstable();
    assert_eq!(all, offered, "every type of the browser is in one group");
    for g in &groups {
        let lower: Vec<String> = g.types.iter().map(|(_, n)| n.to_lowercase()).collect();
        let mut sorted = lower.clone();
        sorted.sort();
        assert_eq!(lower, sorted, "{} is by name", g.name);
    }
    // A filter narrows the groups and drops the ones left empty.
    let only = device_groups(&reg, |d| d.type_id == "atomik.reverb");
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].types, [("atomik.reverb", "Reverb")]);
}

// -------------------------------------------------- a browser row dragged onto a track header

fn drag_row_out(v: &mut RackView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, t: &str) {
    v.set_browser_visible(true);
    let i = v
        .browser_rows(reg)
        .iter()
        .position(|r| *r == t)
        .expect("row");
    let row = v.browser_row_center(i).expect("browser open");
    let _ = v.handle(&down(row), doc, reg);
    let _ = v.handle(&mv(Point::new(row.x + 80.0, row.y + 80.0)), doc, reg);
}

#[test]
fn a_dragged_browser_row_offers_what_it_would_replace_a_device_with() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert_eq!(v.dragged_replacement(), None);
    drag_row_out(&mut v, &doc, &reg, "atomik.mono-bass");
    assert_eq!(
        v.dragged_replacement(),
        Some(CreateSource::Type("atomik.mono-bass".into()))
    );
    let _ = v.handle(&up(Point::new(10.0, 10.0)), &doc, &reg);
    assert_eq!(v.dragged_replacement(), None, "the drag is over");
    // A press that has not travelled yet is no drag.
    v.set_browser_visible(true);
    let i = v
        .browser_rows(&reg)
        .iter()
        .position(|r| *r == "atomik.reverb")
        .unwrap();
    let row = v.browser_row_center(i).unwrap();
    let _ = v.handle(&down(row), &doc, &reg);
    assert_eq!(v.dragged_replacement(), None);
}

#[test]
fn a_track_header_outlines_for_a_row_that_can_replace_its_device_and_only_then() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let header = v.track_header_center(&key("lead")).expect("header");
    let synth = CreateSource::Type("atomik.mono-bass".into());
    let fx = CreateSource::Type("atomik.reverb".into());
    // An instrument row over an instrument track: the header is the target.
    assert_eq!(
        v.replace_hover(Some(header), Some(&synth), &doc, &reg),
        Some(key("lead"))
    );
    assert_eq!(v.header_drop_target(), Some(&key("lead")));
    assert!(v.needs_repaint());
    // An effect cannot replace an instrument: nothing is outlined.
    assert_eq!(v.replace_hover(Some(header), Some(&fx), &doc, &reg), None);
    assert_eq!(v.header_drop_target(), None);
    // Away from the headers, or with no row, nothing is either.
    assert_eq!(
        v.replace_hover(Some(Point::new(600.0, header.y)), Some(&synth), &doc, &reg),
        None
    );
    assert_eq!(v.replace_hover(Some(header), None, &doc, &reg), None);
    assert_eq!(v.replace_hover(None, Some(&synth), &doc, &reg), None);
    assert_eq!(v.header_drop_target(), None);
}

#[test]
fn a_taken_row_closes_a_create_browser_and_a_plain_browser_stays() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    v.browser_row_taken();
    assert!(v.browser_visible(), "the plain browser stays open");
    v.open_browser_create(atomik_ui::browser::CreateKind::Instrument, &reg);
    v.browser_row_taken();
    assert!(!v.browser_visible());
}
