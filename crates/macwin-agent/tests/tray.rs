//! The tray logic with a fake UI and fake backends over QUIC on 127.0.0.1:
//! two machines pair from their menus (a wrong code first), choose sides,
//! share, pause, change a setting, hit the emergency chord and quit.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use macwin_agent::config::Config;
use macwin_agent::driver::Backends;
use macwin_agent::lan::Discovery;
use macwin_agent::tray::{self, DiscoverySource, TrayEnv, run_tray};
use macwin_core::types::{CaptureFault, Os, Rect, ScreenInfo, Side};
use macwin_net::Identity;
use macwin_platform::PlatformError;
use macwin_platform::testing::{
    FakeCapture, FakeCaptureHandle, FakeClipboard, FakeInjector, FakeScreens,
};
use macwin_platform::tray::{Dialog, MenuItem, TrayEvent, TrayView, UiCommand};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const T: Duration = Duration::from_secs(20);

fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One tray with its fake UI: the last view and the open dialog.
struct Machine {
    dir: tempfile::TempDir,
    commands: std_mpsc::Receiver<UiCommand>,
    events: mpsc::UnboundedSender<TrayEvent>,
    _stop: watch::Sender<bool>,
    task: JoinHandle<anyhow::Result<()>>,
    captures: Arc<Mutex<Vec<FakeCaptureHandle>>>,
    view: Option<TrayView>,
    dialog: Option<(u32, Dialog)>,
    quit: bool,
}

fn start(name: &str, os: Os, lan: SocketAddr) -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        listen: format!("127.0.0.1:{}", free_port()).parse().unwrap(),
        ..Config::default()
    };
    config.save(dir.path()).unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let made = captures.clone();
    let backends = move |clipboard: bool| -> Result<Backends, PlatformError> {
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
            clipboard: clipboard.then(|| Box::new(FakeClipboard::new().0) as _),
        })
    };
    let (ui, commands) = std_mpsc::channel();
    let (events, event_rx) = mpsc::unbounded_channel();
    let (stop, stop_rx) = watch::channel(false);
    let env = TrayEnv {
        config_dir: dir.path().to_path_buf(),
        staging: dir.path().join("incoming"),
        identity: Identity::generate().unwrap(),
        os,
        device_name: name.into(),
        discovery: DiscoverySource::Fixed(Some(Discovery::at(lan))),
        backends: Arc::new(backends),
        autostart: None,
        retry: Some((
            Duration::from_millis(50),
            Duration::from_millis(200),
            Duration::from_millis(500),
        )),
    };
    let task = tokio::spawn(run_tray(env, ui, event_rx, stop_rx));
    Machine {
        dir,
        commands,
        events,
        _stop: stop,
        task,
        captures,
        view: None,
        dialog: None,
        quit: false,
    }
}

impl Machine {
    fn drain(&mut self) {
        while let Ok(c) = self.commands.try_recv() {
            match c {
                UiCommand::Update(v) => self.view = Some(v),
                UiCommand::Open { id, dialog } => self.dialog = Some((id, dialog)),
                UiCommand::Close { id } => {
                    if self.dialog.as_ref().is_some_and(|(d, _)| *d == id) {
                        self.dialog = None;
                    }
                }
                UiCommand::Quit => self.quit = true,
            }
        }
    }

