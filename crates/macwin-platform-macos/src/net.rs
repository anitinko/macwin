//! The IPv4 broadcast addresses of the active interfaces (LAN discovery).

use std::net::Ipv4Addr;
use std::ptr;

use macwin_platform::net::directed_broadcast;

fn ipv4(sa: *const libc::sockaddr) -> Option<Ipv4Addr> {
    if sa.is_null() {
        return None;
    }
    // SAFETY: a non-null sockaddr from getifaddrs; `sa_family` tells which
    // struct it is, and AF_INET entries are `sockaddr_in`.
    unsafe {
        if i32::from((*sa).sa_family) != libc::AF_INET {
            return None;
        }
        let sin = &*sa.cast::<libc::sockaddr_in>();
        Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)))
    }
}

/// The directed broadcast address (for example 192.168.1.255) of every
/// interface that is up, not loopback and broadcast-capable.
pub fn ipv4_broadcasts() -> Vec<Ipv4Addr> {
    let mut list: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: on success `list` holds a linked list, freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = list;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list, valid until freeifaddrs.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        let flags = ifa.ifa_flags as libc::c_int;
        let wanted = flags & libc::IFF_UP != 0
            && flags & libc::IFF_BROADCAST != 0
            && flags & libc::IFF_LOOPBACK == 0;
        if !wanted {
            continue;
        }
        if let (Some(addr), Some(mask)) = (ipv4(ifa.ifa_addr), ipv4(ifa.ifa_netmask))
            && let Some(b) = directed_broadcast(addr, mask)
            && !out.contains(&b)
        {
            out.push(b);
        }
    }
    // SAFETY: the list of the successful getifaddrs above, freed once.
    unsafe { libc::freeifaddrs(list) };
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn broadcast_addresses_are_not_loopback() {
        for b in super::ipv4_broadcasts() {
            assert!(!b.is_loopback() && !b.is_unspecified(), "{b}");
        }
    }
}
