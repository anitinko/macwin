//! Domain <-> wire conversion. Encoding is infallible; decoding validates
//! every field ("parse, don't validate"), so the rest of macwin only ever
//! sees well-formed domain values.

use macwin_core::geometry::ScreenSet;
use macwin_core::msg::{
    ButtonMsg, ClipboardImage, ClipboardMsg, ClipboardText, ControlMsg, DISCOVERY_NONCE_LEN,
    DatagramMsg, DiscoveryAnswer, DiscoveryMsg, DiscoveryOffer, DiscoveryQuery, DiscoveryWant,
    Enter, FileEntry, FilesHeader, Heartbeat, Hello, InputMsg, KeyMsg, Leave, MouseAbs,
    PairConfirm, PairHello, PairMsg, Ping, Pong, TakeControl, WheelMsg,
};
use macwin_core::types::{
    GeometryError, Hid, ModifierMask, MouseButton, Os, Point, Rect, RoleKind, ScreenInfo, Side,
    WheelUnit,
};

use crate::wire::{
    self, clipboard_frame, control_frame, datagram_frame, discovery_answer, discovery_frame,
    discovery_query, input_frame, pair_frame,
};
use crate::{
    MAX_CAPABILITIES, MAX_CAPABILITY_LEN, MAX_CLIPBOARD_IMAGE, MAX_CLIPBOARD_TEXT, MAX_FILE_DEPTH,
    MAX_FILE_ENTRIES, MAX_FILE_PATH_LEN, MAX_FILES_TOTAL, MAX_STRING_LEN, PAIR_MAC_LEN, ProtoError,
    SPAKE_MSG_LEN,
};

// ---------- domain -> wire ----------

fn point_to_wire(p: Point) -> wire::Point {
    wire::Point { x: p.x, y: p.y }
}

fn os_to_wire(os: Os) -> wire::Os {
    match os {
        Os::MacOs => wire::Os::Macos,
        Os::Windows => wire::Os::Windows,
    }
}

fn side_to_wire(side: Option<Side>) -> wire::Side {
    match side {
        None => wire::Side::Unspecified,
        Some(Side::Left) => wire::Side::Left,
        Some(Side::Right) => wire::Side::Right,
    }
}

fn role_to_wire(role: RoleKind) -> wire::Role {
    match role {
        RoleKind::Idle => wire::Role::Idle,
        RoleKind::Controlling => wire::Role::Controlling,
        RoleKind::Controlled => wire::Role::Controlled,
    }
}

fn button_to_wire(b: MouseButton) -> wire::MouseButton {
    match b {
        MouseButton::Left => wire::MouseButton::Left,
        MouseButton::Right => wire::MouseButton::Right,
        MouseButton::Middle => wire::MouseButton::Middle,
        MouseButton::Back => wire::MouseButton::Back,
        MouseButton::Forward => wire::MouseButton::Forward,
    }
}

fn unit_to_wire(u: WheelUnit) -> wire::WheelUnit {
    match u {
        WheelUnit::Notch120 => wire::WheelUnit::Notch120,
        WheelUnit::Pixel => wire::WheelUnit::Pixel,
    }
}

fn ping_to_wire(p: Ping) -> wire::Ping {
    wire::Ping {
        id: p.id,
        sent_micros: p.sent_micros,
    }
}

fn pong_to_wire(p: Pong) -> wire::Pong {
    wire::Pong {
        id: p.id,
        ping_sent_micros: p.ping_sent_micros,
    }
}

fn hello_to_wire(h: &Hello) -> wire::Hello {
    wire::Hello {
        protocol_version: h.protocol_version,
        capabilities: h.capabilities.clone(),
        os: os_to_wire(h.os) as i32,
        os_version: h.os_version.clone(),
        device_name: h.device_name.clone(),
        screens: h
            .screens
            .iter()
            .map(|s| wire::Screen {
                id: s.id,
                x: s.rect.left(),
                y: s.rect.top(),
                w: s.rect.width().unsigned_abs(),
                h: s.rect.height().unsigned_abs(),
                native_per_logical: s.native_per_logical,
                primary: s.primary,
            })
            .collect(),
        peer_side: side_to_wire(h.peer_side) as i32,
        agent_version: h.agent_version.clone(),
    }
}

