//! The `macwin run` loop of the configuration, shared by `macwin run` and
//! the tray.

use std::path::PathBuf;

use macwin_core::engine::Timings;
use macwin_core::types::{Os, Side};

use crate::config::{Config, PeerConfig};
use crate::driver::{ClipboardSettings, SessionConfig};
use crate::lan::Discovery;
use crate::runner::{self, Mode, RunOptions};

/// Why the configuration cannot run yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReady {
    NotPaired,
    /// Paired, but where the peer's screens are is unknown.
    NoSide,
}

pub struct Launch {
    pub opts: RunOptions,
    pub peer: PeerConfig,
    pub side: Side,
    /// The session shares the clipboard (the backends need one).
    pub with_clipboard: bool,
}

impl Launch {
    pub fn new(
        config: &Config,
        os: Os,
        device_name: String,
        staging: PathBuf,
        discovery: Option<Discovery>,
    ) -> Result<Launch, NotReady> {
        let peer = config.peer.clone().ok_or(NotReady::NotPaired)?;
        let side = peer.side.ok_or(NotReady::NoSide)?;
        let c = &config.clipboard;
        let clipboard = c.enabled.then(|| ClipboardSettings {
            max_text_bytes: c.max_text_bytes(),
            images: c.images,
            max_image_bytes: macwin_proto::MAX_CLIPBOARD_IMAGE,
            files: c.files,
            max_files_bytes: c.max_files_bytes(),
            staging,
            poll: crate::clipboard::POLL_INTERVAL,
        });
        let with_clipboard = clipboard.is_some();
        let session = SessionConfig {
            local_os: os,
            os_version: String::new(),
            device_name,
            peer_side: side,
            remap: config.remap,
            timings: Timings::default(),
            clipboard,
        };
        let mode = match &peer.address {
            Some(a) => Mode::Dial(a.clone()),
            None => Mode::Listen(config.listen),
        };
        Ok(Launch {
            opts: RunOptions {
                session,
                mode,
                peer: peer.fingerprint,
                retry_min: runner::RETRY_MIN,
                retry_max: runner::RETRY_MAX,
                connect_timeout: runner::CONNECT_TIMEOUT,
                discovery,
            },
            peer,
            side,
            with_clipboard,
        })
    }

    /// What the clipboard shares: "text, images, files" or "off".
    pub fn sharing(&self) -> String {
        match &self.opts.session.clipboard {
            None => "off".into(),
            Some(s) => {
                let mut kinds = vec!["text"];
                if s.images {
                    kinds.push("images");
                }
                if s.files {
                    kinds.push("files");
                }
                kinds.join(", ")
            }
        }
    }
}
