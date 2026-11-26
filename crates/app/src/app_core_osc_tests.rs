//! The OSC surface through a real UDP socket on the loopback address: a client socket in the
//! test plays the tablet. Nothing here leaves this computer; every wait is on a condition with a
//! bound of seconds, never on a fixed delay.

use super::*;
use crate::sink::RecordingSink;
use crate::surface::{OscLink, SurfaceLink};
use atomik_midi_io::surface::Osc;
use atomik_midi_io::surface::osc_wire::{Arg, Message, decode, encode};
use std::net::UdpSocket;
use std::time::Duration;

const SYNTH: &str = "atomik.poly-synth";
/// How long a condition may take on a slow machine: about ten seconds of short naps (a count,
/// not a clock).
struct Patience(u32);

impl Patience {
    fn left(&mut self) -> bool {
        std::thread::sleep(Duration::from_millis(2));
        self.0 += 1;
        self.0 < 5000
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn client() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    socket
}

fn osc_core(link: OscLink) -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for i in 1..=2 {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(&format!("s{i}"))),
        })
        .unwrap();
    }
    c.set_surface(Some(
        crate::surface::SurfaceHost::new(Box::new(Osc::new()), Box::new(link))
            .named("Tablet", "Tablet"),
    ));
    c
}

fn local(link: &OscLink) -> std::net::SocketAddr {
    link.local_addr().expect("bound")
}

/// Everything the client has received so far, decoded.
fn received(client: &UdpSocket, into: &mut Vec<Message>) {
    let mut buf = vec![0u8; 65_536];
    while let Ok((n, _)) = client.recv_from(&mut buf) {
        decode(&buf[..n], into).unwrap();
    }
}

fn value_of(messages: &[Message], address: &str) -> Option<Arg> {
    messages
        .iter()
        .rev()
        .find(|m| m.address == address)
        .and_then(|m| m.args.first().cloned())
}

#[test]
fn the_link_binds_the_loopback_address_and_tells_the_driver_about_a_new_client() {
    let mut link = OscLink::bind(0, false, None).unwrap();
    assert!(local(&link).ip().is_loopback(), "this computer only");
    assert!(link.describe().starts_with("OSC on 127.0.0.1:"));
    let tablet = client();
    let packet = encode(&Message::with("/strip/1/fader", Arg::Float(0.5)));
    tablet.send_to(&packet, local(&link)).unwrap();
    let mut got = Vec::new();
    let mut start = Patience(0);
    while got.len() < 2 && start.left() {
        link.poll(&mut got);
    }
    assert_eq!(got.len(), 2, "the hello, then the packet");
    assert_eq!(got[0], encode(&Message::new("/surface/hello")));
    assert_eq!(got[1], packet);
    assert_eq!(link.clients(), [tablet.local_addr().unwrap()]);
    // The same client again is not new.
    tablet.send_to(&packet, local(&link)).unwrap();
    let mut again = Vec::new();
    let mut start = Patience(0);
    while again.is_empty() && start.left() {
        link.poll(&mut again);
    }
    assert_eq!(again, vec![packet]);
    // What the host sends goes to the client.
    link.send(b"/pong\0\0\0,\0\0\0");
    let mut buf = [0u8; 64];
    let (n, _) = tablet.recv_from(&mut buf).expect("an answer");
    assert_eq!(&buf[..n], b"/pong\0\0\0,\0\0\0");
}

#[test]
fn a_reply_port_sends_the_answers_to_the_port_the_client_listens_on() {
    let listener = client();
    let sender = client();
    let mut link = OscLink::bind(0, false, Some(listener.local_addr().unwrap().port())).unwrap();
    sender.send_to(b"/x\0\0", local(&link)).unwrap();
    let mut got = Vec::new();
    let mut start = Patience(0);
    while got.len() < 2 && start.left() {
        link.poll(&mut got);
    }
    link.send(b"/answer\0,\0\0\0");
    let mut buf = [0u8; 64];
    let (n, _) = listener.recv_from(&mut buf).expect("on the reply port");
    assert_eq!(&buf[..n], b"/answer\0,\0\0\0");
}

