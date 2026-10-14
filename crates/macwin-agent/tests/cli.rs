//! The `macwin` binary end to end, each child with its own config dir.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_macwin");

/// Each child gets its own config dir and keeps its key in a file there:
/// tests never touch the real keychain.
fn macwin(config: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.env("MACWIN_CONFIG_DIR", config)
        .env("MACWIN_KEY_STORE", "file")
        .env("MACWIN_DISCOVERY", "off")
        .env("RUST_LOG", "warn");
    c
}

fn fingerprint_of(config: &Path) -> String {
    let out = macwin(config).arg("identity").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("fingerprint: "))
        .expect("fingerprint line")
        .to_string()
}

/// Kills the child when dropped (also when a test fails).
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_with_timeout(mut child: Guard, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(_status) = child.0.try_wait().unwrap() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the child did not exit within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    use std::io::Read;
    if let Some(mut s) = child.0.stdout.take() {
        s.read_to_string(&mut stdout).unwrap();
    }
    if let Some(mut s) = child.0.stderr.take() {
        s.read_to_string(&mut stderr).unwrap();
    }
    let status = child.0.wait().unwrap();
    Output {
        status,
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
    }
}

#[test]
fn identity_is_created_once_and_stable() {
    let dir = tempfile::tempdir().unwrap();
    let first = macwin(dir.path()).arg("identity").output().unwrap();
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stderr).contains("created a new identity"));
    let second = macwin(dir.path()).arg("identity").output().unwrap();
    assert!(!String::from_utf8_lossy(&second.stderr).contains("created"));
    let fp = |o: &Output| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(fp(&first), fp(&second));
    let line = fp(&first);
    let hex = line.strip_prefix("fingerprint: ").unwrap();
    assert_eq!(hex.len(), 64);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(stdout.contains(&format!("config dir: {}", dir.path().display())));
    assert!(stdout.contains("key: unencrypted file"), "{stdout}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("identity.secret"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn phase0_identity_files_are_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let (id, bundle) = macwin_net::Identity::generate_bundle().unwrap();
    let split = bundle.find("-----BEGIN PRIVATE KEY").unwrap();
    std::fs::write(dir.path().join("identity.crt.pem"), &bundle[..split]).unwrap();
    std::fs::write(dir.path().join("identity.key.pem"), &bundle[split..]).unwrap();
    let out = macwin(dir.path()).arg("identity").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("moved the identity"));
    assert_eq!(fingerprint_of(dir.path()), id.fingerprint().to_string());
    assert!(!dir.path().join("identity.key.pem").exists());
    assert!(dir.path().join("identity.secret").exists());
}

#[test]
fn an_unknown_key_store_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = macwin(dir.path())
        .env("MACWIN_KEY_STORE", "vault")
        .arg("identity")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("MACWIN_KEY_STORE"));
}

/// Reads lines from a child's stdout on a thread.
fn lines_of(child: &mut Guard) -> mpsc::Receiver<String> {
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn wait_line(rx: &mpsc::Receiver<String>, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no line starting with {prefix:?}"));
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
}

fn config_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("config.toml")).unwrap()
}

