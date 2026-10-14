//! Codec unit tests: every validation rule, version checks and property tests.

use macwin_core::msg::{
    ButtonMsg, ClipboardMsg, ClipboardText, ControlMsg, DatagramMsg, Enter, Heartbeat, Hello,
    InputMsg, KeyMsg, Leave, MouseAbs, PairConfirm, PairHello, PairMsg, Ping, Pong, TakeControl,
    WheelMsg,
};
use macwin_core::types::{
    Hid, ModifierMask, MouseButton, Os, Point, Rect, RoleKind, ScreenInfo, Side, WheelUnit,
};
use proptest::prelude::*;
use prost::Message;

use crate::wire::{self, control_frame, datagram_frame, input_frame};
use crate::*;

fn valid_screen() -> wire::Screen {
    wire::Screen {
        id: 1,
        x: 0,
        y: 0,
        w: 1440,
        h: 900,
        native_per_logical: 1.0,
        primary: true,
    }
}

fn valid_hello() -> wire::Hello {
    wire::Hello {
        protocol_version: PROTOCOL_VERSION,
        capabilities: vec!["input.inject".into()],
        os: wire::Os::Macos as i32,
        os_version: "15.8.1".into(),
        device_name: "mac".into(),
        screens: vec![valid_screen()],
        peer_side: wire::Side::Right as i32,
        agent_version: "0.1.0".into(),
    }
}

fn decode_hello(h: wire::Hello) -> Result<ControlMsg, ProtoError> {
    let f = wire::ControlFrame {
        kind: Some(control_frame::Kind::Hello(h)),
    };
    decode_control(&f.encode_to_vec())
}

fn decode_input_kind(k: input_frame::Kind) -> Result<InputMsg, ProtoError> {
    decode_input(&wire::InputFrame { kind: Some(k) }.encode_to_vec())
}

#[track_caller]
fn assert_invalid_value(r: Result<impl std::fmt::Debug, ProtoError>, expected_field: &str) {
    match r {
        Err(ProtoError::InvalidValue { field, .. }) => assert_eq!(field, expected_field),
        other => panic!("expected InvalidValue({expected_field}), got {other:?}"),
    }
}

#[track_caller]
fn assert_invalid_enum(r: Result<impl std::fmt::Debug, ProtoError>, expected_field: &str) {
    match r {
        Err(ProtoError::InvalidEnum { field, .. }) => assert_eq!(field, expected_field),
        other => panic!("expected InvalidEnum({expected_field}), got {other:?}"),
    }
}

#[test]
fn valid_hello_decodes() {
    let ControlMsg::Hello(h) = decode_hello(valid_hello()).unwrap() else {
        panic!("not a hello")
    };
    assert_eq!(h.os, Os::MacOs);
    assert_eq!(h.peer_side, Some(Side::Right));
    assert_eq!(h.screens[0].rect, Rect::new(0, 0, 1440, 900).unwrap());
    let mut no_side = valid_hello();
    no_side.peer_side = 0;
    let ControlMsg::Hello(h) = decode_hello(no_side).unwrap() else {
        panic!("not a hello")
    };
    assert_eq!(h.peer_side, None);
}

#[test]
fn missing_kind_is_rejected_for_every_family() {
    assert!(matches!(
        decode_control(&wire::ControlFrame { kind: None }.encode_to_vec()),
        Err(ProtoError::UnknownKind("control frame"))
    ));
    assert!(matches!(
        decode_input(&wire::InputFrame { kind: None }.encode_to_vec()),
        Err(ProtoError::UnknownKind("input frame"))
    ));
    assert!(matches!(
        decode_datagram(&wire::DatagramFrame { kind: None }.encode_to_vec()),
        Err(ProtoError::UnknownKind("datagram frame"))
    ));
    // An unknown oneof tag is skipped by protobuf and also leaves no kind.
    assert!(matches!(
        decode_input(&[0x52, 0x00]),
        Err(ProtoError::UnknownKind(_))
    ));
}

