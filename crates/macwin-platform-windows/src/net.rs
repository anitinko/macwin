//! The IPv4 broadcast addresses of the active interfaces (LAN discovery).

use std::net::Ipv4Addr;
use std::ptr;

use macwin_platform::net::directed_broadcast;
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetIpAddrTable, MIB_IPADDR_DELETED, MIB_IPADDR_DISCONNECTED, MIB_IPADDR_TRANSIENT,
    MIB_IPADDRTABLE,
};

/// The directed broadcast address (for example 192.168.1.255) of every
/// connected interface address that is not loopback.
pub fn ipv4_broadcasts() -> Vec<Ipv4Addr> {
    let mut size = 0u32;
    // SAFETY: a NULL table with size 0 only asks for the needed size.
    let r = unsafe { GetIpAddrTable(ptr::null_mut(), &mut size, 0) };
    if r != ERROR_INSUFFICIENT_BUFFER || size == 0 {
        return Vec::new();
    }
    // u64 elements keep the table 8-byte aligned.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let table = buf.as_mut_ptr().cast::<MIB_IPADDRTABLE>();
    // SAFETY: `buf` holds at least `size` bytes, suitably aligned.
    if unsafe { GetIpAddrTable(table, &mut size, 0) } != NO_ERROR {
        return Vec::new();
    }
    // SAFETY: on success the table holds `dwNumEntries` rows inside `buf`.
    let rows = unsafe {
        std::slice::from_raw_parts((*table).table.as_ptr(), (*table).dwNumEntries as usize)
    };
    let gone = MIB_IPADDR_DISCONNECTED | MIB_IPADDR_DELETED | MIB_IPADDR_TRANSIENT;
    let mut out = Vec::new();
    for row in rows {
        if u32::from(row.wType) & gone != 0 {
            continue;
        }
        // Both are IPv4 addresses in network byte order.
        let addr = Ipv4Addr::from(row.dwAddr.to_ne_bytes());
        let mask = Ipv4Addr::from(row.dwMask.to_ne_bytes());
        if let Some(b) = directed_broadcast(addr, mask)
            && !out.contains(&b)
        {
            out.push(b);
        }
    }
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
