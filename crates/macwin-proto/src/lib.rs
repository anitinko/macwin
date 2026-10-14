//! The macwin wire protocol: a strict protobuf codec for the domain messages
//! in `macwin_core::msg`, stream framing and version negotiation.
//!
//! Three frame families exist, one per channel: control stream, input stream
//! and datagrams. Decoding validates everything; invalid input is an error,
//! never a panic.
#![forbid(unsafe_code)]

mod convert;
pub mod frame;
pub mod wire;

use macwin_core::msg::{
    ClipboardMsg, ControlMsg, DatagramMsg, DiscoveryMsg, FilesHeader, InputMsg, PairMsg,
};
use prost::Message;

pub use convert::check_relative_path;

/// `major << 16 | minor`. Peers with a different major refuse each other.
/// Minor 1 added clipboard streams (capability `clipboard.text`) and
/// pairing; minor 2 clipboard images and files (`clipboard.image`,
/// `clipboard.files`) and pixel wheel deltas (`input.wheel-pixel`).
pub const PROTOCOL_VERSION: u32 = 0x0001_0002;
/// Largest frame body on the control and input streams, the pairing stream
/// and in datagrams.
pub const MAX_FRAME_LEN: usize = 64 * 1024;
/// Largest clipboard text accepted on the wire, in UTF-8 bytes.
pub const MAX_CLIPBOARD_TEXT: usize = 16 * 1024 * 1024;
/// Largest clipboard image (PNG) accepted on the wire.
pub const MAX_CLIPBOARD_IMAGE: usize = 64 * 1024 * 1024;
/// Largest encoded [`ClipboardMsg`] (the content plus protobuf overhead).
pub const MAX_CLIPBOARD_FRAME: usize = MAX_CLIPBOARD_IMAGE + 64;
/// Largest encoded [`FilesHeader`].
pub const MAX_FILES_HEADER: usize = 16 * 1024 * 1024;
/// Most entries in one file transfer.
pub const MAX_FILE_ENTRIES: usize = 65_536;
/// Longest relative path of a file entry, in bytes.
pub const MAX_FILE_PATH_LEN: usize = 4096;
/// Deepest nesting of a file entry.
pub const MAX_FILE_DEPTH: usize = 64;
/// Most file data in one transfer.
pub const MAX_FILES_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
/// Length of a SPAKE2 (Ed25519) message.
pub const SPAKE_MSG_LEN: usize = 33;
/// Length of a pairing key-confirmation MAC (HMAC-SHA256).
pub const PAIR_MAC_LEN: usize = 32;
/// Largest accepted string field, in bytes.
pub const MAX_STRING_LEN: usize = 256;
/// First bytes of every LAN discovery datagram.
pub const DISCOVERY_MAGIC: [u8; 4] = *b"MWD1";
/// Largest LAN discovery datagram, magic included.
pub const MAX_DISCOVERY_LEN: usize = 512;
/// Most capabilities accepted in a Hello.
pub const MAX_CAPABILITIES: usize = 32;
/// Longest accepted capability, in bytes.
pub const MAX_CAPABILITY_LEN: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("malformed protobuf: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("missing field {0}")]
    MissingField(&'static str),
    #[error("missing or unknown message kind in {0}")]
    UnknownKind(&'static str),
    #[error("invalid value {value} for enum field {field}")]
    InvalidEnum { field: &'static str, value: i32 },
    #[error("invalid {field}: {reason}")]
    InvalidValue {
        field: &'static str,
        reason: &'static str,
    },
    #[error("frame of {len} bytes exceeds the 65536-byte limit")]
    FrameTooLarge { len: usize },
    #[error("incompatible protocol versions: local {local:#010x}, peer {peer:#010x}")]
    IncompatibleVersion { local: u32, peer: u32 },
}

/// The major part of a protocol version.
pub const fn version_major(v: u32) -> u16 {
    (v >> 16) as u16
}

/// Accepts a peer with the same major version as [`PROTOCOL_VERSION`].
pub fn check_version(peer: u32) -> Result<(), ProtoError> {
    check_versions(PROTOCOL_VERSION, peer)
}

/// Accepts `peer` when both versions are non-zero and share the major part.
/// Each side checks the version it announced itself against the peer's, so
/// a mismatch is detected on both sides.
pub fn check_versions(local: u32, peer: u32) -> Result<(), ProtoError> {
    if local == 0 || peer == 0 || version_major(local) != version_major(peer) {
        return Err(ProtoError::IncompatibleVersion { local, peer });
    }
    Ok(())
}

pub fn encode_control(m: &ControlMsg) -> Vec<u8> {
    convert::control_to_wire(m).encode_to_vec()
}

pub fn decode_control(b: &[u8]) -> Result<ControlMsg, ProtoError> {
    convert::control_from_wire(wire::ControlFrame::decode(b)?)
}

pub fn encode_input(m: &InputMsg) -> Vec<u8> {
    convert::input_to_wire(m).encode_to_vec()
}

pub fn decode_input(b: &[u8]) -> Result<InputMsg, ProtoError> {
    convert::input_from_wire(wire::InputFrame::decode(b)?)
}

pub fn encode_datagram(m: &DatagramMsg) -> Vec<u8> {
    convert::datagram_to_wire(m).encode_to_vec()
}

pub fn decode_datagram(b: &[u8]) -> Result<DatagramMsg, ProtoError> {
    convert::datagram_from_wire(wire::DatagramFrame::decode(b)?)
}

pub fn encode_clipboard(m: &ClipboardMsg) -> Vec<u8> {
    convert::clipboard_to_wire(m).encode_to_vec()
}

/// True when the peer will accept `m` ([`decode_clipboard`]'s limits).
pub fn clipboard_within_limits(m: &ClipboardMsg) -> bool {
    match m {
        ClipboardMsg::Text(t) => t.text.len() <= MAX_CLIPBOARD_TEXT,
        ClipboardMsg::Image(i) => i.png.len() <= MAX_CLIPBOARD_IMAGE,
        ClipboardMsg::Other { .. } => true,
    }
}

pub fn encode_files_header(h: &FilesHeader) -> Vec<u8> {
    convert::files_header_to_wire(h).encode_to_vec()
}

/// Decodes and validates a files header: relative paths that stay inside
/// the receiving directory, no duplicates, bounded counts and sizes.
pub fn decode_files_header(b: &[u8]) -> Result<FilesHeader, ProtoError> {
    if b.len() > MAX_FILES_HEADER {
        return Err(ProtoError::InvalidValue {
            field: "files header",
            reason: "header larger than 16 MiB",
        });
    }
    count_entries(b)?;
    convert::files_header_from_wire(wire::FilesHeader::decode(b)?)
}

/// Counts the entries of an encoded files header without decoding them and
/// fails beyond [`MAX_FILE_ENTRIES`]: a decoded entry takes about 25 times
/// the 2 bytes of the smallest encoding, so a header packed with empty
/// entries must be refused before it is expanded.
fn count_entries(mut b: &[u8]) -> Result<usize, ProtoError> {
    use prost::encoding::{WireType, decode_key, decode_varint};
    let malformed = || ProtoError::InvalidValue {
        field: "files header",
        reason: "malformed",
    };
    let mut entries = 0;
    while !b.is_empty() {
        let (tag, wire_type) = decode_key(&mut b)?;
        let skip = match wire_type {
            WireType::Varint => {
                decode_varint(&mut b)?;
                0
            }
            WireType::SixtyFourBit => 8,
            WireType::ThirtyTwoBit => 4,
            WireType::LengthDelimited => decode_varint(&mut b)?,
            WireType::StartGroup | WireType::EndGroup => return Err(malformed()),
        };
        let skip = usize::try_from(skip)
            .ok()
            .filter(|s| *s <= b.len())
            .ok_or_else(malformed)?;
        b = &b[skip..];
        if tag == 2 {
            entries += 1;
            if entries > MAX_FILE_ENTRIES {
                return Err(ProtoError::InvalidValue {
                    field: "files_header.entries",
                    reason: "more than 65536 entries",
                });
            }
        }
    }
    Ok(entries)
}

pub fn decode_clipboard(b: &[u8]) -> Result<ClipboardMsg, ProtoError> {
    if b.len() > MAX_CLIPBOARD_FRAME {
        return Err(ProtoError::InvalidValue {
            field: "clipboard frame",
            reason: "frame larger than the clipboard limit",
        });
    }
    convert::clipboard_from_wire(wire::ClipboardFrame::decode(b)?)
}

pub fn encode_pair(m: &PairMsg) -> Vec<u8> {
    convert::pair_to_wire(m).encode_to_vec()
}

pub fn decode_pair(b: &[u8]) -> Result<PairMsg, ProtoError> {
    convert::pair_from_wire(wire::PairFrame::decode(b)?)
}

/// [`DISCOVERY_MAGIC`] followed by the protobuf frame.
pub fn encode_discovery(m: &DiscoveryMsg) -> Vec<u8> {
    let mut out = DISCOVERY_MAGIC.to_vec();
    convert::discovery_to_wire(m)
        .encode(&mut out)
        .expect("a Vec grows as needed");
    out
}

pub fn decode_discovery(b: &[u8]) -> Result<DiscoveryMsg, ProtoError> {
    if b.len() > MAX_DISCOVERY_LEN {
        return Err(ProtoError::InvalidValue {
            field: "discovery datagram",
            reason: "longer than 512 bytes",
        });
    }
    let body = b
        .strip_prefix(&DISCOVERY_MAGIC)
        .ok_or(ProtoError::InvalidValue {
            field: "discovery datagram",
            reason: "not a macwin discovery datagram",
        })?;
    convert::discovery_from_wire(wire::DiscoveryFrame::decode(body)?)
}

#[cfg(test)]
mod tests;
