//! LAN discovery between real UDP sockets on 127.0.0.1.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use macwin_core::types::Os;
use macwin_net::Fingerprint;
use macwin_net::discovery::{Offer, Responder, find_pairing, find_peer};

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

#[tokio::test]
async fn a_search_finds_a_pairing_listener() {
    let r = Responder::start(
        loopback(),
        Offer::Pairing {
            port: 47800,
            os: Os::Windows,
            device_name: "DESKTOP-1".into(),
        },
    )
    .await
    .unwrap();
    let found = find_pairing(&[r.local_addr()], Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].addr, "127.0.0.1:47800".parse().unwrap());
    assert_eq!(
        (found[0].os, found[0].device_name.as_str()),
        (Os::Windows, "DESKTOP-1")
    );
    // A session listener does not answer pairing searches.
    let peer = Responder::start(
        loopback(),
        Offer::Peer {
            port: 47800,
            fingerprint: Fingerprint([1; 32]),
        },
    )
    .await
    .unwrap();
    let none = find_pairing(&[peer.local_addr()], Duration::from_millis(300))
        .await
        .unwrap();
    assert!(none.is_empty(), "{none:?}");
}

#[tokio::test]
async fn a_search_finds_the_peer_by_fingerprint_only() {
    let fp = Fingerprint([7; 32]);
    let r = Responder::start(
        loopback(),
        Offer::Peer {
            port: 50123,
            fingerprint: fp,
        },
    )
    .await
    .unwrap();
    let found = find_peer(&[r.local_addr()], fp, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!(found, vec!["127.0.0.1:50123".parse().unwrap()]);
    let other = find_peer(
        &[r.local_addr()],
        Fingerprint([8; 32]),
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    assert!(other.is_empty());
    // Once dropped, the responder is silent.
    let addr = r.local_addr();
    drop(r);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        find_peer(&[addr], fp, Duration::from_millis(300))
            .await
            .unwrap()
            .is_empty()
    );
}

/// Anyone who fetched the certificate can answer for the peer: a search
/// reports every distinct answer, and garbage does not stop a responder.
#[tokio::test]
async fn every_answer_is_reported_and_garbage_is_ignored() {
    let fp = Fingerprint([9; 32]);
    let real = Responder::start(
        loopback(),
        Offer::Peer {
            port: 47800,
            fingerprint: fp,
        },
    )
    .await
    .unwrap();
    let forged = Responder::start(
        loopback(),
        Offer::Peer {
            port: 9,
            fingerprint: fp,
        },
    )
    .await
    .unwrap();
    let junk = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    for bytes in [&b"MWD1\xff\xff"[..], b"hello", &[0u8; 600][..]] {
        junk.send_to(bytes, real.local_addr()).unwrap();
    }
    let start = Instant::now();
    let mut found = find_peer(
        &[real.local_addr(), forged.local_addr()],
        fp,
        Duration::from_millis(400),
    )
    .await
    .unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(400),
        "collects for the whole wait"
    );
    found.sort();
    let want: Vec<SocketAddr> = vec![
        "127.0.0.1:9".parse().unwrap(),
        "127.0.0.1:47800".parse().unwrap(),
    ];
    assert_eq!(found, want);
}
