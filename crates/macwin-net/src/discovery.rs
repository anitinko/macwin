//! LAN discovery over UDP broadcast (port [`DISCOVERY_PORT`]).
//!
//! A machine that looks for another one broadcasts a [`DiscoveryQuery`] to
//! 255.255.255.255 and to the directed broadcast address of each interface
//! (the caller passes the list); listening machines answer the asker with a
//! unicast [`DiscoveryAnswer`]. Pairing listeners answer pairing queries
//! with their name and OS. Session listeners answer only queries carrying a
//! tag derived from their own certificate fingerprint, and prove it with a
//! second hash, so fingerprints never travel.
//!
//! Discovery is unauthenticated on purpose: an answer only suggests an
//! address. Pairing is protected by the code and sessions by certificate
//! pinning, so a forged answer can at most make a connection attempt fail.
//! Responders ignore queries from outside private, link-local and loopback
//! addresses.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use macwin_core::msg::{
    DISCOVERY_NONCE_LEN, DiscoveryAnswer, DiscoveryMsg, DiscoveryOffer, DiscoveryQuery,
    DiscoveryWant,
};
use macwin_core::types::Os;
use macwin_proto::{
    MAX_DISCOVERY_LEN, PROTOCOL_VERSION, check_version, decode_discovery, encode_discovery,
};
use ring::digest::{Context, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::debug;

use crate::{Fingerprint, NetError};

/// The UDP port responders listen on.
pub const DISCOVERY_PORT: u16 = 47801;
/// How long a search for pairing listeners collects answers.
pub const SEARCH_TIME: Duration = Duration::from_millis(1500);
/// Delays after the first query at which a search repeats it (a datagram
/// can get lost on Wi-Fi).
const RESEND: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(600)];

/// What a [`Responder`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// A pairing listener on `port`.
    Pairing {
        port: u16,
        os: Os,
        device_name: String,
    },
    /// A session listener on `port`, presenting `fingerprint`.
    Peer { port: u16, fingerprint: Fingerprint },
}

/// A machine waiting for pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its pairing listener.
    pub addr: SocketAddr,
    pub os: Os,
    pub device_name: String,
}

fn hash(label: &[u8], nonce: &[u8; DISCOVERY_NONCE_LEN], fp: &Fingerprint) -> [u8; 32] {
    let mut ctx = Context::new(&SHA256);
    ctx.update(label);
    ctx.update(nonce);
    ctx.update(&fp.0);
    ctx.finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

/// The tag a query carries to find the machine presenting `fp`.
pub fn peer_tag(nonce: &[u8; DISCOVERY_NONCE_LEN], fp: &Fingerprint) -> [u8; 32] {
    hash(b"macwin discovery tag", nonce, fp)
}

/// The proof the machine presenting `fp` answers with.
pub fn peer_proof(nonce: &[u8; DISCOVERY_NONCE_LEN], fp: &Fingerprint) -> [u8; 32] {
    hash(b"macwin discovery proof", nonce, fp)
}

/// Addresses that may come from the same LAN.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_lan(IpAddr::V4(v4)),
            None => v6.is_loopback() || v6.is_unicast_link_local() || v6.is_unique_local(),
        },
    }
}

/// The answer of a responder offering `offer` to `query`, if any.
pub fn answer(offer: &Offer, query: &DiscoveryQuery) -> Option<DiscoveryAnswer> {
    let (port, offer) = match (offer, query.want) {
        (
            Offer::Pairing {
                port,
                os,
                device_name,
            },
            DiscoveryWant::Pairing,
        ) => (
            *port,
            DiscoveryOffer::Pairing {
                os: *os,
                device_name: device_name.clone(),
            },
        ),
        (Offer::Peer { port, fingerprint }, DiscoveryWant::Peer { tag })
            if tag == peer_tag(&query.nonce, fingerprint) =>
        {
            (
                *port,
                DiscoveryOffer::Peer {
                    proof: peer_proof(&query.nonce, fingerprint),
                },
            )
        }
        _ => return None,
    };
    Some(DiscoveryAnswer {
        nonce: query.nonce,
        protocol_version: PROTOCOL_VERSION,
        port,
        offer,
    })
}