pub(crate) fn control_to_wire(m: &ControlMsg) -> wire::ControlFrame {
    let kind = match m {
        ControlMsg::Hello(h) => control_frame::Kind::Hello(hello_to_wire(h)),
        ControlMsg::Heartbeat(hb) => control_frame::Kind::Heartbeat(wire::Heartbeat {
            epoch: hb.epoch,
            role: role_to_wire(hb.role) as i32,
        }),
        ControlMsg::Ping(p) => control_frame::Kind::Ping(ping_to_wire(*p)),
        ControlMsg::Pong(p) => control_frame::Kind::Pong(pong_to_wire(*p)),
    };
    wire::ControlFrame { kind: Some(kind) }
}

pub(crate) fn input_to_wire(m: &InputMsg) -> wire::InputFrame {
    let kind = match *m {
        InputMsg::Enter(e) => input_frame::Kind::Enter(wire::Enter {
            epoch: e.epoch,
            at: Some(point_to_wire(e.at)),
        }),
        InputMsg::Leave(l) => input_frame::Kind::Leave(wire::Leave { epoch: l.epoch }),
        InputMsg::TakeControl(t) => {
            input_frame::Kind::TakeControl(wire::TakeControl { epoch: t.epoch })
        }
        InputMsg::Key(k) => input_frame::Kind::Key(wire::Key {
            hid: k.hid.0,
            down: k.down,
            repeat: k.repeat,
        }),
        InputMsg::Button(b) => input_frame::Kind::Button(wire::Button {
            button: button_to_wire(b.button) as i32,
            down: b.down,
            at: Some(point_to_wire(b.at)),
        }),
        InputMsg::Wheel(w) => input_frame::Kind::Wheel(wire::Wheel {
            dx: w.dx,
            dy: w.dy,
            unit: unit_to_wire(w.unit) as i32,
        }),
        InputMsg::ModifierSync(m) => input_frame::Kind::ModifierSync(wire::ModifierSync {
            mask: u32::from(m.0),
        }),
        InputMsg::Ping(p) => input_frame::Kind::Ping(ping_to_wire(p)),
        InputMsg::Pong(p) => input_frame::Kind::Pong(pong_to_wire(p)),
    };
    wire::InputFrame { kind: Some(kind) }
}

pub(crate) fn clipboard_to_wire(m: &ClipboardMsg) -> wire::ClipboardFrame {
    let kind = match m {
        ClipboardMsg::Text(t) => clipboard_frame::Kind::Text(wire::ClipboardText {
            clock: t.clock,
            text: t.text.clone(),
        }),
        ClipboardMsg::Other { clock } => {
            clipboard_frame::Kind::Other(wire::ClipboardOther { clock: *clock })
        }
        ClipboardMsg::Image(i) => clipboard_frame::Kind::Image(wire::ClipboardImage {
            clock: i.clock,
            png: i.png.clone(),
        }),
    };
    wire::ClipboardFrame { kind: Some(kind) }
}

pub(crate) fn pair_to_wire(m: &PairMsg) -> wire::PairFrame {
    let kind = match m {
        PairMsg::Hello(h) => pair_frame::Kind::Hello(wire::PairHello {
            protocol_version: h.protocol_version,
            os: os_to_wire(h.os) as i32,
            device_name: h.device_name.clone(),
            agent_version: h.agent_version.clone(),
            peer_side: side_to_wire(h.peer_side) as i32,
            spake: h.spake.clone(),
        }),
        PairMsg::Confirm(c) => pair_frame::Kind::Confirm(wire::PairConfirm {
            mac: c.mac.to_vec(),
        }),
        PairMsg::Done => pair_frame::Kind::Done(wire::PairDone {}),
    };
    wire::PairFrame { kind: Some(kind) }
}

