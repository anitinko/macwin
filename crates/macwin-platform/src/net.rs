//! Helpers for the OS network-interface lists used by LAN discovery.

use std::net::Ipv4Addr;

/// The directed broadcast address of `addr` in a network with `mask`, or
/// `None` for loopback addresses and /31 or /32 networks (no broadcast).
pub fn directed_broadcast(addr: Ipv4Addr, mask: Ipv4Addr) -> Option<Ipv4Addr> {
    let mask = u32::from(mask);
    if addr.is_loopback() || addr.is_unspecified() || mask.count_ones() > 30 {
        return None;
    }
    Some(Ipv4Addr::from(u32::from(addr) | !mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_addresses() {
        let b = |a: &str, m: &str| directed_broadcast(a.parse().unwrap(), m.parse().unwrap());
        assert_eq!(
            b("192.168.1.20", "255.255.255.0"),
            Some("192.168.1.255".parse().unwrap())
        );
        assert_eq!(
            b("10.1.2.3", "255.0.0.0"),
            Some("10.255.255.255".parse().unwrap())
        );
        assert_eq!(
            b("172.20.5.9", "255.255.240.0"),
            Some("172.20.15.255".parse().unwrap())
        );
        assert_eq!(b("10.0.0.1", "255.255.255.254"), None);
        assert_eq!(b("10.0.0.1", "255.255.255.255"), None);
        assert_eq!(b("127.0.0.1", "255.0.0.0"), None);
    }
}
