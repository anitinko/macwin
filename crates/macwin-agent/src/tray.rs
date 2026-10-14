//! `macwin tray`: `macwin run` with a menu bar (macOS) or notification area
//! (Windows) icon. All logic lives here and is tested with a fake UI; the OS
//! layers only draw [`TrayView`]s, open dialogs and report clicks (see
//! `macwin_platform::tray`).
//!
//! The menu shows the status and offers pause / resume, clipboard sharing,
//! the peer's side, pairing, start at login and quit. The emergency chord
//! pauses sharing; Resume starts it again. Pairing first looks for a
//! machine that waits for pairing and asks for its code; when there is
//! none, this machine waits and shows a code.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use macwin_core::types::{Os, Side};
use macwin_net::pairing::{PairInfo, PairedPeer, PairingCode};
use macwin_net::{Identity, NetError};
use macwin_platform::PlatformError;
use macwin_platform::tray::{Dialog, ItemId, MenuItem, TrayEvent, TrayState, TrayView, UiCommand};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinError, JoinHandle};
use tracing::{error, info, warn};

use crate::autostart::{self, Autostart, State as AutostartState};
use crate::commands::{
    MAX_PAIR_ATTEMPTS, announce_pairing, reachable_ip, save_pairing, wait_for_pairing,
};
use crate::config::{Config, normalize_address};
use crate::driver::Backends;
use crate::instance::{self, AlreadyRunning};
use crate::lan::Discovery;
use crate::launch::{Launch, NotReady};
use crate::runner::{self, RunEvent, RunExit, bind_for, resolve};
use crate::{error_chain, host_os, platform};

pub const PAUSE: ItemId = 1;
pub const CLIPBOARD: ItemId = 2;
pub const SIDE_LEFT: ItemId = 3;
pub const SIDE_RIGHT: ItemId = 4;
pub const PAIR: ItemId = 5;
pub const AUTOSTART: ItemId = 6;
pub const QUIT: ItemId = 7;

/// Bound on stopping a running session (the runner drains for 2 s).
const STOP_WAIT: Duration = Duration::from_secs(5);
/// First and longest pause before sharing restarts after an error (a
/// display change, a permission granted later).
const RESTART_MIN: Duration = Duration::from_secs(5);
const RESTART_MAX: Duration = Duration::from_secs(60);
const CANCELLED: &str = "pairing cancelled";

/// What the icon says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotPaired,
    /// Paired, but the peer's side is unknown.
    NoSide,
    /// Sharing; waiting for the peer.
    Waiting,
    Connected,
    Paused,
    /// The emergency chord paused sharing.
    EmergencyStop,
    Pairing,
    Error(String),
}

/// Everything the menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub status: Status,
    /// The paired machine: its name and side.
    pub peer: Option<(String, Option<Side>)>,
    pub clipboard: bool,
    /// `None`: no login item on this OS.
    pub autostart: Option<bool>,
    /// The last connection problem, for the tooltip.
    pub detail: Option<String>,
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('\u{2026}');
    out
}

pub fn status_text(m: &Model) -> String {
    let name = m.peer.as_ref().map(|(n, _)| quoted(n));
    match (&m.status, name) {
        (Status::Pairing, _) => "Pairing\u{2026}".into(),
        (Status::Error(e), _) => format!("Error: {e}"),
        (Status::NotPaired, _) | (_, None) => "Not paired".into(),
        (Status::NoSide, Some(n)) => format!("Choose the side of {n} below"),
        (Status::Waiting, Some(n)) => format!("Waiting for {n}\u{2026}"),
        (Status::Connected, Some(n)) => format!("Connected to {n}"),
        (Status::Paused, _) => "Paused".into(),
        (Status::EmergencyStop, _) => "Paused by the emergency keys".into(),
    }
}

/// The icon and menu for `m`.
pub fn view(m: &Model) -> TrayView {
    let status = status_text(m);
    let pairing = m.status == Status::Pairing;
    let sharing = matches!(m.status, Status::Waiting | Status::Connected);
    let ready = matches!(&m.peer, Some((_, Some(_))));
    let action = |id, label: String, enabled, checked| MenuItem::Action {
        id,
        label,
        enabled,
        checked,
    };
    let mut items = vec![
        MenuItem::Label(short(&status, 80)),
        MenuItem::Separator,
        action(
            PAUSE,
            if sharing {
                "Pause sharing".into()
            } else {
                "Resume sharing".into()
            },
            sharing || (ready && !pairing),
            false,
        ),
        action(CLIPBOARD, "Share the clipboard".into(), true, m.clipboard),
    ];
    if let Some((name, side)) = &m.peer {
        items.push(MenuItem::Separator);
        for (id, s, word) in [
            (SIDE_LEFT, Side::Left, "left"),
            (SIDE_RIGHT, Side::Right, "right"),
        ] {
            items.push(action(
                id,
                format!("{} is on the {word}", quoted(name)),
                !pairing,
                *side == Some(s),
            ));
        }
    }
    items.push(MenuItem::Separator);
    let pair = match (pairing, &m.peer) {
        (true, _) => "Cancel pairing",
        (false, Some(_)) => "Pair with another computer\u{2026}",
        (false, None) => "Pair with the other computer\u{2026}",
    };
    items.push(action(PAIR, pair.into(), true, false));
    if let Some(on) = m.autostart {
        items.push(action(AUTOSTART, "Start at login".into(), true, on));
    }
    items.push(MenuItem::Separator);
    items.push(action(QUIT, "Quit MacWin".into(), true, false));
    let state = match m.status {
        Status::Connected => TrayState::Connected,
        Status::Waiting | Status::Pairing => TrayState::Idle,
        Status::Paused | Status::EmergencyStop => TrayState::Paused,
        Status::NotPaired | Status::NoSide | Status::Error(_) => TrayState::Attention,
    };
    let tooltip = match &m.detail {
        Some(d) if matches!(m.status, Status::Waiting) => format!("MacWin: {status} ({d})"),
        _ => format!("MacWin: {status}"),
    };
    TrayView {
        state,
        tooltip: short(&tooltip, 120),
        items,
    }
}

