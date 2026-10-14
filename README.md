# macwin

A software KVM for one Mac and one Windows PC on the same LAN: either
machine's keyboard and mouse can drive both screens, and copied text,
images and files are shared between them.

**Status: Phase 3.** Pairing by a one-time code (machines find each other
on the LAN), the identity key in the macOS Keychain / Windows DPAPI, a
config file and `macwin run` (keyboard, mouse with smooth trackpad
scrolling, clipboard sharing of text, images and files, automatic
reconnects, a peer that changed its address is found again) are implemented
and tested with fakes. MacWin runs as a menu bar / notification area app,
starts at login and is packaged as a dmg and a zip with an installer. The
real OS backends and the tray still need testing on hardware.

## Workspace

| Path | What it is |
|---|---|
| `crates/macwin-core` | Pure logic: vocabulary types, edge-mapping geometry, HID keymap, Ctrl/Cmd remap, pressed-key tracking, heartbeat, emergency chord, the controller state machine (`Engine`), clipboard sync ordering. No I/O, threads or clocks. |
| `crates/macwin-proto` | Strict protobuf codec (hand-written `prost` derives, no `protoc`), stream framing, version check. |
| `crates/macwin-net` | QUIC (quinn + rustls/ring) with mutual TLS and SHA-256 certificate pinning; SPAKE2 pairing; LAN discovery; identity bundles; the `Link` abstraction. |
| `crates/macwin-platform` | Backend traits (capture, injection, screens, clipboard, secret store), tray view types and icons, in-memory fakes (`testing` feature). |
| `crates/macwin-platform-macos` | macOS backend: CGEventTap capture, CGEventPost injection, displays, NSPasteboard, login keychain, interfaces, menu bar icon (empty on other OSes). |
| `crates/macwin-platform-windows` | Windows backend: low-level hooks, SendInput, per-monitor-v2 DPI, Win32 clipboard, DPAPI, interfaces, `Run` key, notification area icon (empty on other OSes). |
| `crates/macwin-agent` | Session driver, reconnecting runner, config, tray logic, login item (library), the `macwin` binary and `macwin-tray` (Windows GUI subsystem). |
| `scripts/macos` | Builds and signs `target/MacWin.app`, runs it through LaunchServices, stops it, builds the dmg. |
| `scripts/windows` | Builds the Windows zip; `install.ps1` / `uninstall.ps1` inside it. |
| `tools/keymap-gen` | Generates the HID <-> macOS keycode <-> Windows scancode table from Chromium data. |

## Building

`rust-toolchain.toml` pins Rust 1.96.0. If your `stable` toolchain already is
1.96.0, you can run every command with `RUSTUP_TOOLCHAIN=stable` instead of
installing the pinned one:

```sh
rustup toolchain install 1.96.0 --profile minimal -c rustfmt,clippy -t x86_64-pc-windows-gnu
cargo test --workspace --all-features
cargo build --release -p macwin-agent          # target/release/macwin
```

### Windows binary from a Mac

```sh
brew install mingw-w64                          # provides x86_64-w64-mingw32-gcc
cargo build --release -p macwin-agent --target x86_64-pc-windows-gnu
# -> target/x86_64-pc-windows-gnu/release/macwin.exe
```

Windows CI builds natively with MSVC.

### Packages

```sh
scripts/macos/dmg.sh          # target/MacWin-<version>.dmg (stripped `dist` build, signed app)
scripts/windows/package.sh    # target/macwin-<version>-windows-x64.zip (MinGW cross-build)
```

The dmg holds MacWin.app and a link to /Applications. Other Macs open the
app without a warning only when it is signed with a "Developer ID
Application" identity and notarized, which the script does not do. The zip
holds `macwin.exe`, `macwin-tray.exe`, `install.ps1`, `uninstall.ps1` and a
README; `install.ps1` installs for the current user into
`%LOCALAPPDATA%\Programs\MacWin`, adds it to the PATH, starts the tray at
login and adds firewall rules for private and domain networks after one
administrator prompt. The executables are not code-signed, so SmartScreen may warn.

## Usage

On a Mac, run every command through the signed app bundle
(`scripts/macos/run.sh <command>`, see below) so that macOS grants its
permissions and keychain access to MacWin.app.

### The menu bar / tray app

MacWin.app opened without arguments (from the Finder, the dmg, at login) and
`macwin-tray.exe` on Windows show an icon in the menu bar / notification
area; `macwin tray` does the same from a terminal. The icon shows whether
the other machine is connected, and its menu has:

- **Pair with the other computer...**: looks for a machine that waits for
  pairing on the LAN and asks for its code; when there is none, this machine
  waits and shows a code and its address. Do it on both machines. When
  broadcasts do not get through (another subnet, a guest Wi-Fi), click
  **Pair by address...** in one machine's code window and enter the address
  the other one shows. Afterwards each asks on which side the other
  machine's screens are.
