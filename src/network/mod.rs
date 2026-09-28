//! Network state snapshot: groups, devices, routes and structural readiness.
//!
//! `LiveSnapshot` mirrors every read-only global the shell backend fills in
//! `read_live_state()`. It is built from /proc + /sys (+ one bounded `ubus`
//! query per section for the up/available/pending semantics the LuCI page
//! renders), never from `ip`/`grep`/`awk` forks.

pub mod netlink;
pub mod routes;
pub mod sysfs;

use std::collections::BTreeMap;

use crate::config::uci;
use crate::state;
use crate::system::command;
use crate::system::log;
use crate::types::{Family, Group, Verdict};

/// Per-interface-section state (from `read_section_state`).
#[derive(Debug, Clone, Default)]
pub struct SectionState {
    pub present: bool,
    pub available: bool,
    pub pending: bool,
    pub up: bool,
    pub carrier: String,
    pub devs: Vec<String>,
    pub source: String,
}

/// The full live snapshot.
#[derive(Debug, Clone, Default)]
pub struct LiveSnapshot {
    pub wan4_sec: String,
    pub wan6_sec: String,
    pub modem4_sec: String,
    pub modem6_sec: String,

    pub wan4_devs: Vec<String>,
    pub wan6_devs: Vec<String>,
    pub modem4_devs: Vec<String>,
    pub modem6_devs: Vec<String>,
    pub wan_devs: Vec<String>,
    pub modem_devs: Vec<String>,

    pub wan_device: String,
    pub modem_device: String,
    pub wan_device_source: String,
    pub modem_device_source: String,

    pub wan: SectionState,
    pub wan6: SectionState,
    pub modem: SectionState,
    pub modem6: SectionState,

    pub wan4_ready: bool,
    pub wan6_ready: bool,
    pub modem4_ready: bool,
    pub modem6_ready: bool,
    pub wan6_capable: bool,
    pub modem6_capable: bool,

    pub wan_route4: Option<routes::Route>,
    pub wan_route6: Option<routes::Route>,
    pub modem_route4: Option<routes::Route>,
    pub modem_route6: Option<routes::Route>,

    pub egress4: String,
    pub egress6: String,
    pub active4: String,
    pub active6: String,
    pub default4: Option<routes::Route>,
    pub default6: Option<routes::Route>,

    pub win4_dev: String,
    pub win6_dev: String,
    pub win4_metric: u32,
    pub win6_metric: u32,
    pub win4_owner: String,
    pub win6_owner: String,
    pub win4_ecmp: u32,
    pub win6_ecmp: u32,

    /// External (virtual) routing detected on the FIB winner: a TUN device or
    /// a policy-routing engine owning the default route. `1` does NOT mean the
    /// plugin stops managing the WAN/5G physical uplinks - the physical owner
    /// is reported separately and the reconciler keeps working underneath.
    pub external_route: u8,
    /// "tun" (TUN/TAP device on the FIB winner) or the detected policy engine
    /// (mwan3/daed/sing-box/openclash/clash/passwall/homeproxy), else
    /// "unknown" when the winner is simply unmapped. Empty when
    /// external_route=0.
    pub external_route_source: String,
    /// The physical uplink behind any virtual egress: the best default route
    /// on a non-tunnel device (IPv4 preferred, IPv6 fallback). This stays a
    /// "wan"/"modem" name even while a VPN/TUN owns the FIB, so the UI can
    /// show what the tunnel actually rides on.
    pub external_physical_owner: String,

    pub slot_owner: String,
}

impl LiveSnapshot {
    pub fn group_sec(&self, g: Group, fam: Family) -> &str {
        match (g, fam) {
            (Group::Wan, Family::V4) => &self.wan4_sec,
            (Group::Wan, Family::V6) => &self.wan6_sec,
            (Group::Modem, Family::V4) => &self.modem4_sec,
            (Group::Modem, Family::V6) => &self.modem6_sec,
        }
    }