pub(crate) fn datagram_to_wire(m: &DatagramMsg) -> wire::DatagramFrame {
    let kind = match *m {
        DatagramMsg::MouseAbs(a) => datagram_frame::Kind::MouseAbs(wire::MouseAbs {
            epoch: a.epoch,
            seq: a.seq,
            at: Some(point_to_wire(a.at)),
        }),
        DatagramMsg::Ping(p) => datagram_frame::Kind::Ping(ping_to_wire(p)),
        DatagramMsg::Pong(p) => datagram_frame::Kind::Pong(pong_to_wire(p)),
    };
    wire::DatagramFrame { kind: Some(kind) }
}

// ---------- wire -> domain ----------

fn point_from_wire(p: Option<wire::Point>, field: &'static str) -> Result<Point, ProtoError> {
    p.map(|p| Point::new(p.x, p.y))
        .ok_or(ProtoError::MissingField(field))
}

fn os_from_wire(v: i32, field: &'static str) -> Result<Os, ProtoError> {
    match wire::Os::try_from(v) {
        Ok(wire::Os::Macos) => Ok(Os::MacOs),
        Ok(wire::Os::Windows) => Ok(Os::Windows),
        _ => Err(ProtoError::InvalidEnum { field, value: v }),
    }
}

fn side_from_wire(v: i32, field: &'static str) -> Result<Option<Side>, ProtoError> {
    match wire::Side::try_from(v) {
        Ok(wire::Side::Unspecified) => Ok(None),
        Ok(wire::Side::Left) => Ok(Some(Side::Left)),
        Ok(wire::Side::Right) => Ok(Some(Side::Right)),
        Err(_) => Err(ProtoError::InvalidEnum { field, value: v }),
    }
}

fn role_from_wire(v: i32) -> Result<RoleKind, ProtoError> {
    match wire::Role::try_from(v) {
        Ok(wire::Role::Idle) => Ok(RoleKind::Idle),
        Ok(wire::Role::Controlling) => Ok(RoleKind::Controlling),
        Ok(wire::Role::Controlled) => Ok(RoleKind::Controlled),
        _ => Err(ProtoError::InvalidEnum {
            field: "heartbeat.role",
            value: v,
        }),
    }
}

fn button_from_wire(v: i32) -> Result<MouseButton, ProtoError> {
    match wire::MouseButton::try_from(v) {
        Ok(wire::MouseButton::Left) => Ok(MouseButton::Left),
        Ok(wire::MouseButton::Right) => Ok(MouseButton::Right),
        Ok(wire::MouseButton::Middle) => Ok(MouseButton::Middle),
        Ok(wire::MouseButton::Back) => Ok(MouseButton::Back),
        Ok(wire::MouseButton::Forward) => Ok(MouseButton::Forward),
        _ => Err(ProtoError::InvalidEnum {
            field: "button.button",
            value: v,
        }),
    }
}

fn unit_from_wire(v: i32) -> Result<WheelUnit, ProtoError> {
    match wire::WheelUnit::try_from(v) {
        Ok(wire::WheelUnit::Notch120) => Ok(WheelUnit::Notch120),
        Ok(wire::WheelUnit::Pixel) => Ok(WheelUnit::Pixel),
        _ => Err(ProtoError::InvalidEnum {
            field: "wheel.unit",
            value: v,
        }),
    }
}

fn hid_from_wire(v: u32) -> Result<Hid, ProtoError> {
    let hid = Hid(v);
    if v > 0x00FF_FFFF || !matches!(hid.page(), 0x07 | 0x0C) {
        return Err(ProtoError::InvalidValue {
            field: "key.hid",
            reason: "usage page must be 0x07 or 0x0C",
        });
    }
    Ok(hid)
}

fn mask_from_wire(v: u32) -> Result<ModifierMask, ProtoError> {
    u8::try_from(v)
        .map(ModifierMask)
        .map_err(|_| ProtoError::InvalidValue {
            field: "modifier_sync.mask",
            reason: "mask exceeds 8 bits",
        })
}

