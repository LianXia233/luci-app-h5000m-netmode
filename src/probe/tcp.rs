//! TCP connect probe bound to the interface (opt-in layer).
//!
//! A raw socket is created, `SO_BINDTODEVICE`'d, set non-blocking and
//! `connect`'d with a hard timeout via `poll`. This is the Layer-4 check the
//! shell backend does not have; it is off by default (`tcp_check`), feeds the
//! optional health score, and never joins the auto-switch decision.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::types::{Error, Family, Result};

pub fn tcp_connect(dev: &str, addr: IpAddr, port: u16, timeout: Duration) -> Result<bool> {
    let family = match addr {
        IpAddr::V4(_) => Family::V4,
        IpAddr::V6(_) => Family::V6,
    };
    let af = match family {
        Family::V4 => libc::AF_INET,
        Family::V6 => libc::AF_INET6,
    };
    let fd = unsafe {
        libc::socket(
            af,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(Error::probe(format!(
            "tcp socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let out = (|| -> Result<bool> {
        let cdev = std::ffi::CString::new(dev).map_err(|_| Error::probe("dev NUL"))?;
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
        let rc = match (family, addr) {
            (Family::V4, IpAddr::V4(a)) => unsafe {
                let sa = libc::sockaddr_in {
                    sin_family: libc::AF_INET as u16,
                    sin_port: port.to_be(),
                    sin_addr: libc::in_addr {
                        s_addr: u32::from_ne_bytes(a.octets()).to_be(),
                    },
                    sin_zero: [0; 8],
                };
                libc::connect(
                    fd,
                    &sa as *const libc::sockaddr_in as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            },
            (Family::V6, IpAddr::V6(a)) => unsafe {
                let sa = libc::sockaddr_in6 {
                    sin6_family: libc::AF_INET6 as u16,
                    sin6_port: port.to_be(),
                    sin6_flowinfo: 0,
                    sin6_addr: libc::in6_addr {
                        s6_addr: a.octets(),
                    },
                    sin6_scope_id: 0,
                };
                libc::connect(
                    fd,
                    &sa as *const libc::sockaddr_in6 as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                )
            },
            _ => return Err(Error::probe("family/addr mismatch")),
        };
        let deadline = Instant::now() + timeout;
        loop {
            if rc == 0 {
                return Ok(true);
            }
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
            let prc = unsafe { libc::poll(&mut pollfd, 1, ms) };
            if prc < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Ok(false);
            }
            if prc == 0 {
                return Ok(false); // timeout
            }
            // Check SO_ERROR.
            let mut err: libc::c_int = 0;
            let mut elen = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    &mut err as *mut _ as *mut libc::c_void,
                    &mut elen,
                );
            }
            if err == 0 {
                return Ok(true);
            }
            // ECONNREFUSED means the host answered; EINPROGRESS/etc mean a
            // still-open path. Reachability verdict: any non-network-level
            // error counts as "the path works".
            if err == libc::ECONNREFUSED || err == libc::ECONNRESET {
                return Ok(true);
            }
            return Ok(false);
        }
    })();
    unsafe { libc::close(fd) };
    out
}

/// One TCP probe against a `host:port` pair (default: public domestic services).
pub fn tcp_probe(dev: &str, target: &str, timeout: Duration) -> Result<bool> {
    // target format: "ip" (use port 443) or "ip:port"
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(443)),
        None => (target, 443),
    };
    let ip: IpAddr = host
        .parse()
        .map_err(|_| Error::probe(format!("bad tcp target {host}")))?;
    let _ = SocketAddr::new(ip, port);
    tcp_connect(dev, ip, port, timeout)
}
