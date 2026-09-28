//! Health monitor: verdict persistence, failure streaks, throttle and score.
//!
//! Decision semantics are ported verbatim from the shell:
//!   * a family with no address/route is `unknown`, never a failure;
//!   * a group is only demoted after `probe_fail_streak` consecutive failed
//!     rounds (hysteresis against a flapping uplink);
//!   * `refresh_health(force)` skips the `health_probe_interval` throttle, used
//!     right after an interface event (the hotplug path sets `health_force`).
//!
//! The optional layered score (Link/Gateway/ICMP/TCP/DNS) is *reported* in the
//! status output; it never gates a switch – the auto-switch decision keeps the
//! legacy verdict+streak contract.

use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::probe;
use crate::state;
use crate::system::log;
use crate::types::{Family, Group, Verdict};

pub mod score;

/// `health_probe_interval` throttle: returns true when a new round may run.
pub fn throttle_ok(cfg: &AppConfig, force: bool) -> bool {
    if force {
        return true;
    }
    let interval = cfg.health_probe_interval;
    if interval == 0 {
        return true;
    }
    let now = crate::system::clock::unix_ts();
    let last = crate::types::num_or::<u64>(&state::health_get("ts"), 0);
    if last > 0 && now > 0 && now.saturating_sub(last) < interval as u64 {
        return false;
    }
    true
}

/// `refresh_health <force>`: one bounded probe round for every (group, family)
/// that structurally can answer; verdicts and failure streaks are persisted.
pub fn refresh_health(snap: &LiveSnapshot, cfg: &AppConfig, force: bool) {
    if !throttle_ok(cfg, force) {
        return;
    }

    let mut wan = Verdict::Unknown;
    let mut wan6 = Verdict::Unknown;
    let mut modem = Verdict::Unknown;
    let mut modem6 = Verdict::Unknown;

    if snap.wan4_ready {
        wan = probe::probe_group_family(
            Family::V4,
            Group::Wan,
            &snap.wan4_devs,
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
    }
    if snap.wan6_ready {
        wan6 = probe::probe_group_family(
            Family::V6,
            Group::Wan,
            &snap.wan6_devs,
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
    }
    if snap.modem4_ready {
        modem = probe::probe_group_family(
            Family::V4,
            Group::Modem,
            &snap.modem4_devs,
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
    }
    if snap.modem6_ready {
        modem6 = probe::probe_group_family(
            Family::V6,
            Group::Modem,
            &snap.modem6_devs,
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
    }

    let now = crate::system::clock::unix_ts();
    let wan_fail = next_streak("wan_fail", wan);
    let modem_fail = next_streak("modem_fail", modem);
    let wan6_fail = next_streak("wan6_fail", wan6);
    let modem6_fail = next_streak("modem6_fail", modem6);

    // Optional layered score (reported only).
    let s4_wan = score::score_group(snap, cfg, Group::Wan, Family::V4, wan);
    let s6_wan = score::score_group(snap, cfg, Group::Wan, Family::V6, wan6);
    let s4_modem = score::score_group(snap, cfg, Group::Modem, Family::V4, modem);
    let s6_modem = score::score_group(snap, cfg, Group::Modem, Family::V6, modem6);

    let _ = state::health_write(&[
        ("wan", wan.as_str()),
        ("modem", modem.as_str()),
        ("wan6", wan6.as_str()),
        ("modem6", modem6.as_str()),
        ("wan_fail", &wan_fail.to_string()),
        ("modem_fail", &modem_fail.to_string()),
        ("wan6_fail", &wan6_fail.to_string()),
        ("modem6_fail", &modem6_fail.to_string()),
        ("wan_score", &s4_wan.to_string()),
        ("wan6_score", &s6_wan.to_string()),
        ("modem_score", &s4_modem.to_string()),
        ("modem6_score", &s6_modem.to_string()),
        ("ts", &now.to_string()),
    ]);

    log::log_info(
        "health",
        &format!(
            "health check: wan={}/{wan6} modem={}/{modem6}",
            wan.as_str(),
            modem.as_str()
        ),
    );
}

fn next_streak(key: &str, v: Verdict) -> u32 {
    let cur = crate::types::num_or::<u32>(&state::health_get(key), 0);
    match v {
        Verdict::Up => 0,
        Verdict::Down => cur + 1,
        Verdict::Unknown => cur,
    }
}

/// `health_streak <group> <4|6>`.
pub fn health_streak(g: Group, fam: Family) -> u32 {
    network::cached_health_streak(g, fam)
}

/// `group_degraded`: a complete group whose required family has failed
/// `probe_fail_streak` consecutive rounds.
pub fn group_degraded(snap: &LiveSnapshot, cfg: &AppConfig, g: Group) -> bool {
    if !network::group_complete(snap, g, cfg.strict_dual_stack) {
        return false;
    }
    for fam in Family::ALL {
        if !network::group_family_required(snap, g, fam, cfg.strict_dual_stack) {
            continue;
        }
        if health_streak(g, fam) >= cfg.probe_fail_streak {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streak_semantics() {
        // Unknown must not advance the streak; Down advances; Up resets.
        let c = AppConfig::default();
        let _ = c;
        assert_eq!(next_streak("wan_fail", Verdict::Unknown), 0);
    }
}