    pub fn group_devs(&self, g: Group, fam: Family) -> &[String] {
        match (g, fam) {
            (Group::Wan, Family::V4) => &self.wan4_devs,
            (Group::Wan, Family::V6) => &self.wan6_devs,
            (Group::Modem, Family::V4) => &self.modem4_devs,
            (Group::Modem, Family::V6) => &self.modem6_devs,
        }
    }

    pub fn group_all_devs(&self, g: Group) -> &[String] {
        match g {
            Group::Wan => &self.wan_devs,
            Group::Modem => &self.modem_devs,
        }
    }

    pub fn group_state(&self, g: Group, fam: Family) -> &SectionState {
        match (g, fam) {
            (Group::Wan, Family::V4) => &self.wan,
            (Group::Wan, Family::V6) => &self.wan6,
            (Group::Modem, Family::V4) => &self.modem,
            (Group::Modem, Family::V6) => &self.modem6,
        }
    }

    pub fn group_route(&self, g: Group, fam: Family) -> Option<&routes::Route> {
        match (g, fam) {
            (Group::Wan, Family::V4) => self.wan_route4.as_ref(),
            (Group::Wan, Family::V6) => self.wan_route6.as_ref(),
            (Group::Modem, Family::V4) => self.modem_route4.as_ref(),
            (Group::Modem, Family::V6) => self.modem_route6.as_ref(),
        }
    }

    pub fn group_gateway(&self, g: Group, fam: Family) -> Option<String> {
        self.group_route(g, fam).and_then(|r| r.via.clone())
    }

    /// `slot_metric`: the group's default-route metric, or None (no route).
    pub fn slot_metric(&self, g: Group, fam: Family) -> Option<u32> {
        self.group_route(g, fam).map(|r| r.metric)
    }
}

// ---------------------------------------------------------------------------
// section classification (iface_role single source of truth)
// ---------------------------------------------------------------------------

/// `all_interface_sections` minus the fixed roles.
fn modem_candidates(eth_fallback: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for sec in uci::interface_sections() {
        match sec.as_str() {
            "wan" | "wan6" | "loopback" | "lan" => continue,
            _ => {}
        }
        if eth_fallback.iter().any(|f| f == &sec) {
            continue;
        }
        let dev = uci::section_device_raw(&sec);
        if dev == "@wan" || dev == "@wan6" {
            continue;
        }
        out.push(sec);
    }
    out
}

/// Discover modem sections: any interface section outside wan/wan6/lan/
/// loopback and the eth_fallback list, plus the IPv6 twin of the primary.
pub fn discover_modem(
    _groups: &mut BTreeMap<String, String>,
    eth_fallback: &[String],
) -> (String, String) {
    // groups: section -> "4"|"6" twin classification is per-section; the shell
    // appends the section whose device references @primary as the IPv6 twin.
    let cands = modem_candidates(eth_fallback);
    let primary = cands.first().cloned().unwrap_or_default();
    // Find the twin: a section whose raw device is @<primary>.
    let mut twin = String::new();
    if !primary.is_empty() {
        for sec in &cands {
            if sec == &primary {
                continue;
            }
            if uci::section_device_raw(sec) == format!("@{primary}") {
                twin = sec.clone();
                break;
            }
        }
    }
    (primary, twin)
}

/// `iface_role <section>`: wan|modem|other.
pub fn iface_role(section: &str) -> &'static str {
    if section.is_empty()
        || !section
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@' || c == '-')
    {
        return "other";
    }
    match section {
        "wan" | "wan6" => "wan",
        "loopback" | "lan" => "other",
        _ => {
            let settings = uci::settings_section();
            let eth_fallback: Vec<String> =
                settings.1.get("eth_fallback").cloned().unwrap_or_else(|| {
                    settings
                        .0
                        .get("eth_fallback")
                        .map(|s| s.split_whitespace().map(|w| w.to_string()).collect())
                        .unwrap_or_default()
                });
            let (primary, twin) = discover_modem(&mut BTreeMap::new(), &eth_fallback);
            if section == primary || section == twin {
                "modem"
            } else {
                "other"
            }
        }
    }
}

