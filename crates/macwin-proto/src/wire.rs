//! Raw protobuf types (proto3 semantics), written as hand-made `prost`
//! derives so no `protoc` is needed. Field numbers are part of the protocol:
//! never renumber, only add.
//!
//! Note: `prost::Message` / `prost::Oneof` derive `Debug` and `Default`
//! themselves; enumerations derive them explicitly.

use prost::{Enumeration, Message};

/// One frame on the control stream.
#[derive(Clone, PartialEq, Message)]
pub struct ControlFrame {
    #[prost(oneof = "control_frame::Kind", tags = "1, 2, 3, 4")]
    pub kind: Option<control_frame::Kind>,
}

pub mod control_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Hello(super::Hello),
        #[prost(message, tag = "2")]
        Heartbeat(super::Heartbeat),
        #[prost(message, tag = "3")]
        Ping(super::Ping),
        #[prost(message, tag = "4")]
        Pong(super::Pong),
    }
}

/// One frame on the input stream.
#[derive(Clone, PartialEq, Message)]
pub struct InputFrame {
    #[prost(oneof = "input_frame::Kind", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9")]
    pub kind: Option<input_frame::Kind>,
}

pub mod input_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Enter(super::Enter),
        #[prost(message, tag = "2")]
        Leave(super::Leave),
        #[prost(message, tag = "3")]
        TakeControl(super::TakeControl),
        #[prost(message, tag = "4")]
        Key(super::Key),
        #[prost(message, tag = "5")]
        Button(super::Button),
        #[prost(message, tag = "6")]
        Wheel(super::Wheel),
        #[prost(message, tag = "7")]
        ModifierSync(super::ModifierSync),
        #[prost(message, tag = "8")]
        Ping(super::Ping),
        #[prost(message, tag = "9")]
        Pong(super::Pong),
    }
}

/// One QUIC datagram (no length prefix).
#[derive(Clone, PartialEq, Message)]
pub struct DatagramFrame {
    #[prost(oneof = "datagram_frame::Kind", tags = "1, 2, 3")]
    pub kind: Option<datagram_frame::Kind>,
}

pub mod datagram_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        MouseAbs(super::MouseAbs),
        #[prost(message, tag = "2")]
        Ping(super::Ping),
        #[prost(message, tag = "3")]
        Pong(super::Pong),
    }
}

/// The body of one clipboard stream (after its type byte, no length
/// prefix: the stream ends where the message ends).
#[derive(Clone, PartialEq, Message)]
pub struct ClipboardFrame {
    #[prost(oneof = "clipboard_frame::Kind", tags = "1, 2, 3")]
    pub kind: Option<clipboard_frame::Kind>,
}

pub mod clipboard_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Text(super::ClipboardText),
        #[prost(message, tag = "2")]
        Other(super::ClipboardOther),
        #[prost(message, tag = "3")]
        Image(super::ClipboardImage),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ClipboardImage {
    #[prost(uint64, tag = "1")]
    pub clock: u64,
    #[prost(bytes = "vec", tag = "2")]
    pub png: Vec<u8>,
}