- **Pause sharing / Resume sharing**, **Share the clipboard**,
  **"PC" is on the left / right**, **Start at login**, **Quit MacWin**.

The emergency chord pauses sharing; Resume starts it again. After an error
(for example a permission that is still missing) the tray tries again by
itself, after 5 s and then up to every minute; the tooltip shows the last
connection problem. Only one MacWin (tray, `run` or `pair`) runs per user at
a time: quit the tray before using `macwin pair` or `macwin run`. The tray logs to
`~/Library/Logs/macwin/macwin.log` (Windows `%LOCALAPPDATA%\macwin\macwin.log`).

### 1. Pair the machines (once), from a terminal

On both machines run `macwin pair` (on the Mac `scripts/macos/run.sh pair`,
which cannot ask for input; add `--code`). The first one finds nobody and
waits:

```sh
macwin.exe pair --side left       # the PC; its screens are right of the Mac
# listening on 0.0.0.0:47800
# pairing code: 123-456
# on the other machine run:
#   macwin pair --code 123456
# (if it does not find this machine: macwin pair 192.168.1.20:47800 --code 123456)
```

The second one finds the first on the LAN (UDP broadcast) and pairs with
the code:

```sh
scripts/macos/run.sh pair --code 123456
# found "DESKTOP-1" (windows) at 192.168.1.20:47800
# paired with "DESKTOP-1" (windows), fingerprint ...
# its screens are on the right of this machine
```

`--side` on either machine is enough; the other side is derived. Each code
works for one attempt: after a wrong code the waiting machine shows a new
one (at most 5 attempts, 10 minutes). The machine that dialed remembers the
address and dials it from then on; the other one listens. Without discovery
(another subnet, `discovery = false`) give the address:
`macwin pair HOST[:PORT] --code CODE`; `macwin pair --listen ADDR:PORT` only
waits.

### 2. Share

```sh
macwin.exe run                    # on the PC (or the tray app)
scripts/macos/run.sh run          # on the Mac
```

Push the cursor over the shared edge to control the other machine; touch the
other machine's own mouse or keyboard to take control back. Trackpad and
Magic Mouse scrolling stays smooth on the PC. `run` reconnects by itself when
the network drops or the other machine restarts; when the listening machine
got a new IP address, the dialing one finds it on the LAN and saves the new
address (a host name in `address` is kept). Ctrl+C stops it, and so does
the emergency chord **Ctrl+Alt+Shift+Esc** (Control+Option+Shift+Esc on a Mac).

What you copy appears on the other machine's clipboard:

- **Text**, up to `clipboard-max-kb` (4 MiB by default).
- **Images** (screenshots, pictures copied in a browser or an editor), as
  PNG, up to 64 MiB.
- **Files and folders** copied in Finder or Explorer, up to
  `clipboard-files-max-mb` in total (512 MiB by default). They are sent
  right away when copied and wait in the cache directory
  (`~/Library/Caches/macwin/incoming`, Windows `%LOCALAPPDATA%\macwin\incoming`,
  or `MACWIN_CACHE_DIR`); paste in Finder or Explorer copies them out. Only
  the latest copy is kept there, across reconnects. A copy travels whole
  or not at all: an unreadable item, a name with a backslash or more than
  65 536 items keep it on this machine. Symlinks are skipped, and names
  that Windows does not accept are adjusted (`a:b.txt` becomes `a_b.txt`).

When a copy holds several kinds (Word and Excel put text and a picture),
files win over text, and text over an image.

### Configuration

`config.toml` lives in the config directory
(`~/Library/Application Support/macwin`, Windows `%APPDATA%\macwin`,
or `MACWIN_CONFIG_DIR`):

```sh
macwin config show
macwin config set side right               # where the peer's screens are
macwin config set address pc.local:47800   # dial this; `none` = listen
macwin config set listen 0.0.0.0:47800
macwin config set remap off                # keep Ctrl and Cmd/Win as they are
macwin config set discovery off            # no LAN broadcasts (pair by address)
macwin config set clipboard off            # stop sharing the clipboard
macwin config set clipboard-max-kb 4096    # largest shared text
macwin config set clipboard-images off     # text and files only
macwin config set clipboard-files off      # text and images only
macwin config set clipboard-files-max-mb 2048  # largest shared copy of files
macwin unpair                              # forget the peer
macwin autostart on|off                    # start the tray app at login; no argument: show
```

`macwin autostart on` writes `~/Library/LaunchAgents/io.macwin.MacWin.plist`
(it starts `MacWin.app/Contents/MacOS/macwin tray`; run it through
`scripts/macos/run.sh` so it registers the app) or the `MacWin` value of
the current user's `Run` key (`macwin-tray.exe`). `MACWIN_DISCOVERY=off`
turns discovery off for one command; `MACWIN_DISCOVERY=ADDR:PORT` answers on
and searches only that address (tests).