fn string_from_wire(s: String, field: &'static str) -> Result<String, ProtoError> {
    if s.len() > MAX_STRING_LEN {
        return Err(ProtoError::InvalidValue {
            field,
            reason: "string longer than 256 bytes",
        });
    }
    Ok(s)
}

fn geometry_reason(e: &GeometryError) -> &'static str {
    match e {
        GeometryError::InvalidSize { .. } => "screen size outside 1..=65536",
        GeometryError::InvalidOrigin { .. } => "screen origin outside +-2^20",
        GeometryError::NoScreens => "no screens",
        GeometryError::TooManyScreens(_) => "more than 16 screens",
        GeometryError::DuplicateScreenId(_) => "duplicate screen id",
        GeometryError::InvalidScale(_) => "native_per_logical outside 0.25..=8",
    }
}

fn screens_from_wire(screens: Vec<wire::Screen>) -> Result<Vec<ScreenInfo>, ProtoError> {
    let invalid = |e: GeometryError| ProtoError::InvalidValue {
        field: "hello.screens",
        reason: geometry_reason(&e),
    };
    if screens.len() > macwin_core::geometry::MAX_SCREENS {
        return Err(invalid(GeometryError::TooManyScreens(screens.len())));
    }
    let mut out = Vec::with_capacity(screens.len());
    for s in screens {
        let w = i32::try_from(s.w).unwrap_or(i32::MAX);
        let h = i32::try_from(s.h).unwrap_or(i32::MAX);
        out.push(ScreenInfo {
            id: s.id,
            rect: Rect::new(s.x, s.y, w, h).map_err(invalid)?,
            native_per_logical: s.native_per_logical,
            primary: s.primary,
        });
    }
    // Count, unique ids and scale range.
    ScreenSet::new(out.clone()).map_err(invalid)?;
    Ok(out)
}

fn hello_from_wire(h: wire::Hello) -> Result<Hello, ProtoError> {
    if h.capabilities.len() > MAX_CAPABILITIES
        || h.capabilities.iter().any(|c| c.len() > MAX_CAPABILITY_LEN)
    {
        return Err(ProtoError::InvalidValue {
            field: "hello.capabilities",
            reason: "at most 32 capabilities of at most 64 bytes",
        });
    }
    Ok(Hello {
        protocol_version: h.protocol_version,
        capabilities: h.capabilities,
        os: os_from_wire(h.os, "hello.os")?,
        os_version: string_from_wire(h.os_version, "hello.os_version")?,
        device_name: string_from_wire(h.device_name, "hello.device_name")?,
        screens: screens_from_wire(h.screens)?,
        peer_side: side_from_wire(h.peer_side, "hello.peer_side")?,
        agent_version: string_from_wire(h.agent_version, "hello.agent_version")?,
    })
}

fn ping_from_wire(p: wire::Ping) -> Ping {
    Ping {
        id: p.id,
        sent_micros: p.sent_micros,
    }
}

fn pong_from_wire(p: wire::Pong) -> Pong {
    Pong {
        id: p.id,
        ping_sent_micros: p.ping_sent_micros,
    }
}

pub(crate) fn control_from_wire(f: wire::ControlFrame) -> Result<ControlMsg, ProtoError> {
    match f.kind.ok_or(ProtoError::UnknownKind("control frame"))? {
        control_frame::Kind::Hello(h) => Ok(ControlMsg::Hello(hello_from_wire(h)?)),
        control_frame::Kind::Heartbeat(hb) => Ok(ControlMsg::Heartbeat(Heartbeat {
            epoch: hb.epoch,
            role: role_from_wire(hb.role)?,
        })),
        control_frame::Kind::Ping(p) => Ok(ControlMsg::Ping(ping_from_wire(p))),
        control_frame::Kind::Pong(p) => Ok(ControlMsg::Pong(pong_from_wire(p))),
    }
}

