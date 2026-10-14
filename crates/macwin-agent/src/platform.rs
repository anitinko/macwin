//! The real backends of the host OS (macOS or Windows).

use std::path::Path;

use macwin_platform::{FileSecretStore, PlatformError, SecretStore};

pub use imp::{
    capture, clipboard, init, injector, ipv4_broadcasts, notice, permissions_report, run_tray,
    screens,
};

use crate::driver::Backends;
use crate::identity_store::KeyStoreKind;

/// All backends a KVM session needs; the clipboard only when it is shared.
pub fn backends(with_clipboard: bool) -> Result<Backends, PlatformError> {
    Ok(Backends {
        capture: capture(),
        injector: injector()?,
        screens: screens(),
        clipboard: with_clipboard.then(clipboard),
    })
}

/// The secret store of `kind`; file-based stores live in `config_dir`.
pub fn secret_store(
    kind: KeyStoreKind,
    config_dir: &Path,
) -> Result<Box<dyn SecretStore>, PlatformError> {
    match kind {
        KeyStoreKind::File => Ok(Box::new(FileSecretStore::new(config_dir))),
        KeyStoreKind::Keychain | KeyStoreKind::Dpapi => imp::os_secret_store(kind, config_dir),
    }
}

/// A human-readable name of this machine.
pub fn device_name() -> String {
    imp::device_name()
        .or_else(|| {
            ["COMPUTERNAME", "HOSTNAME"]
                .iter()
                .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;

    use macwin_platform::{
        ClipboardBackend, InputCapture, InputInjector, PlatformError, ScreenSource, SecretStore,
    };
    use macwin_platform_macos::{
        KeychainStore, MacCapture, MacClipboard, MacInjector, MacScreens, permissions,
    };

    use crate::identity_store::KeyStoreKind;

    pub fn init() {}

    pub fn screens() -> Box<dyn ScreenSource> {
        Box::new(MacScreens)
    }

    pub fn capture() -> Box<dyn InputCapture> {
        Box::new(MacCapture::new())
    }

    pub fn injector() -> Result<Box<dyn InputInjector>, PlatformError> {
        Ok(Box::new(MacInjector::new()?))
    }

    pub fn clipboard() -> Box<dyn ClipboardBackend> {
        Box::new(MacClipboard)
    }

    pub fn device_name() -> Option<String> {
        macwin_platform_macos::device_name()
    }

    pub fn ipv4_broadcasts() -> Vec<std::net::Ipv4Addr> {
        macwin_platform_macos::ipv4_broadcasts()
    }

    /// Runs the menu bar icon on this (the main) thread until Quit.
    pub fn run_tray(
        commands: std::sync::mpsc::Receiver<macwin_platform::tray::UiCommand>,
        events: impl Fn(macwin_platform::tray::TrayEvent) + Send + 'static,
    ) -> Result<(), PlatformError> {
        macwin_platform_macos::tray::run(commands, Box::new(events))
    }

    /// A message for the user when there is no tray to show it.
    pub fn notice(message: &str) {
        macwin_platform_macos::tray::notice(message)
    }

    pub fn os_secret_store(
        kind: KeyStoreKind,
        _config_dir: &Path,
    ) -> Result<Box<dyn SecretStore>, PlatformError> {
        match kind {
            KeyStoreKind::Keychain => Ok(Box::new(KeychainStore::new())),
            _ => Err(PlatformError::Unsupported),
        }
    }

    /// `prompt` asks macOS to show the permission prompts first.
    pub fn permissions_report(prompt: bool) -> String {
        let p = if prompt {
            permissions::request()
        } else {
            permissions::check()
        };
        let state = |b: bool| if b { "granted" } else { "MISSING" };
        let mut out = format!(
            "accessibility:    {}\ninput monitoring: {}\npost events:      {}\nsecure input:     {}\n",
            state(p.accessibility),
            state(p.listen_events),
            state(p.post_events),
            if p.secure_input {
                "ON (keystrokes are hidden from macwin until it ends)"
            } else {
                "off"
            },
        );
        if !p.sufficient() {
            out.push_str(
                "\nGrant the missing permissions to the app that runs macwin (MacWin.app, or your terminal) in\n\
                 System Settings > Privacy & Security > Accessibility / Input Monitoring, then restart it.\n",
            );
        }
        out
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use macwin_platform::{
        ClipboardBackend, InputCapture, InputInjector, PlatformError, ScreenSource, SecretStore,
    };
    use macwin_platform_windows::{
        DpapiStore, WinCapture, WinClipboard, WinInjector, WinScreens, init_dpi_awareness,
    };

    use crate::identity_store::KeyStoreKind;

    /// Per-monitor-v2 DPI awareness; must run before any backend is used.
    pub fn init() {
        let _ = init_dpi_awareness();
    }

    pub fn screens() -> Box<dyn ScreenSource> {
        Box::new(WinScreens)
    }

    pub fn capture() -> Box<dyn InputCapture> {
        Box::new(WinCapture::new())
    }

    pub fn injector() -> Result<Box<dyn InputInjector>, PlatformError> {
        Ok(Box::new(WinInjector::new()))
    }

    pub fn clipboard() -> Box<dyn ClipboardBackend> {
        Box::new(WinClipboard)
    }

    pub fn device_name() -> Option<String> {
        None
    }

    pub fn ipv4_broadcasts() -> Vec<std::net::Ipv4Addr> {
        macwin_platform_windows::ipv4_broadcasts()
    }

    /// Runs the notification area icon on this thread until Quit.
    pub fn run_tray(
        commands: std::sync::mpsc::Receiver<macwin_platform::tray::UiCommand>,
        events: impl Fn(macwin_platform::tray::TrayEvent) + Send + 'static,
    ) -> Result<(), PlatformError> {
        macwin_platform_windows::tray::run(commands, Box::new(events))
    }

    /// A message for the user when there is no tray to show it.
    pub fn notice(message: &str) {
        macwin_platform_windows::tray::notice(message)
    }

    pub fn os_secret_store(
        kind: KeyStoreKind,
        config_dir: &Path,
    ) -> Result<Box<dyn SecretStore>, PlatformError> {
        match kind {
            KeyStoreKind::Dpapi => Ok(Box::new(DpapiStore::new(config_dir))),
            _ => Err(PlatformError::Unsupported),
        }
    }

    pub fn permissions_report(_prompt: bool) -> String {
        "Windows needs no permission prompts.\n\
         Limits: elevated (admin) windows do not accept input from a non-elevated macwin, and the lock\n\
         screen / UAC prompts are unreachable until the SYSTEM service exists (Phase 4).\n"
            .into()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::path::Path;

    use macwin_platform::{
        ClipboardBackend, InputCapture, InputInjector, PlatformError, ScreenSource, SecretStore,
        UnsupportedPlatform,
    };

    use crate::identity_store::KeyStoreKind;

    pub fn init() {}

    pub fn screens() -> Box<dyn ScreenSource> {
        Box::new(UnsupportedPlatform)
    }

    pub fn capture() -> Box<dyn InputCapture> {
        Box::new(UnsupportedPlatform)
    }

    pub fn injector() -> Result<Box<dyn InputInjector>, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    pub fn clipboard() -> Box<dyn ClipboardBackend> {
        Box::new(UnsupportedPlatform)
    }

    pub fn device_name() -> Option<String> {
        None
    }

    pub fn ipv4_broadcasts() -> Vec<std::net::Ipv4Addr> {
        Vec::new()
    }

    pub fn run_tray(
        _commands: std::sync::mpsc::Receiver<macwin_platform::tray::UiCommand>,
        _events: impl Fn(macwin_platform::tray::TrayEvent) + Send + 'static,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    pub fn notice(message: &str) {
        eprintln!("{message}");
    }

    pub fn os_secret_store(
        _kind: KeyStoreKind,
        _config_dir: &Path,
    ) -> Result<Box<dyn SecretStore>, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    pub fn permissions_report(_prompt: bool) -> String {
        "unsupported OS\n".into()
    }
}
