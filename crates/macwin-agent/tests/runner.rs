//! The `macwin run` loop with fake backends over QUIC on 127.0.0.1:
//! retrying, reconnecting after the peer restarts, and the emergency exit.

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use macwin_agent::driver::{Backends, SessionConfig, SessionEnd};
use macwin_agent::lan::Discovery;
use macwin_agent::runner::{Mode, RunEvent, RunExit, RunOptions, run};
use macwin_core::engine::Timings;
use macwin_core::types::{CaptureFault, Os, Rect, ScreenInfo, Side};
use macwin_net::{Fingerprint, Identity};
use macwin_platform::PlatformError;
use macwin_platform::testing::{FakeCapture, FakeCaptureHandle, FakeInjector, FakeScreens};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const T: Duration = Duration::from_secs(10);

fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn session(os: Os, name: &str, side: Side) -> SessionConfig {
    SessionConfig {
        local_os: os,
        os_version: "test".into(),
        device_name: name.into(),
        peer_side: side,
        remap: true,
        timings: Timings::default(),
        clipboard: None,
    }
}

/// One `run` loop in a task, with its events, its stop switch and the
/// capture handles of every session it started.
struct Agent {
    events: mpsc::UnboundedReceiver<RunEvent>,
    stop: watch::Sender<bool>,
    captures: Arc<Mutex<Vec<FakeCaptureHandle>>>,
    task: JoinHandle<anyhow::Result<RunExit>>,
}

fn start(cfg: SessionConfig, mode: Mode, id: Identity, peer: Fingerprint) -> Agent {
    start_with(cfg, mode, id, peer, None)
}

fn start_with(
    cfg: SessionConfig,
    mode: Mode,
    id: Identity,
    peer: Fingerprint,
    discovery: Option<Discovery>,
) -> Agent {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let made = captures.clone();
    let make = move || -> Result<Backends, PlatformError> {
        let (capture, handle) = FakeCapture::new();
        made.lock().unwrap().push(handle);
        Ok(Backends {
            capture: Box::new(capture),
            injector: Box::new(FakeInjector::new().0),
            screens: Box::new(FakeScreens(vec![ScreenInfo {
                id: 1,
                rect: Rect::new(0, 0, 1440, 900).unwrap(),
                native_per_logical: 1.0,
                primary: true,
            }])),
            clipboard: None,
        })
    };
    let (tx, events) = mpsc::unbounded_channel();
    let (stop, stop_rx) = watch::channel(false);
    let opts = RunOptions {
        session: cfg,
        mode,
        peer,
        retry_min: Duration::from_millis(50),
        retry_max: Duration::from_millis(200),
        connect_timeout: Duration::from_millis(300),
        discovery,
    };
    let task = tokio::spawn(run(
        opts,
        id,
        make,
        move |e| {
            let _ = tx.send(e);
        },
        stop_rx,
    ));
    Agent {
        events,
        stop,
        captures,
        task,
    }
}

impl Agent {
    /// Waits for an event matching `want`, skipping others.
    async fn expect(&mut self, what: &str, want: impl Fn(&RunEvent) -> bool) -> RunEvent {
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let e = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("the run loop ended");
            if want(&e) {
                return e;
            }
        }
    }

    async fn finish(self) -> RunExit {
        tokio::time::timeout(T, self.task)
            .await
            .expect("the run loop did not exit")
            .unwrap()
            .unwrap()
    }

    fn last_capture(&self) -> FakeCaptureHandle {
        self.captures.lock().unwrap().last().unwrap().clone()
    }
}

fn connected(e: &RunEvent) -> bool {
    matches!(e, RunEvent::Connected { .. })
}

/// Starts a listening agent on `addr`, retrying while the port is still
/// held by a previous endpoint that is shutting down.
async fn listening(addr: SocketAddr, id: &Identity, peer: Fingerprint) -> Agent {
    listening_with(addr, id, peer, None).await
}