/// 255.255.255.255 plus the given directed broadcast addresses, on `port`.
pub fn broadcast_targets(directed: &[Ipv4Addr], port: u16) -> Vec<SocketAddr> {
    let mut out = vec![SocketAddr::from((Ipv4Addr::BROADCAST, port))];
    for ip in directed {
        let addr = SocketAddr::from((*ip, port));
        if !ip.is_unspecified() && !out.contains(&addr) {
            out.push(addr);
        }
    }
    out
}

/// Answers discovery queries until dropped.
pub struct Responder {
    local: SocketAddr,
    task: JoinHandle<()>,
}

impl Responder {
    /// Binds `bind` (normally `0.0.0.0:47801`) and starts answering.
    pub async fn start(bind: SocketAddr, offer: Offer) -> Result<Responder, NetError> {
        let socket = UdpSocket::bind(bind).await?;
        let local = socket.local_addr()?;
        let task = tokio::spawn(serve(socket, offer));
        Ok(Responder { local, task })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(socket: UdpSocket, offer: Offer) {
    let mut buf = vec![0u8; MAX_DISCOVERY_LEN + 1];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                // Windows reports an ICMP "port unreachable" for an earlier
                // answer as a receive error; nothing to do but go on.
                debug!(error = %e, "discovery receive failed");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        if !is_lan(from.ip()) {
            continue;
        }
        let Ok(DiscoveryMsg::Query(query)) = decode_discovery(&buf[..n]) else {
            continue;
        };
        if let Some(a) = answer(&offer, &query) {
            debug!(%from, "answering a discovery query");
            let _ = socket
                .send_to(&encode_discovery(&DiscoveryMsg::Answer(a)), from)
                .await;
        }
    }
}

fn nonce() -> Result<[u8; DISCOVERY_NONCE_LEN], NetError> {
    let mut n = [0u8; DISCOVERY_NONCE_LEN];
    SystemRandom::new()
        .fill(&mut n)
        .map_err(|_| NetError::Identity("no system randomness".into()))?;
    Ok(n)
}

/// Sends `query` to `targets` a few times and passes each matching answer
/// (with its sender) to `on_answer` until `wait` has passed or `on_answer`
/// returns true.
async fn search(
    targets: &[SocketAddr],
    query: &DiscoveryQuery,
    wait: Duration,
    mut on_answer: impl FnMut(SocketAddr, DiscoveryAnswer) -> bool,
) -> Result<(), NetError> {
    let loopback = !targets.is_empty() && targets.iter().all(|t| t.ip().is_loopback());
    let bind = if loopback {
        Ipv4Addr::LOCALHOST
    } else {
        Ipv4Addr::UNSPECIFIED
    };
    let socket = UdpSocket::bind((bind, 0)).await?;
    socket.set_broadcast(true)?;
    let bytes = encode_discovery(&DiscoveryMsg::Query(query.clone()));
    let start = Instant::now();
    let deadline = start + wait;
    let mut resend = RESEND.iter().map(|d| start + *d);
    let mut next_send = Some(start);
    let mut buf = vec![0u8; MAX_DISCOVERY_LEN + 1];
    loop {
        let now = Instant::now();
        if next_send.is_some_and(|t| t <= now) {
            let mut sent = 0;
            let mut last_error = None;
            for t in targets {
                match socket.send_to(&bytes, t).await {
                    Ok(_) => sent += 1,
                    Err(e) => {
                        debug!(target = %t, error = %e, "cannot send a discovery query");
                        last_error = Some(e);
                    }
                }
            }
            if sent == 0
                && let Some(e) = last_error
            {
                return Err(e.into());
            }
            next_send = resend.next().filter(|t| *t < deadline);
        }
        let until = next_send.map_or(deadline, |t| t.min(deadline));
        match tokio::time::timeout_at(until, socket.recv_from(&mut buf)).await {
            Err(_) if until >= deadline => return Ok(()),
            Err(_) => {}
            Ok(Err(e)) => debug!(error = %e, "discovery receive failed"),
            Ok(Ok((n, from))) => {
                let Ok(DiscoveryMsg::Answer(a)) = decode_discovery(&buf[..n]) else {
                    continue;
                };
                if a.nonce == query.nonce
                    && check_version(a.protocol_version).is_ok()
                    && on_answer(from, a)
                {
                    return Ok(());
                }
            }
        }
    }
}

/// The machines waiting for pairing that answer within `wait`.
pub async fn find_pairing(targets: &[SocketAddr], wait: Duration) -> Result<Vec<Found>, NetError> {
    let query = DiscoveryQuery {
        nonce: nonce()?,
        want: DiscoveryWant::Pairing,
    };
    let mut found: Vec<Found> = Vec::new();
    search(targets, &query, wait, |from, a| {
        if let DiscoveryOffer::Pairing { os, device_name } = a.offer {
            let addr = SocketAddr::new(from.ip(), a.port);
            if !found.iter().any(|f| f.addr == addr) {
                found.push(Found {
                    addr,
                    os,
                    device_name,
                });
            }
        }
        false
    })
    .await?;
    Ok(found)
}

/// The session listener of the machine presenting `peer`, if it answers
/// within `wait` (returns at the first answer).
pub async fn find_peer(
    targets: &[SocketAddr],
    peer: Fingerprint,
    wait: Duration,
) -> Result<Option<SocketAddr>, NetError> {
    let nonce = nonce()?;
    let query = DiscoveryQuery {
        nonce,
        want: DiscoveryWant::Peer {
            tag: peer_tag(&nonce, &peer),
        },
    };
    let proof = peer_proof(&nonce, &peer);
    let mut found = None;
    search(targets, &query, wait, |from, a| match a.offer {
        DiscoveryOffer::Peer { proof: p } if p == proof => {
            found = Some(SocketAddr::new(from.ip(), a.port));
            true
        }
        _ => false,
    })
    .await?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(b: u8) -> Fingerprint {
        Fingerprint([b; 32])
    }

    #[test]
    fn answers_match_the_query() {
        let pairing = Offer::Pairing {
            port: 47800,
            os: Os::MacOs,
            device_name: "mac".into(),
        };
        let peer = Offer::Peer {
            port: 5000,
            fingerprint: fp(1),
        };
        let q_pair = DiscoveryQuery {
            nonce: [4; 16],
            want: DiscoveryWant::Pairing,
        };
        let q_peer = |target: Fingerprint| DiscoveryQuery {
            nonce: [5; 16],
            want: DiscoveryWant::Peer {
                tag: peer_tag(&[5; 16], &target),
            },
        };
        let a = answer(&pairing, &q_pair).unwrap();
        assert_eq!((a.nonce, a.port), ([4; 16], 47800));
        assert!(
            matches!(a.offer, DiscoveryOffer::Pairing { ref device_name, .. } if device_name == "mac")
        );
        assert_eq!(answer(&pairing, &q_peer(fp(1))), None);
        assert_eq!(answer(&peer, &q_pair), None);
        assert_eq!(answer(&peer, &q_peer(fp(2))), None);
        let a = answer(&peer, &q_peer(fp(1))).unwrap();
        assert_eq!(
            a.offer,
            DiscoveryOffer::Peer {
                proof: peer_proof(&[5; 16], &fp(1))
            }
        );
        assert_ne!(peer_tag(&[5; 16], &fp(1)), peer_proof(&[5; 16], &fp(1)));
        assert_ne!(peer_tag(&[5; 16], &fp(1)), peer_tag(&[6; 16], &fp(1)));
    }

    #[test]
    fn only_lan_sources_are_answered() {
        for ip in [
            "192.168.1.5",
            "10.0.0.1",
            "172.16.4.4",
            "169.254.1.1",
            "127.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.5",
        ] {
            assert!(is_lan(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "100.64.0.1", "2001:db8::1", "::ffff:1.1.1.1"] {
            assert!(!is_lan(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn broadcast_targets_start_with_the_limited_broadcast() {
        let t = broadcast_targets(
            &[
                "192.168.1.255".parse().unwrap(),
                "192.168.1.255".parse().unwrap(),
                Ipv4Addr::UNSPECIFIED,
                Ipv4Addr::BROADCAST,
                "10.0.255.255".parse().unwrap(),
            ],
            47801,
        );
        let t: Vec<String> = t.iter().map(ToString::to_string).collect();
        assert_eq!(
            t,
            [
                "255.255.255.255:47801",
                "192.168.1.255:47801",
                "10.0.255.255:47801"
            ]
        );
    }
}