#[test]
fn missing_submessages_are_rejected() {
    let r = decode_input_kind(input_frame::Kind::Enter(wire::Enter { epoch: 1, at: None }));
    assert!(
        matches!(r, Err(ProtoError::MissingField("enter.at"))),
        "{r:?}"
    );
    let r = decode_input_kind(input_frame::Kind::Button(wire::Button {
        button: 1,
        down: true,
        at: None,
    }));
    assert!(
        matches!(r, Err(ProtoError::MissingField("button.at"))),
        "{r:?}"
    );
    let f = wire::DatagramFrame {
        kind: Some(datagram_frame::Kind::MouseAbs(wire::MouseAbs {
            epoch: 1,
            seq: 1,
            at: None,
        })),
    };
    assert!(matches!(
        decode_datagram(&f.encode_to_vec()),
        Err(ProtoError::MissingField("mouse_abs.at"))
    ));
}

#[test]
fn unspecified_and_unknown_enums_are_rejected() {
    for role in [0, 4, -1] {
        let f = wire::ControlFrame {
            kind: Some(control_frame::Kind::Heartbeat(wire::Heartbeat {
                epoch: 1,
                role,
            })),
        };
        assert_invalid_enum(decode_control(&f.encode_to_vec()), "heartbeat.role");
    }
    for button in [0, 6] {
        let at = Some(wire::Point { x: 0, y: 0 });
        assert_invalid_enum(
            decode_input_kind(input_frame::Kind::Button(wire::Button {
                button,
                down: true,
                at,
            })),
            "button.button",
        );
    }
    for unit in [0, 3] {
        assert_invalid_enum(
            decode_input_kind(input_frame::Kind::Wheel(wire::Wheel { dx: 0, dy: 1, unit })),
            "wheel.unit",
        );
    }
    let mut h = valid_hello();
    h.os = 0;
    assert_invalid_enum(decode_hello(h), "hello.os");
    let mut h = valid_hello();
    h.os = 3;
    assert_invalid_enum(decode_hello(h), "hello.os");
    let mut h = valid_hello();
    h.peer_side = 7;
    assert_invalid_enum(decode_hello(h), "hello.peer_side");
}

#[test]
fn hid_pages_and_ranges_are_checked() {
    let key = |hid| {
        decode_input_kind(input_frame::Kind::Key(wire::Key {
            hid,
            down: true,
            repeat: false,
        }))
    };
    assert!(key(0x07_0004).is_ok());
    assert!(key(0x0C_00CD).is_ok());
    assert_invalid_value(key(0x01_0082), "key.hid");
    assert_invalid_value(key(0x00_0004), "key.hid");
    assert_invalid_value(key(0x0107_0004), "key.hid");
    assert_invalid_value(key(u32::MAX), "key.hid");
}

#[test]
fn modifier_mask_is_eight_bits() {
    let sync =
        |mask| decode_input_kind(input_frame::Kind::ModifierSync(wire::ModifierSync { mask }));
    assert!(matches!(
        sync(0xFF),
        Ok(InputMsg::ModifierSync(ModifierMask(0xFF)))
    ));
    assert_invalid_value(sync(0x100), "modifier_sync.mask");
}

#[test]
fn screens_are_validated() {
    let with_screens = |screens: Vec<wire::Screen>| {
        let mut h = valid_hello();
        h.screens = screens;
        decode_hello(h)
    };
    assert_invalid_value(with_screens(vec![]), "hello.screens");
    let many = (0..17)
        .map(|i| wire::Screen {
            id: i,
            x: i as i32 * 2000,
            ..valid_screen()
        })
        .collect();
    assert_invalid_value(with_screens(many), "hello.screens");
    assert_invalid_value(
        with_screens(vec![valid_screen(), valid_screen()]),
        "hello.screens",
    );
    for bad in [
        wire::Screen {
            w: 0,
            ..valid_screen()
        },
        wire::Screen {
            h: 70_000,
            ..valid_screen()
        },
        wire::Screen {
            w: u32::MAX,
            ..valid_screen()
        },
        wire::Screen {
            x: 1 << 21,
            ..valid_screen()
        },
        wire::Screen {
            native_per_logical: 0.1,
            ..valid_screen()
        },
        wire::Screen {
            native_per_logical: 9.0,
            ..valid_screen()
        },
        wire::Screen {
            native_per_logical: f64::NAN,
            ..valid_screen()
        },
        wire::Screen {
            native_per_logical: f64::INFINITY,
            ..valid_screen()
        },
    ] {
        assert_invalid_value(with_screens(vec![bad]), "hello.screens");
    }
    // Two valid screens with distinct ids, one with a negative origin.
    let second = wire::Screen {
        id: 2,
        x: -1920,
        y: -200,
        w: 1920,
        h: 1080,
        native_per_logical: 1.5,
        primary: false,
    };
    assert!(with_screens(vec![valid_screen(), second]).is_ok());
}