/// Where LAN discovery comes from.
#[derive(Debug, Clone)]
pub enum DiscoverySource {
    /// The config switch and `MACWIN_DISCOVERY`.
    Config,
    Fixed(Option<Discovery>),
}

/// What the tray works with (real or fake backends).
pub struct TrayEnv {
    pub config_dir: PathBuf,
    /// Where received files wait.
    pub staging: PathBuf,
    pub identity: Identity,
    pub os: Os,
    pub device_name: String,
    pub discovery: DiscoverySource,
    pub backends: Arc<dyn Fn(bool) -> Result<Backends, PlatformError> + Send + Sync>,
    pub autostart: Option<Box<dyn Autostart>>,
    /// Retry timings of the run loop (min, max, connect timeout); `None`:
    /// the defaults.
    pub retry: Option<(Duration, Duration, Duration)>,
}

/// How a pairing task finds the other machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairMode {
    /// Search the LAN; wait with a code when nobody waits.
    Search,
    /// Ask for the address shown on the other machine.
    Address,
}

/// Progress of a pairing task.
enum PairProgress {
    /// This machine waits; the other one has to enter `code`. `address` is
    /// where it can be dialed, `note` says why it may not be found.
    Code {
        code: PairingCode,
        address: Option<String>,
        note: Option<String>,
    },
    /// Ask for the other machine's address.
    AskAddress {
        note: Option<&'static str>,
        reply: oneshot::Sender<Option<String>>,
    },
    /// A machine waits for pairing: ask for its code.
    Ask {
        name: String,
        note: Option<&'static str>,
        reply: oneshot::Sender<Option<String>>,
    },
    Done(anyhow::Result<(PairedPeer, Option<String>)>),
}

/// What an open dialog is for.
enum Purpose {
    Code,
    AskCode(oneshot::Sender<Option<String>>),
    AskAddress(oneshot::Sender<Option<String>>),
    Side,
    Info,
}

struct Sharing {
    stop: watch::Sender<bool>,
    task: JoinHandle<anyhow::Result<RunExit>>,
}

struct Pairing {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    generation: u64,
    /// Start sharing again when pairing fails or is cancelled.
    resume: bool,
}

enum Wake {
    Ui(Option<TrayEvent>),
    Run(u64, RunEvent),
    Exit(Result<anyhow::Result<RunExit>, JoinError>),
    Pair(u64, PairProgress),
    Restart,
    Stop,
}

struct Controller {
    env: TrayEnv,
    ui: std_mpsc::Sender<UiCommand>,
    config: Config,
    status: Status,
    autostart: Option<bool>,
    shown: Option<TrayView>,
    sharing: Option<Sharing>,
    run_generation: u64,
    run_tx: mpsc::UnboundedSender<(u64, RunEvent)>,
    pairing: Option<Pairing>,
    pair_generation: u64,
    pair_tx: mpsc::UnboundedSender<(u64, PairProgress)>,
    dialog: Option<(u32, Purpose)>,
    next_dialog: u32,
    detail: Option<String>,
    /// When sharing restarts after an error, and the pause after the next.
    restart_at: Option<tokio::time::Instant>,
    restart_delay: Duration,
    quit: bool,
}

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|c| *c).await;
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Waits for `task` up to [`STOP_WAIT`], then aborts it (a stuck task must
/// not keep the ports and the input hooks).
async fn finish<T>(task: JoinHandle<T>, what: &str) -> Option<T> {
    let abort = task.abort_handle();
    match tokio::time::timeout(STOP_WAIT, task).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            warn!(error = %e, "{what} failed");
            None
        }
        Err(_) => {
            warn!("{what} did not stop in time");
            abort.abort();
            None
        }
    }
}

async fn sharing_done(s: &mut Option<Sharing>) -> Result<anyhow::Result<RunExit>, JoinError> {
    match s {
        Some(s) => (&mut s.task).await,
        None => std::future::pending().await,
    }
}

