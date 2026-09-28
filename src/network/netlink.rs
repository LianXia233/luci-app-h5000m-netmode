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
//! default, matching what `ip route replace` installs with no `proto` - so the
//! reconciler only ever reaps routes this program owns.
//!
//! Layout notes (this module once got two of them wrong, and both fail the same
//! silent way: the kernel answers -EINVAL and the route never moves):
//!   * `struct rtmsg` is **12** bytes, not 9 - `rtm_flags` is a `u32` and the
//!     attribute array must start at `nlmsghdr + 12`, 4-byte aligned. Writing
//!     only 9 bytes shifts every rtattr by 3 and the kernel walks garbage.
//!   * `rtm_type` must be `RTN_UNICAST` (1); `RTN_UNSPEC` (0) is rejected.
//!   * only an `NLMSG_ERROR` reply carries a status word in its payload; a
//!     successful `RTM_GETROUTE` answer is a `RTM_NEWROUTE` message whose first
//!     payload bytes are the rtmsg itself.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::types::Result;
use crate::types::{Error, Family};

const NETLINK_ROUTE: i32 = 0;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_ACK: u16 = 0x0004;
const NLM_F_REPLACE: u16 = 0x0100;
const NLM_F_CREATE: u16 = 0x0400;
const NLM_F_DUMP: u16 = 0x0300;
const NLMSG_DONE: u16 = 3;

/// Receive timeout for a route dump: a wedged kernel must not be able to park a
/// switch worker forever.
const DUMP_TIMEOUT_S: i64 = 5;

/// `sizeof(struct nlmsghdr)`.
const NLMSG_HDRLEN: usize = 16;
/// `sizeof(struct rtmsg)` - see the module docs: 12, not 9.
const RTMSG_LEN: usize = 12;

/// `RT_TABLE_MAIN`: the table `ip route replace` writes to by default.
const RT_TABLE_MAIN: u8 = 254;
/// `RTN_UNICAST`: the `rtm_type` of an ordinary next-hop route.
const RTN_UNICAST: u8 = 1;
const RTPROT_BOOT: u8 = 3;

const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;

/// Parsed reply of a GETROUTE.
pub struct RouteInfo {
    pub oif: u32,
    pub gateway: Option<IpAddr>,
    pub proto: u8,
    pub metric: u32,
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

/// Build the wire image of one route request.
///
/// Deliberately free of I/O so a unit test can assert its layout: the two
/// layout bugs this module shipped with (a 9-byte rtmsg, `rtm_type = 0`) both
/// produce a plausible-looking buffer and a silent -EINVAL from the kernel.
fn build_request(
    family: Family,
    msg_type: u16,
    flags: u16,
    oif: Option<u32>,
    via: Option<IpAddr>,
    metric: Option<u32>,
) -> Vec<u8> {
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
    // struct rtmsg - 12 bytes, see the module docs
    buf.push(af as u8);
    buf.push(0); // rtm_dst_len: 0 = default route
    buf.push(0); // rtm_src_len
    buf.push(0); // rtm_tos
    buf.push(RT_TABLE_MAIN); // rtm_table
    buf.push(RTPROT_BOOT); // rtm_protocol
    buf.push(0); // rtm_scope: the kernel fills it in
    buf.push(RTN_UNICAST); // rtm_type
    buf.extend_from_slice(&0u32.to_ne_bytes()); // rtm_flags
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
    buf
}

/// Send one request and return the payload that follows the netlink header.
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
        let buf = build_request(family, msg_type, flags, oif, via, metric);
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
        if n < NLMSG_HDRLEN {
            return Err(Error::route("netlink short reply"));
        }
        // Only an NLMSG_ERROR carries a status word; anything else (a
        // RTM_GETROUTE answer, say) starts with the rtmsg itself.
        let replied = u16::from_ne_bytes([reply[4], reply[5]]);
        if replied == NLMSG_ERROR {
            if n < NLMSG_HDRLEN + 4 {
                return Err(Error::route("netlink short error reply"));
            }
            let err_code = i32::from_ne_bytes(reply[16..20].try_into().unwrap());
            if err_code != 0 {
                return Err(Error::route(format!(
                    "netlink error {}: {}",
                    -err_code,
                    std::io::Error::from_raw_os_error(-err_code)
                )));
            }
        }
        Ok(reply[NLMSG_HDRLEN..n].to_vec())
    })();
    unsafe { libc::close(fd) };
    result
}