async fn listening_with(
    addr: SocketAddr,
    id: &Identity,
    peer: Fingerprint,
    discovery: Option<Discovery>,
) -> Agent {
    for _ in 0..100 {
        let mut agent = start_with(
            session(Os::Windows, "pc", Side::Left),
            Mode::Listen(addr),
            id.clone(),
            peer,
            discovery.clone(),
        );
        match tokio::time::timeout(T, agent.events.recv()).await {
            Ok(Some(RunEvent::Listening(_))) => return agent,
            Ok(None) => {
                // The bind failed and the loop returned its error.
                let _ = agent.task.await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => panic!("unexpected first event {other:?}"),
        }
    }
    panic!("cannot listen on {addr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_after_the_peer_restarts_and_stops_on_the_chord() {
    let mac_id = Identity::generate().unwrap();
    let pc_id = Identity::generate().unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();

    // 1. The Mac dials before the PC listens: it keeps retrying.
    let mut mac = start(
        session(Os::MacOs, "mac", Side::Right),
        Mode::Dial(addr.to_string()),
        mac_id.clone(),
        pc_id.fingerprint(),
    );
    mac.expect("a failed dial", |e| matches!(e, RunEvent::Retrying { .. }))
        .await;
    let mut pc1 = listening(addr, &pc_id, mac_id.fingerprint()).await;
    let e = mac.expect("connected", connected).await;
    assert!(
        matches!(e, RunEvent::Connected { ref name, .. } if name == "pc"),
        "{e:?}"
    );
    pc1.expect("connected", connected).await;

    // 2. The PC stops: the Mac's session ends and it redials until a new PC
    //    listener appears.
    pc1.stop.send(true).unwrap();
    assert_eq!(pc1.finish().await, RunExit::Stopped);
    mac.expect("session ended", |e| matches!(e, RunEvent::Ended(_)))
        .await;
    mac.expect("redialing", |e| matches!(e, RunEvent::Retrying { .. }))
        .await;
    let mut pc2 = listening(addr, &pc_id, mac_id.fingerprint()).await;
    mac.expect("reconnected", connected).await;
    pc2.expect("connected", connected).await;

    // 3. The emergency chord on the Mac ends its loop; the PC keeps listening.
    mac.last_capture().emit_fault(CaptureFault::EmergencyChord);
    mac.expect("emergency end", |e| {
        matches!(e, RunEvent::Ended(SessionEnd::EmergencyStop))
    })
    .await;
    assert_eq!(mac.finish().await, RunExit::EmergencyStop);
    pc2.expect("peer gone", |e| matches!(e, RunEvent::Ended(_)))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!pc2.task.is_finished(), "the listener keeps running");
    pc2.stop.send(true).unwrap();
    assert_eq!(pc2.finish().await, RunExit::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_while_dialing_exits_promptly() {
    let id = Identity::generate().unwrap();
    let other = Identity::generate().unwrap();
    let addr = format!("127.0.0.1:{}", free_port());
    let mut mac = start(
        session(Os::MacOs, "mac", Side::Right),
        Mode::Dial(addr),
        id,
        other.fingerprint(),
    );
    mac.expect("a failed dial", |e| matches!(e, RunEvent::Retrying { .. }))
        .await;
    mac.stop.send(true).unwrap();
    assert_eq!(mac.finish().await, RunExit::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_finds_a_peer_that_moved() {
    let mac_id = Identity::generate().unwrap();
    let pc_id = Identity::generate().unwrap();
    let lan: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let pc_addr: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut pc = listening_with(
        pc_addr,
        &pc_id,
        mac_id.fingerprint(),
        Some(Discovery::at(lan)),
    )
    .await;
    // The Mac remembers an address nobody listens on any more.
    let mut mac = start_with(
        session(Os::MacOs, "mac", Side::Right),
        Mode::Dial(format!("127.0.0.1:{}", free_port())),
        mac_id,
        pc_id.fingerprint(),
        Some(Discovery::at(lan)),
    );
    mac.expect("connected", connected).await;
    let moved = mac
        .expect("moved", |e| matches!(e, RunEvent::Moved(_)))
        .await;
    assert_eq!(moved, RunEvent::Moved(pc_addr));
    pc.expect("connected", connected).await;
    mac.stop.send(true).unwrap();
    assert_eq!(mac.finish().await, RunExit::Stopped);
    pc.stop.send(true).unwrap();
    assert_eq!(pc.finish().await, RunExit::Stopped);
}