pub(crate) fn input_from_wire(f: wire::InputFrame) -> Result<InputMsg, ProtoError> {
    Ok(
        match f.kind.ok_or(ProtoError::UnknownKind("input frame"))? {
            input_frame::Kind::Enter(e) => InputMsg::Enter(Enter {
                epoch: e.epoch,
                at: point_from_wire(e.at, "enter.at")?,
            }),
            input_frame::Kind::Leave(l) => InputMsg::Leave(Leave { epoch: l.epoch }),
            input_frame::Kind::TakeControl(t) => {
                InputMsg::TakeControl(TakeControl { epoch: t.epoch })
            }
            input_frame::Kind::Key(k) => InputMsg::Key(KeyMsg {
                hid: hid_from_wire(k.hid)?,
                down: k.down,
                repeat: k.repeat,
            }),
            input_frame::Kind::Button(b) => InputMsg::Button(ButtonMsg {
                button: button_from_wire(b.button)?,
                down: b.down,
                at: point_from_wire(b.at, "button.at")?,
            }),
            input_frame::Kind::Wheel(w) => InputMsg::Wheel(WheelMsg {
                dx: w.dx,
                dy: w.dy,
                unit: unit_from_wire(w.unit)?,
            }),
            input_frame::Kind::ModifierSync(m) => InputMsg::ModifierSync(mask_from_wire(m.mask)?),
            input_frame::Kind::Ping(p) => InputMsg::Ping(ping_from_wire(p)),
            input_frame::Kind::Pong(p) => InputMsg::Pong(pong_from_wire(p)),
        },
    )
}

pub(crate) fn datagram_from_wire(f: wire::DatagramFrame) -> Result<DatagramMsg, ProtoError> {
    Ok(
        match f.kind.ok_or(ProtoError::UnknownKind("datagram frame"))? {
            datagram_frame::Kind::MouseAbs(a) => DatagramMsg::MouseAbs(MouseAbs {
                epoch: a.epoch,
                seq: a.seq,
                at: point_from_wire(a.at, "mouse_abs.at")?,
            }),
            datagram_frame::Kind::Ping(p) => DatagramMsg::Ping(ping_from_wire(p)),
            datagram_frame::Kind::Pong(p) => DatagramMsg::Pong(pong_from_wire(p)),
        },
    )
}

pub(crate) fn clipboard_from_wire(f: wire::ClipboardFrame) -> Result<ClipboardMsg, ProtoError> {
    match f.kind.ok_or(ProtoError::UnknownKind("clipboard frame"))? {
        clipboard_frame::Kind::Text(t) => {
            if t.text.len() > MAX_CLIPBOARD_TEXT {
                return Err(ProtoError::InvalidValue {
                    field: "clipboard_text.text",
                    reason: "text longer than 16 MiB",
                });
            }
            Ok(ClipboardMsg::Text(ClipboardText {
                clock: t.clock,
                text: t.text,
            }))
        }
        clipboard_frame::Kind::Other(o) => Ok(ClipboardMsg::Other { clock: o.clock }),
        clipboard_frame::Kind::Image(i) => {
            if i.png.len() > MAX_CLIPBOARD_IMAGE {
                return Err(ProtoError::InvalidValue {
                    field: "clipboard_image.png",
                    reason: "image larger than 64 MiB",
                });
            }
            Ok(ClipboardMsg::Image(ClipboardImage {
                clock: i.clock,
                png: i.png,
            }))
        }
    }
}

pub(crate) fn files_header_to_wire(h: &FilesHeader) -> wire::FilesHeader {
    wire::FilesHeader {
        clock: h.clock,
        entries: h
            .entries
            .iter()
            .map(|e| wire::FileEntry {
                path: e.path.clone(),
                size: e.size,
                dir: e.dir,
                executable: e.executable,
                mtime_ms: e.mtime_ms,
            })
            .collect(),
    }
}

fn invalid_path(reason: &'static str) -> ProtoError {
    ProtoError::InvalidValue {
        field: "file_entry.path",
        reason,
    }
}