#[test]
fn two_processes_pair_with_a_code() {
    let mac = tempfile::tempdir().unwrap();
    let pc = tempfile::tempdir().unwrap();
    let (fp_mac, fp_pc) = (fingerprint_of(mac.path()), fingerprint_of(pc.path()));
    let mut waiting = Guard(
        macwin(pc.path())
            .args(["pair", "--listen", "127.0.0.1:0", "--side", "left"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let lines = lines_of(&mut waiting);
    let addr = wait_line(&lines, "listening on ");
    let code = wait_line(&lines, "pairing code: ");
    // A wrong code burns the first code; the waiting side shows a new one.
    let wrong = if code == "000-000" {
        "111111"
    } else {
        "000000"
    };
    let bad = macwin(mac.path())
        .args(["pair", &addr, "--code", wrong])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("pairing"),
        "{bad:?}"
    );
    let code = wait_line(&lines, "pairing code: ");
    let good = macwin(mac.path())
        .args(["pair", &addr, "--code", &code])
        .output()
        .unwrap();
    let good_out = String::from_utf8_lossy(&good.stdout);
    assert!(
        good.status.success(),
        "{good_out}\n{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert!(good_out.contains("paired with"), "{good_out}");
    assert!(
        good_out.contains("its screens are on the right"),
        "derived from the PC's --side left: {good_out}"
    );
    let out = wait_with_timeout(waiting, Duration::from_secs(10));
    assert!(out.status.success(), "{out:?}");
    // Each side stored the other's fingerprint; only the dialing side has an address.
    let (mac_cfg, pc_cfg) = (config_of(mac.path()), config_of(pc.path()));
    assert!(
        mac_cfg.contains(&fp_pc) && mac_cfg.contains("address = \"127.0.0.1:"),
        "{mac_cfg}"
    );
    assert!(mac_cfg.contains("side = \"right\""), "{mac_cfg}");
    assert!(
        pc_cfg.contains(&fp_mac) && !pc_cfg.contains("address"),
        "{pc_cfg}"
    );
    assert!(pc_cfg.contains("side = \"left\""), "{pc_cfg}");
    // config set / show / unpair.
    let set = macwin(pc.path())
        .args(["config", "set", "clipboard", "off"])
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    let show = macwin(pc.path()).args(["config", "show"]).output().unwrap();
    assert!(String::from_utf8_lossy(&show.stdout).contains("enabled = false"));
    let unpair = macwin(pc.path()).arg("unpair").output().unwrap();
    assert!(String::from_utf8_lossy(&unpair.stdout).contains("forgot"));
    assert!(!config_of(pc.path()).contains("[peer]"));
}

#[test]
fn run_without_a_pairing_explains_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let out = macwin(dir.path()).arg("run").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not paired yet"));
}

/// Starts a listener and returns it with its bound address.
fn start_listener(config: &Path, peer_fp: &str) -> (Guard, String) {
    let mut child = Guard(
        macwin(config)
            .args([
                "spike",
                "net",
                "--listen",
                "127.0.0.1:0",
                "--peer-fp",
                peer_fp,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(addr) = line.strip_prefix("listening on ") {
                let _ = tx.send(addr.to_string());
            }
        }
    });
    let addr = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("listener address");
    (child, addr)
}

#[test]
fn spike_net_between_two_processes() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (fp_a, fp_b) = (fingerprint_of(a.path()), fingerprint_of(b.path()));
    let (listener, addr) = start_listener(b.path(), &fp_a);
    let dialer = Guard(
        macwin(a.path())
            .args([
                "spike",
                "net",
                "--connect",
                &addr,
                "--peer-fp",
                &fp_b,
                "--count",
                "20",
                "--interval-ms",
                "1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = wait_with_timeout(dialer, Duration::from_secs(30));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "dialer failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for path in ["stream", "datagram"] {
        let line = stdout
            .lines()
            .find(|l| l.starts_with(path))
            .unwrap_or_else(|| panic!("no {path} line in {stdout}"));
        for field in ["n=20", "lost=0", "p50=", "p95=", "p99=", "max="] {
            assert!(line.contains(field), "{field} missing in {line:?}");
        }
    }
    // The listener exits on its own after the normal close.
    let out = wait_with_timeout(listener, Duration::from_secs(10));
    assert!(out.status.success(), "listener exit: {:?}", out.status);
}

#[test]
fn dialer_with_a_wrong_pin_fails_quickly() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let fp_a = fingerprint_of(a.path());
    let (_listener, addr) = start_listener(b.path(), &fp_a);
    let wrong = "00".repeat(32);
    let started = Instant::now();
    let dialer = Guard(
        macwin(a.path())
            .args([
                "spike",
                "net",
                "--connect",
                &addr,
                "--peer-fp",
                &wrong,
                "--count",
                "5",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = wait_with_timeout(dialer, Duration::from_secs(10));
    assert!(
        !out.status.success(),
        "the dialer must refuse an unpinned server"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn pair_finds_the_waiting_machine_on_the_lan() {
    let mac = tempfile::tempdir().unwrap();
    let pc = tempfile::tempdir().unwrap();
    let lan = format!("127.0.0.1:{}", free_udp_port());
    let mut waiting = Guard(
        macwin(pc.path())
            .env("MACWIN_DISCOVERY", &lan)
            .args(["pair", "--listen", "127.0.0.1:0", "--side", "left"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let lines = lines_of(&mut waiting);
    let code = wait_line(&lines, "pairing code: ");
    wait_line(&lines, "  macwin pair --code ");
    let out = macwin(mac.path())
        .env("MACWIN_DISCOVERY", &lan)
        .args(["pair", "--code", &code])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(
        stdout.contains("found \"") && stdout.contains("paired with"),
        "{stdout}"
    );
    let waited = wait_with_timeout(waiting, Duration::from_secs(15));
    assert!(waited.status.success(), "{waited:?}");
    // The searching machine dials from now on, at the address it found.
    let mac_cfg = config_of(mac.path());
    assert!(mac_cfg.contains("address = \"127.0.0.1:"), "{mac_cfg}");
    assert!(mac_cfg.contains("side = \"right\""), "{mac_cfg}");
    assert!(!config_of(pc.path()).contains("address ="));
}

#[test]
fn pair_with_a_code_and_nobody_waiting_explains_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let lan = format!("127.0.0.1:{}", free_udp_port());
    let out = macwin(dir.path())
        .env("MACWIN_DISCOVERY", &lan)
        .args(["pair", "--code", "123456"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no machine waiting for pairing"),
        "{stderr}"
    );
    let off = macwin(dir.path())
        .args(["pair", "--code", "123456"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&off.stderr).contains("LAN discovery is off"));
}

#[test]
fn a_second_agent_with_the_same_config_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut waiting = Guard(
        macwin(dir.path())
            .args(["pair", "--listen", "127.0.0.1:0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let lines = lines_of(&mut waiting);
    wait_line(&lines, "pairing code: ");
    let out = macwin(dir.path()).arg("run").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already running"), "{stderr}");
}