/// Runs the tray logic until Quit, the end of the UI or `stop`; then stops
/// sharing and sends [`UiCommand::Quit`].
pub async fn run_tray(
    env: TrayEnv,
    ui: std_mpsc::Sender<UiCommand>,
    mut events: mpsc::UnboundedReceiver<TrayEvent>,
    mut stop: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (run_tx, mut run_rx) = mpsc::unbounded_channel();
    let (pair_tx, mut pair_rx) = mpsc::unbounded_channel();
    let config = Config::load(&env.config_dir).unwrap_or_default();
    let mut c = Controller {
        autostart: None,
        env,
        ui,
        config,
        status: Status::NotPaired,
        shown: None,
        sharing: None,
        run_generation: 0,
        run_tx,
        pairing: None,
        pair_generation: 0,
        pair_tx,
        dialog: None,
        next_dialog: 0,
        detail: None,
        restart_at: None,
        restart_delay: Duration::ZERO,
        quit: false,
    };
    c.restart_delay = c.restart_min();
    c.refresh_autostart();
    c.start_sharing();
    c.render();
    while !c.quit {
        let wake = tokio::select! {
            e = events.recv() => Wake::Ui(e),
            Some((g, e)) = run_rx.recv() => Wake::Run(g, e),
            r = sharing_done(&mut c.sharing) => Wake::Exit(r),
            Some((g, p)) = pair_rx.recv() => Wake::Pair(g, p),
            () = sleep_until(c.restart_at) => Wake::Restart,
            () = cancelled(&mut stop) => Wake::Stop,
        };
        match wake {
            Wake::Ui(Some(e)) => c.on_ui(e).await,
            Wake::Ui(None) | Wake::Stop => c.quit = true,
            Wake::Run(g, e) => c.on_run(g, e),
            Wake::Exit(r) => c.on_run_exit(r),
            Wake::Pair(g, p) => c.on_pairing(g, p).await,
            Wake::Restart => {
                c.restart_at = None;
                if matches!(c.status, Status::Error(_)) {
                    info!("restarting sharing after an error");
                    c.start_sharing();
                }
            }
        }
        c.render();
    }
    c.shutdown().await;
    let _ = c.ui.send(UiCommand::Quit);
    Ok(())
}

impl Controller {
    fn model(&self) -> Model {
        Model {
            status: self.status.clone(),
            peer: self.config.peer.as_ref().map(|p| (p.name.clone(), p.side)),
            clipboard: self.config.clipboard.enabled,
            autostart: self.autostart,
            detail: self.detail.clone(),
        }
    }

    fn restart_min(&self) -> Duration {
        self.env.retry.map_or(RESTART_MIN, |(min, _, _)| min)
    }

    fn restart_max(&self) -> Duration {
        self.env.retry.map_or(RESTART_MAX, |(_, max, _)| max)
    }

    /// Sets an error status and schedules a restart with a growing pause.
    fn failed(&mut self, error: String) {
        error!(error = %error, "sharing failed");
        self.status = Status::Error(error);
        self.restart_at = Some(tokio::time::Instant::now() + self.restart_delay);
        self.restart_delay = (self.restart_delay * 2).min(self.restart_max());
    }

    fn render(&mut self) {
        let v = view(&self.model());
        if self.shown.as_ref() != Some(&v) {
            self.shown = Some(v.clone());
            let _ = self.ui.send(UiCommand::Update(v));
        }
    }

    fn refresh_autostart(&mut self) {
        self.autostart = self.env.autostart.as_ref().map(|a| match a.state() {
            Ok(s) => s == AutostartState::On,
            Err(e) => {
                warn!(error = %format!("{e:#}"), "cannot read the login item");
                false
            }
        });
    }

    fn reload(&mut self) {
        match Config::load(&self.env.config_dir) {
            Ok(c) => self.config = c,
            Err(e) => warn!(error = %format!("{e:#}"), "cannot read the config"),
        }
    }

    /// Loads the config, applies `f` and saves it.
    fn update_config(&mut self, f: impl FnOnce(&mut Config)) {
        let result = Config::load(&self.env.config_dir).and_then(|mut c| {
            f(&mut c);
            c.save(&self.env.config_dir)?;
            Ok(c)
        });
        match result {
            Ok(c) => self.config = c,
            Err(e) => self.info(format!("Cannot change the settings: {}", error_chain(&e))),
        }
    }

    fn discovery(&self) -> anyhow::Result<Option<Discovery>> {
        match &self.env.discovery {
            DiscoverySource::Fixed(d) => Ok(d.clone()),
            DiscoverySource::Config => {
                Discovery::from_env(self.config.discovery, |k| std::env::var_os(k))
            }
        }
    }

    /// The status when nothing runs.
    fn idle_status(&self) -> Status {
        match &self.config.peer {
            None => Status::NotPaired,
            Some(p) if p.side.is_none() => Status::NoSide,
            Some(_) => Status::Paused,
        }
    }