#[test]
fn a_port_that_is_taken_is_reported() {
    let first = OscLink::bind(0, false, None).unwrap();
    let taken = local(&first).port();
    let err = OscLink::bind(taken, false, None).unwrap_err();
    assert!(err.starts_with(&format!("OSC port {taken}:")), "{err}");
}

#[test]
fn only_the_eight_latest_clients_are_answered() {
    let mut link = OscLink::bind(0, false, None).unwrap();
    let clients: Vec<UdpSocket> = (0..10).map(|_| client()).collect();
    for c in &clients {
        c.send_to(b"/x\0\0", local(&link)).unwrap();
    }
    let mut got = Vec::new();
    let mut start = Patience(0);
    while got.len() < 20 && start.left() {
        link.poll(&mut got);
    }
    assert_eq!(link.clients().len(), 8);
    assert!(!link.clients().contains(&clients[0].local_addr().unwrap()));
    assert!(link.clients().contains(&clients[9].local_addr().unwrap()));
}

#[test]
fn a_tablet_moves_a_fader_and_is_shown_the_console() {
    let link = OscLink::bind(0, false, None).unwrap();
    let addr = local(&link);
    let mut c = osc_core(link);
    let tablet = client();
    let channel = key("channel");
    let before = c
        .doc()
        .param_value(&channel, "fader", c.registry())
        .unwrap();
    // The first packet makes the client known; the next frames answer with the console.
    let fader = encode(&Message::with("/strip/1/fader", Arg::Float(0.25)));
    tablet.send_to(&fader, addr).unwrap();
    let mut heard = Vec::new();
    let mut t = 0.0;
    let mut start = Patience(0);
    loop {
        t += 0.1;
        c.surface_frame(t);
        received(&tablet, &mut heard);
        let moved = c
            .doc()
            .param_value(&channel, "fader", c.registry())
            .unwrap();
        let shown = value_of(&heard, "/strip/1/fader");
        if (moved - before).abs() > 1e-4
            && shown.is_some()
            && value_of(&heard, "/master/fader").is_some()
        {
            break;
        }
        assert!(start.left(), "gave up: {heard:?}");
    }
    assert!(matches!(
        value_of(&heard, "/strip/1/name"),
        Some(Arg::Str(_))
    ));
    assert!(value_of(&heard, "/strip/2/name").is_some());
    assert_eq!(c.surface().unwrap().driver_name(), "osc");
    // A mute from the tablet is one undo step, and the project answers with its new state.
    tablet
        .send_to(&encode(&Message::with("/strip/2/mute", Arg::Int(1))), addr)
        .unwrap();
    let mut start = Patience(0);
    loop {
        t += 0.1;
        c.surface_frame(t);
        received(&tablet, &mut heard);
        if value_of(&heard, "/strip/2/mute") == Some(Arg::Int(1)) {
            break;
        }
        assert!(start.left(), "gave up: {heard:?}");
    }
    let muted = c
        .doc()
        .param_value(&key("channel-2"), "mute", c.registry())
        .unwrap();
    assert!(muted >= 0.5);
}

#[test]
fn a_client_that_arrives_late_is_shown_everything_again() {
    let link = OscLink::bind(0, false, None).unwrap();
    let addr = local(&link);
    let mut c = osc_core(link);
    // The surface connects and sends its first picture to nobody.
    let mut t = 0.0;
    for _ in 0..3 {
        t += 0.1;
        c.surface_frame(t);
    }
    let late = client();
    late.send_to(&encode(&Message::new("/ping")), addr).unwrap();
    let mut heard = Vec::new();
    let mut start = Patience(0);
    loop {
        t += 0.1;
        c.surface_frame(t);
        received(&late, &mut heard);
        if value_of(&heard, "/strip/1/fader").is_some()
            && heard.iter().any(|m| m.address == "/pong")
        {
            break;
        }
        assert!(start.left(), "gave up: {heard:?}");
    }
    assert!(value_of(&heard, "/strip/2/fader").is_some());
    assert!(value_of(&heard, "/master/fader").is_some());
}
