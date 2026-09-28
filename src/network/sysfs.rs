//! Kernel state through /proc and /sys (no forks, no external commands).
//!
//! These are the same kernel-maintained views `ip` prints, read in-process:
//!   * `/sys/class/net/<dev>/`  – carrier, operstate, address, ifindex, type;
//!   * `/proc/net/route`        – IPv4 FIB (including the gateway + metric);
//!   * `/proc/net/ipv6_route`   – IPv6 FIB;
//!   * `SIOCGIFCONF`            – IPv4 interface addresses;
//!   * `/proc/net/if_inet6`     – IPv6 interface addresses.

use std::fs;

pub const SYS_NET: &str = "/sys/class/net";
pub const PROC_NET: &str = "/proc/net";

/// The sysfs root. `H5000M_SYSFS_NET` redirects it so the deterministic test
/// suite can stand up a fake interface tree without root or namespaces.
pub fn sys_net() -> String {
    std::env::var("H5000M_SYSFS_NET").unwrap_or_else(|_| SYS_NET.to_string())
}

/// The /proc/net root (`H5000M_PROC_NET`), redirected for the same reason.
pub fn proc_net() -> String {
    std::env::var("H5000M_PROC_NET").unwrap_or_else(|_| PROC_NET.to_string())
}

pub fn netdev_exists(dev: &str) -> bool {
    !dev.is_empty() && std::path::Path::new(&format!("{}/{dev}", sys_net())).exists()
}

fn read_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// ARPHRD types treated as local tunnels rather than uplinks
/// (`is_tunnel_device`): ARPHRD_NONE=0xFFFE(65534), ARPHRD_TUNNEL=768,
/// ARPHRD_TUNNEL6=769, ARPHRD_SIT=776, ARPHRD_IP6GRE=778.
pub fn is_tunnel_type(t: u32) -> bool {
    matches!(t, 65534 | 768 | 769 | 776 | 778)
}

pub fn dev_type(dev: &str) -> Option<u32> {
    let t = read_file(&format!("{}/{dev}/type", sys_net()))
        .trim()
        .to_string();
    t.parse().ok()
}

pub fn dev_carrier(dev: &str) -> Option<u8> {
    read_file(&format!("{}/{dev}/carrier", sys_net()))
        .trim()
        .parse::<u8>()
        .ok()
}

pub fn dev_operstate(dev: &str) -> String {
    read_file(&format!("{}/{dev}/operstate", sys_net()))
        .trim()
        .to_string()
}

pub fn dev_ifindex(dev: &str) -> Option<u32> {
    read_file(&format!("{}/{dev}/ifindex", sys_net()))
        .trim()
        .parse()
        .ok()
}

// ---------------------------------------------------------------------------
// IPv4 addresses: SIOCGIFCONF
// ---------------------------------------------------------------------------

/// Collect IPv4 addresses per device via `SIOCGIFCONF`.
///
/// The previous implementation parsed `/proc/net/fib_trie`, but that file
/// names no devices at all - its `| /32 link LOCAL` lines carry only scope
/// labels. Every (dev, addr) pair it produced was garbage, `dev_has_family_
/// address` was always false and every IPv4 readiness gate (`wan4_ready`,
/// `modem4_ready`) was stuck at 0. SIOCGIFCONF is the kernel's actual
/// interface-address enumeration and is what `ip addr` itself wraps.
pub fn ipv4_addrs() -> Vec<(String, String)> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Vec::new();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    // 16 KiB is plenty for a SoC router; SIOCGIFCONF writes back the filled
    // length, so an undersized buffer degrades (never overflows).
    let mut buf = vec![0u8; 16 * 1024];
    let mut ifc: libc::ifconf = unsafe { std::mem::zeroed() };
    ifc.ifc_len = buf.len() as libc::c_int;
    ifc.ifc_ifcu.ifcu_buf = buf.as_mut_ptr() as *mut libc::c_char;
    let rc = unsafe { libc::ioctl(fd, libc::SIOCGIFCONF as _, &mut ifc) };
    if rc >= 0 && ifc.ifc_len > 0 {
        let filled = (ifc.ifc_len as usize).min(buf.len());
        out = parse_ifconf(&buf, filled, std::mem::size_of::<libc::ifreq>());
    }
    unsafe { libc::close(fd) };
    out
}

/// Byte-level decoder for the `struct ifreq` array `SIOCGIFCONF` returns.
/// Layout per entry: `char name[16]` (NUL-padded) followed by a `sockaddr`;
/// `AF_INET` is 2 and `sin_addr` sits at sockaddr offset 4. Kept libc-free so
/// the test suite can exercise it on any host.
fn parse_ifconf(buf: &[u8], filled: usize, req_size: usize) -> Vec<(String, String)> {
    const AF_INET: u16 = 2;
    let mut out: Vec<(String, String)> = Vec::new();
    if req_size < 24 {
        return out;
    }
    let mut off = 0usize;
    while off + req_size <= filled && off + 24 <= buf.len() {
        let name_bytes = &buf[off..off + 16];
        let namelen = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
        let name = String::from_utf8_lossy(&name_bytes[..namelen]).to_string();
        // sa_family_t is host-endian in the kernel ABI; on any target the
        // from_ne_bytes/to_ne_bytes pair round-trips it losslessly.
        let family = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]);
        if !name.is_empty() && family == AF_INET {
            let a = [buf[off + 20], buf[off + 21], buf[off + 22], buf[off + 23]];
            let ip = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
            if !out.iter().any(|(d, x)| d == &name && x == &ip) {
                out.push((name, ip));
            }
        }
        off += req_size;
    }
    out
}