    fn start_sharing(&mut self) {
        if self.sharing.is_some() || self.pairing.is_some() {
            return;
        }
        self.restart_at = None;
        self.detail = None;
        self.reload();
        let discovery = match self.discovery() {
            Ok(d) => d,
            Err(e) => {
                self.failed(error_chain(&e));
                return;
            }
        };
        let launch = match Launch::new(
            &self.config,
            self.env.os,
            self.env.device_name.clone(),
            self.env.staging.clone(),
            discovery,
        ) {
            Ok(l) => l,
            Err(NotReady::NotPaired) => {
                self.status = Status::NotPaired;
                return;
            }
            Err(NotReady::NoSide) => {
                self.status = Status::NoSide;
                return;
            }
        };
        let mut opts = launch.opts;
        if let Some((min, max, connect)) = self.env.retry {
            (opts.retry_min, opts.retry_max, opts.connect_timeout) = (min, max, connect);
        }
        self.run_generation += 1;
        let generation = self.run_generation;
        let tx = self.run_tx.clone();
        let backends = self.env.backends.clone();
        let with_clipboard = launch.with_clipboard;
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(runner::run(
            opts,
            self.env.identity.clone(),
            move || backends(with_clipboard),
            move |e| {
                let _ = tx.send((generation, e));
            },
            stop_rx,
        ));
        info!(peer = %launch.peer.name, "sharing started");
        self.sharing = Some(Sharing { stop, task });
        self.status = Status::Waiting;
    }

    async fn stop_sharing(&mut self) {
        let Some(s) = self.sharing.take() else {
            return;
        };
        let _ = s.stop.send(true);
        if let Some(Err(e)) = finish(s.task, "sharing").await {
            warn!(error = %error_chain(&e), "the run loop failed");
        }
        info!("sharing stopped");
    }

    async fn restart_sharing(&mut self) {
        if self.sharing.is_some() {
            self.stop_sharing().await;
            self.start_sharing();
        }
    }

    fn open(&mut self, dialog: Dialog, purpose: Purpose) {
        self.next_dialog += 1;
        let id = self.next_dialog;
        self.dialog = Some((id, purpose));
        let _ = self.ui.send(UiCommand::Open { id, dialog });
    }

    fn info(&mut self, message: String) {
        self.open(
            Dialog {
                message,
                input: false,
                buttons: vec!["OK".into()],
            },
            Purpose::Info,
        );
    }

    fn close_dialog(&mut self) {
        if let Some((id, _)) = self.dialog.take() {
            let _ = self.ui.send(UiCommand::Close { id });
        }
    }

    fn set_side(&mut self, side: Side) {
        self.update_config(|c| {
            if let Some(p) = &mut c.peer {
                p.side = Some(side);
            }
        });
    }

    async fn on_ui(&mut self, e: TrayEvent) {
        match e {
            TrayEvent::Clicked(PAUSE) => {
                if self.sharing.is_some() {
                    self.stop_sharing().await;
                    self.status = Status::Paused;
                    self.restart_at = None;
                } else {
                    self.restart_delay = self.restart_min();
                    self.start_sharing();
                }
            }
            TrayEvent::Clicked(CLIPBOARD) => {
                self.update_config(|c| c.clipboard.enabled = !c.clipboard.enabled);
                self.restart_sharing().await;
            }
            TrayEvent::Clicked(id @ (SIDE_LEFT | SIDE_RIGHT)) => {
                if self.pairing.is_some() {
                    return;
                }
                self.set_side(if id == SIDE_LEFT {
                    Side::Left
                } else {
                    Side::Right
                });
                if self.sharing.is_some() {
                    self.restart_sharing().await;
                } else if !matches!(self.status, Status::Paused | Status::EmergencyStop) {
                    self.start_sharing();
                }
            }
            TrayEvent::Clicked(PAIR) => {
                if self.pairing.is_some() {
                    self.cancel_pairing().await;
                } else {
                    self.start_pairing(PairMode::Search).await;
                }
            }
            TrayEvent::Clicked(AUTOSTART) => self.toggle_autostart(),
            TrayEvent::Clicked(QUIT) | TrayEvent::Exit => self.quit = true,
            TrayEvent::Clicked(other) => warn!(id = other, "unknown menu item"),
            TrayEvent::DialogDone { id, button, text } => {
                if self.dialog.as_ref().is_none_or(|(open, _)| *open != id) {
                    return;
                }
                let Some((_, purpose)) = self.dialog.take() else {
                    return;
                };
                match purpose {
                    // "Pair by address...": dial the machine instead of waiting.
                    Purpose::Code if button == Some(1) => {
                        self.start_pairing(PairMode::Address).await
                    }
                    Purpose::Code => self.cancel_pairing().await,
                    Purpose::AskCode(reply) | Purpose::AskAddress(reply) => {
                        let _ = reply.send((button == Some(1)).then(|| text.trim().to_string()));
                    }
                    Purpose::Side => {
                        let side = match button {
                            Some(0) => Side::Left,
                            Some(1) => Side::Right,
                            _ => return,
                        };
                        self.set_side(side);
                        self.start_sharing();
                    }
                    Purpose::Info => {}
                }
            }
        }
    }