/// Parse a GETROUTE reply: `payload` is the rtmsg followed by its attributes.
fn parse_route_info(family: Family, payload: &[u8]) -> RouteInfo {
    let mut oif = 0u32;
    let mut gateway: Option<IpAddr> = None;
    let mut metric = 0u32;
    // struct rtmsg: family dst_len src_len tos table protocol scope type flags
    // offsets:        0      1       2     3    4      5       6     7   8..12
    let proto = payload.get(5).copied().unwrap_or(0);
    let bytes = payload.get(RTMSG_LEN..).unwrap_or_default();
    let mut off = 0usize;
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
            RTA_PRIORITY if value.len() >= 4 => {
                metric = u32::from_ne_bytes(value[..4].try_into().unwrap_or([0; 4]));
            }
            _ => {}
        }
        off += rta_len;
    }
    RouteInfo {
        oif,
        gateway,
        proto,
        metric,
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
///
/// The device is optional on purpose: an orphan route is by definition one
/// whose device has disappeared, so there is no ifindex left to resolve. Such
/// a delete is only issued when the gateway and the metric identify the route
/// unambiguously (see `reap_orphan_routes`).
pub fn route_del(family: Family, via: Option<IpAddr>, dev: &str, metric: u32) -> Result<()> {
    let oif = if dev.is_empty() {
        None
    } else {
        crate::network::sysfs::dev_ifindex(dev)
    };
    let flags = NLM_F_REQUEST | NLM_F_ACK;
    netlink_request(family, RTM_DELROUTE, flags, oif, via, Some(metric))?;
    Ok(())
}

/// Dump every route of one family and return the payload of each entry.
///
/// A plain `RTM_GETROUTE` is a *lookup*: the kernel answers with the single best
/// route towards a destination, which need not be the entry identified by
/// (device, gateway, metric) - it happily answered with a local route during
/// testing. Only a dump returns the FIB entries themselves, and
/// `is_route_boot` needs the protocol tag of one specific entry.
fn dump_routes(family: Family) -> Result<Vec<Vec<u8>>> {
    let fd = open_route_socket()?;
    let result = (|| -> Result<Vec<Vec<u8>>> {
        let tv = libc::timeval {
            tv_sec: DUMP_TIMEOUT_S,
            tv_usec: 0,
        };
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        let af: u8 = match family {
            Family::V4 => libc::AF_INET as u8,
            Family::V6 => libc::AF_INET6 as u8,
        };
        // nlmsghdr + struct rtgenmsg (a single `family` byte).
        let mut buf = Vec::new();
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // len, patched below
        buf.extend_from_slice(&RTM_GETROUTE.to_ne_bytes());
        buf.extend_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // seq
        buf.extend_from_slice(&(0u32).to_ne_bytes()); // pid
        buf.push(af);
        let total = buf.len() as u32;
        buf[..4].copy_from_slice(&total.to_ne_bytes());
        let sent = unsafe { libc::send(fd, buf.as_ptr() as *const libc::c_void, buf.len(), 0) };
        if sent < 0 {
            return Err(Error::route(format!(
                "netlink dump send: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut raw: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 65536];
        loop {
            let n = unsafe {
                libc::recv(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len(), 0)
            };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                // EAGAIN == EWOULDBLOCK on Linux; matching both would be an
                // unreachable pattern, which `-D warnings` turns into an error.
                let again = e.raw_os_error() == Some(libc::EAGAIN);
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if again {
                    break; // timed out: parse whatever arrived
                }
                return Err(Error::route(format!("netlink dump recv: {e}")));
            }
            let n = n as usize;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            if raw_contains_done(&raw) {
                break;
            }
        }
        Ok(collect_dump_payloads(&raw)?)
    })();
    unsafe { libc::close(fd) };
    result
}

/// Has the kernel already closed the dump with NLMSG_DONE?
fn raw_contains_done(raw: &[u8]) -> bool {
    let mut off = 0usize;
    while off + NLMSG_HDRLEN <= raw.len() {
        let len = u32::from_ne_bytes(raw[off..off + 4].try_into().unwrap_or([0; 4])) as usize;
        let ty = u16::from_ne_bytes(raw[off + 4..off + 6].try_into().unwrap_or([0; 2]));
        if len < NLMSG_HDRLEN {
            return false;
        }
        if ty == NLMSG_DONE {
            return true;
        }
        off += (len + 3) & !3;
    }
    false
}

/// Split a dump into per-entry payloads (RTM_NEWROUTE messages only).
fn collect_dump_payloads(raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut off = 0usize;
    while off + NLMSG_HDRLEN <= raw.len() {
        let len = u32::from_ne_bytes(raw[off..off + 4].try_into().unwrap_or([0; 4])) as usize;
        let ty = u16::from_ne_bytes(raw[off + 4..off + 6].try_into().unwrap_or([0; 2]));
        if len < NLMSG_HDRLEN || off + len > raw.len() {
            break;
        }
        match ty {
            NLMSG_DONE => break,
            NLMSG_ERROR => {
                if len >= NLMSG_HDRLEN + 4 {
                    let err_code =
                        i32::from_ne_bytes(raw[off + 16..off + 20].try_into().unwrap_or([0; 4]));
                    if err_code != 0 {
                        return Err(Error::route(format!("netlink dump error {}", -err_code)));
                    }
                }
            }
            RTM_NEWROUTE => out.push(raw[off + NLMSG_HDRLEN..off + len].to_vec()),
            _ => {}
        }
        off += (len + 3) & !3;
    }
    Ok(out)
}

/// Query the protocol tag of one specific default route. Returns None when no
/// dumped entry matches (device, gateway, metric).
pub fn route_proto(family: Family, via: Option<IpAddr>, dev: &str, metric: u32) -> Option<u8> {
    let oif = if dev.is_empty() {
        None
    } else {
        crate::network::sysfs::dev_ifindex(dev)
    };
    let entries = dump_routes(family).ok()?;
    entries
        .iter()
        // rtm_dst_len == 0: a default route, the only kind this slot owns.
        .filter(|p| p.len() > 1 && p[1] == 0)
        .map(|p| parse_route_info(family, p))
        .find(|r| {
            r.metric == metric
                && match oif {
                    Some(o) => r.oif == o,
                    None => true,
                }
                && match via {
                    Some(v) => r.gateway == Some(v),
                    None => true,
                }
        })
        .map(|r| r.proto)
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

    /// Read a `u16` out of `buf` at `off` (native endian, like the wire).
    fn u16at(buf: &[u8], off: usize) -> u16 {
        u16::from_ne_bytes(buf[off..off + 2].try_into().unwrap())
    }

    /// Read a `u32` out of `buf` at `off`.
    fn u32at(buf: &[u8], off: usize) -> u32 {
        u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap())
    }

    #[test]
    fn request_is_a_well_formed_rtmsg() {
        let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE;
        let buf = build_request(Family::V4, RTM_NEWROUTE, flags, Some(7), None, Some(20));
        // nlmsghdr: the length covers the whole message.
        assert_eq!(u32at(&buf, 0) as usize, buf.len());
        assert_eq!(u16at(&buf, 4), RTM_NEWROUTE);
        // struct rtmsg starts right after the header.
        assert_eq!(buf[NLMSG_HDRLEN], libc::AF_INET as u8);
        assert_eq!(buf[NLMSG_HDRLEN + 1], 0); // dst_len 0 = default route
        assert_eq!(buf[NLMSG_HDRLEN + 4], RT_TABLE_MAIN);
        assert_eq!(buf[NLMSG_HDRLEN + 5], RTPROT_BOOT);
        assert_eq!(buf[NLMSG_HDRLEN + 7], RTN_UNICAST);
        // ... and is 12 bytes long, so the first attribute is aligned at 28.
        let off = NLMSG_HDRLEN + RTMSG_LEN;
        assert_eq!(off, 28);
        assert_eq!(u16at(&buf, off + 2), RTA_OIF);
        assert_eq!(u32at(&buf, off + 4), 7);
    }

    #[test]
    fn ipv6_request_carries_a_16_byte_gateway() {
        let gw = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let flags = NLM_F_REQUEST | NLM_F_ACK;
        let metric = Some(1024u32);
        let buf = build_request(Family::V6, RTM_NEWROUTE, flags, None, Some(gw), metric);
        assert_eq!(buf[NLMSG_HDRLEN], libc::AF_INET6 as u8);
        let off = NLMSG_HDRLEN + RTMSG_LEN;
        assert_eq!(u16at(&buf, off + 2), RTA_GATEWAY);
        // 4 bytes of attribute header + a 16-byte address.
        assert_eq!(u16at(&buf, off), 20);
    }

    #[test]
    fn parse_getroute_payload() {
        // A real RTM_GETROUTE answer: rtmsg(12) + RTA_PRIORITY + RTA_OIF +
        // RTA_GATEWAY, with rtm_protocol = boot at rtmsg offset 5.
        let mut payload = vec![0u8; 12];
        payload[4] = RT_TABLE_MAIN;
        payload[5] = RTPROT_BOOT;
        payload[7] = RTN_UNICAST;
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&(8u16).to_ne_bytes());
        attrs.extend_from_slice(&6u16.to_ne_bytes());
        attrs.extend_from_slice(&9999u32.to_ne_bytes());
        attrs.extend_from_slice(&(8u16).to_ne_bytes());
        attrs.extend_from_slice(&4u16.to_ne_bytes());
        attrs.extend_from_slice(&5u32.to_ne_bytes());
        attrs.extend_from_slice(&(8u16).to_ne_bytes());
        attrs.extend_from_slice(&5u16.to_ne_bytes());
        attrs.extend_from_slice(&[10u8, 0, 0, 1]);
        payload.extend_from_slice(&attrs);
        let info = parse_route_info(Family::V4, &payload);
        assert_eq!(info.proto, RTPROT_BOOT);
        assert_eq!(info.oif, 5);
        assert_eq!(info.gateway, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn short_payload_is_not_a_panic() {
        let info = parse_route_info(Family::V6, &[2, 0, 0, 0]);
        assert_eq!(info.proto, 0);
        assert_eq!(info.oif, 0);
        assert!(info.gateway.is_none());
    }
}