    async fn wait(&mut self, what: &str, want: impl Fn(&Machine) -> bool) {
        let deadline = Instant::now() + T;
        loop {
            self.drain();
            if want(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: status {:?}, dialog {:?}",
                self.status(),
                self.dialog
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn status(&self) -> String {
        match self.view.as_ref().and_then(|v| v.items.first()) {
            Some(MenuItem::Label(l)) => l.clone(),
            _ => String::new(),
        }
    }

    fn message(&self) -> &str {
        self.dialog.as_ref().map_or("", |(_, d)| d.message.as_str())
    }

    fn asks(&self, buttons: &[&str]) -> bool {
        self.dialog.as_ref().is_some_and(|(_, d)| {
            d.buttons
                .iter()
                .map(String::as_str)
                .eq(buttons.iter().copied())
        })
    }

    fn click(&self, id: u32) {
        self.events.send(TrayEvent::Clicked(id)).unwrap();
    }

    fn answer(&mut self, button: Option<usize>, text: &str) {
        let (id, _) = self.dialog.take().expect("an open dialog");
        self.events
            .send(TrayEvent::DialogDone {
                id,
                button,
                text: text.into(),
            })
            .unwrap();
    }

    /// The code shown by a waiting machine.
    fn code(&self) -> String {
        assert!(
            self.message().starts_with("Enter this code"),
            "{}",
            self.message()
        );
        self.message().lines().nth(2).unwrap().trim().to_string()
    }

    async fn finish(self) {
        tokio::time::timeout(T, self.task)
            .await
            .expect("the tray did not stop")
            .unwrap()
            .unwrap();
    }
}

fn connected_to(name: &str) -> impl Fn(&Machine) -> bool {
    let want = format!("Connected to \u{201c}{name}\u{201d}");
    move |m| m.status() == want
}

#[tokio::test(flavor = "multi_thread")]
async fn two_trays_pair_choose_sides_share_and_quit() {
    let lan: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut pc = start("pc", Os::Windows, lan);
    pc.wait("not paired", |m| m.status() == "Not paired").await;

    // Nobody waits yet: the PC waits and shows a code.
    pc.click(tray::PAIR);
    pc.wait("a code", |m| m.message().starts_with("Enter this code"))
        .await;
    assert_eq!(pc.status(), "Pairing\u{2026}");
    let first = pc.code();

    // The Mac finds the PC and asks for its code; a wrong one first.
    let mut mac = start("mac", Os::MacOs, lan);
    mac.click(tray::PAIR);
    mac.wait("the code question", |m| m.asks(&["Cancel", "Pair"]))
        .await;
    assert!(
        mac.message().contains("\u{201c}pc\u{201d}"),
        "{}",
        mac.message()
    );
    let wrong = if first == "000-000" {
        "111111"
    } else {
        "000000"
    };
    mac.answer(Some(1), wrong);
    pc.wait("a new code", |m| {
        m.message().starts_with("Enter this code") && m.code() != first
    })
    .await;
    mac.wait("the question again", |m| {
        m.asks(&["Cancel", "Pair"]) && m.message().contains("did not work")
    })
    .await;
    let code = pc.code();
    mac.answer(Some(1), &code);

    // Neither machine chose a side.
    mac.wait("the side question", |m| m.asks(&["Left", "Right"]))
        .await;
    pc.wait("the side question", |m| m.asks(&["Left", "Right"]))
        .await;
    assert_eq!(mac.status(), "Choose the side of \u{201c}pc\u{201d} below");
    mac.answer(Some(1), "");
    pc.answer(Some(0), "");
    mac.wait("connected", connected_to("pc")).await;
    pc.wait("connected", connected_to("mac")).await;
    let mac_peer = Config::load(mac.dir.path()).unwrap().peer.unwrap();
    assert_eq!(mac_peer.name, "pc");
    assert_eq!(mac_peer.side, Some(Side::Right));
    assert!(mac_peer.address.is_some(), "the searching machine dials");
    let pc_peer = Config::load(pc.dir.path()).unwrap().peer.unwrap();
    assert_eq!((pc_peer.side, pc_peer.address), (Some(Side::Left), None));

    // Pause on the Mac: the PC waits; Resume reconnects.
    mac.click(tray::PAUSE);
    mac.wait("paused", |m| m.status() == "Paused").await;
    pc.wait("waiting", |m| m.status().starts_with("Waiting for"))
        .await;
    mac.click(tray::PAUSE);
    mac.wait("connected again", connected_to("pc")).await;

    // A setting restarts the session with the new config.
    mac.click(tray::CLIPBOARD);
    mac.wait("clipboard off", |m| {
        m.view.as_ref().is_some_and(|v| {
            v.items.iter().any(|i| {
                matches!(i, MenuItem::Action { id, checked: false, .. } if *id == tray::CLIPBOARD)
            })
        })
    })
    .await;
    assert!(!Config::load(mac.dir.path()).unwrap().clipboard.enabled);
    mac.wait("connected after the restart", connected_to("pc"))
        .await;

    // The emergency chord pauses the PC.
    pc.wait("connected", connected_to("mac")).await;
    let capture = pc.captures.lock().unwrap().last().unwrap().clone();
    capture.emit_fault(CaptureFault::EmergencyChord);
    pc.wait("paused by the chord", |m| {
        m.status() == "Paused by the emergency keys"
    })
    .await;

    mac.click(tray::QUIT);
    pc.click(tray::QUIT);
    mac.wait("quit", |m| m.quit).await;
    pc.wait("quit", |m| m.quit).await;
    mac.finish().await;
    pc.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_pairing_closes_the_code() {
    let lan: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut m = start("pc", Os::Windows, lan);
    m.click(tray::PAIR);
    m.wait("a code", |m| m.message().starts_with("Enter this code"))
        .await;
    // The code dialog's Cancel.
    m.answer(None, "");
    m.wait("not paired", |m| m.status() == "Not paired").await;
    // The menu's "Cancel pairing" works as well.
    m.click(tray::PAIR);
    m.wait("a code", |m| m.message().starts_with("Enter this code"))
        .await;
    m.click(tray::PAIR);
    m.wait("the code is gone", |m| {
        m.dialog.is_none() && m.status() == "Not paired"
    })
    .await;
    m.click(tray::QUIT);
    m.wait("quit", |m| m.quit).await;
    m.finish().await;
}