#[test]
fn strings_and_capabilities_are_bounded() {
    let mut h = valid_hello();
    h.device_name = "x".repeat(257);
    assert_invalid_value(decode_hello(h), "hello.device_name");
    let mut h = valid_hello();
    h.device_name = "я".repeat(128); // 256 bytes: still fine
    assert!(decode_hello(h).is_ok());
    let mut h = valid_hello();
    h.os_version = "x".repeat(300);
    assert_invalid_value(decode_hello(h), "hello.os_version");
    let mut h = valid_hello();
    h.agent_version = "x".repeat(300);
    assert_invalid_value(decode_hello(h), "hello.agent_version");
    let mut h = valid_hello();
    h.capabilities = (0..33).map(|i| format!("cap{i}")).collect();
    assert_invalid_value(decode_hello(h), "hello.capabilities");
    let mut h = valid_hello();
    h.capabilities = vec!["c".repeat(65)];
    assert_invalid_value(decode_hello(h), "hello.capabilities");
}

#[test]
fn garbage_is_a_decode_error() {
    assert!(matches!(
        decode_input(&[0xFF, 0xFF, 0xFF]),
        Err(ProtoError::Decode(_))
    ));
    assert!(matches!(
        decode_control(&[0x0A, 0x05, 0x01]),
        Err(ProtoError::Decode(_))
    ));
}

#[test]
fn version_checks() {
    assert!(check_version(PROTOCOL_VERSION).is_ok());
    assert!(check_version(PROTOCOL_VERSION + 7).is_ok(), "other minor");
    assert!(matches!(
        check_version(0x0002_0000),
        Err(ProtoError::IncompatibleVersion {
            local: PROTOCOL_VERSION,
            peer: 0x0002_0000
        })
    ));
    assert!(check_version(0).is_err());
    assert!(check_versions(0, PROTOCOL_VERSION).is_err());
    assert!(check_versions(0x0002_0001, 0x0002_0005).is_ok());
    assert_eq!(version_major(0x0003_0009), 3);
}

#[test]
fn clipboard_text_round_trips_beyond_the_stream_frame_limit() {
    let text = "текст ".repeat(20_000); // ~220 KB, more than MAX_FRAME_LEN
    let m = ClipboardMsg::Text(ClipboardText {
        clock: 42,
        text: text.clone(),
    });
    let bytes = encode_clipboard(&m);
    assert!(bytes.len() > MAX_FRAME_LEN);
    assert_eq!(decode_clipboard(&bytes).unwrap(), m);
    // Debug never shows the content.
    let dbg = format!("{m:?}");
    assert!(!dbg.contains("текст") && dbg.contains(&text.len().to_string()));
}

#[test]
fn clipboard_limits_and_kinds_are_enforced() {
    assert!(matches!(
        decode_clipboard(&wire::ClipboardFrame { kind: None }.encode_to_vec()),
        Err(ProtoError::UnknownKind("clipboard frame"))
    ));
    let too_long = vec![0u8; MAX_CLIPBOARD_FRAME + 1];
    assert_invalid_value(decode_clipboard(&too_long), "clipboard frame");
    // Invalid UTF-8 in the text field is a protobuf decode error.
    let mut bad = wire::ClipboardFrame {
        kind: Some(wire::clipboard_frame::Kind::Text(wire::ClipboardText {
            clock: 1,
            text: "ab".into(),
        })),
    }
    .encode_to_vec();
    let last = bad.len() - 1;
    bad[last] = 0xFF;
    assert!(matches!(decode_clipboard(&bad), Err(ProtoError::Decode(_))));
}

#[test]
fn clipboard_images_round_trip_and_are_bounded() {
    let m = ClipboardMsg::Image(macwin_core::msg::ClipboardImage {
        clock: 3,
        png: vec![0x89, b'P', b'N', b'G'],
    });
    assert_eq!(decode_clipboard(&encode_clipboard(&m)).unwrap(), m);
    assert!(!format!("{m:?}").contains("80"), "no content in Debug");
    let big = ClipboardMsg::Image(macwin_core::msg::ClipboardImage {
        clock: 1,
        png: vec![0; MAX_CLIPBOARD_IMAGE + 1],
    });
    assert!(!clipboard_within_limits(&big));
}