// ---------------------------------------------------------------------------
// snapshot builder
// ---------------------------------------------------------------------------

/// Resolve the real netdevs behind a section (symbolic `@ref` expansion).
fn resolve_section_devices(section: &str, depth: u8) -> Vec<String> {
    if section.is_empty() || depth >= 4 {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let raw = uci::section_device_raw(section);
    for token in raw.split_whitespace() {
        if let Some(ref_name) = token.strip_prefix('@') {
            for dev in resolve_section_devices(ref_name, depth + 1) {
                if !out.contains(&dev) {
                    out.push(dev);
                }
            }
        } else if sysfs::netdev_exists(token) && !out.iter().any(|d| d == token) {
            out.push(token.to_string());
        }
    }
    out
}

#[allow(clippy::field_reassign_with_default)]
fn read_section_state(section: &str) -> SectionState {
    let mut st = SectionState::default();
    st.present = !section.is_empty() && uci::uci_has_key(&format!("network.{section}"));
    st.source = section.to_string();

    // up/available/pending come from netifd through one bounded ubus query.
    // The query is memoised per process by the caller (snapshot lifetime).
    if st.present {
        if let Some(json) = command::ubus_section_status(section) {
            st.available = json_bool(&json, "available");
            st.pending = json_bool(&json, "pending");
            st.up = json_bool(&json, "up");
            let l3 = json_field(&json, "l3_device")
                .filter(|s| !s.is_empty())
                .or_else(|| json_field(&json, "device"));
            match l3 {
                Some(d) if !d.starts_with('@') && sysfs::netdev_exists(&d) => {
                    st.devs = vec![d];
                }
                _ => {}
            }
        }
    }

    // netifd may report a symbolic reference or nothing at all; expand the UCI
    // side in both cases.
    if st.devs.is_empty() {
        for dev in resolve_section_devices(section, 0) {
            if sysfs::netdev_exists(&dev) && !st.devs.contains(&dev) {
                st.devs.push(dev);
            }
        }
    }

    // Carrier is advisory only (eth0 carrier=0 with a working LAN, cellular
    // operstate=unknown): never the sole "link down" signal.
    st.carrier = st
        .devs
        .iter()
        .find_map(|d| sysfs::dev_carrier(d).map(|c| c.to_string()))
        .unwrap_or_else(|| "unknown".into());
    st
}

fn json_bool(json: &str, key: &str) -> bool {
    // Minimal JSON field scan: `"key":true` / `"key":false`.
    // ubus prints pretty JSON - `"up":\ttrue` - so whitespace after the
    // colon must be skipped, otherwise every boolean read as false and
    // the whole readiness/watchdog stack went blind on real netifd.
    let needle = format!("\"{key}\":");
    if let Some(pos) = json.find(&needle) {
        let rest = json[pos + needle.len()..].trim_start();
        if rest.starts_with("true") {
            return true;
        }
    }
    false
}

fn json_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let pos = json.find(&needle)?;
    let rest = &json[pos + needle.len()..];
    let rest = rest.trim_start();
    if let Some(v) = rest.strip_prefix('"') {
        let end = v.find('"')?;
        return Some(v[..end].to_string());
    }
    None
}