/// First frame of a files stream (length-prefixed), followed by raw data.
#[derive(Clone, PartialEq, Message)]
pub struct FilesHeader {
    #[prost(uint64, tag = "1")]
    pub clock: u64,
    #[prost(message, repeated, tag = "2")]
    pub entries: Vec<FileEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct FileEntry {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(uint64, tag = "2")]
    pub size: u64,
    #[prost(bool, tag = "3")]
    pub dir: bool,
    #[prost(bool, tag = "4")]
    pub executable: bool,
    #[prost(sint64, tag = "5")]
    pub mtime_ms: i64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct ClipboardOther {
    #[prost(uint64, tag = "1")]
    pub clock: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClipboardText {
    #[prost(uint64, tag = "1")]
    pub clock: u64,
    #[prost(string, tag = "2")]
    pub text: String,
}

/// One frame on the pairing stream.
#[derive(Clone, PartialEq, Message)]
pub struct PairFrame {
    #[prost(oneof = "pair_frame::Kind", tags = "1, 2, 3")]
    pub kind: Option<pair_frame::Kind>,
}

pub mod pair_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Hello(super::PairHello),
        #[prost(message, tag = "2")]
        Confirm(super::PairConfirm),
        #[prost(message, tag = "3")]
        Done(super::PairDone),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct PairHello {
    #[prost(uint32, tag = "1")]
    pub protocol_version: u32,
    #[prost(enumeration = "Os", tag = "2")]
    pub os: i32,
    #[prost(string, tag = "3")]
    pub device_name: String,
    #[prost(string, tag = "4")]
    pub agent_version: String,
    #[prost(enumeration = "Side", tag = "5")]
    pub peer_side: i32,
    #[prost(bytes = "vec", tag = "6")]
    pub spake: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PairConfirm {
    #[prost(bytes = "vec", tag = "1")]
    pub mac: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct PairDone {}

/// One LAN discovery datagram (after [`crate::DISCOVERY_MAGIC`]).
#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryFrame {
    #[prost(oneof = "discovery_frame::Kind", tags = "1, 2")]
    pub kind: Option<discovery_frame::Kind>,
}

pub mod discovery_frame {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Query(super::DiscoveryQuery),
        #[prost(message, tag = "2")]
        Answer(super::DiscoveryAnswer),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryQuery {
    #[prost(bytes = "vec", tag = "1")]
    pub nonce: Vec<u8>,
    #[prost(oneof = "discovery_query::Want", tags = "2, 3")]
    pub want: Option<discovery_query::Want>,
}

pub mod discovery_query {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Want {
        #[prost(message, tag = "2")]
        Pairing(super::DiscoveryWantPairing),
        #[prost(message, tag = "3")]
        Peer(super::DiscoveryWantPeer),
    }
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct DiscoveryWantPairing {}

#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryWantPeer {
    #[prost(bytes = "vec", tag = "1")]
    pub tag: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryAnswer {
    #[prost(bytes = "vec", tag = "1")]
    pub nonce: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub protocol_version: u32,
    #[prost(uint32, tag = "3")]
    pub port: u32,
    #[prost(oneof = "discovery_answer::Offer", tags = "4, 5")]
    pub offer: Option<discovery_answer::Offer>,
}

pub mod discovery_answer {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Offer {
        #[prost(message, tag = "4")]
        Pairing(super::DiscoveryPairing),
        #[prost(message, tag = "5")]
        Peer(super::DiscoveryPeer),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryPairing {
    #[prost(enumeration = "Os", tag = "1")]
    pub os: i32,
    #[prost(string, tag = "2")]
    pub device_name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DiscoveryPeer {
    #[prost(bytes = "vec", tag = "1")]
    pub proof: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub protocol_version: u32,
    #[prost(string, repeated, tag = "2")]
    pub capabilities: Vec<String>,
    #[prost(enumeration = "Os", tag = "3")]
    pub os: i32,
    #[prost(string, tag = "4")]
    pub os_version: String,
    #[prost(string, tag = "5")]
    pub device_name: String,
    #[prost(message, repeated, tag = "6")]
    pub screens: Vec<Screen>,
    #[prost(enumeration = "Side", tag = "7")]
    pub peer_side: i32,
    #[prost(string, tag = "8")]
    pub agent_version: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Screen {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(sint32, tag = "2")]
    pub x: i32,
    #[prost(sint32, tag = "3")]
    pub y: i32,
    #[prost(uint32, tag = "4")]
    pub w: u32,
    #[prost(uint32, tag = "5")]
    pub h: u32,
    #[prost(double, tag = "6")]
    pub native_per_logical: f64,
    #[prost(bool, tag = "7")]
    pub primary: bool,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Point {
    #[prost(sint32, tag = "1")]
    pub x: i32,
    #[prost(sint32, tag = "2")]
    pub y: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Heartbeat {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
    #[prost(enumeration = "Role", tag = "2")]
    pub role: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Ping {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(uint64, tag = "2")]
    pub sent_micros: u64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Pong {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(uint64, tag = "2")]
    pub ping_sent_micros: u64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Enter {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
    #[prost(message, optional, tag = "2")]
    pub at: Option<Point>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Leave {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct TakeControl {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Key {
    #[prost(uint32, tag = "1")]
    pub hid: u32,
    #[prost(bool, tag = "2")]
    pub down: bool,
    #[prost(bool, tag = "3")]
    pub repeat: bool,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Button {
    #[prost(enumeration = "MouseButton", tag = "1")]
    pub button: i32,
    #[prost(bool, tag = "2")]
    pub down: bool,
    #[prost(message, optional, tag = "3")]
    pub at: Option<Point>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Wheel {
    #[prost(sint32, tag = "1")]
    pub dx: i32,
    #[prost(sint32, tag = "2")]
    pub dy: i32,
    #[prost(enumeration = "WheelUnit", tag = "3")]
    pub unit: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct ModifierSync {
    #[prost(uint32, tag = "1")]
    pub mask: u32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct MouseAbs {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
    #[prost(uint64, tag = "2")]
    pub seq: u64,
    #[prost(message, optional, tag = "3")]
    pub at: Option<Point>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Enumeration)]
#[repr(i32)]
pub enum Os {
    Unspecified = 0,
    Macos = 1,
    Windows = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Enumeration)]
#[repr(i32)]
pub enum Side {
    /// No side configured (decodes to `None`).
    Unspecified = 0,
    Left = 1,
    Right = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Enumeration)]
#[repr(i32)]
pub enum Role {
    Unspecified = 0,
    Idle = 1,
    Controlling = 2,
    Controlled = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Enumeration)]
#[repr(i32)]
pub enum MouseButton {
    Unspecified = 0,
    Left = 1,
    Right = 2,
    Middle = 3,
    Back = 4,
    Forward = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Enumeration)]
#[repr(i32)]
pub enum WheelUnit {
    Unspecified = 0,
    Notch120 = 1,
    Pixel = 2,
}