fn entry(path: &str, size: u64, dir: bool) -> wire::FileEntry {
    wire::FileEntry {
        path: path.into(),
        size,
        dir,
        executable: false,
        mtime_ms: 1_700_000_000_000,
    }
}

fn decode_entries(
    entries: Vec<wire::FileEntry>,
) -> Result<macwin_core::msg::FilesHeader, ProtoError> {
    decode_files_header(&wire::FilesHeader { clock: 7, entries }.encode_to_vec())
}

#[test]
fn files_headers_round_trip() {
    let h = macwin_core::msg::FilesHeader {
        clock: 9,
        entries: vec![
            macwin_core::msg::FileEntry {
                path: "Фото".into(),
                size: 0,
                dir: true,
                executable: false,
                mtime_ms: -5,
            },
            macwin_core::msg::FileEntry {
                path: "Фото/a b.jpg".into(),
                size: 123,
                dir: false,
                executable: true,
                mtime_ms: 1_700_000_000_000,
            },
        ],
    };
    let back = decode_files_header(&encode_files_header(&h)).unwrap();
    assert_eq!(back, h);
    assert_eq!(back.total_bytes(), 123);
}

#[test]
fn files_header_paths_cannot_escape() {
    for bad in [
        "",
        "/etc/passwd",
        "a//b",
        "a/",
        "./a",
        "a/./b",
        "../a",
        "a/../../b",
        "a\\b",
        "a\0b",
        "C:",
        "c:/windows",
        &"x".repeat(256),
        &"a/"
            .repeat(64)
            .trim_end_matches('/')
            .replace("a/a", "a/a/a"),
    ] {
        assert_invalid_value(
            decode_entries(vec![entry(bad, 0, false)]),
            "file_entry.path",
        );
    }
    // Long but valid, and names that only Windows dislikes (mapped later).
    assert!(decode_entries(vec![entry("a/".repeat(63).trim_end_matches('/'), 0, false)]).is_ok());
    assert!(decode_entries(vec![entry("what?.txt", 1, false), entry("a:b", 1, false)]).is_ok());
}

#[test]
fn files_header_structure_is_checked() {
    assert_invalid_value(decode_entries(vec![entry("d", 5, true)]), "file_entry.size");
    assert_invalid_value(
        decode_entries(vec![entry("a", 1, false), entry("a", 2, false)]),
        "file_entry.path",
    );
    assert_invalid_value(
        decode_entries(vec![
            entry("a", MAX_FILES_TOTAL, false),
            entry("b", 1, false),
        ]),
        "files_header.entries",
    );
    assert_invalid_value(
        decode_entries(vec![entry("a", u64::MAX, false), entry("b", 2, false)]),
        "files_header.entries",
    );
    let many = (0..=MAX_FILE_ENTRIES)
        .map(|i| entry(&format!("f{i}"), 0, false))
        .collect();
    assert_invalid_value(decode_entries(many), "files_header.entries");
    assert_invalid_value(decode_entries(vec![]), "files_header.entries");
    assert!(decode_files_header(&vec![0u8; MAX_FILES_HEADER + 1]).is_err());
}

#[test]
fn packed_empty_entries_are_refused_before_decoding() {
    // Tag 2, length 0: the smallest possible entry, repeated past the limit.
    let mut b = vec![0x08, 0x07];
    for _ in 0..=MAX_FILE_ENTRIES {
        b.extend_from_slice(&[0x12, 0x00]);
    }
    assert_invalid_value(decode_files_header(&b), "files_header.entries");
    // Truncated and group-encoded fields are malformed.
    assert_invalid_value(decode_files_header(&[0x12, 0x05, 0x01]), "files header");
    assert_invalid_value(decode_files_header(&[0x13]), "files header");
}

fn valid_pair_hello() -> wire::PairHello {
    wire::PairHello {
        protocol_version: PROTOCOL_VERSION,
        os: wire::Os::Windows as i32,
        device_name: "pc".into(),
        agent_version: "0.1.0".into(),
        peer_side: wire::Side::Left as i32,
        spake: vec![0x41; SPAKE_MSG_LEN],
    }
}

fn decode_pair_kind(k: wire::pair_frame::Kind) -> Result<PairMsg, ProtoError> {
    decode_pair(&wire::PairFrame { kind: Some(k) }.encode_to_vec())
}

