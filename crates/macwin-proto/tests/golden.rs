//! Golden bytes: these lock the wire format. If one of them changes, peers of
//! different versions stop understanding each other.

use macwin_core::msg::{
    ClipboardMsg, ClipboardText, ControlMsg, DatagramMsg, DiscoveryMsg, DiscoveryQuery,
    DiscoveryWant, Heartbeat, InputMsg, KeyMsg, MouseAbs, PairMsg,
};
use macwin_core::types::{Hid, Point, RoleKind};
use macwin_proto::{
    decode_clipboard, decode_control, decode_datagram, decode_discovery, decode_input, decode_pair,
    encode_clipboard, encode_control, encode_datagram, encode_discovery, encode_input, encode_pair,
    frame,
};

#[test]
fn clipboard_text() {
    let m = ClipboardMsg::Text(ClipboardText {
        clock: 1,
        text: "hi".into(),
    });
    let bytes = encode_clipboard(&m);
    assert_eq!(bytes, [0x0A, 0x06, 0x08, 0x01, 0x12, 0x02, 0x68, 0x69]);
    assert_eq!(decode_clipboard(&bytes).unwrap(), m);
}

#[test]
fn pair_done() {
    let bytes = encode_pair(&PairMsg::Done);
    assert_eq!(bytes, [0x1A, 0x00]);
    assert_eq!(decode_pair(&bytes).unwrap(), PairMsg::Done);
}

#[test]
fn discovery_pairing_query() {
    let m = DiscoveryMsg::Query(DiscoveryQuery {
        nonce: [0xAB; 16],
        want: DiscoveryWant::Pairing,
    });
    let bytes = encode_discovery(&m);
    let mut want = b"MWD1".to_vec();
    want.extend([0x0A, 0x14, 0x0A, 0x10]);
    want.extend([0xAB; 16]);
    want.extend([0x12, 0x00]);
    assert_eq!(bytes, want);
    assert_eq!(decode_discovery(&bytes).unwrap(), m);
}

#[test]
fn key_a_down() {
    let m = InputMsg::Key(KeyMsg {
        hid: Hid(0x07_0004),
        down: true,
        repeat: false,
    });
    let bytes = encode_input(&m);
    assert_eq!(bytes, [0x22, 0x06, 0x08, 0x84, 0x80, 0x1c, 0x10, 0x01]);
    assert_eq!(decode_input(&bytes).unwrap(), m);
    assert_eq!(
        &frame::encode(&bytes)[..],
        [0x08, 0x22, 0x06, 0x08, 0x84, 0x80, 0x1c, 0x10, 0x01]
    );
}

#[test]
fn heartbeat() {
    let m = ControlMsg::Heartbeat(Heartbeat {
        epoch: 3,
        role: RoleKind::Controlling,
    });
    let bytes = encode_control(&m);
    assert_eq!(bytes, [0x12, 0x04, 0x08, 0x03, 0x10, 0x02]);
    assert_eq!(decode_control(&bytes).unwrap(), m);
}

#[test]
fn mouse_abs_datagram() {
    let m = DatagramMsg::MouseAbs(MouseAbs {
        epoch: 1,
        seq: 2,
        at: Point::new(-1, 5),
    });
    let bytes = encode_datagram(&m);
    assert_eq!(
        bytes,
        [
            0x0A, 0x0A, 0x08, 0x01, 0x10, 0x02, 0x1A, 0x04, 0x08, 0x01, 0x10, 0x0A
        ]
    );
    assert_eq!(decode_datagram(&bytes).unwrap(), m);
}