/// A relative `/`-separated path that cannot escape the receiving
/// directory on any OS. Name characters that only Windows rejects are
/// mapped by the receiver, not refused here.
pub fn check_relative_path(path: &str) -> Result<(), ProtoError> {
    if path.is_empty() || path.len() > MAX_FILE_PATH_LEN {
        return Err(invalid_path("empty or longer than 4096 bytes"));
    }
    if path.contains(['\\', '\0']) {
        return Err(invalid_path("contains a backslash or NUL"));
    }
    let mut depth = 0;
    for component in path.split('/') {
        depth += 1;
        if component.is_empty() || component == "." || component == ".." {
            return Err(invalid_path("empty, '.' or '..' component"));
        }
        if component.len() > 255 {
            return Err(invalid_path("component longer than 255 bytes"));
        }
        // A drive or stream prefix ("C:", "a:b") must not reach Windows paths
        // unmapped; ':' is a plain character on macOS and is mapped there.
        if depth == 1 && component.len() == 2 && component.as_bytes()[1] == b':' {
            return Err(invalid_path("drive prefix"));
        }
    }
    if depth > MAX_FILE_DEPTH {
        return Err(invalid_path("nested deeper than 64 levels"));
    }
    Ok(())
}

pub(crate) fn files_header_from_wire(h: wire::FilesHeader) -> Result<FilesHeader, ProtoError> {
    if h.entries.len() > MAX_FILE_ENTRIES {
        return Err(ProtoError::InvalidValue {
            field: "files_header.entries",
            reason: "more than 65536 entries",
        });
    }
    if h.entries.is_empty() {
        return Err(ProtoError::InvalidValue {
            field: "files_header.entries",
            reason: "no entries",
        });
    }
    let mut seen = std::collections::HashSet::with_capacity(h.entries.len());
    let mut total: u64 = 0;
    let mut entries = Vec::with_capacity(h.entries.len());
    for e in h.entries {
        check_relative_path(&e.path)?;
        if e.dir && e.size != 0 {
            return Err(ProtoError::InvalidValue {
                field: "file_entry.size",
                reason: "a directory carries data",
            });
        }
        if !seen.insert(e.path.clone()) {
            return Err(invalid_path("duplicate path"));
        }
        total = total
            .checked_add(e.size)
            .filter(|t| *t <= MAX_FILES_TOTAL)
            .ok_or(ProtoError::InvalidValue {
                field: "files_header.entries",
                reason: "more than 64 GiB in total",
            })?;
        entries.push(FileEntry {
            path: e.path,
            size: e.size,
            dir: e.dir,
            executable: e.executable,
            mtime_ms: e.mtime_ms,
        });
    }
    Ok(FilesHeader {
        clock: h.clock,
        entries,
    })
}

pub(crate) fn pair_from_wire(f: wire::PairFrame) -> Result<PairMsg, ProtoError> {
    match f.kind.ok_or(ProtoError::UnknownKind("pair frame"))? {
        pair_frame::Kind::Hello(h) => {
            if h.spake.len() != SPAKE_MSG_LEN {
                return Err(ProtoError::InvalidValue {
                    field: "pair_hello.spake",
                    reason: "SPAKE2 message must be 33 bytes",
                });
            }
            Ok(PairMsg::Hello(PairHello {
                protocol_version: h.protocol_version,
                os: os_from_wire(h.os, "pair_hello.os")?,
                device_name: string_from_wire(h.device_name, "pair_hello.device_name")?,
                agent_version: string_from_wire(h.agent_version, "pair_hello.agent_version")?,
                peer_side: side_from_wire(h.peer_side, "pair_hello.peer_side")?,
                spake: h.spake,
            }))
        }
        pair_frame::Kind::Confirm(c) => {
            let mac: [u8; PAIR_MAC_LEN] =
                c.mac
                    .as_slice()
                    .try_into()
                    .map_err(|_| ProtoError::InvalidValue {
                        field: "pair_confirm.mac",
                        reason: "MAC must be 32 bytes",
                    })?;
            Ok(PairMsg::Confirm(PairConfirm { mac }))
        }
        pair_frame::Kind::Done(_) => Ok(PairMsg::Done),
    }
}

