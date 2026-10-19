//! The compressor's "SC from..." helper: the list on the panel, one valid
//! `Command::Batch` per choice applied to a real `Document` (one undo step), Off, replacing a
//! source, a rejected cycle, and the panel showing the source.

mod support;

use atomik_model::{Command, Document, RackDoc};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::sidechain::{SC_ENABLE, ScSource, sidechain_command, sidechain_entries};
use atomik_ui::{DisplayList, Key, Modifiers, RackView, Theme, UiAction, UiEvent};
use support::song::press;
use support::*;

fn add(document: &mut Document, type_id: &str, k: &str) {
    let index = document.doc().devices.len();
    document
        .apply(
            Command::AddDevice {
                index,
                type_id: type_id.to_string(),
                key: Some(key(k)),
            },
            &registry(),
        )
        .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

/// Two instruments (each with its console channel) and a compressor.
fn rig() -> (Document, RackView) {
    let mut document = Document::new(atomik_model::empty_doc());
    add(&mut document, "atomik.poly-synth", "kick");
    add(&mut document, "atomik.poly-synth", "bass");
    add(&mut document, "atomik.compressor", "comp");
    let mut v = RackView::new(Theme::dark());
    // The rack with console devices hidden (they would push the
    // compressor below the view).
    v.set_show_console_devices(false);
    let _ = v.handle(
        &UiEvent::Resize(Size::new(900.0, 1800.0)),
        document.doc(),
        &registry(),
    );
    v.sync(document.doc(), &registry());
    (document, v)
}

fn click(v: &mut RackView, document: &Document, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), document.doc(), &reg);
    out.extend(v.handle(&up(p), document.doc(), &reg));
    out
}

fn batches(a: &[UiAction]) -> Vec<Vec<Command>> {
    commands(a)
        .into_iter()
        .filter_map(|c| match c {
            Command::Batch(b) => Some(b),
            _ => None,
        })
        .collect()
}

fn open_menu(v: &mut RackView, document: &Document) {
    let b = v.sc_button_center(&key("comp")).expect("compressor panel");
    let a = click(v, document, b);
    assert!(commands(&a).is_empty(), "{a:?}");
    assert!(!v.sc_menu_labels().is_empty(), "the list opens");
}

fn choose(v: &mut RackView, document: &Document, row: usize) -> Vec<UiAction> {
    let p = v.sc_menu_row_center(row).expect("row");
    click(v, document, p)
}

fn sc_cables(doc: &RackDoc) -> Vec<(String, String, String)> {
    doc.cables
        .iter()
        .filter(|c| c.to.device == key("comp") && c.to.socket.starts_with("sc_in"))
        .map(|c| {
            (
                c.from.device.0.clone(),
                c.from.socket.clone(),
                c.to.socket.clone(),
            )
        })
        .collect()
}

fn sc_enable(document: &Document) -> f32 {
    document
        .doc()
        .param_value(&key("comp"), SC_ENABLE, &registry())
        .unwrap()
}

#[test]
fn the_list_offers_off_channels_and_other_devices() {
    let (document, mut v) = rig();
    open_menu(&mut v, &document);
    let labels = v.sc_menu_labels();
    assert_eq!(labels[0], "Off");
    let entries = sidechain_entries(document.doc(), &registry(), &key("comp"));
    assert_eq!(entries.len(), labels.len());
    // Two channels, then the two instruments; neither the compressor nor the master.
    let channels = entries
        .iter()
        .filter(|e| matches!(e.source, ScSource::Channel(_)))
        .count();
    let devices = entries
        .iter()
        .filter(|e| matches!(e.source, ScSource::Device(_)))
        .count();
    assert_eq!((channels, devices), (2, 2), "{labels:?}");
    assert!(entries.iter().all(|e| match &e.source {
        ScSource::Channel(k) | ScSource::Device(k) => *k != key("comp"),
        ScSource::Off => true,
    }));
}

#[test]
fn choosing_a_channel_emits_one_batch_that_the_model_accepts_and_undoes_in_one_step() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let ch = document.doc().console.channels[0].channel.clone();
    let before = document.doc().clone();
    let a = choose(&mut v, &document, 1);
    let cmds = commands(&a);
    assert_eq!(cmds.len(), 1, "one command: {a:?}");
    let b = batches(&a);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].len(), 3, "two connects and the enable: {:?}", b[0]);
    assert!(v.sc_menu_labels().is_empty(), "the list closes");
    document
        .apply(cmds[0].clone(), &registry())
        .expect("valid batch");
    v.sync(document.doc(), &registry());
    let mut cables = sc_cables(document.doc());
    cables.sort();
    assert_eq!(
        cables,
        vec![
            (
                ch.0.clone(),
                "insert_send_l".to_string(),
                "sc_in_l".to_string()
            ),
            (
                ch.0.clone(),
                "insert_send_r".to_string(),
                "sc_in_r".to_string()
            ),
        ]
    );
    assert_eq!(sc_enable(&document), 1.0);
    // One undo step restores the cables and the switch.
    document.undo(&registry());
    assert_eq!(document.doc().cables, before.cables);
    assert_eq!(sc_enable(&document), 0.0);
    document.redo(&registry());
    assert_eq!(sc_cables(document.doc()).len(), 2);
}

