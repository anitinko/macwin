//! Where LAN discovery answers and where its searches go. The environment
//! variable `MACWIN_DISCOVERY` overrides the config: `off`, or `ADDR:PORT`
//! to answer on and search only that address (tests on loopback).

use std::ffi::OsString;
use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context;
use macwin_net::discovery::{DISCOVERY_PORT, Found, SEARCH_TIME, broadcast_targets, find_pairing};

pub const DISCOVERY_ENV: &str = "MACWIN_DISCOVERY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// Where a responder listens.
    pub bind: SocketAddr,
    /// The only search target; `None`: the broadcast addresses of the moment.
    fixed: Option<SocketAddr>,
}

impl Discovery {
    /// Broadcasts on the real network.
    pub fn lan() -> Discovery {
        Discovery {
            bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)),
            fixed: None,
        }
    }

    /// Answers on, and searches, only `addr`.
    pub fn at(addr: SocketAddr) -> Discovery {
        Discovery {
            bind: addr,
            fixed: Some(addr),
        }
    }

    /// From the config switch and the environment; `None` when it is off.
    pub fn from_env(
        enabled: bool,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> anyhow::Result<Option<Discovery>> {
        let value = env(DISCOVERY_ENV).filter(|v| !v.is_empty());
        match value.as_ref().map(|v| v.to_string_lossy()) {
            Some(v) if v == "off" => Ok(None),
            Some(v) => {
                let addr = v.parse::<SocketAddr>().with_context(|| {
                    format!("{DISCOVERY_ENV}: expected off or ADDR:PORT, got {v:?}")
                })?;
                Ok(Some(Discovery::at(addr)))
            }
            None => Ok(enabled.then(Discovery::lan)),
        }
    }

    /// Where a search sends its queries now (interfaces come and go).
    pub fn targets(&self) -> Vec<SocketAddr> {
        match self.fixed {
            Some(addr) => vec![addr],
            None => broadcast_targets(&crate::platform::ipv4_broadcasts(), DISCOVERY_PORT),
        }
    }

    /// The machines waiting for pairing; a failed search finds none.
    pub async fn find_pairing(&self) -> anyhow::Result<Vec<Found>> {
        find_pairing(&self.targets(), SEARCH_TIME)
            .await
            .context("the LAN search failed (on a Mac, allow MacWin under Local Network)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_overrides_the_config() {
        let none = |_: &str| None;
        assert_eq!(
            Discovery::from_env(true, none).unwrap(),
            Some(Discovery::lan())
        );
        assert_eq!(Discovery::from_env(false, none).unwrap(), None);
        let off = |_: &str| Some(OsString::from("off"));
        assert_eq!(Discovery::from_env(true, off).unwrap(), None);
        let at = |_: &str| Some(OsString::from("127.0.0.1:47901"));
        let d = Discovery::from_env(false, at).unwrap().unwrap();
        assert_eq!(d.bind, "127.0.0.1:47901".parse().unwrap());
        assert_eq!(d.targets(), vec![d.bind]);
        let bad = |_: &str| Some(OsString::from("nope"));
        assert!(Discovery::from_env(true, bad).is_err());
        assert_eq!(Discovery::lan().bind.port(), DISCOVERY_PORT);
        assert!(Discovery::lan().targets()[0].ip().is_ipv4());
    }
}
