//! Implementations of the `macwin` subcommands.

use std::io::{BufRead, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use macwin_core::engine::{Timings, TransportRole};
use macwin_core::keymap;
use macwin_core::types::{
    CaptureFault, CapturedEvent, GrabCommand, Hid, InjectCommand, Point, ScreenInfo, Side,
};
use macwin_net::discovery::{Offer, Responder};
use macwin_net::pairing::{PairInfo, PairListener, PairedPeer, PairingCode};
use macwin_net::{Identity, NetError, Session, config_dir, connect, listen};
use macwin_platform::{CaptureSink, ClipboardInfo};
use tracing::{info, warn};

use crate::autostart::{self, State};
use crate::cli::{PairArgs, SpikeCaptureArgs, SpikeKvmArgs, SpikeNetArgs, Switch};
use crate::config::{Config, PeerConfig, normalize_address, os_name, side_name};
use crate::driver::{Connected, SessionConfig, SessionEnd, kvm_hello, spawn_session};
use crate::identity_store::{IDENTITY_SECRET, IdentityOrigin, KeyStoreKind, load_or_create};
use crate::instance;
use crate::lan::Discovery;
use crate::launch::{Launch, NotReady};
use crate::runner::{self, RunEvent, RunExit, bind_for, resolve};
use crate::spike_net::{self, SpikeOpts, spike_hello};
use crate::{device_name, host_os, platform};

/// The emergency chord, as shown to users.
pub const EMERGENCY_CHORD: &str = "Ctrl+Alt+Shift+Esc (Control+Option+Shift+Esc on a Mac)";
/// How long `macwin pair` waits for the other machine.
pub const PAIR_WINDOW: Duration = Duration::from_secs(600);
/// Failed attempts after which `macwin pair` gives up.
pub const MAX_PAIR_ATTEMPTS: u32 = 5;

/// This machine's identity and where it came from.
pub struct LoadedIdentity {
    pub identity: Identity,
    pub config_dir: PathBuf,
    /// Where the key is kept, for users.
    pub store: String,
}

/// Loads (or creates, or migrates) this machine's identity.
pub fn load_identity() -> anyhow::Result<LoadedIdentity> {
    let dir = config_dir(|k| std::env::var_os(k))?;
    let kind = KeyStoreKind::from_env(|k| std::env::var_os(k))?;
    let store = platform::secret_store(kind, &dir)
        .with_context(|| format!("the {} key store is unavailable", kind.name()))?;
    let (identity, origin) = load_or_create(store.as_ref(), &dir)?;
    let where_ = store.describe(IDENTITY_SECRET);
    match origin {
        IdentityOrigin::Loaded => {}
        IdentityOrigin::Created => eprintln!("created a new identity in {where_}"),
        IdentityOrigin::Migrated => {
            eprintln!("moved the identity from {} into {where_}", dir.display())
        }
    }
    Ok(LoadedIdentity {
        identity,
        config_dir: dir,
        store: where_,
    })
}

pub fn identity() -> anyhow::Result<()> {
    let id = load_identity()?;
    println!("fingerprint: {}", id.identity.fingerprint());
    println!("key: {}", id.store);
    println!("config dir: {}", id.config_dir.display());
    Ok(())
}

fn supported_os() -> anyhow::Result<macwin_core::types::Os> {
    host_os().ok_or_else(|| anyhow!("unsupported OS: macwin runs on macOS and Windows"))
}

// ---------- pairing and configuration ----------

/// The address the other machine should dial: the bound IP, or for a
/// wildcard bind the IP of the default route (found without sending
/// anything: connecting a UDP socket only picks a route).
pub fn reachable_ip(local: SocketAddr) -> Option<IpAddr> {
    if !local.ip().is_unspecified() {
        return Some(local.ip());
    }
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("192.0.2.1:9").ok()?;
    let ip = probe.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

fn show_code(code: &PairingCode, local: SocketAddr, discoverable: bool) {
    let host = match reachable_ip(local) {
        Some(ip) => SocketAddr::new(ip, local.port()).to_string(),
        None => format!("<this machine's IP>:{}", local.port()),
    };
    println!("pairing code: {code}");
    if discoverable {
        println!(
            "on the other machine run:\n  macwin pair --code {}\n\
             (if it does not find this machine: macwin pair {host} --code {})",
            code.digits(),
            code.digits()
        );
    } else {
        println!(
            "on the other machine run:\n  macwin pair {host} --code {}",
            code.digits()
        );
    }
    let _ = std::io::stdout().flush();
}

/// Waits for the other machine, burning the code after each failed attempt.
/// `on_code` shows every new code; `cancel` ends the wait early.
pub async fn wait_for_pairing(
    listener: &PairListener,
    info: &PairInfo,
    mut on_code: impl FnMut(&PairingCode),
    mut on_failure: impl FnMut(&str),
    cancel: impl std::future::Future<Output = ()>,
) -> anyhow::Result<PairedPeer> {
    let deadline = tokio::time::Instant::now() + PAIR_WINDOW;
    let mut code = PairingCode::generate()?;
    on_code(&code);
    let mut failures = 0;
    tokio::pin!(cancel);
    loop {
        let attempt = tokio::time::timeout_at(deadline, listener.accept(&code, info));
        let result = tokio::select! {
            r = attempt => r,
            () = &mut cancel => bail!("pairing cancelled"),
        };
        match result {
            Err(_) => bail!(
                "no machine paired within {} minutes",
                PAIR_WINDOW.as_secs() / 60
            ),
            Ok(Ok(peer)) => return Ok(peer),
            Ok(Err(NetError::PairingFailed(why))) => {
                failures += 1;
                on_failure(&why);
                if failures >= MAX_PAIR_ATTEMPTS {
                    bail!("{failures} failed attempts; start pairing again");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                code = PairingCode::generate()?;
                on_code(&code);
            }
            Ok(Err(NetError::EndpointClosed)) => bail!("the pairing listener closed"),
            Ok(Err(e)) => warn!(error = %e, "a connection attempt failed; still waiting"),
        }
    }
}

/// Starts answering LAN searches for a pairing listener on `port`.
pub async fn announce_pairing(
    discovery: Option<&Discovery>,
    port: u16,
    info: &PairInfo,
) -> Option<Responder> {
    let d = discovery?;
    // The protocol takes at most 256 bytes; a long Computer Name must not
    // make this machine undiscoverable.
    let offer = Offer::Pairing {
        port,
        os: info.os,
        device_name: info.device_name.chars().take(64).collect(),
    };
    match Responder::start(d.bind, offer).await {
        Ok(r) => Some(r),
        Err(e) => {
            warn!(bind = %d.bind, error = %e, "cannot answer LAN searches");
            None
        }
    }
}

/// `macwin pair` on the waiting side.
async fn listen_pairing(
    bind: SocketAddr,
    id: &Identity,
    info: &PairInfo,
    discovery: Option<&Discovery>,
) -> anyhow::Result<PairedPeer> {
    let listener = macwin_net::pair_listen(bind, id)
        .await
        .with_context(|| format!("cannot listen on {bind}"))?;
    let local = listener.local_addr()?;
    println!("listening on {local}");
    let responder = announce_pairing(discovery, local.port(), info).await;
    let discoverable = responder.is_some();
    let mut shown = false;
    let result = wait_for_pairing(
        &listener,
        info,
        |code| {
            if shown {
                println!("that code is used up; a new one:");
            }
            shown = true;
            show_code(code, local, discoverable);
        },
        |why| eprintln!("pairing attempt failed: {why}"),
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
    )
    .await;
    drop(responder);
    listener.shutdown().await;
    result
}

fn prompt_code(name: Option<&str>) -> anyhow::Result<PairingCode> {
    if !std::io::stdin().is_terminal() {
        bail!("pass the code shown on the other machine with --code");
    }
    match name {
        Some(n) => print!("code shown on \"{n}\": "),
        None => print!("code shown on the other machine: "),
    }
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(PairingCode::parse(&line)?)
}

/// This machine's view of the peer's side, from the local choice and the
/// peer's choice ("its peer is on its left" means "it is on my right").
pub fn agree_side(local: Option<Side>, peer_view: Option<Side>) -> Option<Side> {
    match (local, peer_view) {
        (Some(l), Some(p)) if l == p => {
            warn!(
                side = side_name(l),
                "both machines put the other on the same side; keeping this machine's choice"
            );
            Some(l)
        }
        (Some(l), _) => Some(l),
        (None, Some(p)) => Some(p.opposite()),
        (None, None) => None,
    }
}

/// Saves a new pairing (keeping the side of an earlier pairing with the
/// same machine when neither side chose one now).
pub fn save_pairing(
    config_dir: &Path,
    peer: &PairedPeer,
    local_side: Option<Side>,
    address: Option<String>,
) -> anyhow::Result<Config> {
    let mut config = Config::load(config_dir)?;
    let mut side = agree_side(local_side, peer.peer_side);
    if let Some(old) = &config.peer
        && old.fingerprint == peer.fingerprint
    {
        side = side.or(old.side);
    }
    config.peer = Some(PeerConfig {
        fingerprint: peer.fingerprint,
        name: peer.device_name.clone(),
        os: peer.os,
        side,
        address,
    });
    config.save(config_dir)?;
    Ok(config)
}

pub async fn pair(args: PairArgs) -> anyhow::Result<()> {
    let os = supported_os()?;
    let _lock = instance::lock(&config_dir(|k| std::env::var_os(k))?)?;
    let id = load_identity()?;
    let config = Config::load(&id.config_dir)?;
    let discovery = Discovery::from_env(config.discovery, |k| std::env::var_os(k))?;
    let info = PairInfo {
        os,
        device_name: device_name(),
        peer_side: args.side.map(Side::from),
    };
    let code = args.code.as_deref().map(PairingCode::parse).transpose()?;
    let dial = async |remote: SocketAddr, code: Option<PairingCode>, name: Option<&str>| {
        let code = match code {
            Some(c) => c,
            None => prompt_code(name)?,
        };
        anyhow::Ok(
            macwin_net::pair_connect(bind_for(remote), remote, &id.identity, &code, &info).await?,
        )
    };
    let (peer, address) = match (args.address, args.listen) {
        (Some(target), _) => {
            let target = normalize_address(&target)?;
            let remote = resolve(&target).await?;
            println!("pairing with {target}");
            let peer = dial(remote, code, None)
                .await
                .with_context(|| format!("pairing with {target} failed"))?;
            (peer, Some(target))
        }
        (None, Some(bind)) => (
            listen_pairing(bind, &id.identity, &info, discovery.as_ref()).await?,
            None,
        ),
        (None, None) => {
            let found = match &discovery {
                Some(d) => {
                    println!("looking for a machine waiting for pairing...");
                    d.find_pairing().await.unwrap_or_else(|e| {
                        eprintln!("warning: {e:#}");
                        Vec::new()
                    })
                }
                None if code.is_some() => bail!(
                    "LAN discovery is off: pass the other machine's address \
                     (macwin pair HOST --code CODE)"
                ),
                None => Vec::new(),
            };
            match found.as_slice() {
                [] if code.is_some() => bail!(
                    "no machine waiting for pairing answered on the local network; run \
                     `macwin pair` there first, or pass its address: macwin pair HOST --code CODE"
                ),
                [] => (
                    listen_pairing(config.listen, &id.identity, &info, discovery.as_ref()).await?,
                    None,
                ),
                [f] => {
                    println!(
                        "found \"{}\" ({}) at {}",
                        f.device_name,
                        os_name(f.os),
                        f.addr
                    );
                    let peer = dial(f.addr, code, Some(&f.device_name))
                        .await
                        .with_context(|| format!("pairing with {} failed", f.addr))?;
                    (peer, Some(f.addr.to_string()))
                }
                many => {
                    let list: Vec<String> = many
                        .iter()
                        .map(|f| format!("  {} ({}) at {}", f.device_name, os_name(f.os), f.addr))
                        .collect();
                    bail!(
                        "several machines wait for pairing:\n{}\npass the address of the right one: \
                         macwin pair HOST --code CODE",
                        list.join("\n")
                    )
                }
            }
        }
    };
    let old = Config::load(&id.config_dir)?.peer;
    let config = save_pairing(&id.config_dir, &peer, info.peer_side, address)?;
    if let Some(old) = old
        && old.fingerprint != peer.fingerprint
    {
        println!("replaced the previously paired \"{}\"", old.name);
    }
    let side = config.peer.as_ref().and_then(|p| p.side);
    println!(
        "paired with \"{}\" ({}), fingerprint {}",
        peer.device_name,
        os_name(peer.os),
        peer.fingerprint
    );
    match side {
        Some(s) => println!("its screens are on the {} of this machine", side_name(s)),
        None => println!("tell macwin where its screens are: macwin config set side left|right"),
    }
    println!("start sharing with: macwin run");
    Ok(())
}

pub fn unpair() -> anyhow::Result<()> {
    let dir = config_dir(|k| std::env::var_os(k))?;
    let mut config = Config::load(&dir)?;
    match config.peer.take() {
        None => println!("not paired"),
        Some(p) => {
            config.save(&dir)?;
            println!("forgot \"{}\"; run `macwin unpair` there as well", p.name);
        }
    }
    Ok(())
}

pub fn autostart(action: Option<Switch>) -> anyhow::Result<()> {
    let item = autostart::for_this_copy()?;
    match action {
        None => match item.state()? {
            State::Off => println!("autostart: off"),
            State::On => println!("autostart: on ({})", item.command()),
            State::Other(c) => println!(
                "autostart: on for another copy ({c}); `macwin autostart on` switches to this one"
            ),
        },
        Some(Switch::On) => {
            let exe = std::env::current_exe()?;
            if cfg!(target_os = "macos") && !autostart::in_app_bundle(&exe) {
                eprintln!(
                    "warning: macOS grants the input permissions to MacWin.app; register the app \
                     instead (scripts/macos/run.sh autostart on)"
                );
            }
            item.enable()?;
            println!("MacWin starts at login: {}", item.command());
        }
        Some(Switch::Off) => {
            item.disable()?;
            println!("MacWin no longer starts at login");
        }
    }
    Ok(())
}

pub fn config_show() -> anyhow::Result<()> {
    let dir = config_dir(|k| std::env::var_os(k))?;
    let config = Config::load(&dir)?;
    println!("# {}", Config::path(&dir).display());
    print!("{}", config.to_toml());
    Ok(())
}

pub fn config_set(key: &str, value: &str) -> anyhow::Result<()> {
    let dir = config_dir(|k| std::env::var_os(k))?;
    let mut config = Config::load(&dir)?;
    config.set(key, value)?;
    config.save(&dir)?;
    println!("{key} = {value}");
    Ok(())
}

// ---------- macwin run ----------

fn print_event(e: RunEvent) {
    match e {
        RunEvent::Moved(addr) => println!("the peer answered a LAN search at {addr}"),
        RunEvent::Listening(addr) => println!("listening on {addr}"),
        RunEvent::Dialing(target) => println!("connecting to {target}"),
        RunEvent::Connected { name, remote } => println!("connected to \"{name}\" ({remote})"),
        RunEvent::Role(role) => info!(?role, "role"),
        RunEvent::Ended(end) => println!("session ended: {end:?}"),
        RunEvent::Retrying { after, error } if after.is_zero() => {
            println!("a connection attempt failed: {error}")
        }
        RunEvent::Retrying { after, error } => {
            println!(
                "connection failed: {error}; retrying in {} s",
                after.as_secs()
            )
        }
    }
    let _ = std::io::stdout().flush();
}

pub async fn run() -> anyhow::Result<()> {
    let os = supported_os()?;
    platform::init();
    let _lock = instance::lock(&config_dir(|k| std::env::var_os(k))?)?;
    let id = load_identity()?;
    let config = Config::load(&id.config_dir)?;
    let staging = macwin_net::cache_dir(|k| std::env::var_os(k))?.join("incoming");
    let discovery = Discovery::from_env(config.discovery, |k| std::env::var_os(k))?;
    let launch = Launch::new(&config, os, device_name(), staging, discovery).map_err(|e| {
        match e {
            NotReady::NotPaired => anyhow!(
                "not paired yet: run `macwin pair` on one machine and the command it prints on the other"
            ),
            NotReady::NoSide => anyhow!(
                "tell macwin where the peer's screens are: macwin config set side left|right"
            ),
        }
    })?;
    println!(
        "sharing with \"{}\" ({}), whose screens are on the {}; clipboard: {}",
        launch.peer.name,
        os_name(launch.peer.os),
        side_name(launch.side),
        launch.sharing(),
    );
    println!("emergency release: {EMERGENCY_CHORD}. Ctrl+C here stops macwin.");
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_tx.send(true);
    });
    let dir = id.config_dir.clone();
    let fingerprint = launch.peer.fingerprint;
    let on_event = move |e: RunEvent| {
        if let RunEvent::Moved(addr) = e {
            match Config::save_moved_peer(&dir, &fingerprint, addr) {
                Ok(true) => println!("saved the peer's new address {addr}"),
                Ok(false) => {}
                Err(e) => warn!(error = %format!("{e:#}"), "cannot save the peer's new address"),
            }
        }
        print_event(e);
    };
    let with_clipboard = launch.with_clipboard;
    let exit = runner::run(
        launch.opts,
        id.identity,
        || platform::backends(with_clipboard),
        on_event,
        stop_rx,
    )
    .await?;
    match exit {
        RunExit::Stopped => println!("stopped"),
        RunExit::EmergencyStop => println!("stopped by the emergency chord"),
    }
    Ok(())
}

async fn accept_one(
    addr: SocketAddr,
    id: &Identity,
    args_fp: macwin_net::Fingerprint,
    hello: &macwin_core::msg::Hello,
) -> anyhow::Result<Session> {
    let listener = listen(addr, id, args_fp)
        .await
        .with_context(|| format!("cannot listen on {addr}"))?;
    println!("listening on {}", listener.local_addr()?);
    std::io::stdout().flush()?;
    loop {
        match listener.accept(hello).await {
            Ok(s) => return Ok(s),
            Err(NetError::EndpointClosed) => bail!("the listener closed"),
            Err(e) => warn!(error = %e, "a peer failed the handshake; still listening"),
        }
    }
}

pub async fn spike_net(args: SpikeNetArgs) -> anyhow::Result<()> {
    let os = supported_os()?;
    let id = load_identity()?.identity;
    println!("local fingerprint: {}", id.fingerprint());
    let hello = spike_hello(os, &device_name());
    if let Some(addr) = args.listen {
        let session = accept_one(addr, &id, args.peer_fp, &hello).await?;
        println!(
            "peer connected: {} ({})",
            session.peer.device_name, session.remote
        );
        spike_net::run_listener(session).await?;
        println!("peer closed the session");
        return Ok(());
    }
    let remote = resolve(
        &args
            .connect
            .context("either --listen or --connect is required")?,
    )
    .await?;
    let session = connect(bind_for(remote), remote, &id, args.peer_fp, &hello)
        .await
        .with_context(|| format!("cannot connect to {remote}"))?;
    println!(
        "connected to {} ({})",
        session.peer.device_name, session.remote
    );
    let opts = SpikeOpts {
        count: args.count.max(1),
        interval: Duration::from_millis(args.interval_ms),
    };
    let report = spike_net::run_dialer(session, opts).await?;
    println!("{report}");
    Ok(())
}

// ---------- probes ----------

fn describe_screen(s: &ScreenInfo) -> String {
    format!(
        "screen {}: {}x{} at ({}, {}), scale {:.2}{}",
        s.id,
        s.rect.width(),
        s.rect.height(),
        s.rect.left(),
        s.rect.top(),
        s.native_per_logical,
        if s.primary { ", primary" } else { "" }
    )
}

pub fn probe_screens() -> anyhow::Result<()> {
    platform::init();
    for s in platform::screens().screens()? {
        println!("{}", describe_screen(&s));
    }
    Ok(())
}

pub fn probe_permissions(prompt: bool) {
    print!("{}", platform::permissions_report(prompt));
}

fn print_clipboard(i: &ClipboardInfo) {
    println!(
        "change {}: {} format(s){}",
        i.change_count,
        i.formats.len(),
        if i.ours { ", written by macwin" } else { "" }
    );
    for f in &i.formats {
        match f.size {
            Some(n) => println!("  {} ({n} bytes)", f.name),
            None => println!("  {}", f.name),
        }
    }
}

pub async fn probe_clipboard(watch: bool) -> anyhow::Result<()> {
    let cb = platform::clipboard();
    println!("clipboard formats and sizes only; the content is never shown");
    print_clipboard(&cb.inspect()?);
    if !watch {
        return Ok(());
    }
    println!("watching for changes (Ctrl+C to stop)");
    let mut last = cb.change_count()?;
    let mut tick = tokio::time::interval(Duration::from_millis(300));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = cb.change_count()?;
                if now != last {
                    last = now;
                    print_clipboard(&cb.inspect()?);
                }
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

// ---------- local spikes ----------

fn primary_center(screens: &[ScreenInfo]) -> Point {
    let s = screens.iter().find(|s| s.primary).unwrap_or(&screens[0]);
    Point::new(
        s.rect.left() + s.rect.width() / 2,
        s.rect.top() + s.rect.height() / 2,
    )
}

fn describe_key(hid: Hid) -> String {
    match keymap::name(hid) {
        Some(n) => format!("{n} ({:#08x})", hid.0),
        None => format!("{:#08x}", hid.0),
    }
}

/// Prints events (mouse moves at most 10 per second) and swallows them when
/// grabbing.
struct PrintSink {
    swallow: bool,
    faults: Mutex<mpsc::Sender<CaptureFault>>,
    last_move: Mutex<Option<Instant>>,
}

impl CaptureSink for PrintSink {
    fn on_event(&self, ev: CapturedEvent) -> bool {
        match ev {
            CapturedEvent::MouseMove { pos, delta } => {
                let mut last = self.last_move.lock().unwrap_or_else(|e| e.into_inner());
                if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                    *last = Some(Instant::now());
                    println!(
                        "move   pos ({:.1}, {:.1}) delta ({:.1}, {:.1})",
                        pos.x, pos.y, delta.x, delta.y
                    );
                }
            }
            CapturedEvent::Button { button, down, pos } => {
                println!(
                    "button {button:?} {} at ({:.1}, {:.1})",
                    if down { "down" } else { "up" },
                    pos.x,
                    pos.y
                );
            }
            CapturedEvent::Wheel { dx, dy, .. } => println!("wheel  dx {dx} dy {dy}"),
            CapturedEvent::Key { hid, down, repeat } => println!(
                "key    {} {}{}",
                describe_key(hid),
                if down { "down" } else { "up" },
                if repeat { " (repeat)" } else { "" }
            ),
        }
        !self.swallow
    }

    fn on_fault(&self, fault: CaptureFault) {
        let _ = self
            .faults
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(fault);
    }
}

pub async fn spike_capture(args: SpikeCaptureArgs) -> anyhow::Result<()> {
    platform::init();
    let screens = platform::screens().screens()?;
    let center = primary_center(&screens);
    let (fault_tx, fault_rx) = mpsc::channel();
    let sink = Arc::new(PrintSink {
        swallow: args.grab,
        faults: Mutex::new(fault_tx),
        last_move: Mutex::new(None),
    });
    let mut capture = platform::capture();
    let control = capture.start(sink)?;
    if args.grab {
        println!(
            "GRABBING for {} s: local input is swallowed and the cursor is hidden at the screen center.\n\
             Press {EMERGENCY_CHORD} to release early.",
            args.seconds
        );
        control.command(GrabCommand::Grab { exit: center });
    } else {
        println!(
            "printing local input for {} s (Ctrl+C to stop); nothing is swallowed",
            args.seconds
        );
    }
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut outcome = "timeout";
    while Instant::now() < deadline {
        if let Ok(fault) = fault_rx.try_recv() {
            println!("fail-safe fired: {fault:?}");
            outcome = "fail-safe";
            break;
        }
        if args.grab {
            control.renew_lease(Instant::now() + Duration::from_secs(1));
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = tokio::signal::ctrl_c() => { outcome = "Ctrl+C"; break; }
        }
    }
    if args.grab {
        control.command(GrabCommand::Release { warp_to: center });
    }
    control.stop();
    drop(capture);
    println!("capture stopped ({outcome})");
    Ok(())
}

pub async fn spike_inject() -> anyhow::Result<()> {
    platform::init();
    let screens = platform::screens().screens()?;
    let c = primary_center(&screens);
    let mut injector = platform::injector()?;
    println!(
        "In 3 seconds the cursor draws a 200 px square around the screen center, then \"hello\" is typed\n\
         with physical keys (the active layout decides the letters: a Russian layout gives \"руддщ\").\n\
         Focus a text field now."
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    let half = 100;
    let corners = [
        Point::new(c.x - half, c.y - half),
        Point::new(c.x + half, c.y - half),
        Point::new(c.x + half, c.y + half),
        Point::new(c.x - half, c.y + half),
        Point::new(c.x - half, c.y - half),
    ];
    injector.inject(InjectCommand::MoveTo(corners[0]))?;
    for w in corners.windows(2) {
        for step in 1..=25 {
            let p = Point::new(
                w[0].x + (w[1].x - w[0].x) * step / 25,
                w[0].y + (w[1].y - w[0].y) * step / 25,
            );
            injector.inject(InjectCommand::MoveTo(p))?;
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
    }
    // HID usages of h, e, l, l, o.
    for usage in [0x0B, 0x08, 0x0F, 0x0F, 0x12] {
        let hid = Hid(0x07_0000 | usage);
        injector.inject(InjectCommand::Key {
            hid,
            down: true,
            repeat: false,
        })?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        injector.inject(InjectCommand::Key {
            hid,
            down: false,
            repeat: false,
        })?;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    println!("done");
    Ok(())
}

// ---------- KVM ----------

pub async fn spike_kvm(args: SpikeKvmArgs) -> anyhow::Result<()> {
    let os = supported_os()?;
    platform::init();
    let id = load_identity()?.identity;
    println!("local fingerprint: {}", id.fingerprint());
    let backends = platform::backends(false).context("cannot start the input backends")?;
    let screens = backends.screens.screens()?;
    for s in &screens {
        println!("{}", describe_screen(s));
    }
    let cfg = SessionConfig {
        local_os: os,
        os_version: String::new(),
        device_name: device_name(),
        peer_side: args.peer_side.into(),
        remap: !args.no_remap,
        timings: Timings::default(),
        clipboard: None,
    };
    let hello = kvm_hello(&cfg, screens);
    let (session, role) = match args.listen {
        Some(addr) => (
            accept_one(addr, &id, args.peer_fp, &hello).await?,
            TransportRole::Listener,
        ),
        None => {
            let remote = resolve(
                &args
                    .connect
                    .context("either --listen or --connect is required")?,
            )
            .await?;
            let s = connect(bind_for(remote), remote, &id, args.peer_fp, &hello)
                .await
                .with_context(|| format!("cannot connect to {remote}"))?;
            (s, TransportRole::Dialer)
        }
    };
    println!(
        "connected to {} ({}). Move the cursor over the {:?} edge to cross.\n\
         Emergency release: {EMERGENCY_CHORD}. Ctrl+C here stops the session.",
        session.peer.device_name, session.remote, args.peer_side
    );
    let handle = spawn_session(
        cfg,
        backends,
        Connected {
            link: session.link,
            peer: session.peer,
            role,
        },
    )?;
    let stopper = handle.stopper();
    let link = handle.link();
    let mut roles = handle.role.clone();
    tokio::spawn(async move {
        while roles.changed().await.is_ok() {
            let role = *roles.borrow_and_update();
            println!("role: {role:?}");
        }
    });
    let join = handle.join();
    tokio::pin!(join);
    let end = tokio::select! {
        end = &mut join => end,
        _ = tokio::signal::ctrl_c() => {
            stopper.shutdown();
            join.await
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), link.wait_closed()).await;
    println!("session ended: {end:?}");
    match end {
        SessionEnd::Shutdown | SessionEnd::EmergencyStop | SessionEnd::PeerClosed => Ok(()),
        other => Err(anyhow!("the session failed: {other:?}")),
    }
}