/// Build the live snapshot. Callers must `invalidate` before a fresh view.
pub fn read_live_state() -> LiveSnapshot {
    let mut snap = LiveSnapshot::default();
    let settings = uci::settings_section();
    let eth_fallback: Vec<String> = settings.1.get("eth_fallback").cloned().unwrap_or_else(|| {
        settings
            .0
            .get("eth_fallback")
            .map(|s| s.split_whitespace().map(|w| w.to_string()).collect())
            .unwrap_or_default()
    });

    let (modem4_sec, modem6_sec) = discover_modem(&mut BTreeMap::new(), &eth_fallback);
    snap.wan4_sec = if uci::uci_has_key("network.wan") {
        "wan".into()
    } else {
        String::new()
    };
    snap.wan6_sec = if uci::uci_has_key("network.wan6") {
        "wan6".into()
    } else {
        String::new()
    };
    snap.modem4_sec = modem4_sec;
    snap.modem6_sec = modem6_sec;

    snap.wan4_devs = resolve_section_devices(&snap.wan4_sec, 0);
    snap.wan6_devs = resolve_section_devices(&snap.wan6_sec, 0);
    snap.modem4_devs = resolve_section_devices(&snap.modem4_sec, 0);
    snap.modem6_devs = resolve_section_devices(&snap.modem6_sec, 0);

    let wan_manual = settings.0.get("wan_device").cloned().unwrap_or_default();
    let modem_manual = settings.0.get("modem_device").cloned().unwrap_or_default();
    if !wan_manual.is_empty() {
        snap.wan4_devs = wan_manual
            .split_whitespace()
            .map(|w| w.to_string())
            .collect();
        snap.wan6_devs = snap.wan4_devs.clone();
        snap.wan_device_source = "manual".into();
    } else {
        snap.wan_device_source = "auto".into();
    }
    if !modem_manual.is_empty() {
        snap.modem4_devs = modem_manual
            .split_whitespace()
            .map(|w| w.to_string())
            .collect();
        snap.modem6_devs = snap.modem4_devs.clone();
        snap.modem_device_source = "manual".into();
    } else {
        snap.modem_device_source = "auto".into();
    }

    snap.wan_devs = dedup(&snap.wan4_devs, &snap.wan6_devs);
    snap.modem_devs = dedup(&snap.modem4_devs, &snap.modem6_devs);
    snap.wan_device = snap
        .wan4_devs
        .first()
        .cloned()
        .unwrap_or_else(|| uci::section_device_raw(&snap.wan4_sec));
    snap.modem_device = snap
        .modem4_devs
        .first()
        .cloned()
        .unwrap_or_else(|| uci::section_device_raw(&snap.modem4_sec));

    snap.wan = read_section_state(&snap.wan4_sec);
    snap.wan6 = read_section_state(&snap.wan6_sec);
    let missing = SectionState {
        present: false,
        ..Default::default()
    };
    if snap.modem4_sec.is_empty() {
        snap.modem = missing.clone();
    } else {
        snap.modem = read_section_state(&snap.modem4_sec);
    }
    if snap.modem6_sec.is_empty() {
        snap.modem6 = missing;
    } else {
        snap.modem6 = read_section_state(&snap.modem6_sec);
    }

    // A device claimed by both roles makes every verdict ambiguous: the wired
    // side wins, and the conflict is logged loudly.
    if let Some(shared) = snap
        .modem_devs
        .iter()
        .find(|d| snap.wan_devs.iter().any(|w| w == *d))
    {
        let shared = shared.clone();
        log::log_warn(
            "network",
            &format!("device {shared} is mapped to both exits; treating it as wired WAN"),
        );
        snap.wan_devs.push(shared.clone());
        snap.modem_devs.retain(|d| d != &shared);
        snap.modem4_devs.retain(|d| d != &shared);
        snap.modem6_devs.retain(|d| d != &shared);
    }
    snap.wan_devs = dedup(&snap.wan_devs, &[]);
    snap.wan4_devs = if snap.wan4_devs.is_empty() {
        snap.wan_devs.clone()
    } else {
        snap.wan4_devs.clone()
    };
    snap.wan6_devs = if snap.wan6_devs.is_empty() {
        snap.wan_devs.clone()
    } else {
        snap.wan6_devs.clone()
    };
    snap.modem_devs = dedup(&snap.modem_devs, &[]);

    snap.wan_route4 = routes::group_default_route(Family::V4, Group::Wan, &snap.wan4_devs);
    snap.wan_route6 = routes::group_default_route(Family::V6, Group::Wan, &snap.wan6_devs);
    snap.modem_route4 = routes::group_default_route(Family::V4, Group::Modem, &snap.modem4_devs);
    snap.modem_route6 = routes::group_default_route(Family::V6, Group::Modem, &snap.modem6_devs);

    snap.wan4_ready = group_family_ready(&snap, Group::Wan, Family::V4);
    snap.wan6_ready = group_family_ready(&snap, Group::Wan, Family::V6);
    snap.modem4_ready = group_family_ready(&snap, Group::Modem, Family::V4);
    snap.modem6_ready = group_family_ready(&snap, Group::Modem, Family::V6);
    snap.wan6_capable = group_family_capable(&snap, Group::Wan, Family::V6);
    snap.modem6_capable = group_family_capable(&snap, Group::Modem, Family::V6);

    // The live exit: the FIB decides the used next hop. `ip route get` also
    // honours policy rules; on this system (plain main-table routing) the
    // lowest-metric default route is the kernel's choice, so we read it
    // directly and never transmit a probe packet.
    snap.default4 = routes::winning_default(Family::V4);
    snap.default6 = routes::winning_default(Family::V6);
    snap.egress4 = snap
        .default4
        .as_ref()
        .map(|r| r.dev.clone())
        .unwrap_or_default();
    snap.egress6 = snap
        .default6
        .as_ref()
        .map(|r| r.dev.clone())
        .unwrap_or_default();
    snap.active4 = route_owner(&snap, &snap.egress4, "");
    snap.active6 = route_owner(&snap, &snap.egress6, &snap.active4);

    snap.win4_dev = snap
        .default4
        .as_ref()
        .map(|r| r.dev.clone())
        .unwrap_or_default();
    snap.win6_dev = snap
        .default6
        .as_ref()
        .map(|r| r.dev.clone())
        .unwrap_or_default();
    snap.win4_metric = snap.default4.as_ref().map(|r| r.metric).unwrap_or(0);
    snap.win6_metric = snap.default6.as_ref().map(|r| r.metric).unwrap_or(0);
    snap.win4_owner = match &snap.default4 {
        Some(r) => route_owner(&snap, &r.dev, &snap.active4),
        None => "none".into(),
    };
    snap.win6_owner = match &snap.default6 {
        Some(r) => route_owner(&snap, &r.dev, &snap.active6),
        None => "none".into(),
    };
    snap.win4_ecmp = match &snap.default4 {
        Some(r) => routes::equal_metric_count(Family::V4, r.metric, r),
        None => 0,
    };
    snap.win6_ecmp = match &snap.default6 {
        Some(r) => routes::equal_metric_count(Family::V6, r.metric, r),
        None => 0,
    };
    if snap.win4_dev.is_empty() {
        snap.win4_ecmp = 0;
    }
    if snap.win6_dev.is_empty() {
        snap.win6_ecmp = 0;
    }

    // External (virtual) routing detection. A "other" FIB winner used to make
    // the whole page report "external takeover" with no detail; now the
    // physical carrier behind the virtual egress is resolved and the
    // reconciler keeps managing the physical uplinks underneath.
    let ext4 = !snap.win4_dev.is_empty() && snap.win4_owner == "other";
    let ext6 = !snap.win6_dev.is_empty() && snap.win6_owner == "other";
    snap.external_route = if ext4 || ext6 { 1 } else { 0 };
    if ext4 || ext6 {
        let dev = if ext4 {
            snap.win4_dev.clone()
        } else {
            snap.win6_dev.clone()
        };
        let tun = sysfs::dev_type(&dev)
            .map(sysfs::is_tunnel_type)
            .unwrap_or(false);
        if tun {
            snap.external_route_source = "tun".into();
        } else {
            let engine = detect_policy_engine();
            snap.external_route_source = if engine.is_empty() {
                "unknown".into()
            } else {
                engine
            };
        }
    }
    snap.external_physical_owner = external_physical_owner(&mut snap);

    snap.slot_owner = slot_owner(&snap);
    snap
}

