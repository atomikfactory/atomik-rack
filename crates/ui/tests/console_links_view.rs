//! Console links in the rack (UI half): channel to master cables are not
//! drawn by default (an "Output: Master" display on the channel instead), a show-all mode draws
//! them, every other cable class is drawn in both modes. Goldens regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_links_view`.

mod support;

use atomik_model::console::link_kind;
use atomik_model::{Cable, RackDoc};
use atomik_ui::{Flip, RackView};
use support::starter::*;
use support::*;

fn back_view(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    let mut v = view(doc, reg);
    v.set_flip(Flip::Back);
    v
}

fn is_link(doc: &RackDoc, c: &Cable) -> bool {
    link_kind(doc, c).is_some()
}

#[test]
fn links_are_hidden_by_default_and_every_other_cable_is_drawn() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let v = back_view(doc, &reg);
    assert!(!v.show_console_links());
    let shown = v.visible_cables(doc);
    let links = doc.cables.iter().filter(|c| is_link(doc, c)).count();
    assert!(links >= 36, "two channels with 18 links each: {links}");
    assert!(shown.iter().all(|c| !is_link(doc, c)));
    assert_eq!(shown.len(), doc.cables.len() - links);
    // The source to channel cables, the return loop and main to Audio I/O are all there.
    for expected in [
        cable(("synth", "out_l"), ("channel", "in_l")),
        cable(("master", "main_l"), ("interface", "out_1")),
        cable(("master", "fx_send1_l"), ("echo", "in_l")),
        cable(("echo", "out_l"), ("master", "return1_l")),
    ] {
        assert!(shown.contains(&expected), "{expected:?}");
    }
}

#[test]
fn the_show_links_mode_draws_everything_and_hides_again() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = back_view(doc, &reg);
    v.set_show_console_links(true);
    assert!(v.show_console_links());
    let mut shown = v.visible_cables(doc);
    let mut all = doc.cables.clone();
    shown.sort();
    all.sort();
    assert_eq!(shown, all);
    v.set_show_console_links(false);
    assert!(v.visible_cables(doc).iter().all(|c| !is_link(doc, c)));
}

#[test]
fn the_cable_display_cycle_acts_on_the_remaining_cables() {
    use atomik_ui::CableVisibility;
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = back_view(doc, &reg);
    v.set_cable_visibility(CableVisibility::Hide);
    assert!(v.visible_cables(doc).is_empty());
    v.set_show_console_links(true);
    assert!(v.visible_cables(doc).is_empty(), "Hide hides links too");
    v.set_cable_visibility(CableVisibility::Show);
    assert_eq!(v.visible_cables(doc).len(), doc.cables.len());
}

#[test]
fn a_hidden_link_cannot_be_hovered() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = back_view(doc, &reg);
    let link = cable(("channel", "out_l"), ("master", "bus_l"));
    assert!(is_link(doc, &link));
    // Mouse down in the middle of the link's curve (unfolded so sockets are reachable).
    let mid = v.cable_midpoint(&link).expect("geometry exists");
    let _ = v.handle(&mv(mid), doc, &reg);
    assert_ne!(v.hovered_cable(), Some(&link));
}

#[test]
fn the_channel_display_reads_master_direct_mixed_and_none() {
    let reg = registry();
    let d = two_instruments(&reg);
    let base = d.doc().clone();

    let v = view(&base, &reg);
    let shown = v
        .output_display(&key("channel"))
        .expect("a channel has a display");
    assert_eq!(shown.text, "Output: Master");
    assert!(!shown.warn);
    assert_eq!(shown.tooltip, None);

    // Direct: both outputs cabled to Audio I/O 3 and 4 instead of the master.
    let mut doc = base.clone();
    doc.output_channels = 4;
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    doc.cables
        .push(cable(("channel", "out_l"), ("interface", "out_3")));
    doc.cables
        .push(cable(("channel", "out_r"), ("interface", "out_4")));
    let v = view(&doc, &reg);
    let shown = v.output_display(&key("channel")).unwrap();
    assert_eq!(shown.text, "Output: Audio I/O 3+4");
    assert!(shown.warn);
    assert!(
        shown
            .tooltip
            .as_deref()
            .unwrap()
            .starts_with("Leaves the master")
    );

    // Mixed: out_l stays on the master, out_r goes to Audio I/O 4.
    let mut doc = base.clone();
    doc.output_channels = 4;
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket == "out_r"));
    doc.cables
        .push(cable(("channel", "out_r"), ("interface", "out_4")));
    let v = view(&doc, &reg);
    let shown = v.output_display(&key("channel")).unwrap();
    assert!(shown.warn, "{shown:?}");
    assert!(shown.text.starts_with("Output: Master"));

    // None.
    let mut doc = base.clone();
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    let v = view(&doc, &reg);
    let shown = v.output_display(&key("channel")).unwrap();
    assert_eq!(shown.text, "Output: none");
    assert!(!shown.warn);

    // Not a channel: no display; the master reads where main goes.
    let v = view(&base, &reg);
    assert!(v.output_display(&key("synth")).is_none());
    let master = v.output_display(&key("master")).unwrap();
    assert_eq!(master.text, "Output: Audio I/O 1+2");
    assert!(!master.warn);
}

