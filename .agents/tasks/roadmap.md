# Roadmap after Phase 1 (user said "делай все": do every remaining phase, commit each)

Rules unchanged: in-chat work, RUSTUP_TOOLCHAIN=stable, never run capture/inject/clipboard/kvm
or launch MacWin.app on this Mac, tests on 127.0.0.1, pinned permissive deps, Russian replies.

## Phase 2: files and images through the clipboard, smooth scroll
- Clipboard content priority: files > text > image (Excel/Word text copies carry pictures).
- Images: PNG on the wire (ClipboardMsg::Image, max 64 MiB). mac: read public.png or
  public.tiff (-> PNG via NSBitmapImageRep), write PNG + TIFF. win: read "PNG" format or
  CF_DIBV5/CF_DIB (-> PNG via pure-Rust DIB parser + png crate), write "PNG" + CF_DIB.
  Echo signature = hash of decoded pixels (survives re-encoding by clipboard managers).
- Files: copied files are transferred eagerly on copy (limit config max_files_mb, default 512,
  0 = off) on a uni stream type 0x11: FilesHeader frame (clock + entries: '/'-relative path,
  size, dir, executable, mtime) + raw concatenated data. Receiver writes into a staging dir
  (cache dir/incoming/<id>), keeps only the latest transfer, then puts the top-level paths on
  its clipboard (mac: NSURL writeObjects; win: CF_HDROP + Preferred DropEffect=COPY) so
  Finder/Explorer paste copies them. Symlinks skipped. Names sanitized per OS.
  Echo signature = hash of manifest (rel paths + sizes), location independent.
- Capabilities clipboard.image / clipboard.files; sender only sends what the peer accepts.
- Smooth scroll: WheelUnit::Pixel end to end (mac trackpad continuous deltas).

## Phase 3: product shell
- LAN discovery by UDP broadcast (port 47801): find the pairing listener without typing an IP,
  and re-find the peer by fingerprint when its address changed (update config).
- Autostart: macOS LaunchAgent for MacWin.app `run`; Windows HKCU Run key. `macwin autostart`.
- Tray / menu bar UI (status, pause, clipboard toggle, pair, quit) — thin layer, logic tested.
- Packaging: macOS dmg via hdiutil; Windows zip + install.ps1 (copy, Run key, firewall rule);
  release profile strip; Windows GUI-subsystem binary for the tray.

## Phase 4: Windows secure desktop (lock screen / UAC) — evaluate feasibility; at least design.

## Status
- Phase 2 done (committed): images, files, smooth scroll. Image echo: the clipboard worker
  remembers the last image it read or wrote with a hash of its decoded pixels and reports a
  re-encoded copy with the earlier bytes (core keeps comparing bytes). Pixel wheel is gated by
  capability input.wheel-pixel; older peers get Notch120 (40 px = 1 notch).
- Phase 3 done (phase3-plan.md, phase3-report.md): LAN discovery (UDP 47801; pairing without an
  IP, re-finding a peer that moved), `macwin autostart` (LaunchAgent / HKCU Run), the tray app
  (macOS menu bar, Windows notification area; logic tested with a fake UI), one agent per user
  (lock file), dmg + Windows zip with install.ps1 / uninstall.ps1, `dist` profile. Work moved to a
  Linux sandbox: macOS and Windows code is type-checked with cross clippy and runs in CI.
- Phase 4: feasibility study and design only (phase4-design.md): SYSTEM service + per-session
  helper for the Winlogon desktop and elevated windows; needs owner decisions (admin install in
  Program Files, code signing, Ctrl+Alt+Del policy) before it is built.
- Next: hardware test of phases 0b-3 (checklists in the reports), then the Phase 4 decisions.