/// Policy-routing engines detectable without forking: a bounded /proc scan.
/// `mwan3` has no resident daemon (hotplug driven), so a miss there degrades
/// to "unknown" rather than a false "no external routing".
fn detect_policy_engine() -> String {
    const ENGINES: [&str; 7] = [
        "mwan3",
        "daed",
        "sing-box",
        "openclash",
        "clash",
        "passwall",
        "homeproxy",
    ];
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return String::new();
    };
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{name}/cmdline")) else {
            continue;
        };
        let cmd = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        if cmd.contains("h5000m-netmode") {
            continue; // never match ourselves
        }
        for eng in ENGINES {
            if cmd.contains(eng) {
                return eng.to_string();
            }
        }
    }
    String::new()
}

/// The physical uplink behind whatever owns the FIB: the best default route
/// whose device is not a tunnel (IPv4 preferred, IPv6 fallback).
fn external_physical_owner(snap: &mut LiveSnapshot) -> String {
    for fam in [Family::V4, Family::V6] {
        if let Some(r) = routes::winning_default(fam) {
            let tun = sysfs::dev_type(&r.dev)
                .map(sysfs::is_tunnel_type)
                .unwrap_or(false);
            if !tun {
                return route_owner(snap, &r.dev, "");
            }
        }
    }
    "none".into()
}

