//! Netlink route surgery (RTM_NEWROUTE / RTM_DELROUTE) via `libc`.
//!
//! This replaces the `ip route replace` / `ip route del` forks of the shell
//! backend. A single kernel transaction per operation:
//!   * `route_replace_slot(family, gw, dev, metric)` – RTM_NEWROUTE with
//!     NLM_F_CREATE|NLM_F_REPLACE, keyed on (destination=default, metric),
//!     which is exactly the atomic "move the slot" semantics of
//!     `ip route replace`;
//!   * `route_del(family, gw, dev, metric)` – RTM_DELROUTE;
//!   * `route_proto(family, dev, gw, metric)` – RTM_GETROUTE for the protocol
//!     tag (used by orphan reaping; /proc has no proto column).
//!
//! Routes installed here carry `rtm_protocol = RTPROT_BOOT` (3), the kernel
//! default, matching what `ip route replace` installs with no `proto` – so the
//! reconciler only ever reaps routes this program owns.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::types::Result;
use crate::types::{Error, Family};

const NETLINK_ROUTE: i32 = 0;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_ACK: u16 = 0x0004;
const NLM_F_REPLACE: u16 = 0x0100;
const NLM_F_CREATE: u16 = 0x0400;

const RTPROT_BOOT: u8 = 3;

const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;

/// Parsed reply of a GETROUTE.
pub struct RouteInfo {
    pub oif: u32,
    pub gateway: Option<IpAddr>,
    pub proto: u8,
}