    fn toggle_autostart(&mut self) {
        let Some(item) = &self.env.autostart else {
            return;
        };
        let result = if self.autostart == Some(true) {
            item.disable()
        } else {
            item.enable()
        };
        if let Err(e) = result {
            self.info(format!("Cannot change the login item: {}", error_chain(&e)));
        }
        self.refresh_autostart();
    }

    fn on_run(&mut self, generation: u64, e: RunEvent) {
        if self.sharing.is_none() || generation != self.run_generation {
            return;
        }
        match e {
            RunEvent::Connected { name, remote } => {
                info!(peer = %name, %remote, "connected");
                self.status = Status::Connected;
                self.detail = None;
                self.restart_delay = self.restart_min();
            }
            RunEvent::Retrying { error, .. } => {
                info!(error = %error, "a connection attempt failed");
                self.status = Status::Waiting;
                self.detail = Some(error);
            }
            RunEvent::Ended(reason) => {
                info!(?reason, "session ended");
                self.status = Status::Waiting;
            }
            RunEvent::Listening(_) | RunEvent::Dialing(_) => self.status = Status::Waiting,
            RunEvent::Moved(addr) => {
                if let Some(p) = &self.config.peer {
                    match Config::save_moved_peer(&self.env.config_dir, &p.fingerprint, addr) {
                        Ok(true) => {
                            info!(%addr, "saved the peer's new address");
                            self.reload();
                        }
                        Ok(false) => {}
                        Err(e) => warn!(error = %error_chain(&e), "cannot save the new address"),
                    }
                }
            }
            RunEvent::Role(_) => {}
        }
    }

    fn on_run_exit(&mut self, r: Result<anyhow::Result<RunExit>, JoinError>) {
        self.sharing = None;
        match r {
            Ok(Ok(RunExit::Stopped)) => self.status = Status::Paused,
            Ok(Ok(RunExit::EmergencyStop)) => self.status = Status::EmergencyStop,
            Ok(Err(e)) => self.failed(error_chain(&e)),
            Err(e) => self.failed(format!("internal error: {e}")),
        }
    }

    /// Stops a pairing task (aborting it after [`STOP_WAIT`]).
    async fn stop_pairing(&mut self) -> Option<Pairing> {
        let mut p = self.pairing.take()?;
        let _ = p.cancel.send(true);
        let task = std::mem::replace(&mut p.task, tokio::spawn(async {}));
        finish(task, "pairing").await;
        Some(p)
    }

    async fn start_pairing(&mut self, mode: PairMode) {
        // Switching to "by address" keeps the first attempt's resume flag.
        let resume = match self.stop_pairing().await {
            Some(old) => old.resume,
            None => !matches!(self.status, Status::Paused | Status::EmergencyStop),
        };
        self.restart_at = None;
        self.stop_sharing().await;
        self.close_dialog();
        self.reload();
        let discovery = self.discovery().unwrap_or_else(|e| {
            warn!(error = %error_chain(&e), "LAN discovery is off");
            None
        });
        self.pair_generation += 1;
        let generation = self.pair_generation;
        let tx = self.pair_tx.clone();
        let (cancel, cancel_rx) = watch::channel(false);
        let info = PairInfo {
            os: self.env.os,
            device_name: self.env.device_name.clone(),
            peer_side: None,
        };
        let task = tokio::spawn(pairing_task(
            mode,
            self.env.identity.clone(),
            info,
            self.config.listen,
            discovery,
            move |p| {
                let _ = tx.send((generation, p));
            },
            cancel_rx,
        ));
        self.pairing = Some(Pairing {
            cancel,
            task,
            generation,
            resume,
        });
        self.status = Status::Pairing;
    }

    /// Ends pairing; sharing starts again when it ran before.
    async fn cancel_pairing(&mut self) {
        let Some(p) = self.stop_pairing().await else {
            return;
        };
        self.close_dialog();
        self.after_pairing(p.resume);
    }

    fn after_pairing(&mut self, resume: bool) {
        self.status = self.idle_status();
        if resume {
            self.start_sharing();
        }
    }

