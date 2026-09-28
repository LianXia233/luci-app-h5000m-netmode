//! Routing-table oracles from /proc (no `ip` forks).
//!
//! All the selection logic is ported verbatim from the shell backend:
//! deterministic lowest-metric winner, source-restricted IPv6 defaults handled
//! as netifd-owned, per-group default route, slot metric and ECMP counting.

use std::fs;

use crate::types::{Family, Group};

/// A parsed default route.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub family: Family,
    pub dev: String,
    pub via: Option<String>,
    pub metric: u32,
    /// IPv6 source-restricted prefix (`default from <pfx>`), empty for none.
    pub src: String,
    /// Kernel route protocol when known (from netlink; None from /proc).
    pub proto: Option<u32>,
    pub raw: String,
}

impl Route {
    pub fn dev(&self) -> &str {
        &self.dev
    }
    pub fn gateway(&self) -> Option<&str> {
        self.via.as_deref()
    }

    /// Render the `ip route`-style line (the `default4=`/`default6=` fields).
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("default");
        if !self.src.is_empty() {
            s.push_str(&format!(" from {}", self.src));
        }
        if let Some(gw) = &self.via {
            s.push_str(&format!(" via {gw}"));
        }
        s.push_str(&format!(" dev {}", self.dev));
        s.push_str(&format!(" metric {}", self.metric));
        if let Some(p) = self.proto {
            if p != 3 {
                s.push_str(&format!(" proto {p}"));
            }
        }
        s
    }
}

fn read_lines(path: &str) -> Vec<String> {
    fs::read_to_string(path)
        .map(|t| t.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// /proc/net/route  (IPv4, little-endian hex)
// ---------------------------------------------------------------------------

fn parse_ipv4_proc(lines: Vec<String>) -> Vec<Route> {
    let mut out = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 || f[0] == "Iface" {
            continue;
        }
        let dest = u32::from_str_radix(f[1], 16).unwrap_or(u32::MAX);
        let mask = u32::from_str_radix(f[7], 16).unwrap_or(u32::MAX);
        if dest != 0 || mask != 0 {
            continue; // only defaults
        }
        let gw = u32::from_str_radix(f[2], 16).unwrap_or(0);
        let metric: u32 = f[6].parse().unwrap_or(0);
        let via = if gw == 0 { None } else { Some(fmt_ipv4(gw)) };
        let dev = f[0].to_string();
        out.push(Route {
            family: Family::V4,
            dev,
            via,
            metric,
            src: String::new(),
            proto: None,
            raw: line,
        });
    }
    out
}

fn fmt_ipv4(v: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        v & 0xff,
        (v >> 8) & 0xff,
        (v >> 16) & 0xff,
        (v >> 24) & 0xff
    )
}

// ---------------------------------------------------------------------------
// /proc/net/ipv6_route
// ---------------------------------------------------------------------------
// Columns: dest(32hex) dest_prefix(2hex) src(32hex) src_prefix(2hex)
//          next_hop(32hex) metric(8hex) refcnt use flags(8hex) dev

fn parse_ipv6_proc(lines: Vec<String>) -> Vec<Route> {
    let mut out = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 {
            continue;
        }
        let dest = f[0];
        if dest != "00000000000000000000000000000000" {
            continue; // only defaults
        }
        let src = f[2];
        let metric = u32::from_str_radix(f[5], 16).unwrap_or(0);
        let hop = f[4];
        let dev = f[9].to_string();
        let via = if hop == "00000000000000000000000000000000" {
            None
        } else {
            Some(crate::network::sysfs::format_v6(&hex_groups(hop)))
        };
        let src_fmt = if src == "00000000000000000000000000000000" {
            String::new()
        } else {
            crate::network::sysfs::format_v6(&hex_groups(src))
        };
        out.push(Route {
            family: Family::V6,
            dev,
            via,
            metric,
            src: src_fmt,
            proto: None,
            raw: line,
        });
    }
    out
}

fn hex_groups(hex: &str) -> Vec<u16> {
    let mut groups = Vec::new();
    for i in (0..32).step_by(4) {
        if let Some(chunk) = hex.get(i..i + 4) {
            groups.push(u16::from_str_radix(chunk, 16).unwrap_or(0));
        }
    }
    groups
}

// ---------------------------------------------------------------------------
// oracles
// ---------------------------------------------------------------------------

/// `show_defaults <family>`: all usable default routes of one family.
/// /proc views never contain prohibit/blackhole/unreachable/throw entries and
/// carry no `linkdown` marker; structural checks compensate (see module doc).
pub fn show_defaults(family: Family) -> Vec<Route> {
    match family {
        Family::V4 => parse_ipv4_proc(read_lines("/proc/net/route")),
        Family::V6 => parse_ipv6_proc(read_lines("/proc/net/ipv6_route")),
    }
}