pub(crate) fn discovery_to_wire(m: &DiscoveryMsg) -> wire::DiscoveryFrame {
    let kind = match m {
        DiscoveryMsg::Query(q) => discovery_frame::Kind::Query(wire::DiscoveryQuery {
            nonce: q.nonce.to_vec(),
            want: Some(match q.want {
                DiscoveryWant::Pairing => {
                    discovery_query::Want::Pairing(wire::DiscoveryWantPairing {})
                }
                DiscoveryWant::Peer { tag } => {
                    discovery_query::Want::Peer(wire::DiscoveryWantPeer { tag: tag.to_vec() })
                }
            }),
        }),
        DiscoveryMsg::Answer(a) => discovery_frame::Kind::Answer(wire::DiscoveryAnswer {
            nonce: a.nonce.to_vec(),
            protocol_version: a.protocol_version,
            port: u32::from(a.port),
            offer: Some(match &a.offer {
                DiscoveryOffer::Pairing { os, device_name } => {
                    discovery_answer::Offer::Pairing(wire::DiscoveryPairing {
                        os: os_to_wire(*os) as i32,
                        device_name: device_name.clone(),
                    })
                }
                DiscoveryOffer::Peer { proof } => {
                    discovery_answer::Offer::Peer(wire::DiscoveryPeer {
                        proof: proof.to_vec(),
                    })
                }
            }),
        }),
    };
    wire::DiscoveryFrame { kind: Some(kind) }
}

fn fixed_from_wire<const N: usize>(
    b: &[u8],
    field: &'static str,
    reason: &'static str,
) -> Result<[u8; N], ProtoError> {
    b.try_into()
        .map_err(|_| ProtoError::InvalidValue { field, reason })
}

pub(crate) fn discovery_from_wire(f: wire::DiscoveryFrame) -> Result<DiscoveryMsg, ProtoError> {
    const NONCE: &str = "nonce must be 16 bytes";
    const HASH: &str = "hash must be 32 bytes";
    match f.kind.ok_or(ProtoError::UnknownKind("discovery frame"))? {
        discovery_frame::Kind::Query(q) => {
            let nonce: [u8; DISCOVERY_NONCE_LEN] =
                fixed_from_wire(&q.nonce, "discovery_query.nonce", NONCE)?;
            let want = match q.want.ok_or(ProtoError::UnknownKind("discovery query"))? {
                discovery_query::Want::Pairing(_) => DiscoveryWant::Pairing,
                discovery_query::Want::Peer(p) => DiscoveryWant::Peer {
                    tag: fixed_from_wire(&p.tag, "discovery_query.peer.tag", HASH)?,
                },
            };
            Ok(DiscoveryMsg::Query(DiscoveryQuery { nonce, want }))
        }
        discovery_frame::Kind::Answer(a) => {
            let nonce: [u8; DISCOVERY_NONCE_LEN] =
                fixed_from_wire(&a.nonce, "discovery_answer.nonce", NONCE)?;
            let port =
                u16::try_from(a.port)
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or(ProtoError::InvalidValue {
                        field: "discovery_answer.port",
                        reason: "port outside 1..=65535",
                    })?;
            let offer = match a.offer.ok_or(ProtoError::UnknownKind("discovery answer"))? {
                discovery_answer::Offer::Pairing(p) => DiscoveryOffer::Pairing {
                    os: os_from_wire(p.os, "discovery_answer.pairing.os")?,
                    device_name: string_from_wire(
                        p.device_name,
                        "discovery_answer.pairing.device_name",
                    )?,
                },
                discovery_answer::Offer::Peer(p) => DiscoveryOffer::Peer {
                    proof: fixed_from_wire(&p.proof, "discovery_answer.peer.proof", HASH)?,
                },
            };
            Ok(DiscoveryMsg::Answer(DiscoveryAnswer {
                nonce,
                protocol_version: a.protocol_version,
                port,
                offer,
            }))
        }
    }
}