### Identity

Every machine has a self-signed identity, created on first use:

```sh
macwin identity
# fingerprint: 3f1c...e9 (64 hex digits)
# key: login keychain, item "io.macwin.MacWin" / "identity"
# config dir: ~/Library/Application Support/macwin
```

The certificate and key are kept in the login keychain on macOS and in a
DPAPI-encrypted `identity.dpapi` on Windows. `MACWIN_KEY_STORE=file` keeps
them in an unencrypted `identity.secret` (mode 0600) instead, for headless
machines and tests. Phase 0 identity files are moved into the store
automatically.

### Diagnostics and spikes

```sh
macwin probe screens                 # monitors in native coordinates
macwin probe permissions [--prompt]  # macOS: Accessibility, Input Monitoring, Secure Input
macwin probe clipboard [--watch]     # format names and sizes, never the content
macwin spike capture [--grab]        # print (and with --grab, swallow) local input
macwin spike inject --demo           # cursor draws a square, then "hello" is typed
macwin spike net --listen 0.0.0.0:47800 --peer-fp <hex>    # latency, with manual pins
macwin spike kvm --connect <host>:47800 --peer-fp <hex> --peer-side right
```

### macOS app bundle

macOS grants input permissions per app and per code signature, so run macwin
from a signed bundle:

```sh
scripts/macos/bundle.sh                       # builds and signs target/MacWin.app
scripts/macos/run.sh probe permissions --prompt
scripts/macos/run.sh run                      # output in ~/Library/Logs/macwin/macwin.log
open target/MacWin.app                        # the menu bar app
scripts/macos/stop.sh
```

Grant **Accessibility** and **Input Monitoring** to MacWin in
System Settings > Privacy & Security, and allow **Local Network** when asked.
On macOS versions that ask before apps read the clipboard, allow MacWin
under **Paste from Other Apps**. `bundle.sh` signs with
`MACWIN_SIGN_IDENTITY` or the first "Apple Development" identity, so the
permissions and keychain access survive rebuilds.

Known limits: windows of elevated (admin) Windows apps, the Windows lock
screen and UAC prompts do not accept remote input yet; macOS Secure Input
(password fields) hides keystrokes from macwin while active. Copied files
travel when they are copied, not when they are pasted, so a large copy
takes a moment to become pasteable on the other machine.

## Security model

- There is no unencrypted or unauthenticated session. Every connection is
  TLS 1.3 over QUIC with client certificates, and each side accepts exactly
  one peer, identified by the SHA-256 fingerprint of its certificate.
- Pairing (separate ALPN `macwin-pair/1`) runs SPAKE2 with the one-time code
  over a TLS connection that accepts any certificate. Both certificates'
  fingerprints and a TLS exporter value are bound into the SPAKE2 identities
  and the key confirmation, so a man in the middle needs the code, and each
  attempt tests at most one guess.
- The identity key lives in the Keychain / DPAPI (see above).
- LAN discovery (UDP port 47801) is unauthenticated on purpose: an answer
  only suggests an address. Pairing still needs the code, and a session
  still needs the pinned certificate; a new address is saved only after a
  pinned session to it succeeded. Searches for the paired machine carry a
  hash of the nonce and its fingerprint, not the fingerprint; answers go
  only to private, link-local and loopback addresses. Turn it off with
  `discovery = false`.
- Clipboard: each kind (text, images, files) is shared only while both
  machines enable it. Content marked private by password managers
  (`org.nspasteboard.Concealed` / `Transient`, Windows
  `ExcludeClipboardContentFromMonitorProcessing`,
  `CanUploadToCloudClipboard = 0`) is never sent, and clipboard content and
  file names are never logged. Received file paths are checked (no `..`,
  absolute paths or drive letters) before anything is written, and files
  are only ever written inside the cache directory.
- Emergency chord: **Ctrl+Alt+Shift+Esc** (Control+Option+Shift+Esc on a Mac).
  Capture backends check it on physical keys before anything is swallowed, and
  it always returns local control.

## Keymap data

`crates/macwin-core/src/keymap/generated.rs` is generated and checked in:

```sh
tools/keymap-gen/fetch.sh          # re-download Chromium's dom_code_data.inc (rarely needed)
cargo run -p keymap-gen            # regenerate
cargo run -p keymap-gen -- --check # CI: fails if the table is stale
```

## Licensing

Third-party data and its license are listed in `THIRD_PARTY_NOTICES`.
Dependencies must use permissive licenses; `deny.toml` (cargo-deny, run in CI)
enforces this. The project's own license is not decided yet.