fn route_is_source_restricted(r: &Route) -> bool {
    !r.src.is_empty()
}

/// `winning_default`: the lowest-metric default route ordinary traffic uses;
/// a source-restricted route loses against an unrestricted one of the same
/// metric (it only applies to packets sourced inside that prefix).
pub fn winning_default(family: Family) -> Option<Route> {
    let mut best: Option<Route> = None;
    let mut best_src_restricted = false;
    for r in show_defaults(family) {
        let src_restricted = route_is_source_restricted(&r);
        let better = match &best {
            None => true,
            Some(b) => {
                if r.metric < b.metric {
                    true
                } else {
                    r.metric == b.metric && best_src_restricted && !src_restricted
                }
            }
        };
        if better {
            best_src_restricted = src_restricted;
            best = Some(r);
        }
    }
    best
}

pub fn winner_dev(family: Family) -> Option<String> {
    winning_default(family).map(|r| r.dev)
}

pub fn winner_metric(family: Family) -> Option<u32> {
    winning_default(family).map(|r| r.metric)
}

/// `equal_metric_count`: how many default routes compete with the given route
/// at the same metric and source-restriction class.
pub fn equal_metric_count(family: Family, metric: u32, reference: &Route) -> u32 {
    show_defaults(family)
        .iter()
        .filter(|r| {
            r.metric == metric
                && route_is_source_restricted(r) == route_is_source_restricted(reference)
        })
        .count() as u32
}

/// The default route of one exit group: the lowest-metric unrestricted default
/// whose device belongs to the group (a group's route can be parked in the
/// standby slot while a switch is in flight, so metric is not part of the
/// group selection).
pub fn group_default_route(family: Family, _group: Group, group_devs: &[String]) -> Option<Route> {
    if group_devs.is_empty() {
        return None;
    }
    let mut best: Option<Route> = None;
    let mut any: Option<Route> = None;
    let mut bestm = u32::MAX;
    let mut anym = u32::MAX;
    for r in show_defaults(family) {
        if !group_devs.iter().any(|d| d == &r.dev) {
            continue;
        }
        if any.is_none() || r.metric < anym {
            anym = r.metric;
            any = Some(r.clone());
        }
        // Operable routes first; a `default from <pfx>` line belongs to netifd
        // and cannot be moved by slot surgery.
        if route_is_source_restricted(&r) {
            continue;
        }
        if best.is_none() || r.metric < bestm {
            bestm = r.metric;
            best = Some(r);
        }
    }
    best.or(any)
}

pub fn slot_metric(family: Family, group: Group, group_devs: &[String]) -> Option<u32> {
    group_default_route(family, group, group_devs).map(|r| r.metric)
}

pub fn group_gateway(family: Family, group: Group, group_devs: &[String]) -> Option<String> {
    group_default_route(family, group, group_devs).and_then(|r| r.via)
}

/// `has_default_route_any`: does any default route sit on one of the devices?
pub fn has_default_route_any(family: Family, devs: &[String]) -> bool {
    show_defaults(family)
        .iter()
        .any(|r| devs.iter().any(|d| d == &r.dev))
}

/// Route line for the remembered-next-hop store ("gw dev").
pub fn route_gw_dev(r: &Route) -> (String, String) {
    (r.via.clone().unwrap_or_default(), r.dev.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4_line(dest: &str, gw: &str, metric: &str) -> String {
        format!("ethX\t{dest}\t{gw}\t0003\t0\t0\t{metric}\t00000000\t0\t0\t0")
    }

    #[test]
    fn parse_ipv4_defaults() {
        let lines = vec![
            ipv4_line("00000000", "0101130A", "10"),
            ipv4_line("0101130A", "00000000", "0"), // non-default ignored
            ipv4_line("00000000", "00000000", "50"),
        ];
        let routes = parse_ipv4_proc(lines);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].via.as_deref(), Some("10.19.1.1"));
        assert_eq!(routes[0].metric, 10);
        assert!(routes[1].via.is_none());
    }

    #[test]
    fn ipv6_default_parse() {
        let lines = vec![
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000001 00000000 00000000 ethX".to_string(),
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000000 ethX".to_string(),
        ];
        let routes = parse_ipv6_proc(lines);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].metric, 0);
        assert!(routes[0].via.is_none());
        assert_eq!(routes[1].metric, 1024);
        assert_eq!(routes[1].via.as_deref(), Some("fe80::1"));
    }
}
