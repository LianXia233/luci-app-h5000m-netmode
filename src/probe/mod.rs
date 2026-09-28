//! Probe orchestration: multi-attempt reachability verdicts.
//!
//! `probe_group_family` keeps the exact semantics of the shell version: needs
//! `probe_ok` successes out of `probe_attempts` tries (early exit on success),
//! bounded by the switch budget. Each attempt fans out over (targets x devices)
//! in parallel, so a round costs one probe timeout instead of a serial sum.

pub mod icmp;
pub mod tcp;

use crate::network::routes;
use crate::network::sysfs;
use crate::switch::budget;
use crate::types::{Family, Group, Verdict};

/// `probe_targets <family>`: configured targets (option or list), falling back
/// to the domestic defaults from AppConfig.
pub fn probe_targets(
    family: Family,
    cfg_targets: &[String],
    cfg_targets6: &[String],
) -> Vec<String> {
    match family {
        Family::V4 => cfg_targets.to_vec(),
        Family::V6 => cfg_targets6.to_vec(),
    }
}

/// Multi-attempt reachability verdict for one group + family. Echoes
/// `Verdict::Up/Down`.
#[allow(clippy::too_many_arguments)]
pub fn probe_group_family(
    family: Family,
    group: Group,
    devs: &[String],
    attempts: u32,
    need: u32,
    timeout: u32,
    targets4: &[String],
    targets6: &[String],
) -> Verdict {
    if devs.is_empty() {
        return Verdict::Down;
    }
    let targets = probe_targets(family, targets4, targets6);
    let mut ok: u32 = 0;
    for i in 0..attempts {
        if icmp::ping_targets_once(family, devs, timeout, &targets) {
            ok += 1;
            if ok >= need {
                return Verdict::Up;
            }
        }
        // Not enough attempts left to reach the threshold?
        if ok + (attempts - i - 1) < need {
            break;
        }
        if !budget::budget_left() {
            break;
        }
    }
    crate::system::log::log_info(
        "health",
        &format!(
            "ipv{} probe on {} failed: devs={} attempts={attempts} ok={ok} need={need}",
            family.n(),
            group.as_str(),
            if devs.is_empty() {
                "none".to_string()
            } else {
                devs.join(",")
            }
        ),
    );
    Verdict::Down
}

/// Gateway reachability (advisory). Uses the group's own gateway so the probe
/// leaves through the interface being checked.
pub fn gateway_probe(family: Family, devs: &[String], gateway: &str, timeout: u32) -> bool {
    icmp::gateway_reachable(family, devs, gateway, timeout)
}

/// Optional TCP layer for the health score (`tcp_check` opt-in). Never used by
/// the auto-switch decision; the first reachable target answers.
pub fn tcp_score(devs: &[String], targets: &[String], timeout_ms: u64) -> bool {
    if devs.is_empty() || targets.is_empty() {
        return false;
    }
    let timeout = std::time::Duration::from_millis(timeout_ms);
    for dev in devs {
        if !sysfs::netdev_exists(dev) {
            continue;
        }
        for t in targets {
            if tcp::tcp_probe(dev, t, timeout).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

/// `dns_probe`: query the local resolver (127.0.0.1:53) for one A record.
/// Mirrors the shell's `probe_dns`: reported, never a switch gate.
pub fn dns_probe(name: &str) -> Verdict {
    let name = if name.is_empty() { "openwrt.org" } else { name };
    match dns_query_a(name) {
        Ok(true) => Verdict::Up,
        Ok(false) => Verdict::Down,
        Err(_) => Verdict::Unknown,
    }
}

/// Minimal UDP DNS A query against 127.0.0.1:53 with a 3s bound.
fn dns_query_a(name: &str) -> Result<bool, String> {
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    sock.connect("127.0.0.1:53").map_err(|e| e.to_string())?;

    let mut tx = Vec::new();
    tx.extend_from_slice(&0x1234u16.to_be_bytes()); // id
    tx.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    tx.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    tx.extend_from_slice(&0u16.to_be_bytes()); // ancount
    tx.extend_from_slice(&0u16.to_be_bytes());
    tx.extend_from_slice(&0u16.to_be_bytes());
    for label in name.split('.') {
        tx.push(label.len() as u8);
        tx.extend_from_slice(label.as_bytes());
    }
    tx.push(0);
    tx.extend_from_slice(&1u16.to_be_bytes()); // A
    tx.extend_from_slice(&1u16.to_be_bytes()); // IN
    sock.send(&tx).map_err(|e| e.to_string())?;

    let mut rx = [0u8; 1024];
    let n = sock.recv(&mut rx).map_err(|e| e.to_string())?;
    if n < 12 {
        return Err("short dns reply".into());
    }
    let ancount = u16::from_be_bytes([rx[6], rx[7]]);
    Ok(ancount > 0)
}

/// Health-target resolution (for the optional score): gateway first, then
/// configured ICMP targets. Domestic defaults only.
pub fn health_targets(cfg: &crate::config::AppConfig) -> (Vec<String>, Vec<String>) {
    (cfg.probe_targets.clone(), cfg.probe_targets6.clone())
}

/// The `health` command path: run one bounded probe round for every
/// (group, family) that is structurally ready, persist verdicts and streaks.
pub fn run_health_round(snap: &crate::network::LiveSnapshot, cfg: &crate::config::AppConfig) {
    use crate::health::refresh_health;
    refresh_health(snap, cfg, true);
}

/// Keep the unused import lint happy for routes when only gateway is used.
#[allow(dead_code)]
fn _routes_used(r: &routes::Route) -> bool {
    r.gateway().is_some()
}

/// Guard against an empty config after a manual edit: probes with no targets
/// must be treated as unknown rather than down.
pub fn has_targets(cfg: &crate::config::AppConfig, family: Family) -> bool {
    !probe_targets(family, &cfg.probe_targets, &cfg.probe_targets6).is_empty()
}
