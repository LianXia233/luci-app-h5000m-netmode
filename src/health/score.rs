//! Optional layered health score (reported only; never gates a switch).
//!
//! Weight model (documented defaults):
//!   Link +20 · Gateway +20 · ICMP +30 · TCP +15 · DNS +15  (total 100)
//! A group that is structurally down scores 0 without burning probes.
//!
//! The shell backend has no score; this is an additive diagnostic layer that
//! the status output can expose (`wan_score` etc.). The auto-switch decision
//! still uses the legacy verdict + fail-streak contract, so the score can
//! never cause a behaviour regression.

use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::types::{Family, Group, Verdict};

const LINK_W: u32 = 20;
const GW_W: u32 = 20;
const ICMP_W: u32 = 30;
const TCP_W: u32 = 15;
const DNS_W: u32 = 15;

/// Compute the score for one (group, family).
pub fn score_group(
    snap: &LiveSnapshot,
    cfg: &AppConfig,
    g: Group,
    fam: Family,
    verdict: Verdict,
) -> u32 {
    let mut s: u32 = 0;
    let devs = snap.group_devs(g, fam);

    // Link layer: interface present + carrier up + address present.
    let has_dev = devs.iter().any(|d| crate::network::sysfs::netdev_exists(d));
    let has_carrier = devs
        .iter()
        .find_map(|d| crate::network::sysfs::dev_carrier(d))
        .unwrap_or(0)
        == 1;
    let has_addr = network::group_family_has_address(snap, g, fam);
    if has_dev && has_addr {
        s += LINK_W / 2;
        if has_carrier {
            s += LINK_W / 2;
        }
    }

    // Gateway layer: the group's own default-route gateway answers.
    if let Some(gw) = snap.group_gateway(g, fam) {
        if crate::probe::icmp::gateway_reachable(fam, devs, &gw, cfg.probe_timeout) {
            s += GW_W;
        }
    }

    // ICMP layer: the round verdict.
    match verdict {
        Verdict::Up => s += ICMP_W,
        Verdict::Down => {}
        Verdict::Unknown => s += ICMP_W / 3,
    }

    // TCP layer (opt-in).
    if cfg.tcp_check && !cfg.probe_targets.is_empty() {
        let targets: Vec<String> = cfg
            .probe_targets
            .iter()
            .map(|t| format!("{t}:443"))
            .collect();
        if crate::probe::tcp_score(devs, &targets, (cfg.probe_timeout as u64) * 1000) {
            s += TCP_W;
        }
    } else if cfg.tcp_check {
        // No targets configured: keep the slot neutral instead of zeroing.
        s += TCP_W / 2;
    }

    // DNS layer (opt-in, local resolver).
    if cfg.dns_check {
        if let Verdict::Up = crate::probe::dns_probe(&cfg.dns_probe_name) {
            s += DNS_W;
        }
    }

    s.min(100)
}

/// Map a score to a health state label (Healthy/Degraded/Suspect/Failed).
pub fn state_label(score: u32) -> &'static str {
    match score {
        90..=100 => "healthy",
        60..=89 => "degraded",
        30..=59 => "suspect",
        _ => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_clamped() {
        assert_eq!(score_clamp(120), 100);
        assert_eq!(score_clamp(0), 0);
    }

    fn score_clamp(v: u32) -> u32 {
        v.min(100)
    }

    #[test]
    fn labels() {
        assert_eq!(state_label(100), "healthy");
        assert_eq!(state_label(75), "degraded");
        assert_eq!(state_label(40), "suspect");
        assert_eq!(state_label(10), "failed");
    }
}