    async fn on_pairing(&mut self, generation: u64, p: PairProgress) {
        if self
            .pairing
            .as_ref()
            .is_none_or(|x| x.generation != generation)
        {
            return;
        }
        match p {
            PairProgress::Code {
                code,
                address,
                note,
            } => {
                let address = address.unwrap_or_else(|| "this computer's IP address".into());
                let note = note.map(|n| format!("\n\n{n}")).unwrap_or_default();
                self.open(
                    Dialog {
                        message: format!(
                            "Enter this code on the other computer:\n\n{code}\n\nThere, \
                             choose \u{201c}Pair with the other computer\u{2026}\u{201d} in \
                             the MacWin menu. If it does not find this computer, click \
                             \u{201c}Pair by address\u{2026}\u{201d} in its window and enter \
                             {address}.{note}"
                        ),
                        input: false,
                        buttons: vec!["Cancel".into(), "Pair by address\u{2026}".into()],
                    },
                    Purpose::Code,
                )
            }
            PairProgress::AskAddress { note, reply } => self.open(
                Dialog {
                    message: format!(
                        "{}Enter the address shown in the other computer's pairing window \
                         (for example 192.168.1.20:47800):",
                        note.map(|n| format!("{n}\n\n")).unwrap_or_default()
                    ),
                    input: true,
                    buttons: vec!["Cancel".into(), "Next".into()],
                },
                Purpose::AskAddress(reply),
            ),
            PairProgress::Ask { name, note, reply } => self.open(
                Dialog {
                    message: format!(
                        "{}Enter the code shown on {}:",
                        note.map(|n| format!("{n}\n\n")).unwrap_or_default(),
                        quoted(&name)
                    ),
                    input: true,
                    buttons: vec!["Cancel".into(), "Pair".into()],
                },
                Purpose::AskCode(reply),
            ),
            PairProgress::Done(result) => {
                let resume = self.pairing.take().is_some_and(|p| p.resume);
                self.close_dialog();
                match result {
                    Ok((peer, address)) => self.paired(&peer, address),
                    Err(e) if e.to_string() == CANCELLED => self.after_pairing(resume),
                    Err(e) => {
                        self.after_pairing(resume);
                        self.info(format!("Pairing failed: {}", error_chain(&e)));
                    }
                }
            }
        }
    }

    fn paired(&mut self, peer: &PairedPeer, address: Option<String>) {
        info!(peer = %peer.device_name, "paired");
        match save_pairing(&self.env.config_dir, peer, None, address) {
            Ok(config) => self.config = config,
            Err(e) => {
                self.status = Status::Error(error_chain(&e));
                return;
            }
        }
        let name = quoted(&peer.device_name);
        if self.config.peer.as_ref().is_some_and(|p| p.side.is_none()) {
            self.status = Status::NoSide;
            self.open(
                Dialog {
                    message: format!(
                        "Paired with {name}.\n\nWhere are the screens of {name}: on the left \
                         or on the right of this computer's screens?"
                    ),
                    input: false,
                    buttons: vec!["Left".into(), "Right".into()],
                },
                Purpose::Side,
            );
        } else {
            self.start_sharing();
            self.info(format!("Paired with {name}."));
        }
    }

    async fn shutdown(&mut self) {
        self.stop_pairing().await;
        self.close_dialog();
        self.stop_sharing().await;
    }
}

/// Pairs with the machine that waits for pairing, or waits and shows a
/// code when there is none (or dials an address the user enters); reports
/// through `report`.
async fn pairing_task(
    mode: PairMode,
    id: Identity,
    info: PairInfo,
    listen: SocketAddr,
    discovery: Option<Discovery>,
    report: impl Fn(PairProgress) + Send + 'static,
    mut cancel: watch::Receiver<bool>,
) {
    let result = match mode {
        PairMode::Search => {
            pair_flow(&id, &info, listen, discovery.as_ref(), &report, &mut cancel).await
        }
        PairMode::Address => pair_by_address(&id, &info, &report, &mut cancel).await,
    };
    if let Err(e) = &result {
        info!(error = %error_chain(e), "pairing ended");
    }
    report(PairProgress::Done(result));
}

/// Asks the user for a line of text; `None` when cancelled.
async fn ask(
    report: &impl Fn(PairProgress),
    cancel: &mut watch::Receiver<bool>,
    progress: impl FnOnce(oneshot::Sender<Option<String>>) -> PairProgress,
) -> Option<String> {
    let (reply, answer) = oneshot::channel();
    report(progress(reply));
    tokio::select! {
        r = answer => r.ok().flatten(),
        () = cancelled(cancel) => None,
    }
}

/// Asks for the code shown on `name` (at `addr`) until one works.
async fn dial_with_codes(
    id: &Identity,
    info: &PairInfo,
    addr: SocketAddr,
    name: &str,
    report: &impl Fn(PairProgress),
    cancel: &mut watch::Receiver<bool>,
) -> anyhow::Result<PairedPeer> {
    let mut note = None;
    for attempt in 1..=MAX_PAIR_ATTEMPTS {
        let asked = ask(report, cancel, |reply| PairProgress::Ask {
            name: name.to_string(),
            note,
            reply,
        })
        .await;
        let Some(text) = asked else {
            bail!(CANCELLED);
        };
        let Ok(code) = PairingCode::parse(&text) else {
            note = Some("That was not a 6-digit code.");
            continue;
        };
        let result = tokio::select! {
            r = macwin_net::pair_connect(bind_for(addr), addr, id, &code, info) => r,
            () = cancelled(cancel) => bail!(CANCELLED),
        };
        match result {
            Ok(peer) => return Ok(peer),
            Err(NetError::PairingFailed(why)) if attempt < MAX_PAIR_ATTEMPTS => {
                info!(why, "the code did not work");
                note = Some("That code did not work. The other computer shows a new one.");
            }
            Err(e) => return Err(e).with_context(|| format!("pairing with {addr} failed")),
        }
    }
    bail!("too many wrong codes; start pairing again")
}