#[test]
fn pair_messages_round_trip() {
    let hello = PairMsg::Hello(PairHello {
        protocol_version: PROTOCOL_VERSION,
        os: Os::MacOs,
        device_name: "Dmitriy's MacBook Pro".into(),
        agent_version: "0.1.0".into(),
        peer_side: None,
        spake: vec![7; SPAKE_MSG_LEN],
    });
    let confirm = PairMsg::Confirm(PairConfirm { mac: [9; 32] });
    for m in [hello, confirm, PairMsg::Done] {
        assert_eq!(decode_pair(&encode_pair(&m)).unwrap(), m);
    }
}

#[test]
fn pair_messages_are_validated() {
    assert!(matches!(
        decode_pair(&wire::PairFrame { kind: None }.encode_to_vec()),
        Err(ProtoError::UnknownKind("pair frame"))
    ));
    let mut h = valid_pair_hello();
    h.spake.pop();
    assert_invalid_value(
        decode_pair_kind(wire::pair_frame::Kind::Hello(h)),
        "pair_hello.spake",
    );
    let mut h = valid_pair_hello();
    h.os = 0;
    assert_invalid_enum(
        decode_pair_kind(wire::pair_frame::Kind::Hello(h)),
        "pair_hello.os",
    );
    let mut h = valid_pair_hello();
    h.peer_side = 9;
    assert_invalid_enum(
        decode_pair_kind(wire::pair_frame::Kind::Hello(h)),
        "pair_hello.peer_side",
    );
    let mut h = valid_pair_hello();
    h.device_name = "x".repeat(257);
    assert_invalid_value(
        decode_pair_kind(wire::pair_frame::Kind::Hello(h)),
        "pair_hello.device_name",
    );
    let PairMsg::Hello(ok) =
        decode_pair_kind(wire::pair_frame::Kind::Hello(valid_pair_hello())).unwrap()
    else {
        panic!("not a hello")
    };
    assert_eq!((ok.os, ok.peer_side), (Os::Windows, Some(Side::Left)));
    for len in [0, 31, 33] {
        assert_invalid_value(
            decode_pair_kind(wire::pair_frame::Kind::Confirm(wire::PairConfirm {
                mac: vec![1; len],
            })),
            "pair_confirm.mac",
        );
    }
}

// ---- property tests ----

fn arb_string() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            prop::char::range('a', 'z'),
            prop::char::range('а', 'я'),
            Just(' ')
        ],
        0..40,
    )
    .prop_map(|v| v.into_iter().collect())
}

fn arb_point() -> impl Strategy<Value = Point> {
    (any::<i32>(), any::<i32>()).prop_map(|(x, y)| Point::new(x, y))
}

fn arb_hid() -> impl Strategy<Value = Hid> {
    prop_oneof![
        (0u32..=0xFFFF).prop_map(|u| Hid(0x07_0000 | u)),
        (0u32..=0xFFFF).prop_map(|u| Hid(0x0C_0000 | u)),
    ]
}

fn arb_button() -> impl Strategy<Value = MouseButton> {
    prop::sample::select(MouseButton::ALL.to_vec())
}

fn arb_role() -> impl Strategy<Value = RoleKind> {
    prop::sample::select(vec![
        RoleKind::Idle,
        RoleKind::Controlling,
        RoleKind::Controlled,
    ])
}

fn arb_ping() -> impl Strategy<Value = Ping> {
    (any::<u64>(), any::<u64>()).prop_map(|(id, sent_micros)| Ping { id, sent_micros })
}

fn arb_pong() -> impl Strategy<Value = Pong> {
    (any::<u64>(), any::<u64>()).prop_map(|(id, ping_sent_micros)| Pong {
        id,
        ping_sent_micros,
    })
}

fn arb_screens() -> impl Strategy<Value = Vec<ScreenInfo>> {
    prop::collection::vec(
        (
            -(1i32 << 20)..=(1 << 20),
            -(1i32 << 20)..=(1 << 20),
            1i32..=65_536,
            1i32..=65_536,
            prop::sample::select(vec![0.25, 1.0, 1.25, 1.5, 2.0, 8.0]),
            any::<bool>(),
        ),
        1..=16,
    )
    .prop_map(|v| {
        v.into_iter()
            .enumerate()
            .map(|(i, (x, y, w, h, npl, primary))| ScreenInfo {
                id: i as u32 * 3 + 1,
                rect: Rect::new(x, y, w, h).unwrap(),
                native_per_logical: npl,
                primary,
            })
            .collect()
    })
}

