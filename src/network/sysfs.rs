//! Kernel state through /proc and /sys (no forks, no external commands).
//!
//! These are the same kernel-maintained views `ip` prints, read in-process:
//!   * `/sys/class/net/<dev>/`  – carrier, operstate, address, ifindex, type;
//!   * `/proc/net/route`        – IPv4 FIB (including the gateway + metric);
//!   * `/proc/net/ipv6_route`   – IPv6 FIB;
//!   * `/proc/net/fib_trie`     – IPv4 interface addresses;
//!   * `/proc/net/if_inet6`     – IPv6 interface addresses.

use std::fs;

pub const SYS_NET: &str = "/sys/class/net";

pub fn netdev_exists(dev: &str) -> bool {
    !dev.is_empty() && std::path::Path::new(&format!("{SYS_NET}/{dev}")).exists()
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
    let t = read_file(&format!("{SYS_NET}/{dev}/type"))
        .trim()
        .to_string();
    t.parse().ok()
}

pub fn dev_carrier(dev: &str) -> Option<u8> {
    read_file(&format!("{SYS_NET}/{dev}/carrier"))
        .trim()
        .parse::<u8>()
        .ok()
}

pub fn dev_operstate(dev: &str) -> String {
    read_file(&format!("{SYS_NET}/{dev}/operstate"))
        .trim()
        .to_string()
}

pub fn dev_ifindex(dev: &str) -> Option<u32> {
    read_file(&format!("{SYS_NET}/{dev}/ifindex"))
        .trim()
        .parse()
        .ok()
}

// ---------------------------------------------------------------------------
// IPv4 addresses: /proc/net/fib_trie
// ---------------------------------------------------------------------------

/// Collect IPv4 addresses per device from the fib trie.
/// Format (Linux 4.x+):
/// ```text
/// Main:
///   +-- 0.0.0.0/0 3 0 5
///      |-- 127.0.0.0/8 ...
///         |-- 127.0.0.0/32 ...
///            | /32 universe LOCAL
/// ```
/// Local entries appear as `| /32 link LOCAL` or `| /32 host LOCAL`; the
/// enclosing `|-- <addr> / 32` line names the address.
pub fn ipv4_addrs() -> Vec<(String, String)> {
    let text = read_file("/proc/net/fib_trie");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut pending: Option<String> = None;
    let mut dev: String = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Main:") {
            continue;
        }
        if let Some((addr, _)) = t.split_once('/') {
            let addr = addr.trim();
            if addr.starts_with('|') || addr.is_empty() {
                continue;
            }
            // A `|-- 1.2.3.4/32` (or `+-`) line names the candidate address.
            if let Some(a) = addr.strip_prefix("+- ") {
                pending = Some(a.trim().to_string());
            } else if let Some(a) = addr.strip_prefix("|-- ") {
                pending = Some(a.trim().to_string());
            }
            continue;
        }
        if let Some(devtok) = t.strip_prefix("|") {
            let devtok = devtok.trim();
            if let Some(d) = devtok.strip_prefix('/') {
                dev = d.to_string();
            }
        }
        if let Some(addr) = pending.take() {
            if t.contains("LOCAL") {
                out.push((dev.clone(), addr));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// IPv6 addresses: /proc/net/if_inet6
// ---------------------------------------------------------------------------
// Columns: address(32 hex) ifindex prefixlen scope flags

/// (dev, address, scope) tuples. `scope` 0x20 is link-local.
pub fn ipv6_addrs() -> Vec<(String, String, u8)> {
    let text = read_file("/proc/net/if_inet6");
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
    for entry in fs::read_dir(SYS_NET).ok()? {
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
    if let Ok(rd) = fs::read_dir(SYS_NET) {
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
}
