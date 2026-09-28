//! ICMP probes bound to the probed interface (`SO_BINDTODEVICE`).
//!
//! IPv4 uses the kernel ping socket (`SOCK_DGRAM` + `IPPROTO_ICMP`), IPv6 a raw
//! `IPPROTO_ICMPV6` socket with a hand-built echo request. Both honour
//! `SO_RCVTIMEO` so a silent uplink costs exactly one timeout and never blocks
//! the switch. This replaces the `ping -I <dev>` forks of the shell backend
//! (one probe = one syscall path, no child processes at all).

use std::net::IpAddr;

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

fn set_recv_timeout(fd: i32, secs: u32) {
    let tv = libc::timeval {
        tv_sec: secs as libc::time_t,
        tv_usec: 0,
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

// ---------------------------------------------------------------------------
// IPv4 ping socket
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

fn ping_v4(dev: &str, target: IpAddr, timeout: u32, seq: u16) -> Result<bool> {
    let IpAddr::V4(addr4) = target else {
        return Err(Error::probe("v4 probe with v6 target"));
    };
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP) };
    if fd < 0 {
        return Err(Error::probe(format!(
            "icmp socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let out = (|| -> Result<bool> {
        bind_device(fd, dev)?;
        set_recv_timeout(fd, timeout.max(1));
        let id = (std::process::id() & 0xffff) as u16;
        let mut pkt = Vec::with_capacity(8 + 16);
        pkt.push(ICMP_ECHO_REQUEST);
        pkt.push(0);
        pkt.push(0);
        pkt.push(0); // checksum, patched
        pkt.extend_from_slice(&id.to_ne_bytes());
        pkt.extend_from_slice(&seq.to_ne_bytes());
        pkt.extend_from_slice(b"h5000m");
        let sum = checksum(&pkt);
        pkt[2] = (sum & 0xff) as u8;
        pkt[3] = (sum >> 8) as u8;

        let sa = libc::sockaddr_in {
            sin_family: libc::AF_INET as u16,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(addr4.octets()).to_be(),
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
            return Ok(false); // send failure (e.g. no route yet) = not reachable
        }
        let mut buf = [0u8; 512];
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            return Ok(false); // timeout
        }
        if n < 8 {
            return Ok(false);
        }
        // Echo reply: type 0, id match.
        Ok(buf[0] == ICMP_ECHO_REPLY && u16::from_ne_bytes([buf[4], buf[5]]) == id)
    })();
    unsafe { libc::close(fd) };
    out
}

// ---------------------------------------------------------------------------
// IPv6 raw echo
// ---------------------------------------------------------------------------

fn ping_v6(dev: &str, target: IpAddr, timeout: u32, seq: u16) -> Result<bool> {
    let IpAddr::V6(addr6) = target else {
        return Err(Error::probe("v6 probe with v4 target"));
    };
    // Source address: a non-link-local address on the probed device.
    let src = crate::network::sysfs::ipv6_addrs()
        .into_iter()
        .filter(|(d, _, scope)| d == dev && *scope != 0x20)
        .map(|(_, a, _)| a)
        .next()
        .or_else(|| {
            crate::network::sysfs::ipv6_addrs()
                .into_iter()
                .filter(|(d, _, _)| d == dev)
                .map(|(_, a, _)| a)
                .next()
        });
    let Some(src_str) = src else {
        return Err(Error::probe(format!("no IPv6 source on {dev}")));
    };
    let Ok(src6) = src_str.parse::<std::net::Ipv6Addr>() else {
        return Err(Error::probe("bad v6 source"));
    };

    let fd = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_RAW, libc::IPPROTO_ICMPV6) };
    if fd < 0 {
        return Err(Error::probe(format!(
            "icmpv6 socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let out = (|| -> Result<bool> {
        bind_device(fd, dev)?;
        set_recv_timeout(fd, timeout.max(1));
        let id = (std::process::id() & 0xffff) as u16;
        let mut pkt = Vec::with_capacity(8 + 16);
        pkt.push(ICMPV6_ECHO_REQUEST);
        pkt.push(0);
        pkt.push(0);
        pkt.push(0);
        pkt.extend_from_slice(&id.to_ne_bytes());
        pkt.extend_from_slice(&seq.to_ne_bytes());
        pkt.extend_from_slice(b"h5000m6");

        // Pseudo-header checksum (RFC 4443).
        let mut ph = Vec::with_capacity(40 + pkt.len());
        ph.extend_from_slice(&src6.octets());
        ph.extend_from_slice(&addr6.octets());
        ph.extend_from_slice(&((pkt.len() as u32).to_be_bytes()));
        ph.extend_from_slice(&[0, 0, 0, libc::IPPROTO_ICMPV6 as u8]);
        ph.extend_from_slice(&pkt);
        let sum = checksum(&ph);
        pkt[2] = (sum & 0xff) as u8;
        pkt[3] = (sum >> 8) as u8;

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
            return Ok(false);
        }
        let mut buf = [0u8; 2048];
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            return Ok(false);
        }
        let n = n as usize;
        // Skip the IPv6 header (40 bytes) to reach the ICMPv6 header.
        let icmp = &buf[40.min(n)..];
        if icmp.len() < 8 {
            return Ok(false);
        }
        Ok(icmp[0] == ICMPV6_ECHO_REPLY && u16::from_ne_bytes([icmp[4], icmp[5]]) == id)
    })();
    unsafe { libc::close(fd) };
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

/// Prolonged-wait helper used by tests: cap thread count is unnecessary since
/// rounds are bounded by the probe timeout.
const _: () = ();

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
}