fn dedup(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in a.iter().chain(b.iter()) {
        if !out.iter().any(|x| x == d) {
            out.push(d.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// predicates (all pure reads of the snapshot)
// ---------------------------------------------------------------------------

pub fn group_family_capable(snap: &LiveSnapshot, g: Group, fam: Family) -> bool {
    let sec = snap.group_sec(g, fam);
    if sec.is_empty() {
        return false;
    }
    if !uci::uci_has_key(&format!("network.{sec}")) {
        return false;
    }
    true // a symbolic ref to a dead section is still capable once up
}

pub fn group_family_has_address(snap: &LiveSnapshot, g: Group, fam: Family) -> bool {
    for dev in snap.group_devs(g, fam) {
        if sysfs::dev_has_family_address(dev, fam.n()) {
            return true;
        }
    }
    false
}

/// `group_family_ready`: device exists, holds an address, group has a default
/// route of that family.
pub fn group_family_ready(snap: &LiveSnapshot, g: Group, fam: Family) -> bool {
    if !group_family_capable(snap, g, fam) {
        return false;
    }
    if snap.group_devs(g, fam).is_empty() {
        return false;
    }
    if snap.group_route(g, fam).is_none() {
        return false;
    }
    group_family_has_address(snap, g, fam)
}

/// Whether `fam` participates in the group predicates at all.
///
/// Participation is a purely structural property: a family for which the group
/// has no interface section can never become ready, so demanding it would only
/// reject otherwise-valid single-stack exits (the "ipv6_not_configured"
/// rejection). `strict_dual_stack` therefore no longer forces absent families
/// into the requirement set; it only governs how a *capable* family's
/// wait/probe/commit failures are treated by the switch state machine
/// (`strict=1` fails the switch, `strict=0` continues single-stack).
pub fn group_family_required(
    snap: &LiveSnapshot,
    g: Group,
    fam: Family,
    _strict_dual_stack: bool,
) -> bool {
    group_family_capable(snap, g, fam)
}

pub fn group_complete(snap: &LiveSnapshot, g: Group, strict_dual_stack: bool) -> bool {
    for fam in Family::ALL {
        if !group_family_required(snap, g, fam, strict_dual_stack) {
            continue;
        }
        if !group_family_ready(snap, g, fam) {
            return false;
        }
    }
    true
}

/// `group_complete_online`: structural + cached reachability verdicts.
pub fn group_complete_online(snap: &LiveSnapshot, g: Group, strict_dual_stack: bool) -> bool {
    if !group_complete(snap, g, strict_dual_stack) {
        return false;
    }
    for fam in Family::ALL {
        if !group_family_required(snap, g, fam, strict_dual_stack) {
            continue;
        }
        let verdict = cached_verdict(g, fam);
        if !verdict.is_up() {
            return false;
        }
    }
    true
}

/// Families this group cannot carry at all (for the opt-out hole parking).
pub fn group_hole_families(snap: &LiveSnapshot, g: Group, strict_dual_stack: bool) -> Vec<Family> {
    Family::ALL
        .into_iter()
        .filter(|fam| !group_family_required(snap, g, *fam, strict_dual_stack))
        .collect()
}

// ---------------------------------------------------------------------------
// ownership
// ---------------------------------------------------------------------------

fn route_owner(snap: &LiveSnapshot, dev: &str, fallback: &str) -> String {
    if dev.is_empty() {
        return "none".into();
    }
    if snap.wan_devs.iter().any(|d| d == dev) {
        "wan".into()
    } else if snap.modem_devs.iter().any(|d| d == dev) {
        "modem".into()
    } else if sysfs::dev_type(dev)
        .map(sysfs::is_tunnel_type)
        .unwrap_or(false)
    {
        match fallback {
            "wan" | "modem" => fallback.to_string(),
            _ => "other".into(),
        }
    } else {
        "other".into()
    }
}

/// `slot_owner`: both families must agree; when they do not, IPv4 wins and the
/// reconciler repairs the divergence.
pub fn slot_owner(snap: &LiveSnapshot) -> String {
    let o4 = slot_owner_family(snap, Family::V4);
    if matches!(o4.as_str(), "wan" | "modem") {
        return o4;
    }
    let o6 = slot_owner_family(snap, Family::V6);
    if matches!(o6.as_str(), "wan" | "modem") {
        return o6;
    }
    "none".into()
}

pub fn slot_owner_family(snap: &LiveSnapshot, fam: Family) -> String {
    let dev = routes::winner_dev(fam).unwrap_or_default();
    if dev.is_empty() {
        return String::new();
    }
    let owner = route_owner(snap, &dev, "");
    match owner.as_str() {
        "other" | "none" => route_owner(snap, &snap.egress4, ""),
        _ => owner,
    }
}

// ---------------------------------------------------------------------------
// health-cache reads (the snapshot does not probe; it reuses cached verdicts)
// ---------------------------------------------------------------------------

pub fn cached_verdict(g: Group, fam: Family) -> Verdict {
    let key = format!("{}{}", g.as_str(), fam.suffix());
    Verdict::parse(&state::health_get(&key))
}

pub fn cached_health_streak(g: Group, fam: Family) -> u32 {
    let key = format!("{}{}_fail", g.as_str(), fam.suffix());
    crate::types::num_or(&state::health_get(&key), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_bool_parses() {
        assert!(json_bool("{\"up\":true,\"available\":true}", "up"));
        assert!(!json_bool("{\"up\":false}", "up"));
        assert!(!json_bool("{\"up\":true}", "missing"));
        // Regression: ubus prints pretty JSON with whitespace after the
        // colon; every boolean used to read as false there.
        assert!(json_bool(
            "{\n\t\"up\": true,\n\t\"available\":\ttrue\n}",
            "up"
        ));
        assert!(json_bool(
            "{\n\t\"up\": false,\n\t\"available\": true\n}",
            "available"
        ));
        assert!(!json_bool("{\n\t\"up\": false,\n}", "up"));
        assert!(!json_bool("{\n\t\"up\": true,\n}", "available"));
    }

    #[test]
    fn json_field_parses() {
        assert_eq!(
            json_field("{\"l3_device\":\"eth1\"}", "l3_device"),
            Some("eth1".into())
        );
        assert_eq!(json_field("{}", "l3_device"), None);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn owner_classification() {
        let mut s = LiveSnapshot::default();
        s.wan_devs = vec!["eth1".into()];
        s.modem_devs = vec!["eth2".into()];
        assert_eq!(route_owner(&s, "eth1", ""), "wan");
        assert_eq!(route_owner(&s, "eth2", ""), "modem");
        assert_eq!(route_owner(&s, "lo", ""), "other");
        assert_eq!(route_owner(&s, "", ""), "none");
    }
}
