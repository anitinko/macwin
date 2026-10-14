//! Logging to stderr; `RUST_LOG` overrides the default `info` level. The
//! tray logs to a file instead (it has no terminal).
//!
//! Key identities are only ever logged at TRACE, never above.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// A log above this size is moved to `macwin.log.old` when the tray starts.
const MAX_LOG: u64 = 10 * 1024 * 1024;

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_writer(std::io::stderr)
        .try_init();
}

/// The tray's log: `~/Library/Logs/macwin/macwin.log` on macOS, otherwise
/// `macwin.log` in the cache directory (`%LOCALAPPDATA%\macwin` on
/// Windows). With `MACWIN_CONFIG_DIR` or `MACWIN_CACHE_DIR` set it is in
/// the cache directory everywhere.
pub fn log_path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let overridden = env("MACWIN_CONFIG_DIR").is_some() || env("MACWIN_CACHE_DIR").is_some();
    if cfg!(target_os = "macos") && !overridden {
        return env("HOME").map(|h| PathBuf::from(h).join("Library/Logs/macwin/macwin.log"));
    }
    macwin_net::cache_dir(env)
        .ok()
        .map(|d| d.join("macwin.log"))
}

fn open_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = fs::rename(path, path.with_extension("log.old"));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Logs to [`log_path`]; falls back to stderr. Returns the log file.
pub fn init_file() -> Option<PathBuf> {
    let opened = log_path().and_then(|p| open_log(&p).ok().map(|f| (p, f)));
    let Some((path, file)) = opened else {
        init();
        return None;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_log_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/macwin.log");
        drop(open_log(&path).unwrap());
        fs::write(&path, vec![b'x'; (MAX_LOG + 1) as usize]).unwrap();
        let f = open_log(&path).unwrap();
        assert_eq!(f.metadata().unwrap().len(), 0);
        assert!(dir.path().join("logs/macwin.log.old").exists());
    }
}