/// Pairs with the machine at an address the user enters (no broadcast
/// needed: the address is shown in the other machine's code window).
async fn pair_by_address(
    id: &Identity,
    info: &PairInfo,
    report: &impl Fn(PairProgress),
    cancel: &mut watch::Receiver<bool>,
) -> anyhow::Result<(PairedPeer, Option<String>)> {
    let mut note = None;
    for _ in 0..MAX_PAIR_ATTEMPTS {
        let asked = ask(report, cancel, |reply| PairProgress::AskAddress {
            note,
            reply,
        })
        .await;
        let Some(text) = asked else {
            bail!(CANCELLED);
        };
        let Ok(target) = normalize_address(&text) else {
            note = Some("That is not an address like 192.168.1.20:47800.");
            continue;
        };
        let Ok(addr) = resolve(&target).await else {
            note = Some("That address cannot be found.");
            continue;
        };
        let peer = dial_with_codes(id, info, addr, &target, report, cancel).await?;
        return Ok((peer, Some(target)));
    }
    bail!("no usable address; start pairing again")
}

async fn pair_flow(
    id: &Identity,
    info: &PairInfo,
    listen: SocketAddr,
    discovery: Option<&Discovery>,
    report: &impl Fn(PairProgress),
    cancel: &mut watch::Receiver<bool>,
) -> anyhow::Result<(PairedPeer, Option<String>)> {
    let mut note = match discovery {
        None => Some("LAN discovery is off on this computer.".to_string()),
        Some(_) => None,
    };
    let found = match discovery {
        Some(d) => tokio::select! {
            r = d.find_pairing() => r.unwrap_or_else(|e| {
                warn!(error = %error_chain(&e), "LAN search failed");
                note = Some(error_chain(&e));
                Vec::new()
            }),
            () = cancelled(cancel) => bail!(CANCELLED),
        },
        None => Vec::new(),
    };
    match found.as_slice() {
        [] => {
            let listener = runner::bind_retrying(|| macwin_net::pair_listen(listen, id))
                .await
                .with_context(|| format!("cannot listen on {listen}"))?;
            let local = listener.local_addr()?;
            let responder = announce_pairing(discovery, local.port(), info).await;
            if discovery.is_some() && responder.is_none() {
                note = Some(format!(
                    "This computer cannot answer LAN searches (UDP port {} is in use).",
                    macwin_net::discovery::DISCOVERY_PORT
                ));
            }
            let address =
                reachable_ip(local).map(|ip| SocketAddr::new(ip, local.port()).to_string());
            let result = wait_for_pairing(
                &listener,
                info,
                |code| {
                    report(PairProgress::Code {
                        code: code.clone(),
                        address: address.clone(),
                        note: note.clone(),
                    })
                },
                |why| info!(why, "a pairing attempt failed"),
                cancelled(cancel),
            )
            .await;
            drop(responder);
            listener.shutdown().await;
            match result {
                Err(e) if e.to_string() == CANCELLED => bail!(CANCELLED),
                r => Ok((r?, None)),
            }
        }
        [f] => {
            let peer = dial_with_codes(id, info, f.addr, &f.device_name, report, cancel).await?;
            Ok((peer, Some(f.addr.to_string())))
        }
        many => {
            let names: Vec<String> = many.iter().map(|f| quoted(&f.device_name)).collect();
            bail!(
                "several computers wait for pairing ({}); click \u{201c}Pair by \
                 address\u{2026}\u{201d} on one of them instead",
                names.join(", ")
            )
        }
    }
}

/// Ctrl+C, or SIGTERM (launchd, logout) on macOS. A handler that cannot be
/// installed never fires.
async fn wait_for_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                () = ctrl_c => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    ctrl_c.await;
}

/// `macwin tray`, `macwin-tray.exe` and MacWin.app opened without arguments.
pub fn main() -> ExitCode {
    let log = crate::logging::init_file();
    match run_app() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let msg = error_chain(&e);
            error!(error = %msg, "MacWin stopped");
            eprintln!("error: {msg}");
            let mut notice = format!("MacWin stopped: {msg}");
            if let Some(log) = log {
                notice.push_str(&format!("\n\nLog: {}", log.display()));
            }
            platform::notice(&notice);
            ExitCode::FAILURE
        }
    }
}