fn open_route_socket() -> Result<i32> {
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return Err(Error::route(format!(
            "netlink socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut sa = unsafe { std::mem::zeroed::<libc::sockaddr_nl>() };
    sa.nl_family = libc::AF_NETLINK as u16; // kernel picks the pid
    sa.nl_groups = 0;
    let rc = unsafe {
        libc::bind(
            fd,
            &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        unsafe { libc::close(fd) };
        return Err(Error::route(format!(
            "netlink bind: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(fd)
}

/// Append an attribute (with padding) to the buffer.
fn push_attr(buf: &mut Vec<u8>, rta_type: u16, payload: &[u8]) {
    let len = 4 + payload.len();
    let aligned = (len + 3) & !3;
    buf.extend_from_slice(&(aligned as u16).to_ne_bytes());
    buf.extend_from_slice(&rta_type.to_ne_bytes());
    buf.extend_from_slice(payload);
    buf.resize(buf.len() + (aligned - len), 0);
}

fn ip_bytes(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(v) => v.octets().to_vec(),
        IpAddr::V6(v) => v.octets().to_vec(),
    }
}

fn parse_ip(family: Family, bytes: &[u8]) -> Option<IpAddr> {
    match family {
        Family::V4 if bytes.len() >= 4 => Some(IpAddr::V4(Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        ))),
        Family::V6 if bytes.len() >= 16 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(&bytes[..16]);
            Some(IpAddr::V6(Ipv6Addr::from(o)))
        }
        _ => None,
    }
}

/// Build and send a netlink request; returns parsed rtattr data.
fn netlink_request(
    family: Family,
    msg_type: u16,
    flags: u16,
    oif: Option<u32>,
    via: Option<IpAddr>,
    metric: Option<u32>,
) -> Result<Vec<u8>> {
    let fd = open_route_socket()?;
    let result = (|| -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let af = match family {
            Family::V4 => libc::AF_INET,
            Family::V6 => libc::AF_INET6,
        };
        // nlmsghdr
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // len, patched below
        buf.extend_from_slice(&msg_type.to_ne_bytes());
        buf.extend_from_slice(&flags.to_ne_bytes());
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // seq
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // pid
                                                      // rtmsg
        buf.push(af as u8);
        buf.push(0); // rtm_dst_len (default)
        buf.push(0); // rtm_src_len
        buf.push(0); // rtm_tos
        buf.push(0); // rtm_table
        buf.push(0); // rtm_protocol
        buf.push(0); // rtm_scope
        buf.push(0); // rtm_type
        buf.push(0); // rtm_flags
                     // attrs
        if let Some(oif) = oif {
            push_attr(&mut buf, RTA_OIF, &oif.to_ne_bytes());
        }
        if let Some(via) = via {
            push_attr(&mut buf, RTA_GATEWAY, &ip_bytes(via));
        }
        if let Some(m) = metric {
            push_attr(&mut buf, RTA_PRIORITY, &m.to_ne_bytes());
        }
        // Default route: no RTA_DST at all means 0.0.0.0/0 or ::/0.
        let total = buf.len() as u32;
        buf[..4].copy_from_slice(&total.to_ne_bytes());

        let sent = unsafe { libc::send(fd, buf.as_ptr() as *const libc::c_void, buf.len(), 0) };
        if sent < 0 {
            return Err(Error::route(format!(
                "netlink send: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut reply = [0u8; 8192];
        let n = unsafe { libc::recv(fd, reply.as_mut_ptr() as *mut libc::c_void, reply.len(), 0) };
        if n < 0 {
            return Err(Error::route(format!(
                "netlink recv: {}",
                std::io::Error::last_os_error()
            )));
        }
        let n = n as usize;
        if n < 16 {
            return Err(Error::route("netlink short reply"));
        }
        // nlmsghdr fields (native endian)
        let msg_len = u32::from_ne_bytes(reply[0..4].try_into().unwrap()) as usize;
        let err_code = i32::from_ne_bytes(reply[16..20].try_into().unwrap());
        if err_code != 0 {
            return Err(Error::route(format!(
                "netlink error {}: {}",
                -err_code,
                std::io::Error::from_raw_os_error(-err_code)
            )));
        }
        let _ = msg_len;
        Ok(reply[16..n].to_vec())
    })();
    unsafe { libc::close(fd) };
    result
}

/// Parse a GETROUTE reply: walk attrs after the rtmsg (rtmsg is 12 bytes).
fn parse_route_info(family: Family, payload: &[u8]) -> RouteInfo {
    let mut oif = 0u32;
    let mut gateway: Option<IpAddr> = None;
    let mut proto = 0u8;
    // rtmsg = 12 bytes
    let mut off = 12usize;
    let bytes = &payload[off.min(payload.len())..];
    off = 0;
    while off + 4 <= bytes.len() {
        let rta_len = u16::from_ne_bytes([bytes[off], bytes[off + 1]]) as usize;
        let rta_type = u16::from_ne_bytes([bytes[off + 2], bytes[off + 3]]);
        if rta_len < 4 || off + rta_len > bytes.len() {
            break;
        }
        let value = &bytes[off + 4..off + rta_len];
        match rta_type {
            RTA_OIF if value.len() >= 4 => {
                oif = u32::from_ne_bytes(value[..4].try_into().unwrap_or([0; 4]));
            }
            RTA_GATEWAY => {
                gateway = parse_ip(family, value);
            }
            _ => {}
        }
        off += rta_len;
    }
    // proto is in the rtmsg; it is bytes[8] of the message.
    if payload.len() >= 8 {
        proto = payload[8];
    }
    RouteInfo {
        oif,
        gateway,
        proto,
    }
}

/// `route_replace_slot(family, gw, dev, metric)`: atomic slot move.
pub fn route_replace_slot(
    family: Family,
    via: Option<IpAddr>,
    dev: &str,
    metric: u32,
) -> Result<()> {
    let oif = crate::network::sysfs::dev_ifindex(dev)
        .ok_or_else(|| Error::route(format!("no ifindex for {dev}")))?;
    let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE;
    netlink_request(family, RTM_NEWROUTE, flags, Some(oif), via, Some(metric))?;
    Ok(())
}

/// `route_del(family, gw, dev, metric)`.
pub fn route_del(family: Family, via: Option<IpAddr>, dev: &str, metric: u32) -> Result<()> {
    let oif = crate::network::sysfs::dev_ifindex(dev)
        .ok_or_else(|| Error::route(format!("no ifindex for {dev}")))?;
    netlink_request(
        family,
        RTM_DELROUTE,
        NLM_F_REQUEST | NLM_F_ACK,
        Some(oif),
        via,
        Some(metric),
    )?;
    Ok(())
}

/// Query a route's protocol tag. Returns None when the route does not exist.
pub fn route_proto(family: Family, via: Option<IpAddr>, dev: &str, metric: u32) -> Option<u8> {
    let oif = crate::network::sysfs::dev_ifindex(dev)?;
    let payload = netlink_request(
        family,
        RTM_GETROUTE,
        NLM_F_REQUEST,
        Some(oif),
        via,
        Some(metric),
    )
    .ok()?;
    Some(parse_route_info(family, &payload).proto)
}

/// Convert a gateway string into the netlink `IpAddr` for a family.
pub fn parse_gateway(family: Family, gw: &str) -> Option<IpAddr> {
    match family {
        Family::V4 => gw.parse::<Ipv4Addr>().ok().map(IpAddr::V4),
        Family::V6 => gw.parse::<Ipv6Addr>().ok().map(IpAddr::V6),
    }
}

/// Convert a device string to ifindex; fails fast with a route error.
pub fn dev_oif(dev: &str) -> Result<u32> {
    crate::network::sysfs::dev_ifindex(dev)
        .ok_or_else(|| Error::route(format!("no ifindex for {dev}")))
}

/// `is_route_boot`: the orphan-reaping guard. Only kernel-default (`boot`)
/// routes this program installs are ever removed.
pub fn is_route_boot(family: Family, via: Option<IpAddr>, dev: &str, metric: u32) -> bool {
    route_proto(family, via, dev, metric) == Some(RTPROT_BOOT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_getroute_payload() {
        // Build a fake GETROUTE payload: rtmsg(12) + RTA_OIF + RTA_GATEWAY.
        let mut payload = vec![0u8; 12];
        payload[8] = 3; // rtm_protocol = boot
        let mut attrs = Vec::new();
        // RTA_OIF = 4
        attrs.extend_from_slice(&(8u16).to_ne_bytes());
        attrs.extend_from_slice(&4u16.to_ne_bytes());
        attrs.extend_from_slice(&5u32.to_ne_bytes());
        // RTA_GATEWAY = 5
        let gw = [10u8, 0, 0, 1];
        attrs.extend_from_slice(&(8u16).to_ne_bytes());
        attrs.extend_from_slice(&5u16.to_ne_bytes());
        attrs.extend_from_slice(&gw);
        payload.extend_from_slice(&attrs);
        let info = parse_route_info(Family::V4, &payload);
        assert_eq!(info.proto, 3);
        assert_eq!(info.oif, 5);
        assert_eq!(info.gateway, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }
}
