// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The addresses another machine can reach this viewer on.
//!
//! The service prints its URL when it starts. `http://127.0.0.1:port/` is
//! only useful on the machine itself; someone profiling from a laptop across
//! the room wants the address of this box on the network, and they should not
//! have to go and look it up. So the banner lists every up, non-loopback IPv4
//! interface next to the loopback line.
//!
//! Interfaces come from `getifaddrs`, through the `libc` crate the service
//! already carries -- no extra dependency, and it works in the static binary.

use std::ffi::CStr;
use std::net::Ipv4Addr;

/// One network interface that is up and carrying traffic, with its IPv4
/// address. Loopback is never listed: it is the address everyone knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub addr: Ipv4Addr,
}

/// Every up, running, non-loopback IPv4 interface, in kernel order. Empty
/// when the machine has no network, or when the lookup fails -- the banner
/// then just shows loopback, which is never wrong.
pub fn lan_interfaces() -> Vec<Interface> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates a linked list into `list`; we walk it
    // read-only and free it with freeifaddrs before returning.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return found;
    }
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: every node on the list is a valid ifaddrs until freed.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        let flags = entry.ifa_flags as libc::c_int;
        // Down or carrier-less interfaces (a docker bridge with nothing
        // attached, an unplugged port) are not reachable from anywhere.
        let usable = flags & libc::IFF_UP != 0
            && flags & libc::IFF_RUNNING != 0
            && flags & libc::IFF_LOOPBACK == 0;
        if !usable || entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: ifa_addr is non-null and points at a sockaddr whose family
        // field says what it really is; only AF_INET is read as sockaddr_in.
        let family = unsafe { (*entry.ifa_addr).sa_family } as libc::c_int;
        if family != libc::AF_INET {
            continue;
        }
        let sin = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
        let addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
        // SAFETY: ifa_name is a NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy().into_owned();
        found.push(Interface { name, addr });
    }
    // SAFETY: `list` came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// The lines of the startup banner that name the viewer's URLs, given the
/// host the server bound and the interfaces on the machine. Binding
/// `0.0.0.0` lists loopback and every LAN address; binding loopback says how
/// to open it wider; binding a specific address lists just that one.
pub fn banner_lines(host: &str, port: u16, lan: &[Interface]) -> Vec<String> {
    const LABEL: &str = "  Orbit live viewer:  ";
    let indent = " ".repeat(LABEL.len());
    match host {
        "0.0.0.0" | "::" | "[::]" => {
            let mut lines = vec![format!("{LABEL}http://127.0.0.1:{port}/")];
            for iface in lan {
                lines.push(format!(
                    "{indent}http://{}:{port}/   ({} -- from another machine on the network)",
                    iface.addr, iface.name
                ));
            }
            if lan.is_empty() {
                lines.push(format!("{indent}(no network interface is up, so only this machine can reach it)"));
            }
            lines
        }
        "127.0.0.1" | "localhost" | "::1" => vec![format!(
            "{LABEL}http://{host}:{port}/   (this machine only; --host 0.0.0.0 opens it to the network)"
        )],
        _ => vec![format!("{LABEL}http://{host}:{port}/")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, addr: [u8; 4]) -> Interface {
        Interface { name: name.to_string(), addr: Ipv4Addr::from(addr) }
    }

    #[test]
    fn binding_everywhere_lists_loopback_then_each_lan_address() {
        let lan = [iface("eth0", [192, 168, 1, 197]), iface("wlan0", [10, 0, 0, 5])];
        let lines = banner_lines("0.0.0.0", 44766, &lan);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("http://127.0.0.1:44766/"));
        assert!(lines[1].contains("http://192.168.1.197:44766/"), "{}", lines[1]);
        assert!(lines[1].contains("eth0"));
        assert!(lines[2].contains("http://10.0.0.5:44766/"), "{}", lines[2]);
        // The continuation lines line up under the first URL.
        let column = lines[0].find("http").unwrap();
        assert_eq!(lines[1].find("http"), Some(column));
    }

    #[test]
    fn binding_everywhere_with_no_network_says_so() {
        let lines = banner_lines("0.0.0.0", 1, &[]);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("only this machine"));
    }

    #[test]
    fn binding_loopback_tells_how_to_open_it_up() {
        let lan = [iface("eth0", [192, 168, 1, 197])];
        let lines = banner_lines("127.0.0.1", 44766, &lan);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("http://127.0.0.1:44766/"));
        assert!(lines[0].contains("--host 0.0.0.0"));
        assert!(!lines[0].contains("192.168"));
    }

    #[test]
    fn binding_one_address_lists_just_that() {
        let lan = [iface("eth0", [192, 168, 1, 197]), iface("eth1", [10, 0, 0, 5])];
        let lines = banner_lines("10.0.0.5", 80, &lan);
        assert_eq!(lines, vec!["  Orbit live viewer:  http://10.0.0.5:80/".to_string()]);
    }

    #[test]
    fn the_real_interface_list_never_contains_loopback() {
        for iface in lan_interfaces() {
            assert!(!iface.addr.is_loopback(), "{iface:?}");
            assert!(!iface.name.is_empty());
        }
    }
}
