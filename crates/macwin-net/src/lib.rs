//! macwin networking: QUIC (quinn + rustls with the ring provider) with
//! mandatory mutual TLS and SHA-256 certificate pinning in both directions.
//!
//! A session has a reliable control stream, a reliable high-priority input
//! stream and latest-wins datagrams, exposed through the transport-agnostic
//! [`Link`]. Heartbeats are not handled here: the Engine generates and
//! checks them.
#![forbid(unsafe_code)]

pub mod codes;
pub mod discovery;
mod endpoint;
pub mod fingerprint;
pub mod identity;
pub mod link;
pub mod pairing;
mod session;
pub mod tls;

pub use endpoint::{Listener, connect, listen};
pub use fingerprint::Fingerprint;
pub use identity::{Identity, cache_dir, config_dir};
pub use link::{Inbound, Link, LinkClosed, LinkError, LinkSender};
pub use pairing::{PairInfo, PairListener, PairedPeer, PairingCode, pair_connect, pair_listen};
pub use session::{HELLO_TIMEOUT, Session};

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls configuration error: {0}")]
    Tls(#[from] rustls::Error),
    #[error("cannot connect: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("connection failed: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("protocol error: {0}")]
    Proto(#[from] macwin_proto::ProtoError),
    #[error("the peer did not complete the session setup in time")]
    HelloTimeout,
    #[error("session setup failed: {0}")]
    StreamSetup(&'static str),
    #[error("identity error: {0}")]
    Identity(String),
    #[error("invalid fingerprint: {0}")]
    InvalidFingerprint(String),
    #[error("cannot determine the config directory: {0}")]
    ConfigDir(String),
    #[error("QUIC configuration error: {0}")]
    QuicConfig(String),
    #[error("the endpoint is closed")]
    EndpointClosed,
    /// The attempt used up the pairing code (wrong code, an impostor, or
    /// a connection lost mid-exchange).
    #[error("pairing failed: {0}")]
    PairingFailed(String),
    #[error("invalid pairing code: {0}")]
    InvalidCode(&'static str),
}
