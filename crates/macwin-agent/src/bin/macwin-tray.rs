//! `macwin-tray`: the MacWin tray app. On Windows it is a GUI-subsystem
//! program, so starting it (at login, from Explorer) opens no console.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    macwin_agent::tray::main()
}