fn arb_hello() -> impl Strategy<Value = Hello> {
    (
        any::<u32>(),
        prop::collection::vec(arb_string(), 0..5),
        any::<bool>(),
        arb_string(),
        arb_string(),
        arb_screens(),
        prop::option::of(prop::sample::select(vec![Side::Left, Side::Right])),
        arb_string(),
    )
        .prop_map(
            |(
                protocol_version,
                capabilities,
                mac,
                os_version,
                device_name,
                screens,
                peer_side,
                agent_version,
            )| {
                Hello {
                    protocol_version,
                    capabilities,
                    os: if mac { Os::MacOs } else { Os::Windows },
                    os_version,
                    device_name,
                    screens,
                    peer_side,
                    agent_version,
                }
            },
        )
}

fn arb_control() -> impl Strategy<Value = ControlMsg> {
    prop_oneof![
        arb_hello().prop_map(ControlMsg::Hello),
        (any::<u64>(), arb_role())
            .prop_map(|(epoch, role)| ControlMsg::Heartbeat(Heartbeat { epoch, role })),
        arb_ping().prop_map(ControlMsg::Ping),
        arb_pong().prop_map(ControlMsg::Pong),
    ]
}

fn arb_input() -> impl Strategy<Value = InputMsg> {
    prop_oneof![
        (any::<u64>(), arb_point()).prop_map(|(epoch, at)| InputMsg::Enter(Enter { epoch, at })),
        any::<u64>().prop_map(|epoch| InputMsg::Leave(Leave { epoch })),
        any::<u64>().prop_map(|epoch| InputMsg::TakeControl(TakeControl { epoch })),
        (arb_hid(), any::<bool>(), any::<bool>())
            .prop_map(|(hid, down, repeat)| InputMsg::Key(KeyMsg { hid, down, repeat })),
        (arb_button(), any::<bool>(), arb_point())
            .prop_map(|(button, down, at)| InputMsg::Button(ButtonMsg { button, down, at })),
        (
            any::<i32>(),
            any::<i32>(),
            prop::sample::select(vec![WheelUnit::Notch120, WheelUnit::Pixel])
        )
            .prop_map(|(dx, dy, unit)| InputMsg::Wheel(WheelMsg { dx, dy, unit })),
        any::<u8>().prop_map(|m| InputMsg::ModifierSync(ModifierMask(m))),
        arb_ping().prop_map(InputMsg::Ping),
        arb_pong().prop_map(InputMsg::Pong),
    ]
}

fn arb_datagram() -> impl Strategy<Value = DatagramMsg> {
    prop_oneof![
        (any::<u64>(), any::<u64>(), arb_point())
            .prop_map(|(epoch, seq, at)| DatagramMsg::MouseAbs(MouseAbs { epoch, seq, at })),
        arb_ping().prop_map(DatagramMsg::Ping),
        arb_pong().prop_map(DatagramMsg::Pong),
    ]
}

proptest! {
    #[test]
    fn prop_control_round_trip(m in arb_control()) {
        let bytes = encode_control(&m);
        prop_assert!(bytes.len() <= MAX_FRAME_LEN);
        prop_assert_eq!(decode_control(&bytes).unwrap(), m);
    }

    #[test]
    fn prop_input_round_trip(m in arb_input()) {
        prop_assert_eq!(decode_input(&encode_input(&m)).unwrap(), m);
    }

    #[test]
    fn prop_datagram_round_trip(m in arb_datagram()) {
        prop_assert_eq!(decode_datagram(&encode_datagram(&m)).unwrap(), m);
    }

    #[test]
    fn prop_clipboard_round_trip(clock in any::<u64>(), text in arb_string(), other in any::<bool>()) {
        let m = if other {
            ClipboardMsg::Other { clock }
        } else {
            ClipboardMsg::Text(ClipboardText { clock, text })
        };
        prop_assert_eq!(decode_clipboard(&encode_clipboard(&m)).unwrap(), m);
    }

    #[test]
    fn prop_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_control(&bytes);
        let _ = decode_input(&bytes);
        let _ = decode_datagram(&bytes);
        let _ = decode_clipboard(&bytes);
        let _ = decode_pair(&bytes);
        let _ = decode_discovery(&bytes);
        // Discovery datagrams come from anyone on the LAN.
        let mut magic = DISCOVERY_MAGIC.to_vec();
        magic.extend_from_slice(&bytes);
        let _ = decode_discovery(&magic);
    }
}