#[test]
fn replacing_the_source_disconnects_then_connects_in_one_batch() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let a = choose(&mut v, &document, 1);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    v.sync(document.doc(), &registry());
    // Now key from the second channel.
    open_menu(&mut v, &document);
    let ch1 = document.doc().console.channels[0].channel.clone();
    let ch2 = document.doc().console.channels[1].channel.clone();
    let a = choose(&mut v, &document, 2);
    let b = batches(&a);
    assert_eq!(b.len(), 1);
    assert_eq!(
        b[0].len(),
        4,
        "2 disconnects + 2 connects (already enabled): {:?}",
        b[0]
    );
    let disconnects = b[0]
        .iter()
        .filter(|c| matches!(c, Command::Disconnect { .. }))
        .count();
    let connects = b[0]
        .iter()
        .filter(|c| matches!(c, Command::Connect(_)))
        .count();
    assert_eq!((disconnects, connects), (2, 2));
    // Each input is freed right before it is plugged again.
    assert!(matches!(b[0][0], Command::Disconnect { .. }));
    assert!(matches!(b[0][1], Command::Connect(_)));
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    v.sync(document.doc(), &registry());
    assert!(sc_cables(document.doc()).iter().all(|c| c.0 == ch2.0));
    document.undo(&registry());
    let back = sc_cables(document.doc());
    assert_eq!(back.len(), 2, "the first source is back");
    assert!(back.iter().all(|c| c.0 == ch1.0));
}

#[test]
fn a_device_source_uses_its_out_l_and_out_r() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let entries = sidechain_entries(document.doc(), &registry(), &key("comp"));
    let row = entries
        .iter()
        .position(|e| e.source == ScSource::Device(key("kick")))
        .unwrap();
    let a = choose(&mut v, &document, row);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    let mut cables = sc_cables(document.doc());
    cables.sort();
    assert_eq!(
        cables,
        vec![
            (
                "kick".to_string(),
                "out_l".to_string(),
                "sc_in_l".to_string()
            ),
            (
                "kick".to_string(),
                "out_r".to_string(),
                "sc_in_r".to_string()
            ),
        ]
    );
}

#[test]
fn off_removes_the_cables_and_the_switch_in_one_step() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let a = choose(&mut v, &document, 1);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    v.sync(document.doc(), &registry());
    open_menu(&mut v, &document);
    let a = choose(&mut v, &document, 0);
    let b = batches(&a);
    assert_eq!(b.len(), 1, "{a:?}");
    assert_eq!(b[0].len(), 3);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    assert!(sc_cables(document.doc()).is_empty());
    assert_eq!(sc_enable(&document), 0.0);
    document.undo(&registry());
    assert_eq!(sc_cables(document.doc()).len(), 2);
    assert_eq!(sc_enable(&document), 1.0);
    // Off when nothing is cabled and the switch is off: nothing to do.
    let (document, _) = rig();
    assert!(sidechain_command(document.doc(), &registry(), &key("comp"), &ScSource::Off).is_none());
}

#[test]
fn choosing_the_current_source_again_does_nothing() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let a = choose(&mut v, &document, 1);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    v.sync(document.doc(), &registry());
    open_menu(&mut v, &document);
    let a = choose(&mut v, &document, 1);
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn a_source_that_would_make_a_cycle_is_rejected_whole() {
    let (mut document, _) = rig();
    add(&mut document, "atomik.compressor", "comp2");
    document
        .apply(
            Command::Connect(cable(("comp", "out_l"), ("comp2", "in_l"))),
            &registry(),
        )
        .unwrap();
    let before = document.doc().clone();
    // comp2 is downstream of comp: keying comp from comp2 is a cycle.
    let cmd = sidechain_command(
        document.doc(),
        &registry(),
        &key("comp"),
        &ScSource::Device(key("comp2")),
    )
    .expect("a batch");
    assert!(document.apply(cmd, &registry()).is_err());
    assert_eq!(*document.doc(), before, "nothing changed");
}

#[test]
fn escape_and_outside_clicks_close_the_list_without_a_command() {
    let (document, mut v) = rig();
    open_menu(&mut v, &document);
    let a = v.handle(
        &press(Key::Escape, Modifiers::NONE),
        document.doc(),
        &registry(),
    );
    assert!(commands(&a).is_empty());
    assert!(v.sc_menu_labels().is_empty());
    open_menu(&mut v, &document);
    let a = click(&mut v, &document, Point::new(450.0, 1700.0));
    assert!(commands(&a).is_empty());
    assert!(v.sc_menu_labels().is_empty());
    // The button toggles the list.
    open_menu(&mut v, &document);
    let b = v.sc_button_center(&key("comp")).unwrap();
    let a = click(&mut v, &document, b);
    assert!(commands(&a).is_empty());
    assert!(v.sc_menu_labels().is_empty());
}

#[test]
fn the_panel_shows_the_source_and_the_menu_is_painted() {
    let (mut document, mut v) = rig();
    open_menu(&mut v, &document);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &registry());
    let t: Vec<&str> = dl.texts();
    assert!(t.contains(&"SC: off"), "{t:?}");
    assert!(t.contains(&"Off"), "the list is painted: {t:?}");
    let a = choose(&mut v, &document, 1);
    document
        .apply(commands(&a)[0].clone(), &registry())
        .unwrap();
    let name = {
        let ch = &document.doc().console.channels[0].channel;
        document.doc().device(ch).unwrap().name.clone()
    };
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &registry());
    let t: Vec<&str> = dl.texts();
    let want = format!("SC: {name}");
    assert!(t.iter().any(|x| *x == want), "{want}: {t:?}");
}
