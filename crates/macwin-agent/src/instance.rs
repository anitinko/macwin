//! One agent per user: `run`, `pair` and `tray` hold an exclusive lock on
//! `macwin.lock` in the config directory while they use the network and
//! the input devices. The OS releases it when the process ends.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use anyhow::Context;
use macwin_platform::fs::create_private_dir;

pub const LOCK_FILE: &str = "macwin.lock";

/// Held until dropped.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Another macwin agent of this user is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyRunning;

impl fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "macwin is already running (the MacWin menu bar or tray icon, `macwin run` or \
             `macwin pair`); quit it first",
        )
    }
}

impl std::error::Error for AlreadyRunning {}

pub fn lock(config_dir: &Path) -> anyhow::Result<InstanceLock> {
    create_private_dir(config_dir)
        .with_context(|| format!("cannot create {}", config_dir.display()))?;
    let path = config_dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(AlreadyRunning.into()),
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("cannot lock {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_fails_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = lock(&dir.path().join("cfg")).unwrap();
        let err = lock(&dir.path().join("cfg")).unwrap_err();
        assert!(err.downcast_ref::<AlreadyRunning>().is_some(), "{err:#}");
        drop(first);
        lock(&dir.path().join("cfg")).unwrap();
    }
}