fn discovery_query() -> macwin_core::msg::DiscoveryMsg {
    use macwin_core::msg::{DiscoveryMsg, DiscoveryQuery, DiscoveryWant};
    DiscoveryMsg::Query(DiscoveryQuery {
        nonce: [7; 16],
        want: DiscoveryWant::Peer { tag: [9; 32] },
    })
}

#[test]
fn discovery_round_trips_and_checks_its_envelope() {
    use macwin_core::msg::{DiscoveryAnswer, DiscoveryMsg, DiscoveryOffer};
    let answers = [
        DiscoveryOffer::Pairing {
            os: Os::Windows,
            device_name: "DESKTOP-1".into(),
        },
        DiscoveryOffer::Peer { proof: [3; 32] },
    ]
    .map(|offer| {
        DiscoveryMsg::Answer(DiscoveryAnswer {
            nonce: [1; 16],
            protocol_version: PROTOCOL_VERSION,
            port: 47800,
            offer,
        })
    });
    for m in answers.into_iter().chain([discovery_query()]) {
        let bytes = encode_discovery(&m);
        assert!(bytes.starts_with(&DISCOVERY_MAGIC) && bytes.len() <= MAX_DISCOVERY_LEN);
        assert_eq!(decode_discovery(&bytes).unwrap(), m);
    }
    let mut bytes = encode_discovery(&discovery_query());
    bytes[0] = b'X';
    assert_invalid_value(decode_discovery(&bytes), "discovery datagram");
    let mut long = encode_discovery(&discovery_query());
    long.resize(MAX_DISCOVERY_LEN + 1, 0);
    assert_invalid_value(decode_discovery(&long), "discovery datagram");
    assert!(decode_discovery(b"MWD1\xff\xff").is_err());
}

#[test]
fn discovery_fields_are_validated() {
    use crate::wire::{discovery_answer, discovery_frame, discovery_query};
    let frame = |kind| {
        let mut out = DISCOVERY_MAGIC.to_vec();
        wire::DiscoveryFrame { kind: Some(kind) }
            .encode(&mut out)
            .unwrap();
        out
    };
    let query = |nonce: Vec<u8>, want| {
        frame(discovery_frame::Kind::Query(wire::DiscoveryQuery {
            nonce,
            want,
        }))
    };
    let pairing = Some(discovery_query::Want::Pairing(
        wire::DiscoveryWantPairing {},
    ));
    assert!(decode_discovery(&query(vec![0; 16], pairing.clone())).is_ok());
    assert_invalid_value(
        decode_discovery(&query(vec![0; 15], pairing)),
        "discovery_query.nonce",
    );
    assert!(matches!(
        decode_discovery(&query(vec![0; 16], None)),
        Err(ProtoError::UnknownKind(_))
    ));
    let short_tag = Some(discovery_query::Want::Peer(wire::DiscoveryWantPeer {
        tag: vec![0; 31],
    }));
    assert_invalid_value(
        decode_discovery(&query(vec![0; 16], short_tag)),
        "discovery_query.peer.tag",
    );
    let answer = |port: u32, offer| {
        frame(discovery_frame::Kind::Answer(wire::DiscoveryAnswer {
            nonce: vec![0; 16],
            protocol_version: PROTOCOL_VERSION,
            port,
            offer,
        }))
    };
    let pairing = |os: i32, device_name: String| {
        Some(discovery_answer::Offer::Pairing(wire::DiscoveryPairing {
            os,
            device_name,
        }))
    };
    assert!(decode_discovery(&answer(47800, pairing(1, "mac".into()))).is_ok());
    for port in [0, 65536] {
        assert_invalid_value(
            decode_discovery(&answer(port, pairing(1, "mac".into()))),
            "discovery_answer.port",
        );
    }
    assert_invalid_enum(
        decode_discovery(&answer(47800, pairing(0, "mac".into()))),
        "discovery_answer.pairing.os",
    );
    assert_invalid_value(
        decode_discovery(&answer(47800, pairing(1, "x".repeat(257)))),
        "discovery_answer.pairing.device_name",
    );
    let proof = Some(discovery_answer::Offer::Peer(wire::DiscoveryPeer {
        proof: vec![0; 33],
    }));
    assert_invalid_value(
        decode_discovery(&answer(47800, proof)),
        "discovery_answer.peer.proof",
    );
    assert!(matches!(
        decode_discovery(&answer(47800, None)),
        Err(ProtoError::UnknownKind(_))
    ));
}