#[test]
fn a_channel_cabled_to_two_devices_lists_each_joined_by_a_plus_sign() {
    // Destinations are grouped by device (sockets of one device joined with "+") and the
    // devices are joined with " + ", in cable order; the tooltip says the same.
    let reg = registry();
    let d = with_effects(&reg);
    let mut doc = d.doc().clone();
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    doc.cables
        .push(cable(("channel", "out_l"), ("interface", "out_2")));
    doc.cables
        .push(cable(("channel", "out_r"), ("echo", "in_r")));
    let v = view(&doc, &reg);
    let shown = v.output_display(&key("channel")).unwrap();
    assert_eq!(shown.text, "Output: Audio I/O 2 + Stereo Delay In R");
    assert!(shown.warn);
    assert_eq!(
        shown.tooltip.as_deref(),
        Some("Leaves the master: cabled to Audio I/O 2 + Stereo Delay In R")
    );
}

#[test]
fn the_display_follows_edits_of_the_document() {
    let reg = registry();
    let d = two_instruments(&reg);
    let mut doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    assert_eq!(
        v.output_display(&key("channel")).unwrap().text,
        "Output: Master"
    );
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    v.sync(&doc, &reg);
    assert_eq!(
        v.output_display(&key("channel")).unwrap().text,
        "Output: none"
    );
}

fn unfold(d: &mut atomik_model::Document, reg: &atomik_device_sdk::Registry, k: &str) {
    run(
        d,
        reg,
        [atomik_model::Command::SetFolded {
            key: key(k),
            folded: false,
        }],
    );
}

#[test]
fn rack_channel_output_master_matches_golden() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    unfold(&mut d, &reg, "channel");
    let doc = d.doc();
    let mut v = view(doc, &reg);
    check_golden(
        "rack_channel_output_master_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
}

#[test]
fn rack_channel_output_direct_matches_golden() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    unfold(&mut d, &reg, "channel");
    let mut doc = d.doc().clone();
    doc.output_channels = 4;
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    doc.cables
        .push(cable(("channel", "out_l"), ("interface", "out_3")));
    doc.cables
        .push(cable(("channel", "out_r"), ("interface", "out_4")));
    let mut v = view(&doc, &reg);
    check_golden(
        "rack_channel_output_direct_1x.json",
        &paint_json(&mut v, &doc, &reg),
    );
}

#[test]
fn rack_console_links_hidden_and_shown_match_goldens() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = back_view(doc, &reg);
    check_golden(
        "rack_console_links_hidden_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
    v.set_show_console_links(true);
    check_golden(
        "rack_console_links_shown_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
}

#[test]
fn a_channel_that_leaves_the_master_explains_itself_in_a_tooltip() {
    let reg = registry();
    let d = two_instruments(&reg);
    let mut doc = d.doc().clone();
    doc.output_channels = 4;
    doc.cables
        .retain(|c| !(c.from.device == key("channel") && c.from.socket.starts_with("out_")));
    doc.cables
        .push(cable(("channel", "out_l"), ("interface", "out_3")));
    doc.cables
        .push(cable(("channel", "out_r"), ("interface", "out_4")));
    let mut v = view(&doc, &reg);
    let at = v.plate_center(&key("channel")).unwrap();
    let _ = v.handle(&mv(at), &doc, &reg);
    let tip = v.tooltip().expect("a tooltip on the plate");
    assert!(tip.starts_with("Leaves the master"), "{tip}");
    let other = v.plate_center(&key("channel-2")).unwrap();
    let _ = v.handle(&mv(other), &doc, &reg);
    assert_eq!(v.tooltip(), None, "a channel on the master has none");
}
