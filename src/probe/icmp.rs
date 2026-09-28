//! ICMP probes bound to the probed interface (`SO_BINDTODEVICE`).
//!
//! Each family tries the kernel ping socket first (`SOCK_DGRAM` +
//! `IPPROTO_ICMP[V6]`) and falls back to a raw socket when ping sockets are
//! unavailable (`ping_group_range` gating). On a ping socket the kernel owns
//! the ident: it rewrites ours on send, demuxes echo replies by ident and
//! hands us the bare ICMP header, so a type check alone is sufficient. On a
//! raw socket *every* ICMP packet of that family enters the receive queue, so
//! the loop strips the IPv4 header (IHL) where present and matches type+ident
//! until our reply arrives or the deadline expires. `SO_RCVTIMEO` is re-armed
//! against a hard deadline each round, so a silent uplink costs exactly one
//! probe timeout and never blocks the switch.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use crate::types::{Error, Family, Result};

/// Bind a socket to a device. Fails when the device is gone.
fn bind_device(fd: i32, dev: &str) -> Result<()> {
    let cdev = std::ffi::CString::new(dev).map_err(|_| Error::probe("dev name NUL"))?;
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            cdev.as_ptr() as *const libc::c_void,
            cdev.as_bytes().len() as libc::socklen_t,
        )
    };
    if rc < 0 {
        return Err(Error::probe(format!(
            "SO_BINDTODEVICE({dev}): {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

/// Re-arm `SO_RCVTIMEO` for the remaining wait. Zero remaining time is
/// handled by the caller (`recv_echo_reply` returns before calling this).
fn set_recv_timeout_us(fd: i32, us: i64) {
    let tv = libc::timeval {
        tv_sec: (us / 1_000_000) as _,
        tv_usec: (us % 1_000_000) as _,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// Raw ICMPv6 sockets must declare the checksum offset (RFC 3542); Linux then
/// recomputes the checksum in both directions. Offset 2 = the ICMPv6 header
/// checksum field.
fn set_v6_checksum(fd: i32) {
    let offset: libc::c_int = 2;
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_CHECKSUM,
            &offset as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

fn deadline(timeout: u32) -> Instant {
    Instant::now() + Duration::from_secs(timeout.max(1) as u64)
}

/// Receive loop shared by every probe path.
///
/// `strip_v4_ip` is only set for the raw-IPv4 path: a raw IPv4 socket hands
/// us the IP header first and the ICMP header starts at IHL*4. Raw IPv6
/// sockets (and both ping sockets) start directly at the ICMP header - the
/// old code skipped 40 bytes on IPv6 and read past the packet, which is why
/// every v6 probe read `ok=0` even while replies were on the wire.
///
/// `expect_id` is only set for raw paths: on a ping socket the kernel demuxes
/// by ident, so any echo reply in the queue is ours. On a raw socket the
/// ident check discards traffic from concurrent probers and other daemons.
fn recv_echo_reply(
    fd: i32,
    reply_type: u8,
    expect_id: bool,
    id: u16,
    strip_v4_ip: bool,
    deadline: Instant,
) -> bool {
    let mut buf = [0u8; 2048];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        set_recv_timeout_us(fd, remaining.as_micros().min(i64::MAX as u128) as i64);
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            return false; // timeout (EAGAIN) or socket error
        }
        let n = n as usize;
        let start = if strip_v4_ip {
            if n < 20 {
                continue;
            }
            (((buf[0] & 0x0f) as usize) * 4).min(n)
        } else {
            0
        };
        let icmp = &buf[start..n];
        if icmp.len() < 8 {
            continue;
        }
        if icmp[0] != reply_type {
            continue; // RA / NS / neighbour traffic on raw sockets
        }
        if !expect_id || u16::from_ne_bytes([icmp[4], icmp[5]]) == id {
            return true;
        }
    }
}

// ---------------------------------------------------------------------------
// Packet builders (checksum included; ping sockets let the kernel redo it)
// ---------------------------------------------------------------------------

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;
const ICMPV6_ECHO_REQUEST: u8 = 128;
const ICMPV6_ECHO_REPLY: u8 = 129;

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += data[i] as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_v4_pkt(id: u16, seq: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + 16);
    pkt.push(ICMP_ECHO_REQUEST);
    pkt.push(0);
    pkt.push(0);
    pkt.push(0); // checksum, patched below
    pkt.extend_from_slice(&id.to_ne_bytes());
    pkt.extend_from_slice(&seq.to_ne_bytes());
    pkt.extend_from_slice(b"h5000m");
    let sum = checksum(&pkt);
    pkt[2] = (sum & 0xff) as u8;
    pkt[3] = (sum >> 8) as u8;
    pkt
}

fn send_v4(fd: i32, addr4: Ipv4Addr, pkt: &[u8]) -> Result<()> {
    // `s_addr` wants the address bytes in network order *in memory*. octets()
    // already is that byte sequence, so from_ne_bytes alone is correct; the
    // previous `.to_be()` double-swap turned 223.5.5.5 into 5.5.5.223 on
    // little-endian targets.
    let sa = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes(addr4.octets()),
        },
        sin_zero: [0; 8],
    };
    let rc = unsafe {
        libc::sendto(
            fd,
            pkt.as_ptr() as *const libc::c_void,
            pkt.len(),
            0,
            &sa as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        return Err(Error::probe(format!(
            "icmp sendto: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn build_v6_pkt(id: u16, seq: u16, src6: Ipv6Addr, dst6: Ipv6Addr) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + 16);
    pkt.push(ICMPV6_ECHO_REQUEST);
    pkt.push(0);
    pkt.push(0);
    pkt.push(0);
    pkt.extend_from_slice(&id.to_ne_bytes());
    pkt.extend_from_slice(&seq.to_ne_bytes());
    pkt.extend_from_slice(b"h5000m6");

    // Pseudo-header checksum (RFC 4443). Linux recomputes it for both ping
    // and raw ICMPv6 sockets, so this is belt-and-braces only.
    let mut ph = Vec::with_capacity(40 + pkt.len());
    ph.extend_from_slice(&src6.octets());
    ph.extend_from_slice(&dst6.octets());
    ph.extend_from_slice(&((pkt.len() as u32).to_be_bytes()));
    ph.extend_from_slice(&[0, 0, 0, libc::IPPROTO_ICMPV6 as u8]);
    ph.extend_from_slice(&pkt);
    let sum = checksum(&ph);
    pkt[2] = (sum & 0xff) as u8;
    pkt[3] = (sum >> 8) as u8;
    pkt
}

fn send_v6(fd: i32, addr6: Ipv6Addr, pkt: &[u8]) -> Result<()> {
    let sa = libc::sockaddr_in6 {
        sin6_family: libc::AF_INET6 as u16,
        sin6_port: 0,
        sin6_flowinfo: 0,
        sin6_addr: libc::in6_addr {
            s6_addr: addr6.octets(),
        },
        sin6_scope_id: 0,
    };
    let rc = unsafe {
        libc::sendto(
            fd,
            pkt.as_ptr() as *const libc::c_void,
            pkt.len(),
            0,
            &sa as *const libc::sockaddr_in6 as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        return Err(Error::probe(format!(
            "icmpv6 sendto: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Probe entry points: ping socket first, raw fallback
// ---------------------------------------------------------------------------

fn ping_v4(dev: &str, target: IpAddr, timeout: u32, seq: u16) -> Result<bool> {
    let IpAddr::V4(addr4) = target else {
        return Err(Error::probe("v4 probe with v6 target"));
    };
    let id = (std::process::id() & 0xffff) as u16;
    let dl = deadline(timeout);

    let dgram = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP) };
    if dgram >= 0 {
        let out = (|| -> Result<bool> {
            bind_device(dgram, dev)?;
            let pkt = build_v4_pkt(id, seq);
            send_v4(dgram, addr4, &pkt)?;
            Ok(recv_echo_reply(
                dgram,
                ICMP_ECHO_REPLY,
                false,
                id,
                false,
                dl,
            ))
        })();
        unsafe { libc::close(dgram) };
        if let Ok(v) = out {
            return Ok(v);
        }
    }

    // Fallback: raw ICMP (root). The ident check discards replies belonging
    // to concurrent probers sharing the same device socket queues.
    let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_RAW, libc::IPPROTO_ICMP) };
    if raw < 0 {
        return Err(Error::probe(format!(
            "icmp socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let out = (|| -> Result<bool> {
        bind_device(raw, dev)?;
        let pkt = build_v4_pkt(id, seq);
        send_v4(raw, addr4, &pkt)?;
        Ok(recv_echo_reply(raw, ICMP_ECHO_REPLY, true, id, true, dl))
    })();
    unsafe { libc::close(raw) };
    out
}

/// Source address: a non-link-local address on the probed device.
fn v6_source(dev: &str) -> Result<Ipv6Addr> {
    let pick = |non_ll: bool| {
        crate::network::sysfs::ipv6_addrs()
            .into_iter()
            .filter(|(d, _, scope)| d == dev && (!non_ll || *scope != 0x20))
            .map(|(_, a, _)| a)
            .next()
    };
    let src_str = pick(true).or_else(|| pick(false));
    let Some(src_str) = src_str else {
        return Err(Error::probe(format!("no IPv6 source on {dev}")));
    };
    src_str
        .parse::<Ipv6Addr>()
        .map_err(|_| Error::probe("bad v6 source"))
}

fn ping_v6(dev: &str, target: IpAddr, timeout: u32, seq: u16) -> Result<bool> {
    let IpAddr::V6(addr6) = target else {
        return Err(Error::probe("v6 probe with v4 target"));
    };
    let src6 = v6_source(dev)?;
    let id = (std::process::id() & 0xffff) as u16;
    let dl = deadline(timeout);

    let dgram = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, libc::IPPROTO_ICMPV6) };
    if dgram >= 0 {
        let out = (|| -> Result<bool> {
            bind_device(dgram, dev)?;
            let pkt = build_v6_pkt(id, seq, src6, addr6);
            send_v6(dgram, addr6, &pkt)?;
            Ok(recv_echo_reply(
                dgram,
                ICMPV6_ECHO_REPLY,
                false,
                id,
                false,
                dl,
            ))
        })();
        unsafe { libc::close(dgram) };
        if let Ok(v) = out {
            return Ok(v);
        }
    }

    let raw = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_RAW, libc::IPPROTO_ICMPV6) };
    if raw < 0 {
        return Err(Error::probe(format!(
            "icmpv6 socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    set_v6_checksum(raw);
    let out = (|| -> Result<bool> {
        bind_device(raw, dev)?;
        let pkt = build_v6_pkt(id, seq, src6, addr6);
        send_v6(raw, addr6, &pkt)?;
        Ok(recv_echo_reply(raw, ICMPV6_ECHO_REPLY, true, id, false, dl))
    })();
    unsafe { libc::close(raw) };
    out
}

/// One bounded ICMP probe: `ping_one <family> <dev> <timeout> <target>`.
pub fn ping_one(family: Family, dev: &str, timeout: u32, target: &str) -> Result<bool> {
    let ip: IpAddr = target
        .parse()
        .map_err(|_| Error::probe(format!("bad target {target}")))?;
    let seq = (std::process::id() & 0xffff) as u16;
    match family {
        Family::V4 => ping_v4(dev, ip, timeout, seq),
        Family::V6 => ping_v6(dev, ip, timeout, seq),
    }
}

/// Fire every (target, device) probe in parallel; any success answers the
/// round. Exactly the semantics of the shell `ping_targets_once`.
pub fn ping_targets_once(
    family: Family,
    devs: &[String],
    timeout: u32,
    targets: &[String],
) -> bool {
    if devs.is_empty() {
        return false;
    }
    let mut handles = Vec::new();
    for target in targets {
        // Family filter: a v6 target only probes v6 rounds, and vice versa.
        let fam_ok = if family == Family::V6 {
            target.contains(':')
        } else {
            target.parse::<std::net::Ipv4Addr>().is_ok()
        };
        if !fam_ok {
            continue;
        }
        for dev in devs {
            let dev = dev.clone();
            let target = target.clone();
            handles.push(std::thread::spawn(move || {
                ping_one(family, &dev, timeout, &target).unwrap_or(false)
            }));
        }
    }
    if handles.is_empty() {
        return false;
    }
    for h in handles {
        if h.join().unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Gateway reachability (advisory, never a sole verdict).
pub fn gateway_reachable(family: Family, devs: &[String], gw: &str, timeout: u32) -> bool {
    if gw.is_empty() {
        return false;
    }
    for dev in devs {
        if ping_one(family, dev, timeout, gw).unwrap_or(false) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_basic() {
        // Checksum of [0x00, 0x00, 0x00, 0x01] should be 0xfffe (one's
        // complement of 1).
        let c = checksum(&[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(c, 0xfffe);
        let c = checksum(&[0x01, 0x01]); // 0x0101
        assert_eq!(c, 0xfefe);
    }

    #[test]
    fn v4_pkt_layout() {
        let pkt = build_v4_pkt(0x1234, 7);
        assert_eq!(pkt.len(), 8 + 6);
        assert_eq!(pkt[0], ICMP_ECHO_REQUEST);
        assert_eq!(u16::from_ne_bytes([pkt[4], pkt[5]]), 0x1234);
        // Checksum field must verify against the rest of the packet.
        let mut copy = pkt.clone();
        copy[2] = 0;
        copy[3] = 0;
        assert_eq!(checksum(&copy), u16::from_be_bytes([pkt[2], pkt[3]]));
    }

    #[test]
    fn v6_pkt_layout() {
        let src = "2409:8d5b::1".parse::<Ipv6Addr>().unwrap();
        let dst = "2400:3200::1".parse::<Ipv6Addr>().unwrap();
        let pkt = build_v6_pkt(0x4321, 9, src, dst);
        assert_eq!(pkt.len(), 8 + 7);
        assert_eq!(pkt[0], ICMPV6_ECHO_REQUEST);
        assert_eq!(u16::from_ne_bytes([pkt[4], pkt[5]]), 0x4321);
    }
}