// ---------------------------------------------------------------------------
// IPv6 addresses: /proc/net/if_inet6
// ---------------------------------------------------------------------------
// Columns: address(32 hex) ifindex prefixlen scope flags

/// (dev, address, scope) tuples. `scope` 0x20 is link-local.
pub fn ipv6_addrs() -> Vec<(String, String, u8)> {
    let text = read_file(&format!("{}/if_inet6", proc_net()));
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let ifindex: u32 = f[1].parse().unwrap_or(0);
        let scope: u8 = u8::from_str_radix(f[3], 16).unwrap_or(0);
        if let Some(dev) = dev_by_index(ifindex) {
            let addr = format_ipv6(f[0]);
            out.push((dev, addr, scope));
        }
    }
    out
}

fn dev_by_index(idx: u32) -> Option<String> {
    for entry in fs::read_dir(sys_net()).ok()? {
        let entry = entry.ok()?;
        if dev_ifindex(entry.file_name().to_str()?) == Some(idx) {
            return Some(entry.file_name().to_string_lossy().to_string());
        }
    }
    None
}

/// 32 hex chars -> canonical IPv6 string.
fn format_ipv6(hex: &str) -> String {
    let mut groups: Vec<u16> = Vec::new();
    for i in (0..32).step_by(4) {
        if let Some(chunk) = hex.get(i..i + 4) {
            groups.push(u16::from_str_radix(chunk, 16).unwrap_or(0));
        }
    }
    format_v6(&groups)
}

pub fn format_v6(groups: &[u16]) -> String {
    let mut s = String::new();
    let mut best_start = 0usize;
    let mut best_len = 0usize;
    let mut cur_start = 0usize;
    let mut cur_len = 0usize;
    let _n = groups.len();
    for (i, g) in groups.iter().enumerate() {
        if *g == 0 {
            if cur_len == 0 {
                cur_start = i;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_len = cur_len;
                best_start = cur_start;
            }
        } else {
            cur_len = 0;
        }
    }
    if best_len < 2 {
        // No compressible run.
        s = groups
            .iter()
            .map(|g| format!("{g:x}"))
            .collect::<Vec<_>>()
            .join(":");
        return s;
    }
    for (i, g) in groups.iter().enumerate() {
        if i == best_start {
            if s.is_empty() {
                s.push_str("::");
            } else if !s.ends_with(':') {
                s.push(':');
                s.push(':');
            }
            continue;
        }
        if i >= best_start && i < best_start + best_len {
            continue;
        }
        if !s.is_empty() && !s.ends_with(':') {
            s.push(':');
        }
        s.push_str(&format!("{g:x}"));
    }
    if s.is_empty() {
        s = "::".into();
    }
    s
}

/// Interface addresses grouped by (dev, family): for `group_family_has_address`.
/// IPv6 requires a non-link-local address (scope != link).
pub fn dev_has_family_address(dev: &str, family: u8) -> bool {
    match family {
        4 => ipv4_addrs().iter().any(|(d, _)| d == dev),
        6 => ipv6_addrs()
            .iter()
            .any(|(d, _, scope)| d == dev && *scope != 0x20),
        _ => false,
    }
}

/// All physical netdevs present, sorted (for `list-devices`).
pub fn list_netdevs() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Ok(rd) = fs::read_dir(sys_net()) {
        for e in rd.flatten() {
            v.push(e.file_name().to_string_lossy().to_string());
        }
    }
    v.sort();
    v
}

/// Warm standby route presence for a group+family uses route tables; helper
/// reused by the status output to expose device state.
pub struct DevInfo {
    pub carrier: String,
    pub operstate: String,
    pub present: bool,
}

pub fn dev_info(dev: &str) -> DevInfo {
    DevInfo {
        carrier: match dev_carrier(dev) {
            Some(0) => "0".into(),
            Some(1) => "1".into(),
            _ => "unknown".into(),
        },
        operstate: dev_operstate(dev),
        present: netdev_exists(dev),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v6_formatting() {
        let groups = [0x2001, 0xdb8, 0, 0, 0, 0, 0, 1];
        assert_eq!(format_v6(&groups), "2001:db8::1");
        let groups = [0, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(format_v6(&groups), "::1");
        let groups = [0xfe80, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(format_v6(&groups), "fe80::");
    }

    #[test]
    fn ifconf_parses_ipv4_entries() {
        // Three struct-ifreq slots (req_size 40): two AF_INET entries and one
        // non-inet entry that must be skipped.
        let mut buf = vec![0u8; 120];
        let mut put = |off: usize, name: &[u8], family: u16, ip: [u8; 4]| {
            buf[off..off + name.len()].copy_from_slice(name);
            buf[off + 16..off + 18].copy_from_slice(&family.to_ne_bytes());
            buf[off + 20..off + 24].copy_from_slice(&ip);
        };
        put(0, b"eth0", 2, [192, 168, 10, 1]);
        put(40, b"eth2", 2, [10, 7, 109, 108]);
        put(80, b"lo0", 0, [127, 0, 0, 1]);
        let got = parse_ifconf(&buf, 120, 40);
        assert_eq!(
            got,
            vec![
                ("eth0".to_string(), "192.168.10.1".to_string()),
                ("eth2".to_string(), "10.7.109.108".to_string())
            ]
        );
    }

    #[test]
    fn ifconf_tolerates_truncation_and_empty_names() {
        // A filled length that is not a multiple of req_size must not read
        // past the buffer, and an all-zero (empty) name is skipped.
        let buf = vec![0u8; 40];
        assert!(parse_ifconf(&buf, 39, 40).is_empty());
        assert!(parse_ifconf(&buf, 40, 40).is_empty());
    }
}