fn run_app() -> anyhow::Result<()> {
    let os = host_os().context("unsupported OS: macwin runs on macOS and Windows")?;
    platform::init();
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    let config_dir = macwin_net::config_dir(|k| std::env::var_os(k))?;
    let _lock = match instance::lock(&config_dir) {
        Ok(l) => l,
        Err(e) if e.is::<AlreadyRunning>() => {
            info!("another MacWin is running");
            platform::notice(
                "MacWin is already running: look for its icon in the menu bar or the notification area.",
            );
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let id = crate::commands::load_identity()?;
    let staging = macwin_net::cache_dir(|k| std::env::var_os(k))?.join("incoming");
    let env = TrayEnv {
        config_dir: id.config_dir.clone(),
        staging,
        identity: id.identity,
        os,
        device_name: crate::device_name(),
        discovery: DiscoverySource::Config,
        backends: Arc::new(platform::backends),
        autostart: autostart::for_this_copy()
            .inspect_err(|e| info!(error = %error_chain(e), "no login item"))
            .ok(),
        retry: None,
    };
    let (ui_tx, ui_rx) = std_mpsc::channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);
    info!("MacWin tray started");
    let controller = runtime.spawn(run_tray(env, ui_tx, event_rx, stop_rx));
    runtime.spawn(async move {
        wait_for_signal().await;
        let _ = stop_tx.send(true);
    });
    // Blocks this (main) thread until Quit; on macOS Quit ends the process.
    let ui = platform::run_tray(ui_rx, move |e| {
        let _ = event_tx.send(e);
    });
    let done = runtime.block_on(tokio::time::timeout(STOP_WAIT * 2, controller));
    ui.context("the tray icon failed")?;
    match done {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(e).context("the tray failed"),
        Err(_) => bail!("the tray did not stop in time"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(status: Status, peer: Option<(&str, Option<Side>)>) -> Model {
        Model {
            status,
            peer: peer.map(|(n, s)| (n.to_string(), s)),
            clipboard: true,
            autostart: Some(false),
            detail: None,
        }
    }

    fn action(v: &TrayView, id: ItemId) -> (String, bool, bool) {
        v.items
            .iter()
            .find_map(|i| match i {
                MenuItem::Action {
                    id: x,
                    label,
                    enabled,
                    checked,
                } if *x == id => Some((label.clone(), *enabled, *checked)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {id}"))
    }

    fn has(v: &TrayView, id: ItemId) -> bool {
        v.items
            .iter()
            .any(|i| matches!(i, MenuItem::Action { id: x, .. } if *x == id))
    }

    #[test]
    fn the_menu_follows_the_state() {
        let unpaired = view(&model(Status::NotPaired, None));
        assert_eq!(unpaired.items[0], MenuItem::Label("Not paired".into()));
        assert_eq!(unpaired.state, TrayState::Attention);
        assert_eq!(
            action(&unpaired, PAUSE),
            ("Resume sharing".into(), false, false)
        );
        assert!(!has(&unpaired, SIDE_LEFT));
        assert_eq!(
            action(&unpaired, PAIR).0,
            "Pair with the other computer\u{2026}"
        );
        assert_eq!(
            action(&unpaired, AUTOSTART),
            ("Start at login".into(), true, false)
        );

        let connected = view(&model(Status::Connected, Some(("PC", Some(Side::Right)))));
        assert_eq!(
            connected.items[0],
            MenuItem::Label("Connected to \u{201c}PC\u{201d}".into())
        );
        assert_eq!(connected.state, TrayState::Connected);
        assert_eq!(connected.tooltip, "MacWin: Connected to \u{201c}PC\u{201d}");
        assert_eq!(
            action(&connected, PAUSE),
            ("Pause sharing".into(), true, false)
        );
        assert_eq!(
            action(&connected, CLIPBOARD),
            ("Share the clipboard".into(), true, true)
        );
        assert!(!action(&connected, SIDE_LEFT).2 && action(&connected, SIDE_RIGHT).2);
        assert_eq!(
            action(&connected, PAIR).0,
            "Pair with another computer\u{2026}"
        );

        let no_side = view(&model(Status::NoSide, Some(("PC", None))));
        assert_eq!(no_side.state, TrayState::Attention);
        assert!(
            !action(&no_side, PAUSE).1,
            "nothing to resume without a side"
        );
        assert!(!action(&no_side, SIDE_LEFT).2 && !action(&no_side, SIDE_RIGHT).2);

        let pairing = view(&model(Status::Pairing, Some(("PC", Some(Side::Left)))));
        assert_eq!(action(&pairing, PAIR).0, "Cancel pairing");
        assert!(!action(&pairing, PAUSE).1 && !action(&pairing, SIDE_RIGHT).1);

        let paused = view(&model(
            Status::EmergencyStop,
            Some(("PC", Some(Side::Left))),
        ));
        assert_eq!(paused.state, TrayState::Paused);
        assert_eq!(
            action(&paused, PAUSE),
            ("Resume sharing".into(), true, false)
        );

        let mut no_login = model(Status::Waiting, Some(("PC", Some(Side::Left))));
        no_login.autostart = None;
        assert!(!has(&view(&no_login), AUTOSTART));
        assert_eq!(
            status_text(&no_login),
            "Waiting for \u{201c}PC\u{201d}\u{2026}"
        );
    }

    #[test]
    fn long_errors_are_shortened() {
        let v = view(&model(Status::Error("x".repeat(500)), None));
        let MenuItem::Label(l) = &v.items[0] else {
            panic!()
        };
        assert_eq!(l.chars().count(), 80);
        assert!(v.tooltip.chars().count() <= 120);
    }
}
