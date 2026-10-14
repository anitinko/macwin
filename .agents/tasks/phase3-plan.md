# Phase 3 plan (product shell)

Scope (roadmap.md): LAN discovery, autostart, tray / menu bar UI, packaging. Phase 4 (Windows
secure desktop) gets a feasibility study and a design only.

Constraints: unchanged (pinned permissive deps, no new crates unless unavoidable, unsafe only in
the OS crates, tests bind 127.0.0.1 only, never touch the real keychain / registry / LaunchAgents
in tests). Work happens in a Linux sandbox: macOS code is type-checked with
`cargo clippy --target aarch64-apple-darwin` (clang + stub SDK headers), Windows code with
`--target x86_64-pc-windows-gnu` (mingw); both run for real only in CI (macos-15, windows-2025).

## Decisions
- Discovery: UDP 47801, datagram = magic `MWD1` + protobuf `DiscoveryFrame` (core msg + proto
  codec, max 512 bytes). Query {nonce, want: pairing | peer(tag)}; answer {nonce, version, port,
  pairing{os, name} | peer{proof}}. tag/proof = SHA-256 over a label, the nonce and the
  fingerprint, so fingerprints never travel. Queries go to 255.255.255.255 and every interface's
  directed broadcast (getifaddrs / GetIpAddrTable); answers are unicast to the asker. Responders
  answer only private / link-local / loopback sources. Unauthenticated by design: pairing is
  protected by the code, sessions by certificate pinning; a discovered address is saved only
  after a pinned session to it succeeded, and only over an IP address (host names stay).
- `macwin pair`: no address -> search first; one machine waiting -> ask for its code; none ->
  wait and show a code (also announced on the LAN); `--code` without address requires exactly
  one machine found. `--listen` only waits.
- `macwin run`: the listener answers peer queries; the dialer searches after each failed dial
  and dials a new address at once; `RunEvent::Moved` -> config saved.
- Config `discovery = true`; env `MACWIN_DISCOVERY=off|ADDR:PORT` (tests: loopback).
- One agent per user: `run`, `pair`, `tray` take an exclusive lock on `<config>/macwin.lock`.
- Autostart: macOS LaunchAgent `~/Library/LaunchAgents/io.macwin.MacWin.plist` (`tray`,
  RunAtLoad, KeepAlive on crash, AssociatedBundleIdentifiers); Windows HKCU Run value `MacWin` =
  `macwin-tray.exe`. `macwin autostart [on|off]`.
- Tray: pure controller in the agent (tray view model, pairing flow, side choice, pause,
  clipboard and autostart toggles, emergency chord = pause) driving a thin OS layer through
  `UiCommand` / `TrayEvent` channels polled every 100 ms. macOS: NSStatusItem + NSMenu on the
  main thread, dialogs through `osascript`. Windows: Shell_NotifyIcon + popup menu + small
  Win32 dialog windows. Generated icons (pure Rust pixels). Logs go to a file.
- Binaries: `macwin` (CLI, `macwin tray`; MacWin.app opened without arguments starts the tray),
  `macwin-tray` (Windows GUI subsystem).
- Packaging: `[profile.dist]` (release, stripped, no debug info); `scripts/macos/dmg.sh`
  (hdiutil); `scripts/windows/package.sh` -> zip with install.ps1 / uninstall.ps1 (per-user
  install, Run key via `macwin autostart on`, firewall rules after one UAC prompt).

## Steps
1. Discovery: core msg, proto codec, net discovery, platform interface lists, agent plan,
   runner + pair integration, config key, instance lock, tests.
2. Autostart module + CLI.
3. Tray: platform types + icons, agent controller + tests, macOS and Windows layers, entry
   points, file logging.
4. Packaging scripts, dist profile, CI script checks.
5. README, roadmap, phase 3 report, phase 4 design; full sweep; commit; PR; CI.

## Status
- Steps 1-5 done (see phase3-report.md); Phase 4 design in phase4-design.md. CI on macos-15 and
  windows-2025 runs the OS-specific tests and the script checks.
